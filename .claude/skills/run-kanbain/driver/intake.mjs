// Drive ticket intake (Phase 3a): drop a Jira ticket with a PDF, review the
// enhanced version, create it, drop it again to add description version 2,
// and check the attachment text, the diff and the timeline.
// Needs the sandbox and the mock upstream with mock-script-intake.json.
// Usage: node intake.mjs [baseUrl] [outDir] [pdfPath]
import { mkdirSync } from 'node:fs';
import { chromium } from 'playwright';

const base = process.argv[2] ?? 'http://localhost:3000';
const outDir = process.argv[3] ?? 'screenshots';
const pdfPath = process.argv[4];
mkdirSync(outDir, { recursive: true });

const browser = await chromium.launch({ args: ['--no-sandbox'] });
const context = await browser.newContext({
  viewport: { width: 1600, height: 1000 },
});
const errors = [];
const page = await context.newPage();
page.on('console', (msg) => {
  if (msg.type() === 'error') errors.push(msg.text());
});
page.on('pageerror', (err) => errors.push(`pageerror: ${err.message}`));
page.on('response', (res) => {
  if (res.url().includes('/api/') && res.status() >= 400) {
    errors.push(`${res.status()} ${res.request().method()} ${res.url()}`);
  }
});
const shot = async (name) => {
  await page.screenshot({ path: `${outDir}/${name}.png` });
  console.log(`screenshot: ${outDir}/${name}.png`);
};
const check = (cond, what) => {
  console.log(`${cond ? 'PASS' : 'FAIL'}: ${what}`);
  if (!cond) process.exitCode = 1;
};

await page.goto(base, { timeout: 180_000 });
const continueButton = page.getByRole('button', { name: /continue/i });
if (await continueButton.isVisible({ timeout: 15_000 }).catch(() => false)) {
  await continueButton.click();
}
await page.goto(`${base}/tickets`, { timeout: 180_000 });
await page.waitForTimeout(3_000);

// A fresh project.
const suffix = Date.now().toString().slice(-5);
const firstProject = page.getByRole('button', { name: 'New project' });
if (await firstProject.isVisible().catch(() => false)) {
  await firstProject.click();
} else {
  await page.getByRole('button', { name: 'Project', exact: true }).click();
}
await page.getByLabel('Name', { exact: true }).fill(`Intake ${suffix}`);
await page.getByLabel('Ticket prefix').fill('KAN');
await page.getByRole('button', { name: 'Create project' }).click();
await page.getByRole('combobox', { name: 'Project' }).waitFor();
await page.waitForTimeout(1_000);

const jira = [
  'h2. [PROJEKTNAME-123] Password reset mail never arrives',
  '',
  'When a user clicks *Forgot password*, no mail is sent.',
  '',
  '* Steps: open login, click forgot password',
  '* Expected: mail within a minute',
].join('\n');

async function waitForReview() {
  await page
    .getByLabel('Title', { exact: true })
    .waitFor({ timeout: 240_000 })
    .catch(async (err) => {
      await shot('i-fail-review');
      throw err;
    });
}

