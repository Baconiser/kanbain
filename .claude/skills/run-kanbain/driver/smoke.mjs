// Smoke-drive a running KanbAIn dev server with headless Chromium.
// Usage: node smoke.mjs [baseUrl] [outDir]
import { mkdirSync } from 'node:fs';
import { chromium } from 'playwright';

const base = process.argv[2] ?? 'http://localhost:3000';
const outDir = process.argv[3] ?? 'screenshots';
mkdirSync(outDir, { recursive: true });

const browser = await chromium.launch({ args: ['--no-sandbox'] });
const page = await (
  await browser.newContext({ viewport: { width: 1440, height: 900 } })
).newPage();

const consoleErrors = [];
const failedApi = [];
page.on('console', (msg) => {
  if (msg.type() === 'error') consoleErrors.push(msg.text());
});
page.on('pageerror', (err) => consoleErrors.push(`pageerror: ${err.message}`));
page.on('response', (res) => {
  if (res.url().includes('/api/') && res.status() >= 400) {
    failedApi.push(`${res.status()} ${res.request().method()} ${res.url()}`);
  }
});

async function shot(name) {
  const path = `${outDir}/${name}.png`;
  await page.screenshot({ path });
  console.log(`screenshot: ${path}`);
}

// First paint can take a while: Vite compiles routes on demand.
await page.goto(base, { timeout: 180_000 });
await page.waitForLoadState('domcontentloaded');

// A fresh data folder starts on onboarding; continue through it.
const continueButton = page.getByRole('button', { name: /continue/i });
if (await continueButton.isVisible({ timeout: 20_000 }).catch(() => false)) {
  await shot('01-onboarding');
  await continueButton.click();
}
await page.waitForURL(/\/workspaces/, { timeout: 60_000 });
await page.waitForTimeout(2_000);
await shot('02-workspaces');

await page.goto(`${base}/tickets`);
await page.getByText('Local tickets are coming').waitFor({ timeout: 60_000 });
await shot('03-tickets');

await page.goto(`${base}/workspaces`);
await page.getByRole('button', { name: 'Settings' }).first().click();
await page.getByRole('button', { name: 'Agents' }).click();
await page.waitForTimeout(2_000);
await shot('04-settings-agents');
const settingsText = (await page.locator('body').innerText()).toLowerCase();
for (const agent of ['Claude Code', 'OpenCode', 'Codex', 'Gemini', 'Cursor']) {
  const listed = settingsText.includes(agent.toLowerCase());
  console.log(`settings lists ${agent}: ${listed}`);
}

console.log(`console errors (${consoleErrors.length}):`);
for (const e of consoleErrors) console.log(`  ${e}`);
console.log(`failed API calls (${failedApi.length}):`);
for (const f of failedApi) console.log(`  ${f}`);

await browser.close();
