import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
import { readFileSync } from 'node:fs';
import { defineConfig } from 'vite';

const { version } = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8'));

export default defineConfig({
  define: { __APP_VERSION__: JSON.stringify(version) },
  plugins: [sveltekit({
    preprocess: vitePreprocess(),
    adapter: adapter({ fallback: 'index.html', strict: false }),
    version: { pollInterval: 0 }
  })],
  build: {
    target: ['chrome111', 'edge111', 'firefox114', 'safari16.4']
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true
  }
});
