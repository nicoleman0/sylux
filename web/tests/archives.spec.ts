import { expect, test, type Page } from '@playwright/test';

async function workspace(page: Page) {
  const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: 'root:1' };
  const entries = [
    { id: 'folder', parent_id: 'root', name: 'Documents', kind: 'folder', etag: 'folder:1' },
    { id: 'file', parent_id: 'root', name: 'Notes.txt', kind: 'file', size: 12, etag: 'file:1' },
  ];
  const downloads: string[] = [];
  page.on('download', download => downloads.push(new URL(download.url()).pathname));
  let release: (() => void) | undefined;
  let gate: Promise<void> | undefined;
  let failure: { status: number; code: string; limit?: number } | undefined;
  const selections: string[][] = [];
  await page.route('**/*', async route => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (value: unknown, status = 200) => route.fulfill({ status, json: value });
    if (path === '/instance') return json({ name: 'Sylux' });
    if (path === '/instance/appearance') return json({ accentColor: null, loginBackgroundColor: null, logoUrl: null, loginImageUrl: null });
    if (path === '/setup') return json({ setupRequired: false, user: null });
    if (path === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (path === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' });
    if (path === '/auth/logout') return route.fulfill({ status: 204 });
    if (path === '/folders/children') return json({ root, items: [], next_cursor: null, has_more: false });
    if (path === '/folders/root/children') return json({ items: entries, next_cursor: null, has_more: false });
    if (path === '/archives') {
      selections.push(request.postDataJSON().entry_ids);
      await gate;
      if (failure) return json({ ...failure, message: failure.code, request_id: 'fixture' }, failure.status);
      return json({ url: '/archives/ticket', name: 'Documents.zip', entry_count: 2 });
    }
    if (path === '/archives/ticket' || path === '/entries/file/content') {
      return route.fulfill({ headers: { 'Content-Disposition': 'attachment; filename="Documents.zip"' }, contentType: 'application/zip', body: 'fixture bytes' });
    }
    return route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('row', { name: 'Documents', exact: true })).toBeVisible();
  return {
    selections,
    archiveGets: () => downloads.filter(path => path === '/archives/ticket').length,
    fileGets: () => downloads.filter(path => path === '/entries/file/content').length,
    pause: () => { gate = new Promise<void>(resolve => release = resolve); },
    finish: () => { release?.(); gate = undefined; },
    fail: (status: number, code: string, limit?: number) => { failure = { status, code, limit }; },
  };
}

async function folderDownload(page: Page) {
  const options = page.getByRole('button', { name: 'Options for Documents', exact: true });
  await options.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('menuitem', { name: 'Download', exact: true })).toBeFocused();
  await page.keyboard.press('Enter');
}

for (const theme of ['light', 'dark'] as const) {
  for (const width of [1280, 390]) {
    test(`folder and selection downloads at ${width} in ${theme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
      const backend = await workspace(page);
      backend.pause();
      await folderDownload(page);
      const pending = page.getByRole('button', { name: 'Preparing download for Documents', exact: true });
      await expect(pending).toHaveAttribute('aria-busy', 'true');
      await pending.click();
      await expect(page.getByRole('menuitem', { name: 'Download', exact: true })).toHaveAttribute('data-disabled');
      await page.keyboard.press('Escape');
      await expect(pending).toBeFocused();
      expect(backend.selections).toEqual([['folder']]);
      await page.screenshot({ path: test.info().outputPath(`archive-pending-${width}-${theme}.png`), fullPage: true });
      backend.finish();
      await expect(page.getByRole('status')).toContainText('Download requested. Check your browser for progress.');
      await expect.poll(backend.archiveGets).toBe(1);
      await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
      await expect(page.getByText('Download requested. Check your browser for progress.', { exact: true })).toHaveCount(0);
      await page.getByRole('checkbox', { name: 'Select Documents', exact: true }).check();
      await page.getByRole('checkbox', { name: 'Select Notes.txt', exact: true }).check();
      const download = page.getByRole('button', { name: 'Download', exact: true });
      await download.focus();
      await expect(download).toBeFocused();
      await page.screenshot({ path: test.info().outputPath(`archive-selection-${width}-${theme}.png`), fullPage: true });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.keyboard.press('Enter');
      await expect.poll(() => backend.selections.length).toBe(2);
      expect(backend.selections[1]).toEqual(['folder', 'file']);
      await expect(download).toBeEnabled();
      await page.getByRole('checkbox', { name: 'Select Documents', exact: true }).uncheck();
      await download.click();
      await expect.poll(backend.fileGets).toBe(1);
      expect(backend.selections.length).toBe(2);
    });
  }
}

for (const [status, code, message] of [
  [413, 'archive_entry_limit', 'including folder contents. Download smaller groups.'],
  [409, 'archive_name_conflict', 'Two selected items have the same name. Download them separately.'],
  [404, 'entry_not_found', 'Some selected items are no longer available. Refresh your files.'],
  [500, 'internal', 'Download could not be started.'],
] as const) {
  test(`archive preparation reports ${code} and allows retry`, async ({ page }) => {
    const backend = await workspace(page);
    backend.fail(status, code, 10000);
    await folderDownload(page);
    await expect(page.getByRole('status')).toContainText(message);
    await expect(page.getByRole('button', { name: 'Options for Documents', exact: true })).toHaveAttribute('aria-busy', 'false');
    expect(backend.archiveGets()).toBe(0);
    await folderDownload(page);
    await expect.poll(() => backend.selections.length).toBe(2);
  });
}

test('refresh clears feedback while an active archive request continues', async ({ page }) => {
  const backend = await workspace(page);
  backend.pause();
  await folderDownload(page);
  await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
  await expect(page.getByText('Preparing download…', { exact: true })).toBeVisible();
  backend.finish();
  await expect.poll(backend.archiveGets).toBe(1);
  await expect(page.getByText('Download requested. Check your browser for progress.', { exact: true })).toHaveCount(0);
});

test('sign-out discards a pending archive ticket and its feedback', async ({ page }) => {
  const backend = await workspace(page);
  backend.pause();
  await folderDownload(page);
  await page.getByRole('button', { name: 'Log out', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
  backend.finish();
  await expect(page.getByText('Preparing download…', { exact: true })).toHaveCount(0);
  expect(backend.archiveGets()).toBe(0);
});

test('archive work stays visible across destinations without restoring cleared feedback', async ({ page }) => {
  const backend = await workspace(page);
  backend.pause();
  await folderDownload(page);
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await expect(page.getByText('Preparing download…', { exact: true })).toBeVisible();
  backend.finish();
  await expect.poll(backend.archiveGets).toBe(1);
  await page.getByRole('button', { name: 'Files', exact: true }).click();
  await expect(page.getByText('Download requested. Check your browser for progress.', { exact: true })).toHaveCount(0);
});

test('expired session during archive preparation preserves the sign-in explanation', async ({ page }) => {
  const backend = await workspace(page);
  backend.fail(401, 'unauthorized');
  await folderDownload(page);
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
  await expect(page.getByRole('status')).toContainText('Your session ended. Sign in again to continue.');
  expect(backend.archiveGets()).toBe(0);
});
