import assert from 'node:assert/strict';
import test from 'node:test';
import { validateAudit } from './npm-audit.mjs';

const before = new Date('2026-11-07');
const advisory = (name, id, severity, range) => ({ dependency: name, name, severity, range,
  url: `https://github.com/advisories/${id}` });
const finding = (name, via, severity = 'high') => ({ name, severity, via, nodes: [`node_modules/${name}`],
  fixAvailable: false });
const known = () => ({
  auditReportVersion: 2,
  vulnerabilities: {
    picomatch: finding('picomatch', [advisory('picomatch', 'GHSA-fc5q-39xc-vjpf', 'high', '<=4.0.7')]),
    fdir: finding('fdir', ['picomatch']),
    tinyglobby: finding('tinyglobby', ['fdir', 'picomatch']),
    vite: finding('vite', ['picomatch', 'tinyglobby']),
    'svelte-check': finding('svelte-check', ['fdir']),
    '@sveltejs/vite-plugin-svelte': finding('@sveltejs/vite-plugin-svelte', ['vite']),
  },
  metadata: { vulnerabilities: { info: 0, low: 0, moderate: 0, high: 6, critical: 0, total: 6 } },
});
const withDeepmerge = () => {
  const r = known();
  r.vulnerabilities.deepmerge = finding('deepmerge',
    [advisory('deepmerge', 'GHSA-gcqx-w48m-w2h4', 'low', '>=4.2.0 <=4.3.1')], 'low');
  r.vulnerabilities['@sveltejs/vite-plugin-svelte'].via = ['vite', 'deepmerge'];
  r.metadata.vulnerabilities = { info: 0, low: 1, moderate: 0, high: 6, critical: 0, total: 7 };
  return r;
};
const lock = { packages: {
  'node_modules/picomatch': { version: '4.0.7' },
  'node_modules/fdir': { version: '6.5.0' },
  'node_modules/tinyglobby': { version: '0.2.17' },
  'node_modules/vite': { version: '8.3.6' },
  'node_modules/svelte-check': { version: '4.7.8' },
  'node_modules/@sveltejs/vite-plugin-svelte': { version: '7.3.1' },
  'node_modules/deepmerge': { version: '4.3.1' },
} };
const ids = result => result.exceptions.map(e => e.id);

test('allows only the documented picomatch finding', () => {
  assert.deepEqual(ids(validateAudit(known(), lock, before)), ['GHSA-fc5q-39xc-vjpf']);
});
test('allows picomatch plus deepmerge while both are valid', () => {
  assert.deepEqual(ids(validateAudit(withDeepmerge(), lock, before)),
    ['GHSA-fc5q-39xc-vjpf', 'GHSA-gcqx-w48m-w2h4']);
});
test('picomatch exception expires on its own date', () => {
  assert.doesNotThrow(() => validateAudit(known(), lock, new Date('2026-11-30T23:59:59Z')));
  assert.throws(() => validateAudit(known(), lock, new Date('2026-12-01T00:00:00Z')),
    /GHSA-fc5q-39xc-vjpf exception expired/);
});
test('deepmerge exception expires on its own date, picomatch stays valid', () => {
  const lastDay = new Date('2026-11-21T23:59:59Z');
  const expired = new Date('2026-11-22T00:00:00Z');
  assert.doesNotThrow(() => validateAudit(withDeepmerge(), lock, lastDay));
  assert.throws(() => validateAudit(withDeepmerge(), lock, expired), /GHSA-gcqx-w48m-w2h4 exception expired/);
  assert.doesNotThrow(() => validateAudit(known(), lock, expired));
});
test('rejects unknown findings even at low severity', () => {
  const report = known();
  report.vulnerabilities.other = { severity: 'low', via: [] };
  report.metadata.vulnerabilities.low = 1;
  report.metadata.vulnerabilities.total = 7;
  assert.throws(() => validateAudit(report, lock, before));
});
test('rejects a missing dependent', () => {
  const report = known();
  delete report.vulnerabilities.fdir;
  report.metadata.vulnerabilities.high = 5;
  report.metadata.vulnerabilities.total = 5;
  assert.throws(() => validateAudit(report, lock, before));
});
test('rejects malformed or error responses, and changed locks', () => {
  for (const report of [{}, { error: {} }, { auditReportVersion: 2, vulnerabilities: {} }]) {
    assert.throws(() => validateAudit(report, lock, before));
  }
  assert.throws(() => validateAudit(known(), { packages: {} }, before));
  const bumped = structuredClone(lock);
  bumped.packages['node_modules/picomatch'].version = '4.0.8';
  assert.throws(() => validateAudit(known(), bumped, before), /picomatch lock version/);
});
test('rejects advisory identity, range, severity, dependent and count drift', () => {
  for (const mutate of [
    r => { r.vulnerabilities.picomatch.via[0].url = 'https://example.invalid/unknown'; },
    r => { r.vulnerabilities.picomatch.via[0].range = '<=4.0.8'; },
    r => { r.vulnerabilities.picomatch.severity = 'critical'; },
    r => { r.vulnerabilities.vite.via = ['picomatch', 'tinyglobby', 'rollup']; },
    r => { r.vulnerabilities['svelte-check'].via = ['picomatch']; },
    r => { r.metadata.vulnerabilities.total = 5; },
  ]) {
    const report = known(); mutate(report);
    assert.throws(() => validateAudit(report, lock, before));
  }
});
