// Drive the git-history check (Phase 3b): a repo with earlier commits for
// PROJEKTNAME-123, a ticket with that External ID, "Allow analysis", the
// prior-work summary, the old commits on the timeline and the changed files;
// then a sandboxed run on the ticket adds its own commit and run events.
// Needs the sandbox and the mock upstream with mock-script-history.json.
// Usage: node history.mjs <frontendUrl> <outDir> <backendPort> <token> <repoDir>
import { execSync } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import { chromium } from 'playwright';

const [base, outDir, port, token, repoDir] = process.argv.slice(2);
mkdirSync(outDir, { recursive: true });
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
const sh = (cmd, cwd, env = {}) =>
  execSync(cmd, { cwd, encoding: 'utf8', env: { ...process.env, ...env } }).trim();

// 1. A repo with work from before KanbAIn.
sh(`rm -rf ${repoDir} && mkdir -p ${repoDir}`);
sh('git init -q -b main && git config user.email anna@example.com && git config user.name "Anna Dev"', repoDir);
const at = (date) => ({ GIT_AUTHOR_DATE: date, GIT_COMMITTER_DATE: date });
const commit = (file, text, message, date) => {
  sh(`mkdir -p $(dirname ${file}) && printf '%s\\n' "${text}" > ${file} && git add . && git commit -qm "${message}"`, repoDir, at(date));
};
commit('README.md', '# shop', 'init', '2025-03-01T10:00:00Z');
commit('mailer.rs', 'pub fn send_reset_mail() {}', 'PROJEKTNAME-123: send reset mails over SMTP', '2025-03-02T10:00:00Z');
commit('templates/reset.html', '<p>Reset</p>', 'Fix the reset template for projektname-123', '2025-03-03T10:00:00Z');
commit('other.rs', 'x', 'PROJEKTNAME-1234: unrelated', '2025-03-04T10:00:00Z');
sh('git checkout -q -b feature/PROJEKTNAME-123-expiry', repoDir);
commit('token.rs', 'pub fn expired() -> bool { false }', 'Add the token expiry check', '2025-03-05T10:00:00Z');
sh('git checkout -q main', repoDir);

const suffix = Date.now().toString().slice(-5);
const repo = await api('POST', '/repos', { path: repoDir, display_name: `shop-${suffix}` });
const project = await api('POST', '/projects', {
  name: `History ${suffix}`,
  id_prefix: 'KAN',
  repo_ids: [repo.id],
});

// 2. The UI: create the ticket, allow the analysis.
const browser = await chromium.launch({ args: ['--no-sandbox'] });
const page = await (await browser.newContext({ viewport: { width: 1600, height: 1100 } })).newPage();
const errors = [];
page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
page.on('response', (r) => {
  if (r.url().includes('/api/') && r.status() >= 400) errors.push(`${r.status()} ${r.url()}`);
});
const shot = async (name) => {
  await page.screenshot({ path: `${outDir}/${name}.png` });
  console.log(`screenshot: ${outDir}/${name}.png`);
};
await page.goto(`${base}/tickets`, { timeout: 180_000 });
await page.waitForTimeout(3_000);
await page.getByRole('combobox', { name: 'Project' }).selectOption({ label: `History ${suffix}` });
await page.waitForTimeout(1_000);
await page.getByRole('button', { name: 'New ticket' }).click();
await page.locator('#ticket-title').fill('Password reset mail never arrives');
await page.locator('#ticket-external-id').fill('PROJEKTNAME-123');
await page.getByRole('button', { name: 'Create ticket' }).click();
await page.getByLabel('Ticket title', { exact: true }).waitFor({ timeout: 15_000 });
const permission = page.getByTestId('history-permission');
await permission.waitFor({ timeout: 30_000 }).catch(async (e) => {
  await shot('h-fail-permission');
  throw e;
});
const permissionText = await permission.textContent();
check(permissionText.includes('Found 3 commits for PROJEKTNAME-123'), `permission prompt: ${permissionText.slice(0, 60)}`);
await permission.scrollIntoViewIfNeeded();
await shot('h01-found-commits');
await permission.getByRole('button', { name: 'Allow' }).click();
await page.getByTestId('prior-work').waitFor({ timeout: 240_000 }).catch(async (e) => {
  await shot('h-fail-prior-work');
  throw e;
});
const priorWork = await page.getByTestId('prior-work').textContent();
check(priorWork.includes('What was left open') && priorWork.includes('token.rs'), 'prior-work summary shown');
check(/history · haiku · \d+ tokens/.test(priorWork), 'history agent, model and tokens shown');
const files = await page.getByTestId('ticket-files').textContent();
for (const f of ['mailer.rs', 'templates/reset.html', 'token.rs']) {
  check(files.includes(f), `file ${f} linked`);
}
check(!files.includes('other.rs'), 'PROJEKTNAME-1234 is not PROJEKTNAME-123');
await page.getByTestId('prior-work').scrollIntoViewIfNeeded();
await shot('h02-prior-work');
const timeline = page.getByTestId('ticket-timeline');
await timeline.scrollIntoViewIfNeeded();
await page.waitForTimeout(500);
const timelineText = await timeline.textContent();
check(timelineText.includes('Commit') && timelineText.includes('send reset mails over SMTP'), 'timeline: old commit');
check((timelineText.match(/before KanbAIn/g) ?? []).length >= 3, 'timeline: commits marked before KanbAIn');
check(timelineText.includes('Found 3 commits for PROJEKTNAME-123'), 'timeline: history check');
check(/Prior work summarized by history/.test(timelineText), 'timeline: history analysis');
const firstEvent = await timeline.locator('li').first().getAttribute('data-event-type');
check(firstEvent === 'prior_commit', 'timeline starts with the oldest commit');
await shot('h03-timeline');

