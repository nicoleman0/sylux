import fs from 'node:fs';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import https from 'node:https';
import { createRequire } from 'node:module';
import path from 'node:path';
import type { Browser, BrowserContext, Page, Request } from '../../web/node_modules/playwright-core';

const requireWeb = createRequire(path.resolve(__dirname, '../../web/package.json'));
const { test, expect, chromium }: typeof import('../../web/node_modules/@playwright/test') = requireWeb('@playwright/test');
const origin = process.env.S1_HTTPS_ORIGIN ?? '';
const fixtures = process.env.S1_BROWSER_FIXTURES ? JSON.parse(process.env.S1_BROWSER_FIXTURES) as {
  handle: string; folder: string; nested: string; file: string; folder_id: string;
  nested_id: string; file_id: string; sha256: string; length: number;
} : {} as {
  handle: string; folder: string; nested: string; file: string; folder_id: string;
  nested_id: string; file_id: string; sha256: string; length: number;
};
const password = process.env.S1_BROWSER_PASSWORD ?? '';
const phase = process.env.S1_BROWSER_PHASE ?? 'discovery';
let browser: Browser;
let context: BrowserContext;
let page: Page;

// Each 32-byte block is SHA-256 of its index, so a misplaced or replayed chunk changes the digest.
function patterned(length: number): Buffer {
  const bytes = Buffer.alloc(length);
  const counter = Buffer.alloc(8);
  for (let block = 0; block * 32 < length; block++) {
    counter.writeBigUInt64LE(BigInt(block));
    createHash('sha256').update(counter).digest().copy(bytes, block * 32);
  }
  return bytes;
}

async function signIn(tab = page): Promise<void> {
  await tab.goto(origin);
  await tab.getByLabel('Account name').fill(fixtures.handle);
  await tab.getByLabel('Password').fill(password);
  await tab.getByRole('button', { name: 'Sign in' }).click();
  await expect(tab.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
}

function markListingProgress(marker: string): void {
  test.info().annotations.push({ type: 's1_listing_progress', description: marker });
}

async function chooseUpload(file: { name: string; mimeType: string; buffer: Buffer }): Promise<void> {
  await expect(page.locator('#upload-file')).toBeEnabled();
  const admission = page.waitForRequest(request => request.method() === 'POST' && new URL(request.url()).pathname === '/uploads');
  await page.locator('#upload-file').setInputFiles(file);
  await admission;
}

test.beforeAll(async ({}, workerInfo) => {
  if (!origin || !process.env.S1_BROWSER_FIXTURES || !process.env.S1_BROWSER_FIXTURE || !process.env.S1_BROWSER_PHASE || !password || !process.env.S1_CA_CERT || !process.env.S1_CERTUTIL) {
    throw new Error('owned browser fixture required');
  }
  if (!fs.lstatSync(path.join(process.env.S1_BROWSER_FIXTURE, '.fixture-owner')).isFile()) throw new Error('owned fixture required');
  const home = path.join(process.env.S1_BROWSER_FIXTURE, `browser-${process.env.S1_BROWSER_PHASE}`, `storage-${workerInfo.workerIndex}-${process.pid}`);
  fs.mkdirSync(path.join(home, '.pki/nssdb'), { recursive: true, mode: 0o700 });
  const nss = path.join(home, '.pki/nssdb');
  execFileSync(process.env.S1_CERTUTIL, ['-N', '--empty-password', '-d', `sql:${nss}`], { timeout: 10_000, stdio: 'ignore' });
  execFileSync(process.env.S1_CERTUTIL, ['-A', '-d', `sql:${nss}`, '-n', 'Sylux disposable CA', '-t', 'C,,', '-i', process.env.S1_CA_CERT], { timeout: 10_000, stdio: 'ignore' });
  browser = await chromium.launch({
    executablePath: chromium.executablePath(), headless: true,
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: home, XDG_CACHE_HOME: home },
    args: ['--host-resolver-rules=MAP fixture.test 127.0.0.1', '--no-proxy-server'],
  });
  expect(browser.version()).toBe('158.0.8127.18');
});
async function storageContext(): Promise<BrowserContext> {
  const owned = await browser.newContext({ ignoreHTTPSErrors: false, acceptDownloads: true, viewport: { width: 1440, height: 900 } });
  await owned.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (url.hostname === 'fixture.test' && url.port === new URL(origin).port && url.protocol === 'https:') await route.continue();
    else await route.abort('blockedbyclient');
  });
  return owned;
}

test.beforeEach(async () => {
  context = await storageContext();
  page = await context.newPage();
});
test.afterEach(async () => { await context?.close(); });
test.afterAll(async () => { await browser?.close(); });

test('private libraries keep same-name folders and saved routes separate', { annotation: { type: 'harness_case', description: 'storage_private_libraries' } }, async () => {
  await signIn();
  const handle = `library-owner-${phase}`;
  const memberPassword = 'Private library fixture password';
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Create account', exact: true }).click();
  const account = page.getByRole('dialog', { name: 'Create account', exact: true });
  await account.getByLabel('Account name').fill(handle);
  await account.getByLabel('Password', { exact: true }).fill(memberPassword);
  await account.getByLabel('Confirm password', { exact: true }).fill(memberPassword);
  await account.getByRole('button', { name: 'Create account', exact: true }).click();
  await expect(account).toBeHidden();

  async function signInMember() {
    await page.getByLabel('Account name').fill(handle);
    await page.getByLabel('Password', { exact: true }).fill(memberPassword);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  }
  const myLibraries = page.getByRole('button', { name: 'My libraries', exact: true });
  const files = (filename: string) => page.getByRole('button', { name: `Preview ${filename}`, exact: true });
  await page.getByRole('button', { name: 'Log out', exact: true }).click();
  await signInMember();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  await myLibraries.click();
  const libraries = page.getByRole('list', { name: 'My libraries' });
  await expect(libraries.getByRole('link')).toHaveText(['Files']);
  const bookmarks: string[] = [];
  for (const [name, filename] of [['Field work', 'survey.txt'], ['Studio', 'sketch.txt']]) {
    await page.getByRole('button', { name: 'New library', exact: true }).press('Enter');
    const dialog = page.getByRole('dialog', { name: 'New library' });
    await dialog.getByLabel('Library name').fill(name);
    await dialog.getByRole('button', { name: 'Create library' }).press('Enter');
    await expect(dialog).toBeHidden();
    await libraries.getByRole('link', { name, exact: true }).press('Enter');
    await expect(page.getByRole('heading', { name, exact: true })).toBeFocused();
    await page.getByRole('button', { name: 'New folder', exact: true }).click();
    await page.getByLabel('Folder name', { exact: true }).fill('Projects');
    await page.getByRole('button', { name: 'Create folder', exact: true }).click();
    await page.getByRole('button', { name: 'Open Projects', exact: true }).press('Enter');
    await chooseUpload({ name: filename, mimeType: 'text/plain', buffer: Buffer.from(name) });
    await expect(files(filename)).toBeVisible();
    bookmarks.push(page.url());
    await myLibraries.click();
  }
  await expect(libraries.getByRole('link')).toHaveText(['Files', 'Field work', 'Studio']);
  expect(bookmarks[0]).not.toBe(bookmarks[1]);
  for (const [index, filename] of ['survey.txt', 'sketch.txt'].entries()) {
    await page.goto(bookmarks[index]);
    await expect(files(filename)).toBeVisible();
    await page.reload();
    await signInMember();
    await expect(files(filename)).toBeVisible();
    await expect(files(index === 0 ? 'sketch.txt' : 'survey.txt')).toHaveCount(0);
  }

  const workspaceLocation = page.locator('.workspace-location > span:last-child');
  await expect(workspaceLocation).toHaveText('Studio');
  await expect(myLibraries).toHaveAttribute('aria-current', 'location');
  await expect(page.getByRole('button', { name: 'Files', exact: true })).not.toHaveAttribute('aria-current');
  for (const [action, filename] of [['Copy', 'survey.txt'], ['Move', 'sketch.txt']]) {
    const [source, destination] = action === 'Copy' ? bookmarks : [...bookmarks].reverse();
    const [sourceLibrary, destinationLibrary] = action === 'Copy' ? ['Field work', 'Studio'] : ['Studio', 'Field work'];
    await page.goto(source);
    await expect(files(filename)).toBeVisible();
    await page.getByRole('button', { name: `Options for ${filename}`, exact: true }).click();
    await page.getByRole('menuitem', { name: action, exact: true }).click();
    const picker = page.getByRole('dialog', { name: `${action} ${filename}`, exact: true });
    await expect(picker.getByRole('navigation', { name: `${action} destination path` })).toHaveText(sourceLibrary);
    const chooseLibrary = picker.getByRole('button', { name: 'Choose a library', exact: true });
    await expect(chooseLibrary).toBeEnabled();
    await chooseLibrary.click();
    const choices = picker.getByRole('group', { name: 'Destination libraries' });
    await choices.getByRole('button', { name: destinationLibrary, exact: true }).click();
    await picker.getByRole('group', { name: 'Destination folders' }).getByRole('button', { name: 'Projects', exact: true }).click();
    const confirm = picker.getByRole('button', { name: `${action} here`, exact: true });
    await expect(confirm).toBeEnabled();
    await confirm.press('Enter');
    await expect(picker).toBeHidden();

    if (action === 'Copy') await expect(files(filename)).toBeVisible();
    else await expect(files(filename)).toHaveCount(0);
    await page.goto(destination);
    await expect(files(filename)).toBeVisible();
    await files(filename).click();
    const preview = page.getByRole('dialog', { name: `Preview ${filename}`, exact: true });
    await expect(preview).toContainText(sourceLibrary);
    await preview.getByRole('button', { name: 'Close preview', exact: true }).click();
  }

  await page.getByRole('button', { name: 'Log out', exact: true }).click();
  await signIn();
  await myLibraries.click();
  await expect(libraries.getByRole('link')).toHaveText(['Files']);
  await page.getByRole('button', { name: 'Files', exact: true }).click();
  await page.getByLabel('Search files and folders').fill('survey.txt');
  await expect(page.getByText('No matching files or folders.')).toBeVisible();
  for (const bookmark of bookmarks) {
    await page.goto(bookmark);
    await expect(page.getByRole('alert')).toContainText('This folder is unavailable');
  }
});

