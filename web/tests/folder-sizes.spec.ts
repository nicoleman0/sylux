import { expect, test, type Page } from '@playwright/test';

const longName = 'Financial statements and family photographs from the shared archive';
async function workspace(page: Page) {
  const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
  let bytes = 2048;
  let release: (() => void) | undefined;
  let pending: Promise<void> | undefined;
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    const json = (value: unknown) => route.fulfill({ json: value });
    if (path === '/instance') return json({ name: 'Sylux' });
    if (path === '/setup') return json({ setupRequired: false, user: null });
    if (path === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (path === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' });
    if (path === '/folders/children' || path === '/folders/root/children') {
      await pending;
      return json({ root, items: [
        { id: 'known', parent_id: 'root', name: longName, kind: 'folder', size: bytes, etag: '"known:1"' },
        { id: 'zero', parent_id: 'root', name: 'Empty', kind: 'folder', size: 0, etag: '"zero:1"' },
        { id: 'null', parent_id: 'root', name: 'Unavailable', kind: 'folder', size: null, etag: '"null:1"' },
        { id: 'absent', parent_id: 'root', name: 'Older response', kind: 'folder', etag: '"absent:1"' },
        { id: 'file', parent_id: 'root', name: 'Notes.txt', kind: 'file', size: 1024, etag: '"file:1"' },
      ], next_cursor: null, has_more: false });
    }
    if (path === '/entries/search') return json({ items: [{ entry: { id: 'known', parent_id: 'root', name: longName, kind: 'folder', etag: '"known:1"' }, location: [root] }], next_cursor: null, has_more: false });
    return route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('row', { name: longName, exact: true })).toBeVisible();
  return {
    changeAndPause: () => { bytes = 4096; pending = new Promise<void>(resolve => release = resolve); },
    finish: () => { release?.(); pending = undefined; },
  };
}

for (const theme of ['light', 'dark'] as const) {
  for (const width of [1280, 600, 390]) {
    test(`folder totals, unknown metadata and refresh at ${width} in ${theme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      const backend = await workspace(page);
      const size = (name: string) => page.getByRole('row', { name, exact: true }).locator(width <= 620 ? '.mobile-file-size' : ':scope > .file-size');
      for (const name of [longName, 'Empty', 'Unavailable', 'Older response', 'Notes.txt']) {
        await expect(size(name)).toBeVisible();
      }
      await expect(size(longName)).toContainText('2 KB');
      await expect(size('Empty')).toContainText('0 B');
      await expect(size('Unavailable')).toContainText('Size unknown');
      await expect(size('Older response')).toContainText('Size unknown');
      await expect(size('Notes.txt')).toContainText('1 KB');
      const open = page.getByRole('button', { name: `Open ${longName}`, exact: true });
      await open.focus();
      await expect(open).toBeFocused();
      expect(await open.locator('.filename').evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      const options = page.getByRole('button', { name: `Options for ${longName}`, exact: true });
      await options.focus();
      await expect(options).toBeFocused();
      await expect(options).toBeInViewport();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: test.info().outputPath(`syl40-${width}-${theme}.png`), fullPage: true });
      backend.changeAndPause();
      await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Refresh folder', exact: true })).toHaveAttribute('aria-busy', 'true');
      backend.finish();
      await expect(size(longName)).toContainText('4 KB');
      await expect(page.getByRole('button', { name: 'Refresh folder', exact: true })).not.toHaveAttribute('aria-busy', 'true');
      await page.getByLabel('Search files and folders', { exact: true }).fill('archive');
      await page.getByLabel('Search files and folders', { exact: true }).press('Enter');
      await expect(page.locator('.search-row > .file-size')).toHaveText('—');
    });
  }
}
