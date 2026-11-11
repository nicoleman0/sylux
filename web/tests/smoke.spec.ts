import { Buffer } from 'node:buffer';
import { createHash } from 'node:crypto';
import { expect, test } from '@playwright/test';

async function installBackend(page: import('@playwright/test').Page): Promise<void> {
  const root = { id: 'root-1', parent_id: null, name: 'Files', kind: 'folder', generation: 1, etag: 'root-1:1' };
  const entries: Array<Record<string, unknown>> = [
    { id: 'file-1', parent_id: 'root-1', name: 'Welcome.txt', kind: 'file', size: 13, generation: 1, etag: '"g1"' },
  ];
  const deleted: Array<Record<string, unknown>> = [];
  let uploadedName = '';
  let instanceName = 'Sylux';
  const accounts = [{ account_id: 'account-1', handle: 'owner', role: 'administrator', enabled: true }];
  await page.route('**/*', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();
    const json = (value: unknown, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(value) });
    if (path === '/instance') {
      if (method === 'PATCH') instanceName = (request.postDataJSON() as { name: string }).name.trim();
      return json({ name: instanceName });
    }
    if (path === '/auth/csrf') return json({ csrf_token: 'test-csrf' });
    if (path === '/auth/local') return json({ account_id: 'account-1', handle: 'owner', role: 'administrator', csrf_token: 'session-csrf' });
    if (path === '/setup/admin') return json({ code: 'setup_complete' }, 201);
    if (path === '/auth/logout') return route.fulfill({ status: 204 });
    if (path === '/accounts' && method === 'GET') return json(accounts);
    if (path === '/accounts' && method === 'POST') {
      const body = request.postDataJSON() as { handle: string };
      accounts.push({ account_id: 'account-2', handle: body.handle, role: 'user', enabled: true });
      return route.fulfill({ status: 204 });
    }
    if (path === '/accounts/account-2/reset' && method === 'POST') return route.fulfill({ status: 204 });
    if (path === '/accounts/account-2/disable' && method === 'POST') { accounts[1]!.enabled = false; return route.fulfill({ status: 204 }); }
    if (path === '/me/password' && method === 'POST') return route.fulfill({ status: 204 });

    if (path === '/folders/children' && method === 'GET') return json({ items: [], root, next_cursor: null, has_more: false });
    if (path === '/folders/children' && method === 'POST') {
      const body = request.postDataJSON() as { parent_id: string; name: string };
      const folder = { id: 'folder-1', parent_id: body.parent_id, name: body.name, kind: 'folder', generation: 1, etag: '"g1"' };
      entries.push(folder);
      return json(folder, 201);
    }
    if (path === '/folders/root-1/children') return json({ items: entries, next_cursor: null, has_more: false });
    if (path === '/trash' && method === 'GET') return json({ items: deleted.map((entry) => ({ ...entry, original_path: [] })) });
    if (path.startsWith('/entries/') && method === 'DELETE') {
      const entry = entries.find((item) => item.id === path.split('/')[2]);
      if (entry) { entries.splice(entries.indexOf(entry), 1); deleted.push(entry); }
      return route.fulfill({ status: 204 });
    }
    if (path.startsWith('/trash/') && method === 'POST') {
      const entry = deleted.find((item) => item.id === path.split('/')[2]);
      if (entry) { deleted.splice(deleted.indexOf(entry), 1); entries.push(entry); }
      return route.fulfill({ status: 204 });
    }
    if (path.startsWith('/entries/') && method === 'PATCH') {
      const entry = entries.find((item) => item.id === path.split('/')[2]);
      if (entry) entry.name = (request.postDataJSON() as { name: string }).name;
      return json(entry ?? {});
    }
    if (path === '/mutation-tickets') {
      uploadedName = (request.postDataJSON() as { name: string }).name;
      return json({ mutation_id: 'upload-1', ticket: 'test-ticket', expires_at: '2030-01-01T00:00:00Z' }, 201);
    }
    if (path === '/uploads') return json({ upload_id: 'upload-1', status: 'admitted' }, 201);
    if (path === '/uploads/upload-1/content') return json({ upload_id: 'upload-1', mutation_id: 'upload-1', status: 'staged', length: 12, sha256: createHash('sha256').update('mock fixture').digest('hex'), published: false });
    if (path === '/uploads/upload-1/commit') {
      const admission = entries.find((item) => item.id === 'upload-1');
      if (admission) entries.splice(entries.indexOf(admission), 1);
      entries.push({ id: 'upload-1', parent_id: 'root-1', name: uploadedName, kind: 'file', size: 12, generation: 1, etag: '"g1"' });
      return json({ status: 'committed' });
    }
    if (path === '/entries/file-1/content') return route.fulfill({ status: 200, contentType: 'application/octet-stream', body: 'hello download' });
    if (path.startsWith('/uploads/') && method === 'DELETE') return route.fulfill({ status: 204 });
    return route.continue();
  });
}

async function signOut(page: import('@playwright/test').Page, behindDialog = false): Promise<void> {
  const action = page.getByRole('button', { name: 'Log out', exact: true });
  if (behindDialog) await action.evaluate((button: HTMLButtonElement) => button.click());
  else await action.click();
}

async function login(page: import('@playwright/test').Page): Promise<void> {
  await installBackend(page);
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: /sign in/i }).last().click();
  await expect(page.getByRole('heading', { level: 1, name: 'Files' })).toBeVisible();
  await expect(page.getByRole('article').filter({ hasText: 'Welcome.txt' })).toBeVisible();
}

