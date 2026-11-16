import { expect, test } from '@playwright/test';

async function installFileWorkspace(page: import('@playwright/test').Page): Promise<void> {
  const root = { id: 'root-1', parent_id: null, name: 'Files', kind: 'folder', etag: 'root-1:1' };
  const entries = [
    { id: 'report-1', parent_id: 'root-1', name: 'annual-report-final-2026-11-14-board-approved-revision-27.pdf', kind: 'file', size: 1_234_567_890, modified_at: 1_767_227_400, etag: 'report-1:1' },
    { id: 'report-2', parent_id: 'root-1', name: 'zipped-archive-2026-11-14.zip', kind: 'file', size: 9_876_543_210, modified_at: 1_767_227_400, etag: 'report-2:1' },
  ];
  await page.route('**/*', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (value: unknown) => route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(value) });
    if (path === '/instance') return json({ name: 'Sylux' });
    if (path === '/instance/appearance') return json({ accentColor: null, loginBackgroundColor: null, logoUrl: null, loginImageUrl: null });
    if (path === '/auth/csrf') return json({ csrf_token: 'typography-test-csrf' });
    if (path === '/auth/local') return json({ account_id: 'account-1', handle: 'owner', role: 'administrator', csrf_token: 'typography-test-session' });
    if (path === '/accounts') return json([{ account_id: 'account-1', handle: 'owner', role: 'administrator', enabled: true }]);
    if (path === '/trash') return json({ items: [{ ...entries[0], original_path: [] }], next_cursor: null, has_more: false });
    if (path === '/entries/search') return json({ items: [{ entry: entries[0], location: [root] }], next_cursor: null, has_more: false });
    if (path === '/folders/children') return json({ items: [], root, next_cursor: null, has_more: false });
    if (path === '/folders/root-1/children') return json({ items: entries, next_cursor: null, has_more: false });
    return route.continue();
  });
}

async function setAppTheme(page: import('@playwright/test').Page, theme: 'light' | 'dark'): Promise<void> {
  const isDark = await page.locator('html').evaluate((node) => node.classList.contains('dark'));
  if (isDark !== (theme === 'dark')) {
    await page.getByRole('button', { name: isDark ? 'Switch to light mode' : 'Switch to dark mode' }).click();
  }
  await expect(page.locator('body')).toHaveCSS('background-color', theme === 'dark' ? 'rgb(23, 28, 28)' : 'rgb(255, 255, 255)');
}

test('catalogue documents the selected data type without experiment controls', async ({ page }, testInfo) => {
  await page.goto('/design-system.html?typography=filenames');
  await expect(page.getByRole('heading', { name: 'Data roles', exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Monospace experiment' })).toHaveCount(0);
  await expect(page.getByRole('navigation', { name: 'Typography experiment variants' })).toHaveCount(0);

  await expect(page.locator('[data-type-sample="--type-meta"] .type-specimen')).toHaveCSS('font-family', /JetBrains Mono/);
  await expect(page.locator('[data-type-sample="--type-note"] .type-specimen')).toHaveCSS('font-family', /system-ui/);
  await expect(page.locator('.data-role-samples .list-head')).toHaveCSS('font-family', /system-ui/);
  await expect(page.locator('.data-role-samples .file-modified')).toHaveCSS('font-family', /JetBrains Mono/);
  await expect(page.locator('.data-role-samples .file-size')).toHaveCSS('font-family', /JetBrains Mono/);
  await expect(page.locator('.data-role-samples p')).toHaveCSS('font-family', /system-ui/);
  await expect(page.locator('.modified-row .filename').last()).toHaveCSS('font-family', /system-ui/);
  await expect(page.locator('.modified-row .filename').last()).toContainText('quarterly-results-export_2026-11-14_final-review-approved.pdf');
  await expect.poll(() => page.evaluate(() => document.fonts.check('12px "JetBrains Mono"'))).toBe(true);

  await page.screenshot({ path: testInfo.outputPath('catalogue-light.png'), fullPage: true, animations: 'disabled' });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole('button', { name: 'Use dark theme' }).click();
  await expect(page.locator('body')).toHaveCSS('background-color', 'rgb(23, 28, 28)');
  await expect(page.locator('.mobile-file-size').last()).toContainText('1.15 GB');
  await expect(page.locator('.mobile-file-size time').last()).toHaveAttribute('datetime');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath('catalogue-narrow-dark.png'), fullPage: true, animations: 'disabled' });
});

test('app metadata uses JetBrains Mono while column headings and interface text stay sans', async ({ page }, testInfo) => {
  await installFileWorkspace(page);
  await page.setViewportSize({ width: 1280, height: 844 });
  await page.goto('/?typography=filenames');
  await expect(page.locator('html')).not.toHaveAttribute('data-typography-experiment');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('test-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Files' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'New folder', exact: true })).toBeEnabled();
  await expect(page.locator('.list-head .sort-heading').first()).toBeEnabled();

  const headings = page.locator('.list-head .sort-heading');
  await expect(headings).toHaveCount(3);
  for (let index = 0; index < 3; index++) await expect(headings.nth(index)).toHaveCSS('font-family', /system-ui/);
  await expect(page.locator('.file-row .file-size').first()).toHaveCSS('font-family', /JetBrains Mono/);
  await expect(page.locator('.file-row .file-modified').first()).toHaveCSS('font-family', /JetBrains Mono/);
  await expect(page.locator('.mobile-file-size').first()).toHaveCSS('font-family', /JetBrains Mono/);
  await expect(page.locator('.sidebar-account-role')).toHaveCSS('font-family', /JetBrains Mono/);
  await expect(page.locator('.filename').first()).toHaveCSS('font-family', /system-ui/);
  await expect(page.locator('.sidebar-brand')).toHaveCSS('font-family', /system-ui/);
  await expect(page.locator('h1')).toHaveCSS('font-family', /system-ui/);
  await expect(page.locator('.file-row .file-size').first()).toHaveText('1.1 GB');
  await expect(page.locator('.file-row .file-modified').first()).toContainText('Jan');
  await expect(page.locator('.filename').first()).toHaveText('annual-report-final-2026-11-14-board-approved-revision-27.pdf');
  await expect.poll(() => page.evaluate(() => document.fonts.check('12px "JetBrains Mono"'))).toBe(true);

  for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: 844 });
    for (const theme of ['light', 'dark'] as const) {
      await setAppTheme(page, theme);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: testInfo.outputPath(`app-${width}-${theme}.png`), fullPage: true, animations: 'disabled' });
    }
  }
});

