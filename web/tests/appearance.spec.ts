import { expect, test, type Page } from '@playwright/test';
import { appearanceStyle, contrast, defaultAppearance } from '../src/lib/appearance';

const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGMQyUkAIgYIBQAXXgOB3tP5eAAAAABJRU5ErkJggg==', 'base64');
const asset = '/instance/appearance/assets/' + 'a'.repeat(64);
async function fixture(page: Page) {
  let appearance = { ...defaultAppearance };
  let calls = 0;
  let fail = 0;
  let delay = false;
  let release = () => {};
  let lastBody: Record<string, any> = {};
  const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: 'root:1' };
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    const method = route.request().method();
    const json = (value: unknown, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(value) });
    if (path === asset) return route.fulfill({ contentType: 'image/png', body: png });
    if (path === '/instance/appearance') {
      if (method === 'PUT') {
        calls++; lastBody = route.request().postDataJSON();
        if (delay) await new Promise<void>(resolve => release = resolve);
        if (fail) return json({}, fail);
        appearance = { accentColor: lastBody.accentColor, loginBackgroundColor: lastBody.loginBackgroundColor,
          logoUrl: lastBody.logo === null ? null : lastBody.logo.keep ? appearance.logoUrl : asset,
          loginImageUrl: lastBody.loginImage === null ? null : lastBody.loginImage.keep ? appearance.loginImageUrl : asset };
      }
      return json(appearance);
    }
    if (path === '/instance') return json({ name: 'Sylux' });
    if (path === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (path === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'administrator', csrf_token: 'session-csrf' });
    if (path === '/auth/logout') return route.fulfill({ status: 204 });
    if (path === '/me') return route.fulfill({ status: 401 });
    if (path === '/accounts') return json([{ account_id: 'owner', handle: 'owner', role: 'administrator', enabled: true }]);
    if (path === '/setup') return json({ setup_required: false });
    if (path === '/folders/children') return json({ items: [], root, next_cursor: null, has_more: false });
    if (path === '/folders/root/children') return json({ items: [], next_cursor: null, has_more: false });
    if (path === '/folders/root/path') return json({ items: [root] });
    return route.continue();
  });
  return { get calls() { return calls; }, get body() { return lastBody; }, fail: (status: number) => fail = status, delay: () => delay = true, release: () => release() };
}
async function settings(page: Page) {
  await page.goto('/');
  await page.getByLabel('Account name', { exact: true }).fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('test-password-long');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Appearance', exact: true }).click();
  await expect(page.getByLabel('Accent color', { exact: true })).toBeEnabled();
}

