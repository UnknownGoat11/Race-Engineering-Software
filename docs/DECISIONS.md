# Architecture Decision Log

Format per entry: Decision · Alternatives · Chosen solution · Why · Tradeoffs · Date.
Entries marked *(provisional)* must be re-confirmed by measurement (see docs/TEST_PLAN.md) before v1.0.0.

---

## D-001 Desktop shell: Tauri 2
- **Alternatives:** Electron; native Win32/WinUI; egui/iced pure-Rust UI.
- **Chosen:** Tauri 2.x (stable line; CLI/API 2.12.0 verified on npm 2026-09-30; Tauri 3 is alpha → rejected).
- **Why:** Rust backend in-process (shared memory, TLS, recording all native), small installer, WebView2 is
  preinstalled on Windows 11, React ecosystem for a dense professional UI, capability-based IPC permissions.
- **Tradeoffs:** WebView IPC has a cost → mitigated by batching (D-015). Electron would bundle Chromium
  (~150 MB) and put Node in the privileged path; no demonstrated blocker for Tauri.
- **Date:** 2026-09-30

## D-002 LMU data source: native `LMU_Data` shared memory, read-only, no plugin
- **Alternatives:** legacy rF2 SharedMemoryMapPlugin (`$rFactor2SMMP_*`), third-party plugins
  (TelemetryTool `$TT_LMU_*`), REST API only, native DuckDB recordings only.
- **Chosen:** Primary = LMU's own shared-memory interface shipped in
  `Support/SharedMemoryInterface/` (mapping `LMU_Data`, events `LMU_Data_HoldEvent`/`LMU_Data_DataEvent`,
  lock `LMU_SharedMemoryLockData` + `LMU_SharedMemoryLockEvent`). REST = supplementary low-rate (≤1 Hz)
  behind feature flag `ENABLE_LMU_REST`. DuckDB = offline import only.
- **Why:** Official, shipped with the game, no DLL injection or plugin install; highest rate; the plugins
  folder even contains a marker that legacy plugins are no longer needed on recent builds.
- **Tradeoffs:** Struct layout is proprietary and can change on patch day → D-003.
- **Date:** 2026-09-30

## D-003 Binary decoding: explicit offsets verified against a compiled layout probe
- **Alternatives:** transmute into `#[repr(C, packed(4))]` Rust structs; bindgen on the header at build time.
- **Chosen:** Copy the needed byte ranges out of the mapping under the LMU lock, then decode with explicit,
  bounds-checked little-endian reads at named offset constants. A C++ probe (`tools/layout-probe`) is
  compiled with MSVC against the headers of the *installed* game and emits every struct size/offset as JSON
  (`docs/research/lmu_abi_layout.json`); Rust unit tests assert every offset constant equals the probe output.
  At runtime the reader validates the mapping size, `gameVersion`, event availability and value sanity; on
  mismatch it stops decoding (spec §191).
- **Why:** No undefined behaviour from misaligned/packed references, no assumption that Rust and MSVC agree on
  layout, and the header license forbids redistributing the header (so bindgen-at-build on CI is impossible;
  CI uses the committed probe JSON instead).
- **Tradeoffs:** Offsets must be regenerated when LMU changes the SDK; the fingerprint check makes that loud.
- **Date:** 2026-09-30

## D-004 Transport: persistent TCP + TLS 1.3 (rustls) on port 443
- **Alternatives:** WebSocket, QUIC, UDP+DTLS, WebRTC.
- **Chosen:** `TcpTlsTelemetryTransport` behind a `TelemetryTransport` trait; ALPN `lmure/1`; default port 443
  (passes most outbound firewalls); configurable for self-hosting.
- **Why:** Both ends are native; user explicitly requires TCP; TLS via rustls avoids OpenSSL on Windows.
- **Tradeoffs:** TCP head-of-line blocking under loss → bounded latest-wins queues, periodic snapshots, stale
  detection (spec §174). QUIC can be added behind the trait later.
- **Date:** 2026-09-30

