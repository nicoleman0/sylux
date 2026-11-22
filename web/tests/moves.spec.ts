import { expect, test, type Page, type Route } from '@playwright/test';

async function installMoveBackend(page: Page) {
  const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
  const entries = [
    { id: 'file', parent_id: 'root', name: 'notes.bin', kind: 'file', size: 3, etag: '"file:1"' },
    { id: 'destination', parent_id: 'root', name: 'Destination', kind: 'folder', etag: '"destination:1"' },
    { id: 'nested', parent_id: 'destination', name: 'Nested', kind: 'folder', etag: '"nested:1"' },
  ];
  const moves: Array<{ id: string; parent: string; etag?: string }> = [];
  let failure: string | null = null;
  await page.route('**/*', async route => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (value: unknown, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(value) });
    if (path === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (path === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' });
    if (path === '/folders/children') return json({ root, items: entries.filter(entry => entry.parent_id === 'root'), next_cursor: null, has_more: false });
    if (/^\/folders\/[^/]+\/children$/.test(path)) return json({ items: entries.filter(entry => entry.parent_id === path.split('/')[2]), next_cursor: null, has_more: false });
    if (/^\/entries\/[^/]+\/move$/.test(path)) {
      const id = path.split('/')[2]!;
      const parent = (request.postDataJSON() as { parent_id: string }).parent_id;
      moves.push({ id, parent, etag: request.headers()['if-match'] });
      if (failure) return json({ code: failure }, failure === 'not_found' ? 404 : failure === 'unavailable' ? 503 : 409);
      const entry = entries.find(entry => entry.id === id)!;
      entry.parent_id = parent; entry.etag = `"${id}:2"`;
      return json(entry);
    }
    await route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.locator('.file-row').filter({ hasText: 'notes.bin' })).toBeVisible();
  return { entries, moves, setFailure: (code: string | null) => { failure = code; } };
}

test('file moves stay pending until the request settles and explain collisions on narrow screens', async ({ page }, testInfo) => {
  const backend = await installMoveBackend(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole('button', { name: 'Options for notes.bin', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Move', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('button', { name: 'Move here', exact: true })).toBeDisabled();
  await dialog.getByRole('button', { name: 'Destination', exact: true }).click();
  await dialog.getByRole('button', { name: 'Nested', exact: true }).click();
  const breadcrumb = dialog.getByRole('navigation', { name: 'Move destination path' });
  await expect(breadcrumb.locator('[aria-current="location"]')).toHaveText('Nested');
  await expect(dialog.getByText('Nested', { exact: true })).toHaveCount(1);
  let held: Route | undefined;
  await page.route('**/entries/file/move', route => { held = route; });
  backend.setFailure('name_conflict');
  const confirm = dialog.getByRole('button', { name: 'Move here', exact: true });
  await expect(confirm).toBeEnabled();
  await confirm.focus();
  const requests: string[] = [];
  page.on('request', request => {
    if (request.url().endsWith('/entries/file/move')) requests.push(request.url());
  });
  await page.keyboard.press('Enter');
  await expect.poll(() => !!held).toBe(true);
  const pending = dialog.getByRole('button', { name: 'Moving…', exact: true });
  await expect(pending).toBeDisabled();
  await expect(pending).toHaveAttribute('aria-busy', 'true');
  await expect(pending).toBeFocused();
  await page.keyboard.press('Enter');
  await page.keyboard.press('Escape');
  await expect(pending).toBeFocused();
  await page.mouse.click(2, 2);
  await expect(dialog).toBeVisible();
  await dialog.screenshot({ path: testInfo.outputPath('move-narrow-pending.png'), animations: 'disabled' });
  await held!.fallback();
  await page.unroute('**/entries/file/move');
  await expect(dialog.getByRole('alert')).toContainText('Choose another folder');
  expect(requests).toHaveLength(1);
  await dialog.screenshot({ path: testInfo.outputPath('move-narrow-conflict.png'), animations: 'disabled' });
  await expect(page.locator('.file-row').filter({ hasText: 'notes.bin' })).toBeVisible();
  backend.setFailure(null);
  await dialog.getByRole('button', { name: 'Move here', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('status')).toContainText('Moved to Nested');
  await expect(page.locator('.file-row').filter({ hasText: 'notes.bin' })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeFocused();
  await page.screenshot({ path: testInfo.outputPath('move-narrow-success.png'), animations: 'disabled' });
  expect(backend.moves).toEqual([
    { id: 'file', parent: 'nested', etag: '"file:1"' },
    { id: 'file', parent: 'nested', etag: '"file:1"' },
  ]);
  await page.getByRole('button', { name: 'Open Destination', exact: true }).click();
  await page.getByRole('button', { name: 'Open Nested', exact: true }).click();
  await expect(page.locator('.file-row').filter({ hasText: 'notes.bin' })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});

test('folder picker hides a selected branch and keeps siblings and ancestors reachable by keyboard', async ({ page }, testInfo) => {
  const backend = await installMoveBackend(page);
  backend.entries.push(
    { id: 'descendant', parent_id: 'nested', name: 'Descendant', kind: 'folder', etag: '"descendant:1"' },
    { id: 'sibling', parent_id: 'destination', name: 'Sibling', kind: 'folder', etag: '"sibling:1"' },
  );
  const visited: string[] = [];
  page.on('request', request => {
    const match = new URL(request.url()).pathname.match(/^\/folders\/([^/]+)\/children$/);
    if (match) visited.push(match[1]!);
  });
  await page.getByRole('button', { name: 'Open Destination', exact: true }).click();
  const trigger = page.getByRole('button', { name: 'Options for Nested', exact: true });
  const dialog = page.getByRole('dialog');
  const breadcrumb = dialog.getByRole('navigation', { name: 'Move destination path' });
  const confirm = dialog.getByRole('button', { name: 'Move here', exact: true });
  for (const width of [390, 1280]) {
    for (const theme of ['light', 'dark'] as const) {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await trigger.focus();
      await page.keyboard.press('Enter');
      const move = page.getByRole('menuitem', { name: 'Move', exact: true });
      await move.focus();
      await page.keyboard.press('Enter');
      await expect(confirm).toBeEnabled();
      await dialog.getByRole('button', { name: 'Destination', exact: true }).focus();
      await page.keyboard.press('Enter');
      await expect(breadcrumb.locator('[aria-current="location"]')).toHaveText('Destination');
      await expect(confirm).toBeDisabled();
      await expect(dialog.getByRole('group', { name: 'Destination folders' }).getByText(/Nested|Descendant/)).toHaveCount(0);
      const sibling = dialog.getByRole('button', { name: 'Sibling', exact: true });
      await sibling.focus();
      await expect(sibling).toBeFocused();
      await expect(page.locator('html')).toHaveClass(theme === 'dark' ? /dark/ : '');
      expect(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
      await dialog.screenshot({ path: testInfo.outputPath(`move-${width}-${theme}.png`), animations: 'disabled' });
      await page.keyboard.press('Enter');
      await expect(breadcrumb.locator('[aria-current="location"]')).toHaveText('Sibling');
      await expect(confirm).toBeEnabled();
      await breadcrumb.getByRole('button', { name: 'Files', exact: true }).focus();
      await page.keyboard.press('Enter');
      await expect(breadcrumb.locator('[aria-current="location"]')).toHaveText('Files');
      await expect(confirm).toBeEnabled();
      await page.keyboard.press('Escape');
      await expect(dialog).toBeHidden();
      await expect(trigger).toBeFocused();
    }
  }
  expect(visited).not.toContain('nested');
  expect(visited).not.toContain('descendant');
  expect(backend.moves).toEqual([]);
});

test('a destination that disappears is reported by the Move picker', async ({ page }) => {
  const backend = await installMoveBackend(page);
  await page.getByRole('button', { name: 'Options for notes.bin', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Move', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('button', { name: 'Destination', exact: true }).click();
  backend.setFailure('not_found');
  await dialog.getByRole('button', { name: 'Move here', exact: true }).click();
  await expect(dialog.getByRole('alert')).toContainText('no longer available');
  await expect(dialog.getByRole('button', { name: 'Refresh destinations' })).toBeVisible();
});

test('failed destination navigation blocks moving until that destination loads', async ({ page }) => {
  const backend = await installMoveBackend(page);
  let unavailable = true;
  await page.route('**/folders/nested/children*', route => unavailable
    ? route.fulfill({ status: 503, json: { code: 'unavailable' } })
    : route.fulfill({ json: { items: [], next_cursor: null, has_more: false } }));
  await page.getByRole('button', { name: 'Options for notes.bin', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Move', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('button', { name: 'Destination', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Move here', exact: true })).toBeEnabled();
  await dialog.getByRole('button', { name: 'Nested', exact: true }).click();
  await expect(dialog.getByRole('alert')).toContainText('Could not open this destination');
  await expect(dialog.getByRole('navigation', { name: 'Move destination path' }).locator('[aria-current="location"]')).toHaveText('Nested');
  await expect(dialog.getByRole('button', { name: 'Move here', exact: true })).toBeDisabled();
  expect(backend.moves).toHaveLength(0);
  unavailable = false;
  await dialog.getByRole('button', { name: 'Refresh destinations', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Move here', exact: true })).toBeEnabled();
  await dialog.getByRole('button', { name: 'Move here', exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(backend.moves).toEqual([{ id: 'file', parent: 'nested', etag: '"file:1"' }]);
});

async function dragEntry(page: Page, sourceName: string, targetName: string) {
  const source = page.getByRole('row', { name: sourceName, exact: true });
  const target = page.getByRole('row', { name: targetName, exact: true });
  const dataTransfer = await page.evaluateHandle(() => new DataTransfer());
  await source.dispatchEvent('dragstart', { dataTransfer });
  await target.dispatchEvent('dragover', { dataTransfer });
  await target.dispatchEvent('drop', { dataTransfer });
  await source.dispatchEvent('dragend', { dataTransfer });
  await dataTransfer.dispose();
}

test('internal dragging moves a file into a folder with its current ETag', async ({ page }) => {
  const backend = await installMoveBackend(page);
  await dragEntry(page, 'notes.bin', 'Destination');
  await expect.poll(() => backend.moves).toEqual([{ id: 'file', parent: 'destination', etag: '"file:1"' }]);
  await expect(page.getByRole('status').filter({ hasText: 'Moved to Destination' })).toBeVisible();
  await expect(page.getByRole('row', { name: 'notes.bin', exact: true })).toHaveCount(0);
});

test('invalid drops do not mutate storage and failed drops retain the source', async ({ page }) => {
  const backend = await installMoveBackend(page);
  await dragEntry(page, 'Destination', 'Destination');
  await dragEntry(page, 'Destination', 'notes.bin');
  expect(backend.moves).toEqual([]);
  backend.setFailure('name_conflict');
  await dragEntry(page, 'notes.bin', 'Destination');
  await expect(page.getByRole('alert')).toContainText(/name|another folder/i);
  await expect(page.getByRole('row', { name: 'notes.bin', exact: true })).toBeVisible();
  expect(backend.moves).toEqual([{ id: 'file', parent: 'destination', etag: '"file:1"' }]);
});


test('unknown entry drags cannot trigger moves', async ({ page }) => {
  const backend = await installMoveBackend(page);
  const destination = page.getByRole('row', { name: 'Destination', exact: true });
  const dataTransfer = await page.evaluateHandle(() => {
    const data = new DataTransfer();
    data.setData('application/x-sylux-entry', 'unknown-id');
    return data;
  });
  await destination.dispatchEvent('dragover', { dataTransfer });
  await expect(destination).toHaveAttribute('data-drop-active', 'false');
  await destination.dispatchEvent('drop', { dataTransfer });
  await dataTransfer.dispose();
  expect(backend.moves).toEqual([]);
  await expect(page.getByRole('row', { name: 'notes.bin', exact: true })).toBeVisible();
});

test('an uncertain drag move blocks retry until files are refreshed', async ({ page }) => {
  const backend = await installMoveBackend(page);
  backend.setFailure('unavailable');
  await dragEntry(page, 'notes.bin', 'Destination');
  await expect(page.getByRole('alert')).toContainText('Move could not be confirmed');
  const source = page.getByRole('row', { name: 'notes.bin', exact: true });
  await expect(source).toHaveAttribute('draggable', 'false');
  await dragEntry(page, 'notes.bin', 'Destination');
  expect(backend.moves).toHaveLength(1);
  backend.setFailure(null);
  await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
  await expect(source).toHaveAttribute('draggable', 'true');
  await dragEntry(page, 'notes.bin', 'Destination');
  await expect(page.getByRole('status').filter({ hasText: 'Moved to Destination' })).toBeVisible();
  expect(backend.moves).toHaveLength(2);
});

for (const boundary of ['dismiss', 'navigation']) {
  test(`a delayed move revalidation failure does not resurrect feedback after ${boundary}`, async ({ page }) => {
    await installMoveBackend(page);
    await page.getByRole('button', { name: 'Options for notes.bin', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Move', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('button', { name: 'Destination', exact: true }).click();
    let release!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    let done!: () => void;
    const finished = new Promise<void>(resolve => { done = resolve; });
    await page.route('**/folders/children?*', async route => { await held; await route.abort('failed'); done(); });
    await dialog.getByRole('button', { name: 'Move here', exact: true }).click();
    await expect(page.getByRole('status').filter({ hasText: 'Moved to Destination' })).toBeVisible();
    if (boundary === 'dismiss') {
      await page.getByRole('button', { name: 'Dismiss notification', exact: true }).focus();
      await page.keyboard.press('Enter');
    } else await page.getByRole('button', { name: 'Account', exact: true }).click();
    release(); await finished;
    await expect(page.locator('.workspace-feedback')).toHaveCount(0);
  });
}
