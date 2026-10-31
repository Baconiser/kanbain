// Drive the planning flow and approval gate (Phase 4) against the sandbox
// and the mock upstream with mock-script-planning.json.
//
// A repo where PLN-1 (done) changed src/mailer.rs; then, in the UI, PLN-2
// (ask mode) is assigned to the planner, which asks questions; the answer
// resumes it and it writes plan v1; an edit makes v2; "Send back" makes the
// planner write v3; approving moves the ticket to In progress. Then PLN-3
// (yolo) is dragged to Planning through the bulk API and gets a plan whose
// assumptions cite PLN-1 and its file.
// Usage: node planning.mjs <frontendUrl> <outDir> <backendPort> <token> <repoDir> <mockLog>
import { execSync } from 'node:child_process';
import { mkdirSync, readFileSync } from 'node:fs';
import { chromium } from 'playwright';

const [base, outDir, port, token, repoDir, mockLog] = process.argv.slice(2);
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
async function until(what, fn, ms = 300_000) {
  const started = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - started > ms) throw new Error(`timed out waiting for ${what}`);
    await sleep(2000);
  }
}

// 1. A repo where PLN-1 changed the mailer.
sh(`rm -rf ${repoDir} && mkdir -p ${repoDir}/src`);
sh('git init -q -b main && git config user.email anna@example.com && git config user.name "Anna Dev"', repoDir);
sh(`printf '# shop\\n' > README.md && git add . && git commit -qm init`, repoDir);
sh(`printf 'pub fn send_reset_mail(to: &str) {\\n    smtp::send(to, "reset");\\n}\\n' > src/mailer.rs && git add . && git commit -qm "PLN-1: send password reset mails over SMTP"`, repoDir);

const suffix = Date.now().toString().slice(-5);
const projectName = `Planning ${suffix}`;
const repo = await api('POST', '/repos', { path: repoDir, display_name: `plan-shop-${suffix}` });
const project = await api('POST', '/projects', { name: projectName, id_prefix: 'PLN', repo_ids: [repo.id] });
const exported = await api('GET', `/export?format=json&project_id=${project.id}`);
const statuses = exported.projects[0].statuses;
const statusOf = (kind) => statuses.find((s) => s.kind === kind);

const pln1 = await api('POST', `/projects/${project.id}/issues`, {
  title: 'Password reset mail never arrives',
  description: 'Users request a password reset but the reset mail never arrives. Send it over SMTP.',
});
await until('PLN-1 file links', async () => {
  const h = await api('GET', `/issues/${pln1.id}/history`);
  return h.files.some((f) => f.path === 'src/mailer.rs');
}, 60_000);
await api('PUT', `/issues/${pln1.id}`, { status_id: statusOf('done').id });

const pln2 = await api('POST', `/projects/${project.id}/issues`, {
  title: 'Password reset link should expire',
  description: 'The link in the password reset mail works forever. It should expire, and an expired link should say so.',
});
// Related search needs PLN-1 indexed.
await until('PLN-1 in search', async () => {
  const r = await api('POST', '/search', { query: 'password reset mail', project_id: project.id, kinds: ['ticket'] });
  return r.hits.some((h) => h.issue_id === pln1.id);
}, 120_000);

// 2. The UI.
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
const failShot = (name) => async (e) => {
  await shot(name);
  throw e;
};
await page.goto(`${base}/tickets`, { timeout: 180_000 });
await page.waitForTimeout(3_000);
await page.getByRole('combobox', { name: 'Project' }).selectOption({ label: projectName });
await page.waitForTimeout(1_000);
await page.getByText('Password reset link should expire').first().click();
await page.getByTestId('ticket-tab-plan').click();
const plan = page.getByTestId('ticket-plan');
await plan.waitFor({ timeout: 15_000 });
await plan.scrollIntoViewIfNeeded();
await page.getByTestId('plan-start').click();

// Ask mode: questions, the marker, the answer.
await page.getByTestId('planning-waiting').waitFor({ timeout: 600_000 }).catch(failShot('p-fail-waiting'));
check(true, 'planner asked questions: "Waiting for your answer" shown');
const marker = page.getByTestId('planning-marker-waiting_for_answer');
check(await marker.isVisible(), 'board card shows "Waiting for your answer"');
await page.getByTestId('ticket-tab-activity').click();
const comments = await page.locator('section', { hasText: 'Questions from the planner' }).last().textContent();
check(comments.includes('expire after 30 or 60 minutes'), 'questions posted as an agent comment');
await page.getByTestId('ticket-tab-plan').click();
await page.getByTestId('planning-waiting').scrollIntoViewIfNeeded();
await shot('p01-questions');

await page.getByTestId('ticket-tab-activity').click();
await page.getByLabel('New comment').fill('1. 30 minutes.\n2. Yes, revoke the older links.');
await page.getByRole('button', { name: 'Comment', exact: true }).click();
await page.getByTestId('ticket-tab-plan').click();
await page.getByTestId('plan-version-1').waitFor({ timeout: 600_000 }).catch(failShot('p-fail-v1'));
const assumptions1 = await page.getByTestId('plan-assumptions').inputValue();
check(assumptions1.includes('answered by the user'), 'plan v1 uses the answer');
check(assumptions1.includes('PLN-1') && assumptions1.includes('mailer.rs'), `plan v1 cites PLN-1 and its file: ${assumptions1.split('\n')[0]}`);
check(await page.getByTestId('planning-marker-awaiting_approval').isVisible(), 'board card shows "Plan to approve"');
await page.getByTestId('plan-form').scrollIntoViewIfNeeded();
await shot('p02-plan-v1');

