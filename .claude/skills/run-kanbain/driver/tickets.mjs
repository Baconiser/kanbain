// Drive the local ticket board: create a project and tickets, edit, comment,
// drag between columns, and check live updates in a second window.
// Usage: node tickets.mjs [baseUrl] [outDir]
import { mkdirSync } from 'node:fs';
import { chromium } from 'playwright';

const base = process.argv[2] ?? 'http://localhost:3000';
const outDir = process.argv[3] ?? 'screenshots';
mkdirSync(outDir, { recursive: true });

const browser = await chromium.launch({ args: ['--no-sandbox'] });
const context = await browser.newContext({
  viewport: { width: 1600, height: 1000 },
});
const errors = [];
function watch(page, label) {
  page.on('console', (msg) => {
    if (msg.type() === 'error') errors.push(`${label}: ${msg.text()}`);
  });
  page.on('pageerror', (err) => errors.push(`${label} pageerror: ${err.message}`));
  page.on('response', (res) => {
    if (res.url().includes('/api/') && res.status() >= 400) {
      errors.push(`${label}: ${res.status()} ${res.request().method()} ${res.url()}`);
    }
  });
}

const page = await context.newPage();
watch(page, 'main');
const shot = async (p, name) => {
  await p.screenshot({ path: `${outDir}/${name}.png` });
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

// 1. Create a project.
const suffix = Date.now().toString().slice(-5);
const projectName = `Demo ${suffix}`;
const firstProject = page.getByRole('button', { name: 'New project' });
if (await firstProject.isVisible().catch(() => false)) {
  await firstProject.click();
} else {
  await page.getByRole('button', { name: 'Project', exact: true }).click();
}
await page.getByLabel('Name', { exact: true }).fill(projectName);
await page.getByLabel('Ticket prefix').fill('DEMO');
await page.getByRole('button', { name: 'Create project' }).click();
await page.getByRole('combobox', { name: 'Project' }).waitFor();
await page.waitForTimeout(1_000);
for (const column of ['Backlog', 'Planning', 'In progress', 'Review', 'Blocked']) {
  check(
    await page.getByText(column, { exact: true }).first().isVisible(),
    `column ${column} shown`
  );
}
await shot(page, 't01-empty-board');

// 2. Create a ticket.
await page.getByRole('button', { name: 'New ticket' }).click();
await page.getByLabel('Title', { exact: true }).fill('Login page');
await page.getByLabel('Description', { exact: true }).fill('Users sign in with email.');
await page.getByLabel('External ID', { exact: true }).fill('PROJ-123');
await page.getByRole('button', { name: 'Create ticket' }).click();
await page
  .getByLabel('Ticket title', { exact: true })
  .waitFor({ timeout: 15_000 })
  .catch(async (err) => {
    await shot(page, 't-fail-panel');
    throw err;
  });
check(
  (await page.getByLabel('Ticket title', { exact: true }).inputValue()) === 'Login page',
  'ticket panel opens with the new ticket'
);
check(
  await page.getByText('DEMO-1 · PROJ-123').first().isVisible(),
  'card and panel show internal and External ID'
);

// 3. Edit: tag, sub-ticket, comment, priority.
await page.getByRole('button', { name: 'Edit tags' }).click();
await page.getByLabel('New tag').fill('frontend');
await page.getByLabel('New tag').press('Enter');
await page.keyboard.press('Escape');
await page.getByLabel('New sub-ticket title').fill('Validate email');
await page.getByLabel('New sub-ticket title').press('Enter');
await page.getByTestId('ticket-tab-activity').click();
await page.getByLabel('New comment').fill('First comment');
await page.getByRole('button', { name: 'Comment', exact: true }).click();
await page.getByRole('combobox', { name: 'Priority' }).click();
await page.getByRole('option', { name: 'high' }).click();
await page.waitForTimeout(1_500);
check(await page.getByText('First comment', { exact: true }).isVisible(), 'comment appears');
await page.getByTestId('ticket-tab-overview').click();
check(
  await page.getByText('Validate email').first().isVisible(),
  'sub-ticket appears'
);
await shot(page, 't02-ticket-panel');

// 4. Second window sees live changes.
const second = await context.newPage();
watch(second, 'second');
await second.goto(`${base}/tickets`, { timeout: 180_000 });
await second.getByText('Login page').first().waitFor({ timeout: 60_000 });
const title = page.getByLabel('Ticket title', { exact: true });
await title.fill('Login page (renamed)');
await title.blur();
await second
  .getByText('Login page (renamed)')
  .first()
  .waitFor({ timeout: 10_000 })
  .then(() => check(true, 'rename shows live in second window'))
  .catch(() => check(false, 'rename shows live in second window'));

// 5. Drag the card from Backlog to In progress in the main window.
await page.getByRole('button', { name: 'Close ticket' }).click();
// The sub-ticket's card names its parent too; the parent card comes first.
const card = page
  .locator('[data-rfd-draggable-id]', { hasText: 'Login page (renamed)' })
  .first();
const target = page.locator('[data-rfd-droppable-id]').nth(3);
const from = await card.boundingBox();
const to = await target.boundingBox();
await page.mouse.move(from.x + from.width / 2, from.y + 20);
await page.mouse.down();
await page.mouse.move(from.x + from.width / 2 + 10, from.y + 30, { steps: 5 });
await page.mouse.move(to.x + to.width / 2, to.y + 40, { steps: 25 });
await page.waitForTimeout(300);
await page.mouse.up();
await page.waitForTimeout(1_500);
const inProgressColumn = second.locator('[data-rfd-droppable-id]').nth(3);
check(
  (await inProgressColumn.innerText()).includes('Login page (renamed)'),
  'drag to In progress shows in second window'
);
await shot(second, 't03-second-window-after-drag');

// 6. Reload: everything persisted.
await page.reload();
await page.getByText('Login page (renamed)').first().waitFor({ timeout: 60_000 });
check(
  (await page.locator('[data-rfd-droppable-id]').nth(3).innerText()).includes(
    'Login page (renamed)'
  ),
  'ticket still In progress after reload'
);
// Open the parent from In progress, not its sub-ticket's card in Backlog.
await page
  .locator('[data-rfd-droppable-id]')
  .nth(3)
  .getByText('Login page (renamed)')
  .first()
  .click();
await page.getByTestId('ticket-tab-activity').click();
await page.getByText('First comment', { exact: true }).waitFor({ timeout: 10_000 });
check(true, 'comment persisted after reload');
await shot(page, 't04-after-reload');

console.log(errors.length ? `errors:\n${errors.join('\n')}` : 'no console/API errors');
await browser.close();
