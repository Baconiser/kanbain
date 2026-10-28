// Sandbox UI check: Settings → Sandbox and the run summary of a workspace.
// Usage: node sandbox.mjs <frontendUrl> <shotsDir> [workspaceId]
import { mkdirSync } from 'node:fs';
import { chromium } from 'playwright';

const [base, shots, workspaceId] = process.argv.slice(2);
mkdirSync(shots, { recursive: true });

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const consoleErrors = [];
const failedApi = [];
page.on('console', (m) => m.type() === 'error' && consoleErrors.push(m.text()));
page.on('response', (r) => {
  if (r.url().includes('/api/') && r.status() >= 400) {
    failedApi.push(`${r.status()} ${r.url()}`);
  }
});

const check = (ok, label) => console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`);

await page.goto(`${base}/workspaces`, { timeout: 180_000 });
await page.waitForTimeout(3000);

await page.getByRole('button', { name: 'Settings' }).first().click();
await page.getByRole('button', { name: 'Sandbox', exact: true }).click();
await page.waitForTimeout(1500);
const settingsText = await page.locator('body').innerText();
check(/Docker .* is available/.test(settingsText), 'Docker shown as available');
check(settingsText.includes('Agent image ready'), 'agent image ready');
check(settingsText.includes('A key is stored.'), 'Anthropic key shown as stored');
check(settingsText.includes('Budget per run (USD)'), 'limits card rendered');
await page.screenshot({ path: `${shots}/sandbox-settings.png` });
await page.keyboard.press('Escape');

if (workspaceId) {
  await page.goto(`${base}/workspaces/${workspaceId}`);
  await page.waitForTimeout(4000);
  // Open the Logs panel, which lists the runs.
  await page.locator('body').click({ position: { x: 700, y: 300 } });
  await page.keyboard.press('v');
  await page.keyboard.press('l');
  await page.waitForTimeout(3000);
  const text = await page.locator('body').innerText();
  check(text.includes('Ran in the sandbox'), 'run summary shown');
  check(/\$0\.0\d/.test(text), 'run cost shown');
  check(text.includes('example.com'), 'blocked host listed');
  await page.screenshot({ path: `${shots}/sandbox-run.png` });
}

console.log('console errors:', consoleErrors.length ? consoleErrors : 'none');
console.log('failed api calls:', failedApi.length ? failedApi : 'none');
await browser.close();
