import { expect, test, type Page } from '@playwright/test';

const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
const source = { id: 'source', parent_id: 'root', name: 'Project', kind: 'folder', etag: '"source:1"' };
const destination = { id: 'destination', parent_id: 'root', name: 'Destination', kind: 'folder', etag: '"destination:1"' };
const child = { id: 'child', parent_id: 'source', name: 'SYL-81 résumé.bin', kind: 'file', etag: '"child:1"', size: 12 };

async function signIn(page: Page, owner = 'owner') {
  await page.getByLabel('Account name').fill(owner);
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
}

async function fixture(page: Page) {
  const entries = [source, destination, child];
  const requests: Array<{ path: string; payload: unknown; etag: string | undefined }> = [];
  let effect: 'healthy' | 'conflict' | 'lost' | 'invalid' | 'other' | 'large' = 'healthy';
  let release: Promise<void> | null = null;
  let releasePreparation: Promise<void> | null = null;
  let unavailableStatus = false;
  const copy = () => entries.some(entry => entry.id === 'copied');
  await page.route('**/*', async route => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const listing = (parent: string) => ({ items: entries.filter(entry => entry.parent_id === parent), next_cursor: null, has_more: false });
    if (path === '/auth/csrf') return route.fulfill({ json: { csrf_token: 'csrf' } });
    if (path === '/auth/local') {
      const { handle } = request.postDataJSON();
      return route.fulfill({ json: { account_id: handle, handle, role: 'user', csrf_token: 'csrf' } });
    }
    if (path === '/folders/children') return route.fulfill({ json: { root, ...listing(root.id) } });
    if (/^\/folders\/[^/]+\/children$/.test(path)) return route.fulfill({ json: listing(path.split('/')[2]) });
    if (path === '/trash') return route.fulfill({ json: listing('trash') });
    if (path === '/auth/logout') return route.fulfill({ status: 204 });
    if (path === '/entries/source/copy-ticket' || path === '/copies' || path === '/copies/status') {
      requests.push({ path, payload: request.postDataJSON(), etag: request.headers()['if-match'] });
      if (path.endsWith('/copy-ticket')) {
        if (releasePreparation) await releasePreparation;
        return route.fulfill({ json: { ticket: 'original-ticket' } });
      }
      if (effect === 'invalid') return route.fulfill({ status: 400, json: { code: 'invalid_copy_ticket' } });
      if (effect === 'other') return route.fulfill({ status: 403, json: { code: 'copy_other_account' } });
      if (effect === 'large') return route.fulfill({ status: 422, json: { code: 'copy_too_large' } });
      if (path === '/copies/status') {
        if (unavailableStatus) return route.fulfill({ status: 503, json: { code: 'copy_unavailable' } });
        return route.fulfill({ json: { state: copy() ? 'committed' : 'unknown' } });
      }
      if (release) await release;
      if (effect === 'conflict') return route.fulfill({ status: 409, json: { code: 'name_conflict' } });
      if (!copy()) {
        entries.push({ ...source, id: 'copied', parent_id: destination.id });
        entries.push({ ...child, id: 'copied-child', parent_id: 'copied' });
      }
      if (effect === 'lost') return route.abort('connectionreset');
      return route.fulfill({ json: { state: 'committed' } });
    }
    await route.continue();
  });
  await page.goto('/');
  await signIn(page);
  return {
    requests,
    hold: (promise: Promise<void>) => { release = promise; },
    holdPreparation: (promise: Promise<void>) => { releasePreparation = promise; },
    effect: (next: typeof effect) => { effect = next; },
    unavailableStatus: (unavailable: boolean) => { unavailableStatus = unavailable; },
  };
}

async function chooser(page: Page) {
  const actions = page.getByRole('button', { name: 'Options for Project', exact: true });
  await actions.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('menuitem', { name: 'Move', exact: true })).toBeVisible();
  await page.getByRole('menuitem', { name: 'Copy', exact: true }).focus();
  await page.keyboard.press('Enter');
  return page.getByRole('dialog', { name: 'Copy Project' });
}

