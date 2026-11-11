import { expect, test, type Page } from '@playwright/test';

async function installMoveBackend(page: Page) {
  const root = { id: 'root', parent_id: null, name: 'My files', kind: 'folder', etag: '"root:1"' };
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
      if (failure) return json({ code: failure }, failure === 'not_found' ? 404 : 409);
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
  await expect(page.getByRole('article').filter({ hasText: 'notes.bin' })).toBeVisible();
  return { moves, setFailure: (code: string | null) => { failure = code; } };
}

test('file moves use a nested destination and explain collisions on narrow screens', async ({ page }) => {
  const backend = await installMoveBackend(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole('button', { name: 'Move notes.bin', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('button', { name: 'Move here', exact: true })).toBeDisabled();
  await dialog.getByRole('button', { name: 'Destination', exact: true }).click();
  await dialog.getByRole('button', { name: 'Nested', exact: true }).click();
  await expect(dialog.getByText('Destination: My files / Destination / Nested', { exact: true })).toBeVisible();
  backend.setFailure('name_conflict');
  await dialog.getByRole('button', { name: 'Move here', exact: true }).click();
  await expect(dialog.getByRole('alert')).toContainText('Choose another folder');
  await expect(page.getByRole('article').filter({ hasText: 'notes.bin' })).toBeVisible();
  backend.setFailure(null);
  await dialog.getByRole('button', { name: 'Move here', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('status')).toContainText('notes.bin moved to Nested');
  await expect(page.getByRole('article').filter({ hasText: 'notes.bin' })).toHaveCount(0);
  expect(backend.moves).toEqual([
    { id: 'file', parent: 'nested', etag: '"file:1"' },
    { id: 'file', parent: 'nested', etag: '"file:1"' },
  ]);
  await page.getByRole('button', { name: 'Open Destination', exact: true }).click();
  await page.getByRole('button', { name: 'Open Nested', exact: true }).click();
  await expect(page.getByRole('article').filter({ hasText: 'notes.bin' })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});

test('folder picker prevents cycles and returns focus after keyboard dismissal', async ({ page }) => {
  const backend = await installMoveBackend(page);
  const trigger = page.getByRole('button', { name: 'Move Destination', exact: true });
  await trigger.click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('button', { name: 'Destination (cannot move into itself)', exact: true })).toBeDisabled();
  await expect(dialog.getByRole('button', { name: 'Move here', exact: true })).toBeDisabled();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(trigger).toBeFocused();
  await page.getByRole('button', { name: 'Move notes.bin', exact: true }).click();
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
  await page.getByRole('button', { name: 'Move notes.bin', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('button', { name: 'Destination', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Move here', exact: true })).toBeEnabled();
  await dialog.getByRole('button', { name: 'Nested', exact: true }).click();
  await expect(dialog.getByRole('alert')).toContainText('Could not open this destination');
  await expect(dialog.getByText('Destination: My files / Destination / Nested', { exact: true })).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Move here', exact: true })).toBeDisabled();
  expect(backend.moves).toHaveLength(0);
  unavailable = false;
  await dialog.getByRole('button', { name: 'Refresh destinations', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Move here', exact: true })).toBeEnabled();
  await dialog.getByRole('button', { name: 'Move here', exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(backend.moves).toEqual([{ id: 'file', parent: 'nested', etag: '"file:1"' }]);
});
