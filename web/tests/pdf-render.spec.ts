import { expect, test, type Page } from '@playwright/test';

type Mode = 'ready' | 'page-stall' | 'render-stall' | 'render-error' | 'runtime-error';
type PdfMock = { mode: Mode; pages: number; renders: number; cancelled: number; cleaned: number; destroyed: number; active: number; peak: number; releasePage?: () => void };

async function mockState(page: Page) {
  return page.evaluate(() => (window as unknown as { pdfMock: PdfMock }).pdfMock);
}

async function openPdf(page: Page, mode: Mode, zeroWidth = false) {
  await page.addInitScript(({ mode, zeroWidth }) => {
    Object.assign(window, { pdfMock: { mode, pages: 0, renders: 0, cancelled: 0, cleaned: 0, destroyed: 0, active: 0, peak: 0 } });
    if (zeroWidth) document.addEventListener('DOMContentLoaded', () => {
      const style = document.createElement('style');
      style.textContent = '.pdf-page { width: 0 !important; padding-inline: 0 !important; }';
      document.head.append(style);
    });
  }, { mode, zeroWidth });
  const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"1"' };
  const file = { id: 'preview', parent_id: 'root', name: 'document.pdf', kind: 'file', size: 30, etag: '"1"' };
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith('/pdfjs-dist.js')) return route.fulfill({ contentType: 'application/javascript', body: `
      export const GlobalWorkerOptions = {};
      export function getDocument() {
        const state = window.pdfMock;
        const page = {
          getViewport: ({ scale }) => ({ width: 600 * scale, height: 900 * scale }),
          cleanup: () => state.cleaned++,
          render: ({ canvas }) => {
            state.renders++;
            state.active++;
            state.peak = Math.max(state.peak, state.active);
            let reject;
            let settled = false;
            const finish = () => { if (!settled) { settled = true; state.active--; } };
            const promise = state.mode === 'render-stall' ? new Promise((_, fail) => { reject = fail; })
              : state.mode === 'render-error' ? Promise.reject(new Error('render failure sentinel'))
              : state.mode === 'runtime-error' ? Promise.reject(new TypeError('this[#n].getOrInsertComputed is not a function'))
              : Promise.resolve().then(() => { canvas.getContext('2d').fillRect(0, 0, 10, 10); });
            promise.then(finish, finish);
            return { promise, cancel: () => { state.cancelled++; reject?.(new Error('cancelled')); } };
          },
        };
        return { promise: Promise.resolve({ numPages: 3, getPage: () => {
          state.pages++;
          return state.mode === 'page-stall' ? new Promise(resolve => { state.releasePage = () => resolve(page); }) : Promise.resolve(page);
        } }), destroy: async () => { state.destroyed++; } };
      }
    ` });
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
  await expect.poll(async () => (await mockState(page)).pages).toBe(1);
  return page.getByRole('dialog');
}

for (const scenario of ['zero-width', 'page-stall', 'render-stall'] as const) {
  test(`PDF ${scenario} stops after the page deadline`, async ({ page }) => {
    const dialog = await openPdf(page, scenario === 'zero-width' ? 'ready' : scenario, scenario === 'zero-width');
    if (scenario === 'zero-width') expect(await dialog.locator('.pdf-page').evaluate(element => element.clientWidth)).toBe(0);
    if (scenario === 'render-stall') await expect.poll(async () => (await mockState(page)).renders).toBe(1);
    await page.clock.fastForward(19_999);
    await expect(dialog.getByRole('alert')).toHaveCount(0);
    await page.clock.fastForward(1);
    await expect(dialog.getByRole('alert')).toContainText('This page took too long to display');
    await expect(dialog.getByRole('status')).toHaveCount(0);
    await expect(dialog.getByRole('button', { name: 'Download file', exact: true })).toBeVisible();
    if (scenario === 'render-stall') await expect.poll(async () => (await mockState(page)).active).toBe(0);
    await dialog.getByRole('button', { name: 'Close preview', exact: true }).click();
    if (scenario === 'page-stall') {
      await page.evaluate(() => (window as unknown as { pdfMock: PdfMock }).pdfMock.releasePage?.());
      await expect.poll(async () => (await mockState(page)).cleaned).toBe(1);
    }
    await expect.poll(async () => (await mockState(page)).destroyed).toBeGreaterThan(0);
  });
}

test('PDF width recovery keeps the remaining page deadline', async ({ page }) => {
  const dialog = await openPdf(page, 'render-stall', true);
  expect((await mockState(page)).renders).toBe(0);
  await page.clock.fastForward(15_000);
  await dialog.locator('.pdf-page').evaluate(element => { (element as HTMLElement).style.setProperty('width', '600px', 'important'); });
  await expect.poll(async () => {
    await page.clock.runFor(16);
    return (await mockState(page)).renders;
  }).toBe(1);
  await page.clock.fastForward(5000);
  await expect(dialog.getByRole('alert')).toContainText('This page took too long to display');
  expect((await mockState(page)).pages).toBe(1);
});

