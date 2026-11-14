import path from 'node:path';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';
// @ts-expect-error The build helper is a checked Node ESM module without a declaration file.
import { webNoticesPlugin } from '../tools/collect-web-notices.mjs';

export default defineConfig({
  // Prebundle the lazy PDF import before it can reload an active preview.
  optimizeDeps: { include: ['pdfjs-dist', 'pdfjs-dist/web/pdf_viewer.mjs'] },
  plugins: [tailwindcss(), svelte(), webNoticesPlugin()],
  resolve: {
    alias: { $lib: path.resolve('./src/lib') },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: false,
  },
  server: {
    host: '127.0.0.1',
  },
  preview: {
    host: '127.0.0.1',
  },
});
