import path from 'node:path';
import { createRequire } from 'node:module';

const requireFromWeb = createRequire(path.resolve(__dirname, '../../web/package.json'));
const { defineConfig } = requireFromWeb('@playwright/test');
const { verifyOutput } = requireFromWeb(path.resolve(__dirname, '../../scripts/browser-output.mjs'));
const listing = process.argv.includes('--list');
const baseURL = process.env.S1_HTTPS_ORIGIN;
if (baseURL && !/^https:\/\/fixture\.test:\d+$/.test(baseURL)) {
  throw new Error('S1_HTTPS_ORIGIN must be the owned fixture.test loopback HTTPS origin');
}
const output = process.env.S1_BROWSER_OUTPUT;
if ((!listing || output) && (!output || !process.env.S1_BROWSER_OWNER)) {
  throw new Error('fresh owned browser output required');
}
const outputDir = output
  ? verifyOutput(output, process.env.S1_BROWSER_OWNER, path.resolve(__dirname, '../../evidence'))
  : path.resolve(__dirname, '../../evidence/browser-discovery-unused');
if (!listing && !baseURL) throw new Error('owned TLS origin required');

// Fixture tests explicitly own browser creation, isolated NSS HOME and close/join.
// No default Playwright browser/profile, installer or product readiness inference.
export default defineConfig({
  testDir: __dirname,
  testMatch: process.env.S1_BROWSER_CANARY === '1'
    ? 'reporter-canary.spec.ts'
    : ['harness.spec.ts', 'storage.spec.ts', 'accessibility.spec.ts'],
  fullyParallel: false,
  workers: 1,
  forbidOnly: true,
  retries: 0,
  preserveOutput: 'never',
  captureGitInfo: { commit: false, diff: false },
  reporter: [[path.join(__dirname, 'safe-reporter.ts')]],
  outputDir,
  timeout: 30_000,
  globalTimeout: 300_000,
  use: {
    baseURL,
    ignoreHTTPSErrors: false,
    trace: 'off',
    video: 'off',
    screenshot: 'off',
    storageState: undefined,
    acceptDownloads: false,
  },
});
