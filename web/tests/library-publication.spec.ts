import { expect, test } from '@playwright/test';

const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: 'root:1' };
const name = 'Reference library for the whole team';
const library = { id: 'library', parent_id: null, name, kind: 'folder', etag: 'library:1' };

for (const [width, scheme] of [[1280, 'light'], [390, 'dark']] as const) {
  test(`publication consent and uncertain response at ${width}px in ${scheme}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 760 });
    await page.emulateMedia({ colorScheme: scheme });
    let publication: string | null = null;
    let writeRequests = 0;
    const held = Promise.withResolvers<void>();
    await page.route('**/*', async route => {
      const { pathname } = new URL(route.request().url());
      const json = (value: unknown) => route.fulfill({ json: value });
      if (pathname === '/instance') return json({ name: 'Sylux' });
      if (pathname === '/auth/csrf') return json({ csrf_token: 'csrf' });
      if (pathname === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' });
      if (pathname === '/folders/children') return json({ root, items: [], next_cursor: null, has_more: false });
      if (pathname === '/folders/root/children') return json({ items: [], next_cursor: null, has_more: false });
      if (pathname === '/libraries') return json({ items: [root, library] });
      if (pathname === '/entries/library/direct-grants') return json({ items: [{ id: 'direct', handle: 'reader' }], publication_id: publication });
      if (pathname === '/libraries/library/publication') {
        writeRequests++;
        await held.promise;
        publication = 'published-library';
        return route.abort('connectionreset');
      }
      if (pathname === '/library-publications/published-library') {
        writeRequests++;
        publication = null;
        return route.fulfill({ status: 204 });
      }
      return route.continue();
    });
    const button = (label: string) => page.getByRole('button', { name: label, exact: true });
    await page.goto('/');
    await page.getByLabel('Account name').fill('owner');
    await page.getByLabel('Password', { exact: true }).fill('disposable password');
    await button('Sign in').click();
    await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
    if (width === 390) await button('Toggle Sidebar').click();
    await button('My libraries').click();
    const share = button(`Share ${name}`);
    await share.press('Enter');
    const popup = page.getByRole('dialog', { name: /^(Share|Publish|Withdraw publication of) Reference library/ });
    const review = button('Publish library…');
    await review.press('Enter');
    await expect(button('Cancel')).toBeFocused();
    await expect(popup.getByRole('heading')).toHaveText(`Publish ${name}?`);
    await expect(page.locator('html')).toHaveCSS('color-scheme', scheme);
    expect((await popup.boundingBox())!.width).toBeLessThan(width);
    expect(await popup.evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
    await page.screenshot({ path: info.outputPath(`publication-${width}-${scheme}.png`), animations: 'disabled' });
    await page.keyboard.press('Escape');
    await expect(review).toBeFocused();
    expect(writeRequests).toBe(0);
    await review.press('Enter');
    await button('Publish library').press('Enter');
    try {
      await expect(button('Publishing…')).toHaveAttribute('aria-busy', 'true');
      await expect(button('Cancel')).toBeDisabled();
      await page.keyboard.press('Escape');
      await expect(popup).toBeVisible();
      await expect.poll(() => writeRequests).toBe(1);
    } finally {
      held.resolve();
    }
    const refresh = button('Refresh access');
    await expect(popup.getByRole('alert')).toContainText('Could not confirm the change.');
    await expect(refresh).toBeFocused();
    await refresh.press('Enter');
    const withdraw = button('Withdraw publication…');
    await expect(withdraw).toBeFocused();
    await expect(popup.getByText('Read-only access for everyone signed in.')).toBeVisible();
    expect(writeRequests).toBe(1);
    await withdraw.press('Enter');
    await expect(button('Cancel')).toBeFocused();
    await expect(popup.getByText('Any separate direct shares', { exact: false })).toBeVisible();
    await button('Withdraw publication').press('Enter');
    await expect(popup.getByRole('status')).toHaveText('Publication withdrawn.');
    await expect(popup.getByRole('list', { name: 'Accounts with direct access' })).toContainText('reader');
    await popup.getByRole('button', { name: 'Close', exact: true }).last().press('Enter');
    await expect(share).toBeFocused();
    expect(writeRequests).toBe(2);
  });
}
