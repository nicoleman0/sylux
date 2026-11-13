import { Buffer } from 'node:buffer';
import { createHash } from 'node:crypto';
import { test, expect, type Locator, type Page } from '@playwright/test';

type TestEntry = { id: string; parent_id: string | null; name: string; kind: 'file' | 'folder'; etag: string; size?: number };
type Admission = { parent_id: string; parent_etag: string; name: string; length: number; replacement?: { entry_id: string; etag: string } };
type Source = { name: string; batches?: Source[][]; unreadable?: boolean; unavailable?: boolean; content?: string };

async function backend(page: Page, fault?: { name: string; outcome: 'failed' | 'uncertain' | 'folder-uncertain' }) {
  const root: TestEntry = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
  const entries: TestEntry[] = [
    { id: 'destination', parent_id: 'root', name: 'Destination', kind: 'folder', etag: '"destination:1"' },
    { id: 'inner', parent_id: 'destination', name: 'Inner', kind: 'folder', etag: '"inner:1"' },
    { id: 'existing', parent_id: 'root', name: 'same.txt', kind: 'file', size: 3, etag: '"existing:1"' },
  ];
  const tickets = new Map<string, Admission>();
  const states = new Map<string, string>();
  const events: Array<{ kind: string; name: string; parent: string }> = [];
  const sent: string[] = [];
  let sequence = 0;
  let active = 0;
  let maxActive = 0;
  const folder = (id: string) => id === 'root' ? root : entries.find(entry => entry.id === id)!;
  const bump = (id: string) => { const parent = folder(id); parent.etag = `"${id}:${++sequence}"`; };
  await page.route('**/*', async route => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ json: body, status });
    if (path === '/trash') return json({ items: [] });
    if (path === '/entries/search') return json({ items: [], next_cursor: null, has_more: false });
    if (path === '/auth/logout') return route.fulfill({ status: 204 });
    if (path === '/instance') return json({ name: 'Sylux' });
    if (path === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (path === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user' });
    if (path === '/folders/children' && request.method() === 'GET') return json({ root, items: entries.filter(entry => entry.parent_id === 'root'), next_cursor: null, has_more: false });
    if (path === '/folders/children' && request.method() === 'POST') {
      const body = request.postDataJSON() as { parent_id: string; name: string };
      events.push({ kind: 'folder', name: body.name, parent: body.parent_id });
      if (entries.some(entry => entry.parent_id === body.parent_id && entry.name === body.name)) return json({ code: 'name_conflict' }, 409);
      const entry: TestEntry = { id: `folder-${++sequence}`, parent_id: body.parent_id, name: body.name, kind: 'folder', etag: '"folder:1"' };
      entries.push(entry); bump(body.parent_id);
      if (fault?.name === body.name && fault.outcome === 'folder-uncertain') return route.abort('failed');
      return json(entry, 201);
    }
    if (/^\/folders\/[^/]+\/children$/.test(path)) {
      const children = entries.filter(entry => entry.parent_id === path.split('/')[2]);
      // Nested descriptors must also be found beyond the first page.
      if (path === '/folders/destination/children' && !url.searchParams.has('cursor')) return json({ items: [], next_cursor: 'next', has_more: true });
      return json({ items: children, next_cursor: null, has_more: false });
    }
    if (path === '/mutation-tickets') {
      const body = request.postDataJSON() as Admission;
      const id = `upload-${++sequence}`; tickets.set(id, body);
      return json({ mutation_id: id, ticket: id, expires_at: '2030-01-01' }, 201);
    }
    if (path === '/uploads') {
      const body = request.postDataJSON() as Admission;
      expect(body.parent_etag).toBe(folder(body.parent_id).etag);
      const conflict = entries.find(entry => entry.parent_id === body.parent_id && entry.name === body.name);
      if (conflict && !body.replacement) return json({ code: 'name_collision', existing: conflict }, 409);
      if (body.replacement) {
        expect(body.replacement).toEqual({ entry_id: conflict!.id, etag: conflict!.etag });
        expect(request.headers()['if-match']).toBe(conflict!.etag);
      }
      const id = request.headers()['x-mutation-id']!;
      states.set(id, 'admitted'); active++; maxActive = Math.max(active, maxActive);
      events.push({ kind: 'file', name: body.name, parent: body.parent_id });
      return json({ upload_id: id }, 201);
    }
    if (/^\/uploads\/[^/]+\/content$/.test(path)) {
      const id = path.split('/')[2]!; const admission = tickets.get(id)!;
      const bytes = request.postDataBuffer() ?? Buffer.alloc(0); sent.push(admission.name);
      return json({ upload_id: id, mutation_id: id, status: 'staged', length: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex'), published: false });
    }
    if (path.endsWith('/commit')) {
      const id = path.split('/')[2]!; const admission = tickets.get(id)!;
      expect(request.postDataJSON().parent_etag).toBe(folder(admission.parent_id).etag);
      active--;
      if (fault?.name === admission.name) {
        states.set(id, fault.outcome === 'uncertain' ? 'publishing' : 'rejected');
        return fault.outcome === 'uncertain' ? route.abort('failed') : json({ code: 'failed' }, 500);
      }
      if (admission.replacement) entries.splice(entries.findIndex(entry => entry.id === admission.replacement!.entry_id), 1);
      entries.push({ id, parent_id: admission.parent_id, name: admission.name, kind: 'file', etag: `"${id}:1"`, size: admission.length });
      bump(admission.parent_id); states.set(id, 'committed');
      return json({ status: 'committed' }, 201);
    }
    if (/^\/uploads\/[^/]+$/.test(path)) {
      const id = path.split('/')[2]!;
      if (request.method() === 'DELETE') { states.set(id, 'rejected'); active--; return route.fulfill({ status: 204 }); }
      return json({ state: states.get(id), status: states.get(id) });
    }
    return route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('article', { name: 'Destination', exact: true })).toBeVisible();
  return { events, entries, sent, maxActive: () => maxActive, resolve: (name: string, state: 'committed' | 'rejected') => {
    const [id, admission] = [...tickets.entries()].find(([, body]) => body.name === name)!;
    states.set(id, state);
    if (state === 'committed') {
      entries.push({ id, parent_id: admission.parent_id, name, kind: 'file', etag: `"${id}:1"`, size: admission.length });
      bump(admission.parent_id);
    }
  } };

}

async function desktopDrop(page: Page, target: Locator, sources: Source[], fallback = false, capture = false) {
  const dataTransfer = await page.evaluateHandle(({ sources, fallback }) => {
    const data = new DataTransfer();
    let protectedData = false;
    const entry = (source: Source): object => {
      if (source.batches) {
        return { name: source.name, isDirectory: true, isFile: false, createReader: () => {
          let batch = 0;
          return { readEntries: (success: (entries: object[]) => void, failure: (error: DOMException) => void) => {
            if (source.unreadable) return setTimeout(() => failure(new DOMException('No access', 'NotReadableError')));
            const children = source.batches![batch++] ?? [];
            setTimeout(() => success(children.map(entry)));
          } };
        } };
      }
      return { name: source.name, isDirectory: false, isFile: true, file: (success: (file: File) => void, failure: (error: DOMException) => void) => {
        setTimeout(() => source.unreadable ? failure(new DOMException('No access', 'NotReadableError')) : success(new File([source.content ?? source.name], source.name)));
      } };
    };
    if (fallback) {
      const files = sources.map(source => {
        const file = new File([source.content ?? source.name], source.name);
        if (source.unreadable) Object.defineProperty(file, 'arrayBuffer', { value: () => Promise.reject(new DOMException('No access', 'NotReadableError')) });
        return file;
      });
      files.forEach(file => data.items.add(file));
      if (sources.some(source => source.unreadable)) {
        Object.defineProperty(data, 'items', { value: files.map(file => ({ kind: 'file', getAsFile: () => file })) });
      }
    }
    else {
      Object.defineProperty(data, 'items', { value: sources.map(source => ({ kind: 'file', webkitGetAsEntry: () => {
        if (protectedData) throw new Error('DataTransfer was accessed after the event');
        return source.unavailable ? null : entry(source);
      }, getAsFile: () => null })) });
      Object.defineProperty(data, 'types', { value: ['Files'] });
      (data as DataTransfer & { protect: () => void }).protect = () => { protectedData = true; };
    }
    return data;
  }, { sources, fallback });
  await target.dispatchEvent('dragover', { dataTransfer });
  await expect(target).toHaveAttribute('data-drop-active', 'true');
  if (capture) await page.screenshot({ path: test.info().outputPath('desktop-drop-highlight.png'), fullPage: true });
  await target.evaluate((element, data) => {
    element.dispatchEvent(new DragEvent('drop', { dataTransfer: data, bubbles: true, cancelable: true }));
    (data as DataTransfer & { protect?: () => void }).protect?.();
  }, dataTransfer);
  await dataTransfer.dispose();
}

const current = (page: Page) => page.locator('[data-drop-target="current-folder"]');
const summary = (page: Page) => page.locator('.upload-batch-status');
const completed = (page: Page) => page.getByRole('status').filter({ hasText: 'Upload complete.' });

test('mixed desktop drops preserve nested and empty folders across directory batches', async ({ page }) => {
  const store = await backend(page);
  await desktopDrop(page, current(page), [
    { name: 'Album', batches: [[{ name: 'first.txt' }, { name: 'Nested', batches: [[{ name: 'inner.txt' }]] }], [{ name: 'last.txt' }]] },
    { name: 'Empty', batches: [] }, { name: 'loose.txt' },
  ], false, true);
  await expect(completed(page)).toBeAttached();
  const album = store.entries.find(entry => entry.name === 'Album')!;
  const nested = store.entries.find(entry => entry.name === 'Nested')!;
  expect(store.events).toEqual([
    { kind: 'folder', name: 'Album', parent: 'root' },
    { kind: 'file', name: 'first.txt', parent: album.id },
    { kind: 'folder', name: 'Nested', parent: album.id },
    { kind: 'file', name: 'inner.txt', parent: nested.id },
    { kind: 'file', name: 'last.txt', parent: album.id },
    { kind: 'folder', name: 'Empty', parent: 'root' },
    { kind: 'file', name: 'loose.txt', parent: 'root' },
  ]);
  expect(store.maxActive()).toBe(1);
  await expect(current(page)).toHaveAttribute('data-drop-active', 'false');
});

test('files fallback supports folder rows and Files sidebar as separate drop destinations', async ({ page }) => {
  const store = await backend(page);
  await desktopDrop(page, page.getByRole('article', { name: 'Destination', exact: true }), [{ name: 'row.txt', content: '' }], true);
  await expect(completed(page)).toBeAttached();
  await page.getByText('Results for 1 item', { exact: true }).click();
  await expect(page.locator('[data-upload-result="row.txt"]')).toContainText('Saved');
  await page.getByRole('button', { name: 'Open Destination', exact: true }).click();
  await desktopDrop(page, page.locator('[data-sidebar=menu-button][data-drop-target=root]'), [{ name: 'root.txt' }], true);
  await expect(completed(page)).toBeAttached();
  expect(store.events).toEqual([{ kind: 'file', name: 'row.txt', parent: 'destination' }, { kind: 'file', name: 'root.txt', parent: 'root' }]);
});

test('chooser uploads multiple files immediately and refreshes a nested current destination between files', async ({ page }) => {
  const store = await backend(page);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.getByRole('button', { name: 'Open Destination', exact: true }).click();
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await page.getByRole('button', { name: 'Open Inner', exact: true }).click();
  await page.getByLabel('Choose files to upload').setInputFiles([
    { name: 'one.txt', mimeType: 'text/plain', buffer: Buffer.from('one') },
    { name: 'two.txt', mimeType: 'text/plain', buffer: Buffer.from('two') },
  ]);
  await expect(completed(page)).toBeAttached();
  await expect(page.getByRole('button', { name: 'Upload', exact: true })).toHaveCount(0);
  expect(store.events).toEqual([{ kind: 'file', name: 'one.txt', parent: 'inner' }, { kind: 'file', name: 'two.txt', parent: 'inner' }]);
  await page.getByLabel('Choose files to upload').setInputFiles([]);
  expect(store.events).toHaveLength(2);
  await expect(completed(page)).toBeAttached();
  expect(errors).toEqual([]);
});

test('desktop conflict waits for replacement confirmation before bytes or later items', async ({ page }) => {
  const store = await backend(page);
  await desktopDrop(page, current(page), [{ name: 'same.txt' }, { name: 'after.txt' }]);
  await expect(page.getByRole('dialog')).toContainText('Replace same.txt?');
  expect(store.sent).toEqual([]);
  await page.getByRole('button', { name: 'Replace file', exact: true }).click();
  await expect(completed(page)).toBeAttached();
  expect(store.sent).toEqual(['same.txt', 'after.txt']);
});

test('declining replacement stops the batch without sending later files', async ({ page }) => {
  const store = await backend(page);
  await desktopDrop(page, current(page), [{ name: 'same.txt' }, { name: 'after.txt' }]);
  await page.getByRole('button', { name: 'Keep existing file', exact: true }).click();
  await expect(summary(page)).toContainText('Upload stopped at same.txt.');
  await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeEnabled();
  await expect(page.locator('[data-upload-result="same.txt"]')).toContainText('Canceled');
  await expect(page.locator('[data-upload-result="after.txt"]')).toContainText('Not attempted');
  expect(store.sent).toEqual([]);
});

for (const outcome of ['failed', 'uncertain'] as const) {
  test(`${outcome} upload stops later items and reports partial completion`, async ({ page }) => {
    const store = await backend(page, { name: 'broken.txt', outcome });
    await desktopDrop(page, current(page), [{ name: 'saved.txt' }, { name: 'broken.txt' }, { name: 'after.txt' }]);
    await expect(summary(page)).toContainText('Upload stopped at broken.txt.');
    await expect(page.locator('[data-upload-result="saved.txt"]')).toContainText('Saved');
    await expect(page.locator('[data-upload-result="broken.txt"]')).toContainText(outcome === 'uncertain' ? 'Outcome unknown' : 'Failed');
    await expect(page.locator('[data-upload-result="after.txt"]')).toContainText('Not attempted');
    expect(store.sent).toEqual(['saved.txt', 'broken.txt']);
    await expect(page.getByRole('article', { name: 'saved.txt', exact: true })).toBeVisible();
    if (outcome === 'failed') {
      await page.getByRole('button', { name: 'Account', exact: true }).click();
      await expect(page.locator('[data-upload-result="broken.txt"]')).toContainText('Failed');
      await page.getByRole('button', { name: 'Dismiss results', exact: true }).click();
      await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
    }
    if (outcome === 'uncertain') {
      await expect(page.getByText('Upload status needs checking')).toBeVisible();
      await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeDisabled();
      await expect(completed(page)).toHaveCount(0);
      await page.screenshot({ path: test.info().outputPath('desktop-upload-unknown-results.png'), fullPage: true });
      store.resolve('broken.txt', 'committed');
      await page.getByRole('button', { name: 'Check upload status', exact: true }).click();
      await expect(page.locator('[data-upload-result="broken.txt"]')).toContainText('Saved');
      await expect(summary(page)).toHaveText('Upload stopped.');
      await expect(page.locator('[data-upload-result="after.txt"]')).toContainText('Not attempted');
      expect(store.sent).toEqual(['saved.txt', 'broken.txt']);
    }
  });
}

test('unreadable directory stops before any storage changes', async ({ page }) => {
  const store = await backend(page);
  await desktopDrop(page, current(page), [{ name: 'good.txt' }, { name: 'Private', batches: [], unreadable: true }]);
  await expect(summary(page)).toContainText('could not be read. Nothing was uploaded.');
  expect(store.events).toEqual([]);
});

test('folder collision stops without merging into existing folders', async ({ page }) => {
  const store = await backend(page);
  await desktopDrop(page, current(page), [{ name: 'Destination', batches: [[{ name: 'inside.txt' }]] }, { name: 'after.txt' }]);
  await expect(summary(page)).toContainText('An item already has this name. Rename it before trying again.');
  expect(store.sent).toEqual([]);
  expect(store.events).toEqual([{ kind: 'folder', name: 'Destination', parent: 'root' }]);
});

test('lost folder response states uncertainty and stops later items', async ({ page }) => {
  const store = await backend(page, { name: 'New folder', outcome: 'folder-uncertain' });
  await desktopDrop(page, current(page), [{ name: 'New folder', batches: [] }, { name: 'after.txt' }]);
  await expect(summary(page)).toContainText('The last item may have been added. Refresh your files before retrying it.');
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await page.getByRole('button', { name: 'Dismiss results', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Upload results', exact: true })).toHaveCount(0);
  expect(store.sent).toEqual([]);
  await page.getByRole('button', { name: 'Files', exact: true }).click();
  await expect(page.getByRole('article', { name: 'New folder', exact: true })).toBeVisible();
});

test('cancel during saving retains the confirmed file and stops the remaining queue', async ({ page }) => {
  const store = await backend(page);
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  let saving!: () => void;
  const savingStarted = new Promise<void>(resolve => { saving = resolve; });
  await page.route('**/uploads/*/commit', async route => { saving(); await gate; return route.fallback(); });
  await desktopDrop(page, current(page), [{ name: 'saving.txt' }, { name: 'after.txt' }]);
  await savingStarted;
  await expect(page.getByRole('status', { name: 'Uploading', exact: true })).toBeVisible();
  await expect(summary(page)).toHaveCount(0);
  await expect(page.locator('.upload-batch-report > details')).toHaveCount(0);
  await expect(page.locator('.upload-status-line')).toHaveCount(0);
  await page.screenshot({ path: test.info().outputPath('desktop-upload-saving.png'), fullPage: true });
  await page.getByRole('button', { name: 'Cancel upload', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Cancel upload', exact: true })).toBeDisabled();
  await expect(page.getByRole('status', { name: 'Uploading', exact: true })).toContainText('Stopping after this save');
  release();
  await expect(summary(page)).toContainText('Upload stopped at saving.txt.');
  await expect(page.locator('[data-upload-result="saving.txt"]')).toContainText('Saved');
  await expect(page.locator('[data-upload-result="after.txt"]')).toContainText('Not attempted');
  await page.screenshot({ path: test.info().outputPath('desktop-upload-canceled-results.png'), fullPage: true });
  expect(store.sent).toEqual(['saving.txt']);
});


test('cancel while admission is pending does not reopen a late replacement prompt', async ({ page }) => {
  const store = await backend(page);
  let release!: () => void;
  let started!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const pending = new Promise<void>(resolve => { started = resolve; });
  await page.route('**/uploads', async route => { started(); await gate; return route.fallback(); });
  await desktopDrop(page, current(page), [{ name: 'same.txt' }, { name: 'after.txt' }]);
  await pending;
  await page.getByRole('button', { name: 'Cancel upload', exact: true }).click();
  release();
  await expect(summary(page)).toContainText('Upload stopped at same.txt.');
  await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeEnabled();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(store.sent).toEqual([]);
});


test('unreadable zero-byte fallback is rejected before storage changes', async ({ page }) => {
  const store = await backend(page);
  await desktopDrop(page, current(page), [{ name: 'Unreadable', content: '', unreadable: true }], true);
  await expect(summary(page)).toContainText('could not be read. Nothing was uploaded.');
  expect(store.events).toEqual([]);
});


test('unavailable drop errors and prior results survive the next batch', async ({ page }) => {
  const store = await backend(page);
  await desktopDrop(page, current(page), [{ name: 'saved.txt' }]);
  await expect(completed(page)).toBeAttached();
  await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeEnabled();
  await desktopDrop(page, current(page), [{ name: 'unavailable', unavailable: true }]);
  await expect(summary(page)).toContainText('A dropped item could not be read.');
  await expect(page.getByRole('region', { name: 'Upload results', exact: true }).locator('[data-upload-result]')).toHaveCount(0);
  await expect(page.locator('.workspace-notice')).toHaveCount(0);
  expect(store.sent).toEqual(['saved.txt']);
  await desktopDrop(page, current(page), [{ name: 'next.txt' }]);
  await expect(completed(page)).toBeAttached();
  const previous = page.getByRole('region', { name: 'Previous upload results', exact: true });
  await expect(previous).toHaveCount(2);
  await expect(previous.filter({ hasText: 'A dropped item could not be read.' })).toBeVisible();
});

test('confirmed uploads replace a quiet spinner with one compact accessible summary', async ({ page }) => {
  await backend(page);
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  await page.route('**/uploads/*/commit', async route => { await gate; return route.fallback(); });
  await desktopDrop(page, current(page), [{ name: 'compact.txt' }]);
  await expect(page.getByRole('status', { name: 'Uploading', exact: true }).locator('svg')).toBeVisible();
  await expect(summary(page)).toHaveCount(0);
  await expect(page.locator('.upload-batch-report > details')).toHaveCount(0);
  await expect(page.locator('.upload-pending .sr-only')).toBeAttached();
  await expect(page.getByRole('progressbar')).toHaveCount(0);
  release();
  await expect(completed(page)).toBeAttached();
  await expect(page.locator('.upload-pending')).toHaveCount(0);
  const report = page.getByRole('region', { name: 'Upload results', exact: true });
  await expect(report.locator('summary')).toHaveText('Upload complete. Results for 1 item');
  await expect(report.locator('summary svg')).toBeVisible();
  await expect(report.locator('details')).not.toHaveAttribute('open');
  await expect(summary(page)).toHaveCount(0);
  await expect(page.locator('.upload-status')).toHaveCount(0);
  await expect(page.locator('.workspace-notice')).toHaveCount(0);
  await report.locator('summary').click();
  await expect(page.locator('[data-upload-result="compact.txt"]')).toContainText('Saved');
});

test('completed results survive folder, view, search and page changes until explicit dismissal', async ({ page }) => {
  const store = await backend(page);
  const report = page.getByRole('region', { name: 'Upload results', exact: true });
  await desktopDrop(page, current(page), [{ name: 'retained.txt' }]);
  await expect(completed(page)).toBeAttached();
  await page.getByRole('button', { name: 'Open Destination', exact: true }).click();
  await expect(report).toBeVisible();
  await page.goBack();
  await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await expect(report).toBeVisible();
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await expect(report).toBeVisible();
  await page.getByRole('button', { name: 'Files', exact: true }).click();
  await page.getByLabel('Search files and folders').fill('missing');
  await expect(report).toBeVisible();
  await page.getByRole('button', { name: 'Clear search', exact: true }).click();
  await page.route('**/folders/root/children*', route => route.fulfill({ json: { items: store.entries.filter(entry => entry.parent_id === 'root'), next_cursor: new URL(route.request().url()).searchParams.has('cursor') ? null : 'more', has_more: !new URL(route.request().url()).searchParams.has('cursor') } }));
  await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(report).toBeVisible();
  await page.getByRole('button', { name: 'Previous page', exact: true }).click();
  await page.getByRole('button', { name: 'Dismiss results', exact: true }).click();
  await expect(report).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeFocused();
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await expect(report).toHaveCount(0);
});

test('results from two batches stay independently dismissible and clear on signout', async ({ page }) => {
  await backend(page);
  await desktopDrop(page, current(page), [{ name: 'first.txt' }]);
  await expect(completed(page)).toBeAttached();
  await desktopDrop(page, current(page), [{ name: 'second.txt' }]);
  await expect(completed(page)).toBeAttached();
  const previous = page.getByRole('region', { name: 'Previous upload results', exact: true });
  const latest = page.getByRole('region', { name: 'Upload results', exact: true });
  await previous.locator('summary').click();
  await expect(previous.locator('[data-upload-result="first.txt"]')).toContainText('Saved');
  await latest.locator('summary').click();
  await expect(latest.locator('[data-upload-result="second.txt"]')).toContainText('Saved');
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await previous.getByRole('button', { name: 'Dismiss results', exact: true }).click();
  await expect(previous).toHaveCount(0);
  await expect(latest).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
  await page.getByRole('button', { name: 'Log out', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password').fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeVisible();
  await expect(latest).toHaveCount(0);
  await expect(previous).toHaveCount(0);
});

test('navigation retains unknown results and global reconciliation controls', async ({ page }) => {
  const store = await backend(page, { name: 'unknown.txt', outcome: 'uncertain' });
  await desktopDrop(page, current(page), [{ name: 'unknown.txt' }, { name: 'after.txt' }]);
  await expect(page.locator('[data-upload-result="unknown.txt"]')).toContainText('Outcome unknown');
  await page.getByRole('button', { name: 'Open Destination', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Upload results', exact: true })).toBeVisible();
  await expect(page.getByText('Upload status needs checking')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await expect(page.getByText('Upload status needs checking')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Dismiss results', exact: true })).toHaveCount(0);
  let unavailable = true;
  await page.route('**/uploads/*', route => unavailable && route.request().method() === 'GET' ? route.abort('failed') : route.fallback());
  await page.getByRole('button', { name: 'Check upload status', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Upload results', exact: true })).toContainText('Upload status is unavailable. Try checking again before starting another upload.');
  unavailable = false;
  await page.getByRole('button', { name: 'Check upload status', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Upload results', exact: true })).toContainText('Upload is admitted or still processing.');
  store.resolve('unknown.txt', 'committed');
  await page.getByRole('button', { name: 'Check upload status', exact: true }).click();
  await expect(page.getByText('Upload status needs checking')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Dismiss results', exact: true })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Upload results', exact: true })).toBeVisible();
  expect(store.sent).toEqual(['unknown.txt']);
});

test('navigation preserves the active queue and its completed report', async ({ page }) => {
  const store = await backend(page);
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  let saving!: () => void;
  const savingStarted = new Promise<void>(resolve => { saving = resolve; });
  await page.route('**/uploads/*/commit', async route => { saving(); await gate; return route.fallback(); });
  await desktopDrop(page, current(page), [{ name: 'first.txt' }, { name: 'second.txt' }]);
  await savingStarted;
  await expect(page.getByRole('status', { name: 'Uploading', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Open Destination', exact: true }).click();
  await expect(page.locator('.upload-batch-report > details')).toHaveCount(0);
  await expect(page.getByRole('status', { name: 'Uploading', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Cancel upload', exact: true })).toBeVisible();
  release();
  await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeEnabled();
  await expect(page.getByRole('region', { name: 'Upload results', exact: true })).toBeVisible();
  expect(store.sent).toEqual(['first.txt', 'second.txt']);
  expect(store.events.every(event => event.parent === 'root')).toBe(true);
});

test('drop targets are rounded and confined to the listing, with outside drops blocked', async ({ page }) => {
  const store = await backend(page);
  const region = current(page);
  await expect(region.getByRole('heading')).toHaveCount(0);
  await expect(region.getByLabel('Search files and folders')).toHaveCount(0);
  await expect(region.getByRole('button', { name: 'Choose files', exact: true })).toHaveCount(0);
  const transfer = await page.evaluateHandle(() => { const transfer = new DataTransfer(); transfer.items.add(new File(['x'], 'outside.txt')); return transfer; });
  await page.locator('.page-heading').dispatchEvent('dragover', { dataTransfer: transfer });
  await expect(region).toHaveAttribute('data-drop-active', 'false');
  expect(await page.locator('.page-heading').evaluate((element, dataTransfer) => !element.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer })), transfer)).toBe(true);
  expect(store.events).toEqual([]);
  await region.dispatchEvent('dragover', { dataTransfer: transfer });
  await expect(region).toHaveAttribute('data-drop-active', 'true');
  for (const target of [region, page.getByRole('article', { name: 'Destination', exact: true }), page.locator('[data-sidebar=menu-button][data-drop-target=root]')]) {
    await target.dispatchEvent('dragover', { dataTransfer: transfer });
    expect(await target.evaluate(element => parseFloat(getComputedStyle(element).borderRadius))).toBeGreaterThan(0);
  }
  await transfer.dispose();
});

// Keep actual XHR requests, but control progress events independently of local network speed.
async function controlUploadProgress(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const send = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.send = function (body) {
      const progress = this.upload.onprogress;
      if (progress && body instanceof Blob) {
        const upload = this.upload;
        const size = body.size;
        (window as unknown as { emitUploadProgress: (sent: number) => void }).emitUploadProgress = (sent) => progress.call(upload, new ProgressEvent('progress', { loaded: sent, total: size, lengthComputable: true }));
        this.upload.onprogress = null;
      }
      return send.call(this, body);
    };
  });
}

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>(done => { resolve = done; });
  return { promise, resolve };
}

test('60 mixed-size folder files keep byte progress across transfers and distinguish sending from saving', async ({ page }) => {
  await controlUploadProgress(page);
  const store = await backend(page);
  const first = deferred(); const second = deferred(); const last = deferred();
  let contentIndex = 0; let commitIndex = 0;
  await page.route('**/uploads/*/content', async route => {
    if (++contentIndex === 2) await second.promise;
    return route.fallback();
  });
  await page.route('**/uploads/*/commit', async route => {
    const index = ++commitIndex;
    if (index === 1) await first.promise;
    if (index === 60) await last.promise;
    return route.fallback();
  });
  const folderName = 'Trip photographs and documents with a long folder name';
  const files = Array.from({ length: 60 }, (_, index) => ({ name: `file-${String(index).padStart(2, '0')}.txt`, content: 'x'.repeat((index === 0 ? 10 : index === 1 ? 100 : 1) * 1024) }));
  await desktopDrop(page, current(page), [{ name: folderName, batches: [files] }]);
  const progress = page.getByRole('progressbar', { name: 'Batch bytes sent', exact: true });
  const panel = page.locator('.upload-batch-progress');
  await expect(panel).toContainText('Saving:');
  await expect(panel).toContainText('0 of 60 files saved');
  await expect(panel).toContainText('1 folder created');
  await expect(progress).toHaveAttribute('aria-valuenow', '5');
  const height = await panel.evaluate(element => element.getBoundingClientRect().height);
  first.resolve();
  await expect(panel).toContainText('1 of 60 files saved');
  await expect(panel).toContainText('Sending:');
  await expect(progress).toHaveAttribute('aria-valuenow', '5');
  await page.evaluate(() => (window as unknown as { emitUploadProgress: (sent: number) => void }).emitUploadProgress(50 * 1024));
  await expect(progress).toHaveAttribute('aria-valuenow', '35');
  await page.evaluate(() => (window as unknown as { emitUploadProgress: (sent: number) => void }).emitUploadProgress(10 * 1024));
  await expect(progress).toHaveAttribute('aria-valuenow', '35');
  await expect(panel).toContainText('60 KB of 168 KB sent');
  await expect(page.getByRole('status', { name: 'Uploading', exact: true })).toContainText('1 of 60 files saved. At least 30% sent.');
  await expect(panel.locator('.upload-current-path')).toHaveText('file-01.txt');
  await expect(panel.locator('.upload-current-path')).toHaveAttribute('title', `${folderName}/file-01.txt`);
  await expect(panel).toContainText(`To Files / ${folderName}`);
  await expect(panel).toContainText('To Files');
  expect(await panel.evaluate(element => element.getBoundingClientRect().height)).toBe(height);
  await expect(page.locator('.upload-batch-report > details')).toHaveCount(0);
  await page.screenshot({ path: test.info().outputPath('batch-progress-desktop.png'), fullPage: true, animations: 'disabled' });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.locator('header')).toHaveCount(1);
  await expect(page.locator('#main-content')).toHaveCount(1);
  await expect(page.getByRole('heading', { name: 'Files', exact: true })).toHaveCount(1);
  await page.getByRole('button', { name: 'Switch to dark mode', exact: true }).click();
  await page.getByRole('button', { name: 'Switch to light mode', exact: true }).click();
  await page.screenshot({ path: test.info().outputPath('batch-progress-mobile.png'), animations: 'disabled' });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole('button', { name: 'Switch to dark mode', exact: true }).click();
  await page.screenshot({ path: test.info().outputPath('batch-progress-dark-mobile.png'), animations: 'disabled' });
  second.resolve();
  await expect(panel).toContainText('59 of 60 files saved');
  await expect(panel).toContainText('Saving:');
  await expect(progress).toHaveAttribute('aria-valuenow', '100');
  await expect(completed(page)).toHaveCount(0);
  last.resolve();
  await expect(completed(page)).toBeAttached();
  await expect(progress).toHaveCount(0);
  expect(store.sent).toHaveLength(60);
  expect(store.maxActive()).toBe(1);
});

test('all-zero-byte files show sending separately from confirmed saving without invalid percentages', async ({ page }) => {
  await backend(page);
  const gate = deferred();
  await page.route('**/uploads/*/commit', async route => { await gate.promise; return route.fallback(); });
  await page.getByLabel('Choose files to upload').setInputFiles([
    { name: 'empty-one.txt', mimeType: 'text/plain', buffer: Buffer.alloc(0) },
    { name: 'empty-two.txt', mimeType: 'text/plain', buffer: Buffer.alloc(0) },
  ]);
  const panel = page.locator('.upload-batch-progress');
  await expect(panel).toContainText('No file bytes to send');
  await expect(panel).toContainText('0 of 2 files saved');
  await expect(panel).toContainText('Saving:');
  await expect(page.getByRole('progressbar', { name: 'Batch files sent', exact: true })).toHaveAttribute('aria-valuenow', '50');
  gate.resolve();
  await expect(completed(page)).toBeAttached();
});

test('all bytes sent never implies completion when the final save outcome is unknown', async ({ page }) => {
  const store = await backend(page, { name: 'unknown.txt', outcome: 'uncertain' });
  const gate = deferred(); let commits = 0;
  await page.route('**/uploads/*/commit', async route => { if (++commits === 2) await gate.promise; return route.fallback(); });
  await desktopDrop(page, current(page), [{ name: 'saved.txt' }, { name: 'unknown.txt' }]);
  await expect(page.getByRole('progressbar', { name: 'Batch bytes sent', exact: true })).toHaveAttribute('aria-valuenow', '100');
  await expect(page.locator('.upload-batch-progress')).toContainText('1 of 2 files saved');
  await expect(page.locator('.upload-batch-progress')).toContainText('Saving:');
  await expect(completed(page)).toHaveCount(0);
  gate.resolve();
  await expect(page.locator('[data-upload-result="saved.txt"]')).toContainText('Saved');
  await expect(page.locator('[data-upload-result="unknown.txt"]')).toContainText('Outcome unknown');
  await expect(completed(page)).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeDisabled();
  expect(store.sent).toEqual(['saved.txt', 'unknown.txt']);
});

test('confirmed save counts update while the post-commit listing refresh is held', async ({ page }) => {
  const store = await backend(page);
  const gate = deferred();
  await page.route('**/folders/children*', async route => {
    if (route.request().method() === 'GET' && store.entries.some(entry => entry.name === 'confirmed.txt')) await gate.promise;
    return route.fallback();
  });
  await desktopDrop(page, current(page), [{ name: 'confirmed.txt' }, { name: 'after.txt' }]);
  const panel = page.locator('.upload-batch-progress');
  await expect(panel).toContainText('1 of 2 files saved');
  await expect(panel).toContainText('Refreshing files:');
  await expect(panel.locator('.upload-current-path')).toHaveText('confirmed.txt');
  await expect(page.getByRole('status', { name: 'Uploading', exact: true })).toContainText('1 of 2 files saved');
  expect(store.sent).toEqual(['confirmed.txt']);
  gate.resolve();
  await expect(completed(page)).toBeAttached();
});

test('cancel acknowledges a pending cancellation response and disables repeated cancellation', async ({ page }) => {
  await backend(page);
  const content = deferred(); const cancel = deferred(); const cancelStarted = deferred();
  await page.route('**/uploads/*/content', async route => { await content.promise; return route.fallback(); });
  await page.route('**/uploads/*', async route => {
    if (route.request().method() === 'DELETE') { cancelStarted.resolve(); await cancel.promise; }
    return route.fallback();
  });
  await desktopDrop(page, current(page), [{ name: 'cancel.txt' }, { name: 'after.txt' }]);
  await expect(page.locator('.upload-batch-progress')).toContainText('Sending:');
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await page.getByRole('button', { name: 'Cancel upload', exact: true }).click();
  await cancelStarted.promise;
  await expect(page.getByRole('status', { name: 'Uploading', exact: true })).toContainText('Canceling current upload');
  await expect(page.getByRole('button', { name: 'Cancel upload', exact: true })).toBeDisabled();
  await expect(completed(page)).toHaveCount(0);
  cancel.resolve(); content.resolve();
  await expect(page.locator('[data-upload-result="cancel.txt"]')).toContainText('Canceled');
  await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
  await expect(page.locator('[data-upload-result="after.txt"]')).toContainText('Not attempted');
});

test.describe('touch batch locations', () => {
  test.use({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 844 } });
  test('full filename suffix and destination are available without hover', async ({ page }) => {
    await backend(page);
    const gate = deferred();
    await page.route('**/uploads/*/commit', async route => { await gate.promise; return route.fallback(); });
    await page.getByRole('button', { name: 'Open Destination', exact: true }).click();
    const folder = 'An exceptionally long project destination containing multiple document types';
    const filename = 'A long filename whose distinguishing final suffix must stay available on a touch screen.final-receipt.pdf';
    await desktopDrop(page, current(page), [{ name: folder, batches: [[{ name: filename }, { name: 'after.txt' }]] }]);
    const details = page.locator('.upload-location-details');
    await expect(details).not.toHaveAttribute('open');
    expect(await details.locator('summary').evaluate(element => element.getBoundingClientRect().height)).toBeGreaterThanOrEqual(44);
    await details.locator('summary').focus();
    await expect(details.locator('summary')).toBeFocused();
    await details.locator('summary').tap();
    await expect(details).toHaveAttribute('open', '');
    await expect(details.locator('dd').nth(0)).toHaveText(`${folder}/${filename}`);
    await expect(details.locator('dd').nth(1)).toHaveText(`Destination / ${folder}`);
    await expect(details.locator('dd').nth(0)).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await details.locator('summary').tap();
    await expect(details).not.toHaveAttribute('open');
    gate.resolve();
    await expect(completed(page)).toBeAttached();
  });
});


test('single file sending and saving stay visible across account, trash and search', async ({ page }) => {
  const store = await backend(page);
  const content = deferred(); const saving = deferred();
  await page.route('**/uploads/*/content', async route => { await content.promise; return route.fallback(); });
  await page.route('**/uploads/*/commit', async route => { await saving.promise; return route.fallback(); });
  await page.getByRole('button', { name: 'Open Destination', exact: true }).click();
  await desktopDrop(page, current(page), [{ name: 'single-long-filename-with-a-distinctive-suffix.txt' }]);
  const activity = page.getByRole('status', { name: 'Uploading', exact: true });
  await expect(activity).toContainText('Sending:');
  await expect(activity).toContainText('single-long-filename-with-a-distinctive-suffix.txt');
  await expect(activity).toContainText('To Destination');
  await expect(page.getByRole('progressbar')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Dismiss results', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await expect(activity).toContainText('Sending:');
  content.resolve();
  await expect(activity).toContainText('Saving:');
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await expect(activity).toContainText('Saving:');
  await page.getByRole('button', { name: 'Files', exact: true }).click();
  await page.getByLabel('Search files and folders').fill('missing');
  await expect(activity).toContainText('To Destination');
  await page.screenshot({ path: test.info().outputPath('single-activity-desktop-light.png'), fullPage: true });
  await page.getByRole('button', { name: 'Switch to dark mode', exact: true }).click();
  await page.screenshot({ path: test.info().outputPath('single-activity-desktop-dark.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  await expect(page.locator('header')).toHaveCount(1);
  await expect(page.locator('header')).toBeInViewport();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: test.info().outputPath('single-activity-mobile-dark.png'), fullPage: true, animations: 'disabled' });
  await page.getByRole('button', { name: 'Switch to light mode', exact: true }).click();
  await page.screenshot({ path: test.info().outputPath('single-activity-mobile-light.png'), fullPage: true });
  saving.resolve();
  await expect(completed(page)).toBeAttached();
  await expect(page.getByRole('button', { name: 'Dismiss results', exact: true })).toBeVisible();
  expect(store.events).toEqual([{ kind: 'file', name: 'single-long-filename-with-a-distinctive-suffix.txt', parent: 'destination' }]);
});


test('a later active batch preserves the prior failed batch labels', async ({ page }) => {
  await backend(page, { name: 'broken.txt', outcome: 'failed' });
  await desktopDrop(page, current(page), [{ name: 'broken.txt' }, { name: 'not-attempted.txt' }]);
  await expect(page.locator('[data-upload-result="not-attempted.txt"]')).toContainText('Not attempted');
  const saving = deferred();
  await page.route('**/uploads/*/commit', async route => { await saving.promise; return route.fallback(); });
  await desktopDrop(page, current(page), [{ name: 'next.txt' }]);
  await expect(page.getByRole('status', { name: 'Uploading', exact: true })).toContainText('Saving:');
  const previous = page.getByRole('region', { name: 'Previous upload results', exact: true });
  await expect(previous.locator('[data-upload-result="not-attempted.txt"]')).toContainText('Not attempted');
  await expect(previous.locator('[data-upload-result="broken.txt"]')).toContainText('Failed');
  await expect(previous).toContainText('Upload failed. No saved file is implied.');
  saving.resolve();
  await expect(completed(page)).toBeAttached();
});
