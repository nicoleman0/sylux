import { expect, test, type Locator } from '@playwright/test';
import type { PublicLink } from '../src/lib/public-folder-links';
import { mockPrivateLibraries, signInLibraryOwner } from './fixtures/private-libraries';

for (const [width, colorScheme] of [[1280, 'light'], [360, 'dark']] as const) {
  test(`owner can manage public folder links by keyboard at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.emulateMedia({ colorScheme });
    await mockPrivateLibraries(page);
    const links: PublicLink[] = [];
    let next = 0;
    await page.route('**/share-links', route => {
      if (route.request().method() === 'GET') return route.fulfill({ json: links });
      const { entry_id } = route.request().postDataJSON();
      const now = Math.floor(Date.now() / 1000);
      const link: PublicLink = {
        id: String(++next), kind: 'folder', entry_id,
        name: entry_id === 'field-work' ? 'Field work' : 'Projects',
        created_at: now, expires_at: now + 86400,
      };
      links.unshift(link);
      return route.fulfill({ status: 201, json: { ...link, url: `${new URL(page.url()).origin}/share/test/browse` } });
    });
    await page.route('**/share-links/*', route => {
      const id = new URL(route.request().url()).pathname.split('/').at(-1);
      const index = links.findIndex(link => link.id === id);
      expect(index).toBeGreaterThanOrEqual(0);
      links.splice(index, 1);
      return route.fulfill({ status: 204 });
    });
    await page.route('**/entries/field-work/direct-grants', route => route.fulfill({ json: { items: [] } }));
    await page.goto('/#libraries');
    await signInLibraryOwner(page);

    const button = (name: string) => page.getByRole('button', { name, exact: true });
    const sharing = (name: string) => page.getByRole('dialog', { name: `Share ${name}` });
    const active = (name: string) => button(`Public link active for ${name}. Manage public links`);
    const done = (dialog: Locator) => dialog.getByRole('button', { name: 'Done', exact: true });
    const create = async (name: string) => {
      const dialog = sharing(name);
      await expect(dialog).toContainText('They can see files added, restored or replaced, including through WebDAV sync, until the link expires or you revoke it.');
      const submit = dialog.getByRole('button', { name: 'Create link', exact: true });
      await expect(submit).toHaveAttribute('aria-disabled', 'false');
      await submit.press('Enter');
      await expect(dialog.getByRole('textbox', { name: 'Folder link' })).toBeFocused();
      await done(dialog).press('Enter');
    };
    const revoke = async (name: string) => {
      const dialog = sharing(name);
      await dialog.getByRole('button', { name: `Revoke link 1 for ${name}`, exact: true }).press('Enter');
      await expect(dialog.getByRole('status')).toHaveText('Link revoked.');
      await expect(done(dialog)).toBeFocused();
      await done(dialog).press('Enter');
    };

    const rootTrigger = button('Public link for Field work');
    await rootTrigger.press('Enter');
    await create('Field work');
    await expect(active('Field work')).toBeFocused();
    await expect(active('Field work')).toHaveText('Public link active');
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    await active('Field work').press('Enter');
    await create('Field work');
    await active('Field work').press('Enter');
    await revoke('Field work');
    await expect(active('Field work')).toBeFocused();
    await active('Field work').press('Enter');
    await revoke('Field work');
    await expect(rootTrigger).toBeFocused();
    await expect(active('Field work')).toHaveCount(0);

    const share = button('Share Field work');
    await share.press('Enter');
    const directDialog = sharing('Field work');
    await expect(directDialog.getByRole('region', { name: 'Shared with all' })).toBeVisible();
    const close = directDialog.locator('[data-slot=dialog-footer]').getByRole('button', { name: 'Close', exact: true });
    await expect(close).toBeEnabled();
    await close.press('Enter');
    await expect(sharing('Field work')).toHaveCount(0);
    await expect(share).toBeFocused();

    await page.getByRole('link', { name: 'Field work', exact: true }).press('Enter');
    const options = button('Options for Projects');
    await options.press('Enter');
    await page.getByRole('menuitem', { name: 'Public link', exact: true }).press('Enter');
    await create('Projects');
    await expect(options).toBeFocused();
    const row = page.getByRole('row', { name: 'Projects', exact: true });
    await expect(row).toContainText('Public link active');
    await button('Open Projects').press('Enter');
    await expect(active('Projects')).toHaveText('Public link active');
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    await active('Projects').press('Enter');
    await revoke('Projects');
    await expect(active('Projects')).toHaveCount(0);
    await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeFocused();
    if (width === 360) await button('Toggle Sidebar').press('Enter');
    await page.getByRole('link', { name: 'Sylux', exact: true }).press('Enter');
    await expect(page.getByRole('heading', { name: 'My libraries' })).toBeVisible();
    await expect(page.getByText('Public link active', { exact: true })).toHaveCount(0);
  });
}

test('an unavailable link list keeps the last known status until refresh succeeds', async ({ page }) => {
  await mockPrivateLibraries(page);
  const now = Math.floor(Date.now() / 1000);
  let links: PublicLink[] | null = [{
    id: 'link', kind: 'folder', entry_id: 'field-work', name: 'Field work',
    created_at: now, expires_at: now + 3600,
  }];
  let releaseFailure!: () => void;
  const heldFailure = new Promise<void>(resolve => { releaseFailure = resolve; });
  await page.route('**/share-links', async route => {
    if (links) return route.fulfill({ json: links });
    await heldFailure;
    return route.fulfill({ status: 500, json: { code: 'unavailable' } });
  });

  await page.goto('/#libraries');
  await signInLibraryOwner(page);
  const active = page.getByRole('button', { name: 'Public link active for Field work. Manage public links' });
  const warning = page.getByRole('status').filter({ hasText: 'Could not check public links.' });
  await expect(active).toBeVisible();

  links = null;
  await page.getByRole('link', { name: 'Field work', exact: true }).press('Enter');
  await expect(page.getByRole('heading', { name: 'Field work', exact: true })).toBeFocused();
  await active.focus();
  releaseFailure();
  await expect(warning).toContainText('Open Shared links in Account');
  await expect(active).toBeFocused();

  links = [];
  const refresh = page.getByRole('button', { name: 'Refresh folder', exact: true });
  await refresh.press('Enter');
  await expect(warning).toHaveCount(0);
  await expect(active).toHaveCount(0);
});