test('desktop and narrow viewport keep folder browsing and upload controls usable', { annotation: { type: 'harness_case', description: 'storage_responsive_controls' } }, async () => {
  await signIn();
  await expect(page.getByRole('navigation', { name: 'Files', exact: true })).toBeVisible();
  await expect(page.getByLabel('Choose files to upload')).toBeAttached();
  await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Log out', exact: true })).toBeVisible();

  // Playwright mobile viewport emulation only; this is not a physical-device claim.
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeInViewport();
  await expect(page.getByRole('button', { name: 'Log out', exact: true })).toBeInViewport();
  await page.setViewportSize({ width: 1440, height: 900 });
});

test('loading, empty, and recoverable listing error are distinguishable', { annotation: { type: 'harness_case', description: 'storage_listing_states' } }, async () => {
  await signIn();
  markListingProgress('signed-in');
  let release!: () => void;
  const paused = new Promise<void>(resolve => { release = resolve; });
  let arrived!: () => void;
  const requestArrived = new Promise<void>(resolve => { arrived = resolve; });
  // Hold the real authenticated route response, then release it for the empty/error checks.
  await page.route(`**/folders/${fixtures.folder_id}/children**`, async route => {
    arrived();
    await paused;
    await route.continue();
  });
  await page.getByRole('button', { name: `Open ${fixtures.folder}`, exact: true }).click();
  await requestArrived;
  markListingProgress('first-folder-response-held');
  await expect(page.getByRole('status').filter({ hasText: 'Opening files…' })).toBeVisible();
  release();
  await page.unrouteAll({ behavior: 'ignoreErrors' });

  await page.getByRole('navigation', { name: 'Folder path' }).getByRole('button', { name: 'Files' }).click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  markListingProgress('returned-to-root');
  await page.getByRole('button', { name: `Open ${fixtures.folder}`, exact: true }).click();
  await page.locator('.file-row').filter({ hasText: fixtures.nested }).getByRole('button', { name: 'Open' }).click();
  await expect(page.getByText(fixtures.file, { exact: true })).toBeVisible();
  markListingProgress('nested-file-visible');
  const breadcrumb = page.getByRole('navigation', { name: 'Folder path' });
  await expect(breadcrumb.getByRole('button', { name: 'Files' })).not.toHaveAttribute('aria-current', 'page');
  await expect(page.getByRole('heading', { name: fixtures.nested, level: 1 })).toBeVisible();
  await expect(breadcrumb.getByRole('button', { name: fixtures.nested })).toHaveCount(0);
  await breadcrumb.getByRole('button', { name: fixtures.folder }).click();
  await expect(page.getByRole('heading', { name: fixtures.folder })).toBeVisible();
  await breadcrumb.getByRole('button', { name: 'Files' }).click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  markListingProgress('breadcrumb-roundtrip-complete');
  await page.route(`**/folders/${fixtures.folder_id}/children**`, route => route.abort('failed'));
  await page.getByRole('button', { name: `Open ${fixtures.folder}`, exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Could not load this folder');
  await page.unrouteAll({ behavior: 'ignoreErrors' });
  markListingProgress('error-state-complete');

  // Use an actual empty folder, created through the UI and real mutation routes.
  await page.getByRole('navigation', { name: 'Folder path' }).getByRole('button', { name: 'Files' }).click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  const emptyName = `empty-${phase}-${Date.now()}`;
  await page.getByRole('button', { name: 'New folder', exact: true }).click();
  await page.getByLabel('Folder name', { exact: true }).fill(emptyName);
  await page.getByRole('button', { name: 'Create folder' }).click();
  await page.locator('.file-row').filter({ hasText: emptyName }).getByRole('button', { name: 'Open' }).click();
  await expect(page.getByText('This folder is empty')).toBeVisible();
  markListingProgress('empty-folder-complete');

  await page.getByRole('navigation', { name: 'Folder path' }).getByRole('button', { name: 'Files' }).click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();

  await page.getByRole('button', { name: `Open ${fixtures.folder}`, exact: true }).click();
  await expect(page.getByRole('heading', { name: fixtures.folder })).toBeVisible();
  markListingProgress('race-parent-open');
  let releaseStale!: () => void;
  const stalePaused = new Promise<void>(resolve => { releaseStale = resolve; });
  let staleArrived!: () => void;
  const staleRequestArrived = new Promise<void>(resolve => { staleArrived = resolve; });
  await page.route(`**/folders/${fixtures.folder_id}/children**`, async route => {
    staleArrived();
    await stalePaused;
    await route.continue();
  });
  const staleResponse = page.waitForResponse(response =>
    new URL(response.url()).pathname === `/folders/${fixtures.folder_id}/children`
    && response.request().method() === 'GET'
  );
  await page.getByRole('button', { name: 'Refresh folder' }).click();
  await staleRequestArrived;
  markListingProgress('stale-response-held');
  await page.getByRole('navigation', { name: 'Files', exact: true }).getByRole('button', { name: 'Files', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  await expect(page.getByRole('button', { name: `Open ${fixtures.folder}`, exact: true })).toBeVisible();
  markListingProgress('root-open-under-race');
  releaseStale();
  await staleResponse;
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  await expect(page.getByRole('button', { name: `Open ${fixtures.folder}`, exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: `Open ${fixtures.nested}`, exact: true })).toHaveCount(0);
  markListingProgress('stale-response-drained');
  await page.unrouteAll({ behavior: 'ignoreErrors' });
  await page.getByRole('button', { name: `Open ${fixtures.folder}`, exact: true }).click();
  await expect(page.getByRole('heading', { name: fixtures.folder })).toBeVisible();

  const firstPage = Array.from({ length: 100 }, (_, index) => {
    const id = (0x90000000000000000000000000000000n + BigInt(index)).toString(16).padStart(32, '0');
    return { id, parent_id: fixtures.folder_id, name: `page-${phase}-${String(index).padStart(3, '0')}`, kind: 'folder', etag: `"${id}:1"` };
  });
  const secondPage = [{ id: '900000000000000000000000000000ff', parent_id: fixtures.folder_id, name: `page-${phase}-100`, kind: 'folder', etag: '"900000000000000000000000000000ff:1"' }];
  const cursors: Array<string | null> = [];
  await page.route(`**/folders/${fixtures.folder_id}/children**`, async route => {
    const url = new URL(route.request().url());
    const cursor = url.searchParams.get('cursor');
    cursors.push(cursor);
    if (!cursor) {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ items: firstPage, next_cursor: 'synthetic-next-page', has_more: true }) });
    } else {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ items: secondPage, next_cursor: null, has_more: false }) });
    }
  });
  await page.getByRole('button', { name: 'Refresh folder' }).click();
  await expect(page.getByRole('button', { name: 'Next page' })).toBeVisible();
  await expect(page.getByText('100 items on this page', { exact: true })).toBeVisible();
  markListingProgress('first-page-visible');
  await page.getByRole('button', { name: 'Next page' }).click();
  await expect(page.getByRole('button', { name: 'Previous page' })).toBeVisible();
  await expect(page.locator('.file-row').filter({ hasText: `page-${phase}-100` })).toBeVisible();
  markListingProgress('next-page-visible');
  await page.getByRole('button', { name: 'Previous page' }).click();
  await expect(page.locator('.file-row').filter({ hasText: `page-${phase}-000` })).toBeVisible();
  await expect(page.locator('.file-row').filter({ hasText: `page-${phase}-100` })).toHaveCount(0);
  expect(cursors).toEqual([null, 'synthetic-next-page', null]);
  markListingProgress('previous-page-verified');
});

