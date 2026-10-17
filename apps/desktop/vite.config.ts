import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// Vite configuration for the Tauri WebView frontend.
// - Fixed dev port (must match build.devUrl in src-tauri/tauri.conf.json).
// - No remote assets: everything is bundled locally so the strict CSP (default-src 'self') holds.
export default defineConfig({
  plugins: [react()],
  // Keep Rust compiler output visible when run through `tauri dev`.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // The Rust side is rebuilt by the Tauri CLI; Vite must not watch it.
      ignored: ['**/src-tauri/**'],
    },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    // WebView2 (Chromium, evergreen) on Windows 11.
    target: 'es2022',
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: false,
  },
});