test('setup is explicit and returns to login rather than creating a session', async ({ page }) => {
  await installBackend(page);
  await page.goto('/');
  await page.getByRole('button', { name: 'First-time setup' }).click();
  await page.getByLabel('One-use setup authorization').fill('mock-one-use-authorization');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('mock-password-long');
  await page.getByRole('button', { name: 'Create administrator' }).click();
  await expect(page.getByRole('status')).toContainText('Setup complete');
  await expect(page.getByLabel('Password')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await expect(page.locator('footer')).toContainText('Durability not qualified');
});

test('login, folder create/rename, and upload work with mocked API', async ({ page }) => {
  await login(page);
  await expect(page.getByText('Welcome.txt')).toBeVisible();
  await page.getByRole('button', { name: 'New folder', exact: true }).click();
  await page.getByLabel('Folder name', { exact: true }).fill('Research');
  await page.getByRole('button', { name: 'Create folder' }).click();
  await expect(page.getByRole('article').filter({ hasText: 'Research' })).toBeVisible();
  await page.getByRole('button', { name: 'Options for Welcome.txt', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Rename', exact: true }).click();
  await page.getByLabel('New name').fill('Notes');
  await page.getByRole('button', { name: 'Save name' }).click();
  await expect(page.getByText('Notes')).toBeVisible();

  await page.locator('#upload-file').setInputFiles({ name: 'fixture.txt', mimeType: 'text/plain', buffer: Buffer.from('mock fixture') });
  await page.getByRole('button', { name: 'Upload', exact: true }).click();
  await expect(page.getByRole('article').filter({ hasText: 'fixture.txt' })).toBeVisible({ timeout: 8000 });
  await expect(page.getByRole('status').filter({ hasText: 'Upload complete' })).toBeVisible();
  await page.getByRole('button', { name: 'Options for Notes', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'Download', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await signOut(page);
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
});

test('narrow viewport keeps sign-in and durability warning visible', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 780 });
  await page.goto('/');
  await expect(page.getByLabel('Account name')).toBeVisible();
  await expect(page.locator('footer')).toContainText('Durability not qualified');
  expect(await page.locator('body').evaluate((body) => body.scrollWidth <= window.innerWidth)).toBeTruthy();
});

test('dark mode follows the system until changed and persists across reloads', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.goto('/');
  const toggle = page.getByRole('button', { name: /Switch to (dark|light) mode/ });
  await expect(toggle).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('html')).toHaveClass(/dark/);

  await page.emulateMedia({ colorScheme: 'light' });
  await expect(toggle).toHaveAttribute('aria-pressed', 'false');
  await toggle.click();
  await expect(page.locator('html')).toHaveClass(/dark/);
  await page.reload();
  await expect(toggle).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('html')).toHaveClass(/dark/);
  await toggle.click();
  await page.reload();
  await expect(toggle).toHaveAttribute('aria-pressed', 'false');
  await expect(page.locator('html')).not.toHaveClass(/dark/);
});

test('keyboard sign-in and rename dialog restore focus', async ({ page }) => {
  await installBackend(page);
  await page.goto('/');
  await page.getByLabel('Account name').focus();
  await page.keyboard.type('owner');
  await page.keyboard.press('Tab');
  await page.keyboard.type('mock-password');
  await page.keyboard.press('Tab');
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeFocused();
  await page.keyboard.press('Enter');
  const rename = page.getByRole('button', { name: 'Options for Welcome.txt', exact: true });
  await rename.focus();
  await page.keyboard.press('Enter');
  await page.getByRole('menuitem', { name: 'Rename', exact: true }).click();
  await expect(page.getByLabel('New name')).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(rename).toBeFocused();
});