test('search, Trash and Accounts keep sans headings and monospace values', async ({ page }, testInfo) => {
  await installFileWorkspace(page);
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('test-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Files' })).toBeVisible();

  for (const theme of ['light', 'dark'] as const) {
    await page.setViewportSize({ width: 1280, height: 844 });
    await setAppTheme(page, theme);
    await page.getByLabel('Search files').fill('report');
    await expect(page.locator('.search-row')).toHaveCount(1);
    await expect(page.locator('.search-head .sort-heading')).toHaveCount(4);
    for (const heading of await page.locator('.search-head .sort-heading').all()) await expect(heading).toHaveCSS('font-family', /system-ui/);
    await expect(page.locator('.search-row .file-size')).toHaveCSS('font-family', /JetBrains Mono/);
    await expect(page.locator('.search-location')).toHaveCSS('font-family', /JetBrains Mono/);
    for (const width of [1280, 390]) {
      await page.setViewportSize({ width, height: 844 });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: testInfo.outputPath(`search-${width}-${theme}.png`), fullPage: true });
    }
    await page.setViewportSize({ width: 1280, height: 844 });
    await page.getByRole('button', { name: 'Clear search', exact: true }).click();
    await page.getByRole('button', { name: 'Trash', exact: true }).click();
    await expect(page.locator('.trash-row')).toHaveCount(1);
    await expect(page.locator('.trash-sort .sort-heading')).toHaveCount(4);
    for (const heading of await page.locator('.trash-sort .sort-heading').all()) await expect(heading).toHaveCSS('font-family', /system-ui/);
    await expect(page.locator('.trash-row .file-size')).toHaveCSS('font-family', /JetBrains Mono/);
    for (const width of [1280, 390]) {
      await page.setViewportSize({ width, height: 844 });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: testInfo.outputPath(`trash-${width}-${theme}.png`), fullPage: true });
    }
    await page.setViewportSize({ width: 1280, height: 844 });
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await expect(page.locator('.account-row')).toHaveCount(1);
    await expect(page.locator('.account-head .sort-heading')).toHaveCount(3);
    for (const heading of await page.locator('.account-head .sort-heading').all()) await expect(heading).toHaveCSS('font-family', /system-ui/);
    await expect(page.locator('.account-role')).toHaveCSS('font-family', /JetBrains Mono/);
    const sort = page.locator('.account-head .sort-heading').first();
    await expect(sort).toBeEnabled();
    await sort.focus();
    await sort.press('Enter');
    await expect(sort).toBeFocused();
    await expect(page.locator('.account-head [aria-label="Name"]')).toHaveAttribute('aria-sort', 'descending');
    await sort.press('Enter');
    await expect(page.locator('.account-head [aria-label="Name"]')).toHaveAttribute('aria-sort', 'ascending');
    await page.setViewportSize({ width: 390, height: 844 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`accounts-narrow-${theme}.png`), fullPage: true });
    await page.setViewportSize({ width: 1280, height: 844 });
    await page.getByRole('button', { name: 'Files', exact: true }).click();
  }
});
