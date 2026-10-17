# Architecture — LMU Race Engineering

Status: living document. Last updated 2026-09-30.
Related: [DECISIONS.md](DECISIONS.md), [PROTOCOL.md](PROTOCOL.md), [STORAGE_DESIGN.md](STORAGE_DESIGN.md),
[LMU_TELEMETRY_RESEARCH.md](LMU_TELEMETRY_RESEARCH.md), [ASSUMPTIONS.md](ASSUMPTIONS.md).

## 1. System overview

```text
                              INTERNET
                                 │
                       ┌─────────▼─────────┐
                       │   RELAY (Linux)   │  lmu-relay (Rust, Tokio, rustls)
                       │ auth · sessions   │  TCP/TLS :443 (telemetry)  HTTPS admin :8443 (health/metrics, auth'd)
                       │ route · heartbeat │  Postgres: accounts, session metadata (never telemetry samples)
                       └──────┬─────┬──────┘
                 TLS (outbound)│     │TLS (outbound)
              ┌────────────────┘     └───────────────┐
      ┌───────▼──────────┐                   ┌───────▼──────────┐
      │ DRIVER PC (Win11)│                   │ ENGINEER PC      │
      │ LMU Race Eng.app │                   │ LMU Race Eng.app │
      │  mode = driver   │                   │  mode = engineer │
      └───────┬──────────┘                   └──────────────────┘
              │ read-only shared memory (LMU_Data)
       ┌──────▼──────┐
       │ Le Mans      │
       │ Ultimate     │
       └─────────────┘
```

Both PCs run the same installer / same binary. Mode is a persisted setting. Both endpoints make **outbound**
connections only; no port forwarding, no VPN. The relay owns the public endpoint.

The driver PC is the only source of vehicle telemetry. The relay never alters telemetry: on the hot path it
reads the fixed-size frame header, validates length/type/sequence/session, stamps receive/forward times into
a relay-timing trailer, and forwards bytes.

## 2. Repository layout

```text
Cargo.toml                    Rust workspace
crates/
  lmu-reader/                 LMU_Data shared-memory acquisition (Windows). Offsets verified against
                              tools/layout-probe output. No UI, no network, no business logic.
  telemetry-model/            Canonical telemetry model, channel registry (stable numeric IDs), units,
                              provenance (MEASURED/DERIVED/ESTIMATED/STALE/UNAVAILABLE), validation.
  lmu-normalizer/             Raw LMU snapshot -> canonical TelemetryFrame (the ONLY place LMU field
                              names are mapped to canonical channels; the ONLY place unit conversion from
                              LMU raw units happens).
  race-engines/               Deterministic calculation engines: timing, fuel, stint, tyre, brake, energy,
                              race, track, alert. Pure functions of canonical input. Versioned.
  protocol/                   Frame codec (header, CRC32C), message types, telemetry TLV encoding,
                              version negotiation. No I/O.
  transport/                  TelemetryTransport trait + TcpTlsTelemetryTransport (tokio + rustls),
                              connection state machine, heartbeat, reconnect with backoff, clock offset.
  recorder/                   Local crash-tolerant recording (append-only chunked log) + SQLite session
                              index + readback/replay + export (CSV, Parquet).
  synthetic/                  SyntheticTelemetryProvider (seeded, dev/test/demo only) + network fault
                              simulator. Never linked into the production acquisition path.
  app-core/                   Driver & engineer orchestration used by the desktop shell: pipelines,
                              session state machine, current-state cache, diagnostics.
  relay/                      Relay server binary (lmu-relay).
apps/desktop/                 Tauri 2 shell: src-tauri (Rust commands only, thin) + src (React/TS UI).
tools/
  layout-probe/               C++ program compiled with MSVC against the *installed* LMU SDK headers;
                              emits sizes/offsets JSON used to verify Rust offsets. Headers are never
                              copied into this repo (license).
  lmu-diag/                   Gate-1 CLI diagnostic viewer: reads real LMU_Data, prints decoded channels,
                              measures rates, logs to JSONL.
deploy/                       Dockerfile, docker-compose.yml, relay config examples.
docs/                         All design and user documentation.
data/tracks/                  Versioned cached track-map geometry (JSON).
```

