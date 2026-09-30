# Wire Protocol — `lmure` v1.0

Status: normative for protocol major 1. Implemented in `crates/protocol`. Every change to this file that
alters bytes on the wire must bump `PROTOCOL_MINOR` (compatible addition) or `PROTOCOL_MAJOR` (breaking).

* Transport: TCP, TLS 1.3 (rustls), ALPN `lmure/1`, default port 443.
* Byte order: **little-endian** for every integer and float in this document.
* Telemetry schema version (`TELEMETRY_SCHEMA_VERSION`, u16) is independent of the protocol version.

## 1. Frame layout

```text
offset size field
0      2    magic            = 0x4C 0x52  ("LR")
2      1    protocol_major   = 1
3      1    protocol_minor   = 0
4      1    msg_type         (see §3)
5      1    flags            bit0 RELAY_TRAILER present · bit1 PAYLOAD_ZSTD · bits2-7 reserved (must be 0)
6      1    priority         0=P0 1=P1 2=P2 0xFF=control
7      1    reserved         = 0
8      4    payload_len      u32, bytes of payload only; MUST be ≤ MAX_PAYLOAD_BYTES (262144)
12     8    session_id       u64, relay-assigned; 0 before a session exists
20     8    sequence         u64, per (sender, session) stream; see §5
28     8    source_ts_ns     u64, sender monotonic clock (ns since sender process epoch)
36     4    reserved2        = 0
40     N    payload
40+N   24   relay trailer    ONLY if flags.bit0: relay_rx_ns u64 · relay_tx_ns u64 (relay clock, ns since
                             Unix epoch) · relay_dropped u32 (live frames dropped for this receiver since the
                             previous forwarded frame) · reserved u32
end    4    crc32c           CRC-32C (Castagnoli) over every preceding byte of the frame
```

Header = 40 bytes; minimum frame = 44 bytes. Receivers MUST:
1. read exactly 40 bytes; reject wrong magic (close connection, `ERROR BadMagic`);
2. reject `protocol_major` ≠ supported major (`ERROR Incompatible`) **without decoding the payload**;
3. reject `payload_len` > `MAX_PAYLOAD_BYTES` *before allocating*;
4. read payload (+ trailer) + crc; verify CRC; on failure close the connection (framing is then untrusted);
5. reject non-zero reserved bits/fields.

TCP is a byte stream: implementations read with an accumulating buffer; one `read()` ≠ one frame.

The relay validates the header and CRC, then **appends** the relay trailer (sets flags.bit0 and recomputes the
CRC) when forwarding driver telemetry to an engineer. It never decodes or modifies the payload.

## 2. Priorities & queues

* P0: vehicle-critical state, timing, fuel, position, race status. P1: tyres, brakes, energy. P2: detail.
* Control frames (`priority = 0xFF`) go through a small reliable queue and are never dropped.
* Live telemetry frames go through a per-receiver **latest-wins** slot per priority. When a newer frame of the
  same `msg_type`+priority arrives before the old one was written, the old one is dropped and counted in
  `relay_dropped`. Snapshots are never superseded by deltas (a pending snapshot is sent first).

## 3. Message types

| id   | name               | dir            | payload encoding | purpose |
|------|--------------------|----------------|------------------|---------|
| 0x01 | HELLO              | C→R, R→C       | postcard `Hello` / `ServerHello` | version + role + capabilities |
| 0x02 | AUTH               | C→R            | postcard `Auth`  | password / access token / refresh token |
| 0x03 | AUTH_OK            | R→C            | postcard `AuthOk`| identity, role, new tokens |
| 0x04 | ACCOUNT_REGISTER   | C→R            | postcard         | only if relay allows open registration |
| 0x10 | SESSION_CREATE     | D→R            | postcard `SessionCreate` | vehicle/track/session/LMU version/capabilities |
| 0x11 | SESSION_JOIN       | E→R            | postcard `SessionJoin`   | by code |
| 0x12 | SESSION_INFO       | R→C            | postcard `SessionInfo`   | id, code, driver identity, car, track, started, schema |
| 0x13 | SESSION_RESUME     | D→R, E→R       | postcard         | reconnect to existing session (resume token / session id) |
| 0x14 | SESSION_CLOSE      | D→R, R→E       | postcard         | driver stopped / expired |
| 0x15 | SESSION_LIST       | E→R, R→E       | postcard         | sessions this engineer may join (never others) |
| 0x16 | JOIN_REQUEST       | R→D            | postcard         | engineer X asks to join; driver decides |
| 0x17 | JOIN_DECISION      | D→R            | postcard         | allow / deny (+ remember) |
| 0x18 | ENGINEER_GRANT     | D→R            | postcard         | add/remove authorized engineer account |
| 0x20 | SCHEMA_NEGOTIATE   | D→R→E          | postcard         | schema version + capability map (channel availability, source, native rate) |
| 0x21 | TELEMETRY_DELTA    | D→R→E          | TLV (§4)         | changed/high-rate channels |
| 0x22 | TELEMETRY_SNAPSHOT | D→R→E          | TLV (§4) full    | every channel incl. unavailable markers |
| 0x23 | SCORING_UPDATE     | D→R→E          | postcard         | race/session scoring + competitor list (native scoring rate) |
| 0x24 | WEATHER_UPDATE     | D→R→E          | TLV (§4)         | weather channels (on change / ≤1 Hz) |
| 0x25 | SESSION_EVENT      | D→R→E          | postcard         | lap/sector/pit/flag/LMU-lost/recording events |
| 0x26 | SNAPSHOT_REQUEST   | E→R→D          | postcard         | ask for a fresh snapshot (after gap/reconnect) |
| 0x27 | DRIVER_STATUS      | D→R→E          | postcard         | layered health: LMU source, processing, recording, rates (1 Hz) |
| 0x28 | RECORDING_STATUS   | D→R→E          | postcard         | recording active/failed/disk space |
| 0x30 | PING               | C→R            | postcard `Ping { id, t0 }` | heartbeat + clock sync |
| 0x31 | PONG               | R→C            | postcard `Pong { id, t0, t1, t2 }` | t1/t2 = relay rx/tx (ns since Unix epoch) |
| 0x3E | DISCONNECT         | any            | postcard `{reason}` | graceful close |
| 0x3F | ERROR              | any            | postcard `ErrorMsg { code, message, fatal }` | |

