import { expect, test, type Page } from '@playwright/test';

type Failure = { status: number; code: string };
type Mutation = { id: string; action: 'move' | 'trash'; parent?: string; etag?: string };

async function installBulkBackend(page: Page, paginated = false) {
  const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
  const entries = [
    { id: 'alpha', parent_id: 'root', name: 'alpha.bin', kind: 'file', size: 3, etag: '"alpha:1"' },
    { id: 'beta', parent_id: 'root', name: 'beta.bin', kind: 'file', size: 4, etag: '"beta:1"' },
    { id: 'gamma', parent_id: 'root', name: 'gamma.bin', kind: 'file', size: 5, etag: '"gamma:1"' },
    { id: 'destination', parent_id: 'root', name: 'Destination', kind: 'folder', etag: '"destination:1"' },
    { id: 'later', parent_id: 'root', name: 'later.bin', kind: 'file', size: 6, etag: '"later:1"' },
  ];
  const mutations: Mutation[] = [];
  const failures = new Map<string, Failure>();
  const deleted = new Set<string>();
  const appliedFailures = new Set<string>();
  let rootReads = 0;
  await page.route('**/*', async route => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (value: unknown, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(value) });
    if (path === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (path === '/auth/local') {
      const handle = (request.postDataJSON() as { handle: string }).handle;
      return json({ account_id: handle, handle, role: 'user', csrf_token: 'csrf' });
    }
    if (path === '/auth/logout') return route.fulfill({ status: 204 });
    if (path === '/trash') return json({ items: entries.filter(entry => deleted.has(entry.id)).map(entry => ({ ...entry, original_path: [] })) });
    if (path === '/folders/children' || /^\/folders\/[^/]+\/children$/.test(path)) {
      const isRoot = path === '/folders/children' || path === '/folders/root/children';
      if (isRoot) rootReads++;
      const parent = isRoot ? 'root' : path.split('/')[2];
      const visible = entries.filter(entry => entry.parent_id === parent && !deleted.has(entry.id));
      const nextPage = url.searchParams.has('cursor');
      const items = isRoot && paginated ? visible.filter(entry => nextPage ? entry.id === 'later' : entry.id !== 'later') : visible.filter(entry => entry.id !== 'later');
      return json({ ...(path === '/folders/children' ? { root } : {}), items, next_cursor: isRoot && paginated && !nextPage ? 'second-page' : null, has_more: isRoot && paginated && !nextPage });
    }
    if (/^\/entries\/[^/]+\/move$/.test(path) || (request.method() === 'DELETE' && /^\/entries\/[^/]+$/.test(path))) {
      const id = path.split('/')[2]!;
      const action = request.method() === 'DELETE' ? 'trash' : 'move';
      const parent = action === 'move' ? (request.postDataJSON() as { parent_id: string }).parent_id : undefined;
      mutations.push({ id, action, ...(parent ? { parent } : {}), etag: request.headers()['if-match'] });
      const failure = failures.get(id);
      if (failure) {
        if (appliedFailures.has(id)) {
          if (action === 'trash') deleted.add(id);
          else entries.find(entry => entry.id === id)!.parent_id = parent!;
        }
        return json({ code: failure.code }, failure.status);
      }
      const entry = entries.find(entry => entry.id === id)!;
      if (action === 'trash') { deleted.add(id); return route.fulfill({ status: 204 }); }
      entry.parent_id = parent!; entry.etag = `"${id}:2"`;
      return json(entry);
    }
    await route.continue();
  });
  await page.goto('/');
  await signIn(page, 'owner');
  return { mutations, failures, appliedFailures, entries, deleted, rootReads: () => rootReads };
}

async function signIn(page: Page, handle: string) {
  await page.getByLabel('Account name').fill(handle);
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('article', { name: 'alpha.bin', exact: true })).toBeVisible();
}

async function select(page: Page, ...names: string[]) {
  for (const name of names) await page.getByRole('checkbox', { name: `Select ${name}`, exact: true }).check();
}

