# LMU Race Engineering — desktop app

One Tauri 2 application for both roles (driver PC and engineer PC); the operating mode is a persisted
setting chosen on first launch. See [docs/ARCHITECTURE.md](../../docs/ARCHITECTURE.md) and
[docs/DECISIONS.md](../../docs/DECISIONS.md) (D-001, D-011, D-012, D-014, D-015).

```text
apps/desktop/
  src/                  React 19 + TypeScript (strict) UI
    lib/ipc.ts          typed wrappers for the backend commands (the only IPC entry point)
    lib/units.ts        the ONLY place display-unit conversion/formatting happens
    styles/tokens.css   design tokens (colours, type, spacing)
    components/         AppErrorBoundary, PanelErrorBoundary, ConfigIssueBanner
    screens/            mode selection, shell, overview, settings
  src-tauri/            Rust shell (thin): commands, config, logging
    capabilities/       IPC permissions (minimum set)
    tauri.conf.json     window, CSP, bundle (NSIS/MSI) configuration
```

## Prerequisites (Windows 11)

- Node.js ≥ 22 and pnpm (repo uses pnpm 12)
- Rust stable (MSVC toolchain) with Visual Studio Build Tools (C++ workload + Windows SDK)
- WebView2 runtime (preinstalled on Windows 11)

Use **PowerShell** (or cmd) for `cargo`/`pnpm tauri`: Git Bash's `/usr/bin/link` shadows MSVC `link.exe`.

## Commands

Run from `apps/desktop`:

| Command | What it does |
|---|---|
| `pnpm install` | install frontend dependencies |
| `pnpm tauri dev` | run the full app (Vite dev server on :1420 + Rust backend, hot reload) |
| `pnpm dev` | Vite dev server only (in a plain browser the backend is unavailable; the UI says so) |
| `pnpm typecheck` | `tsc --noEmit` |
| `pnpm lint` | ESLint (typescript-eslint strict type-checked + react-hooks), zero warnings allowed |
| `pnpm test` | Vitest (jsdom) unit/component tests |
| `pnpm build` | typecheck + Vite production build into `dist/` |
| `pnpm tauri build` | release build + installers (NSIS `.exe`, MSI) |
| `pnpm tauri build --debug --bundles nsis` | quick debug NSIS installer |

Rust (from the repo root):

```powershell
cargo build  -p lmu-race-engineering
cargo test   -p lmu-race-engineering
cargo clippy -p lmu-race-engineering -- -D warnings
```

Installers are written to `target/<debug|release>/bundle/{nsis,msi}/` at the workspace root.

## Build environment

The deployment environment is fixed at build time by `LMURE_ENV` = `development` | `staging` | `production`
(default: `development` for debug builds, `production` for release builds). It is shown in the header and in
Settings, and cannot be changed through the config file.

```powershell
$env:LMURE_ENV = "staging"; pnpm tauri build
```

## Configuration

Per-user JSON at `%APPDATA%\com.lmurace.engineering\config.json` (`schema_version` 1). Missing fields take
defaults; unknown fields and out-of-range values are rejected. If the file is invalid the app starts with safe
defaults, shows a banner explaining why, and preserves the rejected file as
`config.invalid-<UTC timestamp>.json`. Writes are atomic (temp file + fsync + rename).
`network.extra_ca_file` is honoured only in `development` builds.

## Logs

JSON lines, daily rolling, 14 files kept, in `%LOCALAPPDATA%\com.lmurace.engineering\logs\`. Every application
event has a `component` field (`desktop.app`, `desktop.config`, `desktop.ipc`, `desktop.panic`). Debug builds
also log to stderr. Filter with `LMURE_LOG` (EnvFilter syntax), e.g. `$env:LMURE_LOG = "debug"`.

## Security

- Explicit typed commands only: `get_app_info`, `get_config`, `set_mode`. Each has its own permission
  (app manifest in `build.rs`) and is granted in `capabilities/default.json` together with
  `core:event:default` and `core:window:default`. No shell, no fs plugin.
- CSP `default-src 'self'` with no remote origins; all assets and fonts are local. A looser `devCsp` applies to
  `tauri dev` only (Vite HMR needs inline styles and a websocket).

## Adding a command

1. Implement it in `src-tauri/src/commands.rs` and register it in `generate_handler!` (`lib.rs`).
2. Add its name to `COMMANDS` in `src-tauri/build.rs`.
3. Grant `allow-<command-name>` in `src-tauri/capabilities/default.json`.
4. Add a typed wrapper and payload types in `src/lib/ipc.ts`.
