import { Buffer } from 'node:buffer';
import { createHash } from 'node:crypto';
import { test, expect, type Page } from '@playwright/test';

async function replacementBackend(page: Page, outcome: 'confirmed' | 'lost-response' | 'uncertain' | 'changed' = 'confirmed') {
  const root = { id: 'root', parent_id: null, name: 'My files', kind: 'folder', etag: '"root:1"' };
  const original = { id: 'old', parent_id: 'root', name: 'same.bin', kind: 'file', size: 9, etag: '"old:1"' };
  let active: Array<Record<string, unknown>> = [original];
  let deleted: Array<Record<string, unknown>> = [];
  let issued = 0;
  let replacement = false;
  let sent = 0;
  let committed = false;
  const bytes = Buffer.from('replacement bytes');
  const digest = createHash('sha256').update(bytes).digest('hex');
  await page.route('**/*', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, json: body });
    if (path === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (path === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user' });
    if (path === '/folders/children') return json({ root, items: [], has_more: false, next_cursor: null });
    if (path === '/folders/root/children') return json({ items: active, has_more: false, next_cursor: null });
    if (path === '/trash') return json({ items: deleted.map((entry) => ({ ...entry, original_path: [] })) });
    if (path === '/mutation-tickets') {
      const body = request.postDataJSON();
      replacement = !!body.replacement;
      if (replacement) expect(body.replacement).toEqual({ entry_id: 'old', etag: '"old:1"' });
      return json({ mutation_id: `upload-${++issued}`, ticket: `ticket-${issued}`, expires_at: '2030-01-01' }, 201);
    }
    if (path === '/uploads') {
      if (!replacement) return json({ code: 'name_collision', message: 'Name conflict', request_id: 'test', existing: original }, 409);
      expect(request.headers()['if-match']).toBe('"old:1"');
      expect(request.headers()['if-none-match']).toBeUndefined();
      return json({ upload_id: `upload-${issued}` }, 201);
    }
    if (path.endsWith('/content') && request.method() === 'PUT') {
      sent++;
      expect(request.postDataBuffer()).toEqual(bytes);
      return json({ upload_id: `upload-${issued}`, mutation_id: `upload-${issued}`, status: 'staged', length: bytes.length, sha256: digest, published: false });
    }
    if (path.endsWith('/commit')) {
      if (outcome === 'changed') return json({ code: 'replacement_changed' }, 412);
      expect(request.headers()['if-match']).toBe('"old:1"');
      if (outcome !== 'uncertain') {
        active = [{ ...original, id: 'new', size: bytes.length, etag: '"new:1"' }];
        deleted = [original];
        committed = true;
      }
      return outcome === 'confirmed' ? json({ status: 'committed' }, 201) : route.abort('failed');
    }
    if (/^\/uploads\/upload-\d+$/.test(path) && request.method() === 'DELETE') return route.fulfill({ status: 204 });
    if (/^\/uploads\/upload-\d+$/.test(path)) return json({ state: committed ? 'committed' : 'publishing', status: committed ? 'committed' : 'staged' });
    return route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('a long test password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByText('same.bin', { exact: true })).toBeVisible();
  const choose = async () => {
    await page.getByLabel('Choose a file to upload').setInputFiles({ name: 'same.bin', mimeType: 'application/octet-stream', buffer: bytes });
    await page.getByRole('button', { name: 'Upload', exact: true }).click();
    await expect(page.getByRole('dialog')).toBeVisible();
  };
  return { choose, sent: () => sent };
}

for (const width of [1280, 390]) {
  test(`replacement asks before sending bytes and explains recovery at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const backend = await replacementBackend(page);
    await backend.choose();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('Replace same.bin?');
    await expect(dialog).toContainText('previous file will move to deleted items');
    expect(backend.sent()).toBe(0);
    await page.screenshot({ path: test.info().outputPath(`replacement-${width}.png`), fullPage: true, animations: 'disabled' });
    await page.getByRole('button', { name: 'Keep existing file' }).click();
    await expect(dialog).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Upload', exact: true })).toBeFocused();
    expect(backend.sent()).toBe(0);
    await backend.choose();
    await page.getByRole('button', { name: 'Replace file', exact: true }).click();
    await expect(page.getByRole('status').filter({ hasText: 'Replacement complete.' })).toContainText('Replacement complete. The previous file is in deleted items.');
    expect(backend.sent()).toBe(1);
    await page.getByRole('button', { name: 'Deleted items', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Restore same.bin' })).toBeVisible();
  });
}

test('lost replacement response is resolved through upload status', async ({ page }) => {
  const backend = await replacementBackend(page, 'lost-response');
  await backend.choose();
  await page.getByRole('button', { name: 'Replace file', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Replacement complete.' })).toContainText('Replacement complete. The previous file is in deleted items.');
  await expect(page.getByText('Upload status needs checking')).toHaveCount(0);
});

test('uncertain replacement does not claim success or retry the upload', async ({ page }) => {
  const backend = await replacementBackend(page, 'uncertain');
  await backend.choose();
  await page.getByRole('button', { name: 'Replace file', exact: true }).click();
  await expect(page.getByText('Upload status needs checking')).toBeVisible();
  await expect(page.getByText('Replacement complete.', { exact: false })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Upload', exact: true })).toBeDisabled();
  expect(backend.sent()).toBe(1);
});


test('changed replacement target explains failure and allows a fresh upload', async ({ page }) => {
  const backend = await replacementBackend(page, 'changed');
  await backend.choose();
  await page.getByRole('button', { name: 'Replace file', exact: true }).click();
  await expect(page.getByText('Replacement was not saved because the file or folder changed.', { exact: false })).toBeVisible();
  await expect(page.getByText('Upload status needs checking')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Upload', exact: true })).toBeEnabled();
  await expect(page.getByText('same.bin', { exact: true })).toBeVisible();
  expect(backend.sent()).toBe(1);
});
