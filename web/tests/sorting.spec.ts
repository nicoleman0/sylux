import { expect, test, type Page } from '@playwright/test';
import type { Entry, Folder } from '../src/lib/listing';

async function installSortingBackend(page: Page) {
  const root: Folder = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
  const child: Folder = { id: 'child', parent_id: 'root', name: 'Projects', kind: 'folder', etag: '"child:1"' };
  const entries: Entry[] = [child,
    { id: 'alpha', parent_id: 'root', name: 'Alpha.txt', kind: 'file', size: 300, etag: '"a:1"' },
    { id: 'zulu', parent_id: 'root', name: 'Zulu.txt', kind: 'file', size: 100, etag: '"z:1"' },
  ];
  const requests: URL[] = [];
  await page.route('**/*', async route => {
    const url = new URL(route.request().url());
    const json = (value: unknown) => route.fulfill({ json: value });
    if (url.pathname === '/instance') return json({ name: 'Sylux' });
    if (url.pathname === '/setup') return json({ setupRequired: false, user: null });
    if (url.pathname === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (url.pathname === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' });
    if (url.pathname === '/auth/logout') return json({});
    if (url.pathname === '/folders/children') return json({ root, items: entries, next_cursor: null, has_more: false });
    if (/^\/folders\/[^/]+\/children$/.test(url.pathname)) {
      requests.push(url);
      const sort = url.searchParams.get('sort') ?? 'name_asc';
      const cursor = url.searchParams.get('cursor');
      const files = sort === 'name_desc' || sort === 'size_asc' ? [entries[2], entries[1]] : [entries[1], entries[2]];
      const all = url.pathname.includes('/child/') ? [] : [child, ...files];
      return json({ items: cursor ? all.slice(2) : all.slice(0, 2), next_cursor: !cursor && all.length > 2 ? `${sort}:next` : null, has_more: !cursor && all.length > 2 });
    }
    if (url.pathname === '/entries/search') {
      requests.push(url);
      return json({ items: [{ entry: entries[1], location: [root] }], next_cursor: null, has_more: false });
    }
    return route.continue();
  });
  await page.goto('/');
  await signIn(page);
  return requests;
}

async function signIn(page: Page) {
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByLabel('Sort files by', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Open Projects', exact: true })).toBeVisible();
}

const names = (page: Page) => page.locator('.file-list .file-row').evaluateAll(rows => rows.map(row => row.getAttribute('aria-label')));

test('sort resets paging and selection and persists through folders, history and refresh', async ({ page }) => {
  const requests = await installSortingBackend(page);
  const sort = page.getByLabel('Sort files by', { exact: true });
  await expect(sort).toHaveValue('name_asc');
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await page.getByLabel('Select Zulu.txt', { exact: true }).check();
  await sort.selectOption('size_asc');
  await expect.poll(() => names(page)).toEqual(['Projects', 'Zulu.txt']);
  await expect(page.getByText('0 selected', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toHaveCount(0);
  expect(requests.at(-1)?.searchParams.get('cursor')).toBeNull();
  expect(requests.at(-1)?.searchParams.get('sort')).toBe('size_asc');
  await page.getByRole('button', { name: 'Open Projects', exact: true }).click();
  await expect(page.getByText('This folder is empty', { exact: true })).toBeVisible();
  expect(requests.at(-1)?.pathname).toBe('/folders/child/children');
  expect(requests.at(-1)?.searchParams.get('sort')).toBe('size_asc');
  await page.goBack();
  await expect.poll(() => names(page)).toEqual(['Projects', 'Zulu.txt']);
  await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
  await expect(sort).toHaveValue('size_asc');
  await expect.poll(() => names(page)).toEqual(['Projects', 'Zulu.txt']);
  await page.screenshot({ path: '/tmp/sylux-sorting-desktop.png', fullPage: true });
  expect(requests.at(-1)?.searchParams.get('sort')).toBe('size_asc');
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect.poll(() => names(page)).toEqual(['Alpha.txt']);
  expect(requests.at(-1)?.searchParams.get('cursor')).toBe('size_asc:next');
  await page.getByRole('button', { name: 'Previous page', exact: true }).click();
  await expect.poll(() => names(page)).toEqual(['Projects', 'Zulu.txt']);
});

test('late listing responses cannot replace a newer sort', async ({ page }) => {
  await installSortingBackend(page);
  let release!: () => void;
  const gate = new Promise<void>(resolve => release = resolve);
  await page.route('**/folders/root/children*', async route => {
    if (new URL(route.request().url()).searchParams.get('sort') !== 'name_desc') return route.fallback();
    await gate;
    await route.fulfill({ json: { items: [{ id: 'stale', parent_id: 'root', name: 'Stale.txt', kind: 'file', size: 1, etag: '"stale"' }], next_cursor: null, has_more: false } });
  });
  const sort = page.getByLabel('Sort files by', { exact: true });
  await sort.selectOption('name_desc');
  await sort.selectOption('size_desc');
  await expect.poll(() => names(page)).toEqual(['Projects', 'Alpha.txt']);
  const staleResponse = page.waitForResponse(response => new URL(response.url()).searchParams.get('sort') === 'name_desc');
  release();
  await staleResponse;
  await expect.poll(() => names(page)).toEqual(['Projects', 'Alpha.txt']);
  await expect(page.getByRole('article', { name: 'Stale.txt', exact: true })).toHaveCount(0);
  await expect(sort).toHaveValue('size_desc');
});

test('failed sort retries the first page with the selected order', async ({ page }) => {
  const requests = await installSortingBackend(page);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  let fail = true;
  const attempts: URL[] = [];
  await page.route('**/folders/root/children*', route => {
    const url = new URL(route.request().url());
    if (url.searchParams.get('sort') !== 'size_asc') return route.fallback();
    attempts.push(url);
    return fail ? route.fulfill({ status: 503, json: { code: 'unavailable' } }) : route.fallback();
  });
  const sort = page.getByLabel('Sort files by', { exact: true });
  await sort.selectOption('size_asc');
  await expect(page.getByRole('alert')).toContainText('Could not open this folder');
  await expect(sort).toHaveValue('size_asc');
  await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toHaveCount(0);
  fail = false;
  await page.getByRole('button', { name: 'Try again', exact: true }).click();
  await expect.poll(() => names(page)).toEqual(['Projects', 'Zulu.txt']);
  expect(attempts).toHaveLength(2);
  expect(attempts.every(url => !url.searchParams.has('cursor'))).toBe(true);
  expect(requests.at(-1)?.searchParams.get('sort')).toBe('size_asc');
});

test('an old failed sort request cannot replace a newer success with an error', async ({ page }) => {
  await installSortingBackend(page);
  let release!: () => void;
  const gate = new Promise<void>(resolve => release = resolve);
  await page.route('**/folders/root/children*', async route => {
    if (new URL(route.request().url()).searchParams.get('sort') !== 'name_desc') return route.fallback();
    await gate;
    await route.fulfill({ status: 503, json: { code: 'unavailable' } });
  });
  const sort = page.getByLabel('Sort files by', { exact: true });
  await sort.selectOption('name_desc');
  await sort.selectOption('size_desc');
  await expect.poll(() => names(page)).toEqual(['Projects', 'Alpha.txt']);
  const failedResponse = page.waitForResponse(response => new URL(response.url()).searchParams.get('sort') === 'name_desc');
  release();
  await failedResponse;
  await expect.poll(() => names(page)).toEqual(['Projects', 'Alpha.txt']);
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(sort).toHaveValue('size_desc');
});

test('search and destination picker keep their separate defaults and logout resets sort', async ({ page }) => {
  const requests = await installSortingBackend(page);
  await page.getByLabel('Sort files by', { exact: true }).selectOption('size_desc');
  await page.getByRole('button', { name: 'Options for Alpha.txt', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Move', exact: true }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  expect(requests.at(-1)?.searchParams.get('sort')).toBe('name_asc');
  await page.keyboard.press('Escape');
  await page.getByLabel('Search files and folders').fill('Alpha');
  await expect(page.getByRole('region', { name: 'Search results', exact: true })).toBeVisible();
  await expect.poll(() => requests.at(-1)?.pathname).toBe('/entries/search');
  expect(requests.at(-1)?.searchParams.has('sort')).toBe(false);
  await page.getByRole('button', { name: 'Clear search', exact: true }).click();
  await expect(page.getByLabel('Sort files by', { exact: true })).toHaveValue('size_desc');
  await page.getByRole('button', { name: 'Log out', exact: true }).click();
  await signIn(page);
  await expect(page.getByLabel('Sort files by', { exact: true })).toHaveValue('name_asc');
});

for (const theme of ['light', 'dark'] as const) {
  test(`native sorting works with keyboard at 375px in ${theme} theme`, async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await page.emulateMedia({ colorScheme: theme });
    await installSortingBackend(page);
    const sort = page.getByLabel('Sort files by', { exact: true });
    await sort.focus();
    await expect(sort).toBeFocused();
    await page.keyboard.press('s');
    await page.keyboard.press('Enter');
    await expect(sort).toHaveValue('size_asc');
    await expect.poll(() => names(page)).toEqual(['Projects', 'Zulu.txt']);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: `/tmp/sylux-sorting-${theme}-375.png`, fullPage: true });
  });
}