async function moveSelected(page: Page) {
  await page.getByRole('button', { name: 'Move selected', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('button', { name: 'Destination', exact: true }).click();
  await dialog.getByRole('button', { name: 'Move here', exact: true }).click();
}

async function trashSelected(page: Page) {
  await page.getByRole('button', { name: 'Move selected to trash', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Move to trash', exact: true }).click();
}

test('selection works with keyboard and pointer, selects loaded items only, and resets across pages and folders', async ({ page }) => {
  const backend = await installBulkBackend(page, true);
  const alpha = page.getByRole('checkbox', { name: 'Select alpha.bin', exact: true });
  await alpha.focus();
  await page.keyboard.press('Space');
  await expect(alpha).toBeChecked();
  await expect(page.getByText('1 selected', { exact: true })).toBeVisible();
  await select(page, 'beta.bin');
  await expect(page.getByText('2 selected', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Select all loaded items', exact: true }).click();
  await expect(page.getByText('4 selected', { exact: true })).toBeVisible();
  await expect(page.getByRole('checkbox', { name: 'Select later.bin', exact: true })).toHaveCount(0);
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.getByRole('button', { name: 'Clear selection', exact: true }).click();
  await expect(alpha).not.toBeChecked();
  await select(page, 'alpha.bin');
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('checkbox', { name: 'Select later.bin', exact: true })).not.toBeChecked();
  await expect(page.getByRole('button', { name: 'Move selected', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Previous page', exact: true }).click();
  await expect(alpha).not.toBeChecked();
  await select(page, 'alpha.bin');
  await page.getByRole('button', { name: 'Open Destination', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Move selected', exact: true })).toHaveCount(0);
  expect(backend.mutations).toEqual([]);
});

test('bulk move confirms a destination, reports per-item failures, and retries only failed IDs', async ({ page }) => {
  const backend = await installBulkBackend(page);
  backend.failures.set('beta', { status: 409, code: 'name_conflict' });
  backend.failures.set('gamma', { status: 403, code: 'forbidden' });
  await select(page, 'alpha.bin', 'beta.bin', 'gamma.bin');
  await moveSelected(page);
  await expect(page.getByRole('article', { name: 'alpha.bin', exact: true })).toHaveCount(0);
  await expect(page.getByRole('article', { name: 'beta.bin', exact: true })).toBeVisible();
  await expect(page.getByRole('article', { name: 'gamma.bin', exact: true })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Move results' }).getByText(/another.*name|name.*exists|name.*conflict|another folder/i)).toBeVisible();
  await expect(page.getByRole('region', { name: 'Move results' }).getByText(/permission|forbidden|not allowed/i)).toBeVisible();
  expect(backend.mutations).toEqual(['alpha', 'beta', 'gamma'].map(id => ({ id, action: 'move', parent: 'destination', etag: `"${id}:1"` })));
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  backend.failures.clear();
  await page.getByRole('button', { name: 'Retry failed items', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Destination', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Move here', exact: true }).click();
  await expect(page.getByRole('article', { name: 'beta.bin', exact: true })).toHaveCount(0);
  await expect(page.getByRole('article', { name: 'gamma.bin', exact: true })).toHaveCount(0);
  expect(backend.mutations.map(item => item.id)).toEqual(['alpha', 'beta', 'gamma', 'beta', 'gamma']);
});

test('bulk trash requires confirmation and keeps non-empty or forbidden items visible', async ({ page }) => {
  const backend = await installBulkBackend(page);
  backend.failures.set('destination', { status: 409, code: 'folder_not_empty' });
  backend.failures.set('beta', { status: 403, code: 'forbidden' });
  await select(page, 'alpha.bin', 'beta.bin', 'Destination');
  await page.getByRole('button', { name: 'Move selected to trash', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('3 selected items');
  expect(backend.mutations).toEqual([]);
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  expect(backend.mutations).toEqual([]);
  await trashSelected(page);
  await expect(page.getByRole('article', { name: 'alpha.bin', exact: true })).toHaveCount(0);
  await expect(page.getByRole('article', { name: 'Destination', exact: true })).toBeVisible();
  await expect(page.getByRole('article', { name: 'beta.bin', exact: true })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Trash results' }).getByText(/empty.*folder|folder.*empty/i)).toBeVisible();
  expect(backend.mutations).toEqual(['alpha', 'beta', 'destination'].map(id => ({ id, action: 'trash', etag: `"${id}:1"` })));
});

test('uncertain bulk responses require a refresh before any retry', async ({ page }) => {
  const backend = await installBulkBackend(page);
  backend.failures.set('beta', { status: 503, code: 'unavailable' });
  await select(page, 'alpha.bin', 'beta.bin');
  await trashSelected(page);
  await expect(page.getByRole('article', { name: 'alpha.bin', exact: true })).toHaveCount(0);
  await expect(page.getByRole('article', { name: 'beta.bin', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Refresh before retry', exact: true })).toBeVisible();
  const retry = page.getByRole('button', { name: 'Retry failed items', exact: true });
  if (await retry.count()) await expect(retry).toBeDisabled();
  const reads = backend.rootReads();
  backend.failures.clear();
  await page.getByRole('button', { name: 'Refresh before retry', exact: true }).click();
  await expect.poll(backend.rootReads).toBeGreaterThan(reads);
  expect(backend.mutations.map(item => item.id)).toEqual(['alpha', 'beta']);
});

test('selection is cleared when switching accounts', async ({ page }) => {
  const backend = await installBulkBackend(page);
  await select(page, 'alpha.bin', 'beta.bin');
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  await signIn(page, 'second');
  await expect(page.getByRole('checkbox', { name: 'Select alpha.bin', exact: true })).not.toBeChecked();
  await expect(page.getByRole('button', { name: 'Move selected', exact: true })).toHaveCount(0);
  expect(backend.mutations).toEqual([]);
});

test('an expired session stops the batch before later selected items are sent', async ({ page }) => {
  const backend = await installBulkBackend(page);
  backend.failures.set('alpha', { status: 401, code: 'unauthorized' });
  await select(page, 'alpha.bin', 'beta.bin', 'gamma.bin');
  await trashSelected(page);
  await expect(page.getByLabel('Account name')).toBeVisible();
  expect(backend.mutations.map(item => item.id)).toEqual(['alpha']);
  backend.failures.clear();
  await signIn(page, 'second');
  await expect(page.getByRole('checkbox', { name: 'Select beta.bin', exact: true })).not.toBeChecked();
  expect(backend.mutations.map(item => item.id)).toEqual(['alpha']);
});


test('refresh reconciles a committed trash operation after its response fails without replaying it', async ({ page }) => {
  const backend = await installBulkBackend(page);
  backend.failures.set('alpha', { status: 503, code: 'unavailable' });
  backend.appliedFailures.add('alpha');
  await select(page, 'alpha.bin', 'beta.bin');
  await trashSelected(page);
  await expect(page.getByRole('button', { name: 'Refresh before retry', exact: true })).toBeVisible();
  backend.failures.clear();
  await page.getByRole('button', { name: 'Refresh before retry', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Refresh before retry', exact: true })).toHaveCount(0);
  await expect(page.getByRole('article', { name: 'alpha.bin', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Retry failed items', exact: true })).toHaveCount(0);
  expect(backend.mutations.map(item => item.id)).toEqual(['alpha', 'beta']);
});

test('bulk destination picker excludes a selected folder', async ({ page }) => {
  const backend = await installBulkBackend(page);
  await select(page, 'alpha.bin', 'Destination');
  await page.getByRole('button', { name: 'Move selected', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('button', { name: /Destination.*cannot move into itself/ })).toBeDisabled();
  await expect(dialog.getByRole('button', { name: 'Move here', exact: true })).toBeDisabled();
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  expect(backend.mutations).toEqual([]);
});

async function installSearchBulkBackend(page: Page, count = 4, pageSize = 50) {
  const backend = await installBulkBackend(page);
  const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
  const sources = ['Work', 'Home'].map((name, index) => ({ id: `source-${index}`, parent_id: 'root', name, kind: 'folder', etag: `"source-${index}:1"` }));
  backend.entries.push(...sources);
  for (let index = 0; index < count; index++) backend.entries.push({
    id: `match-${index}`, parent_id: sources[index % 2]!.id, name: count === 4 ? 'match.bin' : `match-${String(index).padStart(2, '0')}.bin`, kind: 'file', size: index, etag: `"match-${index}:1"`,
  });
  let staleNext = false;
  let searches = 0;
  await page.route('**/entries/search**', async route => {
    searches++;
    const url = new URL(route.request().url());
    if (staleNext && url.searchParams.has('cursor')) {
      staleNext = false;
      return route.fulfill({ status: 409, json: { code: 'listing_changed' } });
    }
    const query = url.searchParams.get('q')!;
    const matches = backend.entries.filter(entry => entry.name.includes(query) && !backend.deleted.has(entry.id));
    const offset = Number(url.searchParams.get('cursor') ?? 0);
    const items = matches.slice(offset, offset + pageSize).map(entry => ({ entry, location: entry.parent_id === 'root' ? [root] : [root, backend.entries.find(folder => folder.id === entry.parent_id)!] }));
    const more = offset + pageSize < matches.length;
    return route.fulfill({ json: { items, next_cursor: more ? String(offset + pageSize) : null, has_more: more } });
  });
  await page.getByRole('searchbox').fill('match');
  await expect(page.getByRole('region', { name: 'Search results', exact: true })).toHaveAttribute('aria-busy', 'false');
  return { ...backend, staleNext: () => { staleNext = true; }, searches: () => searches };
}

test('search selection supports pointer and keyboard ranges and clears on query or page changes', async ({ page }) => {
  await installSearchBulkBackend(page, 6, 4);
  const checkboxes = page.getByRole('region', { name: 'Search results', exact: true }).getByRole('checkbox');
  await checkboxes.nth(0).click();
  await checkboxes.nth(2).click({ modifiers: ['Shift'] });
  await expect(page.getByText('3 selected', { exact: true })).toBeVisible();
  await checkboxes.nth(2).click({ modifiers: ['Shift'] });
  await expect(page.getByRole('region', { name: 'Search results', exact: true }).locator('input:checked')).toHaveCount(0);
  await checkboxes.nth(1).focus();
  await page.keyboard.press('Space');
  await checkboxes.nth(3).focus();
  await page.keyboard.press('Shift+Space');
  await expect(page.getByText('3 selected', { exact: true })).toBeVisible();
  await page.keyboard.press('Shift+Space');
  await expect(page.getByRole('region', { name: 'Search results', exact: true }).locator('input:checked')).toHaveCount(0);
  await checkboxes.nth(0).check();
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Search results', exact: true }).locator('input:checked')).toHaveCount(0);
  await checkboxes.nth(0).check();
  await page.getByRole('button', { name: 'Previous page', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Search results', exact: true }).locator('input:checked')).toHaveCount(0);
  await checkboxes.nth(0).check();
  await page.getByRole('searchbox').fill('match-0');
  await expect(page.getByRole('button', { name: 'Move selected', exact: true })).toHaveCount(0);
  await page.getByRole('searchbox').fill('match');
  await expect(checkboxes).toHaveCount(4);
});

test('search range checkboxes name duplicate files by containing folder and clear selection after stale cursor restart', async ({ page }) => {
  const backend = await installSearchBulkBackend(page, 4, 2);
  const work = page.getByRole('checkbox', { name: 'Select match.bin in Files / Work', exact: true });
  const home = page.getByRole('checkbox', { name: 'Select match.bin in Files / Home', exact: true });
  await work.check();
  await home.click({ modifiers: ['Shift'] });
  await expect(page.getByText('Current loaded page only', { exact: true })).toBeVisible();
  backend.staleNext();
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByText('Files changed. Search results have been restarted.', { exact: true })).toBeVisible();
  await expect(work).not.toBeChecked();
  await expect(home).not.toBeChecked();
});

test('search moves 50 loaded matches with their ETags and shows changed locations', async ({ page }) => {
  const backend = await installSearchBulkBackend(page, 51);
  await page.getByRole('checkbox', { name: 'Select match-00.bin in Files / Work', exact: true }).check();
  await page.getByRole('button', { name: 'Select all loaded items', exact: true }).click();
  await expect(page.getByText('50 selected', { exact: true })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  for (const width of [390, 1280]) {
    await page.setViewportSize({ width, height: 844 });
    for (const theme of ['light', 'dark']) {
      if (theme === 'dark') await page.getByRole('button', { name: 'Switch to dark mode', exact: true }).click();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      await page.screenshot({ path: test.info().outputPath(`search-selected-${width}-${theme}.png`), animations: 'disabled' });
    }
    await page.getByRole('button', { name: 'Switch to light mode', exact: true }).click();
  }
  await moveSelected(page);
  await expect(page.getByRole('region', { name: 'Move results' }).getByText('50 of 50 succeeded.', { exact: true })).toBeVisible();
  expect(backend.mutations).toEqual(Array.from({ length: 50 }, (_, index) => ({ id: `match-${index}`, action: 'move', parent: 'destination', etag: `"match-${index}:1"` })));
  await expect(page.getByRole('checkbox', { name: 'Select match-00.bin in Files / Destination', exact: true })).toBeVisible();
  expect(backend.entries.find(entry => entry.id === 'match-50')!.parent_id).toBe('source-0');
});

test('search bulk retries conflicts across original folders and reconciles applied uncertain trash without replay', async ({ page }) => {
  const backend = await installSearchBulkBackend(page, 2);
  backend.failures.set('match-1', { status: 409, code: 'name_conflict' });
  await page.getByRole('checkbox').first().check();
  await page.getByRole('button', { name: 'Select all loaded items', exact: true }).click();
  await moveSelected(page);
  await expect(page.getByRole('region', { name: 'Move results' }).getByText('1 of 2 succeeded.', { exact: true })).toBeVisible();
  backend.failures.clear();
  await page.getByRole('button', { name: 'Retry failed items', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Destination', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Move here', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Move results' }).getByText('2 of 2 succeeded.', { exact: true })).toBeVisible();
  expect(backend.mutations.map(item => item.id)).toEqual(['match-0', 'match-1', 'match-1']);
  await page.getByRole('button', { name: 'Dismiss results', exact: true }).click();
  backend.failures.set('match-0', { status: 503, code: 'unavailable' });
  backend.appliedFailures.add('match-0');
  await page.getByRole('checkbox').first().check();
  await page.getByRole('button', { name: 'Select all loaded items', exact: true }).click();
  await trashSelected(page);
  await expect(page.getByRole('button', { name: 'Refresh before retry', exact: true })).toBeVisible();
  backend.failures.clear();
  await page.getByRole('button', { name: 'Refresh before retry', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Trash results' }).getByText('2 of 2 succeeded.', { exact: true })).toBeVisible();
  expect(backend.mutations.map(item => item.id)).toEqual(['match-0', 'match-1', 'match-1', 'match-0', 'match-1']);
});


test('search trash reports a populated folder failure and keeps it available', async ({ page }) => {
  const backend = await installSearchBulkBackend(page, 2);
  backend.entries.push({ id: 'match-folder', parent_id: 'source-0', name: 'match-folder', kind: 'folder', etag: '"match-folder:1"' });
  backend.failures.set('match-folder', { status: 409, code: 'folder_not_empty' });
  await page.getByRole('searchbox').fill('match-');
  await expect(page.getByRole('checkbox')).toHaveCount(3);
  await page.getByRole('checkbox').first().check();
  await page.getByRole('button', { name: 'Select all loaded items', exact: true }).click();
  await trashSelected(page);
  await expect(page.getByRole('region', { name: 'Trash results' })).toContainText('Folder is not empty. Empty it before retrying.');
  await expect(page.getByRole('checkbox', { name: 'Select match-folder in Files / Work', exact: true })).not.toBeChecked();
  expect(backend.mutations.map(item => item.id)).toEqual(['match-0', 'match-1', 'match-folder']);
});

test('search retry does not replay a failed item outside the refreshed loaded page or with a changed ETag', async ({ page }) => {
  const backend = await installSearchBulkBackend(page, 2);
  backend.failures.set('match-0', { status: 409, code: 'name_conflict' });
  backend.failures.set('match-1', { status: 409, code: 'name_conflict' });
  await page.getByRole('checkbox').first().check();
  await page.getByRole('button', { name: 'Select all loaded items', exact: true }).click();
  await moveSelected(page);
  await expect(page.getByRole('region', { name: 'Move results' }).getByText('0 of 2 succeeded.', { exact: true })).toBeVisible();
  backend.failures.clear();
  backend.entries.find(entry => entry.id === 'match-1')!.etag = '"match-1:2"';
  await page.getByRole('searchbox').fill('alpha');
  await expect(page.getByRole('checkbox', { name: 'Select alpha.bin in Files', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Retry failed items', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('region', { name: 'Move results' })).toContainText('This version is no longer on the loaded page. Find and select the item again');
  await expect(page.getByRole('region', { name: 'Move results' })).toContainText('This item changed. Select it again');
  expect(backend.mutations.map(item => item.id)).toEqual(['match-0', 'match-1']);
});
