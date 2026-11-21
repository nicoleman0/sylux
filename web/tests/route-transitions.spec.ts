import { expect, test, type Page, type Route, type TestInfo } from '@playwright/test';

const root = { id: 'root', name: 'Files', kind: 'folder', parent_id: null, etag: '"root:1"' };
const projects = { id: 'projects', name: 'Projects', kind: 'folder', parent_id: 'root', etag: '"projects:1"' };
const entry = (id: string, name: string, parent_id = 'root') => ({ id, name, parent_id, kind: 'file', etag: '"entry:1"', size: 4 });
const list = (items: unknown[]) => ({ items, next_cursor: null, has_more: false });

type Frame = { heading: string; content: boolean; pendingCopy: string[]; busy: boolean; ms: number };
declare global {
  interface Window { routeFrames: Frame[]; }
}

async function fixture(page: Page) {
  let owner = 'owner';
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    switch (path) {
      case '/instance': return route.fulfill({ json: { name: 'Sylux' } });
      case '/instance/appearance': return route.fulfill({ json: {} });
      case '/auth/csrf': return route.fulfill({ json: { csrf_token: 'csrf' } });
      case '/auth/local':
        owner = route.request().postDataJSON().handle;
        return route.fulfill({ json: { account_id: owner, handle: owner, role: 'user', csrf_token: 'csrf' } });
      case '/auth/logout': return route.fulfill({ json: {} });
      case '/folders/children': return route.fulfill({ json: { root, ...list([projects]) } });
      case '/folders/root/children': return route.fulfill({ json: list([projects, entry('notes', `${owner}-notes.txt`)]) });
      case '/folders/projects/path': return route.fulfill({ json: { items: [root, projects] } });
      case '/folders/projects/children': return route.fulfill({ json: list([entry('project', 'project.txt', 'projects')]) });
      case '/trash': return route.fulfill({ json: list([{ ...entry('deleted', 'deleted.txt'), original_path: ['Files'] }]) });
      case '/me/usage': return route.fulfill({ json: { files_bytes: owner === 'owner' ? 1536 : 2048, trash_bytes: 4 } });
      default: return route.continue();
    }
  });
  await page.addInitScript(() => {
    window.routeFrames = [];
    const sample = (ms: number) => {
      const main = document.getElementById('main-content');
      const heading = main?.querySelector('h1');
      if (heading) {
        const scope = heading.closest('.files-content, .trash-content, .account-content') ?? main!;
        const content = scope.querySelector('.route-placeholder, .file-row, .trash-row, .account-details, form');
        const pendingCopy = [...main!.querySelectorAll('[role=status], .usage-details dd, .list-summary')]
          .map(element => element.textContent?.trim() ?? '')
          .filter(copy => /Loading|Opening|Searching|Refreshing/.test(copy));
        window.routeFrames.push({
          heading: heading.textContent?.trim() ?? '',
          content: !!content?.getBoundingClientRect().height,
          pendingCopy,
          busy: scope.getAttribute('aria-busy') === 'true' || !!scope.querySelector('[aria-busy=true]'),
          ms,
        });
      }
      requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
  });
}

async function login(page: Page, owner = 'owner') {
  await page.getByLabel('Account name', { exact: true }).fill(owner);
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
}

async function sidebar(page: Page, destination: string, narrow = false) {
  if (narrow) await page.getByRole('button', { name: 'Toggle Sidebar', exact: true }).click();
  const button = page.getByRole('button', { name: destination, exact: true });
  await button.focus();
  await page.keyboard.press('Enter');
}

async function recordFast(page: Page, info: TestInfo, label: string, destination: string, content: string, action: () => Promise<unknown>) {
  await page.evaluate(() => { window.routeFrames = []; });
  await action();
  await expect(page.getByRole('heading', { level: 1, name: destination, exact: true })).toBeFocused();
  await expect(page.getByText(content, { exact: true })).toBeVisible();
  const frames = await page.evaluate(() => new Promise<Frame[]>(resolve => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve(window.routeFrames)));
  }));
  await info.attach(label, { body: JSON.stringify(frames, null, 2), contentType: 'application/json' });
  expect(frames.some(frame => frame.heading === destination && !frame.busy)).toBe(true);
  expect(frames.filter(frame => !frame.content || frame.pendingCopy.length)).toEqual([]);
}

const destinations = [
  { hash: 'files', heading: 'Files', content: 'owner-notes.txt' },
  { hash: 'files/projects', heading: 'Projects', content: 'project.txt' },
  { hash: 'trash', heading: 'Trash', content: 'deleted.txt' },
  { hash: 'account', heading: 'Account', content: '1.5 KB' },
];

