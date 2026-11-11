import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { collectWebNotices } from './collect-web-notices.mjs';

test('selects emitted package modules and preserves root notices', (t) => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'sylux-web-notices-'));
  t.after(() => fs.rmSync(temp, { recursive: true, force: true }));
  const packageDir = path.join(temp, 'node_modules', '@scope', 'widget');
  fs.mkdirSync(packageDir, { recursive: true });
  fs.writeFileSync(path.join(packageDir, 'package.json'), JSON.stringify({ name: '@scope/widget', version: '1.2.3' }));
  fs.writeFileSync(path.join(packageDir, 'LICENSE-MIT'), 'Copyright fixture\nPermission granted.\n');
  fs.writeFileSync(path.join(packageDir, 'NOTICE'), 'Attribution fixture\n');
  const output = collectWebNotices([
    path.join(packageDir, 'dist', 'index.js') + '?v=1',
    path.join(temp, 'src', 'app.ts'),
  ]);
  assert.match(output, /@scope\/widget@1\.2\.3 \/ LICENSE-MIT/);
  assert.match(output, /Copyright fixture\nPermission granted\.\n/);
  assert.match(output, /Attribution fixture\n/);
  assert.ok(!output.includes(temp));
});

test('fails when a bundled package has no root notice', (t) => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'sylux-web-notices-'));
  t.after(() => fs.rmSync(temp, { recursive: true, force: true }));
  const packageDir = path.join(temp, 'node_modules', 'missing');
  fs.mkdirSync(packageDir, { recursive: true });
  fs.writeFileSync(path.join(packageDir, 'package.json'), JSON.stringify({ name: 'missing', version: '0.1.0' }));
  assert.throws(() => collectWebNotices([path.join(packageDir, 'index.js')]), /No root license or notice file for bundled package missing@0\.1\.0/);
});
