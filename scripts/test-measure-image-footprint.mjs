// Guards for scripts/measure-image-footprint.mjs: timestamp precision and bounded readiness polling.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

const script = new URL('./measure-image-footprint.mjs', import.meta.url).pathname;
const run = (args, env = {}) => spawnSync('node', [script, ...args], { encoding: 'utf8', env: { ...process.env, ...env } });

test('epoch-ms keeps the sub-second digits as milliseconds', () => {
  const cases = [
    ['2026-11-18T11:23:41.123456789Z', 1795001021123.456789],
    ['2026-11-18T11:23:41.900000000Z', 1795001021900],
    ['2026-11-18T11:23:41Z', 1795001021000],
  ];
  for (const [stamp, ms] of cases) {
    const out = run(['epoch-ms', stamp]);
    assert.equal(out.status, 0, out.stderr);
    assert.ok(Math.abs(Number(out.stdout) - ms) < 0.01, `${stamp}: got ${out.stdout.trim()}, want ${ms}`);
  }
});

test('epoch-ms rejects an unparseable timestamp', () => {
  assert.notEqual(run(['epoch-ms', 'not a time']).status, 0);
});

test('ready gives up after READY_TIMEOUT_MS when nothing listens', () => {
  const started = Date.now();
  const out = run(['ready', 'https://127.0.0.1:9', '/dev/null'], { READY_TIMEOUT_MS: '300' });
  assert.notEqual(out.status, 0);
  assert.match(out.stderr, /not ready within 300 ms/);
  assert.ok(Date.now() - started < 5000);
});
