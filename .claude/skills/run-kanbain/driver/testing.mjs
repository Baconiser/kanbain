// Drive the tester and the fix loop (Phase 6) against the sandbox and the
// mock upstream with mock-script-testing.json.
//
// TST-1 "Cart total": the coder's total() panics on an empty cart. The
// tester writes tests (plus a file outside the test paths, which the fence
// drops) and reports the bug; a fix run repairs it in round 1, the re-test
// reports nothing and the ticket moves to Review.
// TST-2 "Discount codes": the plan has no checkpoint, so KanbAIn adds the
// final one. Its bug survives every fix run; after round 3 the ticket goes
// to Blocked with a notification. Marking the issue won't fix and retesting
// then passes it.
// Usage: node testing.mjs <frontendUrl> <outDir> <backendPort> <token> <repoDir> <mockLog> <backendLog>
import { execSync } from 'node:child_process';
import { mkdirSync, readFileSync } from 'node:fs';
import { chromium } from 'playwright';

const [base, outDir, port, token, repoDir, mockLog, backendLog] =
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
const fatal = (message) => Object.assign(new Error(message), { fatal: true });
async function until(what, fn, ms = 900_000) {
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

// 1. A small repo; ticket branches start from it.
sh(`rm -rf ${repoDir} && mkdir -p ${repoDir}/src`);
sh('git init -q -b main && git config user.email anna@example.com && git config user.name "Anna Dev"', repoDir);
sh(`printf '# shop\\n' > README.md && printf '// modules\\n' > src/lib.rs && git add . && git commit -qm init`, repoDir);

const suffix = Date.now().toString().slice(-5);
const projectName = `Testing ${suffix}`;
const repo = await api('POST', '/repos', { path: repoDir, display_name: `tst-shop-${suffix}` });
const project = await api('POST', '/projects', { name: projectName, id_prefix: 'TST', repo_ids: [repo.id] });
const exported = await api('GET', `/export?format=json&project_id=${project.id}`);
const statuses = exported.projects[0].statuses;
const statusOf = (kind) => statuses.find((s) => s.kind === kind);
const ticketA = await api('POST', `/projects/${project.id}/issues`, {
  title: 'Cart total',
  description: 'The cart shows the total of its items. An empty cart totals 0.',
  mode: 'yolo',
});
const ticketB = await api('POST', `/projects/${project.id}/issues`, {
  title: 'Discount codes',
  description: 'SAVE10 takes 10% off the order total.',
  mode: 'yolo',
});

// 2. Plans from the planner (yolo), both at once.
for (const t of [ticketA, ticketB]) await api('POST', `/issues/${t.id}/planning/start`);
for (const t of [ticketA, ticketB]) {
  await until(`the plan of ${t.title}`, async () => {
    const v = await api('GET', `/issues/${t.id}/planning`);
    if (v.planning?.status === 'failed') throw fatal(v.planning.error);
    return v.planning?.status === 'awaiting_approval';
  });
}

// 3. Approve TST-2 through the API, TST-1 in the UI.
const planB = (await api('GET', `/issues/${ticketB.id}/planning`)).plans.at(-1);
await api('POST', `/plans/${planB.id}/approve`);

const browser = await chromium.launch({ args: ['--no-sandbox'] });
const page = await (await browser.newContext({ viewport: { width: 1600, height: 1300 } })).newPage();
const unfold = async (testId) => {
  const toggle = page.getByTestId(`${testId}-toggle`);
  if ((await toggle.getAttribute('aria-expanded')) === 'false') await toggle.click();
};
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
const failShot = (name) => async (e) => {
  await shot(name);
  throw e;
};
const openTicket = async (title, tab = 'coding') => {
  await page.goto(`${base}/tickets`, { timeout: 180_000 });
  const picker = page.getByRole('combobox', { name: 'Project' });
  await picker.waitFor({ timeout: 90_000 }).catch(failShot(`t-fail-open-${tab}`));
  await page.waitForTimeout(1_000);
  await picker.selectOption({ label: projectName });
  await page.waitForTimeout(1_000);
  // By display id: a sub-ticket has its subtask's title, often the same.
  const ticket = title === 'Cart total' ? ticketA : ticketB;
  await page.getByText(`TST-${ticket.number}`, { exact: true }).first().click();
  await page.getByTestId(`ticket-tab-${tab}`).click();
};
await openTicket('Cart total', 'plan');
await page.getByTestId('ticket-plan').waitFor({ timeout: 15_000 });
await page.getByTestId('plan-approve').click();
await page.getByTestId('ticket-tab-coding').click();
const coding = page.getByTestId('ticket-coding');
await coding.waitFor({ timeout: 30_000 }).catch(failShot('t-fail-coding'));

const view = (t) => api('GET', `/issues/${t.id}/coding`);
const issue = (t) => api('GET', `/issues/${t.id}`);
const blockedOn = (v) => {
  if (v.orchestration?.status === 'blocked') throw fatal(v.orchestration.error);
};

// 4. TST-1: the tester finds the seeded bug; a fix run starts.
const fixing = await until('the first fix run of TST-1', async () => {
  const v = await view(ticketA);
  blockedOn(v);
  return v.checkpoints[0]?.status === 'fixing' ? v : null;
}).catch(failShot('t-fail-fixing'));
check(fixing.checkpoints.length === 1 && !fixing.checkpoints[0].is_final, 'TST-1 has its plan\'s one checkpoint (no final one added)');
const found = fixing.issues_found;
check(found.length === 1 && found[0].severity === 'high' && found[0].category === 'edge_case' && found[0].status === 'open',
  `the tester's issue is stored: ${found.map((i) => `${i.severity}/${i.category}/${i.status} ${i.file}:${i.line}`).join(', ')}`);
check(JSON.stringify(fixing.checkpoints[0].rejected_files) === JSON.stringify(['tst-shop/src/cart_fixture.rs']),
  `the change outside the test paths was dropped: ${JSON.stringify(fixing.checkpoints[0].rejected_files)}`);
check((await issue(ticketA)).status_id === statusOf('testing').id, 'TST-1 is in Testing while its checkpoint runs');
check((await issue(ticketA)).fix_round === 1, 'TST-1 is in fix round 1');
await page.getByTestId('coding-checkpoint-rejected').waitFor({ timeout: 15_000 }).catch(() => {});
await page.getByTestId('issue-found').first().waitFor({ timeout: 15_000 }).catch(() => {});
await coding.scrollIntoViewIfNeeded();
await shot('t01-fixing');

// 5. TST-1: fixed in round 1, re-tested clean, in Review.
const doneA = await until('TST-1 done', async () => {
  const v = await view(ticketA);
  blockedOn(v);
  return v.orchestration.status === 'done' ? v : null;
}).catch(failShot('t-fail-done-a'));
check(doneA.checkpoints[0].status === 'passed' && doneA.checkpoints[0].round === 1, `checkpoint passed after 1 fix run (${doneA.checkpoints[0].status}, round ${doneA.checkpoints[0].round})`);
check(doneA.issues_found[0].status === 'fixed' && doneA.issues_found[0].fixed_round === 1, `the issue is fixed in round 1 (${doneA.issues_found[0].status}, ${doneA.issues_found[0].fixed_round})`);
check((await issue(ticketA)).status_id === statusOf('review').id, 'TST-1 moved to Review');
check(markerCount('Test paths') > 0 && markerCount('**/tests/**') > 0, 'the tester read the test paths in checkpoint.md');
check(markerCount('# Open issues at checkpoint') > 0, 'the fix run read issues.md');
check(markerCount('Issues from the last test run') > 0, 'the re-test saw the earlier issues');

const branchA = doneA.ticket_branch;
const log = sh(`git log --format=%s ${branchA}`, repoDir).split('\n');
console.log(`TST-1 ticket branch ${branchA}:\n  ${log.join('\n  ')}`);
const testCommit = sh(`git log --format=%H --grep='^TST-1: Tests for checkpoint 1$' ${branchA}`, repoDir).split('\n')[0];
check(!!testCommit, 'the tester\'s tests are one "TST-1: Tests for checkpoint 1" commit');
if (testCommit) {
  const files = sh(`git show --name-only --format= ${testCommit}`, repoDir);
  check(files === 'tests/cart_total.rs', `the test commit holds only tests/cart_total.rs (${files.replace(/\n/g, ', ')})`);
}
check(log.includes('TST-1: Merge tests for checkpoint 1'), 'the tests are merged into the ticket branch');
check(log.includes('TST-1: Merge fixes for checkpoint 1 (fix round 1)'), 'the fix is merged into the ticket branch');
check(sh(`git show ${branchA}:src/cart.rs`, repoDir).includes('items.iter().sum()'), 'src/cart.rs on the ticket branch is the fixed one');
check(sh(`git ls-tree -r --name-only ${branchA}`, repoDir).split('\n').every((f) => f !== 'src/cart_fixture.rs'), 'the dropped file is not on the ticket branch');
check(!sh('git log --format=%s main', repoDir).includes('TST-'), 'main is untouched');

await openTicket('Cart total');
await coding.scrollIntoViewIfNeeded();
await page.getByTestId('coding-status').filter({ hasText: 'Merged and tested' }).waitFor({ timeout: 15_000 }).catch(failShot('t-fail-done-ui'));
await page.getByTestId('coding-checkpoint-1').click();
await page.waitForTimeout(500);
await shot('t02-passed');
await page.getByTestId('ticket-tab-activity').click();
const timeline = page.getByTestId('ticket-timeline');
await unfold('ticket-timeline');
await page.getByRole('button', { name: /Show \d+ earlier event/ }).click().catch(() => {});
await page.waitForTimeout(800);
const timelineText = await timeline.textContent();
for (const line of [
  'Tester started on checkpoint 1: Cart totals are right',
  'Tests for checkpoint 1: 1 new, 0 still open, 0 fixed; dropped 1 change outside the test paths',
  'Fix round 1: a coder works on 1 open issue of checkpoint 1',
  'Fix round 1 merged into the ticket branch',
  'Tester re-checking checkpoint 1 after fix 1',
  'Tests for checkpoint 1: 0 new, 0 still open, 1 fixed',
  'Checkpoint 1 passed: Cart totals are right',
  'All subtasks merged and tested; ready for review',
]) {
  check(timelineText.includes(line), `timeline: ${line}`);
}
await timeline.scrollIntoViewIfNeeded();
await shot('t03-timeline');

// 6. TST-2: out of fix rounds, Blocked with a notification.
const blockedB = await until('TST-2 blocked', async () => {
  const v = await view(ticketB);
  return v.orchestration?.status === 'blocked' ? v : null;
}).catch(failShot('t-fail-blocked'));
const cpB = blockedB.checkpoints[0];
check(blockedB.checkpoints.length === 1 && cpB.is_final, 'TST-2 got the final checkpoint (its plan has none)');
check(cpB.status === 'failed' && /1 open issue after 3 fix rounds/.test(cpB.error ?? ''), `checkpoint failed: ${cpB.error}`);
check(cpB.round === 3, `3 fix runs (${cpB.round})`);
check(blockedB.issues_found.length === 1 && blockedB.issues_found[0].status === 'open',
  `the re-tests repeated the issue by id instead of duplicating it (${blockedB.issues_found.length} found)`);
const ticketBNow = await issue(ticketB);
check(ticketBNow.fix_round === 3, `TST-2 used all 3 fix rounds (${ticketBNow.fix_round})`);
check(ticketBNow.status_id === statusOf('blocked').id, 'TST-2 moved to Blocked');
const backend = readFileSync(backendLog, 'utf8');
check(/Notification: Ticket blocked: "Discount codes": Failed: checkpoint 1 \(1 open issue after 3 fix rounds\)/.test(backend), 'a "Ticket blocked" notification went out');
const timelineB = await api('GET', `/issues/${ticketB.id}/timeline`);
const count = (type) => timelineB.filter((e) => e.event_type === type).length;
check(count('tests_finished') === 4 && count('fix_started') === 3, `4 tester runs, 3 fix runs (${count('tests_finished')}, ${count('fix_started')})`);

await openTicket('Discount codes');
await coding.scrollIntoViewIfNeeded();
await page.getByTestId('issue-found').first().waitFor({ timeout: 15_000 }).catch(failShot('t-fail-blocked-ui'));
await shot('t04-blocked');

// 7. TST-2: accept the issue as it is, retest; it passes.
const beforeRetest = markerCount('Accepted as they are');
await page.getByTestId('issue-found-wontfix').click();
await page.getByTestId('issue-found').filter({ hasText: "Won't fix" }).waitFor({ timeout: 15_000 });
await page.getByTestId('coding-checkpoint-retry').click();
const doneB = await until('TST-2 done after the retest', async () => {
  const v = await view(ticketB);
  return v.orchestration.status === 'done' ? v : null;
}).catch(failShot('t-fail-retest'));
check(doneB.checkpoints[0].status === 'passed', 'the retest passed with the issue accepted');
check(doneB.issues_found.length === 1 && doneB.issues_found[0].status === 'wontfix', 'the issue stays won\'t fix');
check(markerCount('Accepted as they are') > beforeRetest, 'the tester was told which issue is accepted');
check((await issue(ticketB)).status_id === statusOf('review').id, 'TST-2 moved to Review');
await page.getByTestId('coding-status').filter({ hasText: 'Merged and tested' }).waitFor({ timeout: 15_000 }).catch(() => {});
await shot('t05-wontfix-passed');

// 8. Issues found are searchable (E6).
const hits = await until('the issue in search', async () => {
  const r = await api('POST', '/search', { query: 'SAVE10 discount', project_id: project.id, kinds: ['issue_found'] });
  return r.hits.length > 0 ? r.hits : null;
}, 120_000).catch(() => []);
check(hits.length > 0 && hits[0].kind === 'issue_found', `search finds the issue (${hits.length} hits)`);

// 9. The project's test paths round-trip.
const saved = await api('PUT', `/projects/${project.id}`, {
  settings: { ...project.settings, test_paths: ['qa/**'] },
});
check(JSON.stringify(saved.settings.test_paths) === '["qa/**"]', 'project test paths can be set');

console.log(errors.length ? `errors:\n${errors.join('\n')}` : 'no console errors or failed API calls');
check(errors.length === 0, 'no console errors or failed API calls');
await browser.close();
