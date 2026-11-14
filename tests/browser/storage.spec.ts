import fs from 'node:fs';
import { execFileSync } from 'node:child_process';
import https from 'node:https';
import { createRequire } from 'node:module';
import path from 'node:path';
import type { Browser, BrowserContext, Page } from '../../web/node_modules/playwright-core';

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

async function signIn(): Promise<void> {
  await page.goto(origin);
  await page.getByLabel('Account name').fill(fixtures.handle);
  await page.getByLabel('Password').fill(password);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
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
test.beforeEach(async () => {
  context = await browser.newContext({ ignoreHTTPSErrors: false, acceptDownloads: true, viewport: { width: 1440, height: 900 } });
  await context.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (url.hostname === 'fixture.test' && url.port === new URL(origin).port && url.protocol === 'https:') await route.continue();
    else await route.abort('blockedbyclient');
  });
  page = await context.newPage();
});
test.afterEach(async () => { await context?.close(); });
test.afterAll(async () => { await browser?.close(); });

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
  await expect(page.getByRole('status').filter({ hasText: 'Loading files' })).toBeVisible();
  release();
  await page.unrouteAll({ behavior: 'ignoreErrors' });

  await page.getByRole('navigation', { name: 'Folder path' }).getByRole('button', { name: 'Files' }).click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
  markListingProgress('returned-to-root');
  await page.getByRole('button', { name: `Open ${fixtures.folder}`, exact: true }).click();
  await page.getByRole('article').filter({ hasText: fixtures.nested }).getByRole('button', { name: 'Open' }).click();
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
  const emptyName = `empty-${phase}-${Date.now()}`;
  await page.getByRole('button', { name: 'New folder', exact: true }).click();
  await page.getByLabel('Folder name', { exact: true }).fill(emptyName);
  await page.getByRole('button', { name: 'Create folder' }).click();
  await page.getByRole('article').filter({ hasText: emptyName }).getByRole('button', { name: 'Open' }).click();
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
  await expect(page.getByRole('article').filter({ hasText: `page-${phase}-100` })).toBeVisible();
  markListingProgress('next-page-visible');
  await page.getByRole('button', { name: 'Previous page' }).click();
  await expect(page.getByRole('article').filter({ hasText: `page-${phase}-000` })).toBeVisible();
  await expect(page.getByRole('article').filter({ hasText: `page-${phase}-100` })).toHaveCount(0);
  expect(cursors).toEqual([null, 'synthetic-next-page', null]);
  markListingProgress('previous-page-verified');
});

test('Unicode, zero-byte, and binary files round-trip through real upload and download routes', { annotation: { type: 'harness_case', description: 'storage_upload_download_roundtrip' } }, async () => {
  await signIn();
  await page.evaluate(() => {
    Object.defineProperty(window, '__s1LargeBlobReads', { configurable: true, writable: true, value: 0 });
    const original = Blob.prototype.arrayBuffer;
    Blob.prototype.arrayBuffer = function() {
      if (this.size > 64 * 1024) {
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
    const savedItem = page.getByRole('article').filter({ has: page.getByText(item.name, { exact: true }) });
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
  release();
  await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeEnabled({ timeout: 20_000 });
  await expect(report).toBeVisible();
  await report.locator('summary').click();
  await expect(report.locator(`[data-upload-result="slow-upload-${phase}.bin"]`)).toContainText('Saved');
  await page.getByRole('navigation', { name: 'Files', exact: true }).getByRole('button', { name: 'Files', exact: true }).click();
  await expect(page.getByRole('article', { name: `slow-upload-${phase}.bin`, exact: true })).toBeVisible();
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
  await expect(page.getByRole('article').filter({ hasText: fileName })).toHaveCount(0);
});

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
    if (!firstStatusLookupFailed && request.method() === 'GET' && /^\/uploads\/[A-Za-z0-9-]+$/.test(target.pathname)) {
      firstStatusLookupFailed = true;
      await route.abort('failed');
      return;
    }
    await route.continue();
  });
  await page.route('**/uploads/*/commit', async route => {
    const request = route.request();
    const target = new URL(request.url());
    if (target.origin !== fixtureOrigin.origin || target.search || !/^\/uploads\/[A-Za-z0-9-]+\/commit$/.test(target.pathname) || request.method() !== 'POST') {
      throw new Error('unexpected fixture commit route');
    }
    const headers = await request.allHeaders();
    const body = request.postDataBuffer();
    const statusCode = await new Promise<number>((resolve, reject) => {
      const forwarded = https.request({
        hostname: 'fixture.test',
        servername: 'fixture.test',
        family: 4,
        port: Number(fixtureOrigin.port),
        path: `${target.pathname}${target.search}`,
        method: request.method(),
        headers,
        ca,
        rejectUnauthorized: true,
        lookup: (_hostname, _options, callback) => callback(null, '127.0.0.1', 4),
      }, response => {
        response.once('end', () => resolve(response.statusCode ?? 0));
        response.resume();
      });
      forwarded.setTimeout(10_000, () => forwarded.destroy(new Error('fixture commit forward timed out')));
      forwarded.once('error', reject);
      if (body) forwarded.write(body);
      forwarded.end();
    });
    expect(statusCode).toBe(201);
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
  await expect(page.getByRole('status').filter({ hasText: 'Upload complete' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Check upload status' }).click();
  await expect(unresolved).toBeHidden();
  await expect(page.getByRole('article').filter({ hasText: fileName })).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole('status').filter({ hasText: 'Upload complete' })).toBeAttached();
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
  await expect(page.getByRole('status').filter({ hasText: 'Upload is admitted or still processing' })).toBeVisible();
  await expect(admissionWarning).toBeVisible();
  await expect(page.locator('#upload-file')).toBeDisabled();
  expect(ticketRequests).toBe(1);
  expect(admissionRequests).toBe(1);
  await expect(page.getByRole('article').filter({ hasText: admissionFile })).toHaveCount(0);
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