test('delete confirmation and restore work at mobile width', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await login(page);
  await page.getByRole('button', { name: 'Options for Welcome.txt', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
  await expect(page.getByRole('dialog')).toContainText('Permanent removal is not available');
  await page.getByRole('button', { name: 'Move to trash' }).click();
  await expect(page.getByRole('article').filter({ hasText: 'Welcome.txt' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await page.getByRole('button', { name: 'Restore Welcome.txt' }).click();
  await expect(page.getByRole('heading', { name: 'Trash', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Files', exact: true }).click();
  await expect(page.getByRole('article').filter({ hasText: 'Welcome.txt' })).toBeVisible();
  expect(await page.locator('body').evaluate((body) => body.scrollWidth <= window.innerWidth)).toBeTruthy();
});


test('deleted names stay with their session when a request finishes late', async ({ page }) => {
  await installBackend(page);
  let active = 'owner';
  let count = 0;
  let release!: () => void;
  let started!: () => void;
  const held = new Promise<void>((resolve) => { release = resolve; });
  const requested = new Promise<void>((resolve) => { started = resolve; });
  await page.route('**/auth/local', async (route) => {
    active = (route.request().postDataJSON() as { handle: string }).handle;
    await route.fulfill({ json: { account_id: active, handle: active, role: 'user', csrf_token: 'session-csrf' } });
  });
  await page.route('**/trash', async (route) => {
    const actor = active;
    if (actor === 'owner' && ++count === 2) { started(); await held; }
    const item = actor === 'owner' ? [{ id: 'private', name: 'Private A', kind: 'file', parent_id: 'root-1', etag: 'a', original_path: [] }] : [];
    await route.fulfill({ json: { items: item } });
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Restore Private A' })).toBeVisible();
  await page.getByRole('button', { name: 'Refresh trash' }).click();
  await requested;
  await signOut(page);
  await page.getByLabel('Account name').fill('second');
  await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Trash' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await expect(page.getByText('Trash is empty.')).toBeVisible();
  const lateResponse = page.waitForResponse((response) => response.url().endsWith('/trash'));
  release();
  await lateResponse;
  await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  await expect(page.getByRole('button', { name: 'Restore Private A' })).toHaveCount(0);
  await expect(page.getByText('Trash is empty.')).toBeVisible();
});

test('trash entries show their original folders for duplicate names', async ({ page }) => {
  await installBackend(page);
  await page.route('**/trash', (route) => route.fulfill({ json: { items: [
    { id: 'a', name: 'notes.txt', kind: 'file', parent_id: 'a-folder', etag: 'a', original_path: ['Work'] },
    { id: 'b', name: 'notes.txt', kind: 'file', parent_id: 'b-folder', etag: 'b', original_path: ['Home'] },
  ] } }));
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await expect(page.getByRole('listitem').filter({ hasText: 'notes.txt' })).toHaveCount(2);
  await expect(page.getByRole('listitem').filter({ hasText: 'Files / Work' })).toBeVisible();
  await expect(page.getByRole('listitem').filter({ hasText: 'Files / Home' })).toBeVisible();
});


test('sign out hides account controls until the logout request finishes', async ({ page }) => {
  await installBackend(page);
  let release!: () => void;
  let started!: () => void;
  const held = new Promise<void>((resolve) => { release = resolve; });
  const requested = new Promise<void>((resolve) => { started = resolve; });
  let actor = 'owner';
  await page.route('**/auth/local', (route) => {
    actor = (route.request().postDataJSON() as { handle: string }).handle;
    return route.fulfill({ json: { account_id: actor, handle: actor, role: 'user', csrf_token: 'session-csrf' } });
  });
  await page.route('**/trash', (route) => route.fulfill({ json: { items: actor === 'owner' ? [{ id: 'secret', name: 'Private A', kind: 'file', parent_id: 'root-1', etag: 'a', original_path: [] }] : [] } }));
  await page.route('**/auth/logout', async (route) => { started(); await held; await route.fulfill({ status: 204 }); });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Restore Private A' })).toBeVisible();
  await signOut(page);
  await requested;
  await expect(page.getByText('Signing out…')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Trash', exact: true })).toHaveCount(0);
  await expect(page.getByLabel('Account name')).toHaveCount(0);
  release();
  await expect(page.getByLabel('Account name')).toBeVisible();
  await page.getByLabel('Account name').fill('second');
  await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await expect(page.getByText('Trash is empty.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Restore Private A' })).toHaveCount(0);
});

test('identical deleted names and paths have distinct references', async ({ page }) => {
  await installBackend(page);
  await page.route('**/trash', (route) => route.fulfill({ json: { items: [
    { id: 'older-entry', name: 'notes.txt', kind: 'file', parent_id: 'old-folder', etag: 'a', original_path: ['Work'] },
    { id: 'newer-entry', name: 'notes.txt', kind: 'file', parent_id: 'new-folder', etag: 'b', original_path: ['Work'] },
  ] } }));
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await expect(page.getByRole('listitem').filter({ hasText: 'Item reference: older-entry' })).toBeVisible();
  await expect(page.getByRole('listitem').filter({ hasText: 'Item reference: newer-entry' })).toBeVisible();
  await expect(page.getByRole('button', { name: /Restore notes.txt.*item reference older-entry/ })).toBeVisible();
  await expect(page.getByRole('button', { name: /Restore notes.txt.*item reference newer-entry/ })).toBeVisible();
});

test('account screens create, reset, disable and change passwords', async ({ page }) => {
  await login(page);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Create account', exact: true }).click();
  const create = page.getByRole('dialog', { name: 'Create account', exact: true });
  await create.getByLabel('Account name').fill('second');
  await create.getByLabel('Password', { exact: true }).fill('long enough password');
  await create.getByRole('button', { name: 'Create account', exact: true }).click();
  await expect(page.getByText('second', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Reset password', exact: true }).click();
  const reset = page.getByRole('dialog', { name: 'Reset password for second' });
  await reset.getByLabel('New password', { exact: true }).fill('another long password');
  await reset.getByRole('button', { name: 'Reset password', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('Password reset');
  await expect(page.getByRole('heading', { name: 'Accounts', exact: true })).toBeFocused();
  await page.getByRole('button', { name: 'Disable', exact: true }).click();
  await page.getByRole('dialog', { name: 'Disable second?' }).getByRole('button', { name: 'Disable account', exact: true }).click();
  await expect(page.getByText('Disabled', { exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Accounts', exact: true })).toBeFocused();
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await page.getByRole('button', { name: 'Change password', exact: true }).click();
  await page.getByLabel('Current password').fill('old long password');
  await page.getByLabel('New password', { exact: true }).fill('new long password');
  await page.getByRole('dialog', { name: 'Change password', exact: true }).getByRole('button', { name: 'Change password', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('Password changed');
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
});

test('logging out clears account form secrets before another sign-in', async ({ page }) => {
  await login(page);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Create account', exact: true }).click();
  let create = page.getByRole('dialog', { name: 'Create account', exact: true });
  await create.getByLabel('Account name').fill('second');
  await create.getByLabel('Password', { exact: true }).fill('temporary password');
  await create.getByRole('button', { name: 'Create account', exact: true }).click();
  await page.getByRole('button', { name: 'Create account', exact: true }).click();
  await create.getByLabel('Account name').fill('unfinished');
  await create.getByLabel('Password', { exact: true }).fill('unfinished password');
  await create.getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await page.getByRole('button', { name: 'Change password', exact: true }).click();
  await page.getByLabel('Current password').fill('private current password');
  await page.getByLabel('New password', { exact: true }).fill('private new password');
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Reset password', exact: true }).click();
  await page.getByRole('dialog').getByLabel('New password', { exact: true }).fill('private reset password');
  await signOut(page, true);
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('mock-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await page.getByRole('button', { name: 'Change password', exact: true }).click();
  await expect(page.getByLabel('Current password')).toHaveValue('');
  await expect(page.getByLabel('New password', { exact: true })).toHaveValue('');
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.getByRole('button', { name: 'Create account', exact: true }).click();
  create = page.getByRole('dialog', { name: 'Create account', exact: true });
  await expect(create.getByLabel('Account name')).toHaveValue('');
  await expect(create.getByLabel('Password', { exact: true })).toHaveValue('');
  await create.getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.getByRole('button', { name: 'Reset password', exact: true }).click();
  await expect(page.getByRole('dialog').getByLabel('New password', { exact: true })).toHaveValue('');
});


test('late account list cannot replace the next session list', async ({ page }) => {
  await login(page);
  let release!: () => void;
  let started!: () => void;
  const waiting = new Promise<void>((resolve) => { started = resolve; });
  const gate = new Promise<void>((resolve) => { release = resolve; });
  let requests = 0;
  await page.route('**/accounts', async (route) => {
    if (route.request().method() !== 'GET') return route.fallback();
    requests++;
    if (requests === 1) {
      started();
      await gate;
      return route.fulfill({ json: [{ account_id: 'old', handle: 'old-private-name', role: 'user', enabled: true }] });
    }
    return route.fulfill({ json: [{ account_id: 'new', handle: 'new-account', role: 'user', enabled: true }] });
  });
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await waiting;
  await signOut(page);
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('mock-password');
  await page.getByRole('button', { name: /sign in/i }).last().click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.getByText('new-account', { exact: true })).toBeVisible();
  const oldResponse = page.waitForResponse((response) => response.url().endsWith('/accounts'));
  release();
  await oldResponse;
  await page.evaluate(() => new Promise(requestAnimationFrame));
  await expect(page.getByText('old-private-name')).toHaveCount(0);
  await expect(page.getByText('new-account', { exact: true })).toBeVisible();
});

test('late password change cannot end the next session', async ({ page }) => {
  await login(page);
  let release!: () => void;
  let started!: () => void;
  const waiting = new Promise<void>((resolve) => { started = resolve; });
  const gate = new Promise<void>((resolve) => { release = resolve; });
  await page.route('**/me/password', async (route) => {
    started(); await gate; await route.fulfill({ status: 204 });
  });
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await page.getByRole('button', { name: 'Change password', exact: true }).click();
  await page.getByLabel('Current password').fill('old long password');
  await page.getByLabel('New password').fill('new long password');
  await page.getByRole('dialog', { name: 'Change password', exact: true }).getByRole('button', { name: 'Change password', exact: true }).click();
  await waiting;
  await signOut(page, true);
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('mock-password');
  await page.getByRole('button', { name: /sign in/i }).last().click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  const oldResponse = page.waitForResponse((response) => response.url().endsWith('/me/password'));
  release();
  await oldResponse;
  await page.evaluate(() => new Promise(requestAnimationFrame));
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
});

test('late sign-in root cannot replace the next session workspace', async ({ page }) => {
  await installBackend(page);
  let release!: () => void;
  let started!: () => void;
  const waiting = new Promise<void>((resolve) => { started = resolve; });
  const gate = new Promise<void>((resolve) => { release = resolve; });
  let requests = 0;
  await page.route((url) => url.pathname === '/folders/children', async (route) => {
    if (route.request().method() !== 'GET') return route.fallback();
    const old = ++requests === 1;
    if (old) { started(); await gate; }
    return route.fulfill({ json: { root: { id: old ? 'old-root' : 'new-root', parent_id: null, name: '', kind: 'folder', etag: 'root:1' }, items: [{ id: old ? 'old-folder' : 'new-folder', parent_id: old ? 'old-root' : 'new-root', name: old ? 'Old private folder' : 'New folder', kind: 'folder', etag: 'folder:1' }], next_cursor: null, has_more: false } });
  });
  await page.route((url) => url.pathname === '/folders/new-root/children', (route) => route.fulfill({ json: { items: [{ id: 'new-file', parent_id: 'new-root', name: 'New session file', kind: 'file', size: 1, etag: 'file:1' }], next_cursor: null, has_more: false } }));
  await page.route((url) => url.pathname === '/folders/old-root/children', (route) => route.fulfill({ json: { items: [{ id: 'old-file', parent_id: 'old-root', name: 'Old private file', kind: 'file', size: 1, etag: 'file:1' }], next_cursor: null, has_more: false } }));
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: /sign in/i }).last().click();
  await waiting;
  await signOut(page);
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: /sign in/i }).last().click();
  await expect(page.getByText('New session file')).toBeVisible();
  const oldResponse = page.waitForResponse((response) => new URL(response.url()).pathname === '/folders/children');
  release(); await oldResponse; await page.evaluate(() => new Promise(requestAnimationFrame));
  await expect(page.getByText('Old private file')).toHaveCount(0);
  await expect(page.getByText('New session file')).toBeVisible();
});

test('late folder listing cannot replace the next session listing', async ({ page }) => {
  await login(page);
  let release!: () => void;
  let started!: () => void;
  const waiting = new Promise<void>((resolve) => { started = resolve; });
  const gate = new Promise<void>((resolve) => { release = resolve; });
  let requests = 0;
  await page.route((url) => url.pathname === '/folders/root-1/children', async (route) => {
    requests++;
    const old = requests === 1;
    if (old) { started(); await gate; }
    return route.fulfill({ json: { items: [{ id: old ? 'old' : 'new', parent_id: 'root-1', name: old ? 'Old private file' : 'New session file', kind: 'file', size: 1, etag: 'file:1' }], next_cursor: null, has_more: false } });
  });
  await page.getByRole('button', { name: 'Refresh folder' }).click();
  await waiting;
  await signOut(page);
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: /sign in/i }).last().click();
  await expect(page.getByText('New session file')).toBeVisible();
  const oldResponse = page.waitForResponse((response) => new URL(response.url()).pathname === '/folders/root-1/children');
  release(); await oldResponse; await page.evaluate(() => new Promise(requestAnimationFrame));
  await expect(page.getByText('Old private file')).toHaveCount(0);
  await expect(page.getByText('New session file')).toBeVisible();
});

test('late upload ticket cannot continue in the next session', async ({ page }) => {
  await login(page);
  let release!: () => void; let started!: () => void; let admissions = 0;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const waiting = new Promise<void>((resolve) => { started = resolve; });
  await page.route('**/mutation-tickets', async (route) => { started(); await gate; await route.fulfill({ json: { mutation_id: 'old-upload', ticket: 'old-ticket', expires_at: '2030-01-01T00:00:00Z' }, status: 201 }); });
  await page.route('**/uploads', async (route) => { admissions++; await route.fallback(); });
  await page.getByLabel('Choose a file to upload').setInputFiles({ name: 'private-old-name.txt', mimeType: 'text/plain', buffer: Buffer.from('old') });
  await page.getByRole('button', { name: 'Upload', exact: true }).click(); await waiting;
  await signOut(page);
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await page.getByLabel('Account name').fill('owner'); await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: /sign in/i }).last().click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  const response = page.waitForResponse((r) => new URL(r.url()).pathname === '/mutation-tickets');
  release(); await response; await page.evaluate(() => new Promise(requestAnimationFrame));
  expect(admissions).toBe(0);
  await expect(page.getByText('private-old-name.txt')).toHaveCount(0);
  await expect(page.getByLabel('Choose a file to upload')).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Upload', exact: true })).toBeDisabled();
});

test('late upload content cannot commit after logout', async ({ page }) => {
  await login(page);
  let release!: () => void; let started!: () => void; let commits = 0;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const waiting = new Promise<void>((resolve) => { started = resolve; });
  await page.route('**/uploads/upload-1/content', async (route) => { started(); await gate; await route.fulfill({ json: { upload_id: 'upload-1', mutation_id: 'upload-1', status: 'staged', length: 3, sha256: '0'.repeat(64), published: false } }).catch(() => {}); });
  await page.route('**/uploads/upload-1/commit', async (route) => { commits++; await route.fallback(); });
  await page.getByLabel('Choose a file to upload').setInputFiles({ name: 'old-content.txt', mimeType: 'text/plain', buffer: Buffer.from('old') });
  await page.getByRole('button', { name: 'Upload', exact: true }).click(); await waiting;
  await signOut(page);
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await page.getByLabel('Account name').fill('owner'); await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: /sign in/i }).last().click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  release(); await page.evaluate(() => new Promise(requestAnimationFrame));
  expect(commits).toBe(0);
  await expect(page.getByText('old-content.txt')).toHaveCount(0);
  await expect(page.getByLabel('Choose a file to upload')).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Upload', exact: true })).toBeDisabled();
});

test('sign-in submissions are serialized before logout', async ({ page }) => {
  await installBackend(page);
  let release!: () => void; let started!: () => void; let attempts = 0;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const waiting = new Promise<void>((resolve) => { started = resolve; });
  await page.route('**/auth/local', async (route) => { attempts++; started(); await gate; await route.fulfill({ json: { account_id: 'account-1', handle: 'owner', role: 'administrator', csrf_token: 'session-csrf' } }); });
  await page.goto('/'); await page.getByLabel('Account name').fill('owner'); await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: /sign in/i }).last().click(); await waiting;
  await expect(page.getByRole('button', { name: 'Signing in…', exact: true })).toBeDisabled();
  await page.locator('form').first().evaluate((form: HTMLFormElement) => form.requestSubmit());
  release(); await expect(page.getByRole('button', { name: 'Account', exact: true })).toBeVisible();
  expect(attempts).toBe(1);
  await signOut(page);
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
});

test('unresolved upload state is cleared on logout', async ({ page }) => {
  await login(page);
  await page.route('**/uploads/upload-1/content', (route) => route.fulfill({ status: 500 }));
  await page.route('**/uploads/upload-1', (route) => route.fulfill({ status: 503 }));
  await page.getByLabel('Choose a file to upload').setInputFiles({ name: 'unresolved-private.txt', mimeType: 'text/plain', buffer: Buffer.from('old') });
  await page.getByRole('button', { name: 'Upload', exact: true }).click();
  await expect(page.getByText('Upload status needs checking')).toBeVisible();
  await signOut(page);
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await page.getByLabel('Account name').fill('owner'); await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: /sign in/i }).last().click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  await expect(page.getByText('Upload status needs checking')).toHaveCount(0);
  await expect(page.getByText('unresolved-private.txt')).toHaveCount(0);
  await expect(page.getByLabel('Choose a file to upload')).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Upload', exact: true })).toBeDisabled();
});

test('late folder creation cannot change the next session', async ({ page }) => {
  await login(page);
  let release!: () => void; let started!: () => void; let roots = 0;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const waiting = new Promise<void>((resolve) => { started = resolve; });
  await page.route((url) => url.pathname === '/folders/children', async (route) => {
    if (route.request().method() === 'GET') { roots++; return route.fallback(); }
    started(); await gate; await route.fulfill({ status: 201, json: { id: 'old', parent_id: 'root-1', name: 'old-private-folder', kind: 'folder', etag: 'g1' } });
  });
  await page.getByRole('button', { name: 'New folder', exact: true }).click();
  await page.getByLabel('Folder name', { exact: true }).fill('old-private-folder');
  await page.getByRole('button', { name: 'Create folder' }).click(); await waiting;
  await signOut(page, true);
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await page.getByLabel('Account name').fill('owner'); await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: /sign in/i }).last().click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  await page.getByRole('button', { name: 'New folder', exact: true }).click();
  await page.getByLabel('Folder name', { exact: true }).fill('new-session-input');
  const response = page.waitForResponse((r) => new URL(r.url()).pathname === '/folders/children' && r.request().method() === 'POST');
  release(); await response; await page.evaluate(() => new Promise(requestAnimationFrame));
  await expect(page.getByLabel('Folder name', { exact: true })).toHaveValue('new-session-input');
  await expect(page.getByText('Folder created.')).toHaveCount(0);
  expect(roots).toBe(1);
});

test('pending logout hides the workspace and prevents new actions', async ({ page }) => {
  await login(page);
  let release!: () => void; let started!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const waiting = new Promise<void>((resolve) => { started = resolve; });
  await page.route('**/auth/logout', async (route) => { started(); await gate; await route.fulfill({ status: 204 }); });
  await signOut(page); await waiting;
  await expect(page.getByRole('status')).toHaveText('Signing out…');
  await expect(page.getByLabel('Folder name', { exact: true })).toHaveCount(0);
  await expect(page.getByLabel('Choose a file to upload')).toHaveCount(0);
  await expect(page.getByLabel('Account name')).toHaveCount(0);
  release();
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await page.getByLabel('Account name').fill('owner'); await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: /sign in/i }).last().click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  await page.getByRole('button', { name: 'New folder', exact: true }).click();
  await expect(page.getByLabel('Folder name', { exact: true })).toHaveValue('');
  await page.keyboard.press('Escape');
});

