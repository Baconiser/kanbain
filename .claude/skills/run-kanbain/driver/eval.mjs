// Retrieval evaluation (Phase 3e): about 40 query → expected-hit pairs over
// this repo's code, the seeded tickets and benchmark run summaries. Reports
// recall@10 and MRR with and without the reranker, and the search latency.
// Projects are found by name prefix: "KanbAIn code" (search.mjs/code-search.mjs
// clone of this repo), "Search " (30 seeded tickets) and "Bench " (runs).
// Usage: node eval.mjs <backendPort> <token> [variant=both|plain|rerank] [k=10]
const [port, token, variantArg = 'both', kArg = '10'] = process.argv.slice(2);
const K = Number(kArg);
const apiBase = `http://127.0.0.1:${port}/api`;
async function api(method, path, body) {
  const res = await fetch(apiBase + path, {
    method,
    headers: { 'content-type': 'application/json', 'x-kanbain-token': token },
    body: body ? JSON.stringify(body) : undefined,
  });
  const json = await res.json().catch(() => ({}));
  if (!res.ok || json.success === false) throw new Error(`${method} ${path} -> ${res.status}`);
  return json.data;
}

const CODE = [
  ['where are ticket numbers assigned', 'crates/db/src/models/issue.rs', 'fn create_with_version'],
  ['detect an external id like PROJ-123 in ticket text', 'crates/utils/src/ticket_id.rs', 'detect_external_id'],
  ['branch name for a ticket kanbain/ID-slug', 'crates/utils/src/ticket_id.rs', 'ticket_branch_name'],
  ['extract text from a pdf attachment', 'crates/services/src/services/text_extract.rs'],
  ['read paragraphs from a word docx file', 'crates/services/src/services/text_extract.rs', 'extract_docx'],
  ['find commits whose message mentions a ticket id', 'crates/git/src/history.rs', 'find_ticket_commits'],
  ['split source code into chunks with tree-sitter tags queries', 'crates/embeddings/src/chunk/code.rs'],
  ['reciprocal rank fusion of ranked lists', 'crates/embeddings/src/service.rs', 'rrf'],
  ['register sqlite-vec as an auto extension', 'crates/embeddings/src/store.rs', 'register_sqlite_vec'],
  ['allowlist of hosts the sandbox egress proxy lets through', 'crates/sandbox/src/egress.rs'],
  ['stop a streamed model response when the run budget is exhausted', 'crates/model-proxy/src/lib.rs'],
  ['load the editable model price table', 'crates/model-proxy/src/prices.rs'],
  ['reject api requests without the launch token', 'crates/server/src/middleware/launch_token.rs'],
  ['daily sqlite backup and pruning old backups', 'crates/db/src/backup.rs'],
  ['create a docker container with cpu and memory limits', 'crates/sandbox/src/docker.rs'],
  ['move a git branch into the container with a bundle', 'crates/sandbox/src/transfer.rs'],
  ['claude code arguments for tools and appended system prompt', 'crates/executors/src/adapters/claude.rs'],
  ['opencode permission map that denies tools', 'crates/executors/src/adapters/opencode.rs', 'permissions'],
  ['parse the result line of claude -p json output', 'crates/executors/src/adapters/headless.rs'],
  ['task prompt for the intake agent', 'crates/local-deployment/src/intake.rs', 'intake_task'],
  ['ask the history agent to summarize prior commits', 'crates/local-deployment/src/history.rs'],
  ['sql triggers that write timeline events when status changes', 'crates/db/migrations/20261030000000_ticket_intake_timeline.sql'],
  ['merge description edits made within ten minutes', 'crates/db/src/models/issue_description.rs', 'record_edit'],
  ['import skills and agents from a zip archive', 'crates/services/src/services/agent_library.rs'],
  ['dialog to paste a ticket and drop files for intake', 'packages/web-core/src/pages/tickets/DropTicketDialog.tsx'],
  ['human readable sentence for each timeline event', 'packages/web-core/src/pages/tickets/TicketTimeline.tsx', 'describeEvent'],
  ['test connection button for the embedder settings', 'packages/web-core/src/shared/dialogs/settings/settings/SearchSettingsSection.tsx'],
  ['json patch live stream of the ticket board', 'crates/services/src/services/events/tickets.rs'],
  ['is an url on a private network or loopback', 'crates/embeddings/src/config.rs', 'is_remote_url'],
  ['markdown chunking with overlap between chunks', 'crates/embeddings/src/chunk/text.rs'],
  ['run claude setup-token to connect a claude account', 'crates/local-deployment/src/claude_login.rs'],
];
const TICKETS = [
  ['users cannot reset their password', 'Password reset email never arrives'],
  ['discount code is not applied at checkout', 'Checkout total ignores coupons'],
  ['people get logged out after a few minutes', 'Session expires too quickly'],
  ['special characters broken after spreadsheet upload', 'CSV import drops umlauts'],
  ['customers want to pay with two vouchers', 'Gift cards cannot be combined'],
  ['sign in with a google account', 'Login with Google'],
  ['the same last item is sold twice at once', 'Inventory count goes negative'],
  ['wrong delivery fee for orders to austria', 'Shipping cost wrong for Austria'],
  ['PROJ-123', 'Checkout total ignores coupons'],
];
const RUNS = [['earlier agent run that made ticket numbers start at 100', 'run']];