for (const width of [1280, 390]) for (const theme of ['light', 'dark'] as const) {
  test(`fast cold and warm route frames at ${width}px in ${theme}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 850 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await fixture(page);
    for (const { hash, heading, content } of [...destinations].reverse()) {
      await page.goto(`/#${hash}`);
      await recordFast(page, info, `cold-${heading}`, heading, content, () => login(page));
      if (heading !== 'Files') await page.reload();
    }
    await recordFast(page, info, 'warm-folder', 'Projects', 'project.txt', async () => {
      await page.getByRole('button', { name: 'Open Projects', exact: true }).focus();
      await page.keyboard.press('Enter');
    });
    for (const { heading, content } of [destinations[2], destinations[3], destinations[0]]) {
      await recordFast(page, info, `warm-${heading}`, heading, content, () => sidebar(page, heading, width === 390));
    }
    for (const { heading, content } of [destinations[3], destinations[2], destinations[1]]) {
      await recordFast(page, info, `back-${heading}`, heading, content, () => page.goBack());
    }
    for (const { heading, content } of [destinations[2], destinations[3], destinations[0]]) {
      await recordFast(page, info, `forward-${heading}`, heading, content, () => page.goForward());
    }
    await page.keyboard.press('ArrowDown');
    await expect(page.getByRole('button', { name: 'Open Projects', exact: true })).toBeFocused();
    await page.screenshot({ path: info.outputPath('keyboard-focus.png') });
  });
}

async function hold(page: Page, url: string) {
  let request: Route | undefined;
  await page.route(url, route => { request = route; }, { times: 1 });
  return async () => {
    await expect.poll(() => !!request).toBe(true);
    return request!;
  };
}

for (const moveAway of [false, true]) {
  test(`queued heading arrow ${moveAway ? 'does not steal moved focus' : 'focuses the loaded row'}`, async ({ page }) => {
    await fixture(page);
    const deferred = await hold(page, '**/folders/root/children?*');
    await page.goto('/'); await login(page);
    const request = await deferred();
    const heading = page.getByRole('heading', { level: 1, name: 'Files', exact: true });
    await heading.focus();
    await page.keyboard.press('ArrowDown');
    const logout = page.getByRole('button', { name: 'Log out', exact: true });
    if (moveAway) await logout.focus();
    await request.fulfill({ json: list([projects]) });
    const row = page.getByRole('button', { name: 'Open Projects', exact: true });
    await expect(row).toBeVisible();
    if (moveAway) await expect(logout).toBeFocused();
    else {
      await expect(row).toBeFocused();
      await expect(row).toHaveCSS('outline-style', 'solid');
    }
  });
}

test('slow route retains an inert view, reports pending once, and retries the failed destination', async ({ page }, info) => {
  await fixture(page); await page.goto('/'); await login(page);
  const heading = page.getByRole('heading', { level: 1, name: 'Files', exact: true });
  await expect(heading).toBeFocused();
  const deferred = await hold(page, '**/trash?*');
  await sidebar(page, 'Trash');
  const request = await deferred();
  const status = page.getByRole('status').filter({ hasText: 'Opening Trash' });
  await expect(status).toBeVisible();
  await expect(page.locator('.files-content')).toHaveAttribute('inert', '');
  await expect(page.locator('.files-content')).toContainText('owner-notes.txt');
  const trigger = page.getByRole('button', { name: 'Trash', exact: true });
  await expect(trigger).toHaveAttribute('aria-busy', 'true');
  await page.getByRole('button', { name: 'Open Projects', exact: true }).evaluate(element => (element as HTMLElement).focus());
  await expect(trigger).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(status).toBeVisible();
  await page.screenshot({ path: info.outputPath('slow-trash.png') });
  await request.fulfill({ status: 503 });
  await expect(page.getByRole('alert')).toHaveText('Could not load trash. Try again.');
  await expect(status).toHaveCount(0);
  const retry = page.getByRole('button', { name: 'Try again', exact: true });
  await expect(retry).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.getByText('deleted.txt', { exact: true })).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
});

