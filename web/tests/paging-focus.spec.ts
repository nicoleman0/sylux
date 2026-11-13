import { expect, test, type Page } from '@playwright/test';

async function workspace(page: Page, search: boolean) {
  const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
  const entry = (name: string) => ({ id: name, parent_id: 'root', name: `${name}.txt`, kind: 'file', size: 1, etag: `"${name}:1"` });
  await page.route('**/*', route => {
    const url = new URL(route.request().url());
    const json = (body: unknown) => route.fulfill({ json: body });
    if (url.pathname === '/instance') return json({ name: 'Sylux' });
    if (url.pathname === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (url.pathname === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user' });
    if (url.pathname === '/folders/children') return json({ root, items: [], next_cursor: null, has_more: false });
    if (url.pathname === '/folders/root/children' || url.pathname === '/entries/search') {
      const last = url.searchParams.has('cursor');
      const item = entry(last ? 'Last' : 'First');
      return json({ items: url.pathname === '/entries/search' ? [{ entry: item, location: [root] }] : [item], next_cursor: last ? null : 'next', has_more: !last });
    }
    return route.continue();
  });
  await page.goto('/');
  await page.getByLabel('Account name').fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Preview First.txt', exact: true })).toBeVisible();
  if (search) {
    await page.getByLabel('Search files and folders').fill('txt');
    await expect(page.getByRole('region', { name: 'Search results', exact: true })).toHaveAttribute('aria-busy', 'false');
  }
}

for (const search of [false, true]) {
  test(`${search ? 'search' : 'folder'} paging keeps keyboard focus when next and previous disappear`, async ({ page }) => {
    await workspace(page, search);
    const next = page.getByRole('button', { name: 'Next page', exact: true });
    await next.focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('button', { name: 'Preview Last.txt', exact: true })).toBeFocused();
    const previous = page.getByRole('button', { name: 'Previous page', exact: true });
    await previous.focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('button', { name: 'Preview First.txt', exact: true })).toBeFocused();
  });
}

test('empty search page focuses the folder heading', async ({ page }) => {
  await workspace(page, true);
  await page.route('**/entries/search**', route => {
    if (new URL(route.request().url()).searchParams.has('cursor')) {
      return route.fulfill({ json: { items: [], next_cursor: null, has_more: false } });
    }
    return route.fallback();
  });
  await page.getByRole('button', { name: 'Next page', exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: 'Files', exact: true })).toBeFocused();
});

for (const search of [false, true]) {
  test(`${search ? 'search' : 'folder'} failed paging focuses recovery`, async ({ page }) => {
    await workspace(page, search);
    await page.route(search ? '**/entries/search**' : '**/folders/root/children**', route => {
      if (new URL(route.request().url()).searchParams.has('cursor')) return route.fulfill({ status: 500, json: { code: 'failed' } });
      return route.fallback();
    });
    await page.getByRole('button', { name: 'Next page', exact: true }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeFocused();
  });

  test(`${search ? 'search' : 'folder'} late paging does not steal Account focus`, async ({ page }) => {
    await workspace(page, search);
    let release!: () => void;
    let started!: () => void;
    let settled!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    const requestStarted = new Promise<void>(resolve => { started = resolve; });
    const requestSettled = new Promise<void>(resolve => { settled = resolve; });
    await page.route(search ? '**/entries/search**' : '**/folders/root/children**', async route => {
      if (new URL(route.request().url()).searchParams.has('cursor')) {
        started(); await gate;
        try { await route.fallback(); } finally { settled(); }
        return;
      }
      return route.fallback();
    });
    await page.getByRole('button', { name: 'Next page', exact: true }).focus();
    await page.keyboard.press('Enter');
    await requestStarted;
    await page.getByRole('button', { name: 'Account', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
    release();
    await requestSettled;
    await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    await expect(page.getByRole('heading', { name: 'Account', exact: true })).toBeFocused();
  });
}

test('late search paging does not steal focus from a changed query', async ({ page }) => {
  await workspace(page, true);
  let release!: () => void;
  let started!: () => void;
  let settled!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const requestStarted = new Promise<void>(resolve => { started = resolve; });
  const requestSettled = new Promise<void>(resolve => { settled = resolve; });
  await page.route('**/entries/search**', async route => {
    if (new URL(route.request().url()).searchParams.has('cursor')) {
      started(); await gate;
      try { await route.fallback(); } finally { settled(); }
      return;
    }
    return route.fallback();
  });
  await page.getByRole('button', { name: 'Next page', exact: true }).focus();
  await page.keyboard.press('Enter');
  await requestStarted;
  const input = page.getByLabel('Search files and folders');
  await input.fill('different');
  await expect(input).toBeFocused();
  release();
  await requestSettled;
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  await expect(input).toBeFocused();
});

for (const search of [false, true]) {
  test(`${search ? 'search' : 'folder'} stale cursor restart focuses refreshed results`, async ({ page }) => {
    await workspace(page, search);
    let stale = true;
    await page.route(search ? '**/entries/search**' : '**/folders/root/children**', route => {
      const url = new URL(route.request().url());
      if (url.searchParams.has('cursor') && stale) {
        stale = false;
        return route.fulfill({ status: 409, json: { code: 'stale_cursor' } });
      }
      return route.fallback();
    });
    await page.getByRole('button', { name: 'Next page', exact: true }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('button', { name: 'Preview First.txt', exact: true })).toBeFocused();
  });
}
