#!/usr/bin/env node
// Opt-in local benchmark. No credentials or fixture paths enter evidence.
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { createHash, X509Certificate } from 'node:crypto';
import { createRequire } from 'node:module';
import { readFileSync, mkdirSync, writeFileSync, appendFileSync, statfsSync } from 'node:fs';
import { resolve } from 'node:path';
import { parseArgs, redactFailure, validatePage, validateSearch, validateTrash, validatePath, observeChild, stopChild } from './workflow-measurement-helpers.mjs';
import { cpus, totalmem, release, platform, arch } from 'node:os';
import https from 'node:https';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';

if (process.versions.node !== '24.22.2') throw new Error('Use pinned Node 24.22.2 from .nvmrc');
const args = process.argv.slice(2);
const { quick, profile, transfersOnly, searchOnly, searchMatrix, withoutSearch, scales } = parseArgs(args);
const repetitions = quick ? 2 : 7;
const contentSizes = quick ? [4 * 1024 ** 2, 8 * 1024 ** 2] : [256 * 1024 ** 2, 1024 ** 3];
const output = resolve('reports/syl-48', `${new Date().toISOString().replaceAll(':', '-')}-${quick ? 'quick' : 'full'}`);
mkdirSync(output, { recursive: true, mode: 0o700 });
const snapshot = resolve(output, 'harness-snapshot');
mkdirSync(snapshot, { mode: 0o700 });
const samples = [], resources = [], dimensions = [], lifecycle = [];
const rawFile = resolve(output, 'samples.jsonl');
const files = ['Cargo.toml', 'tests/performance_fixture.rs', 'scripts/measure-workflows.mjs', 'scripts/workflow-measurement-helpers.mjs', 'scripts/workflow-measurement-helpers.test.mjs'];
const run = (cmd, argv) => {
  const r = spawnSync(cmd, argv, { encoding: 'utf8', timeout: 15000 });
  if (r.status !== 0) throw new Error(`${cmd} failed`);
  return r.stdout.trim();
};
const evidence = {
  schema: 1, command: `${process.execPath} scripts/measure-workflows.mjs${args.length ? ' ' + args.join(' ') : ''}`,
  source: null,
  fixtureWorkerThreads: 4, ttfbDefinition: 'request start to response headers callback; headers-arrival proxy', profile, quick, scales, repetitions, contentSizes, transfersOnly, searchOnly, searchMatrix, withoutSearch, exclusions: withoutSearch ? ['all account search', 'search during upload', 'repeated typing', 'search queue probes'] : searchMatrix ? ['browser journeys', 'uploads and content', 'standalone browse cases'] : [],
  started: new Date().toISOString(), node: process.versions.node,
  machine: { os: platform(), release: release(), architecture: arch(), cpu: cpus()[0].model, logicalCpus: cpus().length, ramBytes: totalmem() },
  harnessHashes: {},
  cache: 'Fresh process and fresh fixture per scale. Uploads and seeding warm OS/SQLite caches. No disk-cache eviction. first_request is not disk-cold. HTTPS keepalive enabled; no browser HTTP cache (product no-store).',
  percentileMethod: 'nearest rank; exploratory sparse samples, p99 is not a stable tail estimate',
};
const record = value => { samples.push(value); appendFileSync(rawFile, JSON.stringify(value) + '\n', { mode: 0o600 }); };
const progress = text => console.log(`[syl-48] ${text}`);
const webRequire = createRequire(resolve('web/package.json'));
let chromium;
evidence.browserChannel = 'installed Chrome; version recorded at run time';