test('Unicode, zero-byte, and binary files round-trip through real upload and download routes', { annotation: { type: 'harness_case', description: 'storage_upload_download_roundtrip' } }, async () => {
  await signIn();
  await page.evaluate(() => {
    Object.defineProperty(window, '__s1LargeBlobReads', { configurable: true, writable: true, value: 0 });
    const original = Blob.prototype.arrayBuffer;
    Blob.prototype.arrayBuffer = function() {
      // Uploads read bounded slices for hashing; no read may exceed the 8 MiB chunk limit.
      if (this.size > 8 * 1024 * 1024) {
        (window as unknown as { __s1LargeBlobReads: number }).__s1LargeBlobReads += 1;
      }
      return original.call(this);
    };
  });
  const cases = [
    { name: `résumé-雪-${phase}.txt`, bytes: new TextEncoder().encode('naïve 🧪\n') },
    { name: `zero-${phase}.bin`, bytes: new Uint8Array() },
    { name: `opaque-${phase}.bin`, bytes: new Uint8Array([0, 255, 1, 128, 13, 10]) },
    { name: `bounded-${phase}.bin`, bytes: new Uint8Array(1024 * 1024).fill(0x5a) },
  ];
  for (const item of cases) {
    await chooseUpload({ name: item.name, mimeType: 'application/octet-stream', buffer: Buffer.from(item.bytes) });
    const savedItem = page.locator('.file-row').filter({ has: page.getByText(item.name, { exact: true }) });
    await expect(savedItem.getByText(item.name, { exact: true })).toBeVisible({ timeout: 15_000 });
    const downloadPromise = page.waitForEvent('download');
    await savedItem.getByRole('button', { name: `Options for ${item.name}`, exact: true }).click();
    await page.getByRole('menuitem', { name: 'Download', exact: true }).click();
    const download = await downloadPromise;
    expect(download.suggestedFilename()).toBe(item.name);
    const stream = await download.createReadStream();
    const chunks: Buffer[] = [];
    for await (const chunk of stream!) chunks.push(Buffer.from(chunk));
    expect(Buffer.concat(chunks)).toEqual(Buffer.from(item.bytes));
  }
  const folderRow = page.locator('.file-row').filter({ has: page.getByText(fixtures.folder, { exact: true }) });
  const folderDownload = page.waitForEvent('download');
  await folderRow.getByRole('button', { name: `Options for ${fixtures.folder}`, exact: true }).click();
  await page.getByRole('menuitem', { name: 'Download', exact: true }).click();
  const folderZip = await folderDownload;
  expect(folderZip.suggestedFilename()).toBe(`${fixtures.folder}.zip`);
  expect(await folderZip.failure()).toBeNull();
  const archivePath = await folderZip.path();
  const checked = JSON.parse(execFileSync('python3', ['-c', `
import hashlib, json, sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as archive:
    assert archive.testzip() is None
    print(json.dumps({name: hashlib.sha256(archive.read(name)).hexdigest() for name in archive.namelist() if not name.endswith('/')}))
`, archivePath!], { encoding: 'utf8' }));
  expect(checked[`${fixtures.folder}/${fixtures.nested}/${fixtures.file}`]).toBe(fixtures.sha256);

  for (const name of [fixtures.folder, cases[0]!.name, cases[1]!.name]) {
    await page.getByRole('checkbox', { name: `Select ${name}`, exact: true }).check();
  }
  const selectionDownload = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Download', exact: true }).click();
  const selectionZip = await selectionDownload;
  expect(selectionZip.suggestedFilename()).toBe('download.zip');
  expect(await selectionZip.failure()).toBeNull();
  const selectionPath = await selectionZip.path();
  const selectedNames = JSON.parse(execFileSync('python3', ['-c', `
import json, sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as archive:
    assert archive.testzip() is None
    print(json.dumps(archive.namelist()))
`, selectionPath!], { encoding: 'utf8' }));
  expect(selectedNames).toContain(`${fixtures.folder}/${fixtures.nested}/${fixtures.file}`);
  expect(selectedNames).toContain(cases[0]!.name);
  expect(selectedNames).toContain(cases[1]!.name);
  expect(await page.evaluate(() => (window as unknown as { __s1LargeBlobReads: number }).__s1LargeBlobReads))
    .toBe(0);
});

test('browsing remains available while a real upload body is delayed', { annotation: { type: 'harness_case', description: 'storage_delayed_upload_browsing' } }, async () => {
  await signIn();
  let release!: () => void;
  const paused = new Promise<void>(resolve => { release = resolve; });
  let arrived!: () => void;
  const requestArrived = new Promise<void>(resolve => { arrived = resolve; });
  await page.route('**/uploads/*/content', async route => {
    arrived();
    await paused;
    await route.continue();
  });
  await chooseUpload({ name: `slow-upload-${phase}.bin`, mimeType: 'application/octet-stream', buffer: Buffer.alloc(512 * 1024, 0x5a) });
  await requestArrived;
  await page.getByRole('button', { name: `Open ${fixtures.folder}`, exact: true }).click();
  await expect(page.getByRole('heading', { name: fixtures.folder })).toBeVisible();
  const report = page.getByRole('region', { name: 'Upload results', exact: true });
  await expect(report).toBeVisible();
  await expect(report.getByRole('status', { name: 'Uploading', exact: true })).toBeVisible();
  await expect(report.locator('details')).toHaveCount(0);
  release();
  await expect(report.getByRole('status').filter({ hasText: 'Uploaded to Files' })).toBeVisible({ timeout: 20_000 });
  await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeEnabled();
  await expect(report.locator('details')).toHaveCount(0);
  await page.getByRole('navigation', { name: 'Files', exact: true }).getByRole('button', { name: 'Files', exact: true }).click();
  await expect(page.getByRole('row', { name: `slow-upload-${phase}.bin`, exact: true })).toBeVisible();
});

test('cancel aborts the in-flight upload without claiming a saved file', { annotation: { type: 'harness_case', description: 'storage_upload_cancel' } }, async () => {
  await signIn();
  let release!: () => void;
  const paused = new Promise<void>(resolve => { release = resolve; });
  let arrived!: () => void;
  const requestArrived = new Promise<void>(resolve => { arrived = resolve; });
  await page.route('**/uploads/*/content', async route => { arrived(); await paused; await route.continue(); });
  const fileName = `cancel-me-${phase}.bin`;
  await chooseUpload({ name: fileName, mimeType: 'application/octet-stream', buffer: Buffer.alloc(1024 * 1024, 0x44) });
  await requestArrived;
  await page.getByRole('button', { name: 'Cancel upload' }).click();
  release();
  await expect(page.getByRole('status').filter({ hasText: 'Upload canceled' })).toContainText('Upload canceled');
  await expect(page.locator('.file-row').filter({ hasText: fileName })).toHaveCount(0);
});

async function forwardFixtureMutation(request: Request, expected: RegExp): Promise<number> {
  const fixtureOrigin = new URL(origin);
  const target = new URL(request.url());
  if (target.origin !== fixtureOrigin.origin || target.search || !expected.test(target.pathname) || request.method() !== 'POST') {
    throw new Error('unexpected fixture mutation route');
  }
  const headers = await request.allHeaders();
  const ca = fs.readFileSync(process.env.S1_CA_CERT!);
  return new Promise<number>((resolve, reject) => {
    const forwarded = https.request({
      hostname: fixtureOrigin.hostname,
      family: 4,
      port: Number(fixtureOrigin.port),
      path: target.pathname,
      method: request.method(),
      headers,
      ca,
      rejectUnauthorized: true,
      lookup: (_hostname, _options, callback) => callback(null, '127.0.0.1', 4),
    }, response => {
      response.once('end', () => resolve(response.statusCode ?? 0));
      response.resume();
    });
    forwarded.setTimeout(10_000, () => forwarded.destroy(new Error('fixture mutation forward timed out')));
    forwarded.once('error', reject);
    forwarded.end(request.postDataBuffer() ?? undefined);
  });
}

