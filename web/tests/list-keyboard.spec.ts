import { expect, test, type Page } from '@playwright/test';
import { mockPrivateLibraries, signInLibraryOwner } from './fixtures/private-libraries';

test('library creation returns focus and a library opens from its keyboard-reachable link', async ({ page }) => {
  await mockPrivateLibraries(page);
  await page.goto('/#libraries');
  await signInLibraryOwner(page);
  const create = page.getByRole('button', { name: 'New library', exact: true });
  await create.press('Enter');
  const dialog = page.getByRole('dialog', { name: 'New library', exact: true });
  await dialog.getByLabel('Library name').fill('Studio');
  await dialog.getByRole('button', { name: 'Create library' }).press('Enter');
  await expect(dialog.getByRole('alert')).toHaveText('You already have a library with this name.');
  await dialog.getByLabel('Library name').fill('Reading');
  await dialog.getByLabel('Library name').press('Enter');
  await expect(dialog).toBeHidden();
  await expect(create).toBeFocused();
  await expect(page.getByRole('status').filter({ hasText: 'Created Reading.' })).toBeVisible();
  const link = page.getByRole('link', { name: 'Reading', exact: true });
  await link.focus();
  await expect(link).toHaveCSS('outline-style', 'solid');
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: 'Reading', exact: true })).toBeFocused();
  await expect(page.getByText('This folder is empty')).toBeVisible();
});

const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
const names = ['alpha', 'beta', 'gamma', 'delta'];

