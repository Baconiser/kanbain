// Drive the orchestrator (Phase 5) against the sandbox and the mock upstream
// with mock-script-orchestration.json.
//
// ORC-1 (yolo) gets a 3-subtask plan from the planner: 1 and 2 touch
// disjoint files and run in parallel, 3 depends on both. Both 1 and 2 also
// append to src/lib.rs, which the plan didn't list, so the second merge
// conflicts and that subtask re-runs on the updated ticket branch. Subtask 3
// is stopped and retried in the UI, then the backend is restarted under it;
// the orchestrator resumes it and the ticket ends in Review with all three
// merges on the ticket branch.
// Usage: node orchestration.mjs <frontendUrl> <outDir> <backendPort> <token> <repoDir> <mockLog> <restartCmd>
import { exec, execSync } from 'node:child_process';
import { mkdirSync, readFileSync } from 'node:fs';
import { chromium } from 'playwright';

const [base, outDir, port, token, repoDir, mockLog, restartCmd] =
  process.argv.slice(2);
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
  return json.data ?? json;
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sh = (cmd, cwd) => execSync(cmd, { cwd, encoding: 'utf8' }).trim();
async function until(what, fn, ms = 600_000) {
  const started = Date.now();
  for (;;) {
    const value = await fn().catch((e) => {
      if (e.fatal) throw e;
      return null;
    });
    if (value) return value;
    if (Date.now() - started > ms) throw new Error(`timed out waiting for ${what}`);
    await sleep(2000);
  }
}
const mockRequests = () =>
  readFileSync(mockLog, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l));
const markerCount = (marker) =>
  mockRequests().filter((r) => r.markers.includes(marker)).length;

// 1. A small repo; the ticket branch starts from it.
sh(`rm -rf ${repoDir} && mkdir -p ${repoDir}/src`);
sh('git init -q -b main && git config user.email anna@example.com && git config user.name "Anna Dev"', repoDir);
sh(`printf '# shop\\n' > README.md && printf '// modules\\n' > src/lib.rs && git add . && git commit -qm init`, repoDir);

const suffix = Date.now().toString().slice(-5);
const projectName = `Orchestration ${suffix}`;
const repo = await api('POST', '/repos', { path: repoDir, display_name: `orc-shop-${suffix}` });
const project = await api('POST', '/projects', { name: projectName, id_prefix: 'ORC', repo_ids: [repo.id] });
const exported = await api('GET', `/export?format=json&project_id=${project.id}`);
const statuses = exported.projects[0].statuses;
const statusOf = (kind) => statuses.find((s) => s.kind === kind);
const ticket = await api('POST', `/projects/${project.id}/issues`, {
  title: 'Order checkout',
  description: 'Customers can place an order and get a confirmation mail.',
  mode: 'yolo',
});

// 2. A plan from the planner (yolo).
await api('POST', `/issues/${ticket.id}/planning/start`);
await until('the plan', async () => {
  const v = await api('GET', `/issues/${ticket.id}/planning`);
  if (v.planning?.status === 'failed') throw Object.assign(new Error(v.planning.error), { fatal: true });
  return v.planning?.status === 'awaiting_approval';
});

// 3. Approve in the UI; coding starts.
const browser = await chromium.launch({ args: ['--no-sandbox'] });
const page = await (await browser.newContext({ viewport: { width: 1600, height: 1300 } })).newPage();
const errors = [];
let restarting = false;
page.on('console', (m) => m.type() === 'error' && !restarting && errors.push(m.text()));
page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
page.on('response', (r) => {
  if (!restarting && r.url().includes('/api/') && r.status() >= 400) errors.push(`${r.status()} ${r.url()}`);
});
const shot = async (name) => {
  await page.screenshot({ path: `${outDir}/${name}.png` });
  console.log(`screenshot: ${outDir}/${name}.png`);
};
const failShot = (name) => async (e) => {
  await shot(name);
  throw e;
};
const openTicket = async () => {
  await page.goto(`${base}/tickets`, { timeout: 180_000 });
  await page.waitForTimeout(3_000);
  await page.getByRole('combobox', { name: 'Project' }).selectOption({ label: projectName });
  await page.waitForTimeout(1_000);
  await page.getByText('Order checkout').first().click();
  await page.getByTestId('ticket-tab-plan').click();
  await page.getByTestId('ticket-plan').waitFor({ timeout: 15_000 });
};
await openTicket();
await page.getByTestId('ticket-plan').scrollIntoViewIfNeeded();
await page.getByTestId('plan-approve').click();
await page.getByTestId('ticket-tab-coding').click();
const coding = page.getByTestId('ticket-coding');
await coding.waitFor({ timeout: 30_000 }).catch(failShot('o-fail-coding'));
await coding.scrollIntoViewIfNeeded();

