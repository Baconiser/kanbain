// Drive Phase 7 (scribe, review, merge, changelog export) against the
// sandbox and the mock upstream with mock-script-review.json.
//
// REV-1 (yolo) is planned and coded like ORC-1 in orchestration.mjs and
// reaches Review. The scribe writes an entry per coder, tester and fix run.
// In the Review tab: a commit on main blocks Merge until it is gone again,
// "Send to coders" turns feedback into an issue the fix loop handles, and
// Merge (with push) lands the work on main and the bare remote. Both
// changelog exports are checked.
// Usage: node review.mjs <frontendUrl> <outDir> <backendPort> <token> <repoDir> <mockLog>
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
async function raw(path) {
  const res = await fetch(apiBase + path, { headers: { 'x-kanbain-token': token } });
  return { status: res.status, headers: res.headers, text: await res.text() };
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sh = (cmd, cwd) => execSync(cmd, { cwd, encoding: 'utf8' }).trim();
async function until(what, fn, ms = 900_000) {
  const started = Date.now();
  for (;;) {
    const value = await fn().catch((e) => {
      if (e.fatal) throw e;
      return null;
    });
    if (value) return value;
    if (Date.now() - started > ms) throw new Error(`timed out waiting for ${what}`);
    await sleep(3000);
  }
}
const mockRequests = () =>
  readFileSync(mockLog, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l));

// 1. A repo with a bare remote, so Merge can push.
const remoteDir = `${repoDir}-remote.git`;
sh(`rm -rf ${repoDir} ${remoteDir} && mkdir -p ${repoDir}/src && git init -q --bare -b main ${remoteDir}`);
sh('git init -q -b main && git config user.email anna@example.com && git config user.name "Anna Dev"', repoDir);
sh(`printf '# shop\\n' > README.md && printf '// modules\\n' > src/lib.rs && git add . && git commit -qm init`, repoDir);
sh(`git remote add origin ${remoteDir} && git push -q -u origin main`, repoDir);

const suffix = Date.now().toString().slice(-5);
const projectName = `Review ${suffix}`;
const repo = await api('POST', '/repos', { path: repoDir, display_name: `rev-shop-${suffix}` });
const project = await api('POST', '/projects', { name: projectName, id_prefix: 'REV', repo_ids: [repo.id] });
const exported = await api('GET', `/export?format=json&project_id=${project.id}`);
const statuses = exported.projects[0].statuses;
const statusOf = (kind) => statuses.find((s) => s.kind === kind);
const ticket = await api('POST', `/projects/${project.id}/issues`, {
  title: 'Order checkout',
  description: 'Customers can place an order and get a confirmation mail.',
  mode: 'yolo',
});
const ticketStatus = async () => (await api('GET', `/issues/${ticket.id}`)).status_id;
const review = () => api('GET', `/issues/${ticket.id}/review`);

// 2. Plan (yolo) and approve it in the UI.
await api('POST', `/issues/${ticket.id}/planning/start`);
await until('the plan', async () => {
  const v = await api('GET', `/issues/${ticket.id}/planning`);
  if (v.planning?.status === 'failed') throw Object.assign(new Error(v.planning.error), { fatal: true });
  return v.planning?.status === 'awaiting_approval';
});

const browser = await chromium.launch({ args: ['--no-sandbox'] });
const context = await browser.newContext({ viewport: { width: 1600, height: 1300 }, acceptDownloads: true });
const page = await context.newPage();
const errors = [];
page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
page.on('response', (r) => {
  if (r.url().includes('/api/') && r.status() >= 400) errors.push(`${r.status()} ${r.url()}`);
});
const shot = async (name) => {
  await page.screenshot({ path: `${outDir}/${name}.png`, fullPage: false });
  console.log(`screenshot: ${outDir}/${name}.png`);
};
const unfold = async (testId) => {
  const toggle = page.getByTestId(`${testId}-toggle`);
  if ((await toggle.count()) && (await toggle.getAttribute('aria-expanded')) === 'false') {
    await toggle.click();
  }
};
const openTicket = async (tab) => {
  await page.goto(`${base}/tickets`, { timeout: 180_000 });
  await page.waitForTimeout(3_000);
  await page.getByRole('combobox', { name: 'Project' }).selectOption({ label: projectName });
  await page.waitForTimeout(1_000);
  await page.getByText('Order checkout').first().click();
  await page.getByTestId(`ticket-tab-${tab}`).click();
};
await openTicket('plan');
await page.getByTestId('ticket-plan').waitFor({ timeout: 15_000 });
await page.getByTestId('plan-approve').click();

