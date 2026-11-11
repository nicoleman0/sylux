import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { collectWebNotices } from './collect-web-notices.mjs';

test('preserves PDF font and decoder notices alongside renderer license', (t) => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'sylux-pdf-notices-'));
  t.after(() => fs.rmSync(temp, { recursive: true, force: true }));
  const root = path.join(temp, 'node_modules', 'pdfjs-dist');
  fs.mkdirSync(root, { recursive: true });
  fs.writeFileSync(path.join(root, 'package.json'), JSON.stringify({ name: 'pdfjs-dist', version: '1.0.0' }));
  fs.writeFileSync(path.join(root, 'LICENSE'), 'Renderer terms\n');
  for (const directory of ['standard_fonts', 'wasm', 'cmaps']) {
    fs.mkdirSync(path.join(root, directory));
    fs.writeFileSync(path.join(root, directory, 'LICENSE_ASSET'), `${directory} terms\n`);
    fs.writeFileSync(path.join(root, directory, 'payload.bin'), 'not a notice');
  }
  const output = collectWebNotices([path.join(root, 'build/pdf.mjs')]);
  for (const directory of ['standard_fonts', 'wasm', 'cmaps']) {
    assert.ok(output.includes(`${directory}/LICENSE_ASSET`));
    assert.ok(output.includes(`${directory} terms\n`));
  }
  assert.ok(!output.includes('not a notice'));
});

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
