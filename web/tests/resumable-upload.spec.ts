import { test, expect, type Page } from '@playwright/test';
import { createHash } from 'node:crypto';

async function fixture(page: Page) {
  const id = '1234567890abcdef1234567890abcdef';
  const original = Buffer.from('abcdefghijkl');
  const root = { id: 'root', name: 'Files', kind: 'folder', parent_id: null, etag: '"root:1"' };
  let bytes = Buffer.alloc(0), committed = false, commits = 0, admissions = 0;
  let paused = true, loseCommit = false;
  const offsets: number[] = [];
  await page.route('**/*', async route => {
    const request = route.request(), path = new URL(request.url()).pathname;
    const json = (value: unknown, status = 200) => route.fulfill({ json: value, status });
    if (path === '/instance') return json({ name: 'Sylux' });
    if (path === '/instance/appearance') return json({});
    if (path === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (path === '/auth/local') return json({ account_id: request.postDataJSON().handle, handle: 'owner', role: 'user' });
    if (path === '/auth/logout') return route.fulfill({ status: 204 });
    const entry = { id: 'file', parent_id: 'root', kind: 'file', name: 'video.bin', etag: '"file:1"', size: original.length };
    if (path === '/folders/children') return json({ root, items: committed ? [entry] : [], next_cursor: null, has_more: false });
    if (path === '/folders/root/path') return json({ items: [root] });
    if (path === '/folders/root/children') return json({ items: committed ? [entry] : [], next_cursor: null, has_more: false });
    if (path === '/mutation-tickets') return json({ mutation_id: id, ticket: 'original-ticket', expires_at: 1893456000, max_chunk_length: 4 }, 201);
    if (path === '/uploads') { admissions++; return json({ upload_id: id }, 201); }
    if (path === `/uploads/${id}/content`) {
      const offset = Number(request.headers()['upload-offset']); offsets.push(offset);
      expect(request.headers()['x-mutation-id']).toBe('original-ticket');
      if (paused && offset >= 4) return route.abort('connectionreset');
      if (offset !== bytes.length) return json({ code: 'upload_offset_mismatch' }, 409);
      const chunk = request.postDataBuffer()!;
      expect(request.headers()['upload-checksum']).toBe(createHash('sha256').update(chunk).digest('hex'));
      bytes = Buffer.concat([bytes, chunk]);
      return json({ upload_id: id, mutation_id: id, status: bytes.length === original.length ? 'staged' : 'admitted', bytes_received: bytes.length, length: original.length, sha256: createHash('sha256').update(bytes).digest('hex'), published: false });
    }
    if (path === `/uploads/${id}/commit`) {
      commits++; committed = true;
      if (loseCommit) return route.abort('connectionreset');
      return json({ status: 'committed' }, 201);
    }
    if (path === `/uploads/${id}`) {
      if (request.method() === 'DELETE') return route.fulfill({ status: 204 });
      return json({ upload_id: id, mutation_id: id, state: committed ? 'committed' : bytes.length === original.length ? 'publishing' : 'admitted', status: committed ? 'committed' : bytes.length === original.length ? 'staged' : 'admitted', bytes_received: bytes.length, length: original.length, writer_active: false, sha256: bytes.length === original.length ? createHash('sha256').update(bytes).digest('hex') : null });
    }
    return route.continue();
  });
  const login = async (owner = 'owner') => {
    await page.getByLabel('Account name').fill(owner);
    await page.getByLabel('Password').fill('password');
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeVisible();
  };
  await page.goto('/'); await login();
  return { original, login, offsets, bytes: () => bytes, admissions: () => admissions, commits: () => commits, resume: () => { paused = false; }, loseCommit: () => { loseCommit = true; } };
}

test('reload requires matching file and resumes the same admission at server offset', async ({ page }) => {
  const server = await fixture(page);
  await page.getByLabel('Choose files to upload').setInputFiles({ name: 'video.bin', mimeType: 'application/octet-stream', buffer: server.original });
  await expect.poll(() => server.bytes().length).toBe(4);
  await page.reload(); await server.login();
  await page.getByRole('button', { name: 'Check upload status', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Select original file', exact: true })).toBeVisible();
  await page.getByLabel('Original upload file', { exact: true }).setInputFiles({ name: 'video.bin', mimeType: 'application/octet-stream', buffer: Buffer.from('abcdWRONG!!!') });
  await expect(page.getByText('This file does not match the original upload.', { exact: false })).toBeVisible();
  expect(server.bytes().toString()).toBe('abcd'); expect(server.admissions()).toBe(1);
  server.resume(); server.loseCommit();
  await page.getByLabel('Original upload file', { exact: true }).setInputFiles({ name: 'video.bin', mimeType: 'application/octet-stream', buffer: server.original });
  await expect.poll(() => server.commits()).toBe(1);
  await expect(page.getByRole('row', { name: 'video.bin', exact: true })).toBeVisible();
  expect(server.bytes()).toEqual(server.original); expect(server.admissions()).toBe(1);
  expect(server.offsets.filter(offset => offset === 0)).toHaveLength(1);
  expect(await page.evaluate(() => sessionStorage.getItem('sylux-upload-v1:owner'))).toBeNull();
});

test('connection loss resumes retained file without a new admission', async ({ page }) => {
  const server = await fixture(page);
  await page.getByLabel('Choose files to upload').setInputFiles({ name: 'video.bin', mimeType: 'application/octet-stream', buffer: server.original });
  await expect(page.getByRole('button', { name: 'Check upload status', exact: true })).toBeVisible({ timeout: 15_000 });
  server.resume();
  await page.getByRole('button', { name: 'Check upload status', exact: true }).click();
  await page.getByRole('button', { name: 'Resume upload', exact: true }).click();
  await expect.poll(() => server.commits()).toBe(1);
  expect(server.bytes()).toEqual(server.original); expect(server.admissions()).toBe(1);
});

test('another account cannot restore the tab journal and logout clears its own record', async ({ page }) => {
  const server = await fixture(page);
  await page.getByLabel('Choose files to upload').setInputFiles({ name: 'video.bin', mimeType: 'application/octet-stream', buffer: server.original });
  await expect.poll(() => server.bytes().length).toBe(4);
  await page.reload(); await server.login('other');
  await expect(page.getByRole('button', { name: 'Check upload status', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Log out', exact: true }).click();
  await server.login();
  await expect(page.getByRole('button', { name: 'Check upload status', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Log out', exact: true }).click();
  expect(await page.evaluate(() => sessionStorage.getItem('sylux-upload-v1:owner'))).toBeNull();
});

test('lost commit reply clears recovery after original status confirms the save', async ({ page }) => {
  const server = await fixture(page); server.resume(); server.loseCommit();
  await page.getByLabel('Choose files to upload').setInputFiles({ name: 'video.bin', mimeType: 'application/octet-stream', buffer: server.original });
  await expect(page.getByRole('row', { name: 'video.bin', exact: true })).toBeVisible();
  expect(server.commits()).toBe(1);
  expect(await page.evaluate(() => sessionStorage.getItem('sylux-upload-v1:owner'))).toBeNull();
});

test('expired never-admitted journal is resolved before another upload', async ({ page }) => {
  const server = await fixture(page);
  await page.evaluate(() => sessionStorage.setItem('sylux-upload-v1:owner', JSON.stringify({
    version: 1, owner: 'owner', id: '1234567890abcdef1234567890abcdef', ticket: 'expired-ticket',
    body: { parent_id: 'root', parent_etag: '"root:1"', name: 'video.bin', length: 12 },
    identity: { name: 'video.bin', size: 12, fingerprint: 'a'.repeat(64) }, expires: 1, chunkSize: 4,
  })));
  await page.route('**/uploads/1234567890abcdef1234567890abcdef', route => route.fulfill({ status: 404, json: { code: 'upload_not_found' } }));
  await page.route('**/uploads', route => route.fulfill({ status: 401, json: { code: 'invalid_mutation_ticket' } }));
  await page.reload(); await server.login();
  await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Check upload status', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeEnabled();
  expect(await page.evaluate(() => sessionStorage.getItem('sylux-upload-v1:owner'))).toBeNull();
  expect(server.admissions()).toBe(0);
});