// 3. Coding, testing, Review.
await until('Review', async () => (await ticketStatus()) === statusOf('review').id, 1_200_000);
check(true, 'REV-1 reached Review');

// 4. The scribe's entries: 3 coders, 2 checkpoints' testers.
const withEntries = await until('the changelog entries', async () => {
  const v = await review();
  const kinds = v.changelog.map((e) => e.kind);
  const coders = kinds.filter((k) => k === 'coder').length;
  const testers = kinds.filter((k) => k === 'tester').length;
  return coders >= 3 && testers >= 2 ? v : null;
}, 300_000);
const coderEntries = withEntries.changelog.filter((e) => e.kind === 'coder');
check(coderEntries.every((e) => e.summary.includes('Scripted scribe entry')), 'coder entries carry the scribe\'s text');
check(coderEntries.every((e) => !e.scribe_error), 'no scribe errors');
check(
  coderEntries.some((e) => e.files.some((f) => f.endsWith('/src/model.rs') || f.endsWith('src/model.rs'))),
  `a coder entry lists src/model.rs (${coderEntries.map((e) => e.files.join(' ')).join(' | ')})`
);
check(coderEntries.every((e) => e.commit_refs.length > 0), 'coder entries name their merge commits');
const scribeCalls = mockRequests().filter((r) => r.markers.includes('Write the changelog entry for'));
check(scribeCalls.length >= 5, `the scribe ran headless per run (${scribeCalls.length} requests)`);
check(scribeCalls.some((r) => /haiku/i.test(r.model ?? '')), `the scribe uses Haiku (${[...new Set(scribeCalls.map((r) => r.model))]})`);
const timeline = await api('GET', `/issues/${ticket.id}/timeline`);
check(timeline.some((e) => e.event_type === 'changelog_entry'), 'changelog entries show on the timeline');

// 5. The Review tab.
await openTicket('review');
const reviewTab = page.getByTestId('ticket-review');
await reviewTab.waitFor({ timeout: 30_000 });
await page.waitForTimeout(1500);
check(await page.getByTestId('review-merge').isEnabled(), 'Merge is enabled in Review');
check((await page.getByTestId('review-cost').innerText()).startsWith('$'), `the cost shows (${await page.getByTestId('review-cost').innerText()})`);
await unfold('review-changelog-section');
check((await page.getByTestId('review-changelog').locator('li').count()) >= 5, 'the changelog lists the entries');
await shot('r01-review');

// 6. main moved: Merge is blocked until the branch is on top again.
sh(`printf 'notes\\n' > NOTES.md && git add NOTES.md && git commit -qm 'Unrelated change on main'`, repoDir);
const behind = await review();
check(behind.blockers.some((b) => b.includes('behind main')), `a commit on main blocks Merge (${behind.blockers})`);
await openTicket('review');
await reviewTab.waitFor({ timeout: 30_000 });
await page.waitForTimeout(1500);
check(!(await page.getByTestId('review-merge').isEnabled()), 'Merge is disabled while behind');
check(await page.getByTestId('review-rebase').isVisible(), '"Rebase with agent" is offered');
await shot('r02-behind');
sh('git reset -q --hard HEAD~1', repoDir);
check((await review()).blockers.length === 0, 'the blocker clears once main is back');

