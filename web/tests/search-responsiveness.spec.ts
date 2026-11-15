import { expect, test, type Page, type Route } from '@playwright/test';

const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: 'root:1' };
const file = (name: string) => ({ id: name, parent_id: 'root', name, kind: 'file', size: 3, etag: 'file:1' });
const result = (query: string) => ({ items: [{ entry: file(`${query}.txt`), location: [root] }], next_cursor: null, has_more: false });

async function login(page: Page) {
  await page.route('**/*', route => {
    const path = new URL(route.request().url()).pathname;
    const json = (value: unknown) => route.fulfill({ json: value });
    if (path === '/instance') return json({ name: 'Sylux' });
    if (path === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (path === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'administrator', csrf_token: 'csrf' });
    if (path === '/accounts') return json([{ account_id: 'owner', handle: 'owner', role: 'administrator', enabled: true }]);
    if (path === '/folders/children') return json({ root, items: [], next_cursor: null, has_more: false });
    if (path === '/folders/root/path') return json({ items: [root] });
    if (path === '/folders/root/children') return json({ items: [file('notes.txt')], next_cursor: null, has_more: false });
    if (path === '/trash') return json({ items: [], next_cursor: null, has_more: false });
    return route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.locator('.file-row')).toBeVisible();
}

// Opt-in measurement of the real input/render path. Change only the timer between runs.
test('measure search scheduling', async ({ page }, info) => {
  test.skip(!process.env.SEARCH_MEASURE, 'Run explicitly when comparing scheduling changes.');
  test.setTimeout(90_000);
  await login(page);
  await page.evaluate(() => {
    const measured = window as Window & { searchTimings?: { query: string; start: number; response: number }[] };
    measured.searchTimings = [];
    const original = window.fetch;
    window.fetch = async (...args) => {
      const url = new URL(String(args[0]), location.href);
      const start = performance.now();
      const response = await original(...args);
      if (url.pathname === '/entries/search') measured.searchTimings!.push({ query: url.searchParams.get('q')!, start, response: performance.now() });
      return response;
    };
  });
  const samples: unknown[] = [];
  for (const gap of [60, 100]) for (const apiDelay of [40, 300]) for (let repeat = 0; repeat < 3; repeat++) {
    const requests: { query: string; at: number }[] = [];
    await page.route('**/entries/search?*', async route => {
      const query = new URL(route.request().url()).searchParams.get('q')!;
      requests.push({ query, at: Date.now() });
      await new Promise(resolve => setTimeout(resolve, apiDelay));
      await route.fulfill({ json: result(query) });
    });
    const query = `test${repeat}`;
    const timing = await page.evaluate(async ({ query, gap }) => {
      const input = document.getElementById('file-search') as HTMLInputElement;
      let finalInput = 0;
      let staleRenders = 0;
      const observer = new MutationObserver(() => {
        const row = document.querySelector('.search-row .filename');
        if (row && row.textContent !== `${input.value}.txt`) staleRenders++;
      });
      observer.observe(document.querySelector('.workspace') ?? document.body, { subtree: true, childList: true, characterData: true });
      const finished = new Promise<number>(resolve => {
        const check = new MutationObserver(() => {
          if (document.querySelector('.search-row .filename')?.textContent === `${query}.txt`) { check.disconnect(); resolve(performance.now()); }
        });
        check.observe(document.body, { subtree: true, childList: true, characterData: true });
      });
      for (let i = 1; i <= query.length; i++) {
        input.value = query.slice(0, i);
        finalInput = performance.now();
        input.dispatchEvent(new InputEvent('input', { bubbles: true, data: query[i - 1], inputType: 'insertText' }));
        if (i < query.length) await new Promise(resolve => setTimeout(resolve, gap));
      }
      const rendered = await finished;
      observer.disconnect();
      const timings = (window as Window & { searchTimings?: { query: string; start: number; response: number }[] }).searchTimings!;
      const final = timings.findLast(item => item.query === query)!;
      return { finalKeystrokeToResultsMs: rendered - finalInput, schedulingMs: final.start - finalInput,
        apiMs: final.response - final.start, responseToRenderMs: rendered - final.response, staleRenders };
    }, { query, gap });
    samples.push({ gapMs: gap, apiDelayMs: apiDelay, repeat, requests: requests.length, ...timing });
    await page.getByRole('button', { name: 'Clear search', exact: true }).click();
    await page.unroute('**/entries/search?*');
  }
  console.log(`SEARCH_MEASURE ${JSON.stringify({ delayMs: process.env.SEARCH_MEASURE, samples })}`);
  await info.attach('search-measurements', { body: JSON.stringify(samples, null, 2), contentType: 'application/json' });
});

async function freezeClock(page: Page) {
  await page.clock.install();
  await page.clock.pauseAt(new Date());
}

test('live search coalesces fast typing and dispatches after 120ms', async ({ page }) => {
  await login(page);
  await freezeClock(page);
  const queries: string[] = [];
  await page.route('**/entries/search?*', route => {
    const query = new URL(route.request().url()).searchParams.get('q')!;
    queries.push(query);
    return route.fulfill({ json: result(query) });
  });
  const input = page.getByLabel('Search files and folders');
  for (const value of ['r', 're', 'rep', 'report']) {
    await input.fill(value);
    await page.clock.runFor(100);
    expect(queries).toEqual([]);
  }
  await page.clock.runFor(20);
  await expect(page.getByRole('row', { name: 'report.txt', exact: true })).toBeVisible();
  expect(queries).toEqual(['report']);
});

test('Enter submits immediately once; clear cancels pending search and restores browse', async ({ page }) => {
  await login(page);
  await freezeClock(page);
  let held: Route | undefined;
  let requests = 0;
  await page.route('**/entries/search?*', route => { held = route; requests++; });
  const input = page.getByLabel('Search files and folders');
  await input.fill('report');
  await input.press('Enter');
  await expect.poll(() => requests).toBe(1);
  await expect(page.getByRole('region', { name: 'Search results' })).toHaveAttribute('aria-busy', 'true');
  await input.press('Enter');
  await page.clock.runFor(500);
  expect(requests).toBe(1);
  await held!.fulfill({ json: result('report') });
  await expect(page.getByRole('row', { name: 'report.txt', exact: true })).toBeVisible();
  await input.fill('unsent');
  await page.getByRole('button', { name: 'Clear search', exact: true }).click();
  await expect(input).toBeFocused();
  await expect(input).toHaveValue('');
  await expect(page.getByRole('row', { name: 'notes.txt', exact: true })).toBeVisible();
  await page.clock.runFor(500);
  expect(requests).toBe(1);
  // The native search-input clear path is immediate too.
  await input.fill('another');
  await input.fill('');
  await page.clock.runFor(500);
  expect(requests).toBe(1);
  await expect(page.getByRole('region', { name: 'Search results' })).toHaveCount(0);
});

test('composition cancels pending work and sends only the committed IME value', async ({ page }) => {
  await login(page);
  await freezeClock(page);
  const queries: string[] = [];
  await page.route('**/entries/search?*', route => {
    const query = new URL(route.request().url()).searchParams.get('q')!;
    queries.push(query);
    return route.fulfill({ json: result(query) });
  });
  const input = page.getByLabel('Search files and folders');
  await input.fill('pending');
  await input.dispatchEvent('compositionstart');
  // An IME can temporarily remove all text before producing its next candidate.
  await input.fill('');
  await input.evaluate(element => {
    const input = element as HTMLInputElement;
    input.value = 'に';
    input.dispatchEvent(new InputEvent('input', { bubbles: true, isComposing: true, data: 'に' }));
  });
  await page.clock.runFor(500);
  await input.press('Enter');
  expect(queries).toEqual([]);
  await input.evaluate(element => {
    const input = element as HTMLInputElement;
    input.value = '日本';
    input.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true, data: '日本' }));
    input.dispatchEvent(new InputEvent('input', { bubbles: true, data: '日本' }));
  });
  await page.clock.runFor(120);
  await expect(page.getByRole('row', { name: '日本.txt', exact: true })).toBeVisible();
  expect(queries).toEqual(['日本']);
});

