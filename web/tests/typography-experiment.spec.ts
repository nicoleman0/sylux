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
    if (path === '/folders/children') return json({ items: [], root, next_cursor: null, has_more: false });
    if (path === '/folders/root-1/children') return json({ items: entries, next_cursor: null, has_more: false });
    return route.continue();
  });
}

test('typography variants stay scoped to explicit development URLs', async ({ page }, testInfo) => {
  await page.goto('/design-system.html?keep=sample#rows');
  await expect(page.locator('html')).not.toHaveAttribute('data-typography-experiment');
  await page.screenshot({ path: testInfo.outputPath('catalogue-baseline.png'), fullPage: true, animations: 'disabled' });
  await page.getByRole('link', { name: /Metadata/ }).first().click();
  await expect(page).toHaveURL(/\/design-system\.html\?keep=sample&typography=metadata#rows$/);
  await expect(page.locator('html')).toHaveAttribute('data-typography-experiment', 'metadata');
  await expect(page.locator('.file-size').last()).toHaveCSS('font-family', /JetBrains Mono/);
  await expect(page.locator('.file-modified').last()).toHaveCSS('font-family', /JetBrains Mono/);
  await expect(page.locator('.modified-head')).not.toHaveCSS('font-family', /JetBrains Mono/);
  await expect(page.locator('.sort-heading').first()).not.toHaveCSS('font-family', /JetBrains Mono/);
  await page.screenshot({ path: testInfo.outputPath('catalogue-metadata.png'), fullPage: true, animations: 'disabled' });

  await page.getByRole('link', { name: /Headings/ }).first().click();
  await expect(page.locator('.sort-heading').first()).toHaveCSS('font-family', /JetBrains Mono/);
  await expect(page.locator('.modified-head')).toHaveCSS('font-family', /JetBrains Mono/);
  await expect(page.locator('.filename').last()).not.toHaveCSS('font-family', /JetBrains Mono/);
  await page.screenshot({ path: testInfo.outputPath('catalogue-headings.png'), fullPage: true, animations: 'disabled' });

  await page.getByRole('link', { name: /Filenames/ }).first().click();
  await expect(page.locator('.filename').last()).toHaveCSS('font-family', /JetBrains Mono/);
  await expect.poll(() => page.evaluate(() => document.fonts.check('12px "JetBrains Mono"'))).toBe(true);
  await expect(page.locator('.filename').last()).toContainText('quarterly-results-export_2026-11-14_final-review-approved.pdf');
  await expect(page.locator('.file-size').last()).toHaveText('1.15 GB');
  await expect(page.locator('.file-modified').last()).toContainText('Jan');
  await page.screenshot({ path: testInfo.outputPath('catalogue-filenames.png'), fullPage: true, animations: 'disabled' });

  await page.getByRole('link', { name: /Baseline/ }).first().click();
  await expect(page.locator('html')).not.toHaveAttribute('data-typography-experiment');
});

test('long filename and metadata samples fit desktop and narrow light and dark views', async ({ page }) => {
  for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: 844 });
    for (const theme of ['light', 'dark'] as const) {
      await page.goto('/design-system.html?typography=filenames');
      if (theme === 'dark') await page.getByRole('button', { name: 'Use dark theme' }).click();
      const filename = page.locator('.filename').last();
      await expect(filename).toBeVisible();
      await expect(page.locator('.mobile-file-size').last()).toContainText('1.15 GB');
      await expect(page.locator('.mobile-file-size time').last()).toHaveAttribute('datetime');
      await expect(page.locator('body')).toHaveCSS('background-color', theme === 'dark' ? 'rgb(23, 28, 28)' : 'rgb(255, 255, 255)');
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      expect(await filename.evaluate((node) => node.getBoundingClientRect().width)).toBeGreaterThan(0);
      expect(await page.locator('html').evaluate((node) => node.classList.contains('dark'))).toBe(theme === 'dark');
    }
  }
});

test('application typography variants render file data at desktop and narrow widths in both themes', async ({ page }, testInfo) => {
  await installFileWorkspace(page);
  for (const mode of ['baseline', 'metadata', 'headings', 'filenames'] as const) {
    await page.setViewportSize({ width: 1280, height: 844 });
    await page.goto(mode === 'baseline' ? '/' : `/?typography=${mode}`);
    await page.getByLabel('Account name').fill('owner');
    await page.getByLabel('Password').fill('test-password');
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'Files' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'New folder', exact: true })).toBeEnabled();
    await expect(page.locator('.list-head .sort-heading').first()).toBeEnabled();
    if (mode === 'baseline') await expect(page.locator('html')).not.toHaveAttribute('data-typography-experiment');
    else await expect(page.locator('html')).toHaveAttribute('data-typography-experiment', mode);
    const headings = page.locator('.list-head .sort-heading');
    await expect(headings).toHaveCount(3);
    for (let index = 0; index < 3; index++) {
      const family = await headings.nth(index).evaluate((node) => getComputedStyle(node).fontFamily);
      expect(family.includes('JetBrains Mono')).toBe(mode === 'headings' || mode === 'filenames');
    }
    for (const width of [1280, 390]) {
      await page.setViewportSize({ width, height: 844 });
      for (const theme of ['light', 'dark'] as const) {
        const isDark = await page.locator('html').evaluate((node) => node.classList.contains('dark'));
        if (isDark !== (theme === 'dark')) {
          await page.getByRole('button', { name: isDark ? 'Switch to light mode' : 'Switch to dark mode' }).click();
        }
        await expect(page.locator('body')).toHaveCSS('background-color', theme === 'dark' ? 'rgb(23, 28, 28)' : 'rgb(255, 255, 255)');
        await expect(page.locator('.filename').first()).toHaveText('annual-report-final-2026-11-14-board-approved-revision-27.pdf');
        await expect(page.locator('.file-row .file-size').first()).toHaveText('1.1 GB');
        await expect(page.locator('.file-row .file-modified').first()).toContainText('Jan');
        if (mode === 'metadata' || mode === 'headings' || mode === 'filenames') {
          await expect(page.locator('.file-row .file-size').first()).toHaveCSS('font-family', /JetBrains Mono/);
        }
        if (mode === 'headings' || mode === 'filenames') await expect(page.locator('.sort-heading').first()).toHaveCSS('font-family', /JetBrains Mono/);
        if (mode === 'filenames') {
          await expect(page.locator('.filename').first()).toHaveCSS('font-family', /JetBrains Mono/);
          await expect.poll(() => page.evaluate(() => document.fonts.check('12px "JetBrains Mono"'))).toBe(true);
        }
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        await page.screenshot({ path: testInfo.outputPath(`app-${mode}-${width}-${theme}.png`), fullPage: true, animations: 'disabled' });
      }
    }
  }
});
