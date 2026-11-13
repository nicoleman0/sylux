import { expect, test } from '@playwright/test';

for (const missingMap of ['Map', 'WeakMap'] as const) {
  test(`missing ${missingMap}.getOrInsertComputed leaves PDF download available without loading the renderer`, async ({ page }) => {
    await page.addInitScript((missing) => {
      const prototype = missing === 'Map' ? Map.prototype : WeakMap.prototype;
      Object.defineProperty(prototype, 'getOrInsertComputed', { value: undefined, configurable: true });
    }, missingMap);

    const rendererRequests: string[] = [];
    page.on('request', request => {
      if (/PdfPreview\.svelte|pdfjs-dist|pdf\.worker|pdf\.renderer/.test(request.url())) rendererRequests.push(request.url());
    });

    const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
    await page.route('**/*', async route => {
      const path = new URL(route.request().url()).pathname;
      if (path === '/auth/csrf') return route.fulfill({ json: { csrf_token: 'csrf' } });
      if (path === '/auth/local') return route.fulfill({ json: { account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' } });
      if (path === '/folders/children') return route.fulfill({ json: { root, items: [{ id: 'pdf', parent_id: 'root', kind: 'file', name: 'document.pdf', size: 32, etag: '"1"' }], next_cursor: null, has_more: false } });
      if (path === '/folders/root/children') return route.fulfill({ json: { items: [{ id: 'pdf', parent_id: 'root', kind: 'file', name: 'document.pdf', size: 32, etag: '"1"' }], next_cursor: null, has_more: false } });
      if (path === '/entries/pdf/content') return route.fulfill({ contentType: 'application/pdf', body: '%PDF-1.7\nplaceholder' });
      await route.continue();
    });

    await page.goto('/');
    await page.getByLabel('Account name').fill('owner');
    await page.getByLabel('Password', { exact: true }).fill('password');
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await page.getByRole('button', { name: 'Preview document.pdf', exact: true }).click();

    const dialog = page.getByRole('dialog');
    await expect(dialog.getByRole('alert')).toContainText('PDF previews need a newer browser');
    await expect(dialog.getByRole('button', { name: 'Download file', exact: true })).toBeVisible();
    expect(rendererRequests).toEqual([]);
  });
}
