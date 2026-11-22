import { expect, test, type Locator } from '@playwright/test';
import { contrast } from '../src/lib/appearance';

function hex(color: string): string {
  const rgb = color.match(/^rgb\((\d+), (\d+), (\d+)\)$/);
  if (!rgb) throw new Error(`Expected opaque RGB, got ${color}`);
  return '#' + rgb.slice(1).map(channel => Number(channel).toString(16).padStart(2, '0')).join('');
}

function controlColors(control: Locator) {
  return control.evaluate(node => {
    const styles = getComputedStyle(node);
    return { background: styles.backgroundColor, foreground: styles.color };
  });
}

test('catalogue uses shared tokens and switches theme', async ({ page }) => {
  await page.goto('/design-system.html');
  await expect(page.getByRole('heading', { name: 'Design system', exact: true })).toBeVisible();
  const primary = page.locator('[data-color-token="--primary"] code');
  await expect(primary).toHaveText('#5142a6');
  const sorting = page.getByRole('table', { name: 'Sorting examples' });
  await expect(sorting.getByRole('columnheader', { name: 'Name', exact: true })).toHaveAttribute('aria-sort', 'ascending');
  const pending = sorting.getByRole('button', { name: 'Sort by modified, descending', exact: true });
  await expect(pending).toHaveAttribute('aria-busy', 'true');
  await expect(pending).toBeEnabled();
  await page.screenshot({ path: test.info().outputPath('sylux-modified-catalogue-light.png'), fullPage: true, animations: 'disabled' });
  await page.getByRole('button', { name: 'Use dark theme' }).click();
  await expect(primary).toHaveText('#b9abfa');
  await page.screenshot({ path: test.info().outputPath('sylux-modified-catalogue-dark.png'), fullPage: true, animations: 'disabled' });
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
  await expect(page.locator('.modified-row').filter({ hasText: 'Report.pdf' }).locator('.mobile-file-size time')).toBeVisible();
  await expect(page.locator('.modified-row').filter({ hasText: 'Projects' }).locator('.mobile-file-size')).toContainText('0 B');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('primary action hover and disabled states stay readable in both themes', async ({ page }) => {
  await page.goto('/design-system.html');
  const primary = page.getByRole('button', { name: 'Save changes' });
  const disabled = page.getByRole('button', { name: 'Unavailable' });
  for (const theme of ['light', 'dark']) {
    if (theme === 'dark') await page.getByRole('button', { name: 'Use dark theme' }).click();
    await expect(disabled).toBeDisabled();
    await expect(disabled).toHaveCSS('opacity', '1');
    const rest = await controlColors(primary);
    await primary.hover();
    await expect.poll(async () => (await controlColors(primary)).background).not.toBe(rest.background);
    await primary.evaluate(async node => {
      await Promise.all(node.getAnimations().map(animation => animation.finished));
    });
    const [hover, unavailable] = await Promise.all([controlColors(primary), controlColors(disabled)]);
    expect(unavailable.background).not.toBe(rest.background);
    for (const { background, foreground } of [rest, hover, unavailable]) {
      expect(contrast(hex(background), hex(foreground))).toBeGreaterThanOrEqual(4.5);
    }
  }
});
