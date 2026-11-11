import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import SafeReporter from '../tests/browser/safe-reporter.ts';

const canary = randomBytes(32).toString('hex');
const write = process.stdout.write;
const captured = [];
try {
  process.stdout.write = chunk => { captured.push(String(chunk)); return true; };
  const reporter = new SafeReporter();
  reporter.onTestEnd({ id: canary, title: canary }, { status: 'failed', errors: [canary], attachments: [canary], headers: { Cookie: canary }, body: canary });
  reporter.onStdOut(canary);
  reporter.onStdErr(canary);
  reporter.onError(new Error(canary));
  reporter.onEnd({ status: canary });
  for (const file of ['harness.spec.ts', 'storage.spec.ts', 'accessibility.spec.ts', `${canary}.spec.ts`]) {
    reporter.onTestEnd({ id: canary }, { status: 'failed', errors: [{ message: canary, stack: `${canary}\n at /private/${canary}/${file}:41:7` }] });
  }
} finally {
  process.stdout.write = write;
}
assert(!captured.join('').includes(canary), 'secret canary leaked from reporter');
const events = captured.map(line => JSON.parse(line));
assert.equal(events.length, 7);
assert.equal(events[0].status, 'failed');
assert.equal(events[2].status, 'unknown');
for (const [index, file] of ['harness.spec.ts', 'storage.spec.ts', 'accessibility.spec.ts'].entries()) {
  assert.equal(events[index + 3].failure_file, file);
  assert.equal(events[index + 3].failure_line, 41);
}
assert(!('failure_file' in events[6]));
assert(!('failure_line' in events[6]));
const root = path.resolve(import.meta.dirname, '..');
const filterInput = [
  `npm stderr ${canary}`,
  JSON.stringify({ event: 'case', case_id: 'storage_listing_states', status: 'failed', failure_file: 'storage.spec.ts', failure_line: 41, message: canary, network_error: 'ERR_CONNECTION_RESET' }),
  JSON.stringify({ event: 'case', case_id: 'trusted_assets', status: 'failed', failure_file: canary, failure_line: canary, network_error: canary, unexpected_resolution: canary, id: canary }),
  JSON.stringify({ event: 'case', case_id: canary, status: 'failed' }),
  JSON.stringify({ event: 'case', case_id: 'trusted_assets', status: canary }),
  JSON.stringify({ event: 'runner_error', message: canary }),
  JSON.stringify({ event: 'end', status: 'failed', message: canary }),
].join('\n');
const filtered = spawnSync('cargo', ['test', '--features', 'test-harness', '--test', 'browser_fixture', '--', '--safe-reporter-events'], {
  cwd: root, encoding: 'utf8', timeout: 120_000, input: filterInput,
});
assert.equal(filtered.status, 0, 'fixture safe reporter filter did not execute');
assert(!(filtered.stdout + filtered.stderr).includes(canary), 'secret canary leaked from fixture diagnostics');
assert.deepEqual(filtered.stdout.trim().split('\n').map(line => JSON.parse(line)), [
  { event: 'case', case_id: 'storage_listing_states', status: 'failed', failure_file: 'storage.spec.ts', failure_line: 41, network_error: 'ERR_CONNECTION_RESET' },
  { event: 'case', case_id: 'trusted_assets', status: 'failed' },
  { event: 'runner_error' },
  { event: 'end', status: 'failed' },
]);
const evidence = path.join(root, 'evidence');
fs.mkdirSync(evidence, { recursive: true, mode: 0o700 });
const parent = fs.mkdtempSync(path.join(evidence, 'reporter-canary-'));
const output = path.join(parent, 'owned-output');
const result = spawnSync(process.execPath, ['scripts/run-browser.mjs', '--canary'], {
  cwd: root, encoding: 'utf8', timeout: 30_000,
  env: { ...process.env, S1_REPORTER_CANARY: canary, S1_BROWSER_OUTPUT: output,
    S1_HTTPS_ORIGIN: 'https://fixture.test:1', DEBUG: '', PWDEBUG: '', NODE_OPTIONS: '' },
});
if (result.status !== 1 || result.error) throw new Error('reporter real failure canary did not execute as expected');
if ((result.stdout + result.stderr).includes(canary)) throw new Error('reporter secret canary leaked to process output');
const observed = result.stdout.trim().split('\n').map(line => JSON.parse(line));
if (observed.length !== 4 || observed[0].event !== 'discovery' || observed[1].status !== 'passed' || observed[2].status !== 'failed' || observed[3].status !== 'failed') {
  throw new Error('reporter real success/failure cases missing');
}
function scan(directory) {
  for (const name of fs.readdirSync(directory)) {
    const file = path.join(directory, name);
    if (name.includes(canary)) throw new Error('secret canary leaked to artifact filename');
    const stat = fs.lstatSync(file);
    if (stat.isSymbolicLink()) throw new Error('unexpected canary artifact symlink');
    if (stat.isDirectory()) scan(file);
    else if (fs.readFileSync(file).includes(Buffer.from(canary))) throw new Error('secret canary leaked to retained artifact');
  }
}
scan(parent);
// Retain only safe control evidence; exact mkdtemp root, child has exited.
fs.rmSync(parent, { recursive: true });
console.log('safe reporter unit + actual Playwright success/failure artifact canaries: PASS');