test('password change clears trash before the next session', async ({ page }) => {
  await installBackend(page);
  let actor = 'owner';
  await page.route('**/auth/local', (route) => {
    actor = (route.request().postDataJSON() as { handle: string }).handle;
    return route.fulfill({ json: { account_id: actor, handle: actor, role: 'user', csrf_token: 'session-csrf' } });
  });
  await page.route('**/trash', (route) => route.fulfill({ json: { items: actor === 'owner' ? [{ id: 'secret', name: 'Private A', kind: 'file', parent_id: 'root-1', etag: 'a', original_path: [] }] : [] } }));
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner'); await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Restore Private A' })).toBeVisible();
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await page.getByRole('button', { name: 'Change password', exact: true }).click();
  await page.getByLabel('Current password').fill('old long password');
  await page.getByLabel('New password', { exact: true }).fill('new long password');
  await page.getByRole('dialog', { name: 'Change password', exact: true }).getByRole('button', { name: 'Change password', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await page.getByLabel('Account name').fill('second'); await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await expect(page.getByText('Trash is empty.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Restore Private A' })).toHaveCount(0);
});

async function signInInstalledBackend(page: import('@playwright/test').Page): Promise<void> {
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
}

test('signed-in root failure offers file retry without reporting a login failure', async ({ page }) => {
  await installBackend(page);
  let attempts = 0;
  await page.route((url) => url.pathname === '/folders/children', (route) => {
    if (route.request().method() !== 'GET') return route.fallback();
    return ++attempts === 1 ? route.fulfill({ status: 503 }) : route.fallback();
  });
  await signInInstalledBackend(page);
  await expect(page.getByRole('button', { name: 'Log out' })).toBeVisible();
  await expect(page.getByRole('alert')).toContainText('Could not load this folder');
  await expect(page.getByText('Sign-in could not be completed', { exact: false })).toHaveCount(0);
  await page.getByRole('button', { name: 'Try again', exact: true }).click();
  await expect(page.getByRole('article').filter({ hasText: 'Welcome.txt' })).toBeVisible();
  expect(attempts).toBe(2);
});

