import { test, expect, type Page, type Route } from '@playwright/test';
import { createHash } from 'node:crypto';

type Faults = {
  pauseFrom?: number; // abort PATCHes at or after this offset without storing bytes
  dropOnce?: number; // abort one PATCH at this offset without storing bytes
  refuseOnce?: { offset: number; status: number; code: string }; // one PATCH rejected before storing bytes
  ackLost?: number; // store one chunk at this offset, then lose its reply
  finalTimeout?: boolean; // store the final chunk, keep the writer busy, then reply 504
  holdFinal?: boolean; // store the final chunk and never reply
  writerPolls?: number; // status checks that report an active writer
  statusFailures?: number; // status checks that fail at the network
  commit?: 'never-arrive' | 'lose-reply' | 'lose-reply-and-status' | 'collision' | 'landed-first';
  expiresAt?: number;
};

async function fixture(page: Page, faults: Faults = {}) {
  const id = '1234567890abcdef1234567890abcdef';
  const original = Buffer.from('abcdefghijkl');
  const root = { id: 'root', name: 'Files', kind: 'folder', parent_id: null, etag: '"root:1"' };
  let bytes = Buffer.alloc(0), committed = false, posts = 0, effects = 0, admissions = 0;
  const offsets: number[] = [];
  const held: Route[] = [];
  const digest = () => createHash('sha256').update(bytes).digest('hex');
  const staged = () => bytes.length === original.length;
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
    if (path === '/mutation-tickets') return json({ mutation_id: id, ticket: 'original-ticket', expires_at: faults.expiresAt ?? 1893456000, max_chunk_length: 4 }, 201);
    if (path === '/uploads') { admissions++; return json({ upload_id: id }, 201); }
    if (path === `/uploads/${id}/content`) {
      const offset = Number(request.headers()['upload-offset']); offsets.push(offset);
      expect(request.headers()['x-mutation-id']).toBe('original-ticket');
      if (faults.pauseFrom !== undefined && offset >= faults.pauseFrom) return route.abort('connectionreset');
      if (faults.dropOnce === offset) { faults.dropOnce = undefined; return route.abort('connectionreset'); }
      const refusal = faults.refuseOnce;
      if (refusal?.offset === offset) { faults.refuseOnce = undefined; return json({ code: refusal.code }, refusal.status); }
      if (offset !== bytes.length) return json({ code: 'upload_offset_mismatch' }, 409);
      const chunk = request.postDataBuffer() ?? Buffer.alloc(0);
      expect(request.headers()['upload-checksum']).toBe(createHash('sha256').update(chunk).digest('hex'));
      bytes = Buffer.concat([bytes, chunk]);
      if (faults.ackLost === offset) { faults.ackLost = undefined; return route.abort('connectionreset'); }
      if (staged() && faults.holdFinal) { held.push(route); return; }
      if (staged() && faults.finalTimeout) { faults.finalTimeout = false; faults.writerPolls = 2; return json({ code: 'gateway_timeout' }, 504); }
      return json({ upload_id: id, mutation_id: id, status: staged() ? 'staged' : 'admitted', bytes_received: bytes.length, length: original.length, sha256: digest(), published: false });
    }
    if (path === `/uploads/${id}/commit`) {
      posts++;
      const mode = faults.commit; faults.commit = undefined;
      if (mode === 'never-arrive') return route.abort('connectionreset');
      if (mode === 'collision') return json({ code: 'name_collision', existing: { id: 'other', name: 'video.bin', kind: 'file', etag: '"other:1"' } }, 409);
      // An earlier POST of the same mutation commits just before this one arrives.
      if (mode === 'landed-first') { committed = true; effects++; }
      // Like the server, a repeated save of one mutation has one effect.
      if (committed) return json({ code: 'upload_not_staged' }, 409);
      committed = true; effects++;
      if (mode === 'lose-reply-and-status') faults.statusFailures = 1;
      if (mode) return route.abort('connectionreset');
      return json({ status: 'committed' }, 201);
    }
    if (path === `/uploads/${id}`) {
      if (request.method() === 'DELETE') return route.fulfill({ status: 204 });
      if (faults.statusFailures) { faults.statusFailures--; return route.abort('failed'); }
      const writer = !!faults.writerPolls; if (writer) faults.writerPolls!--;
      const state = committed ? 'committed' : staged() && !writer ? 'publishing' : 'admitted';
      const status = committed ? 'committed' : writer ? 'uploading' : staged() ? 'staged' : 'admitted';
      return json({ upload_id: id, mutation_id: id, state, status, bytes_received: bytes.length, length: original.length, writer_active: writer, sha256: staged() && !writer ? digest() : null });
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
  return {
    original, login, offsets, faults, bytes: () => bytes, admissions: () => admissions, posts: () => posts, effects: () => effects,
    journal: () => page.evaluate(() => JSON.parse(sessionStorage.getItem('sylux-upload-v1:owner') ?? 'null')),
    choose: async () => {
      await expect(button(page, 'Choose files')).toBeEnabled();
      await page.getByLabel('Choose files to upload').setInputFiles({ name: 'video.bin', mimeType: 'application/octet-stream', buffer: original });
    },
    release: async () => { for (const route of held.splice(0)) await route.abort('connectionreset').catch(() => undefined); },
  };
}

const button = (page: Page, name: string) => page.getByRole('button', { name, exact: true });
const savedRow = (page: Page) => page.getByRole('row', { name: 'video.bin', exact: true });

test('reload requires matching file and resumes the same admission at server offset', async ({ page }) => {
  const server = await fixture(page, { pauseFrom: 4 });
  await server.choose();
  await expect.poll(() => server.bytes().length).toBe(4);
  await page.reload(); await server.login();
  await button(page, 'Check upload status').click();
  await expect(button(page, 'Select original file')).toBeVisible();
  // Same name and size: one with reordered chunks, one with different bytes after the saved offset.
  for (const wrong of ['efghabcdijkl', 'abcdWRONG!!!']) {
    await page.getByLabel('Original upload file', { exact: true }).setInputFiles({ name: 'video.bin', mimeType: 'application/octet-stream', buffer: Buffer.from(wrong) });
    await expect(page.getByText('This file does not match the original upload.', { exact: false })).toBeVisible();
  }
  expect(server.bytes().toString()).toBe('abcd'); expect(server.admissions()).toBe(1);
  server.faults.pauseFrom = undefined; server.faults.commit = 'lose-reply';
  const chooser = page.waitForEvent('filechooser');
  await button(page, 'Select original file').focus();
  await page.keyboard.press('Enter');
  await (await chooser).setFiles({ name: 'video.bin', mimeType: 'application/octet-stream', buffer: server.original });
  await expect(savedRow(page)).toBeVisible();
  expect(server.posts()).toBe(1); expect(server.effects()).toBe(1);
  expect(server.bytes()).toEqual(server.original); expect(server.admissions()).toBe(1);
  expect(server.offsets.filter(offset => offset === 0)).toHaveLength(1);
  expect(await server.journal()).toBeNull();
});

test('connection loss resumes retained file without a new admission', async ({ page }) => {
  const server = await fixture(page, { pauseFrom: 4 });
  await server.choose();
  await expect(button(page, 'Check upload status')).toBeVisible({ timeout: 15_000 });
  server.faults.pauseFrom = undefined;
  await button(page, 'Check upload status').click();
  await button(page, 'Resume upload').click();
  await expect.poll(() => server.effects()).toBe(1);
  expect(server.bytes()).toEqual(server.original); expect(server.admissions()).toBe(1);
});

test('a lost chunk reply after the server checkpoint continues from the saved offset', async ({ page }) => {
  const server = await fixture(page, { ackLost: 4 });
  await server.choose();
  await expect(savedRow(page)).toBeVisible();
  expect(server.offsets).toEqual([0, 4, 8]);
  expect(server.bytes()).toEqual(server.original); expect(server.posts()).toBe(1);
});

test('an active writer is awaited before the same chunk is sent again', async ({ page }) => {
  const server = await fixture(page, { dropOnce: 4, writerPolls: 2 });
  await server.choose();
  await expect(page.getByText('The server is receiving or verifying this upload.', { exact: false })).toBeVisible();
  await expect(savedRow(page)).toBeVisible();
  expect(server.offsets).toEqual([0, 4, 4, 8]);
  expect(server.bytes()).toEqual(server.original); expect(server.posts()).toBe(1);
});

test('transient status failures use the retry budget instead of stopping the upload', async ({ page }) => {
  const server = await fixture(page, { dropOnce: 4, statusFailures: 2 });
  await server.choose();
  await expect(savedRow(page)).toBeVisible();
  expect(server.offsets).toEqual([0, 4, 4, 8]);
  expect(server.bytes()).toEqual(server.original); expect(server.posts()).toBe(1);
});

for (const [status, code] of [[408, 'chunk_idle_timeout'], [503, 'server_stopping']] as const) {
  test(`a ${status} ${code} chunk reply resumes from the acknowledged offset`, async ({ page }) => {
    const server = await fixture(page, { refuseOnce: { offset: 4, status, code } });
    await server.choose();
    await expect(savedRow(page)).toBeVisible();
    expect(server.offsets).toEqual([0, 4, 4, 8]);
    expect(server.bytes()).toEqual(server.original); expect(server.posts()).toBe(1);
  });
}

test('a canceled upload stops instead of resending its chunk', async ({ page }) => {
  const server = await fixture(page, { refuseOnce: { offset: 4, status: 409, code: 'upload_canceled' } });
  await server.choose();
  await expect(button(page, 'Check upload status')).toBeVisible();
  expect(server.offsets).toEqual([0, 4]); expect(server.posts()).toBe(0);
});

test('a final chunk timed out during verification is confirmed by status, not resent', async ({ page }) => {
  const server = await fixture(page, { finalTimeout: true });
  await server.choose();
  await expect(page.getByText('The server is receiving or verifying this upload.', { exact: false })).toBeVisible();
  await expect(savedRow(page)).toBeVisible();
  expect(server.offsets).toEqual([0, 4, 8]);
  expect(server.posts()).toBe(1); expect(server.effects()).toBe(1);
});

test('reload between staging and the first save offers an explicit finish', async ({ page }) => {
  const server = await fixture(page, { holdFinal: true });
  await server.choose();
  await expect.poll(() => server.bytes().length).toBe(12);
  await page.reload(); await server.release(); await server.login();
  expect((await server.journal()).commitStarted).toBeUndefined();
  await button(page, 'Check upload status').click();
  await expect(page.getByText('Upload not saved yet')).toBeVisible();
  await expect(page.getByText('The file is not saved yet. Finish saving to add it.', { exact: false })).toBeVisible();
  expect(server.posts()).toBe(0);
  await button(page, 'Finish saving').focus();
  await page.keyboard.press('Enter');
  await expect(savedRow(page)).toBeVisible();
  expect(server.posts()).toBe(1); expect(server.effects()).toBe(1);
  expect(server.offsets.filter(offset => offset === 8)).toHaveLength(1);
  expect(await server.journal()).toBeNull();
});

test('a save that never reached the server is finished once after reload', async ({ page }) => {
  const server = await fixture(page, { commit: 'never-arrive' });
  await server.choose();
  await expect(button(page, 'Check upload status')).toBeVisible();
  expect(server.posts()).toBe(1); expect(server.effects()).toBe(0);
  await page.reload(); await server.login();
  const journal = await server.journal();
  expect(journal.commitStarted).toBe(true); expect(journal.ticket).toBe('original-ticket');
  // Reload and status checks only read status; saving needs the explicit action.
  await button(page, 'Check upload status').click();
  await expect(button(page, 'Finish saving')).toBeVisible();
  await button(page, 'Check upload status').click();
  await expect(button(page, 'Finish saving')).toBeVisible();
  expect(server.posts()).toBe(1);
  await button(page, 'Finish saving').click();
  await expect(savedRow(page)).toBeVisible();
  await expect(button(page, 'Finish saving')).toHaveCount(0);
  expect(server.posts()).toBe(2); expect(server.effects()).toBe(1);
  expect(server.bytes()).toEqual(server.original); expect(server.admissions()).toBe(1);
  expect(await server.journal()).toBeNull();
});

test('a refused finish explains the conflict and does not save again on its own', async ({ page }) => {
  const server = await fixture(page, { commit: 'never-arrive' });
  await server.choose();
  await expect(button(page, 'Check upload status')).toBeVisible();
  await button(page, 'Check upload status').click();
  server.faults.commit = 'collision';
  await button(page, 'Finish saving').click();
  await expect(page.getByRole('alert').getByText('An item with this name now exists in the destination, so this upload was not saved.', { exact: false })).toBeVisible();
  await expect(button(page, 'Finish saving')).toHaveCount(0);
  await expect(button(page, 'Check upload status')).toBeFocused();
  expect((await server.journal()).commitStarted).toBe(false);
  expect(server.posts()).toBe(2); expect(server.effects()).toBe(0);
  await button(page, 'Cancel upload').click();
  await expect(button(page, 'Check upload status')).toHaveCount(0);
  expect(server.posts()).toBe(2);
});

test('a finish that meets an already completed save resolves from status', async ({ page }) => {
  const server = await fixture(page, { commit: 'never-arrive' });
  await server.choose();
  await expect(button(page, 'Check upload status')).toBeVisible();
  await button(page, 'Check upload status').click();
  await expect(button(page, 'Finish saving')).toBeVisible();
  server.faults.commit = 'landed-first';
  await button(page, 'Finish saving').click();
  await expect(button(page, 'Check upload status')).toHaveCount(0);
  await expect(savedRow(page)).toBeVisible();
  expect(server.posts()).toBe(2); expect(server.effects()).toBe(1);
  expect(await server.journal()).toBeNull();
});

test('lost commit reply clears recovery after original status confirms the save', async ({ page }) => {
  const server = await fixture(page, { commit: 'lose-reply' });
  await server.choose();
  await expect(savedRow(page)).toBeVisible();
  expect(server.posts()).toBe(1); expect(server.effects()).toBe(1);
  expect(await server.journal()).toBeNull();
});

test('a lost reply after a real save resolves on reload without another save', async ({ page }) => {
  const server = await fixture(page, { commit: 'lose-reply-and-status' });
  await server.choose();
  await expect(button(page, 'Check upload status')).toBeVisible();
  await page.reload(); await server.login();
  // The listing already shows the saved file; the alert must resolve from status alone.
  await expect(savedRow(page)).toBeVisible();
  await button(page, 'Check upload status').click();
  await expect(button(page, 'Check upload status')).toHaveCount(0);
  await expect(button(page, 'Finish saving')).toHaveCount(0);
  expect(server.posts()).toBe(1); expect(server.effects()).toBe(1);
  expect(await server.journal()).toBeNull();
});

test('an expired upload cannot resume and is not read again', async ({ page }) => {
  const now = Date.now();
  const expiresAt = Math.floor(now / 1000) + 6;
  await page.clock.setFixedTime(now);
  const server = await fixture(page, { pauseFrom: 4, expiresAt });
  await server.choose();
  await expect.poll(() => server.bytes().length).toBe(4);
  await page.reload(); await server.login();
  await button(page, 'Check upload status').click();
  await expect(button(page, 'Select original file')).toBeVisible();
  expect((await server.journal()).expires).toBe(expiresAt);
  await page.clock.setFixedTime((expiresAt + 1) * 1000);
  const sent = server.offsets.length;
  server.faults.pauseFrom = undefined;
  await page.getByLabel('Original upload file', { exact: true }).setInputFiles({ name: 'video.bin', mimeType: 'application/octet-stream', buffer: server.original });
  await expect(page.getByText('Upload expired')).toBeVisible();
  await expect(page.getByText('The 24-hour upload window ended before this upload finished', { exact: false })).toBeVisible();
  await expect(button(page, 'Select original file')).toHaveCount(0);
  await expect(page.getByText('Checking file', { exact: false })).toHaveCount(0);
  await button(page, 'Check upload status').click();
  await expect(page.getByText('Upload expired')).toBeVisible();
  await expect(button(page, 'Select original file')).toHaveCount(0);
  expect(server.offsets.length).toBe(sent);
  await button(page, 'Cancel upload').click();
  await expect(button(page, 'Check upload status')).toHaveCount(0);
  expect(server.effects()).toBe(0);
});

test('another account cannot restore the tab journal and logout clears its own record', async ({ page }) => {
  const server = await fixture(page, { pauseFrom: 4 });
  await server.choose();
  await expect.poll(() => server.bytes().length).toBe(4);
  await page.reload(); await server.login('other');
  await expect(button(page, 'Check upload status')).toHaveCount(0);
  await button(page, 'Log out').click();
  await server.login();
  await expect(button(page, 'Check upload status')).toBeVisible();
  await button(page, 'Log out').click();
  expect(await server.journal()).toBeNull();
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
  await expect(button(page, 'Choose files')).toBeDisabled();
  await button(page, 'Check upload status').click();
  await expect(button(page, 'Choose files')).toBeEnabled();
  expect(await server.journal()).toBeNull();
  expect(server.admissions()).toBe(0);
});
