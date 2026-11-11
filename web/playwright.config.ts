import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  timeout: 20_000,
  retries: 0,
  workers: 1,
  reporter: [['list'], ['json', { outputFile: '../evidence/poc/p6/smoke-results.json' }]],
  use: { baseURL: 'http://127.0.0.1:4178', browserName: 'chromium', headless: true, viewport: { width: 1280, height: 900 } },
  webServer: {
    command: 'npm run dev -- --host 127.0.0.1 --port 4178 --strictPort',
    url: 'http://127.0.0.1:4178',
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
});
