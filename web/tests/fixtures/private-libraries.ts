import { expect, type Page } from '@playwright/test';
import type { Entry, Folder } from '../../src/lib/listing';

const folder = (id: string, name: string, parent_id: string | null = null): Folder =>
  ({ id, name, parent_id, kind: 'folder', etag: `"${id}:1"` });
const root = folder('root', 'Files');
const library = folder('field-work', 'Field work');
const studio = folder('studio', 'Studio');

export async function mockPrivateLibraries(page: Page, options: { name?: string; logoUrl?: string; admin?: boolean } = {}): Promise<void> {
  const libraries = [root, library, studio];
  const folders = [folder('field-projects', 'Projects', library.id), folder('studio-projects', 'Projects', studio.id)];
  const files: Entry[] = folders.map((parent, index) => ({
    id: `file-${index}`, name: index === 0 ? 'survey.txt' : 'sketch.txt', parent_id: parent.id,
    kind: 'file', size: 4, etag: '"file:1"',
  }));
  const inventory = [...libraries, ...folders, ...files];
  await page.route('**/*', route => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;
    const json = (value: unknown) => route.fulfill({ json: value });
    const pageOf = (items: Entry[]) => ({ items, next_cursor: null, has_more: false });
    if (pathname === '/instance') return json({ name: options.name ?? 'Sylux' });
    if (pathname === '/instance/appearance') return json({ accentColor: null, loginBackgroundColor: null, logoUrl: options.logoUrl ?? null, loginImageUrl: null });
    if (pathname === '/accounts') return json([{ account_id: 'owner', handle: 'owner', role: 'administrator', enabled: true }]);
    if (pathname === '/auth/logout') return route.fulfill({ status: 204 });
    if (pathname === '/auth/csrf') return json({ csrf_token: 'csrf' });
    if (pathname === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: options.admin ? 'administrator' : 'user' });
    if (pathname === '/libraries' && request.method() === 'POST') {
      const name = request.postDataJSON().name as string;
      if (libraries.some(item => item.name === name)) return route.fulfill({ status: 409, json: { code: 'name_conflict' } });
      const created = folder('new-library', name);
      libraries.push(created); inventory.push(created);
      return route.fulfill({ status: 201, json: created });
    }
    if (pathname === '/share-links' && request.method() === 'GET') return json([]);
    if (pathname === '/libraries') return json({ items: libraries });
    if (pathname === '/folders/children') return json({ ...pageOf([]), root });
    const target = inventory.find(item => item.id === pathname.split('/')[2]);
    if (pathname.startsWith('/folders/') && target?.kind === 'folder') {
      if (pathname.endsWith('/children')) return json(pageOf(inventory.filter(item => item.parent_id === target.id)));
      if (pathname.endsWith('/path')) return json({ items: target.parent_id ? [inventory.find(item => item.id === target.parent_id), target] : [target] });
    }
    return route.continue();
  });
}

export async function signInLibraryOwner(page: Page): Promise<void> {
  await page.getByLabel('Account name', { exact: true }).fill('owner');
  await page.getByLabel('Password', { exact: true }).fill('password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
}
