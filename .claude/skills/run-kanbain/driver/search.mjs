// Drive ticket search (Phase 3c) with a real embedder (TEI on the host):
// 30 seeded tickets, a paraphrase query, an id by keyword, a near-duplicate
// at intake, partial re-embedding after an edit, and a model switch
// (blue/green). Then the UI: Settings → Search, Related, the command bar's
// ticket search and possible duplicates in the intake review.
// Usage: node search.mjs <frontendUrl> <outDir> <backendPort> <token>
//          <embeddingsDb> <teiA> <modelA> <teiB> <modelB>
import { execFileSync } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import { chromium } from 'playwright';

const [base, outDir, port, token, embeddingsDb, teiA, modelA, teiB, modelB] =
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
  return json.data;
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sql = (query) =>
  execFileSync('python3', ['-c', `import sqlite3,sys;c=sqlite3.connect(sys.argv[1]);print(c.execute(sys.argv[2]).fetchone()[0])`, embeddingsDb, query], { encoding: 'utf8' }).trim();
const BGE_QUERY = 'Represent this sentence for searching relevant passages: ';
const embedder = (url, model) => ({
  provider: 'tei',
  base_url: url,
  model,
  dimensions: null,
  query_prefix: BGE_QUERY,
  document_prefix: '',
  batch_size: 32,
  max_input_tokens: 512,
  timeout_secs: 60,
});

async function setSearch(text) {
  const info = await api('GET', '/info');
  await api('PUT', '/config', { ...info.config, search: { ...info.config.search, text } });
}
async function waitIndexed(model, label) {
  for (let i = 0; i < 300; i++) {
    const s = await api('GET', '/embeddings/status');
    const active = s.spaces.find((x) => x.space.state === 'active' && x.space.model === model);
    if (active && active.embedded === active.chunks && s.jobs.queued + s.jobs.running === 0) {
      console.log(`${label}: ${active.embedded}/${active.chunks} chunks in space ${active.space.id} (${active.space.dimension} dims)`);
      return s;
    }
    await sleep(1000);
  }
  throw new Error(`${label}: index not ready`);
}

