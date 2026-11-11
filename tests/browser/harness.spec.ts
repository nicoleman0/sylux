import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { createHash, randomBytes } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { env } from 'node:process';
import type { Browser, BrowserContext } from '../../web/node_modules/playwright-core';

const requireWeb = createRequire(path.resolve(__dirname, '../../web/package.json'));
const { test, expect, chromium }: typeof import('../../web/node_modules/@playwright/test') = requireWeb('@playwright/test');
let browser: Browser;
let context: BrowserContext;
let engine: string;
let executableSha256: string;
const origin = process.env.S1_HTTPS_ORIGIN ?? '';
const output = process.env.S1_BROWSER_OUTPUT ?? '';
const fixture = process.env.S1_BROWSER_FIXTURE ?? '';

// Every network request from fixture pages is restricted to the owned retained
// listener ports. fixture.test is a reserved test domain mapped in this process
// only; no ambient proxy, public DNS, global trust, TLS-ignore, traces or HAR.
test.beforeAll(async () => {
  if (!origin || !output || !fixture || !env.S1_BROWSER_FIXTURES || !env.S1_BROWSER_PHASE || !env.S1_CA_CERT || !env.S1_CERTUTIL || !env.S1_UNRELATED_ORIGIN) {
    throw new Error('owned browser fixture required');
  }
  if (!fs.lstatSync(path.join(fixture, '.fixture-owner')).isFile()) throw new Error('owned fixture required');
  const home = path.join(fixture, `browser-${process.env.S1_BROWSER_PHASE}`);
  // Another spec may already have created a nested per-browser home below this
  // shared phase directory. Reuse the owned parent and initialize its NSS DB.
  fs.mkdirSync(home, { recursive: true, mode: 0o700 });
  const nss = path.join(home, '.pki/nssdb');
  fs.mkdirSync(nss, { recursive: true, mode: 0o700 });
  const certutil = process.env.S1_CERTUTIL!;
  const run = (args: string[]) => execFileSync(certutil, args, { timeout: 10_000, stdio: 'ignore' });
  run(['-N', '--empty-password', '-d', `sql:${nss}`]);
  run(['-A', '-d', `sql:${nss}`, '-n', 'Sylux disposable CA', '-t', 'C,,', '-i', process.env.S1_CA_CERT!]);
  const executable = chromium.executablePath();
  const hash = createHash('sha256');
  for await (const chunk of fs.createReadStream(executable)) hash.update(chunk);
  executableSha256 = hash.digest('hex');
  browser = await chromium.launch({
    executablePath: executable,
    headless: true,
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: home, XDG_CACHE_HOME: home },
    args: ['--host-resolver-rules=MAP fixture.test 127.0.0.1', '--no-proxy-server'],
  });
  engine = browser.version();
  expect(engine).toBe('158.0.8127.18');
  context = await browser.newContext({ ignoreHTTPSErrors: false, acceptDownloads: false });
  const ports = new Set([new URL(origin).port, new URL(process.env.S1_UNRELATED_ORIGIN!).port]);
  await context.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (['fixture.test', '127.0.0.1'].includes(url.hostname) && ports.has(url.port) && ['https:', 'http:'].includes(url.protocol)) {
      await route.continue();
    } else await route.abort('blockedbyclient');
  });
});
test.afterAll(async () => {
  if (context) await context.close();
  if (browser) {
    await browser.close();
    fs.writeFileSync(path.join(output, 'engine.json'), JSON.stringify({
      engine, executable_sha256: executableSha256,
      playwright: requireWeb('@playwright/test/package.json').version,
      isolated_nss: true, normal_tls_verification: true, closed: true,
    }), { flag: 'wx', mode: 0o600 });
  }
});

