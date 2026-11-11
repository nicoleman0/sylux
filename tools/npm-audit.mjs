import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

// Private S1 build-only exception. See docs/dependency-notices.md; not a clean audit.
export function validateAudit(report, lock, now = new Date()) {
  assert.equal(report.auditReportVersion, 2);
  assert.equal(report.error, undefined);
  assert.ok(report.vulnerabilities && typeof report.vulnerabilities === 'object');
  const counts = report.metadata?.vulnerabilities;
  assert.ok(counts);
  const severities = ['info', 'low', 'moderate', 'high', 'critical'];
  for (const severity of severities) assert.ok(Number.isSafeInteger(counts[severity]) && counts[severity] >= 0);
  assert.equal(counts.total, severities.reduce((sum, severity) => sum + counts[severity], 0));
  const names = Object.keys(report.vulnerabilities).sort();
  assert.equal(names.length, counts.total);
  if (names.length === 0) return { status: 'no reported npm advisories', exception: null };
  assert.ok(Number.isFinite(now.valueOf()) && now < new Date('2026-11-22T00:00:00Z'), 'S1 exception expired');
  assert.deepEqual(names, ['@sveltejs/vite-plugin-svelte', 'deepmerge']);
  assert.equal(lock.packages['node_modules/deepmerge']?.version, '4.3.1');
  assert.equal(lock.packages['node_modules/@sveltejs/vite-plugin-svelte']?.version, '7.3.1');
  assert.deepEqual(counts, { info: 0, low: 2, moderate: 0, high: 0, critical: 0, total: 2 });
  for (const name of names) {
    const finding = report.vulnerabilities[name];
    assert.equal(finding.name, name);
    assert.equal(finding.severity, 'low');
    assert.equal(finding.fixAvailable, false);
    assert.deepEqual(finding.nodes, [`node_modules/${name}`]);
  }
  assert.deepEqual(report.vulnerabilities['@sveltejs/vite-plugin-svelte'].via, ['deepmerge']);
  const via = report.vulnerabilities.deepmerge.via;
  assert.equal(via.length, 1);
  assert.equal(via[0].name, 'deepmerge');
  assert.equal(via[0].dependency, 'deepmerge');
  assert.equal(via[0].severity, 'low');
  assert.equal(via[0].range, '>=4.2.0 <=4.3.1');
  assert.equal(via[0].url, 'https://github.com/advisories/GHSA-gcqx-w48m-w2h4');
  return { status: 'accepted private S1 build-time risk; NOT a clean audit',
    exception: 'GHSA-gcqx-w48m-w2h4', expires: '2026-11-22T00:00:00Z' };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const web = fileURLToPath(new URL('../web/', import.meta.url));
  assert.equal(process.versions.node, '24.22.2');
  const npmVersion = spawnSync('npm', ['--version'], { encoding: 'utf8' });
  assert.equal(npmVersion.status, 0);
  assert.equal(npmVersion.stdout.trim(), '11.19.1');
  const pkg = JSON.parse(readFileSync(`${web}/package.json`, 'utf8'));
  assert.equal(pkg.private, true);
  const result = spawnSync('npm', ['audit', '--json', '--include=dev'], { cwd: web, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024 });
  assert.ok(result.status === 0 || result.status === 1, result.stderr || 'npm audit failed');
  const report = JSON.parse(result.stdout);
  // Preserve the unfiltered response before applying the explicit disposition.
  console.log(JSON.stringify({ audit: report }, null, 2));
  console.log(JSON.stringify({ disposition: validateAudit(report,
    JSON.parse(readFileSync(`${web}/package-lock.json`, 'utf8'))) }, null, 2));
}
