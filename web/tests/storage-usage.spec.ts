import { expect, test, type Page, type Route } from '@playwright/test';

type Role = 'administrator' | 'user';

async function signIn(page: Page, role: Role, hash: string, usage: (route: Route) => unknown, allUsage: (route: Route) => unknown) {
  const requested: string[] = [];
  await page.route('**/*', route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/instance') return route.fulfill({ json: { name: 'Sylux' } });
    if (path === '/auth/csrf') return route.fulfill({ json: { csrf_token: 'csrf' } });
    if (path === '/auth/local') return route.fulfill({ json: { account_id: 'owner', handle: 'owner', role, csrf_token: 'csrf' } });
    if (path === '/accounts') return route.fulfill({ json: [
      { account_id: 'owner', handle: 'owner', role: 'administrator', enabled: true },
      { account_id: 'member', handle: 'member', role: 'user', enabled: true },
      { account_id: 'empty', handle: 'empty', role: 'user', enabled: true },
    ] });
    if (path === '/me/usage') { requested.push(path); return usage(route); }
    if (path === '/accounts/usage') { requested.push(path); return allUsage(route); }
    if (path === '/folders/children') return route.fulfill({ json: { root: { id: 'root', name: 'Files', kind: 'folder', etag: '1' }, items: [], next_cursor: null, has_more: false } });
    return route.continue();
  });
  await page.goto(hash);
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('long enough password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  return requested;
}

const all = {
  volume: { available_bytes: 412 * 1024 ** 3, total_bytes: 931 * 1024 ** 3 },
  accounts: [
    { account_id: 'member', files_bytes: 5 * 1024 ** 2, trash_bytes: null },
    { account_id: 'owner', files_bytes: 1536, trash_bytes: 0 },
    { account_id: 'empty', files_bytes: 0, trash_bytes: 0 },
  ],
};

for (const width of [1280, 390]) for (const theme of ['light', 'dark'] as const) {
  test(`account storage shows loading, values, zero and unknown ${width} ${theme}`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    let release!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    const requested = await signIn(page, 'user', '/#account', async route => { await held; return route.fulfill({ json: { files_bytes: 3 * 1024 ** 2 + 512 * 1024, trash_bytes: 0 } }); }, route => route.fulfill({ status: 500 }));
    const storage = page.getByRole('region', { name: 'Storage' });
    await expect(storage).toHaveAttribute('aria-busy', 'true');
    await expect(storage.getByRole('definition')).toHaveText(['Loading…', 'Loading…']);
    release();
    await expect(storage).toHaveAttribute('aria-busy', 'false');
    await expect(storage.getByRole('definition')).toHaveText(['3.5 MB', '0 B']);
    expect(await page.evaluate(() => document.body.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`account-${width}-${theme}.png`), fullPage: true });
    expect(requested).toEqual(['/me/usage']);
  });

  test(`administrator accounts show per-account storage and free space ${width} ${theme}`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await signIn(page, 'administrator', '/#settings', route => route.fulfill({ status: 500 }), route => route.fulfill({ json: all }));
    await expect(page.getByRole('heading', { name: 'Accounts', exact: true })).toBeVisible();
    await expect(page.getByText(/^Data volume:/)).toHaveText('Data volume: 412 GB free of 931 GB');
    const cells = (handle: string) => page.getByRole('row', { name: handle, exact: true }).locator('.account-usage');
    await expect(cells('owner')).toHaveText(['1.5 KB', '0 B']);
    await expect(cells('member')).toHaveText(['5 MB', 'Unknown']);
    await expect(cells('empty')).toHaveText(['0 B', '0 B']);
    if (width < 620) await expect(page.getByRole('row', { name: 'member', exact: true }).locator('.account-usage-summary')).toHaveText('Files 5 MB · Trash Unknown');
    expect(await page.evaluate(() => document.body.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`settings-${width}-${theme}.png`), fullPage: true });
  });
}

test('failed usage reads as unknown, never zero, and can be retried', async ({ page }) => {
  let fail = true;
  await signIn(page, 'user', '/#account', route => fail ? route.fulfill({ status: 500 }) : route.fulfill({ json: { files_bytes: 0, trash_bytes: 2048 } }), route => route.fulfill({ status: 500 }));
  const storage = page.getByRole('region', { name: 'Storage' });
  await expect(storage.getByRole('definition')).toHaveText(['Unknown', 'Unknown']);
  await expect(storage.getByRole('alert')).toHaveText('Could not load storage use.');
  fail = false;
  await storage.getByRole('button', { name: 'Try again' }).click();
  await expect(storage.getByRole('definition')).toHaveText(['0 B', '2 KB']);
  await expect(storage.getByRole('alert')).toHaveCount(0);
});

test('administrator free space and account figures fall back to unknown', async ({ page }) => {
  await signIn(page, 'administrator', '/#settings', route => route.fulfill({ status: 500 }), route => route.fulfill({ json: { volume: null, accounts: [] } }));
  await expect(page.getByText(/^Data volume:/)).toHaveText('Data volume: Unknown');
  await expect(page.getByRole('row', { name: 'owner', exact: true }).locator('.account-usage')).toHaveText(['Unknown', 'Unknown']);
});