const projects = await api('GET', '/projects');
const byPrefix = (p) => projects.filter((x) => x.name.startsWith(p)).sort((a, b) => b.created_at.localeCompare(a.created_at))[0];
const code = byPrefix('KanbAIn code');
const tickets = byPrefix('Search ');
const bench = byPrefix('Bench ');
const cases = [];
if (code) for (const [q, path, symbol] of CODE) cases.push({ group: 'code', q, project: code.id, kinds: ['code'], match: (h) => h.path === path && (!symbol || (h.symbol ?? '').includes(symbol)) });
if (tickets) for (const [q, title] of TICKETS) cases.push({ group: 'tickets', q, project: tickets.id, kinds: ['ticket', 'comment', 'attachment', 'prior_work', 'run'], match: (h, r) => r.tickets[h.issue_id]?.title === title });
if (bench) for (const [q, kind] of RUNS) cases.push({ group: 'runs', q, project: bench.id, kinds: ['run'], match: (h) => h.kind === kind });
console.log(`${cases.length} queries (code ${code ? CODE.length : 0}, tickets ${tickets ? TICKETS.length : 0}, runs ${bench ? RUNS.length : 0})`);

async function evaluate(rerank) {
  let found = 0, rr = 0, ms = 0, reranked = 0;
  const misses = [];
  // Per group too: the overall number once hid a reranker that helped
  // nothing on tickets and scrambled the top of the code results.
  const groups = {};
  for (const c of cases) {
    const started = Date.now();
    const r = await api('POST', '/search', { query: c.q, project_id: c.project, kinds: c.kinds, limit: K, rerank });
    ms += Date.now() - started;
    if (r.reranked) reranked++;
    const i = r.hits.findIndex((h) => c.match(h, r));
    const g = (groups[c.group] ??= { n: 0, found: 0, rr: 0 });
    g.n++;
    if (i >= 0) {
      g.found++;
      g.rr += 1 / (i + 1);
      found++;
      rr += 1 / (i + 1);
    } else {
      misses.push(c.q);
    }
  }
  return { recall: found / cases.length, mrr: rr / cases.length, ms: ms / cases.length, reranked, misses, groups };
}

const variants = variantArg === 'both' ? [false, true] : [variantArg === 'rerank'];
for (const rerank of variants) {
  const r = await evaluate(rerank);
  console.log(`${rerank ? 'rerank' : 'rrf   '}  recall@${K} ${r.recall.toFixed(3)}  MRR ${r.mrr.toFixed(3)}  mean ${Math.round(r.ms)} ms  reranked ${r.reranked}/${cases.length}`);
  for (const [name, g] of Object.entries(r.groups)) {
    console.log(`  ${name.padEnd(8)} recall@${K} ${(g.found / g.n).toFixed(3)}  MRR ${(g.rr / g.n).toFixed(3)}  (${g.n})`);
  }
  if (r.misses.length) console.log(`  missed: ${r.misses.join(' | ')}`);
}
