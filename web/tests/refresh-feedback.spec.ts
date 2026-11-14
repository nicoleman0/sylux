import { expect, test, type Page, type Route } from '@playwright/test';

const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: 'root:1' };
const file = { id: 'file', parent_id: 'root', name: 'notes.txt', kind: 'file', size: 3, etag: 'file:1' };
async function login(page: Page) {
  await page.route('**/*', route => {
    const path = new URL(route.request().url()).pathname;
    const json = (value: unknown) => route.fulfill({ json: value });
    if (path === '/instance') return json({ name: 'Sylux' });
    if (path === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (path === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'administrator', csrf_token: 'csrf' });
    if (path === '/accounts') return json([{ account_id: 'owner', handle: 'owner', role: 'administrator', enabled: true }]);
    if (path === '/folders/children') return json({ root, items: [], next_cursor: null, has_more: false });
    if (path === '/folders/root/children') return json({ items: [file], next_cursor: null, has_more: false });
    if (path === '/trash') return json({ items: [{ ...file, original_path: ['Work'] }] });
    return route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('article')).toBeVisible();
}

for (const surface of ['Files', 'Trash', 'Accounts'] as const) {
  test(`${surface} refresh reflects held, failed and fast requests without duplicate clicks`, async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await login(page);
    if (surface === 'Trash') await page.getByRole('button', { name: 'Trash', exact: true }).click();
    if (surface === 'Accounts') await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const name = surface === 'Files' ? 'Refresh folder' : surface === 'Trash' ? 'Refresh trash' : 'Refresh';
    const button = page.getByRole('button', { name, exact: true });
    await expect(button).toBeEnabled();
    let held: Route | undefined;
    let requests = 0;
    const path = surface === 'Files' ? '**/folders/root/children*' : surface === 'Trash' ? '**/trash' : '**/accounts';
    await page.route(path, route => { requests++; held = route; });
    const before = await button.boundingBox();
    await button.focus();
    await page.keyboard.press('Enter');
    await expect.poll(() => !!held).toBe(true);
    await expect(button).toBeDisabled();
    await expect(button).toHaveAttribute('aria-busy', 'true');
    await expect(button).toContainText('Refreshing');
    const during = await button.boundingBox();
    expect(during?.width).toBe(before?.width);
    // A second activation while the request is held must not dispatch again.
    await page.keyboard.press('Enter');
    expect(requests).toBe(1);
    if (surface === 'Files') await expect(page.getByRole('article')).toBeVisible();
    if (surface === 'Trash') await expect(page.getByRole('button', { name: /Restore notes/ })).toBeVisible();
    await held!.fulfill({ status: 503, json: { code: 'unavailable' } });
    await expect(button).toBeEnabled();
    await expect(button).toHaveAttribute('aria-busy', 'false');
    await expect(page.getByRole('alert')).toContainText(/Could not/);
    await page.unroute(path);
    await button.click();
    await expect(button).toBeEnabled();
    await expect(page.getByRole('alert')).toHaveCount(0);
  });
}

test('instance reload remains visible and busy through retry success and failure', async ({ page }) => {
  await login(page);
  await page.route('**/instance', route => route.fulfill({ status: 503, json: { code: 'unavailable' } }));
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Instance', exact: true }).click();
  const reload = page.getByRole('button', { name: 'Reload instance name', exact: true });
  await expect(reload).toBeEnabled();
  let held: Route | undefined;
  let count = 0;
  await page.route('**/instance', route => { held = route; count++; });
  await reload.click();
  await expect.poll(() => !!held).toBe(true);
  await expect(reload).toBeDisabled();
  await expect(reload).toContainText(/Reloading|Refreshing/);
  await page.keyboard.press('Enter');
  expect(count).toBe(1);
  await held!.fulfill({ status: 503, json: { code: 'unavailable' } });
  await expect(reload).toBeEnabled();
  held = undefined;
  await reload.click();
  await expect.poll(() => !!held).toBe(true);
  await held!.fulfill({ json: { name: 'Recovered' } });
  await expect(page.getByLabel('Instance name', { exact: true })).toHaveValue('Recovered');
  await expect(page.getByRole('alert')).toHaveCount(0);
});

