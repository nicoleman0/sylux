import path from 'node:path';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';
// @ts-expect-error The build helper is a checked Node ESM module without a declaration file.
import { webNoticesPlugin } from '../tools/collect-web-notices.mjs';

export default defineConfig({
  // Late icon prebundles can replace Svelte's runtime during a lazy preview import.
  optimizeDeps: {
    include: [
      'pdfjs-dist',
      'pdfjs-dist/web/pdf_viewer.mjs',
      '@lucide/svelte/icons/chevron-up',
      '@lucide/svelte/icons/chevron-down',
      '@lucide/svelte/icons/minus',
      '@lucide/svelte/icons/search',
    ],
  },
  plugins: [tailwindcss(), svelte(), webNoticesPlugin()],
  resolve: {
    alias: { $lib: path.resolve('./src/lib') },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: false,
    rolldownOptions: {
      input: { index: 'index.html', 'public-folder': 'src/public-folder.css' },
      output: {
        assetFileNames: asset => asset.names.includes('public-folder.css')
          ? 'public-folder.css'
          : 'assets/[name]-[hash][extname]',
      },
    },
  },
  server: {
    host: '127.0.0.1',
  },
  preview: {
    host: '127.0.0.1',
  },
});
