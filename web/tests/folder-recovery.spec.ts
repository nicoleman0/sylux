import { expect, test, type Page } from '@playwright/test';
import type { Entry, Folder } from '../src/lib/listing';
import { MockTransport } from '../src/lib/mock-transport';
import { HttpError } from '../src/lib/api';

type Item = Entry & { deleted?: boolean };

async function backend(page: Page, large = false) {
  const root: Folder = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
  const entries: Item[] = [
    { id: 'tree', parent_id: 'root', name: 'Tree', kind: 'folder', etag: '"tree:1"' },
    { id: 'nested', parent_id: 'tree', name: 'Nested', kind: 'folder', etag: '"nested:1"' },
    { id: 'file', parent_id: 'nested', name: 'exact.bin', kind: 'file', size: 7, etag: '"file:1"' },
    { id: 'earlier', parent_id: 'nested', name: 'earlier.bin', kind: 'file', size: 3, etag: '"earlier:2"', deleted: true },
  ];
  if (large) {
    entries.splice(1);
    for (let index = 0; index < 9; index++) entries.push({ id: `branch-${index}`, parent_id: 'tree', name: `Branch ${index}`, kind: 'folder', etag: `"branch-${index}:1"` });
    for (let index = 0; index < 990; index++) entries.push({ id: `leaf-${index}`, parent_id: `branch-${index % 9}`, name: `Leaf ${index}.bin`, kind: 'file', size: 7, etag: `"leaf-${index}:1"` });
  }
  const mutations: Array<{ method: string; id: string; etag?: string }> = [];
  let failure = 0;
  let applied = false;
  let block: Promise<void> | null = null;
  let release: (() => void) | null = null;
  let readsFail = false;
  let rootReadFail = false;
  let rootReadBlock: Promise<void> | null = null;
  let releaseRootRead: (() => void) | null = null;
  let afterTrashRead: (() => void) | null = null;
  let reverseSearch = false;
  let sequence = 10;
  const active = (entry: Item): boolean => {
    let current: Item | undefined = entry;
    const seen = new Set<string>();
    while (current) {
      if (current.deleted || seen.has(current.id)) return false;
      seen.add(current.id);
      if (current.parent_id === 'root') return true;
      current = entries.find(parent => parent.id === current!.parent_id);
    }
    return false;
  };
  const bump = (entry: Item) => { entry.etag = `"${entry.id}:${++sequence}"`; };
  const trash = (entry: Item) => {
    const pending = [entry];
    while (pending.length) { const current = pending.pop()!; bump(current); if (current.kind === 'folder') pending.push(...entries.filter(child => child.parent_id === current.id && !child.deleted)); }
    entry.deleted = true;
  };
  await page.route('**/*', async route => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (value: unknown, status = 200) => route.fulfill({ status, json: value });
    if (path === '/instance') return json({ name: 'Sylux' });
    if (path === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (path === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' });
    if (path === '/auth/logout') return json({});
    if (path === '/trash') {
      if (readsFail) return json({ code: 'unavailable' }, 503);
      const items = entries.filter(entry => entry.deleted).map(entry => {
        const original_path: string[] = []; let parent = entries.find(item => item.id === entry.parent_id);
        while (parent) { original_path.unshift(parent.name); parent = entries.find(item => item.id === parent!.parent_id); }
        return { ...entry, original_path };
      });
      const next = afterTrashRead; afterTrashRead = null; next?.();
      return json({ items: items.sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : a.id.localeCompare(b.id)), next_cursor: null, has_more: false });
    }
    if (path === '/entries/search') {
      const items = entries.filter(entry => active(entry) && entry.name.includes(url.searchParams.get('q') ?? '')).map(entry => {
        const location: Item[] = [];
        let parent = entries.find(item => item.id === entry.parent_id);
        while (parent) { location.unshift(parent); parent = entries.find(item => item.id === parent!.parent_id); }
        return { entry, location: [root, ...location] };
      });
      return json({ items: reverseSearch ? items.reverse() : items, next_cursor: null, has_more: false });
    }
    if (path === '/folders/children' || /^\/folders\/[^/]+\/children$/.test(path)) {
      const id = path === '/folders/children' ? 'root' : path.split('/')[2]!;
      if (path === '/folders/children') {
        if (rootReadBlock) await rootReadBlock;
        if (rootReadFail) return json({ code: 'unavailable' }, 503);
      }
      if (readsFail) return json({ code: 'unavailable' }, 503);
      if (id !== 'root' && !entries.some(entry => entry.id === id && entry.kind === 'folder' && active(entry))) return json({ code: 'not_found' }, 404);
      return json({ ...(id === 'root' ? { root } : {}), items: entries.filter(entry => entry.parent_id === id && active(entry)), next_cursor: null, has_more: false });
    }
    if (/^\/folders\/[^/]+\/path$/.test(path)) {
      const id = path.split('/')[2]!;
      let entry = entries.find(entry => entry.id === id);
      if (id === 'root') return json({ items: [root] });
      if (!entry || !active(entry)) return json({ code: 'not_found' }, 404);
      const items: Item[] = [];
      while (entry) { items.unshift(entry); entry = entries.find(parent => parent.id === entry!.parent_id); }
      return json({ items: [root, ...items] });
    }
    if (/^\/entries\/[^/]+$/.test(path) && request.method() === 'PATCH') {
      mutations.push({ method: 'PATCH', id: path.split('/')[2]!, etag: request.headers()['if-match'] });
      return json({ code: 'unexpected_mutation' }, 500);
    }
    if ((/^\/entries\/[^/]+$/.test(path) && request.method() === 'DELETE') || /^\/trash\/[^/]+\/restore$/.test(path)) {
      const id = path.split('/')[2]!;
      const entry = entries.find(entry => entry.id === id)!;
      const restore = request.method() === 'POST';
      mutations.push({ method: request.method(), id, etag: request.headers()['if-match'] });
      if (block) await block;
      if (!entry || (restore ? !entry.deleted : !active(entry))) return json({ code: 'not_found' }, 404);
      if (request.headers()['if-match'] !== entry.etag) return json({ code: 'stale_etag' }, 412);
      if (restore && entry.parent_id !== 'root' && !entries.some(parent => parent.id === entry.parent_id && active(parent))) return json({ code: 'parent_unavailable' }, 409);
      if (!failure || applied) { if (restore) { entry.deleted = false; bump(entry); } else trash(entry); }
      if (failure) return failure < 0 ? route.abort('failed') : json({ code: 'unavailable' }, failure);
      return route.fulfill({ status: 204 });
    }
    if (/^\/entries\/[^/]+\/content$/.test(path)) {
      const entry = entries.find(entry => entry.id === path.split('/')[2]);
      return entry && active(entry) ? route.fulfill({ body: 'bytes\0x', contentType: 'application/octet-stream' }) : json({ code: 'not_found' }, 404);
    }
    return route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Open Tree', exact: true })).toBeVisible();
  return { entries, mutations, trash, rootReadFail: (fail: boolean) => { rootReadFail = fail; }, holdRootRead: () => { rootReadBlock = new Promise<void>(resolve => { releaseRootRead = resolve; }); }, releaseRootRead: () => { releaseRootRead?.(); rootReadBlock = null; }, afterTrashRead: (callback: () => void) => { afterTrashRead = callback; }, reverseSearch: (reverse: boolean) => { reverseSearch = reverse; }, fail: (status: number, apply = false) => { failure = status; applied = apply; }, readsFail: (fail: boolean) => { readsFail = fail; }, hold: () => { block = new Promise<void>(resolve => { release = resolve; }); }, release: () => { release?.(); block = null; } };
}

async function themeForScreenshot(page: Page, theme: 'light' | 'dark') {
  await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
  if (theme === 'dark') await expect(page.locator('html')).toHaveClass(/dark/);
  else await expect(page.locator('html')).not.toHaveClass(/dark/);
  await page.evaluate(async () => {
    await Promise.all(document.getAnimations().filter(animation => animation.effect?.getComputedTiming().iterations !== Infinity).map(animation => animation.finished.catch(() => {})));
  });
}

async function navigate(page: Page, destination: 'Files' | 'Trash' | 'Account') {
  if ((page.viewportSize()?.width ?? 1280) < 768) await page.getByRole('button', { name: 'Toggle Sidebar', exact: true }).click();
  await page.getByRole('button', { name: destination, exact: true }).click();
  if ((page.viewportSize()?.width ?? 1280) < 768) { await page.keyboard.press('Escape'); await expect(page.locator('[data-mobile=true]')).toBeHidden(); }
}

async function confirmTrash(page: Page, name = 'Tree') {
  await page.getByRole('button', { name: `Options for ${name}`, exact: true }).click();
  await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('folder and its contents');
  return dialog;
}

for (const width of [1280, 390]) for (const theme of ['light', 'dark'] as const) {
  test(`folder tree confirmation, pending and recovery at ${width}px in ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 }); await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const store = await backend(page);
    const dialog = await confirmTrash(page);
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    expect(store.mutations).toEqual([]);
    await confirmTrash(page); store.hold();
    const button = page.getByRole('dialog').getByRole('button', { name: 'Move to trash', exact: true });
    await button.focus(); await page.keyboard.press('Enter');
    await expect(page.getByRole('button', { name: 'Moving to trash…', exact: true })).toBeDisabled();
    await expect(page.getByRole('dialog')).toHaveAttribute('aria-busy', 'true');
    expect(store.mutations).toHaveLength(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: test.info().outputPath(`folder-pending-${width}-${theme}.png`), animations: 'disabled' });
    store.release();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(page.getByRole('status').filter({ hasText: /^Moved to trash$/ })).toBeVisible();
    await navigate(page, 'Trash');
    await expect(page.getByRole('status').filter({ hasText: /^Moved to trash$/ })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Restore Tree', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Restore earlier.bin', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('Restore its parent folder first');
    store.hold(); await page.getByRole('button', { name: 'Restore Tree', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Restore Tree', exact: true })).toHaveText('Restoring…');
    await expect(page.getByRole('button', { name: 'Restore Tree', exact: true })).toHaveAttribute('aria-busy', 'true');
    store.release();
    await expect(page.getByRole('button', { name: 'Restore Tree', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Restore earlier.bin', exact: true })).toBeVisible();
    await page.screenshot({ path: test.info().outputPath(`folder-restored-${width}-${theme}.png`), animations: 'disabled' });
    await navigate(page, 'Files');
    await page.getByRole('button', { name: 'Open Tree', exact: true }).click();
    await page.getByRole('button', { name: 'Open Nested', exact: true }).click();
    await expect(page.getByRole('article', { name: 'exact.bin', exact: true })).toBeVisible();
    await expect(page.getByRole('article', { name: 'earlier.bin', exact: true })).toHaveCount(0);
  });
}

for (const [status, apply] of [[503, false], [503, true], [-1, true]] as const) test(`unknown trash inspects current state without retry, status=${status}, applied=${apply}`, async ({ page }) => {
  const store = await backend(page); store.fail(status, apply);
  await confirmTrash(page); await page.getByRole('dialog').getByRole('button', { name: 'Move to trash', exact: true }).click();
  const result = page.getByRole('region', { name: 'Unconfirmed file action' });
  await expect(result).toContainText('Outcome unconfirmed');
  await navigate(page, 'Account'); await expect(result).toBeVisible();
  await navigate(page, 'Trash'); await expect(result).toBeVisible();
  store.readsFail(true); await result.getByRole('button', { name: 'Check Files and Trash', exact: true }).click();
  await expect(result).toContainText('Could not check Files and Trash');
  await expect(result.getByRole('button', { name: 'Dismiss result', exact: true })).toHaveCount(0);
  if (!apply) for (const width of [1280, 390]) for (const theme of ['light', 'dark'] as const) {
    await page.setViewportSize({ width, height: 844 }); await themeForScreenshot(page, theme);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: test.info().outputPath(`folder-unconfirmed-${width}-${theme}.png`), animations: 'disabled' });
  }
  store.readsFail(false); await result.getByRole('button', { name: 'Check Files and Trash', exact: true }).click();
  await expect(result).toHaveCount(0);
  await expect(page.getByRole('status').filter({ hasText: `Tree is in ${apply ? 'Trash' : 'Files'}` })).toBeVisible();
  expect(store.mutations).toHaveLength(1);
  await page.getByRole('button', { name: 'Refresh trash', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: /Tree is in/ })).toHaveCount(0);
});

test('unknown child restore with a newly hidden ancestor has an explicit recovery route', async ({ page }) => {
  const store = await backend(page); store.fail(503, true);
  await navigate(page, 'Trash');
  await page.getByRole('button', { name: 'Restore earlier.bin', exact: true }).click();
  const result = page.getByRole('region', { name: 'Unconfirmed file action' }); await expect(result).toBeVisible();
  store.trash(store.entries.find(entry => entry.id === 'tree')!); store.fail(0);
  await result.getByRole('button', { name: 'Check Files and Trash', exact: true }).click();
  await expect(result).toContainText('original folder is unavailable');
  await result.getByRole('button', { name: 'Open Trash', exact: true }).click();
  await result.getByRole('button', { name: 'Dismiss result', exact: true }).click();
  await page.getByRole('button', { name: 'Restore Tree', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Restore Tree', exact: true })).toHaveCount(0);
  await navigate(page, 'Files');
  await page.getByRole('button', { name: 'Open Tree', exact: true }).click(); await page.getByRole('button', { name: 'Open Nested', exact: true }).click();
  await expect(page.getByRole('article', { name: 'earlier.bin', exact: true })).toBeVisible();
  expect(store.mutations.map(item => item.id)).toEqual(['earlier', 'tree']);
});

test('an already open rename cannot dispatch after an uncertain restore arrives', async ({ page }) => {
  const store = await backend(page); store.fail(503); store.hold();
  await navigate(page, 'Trash');
  await page.getByRole('button', { name: 'Restore earlier.bin', exact: true }).click();
  await navigate(page, 'Files');
  await expect(page.getByRole('status').filter({ hasText: 'Restoring earlier.bin…' })).toBeVisible();
  await page.getByRole('button', { name: 'Options for Tree', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Rename', exact: true }).click();
  await page.getByLabel('New name', { exact: true }).fill('New tree');
  store.release();
  await expect(page.getByRole('region', { name: 'Unconfirmed file action' })).toBeAttached();
  await page.getByRole('dialog').getByRole('button', { name: 'Save name', exact: true }).click();
  await expect(page.getByRole('dialog').getByRole('alert')).toContainText('check Files and Trash');
  expect(store.mutations.map(item => item.method)).toEqual(['POST']);
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.getByRole('button', { name: 'Check Files and Trash', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Unconfirmed file action' })).toHaveCount(0);
});

test('1000-entry mock tree acknowledges one pending action before completion', async ({ page }) => {
  const store = await backend(page, true); expect(store.entries).toHaveLength(1000);
  await confirmTrash(page); store.hold();
  const start = performance.now();
  await page.getByRole('dialog').getByRole('button', { name: 'Move to trash', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Moving to trash…', exact: true })).toBeDisabled();
  const pendingMs = performance.now() - start;
  expect(store.entries.filter(entry => entry.deleted)).toHaveLength(0);
  const completionStart = performance.now(); store.release();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Open Tree', exact: true })).toHaveCount(0);
  console.log(`FOLDER_RECOVERY_UI_COST entries=1000 visible_root_rows=1 pending_ack_ms=${pendingMs.toFixed(1)} completion_render_ms=${(performance.now() - completionStart).toFixed(1)} transport=mock browser=Chrome`);
  await navigate(page, 'Trash');
  await page.getByRole('button', { name: 'Restore Tree', exact: true }).click();
  expect(store.entries.filter(entry => entry.deleted)).toHaveLength(0);
  expect(store.mutations.map(item => item.id)).toEqual(['tree', 'tree']);
});

test('prototype root markers hide descendants and preserve earlier deletions', async () => {
  const mock = new MockTransport(); await mock.request('/auth/local', { method: 'POST', body: { handle: 'owner' } });
  const initial = await mock.request<{ root: Folder }>('/folders/children');
  const top = await mock.request<Folder>('/folders/children', { method: 'POST', body: { parent_id: initial.root.id, name: 'Tree' } });
  const child = await mock.request<Folder>('/folders/children', { method: 'POST', body: { parent_id: top.id, name: 'Child' } });
  const earlier = await mock.request<Folder>('/folders/children', { method: 'POST', body: { parent_id: child.id, name: 'Earlier' } });
  await mock.request(`/entries/${earlier.id}`, { method: 'DELETE', headers: { 'If-Match': earlier.etag } });
  const path = await mock.request<{ items: Folder[] }>(`/folders/${top.id}/path`);
  await mock.request(`/entries/${top.id}`, { method: 'DELETE', headers: { 'If-Match': path.items.at(-1)!.etag } });
  await expect(mock.request(`/folders/${child.id}/children`)).rejects.toBeInstanceOf(HttpError);
  const trash = await mock.request<{ items: Entry[] }>('/trash'); expect(trash.items).toHaveLength(2);
  await expect(mock.request(`/trash/${earlier.id}/restore`, { method: 'POST', headers: { 'If-Match': trash.items.find(entry => entry.id === earlier.id)!.etag } })).rejects.toMatchObject({ status: 409 });
  await mock.request(`/trash/${top.id}/restore`, { method: 'POST', headers: { 'If-Match': trash.items.find(entry => entry.id === top.id)!.etag } });
  const restored = await mock.request<{ items: Entry[] }>(`/folders/${child.id}/children`); expect(restored.items).toHaveLength(0);
  await expect(mock.request(`/entries/${child.id}`, { method: 'PATCH', headers: { 'If-Match': child.etag }, body: { name: 'stale' } })).rejects.toMatchObject({ status: 412 });
});

test('unconfirmed long names wrap on narrow screens in both themes', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const store = await backend(page);
  const name = 'w'.repeat(255);
  store.entries.find(entry => entry.id === 'tree')!.name = name;
  await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
  store.fail(503);
  await confirmTrash(page, name);
  await page.getByRole('dialog').getByRole('button', { name: 'Move to trash', exact: true }).click();
  const result = page.getByRole('region', { name: 'Unconfirmed file action' });
  await expect(result).toBeVisible();
  for (const theme of ['light', 'dark'] as const) {
    await themeForScreenshot(page, theme);
    await page.screenshot({ path: test.info().outputPath(`folder-unconfirmed-long-390-${theme}.png`), animations: 'disabled' });
    expect(await result.evaluate(element => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  }
  expect(store.mutations).toHaveLength(1);
});

for (const reverse of [false, true]) for (const lostReply of [false, true]) test(`search trash covers selected descendants once, childFirst=${reverse}, lostReply=${lostReply}`, async ({ page }) => {
  const store = await backend(page); store.reverseSearch(reverse);
  store.entries.find(entry => entry.id === 'tree')!.name = 'match-tree';
  store.entries.find(entry => entry.id === 'file')!.name = 'match-file.bin';
  await page.getByRole('searchbox').fill('match');
  const names = ['Select match-tree in Files', 'Select match-file.bin in Files / match-tree / Nested'];
  for (const name of reverse ? names.reverse() : names) await page.getByRole('checkbox', { name, exact: true }).check();
  if (lostReply) store.fail(-1, true);
  await page.getByRole('button', { name: 'Move selected to trash', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('listitem')).toHaveCount(1);
  await expect(dialog).toContainText('Folders include their contents');
  await dialog.getByRole('button', { name: 'Move to trash', exact: true }).click();
  const result = page.getByRole('region', { name: 'Trash results', exact: true });
  if (lostReply) {
    await expect(result).toContainText('1 unconfirmed');
    await result.getByRole('button', { name: 'Refresh before retry', exact: true }).click();
  }
  await expect(result.getByRole('status')).toHaveText('Moved to trash');
  await expect(result.getByRole('button', { name: 'Refresh before retry', exact: true })).toHaveCount(0);
  expect(store.mutations.map(item => item.id)).toEqual(['tree']);
  await navigate(page, 'Trash');
  store.fail(0); await page.getByRole('button', { name: 'Restore match-tree', exact: true }).click();
  expect(store.entries.find(entry => entry.id === 'file')!.deleted).not.toBe(true);
  expect(store.mutations.map(item => item.id)).toEqual(['tree', 'tree']);
});

for (const [status, apply] of [[503, false], [503, true], [-1, false], [-1, true]] as const) test(`ordinary unknown restore inspects without replay, status=${status}, applied=${apply}`, async ({ page }) => {
  const store = await backend(page); await navigate(page, 'Trash'); store.fail(status, apply);
  await page.getByRole('button', { name: 'Restore earlier.bin', exact: true }).click();
  const result = page.getByRole('region', { name: 'Unconfirmed file action' }); await expect(result).toContainText('Restore unconfirmed');
  await result.getByRole('button', { name: 'Check Files and Trash', exact: true }).focus(); await page.keyboard.press('Enter');
  await expect(result).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Trash', exact: true })).toBeFocused();
  await expect(page.getByRole('status').filter({ hasText: `earlier.bin is in ${apply ? 'Files' : 'Trash'}` })).toBeVisible();
  expect(store.mutations.map(item => ({ method: item.method, id: item.id }))).toEqual([{ method: 'POST', id: 'earlier' }]);
  if (apply) {
    await navigate(page, 'Files'); await page.getByRole('button', { name: 'Open Tree', exact: true }).click(); await page.getByRole('button', { name: 'Open Nested', exact: true }).click();
    await expect(page.getByRole('article', { name: 'earlier.bin', exact: true })).toBeVisible();
  } else await expect(page.getByRole('button', { name: 'Restore earlier.bin', exact: true })).toBeVisible();
});

test('inspection keeps a visible recovery route when the following root refresh fails', async ({ page }) => {
  const store = await backend(page); store.fail(503, true); await confirmTrash(page); await page.getByRole('dialog').getByRole('button', { name: 'Move to trash', exact: true }).click();
  store.rootReadFail(true);
  const result = page.getByRole('region', { name: 'Unconfirmed file action' });
  await result.getByRole('button', { name: 'Check Files and Trash', exact: true }).click();
  await expect(result).toContainText('Found in Trash, but this view could not refresh');
  await expect(result.getByRole('button', { name: 'Check Files and Trash', exact: true })).toBeEnabled();
  await expect(page.getByRole('button', { name: 'New folder', exact: true })).toBeDisabled();
  store.rootReadFail(false); await result.getByRole('button', { name: 'Check Files and Trash', exact: true }).click();
  await expect(result).toHaveCount(0); await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeFocused();
  await expect(page.getByRole('button', { name: 'Open Tree', exact: true })).toHaveCount(0);
  expect(store.mutations).toHaveLength(1);
});

test('inspection does not move focus away from a newer interaction', async ({ page }) => {
  const store = await backend(page); store.fail(503); await confirmTrash(page); await page.getByRole('dialog').getByRole('button', { name: 'Move to trash', exact: true }).click();
  store.holdRootRead();
  const result = page.getByRole('region', { name: 'Unconfirmed file action' });
  await result.getByRole('button', { name: 'Check Files and Trash', exact: true }).click();
  await expect(result).toContainText('Found in Files. Refreshing this view');
  const search = page.getByRole('searchbox'); await search.focus(); store.releaseRootRead();
  await expect(result).toHaveCount(0); await expect(search).toBeFocused(); expect(store.mutations).toHaveLength(1);
});

test('contradictory Trash and Files reads require another inspection', async ({ page }) => {
  const store = await backend(page); await navigate(page, 'Trash'); store.fail(503);
  await page.getByRole('button', { name: 'Restore earlier.bin', exact: true }).click();
  store.afterTrashRead(() => { const entry = store.entries.find(entry => entry.id === 'earlier')!; entry.deleted = false; entry.etag = '"earlier:3"'; });
  const result = page.getByRole('region', { name: 'Unconfirmed file action' });
  await result.getByRole('button', { name: 'Check Files and Trash', exact: true }).click();
  await expect(result).toContainText('Files and Trash changed while checking');
  await expect(result.getByRole('button', { name: 'Dismiss result', exact: true })).toHaveCount(0);
  await expect(page.getByRole('status').filter({ hasText: /earlier.bin is in/ })).toHaveCount(0);
  await result.getByRole('button', { name: 'Check Files and Trash', exact: true }).click();
  await expect(result).toHaveCount(0); await expect(page.getByRole('status').filter({ hasText: 'earlier.bin is in Files' })).toBeVisible();
  expect(store.mutations).toHaveLength(1);
});

test('prototype create and replacement commits replay stored results before namespace guards', async () => {
  const mock = new MockTransport(); await mock.request('/auth/local', { method: 'POST', body: { handle: 'owner' } });
  const root = (await mock.request<{ root: Folder }>('/folders/children')).root;
  const upload = async (name: string, replacement?: { entry_id: string; etag: string }) => {
    const parent = (await mock.request<{ root: Folder }>('/folders/children')).root;
    const ticket = await mock.request<{ ticket: string }>('/mutation-tickets', { method: 'POST' });
    const admission = { parent_id: parent.id, parent_etag: parent.etag, name, length: 3, replacement };
    const headers = { 'X-Mutation-ID': ticket.ticket };
    const admitted = await mock.request<{ id: string }>('/uploads', { method: 'POST', headers, body: admission });
    await mock.request(`/uploads/${admitted.id}/content`, { method: 'PUT', body: new Blob(['abc']) });
    const committed = await mock.request(`/uploads/${admitted.id}/commit`, { method: 'POST' });
    const after = await mock.request(`/folders/${root.id}/children`);
    expect(await mock.request(`/uploads/${admitted.id}/commit`, { method: 'POST' })).toEqual(committed);
    expect(await mock.request(`/folders/${root.id}/children`)).toEqual(after);
    expect(await mock.request('/uploads', { method: 'POST', headers, body: admission })).toEqual(admitted);
    expect(await mock.request(`/uploads/${admitted.id}`)).toMatchObject({ state: 'committed' });
  };
  await upload('fresh.bin');
  const created = (await mock.request<{ items: Entry[] }>(`/folders/${root.id}/children`)).items.find(entry => entry.name === 'fresh.bin')!;
  await upload('fresh.bin', { entry_id: created.id, etag: created.etag });
  const trash = await mock.request<{ items: Entry[] }>('/trash'); expect(trash.items).toHaveLength(1);
  expect((await mock.request<{ items: Entry[] }>(`/folders/${root.id}/children`)).items.filter(entry => entry.name === 'fresh.bin')).toHaveLength(1);
});

test('a newer destination keeps its focus while inspection waits for a read', async ({ page }) => {
  const store = await backend(page); store.fail(503); await confirmTrash(page); await page.getByRole('dialog').getByRole('button', { name: 'Move to trash', exact: true }).click();
  store.holdRootRead();
  const result = page.getByRole('region', { name: 'Unconfirmed file action' });
  await result.getByRole('button', { name: 'Check Files and Trash', exact: true }).click();
  await expect(result).toContainText('Found in Files. Refreshing this view');
  await navigate(page, 'Account');
  const heading = page.getByRole('heading', { name: 'Account', exact: true }); await expect(heading).toBeFocused();
  store.releaseRootRead();
  await expect(result).toContainText('Check again to refresh the current view'); await expect(heading).toBeFocused();
  await result.getByRole('button', { name: 'Check Files and Trash', exact: true }).click();
  await expect(result).toHaveCount(0); await expect(heading).toBeFocused(); expect(store.mutations).toHaveLength(1);
});