## D-005 Wire format: custom framed binary; TLV telemetry; postcard for control messages
- **Alternatives:** Protobuf (prost), FlatBuffers, MessagePack, bincode everywhere.
- **Chosen:** Fixed 40-byte little-endian frame header + payload + CRC32C trailer (PROTOCOL.md).
  Telemetry payloads (`TELEMETRY_DELTA`/`TELEMETRY_SNAPSHOT`) are hand-encoded channel TLV records keyed by
  stable `u16` channel IDs from the channel registry (no strings, no reflection on the hot path).
  Control/metadata messages (HELLO, AUTH, SESSION_*, SCHEMA_NEGOTIATE, ERROR…) use `postcard` (serde, varint,
  no_std-grade compact) with explicit versioned structs.
- **Why:** Hot path is a tight loop over typed values; control messages benefit from serde ergonomics without
  JSON size. Channel-ID TLV lets the schema grow independently of transport (§97).
- **Tradeoffs:** Custom TLV needs its own tests/fuzzing (added in `protocol` tests). CRC32C in addition to TLS
  exists to catch our own framing bugs (§328), cost ≈ negligible.
- **Date:** 2026-09-30

## D-006 Local recording: append-only chunked log + SQLite index; Parquet/CSV export *(provisional)*
- **Alternatives:** DuckDB appender; Parquet written live; one-row-per-sample SQL.
- **Chosen:** `.lmurec` files = sequence of self-delimiting chunks `[magic|len|crc32c|zstd(payload)]`, each
  chunk ≈1 s of canonical frames (same TLV encoding as the network) plus optional raw LMU byte blocks. A
  crash can only lose the unflushed tail; recovery scans chunks until the first bad CRC and marks the
  session INTERRUPTED. Session/lap/stint/event index in SQLite (`rusqlite`, bundled). Export: CSV + Parquet.
- **Why:** Sequential append is the cheapest possible write path and crash-safe; one encoding for live and
  recorded data (§273). DuckDB/Parquet are poor at crash-tolerant incremental appends.
- **Tradeoffs:** Analytics need a readback layer (recorder::reader) — provided. Native DuckDB *export* is not
  implemented in v0.x (Parquet is directly queryable by DuckDB); stated honestly in docs.
- **Date:** 2026-09-30

## D-007 Relay persistence: PostgreSQL (accounts, session metadata only)
- **Chosen:** `sqlx` (runtime-checked queries, migrations in `crates/relay/migrations`). An in-memory store
  implements the same trait for tests/dev. No telemetry samples in Postgres, ever (§57, §180).
- **Date:** 2026-09-30

## D-008 Authentication & session authorization
- **Chosen:** Accounts with Argon2id password hashes. Short-lived access tokens (JWT, EdDSA/Ed25519 signed by
  the relay, 15 min) + opaque refresh tokens (random 256-bit, stored hashed, rotating). Clients keep the
  refresh token in Windows Credential Manager. Session code (6 chars, Crockford base32) is only a pairing
  identifier. An engineer may join only if (a) authenticated, (b) role ENGINEER, and (c) the driver has
  authorized that engineer account for the session (driver's authorized-engineer list, or live approval of a
  join request in the driver app). Unauthorized joins are rejected and rate-limited.
- **Date:** 2026-09-30

## D-009 Live queue policy: bounded, latest-wins for live data; lossless only for recording
- **Chosen:** Every inter-task and per-connection queue is bounded. For live telemetry, when a consumer
  lags, intermediate samples are dropped and the newest is kept (counted as `dropped_obsolete`). Periodic
  full snapshots (1 Hz default) repair any state lost by drops or packet loss.
- **Date:** 2026-09-30

## D-010 Calculations: single Rust implementation (`race-engines`), driver-side by default
- **Chosen:** Engines run on the driver (they need every raw sample); results are transmitted as DERIVED
  channels along with the raw inputs. The same crate is linked into the engineer build for recomputation over
  recordings. The frontend contains no fuel/timing/tyre logic. Every derived channel carries
  `calculation_engine_version`.
