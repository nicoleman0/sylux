#!/usr/bin/env node
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { compareSyntax } from './ci-comment-syntax.mjs';

const cases = [
  ['TS templates and nested templates', 'example.ts', 'const x: string = `hello ${`inner ${1}`}`; // old', 'const x: string = `hello ${`inner ${1}`}`; // new', true],
  ['JS regex', 'example.mjs', 'const x = /[/*]/g; // old', 'const x = /[/*]/g; // new', true],
  ['literal comments', 'example.js', 'const x = `// old`; /* old */', 'const x = `// new`; /* old */', false],
  ['raw tagged templates', 'example.ts', 'const x = tag`a\\nb`; // old', 'const x = tag`a\nb`; // new', false],
  ['types retained', 'example.ts', 'let x: string; // old', 'let x: number; // new', false],
  ['JSX text', 'example.tsx', 'const x = <p>// old</p>;', 'const x = <p>// new</p>;', false],
  ['JSX comments', 'example.tsx', 'const x = <p>{/* old */}Hello</p>;', 'const x = <p>{/* new */}Hello</p>;', true],
  ['parse diagnostics', 'example.ts', 'const x = 1; // old', 'const x = ; // new', false],
  ['lint directive', 'example.ts', '// eslint-disable no-eval\nx();', '// eslint-disable no-alert\nx();', false],
  ['TS directive', 'example.ts', '// @ts-check\nx();', '// ordinary\nx();', false],
  ['triple slash reference', 'example.ts', '/// <reference path="a"/>\nx();', '/// <reference path="b"/>\nx();', false],
  ['source map', 'example.js', 'x();\n//# sourceMappingURL=a', 'x();\n//# sourceMappingURL=b', false],
  ['webpack magic comment', 'example.js', 'import(/* webpackChunkName: "old" */ "./x.js");', 'import(/* webpackChunkName: "new" */ "./x.js");', false],
  ['hashbang', 'example.js', '#!/usr/bin/node\nx();', '#!/usr/bin/other\nx();', false],
  ['JSDoc annotation', 'example.js', '/** @type {string} */\nlet x;', '/** @type {number} */\nlet x;', false],
  ['JSDoc prose', 'example.js', '/** old */\nlet x;', '/** new */\nlet x;', true],
  ['TS parameter and return prose', 'example.ts', '/** @param x old\n * @returns old */\nfunction f(x: string): string {return x}', '/** short */\nfunction f(x: string): string {return x}', true],
  ['TSX parameter prose', 'example.tsx', '/** @param x old */\nfunction f(x: string) {return <p>{x}</p>}', 'function f(x: string) {return <p>{x}</p>}', true],
  ['JS parameter types remain significant', 'example.js', '/** @param {string} x */\nfunction f(x) {return x}', '/** @param {number} x */\nfunction f(x) {return x}', false],
  ['TS other tags remain significant', 'example.ts', '/** @deprecated old */\nfunction f() {}', 'function f() {}', false],
  ['Svelte removed header and blank lines', 'Example.svelte', '<!-- Header -->\n\n<p>Hello</p>', '<p>Hello</p>', true],
  ['Svelte removed indented comment line', 'Example.svelte', '<div>\n  <!-- Gap -->\n  <p>Hello</p>\n</div>', '<div>\n  <p>Hello</p>\n</div>', true],
  ['Svelte pre whitespace retained', 'Example.svelte', '<pre>\n<!-- Gap -->\n<p>Hello</p></pre>', '<pre>\n<p>Hello</p></pre>', false],
  ['Svelte inline whitespace retained', 'Example.svelte', '<span>a</span> <!-- Gap --><span>b</span>', '<span>a</span><span>b</span>', false],
  ['Svelte scripts, types, expressions and CSS', 'Example.svelte', '<script lang="ts">// old\nlet x: string = "hello";</script><p>{x}</p><style>/* old */ p {color:red}</style>', '<script lang="ts">// new\nlet x: string = "hello";</script><p>{x}</p><style>/* new */ p {color:red}</style>', true],
  ['Svelte script comment deletion', 'Example.svelte', '<script>// old\nlet x = 1;</script><p>{x}</p>', '<script>let x = 1;</script><p>{x}</p>', true],
  ['Svelte nested expressions', 'Example.svelte', '<script>let x = `a ${1}`; // old\n</script>{#if x}<p>{x}</p>{/if}', '<script>let x = `a ${1}`; // new\n</script>{#if x}<p>{x}</p>{/if}', true],
  ['Svelte HTML comment', 'Example.svelte', '<p>hello<!-- old -->world</p>', '<p>helloworld</p>', true],
  ['Svelte literal comment text', 'Example.svelte', '<script>let x = `// old`;</script><p>{x}</p>', '<script>let x = `// new`;</script><p>{x}</p>', false],
  ['Svelte HTML text', 'Example.svelte', '<p>old</p><!-- old -->', '<p>new</p><!-- new -->', false],
  ['Svelte script types', 'Example.svelte', '<script lang="ts">let x: string;</script>', '<script lang="ts">let x: number;</script>', false],
  ['Svelte CSS values', 'Example.svelte', '<style>p {--x:"/* old */"}</style>', '<style>p {--x:"/* new */"}</style>', false],
  ['Svelte CSS changed rule', 'Example.svelte', '<style>p {color:red}</style>', '<style>p {color:blue}</style>', false],
  ['Svelte directive', 'Example.svelte', '<!-- svelte-ignore a --><p>Hi</p>', '<!-- svelte-ignore b --><p>Hi</p>', false],
  ['Svelte script directive', 'Example.svelte', '<script>// @ts-check\nx();</script>', '<script>// ordinary\nx();</script>', false],
  ['Svelte CSS directive', 'Example.svelte', '<style>/* @tailwind old */p {color:red}</style>', '<style>/* ordinary */p {color:red}</style>', false],
  ['Svelte parse error', 'Example.svelte', '<p>Hello</p>', '<p>{</p>', false],
];

for (const [name, path, before, after, expected] of cases) {
  test(name, () => assert.equal(compareSyntax(path, before, after), expected));
}

test('CLI returns equality and difference statuses', () => {
  const root = mkdtempSync(join(tmpdir(), 'ci-comment-syntax-'));
  try {
    const before = join(root, 'before');
    const after = join(root, 'after');
    writeFileSync(before, 'const x = /[/*]/; // old');
    writeFileSync(after, 'const x = /[/*]/; // new');
    const helper = new URL('./ci-comment-syntax.mjs', import.meta.url);
    assert.equal(spawnSync(process.execPath, [helper.pathname, 'example.ts', before, after]).status, 0);
    writeFileSync(after, 'const x = /[/*]/g; // new');
    assert.equal(spawnSync(process.execPath, [helper.pathname, 'example.ts', before, after]).status, 1);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});


test('CLI signals missing pinned dependencies', () => {
  const root = mkdtempSync(join(tmpdir(), 'ci-comment-dependencies-'));
  try {
    const scripts = join(root, 'scripts');
    mkdirSync(scripts);
    const helper = join(scripts, 'ci-comment-syntax.mjs');
    writeFileSync(helper, readFileSync(new URL('./ci-comment-syntax.mjs', import.meta.url)));
    const source = join(root, 'source');
    writeFileSync(source, 'const x = 1;');
    const result = spawnSync(process.execPath, [helper, 'example.ts', source, source], {
      encoding: 'utf8', env: { ...process.env, NODE_PATH: '' },
    });
    assert.equal(result.status, 2);
    assert.match(result.stderr, /npm --prefix web ci/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