// 7. Request changes from the coders: an issue at the final checkpoint,
//    fixed by a fix run, checked by the tester, back to Review.
await openTicket('review');
await reviewTab.waitFor({ timeout: 30_000 });
await unfold('review-request');
await page.getByTestId('review-feedback').fill('Validate orders before saving them.');
await page.getByTestId('review-to-coders').click();
await page.getByTestId('review-notice').waitFor({ timeout: 15_000 });
check(true, 'feedback sent to the coders');
await until('In progress', async () => (await ticketStatus()) === statusOf('in_progress').id, 60_000);
check(true, 'the ticket is In progress again');
await until('Review again', async () => (await ticketStatus()) === statusOf('review').id, 900_000);
const afterFix = await until('the fix entry', async () => {
  const v = await review();
  return v.changelog.some((e) => e.kind === 'fix') ? v : null;
}, 300_000);
const feedbackIssue = afterFix.issues_found.find((i) => i.description.includes('Validate orders'));
check(!!feedbackIssue, 'the feedback is an issue found');
check(feedbackIssue?.status === 'fixed', `the re-test marked it fixed (${feedbackIssue?.status})`);
const fixEntry = afterFix.changelog.find((e) => e.kind === 'fix');
check(fixEntry.issues_fixed.includes(feedbackIssue?.id), 'the fix entry names the issue it fixed');
check(mockRequests().some((r) => r.markers.includes('Review feedback:')), 'the fixer saw the feedback');
const events = await api('GET', `/issues/${ticket.id}/timeline`);
check(events.some((e) => e.event_type === 'changes_requested' && e.payload.to === 'coders'), 'the timeline records the request');

// 8. Merge with push.
await openTicket('review');
await reviewTab.waitFor({ timeout: 30_000 });
await page.waitForTimeout(1500);
await page.getByTestId('review-push').check();
await page.getByTestId('review-merge').click();
const notice = await page.getByTestId('review-notice').innerText({ timeout: 60_000 });
check(notice.includes('Merged 1 repo') && notice.includes('pushed'), `merge notice: ${notice}`);
await shot('r03-merged');
check((await ticketStatus()) === statusOf('done').id, 'the ticket is Done');
const mainLog = sh('git log --format=%s main', repoDir);
check(mainLog.split('\n')[0].startsWith('REV-1: Order checkout'), `main's top commit is the ticket (${mainLog.split('\n')[0]})`);
check(sh('git show --name-only --format= main', repoDir).includes('src/route.rs'), 'main contains the work');
check(sh('git cat-file -p main:src/route.rs', repoDir).includes('validated before saving'), 'main contains the review fix');
check(sh(`git --git-dir=${remoteDir} rev-parse main`) === sh('git rev-parse main', repoDir), 'the remote main was pushed');
const merged = await review();
check(merged.changelog.some((e) => e.kind === 'merge' && e.commit_refs.length === 1), 'a merge entry records the commit');
check(merged.merged && merged.blockers.length === 0, `Review says it is merged (${merged.merged}, ${merged.blockers})`);

// 9. Exports.
const ticketMd = await raw(`/issues/${ticket.id}/changelog.md`);
check(ticketMd.status === 200, 'ticket export responds');
check((ticketMd.headers.get('content-disposition') ?? '').includes('REV-1-changelog.md'), `ticket export file name (${ticketMd.headers.get('content-disposition')})`);
check(ticketMd.text.startsWith('# REV-1: Order checkout'), 'ticket export title');
for (const part of ['### Coder · Subtask 1', '### Tester', '### Fix', '### Merge · Merged into main', '#### What changed', 'Review feedback: Validate orders']) {
  check(ticketMd.text.includes(part), `ticket export contains "${part}"`);
}
const projectMd = await raw(`/projects/${project.id}/changelog.md`);
check(projectMd.text.startsWith(`# Changelog: ${projectName}`), 'project export title');
check(projectMd.text.includes('## REV-1: Order checkout') && projectMd.text.includes('#### Merge'), 'project export nests the ticket');

await openTicket('review');
await reviewTab.waitFor({ timeout: 30_000 });
const [download] = await Promise.all([
  page.waitForEvent('download', { timeout: 15_000 }),
  page.getByTestId('review-export').click(),
]);
check(download.suggestedFilename() === 'REV-1-changelog.md', `UI export downloads ${download.suggestedFilename()}`);
const [projectDownload] = await Promise.all([
  page.waitForEvent('download', { timeout: 15_000 }),
  page.getByTestId('project-changelog-export').click(),
]);
check(projectDownload.suggestedFilename().endsWith('-changelog.md'), `project export downloads ${projectDownload.suggestedFilename()}`);
await shot('r04-done');

console.log(`browser errors: ${errors.length ? errors.join('\n  ') : 'none'}`);
await browser.close();
