import { expect, test, type Page } from '@playwright/test';
import { ApiClient, HttpError, type ApiTransport } from '../src/lib/api';
import { ListingApi, type Folder, type TrashPage } from '../src/lib/listing';
import { MockTransport } from '../src/lib/mock-transport';

const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: 'root:1' };
const longName = `Archive-${'long-project-name-'.repeat(8)}.txt`;
const longPath = ['Projects', 'nested-folder-'.repeat(15)];
function deletedItems() {
  return [
    { id: 'folder', name: 'Projects', kind: 'folder', parent_id: 'root', original_path: [], etag: 'folder:1' },
    { id: 'long', name: longName, kind: 'file', size: 3, parent_id: 'folder', original_path: longPath, etag: 'long:1' },
    { id: 'a', name: 'notes.txt', kind: 'file', size: 3, parent_id: 'a-folder', original_path: ['Work'], etag: 'a:1' },
    { id: 'b', name: 'notes.txt', kind: 'file', size: 3, parent_id: 'b-folder', original_path: ['Work'], etag: 'b:1' },
    { id: 'c', name: 'notes.txt', kind: 'file', size: 3, parent_id: 'c-folder', original_path: ['Home'], etag: 'c:1' },
    ...Array.from({ length: 100 }, (_, i) => ({ id: `item-${i}`, name: `zReport ${String(i).padStart(3, '0')}.txt`, kind: 'file', size: 3, parent_id: 'root', original_path: [], etag: `${i}:1` })),
  ].sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : a.id.localeCompare(b.id));
}
async function openTrash(page: Page, width = 1280) {
  let items = deletedItems();
  let failRestore = true;
  await page.route('**/*', route => {
    const path = new URL(route.request().url()).pathname;
    const json = (value: unknown, status = 200) => route.fulfill({ status, json: value });
    if (path === '/instance') return json({ name: 'Sylux' });
    if (path === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (path === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' });
    if (path === '/folders/children') return json({ root, items: [], next_cursor: null, has_more: false });
    if (path === '/folders/root/children') return json({ items: [], next_cursor: null, has_more: false });
    if (path === '/trash') {
      const offset = Number(new URL(route.request().url()).searchParams.get('cursor') ?? 0);
      return json({ items: items.slice(offset, offset + 100), next_cursor: offset + 100 < items.length ? String(offset + 100) : null, has_more: offset + 100 < items.length });
    }
    if (path.startsWith('/trash/')) {
      if (failRestore) return json({ code: 'conflict' }, 409);
      items = items.filter(item => item.id !== path.split('/')[2]);
      return route.fulfill({ status: 204 });
    }
    return route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  if (width === 390) await page.getByRole('button', { name: 'Toggle Sidebar', exact: true }).click();
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  if (width === 390) { await page.keyboard.press('Escape'); await expect(page.locator('[data-mobile=true]')).toBeHidden(); }
  await expect(page.getByRole('button', { name: 'Restore Projects', exact: true })).toBeVisible();
  return { succeed: () => { failRestore = false; }, empty: () => { items = []; } };
}

for (const theme of ['light', 'dark'] as const) {
  for (const width of [390, 1280]) {
    test(`compact Trash at ${width}px in ${theme} keeps recovery and duplicate context`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      const backend = await openTrash(page, width);
      const trash = page.getByRole('region', { name: 'Trash', exact: true });
      await expect(trash.getByText('Original folder:', { exact: false })).toHaveCount(0);
      await expect(trash.getByText('Restore a parent folder', { exact: false })).toHaveCount(0);
      await expect(trash.locator('.trash-row')).toHaveCount(100);
      await expect(page.getByRole('button', { name: /Restore notes.txt.*item reference a/ })).toBeVisible();
      await expect(page.getByRole('button', { name: /Restore notes.txt.*item reference b/ })).toBeVisible();
      await expect(page.getByRole('button', { name: 'Restore notes.txt from Files / Home', exact: true })).toBeVisible();
      const row = trash.locator('.trash-row').filter({ hasText: longName });
      await expect(row.locator('svg')).toHaveCount(1);
      await expect(row).toContainText(longPath.join(' / '));
      expect(await row.evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      const restore = page.getByRole('button', { name: 'Restore Projects', exact: true });
      await restore.focus();
      await page.screenshot({ path: test.info().outputPath(`trash-${width}-${theme}.png`) });
      await page.getByRole('button', { name: 'Next page', exact: true }).focus();
      await page.keyboard.press('Enter');
      await expect(trash.locator('.trash-row')).toHaveCount(5);
      await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toBeVisible();
      await page.screenshot({ path: test.info().outputPath(`trash-page-${width}-${theme}.png`) });
      await page.getByRole('button', { name: 'Previous page', exact: true }).click();
      await restore.focus();
      await page.keyboard.press('Enter');
      await expect(page.getByRole('alert')).toContainText('Restore its parent folder first');
      await expect(restore).toBeEnabled();
      await expect(restore).toBeFocused();
      backend.succeed();
      await restore.click();
      await expect(restore).toHaveCount(0);
      await expect(trash.getByRole('button', { name: `Restore ${longName}`, exact: true })).toBeFocused();
      await expect(page.getByRole('status')).toContainText('Restored');
      backend.empty();
      await page.getByRole('button', { name: 'Refresh trash', exact: true }).click();
      await expect(trash.getByText('Trash is empty.', { exact: true })).toBeVisible();
      await expect(trash.locator('.trash-row')).toHaveCount(0);
    });
  }
}


test('refresh disables Restore until the updated Trash list arrives', async ({ page }) => {
  await openTrash(page);
  let held: import('@playwright/test').Route | undefined;
  await page.route('**/trash?*', route => { held = route; });
  await page.getByRole('button', { name: 'Refresh trash', exact: true }).click();
  await expect.poll(() => !!held).toBe(true);
  const restore = page.getByRole('button', { name: 'Restore Projects', exact: true });
  await expect(restore).toBeDisabled();
  await expect(page.getByRole('button', { name: /Restore notes.txt.*item reference a/ })).toBeDisabled();
  await held!.fulfill({ json: { items: deletedItems().slice(0, 100), next_cursor: '100', has_more: true } });
  await expect(restore).toBeEnabled();
});

test('a child can be restored after recovering its deleted parent', async ({ page }) => {
  await openTrash(page);
  const parent = { id: 'parent', name: 'Parent', kind: 'folder', parent_id: 'root', original_path: [], etag: 'parent:1' };
  const child = { id: 'child', name: 'Child.txt', kind: 'file', size: 3, parent_id: 'parent', original_path: ['Parent'], etag: 'child:1' };
  let parentDeleted = true;
  let childDeleted = true;
  await page.route('**/trash?*', route => route.fulfill({ json: { items: [...(childDeleted ? [child] : []), ...(parentDeleted ? [parent] : [])], next_cursor: null, has_more: false } }));
  await page.route('**/trash/parent/restore', route => { parentDeleted = false; return route.fulfill({ status: 204 }); });
  await page.route('**/trash/child/restore', route => {
    if (parentDeleted) return route.fulfill({ status: 409, json: { code: 'conflict' } });
    childDeleted = false;
    return route.fulfill({ status: 204 });
  });
  await page.getByRole('button', { name: 'Refresh trash', exact: true }).click();
  await page.getByRole('button', { name: 'Restore Child.txt', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Restore its parent folder first');
  await page.getByRole('button', { name: 'Restore Parent', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Restore Parent', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Restore Child.txt', exact: true }).click();
  await expect(page.getByText('Trash is empty.', { exact: true })).toBeVisible();
});

async function pagedTrash(page: Page) {
  const controls = await openTrash(page);
  let items = Array.from({ length: 201 }, (_, i) => ({ id: `deleted-${i}`, name: `Deleted ${String(i).padStart(3, '0')}.txt`, kind: 'file', parent_id: 'root', size: 1, etag: `deleted-${i}:1`, original_path: [] }));
  const reads: number[] = [];
  const restores: string[] = [];
  let failure = 0;
  await page.route('**/trash?*', route => {
    const url = new URL(route.request().url());
    expect(url.searchParams.get('limit')).toBe('100');
    const offset = Number(url.searchParams.get('cursor') ?? 0);
    reads.push(offset);
    return route.fulfill({ json: { items: items.slice(offset, offset + 100), next_cursor: offset + 100 < items.length ? String(offset + 100) : null, has_more: offset + 100 < items.length } });
  });
  await page.route('**/trash/*/restore', route => {
    const id = new URL(route.request().url()).pathname.split('/')[2];
    restores.push(id);
    if (failure) return route.fulfill({ status: failure, json: { code: 'unavailable' } });
    items = items.filter(item => item.id !== id);
    return route.fulfill({ status: 204 });
  });
  await page.getByRole('button', { name: 'Refresh trash', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Restore Deleted 000.txt', exact: true })).toBeVisible();
  return { ...controls, reads, restores, failRestore: (status: number) => { failure = status; } };
}

test('Trash renders one bounded page and preserves keyboard paging focus through the last page', async ({ page }) => {
  await pagedTrash(page);
  const rows = page.locator('.trash-row');
  await expect(rows).toHaveCount(100);
  await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toHaveCount(0);
  for (const [count, first] of [[100, '100'], [1, '200']] as const) {
    await page.getByRole('button', { name: 'Next page', exact: true }).focus();
    await page.keyboard.press('Enter');
    await expect(rows).toHaveCount(count);
    await expect(page.getByRole('button', { name: `Restore Deleted ${first}.txt`, exact: true })).toBeFocused();
  }
  await expect(page.getByRole('button', { name: 'Next page', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Previous page', exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('button', { name: 'Restore Deleted 100.txt', exact: true })).toBeFocused();
});

test('Trash held pages prevent duplicate dispatch and preserve newer focus', async ({ page }) => {
  await pagedTrash(page);
  let held: import('@playwright/test').Route | undefined;
  let calls = 0;
  await page.route('**/trash?*', route => {
    if (!new URL(route.request().url()).searchParams.has('cursor')) return route.fallback();
    calls++; held = route;
  });
  const next = page.getByRole('button', { name: 'Next page', exact: true });
  await next.focus(); await page.keyboard.press('Enter');
  await expect(next).toBeDisabled();
  await expect(next).toHaveAttribute('aria-busy', 'true');
  await expect(next).toContainText('Loading…');
  await page.keyboard.press('Enter'); expect(calls).toBe(1);
  const refresh = page.getByRole('button', { name: 'Refresh trash', exact: true });
  await page.getByRole('button', { name: 'Account', exact: true }).focus();
  await held!.fallback();
  await expect(page.getByRole('button', { name: 'Restore Deleted 100.txt', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Account', exact: true })).toBeFocused();
  await expect(refresh).toBeEnabled();
});

for (const destination of ['Account', 'signout'] as const) {
  test(`late Trash page cannot alter ${destination}`, async ({ page }) => {
    await pagedTrash(page);
    let held: import('@playwright/test').Route | undefined;
    await page.route('**/trash?*', route => {
      if (!new URL(route.request().url()).searchParams.has('cursor')) return route.fallback();
      held = route;
    });
    await page.getByRole('button', { name: 'Next page', exact: true }).click();
    await expect.poll(() => !!held).toBe(true);
    if (destination === 'Account') {
      await page.getByRole('button', { name: 'Account', exact: true }).click();
      await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
    } else {
      await page.route('**/auth/logout', route => route.fulfill({ status: 204 }));
      await page.getByRole('button', { name: 'Account', exact: true }).click();
      await page.getByRole('button', { name: 'Sign out', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
    }
    await held!.fallback();
    await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    await expect(page.locator('.trash-row')).toHaveCount(0);
    if (destination === 'Account') await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
  });
}

test('Trash transient page failure keeps rows and retries the requested page', async ({ page }) => {
  const backend = await pagedTrash(page);
  let fail = true;
  await page.route('**/trash?*', route => {
    if (new URL(route.request().url()).searchParams.has('cursor') && fail) return route.fulfill({ status: 503, json: { code: 'unavailable' } });
    return route.fallback();
  });
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeFocused();
  await expect(page.locator('.trash-row')).toHaveCount(100);
  await expect(page.getByRole('button', { name: 'Restore Deleted 000.txt', exact: true })).toBeVisible();
  await expect(page.getByText('Trash is empty.', { exact: true })).toHaveCount(0);
  fail = false;
  await page.getByRole('button', { name: 'Try again', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Restore Deleted 100.txt', exact: true })).toBeFocused();
  expect(backend.reads.at(-1)).toBe(100);
});

for (const status of [400, 409]) {
  test(`Trash stale ${status} page restarts with compact guidance`, async ({ page }) => {
    await pagedTrash(page);
    await page.route('**/trash?*', route => new URL(route.request().url()).searchParams.has('cursor') ? route.fulfill({ status, json: { code: status === 409 ? 'listing_changed' : 'invalid_cursor' } }) : route.fallback());
    await page.getByRole('button', { name: 'Next page', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Restore Deleted 000.txt', exact: true })).toBeFocused();
    await expect(page.getByRole('status')).toContainText('Trash changed. The listing has been restarted.');
    await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toHaveCount(0);
  });
}

test('restoring from page two resets cursors and completed feedback clears when paging', async ({ page }) => {
  const backend = await pagedTrash(page);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await page.getByRole('button', { name: 'Restore Deleted 100.txt', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Restore Deleted 000.txt', exact: true })).toBeFocused();
  await expect(page.getByRole('status')).toContainText('Restored');
  await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toHaveCount(0);
  expect(backend.restores).toEqual(['deleted-100']);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Restore Deleted 101.txt', exact: true })).toBeFocused();
  await expect(page.getByText('Restored', { exact: true })).toHaveCount(0);
});

test('historical duplicates retain their reference when their peer is on another page', async ({ page }) => {
  await pagedTrash(page);
  await page.route('**/trash?*', route => {
    const last = new URL(route.request().url()).searchParams.has('cursor');
    const items = Array.from({ length: last ? 1 : 100 }, (_, i) => ({ id: `history-${String(last ? 100 : i).padStart(3, '0')}`, name: 'notes.txt', kind: 'file', parent_id: 'root', etag: '1', size: 1, original_path: [], needs_reference: true }));
    return route.fulfill({ json: { items, next_cursor: last ? null : '100', has_more: !last } });
  });
  await page.getByRole('button', { name: 'Refresh trash', exact: true }).click();
  await expect(page.getByRole('button', { name: /Restore notes.txt.*item reference history-000/ })).toBeVisible();
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('button', { name: /Restore notes.txt.*item reference history-100/ })).toBeFocused();
  await expect(page.getByText('Item reference: history-100', { exact: true })).toBeVisible();
});

test('an uncertain page-two restore inspects bounded pages without replaying the mutation', async ({ page }) => {
  const backend = await pagedTrash(page);
  backend.failRestore(503);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await page.getByRole('button', { name: 'Restore Deleted 100.txt', exact: true }).click();
  const result = page.getByRole('region', { name: 'Unconfirmed file action' });
  await expect(result).toContainText('Outcome unconfirmed');
  backend.reads.length = 0;
  await result.getByRole('button', { name: 'Check Files and Trash', exact: true }).click();
  await expect(result).toHaveCount(0);
  await expect(page.getByRole('status')).toContainText('Deleted 100.txt is in Trash');
  expect(backend.reads).toEqual([0, 100, 0]);
  expect(backend.restores).toEqual(['deleted-100']);
});

function inspection(pages: (TrashPage | Error)[], onRead?: (index: number) => void) {
  const paths: string[] = [];
  const transport: ApiTransport = { async request<T>(path: string): Promise<T> {
    const index = paths.length;
    paths.push(path); onRead?.(index);
    const page = pages[index];
    if (!page) throw new Error('Unexpected extra Trash request');
    if (page instanceof Error) throw page;
    return page as T;
  } };
  const client = new ApiClient(transport);
  return { api: new ListingApi(client), client, paths };
}
const deletedFixture = (index: number) => ({ id: `id-${index}`, name: `Deleted ${String(index).padStart(3, '0')}`, parent_id: 'root', kind: 'file' as const, etag: '1', size: 1, original_path: [] });
const deletedPage = (start: number, count: number, cursor: string | null): TrashPage => ({ items: Array.from({ length: count }, (_, i) => deletedFixture(start + i)), next_cursor: cursor, has_more: !!cursor });

test('targeted Trash inspection retains requested IDs beyond page one and proves missing IDs at the end', async () => {
  const { api, paths } = inspection([deletedPage(0, 100, 'one'), deletedPage(100, 100, 'two'), deletedPage(200, 1, null)]);
  const result = await api.deletedTargets(['id-150', 'missing'], () => true);
  expect(result.map(item => item.id)).toEqual(['id-150']);
  expect(paths).toEqual(['/trash?limit=100&sort=name_asc', '/trash?limit=100&sort=name_asc&cursor=one', '/trash?limit=100&sort=name_asc&cursor=two']);
});

test('targeted Trash inspection stops once all targets are found and skips empty requests', async () => {
  const { api, paths } = inspection([deletedPage(0, 100, 'one')]);
  expect(await api.deletedTargets([], () => true)).toEqual([]);
  expect(paths).toHaveLength(0);
  expect((await api.deletedTargets(['id-0'], () => true)).map(item => item.id)).toEqual(['id-0']);
  expect(paths).toHaveLength(1);
});

for (const [label, second] of [
  ['repeated cursor', deletedPage(100, 100, 'one')],
  ['nonadvancing rows', deletedPage(0, 100, 'two')],
  ['incomplete page', { items: [], next_cursor: 'two', has_more: true }],
  ['stale cursor', new HttpError(409)],
] as const) {
  test(`targeted Trash inspection rejects ${label} without returning partial results`, async () => {
    const { api, paths } = inspection([deletedPage(0, 100, 'one'), second]);
    await expect(api.deletedTargets(['id-0', 'missing'], () => true)).rejects.toThrow();
    expect(paths).toHaveLength(2);
  });
}

test('targeted Trash inspection stops after session change or expiry', async () => {
  let current = true;
  const changed = inspection([deletedPage(0, 100, 'one')], () => { current = false; });
  await expect(changed.api.deletedTargets(['missing'], () => current)).rejects.toThrow('session changed');
  expect(changed.paths).toHaveLength(1);
  const expired = inspection([deletedPage(0, 100, 'one'), new HttpError(401)]);
  let expirations = 0;
  expired.client.setSessionExpiredHandler(() => { expirations++; });
  await expect(expired.api.deletedTargets(['missing'], () => true)).rejects.toMatchObject({ status: 401 });
  expect(expirations).toBe(1);
  expect(expired.paths).toHaveLength(2);
});

test('initial Trash loading preserves focus moved to another control', async ({ page }) => {
  await openTrash(page);
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  let held: import('@playwright/test').Route | undefined;
  await page.route('**/trash?*', route => { held = route; });
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await expect.poll(() => !!held).toBe(true);
  const account = page.getByRole('button', { name: 'Account', exact: true });
  await account.focus();
  await held!.fulfill({ json: { items: [], next_cursor: null, has_more: false } });
  await expect(page.getByText('Trash is empty.', { exact: true })).toBeVisible();
  await expect(account).toBeFocused();
});

test('prototype Trash bounds pages, references historical peers, and rejects stale or invalid cursors', async () => {
  const mock = new MockTransport();
  await mock.request('/auth/local', { method: 'POST', body: { handle: 'owner' } });
  for (let i = 0; i < 101; i++) {
    const entry = await mock.request<Folder>('/folders/children', { method: 'POST', body: { parent_id: 'root', name: 'Same' } });
    await mock.request(`/entries/${entry.id}`, { method: 'DELETE', headers: { 'If-Match': entry.etag } });
  }
  const first = await mock.request<TrashPage>('/trash?limit=100');
  expect(first.items).toHaveLength(100);
  expect(first.has_more).toBe(true);
  expect(first.items.every(item => item.needs_reference)).toBe(true);
  await mock.request('/mutation-tickets', { method: 'POST', body: {} });
  await mock.request('/accounts', { method: 'POST', body: { handle: 'second' } });
  const last = await mock.request<TrashPage>(`/trash?limit=100&cursor=${encodeURIComponent(first.next_cursor!)}`);
  expect(last.items).toHaveLength(1);
  expect(last.items[0].needs_reference).toBe(true);
  expect(last.next_cursor).toBeNull();
  expect((await mock.request<TrashPage>('/trash?limit=200')).items).toHaveLength(101);
  for (const suffix of ['limit=201', 'limit=0', 'cursor=bad']) await expect(mock.request(`/trash?${suffix}`)).rejects.toMatchObject({ status: 400 });
  const foreign = JSON.parse(atob(first.next_cursor!)); foreign.account = 'other';
  await expect(mock.request(`/trash?cursor=${encodeURIComponent(btoa(JSON.stringify(foreign)))}`)).rejects.toMatchObject({ status: 400 });
  await mock.request('/folders/children', { method: 'POST', body: { parent_id: 'root', name: 'New folder' } });
  await expect(mock.request(`/trash?cursor=${encodeURIComponent(first.next_cursor!)}`)).rejects.toMatchObject({ status: 409, code: 'listing_changed' });
});

test('confirmed restore invalidates old cursors even when its automatic first-page refresh fails', async ({ page }) => {
  const backend = await pagedTrash(page);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Restore Deleted 100.txt', exact: true })).toBeVisible();
  await page.route('**/trash?*', route => route.fulfill({ status: 503, json: { code: 'unavailable' } }));
  await page.getByRole('button', { name: 'Restore Deleted 100.txt', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeFocused();
  await expect(page.getByRole('status')).toContainText('Restored');
  await expect(page.getByRole('alert')).toContainText('Could not load trash');
  await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Next page', exact: true })).toHaveCount(0);
  await expect(page.locator('.trash-row')).toHaveCount(0);
  await expect(page.getByText('Trash is empty.', { exact: true })).toHaveCount(0);
  expect(backend.restores).toEqual(['deleted-100']);
});

test('failed stale Trash restart withholds invalid rows and cursors until first-page retry succeeds', async ({ page }) => {
  await pagedTrash(page);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Restore Deleted 100.txt', exact: true })).toBeVisible();
  let held: import('@playwright/test').Route | undefined;
  let fail = true;
  const cursors: (string | null)[] = [];
  await page.route('**/trash?*', route => {
    const cursor = new URL(route.request().url()).searchParams.get('cursor');
    cursors.push(cursor);
    if (!fail) return route.fallback();
    if (cursor) return route.fulfill({ status: 409, json: { code: 'listing_changed' } });
    held = route;
  });
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect.poll(() => !!held).toBe(true);
  await expect(page.locator('.trash-row')).toHaveCount(0);
  await expect(page.getByRole('navigation', { name: 'Trash pages', exact: true })).toHaveCount(0);
  await expect(page.getByText('Trash changed. The listing has been restarted.', { exact: true })).toHaveCount(0);
  await held!.fulfill({ status: 503, json: { code: 'unavailable' } });
  await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeFocused();
  await expect(page.getByRole('alert')).toHaveText('Trash changed. Could not reload it. Try again.');
  await expect(page.locator('.trash-row')).toHaveCount(0);
  await expect(page.getByText('Trash is empty.', { exact: true })).toHaveCount(0);
  fail = false;
  await page.getByRole('button', { name: 'Try again', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Restore Deleted 000.txt', exact: true })).toBeFocused();
  await expect(page.locator('.trash-row')).toHaveCount(100);
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toHaveCount(0);
  expect(cursors).toEqual(['200', null, null]);
});

test('a malformed single Trash row uses retry feedback without crashing the existing page', async ({ page }) => {
  await pagedTrash(page);
  const crashes: string[] = [];
  page.on('pageerror', error => crashes.push(error.message));
  await page.route('**/trash?*', route => route.fulfill({ json: { items: [{ ...deletedFixture(0), original_path: null }], next_cursor: null, has_more: false } }));
  await page.getByRole('button', { name: 'Refresh trash', exact: true }).click();
  await expect(page.getByRole('alert')).toHaveText('Could not load trash. Try again.');
  await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeVisible();
  await expect(page.locator('.trash-row')).toHaveCount(100);
  expect(crashes).toEqual([]);
});

test('Trash validates each consumed field even on a single-row page', async () => {
  for (const invalid of [
    { id: null }, { name: 1 }, { etag: '' }, { kind: 'link' }, { parent_id: null },
    { original_path: undefined }, { original_path: [1] }, { needs_reference: 'yes' },
  ]) {
    const page = { items: [{ ...deletedFixture(0), ...invalid }], next_cursor: null, has_more: false } as unknown as TrashPage;
    const { api } = inspection([page]);
    await expect(api.deleted()).rejects.toThrow('Invalid Trash entry response');
  }
});

test('Trash byte ordering handles UTF-16 differences within and across pages', async () => {
  const first = { ...deletedFixture(0), name: '\uFF21.txt' };
  const second = { ...deletedFixture(1), name: '\u{1F4C1}.txt' };
  expect(first.name > second.name).toBe(true);
  const within = inspection([{ items: [first, second], next_cursor: null, has_more: false }]);
  expect((await within.api.deletedTargets([first.id, second.id], () => true)).map(item => item.name)).toEqual([first.name, second.name]);
  const across = inspection([
    { items: [first], next_cursor: 'next', has_more: true },
    { items: [second], next_cursor: null, has_more: false },
  ]);
  expect((await across.api.deletedTargets([first.id, second.id], () => true)).map(item => item.name)).toEqual([first.name, second.name]);
  expect(across.paths).toHaveLength(2);
});
