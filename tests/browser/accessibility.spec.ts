import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import type { Browser, BrowserContext, Page } from '../../web/node_modules/playwright-core';

const requireWeb = createRequire(path.resolve(__dirname, '../../web/package.json'));
const { test, expect, chromium }: typeof import('../../web/node_modules/@playwright/test') = requireWeb('@playwright/test');
const origin = process.env.S1_HTTPS_ORIGIN ?? '';
const fixture = process.env.S1_BROWSER_FIXTURES
  ? JSON.parse(process.env.S1_BROWSER_FIXTURES) as { handle: string; folder: string; nested: string; file: string }
  : {} as { handle: string; folder: string; nested: string; file: string };
let browser: Browser;
let context: BrowserContext;
let page: Page;

async function signIn(): Promise<void> {
  await page.goto(origin);
  await page.getByLabel('Account name').fill(fixture.handle);
  await page.getByLabel('Password').fill(process.env.S1_BROWSER_PASSWORD!);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
}

test.beforeAll(async ({}, workerInfo) => {
  if (!origin || !process.env.S1_BROWSER_FIXTURES || !process.env.S1_BROWSER_FIXTURE || !process.env.S1_BROWSER_PHASE || !process.env.S1_BROWSER_PASSWORD || !process.env.S1_CA_CERT || !process.env.S1_CERTUTIL) {
    throw new Error('owned browser fixture required');
  }
  if (!fs.lstatSync(path.join(process.env.S1_BROWSER_FIXTURE, '.fixture-owner')).isFile()) throw new Error('owned fixture required');
  const home = path.join(process.env.S1_BROWSER_FIXTURE!, `browser-${process.env.S1_BROWSER_PHASE}`, `accessibility-${workerInfo.workerIndex}-${process.pid}`);
  fs.mkdirSync(path.join(home, '.pki/nssdb'), { recursive: true, mode: 0o700 });
  const certutil = process.env.S1_CERTUTIL!;
  const nss = path.join(home, '.pki/nssdb');
  execFileSync(certutil, ['-N', '--empty-password', '-d', `sql:${nss}`]);
  execFileSync(certutil, ['-A', '-d', `sql:${nss}`, '-n', 'Sylux disposable CA', '-t', 'C,,', '-i', process.env.S1_CA_CERT!]);
  browser = await chromium.launch({
    executablePath: chromium.executablePath(), headless: true,
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: home, XDG_CACHE_HOME: home },
    args: ['--host-resolver-rules=MAP fixture.test 127.0.0.1', '--no-proxy-server'],
  });
  expect(browser.version()).toBe('158.0.8127.18');
});
test.beforeEach(async () => {
  context = await browser.newContext({ ignoreHTTPSErrors: false, viewport: { width: 1280, height: 800 } });
  await context.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (url.hostname === 'fixture.test' && url.port === new URL(origin).port && url.protocol === 'https:') await route.continue();
    else await route.abort('blockedbyclient');
  });
  page = await context.newPage();
});
test.afterEach(async () => { await context?.close(); });
test.afterAll(async () => { await browser?.close(); });

test('keyboard-only sign-in exposes named fields and a visible focus indicator', { annotation: { type: 'harness_case', description: 'accessibility_keyboard_sign_in' } }, async () => {
  await page.goto(origin);
  const accountName = page.getByLabel('Account name');
  await expect(accountName).toBeVisible();
  const unfocusedShadow = await accountName.evaluate(element => getComputedStyle(element).boxShadow);
  for (let tabs = 0; tabs < 12 && !await accountName.evaluate(element => element === document.activeElement); tabs++) {
    await page.keyboard.press('Tab');
  }
  await expect(accountName).toBeFocused();
  await expect(page.getByLabel('Password')).toHaveAttribute('type', 'password');
  const focusStyle = await accountName.evaluate(element => {
    const style = getComputedStyle(element);
    return { outline: style.outlineStyle !== 'none' && parseFloat(style.outlineWidth) > 0, shadow: style.boxShadow };
  });
  expect(focusStyle.outline || focusStyle.shadow !== 'none' && focusStyle.shadow !== unfocusedShadow).toBe(true);
  await page.keyboard.type(fixture.handle);
  await page.keyboard.press('Tab');
  await expect(page.getByLabel('Password')).toBeFocused();
  await page.keyboard.type(process.env.S1_BROWSER_PASSWORD!);
  await page.keyboard.press('Tab');
  await expect(page.getByRole('button', { name: /sign in/i })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
});

test('folder, upload, rename, and logout controls have accessible names', { annotation: { type: 'harness_case', description: 'accessibility_control_names' } }, async () => {
  await signIn();
  await expect(page.getByLabel('Folders')).toBeVisible();
  await page.getByRole('button', { name: 'Create', exact: true }).click();
  await expect(page.getByLabel('Folder name', { exact: true })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(page.getByLabel('Choose a file to upload')).toBeAttached();
  await expect(page.getByRole('button', { name: 'Refresh folder' })).toBeVisible();
  const folders = page.getByLabel('Folders');
  await folders.getByRole('button', { name: new RegExp(fixture.folder) }).click();
  await page.getByRole('article').filter({ hasText: fixture.nested }).getByRole('button', { name: 'Open' }).click();
  const item = page.getByRole('article').filter({ hasText: fixture.file });
  await item.getByRole('button', { name: `Options for ${fixture.file}`, exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'Download', exact: true })).toBeVisible();
  await expect(page.getByRole('menuitem', { name: 'Rename', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Log out' })).toBeVisible();
});

test('rename dialog supports Escape and restores focus to its invoking control', { annotation: { type: 'harness_case', description: 'accessibility_rename_escape_focus' } }, async () => {
  await signIn();
  const folders = page.getByLabel('Folders');
  await folders.getByRole('button', { name: new RegExp(fixture.folder) }).click();
  await page.getByRole('article').filter({ hasText: fixture.nested }).getByRole('button', { name: 'Open' }).click();
  const rename = page.getByRole('article').filter({ hasText: fixture.file }).getByRole('button', { name: `Options for ${fixture.file}`, exact: true });
  await rename.click();
  await page.getByRole('menuitem', { name: 'Rename', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: /rename/i });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('textbox', { name: /new name/i })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(rename).toBeFocused();
});

test('live failure state is announced and has a text retry action', { annotation: { type: 'harness_case', description: 'accessibility_live_failure_retry' } }, async () => {
  await signIn();
  await page.route('**/folders/*/children**', route => route.abort('failed'));
  await page.getByRole('button', { name: 'Refresh folder' }).click();
  const error = page.getByRole('alert');
  await expect(error).toContainText(/open this folder/i);
  await expect(error.getByRole('button', { name: /try again/i })).toBeVisible();
  await page.unrouteAll({ behavior: 'ignoreErrors' });
});
