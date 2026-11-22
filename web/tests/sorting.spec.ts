import { chooseFolderSort, folderSortHeader } from './fixtures/folder-sort';
import { expect, test, type Page } from '@playwright/test';
import type { Entry, Folder } from '../src/lib/listing';

function hold() {
  let release!: () => void;
  const gate = new Promise<void>(resolve => release = resolve);
  return { gate, release };
}

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
  await expect(page.getByRole('button', { name: 'Open Projects', exact: true })).toBeVisible();
}

const names = (page: Page) => page.locator('.file-list .file-row').evaluateAll(rows => rows.map(row => row.getAttribute('aria-label')));

test('sort resets paging and selection and persists through folders, history and refresh', async ({ page }) => {
  const requests = await installSortingBackend(page);
  const size = folderSortHeader(page, 'Size');
  await expect(folderSortHeader(page, 'Name')).toHaveAttribute('aria-sort', 'ascending');
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await page.getByLabel('Select Zulu.txt', { exact: true }).check();
  await size.getByRole('button').click();
  await expect.poll(() => names(page)).toEqual(['Projects', 'Zulu.txt']);
  await expect(page.getByRole('group', { name: 'Selected items', exact: true })).toHaveCount(0);
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
  await expect(size).toHaveAttribute('aria-sort', 'ascending');
  await expect.poll(() => names(page)).toEqual(['Projects', 'Zulu.txt']);
  await page.screenshot({ path: test.info().outputPath('sylux-sorting-desktop.png'), fullPage: true });
  expect(requests.at(-1)?.searchParams.get('sort')).toBe('size_asc');
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect.poll(() => names(page)).toEqual(['Alpha.txt']);
  expect(requests.at(-1)?.searchParams.get('cursor')).toBe('size_asc:next');
  await page.getByRole('button', { name: 'Previous page', exact: true }).click();
  await expect.poll(() => names(page)).toEqual(['Projects', 'Zulu.txt']);
});

test('rapid reversals preserve displayed rows and focus and ignore late successes', async ({ page }) => {
  const requests = await installSortingBackend(page);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  const row = page.getByRole('row', { name: 'Zulu.txt', exact: true });
  await expect(row).toBeVisible();
  const rowNode = await row.elementHandle();
  const older = [hold()];
  let attempts = 0;
  await page.route('**/folders/root/children*', async route => {
    const url = new URL(route.request().url());
    requests.push(url);
    const index = attempts++;
    if (index >= older.length) return route.fallback();
    await older[index].gate;
    await route.fulfill({ json: { items: [{ id: 'stale', parent_id: 'root', name: 'Stale.txt', kind: 'file', size: 1, etag: '"stale"' }], next_cursor: null, has_more: false } });
  });

  const header = folderSortHeader(page, 'Name');
  const button = header.getByRole('button');
  await button.focus();
  await page.keyboard.press('Enter');
  await expect.poll(() => requests.at(-1)?.searchParams.get('sort')).toBe('name_desc');
  await expect(button).toHaveAccessibleName('Sort by name, ascending');
  await expect(button).toHaveAttribute('aria-busy', 'true');
  await expect(header).toHaveAttribute('aria-sort', 'ascending');
  await expect(button).toBeFocused();
  expect(await row.evaluate((node, original) => node === original, rowNode)).toBe(true);
  await expect(page.getByText(/^Loading/)).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toBeDisabled();
  expect(new URLSearchParams(new URL(page.url()).hash.split('?')[1]).get('sort')).toBeNull();
  await page.screenshot({ path: test.info().outputPath('sylux-sort-pending.png') });

  await page.keyboard.press('Space');
  await expect(button).toHaveAccessibleName('Sort by name, descending');
  await expect(button).toHaveAttribute('aria-busy', 'false');
  await expect(row).toBeVisible();
  await page.keyboard.press('Enter');
  await expect(header).toHaveAttribute('aria-sort', 'descending');
  await expect(button).toHaveAttribute('aria-busy', 'false');
  await expect(button).toBeFocused();
  await expect.poll(() => names(page)).toEqual(['Projects', 'Zulu.txt']);
  await expect(page).toHaveURL(/sort=name_desc$/);

  for (const [index, order] of [[0, 'name_desc']] as const) {
    const stale = page.waitForResponse(response => new URL(response.url()).searchParams.get('sort') === order);
    older[index].release();
    await (await stale).finished();
  }
  await expect.poll(() => names(page)).toEqual(['Projects', 'Zulu.txt']);
  await expect(header).toHaveAttribute('aria-sort', 'descending');
  await expect(page).toHaveURL(/sort=name_desc$/);
});