test('appearance preview, cancel, save, public login, fallback and reset', async ({ page }) => {
  const server = await fixture(page);
  await settings(page);
  const original = await page.locator('html').evaluate(node => getComputedStyle(node).getPropertyValue('--primary'));
  await page.getByLabel('Accent color', { exact: true }).fill('#7234a8');
  await expect(page.getByRole('region', { name: 'Appearance preview' }).locator('.preview-submit')).not.toHaveCSS('background-color', original);
  expect(await page.locator('html').evaluate(node => getComputedStyle(node).getPropertyValue('--primary'))).toBe(original);
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Save appearance' })).toBeDisabled();
  expect(server.calls).toBe(0);
  await page.getByLabel('Accent color', { exact: true }).fill('#7234a8');
  await page.getByLabel('Login background color', { exact: true }).fill('#c7b1e0');
  await page.getByLabel('Logo', { exact: true }).setInputFiles({ name: 'logo.png', mimeType: 'image/png', buffer: png });
  await expect(page.getByRole('button', { name: 'Remove logo' })).toBeVisible();
  await page.getByLabel('Login image', { exact: true }).setInputFiles({ name: 'background.png', mimeType: 'image/png', buffer: png });
  await expect(page.getByRole('button', { name: 'Remove login image' })).toBeVisible();
  await page.getByRole('button', { name: 'Save appearance' }).click();
  await expect(page.getByRole('status')).toContainText('Appearance saved.');
  expect(server.calls).toBe(1);
  expect(server.body.logo.data).toBe(png.toString('base64'));
  await page.getByRole('button', { name: 'Instance', exact: true }).click();
  await page.getByRole('button', { name: 'Appearance', exact: true }).click();
  await expect(page.getByText('Appearance saved.', { exact: true })).toHaveCount(0);
  await expect(page.locator('.preview-header img')).toHaveAttribute('src', asset);
  await page.reload();
  await expect(page.locator('.brand img')).toHaveAttribute('src', asset);
  await expect(page.locator('.signin-main')).toHaveCSS('background-color', 'rgb(199, 177, 224)');
  await page.route('**/instance/appearance/assets/*', route => route.fulfill({ status: 404 }));
  await page.reload();
  await expect(page.locator('.brand svg')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
  await settings(page);
  await page.getByRole('button', { name: 'Reset defaults' }).click();
  expect(server.calls).toBe(1);
  await page.getByRole('button', { name: 'Save appearance' }).click();
  await expect(page.getByRole('status')).toContainText('Appearance saved.');
  expect(server.body).toEqual({ accentColor: null, loginBackgroundColor: null, logo: null, loginImage: null });
});

test('appearance validates local files, prevents duplicate saves and reconciles uncertainty', async ({ page }) => {
  const server = await fixture(page);
  await settings(page);
  await page.getByLabel('Logo', { exact: true }).setInputFiles({ name: 'unsafe.svg', mimeType: 'image/svg+xml', buffer: Buffer.from('<svg/>') });
  await expect(page.getByRole('alert')).toContainText('Choose a PNG, JPEG or WebP');
  await page.getByLabel('Logo', { exact: true }).setInputFiles({ name: 'large.png', mimeType: 'image/png', buffer: Buffer.alloc(512 * 1024 + 1) });
  await expect(page.getByRole('alert')).toContainText('512 KiB');
  await page.getByLabel('Accent color', { exact: true }).fill('#ffcc00');
  server.delay(); server.fail(500);
  await page.getByRole('button', { name: 'Save appearance' }).click();
  await expect(page.getByRole('button', { name: 'Saving…' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Instance', exact: true })).toBeDisabled();
  server.release();
  await expect(page.getByRole('alert')).toContainText('Save could not be confirmed');
  await expect(page.getByRole('button', { name: 'Save appearance' })).toBeDisabled();
  await page.getByRole('button', { name: 'Reload appearance' }).click();
  await expect(page.getByLabel('Accent color', { exact: true })).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Save appearance' })).toBeDisabled();
  expect(server.calls).toBe(1);
});

test('appearance form fits both themes and narrow layout with keyboard focus', async ({ page }) => {
  await fixture(page);
  await settings(page);
  await page.getByLabel('Accent color', { exact: true }).fill('#fd81a4');
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: 1000 });
    for (const dark of [false, true]) {
      const isDark = await page.locator('html').evaluate(node => node.classList.contains('dark'));
      if (isDark !== dark) await page.getByRole('button', { name: dark ? 'Switch to dark mode' : 'Switch to light mode', exact: true }).click();
      await page.getByLabel('Accent color', { exact: true }).focus();
      await page.keyboard.press('Tab');
      await expect(page.getByRole('button', { name: 'Use default' }).first()).toBeFocused();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(page.getByRole('button', { name: 'Cancel', exact: true })).toHaveCSS('color', await page.locator('body').evaluate(node => getComputedStyle(node).color));
      await page.screenshot({ path: test.info().outputPath(`appearance-${width}-${dark ? 'dark' : 'light'}.png`), fullPage: true, animations: 'disabled' });
    }
  }
});

test('custom accents have readable foreground pairs in both themes', () => {
  for (const dark of [false, true]) for (const color of ['#ffffff', '#000000', '#ff0000', '#00ff00', '#0000ff', '#ffff00', '#808080', '#fd81a4']) {
    const tokens = Object.fromEntries(appearanceStyle({ ...defaultAppearance, accentColor: color }, dark).split(';').map(item => item.split(':')));
    expect(contrast(tokens['--primary'], tokens['--primary-foreground'])).toBeGreaterThanOrEqual(4.5);
    expect(contrast(tokens['--primary-hover'], tokens['--primary-foreground'])).toBeGreaterThanOrEqual(4.5);
    expect(contrast(tokens['--accent'], tokens['--accent-foreground'])).toBeGreaterThanOrEqual(4.5);
    expect(contrast(tokens['--primary'], dark ? '#1d2323' : '#ffffff')).toBeGreaterThanOrEqual(4.5);
  }
});

test('a delayed public appearance read cannot overwrite a later save', async ({ page }) => {
  await fixture(page);
  let first = true;
  let release = () => {};
  await page.route('**/instance/appearance', async route => {
    if (first && route.request().method() === 'GET') {
      first = false;
      await new Promise<void>(resolve => release = resolve);
      return route.fulfill({ contentType: 'application/json', body: JSON.stringify(defaultAppearance) });
    }
    return route.fallback();
  });
  await settings(page);
  await page.getByLabel('Accent color', { exact: true }).fill('#7234a8');
  await page.getByRole('button', { name: 'Save appearance' }).click();
  await expect(page.getByText('Appearance saved.', { exact: true })).toBeVisible();
  const savedPrimary = await page.locator('html').evaluate(node => node.style.getPropertyValue('--primary'));
  const response = page.waitForResponse(response => response.url().endsWith('/instance/appearance') && response.request().method() === 'GET');
  release();
  await response;
  await expect.poll(() => page.locator('html').evaluate(node => node.style.getPropertyValue('--primary'))).toBe(savedPrimary);
});