- **Date:** 2026-09-30

## D-011 Frontend rendering: external store + canvas; uPlot for time-series *(provisional)*
- **Chosen:** React 19 + TypeScript strict + Vite. High-rate data lives in a non-React store with typed-array
  ring buffers; components subscribe to narrow selectors and repaint on `requestAnimationFrame`. Graphs use
  uPlot (canvas, ~50 KB, handles 100k+ points at 60 FPS); track map is a custom canvas renderer.
- **Tradeoffs:** Must be benchmarked (TEST_PLAN §perf); WebGL fallback if uPlot cannot hold 60 FPS.
- **Date:** 2026-09-30

## D-012 Rust→WebView data path: Tauri `Channel` with ≤60 Hz batches
- **Chosen:** The UI bridge task batches canonical deltas and pushes them through a `tauri::ipc::Channel`
  at most 60 times/s, only while the window is visible. The driver UI receives only a 4 Hz status summary.
- **Date:** 2026-09-30

## D-013 Clock synchronization: NTP-style offset estimation against relay time
- **Chosen:** Each client sends PING with its monotonic send time; relay replies PONG with its receive and
  send times; clients keep a sliding window and use the minimum-RTT sample to estimate offset
  (θ = ((t1−t0)+(t2−t3))/2). Accuracy bound = RTT_min/2, reported in diagnostics.
- **Date:** 2026-09-30

## D-014 Logging: `tracing` with JSON file output
- **Chosen:** `tracing` + `tracing-subscriber` (JSON layer) + `tracing-appender` (daily rolling, 14 files).
  Components use a `component` field. Secrets are never logged; the diagnostics exporter applies a redaction
  pass as a second safety net.
- **Date:** 2026-09-30

## D-015 Installer & updates
- **Chosen:** Tauri bundler NSIS `.exe` (per-user install, no admin, Start Menu + optional desktop shortcut,
  WebView2 embedded bootstrapper) + optional MSI. Updates via `tauri-plugin-updater` with minisign-signed
  artifacts from GitHub Releases; the app never installs while recording/streaming is active (prompt
  "Install when safe", postponable).
- **Date:** 2026-09-30

## D-016 Relay TLS certificates
- **Chosen:** Relay loads PEM cert/key from files and hot-reloads them when they change (so certbot/acme.sh
  renewals apply without restart). docker-compose documents certbot. Development uses a locally generated
  CA (`lmu-relay dev-cert`) that dev clients trust explicitly via config in `development` environment only.
- **Date:** 2026-09-30

## D-018 LMU is strictly read-only (owner requirement)
- **Alternatives:** spec §187/§235 allowed the app to offer edits to LMU's telemetry recording config with backup.
- **Chosen:** The application never creates, modifies, overwrites or deletes anything in the LMU installation,
  its UserData/config files, or its process. Allowed interactions: opening `LMU_Data` with read-only access,
  participating in the SDK's documented reader lock handshake (a shared-memory flag held only for the
  duration of the memcpy — owner approved this on 2026-09-30; no files),
  REST **GET** requests only, and opening LMU DuckDB recordings `read_only`. The only permissible footprint is
  *adding* a file or plugin, and only with the user's explicit consent. Features that would need an LMU config
  change (e.g. native recording frequency) are documented as manual steps for the user instead.
- **Why:** Explicit product-owner requirement (2026-09-30); also minimizes anti-cheat and patch-day risk.
- **Date:** 2026-09-30

## D-017 Channel IDs and schema versioning
- **Chosen:** A single registry in `telemetry-model::registry` defines every channel (id, canonical name,
  MoTeC-style export name, unit, type, source, priority P0/P1/P2, frequency class, capability). IDs are
  append-only and never reused. `TELEMETRY_SCHEMA_VERSION` increments when channels are added/changed;
  transport `PROTOCOL_VERSION` (major.minor) is independent. Generated docs: `docs/CHANNEL_IDS.md`.
- **Date:** 2026-09-30
