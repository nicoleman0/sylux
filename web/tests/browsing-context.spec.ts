import { expect, test, type Page } from '@playwright/test';
import type { Entry, Folder } from '../src/lib/listing';
import { parseLocation, savedContext } from '../src/lib/browsing-context';

const root: Folder = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
const child: Folder = { id: 'projects', parent_id: 'root', name: 'Projects', kind: 'folder', etag: '"projects:1"' };
const file = (n: number): Entry => ({ id: `file-${n}`, parent_id: 'projects', name: `Match ${n}.txt`, kind: 'file', size: 5, etag: '"file:1"' });

async function backend(page: Page, administrator = false) {
  const requests: URL[] = [];
  let stale: 400 | 409 | null = null;
  let missing = false;
  await page.route('**/*', async route => {
    const url = new URL(route.request().url());
    const json = (value: unknown) => route.fulfill({ json: value });
    if (url.pathname === '/instance') return json({ name: 'Sylux' });
    if (url.pathname === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (url.pathname === '/auth/local') {
      const handle = route.request().postDataJSON().handle;
      return json({ account_id: handle, handle, role: administrator ? 'administrator' : 'user', csrf_token: 'csrf' });
    }
    if (url.pathname === '/accounts') return json([{ account_id: 'member', handle: 'member', role: 'user', enabled: true }]);
    if (url.pathname === '/auth/logout') return json({});
    if (url.pathname === '/folders/children') return json({ root, items: [child], next_cursor: null, has_more: false });
    if (/^\/folders\/[^/]+\/path$/.test(url.pathname)) {
      requests.push(url);
      if (missing || !url.pathname.includes('/projects/')) return route.fulfill({ status: 404, json: { code: 'not_found' } });
      return json({ items: [root, child] });
    }
    if (/^\/folders\/[^/]+\/children$/.test(url.pathname) || url.pathname === '/entries/search') {
      requests.push(url);
      const cursor = url.searchParams.get('cursor');
      if (cursor && stale) {
        const status = stale; stale = null;
        return route.fulfill({ status, json: { code: status === 400 ? 'invalid_cursor' : 'listing_changed' } });
      }
      const items = Array.from({ length: 100 }, (_, index) => file(index + (cursor ? 100 : 0)));
      const search = url.pathname === '/entries/search';
      return json({ items: search ? items.map(entry => ({ entry, location: [root, child] })) : url.pathname.includes('/root/') ? [child, ...items] : items, next_cursor: cursor ? null : 'page-two', has_more: !cursor });
    }
    if (/^\/entries\/[^/]+\/content$/.test(url.pathname)) return route.fulfill({ contentType: 'text/plain', body: 'hello' });
    if (url.pathname === '/trash') return json({ items: [] });
    return route.continue();
  });
  return { requests, stale: (status: 400 | 409) => { stale = status; }, missing: () => { missing = true; } };
}

async function signIn(page: Page, owner = 'owner', saved = true) {
  await page.getByLabel('Account name', { exact: true }).fill(owner);
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
  if (saved) await expect.poll(() => page.evaluate(() => history.state?.sylux?.owner)).toBe(owner);
}

async function scroll(page: Page) {
  await page.evaluate(() => window.scrollTo(0, 1400));
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(1400);
  await expect.poll(() => page.evaluate(() => history.state?.sylux?.scroll)).toBe(1400);
}

for (const theme of ['light', 'dark']) {
  for (const width of [1280, 390]) {
    test(`search containing-folder Back restores page and document scroll at ${width}px in ${theme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 844 });
      await page.emulateMedia({ colorScheme: theme as 'light' | 'dark' });
      await backend(page); await page.goto('/'); await signIn(page);
      await page.getByLabel('Sort files by', { exact: true }).selectOption('size_desc');
      await page.getByLabel('Search files and folders').fill('Match');
      await expect(page.getByRole('region', { name: 'Search results' })).toBeVisible();
      await page.getByRole('button', { name: 'Next page', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Preview Match 100.txt', exact: true })).toBeVisible();
      await page.getByLabel('Select Match 100.txt in Files / Projects', { exact: true }).check();
      await scroll(page);
      await page.getByRole('button', { name: 'Open containing folder for Match 120.txt', exact: true }).evaluate(element => (element as HTMLElement).click());
      await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
      await page.goBack();
      await expect(page.getByLabel('Search files and folders')).toHaveValue('Match');
      await expect(page.getByRole('button', { name: 'Preview Match 100.txt', exact: true })).toBeVisible();
      await expect(page.getByRole('group', { name: 'Selected items', exact: true })).toHaveCount(0);
      await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(1400);
      await page.screenshot({ path: `/tmp/sylux-context-${width}-${theme}.png`, fullPage: false });
      await page.goForward();
      await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
      await expect(page.getByLabel('Sort files by', { exact: true })).toHaveValue('size_desc');
    });
  }
}

test('reload restores signed-in owner page after authentication; copied URL starts page one', async ({ page }) => {
  const fixture = await backend(page); await page.goto('/#files/projects?sort=modified_desc'); await signIn(page);
  await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
  expect(fixture.requests.some(url => url.pathname === '/folders/projects/path')).toBeTruthy();
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Preview Match 100.txt', exact: true })).toBeVisible();
  await scroll(page);
  await page.reload(); await signIn(page);
  await expect(page.getByRole('button', { name: 'Preview Match 100.txt', exact: true })).toBeVisible();
  await expect(page.getByLabel('Sort files by', { exact: true })).toHaveValue('modified_desc');
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(1400);
  const copiedPage = await page.context().newPage();
  const copiedFixture = await backend(copiedPage);
  await copiedPage.goto(page.url());
  expect(await copiedPage.evaluate(() => history.state)).toBeNull();
  await signIn(copiedPage);
  await expect(copiedPage.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
  await expect(copiedPage.getByRole('button', { name: 'Preview Match 0.txt', exact: true })).toBeVisible();
  await expect(copiedPage.getByLabel('Search files and folders')).toHaveValue('');
  await expect(copiedPage.getByLabel('Sort files by', { exact: true })).toHaveValue('modified_desc');
  expect(copiedFixture.requests.at(-1)?.searchParams.get('cursor')).toBeNull();
  expect(copiedFixture.requests.at(-1)?.searchParams.get('sort')).toBe('modified_desc');
  await copiedPage.close();
});

for (const status of [400, 409] as const) {
  test(`stale restored cursor ${status} safely resets with notice`, async ({ page }) => {
    const fixture = await backend(page); await page.goto('/'); await signIn(page);
    await page.getByRole('button', { name: 'Next page', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Preview Match 100.txt', exact: true })).toBeVisible();
    await scroll(page);
    await page.getByRole('button', { name: 'Open Projects', exact: true }).evaluate(element => (element as HTMLElement).click());
    fixture.stale(status); await page.goBack();
    await expect(page.getByText('This folder changed. The listing has been restarted.', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Preview Match 0.txt', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toHaveCount(0);
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(0);
  });
}

test('unavailable deep link gives safe Files recovery and foreign owner resets context', async ({ page }) => {
  const fixture = await backend(page); fixture.missing();
  await page.goto('/#files/projects?q=private'); await signIn(page, 'owner', false);
  await expect(page.getByText('This folder is unavailable. Open Files to continue.', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Files', exact: true }).first().click();
  await expect(page.getByRole('button', { name: 'Preview Match 0.txt', exact: true })).toBeVisible();
  await page.getByLabel('Search files and folders').fill('private');
  await expect.poll(() => page.evaluate(() => history.state?.sylux?.query)).toBe('private');
  await page.reload(); await signIn(page, 'other');
  await expect(page.getByLabel('Search files and folders')).toHaveValue('');
  await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeVisible();
});

test('malformed URL and saved cursors are bounded and fail closed', () => {
  expect(parseLocation('#files/%ZZ?q=x').folder).toBe('');
  expect(parseLocation('#files/root?q=%00&sort=bogus')).toEqual({ destination: 'files', folder: 'root', query: '', sort: 'name_asc' });
  const location = parseLocation('#files/root');
  const state = { ...location, version: 1, owner: 'owner', cursors: [null], page: 0, searchCursors: [null], searchPage: 0, scroll: 20 };
  expect(savedContext(state, 'other', location)).toBeNull();
  expect(savedContext({ ...state, page: 20 }, 'owner', location)).toBeNull();
  expect(savedContext(state, 'owner', location)?.scroll).toBe(20);
  expect(savedContext({ ...state, folder: { invalid: true } }, 'owner', location)).toBeNull();
  expect(savedContext({ ...state, query: [] }, 'owner', location)).toBeNull();
  expect(savedContext({ ...state, destination: 'foreign' }, 'owner', location)).toBeNull();
});

test('preview close retains page and scroll; browser Back closes preview and restores folder', async ({ page }) => {
  await backend(page); await page.goto('/'); await signIn(page);
  await page.getByRole('button', { name: 'Open Projects', exact: true }).click();
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Preview Match 100.txt', exact: true })).toBeVisible();
  await scroll(page);
  const trigger = page.getByRole('button', { name: 'Preview Match 120.txt', exact: true });
  await trigger.evaluate(element => (element as HTMLElement).click());
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(1400);
  await trigger.evaluate(element => (element as HTMLElement).click());
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.goBack();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Preview Match 0.txt', exact: true })).toBeVisible();
});

test('late restored path cannot replace a newer destination or focus', async ({ page }) => {
  await backend(page); await page.goto('/'); await signIn(page);
  await page.getByRole('button', { name: 'Open Projects', exact: true }).click();
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  let release!: () => void;
  let requested!: () => void;
  const pending = new Promise<void>(resolve => { requested = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  await page.route('**/folders/projects/path', async route => { requested(); await gate; await route.fulfill({ json: { items: [root, { ...child, name: 'Late folder' }] } }); });
  await page.goBack(); await pending;
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
  const response = page.waitForResponse('**/folders/projects/path');
  release(); await response;
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  await expect(page.getByRole('heading', { name: 'Late folder', exact: true })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
});

for (const status of [400, 409] as const) {
  test(`stale restored search cursor ${status} resets results and selection`, async ({ page }) => {
    const fixture = await backend(page); await page.goto('/'); await signIn(page);
    await page.getByLabel('Search files and folders').fill('Match');
    await page.getByRole('button', { name: 'Next page', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Preview Match 100.txt', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Open containing folder for Match 100.txt', exact: true }).click();
    fixture.stale(status); await page.goBack();
    await expect(page.getByText('Files changed. Search results have been restarted.', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Preview Match 0.txt', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Previous page', exact: true })).toHaveCount(0);
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(0);
  });
}

test('late restoration from an old login cannot render or focus in another account', async ({ page }) => {
  await backend(page); await page.goto('/'); await signIn(page);
  await page.getByRole('button', { name: 'Open Projects', exact: true }).click();
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  let release!: () => void;
  let requested!: () => void;
  const pending = new Promise<void>(resolve => { requested = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  await page.route('**/folders/projects/path', async route => { requested(); await gate; await route.fulfill({ json: { items: [root, { ...child, name: 'Old account folder' }] } }); });
  await page.goBack(); await pending;
  await page.getByRole('button', { name: 'Log out', exact: true }).click();
  await signIn(page, 'other');
  const response = page.waitForResponse('**/folders/projects/path');
  release(); await response;
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  await expect(page.getByRole('heading', { name: 'Old account folder', exact: true })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeFocused();
  await expect(page.getByLabel('Search files and folders')).toHaveValue('');
});

test('clearing a restored search fetches its underlying saved browsing page', async ({ page }) => {
  const fixture = await backend(page); await page.goto('/'); await signIn(page);
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Preview Match 100.txt', exact: true })).toBeVisible();
  await page.getByLabel('Search files and folders').fill('Match');
  await expect(page.getByRole('region', { name: 'Search results' })).toBeVisible();
  await page.getByRole('button', { name: 'Open containing folder for Match 0.txt', exact: true }).click();
  await page.goBack();
  await expect(page.getByLabel('Search files and folders')).toHaveValue('Match');
  await page.getByRole('button', { name: 'Clear search', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Preview Match 100.txt', exact: true })).toBeVisible();
  expect(fixture.requests.at(-1)?.pathname).toBe('/folders/root/children');
  expect(fixture.requests.at(-1)?.searchParams.get('cursor')).toBe('page-two');
});

test('containing-folder navigation in the current folder still preserves search in Back', async ({ page }) => {
  await backend(page); await page.goto('/#files/projects'); await signIn(page);
  await page.getByLabel('Search files and folders').fill('Match');
  await page.getByRole('button', { name: 'Open containing folder for Match 0.txt', exact: true }).click();
  await expect(page.getByLabel('Search files and folders')).toHaveValue('');
  await page.goBack();
  await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
  await expect(page.getByLabel('Search files and folders')).toHaveValue('Match');
  await expect(page.getByRole('region', { name: 'Search results' })).toBeVisible();
});

test('transient ancestry failure retains requested URL, blocks mutations and retries the same folder', async ({ page }) => {
  await backend(page);
  let attempts = 0;
  let release!: () => void;
  let requested!: () => void;
  const pending = new Promise<void>(resolve => { requested = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  await page.route('**/folders/projects/path', async route => {
    attempts++;
    if (attempts === 1) { requested(); await gate; return route.fulfill({ status: 503 }); }
    return route.fulfill({ json: { items: [root, child] } });
  });
  await page.goto('/#files/projects?sort=size_desc');
  await page.getByLabel('Account name', { exact: true }).fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await pending;
  await expect(page.getByRole('button', { name: 'New folder', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Choose files', exact: true })).toBeDisabled();
  await expect(page.getByLabel('Search files and folders')).toBeDisabled();
  await expect(page.getByLabel('Sort files by', { exact: true })).toBeDisabled();
  release();
  await expect(page.getByRole('alert')).toContainText('Could not load this folder');
  expect(new URL(page.url()).hash).toBe('#files/projects?sort=size_desc');
  await expect(page.getByRole('button', { name: 'New folder', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Try again', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'New folder', exact: true })).toBeEnabled();
  expect(attempts).toBe(2);
  expect(new URL(page.url()).hash).toBe('#files/projects?sort=size_desc');
});

for (const destination of ['Account', 'Settings', 'Trash']) {
  test(`${destination} reload and history restore without depending on Files root`, async ({ page }) => {
    await backend(page, true);
    let rootReads = 0;
    await page.route(url => url.pathname === '/folders/children', route => { rootReads++; return route.fulfill({ status: 503 }); });
    await page.goto(`/#${destination.toLowerCase()}`); await signIn(page);
    await expect(page.getByRole('heading', { name: destination, exact: true })).toBeFocused();
    expect(rootReads).toBe(0);
    await page.reload(); await signIn(page);
    await expect(page.getByRole('heading', { name: destination, exact: true })).toBeFocused();
    const alternate = destination === 'Account' ? 'Trash' : 'Account';
    await page.getByRole('button', { name: alternate, exact: true }).click();
    await expect(page.getByRole('heading', { name: alternate, exact: true })).toBeFocused();
    await page.goBack();
    await expect(page.getByRole('heading', { name: destination, exact: true })).toBeFocused();
    await page.goForward();
    await expect(page.getByRole('heading', { name: alternate, exact: true })).toBeFocused();
    expect(rootReads).toBe(0);
  });
}

test('idle account reset dialog and drafts clear on history restoration', async ({ page }) => {
  await backend(page, true); await page.goto('/#account'); await signIn(page);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Reset password', exact: true }).click();
  await page.getByLabel('New password', { exact: true }).fill('private draft password');
  await page.goBack();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
  await page.goForward();
  await page.getByRole('button', { name: 'Reset password', exact: true }).click();
  await expect(page.getByLabel('New password', { exact: true })).toHaveValue('');
});

test('busy account operation keeps its dialog and failure while Back is deferred', async ({ page }) => {
  await backend(page, true); await page.goto('/#account'); await signIn(page);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  let release!: () => void;
  let requested!: () => void;
  const pending = new Promise<void>(resolve => { requested = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  await page.route('**/accounts/member/reset', async route => { requested(); await gate; await route.fulfill({ status: 503 }); });
  await page.getByRole('button', { name: 'Reset password', exact: true }).click();
  await page.getByLabel('New password', { exact: true }).fill('private draft password');
  await page.getByRole('dialog').getByRole('button', { name: 'Reset password', exact: true }).click();
  await pending; await page.goBack();
  await expect(page.getByRole('dialog')).toBeVisible();
  expect(await page.evaluate(() => history.state.sylux.destination)).toBe('account');
  release();
  await expect(page.getByRole('dialog').getByRole('alert')).toContainText('Password could not be reset');
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
  await expect(page.getByRole('alert')).toHaveCount(0);
});

test('busy bulk Back preserves target entry and restores it after completion', async ({ page }) => {
  await backend(page); await page.goto('/'); await signIn(page);
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await page.getByRole('button', { name: 'Files', exact: true }).click();
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Preview Match 100.txt', exact: true })).toBeVisible();
  await page.getByLabel('Select Match 100.txt', { exact: true }).check();
  let release!: () => void;
  let requested!: () => void;
  const pending = new Promise<void>(resolve => { requested = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  await page.route('**/entries/file-100', async route => { requested(); await gate; await route.fulfill({ status: 204 }); });
  await page.getByRole('button', { name: 'Move selected to trash', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Move to trash', exact: true }).click();
  await pending; await page.goBack();
  expect(await page.evaluate(() => history.state.sylux.destination)).toBe('account');
  expect(new URL(page.url()).hash).toBe('#account');
  release();
  await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
  await page.goForward();
  await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Preview Match 100.txt', exact: true })).toBeVisible();
});

test('Files remains populated after restoring Account through browser history', async ({ page }) => {
  await backend(page); await page.goto('/'); await signIn(page);
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await page.getByRole('button', { name: 'Trash', exact: true }).click();
  await page.goBack();
  await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
  await page.getByRole('button', { name: 'Files', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Preview Match 0.txt', exact: true })).toBeVisible();
});

test('search entered while the initial listing loads keeps its query and focus', async ({ page }) => {
  await backend(page);
  let release!: () => void;
  let requested!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const pending = new Promise<void>(resolve => { requested = resolve; });
  await page.route('**/folders/root/children*', async route => {
    requested(); await gate;
    await route.fulfill({ json: { items: [file(0)], next_cursor: null, has_more: false } });
  });
  await page.goto('/');
  await page.getByLabel('Account name', { exact: true }).fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await pending;
  const search = page.getByLabel('Search files and folders');
  await expect(search).toBeEnabled();
  await search.focus(); await page.keyboard.type('Match');
  await expect(page.getByRole('button', { name: 'Preview Match 0.txt', exact: true })).toBeVisible();
  const response = page.waitForResponse('**/folders/root/children*');
  release(); await response;
  await expect(search).toHaveValue('Match');
  await expect(search).toBeFocused();
  await expect(page.getByRole('region', { name: 'Search results', exact: true })).toBeVisible();
});

test('clearing search after interrupting a folder load fetches that folder instead of showing prior rows', async ({ page }) => {
  await backend(page); await page.goto('/'); await signIn(page);
  let release!: () => void;
  let requested!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const pending = new Promise<void>(resolve => { requested = resolve; });
  let reads = 0;
  await page.route('**/folders/projects/children*', async route => {
    reads++;
    const first = reads === 1;
    if (first) { requested(); await gate; }
    await route.fulfill({ json: { items: [{ ...file(0), id: first ? 'obsolete' : 'nested', name: first ? 'Obsolete.txt' : 'Nested.txt' }], next_cursor: null, has_more: false } });
  });
  await page.getByRole('button', { name: 'Open Projects', exact: true }).click();
  await pending;
  const search = page.getByLabel('Search files and folders');
  await search.fill('Match');
  await expect(page.getByRole('region', { name: 'Search results', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Clear search', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Preview Nested.txt', exact: true })).toBeVisible();
  const response = page.waitForResponse('**/folders/projects/children*');
  release(); await response;
  await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Preview Nested.txt', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Preview Obsolete.txt', exact: true })).toHaveCount(0);
});

for (const action of ['Home', 'Files sidebar']) {
  test(`${action} completion cannot cancel Back while its root listing is pending`, async ({ page }) => {
    await backend(page); await page.goto('/#files/projects'); await signIn(page);
    await expect(page.getByRole('button', { name: 'Preview Match 0.txt', exact: true })).toBeVisible();
    let releaseRoot!: () => void;
    let requestedRoot!: () => void;
    let releasePath!: () => void;
    let requestedPath!: () => void;
    const rootGate = new Promise<void>(resolve => { releaseRoot = resolve; });
    const rootPending = new Promise<void>(resolve => { requestedRoot = resolve; });
    const pathGate = new Promise<void>(resolve => { releasePath = resolve; });
    const pathPending = new Promise<void>(resolve => { requestedPath = resolve; });
    await page.route('**/folders/root/children*', async route => {
      requestedRoot(); await rootGate;
      await route.fulfill({ json: { items: [child], next_cursor: null, has_more: false } });
    });
    await page.route('**/folders/projects/path', async route => {
      requestedPath(); await pathGate;
      await route.fulfill({ json: { items: [root, child] } });
    });
    if (action === 'Home') await page.getByRole('link', { name: 'Sylux', exact: true }).click();
    else await page.getByRole('navigation', { name: 'Files', exact: true }).getByRole('button', { name: 'Files', exact: true }).click();
    await rootPending;
    await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeVisible();
    await page.goBack(); await pathPending;
    const rootResponse = page.waitForResponse('**/folders/root/children*');
    releaseRoot(); await rootResponse;
    await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    releasePath();
    await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Preview Match 0.txt', exact: true })).toBeVisible();
    expect(new URL(page.url()).hash).toBe('#files/projects');
    await expect.poll(() => page.evaluate(() => history.state?.sylux?.folder)).toBe('projects');
  });
}
