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
  return { mutations, failures, appliedFailures, entries, rootReads: () => rootReads };
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
  await expect(dialog.getByRole('button', { name: 'Move here', exact: true })).toBeDisabled();
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
