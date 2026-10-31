// The workspace's right sidebar: Ticket / Git / Terminal tabs.
// Usage: node workspace-ticket.mjs <frontendUrl> <shotsDir> <workspaceId> [subTitle parentTitle]
// The workspace must be linked to a ticket (any coder or planner
// workspace from the Phase 4/5 demos is). For a coder workspace pass its
// sub-ticket's and the parent's titles: the sidebar must open on the
// sub-ticket, walk up to the parent and back, and the board's sub-ticket
// must list the coder workspace.
import { mkdirSync } from 'node:fs';
import { chromium } from 'playwright';

const [base, shots, workspaceId, subTitle, parentTitle] = process.argv.slice(2);
mkdirSync(shots, { recursive: true });

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
const errors = [];
page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
page.on('response', (r) => {
  if (r.url().includes('/api/') && r.status() >= 400) {
    errors.push(`${r.status()} ${r.url()}`);
  }
});

let failed = 0;
const check = (ok, what) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${what}`);
  if (!ok) failed++;
};

await page.goto(`${base}/workspaces/${workspaceId}`, { timeout: 180_000 });
const ticketTab = page.getByTestId('workspace-sidebar-tab-ticket');
await ticketTab.waitFor({ timeout: 60_000 });
// Without model credentials the sandbox banner floats over the sidebar's
// top-right corner; it's not what this checks.
await page.addStyleTag({
  content: '.absolute.top-base.right-base.z-40 { display: none !important; }',
});
await ticketTab.click();
const title = page.getByLabel('Ticket title', { exact: true });
await title.waitFor({ timeout: 30_000 });
check(true, 'ticket tab shows the ticket panel');
check(
  (await page.getByTestId('ticket-tab-overview').count()) > 0,
  'ticket panel tabs render'
);
const ticketTitle = await title.inputValue();
console.log('ticket title:', ticketTitle);
if (subTitle) {
  check(ticketTitle === subTitle, `coder workspace opens its sub-ticket "${subTitle}"`);
  await page.getByLabel('Parent ticket').click();
  await page.waitForTimeout(500);
  check(
    (await title.inputValue()) === parentTitle,
    `parent button shows "${parentTitle}"`
  );
  await page.screenshot({ path: `${shots}/ws-ticket-parent.png` });
  await page.getByLabel("Back to this workspace's ticket").click();
  await page.waitForTimeout(500);
  check((await title.inputValue()) === subTitle, 'back returns to the sub-ticket');
}
await page.screenshot({ path: `${shots}/ws-ticket.png` });

// The current workspace is marked in the ticket's workspace list.
await page.getByTestId('ticket-tab-overview').click();
const wsSection = page.getByRole('button', { name: /^Workspaces/ });
if (await wsSection.count()) {
  const expanded = await wsSection.first().getAttribute('aria-expanded');
  if (expanded === 'false') await wsSection.first().click();
}
check(
  (await page.getByText('This workspace', { exact: true }).count()) > 0,
  'current workspace is marked "This workspace"'
);
await page.screenshot({ path: `${shots}/ws-ticket-overview.png` });

await page.getByTestId('workspace-sidebar-tab-git').click();
await page.waitForTimeout(500);
check(!(await title.isVisible()), 'git tab hides the ticket');
await page.screenshot({ path: `${shots}/ws-git.png` });

const termTab = page.getByTestId('workspace-sidebar-tab-terminal');
if (await termTab.count()) {
  await termTab.click();
  await page.waitForTimeout(1500);
  await page.screenshot({ path: `${shots}/ws-terminal.png` });
  check(true, 'terminal tab opens');
}

// Back to the ticket, then hop to the board.
await ticketTab.click();
await page.getByLabel('Open on board').click();
await page.waitForURL(/\/tickets/, { timeout: 30_000 });
const boardTitle = page.getByLabel('Ticket title', { exact: true });
await boardTitle.waitFor({ timeout: 30_000 });
check(
  (await boardTitle.inputValue()) === ticketTitle,
  'open on board shows the same ticket on the board'
);
if (subTitle) {
  const section = page.getByRole('button', { name: /^Workspaces/ });
  if ((await section.first().getAttribute('aria-expanded')) === 'false') {
    await section.first().click();
  }
  await page.waitForTimeout(1000);
  check(
    !(await page.getByText('Start one from the header').isVisible()),
    "the board's sub-ticket lists its coder workspace"
  );
}
await page.screenshot({ path: `${shots}/board-from-ws.png` });

console.log(errors.length ? `errors:\n${errors.join('\n')}` : 'no errors');
await browser.close();
process.exit(failed ? 1 : 0);