// UI_PROJECT=<name> skips the API part and drives the UI against a
// project seeded by an earlier run (saves running both embedders at once).
const uiOnly = process.env.UI_PROJECT;
let suffix = Date.now().toString().slice(-5);
if (uiOnly) {
  suffix = uiOnly.replace(/^Search /, '');
} else {
// 1. Text embedder A and 30 tickets.
const test = await api('POST', '/search/test-embedder', { config: embedder(teiA, modelA), slot: 'text' });
check(test.ok && test.dimension === 384, `test connection: ${modelA} ${test.dimension} dims ${test.latency_ms} ms`);
await setSearch(embedder(teiA, modelA));

const project = await api('POST', '/projects', { name: `Search ${suffix}`, id_prefix: 'SRC' });
const tickets = [
  ['Password reset email never arrives', 'Customers who click "forgot password" wait for the email but it never shows up in their inbox.'],
  ['Checkout total ignores coupons', 'The cart total stays the same after a valid coupon code is applied.', 'PROJ-123'],
  ['Dark mode for the settings page', 'Settings should follow the operating system colour scheme.'],
  ['Export invoices as PDF', 'Accountants need a PDF of each invoice for their records.'],
  ['Slow search on the product list', 'Typing in the product filter takes seconds with 10,000 products.'],
  ['Profile photo upload fails for PNG', 'Uploading a PNG avatar returns a 500 error; JPEG works.'],
  ['Add two-factor authentication', 'Users want to protect their account with an authenticator app.'],
  ['Newsletter unsubscribe link broken', 'The link at the bottom of the newsletter leads to a 404 page.'],
  ['Shipping cost wrong for Austria', 'Orders to Austria are charged the German shipping rate.'],
  ['Session expires too quickly', 'People are logged out after five minutes of inactivity.'],
  ['CSV import drops umlauts', 'Importing a CSV file with ä, ö and ü replaces them with question marks.'],
  ['Order confirmation shows wrong currency', 'Swiss customers see euros on the confirmation page.'],
  ['Add product reviews', 'Shoppers want to rate products and write short reviews.'],
  ['Admin dashboard loads slowly', 'The admin start page needs 12 seconds to render the charts.'],
  ['Mobile menu does not close', 'On phones the navigation menu stays open after choosing a page.'],
  ['Wishlist items disappear', 'Items saved to the wishlist are gone after logging in again.'],
  ['Allow guest checkout', 'Buyers should be able to order without creating an account.'],
  ['Inventory count goes negative', 'Two parallel orders can buy the last item twice.'],
  ['Add sitemap.xml', 'Search engines need a sitemap of all product pages.'],
  ['Cookie banner covers the buy button', 'On small screens the consent banner hides the add-to-cart button.'],
  ['Translate the shop into French', 'All storefront texts need a French version.'],
  ['Gift cards cannot be combined', 'Only one gift card can be used per order; customers want two.'],
  ['Return label generation', 'Customers should print a return label from their order history.'],
  ['Search suggestions show sold-out items', 'Autocomplete lists products that are no longer available.'],
  ['Database backups fail at night', 'The nightly backup job stops with a disk full error.'],
  ['Customer support chat widget', 'Add a chat window so customers can reach support.'],
  ['Price rounding errors in tax', 'VAT is off by one cent for some baskets.'],
  ['Login with Google', 'Offer sign in with a Google account next to email and password.'],
  ['Order history pagination', 'Customers with many orders cannot see older ones.'],
  ['Email change requires confirmation', 'Changing the account email should send a confirmation to the new address.'],
];
let reset, coupons, long;
for (const [title, description, external] of tickets) {
  const issue = await api('POST', `/projects/${project.id}/issues`, {
    title,
    description,
    ...(external ? { external_id: external } : {}),
  });
  if (title.startsWith('Password reset')) reset = issue;
  if (external) coupons = issue;
  if (title.startsWith('Database backups')) long = issue;
}
// One long ticket with several chunks, for the re-embedding check.
const sections = ['Context', 'Steps to reproduce', 'Expected', 'Notes'].map(
  (h, i) => `## ${h}\n\n${`Backup detail ${i} `.repeat(70)}`
);
long = await api('PUT', `/issues/${long.id}`, { description: sections.join('\n\n') });
await waitIndexed(modelA, 'indexed with A');

// 2. A paraphrase finds the reset ticket; an id is found by keyword.
const search = (query, extra = {}) =>
  api('POST', '/search', { query, project_id: project.id, limit: 5, ...extra });
const paraphrase = await search('users cannot reset their password');
const top = paraphrase.hits.map((h) => paraphrase.tickets[h.issue_id]?.title);
console.log('  "users cannot reset their password" →', top.slice(0, 3));
check(paraphrase.semantic, 'semantic search used');
check(paraphrase.hits[0]?.issue_id === reset.id, 'paraphrase finds "Password reset email never arrives" first');
const byId = await search('PROJ-123');
check(byId.hits[0]?.issue_id === coupons.id && byId.hits[0]?.keyword_rank === 1, 'PROJ-123 found by keyword');

// 3. Editing one section re-embeds only that chunk.
const calls = () => Number(sql("SELECT COALESCE(SUM(inputs),0) FROM embedding_calls WHERE kind='document'"));
const chunksOfLong = Number(sql(`SELECT COUNT(*) FROM chunks c JOIN sources s ON s.id=c.source_id WHERE s.kind='ticket' AND s.ref_id='${long.id}'`));
const before = calls();
sections[3] = '## Notes\n\nThe backup disk is full because old dumps are never deleted.';
await api('PUT', `/issues/${long.id}`, { description: sections.join('\n\n') });
await sleep(2500);
await waitIndexed(modelA, 'after the edit');
check(chunksOfLong >= 3, `long ticket has ${chunksOfLong} chunks`);
check(calls() - before === 1, `edit re-embedded ${calls() - before} chunk(s)`);

// 4. Switching the model builds a new space while search keeps working.
await setSearch(embedder(teiB, modelB));
let building;
for (let i = 0; i < 60 && !building; i++) {
  await sleep(500);
  const s = await api('GET', '/embeddings/status');
  building = s.spaces.find((x) => x.space.state === 'building' && x.space.model === modelB);
}
check(!!building, `building space for ${modelB} (${building?.space.dimension} dims)`);
const during = await search('users cannot reset their password');
check(during.semantic && during.hits[0]?.issue_id === reset.id, 'search keeps using the active space while building');
const after = await waitIndexed(modelB, 'indexed with B');
check(after.spaces.some((x) => x.space.model === modelA && x.space.state === 'retired'), 'old space retired');
const withB = await search('users cannot reset their password');
check(withB.hits[0]?.issue_id === reset.id, 'model B finds the reset ticket first too');

// 5. Related tickets.
const related = await api('GET', `/issues/${reset.id}/related`);
console.log('  related to the reset ticket →', related.map((r) => r.ticket.title).slice(0, 3));
check(related.length > 0 && related.every((r) => r.ticket.id !== reset.id), 'related leaves out the ticket itself');
}

