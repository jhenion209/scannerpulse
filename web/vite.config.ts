import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Two pages: the main page and the admin page. `npm run dev` sends the API,
// the audio and the live feed to a `scannerpulse web` on this machine.
const backend = process.env.SCANNERPULSE_DEV_BACKEND ?? 'http://127.0.0.1:8080';

export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: { input: { index: 'index.html', admin: 'admin.html' } },
  },
  server: {
    proxy: {
      '/api/live': { target: backend, ws: true },
      '/api': backend,
      '/audio': backend,
    },
  },
  test: { environment: 'jsdom', include: ['src/**/*.test.ts'] },
});