const view = () => api('GET', `/issues/${ticket.id}/coding`);
const sub = (v, id) => v.subtasks.find((s) => s.plan_subtask_id === id);

// 4. Subtasks 1 and 2 in parallel, 3 waiting for both.
const parallel = await until('subtasks 1 and 2 running together', async () => {
  const v = await view();
  return sub(v, '1').status === 'running' && sub(v, '2').status === 'running' ? v : null;
}, 60_000).catch(() => null);
check(!!parallel, 'subtasks 1 and 2 (disjoint files) run in parallel');
if (parallel) {
  check(sub(parallel, '3').status === 'pending', 'subtask 3 waits');
  check(sub(parallel, '3').waits_for.join(',') === '1,2', `subtask 3 waits for 1 and 2 (${sub(parallel, '3').waits_for})`);
}
check((await api('GET', `/issues/${ticket.id}`)).status_id === statusOf('in_progress').id, 'approved ticket is In progress');
await page.waitForTimeout(1500);
await shot('o01-parallel');

// 5. Subtask 1 merges, subtask 2 conflicts and re-runs.
const conflicted = await until('the merge conflict', async () => {
  const v = await view();
  if (v.orchestration.status === 'blocked') throw Object.assign(new Error(v.orchestration.error), { fatal: true });
  return sub(v, '2').fix_round === 1 ? v : null;
}).catch(failShot('o-fail-conflict'));
check(sub(conflicted, '1').status === 'merged', 'subtask 1 merged first');
check(sub(conflicted, '2').conflict?.files.some((f) => f.endsWith('src/lib.rs')), `conflict in ${sub(conflicted, '2').conflict?.files}`);
await page.getByTestId('coding-subtask-conflict').waitFor({ timeout: 15_000 }).catch(() => {});
await shot('o02-conflict');
const rerun = await until('subtask 2 merged after the re-run', async () => {
  const v = await view();
  if (sub(v, '2').status === 'failed') throw Object.assign(new Error(sub(v, '2').error), { fatal: true });
  return sub(v, '2').status === 'merged' ? v : null;
}).catch(failShot('o-fail-rerun'));
check(sub(rerun, '2').fix_round === 1, 'subtask 2 merged in fix round 1');
check(markerCount('Merge conflict (fix round') > 0 && markerCount('previous_attempt.diff') > 0, 'the re-run read conflict.md and previous_attempt.diff');
check((await api('GET', `/issues/${ticket.id}`)).fix_round === 1, 'ticket fix_round is 1');

// 6. Stop subtask 3 mid-run, then retry it.
const before = markerCount('Already merged into your branch');
await until('subtask 3 coding', async () => markerCount('Already merged into your branch') > before);
const stopButton = page.getByTestId('coding-subtask-3').getByTestId('coding-subtask-stop');
await stopButton.waitFor({ timeout: 15_000 }).catch(failShot('o-fail-stop-button'));
await stopButton.click();
const stopped = await until('subtask 3 stopped', async () => {
  const v = await view();
  return v.orchestration.status === 'stopped' && sub(v, '3').status === 'stopped' ? v : null;
}, 60_000).catch(failShot('o-fail-stopped'));
const stoppedRun = await api('GET', `/execution-processes/${sub(stopped, '3').execution_process_id}`);
check(stoppedRun.status === 'killed', `the stopped coder run was killed (${stoppedRun.status})`);
await page.getByTestId('coding-status').filter({ hasText: 'Stopped' }).waitFor({ timeout: 15_000 }).catch(() => {});
await shot('o03-stopped');