for (const ending of ['sign-out', 'expired'] as const) {
  test(`late private navigation cannot survive ${ending} and a replacement account`, async ({ page }) => {
    await fixture(page); await page.goto('/'); await login(page);
    await expect(page.getByText('owner-notes.txt', { exact: true })).toBeVisible();
    const deferred = await hold(page, '**/trash?*');
    await sidebar(page, 'Trash');
    const request = await deferred();
    const canceled = page.waitForEvent('requestfailed', value => new URL(value.url()).pathname === '/trash');
    if (ending === 'sign-out') await page.getByRole('button', { name: 'Log out', exact: true }).click();
    else {
      await page.route('**/me/usage', route => route.fulfill({ status: 401, json: { code: 'unauthorized' } }), { times: 1 });
      await sidebar(page, 'Account');
    }
    await canceled;
    await expect(page.getByLabel('Account name', { exact: true })).toBeFocused();
    await expect(page.locator('.file-row, .trash-row')).toHaveCount(0);
    if (ending === 'expired') await expect(page.getByText('Your session ended. Sign in again to continue.')).toBeVisible();
    await login(page, 'other');
    await expect(page.getByText('other-notes.txt', { exact: true })).toBeVisible();
    await request.fulfill({ json: list([{ ...entry('private', 'private-trash.txt'), original_path: ['Files'] }]) });
    await expect(page.getByText('private-trash.txt', { exact: true })).toHaveCount(0);
    await expect(page.getByText('owner-notes.txt', { exact: true })).toHaveCount(0);
    await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeFocused();
  });
}

test('Back closes idle sharing and defers navigation while an uncertain link can be checked', async ({ page }) => {
  await fixture(page); await page.goto('/#account'); await login(page);
  await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
  await page.route('**/share-links', route => route.fulfill({ json: [] }));
  const openShare = async () => {
    await sidebar(page, 'Files');
    await page.getByRole('button', { name: 'Options for owner-notes.txt', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Share', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Share owner-notes.txt', exact: true });
    await expect(dialog).toBeVisible();
    return dialog;
  };

  const idle = await openShare();
  await page.goBack();
  await expect(idle).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
  const dialog = await openShare();
  const deferred = await hold(page, '**/share-links');
  await dialog.getByRole('button', { name: 'Create link', exact: true }).click();
  const request = await deferred();
  await page.goBack();
  await expect(dialog).toBeVisible();
  await request.fulfill({ status: 503 });
  await expect(dialog).toContainText('Could not confirm the new link.');
  await dialog.getByRole('button', { name: 'Done', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
});

test('toolbar Refresh recovers a failed folder route and restores listing controls', async ({ page }) => {
  await fixture(page);
  let failures = 1;
  await page.route('**/folders/projects/children**', route => {
    if (failures-- > 0) return route.fulfill({ status: 503, json: { code: 'unavailable' } });
    return route.fulfill({ json: list([entry('project', 'project.txt', 'projects')]) });
  });
  await page.goto('/'); await login(page);
  await expect(page.getByText('owner-notes.txt', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Open Projects', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Could not load this folder. Check your connection and try again.');
  await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
  await expect(page.getByText('project.txt', { exact: true })).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(page.getByLabel('Search files and folders')).toBeEnabled();
  await expect(page.getByRole('button', { name: 'New folder', exact: true })).toBeEnabled();
  await expect(page.locator('#choose-file')).toBeEnabled();
  await expect(page.locator('#folder-sort')).toBeEnabled();
  await page.goBack();
  await expect(page.getByText('owner-notes.txt', { exact: true })).toBeVisible();
});

test('failed toolbar Refresh keeps route feedback actionable for Try again', async ({ page }) => {
  await fixture(page);
  let failures = 2;
  await page.route('**/folders/projects/children**', route => {
    if (failures-- > 0) return route.fulfill({ status: 503, json: { code: 'unavailable' } });
    return route.fulfill({ json: list([entry('project', 'project.txt', 'projects')]) });
  });
  await page.goto('/'); await login(page);
  await expect(page.getByText('owner-notes.txt', { exact: true })).toBeVisible();

  await page.getByRole('button', { name: 'Open Projects', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Could not load this folder. Check your connection and try again.');
  await page.getByRole('button', { name: 'Refresh folder', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Could not load this folder. Check your connection and try again.');
  await expect(page.getByLabel('Search files and folders')).toBeDisabled();
  await expect(page.getByRole('button', { name: 'New folder', exact: true })).toBeDisabled();
  await expect(page.locator('#choose-file')).toBeDisabled();
  await expect(page.locator('#folder-sort')).toBeDisabled();

  await page.getByRole('button', { name: 'Try again', exact: true }).click();
  await expect(page.getByText('project.txt', { exact: true })).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(page.getByLabel('Search files and folders')).toBeEnabled();
  await expect(page.getByRole('button', { name: 'New folder', exact: true })).toBeEnabled();
  await expect(page.locator('#choose-file')).toBeEnabled();
  await expect(page.locator('#folder-sort')).toBeEnabled();
  await page.goBack();
  await expect(page.getByText('owner-notes.txt', { exact: true })).toBeVisible();
});
