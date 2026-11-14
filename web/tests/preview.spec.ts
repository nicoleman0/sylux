import { expect, test } from '@playwright/test';
import { ApiClient, ResponseTooLargeError, type ApiTransport, type RequestOptions } from '../src/lib/api';
import type { FileEntry } from '../src/lib/listing';
import { loadPreview, PREVIEW_LIMIT, PDF_PREVIEW_LIMIT } from '../src/lib/preview';
import { scannedPdf, mixedPdf, manyPagesPdf, blankPdf, annotatedPdf } from './fixtures/pdf-fixtures';

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

const pageCanvas = (page: import('@playwright/test').Page, number = 1) => page.locator(`.pdfViewer .page[data-page-number="${number}"] canvas`).first();

async function rendered(page: import('@playwright/test').Page, number = 1) {
  const canvas = pageCanvas(page, number);
  await expect(canvas).toBeVisible();
  await expect.poll(() => canvas.evaluate((canvas: HTMLCanvasElement) => canvas.width > 0 && canvas.height > 0)).toBe(true);
  await expect(page.locator('.pdf-scroll-container')).toHaveAttribute('aria-busy', 'false');
  return canvas;
}

for (const width of [1280, 390]) {
  for (const theme of ['light', 'dark'] as const) {
    test(`PDF full-window reader renders and zooms at ${width}px in ${theme}`, async ({ page, baseURL }, testInfo) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      const external: string[] = [];
      const origin = new URL(baseURL!).origin;
      page.on('request', request => {
        if (!request.url().startsWith(`${origin}/`) && !/^(blob:|data:)/.test(request.url())) external.push(request.url());
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
      await pdfHarness(page, mixedPdf);
      const dialog = page.getByRole('dialog');
      await rendered(page);
      await expect(dialog.getByRole('spinbutton', { name: 'Page number' })).toHaveValue('1');
      const geometry = await dialog.evaluate(element => {
        const rect = element.getBoundingClientRect();
        const pane = element.querySelector('.pdf-scroll-container')!.getBoundingClientRect();
        return { width: rect.width, height: rect.height, content: pane.height, overflow: element.scrollWidth > element.clientWidth };
      });
      expect(geometry.width).toBe(width);
      expect(geometry.height).toBe(900);
      expect(geometry.content).toBeGreaterThan(700);
      expect(geometry.overflow).toBe(false);
      await expect(dialog.locator('.textLayer').first()).toContainText('Portrait page');
      await expect(dialog.getByText('Read-only preview.')).toHaveCount(0);
      await page.screenshot({ path: testInfo.outputPath(`reader-${width}-${theme}.png`) });
      await dialog.getByRole('button', { name: 'Next page', exact: true }).click();
      await expect(dialog.getByRole('spinbutton', { name: 'Page number' })).toHaveValue('2');
      const canvas = await rendered(page, 2);
      await expect(dialog.getByRole('spinbutton', { name: 'Page number' })).toHaveValue('2');
      await expect(dialog.getByRole('combobox', { name: 'PDF zoom' })).toHaveValue('auto');
      const before = await canvas.evaluate(element => element.getBoundingClientRect().width);
      await dialog.getByRole('button', { name: 'Zoom in', exact: true }).click();
      await expect.poll(() => canvas.evaluate(element => element.getBoundingClientRect().width)).toBeGreaterThan(before);
      await dialog.getByRole('combobox', { name: 'PDF zoom' }).selectOption('page-width');
      await rendered(page, 2);
      await expect.poll(() => canvas.evaluate(element => element.getBoundingClientRect().width)).toBeLessThanOrEqual(width);
      await expect(dialog.getByRole('button', { name: 'Download file', exact: true })).toBeVisible();
      await dialog.getByRole('button', { name: 'Close preview', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Preview document.pdf', exact: true })).toBeFocused();
      await expect.poll(() => page.evaluate(() => {
        const workers = (window as unknown as { pdfWorkers: { created: number; terminated: number } }).pdfWorkers;
        return workers.created > 0 && workers.terminated === workers.created;
      })).toBe(true);
      expect(external).toEqual([]);
    });
  }
}

test('PDF continuous scrolling, text selection and find work across mixed page sizes', async ({ page }) => {
  await pdfHarness(page, mixedPdf);
  await rendered(page);
  await page.locator('.pdf-scroll-container').hover();
  await page.mouse.wheel(0, 1100);
  await expect(page.getByRole('spinbutton', { name: 'Page number' })).not.toHaveValue('1');
  await page.keyboard.press('ControlOrMeta+f');
  const find = page.getByRole('textbox', { name: 'Find in PDF' });
  await expect(find).toBeFocused();
  await find.fill('Landscape');
  await find.press('Enter');
  await expect(page.locator('.textLayer .highlight.selected')).toContainText('Landscape');
  await expect(page.getByRole('spinbutton', { name: 'Page number' })).toHaveValue('2');
  const text = page.locator('.page[data-page-number="2"] .textLayer');
  const selected = await text.evaluate(element => {
    const range = document.createRange(); range.selectNodeContents(element);
    const selection = window.getSelection()!; selection.removeAllRanges(); selection.addRange(range);
    return { text: selection.toString(), selectable: getComputedStyle(element).userSelect !== 'none' };
  });
  expect(selected.text).toContain('Landscape');
  expect(selected.selectable).toBe(true);
  await find.press('Escape');
  await expect(page.getByRole('dialog')).toBeVisible();
  await expect(find).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('PDF image above the former 16M pixel cutoff renders instead of a blank page', async ({ page }) => {
  await pdfHarness(page, scannedPdf);
  const canvas = await rendered(page);
  await expect.poll(() => canvas.evaluate((canvas: HTMLCanvasElement) => {
    const pixel = canvas.getContext('2d')!.getImageData(Math.floor(canvas.width / 2), Math.floor(canvas.height / 2), 1, 1).data;
    return pixel[0] < 70 && pixel[1] < 70 && pixel[2] < 70 && pixel[3] === 255;
  })).toBe(true);
  await expect(page.getByRole('alert')).toHaveCount(0);
});

test('PDF genuine blank page is valid content', async ({ page }) => {
  await pdfHarness(page, blankPdf);
  await rendered(page);
  await expect(page.getByRole('alert')).toHaveCount(0);
});

test('PDF high-DPI zoom keeps canvas resources bounded and renders detail', async ({ browser }) => {
  const context = await browser.newContext({ deviceScaleFactor: 2, viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  await pdfHarness(page, mixedPdf);
  await rendered(page);
  await page.getByRole('combobox', { name: 'PDF zoom' }).selectOption('4');
  await expect.poll(() => page.locator('.page[data-page-number="1"] canvas').count()).toBeGreaterThan(1);
  await expect(page.locator('.pdf-scroll-container')).toHaveAttribute('aria-busy', 'false');
  const viewportPixels = await page.locator('.pdf-scroll-container').evaluate(element => element.clientWidth * element.clientHeight * devicePixelRatio ** 2);
  const resources = await page.locator('.page[data-page-number="1"] canvas').evaluateAll(elements => elements.map(element => {
    const canvas = element as HTMLCanvasElement;
    return { pixels: canvas.width * canvas.height, side: Math.max(canvas.width, canvas.height) };
  }));
  expect(resources[0].pixels).toBeLessThanOrEqual(4_100_000);
  // PDF.js detail canvases cover at least the visible viewport at native DPR.
  for (const canvas of resources) { expect(canvas.pixels).toBeLessThanOrEqual(Math.max(4_000_000, viewportPixels) + 10_000); expect(canvas.side).toBeLessThanOrEqual(4096); }
  await context.close();
});

test('PDF long-document scrolling evicts old canvases', async ({ page }) => {
  await pdfHarness(page, () => manyPagesPdf(40));
  await rendered(page);
  const input = page.getByRole('spinbutton', { name: 'Page number' });
  for (const number of Array.from({ length: 20 }, (_, index) => (index + 1) * 2)) {
    await input.fill(String(number)); await input.press('Enter'); await rendered(page, number);
  }
  const resources = await page.locator('.pdfViewer canvas').evaluateAll(elements => ({
    count: elements.length,
    pixels: elements.reduce((total, element) => total + (element as HTMLCanvasElement).width * (element as HTMLCanvasElement).height, 0),
  }));
  expect(resources.count).toBeLessThanOrEqual(12);
  expect(resources.pixels).toBeLessThanOrEqual(48_000_000);
  await expect(pageCanvas(page, 1)).toHaveCount(0);
});

test('malformed PDFs offer retry and download, and retry renders valid content', async ({ page }) => {
  let malformed = true;
  await pdfHarness(page, () => malformed ? Buffer.from('%PDF-1.7\nnot a document\n%%EOF') : mixedPdf());
  await expect(page.getByRole('alert')).toContainText('Could not open this PDF');
  await expect(page.getByRole('button', { name: 'Download file', exact: true })).toBeVisible();
  malformed = false;
  await page.getByRole('button', { name: 'Retry preview', exact: true }).click();
  await rendered(page);
  await expect(page.getByRole('spinbutton', { name: 'Page number' })).toHaveValue('1');
});

test('rapid PDF page changes and closing do not leave stale rendering on reopen', async ({ page }) => {
  await pdfHarness(page, mixedPdf);
  await rendered(page);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await rendered(page, 3);
  await page.getByRole('button', { name: 'Zoom in', exact: true }).click();
  await page.getByRole('button', { name: 'Close preview', exact: true }).click();
  await page.getByRole('button', { name: 'Preview document.pdf', exact: true }).click();
  await rendered(page);
  await expect(page.getByRole('spinbutton', { name: 'Page number' })).toHaveValue('1');
  await expect(page.getByRole('alert')).toHaveCount(0);
});


test('PDF preserves saved annotation appearances in read-only preview', async ({ page }) => {
  await pdfHarness(page, annotatedPdf);
  const canvas = await rendered(page);
  await expect.poll(() => canvas.evaluate((canvas: HTMLCanvasElement) => {
    const rgba = canvas.getContext('2d')!.getImageData(Math.floor(canvas.width / 2), Math.floor(canvas.height / 2), 1, 1).data;
    return rgba[0] > 220 && rgba[1] < 40 && rgba[2] < 40 && rgba[3] === 255;
  })).toBe(true);
  await expect(page.locator('.annotationLayer input, .annotationLayer textarea')).toHaveCount(0);
});

test('PDF page limit leaves download and retry available', async ({ page }) => {
  await pdfHarness(page, () => manyPagesPdf(501));
  await expect(page.getByRole('alert')).toContainText('500-page preview limit');
  await expect(page.getByRole('button', { name: 'Download file', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Retry preview', exact: true })).toBeVisible();
  await expect(page.locator('.pdfViewer canvas')).toHaveCount(0);
});

test('PDF canceled detail rendering returns to idle without a stale loading state', async ({ page }) => {
  await pdfHarness(page, mixedPdf);
  await rendered(page);
  await page.evaluate(() => {
    const state = window as unknown as { pdfjsViewer: { EventBus: { prototype: { dispatch: (name: string, args: { source?: { pageView?: unknown } }) => void } } }; canceledDetail?: boolean };
    const prototype = state.pdfjsViewer.EventBus.prototype;
    const dispatch = prototype.dispatch;
    // Cancel a real detail render at its start, before PDF.js has a chance to finish it.
    prototype.dispatch = function (name, args) {
      dispatch.call(this, name, args);
      if (name !== 'pagerender' || !args.source?.pageView) return;
      prototype.dispatch = dispatch;
      state.canceledDetail = true;
      const zoom = document.querySelector<HTMLSelectElement>('[aria-label="PDF zoom"]')!;
      zoom.value = '1'; zoom.dispatchEvent(new Event('change', { bubbles: true }));
    };
  });
  await page.getByRole('combobox', { name: 'PDF zoom' }).selectOption('4');
  await expect.poll(() => page.evaluate(() => (window as unknown as { canceledDetail?: boolean }).canceledDetail)).toBe(true);
  await rendered(page);
  await expect(page.getByRole('combobox', { name: 'PDF zoom' })).toHaveValue('1');
  await expect(page.getByRole('alert')).toHaveCount(0);
});

test('PDF page regions have accessible labels without duplicate canvas announcements', async ({ page }) => {
  await pdfHarness(page, mixedPdf);
  const canvas = await rendered(page);
  await expect(page.locator('.page[data-page-number="1"]')).toHaveAttribute('aria-label', /Page.*1/);
  await expect(canvas).toHaveAttribute('role', 'presentation');
});
