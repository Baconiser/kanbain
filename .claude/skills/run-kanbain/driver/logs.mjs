// Settings → Logs: the app's warnings and errors, live.
// Needs a backend that has logged something (a fresh dev container always
// warns about the missing embedder). Checks the page lists the newest entry
// with its level and shows where the log file is.
// Usage: node logs.mjs <frontendUrl> <outDir> <backendPort> <token>
import { mkdirSync } from 'node:fs';
import { chromium } from 'playwright';

const [base, outDir, port, token] = process.argv.slice(2);
mkdirSync(outDir, { recursive: true });
const check = (cond, what) => {
  console.log(`${cond ? 'PASS' : 'FAIL'}: ${what}`);
  if (!cond) process.exitCode = 1;
};
const api = async (path) => {
  const res = await fetch(`http://127.0.0.1:${port}/api${path}`, {
    headers: { 'x-kanbain-token': token },
  });
  return (await res.json()).data;
};

const before = await api('/logs?warnings=true');
const browser = await chromium.launch({ args: ['--no-sandbox'] });
const page = await (await browser.newContext({ viewport: { width: 1500, height: 1000 } })).newPage();
const errors = [];
page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
page.on('response', (r) => {
  if (r.url().includes('/api/logs') && r.status() >= 400) errors.push(`${r.status()} ${r.url()}`);
});
await page.goto(`${base}/workspaces`, { timeout: 180_000 });
await page.waitForTimeout(3_000);
await page.getByRole('button', { name: 'Settings' }).first().click();
await page.getByText('Logs', { exact: true }).first().click();
const log = page.getByTestId('app-log');
await log.waitFor({ timeout: 15_000 });
await page.waitForTimeout(1_500);
const text = await log.textContent();
const last = before.entries.at(-1);
check(before.entries.length > 0, `the backend has ${before.entries.length} warnings/errors`);
check(!last || text.includes(last.message.slice(0, 60)), 'the page lists the newest entry');
check(text.includes(' WARN ') || text.includes('ERROR'), 'lines carry their level');
const file = await page.getByTestId('app-log-file').textContent();
check(file.endsWith('kanbain.log') && !file.includes('..'), `log file shown: ${file}`);
await page.screenshot({ path: `${outDir}/logs.png` });
console.log(`screenshot: ${outDir}/logs.png`);
check(errors.length === 0, errors.length ? errors.join('\n') : 'no page errors or failed /api/logs calls');
await browser.close();
