import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Production build lands in `ui/dist/`; at milestone 8 the daemon embeds it with
// rust-embed via `#[folder = "../../ui/dist"]` (docs/IMPLEMENTATION_PLAN.md §5 & §7).
export default defineConfig({
  plugins: [svelte()],
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
