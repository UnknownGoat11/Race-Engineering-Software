# LMU Race Engineering

A Windows race-engineering telemetry platform for **Le Mans Ultimate (LMU)**. One desktop app, two modes:

- **Driver mode** reads live telemetry from LMU on the driver's PC, records it locally, and streams it to the
  engineers the driver chooses.
- **Engineer mode** connects to a driver with a **one-time code** and shows live data, graphs, strategy and
  alerts, from anywhere.

> **Status (2026-10-13): work in progress, not yet usable as an app.** The data pipeline and the networking
> are built and tested. The main application logic, the Driver and Engineer screens and the installer are not
> built yet. See [Project status](#project-status).

## Rules the product follows

These come from the owner and are enforced in the design ([docs/DECISIONS.md](docs/DECISIONS.md)).

- **LMU is strictly read-only (D-018).** The app never edits, deletes or configures anything in LMU, its
  install folder, its UserData or its process. It reads LMU's shared memory with read-only access and, at most,
  takes the SDK's documented reader lock for the microseconds of a copy. At most a file or plugin may be
  *added* to LMU, and only with the owner's consent.
- **Minimal, trouble-free driver setup (D-019).** No admin rights, no inbound ports, no firewall rules:
  outbound TLS only.
- **Settings and login persist (D-020).** Users log in once; settings survive restarts and updates.
- **Engineers always connect with a fresh one-time code (D-022).** Nothing auto-connects to a previous driver.
  A driver can have **at most 2 engineers** at a time and can remove either at any moment.

## How it works

```
LMU (shared memory, read-only)
   │
   ▼
lmu-reader ──► lmu-normalizer ──► telemetry-model (channels, units, quality)
                                      │
            ┌─────────────────────────┼───────────────────────────┐
            ▼                         ▼                           ▼
        recorder                 race-engines                 transport ──TLS 1.3──► relay ──► engineers
   (.lmurec + SQLite)        (fuel, timing, stint…)          (driver client)        (engineer client)
                                      │
                                      ▼
                         UI data bridge (D-023) ──► React UI (Tauri 2)
```

- Telemetry arrives at about 100 Hz and scoring at 5 Hz. Every value carries a quality flag (valid, stale,
  unavailable, invalid), so the UI never shows a stale or impossible number as if it were live.
- The live path drops old data when a link is slow (latest wins); only the local recording is lossless.
- The relay forwards frames and never decodes telemetry. It stores accounts and session metadata, never
  telemetry.

Full design: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), wire format: [docs/PROTOCOL.md](docs/PROTOCOL.md),
storage: [docs/STORAGE_DESIGN.md](docs/STORAGE_DESIGN.md).

## Repository layout

| Path | What it is |
|---|---|
| `crates/lmu-reader` | Read-only reader for LMU's `LMU_Data` shared memory (layout checks, frame gate, reader lock) |
| `crates/telemetry-model` | The channel registry (stable IDs), units, quality, frames, ring buffer, validation |
| `crates/lmu-normalizer` | Raw LMU values to clean, unit-correct channels, using evidence from real captures |
| `crates/protocol` | Binary wire protocol: framing, messages, sequencing, one-time connect codes |
| `crates/transport` | Driver and engineer client: TLS 1.3, login, resume, reconnect, connectivity test |
| `crates/relay` | The relay server: accounts, tokens, connect codes, session registry, forwarding |
| `crates/recorder` | Crash-tolerant `.lmurec` recording with a SQLite index, and replay |
| `crates/race-engines` | Fuel, timing, race, stint, tyre, brake and energy calculations (in progress) |
| `crates/synthetic` | Seeded fake telemetry and a network fault simulator, for tests and demos only |
| `crates/app-core` | Application logic that ties the pieces together (not started) |
| `apps/desktop` | The Tauri 2 app and the React / TypeScript UI |
| `tools/lmu-diag` | Live viewer and raw capture of LMU's shared memory |
| `tools/lmu-verify` | Turns captures into an evidence report (units, signs, rates, pit states) |
| `tools/gen-ts` | Generates the TypeScript channel registry and golden test vectors |
| `tools/layout-probe` | Compiles against the SDK headers LMU installs, to confirm the shared-memory byte layout |
| `deploy/` | Relay Docker image, compose file and example configuration |
| `docs/` | Architecture, decisions, protocol, assumptions with their evidence, Gate reports |

