import assert from 'node:assert/strict';
import test from 'node:test';
import { validateAudit } from './npm-audit.mjs';

const known = () => ({
  auditReportVersion: 2,
  vulnerabilities: {
    deepmerge: {
      name: 'deepmerge', severity: 'low', range: '>=4.2.0',
      nodes: ['node_modules/deepmerge'], fixAvailable: false,
      via: [{ dependency: 'deepmerge', name: 'deepmerge', severity: 'low',
        url: 'https://github.com/advisories/GHSA-gcqx-w48m-w2h4', range: '>=4.2.0 <=4.3.1' }],
    },
    '@sveltejs/vite-plugin-svelte': {
      name: '@sveltejs/vite-plugin-svelte', severity: 'low',
      nodes: ['node_modules/@sveltejs/vite-plugin-svelte'],
      via: ['deepmerge'], fixAvailable: false,
    },
  },
  metadata: { vulnerabilities: { info: 0, low: 2, moderate: 0, high: 0, critical: 0, total: 2 } },
});
const lock = { packages: {
  'node_modules/deepmerge': { version: '4.3.1' },
  'node_modules/@sveltejs/vite-plugin-svelte': { version: '7.3.1' },
} };
test('allows only the documented private build-time finding', () => {
  assert.equal(validateAudit(known(), lock, new Date('2026-11-07')).exception, 'GHSA-gcqx-w48m-w2h4');
});
test('rejects unknown findings even at low severity', () => {
  const report = known();
  report.vulnerabilities.other = { severity: 'low', via: [] };
  assert.throws(() => validateAudit(report, lock, new Date('2026-11-07')));
});
test('rejects an expired exception', () => {
  assert.throws(() => validateAudit(known(), lock, new Date('2026-11-22')));
});
test('rejects malformed or error responses, and changed locks', () => {
  for (const report of [{}, { error: {} }, { auditReportVersion: 2, vulnerabilities: {} }]) {
    assert.throws(() => validateAudit(report, lock, new Date('2026-11-07')));
  }
  assert.throws(() => validateAudit(known(), { packages: {} }, new Date('2026-11-07')));
});
test('rejects advisory identity, severity and count drift', () => {
  for (const mutate of [
    r => { r.vulnerabilities.deepmerge.via[0].url = 'https://example.invalid/unknown'; },
    r => { r.vulnerabilities.deepmerge.severity = 'high'; },
    r => { r.metadata.vulnerabilities.total = 1; },
  ]) {
    const report = known(); mutate(report);
    assert.throws(() => validateAudit(report, lock, new Date('2026-11-07')));
  }
});
