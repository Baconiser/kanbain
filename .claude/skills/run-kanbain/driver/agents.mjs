// Agent library UI check (Phase 2): Settings → Agents and Sandbox, and the
// agent picker in the create-workspace chat box.
// Usage: node agents.mjs <baseUrl> <shotsDir> <backendPort> <token> [skillZip]
// Prints PASS/FAIL lines, console errors and failed API calls.
import { mkdirSync } from 'node:fs';
import { chromium } from 'playwright';

const [base, shots, backendPort, token, skillZip] = process.argv.slice(2);
mkdirSync(shots, { recursive: true });
const apiBase = `http://127.0.0.1:${backendPort}/api`;
const api = async (path) => {
  const res = await fetch(apiBase + path, {
    headers: { 'x-kanbain-token': token },
  });
  return (await res.json()).data;
};

let failures = 0;
const check = (ok, label) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`);
  if (!ok) failures += 1;
};

const browser = await chromium.launch({ args: ['--no-sandbox'] });
const page = await (
  await browser.newContext({ viewport: { width: 1440, height: 1000 } })
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
const shot = (name) => page.screenshot({ path: `${shots}/${name}.png` });
const dialog = () => page.getByRole('dialog').last();

await page.goto(base, { timeout: 180_000 });
const continueButton = page.getByRole('button', { name: /continue/i });
if (await continueButton.isVisible({ timeout: 15_000 }).catch(() => false)) {
  await continueButton.click();
}
await page.waitForURL(/\/workspaces/, { timeout: 60_000 });

async function openSettings(section) {
  await page.getByRole('button', { name: 'Settings' }).first().click();
  await page.getByRole('button', { name: section, exact: true }).click();
  await page.waitForTimeout(1_500);
}

// --- Settings → Agents ---------------------------------------------------
await openSettings('Agents');
let text = await page.locator('body').innerText();
for (const name of ['planner', 'coder', 'tester', 'scribe', 'intake', 'history']) {
  check(text.includes(name), `default agent ${name} listed`);
}
check(text.includes('Style blocks') && text.includes('Skills'), 'styles and skills cards');
await shot('agents-01-list');

// New style block.
await page.getByRole('button', { name: 'New style block' }).click();
const styleName = `ui-style-${Date.now() % 100000}`;
await page.getByPlaceholder('concise-commits').fill(styleName);
await page.locator('textarea').last().fill('UI-STYLE-MARKER-V1: keep answers short.');
await page.getByRole('button', { name: 'Save' }).last().click();
await page.waitForTimeout(1_500);
let styles = await api('/agent-library/styles');
check(styles.some((s) => s.name === styleName), `style ${styleName} saved`);

// Attach it to the coder agent.
await page.getByRole('button', { name: /^coder/ }).first().click();
await page.getByRole('button', { name: styleName, exact: true }).click();
await page.getByRole('button', { name: 'Save', exact: true }).first().click();
await page.waitForTimeout(1_500);
const coder = (await api('/agent-library/agents')).find((a) => a.name === 'coder');
check(coder.definition.styles.includes(styleName), 'style attached to coder');
await shot('agents-02-coder-editor');

// Editing the style shows who uses it and changes it for them. It stays
// open after saving; open it only if it was closed.
text = await page.locator('body').innerText();
if (!text.includes('Changes apply to')) {
  await page.getByRole('button', { name: new RegExp(`^${styleName}`) }).first().click();
  text = await page.locator('body').innerText();
}
check(text.includes('Changes apply to coder'), 'style editor lists its users');
await page.locator('textarea').last().fill('UI-STYLE-MARKER-V2: keep answers short.');
await page.getByRole('button', { name: 'Save' }).last().click();
await page.waitForTimeout(1_500);
styles = await api('/agent-library/styles');
check(
  styles.find((s) => s.name === styleName)?.body.startsWith('UI-STYLE-MARKER-V2'),
  'style text updated'
);

// New skill, then edit its SKILL.md.
await page.getByRole('button', { name: 'New skill' }).click();
const skillName = `ui-skill-${Date.now() % 100000}`;
await page.getByPlaceholder('pdf').fill(skillName);
await page
  .locator('input:not([type=file]):visible')
  .last()
  .fill('Use for UI demo questions.');
await page.getByRole('button', { name: 'Create skill' }).click();
await page.waitForTimeout(2_000);
const skillText = page.locator('textarea').last();
await skillText.waitFor();
const initial = await skillText.inputValue();
check(initial.includes(`name: ${skillName}`), 'new skill has SKILL.md frontmatter');
await skillText.fill(`${initial}\nSay UI-SKILL-MARKER when used.\n`);
await page.getByRole('button', { name: 'Save' }).last().click();
await page.waitForTimeout(1_500);
const skills = await api('/agent-library/skills');
check(skills.some((s) => s.name === skillName && !s.error), `skill ${skillName} saved`);
await shot('agents-03-skill-editor');

// Import a skill zip through the hidden file input.
if (skillZip) {
  await page.locator('input[type=file][accept]').setInputFiles(skillZip);
  await page.getByText('Import finished').waitFor({ timeout: 15_000 });
  const after = await api('/agent-library/skills');
  check(after.some((s) => s.name === 'zipped-skill'), 'zip import added zipped-skill');
  await shot('agents-04-imported');
}

// Test run: opens a workspace on the scratch repo. The coder editor may
// still be open from above.
const testRun = page.getByRole('button', { name: 'Test run', exact: true });
if (!(await testRun.isVisible().catch(() => false))) {
  await page.getByRole('button', { name: /^coder/ }).first().click();
}
await shot('agents-05a-before-test-run');
await testRun.click();
await page.waitForURL(/\/workspaces\/[0-9a-f-]{36}/, { timeout: 120_000 });
check(true, `test run opened ${page.url().split('/').pop()}`);
await page.waitForTimeout(15_000);
await shot('agents-05-test-run');
const workspaceId = page.url().match(/workspaces\/([0-9a-f-]{36})/)[1];
const workspace = await api(`/workspaces/${workspaceId}`);
check(workspace.name?.startsWith('Test run: coder'), 'test-run workspace named after agent');

// --- Create chat box: agent picker ----------------------------------------
await page.goto(`${base}/workspaces/create`);
await page.waitForTimeout(3_000);
text = await page.locator('body').innerText();
const pickerVisible = text.includes('No agent definition') || /\bcoder\b/.test(text);
if (!pickerVisible) {
  // The repo step comes first; pick the scratch repo if offered.
  const repoButton = page.getByText('agent-test').first();
  if (await repoButton.isVisible().catch(() => false)) {
    await repoButton.click();
    const cont = page.getByRole('button', { name: /continue/i }).first();
    if (await cont.isVisible().catch(() => false)) await cont.click();
    await page.waitForTimeout(2_000);
  }
}
// The picker shows "No agent definition" or the agent the draft remembers.
const agentNames = (await api('/agent-library/agents')).map((a) => a.name);
const picker = page
  .getByRole('button', {
    name: new RegExp(`^(No agent definition|${agentNames.join('|')})$`),
  })
  .last();
if (await picker.isVisible().catch(() => false)) {
  await picker.click();
  await page.getByRole('menuitem', { name: /^coder\s*Claude Code/ }).click();
  await page.waitForTimeout(1_000);
  text = await page.locator('body').innerText();
  check(
    text.includes('Claude Code') && !text.includes('No agent definition'),
    'picking coder selects Claude Code'
  );
  await shot('agents-06-chatbox-picker');
} else {
  check(false, 'agent picker visible in chat box');
  await shot('agents-06-chatbox-missing');
}

// --- Settings → Sandbox: Claude account ----------------------------------
await page.goto(`${base}/workspaces`);
await openSettings('Sandbox');
await page.getByRole('button', { name: 'Connect Claude account' }).click();
await page.getByRole('link', { name: 'Sign in to Claude' }).waitFor({ timeout: 90_000 });
const href = await page.getByRole('link', { name: 'Sign in to Claude' }).getAttribute('href');
check(href?.startsWith('https://claude.com/cai/oauth/authorize?'), 'sign-in link shown');
await shot('agents-07-claude-login');
await page.getByRole('button', { name: 'Cancel' }).last().click();
await page.waitForTimeout(1_500);
text = await page.locator('body').innerText();
check(text.includes('Connect Claude account'), 'login cancelled');

console.log(`console errors (${consoleErrors.length}):`);
for (const e of consoleErrors) console.log(`  ${e}`);
console.log(`failed API calls (${failedApi.length}):`);
for (const f of failedApi) console.log(`  ${f}`);
console.log(failures === 0 ? 'ALL PASS' : `${failures} FAILED`);
await browser.close();
process.exit(failures === 0 ? 0 : 1);
