// Drives a running Sylux container through a small browse/upload/download
// workload and samples its memory from /proc and cgroup v2. Run through
// scripts/measure-image-footprint.sh, which owns the container and prints JSON.
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import https from 'node:https';
import { setTimeout as sleep } from 'node:timers/promises';

const [mode, origin, certPath, ...rest] = process.argv.slice(2);
let agent;
const getAgent = () => (agent ??= new https.Agent({ ca: readFileSync(certPath), keepAlive: true, maxSockets: 8 }));

function request(method, path, { session, body, headers = {}, expect } = {}) {
  const payload = body === undefined ? undefined : Buffer.isBuffer(body) ? body : Buffer.from(JSON.stringify(body));
  const type = Buffer.isBuffer(body) ? 'application/octet-stream' : 'application/json';
  return new Promise((resolve, reject) => {
    const req = https.request(new URL(path, origin), {
      method, agent: getAgent(), timeout: 60000,
      headers: {
        Origin: origin,
        ...(session ? { Cookie: session.cookie, 'X-CSRF-Token': session.csrf } : {}),
        ...(payload ? { 'Content-Type': type, 'Content-Length': payload.length } : {}),
        ...headers,
      },
    }, res => {
      const chunks = [];
      res.on('data', c => chunks.push(c));
      res.on('end', () => {
        const data = Buffer.concat(chunks);
        if (expect !== undefined && res.statusCode !== expect) {
          return reject(new Error(`${method} ${path.split('?')[0]} expected ${expect}, got ${res.statusCode}`));
        }
        resolve({ status: res.statusCode, headers: res.headers, data });
      });
    });
    req.on('timeout', () => req.destroy(new Error(`${method} ${path} timed out`)));
    req.on('error', reject);
    req.end(payload);
  });
}
const json = r => JSON.parse(r.data.toString('utf8'));
const cookieOf = r => r.headers['set-cookie'].map(c => c.split(';')[0]).join('; ');

async function login(handle, password) {
  const anon = await request('GET', '/auth/csrf', { expect: 200 });
  const anonymous = { cookie: cookieOf(anon), csrf: json(anon).csrf_token };
  const r = await request('POST', '/auth/local', { session: anonymous, body: { handle, password }, expect: 200 });
  return { cookie: cookieOf(r), csrf: json(r).csrf_token };
}

// Polls /readyz until it answers 204 and prints the wall-clock time (ms since epoch).
// Exits non-zero after READY_TIMEOUT_MS so a server that never starts cannot hang the caller.
async function ready() {
  const deadline = Date.now() + Number(process.env.READY_TIMEOUT_MS ?? 30000);
  for (;;) {
    try {
      if ((await request('GET', '/readyz')).status === 204) return console.log(Date.now());
    } catch { /* not listening yet */ }
    if (Date.now() >= deadline) throw new Error(`/readyz not ready within ${process.env.READY_TIMEOUT_MS ?? 30000} ms`);
    await sleep(2);
  }
}

// Docker's RFC 3339 timestamp (nanosecond fraction) as epoch milliseconds, keeping sub-millisecond digits.
function epochMs(stamp) {
  const m = /^(.*?)(?:\.(\d+))?Z$/.exec(stamp.trim());
  if (!m) throw new Error(`unrecognised timestamp: ${stamp}`);
  const whole = Date.parse(`${m[1]}Z`);
  if (Number.isNaN(whole)) throw new Error(`unrecognised timestamp: ${stamp}`);
  return whole + Number(`0.${m[2] ?? '0'}`) * 1000;
}

function procStatus(pid) {
  const text = readFileSync(`/proc/${pid}/status`, 'utf8');
  const kib = key => Number(text.match(new RegExp(`^${key}:\\s+(\\d+) kB`, 'm'))[1]);
  return { rssKiB: kib('VmRSS'), peakRssKiB: kib('VmHWM') };
}
function cgroupBytes(cgroup, file) {
  try { return Number(readFileSync(`/sys/fs/cgroup${cgroup}/${file}`, 'utf8')); } catch { return null; }
}
function snapshot(pid, cgroup) {
  return { ...procStatus(pid), cgroupBytes: cgroupBytes(cgroup, 'memory.current'), cgroupPeakBytes: cgroupBytes(cgroup, 'memory.peak') };
}