// 3. A sandboxed run on the ticket.
const issues = await api('GET', `/projects/${project.id}/issues`);
const issue = issues.find((i) => i.external_id === 'PROJEKTNAME-123');
const { workspace, execution_process: exec } = await api('POST', '/workspaces/start', {
  name: `history-${suffix}`,
  repos: [{ repo_id: repo.id, target_branch: 'main' }],
  linked_issue: { issue_id: issue.id },
  executor_config: { executor: 'CLAUDE_CODE' },
  prompt: 'Follow the scripted steps.',
  attachment_ids: null,
});
check(workspace.branch.startsWith('kanbain/PROJEKTNAME-123-password-reset'), `ticket branch ${workspace.branch}`);
let run = exec;
const started = Date.now();
while (run.status === 'running' && Date.now() - started < 10 * 60_000) {
  await sleep(2000);
  run = await api('GET', `/execution-processes/${exec.id}`);
}
check(run.status === 'completed', `run ${run.status}`);
let history;
for (let i = 0; i < 30; i++) {
  await sleep(1000);
  history = await api('GET', `/issues/${issue.id}/history`);
  if (history.commits.some((c) => c.author === 'KanbAIn agent')) break;
}
const agentCommit = history.commits.find((c) => c.author === 'KanbAIn agent');
check(!!agentCommit && agentCommit.subject === 'PROJEKTNAME-123: add reset notes', 'agent commit linked to the ticket');
check(history.files.some((f) => f.path === 'NOTES.md'), 'agent-changed file linked');
check(history.history.prior_count === 3, 'agent commits are not prior work');
check(history.history.analyzed_hash === history.history.prior_hash, 'no re-analysis for agent commits');
const events = await api('GET', `/issues/${issue.id}/timeline`);
const finished = events.find((e) => e.event_type === 'run_finished');
check(events.some((e) => e.event_type === 'run_started'), 'timeline: run started');
check(!!finished && finished.payload.status === 'completed' && finished.payload.tokens_in > 0, `timeline: run finished ${JSON.stringify(finished?.payload).slice(0, 160)}`);
// The run saw prior_work.md and the commit-prefix rule (the mock logs
// which markers appear in a request, tool results included).
const mockLog = execSync('tail -n 20 /tmp/mock.log', { encoding: 'utf8' });
check(mockLog.includes('"## What was done"'), 'the run read prior_work.md');
check(mockLog.includes('Start every commit message with'), 'ticket.md asks for the ID prefix');

await page.reload();
await page.waitForTimeout(3_000);
console.log(errors.length ? `ERRORS:\n${errors.join('\n')}` : 'No console errors or failed API calls');
if (errors.length) process.exitCode = 1;
await browser.close();
