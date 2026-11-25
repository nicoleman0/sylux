import { expect, test } from '@playwright/test';

const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: 'root:1' };
const folder = { id: 'project', parent_id: 'root', name: 'Project', kind: 'folder', etag: 'project:1' };

for (const scenario of [
  { outcome: 'network', observed: 'absent', width: 390, colorScheme: 'dark' },
  { outcome: 'missing', observed: 'absent', width: 1280, colorScheme: 'light' },
  { outcome: 'unavailable', observed: 'present', width: 1280, colorScheme: 'light' },
  { outcome: 'unavailable', observed: 'unknown', width: 390, colorScheme: 'dark' },
] as const) {
  test(`direct revoke observes ${scenario.observed} access after a failed response (${scenario.outcome})`, async ({ page }) => {
    await page.setViewportSize({ width: scenario.width, height: 750 });
    await page.emulateMedia({ colorScheme: scenario.colorScheme });

    const grant = { id: 'grant', handle: 'member' };
    const unaffected = { id: 'unaffected', handle: 'colleague' };
    let reads = 0;
    let deletes = 0;
    let refreshFails = scenario.observed === 'unknown';
    await page.route('**/*', route => {
      const { pathname } = new URL(route.request().url());
      const json = (value: unknown, status = 200) => route.fulfill({ status, json: value });
      if (pathname === '/instance') return json({ name: 'Sylux' });
      if (pathname === '/auth/csrf') return json({ csrf_token: 'csrf' });
      if (pathname === '/auth/local') return json({ account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' });
      if (pathname === '/folders/children' || pathname === '/folders/root/children') {
        return json({ root, items: [folder], next_cursor: null, has_more: false });
      }
      if (pathname === '/entries/project/direct-grants' && route.request().method() === 'GET') {
        reads++;
        if (deletes && refreshFails) return json({ code: 'unavailable' }, 503);
        const removed = deletes && scenario.observed === 'absent';
        return json({ items: removed ? [unaffected] : [grant, unaffected] });
      }
      if (pathname === '/direct-grants/grant' && route.request().method() === 'DELETE') {
        deletes++;
        if (scenario.outcome === 'network') return route.abort('connectionreset');
        if (scenario.outcome === 'missing') return json({ code: 'not_found' }, 404);
        return json({ code: 'unavailable' }, 503);
      }
      return route.continue();
    });

    await page.goto('/');
    await page.getByLabel('Account name').fill('owner');
    await page.getByLabel('Password', { exact: true }).fill('password');
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await page.getByRole('button', { name: 'Options for Project' }).press('Enter');
    await page.getByRole('menuitem', { name: 'Share', exact: true }).press('Enter');

    const dialog = page.getByRole('dialog', { name: 'Share Project' });
    const revoke = dialog.getByRole('button', { name: 'Revoke access for member' });
    await revoke.press('Enter');
    if (scenario.observed === 'unknown') {
      await expect(dialog.getByRole('alert')).toHaveText('Could not confirm revocation. Refresh access before trying again.');
      await expect(revoke).toHaveCount(0);
      const refresh = dialog.getByRole('button', { name: 'Refresh access' });
      await expect(refresh).toBeFocused();
      refreshFails = false;
      await refresh.press('Enter');
      await expect(revoke).toBeEnabled();
      await expect(dialog.getByRole('alert')).toHaveCount(0);
    } else if (scenario.observed === 'absent') {
      await expect(dialog.getByRole('status')).toHaveText('member has no direct access to this folder.');
      await expect(revoke).toHaveCount(0);
      await expect(dialog.getByLabel('Account name')).toBeFocused();
    } else {
      await expect(dialog.getByRole('status')).toHaveText('member still has direct access to this folder.');
      await expect(revoke).toBeEnabled();
      await expect(revoke).toBeFocused();
    }
    await expect(dialog.getByRole('button', { name: 'Revoke access for colleague' })).toBeEnabled();
    expect(reads).toBe(scenario.observed === 'unknown' ? 3 : 2);
    expect(deletes).toBe(1);
  });
}
