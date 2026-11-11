import { expect, test } from '@playwright/test';

test('catalogue uses shared tokens and switches theme', async ({ page }) => {
  await page.goto('/design-system.html');
  await expect(page.getByRole('heading', { name: 'Design system', exact: true })).toBeVisible();
  const primary = page.locator('[data-color-token="--primary"] code');
  await expect(primary).toHaveText('#146c60');
  await page.getByRole('button', { name: 'Use dark theme' }).click();
  await expect(primary).toHaveText('#8bd0bb');
  await expect(page.getByRole('button', { name: 'Unavailable' })).toBeDisabled();
  await expect(page.getByLabel('Disabled field')).toBeDisabled();
  await page.evaluate(() => document.documentElement.style.setProperty('--type-ui', '15px'));
  await expect(page.locator('[data-type-sample="--type-ui"] .type-specimen')).toHaveCSS('font-size', '15px');
  await expect(page.getByRole('button', { name: 'Save changes' })).toHaveCSS('font-size', '15px');
  await page.getByRole('button', { name: 'Projects', exact: true }).click();
  await expect(page.getByRole('status')).toHaveText('Folder action selected.');
});

test('catalogue fits a narrow viewport', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/design-system.html');
  await expect(page.getByLabel('Account name')).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});