// 1. Drop the ticket.
await page.getByRole('button', { name: 'Drop ticket' }).click();
await page.getByLabel('Ticket text').fill(jira);
await shot('i01-drop-dialog');
await page.getByRole('button', { name: 'Enhance' }).click();
// The mock answers fast; the enhancing state may already be gone.
console.log(
  `enhancing state seen: ${await page.getByTestId('intake-enhancing').isVisible({ timeout: 3_000 }).catch(() => false)}`
);
await waitForReview();
check(
  (await page.getByTestId('intake-original').textContent()).includes('h2. [PROJEKTNAME-123]'),
  'original Jira text shown unchanged'
);
check(
  (await page.getByLabel('External ID', { exact: true }).inputValue()) === 'PROJEKTNAME-123',
  'External ID detected'
);
check(
  (await page.getByLabel('Acceptance criteria (one per line)').inputValue()).includes(
    'arrives within one minute'
  ),
  'acceptance criteria from the agent'
);
check(
  /intake · haiku · \d+ tokens/.test(await page.getByTestId('intake-job').textContent()),
  'agent, model and tokens shown'
);
await page.getByLabel('Title', { exact: true }).fill('Password reset email never arrives (edited)');
await shot('i02-review');
await page.getByTestId('intake-accept').click();
await page.getByLabel('Ticket title', { exact: true }).waitFor({ timeout: 15_000 });
await page.waitForTimeout(1_500);
check(
  (await page.getByLabel('Ticket title', { exact: true }).inputValue()).endsWith('(edited)'),
  'ticket created with the edited title'
);
await page.getByTestId('ticket-tab-activity').click();
const timeline = page.getByTestId('ticket-timeline');
await timeline.scrollIntoViewIfNeeded();
const timelineText = async () => (await timeline.textContent()) ?? '';
check((await timelineText()).includes('Created in Backlog'), 'timeline: created');
check(
  (await timelineText()).includes('Description version 1 (edited)'),
  'timeline: version 1 is the edited one'
);
check(
  /structured by intake \(haiku, \d+ tokens, \$/.test(await timelineText()),
  'timeline: intake with agent cost'
);
await shot('i03-ticket-created');

// 2. Drop it again with a PDF: update the existing ticket.
await page.keyboard.press('Escape');
await page.getByRole('button', { name: 'Drop ticket' }).click();
await page.waitForTimeout(500);
await shot('i03b-second-drop');
await page
  .getByLabel('Ticket text')
  .fill('[PROJEKTNAME-123] Password reset mail never arrives (see attached update)');
if (pdfPath) {
  await page.getByLabel('Ticket files').setInputFiles(pdfPath);
}
await page.getByRole('button', { name: 'Enhance' }).click();
await waitForReview();
check(await page.getByTestId('existing-ticket').isVisible(), 'existing ticket offered');
check(
  (await page.getByTestId('existing-ticket').textContent()).includes('KAN-1'),
  'existing ticket is KAN-1'
);
const diff = page.getByTestId('text-diff');
await diff.waitFor({ timeout: 10_000 }).catch(async (err) => {
  await shot('i-fail-diff');
  throw err;
});
check(
  (await diff.locator('[data-kind="insert"]').allTextContents()).some((t) =>
    t.includes('expires after 30 minutes')
  ),
  'diff shows the new acceptance criterion'
);
if (pdfPath) {
  await page.getByRole('button', { name: /Show text \(pdf\)/ }).click();
  check(
    (await page.getByTestId('attachment-text').first().textContent()).includes(
      'expire after 30 minutes'
    ),
    'PDF text extracted in the review'
  );
}
await shot('i04-update-review');
await page.getByRole('button', { name: 'Update KAN-1' }).click();
await page.waitForTimeout(2_000);
await page.getByText(/Password reset email never arrives/).first().click();
await page.getByLabel('Ticket title', { exact: true }).waitFor({ timeout: 15_000 });
await page.waitForTimeout(1_500);
await timeline.scrollIntoViewIfNeeded();
check(
  (await timelineText()).includes('Description version 2 (enhanced)'),
  'timeline: version 2 from the second drop'
);
check(
  (await page.getByLabel('Ticket title', { exact: true }).inputValue()) ===
    'Password reset email never arrives',
  'title updated by the new version'
);
await timeline.getByRole('button', { name: 'Show changes' }).last().click();
await timeline.getByTestId('text-diff').waitFor({ timeout: 10_000 });
check(
  (await timeline.getByTestId('text-diff').locator('[data-kind="insert"]').allTextContents()).some(
    (t) => t.includes('expires after 30 minutes')
  ),
  'timeline diff between versions 1 and 2'
);
if (pdfPath) {
  const attachments = page.getByTestId('ticket-attachments');
  await attachments.scrollIntoViewIfNeeded();
  await attachments.getByRole('button', { name: /Show text/ }).click();
  check(
    (await attachments.getByTestId('attachment-text').textContent()).includes('30 minutes'),
    'PDF text shown on the ticket'
  );
  check((await timelineText()).includes('Attached PROJEKTNAME-123-update.pdf'), 'timeline: attachment');
}
await shot('i05-timeline');

// 3. The manual create dialog warns about the existing External ID.
await page.keyboard.press('Escape');
await page.getByRole('button', { name: 'New ticket' }).click();
await page.locator('#ticket-title').fill('Another PROJEKTNAME-123 report');
await page.locator('#ticket-description').click();
await page.waitForTimeout(800);
check(
  (await page.locator('#ticket-external-id').inputValue()) === 'PROJEKTNAME-123',
  'create dialog fills the External ID from the title'
);
await page.getByRole('button', { name: 'Create ticket' }).click();
check(
  await page.getByTestId('duplicate-external-id').isVisible({ timeout: 5_000 }).catch(() => false),
  'create dialog warns about the existing External ID'
);
await shot('i06-duplicate-warning');
await page.getByRole('button', { name: 'Cancel' }).click();

console.log(errors.length ? `ERRORS:\n${errors.join('\n')}` : 'No console errors or failed API calls');
if (errors.length) process.exitCode = 1;
await browser.close();