const beforeRetry = markerCount('Already merged into your branch');
await page.getByTestId('coding-subtask-3').getByTestId('coding-subtask-retry').click();
await until('subtask 3 coding again', async () => markerCount('Already merged into your branch') > beforeRetry);
check((await view()).orchestration.status === 'running', 'retry set coding running again');

// 7. Restart the backend under the running subtask; it resumes.
// Async, so the page's errors while the backend is down arrive inside the
// window they are ignored for; then a few seconds to reconnect.
restarting = true;
await new Promise((resolve, reject) =>
  exec(restartCmd, (err) => (err ? reject(err) : resolve()))
);
await sleep(8_000);
restarting = false;
const resumed = await until('subtask 3 merged after the restart', async () => {
  const v = await view();
  if (v.orchestration.status === 'blocked') throw Object.assign(new Error(v.orchestration.error), { fatal: true });
  return v.orchestration.status === 'done' ? v : null;
}).catch(failShot('o-fail-resume'));
check(resumed.subtasks.every((s) => s.status === 'merged'), 'all three subtasks merged');
check(resumed.subtasks.every((s) => s.branch === `${resumed.ticket_branch}--${s.plan_subtask_id}`), `subtask branches ${resumed.subtasks.map((s) => s.branch).join(', ')}`);
const archived = await Promise.all(
  resumed.subtasks.map((s) => api('GET', `/workspaces/${s.workspace_id}`).then((w) => w.archived))
);
check(archived.every(Boolean), `merged subtask workspaces are archived (${archived})`);
check(markerCount('pub mod model;\\npub mod mail;') > 0, 'subtask 3 saw both earlier merges in src/lib.rs');
check(markerCount('Running in parallel') > 0, 'a parallel coder was told which subtask runs next to it');

// 8. The ticket branch and the board.
const branch = resumed.ticket_branch;
const merges = sh(`git log --merges --format=%s ${branch}`, repoDir).split('\n');
console.log(`merges on ${branch}:\n  ${merges.join('\n  ')}`);
for (const n of ['1', '2', '3']) {
  check(merges.some((m) => m.startsWith(`ORC-1: Merge subtask ${n}:`)), `ticket branch has the merge of subtask ${n}`);
}
const lib = sh(`git show ${branch}:src/lib.rs`, repoDir);
check(['pub mod model;', 'pub mod mail;', 'pub mod route;'].every((l) => lib.includes(l)), `src/lib.rs on the ticket branch: ${lib.replace(/\n/g, ' | ')}`);
check(sh('git rev-parse --abbrev-ref HEAD', repoDir) === 'main' && !sh('git log --format=%s main', repoDir).includes('Merge'), 'main is untouched');
check((await api('GET', `/issues/${ticket.id}`)).status_id === statusOf('review').id, 'ticket moved to Review');
const board = await api('GET', `/export?format=json&project_id=${project.id}`);
const subIssues = board.projects[0].issues.filter((i) => i.parent_issue_id === ticket.id);
check(subIssues.length === 3 && subIssues.every((i) => i.status_id === statusOf('done').id), `3 sub-tickets, all Done (${subIssues.length})`);

// 9. The UI after a reload, with the timeline.
await openTicket();
await page.getByTestId('ticket-tab-coding').click();
await coding.scrollIntoViewIfNeeded();
await page.getByTestId('coding-status').filter({ hasText: 'All subtasks merged' }).waitFor({ timeout: 15_000 }).catch(failShot('o-fail-done-ui'));
await shot('o04-done');
await page.getByTestId('ticket-tab-activity').click();
const timeline = page.getByTestId('ticket-timeline');
await timeline.scrollIntoViewIfNeeded();
await page.waitForTimeout(800);
const timelineText = await timeline.textContent();
for (const line of [
  'Coding started from plan version 1 (3 subtasks)',
  'Subtask 1 merged into',
  'Subtask 2 conflicted in',
  'Subtask 2 started: Order mail (fix round 1)',
  'Subtask 3 stopped',
  'Coding retried',
  'Coding resumed after a restart',
  'All subtasks merged; ready for review',
]) {
  check(timelineText.includes(line), `timeline: ${line}`);
}
await shot('o05-timeline');

console.log(errors.length ? `errors:\n${errors.join('\n')}` : 'no console errors or failed API calls');
check(errors.length === 0, 'no console errors or failed API calls');
await browser.close();