test('confirmed folder creation survives a failed refresh without encouraging another create', async ({ page }) => {
  await login(page);
  let creates = 0;
  await page.route((url) => url.pathname === '/folders/children', (route) => {
    if (route.request().method() === 'POST') { creates++; return route.fallback(); }
    return route.fulfill({ status: 503 });
  });
  await page.getByRole('button', { name: 'New folder', exact: true }).click();
  await page.getByLabel('Folder name', { exact: true }).fill('Research');
  await page.getByRole('button', { name: 'Create folder' }).click();
  await expect(page.getByRole('status')).toContainText('Folder created. Refresh your files to see it.');
  await page.getByRole('button', { name: 'New folder', exact: true }).click();
  await expect(page.getByLabel('Folder name', { exact: true })).toHaveValue('');
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Refresh folder' }).click();
  await expect(page.getByRole('article').filter({ hasText: 'Research' })).toBeVisible();
  expect(creates).toBe(1);
});

test('rename errors retain the name and focus inside the dialog, then cancel restores focus', async ({ page }) => {
  await login(page);
  await page.route('**/entries/file-1', (route) => route.fulfill({ status: 409 }));
  const trigger = page.getByRole('button', { name: 'Options for Welcome.txt', exact: true });
  await trigger.click();
  await page.getByRole('menuitem', { name: 'Rename', exact: true }).click();
  await page.getByLabel('New name').fill('Already used.txt');
  await page.getByRole('button', { name: 'Save name' }).click();
  await expect(page.getByRole('dialog').getByRole('alert')).toContainText('An item already has this name');
  await expect(page.getByLabel('New name')).toHaveValue('Already used.txt');
  await expect(page.getByLabel('New name')).toBeFocused();
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(trigger).toBeFocused();
});

