import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';
import { spawnSync } from 'node:child_process';
import { createOutput, verifyOutput } from './browser-output.mjs';
const root = path.resolve(import.meta.dirname, '..');
const requireWeb = createRequire(path.join(root, 'web/package.json'));
const cli = path.join(path.dirname(requireWeb.resolve('playwright/package.json')), 'cli.js');
const evidence = path.join(root, 'evidence');
fs.mkdirSync(evidence, { recursive: true, mode: 0o700 });
const owned = fs.mkdtempSync(path.join(evidence, 'output-guard-'));
const outside = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'output-outside-'));
try {
  fs.writeFileSync(path.join(owned, 'sentinel'), 'existing evidence');
  fs.writeFileSync(path.join(outside, 'sentinel'), 'outside evidence');
  fs.symlinkSync(outside, path.join(owned, 'alias'));
  assert.throws(() => createOutput(owned, evidence), /EEXIST/);
  assert.throws(() => createOutput(path.join(owned, 'alias', 'new-output'), evidence), /symlink/);
  const fresh = path.join(owned, 'fresh');
  const identity = createOutput(fresh, evidence);
  assert.equal(verifyOutput(fresh, identity, evidence), path.join(fresh, 'artifacts'));
  assert.throws(() => verifyOutput(fresh, JSON.stringify({ dev: 0, ino: 0, token: 'foreign' }), evidence), /fresh owned/);
  fs.symlinkSync(path.join(outside, 'missing'), path.join(fresh, 'artifacts'));
  assert.throws(() => verifyOutput(fresh, identity, evidence), /symlink/);
  assert.equal(fs.readFileSync(path.join(outside, 'sentinel'), 'utf8'), 'outside evidence');
  for (const output of [owned, path.join(owned, 'alias', 'artifacts')]) {
    const result = spawnSync(process.execPath, [cli, 'test', '--config', 'tests/browser/playwright.config.ts', '--list'], {
      cwd: root, encoding: 'utf8', timeout: 15_000,
      env: { ...process.env, S1_BROWSER_OUTPUT: output, S1_BROWSER_OWNER: '' },
    });
    assert.notEqual(result.status, 0);
    assert.match(result.stdout + result.stderr, /fresh owned browser output|symlink browser output/,
      'config must reject unowned cleanup targets before discovery');
    assert.equal(fs.readFileSync(path.join(owned, 'sentinel'), 'utf8'), 'existing evidence');
    assert.equal(fs.readFileSync(path.join(outside, 'sentinel'), 'utf8'), 'outside evidence');
  }
} finally {
  // Exactly these mkdtemp-owned test roots; no browser workers were started.
  fs.rmSync(owned, { recursive: true });
  fs.rmSync(outside, { recursive: true });
}
console.log('browser output rejection preserves existing/outside sentinels: PASS');
