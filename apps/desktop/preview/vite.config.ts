import react from '@vitejs/plugin-react';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

// Dev-only UI preview harness (`pnpm preview:ui`, port 1431). It is not part of `pnpm build`: the app build uses
// ../vite.config.ts with ../index.html, which never imports src/preview.
//
// The only seam: `@tauri-apps/api/core` and `@tauri-apps/api/event` resolve to the shims in src/preview/tauri,
// which route `invoke`, `listen` and `Channel` to the scenario-driven fake backend (src/preview/backend.ts).
// Every app module therefore runs unchanged, against SYNTHETIC data.
const previewDir = fileURLToPath(new URL('.', import.meta.url));
const appDir = fileURLToPath(new URL('..', import.meta.url));
const shim = (name: string): string => fileURLToPath(new URL(`../src/preview/tauri/${name}.ts`, import.meta.url));

export default defineConfig({
  root: previewDir,
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: [
      { find: /^@tauri-apps\/api\/core$/, replacement: shim('core') },
      { find: /^@tauri-apps\/api\/event$/, replacement: shim('event') },
    ],
  },
  // Never build into the source tree or the app's dist/ (only used if someone runs `vite build` on this config).
  build: { outDir: fileURLToPath(new URL('../node_modules/.cache/preview-ui', import.meta.url)), emptyOutDir: true },
  server: {
    port: 1431,
    strictPort: true,
    fs: { allow: [appDir] },
    watch: { ignored: ['**/src-tauri/**'] },
  },
  preview: { port: 1431, strictPort: true },
});