async function setup(pid, cgroup, authorizationPath, idleSeconds) {
  const handle = 'footprint';
  const password = `fp-${createHash('sha256').update(String(Math.random())).digest('hex').slice(0, 24)}`;
  const authorization = readFileSync(authorizationPath, 'utf8');
  await request('POST', '/setup/admin', { body: { handle, password }, headers: { 'X-Setup-Authorization': authorization }, expect: 201 });
  const session = await login(handle, password);
  await sleep(idleSeconds * 1000);
  return { session, idle: snapshot(pid, cgroup) };
}

async function workload(session, { files, fileMiB, workers, pid, cgroup }) {
  const length = fileMiB * 1024 ** 2;
  const root = json(await request('GET', '/folders/children', { session, expect: 200 })).root;
  const peak = { rssKiB: 0, cgroupBytes: 0 };
  const sampler = setInterval(() => {
    const s = snapshot(pid, cgroup);
    peak.rssKiB = Math.max(peak.rssKiB, s.rssKiB);
    peak.cgroupBytes = Math.max(peak.cgroupBytes, s.cgroupBytes ?? 0);
  }, 100);
  const started = performance.now();
  let uploaded = 0, downloaded = 0, listings = 0, running = true;
  const uploadedFiles = [];
  const worker = async n => {
    // One folder per worker: concurrent commits into one parent would conflict on its etag.
    const folder = json(await request('POST', `/folders/${root.id}/children`, { session, body: { name: `w${n}` }, expect: 201 }));
    const folderId = folder.id ?? folder.entry_id;
    for (let i = n; i < files; i += workers) {
      const parent = (json(await request('GET', `/folders/${folderId}/path`, { session, expect: 200 })).items).at(-1);
      const bytes = Buffer.alloc(length, i % 251);
      bytes.writeUInt32BE(i, 0);
      const admission = { parent_id: folderId, parent_etag: parent.etag, name: `f${i}.bin`, length };
      const ticket = json(await request('POST', '/mutation-tickets', { session, body: { scope: 'upload_create', ...admission }, expect: 201 }));
      const headers = { 'X-Mutation-ID': ticket.ticket };
      await request('POST', '/uploads', { session, body: admission, headers: { ...headers, 'If-None-Match': '*' }, expect: 201 });
      const staged = json(await request('PUT', `/uploads/${ticket.mutation_id}/content`, { session, body: bytes, headers, expect: 200 }));
      const committed = json(await request('POST', `/uploads/${ticket.mutation_id}/commit`, { session, body: { sha256: staged.sha256, parent_etag: parent.etag }, headers: { ...headers, 'If-None-Match': '*' }, expect: 201 }));
      uploaded += length;
      uploadedFiles.push({ id: committed.entry_id ?? committed.result?.entry_id ?? committed.id, sha256: createHash('sha256').update(bytes).digest('hex') });
    }
  };
  const browser = async () => {
    while (running) {
      await request('GET', '/folders/children?sort=name_asc&limit=100', { session, expect: 200 });
      listings++;
      await sleep(50);
    }
  };
  const browsing = browser();
  await Promise.all(Array.from({ length: workers }, (_, n) => worker(n)));
  for (const file of uploadedFiles) {
    const r = await request('GET', `/entries/${file.id}/content`, { session, expect: 200 });
    if (createHash('sha256').update(r.data).digest('hex') !== file.sha256) throw new Error('downloaded bytes differ');
    downloaded += r.data.length;
  }
  running = false;
  await browsing;
  const wallMs = performance.now() - started;
  clearInterval(sampler);
  return { files, fileMiB, workers, uploadedBytes: uploaded, downloadedBytes: downloaded, browseListings: listings, wallMs, ...peak };
}

if (mode === 'epoch-ms') console.log(epochMs(origin));
else if (mode === 'ready') await ready();
else if (mode === 'run') {
  const [pid, cgroup, authorizationPath, idleSeconds, files, fileMiB, workers] = rest;
  const { session, idle } = await setup(Number(pid), cgroup, authorizationPath, Number(idleSeconds));
  const during = await workload(session, { files: Number(files), fileMiB: Number(fileMiB), workers: Number(workers), pid: Number(pid), cgroup });
  await sleep(Number(idleSeconds) * 1000);
  console.log(JSON.stringify({ idle, workload: during, after: snapshot(Number(pid), cgroup) }));
} else throw new Error('mode must be ready, run or epoch-ms');
agent?.destroy();
