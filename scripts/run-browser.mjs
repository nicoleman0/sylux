import path from 'node:path';
import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import { createOutput } from './browser-output.mjs';

const root = path.resolve(import.meta.dirname, '..');
const requireWeb = createRequire(path.join(root, 'web/package.json'));
const cli = path.join(path.dirname(requireWeb.resolve('playwright/package.json')), 'cli.js');
const args = process.argv.slice(2);
// Only discovery or the complete frozen fixture suite. No filters/retries/projects
// from caller arguments that could silently shrink required execution.
if (args.length && !(args.length === 1 && ['--list', '--canary'].includes(args[0]))) {
  throw new Error('browser launcher permits only complete suite, --canary or --list');
}
const canary = args[0] === '--canary';
const env = { ...process.env, S1_BROWSER_CANARY: canary ? '1' : '' };
if (!args.includes('--list')) {
  if (!env.S1_BROWSER_OUTPUT) throw new Error('fresh owned browser output required');
  env.S1_BROWSER_OWNER = createOutput(env.S1_BROWSER_OUTPUT, path.join(root, 'evidence'));
}
const child = spawn(process.execPath, [cli, 'test', '--config', 'tests/browser/playwright.config.ts', ...(canary ? [] : args)], {
  cwd: root, env, stdio: 'inherit',
});
// Same process group as the Rust owner; its timeout retains roots on ambiguous exit.
const code = await new Promise((resolve, reject) => {
  child.once('error', reject);
  child.once('exit', (code, signal) => resolve(signal ? 1 : code ?? 1));
});
process.exitCode = code;
