import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';
import { ApiClient, ResponseTooLargeError, type ApiTransport, type RequestOptions } from '../src/lib/api';
import type { FileEntry } from '../src/lib/listing';
import { loadPreview, PREVIEW_LIMIT, PDF_PREVIEW_LIMIT } from '../src/lib/preview';

const entry = (name: string, size = 10): FileEntry => ({ id: 'preview', parent_id: 'root', kind: 'file', name, size, etag: '"1"' });
function harness(blob: Blob) {
  const requests: Array<{ path: string; options: RequestOptions }> = [];
  const transport: ApiTransport = { async request<T>(path: string, options: RequestOptions = {}): Promise<T> {
    requests.push({ path, options });
    return blob as T;
  } };
  return { client: new ApiClient(transport), requests };
}

test('preview rejects active document types and oversized files before downloading', async () => {
  const { client, requests } = harness(new Blob(['unused']));
  for (const file of [entry('page.html'), entry('drawing.svg'), entry('archive.zip'), entry('large.txt', PREVIEW_LIMIT + 1)]) {
    expect((await loadPreview(client, file, '/content', new AbortController().signal)).kind).toBe('unsupported');
  }
  expect(requests).toEqual([]);
});

test('text previews preserve UTF-8 content and request bounded authenticated bytes', async () => {
  const text = '<script>window.executed = true</script>\nCafé';
  const { client, requests } = harness(new Blob([text], { type: 'application/json' }));
  const signal = new AbortController().signal;
  expect(await loadPreview(client, entry('notes.txt'), '/entries/preview/content', signal)).toEqual({ kind: 'text', text });
  expect(requests).toEqual([{ path: '/entries/preview/content', options: { signal, maxResponseBytes: PREVIEW_LIMIT, headers: {} } }]);
});

test('invalid UTF-8 and binary control bytes do not become text previews', async () => {
  for (const bytes of [new Uint8Array([0xc3, 0x28]), new Uint8Array([65, 0, 66]), new Uint8Array([65, 0x1b, 66])]) {
    const { client } = harness(new Blob([bytes]));
    expect(await loadPreview(client, entry('notes.txt'), '/content', new AbortController().signal)).toMatchObject({ kind: 'unsupported' });
  }
});

test('preview checks returned size even when a custom transport ignores the bound', async () => {
  const { client } = harness(new Blob([new Uint8Array(PREVIEW_LIMIT + 1)]));
  expect(await loadPreview(client, entry('notes.txt'), '/content', new AbortController().signal)).toMatchObject({ kind: 'unsupported', message: expect.stringContaining('2 MiB') });
});

test('a streamed size mismatch offers download without a futile preview retry', async () => {
  const transport: ApiTransport = { async request<T>(): Promise<T> { throw new ResponseTooLargeError(); } };
  expect(await loadPreview(new ApiClient(transport), entry('notes.txt'), '/content', new AbortController().signal)).toMatchObject({ kind: 'unsupported', message: expect.stringContaining('2 MiB') });
});

test('raster signatures replace an untrusted response MIME and dimensions are capped', async () => {
  const png = new Uint8Array(24);
  png.set([137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82]);
  const dimensions = new DataView(png.buffer);
  dimensions.setUint32(16, 800);
  dimensions.setUint32(20, 600);
  const { client } = harness(new Blob([png], { type: 'text/html' }));
  const result = await loadPreview(client, entry('photo.png'), '/content', new AbortController().signal);
  expect(result.kind).toBe('image');
  if (result.kind === 'image') expect(result.blob.type).toBe('image/png');
  dimensions.setUint32(16, 100_000);
  const oversized = harness(new Blob([png]));
  expect((await loadPreview(oversized.client, entry('photo.png'), '/content', new AbortController().signal)).kind).toBe('unsupported');
});

test('malformed raster bytes cannot supply an active image document', async () => {
  const malformed = [new Uint8Array([0xff, 0xd8, 0xff]), new Uint8Array([0xff, 0xd8, 0xff, 0xc0, 0, 255]), new TextEncoder().encode('<svg onload="alert(1)"></svg>')];
  for (const bytes of malformed) {
    const { client } = harness(new Blob([bytes], { type: 'image/svg+xml' }));
    expect((await loadPreview(client, entry('image.jpg'), '/content', new AbortController().signal)).kind).toBe('unsupported');
  }
});

test('preview discards data when its request was aborted', async () => {
  const controller = new AbortController();
  const transport: ApiTransport = { async request<T>(): Promise<T> { controller.abort(); return new Blob(['late response']) as T; } };
  await expect(loadPreview(new ApiClient(transport), entry('notes.txt'), '/content', controller.signal)).rejects.toMatchObject({ name: 'AbortError' });
});

