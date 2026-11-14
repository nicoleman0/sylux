import { expect, test, type Page } from '@playwright/test';

type Mode = 'ready' | 'open-stall' | 'viewer-stall' | 'page-stall' | 'render-stall' | 'render-error' | 'runtime-error';
type PdfMock = { mode: Mode; opens: number; attached: number; pageChanges: number; pages: number; renders: number; scales: number; updates: number; cleaned: number; destroyed: number; detached: number; releaseDocument?: () => void; releasePage?: () => void; releaseRender?: () => void; setRenderState?: (state: number, pageNumber?: number) => void; discardRender?: (pageNumber: number) => void; startDetail?: () => void; cancelDetail?: () => void };

async function mockState(page: Page) {
  return page.evaluate(() => (window as unknown as { pdfMock: PdfMock }).pdfMock);
}

const viewerModule = `
export class EventBus {
  listeners = new Map();
  on(name, callback, options) {
    this._on(name, callback);
    options?.signal?.addEventListener('abort', () => this._off(name, callback), { once: true });
  }
  off(name, callback) { this._off(name, callback); }
  _on(name, callback) {
    const callbacks = this.listeners.get(name) || new Set();
    callbacks.add(callback);
    this.listeners.set(name, callbacks);
  }
  _off(name, callback) { this.listeners.get(name)?.delete(callback); }
  dispatch(name, event) { for (const callback of this.listeners.get(name) || []) callback(event); }
}
export const FindState = { FOUND: 0, NOT_FOUND: 1, WRAPPED: 2, PENDING: 3 };
export const RenderingStates = { INITIAL: 0, RUNNING: 1, PAUSED: 2, FINISHED: 3 };
export class GenericL10n {}
export class PDFFindController { constructor() {} }
export class PDFLinkService { setViewer() {} setDocument() {} }
export class PDFViewer {
  constructor({ container, viewer, eventBus }) {
    this.container = container;
    this.viewer = viewer;
    this.eventBus = eventBus;
    this.page = 1;
    this.scale = 1;
    this.pageSources = new Map();
    window.pdfMock.releaseRender = () => this.finish();
    window.pdfMock.setRenderState = (state, number = this.page) => { this.pageSources.get(number).renderingState = state; };
    window.pdfMock.discardRender = number => this.pageSources.delete(number);
    window.pdfMock.startDetail = () => {
      const pageView = this.pageSources.get(this.page);
      this.detailSource = { renderingState: RenderingStates.RUNNING, pageView };
      pageView.detailView = this.detailSource;
      this.eventBus.dispatch('pagerender', { source: this.detailSource, pageNumber: this.page });
    };
    window.pdfMock.cancelDetail = () => {
      this.detailSource.renderingState = RenderingStates.INITIAL;
      this.detailSource.pageView.detailView = null;
    };
  }
  setDocument(document) {
    this.document = document;
    if (!document) {
      window.pdfMock.detached++;
      this.viewer.replaceChildren();
      return;
    }
    document.getPage(1).then(page => {
      if (window.pdfMock.mode === 'viewer-stall' || !this.document) return;
      this.getPageView(0).setPdfPage(page);
      this.eventBus.dispatch('pagesinit', { source: this });
      this.render();
    }).catch(() => {});
  }
  get pdfDocument() { return this.document; }
  get pagesCount() { return this.document?.numPages || 0; }
  get currentPageNumber() { return this.page; }
  set currentPageNumber(number) {
    if (this.page === number) return;
    this.page = number;
    window.pdfMock.pageChanges++;
    this.eventBus.dispatch('pagechanging', { source: this, pageNumber: number });
    const source = this.getPageView(number - 1);
    if (source.pdfPage) { this.render(); return; }
    this.document?.getPage(number).then(page => {
      if (!this.document) return;
      source.setPdfPage(page);
      if (this.page === number) this.render();
    }).catch(() => {});
  }
  get currentScale() { return Number(this.scale) || 1; }
  get currentScaleValue() { return this.scale; }
  set currentScaleValue(value) {
    this.scale = value;
    window.pdfMock.scales++;
    this.eventBus.dispatch('scalechanging', { source: this, scale: this.currentScale, presetValue: ['auto', 'page-width', 'page-fit'].includes(value) ? value : undefined });
    this.render();
  }
  scrollPageIntoView({ pageNumber }) { this.currentPageNumber = pageNumber; }
  update() { window.pdfMock.updates++; this.render(); }
  cleanup() { window.pdfMock.cleaned++; }
  getPageView(index) {
    let source = this.pageSources.get(index + 1);
    if (!source) {
      source = { id: index + 1, renderingState: RenderingStates.INITIAL,
        setPdfPage(page) { this.pdfPage = page; window.pdfMock.attached++; } };
      this.pageSources.set(index + 1, source);
    }
    return source;
  }
  isPageVisible(number) { return number === this.page; }
  render() {
    if (!this.document || this.container.clientWidth <= 0 || window.pdfMock.mode === 'viewer-stall') return;
    const source = this.getPageView(this.page - 1);
    if (!source.pdfPage) return;
    source.renderingState = RenderingStates.RUNNING;
    this.pageSources.set(this.page, source);
    this.eventBus.dispatch('pagerender', { source, pageNumber: this.page });
    window.pdfMock.renders++;
    if (window.pdfMock.mode === 'render-stall') return;
    queueMicrotask(() => this.finish());
  }
  finish() {
    const state = window.pdfMock;
    const error = state.mode === 'runtime-error' ? new TypeError('this[#n].getOrInsertComputed is not a function')
      : state.mode === 'render-error' ? new Error('render failure sentinel') : undefined;
    if (!error && this.document) {
      const canvas = document.createElement('canvas');
      canvas.width = 600;
      canvas.height = 900;
      this.viewer.replaceChildren(canvas);
    }
    const source = this.pageSources.get(this.page);
    if (source) source.renderingState = RenderingStates.FINISHED;
    this.eventBus.dispatch('pagerendered', { source: this.pageSources.get(this.page), pageNumber: this.page, error });
  }
}
`;

