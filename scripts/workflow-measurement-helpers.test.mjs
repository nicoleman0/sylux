import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import http from 'node:http';
import { parseArgs, redactFailure, validatePage, validateSearch, validateTrash, validatePath, observeChild, stopChild } from './workflow-measurement-helpers.mjs';

test('flags reject contradictory and duplicate workloads before a run', () => {
  for (const args of [['--scale=1000', '--scale=10000'], ['--search-only', '--without-search'], ['--search-only', '--transfers-only'], ['--transfers-only', '--without-search'], ['--quick', '--quick']]) assert.throws(() => parseArgs(args));
  assert.deepEqual(parseArgs(['--quick']).scales, [1000]);
});
test('actual failure sanitizer removes credentials and fixture/checkout paths', () => {
  const fixture = { password: 'secret-password', root: '/owned/fixture', ca: '/owned/fixture/tls/ca', leaf: '/owned/fixture/tls/leaf', sessions: [{ cookie: 'session.secret-cookie', csrf: 'secret-csrf' }] };
  for (const value of [fixture.password, fixture.sessions[0].cookie, 'secret-cookie', fixture.sessions[0].csrf, fixture.root, fixture.ca, fixture.leaf, '/checkout/private/file', '/Users/person/private', '/private/tmp/owned/key']) {
    const result = redactFailure(new Error(`Failed: ${value}`), fixture, '/checkout');
    assert(!result.includes(value), result);
  }
  assert.equal(redactFailure(new Error('ordinary failure\nprivate detail'), undefined, '/checkout'), 'ordinary failure');
});
const page = { items: [{ id: 'a' }, { id: 'b' }], has_more: false, next_cursor: null };
test('actual seed validators reject empty/wrong/unsorted/repeated pages and false pagination', () => {
  validatePage(page, ['a', 'b'], 2);
  for (const bad of [{ ...page, items: [] }, { ...page, items: [{ id: 'b' }, { id: 'a' }] }, { ...page, items: [{ id: 'a' }, { id: 'a' }] }, { ...page, has_more: true }, { ...page, next_cursor: 'unexpected' }]) assert.throws(() => validatePage(bad, ['a', 'b'], 2));
  assert.throws(() => validatePage(page, ['a', 'b'], 2, 0, new Set(['a'])));
  assert.throws(() => validateSearch({ ...page, items: [] }, ['a', 'b'], 2));
  assert.throws(() => validateTrash({ items: [{ id: 'a' }, { id: 'a' }] }, ['a', 'b']));
  assert.throws(() => validatePath({ items: [{ id: 'b' }, { id: 'a' }] }, ['a', 'b']));
});
test('owned child bootstrap failure is bounded and stop is idempotent', async () => {
  const child = spawn('/nonexistent-sylux-measurement-command', [], { stdio: 'pipe' });
  observeChild(child);
  const stop = stopChild(child, { timeoutMs: 100 });
  assert.equal(stopChild(child), stop);
  assert.equal((await stop).spawnFailed, true);
});
test('owned child cancellation drains stdin once and joins', async () => {
  const child = spawn(process.execPath, ['-e', "process.stdin.resume(); process.stdin.on('end',()=>process.exit(0)); console.log('ready')"], { stdio: 'pipe' });
  observeChild(child); await once(child.stdout, 'data');
  const first = stopChild(child); assert.equal(stopChild(child), first);
  assert.equal((await first).code, 0);
});

test('active request cancellation settles before owned server cleanup', async () => {
  const sockets = new Set();
  const server = http.createServer(() => {});
  server.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)); });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  const controller = new AbortController();
  const request = http.get(`http://127.0.0.1:${server.address().port}`, { signal: controller.signal });
  const failed = once(request, 'error');
  await once(server, 'request'); controller.abort();
  assert.equal((await failed)[0].code, 'ABORT_ERR');
  for (const socket of sockets) socket.destroy();
  await new Promise(resolve => server.close(resolve));
});

test('owned child that ignores stdin stop is killed within the bound', async () => {
  const child = spawn(process.execPath, ['-e', "process.stdin.resume(); setInterval(()=>{},1000); console.log('ready')"], { stdio: 'pipe' });
  observeChild(child); await once(child.stdout, 'data');
  const result = await stopChild(child, { timeoutMs: 25, killMs: 1000 });
  assert.equal(result.forcedKill, true); assert.equal(result.signal, 'SIGKILL');
});