test('confirmed rename reports a refresh failure separately and restores focus to the folder', async ({ page }) => {
  await login(page);
  await page.route((url) => url.pathname === '/folders/root-1/children', (route) => route.fulfill({ status: 503 }));
  await page.getByRole('button', { name: 'Options for Welcome.txt', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Rename', exact: true }).click();
  await page.getByLabel('New name').fill('Saved name.txt');
  await page.getByRole('button', { name: 'Save name' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('status')).toContainText('Name updated. Refresh your files to see it.');
  await expect(page.getByRole('alert')).toContainText('Could not load this folder');
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeFocused();
});

for (const resource of ['accounts', 'trash'] as const) {
  test(`${resource} loading and errors do not claim an empty result and offer retry`, async ({ page }) => {
    await login(page);
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    await page.route(`**/${resource}`, async (route) => { await gate; await route.fulfill({ status: 503 }); });
    await page.getByRole('button', { name: resource === 'accounts' ? 'Settings' : 'Trash', exact: true }).click();
    await expect(page.getByRole('status').filter({ hasText: resource === 'accounts' ? 'Loading accounts' : 'Loading trash' })).toBeVisible();
    await expect(page.getByText(resource === 'accounts' ? 'No accounts to show.' : 'Trash is empty.', { exact: true })).toHaveCount(0);
    release();
    await expect(page.getByRole('alert')).toContainText(resource === 'accounts' ? 'Could not load accounts' : 'Could not load trash');
    await expect(page.getByText(resource === 'accounts' ? 'No accounts to show.' : 'Trash is empty.', { exact: true })).toHaveCount(0);
    await page.unroute(`**/${resource}`);
    await page.getByRole('button', { name: resource === 'accounts' ? 'Refresh' : 'Refresh trash' }).click();
    if (resource === 'accounts') await expect(page.getByText('Administrator', { exact: true })).toBeVisible();
    else await expect(page.getByText('Trash is empty.', { exact: true })).toBeVisible();
  });
}

test('expired session clears private UI and directs keyboard focus to sign-in', async ({ page }) => {
  await login(page);
  await page.getByRole('button', { name: 'New folder', exact: true }).click();
  await page.getByLabel('Folder name', { exact: true }).fill('Private unsaved name');
  await page.keyboard.press('Escape');
  await page.route((url) => url.pathname === '/folders/root-1/children', (route) => route.fulfill({ status: 401, json: { code: 'unauthorized' } }));
  await page.getByRole('button', { name: 'Refresh folder' }).click();
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await expect(page.getByRole('status')).toContainText('Your session ended. Sign in again');
  await expect(page.getByLabel('Account name')).toBeFocused();
  await expect(page.getByText('Welcome.txt', { exact: true })).toHaveCount(0);
  await expect(page.getByLabel('Folder name', { exact: true })).toHaveCount(0);
});

test('an old unauthorized response cannot end the next session', async ({ page }) => {
  await login(page);
  let release!: () => void;
  let started!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const waiting = new Promise<void>((resolve) => { started = resolve; });
  let attempts = 0;
  await page.route((url) => url.pathname === '/folders/root-1/children', async (route) => {
    if (++attempts !== 1) return route.fallback();
    started(); await gate; await route.fulfill({ status: 401 });
  });
  await page.getByRole('button', { name: 'Refresh folder' }).click(); await waiting;
  await signOut(page);
  await page.getByLabel('Account name').fill('owner'); await page.getByLabel('Password').fill('mock-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('article').filter({ hasText: 'Welcome.txt' })).toBeVisible();
  const response = page.waitForResponse((r) => new URL(r.url()).pathname === '/folders/root-1/children' && r.status() === 401);
  release(); await response; await page.evaluate(() => new Promise(requestAnimationFrame));
  await expect(page.getByRole('button', { name: 'Log out' })).toBeVisible();
  await expect(page.getByRole('article').filter({ hasText: 'Welcome.txt' })).toBeVisible();
});

test('wrong current password and rejected upload tickets do not end a valid session', async ({ page }) => {
  await login(page);
  await page.route('**/me/password', (route) => route.fulfill({ status: 401 }));
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await page.getByRole('button', { name: 'Change password', exact: true }).click();
  await page.getByLabel('Current password').fill('wrong current password');
  await page.getByLabel('New password', { exact: true }).fill('new long password');
  await page.getByRole('dialog', { name: 'Change password', exact: true }).getByRole('button', { name: 'Change password', exact: true }).click();
  await expect(page.getByRole('dialog').getByRole('alert')).toContainText('Password could not be changed');
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Log out' })).toBeVisible();
  await page.getByRole('button', { name: 'Files', exact: true }).click();
  await page.route('**/mutation-tickets', (route) => route.fulfill({ status: 401, json: { code: 'invalid_mutation_ticket' } }));
  await page.getByLabel('Choose a file to upload').setInputFiles({ name: 'ticket.txt', mimeType: 'text/plain', buffer: Buffer.from('mock fixture') });
  await page.getByRole('button', { name: 'Upload', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('Upload was not admitted');
  await expect(page.getByRole('button', { name: 'Log out' })).toBeVisible();
});

test('mobile delete failure, cancellation, confirmation and restore keep focus reachable', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await login(page);
  await page.route('**/entries/file-1', (route) => route.fulfill({ status: 503 }));
  const trigger = page.getByRole('button', { name: 'Options for Welcome.txt', exact: true });
  await trigger.click();
  await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
  await page.getByRole('button', { name: 'Move to trash' }).click();
  await expect(page.getByRole('dialog').getByRole('alert')).toContainText('Could not delete this item');
  await page.keyboard.press('Escape'); await expect(trigger).toBeFocused();
  await page.unroute('**/entries/file-1');
  await trigger.click(); await page.getByRole('menuitem', { name: 'Delete', exact: true }).click(); await page.getByRole('button', { name: 'Move to trash' }).click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeFocused();
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await page.getByRole('button', { name: 'Restore Welcome.txt' }).click();
  await expect(page.getByRole('heading', { name: 'Trash' })).toBeFocused();
  await expect(page.getByRole('article', { name: 'Welcome.txt', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Files', exact: true }).click();
  await expect(page.getByRole('article').filter({ hasText: 'Welcome.txt' })).toBeVisible();
});

for (const width of [1280, 390]) {
  test(`long filenames, selected upload and completed label fit at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await login(page);
    const longName = `${'long-name-'.repeat(18)}.txt`;
    await page.getByLabel('Choose a file to upload').setInputFiles({ name: longName, mimeType: 'text/plain', buffer: Buffer.from('mock fixture') });
    await expect(page.getByText(longName, { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Upload', exact: true }).click();
    await expect(page.getByRole('status').filter({ hasText: longName }).getByText('Saved', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Cancel upload' })).toHaveCount(0);
    await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toHaveCount(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await expect(page.locator('[data-slot=progress-indicator]')).toHaveCSS('transform', 'matrix(1, 0, 0, 1, 0, 0)');
    await page.screenshot({ path: test.info().outputPath(`layout-${width}.png`), fullPage: true });
  });
}

test('expired session during password change is distinguished from a wrong password', async ({ page }) => {
  await login(page);
  await page.route('**/me/password', (route) => route.fulfill({ status: 401 }));
  await page.route((url) => url.pathname === '/folders/children', (route) => route.fulfill({ status: 401 }));
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await page.getByRole('button', { name: 'Change password', exact: true }).click();
  await page.getByLabel('Current password').fill('old long password');
  await page.getByLabel('New password', { exact: true }).fill('new long password');
  await page.getByRole('dialog', { name: 'Change password', exact: true }).getByRole('button', { name: 'Change password', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Sign in to Sylux' })).toBeVisible();
  await expect(page.getByRole('status')).toContainText('Your session ended');
});

test('options menu supports keyboard dismissal and create dialog restores focus', async ({ page }) => {
  await login(page);
  const options = page.getByRole('button', { name: 'Options for Welcome.txt', exact: true });
  await options.focus();
  await page.keyboard.press('Enter');
  const download = page.getByRole('menuitem', { name: 'Download', exact: true });
  await expect(download).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('menuitem', { name: 'Rename', exact: true })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('menu')).toHaveCount(0);
  await expect(options).toBeFocused();
  const create = page.getByRole('button', { name: 'New folder', exact: true });
  await create.click();
  await expect(page.getByRole('dialog', { name: 'Create folder', exact: true })).toBeVisible();
  await expect(page.getByLabel('Folder name', { exact: true })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(create).toBeFocused();
});

for (const reducedMotion of ['reduce', 'no-preference'] as const) {
  test(`refresh signals pending state with ${reducedMotion} motion`, async ({ page }) => {
    await page.emulateMedia({ reducedMotion });
    await login(page);
    let release!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    await page.route('**/folders/root-1/children*', async route => { await gate; await route.fallback(); });
    const refresh = page.getByRole('button', { name: 'Refresh folder', exact: true });
    await refresh.click();
    await expect(refresh).toBeDisabled();
    await expect(refresh).toHaveAttribute('aria-busy', 'true');
    const icon = refresh.locator('[data-refreshing=true]');
    await expect(icon).toBeVisible();
    if (reducedMotion === 'reduce') await expect(icon).toHaveCSS('animation-name', 'none');
    else await expect(icon).not.toHaveCSS('animation-name', 'none');
    release();
    await expect(refresh).toBeEnabled();
    await expect(refresh).toHaveAttribute('aria-busy', 'false');
  });
}

for (const mode of ['text', 'unsupported', 'error'] as const) {
  test(`filename preview handles ${mode} content and restores focus`, async ({ page }) => {
    await login(page);
    let requests = 0;
    const name = mode === 'unsupported' ? 'Unsafe.html' : 'Welcome.txt';
    if (mode === 'unsupported') {
      await page.route('**/folders/root-1/children*', route => route.fulfill({ json: { items: [{ id: 'file-1', parent_id: 'root-1', name, kind: 'file', size: 13, etag: 'g1' }], next_cursor: null, has_more: false } }));
      await page.getByRole('button', { name: 'Refresh folder' }).click();
    }
    await page.route('**/entries/file-1/content', route => {
      requests++;
      return mode === 'error' ? route.fulfill({ status: 503 }) : route.fulfill({ contentType: 'text/plain', body: '<script>private text</script>' });
    });
    const trigger = page.getByRole('button', { name: `Preview ${name}`, exact: true });
    await trigger.click();
    const dialog = page.getByRole('dialog', { name: `Preview ${name}`, exact: true });
    if (mode === 'text') {
      await expect(dialog.locator('pre')).toHaveText('<script>private text</script>');
      await expect(dialog.locator('script')).toHaveCount(0);
    } else if (mode === 'unsupported') {
      await expect(dialog).toContainText('Preview is not available for this file type');
      expect(requests).toBe(0);
    } else {
      await expect(dialog.getByRole('alert')).toContainText('Could not load this preview');
      await expect(dialog.getByRole('button', { name: 'Retry preview', exact: true })).toBeVisible();
    }
    await expect(dialog.getByRole('button', { name: 'Download file', exact: true })).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(trigger).toBeFocused();
  });
}

test('instance name is saved and survives reload and sign out', async ({ page }) => {
  await login(page);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Instance', exact: true }).click();
  await expect(page.getByLabel('Instance name')).toHaveValue('Sylux');
  await page.getByLabel('Instance name').fill('North House');
  const saved = page.waitForRequest((request) => new URL(request.url()).pathname === '/instance' && request.method() === 'PATCH');
  await page.getByRole('button', { name: 'Save changes', exact: true }).click();
  expect((await saved).postDataJSON()).toEqual({ name: 'North House' });
  await expect(page.getByRole('link', { name: 'North House', exact: true })).toBeVisible();
  await signOut(page);
  await expect(page.getByRole('heading', { name: 'Sign in to North House', exact: true })).toBeVisible();
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Sign in to North House', exact: true })).toBeVisible();
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('mock-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Instance', exact: true }).click();
  await expect(page.getByLabel('Instance name')).toHaveValue('North House');
});

test('home returns from a nested folder and other screens to root files', async ({ page }) => {
  const errors: Error[] = [];
  page.on('pageerror', (error) => errors.push(error));
  await login(page);
  await page.route('**/folders/folder-1/children*', (route) => route.fulfill({ json: { items: [], next_cursor: null, has_more: false } }));
  await page.getByRole('button', { name: 'New folder', exact: true }).click();
  await page.getByLabel('Folder name', { exact: true }).fill('Research');
  await page.getByRole('button', { name: 'Create folder', exact: true }).click();
  await page.getByRole('button', { name: 'Open Research', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Research', exact: true })).toBeVisible();
  const home = page.getByRole('link', { name: 'Sylux', exact: true });
  await home.click();
  await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeVisible();
  await expect(page.getByRole('article', { name: 'Welcome.txt', exact: true })).toBeVisible();
  for (const section of ['Trash', 'Account', 'Settings']) {
    await page.getByRole('button', { name: section, exact: true }).click();
    await expect(page.getByRole('heading', { name: section, exact: true })).toBeVisible();
    await expect(page.getByRole('article', { name: 'Welcome.txt', exact: true })).toHaveCount(0);
    await home.click();
    await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeVisible();
    await expect(page.getByRole('article', { name: 'Welcome.txt', exact: true })).toBeVisible();
  }
  expect(errors).toEqual([]);
});

test('file options toggle, switch rows, and dismiss from outside or Escape', async ({ page }) => {
  await login(page);
  await page.getByRole('button', { name: 'New folder', exact: true }).click();
  await page.getByLabel('Folder name', { exact: true }).fill('Research');
  await page.getByRole('button', { name: 'Create folder', exact: true }).click();
  const options = page.getByRole('button', { name: 'Options for Welcome.txt', exact: true });
  const folderOptions = page.getByRole('button', { name: 'Options for Research', exact: true });
  await options.click({ timeout: 3000 });
  await expect(page.getByRole('menuitem', { name: 'Rename', exact: true })).toBeVisible();
  await options.click({ timeout: 3000 });
  await expect(page.getByRole('menu')).toHaveCount(0);
  await folderOptions.click({ timeout: 3000 });
  await expect(page.getByRole('menuitem', { name: 'Download', exact: true })).toHaveCount(0);
  await options.click({ timeout: 3000 });
  await expect(folderOptions).toHaveAttribute('aria-expanded', 'false');
  await expect(options).toHaveAttribute('aria-expanded', 'true');
  await expect(page.getByRole('menu')).toHaveCount(1);
  await expect(page.getByRole('menuitem', { name: 'Rename', exact: true })).toBeVisible();
  await expect(page.getByRole('menuitem', { name: 'Download', exact: true })).toBeVisible();
  await page.getByRole('heading', { name: 'Files', exact: true }).click();
  await expect(page.getByRole('menu')).toHaveCount(0);
  await folderOptions.click({ timeout: 3000 });
  await page.keyboard.press('Escape');
  await expect(page.getByRole('menu')).toHaveCount(0);
  await expect(folderOptions).toBeFocused();
});

test('browser history cannot reveal a previous account folder after reload', async ({ page }) => {
  await installBackend(page);
  let actor = 'alice';
  const privateName = 'Alice private medical records';
  await page.route('**/auth/local', (route) => {
    actor = (route.request().postDataJSON() as { handle: string }).handle;
    return route.fulfill({ json: { account_id: actor, handle: actor, role: 'user', csrf_token: 'csrf' } });
  });
  await page.route((url) => url.pathname === '/folders/children', (route) => route.fulfill({ json: {
    root: { id: `root-${actor}`, name: 'Files', kind: 'folder', parent_id: null, etag: 'root:1' }, items: [], next_cursor: null, has_more: false,
  } }));
  await page.route('**/folders/root-alice/children*', (route) => route.fulfill({ json: { items: [
    { id: 'secret-alice', parent_id: 'root-alice', name: privateName, kind: 'folder', etag: 'secret:1' },
  ], next_cursor: null, has_more: false } }));
  await page.route('**/folders/root-bob/children*', (route) => route.fulfill({ json: { items: [
    { id: 'bob-file', parent_id: 'root-bob', name: 'Bob notes.txt', kind: 'file', size: 1, etag: 'bob:1' },
  ], next_cursor: null, has_more: false } }));
  await page.route('**/folders/secret-alice/children*', (route) => actor === 'alice'
    ? route.fulfill({ json: { items: [], next_cursor: null, has_more: false } })
    : route.fulfill({ status: 404, json: { code: 'not_found' } }));
  await page.goto('/');
  await page.getByLabel('Account name').fill('alice');
  await page.getByLabel('Password', { exact: true }).fill('alice password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.getByRole('button', { name: `Open ${privateName}`, exact: true }).click();
  await expect(page.getByRole('heading', { name: privateName, exact: true })).toBeVisible();
  const state = await page.evaluate(() => JSON.stringify(window.history.state));
  expect(state).not.toContain(privateName);
  expect(state).not.toContain('folderPath');
  await page.getByRole('link', { name: 'Sylux', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeVisible();
  await page.goBack();
  await expect(page.getByRole('heading', { name: privateName, exact: true })).toBeVisible();
  await signOut(page);
  await page.reload();
  await page.getByLabel('Account name').fill('bob');
  await page.getByLabel('Password', { exact: true }).fill('bob password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('article', { name: 'Bob notes.txt', exact: true })).toBeVisible();
  await page.goBack();
  await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeVisible();
  await expect(page.getByRole('article', { name: 'Bob notes.txt', exact: true })).toBeVisible();
  await expect(page.getByText(privateName, { exact: true })).toHaveCount(0);
});