test('failed sort preserves the current page and can retry from the same header', async ({ page }) => {
  const requests = await installSortingBackend(page);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Preview Zulu.txt', exact: true })).toBeFocused();
  let fail = true;
  const attempts: URL[] = [];
  await page.route('**/folders/root/children*', route => {
    const url = new URL(route.request().url());
    if (url.searchParams.get('sort') !== 'size_asc') return route.fallback();
    attempts.push(url);
    return fail ? route.fulfill({ status: 503, json: { code: 'unavailable' } }) : route.fallback();
  });
  const size = folderSortHeader(page, 'Size');
  const button = size.getByRole('button');
  await button.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('alert')).toContainText('Could not sort files');
  await expect(page.getByRole('alert')).toBeInViewport();
  await expect(folderSortHeader(page, 'Name')).toHaveAttribute('aria-sort', 'ascending');
  expect(await size.getAttribute('aria-sort')).toBeNull();
  await expect(button).toBeFocused();
  await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toBeEnabled();
  await expect.poll(() => names(page)).toEqual(['Zulu.txt']);
  await page.screenshot({ path: test.info().outputPath('sylux-sort-failed.png') });
  fail = false;
  await page.keyboard.press('Enter');
  await expect(size).toHaveAttribute('aria-sort', 'ascending');
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(button).toBeFocused();
  await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toHaveCount(0);
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
  await folderSortHeader(page, 'Name').getByRole('button').click();
  await chooseFolderSort(page, 'Size', 'descending');
  await expect.poll(() => names(page)).toEqual(['Projects', 'Alpha.txt']);
  const failedResponse = page.waitForResponse(response => new URL(response.url()).searchParams.get('sort') === 'name_desc');
  release();
  await failedResponse;
  await expect.poll(() => names(page)).toEqual(['Projects', 'Alpha.txt']);
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(folderSortHeader(page, 'Size')).toHaveAttribute('aria-sort', 'descending');
});

