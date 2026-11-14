import { expect, test, type Page } from '@playwright/test';

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
    ...Array.from({ length: 100 }, (_, i) => ({ id: `item-${i}`, name: `Report ${i}.txt`, kind: 'file', size: 3, parent_id: 'root', original_path: [], etag: `${i}:1` })),
  ];
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
    if (path === '/trash') return json({ items });
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
      await expect(trash.getByRole('listitem')).toHaveCount(105);
      await expect(page.getByRole('button', { name: /Restore notes.txt.*item reference a/ })).toBeVisible();
      await expect(page.getByRole('button', { name: /Restore notes.txt.*item reference b/ })).toBeVisible();
      await expect(page.getByRole('button', { name: 'Restore notes.txt from Files / Home', exact: true })).toBeVisible();
      const row = trash.getByRole('listitem').filter({ hasText: longName });
      await expect(row.locator('svg')).toHaveCount(1);
      await expect(row).toContainText(longPath.join(' / '));
      expect(await row.evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      const restore = page.getByRole('button', { name: 'Restore Projects', exact: true });
      await restore.focus();
      await page.screenshot({ path: `/private/tmp/syl56-${width}-${theme}.png` });
      await page.keyboard.press('Enter');
      await expect(page.getByRole('alert')).toContainText('Restore its parent folder first');
      await expect(restore).toBeEnabled();
      await expect(restore).toBeFocused();
      backend.succeed();
      await restore.click();
      await expect(restore).toHaveCount(0);
      await expect(page.getByRole('heading', { name: 'Trash', exact: true })).toBeFocused();
      await expect(page.getByRole('status')).toContainText('Restored');
      backend.empty();
      await page.getByRole('button', { name: 'Refresh trash', exact: true }).click();
      await expect(trash.getByText('Trash is empty.', { exact: true })).toBeVisible();
      await expect(trash.getByRole('listitem')).toHaveCount(0);
    });
  }
}


test('refresh disables Restore until the updated Trash list arrives', async ({ page }) => {
  await openTrash(page);
  let held: import('@playwright/test').Route | undefined;
  await page.route('**/trash', route => { held = route; });
  await page.getByRole('button', { name: 'Refresh trash', exact: true }).click();
  await expect.poll(() => !!held).toBe(true);
  const restore = page.getByRole('button', { name: 'Restore Projects', exact: true });
  await expect(restore).toBeDisabled();
  await expect(page.getByRole('button', { name: /Restore notes.txt.*item reference a/ })).toBeDisabled();
  await held!.fulfill({ json: { items: deletedItems() } });
  await expect(restore).toBeEnabled();
});

test('a child can be restored after recovering its deleted parent', async ({ page }) => {
  await openTrash(page);
  const parent = { id: 'parent', name: 'Parent', kind: 'folder', parent_id: 'root', original_path: [], etag: 'parent:1' };
  const child = { id: 'child', name: 'Child.txt', kind: 'file', size: 3, parent_id: 'parent', original_path: ['Parent'], etag: 'child:1' };
  let parentDeleted = true;
  let childDeleted = true;
  await page.route('**/trash', route => route.fulfill({ json: { items: [...(parentDeleted ? [parent] : []), ...(childDeleted ? [child] : [])] } }));
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