test('late responses cannot replace newer results or a navigated destination', async ({ page }) => {
  await login(page);
  // Ignore transport cancellation so this exercises the response-generation guard.
  await page.evaluate(() => {
    const original = window.fetch;
    const received = window as Window & { searchResponses?: string[] };
    received.searchResponses = [];
    window.fetch = async (input, options) => {
      const search = String(input).startsWith('/entries/search?');
      const response = await original(input, search ? { ...options, signal: undefined } : options);
      if (search) received.searchResponses!.push(new URL(String(input), location.href).searchParams.get('q')!);
      return response;
    };
  });
  const pending = new Map<string, Route>();
  await page.route('**/entries/search?*', route => { pending.set(new URL(route.request().url()).searchParams.get('q')!, route); });
  const input = page.getByLabel('Search files and folders');
  await input.fill('older');
  await input.press('Enter');
  await expect.poll(() => pending.has('older')).toBe(true);
  await input.fill('newer');
  await input.press('Enter');
  await expect.poll(() => pending.has('newer')).toBe(true);
  await pending.get('newer')!.fulfill({ json: result('newer') });
  await expect(page.getByRole('row', { name: 'newer.txt', exact: true })).toBeVisible();
  await pending.get('older')!.fulfill({ json: result('older') });
  await page.waitForFunction(() => (window as Window & { searchResponses?: string[] }).searchResponses?.includes('older'));
  await expect(page.getByRole('row', { name: 'newer.txt', exact: true })).toBeVisible();
  await expect(page.getByRole('row', { name: 'older.txt', exact: true })).toHaveCount(0);
  await input.fill('leaving');
  await input.press('Enter');
  await expect.poll(() => pending.has('leaving')).toBe(true);
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await pending.get('leaving')!.fulfill({ status: 503, json: { code: 'unavailable' } });
  await page.waitForFunction(() => (window as Window & { searchResponses?: string[] }).searchResponses?.includes('leaving'));
  await expect(page.getByText('Trash is empty.', { exact: true })).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await page.getByRole('button', { name: 'Files', exact: true }).click();
  await expect(input).toHaveValue('');
  await expect(page.getByRole('region', { name: 'Search results' })).toHaveCount(0);
});

