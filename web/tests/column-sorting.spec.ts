import { folderSortHeader } from './fixtures/folder-sort';
import { expect, test, type Page } from '@playwright/test';

const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: 'root:1' };
const files = [
  { id: 'alpha', parent_id: 'root', name: 'Alpha.txt', kind: 'file', size: 10, etag: 'alpha:1' },
  { id: 'bravo', parent_id: 'root', name: 'Bravo.txt', kind: 'file', size: 20, etag: 'bravo:1' },
  { id: 'zulu', parent_id: 'root', name: 'Zulu.txt', kind: 'file', size: 30, etag: 'zulu:1' },
];
const sortedFiles = (sort: string) => sort.endsWith('_desc') ? [...files].reverse() : files;
const fileNames = (page: Page, selector = '.file-row') => page.locator(selector).evaluateAll(rows => rows.map(row => row.getAttribute('aria-label')));

async function installBackend(page: Page) {
  const folderRequests: URL[] = [];
  const searchRequests: URL[] = [];
  const trashRequests: URL[] = [];
  const accounts = [
    { account_id: 'owner', handle: 'owner', role: 'administrator', enabled: true },
    { account_id: 'zoe', handle: 'zoe', role: 'user', enabled: true },
    { account_id: 'ada', handle: 'ada', role: 'administrator', enabled: false },
    { account_id: 'ben', handle: 'ben', role: 'user', enabled: false },
  ];
  const deleted = files.map((entry, index) => ({ ...entry, id: `trash-${entry.id}`, original_path: ['Folder'], modified_at: 1_700_000_000 + index }));

  await page.route('**/*', async route => {
    const url = new URL(route.request().url());
    const json = (value: unknown) => route.fulfill({ json: value });
    if (url.pathname === '/instance') return json({ name: 'Sylux' });
    if (url.pathname === '/setup') return json({ setupRequired: false, user: null });
    if (url.pathname === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (url.pathname === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'administrator', csrf_token: 'csrf' });
    if (url.pathname === '/auth/logout') return json({});
    if (url.pathname === '/accounts') return json(accounts);
    if (url.pathname === '/folders/children') return json({ root, items: [], next_cursor: null, has_more: false });
    if (url.pathname === '/folders/root/children') {
      folderRequests.push(url);
      const sort = url.searchParams.get('sort') ?? 'name_asc';
      const cursor = url.searchParams.get('cursor');
      const ordered = sortedFiles(sort);
      return json({ items: cursor ? ordered.slice(2) : ordered.slice(0, 2), next_cursor: !cursor ? `${sort}:next` : null, has_more: !cursor });
    }
    if (url.pathname === '/entries/search') {
      searchRequests.push(url);
      const sort = url.searchParams.get('sort') ?? 'name_asc';
      const cursor = url.searchParams.get('cursor');
      const ordered = sortedFiles(sort);
      const items = cursor ? ordered.slice(2) : ordered.slice(0, 2);
      return json({ items: items.map(entry => ({ entry, location: [root] })), next_cursor: !cursor ? `${sort}:next` : null, has_more: !cursor });
    }
    if (url.pathname === '/trash') {
      trashRequests.push(url);
      const sort = url.searchParams.get('sort') ?? 'name_asc';
      const cursor = url.searchParams.get('cursor');
      const ordered = sort.endsWith('_desc') ? [...deleted].reverse() : deleted;
      return json({ items: cursor ? ordered.slice(2) : ordered.slice(0, 2), next_cursor: !cursor ? `${sort}:next` : null, has_more: !cursor });
    }
    return route.continue();
  });

  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('row', { name: 'Alpha.txt', exact: true })).toBeVisible();
  return { folderRequests, searchRequests, trashRequests, accounts };
}