test('search and destination picker keep their separate defaults and logout resets sort', async ({ page }) => {
  const requests = await installSortingBackend(page);
  await chooseFolderSort(page, 'Size', 'descending');
  await page.getByRole('button', { name: 'Options for Alpha.txt', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Move', exact: true }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  expect(requests.at(-1)?.searchParams.get('sort')).toBe('name_asc');
  await page.keyboard.press('Escape');
  await page.getByLabel('Search files and folders').fill('Alpha');
  await expect(page.getByRole('region', { name: 'Search results', exact: true })).toBeVisible();
  await expect.poll(() => requests.at(-1)?.pathname).toBe('/entries/search');
  expect(requests.at(-1)?.searchParams.get('sort')).toBe('name_asc');
  await page.getByRole('button', { name: 'Clear search', exact: true }).click();
  await expect(folderSortHeader(page, 'Size')).toHaveAttribute('aria-sort', 'descending');
  await page.getByRole('button', { name: 'Log out', exact: true }).click();
  await signIn(page);
  await expect(folderSortHeader(page, 'Name')).toHaveAttribute('aria-sort', 'ascending');
});

for (const boundary of ['search', 'folder', 'session'] as const) {
  test(`sort response cannot overwrite ${boundary} after invalidation`, async ({ page }) => {
    await installSortingBackend(page);
    const held = hold();
    await page.route('**/folders/root/children*', async route => {
      if (new URL(route.request().url()).searchParams.get('sort') !== 'name_desc') return route.fallback();
      await held.gate;
      return route.fulfill({ json: { items: [], next_cursor: null, has_more: false } });
    });
    const requested = page.waitForRequest(request => new URL(request.url()).searchParams.get('sort') === 'name_desc');
    await folderSortHeader(page, 'Name').getByRole('button').click();
    await requested;
    if (boundary === 'search') {
      await page.getByLabel('Search files and folders').fill('Alpha');
      await expect(page.getByRole('row', { name: 'Alpha.txt', exact: true })).toBeVisible();
    } else if (boundary === 'folder') {
      await page.getByRole('button', { name: 'Open Projects', exact: true }).click();
      await expect(page.getByText('This folder is empty', { exact: true })).toBeVisible();
    } else {
      await page.getByRole('button', { name: 'Log out', exact: true }).click();
      await signIn(page);
    }
    const stale = page.waitForResponse(response => new URL(response.url()).searchParams.get('sort') === 'name_desc');
    held.release();
    await (await stale).finished();
    if (boundary === 'search') await page.getByRole('button', { name: 'Clear search', exact: true }).click();
    if (boundary === 'folder') await page.goBack();
    await expect.poll(() => names(page)).toEqual(['Projects', 'Alpha.txt']);
    await expect(folderSortHeader(page, 'Name')).toHaveAttribute('aria-sort', 'ascending');
    await expect(page.getByRole('alert')).toHaveCount(0);
  });
}

test('a pending folder sort cannot change the listing or focus restored by Back', async ({ page }) => {
  await installSortingBackend(page);
  await page.getByRole('button', { name: 'Open Projects', exact: true }).click();
  await expect(page.getByText('This folder is empty', { exact: true })).toBeVisible();

  const sort = folderSortHeader(page, 'Name');
  const held = hold();
  const descending = (url: URL) => url.pathname === '/folders/child/children' && url.searchParams.get('sort') === 'name_desc';
  await page.route(descending, async route => {
    await held.gate;
    return route.fulfill({ status: 503, json: { code: 'unavailable' } });
  });
  const requested = page.waitForRequest(request => descending(new URL(request.url())));
  await sort.getByRole('button').click();
  await requested;
  await expect(sort.getByRole('button')).toHaveAttribute('aria-busy', 'true');
  await page.goBack();
  const heading = page.getByRole('heading', { name: 'Files', exact: true });
  await expect(heading).toBeFocused();
  await expect.poll(() => names(page)).toEqual(['Projects', 'Alpha.txt']);

  const stale = page.waitForResponse(response => descending(new URL(response.url())));
  held.release();
  await (await stale).finished();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(sort).toHaveAttribute('aria-sort', 'ascending');
  await expect(sort.getByRole('button')).toHaveAttribute('aria-busy', 'false');
  await expect(heading).toBeFocused();
  await expect(page.locator('.list-summary')).toHaveText('2 items on this page');
});

for (const theme of ['light', 'dark'] as const) {
  test(`every Files sort is reachable by keyboard at 320px in ${theme} theme`, async ({ page }) => {
    await page.setViewportSize({ width: 320, height: 812 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await installSortingBackend(page);
    await expect(page.getByLabel('Sort files by', { exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Refresh folder', exact: true }).focus();
    const selectAll = page.getByRole('checkbox', { name: 'Select all loaded items', exact: true });
    await page.keyboard.press('Tab');
    await expect(selectAll).toBeFocused();
    for (const [column, first, second] of [
      ['Name', 'descending', 'ascending'],
      ['Modified', 'ascending', 'descending'],
      ['Size', 'ascending', 'descending'],
    ] as const) {
      const header = folderSortHeader(page, column);
      const button = header.getByRole('button');
      await page.keyboard.press('Tab');
      await expect(button).toBeFocused();
      for (const direction of [first, second]) {
        await expect(button).toHaveAccessibleName(`Sort by ${column.toLowerCase()}, ${direction}`);
        await page.keyboard.press('Space');
        await expect(header).toHaveAttribute('aria-sort', direction);
        await expect(button).toBeFocused();
      }
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: test.info().outputPath(`sylux-sorting-${theme}-320.png`), fullPage: true });
  });
}

for (const staleStatus of [200, 503]) {
  test(`refresh carries pending sort and ignores late ${staleStatus}`, async ({ page }) => {
    const requests = await installSortingBackend(page);
    const held = hold();
    let first = true;
    await page.route('**/folders/root/children*', async route => {
      if (new URL(route.request().url()).searchParams.get('sort') !== 'name_desc' || !first) return route.fallback();
      first = false;
      await held.gate;
      return route.fulfill({ status: staleStatus, json: staleStatus === 200 ? { items: [], next_cursor: null, has_more: false } : { code: 'unavailable' } });
    });
    await folderSortHeader(page, 'Name').getByRole('button').click();
    await page.getByRole('button', { name: 'Refresh folder' }).click();
    await expect(folderSortHeader(page, 'Name')).toHaveAttribute('aria-sort', 'descending');
    await expect.poll(() => names(page)).toEqual(['Projects', 'Zulu.txt']);
    const stale = page.waitForResponse(response => new URL(response.url()).searchParams.get('sort') === 'name_desc');
    held.release();
    await (await stale).finished();
    await expect(page.getByRole('alert')).toHaveCount(0);
    await page.getByRole('button', { name: 'Next page' }).click();
    await expect.poll(() => names(page)).toEqual(['Alpha.txt']);
    expect(requests.at(-1)?.searchParams.get('cursor')).toBe('name_desc:next');
  });

  test(`reversing pending sort preserves page and selection against late ${staleStatus}`, async ({ page }) => {
    await installSortingBackend(page);
    await page.getByRole('button', { name: 'Next page' }).click();
    await page.getByLabel('Select Zulu.txt').check();
    const held = hold();
    await page.route('**/folders/root/children*', async route => {
      if (new URL(route.request().url()).searchParams.get('sort') !== 'name_desc') return route.fallback();
      await held.gate;
      return route.fulfill({ status: staleStatus, json: staleStatus === 200 ? { items: [], next_cursor: null, has_more: false } : { code: 'unavailable' } });
    });
    const button = folderSortHeader(page, 'Name').getByRole('button');
    await button.click();
    await button.click();
    await expect(page.getByLabel('Select Zulu.txt')).toBeChecked();
    await expect(page.getByRole('button', { name: 'Previous page' })).toBeEnabled();
    const stale = page.waitForResponse(response => new URL(response.url()).searchParams.get('sort') === 'name_desc');
    held.release();
    await (await stale).finished();
    await expect.poll(() => names(page)).toEqual(['Zulu.txt']);
    await expect(page.getByRole('alert')).toHaveCount(0);
  });
}

test('failed refresh after pending sort recovers with committed sort', async ({ page }) => {
  await installSortingBackend(page);
  const held = hold();
  let count = 0;
  await page.route('**/folders/root/children*', async route => {
    if (new URL(route.request().url()).searchParams.get('sort') !== 'name_desc') return route.fallback();
    if (++count === 1) { await held.gate; return route.fulfill({ json: { items: [], next_cursor: null, has_more: false } }); }
    return route.fulfill({ status: 503, json: { code: 'unavailable' } });
  });
  await folderSortHeader(page, 'Name').getByRole('button').click();
  await page.getByRole('button', { name: 'Refresh folder' }).click();
  await expect(page.getByText('Could not load this folder. Try again.')).toBeVisible();
  held.release();
  await page.getByRole('button', { name: /try again/i }).click();
  await expect(folderSortHeader(page, 'Name')).toHaveAttribute('aria-sort', 'ascending');
  await expect.poll(() => names(page)).toEqual(['Projects', 'Alpha.txt']);
});

test('sort restores focus only when the focused row leaves', async ({ page }) => {
  await installSortingBackend(page);
  const held = hold();
  await page.route('**/folders/root/children*', async route => {
    if (new URL(route.request().url()).searchParams.get('sort') !== 'name_desc') return route.fallback();
    await held.gate;
    return route.fallback();
  });
  const header = folderSortHeader(page, 'Name').getByRole('button');
  await header.click();
  await page.getByRole('button', { name: 'Preview Alpha.txt' }).focus();
  held.release();
  await expect(header).toBeFocused();
});

for (const target of ['surviving row', 'other control'] as const) {
  test(`sort preserves focus moved to ${target}`, async ({ page }) => {
    await installSortingBackend(page);
    const held = hold();
    await page.route('**/folders/root/children*', async route => {
      if (new URL(route.request().url()).searchParams.get('sort') !== 'name_desc') return route.fallback();
      await held.gate;
      return route.fallback();
    });
    await folderSortHeader(page, 'Name').getByRole('button').click();
    const focus = target === 'surviving row' ? page.getByRole('button', { name: 'Open Projects' }) : page.getByRole('button', { name: 'Refresh folder' });
    await focus.focus();
    held.release();
    await expect(folderSortHeader(page, 'Name')).toHaveAttribute('aria-sort', 'descending');
    await expect(focus).toBeFocused();
  });
}

test('folder creation refresh carries a pending sort', async ({ page }) => {
  await installSortingBackend(page);
  const held = hold();
  let first = true;
  await page.route('**/folders/root/children*', async route => {
    if (new URL(route.request().url()).searchParams.get('sort') !== 'name_desc' || !first) return route.fallback();
    first = false;
    await held.gate;
    return route.fulfill({ json: { items: [], next_cursor: null, has_more: false } });
  });
  await page.route('**/folders/children', route => route.request().method() === 'POST'
    ? route.fulfill({ json: { id: 'new', parent_id: 'root', name: 'New', kind: 'folder', etag: '"new"' } }) : route.fallback());
  await folderSortHeader(page, 'Name').getByRole('button').click();
  await page.getByRole('button', { name: 'New folder' }).click();
  await page.getByLabel('Folder name').fill('New');
  await page.getByRole('button', { name: 'Create folder' }).click();
  await expect(folderSortHeader(page, 'Name')).toHaveAttribute('aria-sort', 'descending');
  await expect.poll(() => names(page)).toEqual(['Projects', 'Zulu.txt']);
  held.release();
});

test('desktop size heading aligns right while narrow sorting remains visible', async ({ page }) => {
  await installSortingBackend(page);
  const size = folderSortHeader(page, 'Size');
  expect(await size.evaluate(el => getComputedStyle(el).textAlign)).toBe('right');
  await page.setViewportSize({ width: 320, height: 812 });
  await expect(size.getByRole('button')).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});