## 3. Process & task model

### Driver PC (single process: the Tauri app)

```text
 [LMU Reader thread]  (dedicated OS thread, event-driven: waits on LMU_Data_HoldEvent/DataEvent)
        │ lock → memcpy of needed regions → unlock (microseconds) → decode+validate outside lock
        ▼
 Arc<TelemetryFrame>  (immutable, shared by reference — never mutated after creation)
        │  tokio::sync::broadcast / watch fan-out (bounded)
   ┌────┼──────────────────┬───────────────────────┬──────────────────────┐
   ▼    ▼                  ▼                       ▼                      ▼
 Recorder task      Engines task            Network task             UI bridge task
 (own thread,       (derived metrics,       (encode TLV, frame,      (≤60 Hz batches to WebView
  buffered writes,   lap/stint/fuel/tyre,    send; latest-wins        via Tauri Channel; only when
  fsync every 1s)    alerts; ~native rate)   under backpressure)      window visible)
```

* The reader never blocks on anything downstream. Every downstream channel is **bounded**. If a consumer
  lags, it loses intermediate samples (network/UI: latest-wins) — except the recorder, which has a large
  bounded queue and reports `RecorderOverrun` events (counted, surfaced in diagnostics) if it ever overflows.
* Network failure cannot affect acquisition or recording: they share no locks and no queues with the
  network task.
* The UI is never in the acquisition path. The window can be minimized/hidden; all tasks continue.

### Engineer PC

```text
 Network task (TLS client) → decode → CurrentTelemetryState (latest value + timestamp + source + validity
 per channel) + TelemetryRingBuffer (fixed capacity, 60 s @ native rate, per channel typed arrays)
        │
        ├── Engines (engineer-side, for interactive/historical analysis only)
        └── UI bridge: ≤60 Hz batched deltas → React external store (not React state) → canvas renderers
```

### Relay

Tokio multi-thread runtime. One task per connection (reader) + one writer task per connection with a
bounded outbound queue. Live telemetry frames to an engineer use a *latest-wins* slot per
(session, priority class) so a slow engineer never causes unbounded queueing; control frames use a small
reliable queue. Sessions live in an in-memory registry (with metadata persisted to Postgres so a relay
restart can restore session identity).

## 4. Data flow (driver)

```text
LMU_Data ─► RawSnapshot (bytes copied under lock, plus event mask, capture monotonic ns)
         ─► Validation (layout fingerprint, bounds, NaN/Inf, ranges, sentinels, ET regression, jumps)
         ─► Normalization (lmu-normalizer: units → SI/canonical, provenance tagging)
         ─► TelemetryFrame (canonical, Option<T> for every channel; None ≠ 0)
         ─► Derived metrics (race-engines, tagged DERIVED/ESTIMATED with formula id)
         ─► Record locally (recorder; raw + canonical)
         ─► Prioritize (P0/P1/P2 channel classes)
         ─► Serialize (protocol TLV by channel id) ─► Frame+CRC ─► TLS ─► relay ─► engineer
```

Ownership of transformations (spec §354):

| Stage      | Owner crate        | Transformation                                   |
|------------|--------------------|--------------------------------------------------|
| Raw        | lmu-reader         | consistent byte snapshot, event mask, timestamps |
| Validated  | lmu-reader/normalizer | structural + numeric validation              |
| Canonical  | lmu-normalizer     | field mapping, unit conversion, provenance       |
| Derived    | race-engines       | fuel/lap, delta, G, etc. (versioned formulas)    |
| Alert      | race-engines::alert| debounced rule evaluation                        |
| Recorded   | recorder           | chunked log + index                              |
| Network    | protocol/transport | TLV encode, frame, TLS                           |
| Rendered   | apps/desktop UI    | display unit conversion + formatting ONLY        |