test('closing previews releases image URLs and aborts pending content requests', async ({ page }) => {
  await page.addInitScript(() => {
    const state = { created: [] as string[], revoked: [] as string[], aborted: 0 };
    Object.assign(window, { previewResources: state });
    const create = URL.createObjectURL.bind(URL);
    const revoke = URL.revokeObjectURL.bind(URL);
    URL.createObjectURL = (blob) => { const url = create(blob); state.created.push(url); return url; };
    URL.revokeObjectURL = (url) => { state.revoked.push(url); revoke(url); };
    const fetchOriginal = window.fetch.bind(window);
    window.fetch = (input, options) => {
      if (String(input).endsWith('/content')) options?.signal?.addEventListener('abort', () => state.aborted++, { once: true });
      return fetchOriginal(input, options);
    };
  });
  const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
  let delay = false;
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/auth/csrf') return route.fulfill({ json: { csrf_token: 'csrf' } });
    if (path === '/auth/local') return route.fulfill({ json: { account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' } });
    if (path === '/folders/children') return route.fulfill({ json: { root, items: [entry('photo.png')], next_cursor: null, has_more: false } });
    if (path === '/folders/root/children') return route.fulfill({ json: { items: [entry('photo.png')], next_cursor: null, has_more: false } });
    if (path === '/entries/preview/content') {
      if (delay) return;
      return route.fulfill({ contentType: 'image/png', body: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=', 'base64') });
    }
    await route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.getByRole('button', { name: 'Preview photo.png', exact: true }).click();
  await expect(page.getByRole('dialog').getByRole('img')).toBeVisible();
  await page.getByRole('button', { name: 'Close preview', exact: true }).click();
  await expect.poll(() => page.evaluate(() => {
    const state = (window as unknown as { previewResources: { created: string[]; revoked: string[] } }).previewResources;
    return state.created.length === 1 && state.revoked.includes(state.created[0]);
  })).toBe(true);
  delay = true;
  await page.getByRole('button', { name: 'Preview photo.png', exact: true }).click();
  await expect(page.getByRole('dialog').getByRole('status')).toHaveText('Loading preview…');
  await page.getByRole('button', { name: 'Close preview', exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { previewResources: { aborted: number } }).previewResources.aborted)).toBe(2);
});

test('PDF preview uses its own byte bound and rejects disguised documents', async () => {
  const { client, requests } = harness(new Blob(['%PDF-1.7\ninvalid body']));
  const signal = new AbortController().signal;
  const result = await loadPreview(client, entry('document.PDF'), '/content', signal);
  expect(result.kind).toBe('pdf');
  expect(requests[0].options.maxResponseBytes).toBe(PDF_PREVIEW_LIMIT);
  const oversized = harness(new Blob([new Uint8Array(PDF_PREVIEW_LIMIT + 1)]));
  expect(await loadPreview(oversized.client, entry('document.pdf'), '/content', signal)).toMatchObject({ kind: 'unsupported', message: expect.stringContaining('20 MiB') });
  expect(await loadPreview(client, entry('document.pdf', PDF_PREVIEW_LIMIT + 1), '/content', signal)).toMatchObject({ kind: 'unsupported' });
  const disguised = harness(new Blob(['<html>not a PDF</html>'], { type: 'application/pdf' }));
  expect(await loadPreview(disguised.client, entry('document.pdf'), '/content', signal)).toMatchObject({ kind: 'unsupported', message: expect.stringContaining('not a PDF') });
});

async function pdfHarness(page: import('@playwright/test').Page, contents: () => Buffer) {
  const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/') {
      const response = await route.fetch();
      return route.fulfill({ response, headers: { ...response.headers(), 'content-security-policy': "connect-src 'self'" } });
    }
    if (path === '/auth/csrf') return route.fulfill({ json: { csrf_token: 'csrf' } });
    if (path === '/auth/local') return route.fulfill({ json: { account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' } });
    if (path === '/folders/children') return route.fulfill({ json: { root, items: [entry('document.pdf')], next_cursor: null, has_more: false } });
    if (path === '/folders/root/children') return route.fulfill({ json: { items: [entry('document.pdf')], next_cursor: null, has_more: false } });
    if (path === '/entries/preview/content') return route.fulfill({ contentType: 'application/pdf', body: contents() });
    await route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.getByRole('button', { name: 'Preview document.pdf', exact: true }).click();
}

for (const width of [1280, 390]) {
  test(`PDF renders pages and zooms at ${width}px without external requests`, async ({ page, baseURL }) => {
    await page.setViewportSize({ width, height: 900 });
    const origin = new URL(baseURL!).origin;
    const external: string[] = [];
    page.on('request', request => {
      if (!request.url().startsWith(`${origin}/`) && !request.url().startsWith('blob:') && !request.url().startsWith('data:')) external.push(request.url());
    });
    await page.addInitScript(() => {
      const state = { created: 0, terminated: 0 };
      Object.assign(window, { pdfWorkers: state });
      const OriginalWorker = window.Worker;
      window.Worker = new Proxy(OriginalWorker, { construct(target, args) {
        const worker: Worker = Reflect.construct(target, args);
        state.created++;
        const terminate = worker.terminate.bind(worker);
        worker.terminate = () => { state.terminated++; terminate(); };
        return worker;
      } });
    });
    const fixture = readFileSync(new URL('./fixtures/three-pages.pdf', import.meta.url));
    await pdfHarness(page, () => fixture);
    const dialog = page.getByRole('dialog');
    const canvas = dialog.locator('canvas');
    await expect(canvas).toBeVisible();
    await expect(dialog.getByText('Page 1 of 3', { exact: true })).toBeVisible();
    const firstPixels = await canvas.evaluate(element => (element as HTMLCanvasElement).toDataURL());
    await dialog.getByRole('button', { name: 'Next page', exact: true }).click();
    await expect(dialog.getByText('Page 2 of 3', { exact: true })).toBeVisible();
    await expect(canvas).toBeVisible();
    expect(await canvas.evaluate(element => (element as HTMLCanvasElement).toDataURL())).not.toBe(firstPixels);
    const initialWidth = await canvas.evaluate(element => element.getBoundingClientRect().width);
    await dialog.getByRole('button', { name: 'Zoom in', exact: true }).click();
    await expect(canvas).toBeVisible();
    expect(await canvas.evaluate(element => element.getBoundingClientRect().width)).toBeGreaterThan(initialWidth);
    await dialog.getByRole('button', { name: 'Fit width', exact: true }).click();
    await expect(canvas).toBeVisible();
    const dimensions = await canvas.evaluate(element => ({ width: (element as HTMLCanvasElement).width, height: (element as HTMLCanvasElement).height }));
    expect(dimensions.width * dimensions.height).toBeLessThanOrEqual(4_000_000);
    expect(dimensions.width).toBeLessThanOrEqual(4096);
    expect(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
    await dialog.getByRole('button', { name: 'Close preview', exact: true }).click();
    await expect.poll(() => page.evaluate(() => {
      const workers = (window as unknown as { pdfWorkers: { created: number; terminated: number } }).pdfWorkers;
      return workers.created > 0 && workers.terminated === workers.created;
    })).toBe(true);
    expect(external).toEqual([]);
  });
}

test('malformed PDFs offer retry and download, and retry renders valid content', async ({ page }) => {
  let malformed = true;
  const fixture = readFileSync(new URL('./fixtures/three-pages.pdf', import.meta.url));
  await pdfHarness(page, () => malformed ? Buffer.from('%PDF-1.7\nnot a document\n%%EOF') : fixture);
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('alert')).toContainText('Could not open this PDF');
  await expect(dialog.getByRole('button', { name: 'Download file', exact: true })).toBeVisible();
  malformed = false;
  await dialog.getByRole('button', { name: 'Retry preview', exact: true }).click();
  await expect(dialog.locator('canvas')).toBeVisible();
  await expect(dialog.getByText('Page 1 of 3', { exact: true })).toBeVisible();
});

test('rapid PDF page changes and closing do not leave stale rendering on reopen', async ({ page }) => {
  const fixture = readFileSync(new URL('./fixtures/three-pages.pdf', import.meta.url));
  await pdfHarness(page, () => fixture);
  const dialog = page.getByRole('dialog');
  await expect(dialog.locator('canvas')).toBeVisible();
  await dialog.evaluate(element => {
    const click = (name: string) => (Array.from(element.querySelectorAll('button')).find(button => button.textContent === name) as HTMLButtonElement).click();
    click('Next page');
    click('Next page');
  });
  await expect(dialog.getByText('Page 3 of 3', { exact: true })).toBeVisible();
  await expect(dialog.locator('canvas')).toBeVisible();
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Zoom in', exact: true }).click();
  await dialog.getByRole('button', { name: 'Close preview', exact: true }).click();
  await page.getByRole('button', { name: 'Preview document.pdf', exact: true }).click();
  await expect(dialog.getByText('Page 1 of 3', { exact: true })).toBeVisible();
  await expect(dialog.locator('canvas')).toBeVisible();
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  expect(await dialog.locator('canvas').evaluate(element => (element as HTMLCanvasElement).width)).toBeGreaterThan(0);
});