let child, buildChild, browser, agent, timer, fixture, currentScale, pending = [], lines = [], active = new Set();
let requestCount = 0;
const interrupted = new AbortController();
const deadline = AbortSignal.any([AbortSignal.timeout(30 * 60 * 1000), interrupted.signal]);
function safeFailure(error) { return redactFailure(error, fixture, resolve('.')); }
function cancelActive() {
  for (const req of active) req.destroy(new Error('Measurement cancelled'));
  for (const item of pending.splice(0)) { clearTimeout(item.timer); item.reject(new Error('Measurement cancelled')); }
  if (buildChild?.pid && buildChild.exitCode === null) { try { process.kill(-buildChild.pid, 'SIGTERM'); } catch {} }
  browser?.close().catch(() => {});
}
deadline.addEventListener('abort', () => {
  if (!interrupted.signal.aborted) evidence.deadlineExceeded = true;
  cancelActive();
}, { once: true });
function onInterrupt(signal) {
  if (interrupted.signal.aborted) return;
  evidence.interruptedBy = signal;
  evidence.activeRequestsAtInterrupt = active.size;
  evidence.fixtureReadyAtInterrupt = fixture?.event === 'ready';
  interrupted.abort(new Error(`Interrupted by ${signal}`));
}
const onSigint = () => onInterrupt('SIGINT'), onSigterm = () => onInterrupt('SIGTERM');
process.on('SIGINT', onSigint); process.on('SIGTERM', onSigterm);
function checkInterrupted() { if (deadline.aborted) throw deadline.reason; }
function fixtureFilesystem(path) {
  const stats = statfsSync(path);
  const info = { scope: 'owned fixture path', type: stats.type, availableBytes: stats.bavail * stats.bsize, physical: null };
  if (platform() === 'darwin') {
    const disk = spawnSync('/usr/sbin/diskutil', ['info', '-plist', path], { encoding: 'utf8', timeout: 15000 });
    if (disk.status === 0) {
      const physical = {};
      for (const key of ['FilesystemType', 'FilesystemName', 'SolidState', 'BusProtocol']) {
        const value = disk.stdout.match(new RegExp(`<key>${key}</key>\\s*<(string|true|false)(?:>([^<]*)</string>|\\s*/>)`));
        if (value) physical[key] = value[1] === 'string' ? value[2] : value[1] === 'true';
      }
      info.physical = physical;
    }
  }
  return info;
}
async function buildFixture() {
  const command = ['test', '--locked', '--features', 'test-harness', '--test', 'performance_fixture', '--no-run', '--message-format=json', ...(profile === 'release' ? ['--release'] : [])];
  evidence.buildCommand = ['cargo', ...command];
  buildChild = spawn('cargo', command, { detached: true, env: { ...process.env, SYL48_SOURCE_SHA: evidence.source }, stdio: ['ignore', 'pipe', 'pipe'] });
  const exited = observeChild(buildChild);
  let artifact;
  buildChild.stderr.on('data', () => {});
  createInterface({ input: buildChild.stdout }).on('line', line => {
    try { const event = JSON.parse(line); if (event.reason === 'compiler-artifact' && event.target?.name === 'performance_fixture' && event.executable) artifact = event; } catch {}
  });
  let rejectCancellation;
  const cancelled = new Promise((_, reject) => { rejectCancellation = () => reject(deadline.reason); deadline.addEventListener('abort', rejectCancellation, { once: true }); });
  let result;
  try { result = await Promise.race([exited, cancelled]); }
  finally { deadline.removeEventListener('abort', rejectCancellation); }
  checkInterrupted();
  if (result.code !== 0 || !artifact) throw new Error('Cargo fixture build failed or executable artifact missing');
  executable = artifact.executable;
  evidence.executable = { sha256: createHash('sha256').update(readFileSync(executable)).digest('hex'), cargoProfile: artifact.profile, fresh: artifact.fresh };
  buildChild = undefined;
}
const stoppedChildren = new WeakSet();
async function stopFixture() {
  if (!child) return;
  const result = await stopChild(child);
  const stopped = lines.find(event => event.event === 'stopped');
  if (!stoppedChildren.has(child)) {
    lifecycle.push({ scale: currentScale, ...result, joined: stopped?.joined === true, cleaned: stopped?.cleaned === true, fixtureRetained: stopped?.cleaned !== true });
    stoppedChildren.add(child);
  }
  if (result.code !== 0 || !stopped?.joined || !stopped?.cleaned) throw new Error('Fixture did not cleanly join; disposable root retained');
}
function nextEvent(timeout = 120000) {
  checkInterrupted();
  if (lines.length) return Promise.resolve(lines.shift());
  return new Promise((resolveEvent, reject) => {
    const item = { resolve: resolveEvent, reject };
    item.timer = setTimeout(() => { pending = pending.filter(x => x !== item); reject(new Error('Fixture response deadline')); }, timeout);
    pending.push(item);
  });
}
async function startFixture(scale) {
  checkInterrupted();
  currentScale = scale; fixture = undefined; lines = [];
  child = spawn(executable, ['--run', String(scale)], { stdio: ['pipe', 'pipe', 'pipe'] });
  observeChild(child);
  child.stderr.on('data', () => {});
  child.on('error', () => { for (const p of pending.splice(0)) {clearTimeout(p.timer); p.reject(new Error('Fixture spawn failed'));} });
  child.on('exit', code => { if (code !== 0) for (const p of pending.splice(0)) {clearTimeout(p.timer); p.reject(new Error(`Fixture exited ${code}`));} });
  createInterface({ input: child.stdout }).on('line', line => {
    let event;
    try { event = JSON.parse(line); } catch { return; }
    if (pending.length) { const p = pending.shift(); clearTimeout(p.timer); p.resolve(event); } else lines.push(event);
  });
  fixture = await nextEvent();
  if (fixture.event !== 'ready') throw new Error('Fixture not ready');
  if (fixture.embedded_fixture_sha256 !== evidence.harnessHashes['tests/performance_fixture.rs'] || fixture.embedded_assets_index_sha256 !== evidence.assetsIndexSha256 || fixture.source_at_build !== evidence.source) throw new Error('Fixture embedded source attribution differs from current build inputs');
  evidence.embeddedAttribution = { fixtureSha256: fixture.embedded_fixture_sha256, assetsIndexSha256: fixture.embedded_assets_index_sha256, source: fixture.source_at_build };
  evidence.fixtureFilesystems ??= []; evidence.fixtureFilesystems.push({ scale, ...fixtureFilesystem(fixture.root) });
  dimensions.push({ scale, ...fixture.dimensions, schemaVersion: fixture.schema_version });
  const ca = readFileSync(fixture.ca);
  agent = new https.Agent({ ca, keepAlive: true, maxSockets: 32, lookup: (_host, _options, callback) => callback(null, [{ address: '127.0.0.1', family: 4 }]) });
  timer = setInterval(() => {
    const r = spawnSync('ps', ['-axo', 'pid=,ppid=,%cpu=,rss='], { encoding: 'utf8', timeout: 2000 });
    if (r.status !== 0) {
      evidence.resourceSamplingError = 'ps unavailable; CPU/RSS unmeasured';
      return;
    }
    const rows = r.stdout.trim().split('\n').map(line => {
      const [pid, parent, cpuPercent, rssKiB] = line.trim().split(/\s+/).map(Number);
      return { pid, parent, cpuPercent, rssKiB };
    });
    const owned = new Set([process.pid, child.pid]);
    for (let n = 0; n < 8; n++) for (const row of rows) if (owned.has(row.parent)) owned.add(row.pid);
    const processes = rows.filter(row => owned.has(row.pid)).map(row => ({ role: row.pid === child.pid ? 'fixture' : row.pid === process.pid ? 'runner' : 'browser_or_sampler_descendant', cpuPercent: row.cpuPercent, rssKiB: row.rssKiB }));
    const data = { scale, elapsedMs: performance.now(), processes };
    resources.push(data); appendFileSync(resolve(output, 'resources.jsonl'), JSON.stringify(data) + '\n', { mode: 0o600 });
  }, 1000);
}
function request(label, path, { method = 'GET', session = fixture.sessions[0], body, headers = {}, expect = 200, length, throttle = false, collect = true, pairId } = {}) {
  checkInterrupted();
  requestCount++;
  const started = performance.now();
  return new Promise((resolveResponse, reject) => {
    const req = https.request(new URL(path, fixture.origin), {
      method, agent, timeout: 120000, signal: deadline,
      headers: { Cookie: `__Host-sylux=${session.cookie}`, Origin: fixture.origin, 'X-CSRF-Token': session.csrf, ...headers,
        ...(body !== undefined ? { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(JSON.stringify(body)) } : {}),
        ...(length !== undefined ? { 'Content-Type': 'application/octet-stream', 'Content-Length': length } : {}) },
    }, res => {
      const ttfbMs = performance.now() - started;
      let bytes = 0; const chunks = []; const digest = createHash('sha256');
      res.on('data', chunk => { bytes += chunk.length; digest.update(chunk); if (collect && bytes <= 8 * 1024 ** 2) chunks.push(chunk); });
      res.on('error', reject);
      res.on('end', () => {
        const elapsedMs = performance.now() - started;
        const sample = { ...(pairId === undefined ? {} : { pairId, startedMs: started, finishedMs: performance.now() }), ...(length !== undefined ? { sentBytes: length, uploadMibPerSecond: length / 1024 ** 2 / (elapsedMs / 1000) } : {}), scale: fixture.dimensions.metadata_entries, kind: 'http', label, method, status: res.statusCode, elapsedMs, ttfbMs, bytes, mibPerSecond: bytes / 1024 ** 2 / (elapsedMs / 1000) };
        record(sample);
        if (res.statusCode !== expect) return reject(new Error(`${label} expected ${expect}, received ${res.statusCode}`));
        const text = collect ? Buffer.concat(chunks).toString('utf8') : '';
        let data; if (text && res.headers['content-type']?.includes('application/json')) {try {data = JSON.parse(text);} catch {return reject(new Error(`${label} invalid JSON`));}}
        resolveResponse({ data, headers: res.headers, bytes, digest: digest.digest('hex'), started, ended: performance.now() });
      });
    });
    active.add(req); req.on('close', () => active.delete(req));
    req.on('timeout', () => req.destroy(new Error(`${label} request timeout`)));
    req.on('error', error => { record({ ...(pairId === undefined ? {} : { pairId, startedMs: started, finishedMs: performance.now() }), kind: 'http_failure', scale: fixture.dimensions.metadata_entries, label, method, elapsedMs: performance.now() - started, error: error.code ?? error.name }); reject(error); });
    if (length !== undefined) {
      (async () => {
        const chunk = Buffer.alloc(65536, 97);
        for (let sent = 0; sent < length;) {
          const part = chunk.subarray(0, Math.min(chunk.length, length - sent));
          if (!req.write(part)) await once(req, 'drain', { signal: deadline });
          sent += part.length;
          if (throttle) await delay(2, undefined, { signal: deadline });
        }
        req.end();
      })().catch(error => req.destroy(error));
    } else req.end(body === undefined ? undefined : JSON.stringify(body));
  });
}
async function upload(session, name, length, label, observe = false) {
  const folder = (await request(`${label}:parent`, `/folders/${session.folder}/children`, { session })).data;
  // Folder descriptors come from the path route, not the child listing.
  const parent = (await request(`${label}:path`, `/folders/${session.folder}/path`, { session })).data.items.at(-1);
  if (!folder || !parent?.etag) throw new Error('Upload parent absent');
  const admission = { parent_id: session.folder, parent_etag: parent.etag, name, length };
  const ticket = (await request(`${label}:ticket`, '/mutation-tickets', { method: 'POST', session, body: { scope: 'upload_create', ...admission }, expect: 201 })).data;
  const headers = { 'X-Mutation-ID': ticket.ticket, 'If-None-Match': '*' };
  await request(`${label}:admit`, '/uploads', { method: 'POST', session, body: admission, headers, expect: 201 });
  const sending = request(`${label}:body`, `/uploads/${ticket.mutation_id}/content`, { method: 'PUT', session, headers, length, throttle: observe });
  sending.catch(() => {});
  let status;
  if (observe) status = await request(`${label}:progress`, `/uploads/${ticket.mutation_id}`, { session });
  const staged = (await sending).data;
  if (status) record({ kind: 'upload_progress', scale: fixture.dimensions.metadata_entries, label, ...Object.fromEntries(['bytes_received', 'length', 'writer_active', 'status'].map(k => [k, status.data[k]])) });
  const committed = (await request(`${label}:commit`, `/uploads/${ticket.mutation_id}/commit`, { method: 'POST', session, headers, body: { sha256: staged.sha256, parent_etag: parent.etag }, expect: 201 })).data;
  const entry = committed.entry_id ?? committed.result?.entry_id;
  if (!entry) throw new Error('Committed upload entry missing');
  return { id: entry, name, length };
}
async function deepMetadata() {
  const deepest = (0x1000 + 15).toString(16).padStart(32, '0');
  const path = await request('deep:path', `/folders/${deepest}/path`);
  validatePath(path.data, fixture.expectations.path);
  for (const sort of ['name_asc', 'name_desc', 'size_asc', 'size_desc', 'modified_asc', 'modified_desc']) {
    const response = await request(`deep:browse:${sort}`, `/folders/${deepest}/children?sort=${sort}&limit=100`);
    validatePage(response.data, fixture.expectations.deep[sort], fixture.dimensions.deep_live_children_by_layer.at(-1));
    record({ kind: 'deep_shape', scale: fixture.dimensions.metadata_entries, sort, firstPageRows: response.data.items.length, hasMore: response.data.has_more, pathEntries: path.data.items.length });
  }
}
async function searchMatrixSamples() {
  for (const [label, query] of [['selective', 'item-000099'], ['broad', 'item'], ['none', 'zzzz-no-match'], ['one', 'i']]) {
    for (let i = 0; i < repetitions; i++) {
      const pairId = `${fixture.dimensions.metadata_entries}:${label}:${i}`;
      const searching = request(`search:${label}`, `/entries/search?q=${query}&limit=100`, { pairId });
      // Overlap HTTP requests without imposing SQLite admission order.
      const browsing = request(`during_search:${label}:browse`, '/folders/children?sort=name_asc&limit=100', { pairId });
      const [found, listed] = await Promise.all([searching, browsing]);
      validateSearch(found.data, fixture.expectations.search_ids[label], fixture.expectations.search_counts[label]);
      validatePage(listed.data, fixture.expectations.root.name_asc, fixture.dimensions.wide_root_live_children);
    }
  }
}
async function apiMatrix() {
  await deepMetadata();
  const sorts = ['name_asc', 'name_desc', 'size_asc', 'size_desc', 'modified_asc', 'modified_desc'];
  for (const sort of sorts) {
    for (let i = 0; i < repetitions; i++) { const response = await request(`browse:${sort}`, `/folders/children?sort=${sort}&limit=100`); validatePage(response.data, fixture.expectations.root[sort], fixture.dimensions.wide_root_live_children); }
    let cursor; const seen = new Set(), cursors = new Set();
    for (let page = 0; page < 10; page++) {
      const response = await request(`pagination:${sort}:page-${page + 1}`, `/folders/children?sort=${sort}&limit=100${cursor ? '&cursor=' + encodeURIComponent(cursor) : ''}`);
      validatePage(response.data, fixture.expectations.root[sort], fixture.dimensions.wide_root_live_children, page * 100, seen);
      cursor = response.data.next_cursor; if (!cursor) break;
      if (cursors.has(cursor)) throw new Error('Listing cursor did not advance'); cursors.add(cursor);
    }
  }
  if (!withoutSearch) for (const [label, query] of [['selective', 'item-000099'], ['broad', 'item'], ['none', 'zzzz-no-match'], ['one', 'i']]) {
    for (let i = 0; i < repetitions; i++) { const response = await request(`search:${label}`, `/entries/search?q=${query}&limit=100`); validateSearch(response.data, fixture.expectations.search_ids[label], fixture.expectations.search_counts[label]); }
  }
  for (let i = 0; i < repetitions; i++) validateTrash((await request('trash', '/trash')).data, fixture.expectations.trash);
}
async function transfers() {
  for (const sessions of [1, 5, 10]) {
    progress(`${fixture.dimensions.metadata_entries}: 60 files with ${sessions} sessions`);
    const before = requestCount, started = performance.now(); let next = 0, done = false, observations = 0, searchObservations = 0, uploadWorkersFinishedAt;
    const observerStartedAt = performance.now();
    const workers = Array.from({ length: sessions }, (_, index) => (async () => {
      while (next < 60) {
        const n = next++;
        const entry = await upload(fixture.sessions[index], `batch-${sessions}-${n}.txt`, quick ? 65536 : 1048576, `batch:${sessions}`, true);
        if (n < 3) {
          const verified = await request(`batch:${sessions}:verify`, `/entries/${entry.id}/content`, { session: fixture.sessions[index], collect: false });
          const expected = createHash('sha256').update(Buffer.alloc(entry.length, 97)).digest('hex');
          if (verified.bytes !== entry.length || verified.digest !== expected) throw new Error('Real upload bytes differ');
        }
      }
    })());
    const all = Promise.all(workers).finally(() => { uploadWorkersFinishedAt = performance.now(); done = true; });
    all.catch(() => {});
    while (!done) {
      await request(`during_batch:${sessions}:browse`, '/folders/children?sort=modified_desc');
      if (!done && !withoutSearch && searchObservations === 0) { await request(`during_batch:${sessions}:search`, '/entries/search?q=item-000099'); searchObservations++; } observations++;
      if (!done) await delay(50);
    }
    const observerFinishedAt = performance.now();
    await all;
    const workloadFinishedAt = performance.now();
    record({ kind: 'batch', scale: fixture.dimensions.metadata_entries, sessions, files: 60, observerSearches: searchObservations, observerPauseMs: 50, bodyChunkBytes: 65536, bodyChunkDelayMs: 2, bytesPerFile: quick ? 65536 : 1048576, uploadWorkersMs: uploadWorkersFinishedAt - started, workloadWallMs: workloadFinishedAt - started, observerWallMs: observerFinishedAt - observerStartedAt, observerTailMs: Math.max(0, observerFinishedAt - uploadWorkersFinishedAt), elapsedMs: workloadFinishedAt - started, requestCount: requestCount - before, browseObservations: observations, browseSearchPairs: searchObservations });
  }
}
async function queueProbes(scale) {
    progress(`${scale}: queue/service probes`);
    for (let n = 0; n < (quick || transfersOnly ? 1 : 3); n++) {
      child.stdin.write(withoutSearch ? 'probes-fast\n' : 'probes\n'); const probe = await nextEvent();
      if (probe.event !== 'probes') throw new Error('Missing probes');
      for (const sample of probe.samples) record({ kind: 'probe', scale, ...sample });
    }
}
async function paint(page) { await page.evaluate(() => new Promise(resolvePaint => requestAnimationFrame(() => requestAnimationFrame(resolvePaint)))); }
async function browserMatrix() {
  const certificate = new X509Certificate(readFileSync(fixture.leaf));
  const spki = createHash('sha256').update(certificate.publicKey.export({ type: 'spki', format: 'der' })).digest('base64');
  browser = await chromium.launch({ channel: 'chrome', timeout: 15000, headless: true, args: [`--ignore-certificate-errors-spki-list=${spki}`, '--host-resolver-rules=MAP fixture.test 127.0.0.1', '--no-proxy-server'] });
  evidence.browser = browser.version();
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  const page = await context.newPage(); page.setDefaultTimeout(120000);
  let browserRequests = 0;
  const requestKinds = {};
  page.on('request', request => {
    browserRequests++;
    const path = new URL(request.url()).pathname.replace(/[0-9a-f]{32}/g, ':id');
    const operation = `${request.method()} ${path}`;
    requestKinds[operation] = (requestKinds[operation] ?? 0) + 1;
    record({ kind: 'browser_request', scale: fixture.dimensions.metadata_entries, operation, timeMs: performance.now() });
  });
  await page.addInitScript(() => {
    window.__uploadProgress = [];
    const send = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.send = function (...params) {
      this.upload.addEventListener('progress', e => window.__uploadProgress.push({ elapsedMs: performance.now(), loaded: e.loaded, total: e.total }));
      return send.apply(this, params);
    };
  });
  await page.goto(fixture.origin);
  await page.locator('#handle').fill('measure-owner'); await page.locator('#password').fill(fixture.password);
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.locator('#folder-sort').waitFor();
  async function action(label, act, predicate, searchQuery) {
    const count = browserRequests, started = performance.now();
    const response = page.waitForResponse(r => predicate(new URL(r.url()).pathname, new URL(r.url()).searchParams) && r.status() === 200);
    response.catch(() => {});
    await act(); const completedResponse = await response; await completedResponse.finished();
    if (searchQuery !== undefined) {
      const result = await completedResponse.json();
      const names = result.items.map(hit => hit.entry.name);
      await page.waitForFunction(({ query, names }) => {
        const section = document.querySelector('section[aria-label="Search results"]');
        return document.querySelector('#file-search')?.value === query && section?.getAttribute('aria-busy') === 'false' && JSON.stringify([...section.querySelectorAll('article')].map(row => row.getAttribute('aria-label'))) === JSON.stringify(names);
      }, { query: searchQuery, names });
    }
    await page.waitForFunction(() => ![...document.querySelectorAll('[role=status]')].some(e => /^(Loading (files|results|trash)|Searching)/.test(e.textContent ?? '')));
    await paint(page);
    record({ kind: 'browser', scale: fixture.dimensions.metadata_entries, label, elapsedMs: performance.now() - started, requestCount: browserRequests - count });
  }
  for (const sort of ['name_desc', 'name_asc', 'size_asc', 'size_desc', 'modified_asc', 'modified_desc']) {
    await action(`sort:${sort}`, () => page.locator('#folder-sort').selectOption(sort), (path, q) => path.endsWith('/children') && q.get('sort') === sort);
  }
  for (let n = 0; n < (quick ? 2 : 9); n++) await action(`page:${n + 2}`, () => page.getByRole('button', { name: 'Next page', exact: true }).click(), (path, q) => path.endsWith('/children') && q.has('cursor'));
  if (!withoutSearch) for (const query of ['item-000099', 'item', 'zzzz-no-match', 'i']) await action(`search:${query}`, () => page.locator('#file-search').fill(query), (path, q) => path === '/entries/search' && q.get('q') === query, query);
  if (!withoutSearch) await action('repeated_typing', () => page.locator('#file-search').pressSequentially('tem-000099', { delay: 35 }), (path, q) => path === '/entries/search' && q.get('q') === 'item-000099', 'item-000099');
  await action('files', () => page.getByRole('navigation', { name: 'Files', exact: true }).getByRole('button', { name: 'Files', exact: true }).click(), path => path.endsWith('/children'));
  await action('real_upload_folder', () => page.getByRole('button', { name: 'Open uploads-00', exact: true }).click(), path => path.endsWith('/children'));
  const bytes = Buffer.alloc(quick ? 65536 : 262144, 97);
  const uploads = Array.from({ length: 60 }, (_, n) => ({ name: `browser-${n}.txt`, mimeType: 'text/plain', buffer: bytes }));
  const started = performance.now(), before = browserRequests;
  const beforeKinds = { ...requestKinds };
  let committed = 0;
  const onResponse = response => { if (new URL(response.url()).pathname.endsWith('/commit') && response.status() === 201) committed++; };
  page.on('response', onResponse);
  await page.locator('#upload-file').setInputFiles(uploads);
  await page.waitForFunction(() => window.__uploadProgress.length > 0);
  // Reading remains available while the real UI transfer runs.
  if (committed === 60) throw new Error('UI upload finished before concurrent browsing');
  await action('browse_during_ui_upload', () => page.getByRole('navigation', { name: 'Files', exact: true }).getByRole('button', { name: 'Files', exact: true }).click(), path => path.endsWith('/children'));
  if (!withoutSearch && committed === 60) throw new Error('UI upload finished before concurrent search');
  if (!withoutSearch) await action('search_during_ui_upload', () => page.locator('#file-search').fill('item-000099'), (path, q) => path === '/entries/search' && q.get('q') === 'item-000099', 'item-000099');
  const until = performance.now() + 180000;
  while (committed < 60 && performance.now() < until) await delay(100);
  if (committed !== 60) { record({ kind: 'browser_batch_failure', scale: fixture.dimensions.metadata_entries, filesCommitted: committed, elapsedMs: performance.now() - started, requestCount: browserRequests - before }); throw new Error(`Browser upload batch deadline: ${committed}/60 committed`); }
  await page.getByText('60 items uploaded', { exact: true }).waitFor();
  await page.getByRole('button', { name: 'Cancel upload', exact: true }).waitFor({ state: 'hidden' });
  await paint(page);
  record({ kind: 'browser_batch', scale: fixture.dimensions.metadata_entries, files: committed, requestBreakdown: Object.fromEntries(Object.entries(requestKinds).map(([key, count]) => [key, count - (beforeKinds[key] ?? 0)]).filter(([, count]) => count)), elapsedMs: performance.now() - started, requestCount: browserRequests - before, progressEvents: await page.evaluate(() => window.__uploadProgress) });
  page.off('response', onResponse);
  await action('trash', () => page.getByRole('button', { name: 'Trash', exact: true }).click(), path => path === '/trash');

  await context.close(); await browser.close(); browser = undefined;
}
async function content() {
  for (const length of contentSizes) {
    progress(`${fixture.dimensions.metadata_entries}: real content ${length / 1024 ** 2} MiB`);
    const entry = await upload(fixture.sessions[0], `large-${length}.txt`, length, `large:${length}`);
    const path = `/entries/${entry.id}/content`;
    let etag;
    for (let i = 0; i < 3; i++) {
      const label = `content:${length}:${i === 0 ? 'first_request' : 'warm'}`;
      const head = await request(`${label}:head`, path, { method: 'HEAD' }); etag = head.headers.etag;
      if (Number(head.headers['content-length']) !== length) throw new Error('HEAD length mismatch');
      await request(`${label}:304`, path, { headers: { 'If-None-Match': etag }, expect: 304 });
      const range = await request(`${label}:range`, path, { headers: { Range: 'bytes=0-4095' }, expect: 206 });
      if (range.bytes !== 4096) throw new Error('Range length mismatch');
      const full = await request(`${label}:download`, path, { collect: false });
      const digest = createHash('sha256'); const chunk = Buffer.alloc(65536, 97);
      for (let n = 0; n < length; n += chunk.length) digest.update(chunk.subarray(0, Math.min(chunk.length, length - n)));
      if (full.bytes !== length || full.digest !== digest.digest('hex')) throw new Error('Full content mismatch');
    }
    const previewStarted = performance.now();
    try {
    const cert = new X509Certificate(readFileSync(fixture.leaf));
    const spki = createHash('sha256').update(cert.publicKey.export({ type: 'spki', format: 'der' })).digest('base64');
    browser = await chromium.launch({ channel: 'chrome', timeout: 15000, args: [`--ignore-certificate-errors-spki-list=${spki}`, '--host-resolver-rules=MAP fixture.test 127.0.0.1', '--no-proxy-server'] });
    const page = await browser.newPage(); page.setDefaultTimeout(120000);
    await page.goto(`${fixture.origin}/#files/${fixture.sessions[0].folder}?sort=name_desc`);
    await page.locator('#handle').fill('measure-owner'); await page.locator('#password').fill(fixture.password);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    const started = performance.now();
    await page.getByRole('button', { name: `Preview ${entry.name}`, exact: true }).click();
    await page.getByText('This file is larger than the 2 MiB preview limit. Download the file to open it.', { exact: true }).waitFor();
    await paint(page);
    record({ kind: 'browser', scale: fixture.dimensions.metadata_entries, label: `preview_size_fallback:${length}`, elapsedMs: performance.now() - started });
    await browser.close(); browser = undefined;
    } catch (error) {
      checkInterrupted();
      evidence.phaseFailures ??= [];
      evidence.phaseFailures.push({ scale: currentScale, phase: 'preview', reason: safeFailure(error) });
      record({ kind: 'phase_failure', scale: currentScale, label: `preview:${length}`, elapsedMs: performance.now() - previewStarted, error: safeFailure(error) });
    } finally { await browser?.close().catch(() => {}); browser = undefined; }

  }
}
let executable;
try {
  evidence.source = run('git', ['rev-parse', 'HEAD']);
  evidence.rustc = run('rustc', ['--version']);
  evidence.assetsIndexSha256 = createHash('sha256').update(readFileSync('web/dist/index.html')).digest('hex');
  for (const file of files) {
    mkdirSync(resolve(snapshot, file, '..'), { recursive: true, mode: 0o700 });
    const source = readFileSync(file); writeFileSync(resolve(snapshot, file), source, { mode: 0o600 });
    evidence.harnessHashes[file] = createHash('sha256').update(source).digest('hex');
  }
  writeFileSync(resolve(output, 'source.diff'), run('git', ['diff', 'HEAD']), { mode: 0o600 });
  ({ chromium } = webRequire('@playwright/test'));
  evidence.playwright = webRequire('@playwright/test/package.json').version;
  await buildFixture();
  for (const scale of scales) {
    progress(`starting ${scale} metadata fixture (${profile})`);
    await startFixture(scale);
    if (searchMatrix) {
      progress(`${scale}: four search shapes with concurrent browsing`);
      await searchMatrixSamples();
      await queueProbes(scale);
    } else if (searchOnly) {
      progress(`${scale}: deep folder/path and isolated bounded no-match search`);
      await deepMetadata();
      validateSearch((await request('search:probe_no_match', '/entries/search?q=zzzz-no-match')).data, fixture.expectations.search_ids.none, fixture.expectations.search_counts.none);
    } else if (transfersOnly) {
      await transfers();
      await queueProbes(scale);
    } else {
    await content();
    progress(`${scale}: API browse/search/trash`); await apiMatrix();
    progress(`${scale}: browser and real UI upload`);
    const browserStarted = performance.now();
    try { await browserMatrix(); } catch (error) {
      checkInterrupted();
      evidence.phaseFailures ??= [];
      evidence.phaseFailures.push({ scale, phase: 'browser', reason: safeFailure(error) });
      record({ kind: 'phase_failure', scale, label: 'browser', elapsedMs: performance.now() - browserStarted, error: safeFailure(error) });
      await browser?.close().catch(() => {}); browser = undefined;
      progress(`${scale}: browser phase failed; retaining evidence and continuing API measurements`);
    }
    await transfers();
    await queueProbes(scale);
    }
    clearInterval(timer); timer = undefined;
    await stopFixture(); child = undefined; agent.destroy();
  }
  evidence.status = evidence.phaseFailures?.length ? 'failed' : 'passed';
  if (evidence.status === 'failed') process.exitCode = 1;
} catch (error) {
  evidence.status = interrupted.signal.aborted ? 'interrupted' : 'failed'; evidence.failure = safeFailure(error);
  console.error(`[syl-48] ${evidence.failure}`); process.exitCode = 1;
} finally {
  clearInterval(timer);
  const draining = [...active].map(req => once(req, 'close').catch(() => {}));
  for (const req of active) req.destroy();
  await Promise.all(draining);
  if (browser) await browser.close().catch(() => {});
  if (buildChild) {
    try { lifecycle.push({ phase: 'build', ...await stopChild(buildChild, { requestStop: () => { try { process.kill(-buildChild.pid, 'SIGTERM'); } catch {} }, group: true }) }); }
    catch (error) { evidence.cleanupFailure = safeFailure(error); process.exitCode = 1; }
  }
  try { await stopFixture(); } catch (error) { evidence.cleanupFailure = safeFailure(error); process.exitCode = 1; if (evidence.status === 'passed') evidence.status = 'failed'; }
  agent?.destroy();
  const groups = new Map();
  for (const s of samples) {
    if (!Number.isFinite(s.elapsedMs)) continue;
    const key = `${s.scale}/${s.kind}/${s.label ?? s.sessions}`;
    const group = groups.get(key) ?? []; group.push(s.elapsedMs); groups.set(key, group);
  }
  const summary = [...groups].map(([group, values]) => {
    values.sort((a, b) => a - b); const percentile = p => values[Math.max(0, Math.ceil(p * values.length) - 1)];
    return { group, n: values.length, p50Ms: percentile(.5), p95Ms: percentile(.95), p99Ms: percentile(.99), minMs: values[0], maxMs: values.at(-1) };
  });
  if (deadline.aborted) { evidence.status = interrupted.signal.aborted ? 'interrupted' : 'failed'; process.exitCode = 1; }
  process.off('SIGINT', onSigint); process.off('SIGTERM', onSigterm);
  evidence.finished = new Date().toISOString(); evidence.dimensions = dimensions; evidence.lifecycle = lifecycle; evidence.httpRequests = requestCount; evidence.resourceSamples = resources.length;
  writeFileSync(resolve(output, 'run.json'), JSON.stringify(evidence, null, 2), { mode: 0o600 });
  writeFileSync(resolve(output, 'summary.json'), JSON.stringify(summary, null, 2), { mode: 0o600 });
  progress(`${evidence.status}: ${output.split('/').at(-1)}; ${samples.length} samples`);
}