test('navigation cancels scheduled work; failures retain an immediate retry', async ({ page }) => {
  await login(page);
  await freezeClock(page);
  let requests = 0;
  await page.route('**/entries/search?*', route => { requests++; return route.fulfill({ status: 503, json: { code: 'unavailable' } }); });
  const input = page.getByLabel('Search files and folders');
  await input.fill('unsent');
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await page.clock.runFor(500);
  expect(requests).toBe(0);
  await page.getByRole('button', { name: 'Files', exact: true }).click();
  await input.fill('retry');
  await input.press('Enter');
  await expect(page.getByRole('alert')).toContainText('Could not search files.');
  await page.route('**/entries/search?*', route => route.fulfill({ json: result('retry') }));
  await page.getByRole('button', { name: 'Try again', exact: true }).click();
  await expect(page.getByRole('row', { name: 'retry.txt', exact: true })).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
});

for (const theme of ['light', 'dark'] as const) for (const width of [390, 1280]) {
  test(`search pending and results at ${width}px in ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await login(page);
    let held: Route | undefined;
    await page.route('**/entries/search?*', route => { held = route; });
    const input = page.getByLabel('Search files and folders');
    await input.fill('report');
    await input.press('Enter');
    await expect.poll(() => !!held).toBe(true);
    await expect(input).toBeFocused();
    await expect(page.getByRole('region', { name: 'Search results' })).toHaveAttribute('aria-busy', 'true');
    await page.screenshot({ path: test.info().outputPath(`search-pending-${width}-${theme}.png`) });
    await held!.fulfill({ json: result('report') });
    await expect(page.getByRole('row', { name: 'report.txt', exact: true })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Search results' })).toHaveAttribute('aria-busy', 'false');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: test.info().outputPath(`search-results-${width}-${theme}.png`) });
  });
}

test('search stays responsive while an upload is active and revalidates after completion', async ({ page }) => {
  await login(page);
  let content: Route | undefined;
  let queries = 0;
  await page.route('**/mutation-tickets', route => route.fulfill({ status: 201, json: { mutation_id: 'upload', ticket: 'ticket', expires_at: '2030-01-01' } }));
  await page.route('**/uploads', route => route.fulfill({ status: 201, json: { upload_id: 'upload' } }));
  await page.route('**/uploads/upload/content', route => { content = route; });
  await page.route('**/uploads/upload/commit', route => route.fulfill({ status: 201, json: { status: 'committed' } }));
  await page.route('**/entries/search?*', route => {
    queries++;
    return route.fulfill({ json: result(new URL(route.request().url()).searchParams.get('q')!) });
  });
  await page.getByLabel('Choose files to upload').setInputFiles({ name: 'uploaded.txt', mimeType: 'text/plain', buffer: Buffer.from('body') });
  await expect.poll(() => !!content).toBe(true);
  const input = page.getByLabel('Search files and folders');
  await expect(input).toBeEnabled();
  await input.fill('report');
  await input.press('Enter');
  await expect(page.getByRole('row', { name: 'report.txt', exact: true })).toBeVisible();
  expect(queries).toBe(1);
  await content!.fulfill({ json: { upload_id: 'upload', mutation_id: 'upload', status: 'staged', length: 4,
    sha256: '230d8358dc8e8890b4c58deeb62912ee2f20357ae92a5cc861b98e68fe31acb5', published: false } });
  await expect.poll(() => queries).toBe(2);
  await expect(page.getByRole('row', { name: 'report.txt', exact: true })).toBeVisible();
  await expect(page.getByText('Uploaded to Files', { exact: true })).toBeVisible();
});