test('lost commit response is resolved through the original upload status', { annotation: { type: 'harness_case', description: 'storage_lost_commit_response' } }, async () => {
  await signIn();
  const fixtureOrigin = new URL(origin);
  const ca = fs.readFileSync(process.env.S1_CA_CERT!);
  let dropped = false;
  let firstStatusLookupFailed = false;
  const statusLookups: string[] = [];
  await page.route('**/uploads/*', async route => {
    const request = route.request();
    const target = new URL(request.url());
    if (request.method() === 'GET' && /^\/uploads\/[A-Za-z0-9-]+$/.test(target.pathname)) statusLookups.push(target.pathname);
    // Fail the outcome check after the lost commit reply, not the status read before sending.
    if (dropped && !firstStatusLookupFailed && request.method() === 'GET' && /^\/uploads\/[A-Za-z0-9-]+$/.test(target.pathname)) {
      firstStatusLookupFailed = true;
      await route.abort('failed');
      return;
    }
    await route.continue();
  });
  await page.route('**/uploads/*/commit', async route => {
    expect(await forwardFixtureMutation(route.request(), /^\/uploads\/[A-Za-z0-9-]+\/commit$/)).toBe(201);
    dropped = true;
    await route.abort('connectionreset');
  });
  const fileName = `commit-response-lost-${phase}.bin`;
  await chooseUpload({ name: fileName, mimeType: 'application/octet-stream', buffer: Buffer.from([0, 255, 7]) });
  await expect.poll(() => dropped).toBe(true);
  await expect.poll(() => firstStatusLookupFailed).toBe(true);
  const unresolved = page.getByRole('alert').filter({ hasText: 'Upload status needs checking' });
  await expect(unresolved).toBeVisible();
  await expect(page.getByRole('button', { name: 'Check upload status' })).toBeVisible();
  await expect(page.getByRole('status').filter({ hasText: 'Uploaded to Files' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Check upload status' }).click();
  await expect(unresolved).toBeHidden();
  await expect(page.locator('.file-row').filter({ hasText: fileName })).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole('status').filter({ hasText: 'Uploaded to Files' })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Upload results', exact: true }).locator('details')).toHaveCount(0);
  await expect(page.locator('.upload-status')).toHaveCount(0);

  let ticketRequests = 0;
  let admissionRequests = 0;
  let admittedId = '';
  page.on('request', request => {
    const target = new URL(request.url());
    if (request.method() === 'POST' && target.pathname === '/mutation-tickets') ticketRequests += 1;
    if (request.method() === 'POST' && target.pathname === '/uploads') admissionRequests += 1;
  });
  await page.route('**/uploads', async route => {
    const request = route.request();
    const target = new URL(request.url());
    if (request.method() !== 'POST' || target.origin !== fixtureOrigin.origin || target.pathname !== '/uploads' || target.search) {
      throw new Error('unexpected fixture admission route');
    }
    const headers = await request.allHeaders();
    const body = request.postDataBuffer();
    const response = await new Promise<{ status: number; body: string }>((resolve, reject) => {
      const forwarded = https.request({
        hostname: 'fixture.test',
        servername: 'fixture.test',
        family: 4,
        port: Number(fixtureOrigin.port),
        path: target.pathname,
        method: request.method(),
        headers,
        ca,
        rejectUnauthorized: true,
        lookup: (_hostname, _options, callback) => callback(null, '127.0.0.1', 4),
      }, incoming => {
        const chunks: Buffer[] = [];
        incoming.on('data', chunk => chunks.push(Buffer.from(chunk)));
        incoming.once('end', () => resolve({ status: incoming.statusCode ?? 0, body: Buffer.concat(chunks).toString('utf8') }));
      });
      forwarded.setTimeout(10_000, () => forwarded.destroy(new Error('fixture admission forward timed out')));
      forwarded.once('error', reject);
      if (body) forwarded.write(body);
      forwarded.end();
    });
    expect(response.status).toBe(201);
    admittedId = JSON.parse(response.body).mutation_id;
    await route.abort('connectionreset');
  });
  const admissionFile = `admission-response-lost-${phase}.bin`;
  await chooseUpload({ name: admissionFile, mimeType: 'application/octet-stream', buffer: Buffer.from([4, 3, 2, 1]) });
  const admissionWarning = page.getByRole('alert').filter({ hasText: 'Upload status needs checking' });
  await expect(admissionWarning).toBeVisible();
  expect(admittedId).toMatch(/^[A-Za-z0-9-]+$/);
  await expect.poll(() => statusLookups.at(-1)).toBe(`/uploads/${admittedId}`);
  await expect(page.getByRole('button', { name: 'Check upload status' })).toBeVisible();
  await page.getByRole('button', { name: 'Check upload status' }).click();
  // Status shows the admission exists with no bytes; it is resumed from the original file, never re-admitted.
  await expect(page.getByRole('alert').filter({ hasText: 'Upload paused' })).toBeVisible();
  // The tab still holds the selected file, so it offers to resume without reselecting it.
  await expect(page.getByRole('alert').getByText('Resume this upload', { exact: false })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Resume upload', exact: true })).toBeVisible();
  await expect(page.locator('#upload-file')).toBeDisabled();
  expect(ticketRequests).toBe(1);
  expect(admissionRequests).toBe(1);
  await expect(page.locator('.file-row').filter({ hasText: admissionFile })).toHaveCount(0);
});

test('uploaded PDF renders and changes pages over real TLS', { annotation: { type: 'harness_case', description: 'storage_pdf_preview' } }, async () => {
  await signIn();
  const name = `preview-${phase}.pdf`;
  const bytes = fs.readFileSync(path.resolve(__dirname, '../../web/tests/fixtures/three-pages.pdf'));
  await chooseUpload({ name, mimeType: 'application/pdf', buffer: bytes });
  const trigger = page.getByRole('button', { name: `Preview ${name}`, exact: true });
  await expect(trigger).toBeVisible({ timeout: 15_000 });
  await trigger.click();
  const preview = page.getByRole('dialog', { name: `Preview ${name}`, exact: true });
  await expect(preview.locator('.page[data-page-number="1"] canvas').first()).toBeVisible({ timeout: 15_000 });
  await expect(preview.getByRole('spinbutton', { name: 'Page number' })).toHaveValue('1');
  await expect(preview.locator('.pdf-scroll-container')).toHaveAttribute('aria-busy', 'false');
  const firstPixels = await preview.locator('.page[data-page-number="1"] canvas').first().evaluate((canvas: HTMLCanvasElement) => canvas.toDataURL());
  await preview.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(preview.getByRole('spinbutton', { name: 'Page number' })).toHaveValue('2');
  await expect(preview.locator('.page[data-page-number="2"] canvas').first()).toBeVisible();
  await expect(preview.locator('.pdf-scroll-container')).toHaveAttribute('aria-busy', 'false');
  expect(await preview.locator('.page[data-page-number="2"] canvas').first().evaluate((canvas: HTMLCanvasElement) => canvas.toDataURL())).not.toBe(firstPixels);
  await preview.getByRole('button', { name: 'Close preview', exact: true }).click();
  await expect(preview).toHaveCount(0);
  await expect(trigger).toBeFocused();
});

test('direct shares show only the live subtree and reconcile a lost restore response', { annotation: { type: 'harness_case', description: 'storage_direct_sharing' } }, async () => {
  test.setTimeout(65_000);
  await signIn();
  const ownerHandle = `share-owner-${phase}`;
  const handle = `share-recipient-${phase}`;
  const sharingPassword = 'Direct sharing fixture password';
  const pdfName = `Shared plan ${phase}.pdf`;
  const pdfBytes = fs.readFileSync(path.resolve(__dirname, '../../web/tests/fixtures/three-pages.pdf'));
  const button = (tab: Page, name: string) => tab.getByRole('button', { name, exact: true });

  await button(page, 'Settings').click();
  for (const accountHandle of [ownerHandle, handle]) {
    await button(page, 'Create account').click();
    const account = page.getByRole('dialog', { name: 'Create account', exact: true });
    await account.getByLabel('Account name').fill(accountHandle);
    await account.getByLabel('Password', { exact: true }).fill(sharingPassword);
    await account.getByLabel('Confirm password', { exact: true }).fill(sharingPassword);
    await account.getByRole('button', { name: 'Create account', exact: true }).click();
    await expect(account).toBeHidden();
  }
  // The seeded administrator is reused after restart.
  await button(page, 'Log out').click();
  await page.getByLabel('Account name').fill(ownerHandle);
  await page.getByLabel('Password', { exact: true }).fill(sharingPassword);
  await button(page, 'Sign in').click();

  await button(page, 'My libraries').click();
  await button(page, 'New library').click();
  const libraryName = `Archive ${phase}`;
  await page.getByLabel('Library name').fill(libraryName);
  await button(page, 'Create library').click();
  await button(page, `Share ${libraryName}`).press('Enter');

  async function grant(name: string) {
    const dialog = page.getByRole('dialog', { name: `Share ${name}`, exact: true });
    await dialog.getByLabel('Account name').fill(handle);
    await dialog.getByRole('button', { name: 'Share', exact: true }).click();
    await expect(dialog.getByRole('status')).toHaveText(`Shared with ${handle}.`);
    await dialog.getByRole('button', { name: 'Close', exact: true }).last().click();
    await expect(dialog).toBeHidden();
  }

  await grant(libraryName);
  await button(page, 'Files').click();
  async function createFolder(name: string) {
    await button(page, 'New folder').click();
    await page.getByLabel('Folder name').fill(name);
    await button(page, 'Create folder').click();
    await expect(button(page, `Open ${name}`)).toBeVisible();
  }
  const privateName = `Private parent ${phase}`;
  const sharedName = `Shared project ${phase}`;
  await createFolder(privateName);
  await button(page, `Open ${privateName}`).click();
  const ownerParent = page.url();
  await createFolder(sharedName);
  await createFolder(`Private sibling ${phase}`);
  const options = button(page, `Options for ${sharedName}`);
  await options.click();
  await page.getByRole('menuitem', { name: 'Share', exact: true }).click();
  await grant(sharedName);
  await expect(options).toBeFocused();
  await button(page, `Open ${sharedName}`).click();
  const ownerFolder = page.url();
  await chooseUpload({ name: pdfName, mimeType: 'application/pdf', buffer: pdfBytes });
  await expect(button(page, `Preview ${pdfName}`)).toBeVisible();

  const recipient = await storageContext();
  try {
    const tab = await recipient.newPage();
    await tab.goto(origin);
    await tab.getByLabel('Account name').fill(handle);
    await tab.getByLabel('Password', { exact: true }).fill(sharingPassword);
    await button(tab, 'Sign in').click();
    await button(tab, 'Shared with me').press('Enter');
    const roots = tab.getByRole('list', { name: 'Folders shared with me' });
    await expect(roots.getByRole('link')).toHaveCount(2);
    await roots.getByRole('link', { name: new RegExp(sharedName) }).press('Enter');
    const folderBookmark = tab.url();
    await expect(tab.getByRole('heading', { name: sharedName, exact: true })).toBeVisible();
    await expect(button(tab, 'New folder')).toHaveCount(0);
    await expect(tab.getByText(privateName)).toHaveCount(0);
    for (const width of [1440, 390]) {
      await tab.setViewportSize({ width, height: 900 });
      for (const scheme of ['dark', 'light'] as const) {
        await tab.emulateMedia({ colorScheme: scheme });
        await expect(tab.locator('html')).toHaveCSS('color-scheme', scheme);
        await expect(tab.getByRole('link', { name: pdfName, exact: true })).toBeVisible();
        await expect(button(tab, `Download ${pdfName}`)).toBeVisible();
        expect(await tab.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
      }
    }
    await tab.setViewportSize({ width: 1440, height: 900 });
    await tab.getByRole('searchbox', { name: 'Search shared files' }).fill('Shared plan');
    await button(tab, 'Search').click();
    const matches = tab.getByRole('list', { name: 'Shared search results' });
    await matches.getByRole('link', { name: new RegExp(pdfName) }).press('Enter');
    const fileBookmark = tab.url();
    const preview = tab.getByRole('dialog', { name: `Preview ${pdfName}`, exact: true });
    await expect(preview.locator('.page[data-page-number="1"] canvas').first()).toBeVisible({ timeout: 15_000 });
    await expect(button(tab, 'Edit')).toHaveCount(0);
    await button(tab, 'Next page').click();
    await expect(preview.getByRole('spinbutton', { name: 'Page number' })).toHaveValue('2');
    const downloaded = tab.waitForEvent('download');
    await button(tab, 'Download file').click();
    const download = await downloaded;
    expect(download.suggestedFilename()).toBe(pdfName);
    expect(fs.readFileSync((await download.path())!)).toEqual(pdfBytes);
    await button(tab, 'Close preview').click();
    await expect(tab).toHaveURL(folderBookmark);

    await button(page, `Options for ${pdfName}`).click();
    await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
    await page.getByRole('dialog').getByRole('button', { name: 'Move to trash', exact: true }).click();
    await expect(button(page, `Preview ${pdfName}`)).toHaveCount(0);
    await tab.goto(fileBookmark);
    await expect(tab.getByRole('alert')).toHaveText('This share is unavailable.');
    await button(page, 'Trash').click();
    const restore = button(page, `Restore ${pdfName}`);
    await restore.click();
    const confirmation = page.getByRole('dialog', { name: `Restore ${pdfName}?`, exact: true });
    await expect(confirmation.getByText(handle, { exact: true })).toBeVisible();
    await expect(confirmation.getByRole('button', { name: 'Cancel' })).toBeFocused();
    await confirmation.getByRole('button', { name: 'Cancel' }).press('Enter');
    await expect(restore).toBeVisible();
    await expect(tab.getByRole('alert')).toHaveText('This share is unavailable.');

    let committed = 0;
    await page.route('**/trash/*/restore', async route => {
      if (!route.request().headers()['x-share-confirmation']) return route.continue();
      expect(await forwardFixtureMutation(route.request(), /^\/trash\/[a-f0-9]{32}\/restore$/)).toBe(204);
      committed++;
      await route.abort('connectionclosed');
    });
    await restore.click();
    await confirmation.getByRole('button', { name: 'Restore and share' }).click();
    await expect(page.getByRole('heading', { name: 'Restore unconfirmed' })).toBeVisible();
    await button(page, 'Check Files and Trash').click();
    await expect(page.getByRole('heading', { name: 'Restore unconfirmed' })).toHaveCount(0);
    expect(committed).toBe(1);
    await tab.reload();
    await tab.getByLabel('Account name').fill(handle);
    await tab.getByLabel('Password', { exact: true }).fill(sharingPassword);
    await button(tab, 'Sign in').click();
    await expect(preview.locator('.page[data-page-number="1"] canvas').first()).toBeVisible({ timeout: 15_000 });
    await page.goto(ownerParent);
    await expect(page).toHaveURL(ownerParent);
    await options.click();
    await page.getByRole('menuitem', { name: 'Share', exact: true }).click();
    await button(page, `Revoke access for ${handle}`).click();
    await expect(page.getByRole('dialog').getByRole('status')).toHaveText(`Revoked access for ${handle}.`);
    await button(tab, 'Close preview').click();
    await expect(tab.getByRole('alert')).toHaveText('This share is unavailable.');
    await tab.goto(fileBookmark);
    await expect(tab.getByRole('alert')).toHaveText('This share is unavailable.');
    expect(ownerFolder).not.toBe(folderBookmark);
  } finally {
    await recipient.close();
  }
});

test('owners publish and withdraw a library and readers follow its live contents', { annotation: { type: 'harness_case', description: 'storage_library_publication' } }, async () => {
  test.setTimeout(65_000);
  await signIn();
  const owner = `publication-owner-${phase}`;
  const member = `publication-reader-${phase}`;
  const memberPassword = 'Disposable publication reader password';
  const library = `Handbook ${phase}`;
  const pdfName = `Team handbook ${phase}.pdf`;
  const privateName = `Private handbook notes ${phase}`;
  const pdfBytes = fs.readFileSync(path.resolve(__dirname, '../../web/tests/fixtures/three-pages.pdf'));
  const button = (tab: Page, name: string) => tab.getByRole('button', { name, exact: true });

  await button(page, 'Settings').click();
  for (const handle of [owner, member]) {
    await button(page, 'Create account').click();
    const dialog = page.getByRole('dialog', { name: 'Create account', exact: true });
    await dialog.getByLabel('Account name').fill(handle);
    await dialog.getByLabel('Password', { exact: true }).fill(memberPassword);
    await dialog.getByLabel('Confirm password').fill(memberPassword);
    await dialog.getByRole('button', { name: 'Create account', exact: true }).press('Enter');
    await expect(dialog).toBeHidden();
  }
  await button(page, 'Log out').click();
  await page.getByLabel('Account name').fill(owner);
  await page.getByLabel('Password', { exact: true }).fill(memberPassword);
  await button(page, 'Sign in').click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  await button(page, 'New folder').click();
  await page.getByLabel('Folder name').fill(privateName);
  await button(page, 'Create folder').press('Enter');
  await button(page, `Open ${privateName}`).press('Enter');
  await chooseUpload({ name: pdfName, mimeType: 'application/pdf', buffer: pdfBytes });
  await expect(button(page, `Preview ${pdfName}`)).toBeVisible();
  const sourceUrl = page.url();

  await button(page, 'My libraries').click();
  await button(page, 'New library').click();
  await page.getByLabel('Library name').fill(library);
  await button(page, 'Create library').press('Enter');
  await expect(page.getByRole('dialog', { name: 'New library' })).toBeHidden();
  const ownerShare = button(page, `Share ${library}`);
  await ownerShare.press('Enter');
  const share = page.getByRole('dialog', { name: new RegExp(`^(Share|Publish|Withdraw publication of) ${library}`) });
  const publish = button(page, 'Publish library…');
  await publish.press('Enter');
  await expect(share.getByRole('heading')).toHaveText(`Publish ${library}?`);
  await expect(share.getByText('Everyone signed in', { exact: false })).toBeVisible();
  await expect(share.getByRole('button', { name: 'Cancel', exact: true })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(publish).toBeFocused();
  await expect(share.getByText('This library is unpublished.')).toBeVisible();
  await publish.press('Enter');
  await button(page, 'Publish library').press('Enter');
  await expect(share.getByRole('status')).toHaveText('Library published.');
  await share.getByRole('button', { name: 'Close', exact: true }).last().click();
  await expect(ownerShare).toBeFocused();
  const ownerLibrary = page.getByRole('list', { name: 'My libraries', exact: true }).getByRole('link', { name: library, exact: true });
  await ownerLibrary.press('Enter');
  await expect(page.getByRole('heading', { name: library })).toBeVisible();
  const destinationUrl = page.url();

  const recipient = await storageContext();
  try {
    const tab = await recipient.newPage();
    await tab.goto(origin);
    await tab.getByLabel('Account name').fill(member);
    await tab.getByLabel('Password', { exact: true }).fill(memberPassword);
    await button(tab, 'Sign in').click();
    await button(tab, 'Shared with all').press('Enter');
    const shared = tab.getByRole('list', { name: 'Libraries shared with all' });
    await shared.getByRole('link', { name: new RegExp(library) }).press('Enter');
    await expect(tab.getByRole('heading', { name: library })).toBeVisible();
    const folderBookmark = tab.url();
    await expect(tab.getByText('This folder is empty.')).toBeVisible();
    await expect(button(tab, 'New folder')).toHaveCount(0);
    await expect(button(tab, `Share ${library}`)).toHaveCount(0);

    await page.goto(sourceUrl);
    const move = async () => {
      await button(page, `Options for ${pdfName}`).click();
      await page.getByRole('menuitem', { name: 'Move', exact: true }).click();
      const picker = page.getByRole('dialog', { name: `Move ${pdfName}`, exact: true });
      await picker.getByRole('button', { name: 'Choose a library', exact: true }).click();
      await picker.getByRole('group', { name: 'Destination libraries' }).getByRole('button', { name: library }).click();
      const moveHere = picker.getByRole('button', { name: 'Move here', exact: true });
      await expect(moveHere).toBeEnabled();
      await moveHere.press('Enter');
    };
    await move();
    const confirmation = page.getByRole('dialog', { name: `Move ${pdfName}?` });
    await expect(confirmation.getByText('Everyone signed in', { exact: false })).toBeVisible();
    const cancel = confirmation.getByRole('button', { name: 'Cancel', exact: true });
    await expect(cancel).toBeFocused();
    await cancel.press('Enter');
    await expect(confirmation).toBeHidden();
    await button(tab, 'Refresh shared files').press('Enter');
    await expect(tab.getByText('This folder is empty.')).toBeVisible();
    const picker = page.getByRole('dialog', { name: `Move ${pdfName}`, exact: true });
    const moveHere = picker.getByRole('button', { name: 'Move here', exact: true });
    await expect(moveHere).toBeEnabled();
    await moveHere.press('Enter');
    await confirmation.getByRole('button', { name: 'Move and share' }).press('Enter');
    await expect(button(page, `Preview ${pdfName}`)).toHaveCount(0);

    await button(tab, 'Refresh shared files').press('Enter');
    for (const width of [1280, 390]) {
      await tab.setViewportSize({ width, height: 850 });
      for (const scheme of ['light', 'dark'] as const) {
        await tab.emulateMedia({ colorScheme: scheme });
        await expect(tab.locator('html')).toHaveCSS('color-scheme', scheme);
        await expect(tab.getByRole('link', { name: pdfName, exact: true })).toBeVisible();
        expect(await tab.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
      }
    }
    await tab.setViewportSize({ width: 1280, height: 850 });
    await tab.getByRole('searchbox', { name: 'Search shared files' }).fill('Team handbook');
    await button(tab, 'Search').press('Enter');
    const results = tab.getByRole('list', { name: 'Shared search results' });
    await results.getByRole('link', { name: new RegExp(pdfName) }).press('Enter');
    const fileBookmark = tab.url();
    const preview = tab.getByRole('dialog', { name: `Preview ${pdfName}` });
    await expect(preview.locator('.page[data-page-number="1"] canvas').first()).toBeVisible({ timeout: 15_000 });
    const download = tab.waitForEvent('download');
    await button(tab, 'Download file').press('Enter');
    expect(fs.readFileSync((await (await download).path())!)).toEqual(pdfBytes);
    await button(tab, 'Close preview').press('Enter');
    await expect(tab).toHaveURL(folderBookmark);

    await page.goto(destinationUrl);
    await button(page, `Options for ${pdfName}`).click();
    await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
    await button(page, 'Move to trash').press('Enter');
    await expect(button(page, `Preview ${pdfName}`)).toHaveCount(0);
    await button(page, 'Trash').click();
    const restore = button(page, `Restore ${pdfName}`);
    await restore.press('Enter');
    const restoring = page.getByRole('dialog', { name: `Restore ${pdfName}?` });
    await expect(restoring.getByText('Everyone signed in', { exact: false })).toBeVisible();
    await restoring.getByRole('button', { name: 'Cancel', exact: true }).press('Enter');
    await expect(restore).toBeVisible();
    await tab.goto(fileBookmark);
    await expect(tab.getByRole('alert')).toHaveText('This share is unavailable.');
    await restore.press('Enter');
    await restoring.getByRole('button', { name: 'Restore and share' }).press('Enter');
    await expect(restore).toHaveCount(0);
    await button(tab, 'Refresh shared files').press('Enter');
    await expect(preview.locator('.page[data-page-number="1"] canvas').first()).toBeVisible({ timeout: 15_000 });

    await button(page, 'My libraries').press('Enter');
    await ownerShare.press('Enter');
    await button(page, 'Withdraw publication…').press('Enter');
    await expect(share.getByText('Any separate direct shares', { exact: false })).toBeVisible();
    await button(page, 'Withdraw publication').press('Enter');
    await expect(share.getByRole('status')).toHaveText('Publication withdrawn.');
    await tab.goto(fileBookmark);
    await expect(tab.getByRole('alert')).toHaveText('This share is unavailable.');
    await tab.goto(folderBookmark);
    await expect(tab.getByRole('alert')).toHaveText('This share is unavailable.');
  } finally {
    await recipient.close();
  }
});

test('reload resumes real private chunks after matching file reselect', { annotation: { type: 'harness_case', description: 'storage_upload_resume' } }, async () => {
  test.setTimeout(60_000);
  await signIn();
  const name = `resume-${phase}.bin`;
  const original = patterned(16 * 1024 * 1024 + 4096);
  let paused = true;
  const offsets: number[] = [];
  await page.route('**/uploads/*/content', async route => {
    const offset = Number(route.request().headers()['upload-offset']);
    offsets.push(offset);
    if (paused && offset >= 8 * 1024 * 1024) await route.abort('connectionreset');
    else await route.continue();
  });
  await chooseUpload({ name, mimeType: 'application/octet-stream', buffer: original });
  await expect.poll(async () => {
    const id = await page.evaluate(() => {
      const key = Object.keys(sessionStorage).find(key => key.startsWith('sylux-upload-v1:'));
      return key ? JSON.parse(sessionStorage.getItem(key)!).id : null;
    });
    if (!id) return 0;
    return page.evaluate(async (id) => (await (await fetch(`/uploads/${id}`)).json()).bytes_received, id);
  }).toBe(8 * 1024 * 1024);
  await page.reload();
  await signIn();
  await page.evaluate(() => {
    Object.defineProperty(window, '__s1MaxBlobRead', { configurable: true, writable: true, value: 0 });
    const original = Blob.prototype.arrayBuffer;
    Blob.prototype.arrayBuffer = function() {
      const state = window as unknown as { __s1MaxBlobRead: number };
      state.__s1MaxBlobRead = Math.max(state.__s1MaxBlobRead, this.size);
      return original.call(this);
    };
  });
  await page.getByRole('button', { name: 'Check upload status', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Select original file', exact: true })).toBeVisible();
  const changed = Buffer.from(original); changed[9 * 1024 * 1024] ^= 0xff;
  const reordered = Buffer.concat([original.subarray(8 * 1024 * 1024, 16 * 1024 * 1024), original.subarray(0, 8 * 1024 * 1024), original.subarray(16 * 1024 * 1024)]);
  for (const wrong of [changed, reordered]) {
    await page.getByLabel('Original upload file', { exact: true }).setInputFiles({ name, mimeType: 'application/octet-stream', buffer: wrong });
    await expect(page.getByText('This file does not match the original upload.', { exact: false })).toBeVisible();
  }
  paused = false;
  await page.getByLabel('Original upload file', { exact: true }).setInputFiles({ name, mimeType: 'application/octet-stream', buffer: original });
  const row = page.getByRole('row', { name, exact: true });
  await expect(row).toBeVisible({ timeout: 20_000 });
  expect(offsets.filter(offset => offset === 0)).toHaveLength(1);
  // Reselect hashing and resumed chunks read the 16 MiB file in bounded slices.
  const maxRead = await page.evaluate(() => (window as unknown as { __s1MaxBlobRead: number }).__s1MaxBlobRead);
  expect(maxRead).toBeGreaterThan(0);
  expect(maxRead).toBeLessThanOrEqual(8 * 1024 * 1024);
  // The list API resolves the final entry; the browser remains on the same authenticated origin.
  const listing = await page.evaluate(async () => await (await fetch('/folders/children')).json());
  const entry = listing.items.find((item: { name: string }) => item.name === name);
  expect(entry).toBeTruthy();
  const download = await page.evaluate(async (id) => {
    const response = await fetch(`/entries/${id}/content`);
    const bytes = await response.arrayBuffer();
    const digest = await crypto.subtle.digest('SHA-256', bytes);
    return { status: response.status, length: bytes.byteLength, sha256: [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, '0')).join('') };
  }, entry.id);
  expect(download.status).toBe(200); expect(download.length).toBe(original.length);
  expect(download.sha256).toBe(createHash('sha256').update(original).digest('hex'));
});

test('owner shares and revokes a file downloaded in a separate signed-out TLS context', {
  annotation: { type: 'harness_case', description: 'storage_public_share' },
}, async () => {
  await signIn();
  await page.getByRole('button', { name: `Open ${fixtures.folder}`, exact: true }).click();
  await page.getByRole('button', { name: `Open ${fixtures.nested}`, exact: true }).click();
  const options = page.getByRole('button', { name: `Options for ${fixtures.file}` });
  await options.focus();
  await page.keyboard.press('Enter');
  await page.getByRole('menuitem', { name: 'Share', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: `Share ${fixtures.file}` });
  await expect(dialog).toContainText('Anyone with a link can download the file');
  await expect(dialog.getByLabel('Expires after')).toHaveValue('7');
  await dialog.getByLabel('Expires after').selectOption('1');
  await dialog.getByRole('button', { name: 'Create link', exact: true }).click();
  const field = dialog.getByRole('textbox', { name: 'Download link' });
  await expect(field).toBeFocused();
  const share = new URL(await field.inputValue());
  expect(share.origin).toBe(origin);
  expect(/^\/share\/[A-Za-z0-9_-]{43}$/.test(share.pathname)).toBeTruthy();

  const signedOut = await browser.newContext({ ignoreHTTPSErrors: false, acceptDownloads: true });
  try {
    const recipient = await signedOut.newPage();
    await recipient.goto(origin);
    await expect(recipient.getByRole('button', { name: 'Sign in' })).toBeVisible();
    const downloadEvent = recipient.waitForEvent('download');
    await recipient.evaluate(url => {
      const link = document.createElement('a');
      link.href = url;
      link.referrerPolicy = 'no-referrer';
      link.click();
    }, share.href);
    const download = await downloadEvent;
    expect(download.suggestedFilename()).toBe(fixtures.file);
    const downloaded = fs.readFileSync((await download.path())!);
    expect(downloaded.length).toBe(fixtures.length);
    expect(createHash('sha256').update(downloaded).digest('hex')).toBe(fixtures.sha256);

    await dialog.getByRole('button', { name: 'Done' }).click();
    await expect(options).toBeFocused();
    await page.getByRole('button', { name: 'Account', exact: true }).click();
    await page.getByRole('button', { name: 'Shared links' }).click();
    const all = page.getByRole('dialog', { name: 'Shared links' });
    await expect(all.getByRole('textbox', { name: 'Download link' })).toHaveCount(0);
    await all.getByRole('button', { name: new RegExp(`Revoke link .* for ${fixtures.file}`) }).click();
    await expect(all.getByRole('status')).toHaveText('Link revoked.');
    const missing = await recipient.evaluate(async url => {
      const response = await fetch(url, { cache: 'no-store', credentials: 'omit' });
      return [response.status, await response.json()];
    }, share.href);
    expect(missing).toEqual([404, { code: 'share_not_found' }]);
  } finally {
    await signedOut.close();
  }
});

test('public folder link browses live content in a signed-out TLS context', {
  annotation: { type: 'harness_case', description: 'storage_public_folder' },
}, async () => {
  await signIn();
  const folder = `Client delivery ${phase}`;
  const nested = 'Ready to download';
  const filename = `draft-${phase}.txt`;
  const moveName = `moved-${phase}.txt`;
  const addition = `added-${phase}.txt`;
  const original = Buffer.from('Original shared bytes');
  const replacement = Buffer.from('Replacement shared bytes');
  const button = (tab: Pick<Page, 'getByRole'>, name: string) => tab.getByRole('button', { name, exact: true });
  const options = async (name: string, action: string) => {
    await button(page, `Options for ${name}`).press('Enter');
    await page.getByRole('menuitem', { name: action, exact: true }).click();
  };
  const createFolder = async (name: string) => {
    await button(page, 'New folder').click();
    const form = page.getByRole('dialog', { name: 'Create folder' });
    await form.getByLabel('Folder name').fill(name);
    await button(form, 'Create folder').click();
    await expect(form).toBeHidden();
    await expect(button(page, `Open ${name}`)).toBeVisible();
  };
  const openFolder = async (name: string) => {
    await button(page, `Open ${name}`).click();
    await expect(page.getByRole('heading', { name, level: 1, exact: true })).toBeVisible();
  };
  const confirmUpload = async (name: string, bytes: Buffer, confirm?: boolean) => {
    await page.locator('#upload-file').setInputFiles({ name, mimeType: 'text/plain', buffer: bytes });
    const warning = page.getByRole('dialog', { name: `Upload to ${nested}?` });
    await expect(warning).toContainText('Anyone with a public link');
    await expect(button(warning, 'Cancel')).toBeFocused();
    await button(warning, confirm ? 'Upload' : 'Cancel').press('Enter');
    await expect(warning).toBeHidden();
  };

  await chooseUpload({ name: moveName, mimeType: 'text/plain', buffer: original });
  await expect(button(page, `Preview ${moveName}`)).toBeVisible();
  await createFolder(folder);
  await openFolder(folder);
  await createFolder(nested);
  await openFolder(nested);
  await chooseUpload({ name: filename, mimeType: 'text/plain', buffer: original });
  await expect(button(page, `Preview ${filename}`)).toBeVisible();
  await button(page.getByRole('navigation', { name: 'Files' }), 'Files').click();
  await options(folder, 'Public link');
  const dialog = page.getByRole('dialog', { name: `Share ${folder}` });
  await expect(dialog).toContainText('Files you add, restore or replace become available');
  await expect(dialog.getByLabel('Expires after')).toHaveValue('7');
  await button(dialog, 'Create link').click();
  const field = dialog.getByRole('textbox', { name: 'Folder link' });
  await expect(field).toBeFocused();
  const url = await field.inputValue();
  await button(dialog, 'Done').click();
  await expect(button(page, `Options for ${folder}`)).toBeFocused();

  const signedOut = await storageContext();
  try {
    const tab = await signedOut.newPage();
    const listing = await tab.goto(url);
    expect(listing?.status()).toBe(200);
    expect((await listing?.allHeaders())?.['referrer-policy']).toBe('no-referrer');
    expect(await signedOut.cookies()).toHaveLength(0);
    await expect(tab.getByRole('heading', { level: 1 })).toHaveText(folder);
    const style = tab.locator('link[rel="stylesheet"]');
    expect(await style.getAttribute('href')).toBe('/public-folder.css');
    await tab.getByRole('link', { name: `${nested} Open folder` }).press('Enter');
    await expect(tab.getByRole('heading', { level: 1 })).toHaveText(nested);
    const probe = await signedOut.newPage();
    const freshStatus = async (saved: string) => {
      const response = await probe.goto(new URL(saved, url).href);
      return [response?.status(), await response?.json()];
    };
    const missing = [404, { code: 'share_not_found' }];
    const breadcrumb = tab.getByRole('navigation', { name: 'Folder path' });
    await expect(breadcrumb.getByRole('link')).toHaveText([folder]);
    await expect(tab.getByRole('searchbox')).toHaveCount(0);

    const entry = (name: string) => tab.getByRole('link', { name: `${name} Download`, exact: true });
    const download = async (file: string, bytes: Buffer) => {
      const response = tab.waitForEvent('download');
      await entry(file).press('Enter');
      const saved = await response;
      expect(saved.suggestedFilename()).toBe(file);
      expect(fs.readFileSync((await saved.path())!)).toEqual(bytes);
    };
    for (const [width, colorScheme] of [[1280, 'light'], [390, 'dark']] as const) {
      await tab.setViewportSize({ width, height: 850 });
      await tab.emulateMedia({ colorScheme });
      await expect(entry(filename)).toBeVisible();
      expect(await tab.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
      expect(await tab.locator('body').evaluate(body => getComputedStyle(body).backgroundColor)).not.toBe('rgba(0, 0, 0, 0)');
    }
    const oldUrl = (await entry(filename).getAttribute('href'))!;
    await download(filename, original);

    await options(moveName, 'Move');
    const picker = page.getByRole('dialog', { name: `Move ${moveName}`, exact: true });
    await picker.getByRole('button', { name: folder, exact: true }).click();
    await picker.getByRole('button', { name: nested, exact: true }).click();
    const moveHere = button(picker, 'Move here');
    await expect(moveHere).toBeEnabled();
    await moveHere.click();
    const moveWarning = page.getByRole('dialog', { name: `Move ${moveName}?` });
    await expect(moveWarning).toContainText('Anyone with a public link');
    await button(moveWarning, 'Cancel').press('Enter');
    await tab.reload();
    await expect(entry(moveName)).toHaveCount(0);
    await expect(moveHere).toBeEnabled();
    await moveHere.click();
    await button(moveWarning, 'Move and share').press('Enter');
    await expect(picker).toBeHidden();
    await tab.reload();
    await download(moveName, original);

    await openFolder(folder);
    await openFolder(nested);
    await confirmUpload(addition, original);
    await expect(button(page, 'Choose files')).toBeEnabled();
    await tab.reload();
    await expect(entry(addition)).toHaveCount(0);
    await confirmUpload(addition, original, true);
    await expect(button(page, `Preview ${addition}`)).toBeVisible();
    await tab.reload();
    await download(addition, original);

    await confirmUpload(filename, replacement, true);
    const replace = page.getByRole('dialog', { name: `Replace ${filename}?` });
    await expect(replace).toContainText('The replacement file will be available');
    await button(replace, 'Replace file').press('Enter');
    await expect(button(page, 'Choose files')).toBeEnabled();
    await tab.reload();
    await download(filename, replacement);
    expect(await freshStatus(oldUrl)).toEqual(missing);

    const downloadUrl = (await entry(addition).getAttribute('href'))!;
    await options(addition, 'Delete');
    await button(page, 'Move to trash').press('Enter');
    await expect(button(page, `Preview ${addition}`)).toHaveCount(0);
    expect(await freshStatus(downloadUrl)).toEqual(missing);
    await button(page, 'Trash').click();
    const restore = button(page, `Restore ${addition}`);
    await restore.press('Enter');
    const restoring = page.getByRole('dialog', { name: `Restore ${addition}?` });
    await expect(restoring).toContainText('Anyone with a public link');
    await button(restoring, 'Cancel').press('Enter');
    await expect(restore).toBeVisible();
    expect(await freshStatus(downloadUrl)).toEqual(missing);
    await restore.press('Enter');
    await button(restoring, 'Restore and share').press('Enter');
    await expect(restore).toHaveCount(0);
    await tab.reload();
    await download(addition, original);

    await button(page, 'Account').click();
    await button(page, 'Shared links').click();
    const all = page.getByRole('dialog', { name: 'Shared links' });
    await all.getByRole('button', { name: new RegExp(`Revoke link .* for ${folder}`) }).click();
    await expect(all.getByRole('status')).toHaveText('Link revoked.');
    for (const saved of [url, downloadUrl]) {
      expect(await freshStatus(saved)).toEqual(missing);
    }
  } finally {
    await signedOut.close();
  }
});

async function openTextEditor(tab: Page, name: string) {
  await tab.getByRole('button', { name: `Preview ${name}`, exact: true }).click();
  await tab.getByRole('dialog', { name: `Preview ${name}` }).getByRole('button', { name: 'Edit', exact: true }).click();
  const dialog = tab.getByRole('dialog', { name: `Edit ${name}` });
  await expect(dialog.getByRole('textbox', { name: 'File content' })).toBeFocused();
  return dialog;
}

test('text previews save and download exact Markdown and plain text bytes', {
  annotation: { type: 'harness_case', description: 'storage_text_edit_roundtrip' },
}, async () => {
  await signIn();
  const cases = [
    { name: `notes-${phase}.md`, before: '# Café\n\n  old\n', after: '# Café\n\n  new 雪\n', width: 1280 },
    { name: `notes-${phase}.txt`, before: '\uFEFFFirst\r\nSecond\r\n', after: '\uFEFFFirst\r\nSecond 🧪\r\n', width: 390 },
  ];
  for (const { name, before, after, width } of cases) {
    await page.setViewportSize({ width, height: 844 });
    const theme = width === 390 ? 'dark' : 'light';
    const toggle = page.getByRole('button', { name: `Switch to ${theme} mode`, exact: true });
    if (await toggle.isVisible()) await toggle.click();
    await chooseUpload({ name, mimeType: 'application/octet-stream', buffer: Buffer.from(before) });
    const dialog = await openTextEditor(page, name);
    const input = dialog.getByRole('textbox');
    const original = before.replace(/^\uFEFF/, '').replaceAll('\r\n', '\n');
    const changed = after.replace(/^\uFEFF/, '').replaceAll('\r\n', '\n');
    await expect(input).toHaveValue(original);
    await input.fill('discard me');
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Edit', exact: true })).toBeFocused();
    await expect(page.getByRole('dialog').locator('pre')).toHaveText(original.trim());
    await page.getByRole('button', { name: 'Edit', exact: true }).press('Enter');
    await expect(input).toHaveValue(original);
    await input.fill(changed);
    await expect(dialog.getByRole('button', { name: 'Save', exact: true })).toBeInViewport();
    await input.press('Control+s');
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(page.getByRole('status').filter({ hasText: 'Changes saved.' })).toBeVisible();
    await page.getByRole('button', { name: `Preview ${name}`, exact: true }).click();
    const reopened = page.getByRole('dialog', { name: `Preview ${name}` });
    await expect(reopened.locator('pre')).toHaveText(changed.trim());
    const requested = page.waitForEvent('download');
    await reopened.getByRole('button', { name: 'Download file' }).click();
    const download = await requested;
    expect(download.suggestedFilename()).toBe(name);
    expect(fs.readFileSync((await download.path())!)).toEqual(Buffer.from(after));
    await reopened.getByRole('button', { name: 'Close preview' }).click();
    await page.getByRole('button', { name: 'Refresh folder' }).click();
    await expect(page.getByRole('status').filter({ hasText: 'Changes saved.' })).toHaveCount(0);
  }
});

test('a stale second tab keeps its draft after the real commit rejects it', {
  annotation: { type: 'harness_case', description: 'storage_text_edit_conflict' },
}, async () => {
  await signIn();
  const name = `edit-conflict-${phase}.txt`;
  await chooseUpload({ name, mimeType: 'text/plain', buffer: Buffer.from('Original\r\n') });
  const local = await openTextEditor(page, name);
  await local.getByRole('textbox').fill('My draft\n');
  const secondContext = await storageContext();
  const other = await secondContext.newPage();
  let release!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; });
  try {
    await signIn(other);
    const remote = await openTextEditor(other, name);
    await remote.getByRole('textbox').fill('Saved in another tab\n');
    let commits = 0;
    await page.route('**/uploads/*/commit', async route => {
      commits++;
      await held;
      await route.continue();
    });
    const attempted = page.waitForRequest(request => new URL(request.url()).pathname.endsWith('/commit'), { timeout: 8000 });
    await local.getByRole('button', { name: 'Save', exact: true }).click();
    await attempted;
    const busy = local.getByRole('button', { name: 'Saving…' });
    await expect(busy).toHaveAttribute('aria-busy', 'true');
    await busy.press('Enter');
    await local.getByRole('textbox').press('Control+s');
    expect(commits).toBe(1);
    await remote.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(other.getByRole('status').filter({ hasText: 'Changes saved.' })).toBeVisible();
    const rejected = page.waitForResponse(response => new URL(response.url()).pathname.endsWith('/commit'), { timeout: 8000 });
    release();
    expect((await rejected).status()).toBe(412);
    await expect(local.getByRole('alert')).toContainText('This file or folder changed');
    await expect(local.getByRole('textbox')).toHaveValue('My draft\n');
    await expect(local.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
    const draft = page.waitForEvent('download');
    await local.getByRole('button', { name: 'Download draft' }).click();
    expect(fs.readFileSync((await (await draft).path())!)).toEqual(Buffer.from('My draft\r\n'));
    await other.getByRole('button', { name: `Preview ${name}`, exact: true }).click();
    await expect(other.getByRole('dialog').locator('pre')).toHaveText('Saved in another tab');
  } finally {
    release();
    await secondContext.close();
  }
});

test('navigation keeps the draft and retry finishes the same staged edit', {
  annotation: { type: 'harness_case', description: 'storage_text_edit_retry' },
}, async () => {
  await signIn();
  const name = `edit-retry-${phase}.md`;
  await chooseUpload({ name, mimeType: 'text/markdown', buffer: Buffer.from('# Before\n') });
  const editor = await openTextEditor(page, name);
  await editor.getByRole('textbox').fill('# After\n');
  await editor.getByRole('textbox').press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.getByRole('navigation', { name: 'Files', exact: true }).getByRole('button', { name: 'Trash' }).click();
  await expect(page.getByRole('heading', { name: 'Trash', exact: true })).toBeVisible();
  await page.goBack();
  await expect(page.getByRole('heading', { name: 'Files', exact: true, level: 1 })).toBeVisible();
  await page.getByRole('button', { name: 'Resume edit' }).click();
  await expect(editor.getByRole('textbox')).toHaveValue('# After\n');

  const commits: string[] = [];
  await page.route('**/uploads/*/commit', route => {
    commits.push(new URL(route.request().url()).pathname);
    return commits.length === 1 ? route.abort('failed') : route.continue();
  });
  await editor.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(editor.getByRole('alert')).toContainText('Could not confirm the save');
  await expect(editor.getByRole('textbox')).toHaveValue('# After\n');
  await editor.getByRole('button', { name: 'Close preview' }).click();
  await page.getByRole('button', { name: 'Resume edit' }).click();
  await expect(editor.getByRole('textbox')).toHaveValue('# After\n');
  await editor.getByRole('button', { name: 'Retry save' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Changes saved.' })).toBeVisible();
  expect(commits).toHaveLength(2);
  expect(commits[0]).toBe(commits[1]);
  await page.getByRole('button', { name: `Preview ${name}`, exact: true }).click();
  await expect(page.getByRole('dialog').locator('pre')).toHaveText('# After');
  await page.getByRole('button', { name: 'Close preview' }).click();

  await page.unrouteAll({ behavior: 'wait' });
  const recovered = await openTextEditor(page, name);
  await recovered.getByRole('textbox').fill('# Confirmed by status\n');
  let lostReplies = 0;
  await page.route('**/uploads/*/commit', async route => {
    expect(await forwardFixtureMutation(route.request(), /^\/uploads\/[A-Za-z0-9-]+\/commit$/)).toBe(201);
    lostReplies++;
    await route.abort('failed');
  });
  await recovered.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('status').filter({ hasText: 'Changes saved.' })).toBeVisible();
  expect(lostReplies).toBe(1);
  await page.getByRole('button', { name: `Preview ${name}`, exact: true }).click();
  await expect(page.getByRole('dialog').locator('pre')).toHaveText('# Confirmed by status');
});

test('WebDAV changes appear in Files and Trash and the app password can be revoked', { annotation: { type: 'harness_case', description: 'storage_webdav_changes' } }, async () => {
  await signIn();
  await page.getByRole('button', { name: 'Account', exact: true }).press('Enter');
  const manage = page.getByRole('button', { name: 'Manage app passwords', exact: true });
  await manage.press('Enter');
  const dialog = page.getByRole('dialog', { name: 'WebDAV app passwords' });
  const name = dialog.getByRole('textbox', { name: 'App name' });
  await name.fill('Desktop fixture');
  await dialog.getByRole('button', { name: 'Create app password' }).press('Enter');
  const passwordField = dialog.getByRole('textbox', { name: 'App password' });
  await expect(passwordField).toBeFocused();
  const appPassword = await passwordField.inputValue();
  if (!appPassword) throw new Error('Missing disposable app password');
  await dialog.getByRole('button', { name: 'Done' }).press('Enter');
  await expect(passwordField).toHaveCount(0);
  await expect(name).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(manage).toBeFocused();

  async function dav(method: string, target: string, body = Buffer.alloc(0), headers: Record<string, string> = {}) {
    const fixtureOrigin = new URL(origin);
    return new Promise<{ status: number; bytes: Buffer }>((resolve, reject) => {
      const request = https.request({
        hostname: '127.0.0.1',
        servername: fixtureOrigin.hostname,
        port: Number(fixtureOrigin.port),
        path: target,
        method,
        ca: fs.readFileSync(process.env.S1_CA_CERT!),
        headers: {
          authorization: `Basic ${Buffer.from(`${fixtures.handle}:${appPassword}`).toString('base64')}`,
          'content-length': String(body.length),
          ...headers,
        },
      }, response => {
        const parts: Buffer[] = [];
        response.on('data', part => parts.push(part));
        response.once('error', reject);
        response.once('end', () => resolve({ status: response.statusCode ?? 0, bytes: Buffer.concat(parts) }));
      });
      request.setTimeout(10_000, () => request.destroy(new Error('fixture DAV request timed out')));
      request.once('error', reject);
      request.end(body);
    });
  }

  await page.getByRole('button', { name: 'Files', exact: true }).press('Enter');
  const upload = `dav-new-${phase}.txt`;
  const renamed = `dav-moved-${phase}.txt`;
  const folder = `DAV delivery ${phase}`;
  const collection = `/dav/Files/${encodeURIComponent(folder)}/`;
  const destination = collection + renamed;
  const preview = (file: string) => page.getByRole('button', { name: `Preview ${file}`, exact: true });
  const bytes = Buffer.from('WebDAV fixture bytes\n');
  await expect(preview(upload)).toHaveCount(0);
  expect((await dav('PUT', `/dav/Files/${upload}`, bytes)).status).toBe(201);
  await page.getByRole('button', { name: 'Refresh folder' }).press('Enter');
  await expect(preview(upload)).toBeVisible();

  expect((await dav('MKCOL', collection)).status).toBe(201);
  expect((await dav('MOVE', `/dav/Files/${upload}`, Buffer.alloc(0), { Destination: origin + destination })).status).toBe(201);
  await page.getByRole('button', { name: 'Refresh folder' }).press('Enter');
  await expect(preview(upload)).toHaveCount(0);
  await page.getByRole('button', { name: `Open ${folder}`, exact: true }).press('Enter');
  await expect(preview(renamed)).toBeVisible();
  const downloaded = await dav('GET', destination);
  expect(downloaded.status).toBe(200);
  expect(createHash('sha256').update(downloaded.bytes).digest('hex'))
    .toBe(createHash('sha256').update(bytes).digest('hex'));

  expect((await dav('DELETE', destination)).status).toBe(204);
  await page.getByRole('button', { name: 'Refresh folder' }).press('Enter');
  await expect(preview(renamed)).toHaveCount(0);
  await page.getByRole('button', { name: 'Trash', exact: true }).press('Enter');
  await expect(page.getByText(renamed, { exact: true })).toBeVisible();
  // Restart reuses this root, where the keyboard test expects the seeded folder first.
  expect((await dav('DELETE', collection)).status).toBe(204);

  await page.getByRole('button', { name: 'Account', exact: true }).press('Enter');
  await page.getByRole('button', { name: 'Switch to dark mode' }).press('Enter');
  await page.setViewportSize({ width: 390, height: 844 });
  await manage.press('Enter');
  await expect(dialog).toBeInViewport();
  await dialog.getByRole('button', { name: 'Revoke Desktop fixture', exact: true }).press('Enter');
  await expect(dialog.getByRole('status')).toHaveText('App password revoked.');
  await expect(name).toBeFocused();
  expect((await dav('PROPFIND', '/dav/', Buffer.alloc(0), { Depth: '1' })).status).toBe(401);
  await page.keyboard.press('Escape');
  await expect(manage).toBeFocused();
});
