import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';

export default defineConfig({
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