// Edit → v2.
await page.getByTestId('plan-assumptions').fill(`${assumptions1}\nKeep using the existing SMTP mailer.`);
await page.getByTestId('plan-save').click();
await page.getByTestId('plan-version-2').waitFor({ timeout: 15_000 }).catch(failShot('p-fail-v2'));
check((await page.getByTestId('plan-version-2').textContent()).includes('edited'), 'edit saved as v2 (edited)');

// Send back → v3 from the planner.
await page.getByTestId('plan-send-back').click();
await page.getByTestId('plan-feedback').fill('Also revoke older links when a new one is sent.');
await page.getByTestId('plan-send-back-submit').click();
await page.getByTestId('plan-version-3').waitFor({ timeout: 600_000 }).catch(failShot('p-fail-v3'));
check((await page.getByTestId('plan-version-2').textContent()).includes('rejected'), 'v2 marked rejected');
const subtasks = await page.getByTestId('plan-subtask-title').count();
check(subtasks === 4, `v3 has the extra subtask (${subtasks} subtasks)`);
await shot('p03-plan-v3');

// Approve.
await page.getByTestId('plan-approve').click();
await page.getByTestId('plan-approved').waitFor({ timeout: 15_000 }).catch(failShot('p-fail-approve'));
const approved = await api('GET', `/issues/${pln2.id}`);
check(approved.status_id === statusOf('in_progress').id, 'approved ticket moved to In progress');
await page.getByTestId('ticket-tab-activity').click();
const timeline = page.getByTestId('ticket-timeline');
await timeline.scrollIntoViewIfNeeded();
await page.waitForTimeout(800);
const timelineText = await timeline.textContent();
for (const line of [
  'Assigned to the planner',
  'The planner asked 2 questions',
  'Planner resumed with your answers',
  'Plan version 1 (by the planner',
  'Plan version 2 (edited by you',
  'Plan version 2 sent back: Also revoke',
  'Plan version 3 (by the planner, 4 subtasks)',
  'Plan version 3 approved',
]) {
  check(timelineText.includes(line), `timeline: ${line}`);
}
await shot('p04-approved-timeline');

// 3. Run internals.
const view = await api('GET', `/issues/${pln2.id}/planning`);
const runs = (await api('GET', `/execution-processes?session_id=${view.planning.session_id}`).catch(() => null)) ?? [];
console.log(`planner session ${view.planning.session_id}: ${Array.isArray(runs) ? runs.length : '?'} runs`);
const log = readFileSync(mockLog, 'utf8').trim().split('\n').map((l) => JSON.parse(l));
check(log.some((r) => r.markers.includes('Related work for') && r.markers.includes('Changed:')), 'the planner read related.md with PLN-1\'s changes');
check(log.some((r) => r.markers.includes('may only write files under /kanbain/out')), 'a Write outside /kanbain/out was refused');
check(log.some((r) => r.tools.includes('Write') && !r.tools.includes('Edit') && !r.tools.includes('Bash')), 'planner had Write but no Edit or Bash');
const indexed = await until('plan in search', async () => {
  const r = await api('POST', '/search', { query: 'revoke older reset links', project_id: project.id, kinds: ['plan'] });
  return r.hits.find((h) => h.issue_id === pln2.id);
}, 60_000).catch(() => null);
check(!!indexed, 'the plan is searchable (kind plan)');

// 4. Yolo, by moving the ticket to Planning (the board drag's API).
const pln3 = await api('POST', `/projects/${project.id}/issues`, {
  title: 'Password reset links expire too early on mobile',
  description: 'Mobile users open the reset mail later; the reset link has already expired.',
  mode: 'yolo',
});
await api('POST', `/projects/${project.id}/issues/bulk`, [
  { id: pln3.id, status_id: statusOf('planning').id },
]);
const yolo = await until('yolo plan', async () => {
  const v = await api('GET', `/issues/${pln3.id}/planning`);
  if (v.planning?.status === 'failed') throw new Error(v.planning.error);
  return v.planning?.status === 'awaiting_approval' ? v : null;
}, 600_000);
const yoloPlan = yolo.plans.at(-1).plan;
check(yoloPlan.assumptions.some((a) => a.startsWith('Decided without asking')), 'yolo plan lists its decisions as assumptions');
check(yoloPlan.assumptions.some((a) => a.includes('PLN-1') && a.includes('Done')), `yolo plan cites the done PLN-1: ${yoloPlan.assumptions[0]}`);
check(yoloPlan.subtasks[0].files.some((f) => f.includes('mailer.rs')), `yolo plan names PLN-1's file: ${yoloPlan.subtasks[0].files.join(', ')}`);
const pln3Now = await api('GET', `/issues/${pln3.id}`);
check(pln3Now.status_id === statusOf('awaiting_approval').id, 'yolo ticket moved to Awaiting approval');
await page.getByText('Password reset links expire too early on mobile').first().click();
await page.getByTestId('ticket-tab-plan').click();
await page.getByTestId('plan-version-1').waitFor({ timeout: 15_000 });
await page.getByTestId('ticket-plan').scrollIntoViewIfNeeded();
await shot('p05-yolo');

console.log(errors.length ? `errors:\n${errors.join('\n')}` : 'no console errors or failed API calls');
check(errors.length === 0, 'no console errors or failed API calls');
await browser.close();
