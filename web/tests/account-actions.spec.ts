import { expect, test, type Page, type Route } from '@playwright/test';

async function accounts(page: Page) {
  const list = [
    { account_id: 'owner', handle: 'owner', role: 'administrator', enabled: true },
    { account_id: 'member', handle: 'member', role: 'user', enabled: true },
    { account_id: 'disabled', handle: 'disabled', role: 'user', enabled: false },
  ];
  await page.route('**/*', route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/instance') return route.fulfill({ json: { name: 'Sylux' } });
    if (path === '/auth/csrf') return route.fulfill({ json: { csrf_token: 'csrf' } });
    if (path === '/auth/local') return route.fulfill({ json: { account_id: 'owner', handle: 'owner', role: 'administrator', csrf_token: 'csrf' } });
    if (path === '/accounts') return route.fulfill({ json: list });
    if (path === '/folders/children') return route.fulfill({ json: { root: { id: 'root', name: 'Files', kind: 'folder', etag: '1' }, items: [], next_cursor: null, has_more: false } });
    if (path === '/folders/root/children') return route.fulfill({ json: { items: [], next_cursor: null, has_more: false } });
    return route.continue();
  });
  await page.goto('/#settings');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('long enough password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Accounts', exact: true })).toBeVisible();
}

for (const width of [1280, 390]) for (const theme of ['light', 'dark'] as const) {
  test(`account menus and status ${width} ${theme}`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await accounts(page);
    const owner = page.getByRole('row', { name: 'owner', exact: true });
    await expect(owner.getByText('You', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: /^Actions for / })).toHaveCount(3);
    expect(await page.evaluate(() => document.body.scrollWidth <= innerWidth)).toBe(true);
    const active = owner.getByRole('button', { name: 'Active', exact: true });
    await page.keyboard.press('Tab');
    await active.focus();
    await expect(page.getByRole('tooltip')).toHaveText('Active');
    await expect(active.locator('svg')).toHaveClass(/circle-check/);
    const disabled = page.getByRole('row', { name: 'disabled', exact: true }).getByRole('button', { name: 'Disabled', exact: true });
    await expect(disabled.locator('svg')).toHaveClass(/square-x/);
    const self = owner.getByRole('button', { name: 'Actions for owner' });
    await self.press('Enter');
    await expect(page.getByRole('menu')).toContainText('last enabled administrator');
    await expect(page.getByRole('menuitem')).toHaveCount(1);
    await page.keyboard.press('Escape');
    await expect(self).toBeFocused();
    const trigger = page.getByRole('button', { name: 'Actions for member' });
    await trigger.press('Enter');
    await expect(page.getByRole('menuitem', { name: 'Reset password' })).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(trigger).toBeFocused();
    if (process.env.SYLUX_REVIEW_SCREENSHOTS) {
      await page.screenshot({ path: testInfo.outputPath(`accounts-${width}-${theme}.png`), fullPage: true });
      await trigger.click();
      await page.screenshot({ path: testInfo.outputPath(`menu-${width}-${theme}.png`), fullPage: true });
      await page.keyboard.press('Escape');
    }
    await self.click();
    await page.getByRole('menuitem', { name: 'Personal account settings' }).click();
    await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
    await expect(page.getByRole('button', { name: 'Change password', exact: true })).toBeVisible();
  });
}

test('account action pending and uncertain results preserve recovery and row focus', async ({ page }) => {
  await accounts(page);
  let held: Route | undefined;
  let count = 0;
  await page.route('**/accounts/member/disable', route => { held = route; count++; });
  const trigger = page.getByRole('button', { name: 'Actions for member' });
  await trigger.click(); await page.getByRole('menuitem', { name: 'Disable', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Disable member?' });
  await expect(dialog).toHaveAccessibleDescription(/Direct shares to and from this account end permanently\. Enabling the account again does not restore them\./);
  await dialog.getByRole('button', { name: 'Disable account' }).click();
  await expect.poll(() => !!held).toBe(true);
  const pending = dialog.getByRole('button', { name: 'Disabling…' });
  await expect(pending).toHaveAttribute('aria-busy', 'true');
  await expect(pending).toBeDisabled();
  await expect(trigger).toBeDisabled();
  await page.keyboard.press('Escape'); await expect(dialog).toBeVisible();
  expect(count).toBe(1);
  await held!.abort('connectionreset');
  await expect(dialog.getByRole('alert')).toContainText('result could not be confirmed');
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(trigger).toBeFocused();
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'result could not be confirmed' })).toHaveCount(0);
});
