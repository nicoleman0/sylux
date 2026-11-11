// Dependency feasibility checks only: no server, application scaffold or browser download.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';
const require = createRequire(new URL('../web/package.json', import.meta.url));
const { transform } = require('lightningcss');
const css = transform({ filename: 'synthetic.css', code: Buffer.from('.fixture { color: #ff0000; }'), minify: true });
assert.equal(css.code.toString(), '.fixture{color:red}');
const { rolldown } = await import(require.resolve('rolldown'));
const bundle = await rolldown({ input: 'virtual:fixture', plugins: [{
  name: 'synthetic-dependency-fixture',
  resolveId(id) { return id === 'virtual:fixture' ? id : null; },
  load(id) { return id === 'virtual:fixture' ? 'export const fixture = "synthetic";' : null; },
}] });
try {
  const result = await bundle.generate({ format: 'es' });
  assert.ok(result.output.some(output => output.type === 'chunk' && output.code.includes('synthetic')));
} finally { await bundle.close(); }
const vite = await import(require.resolve('vite'));
assert.equal(typeof vite.build, 'function');
const plugin = await import(require.resolve('@sveltejs/vite-plugin-svelte'));
assert.equal(typeof plugin.svelte, 'function');
const compiler = require('svelte/compiler');
const compiled = compiler.compile('<p>Synthetic dependency probe</p>', { filename: 'Synthetic.svelte', generate: 'client' });
assert.ok(compiled.js.code.length > 0);
const ts = require('typescript');
assert.equal(ts.version, '6.0.3');
assert.equal(ts.transpileModule('const fixture: string = "synthetic";', { reportDiagnostics: true }).diagnostics.length, 0);
const browsers = JSON.parse(readFileSync(new URL('../web/node_modules/playwright-core/browsers.json', import.meta.url)));
console.log(JSON.stringify({ lightningcss: 'native transform passed', rolldown: 'native bundle passed', vite: 'imported', sveltePlugin: 'imported', svelte: 'compiled synthetic fixture', typescript: ts.version, browserManifestOnly_NotInstalledOrLaunched: browsers }, null, 2));