There is deliberately **no** message type that can carry commands to LMU, the vehicle, inputs, or the
driver's operating system (spec §140, §200). Engineer→driver messages are limited to SNAPSHOT_REQUEST
and session control; the driver app ignores any other message type from the engineer direction.

## 4. Telemetry TLV payload (TELEMETRY_DELTA / TELEMETRY_SNAPSHOT / WEATHER_UPDATE)

```text
u16  schema_version
u64  capture_mono_ns          driver monotonic ns at LMU read
i64  capture_relay_ns_est     driver's estimate of relay clock at capture (i64::MIN if unknown)
f64  source_et_s              LMU elapsed time (mElapsedTime) or NaN if unavailable
u16  record_count
record_count × record:
  u16 channel_id              stable id from telemetry-model registry (docs/CHANNEL_IDS.md)
  u8  tag                     low 5 bits = value type, high 3 bits = quality
  ... value bytes (by type)
```
Value types: 0 NONE (no bytes) · 1 F32 · 2 F64 · 3 I32 · 4 U32 · 5 BOOL(u8) · 6 U8 · 7 I8 · 8 STR (u8 len +
UTF-8, ≤ 64 bytes) · 9 U64. Quality: 0 VALID · 1 INVALID (failed validation) · 2 STALE · 3 UNAVAILABLE ·
4 NOT_SUPPORTED. A record with quality ≠ VALID normally has type NONE. Unknown channel ids MUST be skipped
(forward compatibility) using the type's size; unknown value types are a decode error.

## 5. Sequences

* Every frame a sender emits within a session carries a sequence number that increases by exactly 1 per frame
  of the telemetry class (0x20–0x28), per sender. Control frames use sequence 0.
* Receivers track `expected`, `received`, `lost` (gaps), `duplicated` (seq ≤ last seen and already seen),
  `out_of_order` (seq < highest but not seen). Live state always follows the **highest** sequence; an older
  frame arriving late is counted and discarded for live display (still usable for archival reorder buffer
  of 64 frames).
* `relay_dropped` lets the engineer distinguish intentional relay drops (backpressure) from upstream gaps.
* After reconnect the sender continues its sequence space (driver restart starts a new sequence space and
  sends SESSION_RESUME first; receivers reset tracking on SESSION_INFO).

## 6. Handshake

```text
Client                                 Relay
HELLO{app_ver, proto maj/min,          →
      role, schema_versions[], client_id}
                                       ← HELLO{relay_ver, proto, server_time_ns, limits, env}
                                         or ERROR{Incompatible, fatal}
AUTH{Password|AccessToken|RefreshToken}→
                                       ← AUTH_OK{account, role, access_token, refresh_token, expires}
                                         or ERROR{AuthFailed}
Driver:  SESSION_CREATE / SESSION_RESUME → SESSION_INFO{session_id, code, resume_token}
Engineer: SESSION_JOIN{code}             → SESSION_INFO (if authorized) → relay forwards last cached
                                           SCHEMA_NEGOTIATE + TELEMETRY_SNAPSHOT + SCORING_UPDATE, and sends
                                           SNAPSHOT_REQUEST to the driver
                                         or ERROR{NotAuthorized} (+ JOIN_REQUEST to driver)
```
Environment separation: `ServerHello.env` ∈ {development, staging, production}. A client built/configured for
production refuses a relay that announces a different environment.

## 7. Heartbeat & timeouts (defaults, configurable)

| constant                  | default | meaning |
|---------------------------|---------|---------|
| HEARTBEAT_INTERVAL_MS     | 1000    | client PING period |
| HEARTBEAT_DEGRADED_MS     | 2500    | no PONG → DEGRADED |
| HEARTBEAT_TIMEOUT_MS      | 5000    | no inbound bytes → RECONNECTING |
| RECONNECT_MIN_MS / MAX_MS | 250 / 5000 | exponential backoff with jitter |
| SNAPSHOT_INTERVAL_MS      | 1000    | driver periodic full snapshot |
| SESSION_DRIVER_GRACE_S    | 600     | relay keeps a session alive after driver disconnect |

## 8. Clock synchronization

PING carries `t0` (client clock, ns). The relay stamps `t1` on receipt and `t2` on send (relay wall clock,
ns since Unix epoch); the client records `t3`. offset θ = ((t1−t0)+(t2−t3))/2, delay δ = (t3−t0)−(t2−t1).
Clients keep the last 16 samples and use the one with minimum δ; uncertainty = δ_min/2. The driver writes
`capture_relay_ns_est = capture_client_ns + θ_driver`. The engineer computes
`age = (now_engineer + θ_engineer) − capture_relay_ns_est`, uncertainty = δ_min,driver/2 + δ_min,engineer/2.

## 9. Errors

`ErrorMsg.code`: BadMagic, Incompatible, Malformed, TooLarge, CrcMismatch, AuthFailed, TokenExpired,
NotAuthorized, SessionNotFound, SessionClosed, RateLimited, ServerBusy, Internal. `fatal=true` → sender closes
the connection after sending.
