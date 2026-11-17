import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

// Each exception expires on its own date; deepmerge is only checked when reported.
const PICOMATCH = {
  id: 'GHSA-fc5q-39xc-vjpf', expires: '2026-12-01T00:00:00Z', severity: 'high', range: '<=4.0.7',
  versions: { picomatch: '4.0.7', fdir: '6.5.0', tinyglobby: '0.2.17', vite: '8.3.6', 'svelte-check': '4.7.8',
    '@sveltejs/vite-plugin-svelte': '7.3.1' },
  viaPicomatch: ['fdir', 'tinyglobby', 'vite', 'svelte-check', '@sveltejs/vite-plugin-svelte'],
};
const DEEPMERGE = {
  id: 'GHSA-gcqx-w48m-w2h4', expires: '2026-11-22T00:00:00Z', severity: 'low', range: '>=4.2.0 <=4.3.1',
  versions: { deepmerge: '4.3.1' },
};

// Private S1 build-only exceptions. See docs/dependency-notices.md; not a clean audit.
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
  assert.ok(Number.isFinite(now.valueOf()), 'invalid date');
  const vulns = report.vulnerabilities;
  const withDeepmerge = 'deepmerge' in vulns;
  const used = [PICOMATCH, ...(withDeepmerge ? [DEEPMERGE] : [])];
  for (const exception of used) {
    assert.ok(now < new Date(exception.expires), `${exception.id} exception expired`);
  }
  assert.deepEqual(names, [...PICOMATCH.viaPicomatch, 'picomatch', ...(withDeepmerge ? ['deepmerge'] : [])].sort());
  for (const [name, version] of Object.entries({ ...PICOMATCH.versions, ...(withDeepmerge ? DEEPMERGE.versions : {}) })) {
    assert.equal(lock.packages[`node_modules/${name}`]?.version, version, `${name} lock version`);
  }
  assert.deepEqual(counts, withDeepmerge
    ? { info: 0, low: 1, moderate: 0, high: 6, critical: 0, total: 7 }
    : { info: 0, low: 0, moderate: 0, high: 6, critical: 0, total: 6 });
  for (const name of names) {
    const finding = vulns[name];
    assert.equal(finding.name, name);
    assert.equal(finding.severity, name === 'deepmerge' ? 'low' : 'high');
    assert.deepEqual(finding.nodes, [`node_modules/${name}`]);
  }
  const advisory = (name, spec) => {
    const via = vulns[name].via;
    assert.equal(via.length, 1);
    assert.equal(via[0].name, name);
    assert.equal(via[0].dependency, name);
    assert.equal(via[0].severity, spec.severity);
    assert.equal(via[0].range, spec.range);
    assert.equal(via[0].url, `https://github.com/advisories/${spec.id}`);
  };
  advisory('picomatch', PICOMATCH);
  const sorted = via => [...via].sort();
  assert.deepEqual(sorted(vulns.fdir.via), ['picomatch']);
  assert.deepEqual(sorted(vulns['svelte-check'].via), ['fdir']);
  assert.deepEqual(sorted(vulns.tinyglobby.via), ['fdir', 'picomatch']);
  assert.deepEqual(sorted(vulns.vite.via), ['picomatch', 'tinyglobby']);
  assert.deepEqual(sorted(vulns['@sveltejs/vite-plugin-svelte'].via), withDeepmerge ? ['deepmerge', 'vite'] : ['vite']);
  if (withDeepmerge) advisory('deepmerge', DEEPMERGE);
  return { status: 'accepted private S1 build-time risk; NOT a clean audit',
    exceptions: used.map(({ id, expires }) => ({ id, expires })) };
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