for (const colorScheme of ['light', 'dark'] as const) {
  test(`copy chooser, conflict, pending and completion in ${colorScheme} mode`, async ({ page }) => {
    await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' });
    if (colorScheme === 'dark') await page.setViewportSize({ width: 390, height: 844 });
    const backend = await fixture(page);
    let dialog = await chooser(page);
    const trigger = page.getByRole('button', { name: 'Options for Project', exact: true });
    const ownFolder = dialog.getByRole('button', { name: 'Project (cannot copy into itself)', exact: true });
    await expect(ownFolder).toBeDisabled();
    await expect(dialog.getByRole('button', { name: 'Copy here', exact: true })).toBeDisabled();
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    await expect(trigger).toBeFocused();
    dialog = await chooser(page);
    await dialog.getByRole('button', { name: 'Destination', exact: true }).click();
    const copyButton = dialog.getByRole('button', { name: 'Copy here', exact: true });
    backend.effect('conflict');
    await copyButton.click();
    await expect(dialog.getByRole('alert')).toContainText('Choose another folder');
    await expect(page.getByRole('row', { name: 'Project', exact: true })).toBeVisible();
    backend.effect('healthy');
    let release!: () => void;
    backend.hold(new Promise<void>(resolve => { release = resolve; }));
    await copyButton.click();
    const busy = dialog.getByRole('button', { name: 'Copying…', exact: true });
    await expect(busy).toBeDisabled();
    await expect(busy).toHaveAttribute('aria-busy', 'true');
    await expect.poll(() => backend.requests.filter(({ path }) => path === '/copies').length).toBe(2);
    await expect(dialog.getByRole('button', { name: 'Cancel', exact: true })).toBeDisabled();
    expect(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
    if (process.env.SYL_81_SCREENSHOTS) await page.screenshot({ path: `${process.env.SYL_81_SCREENSHOTS}/chooser-${colorScheme}.png` });
    release();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('status').filter({ hasText: 'Copied to Destination' })).toBeVisible();
    await expect(page.getByRole('row', { name: 'Project', exact: true })).toBeVisible();
    expect(backend.requests[0]).toEqual({ path: '/entries/source/copy-ticket', payload: { parent_id: 'destination' }, etag: '"source:1"' });
    await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
    await expect(page.getByRole('status').filter({ hasText: 'Copied to Destination' })).toHaveCount(0);
    await page.getByRole('button', { name: 'Open Destination', exact: true }).click();
    await page.getByRole('button', { name: 'Open Project', exact: true }).click();
    await expect(page.getByRole('row', { name: child.name, exact: true })).toBeVisible();
    await page.goBack();
    await expect(page.getByRole('status').filter({ hasText: 'Copied to Destination' })).toHaveCount(0);
  });
}