## Project status

| Part | State |
|---|---|
| LMU reader, telemetry model, normalizer, protocol, recorder | Built, reviewed, committed |
| Gate 1: checked against real LMU 1.4.2.0 data (Hypercar, plus LMGT3 race and pit-stop recordings) | Passed for the data path and most channels; follow-up fixes in progress ([docs/GATE1_REPORT.md](docs/GATE1_REPORT.md)) |
| Transport (driver and engineer client) | Built, reviewed, committed |
| Relay server | Built, reviewed, committed; its PostgreSQL store and Docker image are not yet run against a real database |
| Frontend data layer and shared components | Built, reviewed, committed |
| `lmu-verify` evidence tool | Built, reviewed, committed |
| Synthetic data, graph engine, race engines | In progress |
| Track map, main app logic, Driver and Engineer screens | Not started |
| Installer, auto-update, long-running soak tests, user docs | Not started |

Every crate goes through implementation, an adversarial review and a fix pass, and is committed only when its
tests, `clippy -D warnings` and `rustfmt` pass in a clean copy of the repository.

## Building and testing

Requirements: Windows 10/11, Rust (the version is pinned in `rust-toolchain.toml`; `rustup` installs it
automatically), Node.js with pnpm, and the Tauri 2 prerequisites.

```powershell
# Rust: build, test and lint one crate (replace transport with any crate name)
cargo test -p transport
cargo clippy -p transport --all-targets -- -D warnings
cargo fmt -p transport --check

# Frontend
cd apps/desktop
pnpm install
pnpm typecheck
pnpm lint
pnpm test
pnpm build
```

CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) runs the Rust and frontend checks on every push and
pull request.

## Recording LMU data (read-only)

This is how real data for the Gate reports is captured. It only reads LMU's shared memory.

```powershell
cargo build --release -p lmu-diag -p lmu-verify

# Optional: check that the tool can see LMU (LMU running, in a session)
.\target\release\lmu-diag.exe compat

# Record; Ctrl+C stops early and the file stays valid. Use a new file name each time.
.\target\release\lmu-diag.exe capture --seconds 3600 --out C:\Users\<you>\Documents\LMU-Gate1\race1.lmuraw

# Turn recordings into a verification report
.\target\release\lmu-verify.exe report C:\Users\<you>\Documents\LMU-Gate1\race1.lmuraw --out report
```

Recordings are about 300 MB per 15 minutes. They contain driver and player names, so keep them private and
never commit them. Details: [tools/lmu-diag/README.md](tools/lmu-diag/README.md) and
[tools/lmu-verify/README.md](tools/lmu-verify/README.md).

## Documentation

| Document | Contents |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | System overview, process model, data flow, state machines, failure handling, timing |
| [docs/DECISIONS.md](docs/DECISIONS.md) | Numbered design decisions (D-001 onward) with the reasons |
| [docs/PROTOCOL.md](docs/PROTOCOL.md) | Wire protocol, handshake, tickets, connect codes, login tokens |
| [docs/ASSUMPTIONS.md](docs/ASSUMPTIONS.md) | Every assumption about LMU (A-LMU-nnn), its status and its evidence |
| [docs/LMU_CHANNEL_MAP.md](docs/LMU_CHANNEL_MAP.md) / [docs/CHANNEL_IDS.md](docs/CHANNEL_IDS.md) | Where each channel comes from, and its stable ID |
| [docs/LMU_TELEMETRY_RESEARCH.md](docs/LMU_TELEMETRY_RESEARCH.md) | How LMU exposes telemetry (shared memory, REST) |
| [docs/GATE1_REPORT.md](docs/GATE1_REPORT.md) | Results of checking the reader against real LMU data |
| [docs/STORAGE_DESIGN.md](docs/STORAGE_DESIGN.md) | Recording format and index |

## License

Proprietary. All rights reserved.
