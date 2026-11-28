import { chooseFolderSort, folderSortHeader } from './fixtures/folder-sort';
import { expect, test, type Page } from '@playwright/test';
import type { Entry, Folder } from '../src/lib/listing';
import { parseLocation, savedContext } from '../src/lib/browsing-context';
import { mockPrivateLibraries, signInLibraryOwner } from './fixtures/private-libraries';

test('saved folder URLs and browser history keep identically named folders in their own libraries', async ({ page }) => {
  await mockPrivateLibraries(page);
  await page.goto('/#libraries');
  await signInLibraryOwner(page);
  const libraryNav = page.getByRole('button', { name: 'My libraries', exact: true });
  const filesNav = page.getByRole('button', { name: 'Files', exact: true });
  const location = page.locator('.workspace-location > span:last-child');
  const search = page.getByRole('searchbox', { name: 'Search files and folders' });
  await page.route('**/entries/search?**', route => route.fulfill({ json: { items: [], next_cursor: null, has_more: false } }));
  const expectLibraryLocation = async (name: string) => {
    await expect(location).toHaveText(name);
    await expect(libraryNav).toHaveAttribute('data-active', 'true');
    await expect(libraryNav).toHaveAttribute('aria-current', 'location');
    await expect(filesNav).not.toHaveAttribute('data-active', 'true');
    await expect(filesNav).not.toHaveAttribute('aria-current');
  };
  await expect(libraryNav).toHaveAttribute('aria-current', 'page');
  const open = async (library: string, file: string) => {
    await page.getByRole('link', { name: library, exact: true }).press('Enter');
    await expect(page.getByRole('heading', { name: library, exact: true })).toBeFocused();
    await expectLibraryLocation(library);
    await expect(search).toHaveAttribute('placeholder', 'Search this library by name');
    await page.getByRole('button', { name: 'Open Projects', exact: true }).press('Enter');
    await expect(page.getByRole('button', { name: `Preview ${file}`, exact: true })).toBeVisible();
    await expectLibraryLocation(library);
    await expect(search).toHaveAttribute('placeholder', 'Search this folder by name');
    return page.url();
  };
  const first = await open('Field work', 'survey.txt');
  await libraryNav.click();
  const second = await open('Studio', 'sketch.txt');
  expect(second).not.toBe(first);
  const requested = page.waitForRequest(request => new URL(request.url()).pathname === '/entries/search');
  await search.fill('missing');
  expect(new URL((await requested).url()).searchParams.get('folder_id')).toBe('studio-projects');
  await expect(page.getByRole('region', { name: 'Search results' })).toBeVisible();
  await expectLibraryLocation('Studio');
  await search.clear();
  await page.reload();
  await signInLibraryOwner(page);
  await expect(page.getByRole('button', { name: 'Preview sketch.txt' })).toBeVisible();
  await expectLibraryLocation('Studio');
  await expect(page.getByRole('button', { name: 'Preview survey.txt' })).toHaveCount(0);
  await page.goto(first);
  await expect(page.getByRole('button', { name: 'Preview survey.txt' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Preview sketch.txt' })).toHaveCount(0);
  await expectLibraryLocation('Field work');
  await filesNav.click();
  await expect(location).toHaveText('Files');
  await expect(filesNav).toHaveAttribute('aria-current', 'page');
  await expect(filesNav).toHaveAttribute('data-active', 'true');
  await expect(libraryNav).not.toHaveAttribute('aria-current');
  await expect(search).toHaveAttribute('placeholder', 'Search all libraries by name');
  await page.goBack();
  await expect(page.getByRole('button', { name: 'Preview survey.txt' })).toBeVisible();
  await expectLibraryLocation('Field work');
});

test('the destination library list can be retried from a named library', async ({ page }) => {
  await mockPrivateLibraries(page);
  await page.goto('/#files/studio-projects');
  await signInLibraryOwner(page);
  await expect(page.getByRole('button', { name: 'Preview sketch.txt', exact: true })).toBeVisible();
  let attempts = 0;
  await page.route('**/libraries', route => ++attempts === 1
    ? route.fulfill({ status: 503, json: { code: 'mutation_unavailable' } })
    : route.fallback());
  await page.getByRole('button', { name: 'Options for sketch.txt', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Move', exact: true }).click();
  const picker = page.getByRole('dialog', { name: 'Move sketch.txt' });
  await expect(picker.getByRole('navigation', { name: 'Move destination path' })).toHaveText('Studio');
  await picker.getByRole('button', { name: 'Choose a library' }).click();
  await expect(picker.getByRole('alert')).toHaveText('Could not load your libraries. Try again.');
  await expect(picker.getByRole('button', { name: 'Move here' })).toBeDisabled();
  await picker.getByRole('button', { name: 'Refresh destinations' }).click();
  await expect(picker.getByRole('alert')).toHaveCount(0);
  await picker.getByRole('group', { name: 'Destination libraries' }).getByRole('button', { name: 'Field work' }).click();
  await expect(picker.getByRole('navigation', { name: 'Move destination path' })).toHaveText('Field work');
  await expect(picker.getByRole('button', { name: 'Projects' })).toBeVisible();
  await picker.getByRole('button', { name: 'Cancel' }).click();
  await expect(page.getByRole('button', { name: 'Preview sketch.txt', exact: true })).toBeVisible();
  expect(attempts).toBe(2);
});

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
    if (url.pathname === '/libraries') return json({ items: [root] });
    if (url.pathname === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (url.pathname === '/auth/local') {
      const handle = route.request().postDataJSON().handle;
      return json({ account_id: handle, handle, role: administrator ? 'administrator' : 'user', csrf_token: 'csrf' });
    }
    if (url.pathname === '/accounts') return json([{ account_id: 'member', handle: 'member', role: 'user', enabled: true }]);
    if (url.pathname === '/me/usage') return json({ files_bytes: 0, trash_bytes: 0 });
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
    if (url.pathname === '/trash') return json({ items: [], next_cursor: null, has_more: false });
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
      await chooseFolderSort(page, 'Size', 'descending');
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
      const focused = page.locator('.search-results .item-name:focus');
      await expect(focused).toHaveCount(1);
      await expect(focused).toBeInViewport({ ratio: 1 });
      await page.screenshot({ path: test.info().outputPath(`sylux-context-${width}-${theme}.png`), fullPage: false });
      await page.goForward();
      await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
      await expect(folderSortHeader(page, 'Size')).toHaveAttribute('aria-sort', 'descending');
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
  await expect(folderSortHeader(page, 'Modified')).toHaveAttribute('aria-sort', 'descending');
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(1400);
  const copiedPage = await page.context().newPage();
  const copiedFixture = await backend(copiedPage);
  await copiedPage.goto(page.url());
  expect(await copiedPage.evaluate(() => history.state)).toBeNull();
  await signIn(copiedPage);
  await expect(copiedPage.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
  await expect(copiedPage.getByRole('button', { name: 'Preview Match 0.txt', exact: true })).toBeVisible();
  await expect(copiedPage.getByLabel('Search files and folders')).toHaveValue('');
  await expect(folderSortHeader(copiedPage, 'Modified')).toHaveAttribute('aria-sort', 'descending');
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
  const canceled = page.waitForEvent('requestfailed', request => new URL(request.url()).pathname === '/folders/projects/path');
  await page.getByRole('button', { name: 'Account', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
  await canceled; release();
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
  const canceled = page.waitForEvent('requestfailed', request => new URL(request.url()).pathname === '/folders/projects/path');
  await page.getByRole('button', { name: 'Log out', exact: true }).click();
  await signIn(page, 'other');
  await canceled; release();
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
  await expect(page.getByRole('button', { name: /^Sort by/ })).toHaveCount(0);
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
  await page.getByRole('button', { name: 'Actions for member', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Reset password', exact: true }).click();
  await page.getByLabel('New password', { exact: true }).fill('private draft password');
  await page.goBack();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
  await page.goForward();
  await page.getByRole('button', { name: 'Actions for member', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Reset password', exact: true }).click();
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
  await page.getByRole('button', { name: 'Actions for member', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Reset password', exact: true }).click();
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
  const canceled = page.waitForEvent('requestfailed', request => new URL(request.url()).pathname === '/folders/root/children');
  await search.focus(); await page.keyboard.type('Match');
  await expect(page.getByRole('button', { name: 'Preview Match 0.txt', exact: true })).toBeVisible();
  await canceled; release();
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
  const canceled = page.waitForEvent('requestfailed', request => new URL(request.url()).pathname === '/folders/projects/children');
  await search.fill('Match');
  await expect(page.getByRole('region', { name: 'Search results', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Clear search', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Preview Nested.txt', exact: true })).toBeVisible();
  await canceled; release();
  await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Preview Nested.txt', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Preview Obsolete.txt', exact: true })).toHaveCount(0);
});

for (const action of ['Home', 'Files sidebar']) {
  test(`${action} completion cannot cancel Back while its destination is pending`, async ({ page }) => {
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
    const destination = action === 'Home' ? '/libraries' : '/folders/root/children';
    await page.route(url => url.pathname === destination, async route => {
      requestedRoot(); await rootGate;
      await route.fulfill({ json: action === 'Home' ? { items: [root] } : { items: [child], next_cursor: null, has_more: false } });
    });
    await page.route('**/folders/projects/path', async route => {
      requestedPath(); await pathGate;
      await route.fulfill({ json: { items: [root, child] } });
    });
    if (action === 'Home') await page.getByRole('link', { name: 'Sylux', exact: true }).click();
    else await page.getByRole('navigation', { name: 'Files', exact: true }).getByRole('button', { name: 'Files', exact: true }).click();
    await rootPending;
    if (action === 'Home') await expect(page.getByRole('status')).toHaveText('Opening My libraries…');
    else await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeVisible();
    const canceled = page.waitForEvent('requestfailed', request => new URL(request.url()).pathname === destination);
    await page.goBack(); await pathPending;
    await canceled; releaseRoot();
    await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    releasePath();
    await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Preview Match 0.txt', exact: true })).toBeVisible();
    expect(new URL(page.url()).hash).toBe('#files/projects');
    await expect.poll(() => page.evaluate(() => history.state?.sylux?.folder)).toBe('projects');
  });
}

test('search order survives containing-folder Back and resets at logout', async ({ page }) => {
  const { requests } = await backend(page);
  await page.goto('/'); await signIn(page);
  await page.getByLabel('Search files and folders', { exact: true }).fill('Match');
  const results = page.getByRole('region', { name: 'Search results', exact: true });
  await expect(results.getByRole('button', { name: 'Sort by size, ascending', exact: true })).toBeVisible();
  await results.getByRole('button', { name: 'Sort by size, ascending', exact: true }).click();
  await expect.poll(() => requests.at(-1)?.searchParams.get('sort')).toBe('size_asc');
  await results.getByRole('button', { name: 'Open containing folder for Match 0.txt', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
  await page.goBack();
  await expect(results).toHaveAttribute('aria-busy', 'false');
  expect(requests.at(-1)?.pathname).toBe('/entries/search');
  expect(requests.at(-1)?.searchParams.get('sort')).toBe('size_asc');
  await expect(results.getByRole('columnheader', { name: 'Size', exact: true })).toHaveAttribute('aria-sort', 'ascending');
  await page.getByRole('button', { name: 'Log out', exact: true }).click();
  await signIn(page);
  await page.getByLabel('Search files and folders', { exact: true }).fill('Match');
  await expect(results).toHaveAttribute('aria-busy', 'false');
  expect(requests.at(-1)?.searchParams.get('sort')).toBe('name_asc');
});

test('saved sort choices reject invalid and foreign history state', () => {
  const location = parseLocation('#trash?searchSort=size_desc&trashSort=location_desc&accountSort=status_asc');
  expect(location).toMatchObject({ searchSort: 'size_desc', trashSort: 'location_desc', accountSort: 'status_asc' });
  const state = { ...location, version: 1, owner: 'owner', cursors: [null], page: 0, searchCursors: [null], searchPage: 0, scroll: 0 };
  expect(savedContext(state, 'owner', location)).toMatchObject({ trashSort: 'location_desc' });
  expect(savedContext({ ...state, trashSort: 'bogus' }, 'owner', location)).toBeNull();
  expect(savedContext({ ...state, accountSort: 'size_desc' }, 'owner', location)).toBeNull();
  expect(savedContext(state, 'different-owner', location)).toBeNull();
});

for (const branding of [{ name: 'Sylux' }, { name: 'Workshop', logoUrl: '/instance/appearance/assets/' + 'a'.repeat(64) }]) {
  test(`brand returns to My libraries with ${branding.name} branding`, async ({ page }) => {
    await mockPrivateLibraries(page, { ...branding, admin: true });
    await page.goto('/#libraries');
    await signInLibraryOwner(page);
    const brand = page.getByRole('link', { name: branding.name, exact: true });
    await expect(brand).toHaveAttribute('href', '#libraries');
    await page.getByRole('link', { name: 'Field work', exact: true }).click();
    await page.getByRole('button', { name: 'Open Projects', exact: true }).click();
    await expect(page).toHaveURL(/#files\/field-projects$/);
    await brand.focus();
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/#libraries$/);
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await expect(page).toHaveURL(/#settings$/);
    await brand.click();
    await expect(page).toHaveURL(/#libraries$/);
    await page.getByRole('button', { name: 'Log out', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
    await page.getByRole('link', { name: branding.name, exact: true }).click();
    await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
  });
}