async function openPdf(page: Page, mode: Mode, zeroWidth = false) {
  await page.addInitScript(({ mode, zeroWidth }) => {
    Object.assign(window, { pdfMock: { mode, opens: 0, attached: 0, pageChanges: 0, pages: 0, renders: 0, scales: 0, updates: 0, cleaned: 0, destroyed: 0, detached: 0 } });
    if (zeroWidth) document.addEventListener('DOMContentLoaded', () => {
      const style = document.createElement('style');
      style.id = 'zero-width-pdf';
      style.textContent = '.pdf-scroll-container { width: 0 !important; padding-inline: 0 !important; }';
      document.head.append(style);
    });
  }, { mode, zeroWidth });
  const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"1"' };
  const file = { id: 'preview', parent_id: 'root', name: 'document.pdf', kind: 'file', size: 30, etag: '"1"' };
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith('/pdfjs-dist.js')) return route.fulfill({ contentType: 'application/javascript', body: `
      export const GlobalWorkerOptions = {};
      export const AnnotationMode = { DISABLE: 0, ENABLE: 1 };
      export const AnnotationEditorType = { DISABLE: -1 };
      export function getDocument() {
        const state = window.pdfMock;
        state.opens++;
        const waiters = [];
        const document = {
          numPages: 3,
          getPage: () => {
            state.pages++;
            const page = { cleanup: () => { state.cleaned++; } };
            return state.mode === 'page-stall' ? new Promise(resolve => {
              waiters.push(() => resolve(page));
              state.releasePage = () => { for (const release of waiters.splice(0)) release(); };
            }) : Promise.resolve(page);
          },
          destroy: async () => { state.destroyed++; },
        };
        return {
          promise: state.mode === 'open-stall' ? new Promise(resolve => { state.releaseDocument = () => resolve(document); }) : Promise.resolve(document),
          destroy: async () => { state.destroyed++; },
        };
      }
    ` });
    if (path.includes('pdfjs-dist') && /pdf_+viewer/.test(path) && !path.endsWith('.css')) return route.fulfill({ contentType: 'application/javascript', body: viewerModule });
    if (path === '/auth/csrf') return route.fulfill({ json: { csrf_token: 'csrf' } });
    if (path === '/auth/local') return route.fulfill({ json: { account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' } });
    if (path === '/folders/children') return route.fulfill({ json: { root, items: [file], next_cursor: null, has_more: false } });
    if (path === '/folders/root/children') return route.fulfill({ json: { items: [file], next_cursor: null, has_more: false } });
    if (path === '/entries/preview/content') return route.fulfill({ contentType: 'application/pdf', body: '%PDF-1.7\nmock document\n%%EOF' });
    await route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  const preview = page.getByRole('button', { name: 'Preview document.pdf', exact: true });
  await expect(preview).toBeVisible();
  await page.clock.install();
  await page.clock.pauseAt(new Date(Date.now() + 1000));
  await preview.click();
  await expect.poll(async () => (await mockState(page)).opens).toBe(1);
  return page.getByRole('dialog');
}

for (const mode of ['open-stall', 'viewer-stall', 'page-stall', 'render-stall'] as const) {
  test(`PDF ${mode} stops after its deadline and releases the viewer`, async ({ page }) => {
    const dialog = await openPdf(page, mode);
    if (mode === 'render-stall') await expect.poll(async () => (await mockState(page)).renders).toBeGreaterThan(0);
    await page.clock.fastForward(19_999);
    await expect(dialog.getByRole('alert')).toHaveCount(0);
    await page.clock.fastForward(1);
    await expect(dialog.getByRole('alert')).toContainText('took too long');
    await expect(dialog.getByRole('status').filter({ hasText: /Opening PDF|Rendering PDF page/ })).toHaveCount(0);
    await expect(dialog.getByRole('button', { name: 'Download file', exact: true })).toBeVisible();
    await expect.poll(async () => (await mockState(page)).destroyed).toBeGreaterThan(0);
    await dialog.getByRole('button', { name: 'Close preview', exact: true }).click();
    await page.evaluate(() => {
      const state = (window as unknown as { pdfMock: PdfMock }).pdfMock;
      state.releaseDocument?.();
      state.releasePage?.();
      state.releaseRender?.();
    });
    await page.clock.runFor(100);
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(page.getByRole('alert')).toHaveCount(0);
  });
}

test('PDF zero-width recovery keeps the remaining render deadline', async ({ page }) => {
  const dialog = await openPdf(page, 'render-stall', true);
  expect((await mockState(page)).renders).toBe(0);
  await page.clock.fastForward(15_000);
  await page.evaluate(() => document.getElementById('zero-width-pdf')?.remove());
  await expect.poll(async () => {
    await page.clock.runFor(16);
    return (await mockState(page)).renders;
  }).toBeGreaterThan(0);
  await page.clock.fastForward(5000);
  await expect(dialog.getByRole('alert')).toContainText('took too long');
  expect((await mockState(page)).opens).toBe(1);
});

test('PDF resizes do not extend a stalled render deadline', async ({ page }) => {
  const dialog = await openPdf(page, 'render-stall');
  await expect.poll(async () => (await mockState(page)).renders).toBeGreaterThan(0);
  for (let index = 0; index < 4; index++) {
    await page.clock.fastForward(4000);
    await page.setViewportSize({ width: 1000 + index * 40, height: 800 });
    await page.clock.runFor(16);
  }
  await page.clock.fastForward(4000);
  await expect(dialog.getByRole('alert')).toContainText('took too long');
  expect((await mockState(page)).opens).toBe(1);
  await expect.poll(async () => (await mockState(page)).destroyed).toBeGreaterThan(0);
});

test('PDF timeout stays terminal through late render events and resizes', async ({ page }) => {
  const dialog = await openPdf(page, 'render-stall');
  await expect.poll(async () => (await mockState(page)).renders).toBeGreaterThan(0);
  await page.clock.fastForward(20_000);
  await expect(dialog.getByRole('alert')).toContainText('took too long');
  await page.evaluate(() => {
    const state = (window as unknown as { pdfMock: PdfMock }).pdfMock;
    state.mode = 'ready';
    state.releaseRender?.();
  });
  await page.setViewportSize({ width: 900, height: 800 });
  await page.clock.runFor(100);
  await expect(dialog.getByRole('alert')).toContainText('took too long');
  await expect(dialog.getByRole('status').filter({ hasText: /Opening PDF|Rendering PDF page/ })).toHaveCount(0);
});

test('PDF retry opens a fresh document after a terminal timeout', async ({ page }) => {
  const dialog = await openPdf(page, 'render-stall');
  await page.clock.fastForward(20_000);
  await expect(dialog.getByRole('alert')).toBeVisible();
  await page.evaluate(() => { (window as unknown as { pdfMock: PdfMock }).pdfMock.mode = 'ready'; });
  await dialog.getByRole('button', { name: 'Retry preview', exact: true }).click();
  await expect(dialog.locator('canvas')).toBeVisible();
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  expect((await mockState(page)).opens).toBe(2);
  expect((await mockState(page)).destroyed).toBeGreaterThan(0);
});

test('PDF closing a pending render detaches its viewer and ignores late completion', async ({ page }) => {
  const dialog = await openPdf(page, 'render-stall');
  await expect.poll(async () => (await mockState(page)).renders).toBeGreaterThan(0);
  await dialog.getByRole('button', { name: 'Close preview', exact: true }).click();
  await expect.poll(async () => (await mockState(page)).detached).toBeGreaterThan(0);
  await expect.poll(async () => (await mockState(page)).destroyed).toBeGreaterThan(0);
  await page.evaluate(() => (window as unknown as { pdfMock: PdfMock }).pdfMock.releaseRender?.());
  await page.clock.fastForward(25_000);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('alert')).toHaveCount(0);
});

for (const [mode, cause] of [['render-error', 'render failure sentinel'], ['runtime-error', 'this[#n].getOrInsertComputed is not a function']] as const) {
  test(`PDF ${mode} retains its cause in local diagnostics`, async ({ page }) => {
    const diagnostics: Array<{ text: string; error: { name: string; message: string; stack?: string } }> = [];
    page.on('console', async message => {
      if (message.type() !== 'warning' || !message.text().startsWith('PDF preview failed')) return;
      const error = await message.args()[1].evaluate((error: Error) => ({ name: error.name, message: error.message, stack: error.stack }));
      diagnostics.push({ text: message.text(), error });
    });
    const dialog = await openPdf(page, mode);
    await expect(dialog.getByRole('alert')).toContainText('Could not display');
    await expect.poll(() => diagnostics[0]?.error.message).toBe(cause);
    expect(diagnostics[0].error.name).toBe(mode === 'runtime-error' ? 'TypeError' : 'Error');
    expect(diagnostics[0].error.stack).toContain('viewer');
    expect(JSON.stringify(diagnostics)).not.toContain('mock document');
  });
}

test('PDF a later page gets a render deadline that zoom cannot extend', async ({ page }) => {
  const dialog = await openPdf(page, 'ready');
  await expect(dialog.locator('canvas')).toBeVisible();
  await page.evaluate(() => { (window as unknown as { pdfMock: PdfMock }).pdfMock.mode = 'render-stall'; });
  await dialog.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(dialog.getByLabel('Page number', { exact: true })).toHaveValue('2');
  await page.clock.fastForward(15_000);
  await dialog.getByLabel('PDF zoom', { exact: true }).selectOption('2');
  await page.clock.fastForward(4999);
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  await page.clock.fastForward(1);
  await expect(dialog.getByRole('alert')).toContainText('This page took too long to display');
  expect((await mockState(page)).opens).toBe(1);
});


test('PDF a paused render retains a bounded wait after resuming without another render event', async ({ page }) => {
  const dialog = await openPdf(page, 'ready');
  await expect(dialog.locator('canvas')).toBeVisible();
  await page.evaluate(() => { (window as unknown as { pdfMock: PdfMock }).pdfMock.mode = 'render-stall'; });
  await dialog.getByRole('button', { name: 'Next page', exact: true }).click();
  await page.clock.runFor(5000);
  await page.evaluate(() => (window as unknown as { pdfMock: PdfMock }).pdfMock.setRenderState?.(2));
  await page.clock.runFor(30_000);
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  await page.evaluate(() => (window as unknown as { pdfMock: PdfMock }).pdfMock.setRenderState?.(1));
  await page.clock.runFor(14_000);
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  await page.clock.runFor(2000);
  await expect(dialog.getByRole('alert')).toContainText('This page took too long to display');
});


test('PDF canceled off-screen work stops showing activity and a replacement gets a fresh deadline', async ({ page }) => {
  const dialog = await openPdf(page, 'ready');
  await expect(dialog.locator('canvas')).toBeVisible();
  await page.evaluate(() => { (window as unknown as { pdfMock: PdfMock }).pdfMock.mode = 'render-stall'; });
  await dialog.getByRole('button', { name: 'Next page', exact: true }).click();
  await page.clock.runFor(15_000);
  await page.evaluate(() => {
    const state = (window as unknown as { pdfMock: PdfMock }).pdfMock;
    state.setRenderState?.(0, 2);
    state.mode = 'ready';
  });
  await dialog.getByRole('button', { name: 'Previous page', exact: true }).click();
  await page.clock.runFor(25_000);
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  await expect(dialog.getByRole('status').filter({ hasText: /Opening PDF|Rendering PDF page/ })).toHaveCount(0);
  await expect(dialog.locator('.pdf-scroll-container')).toHaveAttribute('aria-busy', 'false');
  await page.evaluate(() => {
    const state = (window as unknown as { pdfMock: PdfMock }).pdfMock;
    state.discardRender?.(2);
    state.mode = 'render-stall';
  });
  await dialog.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(dialog.getByRole('status').filter({ hasText: 'Rendering PDF page' })).toBeVisible();
  await page.clock.fastForward(19_999);
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  await page.clock.fastForward(1);
  await expect(dialog.getByRole('alert')).toContainText('This page took too long to display');
});

test('PDF later-page acquisition shows activity until its bounded failure', async ({ page }) => {
  const dialog = await openPdf(page, 'ready');
  await expect(dialog.locator('canvas')).toBeVisible();
  await page.evaluate(() => { (window as unknown as { pdfMock: PdfMock }).pdfMock.mode = 'page-stall'; });
  await dialog.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(dialog.getByLabel('Page number', { exact: true })).toHaveValue('2');
  await expect(dialog.getByRole('status').filter({ hasText: 'Rendering PDF page' })).toBeVisible();
  await expect(dialog.locator('.pdf-scroll-container')).toHaveAttribute('aria-busy', 'true');
  await page.clock.fastForward(19_999);
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  await page.clock.fastForward(1);
  await expect(dialog.getByRole('alert')).toContainText('This page took too long to load');
  await expect(dialog.getByRole('status').filter({ hasText: /Opening PDF|Rendering PDF page/ })).toHaveCount(0);
  await expect.poll(async () => (await mockState(page)).destroyed).toBeGreaterThan(0);
});


test('PDF a removed detail view releases activity and its replacement gets a fresh deadline', async ({ page }) => {
  const dialog = await openPdf(page, 'ready');
  await expect(dialog.locator('canvas')).toBeVisible();
  await page.evaluate(() => (window as unknown as { pdfMock: PdfMock }).pdfMock.startDetail?.());
  await expect(dialog.getByRole('status').filter({ hasText: 'Rendering PDF page' })).toBeVisible();
  await page.clock.runFor(15_000);
  await page.evaluate(() => (window as unknown as { pdfMock: PdfMock }).pdfMock.cancelDetail?.());
  await page.clock.runFor(25_000);
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  await expect(dialog.getByRole('status').filter({ hasText: /Opening PDF|Rendering PDF page/ })).toHaveCount(0);
  await expect(dialog.locator('.pdf-scroll-container')).toHaveAttribute('aria-busy', 'false');
  await page.evaluate(() => (window as unknown as { pdfMock: PdfMock }).pdfMock.startDetail?.());
  await expect(dialog.getByRole('status').filter({ hasText: 'Rendering PDF page' })).toBeVisible();
  await page.clock.fastForward(19_999);
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  await page.clock.fastForward(1);
  await expect(dialog.getByRole('alert')).toContainText('This page took too long to display');
});

test('PDF numeric zoom survives a pending preset navigation finishing later', async ({ page }) => {
  const dialog = await openPdf(page, 'ready');
  await expect(dialog.locator('canvas')).toBeVisible();
  await page.evaluate(() => { (window as unknown as { pdfMock: PdfMock }).pdfMock.mode = 'page-stall'; });
  await dialog.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(dialog.getByRole('status').filter({ hasText: 'Rendering PDF page' })).toBeVisible();
  await dialog.getByLabel('PDF zoom', { exact: true }).selectOption('2');
  await page.evaluate(() => {
    const state = (window as unknown as { pdfMock: PdfMock }).pdfMock;
    state.mode = 'ready';
    state.releasePage?.();
  });
  await expect(dialog.getByRole('status').filter({ hasText: /Opening PDF|Rendering PDF page/ })).toHaveCount(0);
  await expect(dialog.getByLabel('PDF zoom', { exact: true })).toHaveValue('2');
  await expect(dialog.getByLabel('Page number', { exact: true })).toHaveValue('2');
  await expect(dialog.getByRole('alert')).toHaveCount(0);
});

test('PDF a timed-out navigation ignores its late page without changing viewer state', async ({ page }) => {
  const dialog = await openPdf(page, 'ready');
  await expect(dialog.locator('canvas')).toBeVisible();
  await page.evaluate(() => { (window as unknown as { pdfMock: PdfMock }).pdfMock.mode = 'page-stall'; });
  await dialog.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(dialog.getByRole('status').filter({ hasText: 'Rendering PDF page' })).toBeVisible();
  await page.clock.fastForward(20_000);
  await expect(dialog.getByRole('alert')).toContainText('This page took too long to load');
  const before = await mockState(page);
  await page.evaluate(() => (window as unknown as { pdfMock: PdfMock }).pdfMock.releasePage?.());
  await page.clock.runFor(100);
  const after = await mockState(page);
  expect({ attached: after.attached, scales: after.scales, pageChanges: after.pageChanges, renders: after.renders })
    .toEqual({ attached: before.attached, scales: before.scales, pageChanges: before.pageChanges, renders: before.renders });
  await expect(dialog.getByRole('alert')).toContainText('This page took too long to load');
  await expect(dialog.getByRole('status').filter({ hasText: /Opening PDF|Rendering PDF page/ })).toHaveCount(0);
});
