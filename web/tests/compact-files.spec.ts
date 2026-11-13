import { expect, test, type Page } from '@playwright/test';

const longNames = ['A', 'B'].map(suffix => `Annual-financial-report-${'shared-section-'.repeat(7)}${suffix}.pdf`);
async function workspace(page: Page) {
  const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
  const entries = Array.from({ length: 20 }, (_, i) => ({ id: `file-${i}`, parent_id: 'root', name: i < 2 ? longNames[i] : `Project notes ${i}.txt`, kind: 'file', size: 2048, etag: `"file-${i}:1"` }));
  await page.route('**/*', route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/instance') return route.fulfill({ json: { name: 'Sylux' } });
    if (path === '/setup') return route.fulfill({ json: { setupRequired: false, user: null } });
    if (path === '/auth/csrf') return route.fulfill({ json: { csrf_token: 'csrf' } });
    if (path === '/auth/local') return route.fulfill({ json: { account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' } });
    if (path === '/folders/children' || path === '/folders/root/children') return route.fulfill({ json: { root, items: entries, has_more: false, next_cursor: null } });
    return route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.locator('.selectable-row')).toHaveCount(20);
  await page.mouse.move(0, 0);
}

for (const theme of ['light', 'dark'] as const) {
  test(`compact desktop workspace in ${theme}`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await workspace(page);
    await expect(page.getByRole('group', { name: 'Selected items', exact: true })).toHaveCount(0);
    for (const name of ['Search files and folders', 'Sort files by']) await expect(page.getByLabel(name, { exact: true })).toBeInViewport();
    for (const name of ['New folder', 'Choose files', 'Refresh folder']) await expect(page.getByRole('button', { name, exact: true })).toBeInViewport();
    const visible = await page.locator('.selectable-row').evaluateAll(rows => rows.filter(row => { const r = row.getBoundingClientRect(); return r.top >= 0 && r.bottom <= innerHeight; }).length);
    expect(visible).toBeGreaterThanOrEqual(12);
    await testInfo.attach('visible-rows', { body: String(visible), contentType: 'text/plain' });
    await page.screenshot({ path: `/tmp/syl41-desktop-${theme}.png` });
    const name = page.getByRole('button', { name: `Preview ${longNames[0]}`, exact: true });
    await name.focus();
    await expect(name).toBeFocused();
    expect(await name.locator('.filename').evaluate(el => el.scrollWidth <= el.clientWidth + 1 && el.scrollHeight <= el.clientHeight + 1)).toBe(true);
    const checkbox = page.getByRole('checkbox', { name: `Select ${longNames[0]}`, exact: true });
    await checkbox.focus();
    await page.keyboard.press('Space');
    await expect(page.getByRole('group', { name: 'Selected items', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Clear selection', exact: true }).click();
    await expect(checkbox).not.toBeChecked();
    await expect(checkbox).toBeFocused();
    expect(await name.locator('.filename').evaluate(el => getComputedStyle(el).whiteSpace)).toBe('normal');
  });

  test(`full mobile filenames and touch targets in ${theme}`, async ({ browser }) => {
    const context = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true, colorScheme: theme });
    const page = await context.newPage();
    await workspace(page);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    for (const fullName of longNames) {
      const row = page.getByRole('article', { name: fullName, exact: true });
      const label = row.locator('.filename');
      await expect(label).toHaveText(fullName);
      expect(await label.evaluate(el => el.scrollWidth <= el.clientWidth + 1 && el.scrollHeight <= el.clientHeight + 1)).toBe(true);
      for (const target of [row.locator('.entry-selection'), row.locator('.item-name'), row.getByRole('button', { name: `Options for ${fullName}` })]) {
        const box = await target.boundingBox();
        expect(box?.width).toBeGreaterThanOrEqual(44);
        expect(box?.height).toBeGreaterThanOrEqual(44);
      }
    }
    await page.screenshot({ path: `/tmp/syl41-mobile-${theme}.png` });
    await context.close();
  });

  test(`200 percent layout reflows in ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await workspace(page);
    await page.setViewportSize({ width: 720, height: 450 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(() => document.documentElement.style.zoom = '2');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.getByLabel('Sort files by', { exact: true }).focus();
    await expect(page.getByLabel('Sort files by', { exact: true })).toBeFocused();
    await page.screenshot({ path: `/tmp/syl41-zoom-${theme}.png` });
  });
}
