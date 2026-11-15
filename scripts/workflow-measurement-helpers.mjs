import { dirname } from 'node:path';

export const usage = 'Usage: node scripts/measure-workflows.mjs [--quick] [--release] [--scale=1000|10000|100000|100020] [--without-search | --search-only | --search-matrix | --transfers-only | --trash-only]';
export function parseArgs(args) {
  const switches = ['--quick', '--release', '--without-search', '--search-only', '--search-matrix', '--transfers-only', '--trash-only'];
  if (args.some(a => !switches.includes(a) && !/^--scale=(1000|10000|100000|100020)$/.test(a))) throw new Error(usage);
  const keys = args.map(a => a.split('=')[0]);
  if (new Set(keys).size !== keys.length) throw new Error(`Duplicate option. ${usage}`);
  if (args.filter(a => ['--without-search', '--search-only', '--search-matrix', '--transfers-only', '--trash-only'].includes(a)).length > 1) throw new Error(`Conflicting workload options. ${usage}`);
  const quick = args.includes('--quick');
  const scale = args.find(a => a.startsWith('--scale='));
  return { quick, profile: args.includes('--release') ? 'release' : 'debug', transfersOnly: args.includes('--transfers-only'), searchOnly: args.includes('--search-only'), searchMatrix: args.includes('--search-matrix'), trashOnly: args.includes('--trash-only'), withoutSearch: args.includes('--without-search'), scales: scale ? [Number(scale.split('=')[1])] : quick ? [1000] : [10000, 100000] };
}

export function redactFailure(error, fixture, checkout) {
  let message = String(error?.message ?? error);
  const secrets = fixture ? [fixture.password, ...(fixture.sessions ?? []).flatMap(s => [s.cookie, s.cookie?.split('.').at(-1), s.csrf])] : [];
  const paths = fixture ? [fixture.ca, fixture.leaf, fixture.root, fixture.ca && dirname(dirname(fixture.ca)), checkout] : [checkout];
  for (const value of secrets.filter(Boolean)) message = message.replaceAll(value, '<credential>');
  for (const value of paths.filter(Boolean)) message = message.replaceAll(value, '<local>');
  return message.split('\n')[0].replace(/\/Users\/[^\s"']+|\/(?:private\/)?(?:var\/folders|tmp)\/[^\s"']+/g, '<local>');
}

function requireValue(condition, message) { if (!condition) throw new Error(message); }
export function validatePage(data, expectedIds, total, offset = 0, seen = new Set()) {
  requireValue(Array.isArray(data?.items), 'Listing items missing');
  const expected = expectedIds.slice(offset, offset + 100);
  requireValue(data.items.length === Math.min(100, total - offset), 'Listing row count differs from seed');
  requireValue(expected.length === data.items.length, 'Expected page exceeds seed oracle');
  requireValue(data.has_more === (offset + data.items.length < total), 'Listing has_more differs from seed');
  requireValue(data.has_more ? typeof data.next_cursor === 'string' && data.next_cursor.length > 0 : data.next_cursor === null, 'Listing cursor does not match has_more');
  for (let n = 0; n < data.items.length; n++) {
    const id = data.items[n].id;
    requireValue(id === expected[n], 'Listing membership or sort differs from seed');
    requireValue(!seen.has(id), 'Listing repeated an entry'); seen.add(id);
  }
}
export function validateSearch(data, expectedIds, total) {
  requireValue(Array.isArray(data?.items), 'Search items missing');
  const page = { ...data, items: data.items.map(hit => hit.entry) };
  validatePage(page, expectedIds, total);
}
export function validateTrash(data, expectedIds, offset = 0, seen = new Set()) {
  validatePage(data, expectedIds, expectedIds.length, offset, seen);
  requireValue(data.items.every(row => Array.isArray(row.original_path)), 'Trash location missing');
}
export function validatePath(data, expectedIds) {
  requireValue(Array.isArray(data?.items) && data.items.length === expectedIds.length && data.items.every((row, n) => row.id === expectedIds[n]), 'Deep ancestry differs from seed');
}

const exits = new WeakMap(), stops = new WeakMap();
export function observeChild(child) {
  child.stdin?.on('error', () => {});
  const result = new Promise(resolve => {
    child.once('error', () => resolve({ spawnFailed: true }));
    child.once('close', (code, signal) => resolve({ code, signal }));
  });
  exits.set(child, result);
  return result;
}
async function bounded(promise, ms) {
  let timer;
  try { return await Promise.race([promise, new Promise(resolve => { timer = setTimeout(() => resolve(null), ms); })]); }
  finally { clearTimeout(timer); }
}
export function stopChild(child, { requestStop = () => child.stdin?.end('stop\n'), timeoutMs = 10000, killMs = 5000, group = false } = {}) {
  if (stops.has(child)) return stops.get(child);
  const stopping = (async () => {
    const exited = exits.get(child) ?? observeChild(child);
    if (child.exitCode === null && child.signalCode === null && child.pid) { try { requestStop(); } catch {} }
    let result = await bounded(exited, timeoutMs);
    if (result) return { ...result, forcedKill: false };
    try { if (group && child.pid) process.kill(-child.pid, 'SIGKILL'); else child.kill('SIGKILL'); } catch {}
    result = await bounded(exited, killMs);
    if (!result) throw new Error('Owned child did not exit after kill deadline');
    return { ...result, forcedKill: true };
  })();
  stops.set(child, stopping);
  return stopping;
}