test('PDF late page results are cleaned after switching pages without stale rendering', async ({ page }) => {
  const dialog = await openPdf(page, 'page-stall');
  await page.evaluate(() => { (window as unknown as { pdfMock: PdfMock }).pdfMock.mode = 'ready'; });
  await dialog.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(dialog.getByText('Page 2 of 3', { exact: true })).toBeVisible();
  await expect(dialog.locator('canvas')).toBeVisible();
  expect((await mockState(page)).renders).toBe(1);
  await page.evaluate(() => (window as unknown as { pdfMock: PdfMock }).pdfMock.releasePage?.());
  await expect.poll(async () => (await mockState(page)).cleaned).toBe(1);
  await page.clock.runFor(100);
  expect((await mockState(page)).renders).toBe(1);
  expect((await mockState(page)).pages).toBe(2);
  await expect(dialog.getByText('Page 2 of 3', { exact: true })).toBeVisible();
  await expect(dialog.locator('canvas')).toBeVisible();
});

test('PDF resizes reuse the page and do not extend a stalled render deadline', async ({ page }) => {
  const dialog = await openPdf(page, 'render-stall');
  await expect.poll(async () => (await mockState(page)).renders).toBe(1);
  for (let index = 0; index < 4; index++) {
    await page.clock.fastForward(4000);
    await dialog.locator('.pdf-page').evaluate((element, width) => { (element as HTMLElement).style.width = `${width}px`; }, 600 + index * 20);
    await page.clock.runFor(16);
    await expect.poll(async () => (await mockState(page)).renders).toBeGreaterThan(index + 1);
  }
  await page.clock.fastForward(4000);
  await expect(dialog.getByRole('alert')).toContainText('This page took too long to display');
  const state = await mockState(page);
  expect(state.pages).toBe(1);
  expect(state.peak).toBe(1);
  expect(state.active).toBe(0);
});

test('PDF canvas layout and scrollbar width settle after successful rendering', async ({ page }) => {
  const dialog = await openPdf(page, 'ready');
  await page.addStyleTag({ content: '.pdf-page::-webkit-scrollbar { width: 16px; height: 16px; }' });
  await page.clock.runFor(100);
  await expect(dialog.locator('canvas')).toBeVisible();
  const rendered = (await mockState(page)).renders;
  await page.clock.runFor(1000);
  expect((await mockState(page)).renders).toBe(rendered);
  await page.setViewportSize({ width: 900, height: 800 });
  await expect.poll(async () => {
    await page.clock.runFor(16);
    return (await mockState(page)).renders;
  }).toBeGreaterThan(rendered);
  await expect(dialog.locator('canvas')).toBeVisible();
  const resized = (await mockState(page)).renders;
  await page.clock.runFor(1000);
  expect((await mockState(page)).renders).toBe(resized);
  expect((await mockState(page)).pages).toBe(1);
});

test('PDF timeout stays terminal through resizes and retry starts a fresh preview', async ({ page }) => {
  const dialog = await openPdf(page, 'render-stall');
  await page.clock.fastForward(20_000);
  await expect(dialog.getByRole('alert')).toBeVisible();
  await page.setViewportSize({ width: 900, height: 800 });
  await page.clock.runFor(16);
  await expect(dialog.getByRole('alert')).toBeVisible();
  await page.evaluate(() => { (window as unknown as { pdfMock: PdfMock }).pdfMock.mode = 'ready'; });
  await dialog.getByRole('button', { name: 'Retry preview', exact: true }).click();
  await expect(dialog.locator('canvas')).toBeVisible();
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  expect((await mockState(page)).destroyed).toBeGreaterThan(0);
});

test('PDF page and zoom changes cancel pending renders, and closing releases the canvas', async ({ page }) => {
  const dialog = await openPdf(page, 'render-stall');
  await expect.poll(async () => (await mockState(page)).renders).toBe(1);
  await dialog.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect.poll(async () => (await mockState(page)).pages).toBe(2);
  await dialog.getByRole('button', { name: 'Zoom in', exact: true }).click();
  await expect.poll(async () => (await mockState(page)).pages).toBe(3);
  const canvas = await dialog.locator('canvas').elementHandle();
  await dialog.getByRole('button', { name: 'Close preview', exact: true }).click();
  await expect.poll(async () => (await mockState(page)).active).toBe(0);
  expect(await canvas!.evaluate(element => {
    const target = element as HTMLCanvasElement;
    return { width: target.width, height: target.height };
  })).toEqual({ width: 0, height: 0 });
  await page.clock.fastForward(25_000);
  await expect(page.getByRole('alert')).toHaveCount(0);
});

for (const [mode, cause] of [['render-error', 'render failure sentinel'], ['runtime-error', 'this[#n].getOrInsertComputed is not a function']] as const) {
  test(`PDF ${mode} retains its cause in local diagnostics`, async ({ page }) => {
    const diagnostics: Array<{ text: string; error: { name: string; message: string; stack?: string } }> = [];
    page.on('console', async message => {
      if (message.type() !== 'warning' || !message.text().startsWith('PDF preview failed')) return;
      const error = await message.args()[2].evaluate((error: Error) => ({ name: error.name, message: error.message, stack: error.stack }));
      diagnostics.push({ text: message.text(), error });
    });
    const dialog = await openPdf(page, mode);
    await expect(dialog.getByRole('alert')).toContainText('Could not display this PDF page');
    await expect.poll(() => diagnostics[0]?.error.message).toBe(cause);
    expect(diagnostics[0].text).toContain('PDF preview failed');
    expect(diagnostics[0].error.name).toBe(mode === 'runtime-error' ? 'TypeError' : 'Error');
    expect(diagnostics[0].error.stack).toContain('pdfjs-dist.js');
    expect(JSON.stringify(diagnostics)).not.toContain('mock document');
  });
}
