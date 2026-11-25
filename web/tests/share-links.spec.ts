import { expect, test } from '@playwright/test';

const folder = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: 'root:1' };
const nested = { id: 'parent', parent_id: 'root', name: 'Work', kind: 'folder', etag: 'parent:1' };
const file = { id: 'file', parent_id: 'root', name: 'Shared report.txt', kind: 'file', etag: 'file:1', size: 3 };
const signedOutPath = '/share/synthetic-browser-test-link';

for (const { colorScheme, width } of [
  { colorScheme: 'light', width: 1280 },
  { colorScheme: 'dark', width: 390 },
] as const) {
  test(`share dialog supports expiry, copying and revocation in ${colorScheme} at ${width}px`, async ({ page, context, browser, baseURL }) => {
    await page.setViewportSize({ width, height: 750 });
    await page.emulateMedia({ colorScheme });
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    let active = false;
    let createRequests = 0;
    let failure: '400' | '403' | 'network' | '503' | null = null;
    let holdCreate!: () => void;
    let createArrived!: () => void;
    const held = new Promise<void>(resolve => { holdCreate = resolve; });
    const arrived = new Promise<void>(resolve => { createArrived = resolve; });
    const link = { id: 'link-id', entry_id: 'file', name: file.name, created_at: 1700000000, expires_at: 1702592000 };
    await page.route('**/*', async route => {
      const { pathname } = new URL(route.request().url());
      const json = (value: unknown, status = 200) => route.fulfill({ status, json: value });
      if (pathname === '/instance') return json({ name: 'Sylux' });
      if (pathname === '/auth/csrf') return json({ csrf_token: 'csrf' });
      if (pathname === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' });
      if (pathname === '/folders/children' || pathname === '/folders/root/children') return json({ root: folder, items: [nested, file], next_cursor: null, has_more: false });
      if (pathname === '/share-links' && route.request().method() === 'POST') {
        createRequests++;
        if (failure === 'network') { active = true; return route.abort(); }
        if (failure) return json({ code: 'synthetic_error' }, Number(failure));
        expect(route.request().postDataJSON()).toEqual({ entry_id: 'file', expiry_days: 30 });
        createArrived();
        await held;
        active = true;
        return json({ ...link, url: `${baseURL}${signedOutPath}` }, 201);
      }
      if (pathname === '/share-links') return json(active ? [link] : []);
      if (pathname === '/share-links/link-id' && route.request().method() === 'DELETE') {
        active = false;
        return route.fulfill({ status: 204 });
      }
      return route.continue();
    });
    await page.goto('/');
    await page.getByLabel('Account name').fill('owner');
    await page.getByLabel('Password', { exact: true }).fill('password');
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    const options = page.getByRole('button', { name: `Options for ${file.name}` });
    await options.focus();
    await page.keyboard.press('Enter');
    const shareAction = page.getByRole('menuitem', { name: 'Share', exact: true });
    await shareAction.focus();
    await page.keyboard.press('Enter');
    const dialog = page.getByRole('dialog', { name: `Share ${file.name}` });
    await expect(dialog).toContainText('Anyone with a link can download the file');
    await expect(dialog.getByLabel('Expires after')).toHaveValue('7');
    await expect(dialog.getByRole('button', { name: 'Create link' })).toBeEnabled();
    await expect(dialog).toBeInViewport();
    expect(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await page.screenshot({ path: test.info().outputPath(`share-${colorScheme}-${width}.png`) });

    for (const outcome of ['400', '403', 'network', '503'] as const) {
      failure = outcome;
      await dialog.getByRole('button', { name: 'Create link' }).click();
      await expect(dialog).toContainText(outcome === '400' || outcome === '403'
        ? 'Could not create the link. Check your access and try again.'
        : 'Could not confirm the new link.');
    }
    await expect(dialog.getByRole('button', { name: /Revoke link/ })).toBeVisible();
    await expect(dialog.getByRole('textbox', { name: 'Download link' })).toHaveCount(0);
    await dialog.getByRole('button', { name: /Revoke link/ }).click();
    failure = null;
    await dialog.getByLabel('Expires after').selectOption('30');
    const pending = dialog.getByRole('button', { name: 'Create link' }).click();
    await arrived;
    await expect(dialog.getByRole('button', { name: 'Creating…' })).toHaveAttribute('aria-busy', 'true');
    await page.keyboard.press('Escape');
    await expect(dialog).toBeVisible();
    holdCreate();
    await pending;
    expect(createRequests).toBe(5);
    const url = dialog.getByRole('textbox', { name: 'Download link' });
    await expect(url).toBeFocused();
    await dialog.getByRole('button', { name: 'Copy link' }).click();
    await expect(dialog.getByRole('status')).toHaveText('Link copied.');
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(`${baseURL}${signedOutPath}`);

    const signedOut = await browser.newContext({ acceptDownloads: true });
    try {
      await signedOut.route(`**${signedOutPath}`, route => {
        expect(route.request().headers()).not.toHaveProperty('cookie');
        return active
          ? route.fulfill({ headers: { 'content-disposition': `attachment; filename="${file.name}"` }, body: 'abc' })
          : route.fulfill({ status: 404, json: { code: 'share_not_found' } });
      });
      const recipient = await signedOut.newPage();
      const downloaded = recipient.waitForEvent('download');
      await recipient.evaluate(url => {
        const link = document.createElement('a');
        link.href = url;
        link.click();
      }, await url.inputValue());
      expect((await downloaded).suggestedFilename()).toBe(file.name);
      await dialog.getByRole('button', { name: /Revoke link/ }).click();
      await expect(dialog.getByRole('status')).toHaveText('Link revoked.');
      await expect(url).toHaveCount(0);
      const revoked = await recipient.goto(`${baseURL}${signedOutPath}`);
      expect(revoked?.status()).toBe(404);
    } finally {
      await signedOut.close();
    }
    await page.keyboard.press('Escape');
    await expect(options).toBeFocused();
  });
}
