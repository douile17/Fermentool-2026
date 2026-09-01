import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// The production build is written straight into the core crate's `assets/`
// directory, where rust-embed bakes it into the daemon binary (milestone 8,
// docs/IMPLEMENTATION_PLAN.md §5 & §7).
export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: '../crates/fermentool-core/assets',
    emptyOutDir: true,
  },
  server: {
    port: 5173,
    // During `npm run dev`, proxy API + WebSocket calls to the running daemon.
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8730',
        ws: true,
      },
    },
  },
});
