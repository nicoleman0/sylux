import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';

const require = createRequire(new URL('../web/package.json', import.meta.url));
const { chromium, expect } = require('@playwright/test');
const fixture = process.argv[2];
const version = process.env.SYLUX_QUALIFY_VERSION || 'v0.1.0-beta.9';
const mode = process.env.SYLUX_QUALIFY_DOCKER || 'clean';
const origin = 'https://127.0.0.1:3000';
const handle = 'fixture-admin';
const password = randomBytes(24).toString('hex');
const payload = randomBytes(1024);
const filename = 'first-upload.bin';
const phasePath = '/opt/sylux/phase';
const authorizationPath = '/srv/sylux/secrets/setup.authorization';
const installer = process.env.SYLUX_QUALIFY_WRAPPER
  ? ['bash', process.env.SYLUX_QUALIFY_WRAPPER, process.env.SYLUX_QUALIFY_INSTALLER, process.env.SYLUX_QUALIFY_BUNDLE, version, '--yes']
  : ['bash', 'scripts/install-sylux.sh', version, '--yes'];
const dockerPackageNames = mode === 'existing' ? ['containerd.io', 'docker-ce', 'docker-ce-cli'] : ['docker.io', 'docker-compose-v2'];
const compose = ['docker', '--host', 'unix:///var/run/docker.sock', 'compose', '--project-directory', '/opt/sylux'];

function run(file, args, input) {
  return execFileSync(file, args, { encoding: 'utf8', stdio: 'pipe', input, timeout: 240_000 });
}

function sudo(...args) {
  return run('sudo', args);
}

function composeSnapshot() {
  const plugin = sudo('docker', 'info', '--format', '{{range .ClientInfo.Plugins}}{{if eq .Name "compose"}}{{.Path}}{{end}}{{end}}').trim();
  assert(plugin.startsWith('/'), 'Compose plugin path is unavailable');
  return [sudo('docker', 'compose', 'version'), plugin, sudo('sha256sum', plugin)];
}

function container() {
  const id = sudo('docker', 'ps', '--all', '--no-trunc', '--quiet',
    '--filter', 'label=com.docker.compose.project=sylux').trim();
  assert.match(id, /^[a-f0-9]{64}$/);
  return id;
}

function started(id) {
  return sudo('docker', 'inspect', '--format', '{{.State.StartedAt}}', id);
}

function rerun(expected, token) {
  const result = sudo('env', 'DOCKER_HOST=unix:///dev/null', 'COMPOSE_FILE=/dev/null',
    'SYLUX_DATA_DIR=/unused', ...installer);
  assert(result.includes(`Sylux is ready at ${origin}`));
  assert(result.includes(expected));
  assert(!result.includes(token) && !result.includes(password), 'Credential appeared in installer output');
}

async function session(browser, journey) {
  const context = await browser.newContext({ ignoreHTTPSErrors: false, acceptDownloads: true });
  try {
    const page = await context.newPage();
    await page.goto(origin);
    await journey(page);
  } finally {
    await context.close();
  }
}

async function signIn(page) {
  await page.getByLabel('Account name').fill(handle);
  await page.getByLabel('Password').fill(password);
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Files', level: 1 })).toBeVisible();
}

async function verifyDownload(page) {
  await expect(page.getByText(filename, { exact: true })).toBeVisible();
  await page.getByRole('button', { name: `Options for ${filename}`, exact: true }).click();
  const [download] = await Promise.all([
    page.waitForEvent('download'),
    page.getByRole('menuitem', { name: 'Download', exact: true }).click(),
  ]);
  assert.equal(download.suggestedFilename(), filename);
  assert(readFileSync(await download.path()).equals(payload), 'Downloaded bytes differed');
}