test('trusted_assets', { annotation: { type: 'harness_case', description: 'trusted_assets' } }, async () => {
  const page = await context.newPage();
  try {
    const response = await page.goto(origin, { waitUntil: 'networkidle' });
    expect(response?.status()).toBe(200);
    expect(createHash('sha256').update(await response!.body()).digest('hex')).toBe(process.env.S1_ASSET_SHA256);
    expect(await page.locator('script[src]').count()).toBeGreaterThan(0);
  } finally { await page.close(); }
});
test('trusted_api_routes', { annotation: { type: 'harness_case', description: 'trusted_api_routes' } }, async () => {
  const apiFixture = JSON.parse(env.S1_BROWSER_FIXTURES!);
  const page = await context.newPage();
  try {
    await page.goto(origin, { waitUntil: 'networkidle' });
    const result = await page.evaluate(async ({ handle, password, folderName, nestedId, fileName }: {
      handle: string; password: string; folderName: string; nestedId: string; fileName: string;
    }) => {
      const pre = await fetch('/auth/csrf');
      const preBody = await pre.json();
      const login = await fetch('/auth/local', {
        method: 'POST',
        headers: { 'content-type': 'application/json', 'x-csrf-token': preBody.csrf_token },
        body: JSON.stringify({ handle, password }),
      });
      const loginBody = await login.json();
      const rootResponse = await fetch('/folders/children');
      const root = await rootResponse.json();
      const folder = root.items.find((item: { name: string }) => item.name === folderName);
      const nestedResponse = await fetch(`/folders/${encodeURIComponent(nestedId)}/children`);
      const nested = await nestedResponse.json();
      return {
        csrfStatus: pre.status,
        loginStatus: login.status,
        loginHasCsrf: typeof loginBody.csrf_token === 'string',
        rootStatus: rootResponse.status,
        folderFound: Boolean(folder),
        nestedStatus: nestedResponse.status,
        fileFound: nested?.items?.some((item: { name: string }) => item.name === fileName),
      };
    }, {
      handle: env.S1_BROWSER_HANDLE!,
      password: env.S1_BROWSER_PASSWORD!,
      folderName: apiFixture.folder,
      nestedId: apiFixture.nested_id,
      fileName: apiFixture.file,
    });
    expect(result).toEqual({
      csrfStatus: 200,
      loginStatus: 200,
      loginHasCsrf: true,
      rootStatus: 200,
      folderFound: true,
      nestedStatus: 200,
      fileFound: true,
    });
  } finally { await page.close(); }
});
test('wrong_san', { annotation: { type: 'harness_case', description: 'wrong_san' } }, async () => {
  const page = await context.newPage();
  try {
    await expect(page.goto(origin.replace('fixture.test', '127.0.0.1'))).rejects.toThrow('net::ERR_CERT_COMMON_NAME_INVALID');
  } finally { await page.close(); }
});
test('unrelated_ca', { annotation: { type: 'harness_case', description: 'unrelated_ca' } }, async () => {
  const page = await context.newPage();
  try {
    await expect(page.goto(process.env.S1_UNRELATED_ORIGIN!)).rejects.toThrow('net::ERR_CERT_AUTHORITY_INVALID');
  } finally { await page.close(); }
});
test('plaintext_and_cookie_policy', { annotation: { type: 'harness_case', description: 'plaintext_and_cookie_policy' } }, async () => {
  const value = randomBytes(32).toString('hex');
  await context.addCookies([{ name: '__Host-sylux', value, url: origin, secure: true, httpOnly: true, sameSite: 'Lax' }]);
  const cookies = await context.cookies(origin);
  expect(cookies.some(c => c.name === '__Host-sylux' && c.value === value && c.secure && c.httpOnly && c.domain === 'fixture.test' && c.path === '/')).toBe(true);
  // Cookies are host/path scoped, NOT port scoped. This reserved non-localhost
  // name avoids Chromium's intentional Secure-cookie localhost HTTP exception.
  expect(await context.cookies(origin.replace('https:', 'http:'))).toHaveLength(0);
  expect(await context.cookies(origin.replace('fixture.test', '127.0.0.1'))).toHaveLength(0);
  const page = await context.newPage();
  try {
    const request = page.waitForRequest(origin + '/');
    await page.goto(origin);
    expect((await (await request).allHeaders()).cookie?.includes(value)).toBe(true);
    let plaintextHeaders: Record<string, string> | undefined;
    page.once('request', async request => { plaintextHeaders = await request.allHeaders(); });
    await expect(page.goto(origin.replace('https:', 'http:'))).rejects.toThrow(/net::ERR_(EMPTY_RESPONSE|CONNECTION_RESET|CONNECTION_CLOSED|INVALID_HTTP_RESPONSE)/);
    expect(plaintextHeaders?.cookie).toBeUndefined();
  } finally {
    await page.close();
    await context.clearCookies();
  }
});