test('lost response stays reachable and retries exactly the saved ticket across reload', async ({ page }) => {
  const backend = await fixture(page);
  const dialog = await chooser(page);
  await dialog.getByRole('button', { name: 'Destination', exact: true }).click();
  backend.effect('lost');
  backend.unavailableStatus(true);
  await dialog.getByRole('button', { name: 'Copy here', exact: true }).click();
  const recovery = page.getByRole('region', { name: 'Copy status' });
  await expect(recovery.getByRole('status')).toContainText('Copy outcome unconfirmed');
  await expect(recovery.getByRole('button', { name: 'Retry same copy' })).toBeEnabled();
  await expect(page.getByRole('status').filter({ hasText: 'Copied to Destination' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await expect(recovery).toBeVisible();
  await page.getByRole('navigation', { name: 'Files', exact: true }).getByRole('button', { name: 'Files', exact: true }).click();
  await expect(recovery).toBeVisible();
  await page.reload();
  await signIn(page, 'other');
  await expect(recovery).toHaveCount(0);
  await page.getByRole('button', { name: 'Log out', exact: true }).click();
  await signIn(page);
  await expect(recovery).toBeVisible();
  backend.effect('healthy');
  await recovery.getByRole('button', { name: 'Retry same copy' }).click();
  await expect(recovery).toHaveCount(0);
  await expect(page.getByRole('status').filter({ hasText: 'Copied to Destination' })).toBeVisible();
  expect(backend.requests.filter(({ path }) => path === '/entries/source/copy-ticket')).toHaveLength(1);
  expect(backend.requests.filter(({ path }) => path === '/copies').map(({ payload }) => payload)).toEqual([
    { ticket: 'original-ticket' }, { ticket: 'original-ticket' },
  ]);
});

test('browser navigation during preparation preserves the copy and does not revive completed feedback', async ({ page }) => {
  const backend = await fixture(page);
  const menu = page.getByRole('navigation', { name: 'Files', exact: true });
  await menu.getByRole('button', { name: 'Trash', exact: true }).click();
  await menu.getByRole('button', { name: 'Files', exact: true }).click();
  const dialog = await chooser(page);
  await dialog.getByRole('button', { name: 'Destination', exact: true }).click();
  let release!: () => void;
  backend.holdPreparation(new Promise<void>(resolve => { release = resolve; }));
  backend.effect('lost');
  await dialog.getByRole('button', { name: 'Copy here', exact: true }).click();
  await expect.poll(() => backend.requests.map(({ path }) => path)).toEqual(['/entries/source/copy-ticket']);
  await page.goBack();
  await expect(page.getByRole('heading', { name: 'Trash', exact: true })).toBeAttached();
  release();
  await expect(dialog).toBeHidden();
  await expect.poll(() => backend.requests.map(({ path }) => path)).toEqual([
    '/entries/source/copy-ticket', '/copies', '/copies/status',
  ]);
  await expect(page.getByRole('region', { name: 'Copy status' })).toHaveCount(0);
  await menu.getByRole('button', { name: 'Files', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Copied to Destination' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Open Destination', exact: true }).click();
  await expect(page.getByRole('row', { name: 'Project', exact: true })).toBeVisible();
});

for (const mode of ['status', 'retry'] as const) {
  test(`invalid saved ticket clears on ${mode} after reload`, async ({ page }) => {
    const backend = await fixture(page);
    const dialog = await chooser(page);
    await dialog.getByRole('button', { name: 'Destination', exact: true }).click();
    backend.effect('lost'); backend.unavailableStatus(true);
    await dialog.getByRole('button', { name: 'Copy here', exact: true }).click();
    const recovery = page.getByRole('region', { name: 'Copy status' });
    await expect(recovery).toBeVisible();
    await page.reload(); await signIn(page);
    backend.effect('other');
    await recovery.getByRole('button', { name: mode === 'status' ? 'Check copy status' : 'Retry same copy' }).click();
    await expect(recovery).toContainText('unconfirmed');
    expect(await page.evaluate(() => sessionStorage.getItem('sylux-copy:owner'))).not.toBeNull();
    backend.effect('invalid');
    await recovery.getByRole('button', { name: mode === 'status' ? 'Check copy status' : 'Retry same copy' }).click();
    await expect(recovery).toHaveCount(0);
    await expect(page.getByText('This copy request is invalid.', { exact: false })).toBeVisible();
    expect(await page.evaluate(() => sessionStorage.getItem('sylux-copy:owner'))).toBeNull();
    await chooser(page);
  });
}

test('oversized copy gives actionable feedback', async ({ page }) => {
  const backend = await fixture(page);
  const dialog = await chooser(page);
  await dialog.getByRole('button', { name: 'Destination', exact: true }).click();
  backend.effect('large');
  await dialog.getByRole('button', { name: 'Copy here', exact: true }).click();
  await expect(dialog.getByRole('alert')).toContainText('Copy a smaller folder');
});