let stage = 'installing Docker, Compose and Sylux on a clean host';
let browser;
try {
  const initialPackages = mode === 'existing' ? run('dpkg-query', ['-W', ...dockerPackageNames]) : null;
  const initialCompose = mode === 'existing' ? composeSnapshot() : null;
  const initialInvocation = mode === 'existing' ? sudo('systemctl', 'show', '--property=InvocationID', '--value', 'docker.service') : null;
  const first = sudo(...installer);
  if (initialPackages) {
    assert.equal(run('dpkg-query', ['-W', ...dockerPackageNames]), initialPackages);
    assert.deepEqual(composeSnapshot(), initialCompose);
    assert.equal(sudo('systemctl', 'show', '--property=InvocationID', '--value', 'docker.service'), initialInvocation);
  }
  assert(first.includes(`Sylux is ready at ${origin}`));
  assert(first.includes('Choose First-time setup.'));
  const token = sudo('-u', 'sylux', 'cat', authorizationPath);
  assert(/^[a-f0-9]{64}$/.test(token), 'Invalid fixture authorization');
  assert(!first.includes(token), 'Authorization appeared in installer output');
  const dockerPackages = run('dpkg-query', ['-W', ...dockerPackageNames]);
  const tlsDir = '/opt/sylux/tls';
  sudo('openssl', 'verify', '-CAfile', `${tlsDir}/ca.crt`, '-verify_ip', '127.0.0.1', `${tlsDir}/cert.pem`);
  writeFileSync(path.join(fixture, 'ca.crt'), sudo('cat', `${tlsDir}/ca.crt`), { mode: 0o600 });
  console.log('The installer prepared Docker, Compose, the data root and verified loopback HTTPS.');

  stage = 'opening the isolated Chromium profile';
  const home = path.join(fixture, 'browser');
  const nss = path.join(home, '.pki', 'nssdb');
  mkdirSync(nss, { recursive: true, mode: 0o700 });
  run('certutil', ['-N', '--empty-password', '-d', `sql:${nss}`]);
  run('certutil', ['-A', '-d', `sql:${nss}`, '-n', 'Sylux installer CA', '-t', 'C,,',
    '-i', path.join(fixture, 'ca.crt')]);
  browser = await chromium.launch({
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: home, XDG_CACHE_HOME: home, XDG_DATA_HOME: home },
    args: ['--no-proxy-server'],
    downloadsPath: path.join(fixture, 'downloads'),
  });
  stage = 'refusing uncertain initialization';
  const id = container();
  const before = started(id);
  const recordedPhase = sudo('cat', phasePath);
  run('sudo', ['tee', phasePath], 'initializing\n');
  try {
    let rejected = false;
    try {
      sudo(...installer);
    } catch (error) {
      rejected = error.status === 1 && error.stderr.includes('Initialization was interrupted');
    }
    assert(rejected, 'The installer accepted uncertain initialization');
    assert.equal(sudo('cat', phasePath), 'initializing\n');
    assert.equal(container(), id);
    assert.equal(started(id), before);
  } finally {
    run('sudo', ['tee', phasePath], recordedPhase);
  }
  console.log('An uncertain init record was refused without replacing or restarting the service.');

  stage = 'interrupting browser setup and reusing Docker';
  await session(browser, async page => {
    await page.getByRole('button', { name: 'First-time setup' }).click();
    await page.getByLabel('One-use setup authorization').fill(token);
  });
  const dockerInvocation = sudo('systemctl', 'show', '--property=InvocationID', '--value', 'docker.service');
  assert(/^[a-f0-9]{32}$/.test(dockerInvocation.trim()));
  rerun('Choose First-time setup.', token);
  assert.equal(sudo('systemctl', 'show', '--property=InvocationID', '--value', 'docker.service'), dockerInvocation);
  assert.equal(run('dpkg-query', ['-W', ...dockerPackageNames]), dockerPackages);
  assert(sudo('-u', 'sylux', 'cat', authorizationPath) === token);
  assert.equal(container(), id);
  assert.equal(started(id), before);
  console.log('Unsubmitted browser setup resumed with the same authorization and running Docker service.');

  stage = 'creating the administrator and uploading through the browser';
  await session(browser, async page => {
    await page.getByRole('button', { name: 'First-time setup' }).click();
    await page.getByLabel('One-use setup authorization').fill(token);
    await page.getByLabel('Account name').fill(handle);
    await page.getByLabel('Password').fill(password);
    await page.getByRole('button', { name: 'Create administrator' }).click();
    await expect(page.getByRole('status')).toContainText('Setup complete');
    await signIn(page);
    await page.getByLabel('Choose files to upload').setInputFiles({
      name: filename, mimeType: 'application/octet-stream', buffer: payload,
    });
    await verifyDownload(page);
  });
  console.log('Trusted Chromium created the administrator and uploaded and downloaded the first file.');

  stage = 'restarting and signing in';
  sudo(...compose, 'restart', 'sylux');
  await expect.poll(() => {
    try {
      return run('curl', ['-q', '--silent', '--fail', '--noproxy', '*', '--max-time', '2',
        '--cacert', path.join(fixture, 'ca.crt'), '--output', '/dev/null', '--write-out', '%{http_code}', `${origin}/readyz`]);
    } catch {
      return '';
    }
  }, { timeout: 20_000 }).toBe('204');
  const restarted = started(id);
  assert.notEqual(restarted, before);
  await session(browser, async page => {
    await signIn(page);
    await verifyDownload(page);
  });

  stage = 'rerunning without replacing the service';
  rerun('Sign in with your existing administrator credentials.', token);
  assert.equal(container(), id);
  assert.equal(started(id), restarted);
  await session(browser, async page => {
    await signIn(page);
    await verifyDownload(page);
  });
  console.log('A fresh sign-in and byte-exact download passed after restart and unchanged same-version rerun.');

  stage = 'refusing a different release';
  const wrongTag = [...installer];
  wrongTag[wrongTag.length - 2] = 'v999.0.0-beta.1';
  const stableStart = started(id);
  const stablePhase = sudo('cat', phasePath);
  assert.throws(() => sudo(...wrongTag), error => error.status === 1 && error.stderr.includes('install record does not match'));
  assert.equal(container(), id);
  assert.equal(started(id), stableStart);
  assert.equal(sudo('cat', phasePath), stablePhase);
  await session(browser, async page => { await signIn(page); await verifyDownload(page); });

  stage = 'starting the existing stopped service';
  sudo(...compose, 'stop', 'sylux');
  rerun('Sign in with your existing administrator credentials.', token);
  assert.equal(container(), id);
  assert.notEqual(started(id), restarted);
  await session(browser, async page => {
    await signIn(page);
    await verifyDownload(page);
  });
  console.log('Same-version rerun also started the existing stopped container and preserved the upload.');
} catch (error) {
  if (error instanceof assert.AssertionError && !error.generatedMessage) console.error(error.message);
  // Browser errors may include form data. Report the failing journey only.
  console.error(`Installer qualification failed while ${stage}.`);
  process.exitCode = 1;
} finally {
  await browser?.close();
}