Display-unit conversion lives in one place: `apps/desktop/src/lib/units.ts` (mirrors `telemetry-model::units`
display definitions). All other conversions live in `lmu-normalizer`.

## 5. State machines

### 5.1 Connection state (single definition, `transport::ConnectionState`)

| State          | Entered when                                                                 |
|----------------|------------------------------------------------------------------------------|
| DISCONNECTED   | initial; user stopped; or FAILED acknowledged                                 |
| CONNECTING     | DNS/TCP/TLS connect in progress                                               |
| AUTHENTICATING | TLS up, HELLO/AUTH exchange in progress                                       |
| CONNECTED      | AUTH_OK + session joined/created; heartbeats healthy                          |
| DEGRADED       | CONNECTED but RTT > degraded threshold or loss > threshold or 1 missed heartbeat |
| STALE          | no inbound frame for > stale threshold (default 3 s) while socket still open   |
| RECONNECTING   | socket error / heartbeat timeout (default 5 s) / relay close; backoff running  |
| FAILED         | non-retryable: auth rejected, protocol incompatible, session revoked          |

Telemetry *freshness* (LIVE/DEGRADED/STALE/NO DATA on the engineer UI) is a separate concept computed from
sample age, not from socket state (spec §248).

### 5.2 Driver session state (`app-core::DriverSessionState`)

```text
IDLE → WAITING_FOR_LMU → LMU_DETECTED → TELEMETRY_AVAILABLE → SESSION_READY → STREAMING
                 ▲              │                 │                              │  ▲
                 └──────────────┴── LMU lost ─────┴──────────────────────────────┘  │
 STREAMING ⇄ DISCONNECTED/RECONNECTING (network only; acquisition+recording continue)
 any → ERROR (incompatible LMU interface: reading stops)      STREAMING → COMPLETE (driver stop)
 PAUSED: LMU not in realtime (menus/monitor) — acquisition idles, recording segment paused.
```

## 6. Failure modes & recovery

| Failure                         | Driver                                             | Engineer                              |
|---------------------------------|----------------------------------------------------|---------------------------------------|
| Internet down                   | acquisition+recording continue; RECONNECTING      | sample age grows → STALE → NO DATA    |
| Relay restart                   | reconnect, re-auth, SESSION_RESUME, full snapshot | reconnect, rejoin, snapshot           |
| Engineer app crash              | unaffected                                         | restart → join → snapshot             |
| Driver app crash                | restart → detect LMU → resume session (token) → new recording segment, old one recovered | sees DRIVER DISCONNECTED then resume |
| LMU exits                       | reader detects (process gone / events closed), recording segment finalized, SESSION_EVENT(LmuLost) | "DRIVER TELEMETRY SOURCE LOST" with network CONNECTED |
| LMU interface mismatch          | reading disabled, compatibility report, ERROR       | capability map shows source INVALID  |
| Disk full / write error         | live continues, recorder alerts, retries           | RECORDING_STATUS alert                |

## 7. Security boundaries

* Only Rust code holds credentials. Refresh tokens stored via Windows Credential Manager (`keyring` crate).
  The WebView receives sanitized state only.
* Tauri capabilities: no shell, no fs plugin exposed to the frontend; explicit typed commands only.
* All network traffic TLS 1.3 (rustls), certificate validation always on. Self-hosters may configure an extra
  trusted CA file; there is no "skip verification" code path in any build.
* Engineer → driver direction carries no vehicle/game control message types. The protocol has no such type.
* Relay admin API is a separate listener, bearer-token protected, not reachable through the telemetry port.

## 8. Timing model

Every frame carries: `source_et` (LMU mElapsedTime, s), `capture_mono_ns` (driver monotonic), and the relay
appends receive/forward timestamps. Clients estimate clock offset to relay time by NTP-style ping/pong
(minimum-RTT filtered). Stream age at the engineer = relay_now_estimate − (capture time mapped into relay time).
See PROTOCOL.md §Clock synchronization.