// 6. The UI.
const browser = await chromium.launch({ args: ['--no-sandbox'] });
const page = await (await browser.newContext({ viewport: { width: 1600, height: 1100 } })).newPage();
// Panel sections fold; open one (by its test id) before reading inside it.
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
await page.goto(`${base}/tickets`, { timeout: 180_000 });
await page.waitForTimeout(3_000);
await page.getByRole('combobox', { name: 'Project' }).selectOption({ label: `Search ${suffix}` });
await page.waitForTimeout(1_500);

// Related in the ticket panel.
await page.getByText('Password reset email never arrives').first().click();
const relatedSection = page.getByTestId('ticket-related');
await relatedSection.waitFor({ timeout: 15_000 });
await unfold('ticket-related');
await relatedSection.scrollIntoViewIfNeeded();
check((await relatedSection.getByTestId('search-result').count()) > 0, 'Related section lists tickets');
await shot('s01-related');

// The command bar offers a ticket search for what was typed.
await page.keyboard.press('Escape');
await page.keyboard.press('Control+k');
await page.waitForTimeout(500);
await page.keyboard.type('users cannot reset their password');
const offer = page.getByText('Search tickets for “users cannot reset their password”');
check(await offer.isVisible({ timeout: 5_000 }).catch(() => false), 'command bar offers the ticket search');
await shot('s02-command-bar');
await offer.click();
const firstResult = page.getByTestId('ticket-search-results').getByTestId('search-result').first();
await firstResult.waitFor({ timeout: 30_000 });
check((await firstResult.textContent()).includes('Password reset email never arrives'), 'semantic search dialog finds the reset ticket');
await shot('s03-search-dialog');
await firstResult.click();
await page.getByLabel('Ticket title', { exact: true }).waitFor({ timeout: 15_000 });
check(
  (await page.getByLabel('Ticket title', { exact: true }).inputValue()) === 'Password reset email never arrives',
  'a result opens its ticket'
);
await page.keyboard.press('Escape');

// Possible duplicates in the intake review.
await page.getByRole('button', { name: 'Drop ticket' }).click();
await page.getByLabel('Ticket text').fill('Forgot-password mail is not delivered\n\nWhen customers ask for a new password, no message reaches them.');
await page.getByRole('button', { name: 'Enhance' }).click();
const duplicates = page.getByTestId('possible-duplicates');
await duplicates.waitFor({ timeout: 120_000 }).catch(async (e) => {
  await shot('s-fail-duplicates');
  throw e;
});
check(
  (await duplicates.getByTestId('search-result').first().textContent()).includes('Password reset email never arrives'),
  'near-duplicate is listed first'
);
await duplicates.getByTestId('search-result').first().click();
await page.getByRole('button', { name: /^Update SRC-1$/ }).waitFor({ timeout: 10_000 });
check(true, 'picking the duplicate offers "Update SRC-1"');
await shot('s04-duplicates');
await page.getByRole('button', { name: 'Discard' }).click();

// Settings → Search.
await page.keyboard.press('Control+k');
await page.keyboard.type('Settings');
await page.getByRole('option', { name: /^Settings/ }).click();
await page.waitForTimeout(1_500);
await shot('s05a-settings');
await page.getByRole('button', { name: 'Search', exact: true }).click();
const status = page.getByTestId('search-index-status');
await status.waitFor({ timeout: 15_000 });
await page.getByRole('button', { name: 'Test connection' }).first().click();
const tested = page.getByTestId('search-test').first();
await tested.waitFor({ timeout: 30_000 });
check((await tested.textContent()).includes('768 dimensions'), `settings test connection: ${await tested.textContent()}`);
check((await status.textContent()).includes(modelB), 'index status shows the active space');
await shot('s05-settings-search');

console.log(errors.length ? `ERRORS:\n${errors.join('\n')}` : 'No console errors or failed API calls');
if (errors.length) process.exitCode = 1;
await browser.close();
