import react from '@vitejs/plugin-react';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

// Graph renderer benchmark harness (D-011). Served by `pnpm bench:graph`; it is not part of `pnpm build`.
// Everything is bundled locally (no CDN), like the app.
const benchDir = fileURLToPath(new URL('.', import.meta.url));
const appDir = fileURLToPath(new URL('..', import.meta.url));

export default defineConfig({
  root: benchDir,
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1430,
    // The harness imports the app's sources from ../src.
    fs: { allow: [appDir] },
  },
});
