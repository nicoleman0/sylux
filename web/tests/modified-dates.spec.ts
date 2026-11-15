import { Buffer } from 'node:buffer';
import { createHash } from 'node:crypto';
import { expect, test, type Page } from '@playwright/test';
import type { Entry, Folder } from '../src/lib/listing';

const timestamp = 1767227400; // 1 January 2026, 00:30 UTC crosses the local date boundary.
async function installBackend(page: Page, paged = false) {
  const root: Folder = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
  const folder: Folder = { id: 'projects', parent_id: root.id, name: 'Projects', kind: 'folder', etag: '"folder:1"' };
  const file = (id: string, name: string, modified_at?: number | null): Entry => ({ id, name, kind: 'file', parent_id: root.id, size: 42, etag: `"${id}:1"`, ...(modified_at === undefined ? {} : { modified_at }) });
  const entries: Entry[] = [folder, file('epoch', 'Epoch.txt', 0), file('midnight', 'Midnight.txt', timestamp), file('invalid', 'Invalid.txt', 9e15), file('legacy', 'Legacy.txt'), file('null', 'Null.txt', null)];
  const requests: URL[] = [];
  await page.route('**/*', async route => {
    const url = new URL(route.request().url());
    const json = (value: unknown) => route.fulfill({ json: value });
    if (url.pathname === '/instance') return json({ name: 'Sylux' });
    if (url.pathname === '/setup') return json({ setupRequired: false, user: null });
    if (url.pathname === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (url.pathname === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' });
    if (url.pathname === '/folders/root/path') return json({ items: [root] });
    if (url.pathname === '/folders/children') return json({ root, items: entries, next_cursor: null, has_more: false });
    if (/^\/folders\/[^/]+\/children$/.test(url.pathname)) {
      requests.push(url);
      const sort = url.searchParams.get('sort') ?? 'name_asc';
      const cursor = url.searchParams.get('cursor');
      const files = sort === 'modified_desc' ? [entries[2], entries[1], ...entries.slice(3)] : entries.slice(1);
      const all = url.pathname.includes('/projects/') ? [] : [folder, ...files];
      const items = paged ? cursor ? all.slice(3) : all.slice(0, 3) : all;
      return json({ items, next_cursor: paged && !cursor && all.length > 3 ? `${sort}:next` : null, has_more: paged && !cursor && all.length > 3 });
    }
    return route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('row', { name: 'Midnight.txt', exact: true })).toBeVisible();
  return requests;
}

for (const [timezoneId, localDate, epochDate, fullTime] of [
  ['UTC', '1 Jan 2026', '1 Jan 1970', '00:30:00'],
  ['America/Los_Angeles', '31 Dec 2025', '31 Dec 1969', '16:30:00'],
  ['Asia/Tokyo', '1 Jan 2026', '1 Jan 1970', '09:30:00'],
] as const) {
  test.describe(timezoneId, () => {
    test.use({ timezoneId, locale: 'en-GB' });
    test('dates are local while semantic timestamps retain the API instant', async ({ page }) => {
      await installBackend(page);
      const midnight = page.getByRole('row', { name: 'Midnight.txt', exact: true }).locator('.file-modified time');
      await expect(midnight.locator('[aria-hidden="true"]')).toHaveText(localDate);
      await expect(midnight).toHaveAttribute('datetime', '2026-01-01T00:30:00.000Z');
      await expect(midnight).toHaveAttribute('title', new RegExp(fullTime));
      expect(await midnight.ariaSnapshot()).toContain(`Modified `);
      expect(await midnight.ariaSnapshot()).toContain(fullTime);
      await expect(page.getByRole('row', { name: 'Epoch.txt', exact: true }).locator('.file-modified time [aria-hidden="true"]')).toHaveText(epochDate);
      const folderDate = page.getByRole('row', { name: 'Projects', exact: true }).locator('.file-modified');
      expect(await folderDate.ariaSnapshot()).toContain('Modified date not applicable');
      await expect(folderDate.locator('[title]')).toHaveAttribute('title', 'Modified date not applicable');
      for (const name of ['Legacy.txt', 'Null.txt', 'Invalid.txt']) {
        const unknown = page.getByRole('row', { name, exact: true }).locator('.file-modified');
        await expect(unknown).toContainText('Modified date unknown');
        expect(await unknown.ariaSnapshot()).toContain('Modified date unknown');
        await expect(unknown.locator('time')).toHaveCount(0);
      }
    });
  });
}

const names = (page: Page) => page.locator('.file-row').evaluateAll(rows => rows.map(row => row.getAttribute('aria-label')));

test('modified sorting resets page and selection and persists across navigation and refresh', async ({ page }) => {
  const requests = await installBackend(page, true);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect.poll(() => names(page)).toEqual(['Invalid.txt', 'Legacy.txt', 'Null.txt']);
  await page.getByLabel('Select Legacy.txt', { exact: true }).check();
  const sort = page.getByLabel('Sort files by', { exact: true });
  await sort.selectOption('modified_desc');
  await expect.poll(() => names(page)).toEqual(['Projects', 'Midnight.txt', 'Epoch.txt']);
  await expect(page.getByRole('group', { name: 'Selected items', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toHaveCount(0);
  expect(requests.at(-1)?.searchParams.has('cursor')).toBe(false);
  await page.getByRole('button', { name: 'Open Projects', exact: true }).click();
  await expect(page.getByText('This folder is empty', { exact: true })).toBeVisible();
  expect(requests.at(-1)?.searchParams.get('sort')).toBe('modified_desc');
  await page.goBack();
  await expect.poll(() => names(page)).toEqual(['Projects', 'Midnight.txt', 'Epoch.txt']);
  await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
  await expect(sort).toHaveValue('modified_desc');
  await sort.selectOption('modified_asc');
  await expect.poll(() => names(page)).toEqual(['Projects', 'Epoch.txt', 'Midnight.txt']);
});

test('failed modified sort retries the selected order and stale pages restart it', async ({ page }) => {
  const requests = await installBackend(page, true);
  let fail = true;
  let stale = false;
  await page.route('**/folders/root/children*', route => {
    const url = new URL(route.request().url());
    if (url.searchParams.get('sort') !== 'modified_desc') return route.fallback();
    if (fail) return route.fulfill({ status: 503, json: { code: 'unavailable' } });
    if (stale && url.searchParams.has('cursor')) { stale = false; return route.fulfill({ status: 409, json: { code: 'listing_changed' } }); }
    return route.fallback();
  });
  const sort = page.getByLabel('Sort files by', { exact: true });
  await sort.selectOption('modified_desc');
  await expect(page.getByRole('alert')).toContainText('Could not open this folder');
  await expect(sort).toHaveValue('modified_desc');
  fail = false;
  await page.getByRole('button', { name: 'Try again', exact: true }).click();
  await expect.poll(() => names(page)).toEqual(['Projects', 'Midnight.txt', 'Epoch.txt']);
  stale = true;
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('This folder changed. The listing has been restarted.');
  await expect.poll(() => names(page)).toEqual(['Projects', 'Midnight.txt', 'Epoch.txt']);
  expect(requests.at(-1)?.searchParams.get('sort')).toBe('modified_desc');
  expect(requests.at(-1)?.searchParams.has('cursor')).toBe(false);
});

for (const theme of ['light', 'dark'] as const) {
  for (const width of [1280, 375]) {
    test(`modified dates fit ${width}px in ${theme} theme with keyboard sorting`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await installBackend(page);
      const sort = page.getByLabel('Sort files by', { exact: true });
      await sort.focus();
      await page.keyboard.press('m');
      await page.keyboard.press('Enter');
      await expect(sort).toHaveValue('modified_desc');
      await expect(sort).toBeFocused();
      const row = page.getByRole('row', { name: 'Midnight.txt', exact: true });
      await expect(row.locator(width === 375 ? '.mobile-file-size time' : '.file-modified time')).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      const filename = await row.locator('.filename').boundingBox();
      expect(filename?.width).toBeGreaterThan(60);
      await page.screenshot({ path: test.info().outputPath(`sylux-modified-${theme}-${width}.png`), fullPage: true });
    });
  }
}


for (const touch of [false, true]) {
  test.describe(touch ? 'touch timestamp details' : 'keyboard timestamp details', () => {
    test.use({ hasTouch: touch, locale: 'en-GB', timezoneId: 'UTC' });
    test('Options exposes the full local timestamp without hover', async ({ page }) => {
      if (touch) await page.setViewportSize({ width: 375, height: 812 });
      await installBackend(page);
      const options = page.getByRole('button', { name: 'Options for Midnight.txt', exact: true });
      if (touch) await options.tap();
      else { await options.focus(); await page.keyboard.press('Enter'); }
      const menu = page.getByRole('menu');
      await expect(menu).toBeVisible();
      await expect(menu.locator('time')).toHaveText('Thursday, 1 January 2026 at 00:30:00 UTC');
      await expect(menu.locator('time')).toHaveAttribute('datetime', '2026-01-01T00:30:00.000Z');
      expect(await menu.ariaSnapshot()).toContain('Thursday, 1 January 2026 at 00:30:00 UTC');
      await page.screenshot({ path: test.info().outputPath(`sylux-modified-menu-${touch ? 'touch' : 'keyboard'}.png`), fullPage: true });
      await page.keyboard.press('Escape');
      if (!touch) await expect(options).toBeFocused();
    });
  });
}

test.describe('upload date revalidation', () => {
  test.use({ locale: 'en-GB', timezoneId: 'UTC' });
  test('successful replacement updates the visible date automatically', async ({ page }) => {
    await installBackend(page);
    let committed = false;
    await page.route('**/folders/root/children*', route => route.fulfill({ json: {
      items: [{ id: committed ? 'new' : 'old', parent_id: 'root', name: 'same.txt', kind: 'file', size: 3, etag: committed ? '"new:1"' : '"old:1"', modified_at: committed ? timestamp : 0 }], has_more: false, next_cursor: null,
    } }));
    await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
    const date = () => page.getByRole('row', { name: 'same.txt', exact: true }).locator('.file-modified time [aria-hidden="true"]');
    await expect(date()).toHaveText('1 Jan 1970');
    let replacing = false;
    let sequence = 0;
    await page.route('**/mutation-tickets', route => {
      replacing = !!route.request().postDataJSON().replacement;
      return route.fulfill({ status: 201, json: { mutation_id: `upload-${++sequence}`, ticket: `ticket-${sequence}`, expires_at: '2030-01-01' } });
    });
    await page.route('**/uploads', route => route.fulfill(replacing ? { status: 201, json: { upload_id: `upload-${sequence}` } } : { status: 409, json: { code: 'name_collision', existing: { id: 'old', parent_id: 'root', name: 'same.txt', kind: 'file', size: 3, etag: '"old:1"' } } }));
    await page.route('**/uploads/*/content', route => {
      const bytes = route.request().postDataBuffer()!;
      return route.fulfill({ json: { upload_id: `upload-${sequence}`, mutation_id: `upload-${sequence}`, status: 'staged', length: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex'), published: false } });
    });
    await page.route('**/uploads/*/commit', route => { committed = true; return route.fulfill({ status: 201, json: { status: 'committed' } }); });
    await page.getByLabel('Choose files to upload').setInputFiles({ name: 'same.txt', mimeType: 'text/plain', buffer: Buffer.from('new') });
    await page.getByRole('button', { name: 'Replace file', exact: true }).click();
    await expect(date()).toHaveText('1 Jan 2026');
    await expect(page.getByRole('status').filter({ hasText: 'Replacement complete.' })).toBeVisible();
  });
});