for (const tableName of ['Files in current folder', 'Search matches', 'Trash items', 'Accounts'] as const) {
  test(`clicking the Name column header reverses the visible rows in ${tableName}`, async ({ page }) => {
    await installBackend(page);
    if (tableName === 'Search matches') {
      await page.getByLabel('Search files and folders', { exact: true }).fill('txt');
    } else if (tableName === 'Trash items' || tableName === 'Accounts') {
      await page.getByRole('button', { name: tableName === 'Accounts' ? 'Settings' : 'Trash', exact: true }).click();
    }

    const table = page.getByRole('table', { name: tableName, exact: true });
    const header = table.getByRole('columnheader', { name: 'Name', exact: true });
    const rows = table.locator('[role="row"][aria-label]');
    const rowNames = () => rows.evaluateAll(items => items.map(row => row.getAttribute('aria-label')));
    const accounts = tableName === 'Accounts';
    await expect.poll(rowNames).toEqual(accounts ? ['ada', 'ben', 'owner', 'zoe'] : ['Alpha.txt', 'Bravo.txt']);
    await expect(header).toHaveAttribute('aria-sort', 'ascending');

    if (!accounts) {
      await page.getByRole('button', { name: 'Next page', exact: true }).click();
      await expect.poll(rowNames).toEqual(['Zulu.txt']);
    }

    await header.click();
    await expect(header).toHaveAttribute('aria-sort', 'descending');
    await expect.poll(rowNames).toEqual(accounts ? ['zoe', 'owner', 'ben', 'ada'] : ['Zulu.txt', 'Bravo.txt']);
    if (!accounts) await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toHaveCount(0);

    await header.click();
    await expect(header).toHaveAttribute('aria-sort', 'ascending');
    await expect.poll(rowNames).toEqual(accounts ? ['ada', 'ben', 'owner', 'zoe'] : ['Alpha.txt', 'Bravo.txt']);
  });
}

test('folder column reversal preserves paging and selection and ignores a stale response', async ({ page }) => {
  const { folderRequests } = await installBackend(page);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect.poll(() => fileNames(page)).toEqual(['Zulu.txt']);
  await page.getByLabel('Select Zulu.txt', { exact: true }).check();

  let release!: () => void;
  const gate = new Promise<void>(resolve => release = resolve);
  await page.route('**/folders/root/children*', async route => {
    const url = new URL(route.request().url());
    if (url.searchParams.get('sort') !== 'name_desc') return route.fallback();
    folderRequests.push(url);
    await gate;
    return route.fulfill({ json: { items: [{ ...files[2], name: 'Stale.txt' }], next_cursor: null, has_more: false } });
  });

  const headerCell = folderSortHeader(page, 'Name');
  const header = headerCell.getByRole('button');
  await expect(headerCell).toHaveAttribute('aria-sort', 'ascending');
  await header.focus();
  await page.keyboard.press('Enter');
  await expect.poll(() => folderRequests.at(-1)?.searchParams.get('sort')).toBe('name_desc');
  expect(folderRequests.at(-1)?.searchParams.has('cursor')).toBe(false);
  await expect(headerCell).toHaveAttribute('aria-sort', 'ascending');
  await expect(header).toHaveAttribute('aria-busy', 'true');
  await expect(header).toBeFocused();
  const stale = page.waitForResponse(response => new URL(response.url()).searchParams.get('sort') === 'name_desc');

  await page.keyboard.press('Enter');
  await expect(header).toHaveAttribute('aria-busy', 'false');
  await expect(page.getByLabel('Select Zulu.txt', { exact: true })).toBeChecked();
  await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toBeEnabled();
  await expect.poll(() => fileNames(page)).toEqual(['Zulu.txt']);
  await expect(page.getByRole('table', { name: 'Files in current folder' }).locator('[role="columnheader"][aria-sort="ascending"]')).toHaveCount(1);
  release();
  await stale;
  await expect.poll(() => fileNames(page)).toEqual(['Zulu.txt']);
  await expect(page.getByRole('row', { name: 'Stale.txt', exact: true })).toHaveCount(0);
});