test('folder and destination retry controls stay busy until their requests settle', async ({ page }) => {
  await login(page);
  await page.route('**/folders/root/children*', route => route.fulfill({ status: 503, json: { code: 'unavailable' } }));
  await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
  const retry = page.getByRole('button', { name: 'Try again', exact: true });
  await expect(retry).toBeVisible();
  let held: Route | undefined;
  await page.route('**/folders/root/children*', route => { held = route; });
  await retry.click();
  await expect.poll(() => !!held).toBe(true);
  await expect(retry).toBeVisible();
  await expect(retry).toBeDisabled();
  await expect(retry).toHaveAttribute('aria-busy', 'true');
  await held!.fulfill({ json: { items: [file], next_cursor: null, has_more: false } });
  await expect(page.getByRole('article')).toBeVisible();
  await page.getByRole('button', { name: 'Options for notes.txt', exact: true }).click();
  // Fail the first destination load, then recover through its own retry.
  await page.route('**/folders/root/children*', route => route.fulfill({ status: 503, json: { code: 'unavailable' } }));
  await page.getByRole('menuitem', { name: 'Move', exact: true }).click();
  const destinations = page.getByRole('button', { name: 'Refresh destinations', exact: true });
  await expect(destinations).toBeEnabled();
  held = undefined;
  await page.route('**/folders/root/children*', route => { held = route; });
  await destinations.click();
  await expect.poll(() => !!held).toBe(true);
  await expect(destinations).toBeVisible();
  await expect(destinations).toBeDisabled();
  await expect(destinations).toHaveAttribute('aria-busy', 'true');
  await held!.fulfill({ status: 503, json: { code: 'unavailable' } });
  await expect(destinations).toBeEnabled();
  await expect(page.getByRole('dialog').getByRole('alert')).toContainText('Could not open');
});

for (const theme of ['light', 'dark'] as const) {
  for (const width of [390, 1280]) {
    test(`refresh controls render at ${width}px in ${theme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await login(page);
      if (width === 390) await page.getByRole('button', { name: 'Toggle Sidebar', exact: true }).click();
      await page.getByRole('button', { name: 'Trash', exact: true }).click();
      if (width === 390) { await page.keyboard.press('Escape'); await expect(page.locator('[data-mobile=true]')).toBeHidden(); }
      const button = page.getByRole('button', { name: 'Refresh trash', exact: true });
      await expect(button).toBeEnabled();
      await button.focus();
      await expect(button).toBeFocused();
      await page.screenshot({ path: `/private/tmp/syl57-${width}-${theme}.png` });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    });
  }
}

test('a held Files refresh does not block Trash refresh', async ({ page }) => {
  await login(page);
  let held: Route | undefined;
  await page.route('**/folders/root/children*', route => { held = route; });
  await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
  await expect.poll(() => !!held).toBe(true);
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  const refresh = page.getByRole('button', { name: 'Refresh trash', exact: true });
  await expect(refresh).toBeEnabled();
  let requests = 0;
  await page.route('**/trash', route => { requests++; return route.fulfill({ json: { items: [] } }); });
  await refresh.click();
  await expect(page.getByText('Trash is empty.', { exact: true })).toBeVisible();
  expect(requests).toBe(1);
  await held!.fulfill({ json: { items: [file], next_cursor: null, has_more: false } });
});

test('held folder retry feedback does not cover a newer successful navigation', async ({ page }) => {
  await login(page);
  await page.route('**/folders/root/children*', route => route.fulfill({ status: 503, json: { code: 'unavailable' } }));
  await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
  const retry = page.getByRole('button', { name: 'Try again', exact: true });
  await expect(retry).toBeEnabled();
  let held: Route | undefined;
  await page.route('**/folders/root/children*', route => { held = route; });
  await retry.click();
  await expect.poll(() => !!held).toBe(true);
  await page.unroute('**/folders/root/children*');
  await page.getByRole('button', { name: 'Files', exact: true }).click();
  await expect(page.getByRole('article')).toBeVisible();
  await expect(retry).toHaveCount(0);
  await held!.fulfill({ status: 503, json: { code: 'unavailable' } });
  await expect(page.getByRole('article')).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
});