async function open(page: Page) {
  const entries = names.map(name => ({ id: name, parent_id: 'root', name: `${name}.txt`, kind: 'file', size: 3, etag: `"${name}:1"` }));
  const trashed: string[] = [];
  const renamed: { id: string; name: string }[] = [];
  await page.route('**/*', route => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown) => route.fulfill({ json: body });
    if (url.pathname === '/instance') return json({ name: 'Sylux' });
    if (url.pathname === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (url.pathname === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user' });
    if (url.pathname === '/folders/children' || url.pathname === '/folders/root/children') {
      return json({ ...(url.pathname === '/folders/children' ? { root } : {}), items: entries.filter(entry => !trashed.includes(entry.id)), next_cursor: null, has_more: false });
    }
    if (url.pathname === '/entries/search') return json({ items: [], next_cursor: null, has_more: false });
    const id = url.pathname.split('/')[2];
    if (request.method() === 'DELETE' && url.pathname.startsWith('/entries/')) { trashed.push(id); return route.fulfill({ status: 204 }); }
    if (request.method() === 'PATCH' && url.pathname.startsWith('/entries/')) {
      const entry = entries.find(candidate => candidate.id === id)!;
      entry.name = (request.postDataJSON() as { name: string }).name;
      renamed.push({ id, name: entry.name });
      return json(entry);
    }
    return route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Preview alpha.txt', exact: true })).toBeVisible();
  return { trashed, renamed };
}

const item = (page: Page, name: string) => page.getByRole('button', { name: `Preview ${name}.txt`, exact: true });
const box = (page: Page, name: string) => page.getByRole('checkbox', { name: `Select ${name}.txt`, exact: true });

test('the footer shortcut button is keyboard reachable and regains focus after help', async ({ page }) => {
  await open(page);
  const trigger = page.getByRole('button', { name: 'Keyboard shortcuts', exact: true });
  await expect(trigger).toHaveCount(1);
  await expect(page.locator('footer').getByRole('button', { name: 'Keyboard shortcuts', exact: true })).toBeVisible();
  await trigger.focus();
  await page.keyboard.press('Shift+Tab');
  await expect(trigger).not.toBeFocused();
  await page.keyboard.press('Tab');
  await expect(trigger).toBeFocused();
  await expect(trigger).toHaveCSS('outline-style', 'solid');
  await page.keyboard.press('Enter');
  const help = page.getByRole('dialog', { name: 'Keyboard shortcuts', exact: true });
  await expect(help.getByRole('button', { name: 'Close', exact: true })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(help).toBeHidden();
  await expect(trigger).toBeFocused();

  // Some pointer activations leave focus on the previous control.
  await item(page, 'delta').focus();
  await trigger.evaluate(button => button.click());
  await help.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(help).toBeHidden();
  await expect(trigger).toBeFocused();
});

test('arrows move focus, shift extends the selection, escape clears it', async ({ page }) => {
  await open(page);
  await item(page, 'alpha').focus();
  await page.keyboard.press('ArrowDown');
  await expect(item(page, 'beta')).toBeFocused();
  await expect(item(page, 'beta')).toHaveCSS('outline-style', 'solid');
  await page.keyboard.press('Shift+ArrowDown');
  await page.keyboard.press('Shift+ArrowDown');
  await expect(item(page, 'delta')).toBeFocused();
  await expect(box(page, 'alpha')).not.toBeChecked();
  for (const name of ['beta', 'gamma', 'delta']) await expect(box(page, name)).toBeChecked();
  await page.keyboard.press('Shift+ArrowUp');
  await expect(box(page, 'delta')).not.toBeChecked();
  await page.keyboard.press('End');
  await expect(item(page, 'delta')).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('group', { name: 'Selected items' })).toHaveCount(0);
  await page.keyboard.press('Home');
  await expect(item(page, 'alpha')).toBeFocused();
  await page.keyboard.press('x');
  await expect(box(page, 'alpha')).toBeChecked();
});

test('Enter opens a file and Escape returns focus to the item', async ({ page }) => {
  await open(page);
  await item(page, 'beta').focus();
  await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog', { name: /beta\.txt/ });
  await expect(dialog).toBeVisible();
  // The reader owns the keyboard: list keys do nothing.
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Delete');
  await expect(page.getByRole('alertdialog')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(item(page, 'beta')).toBeFocused();
});

test('F2 renames and Delete asks before trashing the focused item', async ({ page }) => {
  const backend = await open(page);
  await item(page, 'gamma').focus();
  await page.keyboard.press('F2');
  const rename = page.getByRole('dialog', { name: 'Rename item' });
  await expect(rename.getByLabel('New name')).toBeFocused();
  await rename.getByLabel('New name').fill('omega.txt');
  // Typing keys must not trigger list shortcuts.
  await page.keyboard.press('Delete');
  await expect(rename.getByLabel('New name')).toHaveValue('omega.txt');
  await rename.getByRole('button', { name: 'Save name' }).click();
  await expect(rename).toBeHidden();
  expect(backend.renamed).toEqual([{ id: 'gamma', name: 'omega.txt' }]);
  await expect(page.getByRole('button', { name: 'Preview omega.txt', exact: true })).toBeVisible();

  await item(page, 'delta').focus();
  await page.keyboard.press('Delete');
  const confirm = page.getByRole('dialog', { name: 'Delete delta.txt?' });
  await expect(confirm).toBeVisible();
  expect(backend.trashed).toEqual([]);
  await page.keyboard.press('Escape');
  await expect(confirm).toBeHidden();
  await expect(item(page, 'delta')).toBeFocused();
  expect(backend.trashed).toEqual([]);
  await page.keyboard.press('Delete');
  await confirm.getByRole('button', { name: 'Move to trash' }).click();
  await expect.poll(() => backend.trashed).toEqual(['delta']);
});

test('Delete on a selection uses the bulk confirmation', async ({ page }) => {
  const backend = await open(page);
  await item(page, 'alpha').focus();
  await page.keyboard.press('Shift+ArrowDown');
  await page.keyboard.press('Delete');
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('alpha.txt');
  await expect(dialog).toContainText('beta.txt');
  expect(backend.trashed).toEqual([]);
});

test('slash focuses search, but not while typing; question mark lists shortcuts', async ({ page }) => {
  await open(page);
  await item(page, 'alpha').focus();
  await page.keyboard.press('/');
  const search = page.getByLabel('Search files and folders');
  await expect(search).toBeFocused();
  await page.keyboard.type('a/b?');
  await expect(search).toHaveValue('a/b?');
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('slash and question mark do nothing while focus is outside the list', async ({ page }) => {
  await open(page);
  const refresh = page.getByRole('button', { name: 'Refresh folder' });
  await refresh.focus();
  await page.keyboard.press('/');
  await page.keyboard.press('Shift+?');
  await expect(page.getByLabel('Search files and folders')).not.toBeFocused();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(refresh).toBeFocused();
});

test('question mark opens the shortcut list and Escape returns focus', async ({ page }) => {
  await open(page);
  await item(page, 'alpha').focus();
  await page.keyboard.press('Shift+?');
  const help = page.getByRole('dialog', { name: 'Keyboard shortcuts' });
  await expect(help).toBeVisible();
  await expect(help).toContainText('F2');
  await page.keyboard.press('Escape');
  await expect(help).toBeHidden();
  await expect(item(page, 'alpha')).toBeFocused();
});

test('focus stays on the item through a refresh', async ({ page }) => {
  await open(page);
  await item(page, 'beta').focus();
  await page.getByRole('button', { name: 'Refresh folder' }).evaluate(button => (button as HTMLButtonElement).click());
  await expect(page.getByRole('button', { name: 'Refresh folder' })).toBeEnabled();
  await expect(item(page, 'beta')).toBeFocused();
});

test('fresh Files shows how to focus the first item from the folder heading', async ({ page }) => {
  await open(page);
  const heading = page.getByRole('heading', { name: 'Files', level: 1 });
  const hint = page.getByText('Press Down Arrow to focus the first item.', { exact: true });
  await expect(heading).toBeFocused();
  await expect(hint).toBeVisible();
  await expect(heading).toHaveAccessibleDescription('Press Down Arrow to focus the first item.');
  await page.keyboard.press('ArrowDown');
  await expect(item(page, 'alpha')).toBeFocused();
  await expect(hint).toBeHidden();
});

test('first pointer click after fresh Files keeps row geometry stable', async ({ page }) => {
  await open(page);
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeFocused();
  const target = item(page, 'beta');
  const before = (await target.boundingBox())!;
  await page.mouse.move(before.x + 10, before.y + before.height - 3);
  await page.mouse.down();
  expect((await target.boundingBox())!.y).toBe(before.y);
  await page.mouse.up();
  await expect(page.getByRole('dialog', { name: 'Preview beta.txt' })).toBeVisible();
});
