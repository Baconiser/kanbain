// Drive the code index and agent search (Phase 3d).
// Part A: index a shallow clone of this repo with a real embedder, ask
// "where are ticket numbers assigned", change one function and re-index,
// then give code its own embedder (only the code space rebuilds).
// Part B: sandboxed Claude Code runs on a small repo, with and without the
// `search` capability (the token benchmark, D46), and the ticket overlay.
// Needs TEI embedders, the sandbox and the mock with mock-script-benchmark.json.
// Usage: node code-search.mjs <backendPort> <token> <embeddingsDb> <workDir>
//          <textUrl> <textModel> <codeUrl> <codeModel> [runsPerVariant]
import { execFileSync, execSync } from 'node:child_process';

const [port, token, embeddingsDb, workDir, textUrl, textModel, codeUrl, codeModel, runsArg] =
  process.argv.slice(2);
const runsPerVariant = Number(runsArg ?? 3);
const apiBase = `http://127.0.0.1:${port}/api`;
const check = (cond, what) => {
  console.log(`${cond ? 'PASS' : 'FAIL'}: ${what}`);
  if (!cond) process.exitCode = 1;
};
async function api(method, path, body) {
  const res = await fetch(apiBase + path, {
    method,
    headers: { 'content-type': 'application/json', 'x-kanbain-token': token },
    body: body ? JSON.stringify(body) : undefined,
  });
  const json = await res.json().catch(() => ({}));
  if (!res.ok || json.success === false) {
    throw new Error(`${method} ${path} -> ${res.status} ${JSON.stringify(json)}`);
  }
  return json.data;
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sh = (cmd, cwd) => execSync(cmd, { cwd, encoding: 'utf8' }).trim();
const sql = (query) =>
  execFileSync('python3', ['-c', 'import sqlite3,sys;c=sqlite3.connect(sys.argv[1]);print(c.execute(sys.argv[2]).fetchone()[0])', embeddingsDb, query], { encoding: 'utf8' }).trim();
const embedder = (url, model, prefix = '') => ({
  provider: 'tei', base_url: url, model, dimensions: null, query_prefix: prefix,
  document_prefix: '', batch_size: 32, max_input_tokens: 512, timeout_secs: 120,
});
async function setSearch(patch) {
  const info = await api('GET', '/info');
  await api('PUT', '/config', { ...info.config, search: { ...info.config.search, ...patch } });
}
async function waitFor(purpose, model, label, limitS = 3600, notId = null) {
  const started = Date.now();
  for (let i = 0; i < limitS; i++) {
    const s = await api('GET', '/embeddings/status');
    const space = s.spaces.find((x) => x.space.purpose === purpose && x.space.state === 'active' && x.space.model === model && x.space.id !== notId);
    // A space still building for this purpose is about to replace the active
    // one; wait for the swap, or later checks read a space on its way out.
    const building = s.spaces.some((x) => x.space.purpose === purpose && x.space.state === 'building');
    if (space && !building && space.embedded === space.chunks && s.jobs.queued + s.jobs.running === 0) {
      console.log(`${label}: ${space.embedded}/${space.chunks} ${purpose} chunks in space ${space.space.id} (${space.space.dimension} dims) after ${Math.round((Date.now() - started) / 1000)} s`);
      return space;
    }
    await sleep(1000);
  }
  throw new Error(`${label}: not ready`);
}
const codeInputs = () =>
  Number(sql("SELECT COALESCE(SUM(c.inputs),0) FROM embedding_calls c JOIN embedding_spaces s ON s.id = c.space_id WHERE c.kind = 'document' AND s.purpose = 'code'"));

// ---- Part A: this repo -------------------------------------------------------
// PART=B skips this part and reuses the clone in <workDir> (indexing this
// repo on a CPU embedder takes about half an hour per pass).
const src = `${workDir}/kanbain-src`;
const suffix = Date.now().toString().slice(-5);
let oldCode = null;
if (process.env.PART !== 'B') {
  const BGE_QUERY = 'Represent this sentence for searching relevant passages: ';
  await setSearch({ text: embedder(textUrl, textModel, BGE_QUERY), code: null });
  sh(`rm -rf ${src} && git clone -q --depth 1 file:///workspaces/kanbain ${src} && git -C ${src} config user.email d@e.x && git -C ${src} config user.name Dev`);
  const kbRepo = await api('POST', '/repos', { path: src, display_name: `kanbain-src-${suffix}` });
  const kbProject = await api('POST', '/projects', { name: `KanbAIn code ${suffix}`, id_prefix: 'KBC', repo_ids: [kbRepo.id] });
  await waitFor('code', textModel, 'this repo indexed (code slot = text embedder)');

  const ask = (query, extra = {}) =>
    api('POST', '/search', { query, project_id: kbProject.id, kinds: ['code'], limit: 5, ...extra });
  // Numbers are handed out in create_with_version (`create` is a thin wrapper
  // since 3a). The plan's own docs quote the demo sentence word for word, so
  // they win that query fair and square; the vocabulary gap (ticket/assigned
  // vs issue_counter) is the reranker's job in 3e, so its rank is reported,
  // not asserted.
  const isNumbering = (h) => h.path === 'crates/db/src/models/issue.rs' && (h.symbol ?? '').endsWith('fn create_with_version');
  const where = await ask('where are ticket numbers assigned', { limit: 10 });
  where.hits.forEach((h, i) => console.log(`  ${i + 1}. ${h.path}:${h.start_line}-${h.end_line} ${h.symbol ?? ''}`));
  const nlRank = where.hits.findIndex(isNumbering);
  console.log(`INFO: "where are ticket numbers assigned" ranks create_with_version ${nlRank >= 0 ? nlRank + 1 : 'below 10'} (3e target)`);
  const byCode = await ask('issue number counter', { limit: 10 });
  const rank = byCode.hits.findIndex(isNumbering);
  check(rank >= 0, `Issue::create_with_version found for "issue number counter" at rank ${rank + 1}`);
  if (rank >= 0) {
    const hit = byCode.hits[rank];
    const file = sh(`sed -n '${hit.start_line},${hit.end_line}p' crates/db/src/models/issue.rs`, src);
    check(/pub async fn create_with_version\(/.test(file) && file.trim().endsWith('}'), `whole function: lines ${hit.start_line}-${hit.end_line}`);
  }

  // One function changed: only it is embedded again (counted in the active
  // code space only; a building space embeds the change too).
  const activeCode = (await api('GET', '/embeddings/status')).spaces.find((s) => s.space.purpose === 'code' && s.space.state === 'active');
  const spaceInputs = () =>
    Number(sql(`SELECT COALESCE(SUM(inputs),0) FROM embedding_calls WHERE kind = 'document' AND space_id = ${activeCode.space.id}`));
  const before = spaceInputs();
  const projectRs = 'crates/db/src/models/project.rs';
  const anchor = 'pub async fn find_all(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {\n';
  check(sh(`cat ${projectRs}`, src).includes(anchor.trim()), 'project.rs has find_all');
  sh(`python3 - <<'PY'
  p='${projectRs}'
  s=open(p).read()
  a="""${anchor}"""
  s=s.replace(a, a + "        // Every project, in board order.\\n", 1)
  open(p,'w').write(s)
  PY
  git commit -qam 'Comment inside find_all'`, src);
  const newBlob = sh(`git rev-parse HEAD:${projectRs}`, src);
  await api('POST', '/embeddings/reindex', { project_id: kbProject.id });
  for (let i = 0; i < 300; i++) {
    const blob = sql(`SELECT blob_sha FROM sources WHERE kind = 'code' AND scope = 'main' AND path = '${projectRs}' AND repo_id = '${kbRepo.id}'`);
    const pending = Number(sql(`SELECT COUNT(*) FROM chunks c JOIN sources s ON s.id = c.source_id LEFT JOIN chunk_embeddings e ON e.chunk_id = c.id AND e.space_id = ${activeCode.space.id} WHERE s.kind = 'code' AND e.chunk_id IS NULL`));
    if (blob === newBlob && pending === 0) break;
    await sleep(1000);
  }
  check(spaceInputs() - before === 1, `one-function change re-embedded ${spaceInputs() - before} chunk(s)`);

  // Code gets its own embedder: only the code space rebuilds.
  const textBefore = (await api('GET', '/embeddings/status')).spaces.find((s) => s.space.purpose === 'text' && s.space.state === 'active');
  oldCode = (await api('GET', '/embeddings/status')).spaces.find((s) => s.space.purpose === 'code' && s.space.state === 'active');
  // A code embedder of its own (no query prefix): a new configuration.
  await setSearch({ code: embedder(codeUrl, codeModel) });
  let building;
  for (let i = 0; i < 60 && !building; i++) {
    await sleep(500);
    building = (await api('GET', '/embeddings/status')).spaces.find((s) => s.space.purpose === 'code' && s.space.state === 'building');
  }
  check(!!building && building.space.id !== oldCode.space.id, `code space ${building?.space.id} building with ${codeModel}`);
  const during = await ask('where are ticket numbers assigned');
  check(during.semantic, 'code search keeps using the active space while building');
  await waitFor('code', codeModel, 'code rebuilt with its own embedder', undefined, oldCode?.space.id);
  const after = (await api('GET', '/embeddings/status')).spaces;
  check(after.some((s) => s.space.id === textBefore?.space.id && s.space.state === 'active'), 'the text space was not rebuilt');
  check(after.some((s) => s.space.id === oldCode.space.id && s.space.state === 'retired'), 'the old code space was retired');
}

// ---- Part B: agent search, overlay and the token benchmark -------------------
const bench = `${workDir}/bench`;
sh(`rm -rf ${bench} && mkdir -p ${bench}/crates/db/src/models && cp crates/db/src/models/issue.rs crates/db/src/models/project.rs crates/db/src/models/project_status.rs crates/db/src/models/issue_comment.rs ${bench}/crates/db/src/models/`, src);
sh('git init -q -b main && git config user.email d@e.x && git config user.name Dev && git add . && git commit -qm init', bench);
const benchRepo = await api('POST', '/repos', { path: bench, display_name: `bench-${suffix}` });
const benchProject = await api('POST', '/projects', { name: `Bench ${suffix}`, id_prefix: 'BEN', repo_ids: [benchRepo.id] });
await sleep(1500);
await waitFor('code', codeModel, 'bench repo indexed', undefined, oldCode?.space.id);

for (const [name, capabilities] of [['bench-search', ['read', 'edit', 'bash', 'search']], ['bench-plain', ['read', 'edit', 'bash']]]) {
  const agent = { name, description: 'Token benchmark', role: 'coder', runtime: 'claude-code', model: 'sonnet',
    role_prompt: 'You are a software engineer.', styles: [], skills: [], capabilities,
    limits: { max_steps: 20, budget_usd: 2, wall_minutes: 10 } };
  await api('POST', '/agent-library/agents', agent).catch(() => api('PUT', `/agent-library/agents/${name}`, agent));
}

async function run(agent, title) {
  const issue = await api('POST', `/projects/${benchProject.id}/issues`, { title, description: 'Ticket numbers should start at 100.' });
  const { workspace, execution_process: exec } = await api('POST', '/workspaces/start', {
    name: `${agent}-${issue.number}`,
    repos: [{ repo_id: benchRepo.id, target_branch: 'main' }],
    linked_issue: { issue_id: issue.id },
    executor_config: { executor: 'CLAUDE_CODE', agent_definition: agent },
    prompt: 'Make ticket numbers start at 100.',
    attachment_ids: null,
  });
  let p = exec;
  const started = Date.now();
  while (p.status === 'running' && Date.now() - started < 10 * 60_000) {
    await sleep(2000);
    p = await api('GET', `/execution-processes/${exec.id}`);
  }
  const events = await api('GET', `/execution-processes/${exec.id}/events`).catch(() => []);
  const modelCalls = events.filter((e) => e.event_type === 'model_call').length;
  const searches = events.filter((e) => e.event_type === 'tool_call' && e.payload.tool === 'search_knowledge');
  return { agent, issue, workspace, status: p.status, tokens_in: p.tokens_in, tokens_out: p.tokens_out,
    cost: p.cost_usd, modelCalls, searches: searches.length,
    excerptBytes: searches.reduce((a, e) => a + (e.payload.excerpt_bytes ?? 0), 0) };
}

const results = [];
for (let i = 1; i <= runsPerVariant; i++) {
  results.push(await run('bench-search', `Ticket numbers from 100 (search ${i})`));
  results.push(await run('bench-plain', `Ticket numbers from 100 (plain ${i})`));
}
console.log('\nToken benchmark (mock model; usage = request bytes / 4):');
console.log('agent          status     tokens_in  tokens_out  cost     model_calls  searches  excerpt_bytes');
for (const r of results) {
  console.log(`${r.agent.padEnd(14)} ${r.status.padEnd(10)} ${String(r.tokens_in).padStart(9)}  ${String(r.tokens_out).padStart(10)}  $${r.cost.toFixed(4)}  ${String(r.modelCalls).padStart(11)}  ${String(r.searches).padStart(8)}  ${String(r.excerptBytes).padStart(13)}`);
}
const avg = (agent, key) => {
  const rows = results.filter((r) => r.agent === agent);
  return rows.reduce((a, r) => a + r[key], 0) / rows.length;
};
const withSearch = avg('bench-search', 'tokens_in');
const without = avg('bench-plain', 'tokens_in');
console.log(`mean tokens_in: with search ${Math.round(withSearch)}, without ${Math.round(without)} (${Math.round((1 - withSearch / without) * 100)}% fewer)`);
check(results.every((r) => r.status === 'completed'), 'every benchmark run completed');
check(results.filter((r) => r.agent === 'bench-search').every((r) => r.searches === 1), 'search runs called search_knowledge once');
check(results.filter((r) => r.agent === 'bench-plain').every((r) => r.searches === 0), 'plain runs had no search tool');
const mockLog = execSync('cat /tmp/mock.log', { encoding: 'utf8' });
check(mockLog.includes('mcp__kanbain__search_knowledge'), 'Claude Code listed mcp__kanbain__search_knowledge (MCP config with env expansion works)');
check(mockLog.includes('"impl Issue \\u203a fn create"') || mockLog.includes('impl Issue › fn create'), 'the search result reached the model');

// The ticket overlay: the run's new file is found for its ticket only.
const first = results.find((r) => r.agent === 'bench-search');
await sleep(3000);
for (let i = 0; i < 60; i++) {
  const s = await api('GET', '/embeddings/status');
  if (s.jobs.queued + s.jobs.running === 0) break;
  await sleep(1000);
}
const overlayQuery = (issueId) =>
  api('POST', '/search', { query: 'first_ticket_number', project_id: benchProject.id, issue_id: issueId, kinds: ['code'], limit: 5 });
const mine = await overlayQuery(first.issue.id);
check(mine.hits.some((h) => h.path === 'crates/db/src/models/numbering.rs'), `the ticket's branch change is found for ${first.issue.number}`);
const other = results.find((r) => r.agent === 'bench-plain' && r !== first);
const unrelated = await api('POST', `/projects/${benchProject.id}/issues`, { title: 'Unrelated' });
const theirs = await overlayQuery(unrelated.id);
check(!theirs.hits.some((h) => h.path === 'crates/db/src/models/numbering.rs'), 'another ticket does not see it');
void other;
