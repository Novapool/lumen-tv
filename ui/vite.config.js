import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  server: { port: 5173, strictPort: true },
  // WebKitGTK 2.54 handles modern JS; no need to down-level.
  build: { target: 'es2022' },
});