test('search column sort sends the selected order and resets page and selection', async ({ page }) => {
  const { searchRequests } = await installBackend(page);
  await page.getByLabel('Search files and folders', { exact: true }).fill('txt');
  const results = page.getByRole('region', { name: 'Search results', exact: true });
  const firstRow = results.getByRole('row', { name: 'Alpha.txt', exact: true });
  await expect(firstRow).toBeVisible();
  const sizeHeader = results.getByRole('columnheader', { name: 'Size', exact: true });
  const sizeButton = sizeHeader.getByRole('button');
  const sizeValue = firstRow.getByRole('cell', { name: '10 B', exact: true });
  const iconRight = await sizeButton.locator('svg').evaluate(icon => icon.getBoundingClientRect().right);
  const valueRight = await sizeValue.evaluate(cell => cell.getBoundingClientRect().right);
  const headerWidth = await sizeHeader.evaluate(cell => cell.getBoundingClientRect().width);
  const buttonWidth = await sizeButton.evaluate(button => button.getBoundingClientRect().width);
  const buttonInset = await sizeButton.evaluate(button => {
    const style = getComputedStyle(button);
    return parseFloat(style.paddingRight) + parseFloat(style.borderRightWidth);
  });
  expect(buttonWidth).toBeCloseTo(headerWidth, 0);
  expect(valueRight - iconRight).toBeCloseTo(buttonInset, 0);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect.poll(() => fileNames(page)).toEqual(['Zulu.txt']);
  await page.getByLabel('Select Zulu.txt in Files', { exact: true }).check();

  const header = results.getByRole('button', { name: 'Sort by name, descending', exact: true });
  await header.focus();
  await page.keyboard.press('Enter');
  await expect.poll(() => searchRequests.at(-1)?.searchParams.get('sort')).toBe('name_desc');
  expect(searchRequests.at(-1)?.searchParams.has('cursor')).toBe(false);
  await expect(page.getByRole('group', { name: 'Selected items', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toHaveCount(0);
  await expect.poll(() => fileNames(page)).toEqual(['Zulu.txt', 'Bravo.txt']);
  await expect(results.getByRole('table', { name: 'Search matches' }).locator('[role="columnheader"][aria-sort="descending"]')).toHaveCount(1);
  await page.setViewportSize({ width: 320, height: 812 });
  await expect(sizeButton).toBeHidden();
  await expect(page.getByLabel('Sort search by', { exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('a superseded search request cannot replace a newer sort', async ({ page }) => {
  await page.setViewportSize({ width: 375, height: 812 });
  const { searchRequests } = await installBackend(page);
  await page.getByLabel('Search files and folders', { exact: true }).fill('txt');
  const results = page.getByRole('region', { name: 'Search results', exact: true });
  await expect(results.getByRole('row', { name: 'Alpha.txt', exact: true })).toBeVisible();

  let release!: () => void;
  const gate = new Promise<void>(resolve => release = resolve);
  let finishStaleRoute!: () => void;
  const staleRouteFinished = new Promise<void>(resolve => finishStaleRoute = resolve);
  await page.route('**/entries/search*', async route => {
    const url = new URL(route.request().url());
    if (url.searchParams.get('sort') !== 'name_desc') return route.fallback();
    searchRequests.push(url);
    await gate;
    try { await route.fulfill({ json: { items: [{ entry: { ...files[2], name: 'Stale.txt' }, location: [root] }], next_cursor: null, has_more: false } }); }
    catch { /* The newer sort may abort this request before its late response is released. */ }
    finally { finishStaleRoute(); }
  });

  const nameHeader = results.getByRole('button', { name: 'Sort by name, descending', exact: true });
  await nameHeader.click();
  await expect.poll(() => searchRequests.at(-1)?.searchParams.get('sort')).toBe('name_desc');
  await page.locator('#search-sort').selectOption('size_desc');
  await expect.poll(() => searchRequests.at(-1)?.searchParams.get('sort')).toBe('size_desc');
  await expect.poll(() => fileNames(page)).toEqual(['Zulu.txt', 'Bravo.txt']);
  release();
  await staleRouteFinished;
  await expect.poll(() => fileNames(page)).toEqual(['Zulu.txt', 'Bravo.txt']);
  await expect(results.getByRole('row', { name: 'Stale.txt', exact: true })).toHaveCount(0);
});

test('trash sorting restarts at page one and carries the new order into later pages', async ({ page }) => {
  const { trashRequests } = await installBackend(page);
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  const rows = page.locator('.trash-row');
  await expect(rows).toHaveCount(2);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(rows).toHaveCount(1);

  const table = page.getByRole('table', { name: 'Trash items' });
  const header = table.getByRole('button', { name: 'Sort by name, descending', exact: true });
  await header.focus();
  await page.keyboard.press('Enter');
  await expect.poll(() => trashRequests.at(-1)?.searchParams.get('sort')).toBe('name_desc');
  expect(trashRequests.at(-1)?.searchParams.has('cursor')).toBe(false);
  await expect(rows).toHaveCount(2);
  await expect(rows.first()).toContainText('Zulu.txt');
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(rows).toHaveCount(1);
  expect(trashRequests.at(-1)?.searchParams.get('sort')).toBe('name_desc');
  expect(trashRequests.at(-1)?.searchParams.get('cursor')).toBe('name_desc:next');
  await expect(rows.first()).toContainText('Alpha.txt');
});

for (const surface of ['Search', 'Trash'] as const) {
  test(`${surface} sort stays focused and blocks duplicate activation while busy`, async ({ page }) => {
    const { searchRequests, trashRequests } = await installBackend(page);
    if (surface === 'Search') {
      await page.getByLabel('Search files and folders', { exact: true }).fill('txt');
      await expect(page.getByRole('region', { name: 'Search results', exact: true }).getByRole('row', { name: 'Alpha.txt', exact: true })).toBeVisible();
    } else {
      await page.getByRole('button', { name: 'Trash', exact: true }).click();
      await expect(page.locator('.trash-row')).toHaveCount(2);
    }

    const requests = surface === 'Search' ? searchRequests : trashRequests;
    let release!: () => void;
    const gate = new Promise<void>(resolve => release = resolve);
    let intercepted = 0;
    await page.route(surface === 'Search' ? '**/entries/search*' : '**/trash*', async route => {
      const url = new URL(route.request().url());
      if (url.searchParams.get('sort') !== 'name_desc') return route.fallback();
      intercepted++;
      requests.push(url);
      await gate;
      if (surface === 'Search') return route.fulfill({ json: { items: sortedFiles('name_desc').slice(0, 2).map(entry => ({ entry, location: [root] })), next_cursor: null, has_more: false } });
      const items = sortedFiles('name_desc').slice(0, 2).map((entry, index) => ({ ...entry, id: `trash-${entry.id}`, original_path: ['Folder'], modified_at: 1_700_000_000 + index }));
      return route.fulfill({ json: { items, next_cursor: null, has_more: false } });
    });

    const tableName = surface === 'Search' ? 'Search matches' : 'Trash items';
    const table = page.getByRole('table', { name: tableName });
    const trigger = table.getByRole('button', { name: 'Sort by name, descending', exact: true });
    await trigger.focus();
    await expect(trigger).toBeFocused();
    await page.keyboard.press('Enter');

    await expect.poll(() => intercepted).toBe(1);
    const pending = table.getByRole('button', { name: 'Sort by name, ascending', exact: true });
    await expect(pending).toHaveAttribute('aria-disabled', 'true');
    await expect(pending).toHaveAttribute('aria-busy', 'true');
    await expect(pending).toBeFocused();
    await page.keyboard.press('Enter');
    expect(intercepted).toBe(1);
    expect(requests.at(-1)?.searchParams.get('sort')).toBe('name_desc');

    release();
    await expect(pending).toHaveAttribute('aria-busy', 'false');
    await expect(pending).toHaveAttribute('aria-disabled', 'false');
    await expect(pending).toBeFocused();
    expect(intercepted).toBe(1);
  });
}

test('long search names and paths remain visible with reachable controls at 900px', async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 900 });
  await installBackend(page);
  const name = `annual-report-${'long-name-'.repeat(14)}.pdf`;
  const folders = [
    { id: 'dept', parent_id: 'root', name: `Research and development ${'division-'.repeat(4)}`, kind: 'folder', etag: 'dept:1' },
    { id: 'team', parent_id: 'dept', name: `Long-term records ${'archive-'.repeat(4)}`, kind: 'folder', etag: 'team:1' },
    { id: 'year', parent_id: 'team', name: `Previous financial years ${'closed-'.repeat(3)}`, kind: 'folder', etag: 'year:1' },
  ];
  await page.route('**/entries/search*', route => {
    const url = new URL(route.request().url());
    if (url.searchParams.get('q') !== 'long') return route.fallback();
    return route.fulfill({ json: { items: [{ entry: { id: 'long-file', parent_id: 'year', name, kind: 'file', size: 42, etag: 'long-file:1' }, location: [root, ...folders] }], next_cursor: null, has_more: false } });
  });
  await page.getByLabel('Search files and folders', { exact: true }).fill('long');
  const results = page.getByRole('region', { name: 'Search results', exact: true });
  await expect(results.getByRole('row', { name, exact: true })).toBeVisible();
  const row = results.getByRole('row', { name, exact: true });
  const containingFolder = row.getByRole('button', { name: `Open containing folder for ${name}`, exact: true });
  await expect(containingFolder).toBeVisible();
  const bounds = await containingFolder.boundingBox();
  expect(bounds).not.toBeNull();
  expect(bounds!.x).toBeGreaterThanOrEqual(0);
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(900);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  expect(await results.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await containingFolder.focus();
  await expect(containingFolder).toBeFocused();
  await page.screenshot({ path: test.info().outputPath('columns-search-long-path-900.png'), fullPage: true });
});

test('account name, role and status sorting covers every row locally', async ({ page }) => {
  const { accounts } = await installBackend(page);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const rows = () => page.locator('.account-row').evaluateAll(items => items.map(row => row.querySelector('.account-identity strong')?.textContent));
  const headings = page.getByRole('table', { name: 'Accounts' });
  await expect.poll(rows).toEqual(accounts.map(account => account.handle).sort());

  const role = headings.getByRole('button', { name: 'Sort by role, ascending', exact: true });
  await role.focus();
  await page.keyboard.press('Enter');
  await expect.poll(rows).toEqual(['ada', 'owner', 'ben', 'zoe']);
  await expect(headings.locator('[role="columnheader"][aria-sort="ascending"]')).toHaveCount(1);

  await headings.getByRole('button', { name: 'Sort by status, ascending', exact: true }).click();
  await expect.poll(rows).toEqual(['owner', 'zoe', 'ada', 'ben']);
  await headings.getByRole('button', { name: 'Sort by name, ascending', exact: true }).click();
  await expect.poll(rows).toEqual(['ada', 'ben', 'owner', 'zoe']);
  await headings.getByRole('button', { name: 'Sort by name, descending', exact: true }).click();
  await expect.poll(rows).toEqual(['zoe', 'owner', 'ben', 'ada']);
  await expect(page.locator('.account-row')).toHaveCount(4);
});

for (const theme of ['light', 'dark'] as const) {
  for (const width of [1280, 900, 375] as const) {
    test(`sortable headers keep keyboard focus and fit ${width}px in ${theme} theme`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await installBackend(page);

      const folderHeader = folderSortHeader(page, 'Name').getByRole('button');
      await folderHeader.focus();
      await page.keyboard.press('Enter');
      await expect(folderHeader).toBeFocused();
      await expect.poll(() => fileNames(page)).toEqual(['Zulu.txt', 'Bravo.txt']);
      await expect(page.getByRole('table', { name: 'Files in current folder' }).locator('[role="columnheader"][aria-sort="descending"]')).toHaveCount(1);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: test.info().outputPath(`columns-files-${theme}-${width}.png`), fullPage: true });

      await page.getByLabel('Search files and folders', { exact: true }).fill('txt');
      const search = page.getByRole('region', { name: 'Search results', exact: true });
      await expect(search.getByRole('row', { name: 'Alpha.txt', exact: true })).toBeVisible();
      if (width === 375) {
        const sort = page.getByLabel('Sort search by', { exact: true });
        await sort.focus();
        await expect(sort).toBeFocused();
        await sort.selectOption('name_desc');
      } else {
        const searchHeader = search.getByRole('button', { name: 'Sort by name, descending', exact: true });
        await searchHeader.focus();
        await expect(searchHeader).toBeFocused();
        await page.keyboard.press('Enter');
      }
      await expect(search).toHaveAttribute('aria-busy', 'false');
      await expect.poll(() => fileNames(page)).toEqual(['Zulu.txt', 'Bravo.txt']);
      await expect(search.getByRole('table', { name: 'Search matches' }).locator('[role="columnheader"][aria-sort="descending"]')).toHaveCount(1);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: test.info().outputPath(`columns-search-${theme}-${width}.png`), fullPage: true });

      if (width === 375) await page.getByRole('button', { name: 'Toggle Sidebar', exact: true }).click();
      await page.getByRole('button', { name: 'Trash', exact: true }).click();
      if (width === 375) { await page.keyboard.press('Escape'); await expect(page.getByRole('dialog')).toHaveCount(0); }
      await expect(page.locator('.trash-row')).toHaveCount(2);
      if (width === 375) {
        const sort = page.getByLabel('Sort Trash by', { exact: true });
        await sort.focus();
        await expect(sort).toBeFocused();
        await sort.selectOption('name_desc');
      } else {
        const trashHeader = page.getByRole('table', { name: 'Trash items' }).getByRole('button', { name: 'Sort by name, descending', exact: true });
        await trashHeader.focus();
        await expect(trashHeader).toBeFocused();
        await page.keyboard.press('Enter');
      }
      await expect.poll(() => fileNames(page, '.trash-row')).toEqual(['Zulu.txt', 'Bravo.txt']);
      await expect(page.getByRole('table', { name: 'Trash items' }).locator('[role="columnheader"][aria-sort="descending"]')).toHaveCount(1);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: test.info().outputPath(`columns-trash-${theme}-${width}.png`), fullPage: true });

      if (width === 375) await page.getByRole('button', { name: 'Toggle Sidebar', exact: true }).click();
      await page.getByRole('button', { name: 'Settings', exact: true }).click();
      if (width === 375) { await page.keyboard.press('Escape'); await expect(page.getByRole('dialog')).toHaveCount(0); }
      const accountTable = page.getByRole('table', { name: 'Accounts' });
      await expect(accountTable.locator('.account-row')).toHaveCount(4);
      const accountHeader = accountTable.getByRole('button', { name: 'Sort by name, descending', exact: true });
      await accountHeader.focus();
      await expect(accountHeader).toBeFocused();
      await page.keyboard.press('Enter');
      await expect(accountTable.locator('[role="columnheader"][aria-sort="descending"]')).toHaveCount(1);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: test.info().outputPath(`columns-accounts-${theme}-${width}.png`), fullPage: true });
    });
  }
}
