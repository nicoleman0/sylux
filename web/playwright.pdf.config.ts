import { defineConfig } from '@playwright/test';
import smoke from './playwright.config';

export default defineConfig({
  ...smoke,
  testMatch: ['preview.spec.ts', 'pdf-*.spec.ts'],
  grep: /PDF/,
  reporter: [['list']],
  projects: [
    { name: 'chromium', use: { browserName: 'chromium' } },
    { name: 'firefox-classic', use: { browserName: 'firefox', launchOptions: { firefoxUserPrefs: { 'ui.useOverlayScrollbars': 0 } } } },
  ],
});
