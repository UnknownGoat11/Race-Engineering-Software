# Wire Protocol — `lmure` v1.0

Status: normative for protocol major 1. Implemented in `crates/protocol`. Every change to this file that
alters bytes on the wire must bump `PROTOCOL_MINOR` (compatible addition) or `PROTOCOL_MAJOR` (breaking).
The exact 1.0 payload structs (§10) were fixed together with the first implementation; nothing had been
released before, so they define 1.0 itself and `PROTOCOL_MINOR` stays 0. For the same reason the D-022
engineer-access messages (0x15–0x18, §6) replaced the never-released SESSION_LIST / JOIN_REQUEST /
JOIN_DECISION / ENGINEER_GRANT within 1.0, without a compatibility shim.

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
                             previous forwarded frame) · relay_flags u32 (bit0 REPLAYED: cached frame
                             replayed on join, §6 · bits 1–31 reserved, must be 0)
end    4    crc32c           CRC-32C (Castagnoli) over every preceding byte of the frame
```

Header = 40 bytes; minimum frame = 44 bytes. Receivers MUST:
1. read exactly 40 bytes; reject wrong magic (close connection, `ERROR BadMagic`);
2. reject `protocol_major` ≠ supported major (`ERROR Incompatible`) **without decoding the payload**;
3. reject `payload_len` > `MAX_PAYLOAD_BYTES` *before allocating*;
4. read payload (+ trailer) + crc; verify CRC; on failure close the connection (framing is then untrusted);
5. reject non-zero reserved bits/fields (header byte 7, bytes 36–39, flag bits 2–7, trailer relay_flags bits 1–31) and
   a priority byte other than 0/1/2/0xFF.

Steps 1–3 run on the header bytes alone (magic is checked as soon as 2 bytes are buffered, the major as soon as
3 are); nothing is reserved for a frame before they pass. Failures of steps 1–5 are **fatal** (framing is
untrusted; the decoder is poisoned). *Header rule* violations of an otherwise valid frame are **not** fatal: the
receiver skips and counts the frame. Header rules: unknown `msg_type` (skipped for forward compatibility);
priority not allowed for the type (§2); a control-class type (outside 0x20–0x28) with `sequence ≠ 0`; a
stream-class type (0x20–0x28) with `sequence = 0` or `session_id = 0`. A 1.0 sender sets **no** flag bit:
bit0 is set by the relay only, and bit1 (PAYLOAD_ZSTD) is refused by every 1.0 receiver (`Malformed`), so the
encoder (`Frame::encode_with`) rejects any flag (`InvalidSendFlags`).

TCP is a byte stream: implementations read with an accumulating buffer; one `read()` ≠ one frame.

The relay validates the header and CRC, then **appends** the relay trailer (sets flags.bit0 and recomputes the
CRC) when forwarding driver telemetry to an engineer. It never decodes or modifies the payload.

**Session isolation (MUST).** The header `session_id` is sender-controlled, so the relay never routes by it.
Each authenticated connection is bound to at most one session once the relay accepted its SESSION_CREATE,
SESSION_JOIN or SESSION_RESUME; the relay routes every frame from that connection by this binding only. A
client frame whose header violates the binding is a header-rule violation (skipped, counted, never forwarded;
the relay may answer `ERROR NotAuthorized`): before binding the header `session_id` must be 0; after binding
stream frames must carry exactly the bound id and control frames the bound id or 0. A client frame that sets
any flag bit (RELAY_TRAILER, PAYLOAD_ZSTD), or whose type the connection's authenticated role may not send
(§3 "dir", e.g. an engineer sending TELEMETRY_SNAPSHOT or ENGINEER_KICK), is a header-rule violation as well:
never forwarded, cached or acted on. Implementation: `frame::check_session_binding`,
`frame::check_client_frame` (all relay-side header rules in one call).

Decoded frames are zero-copy views of the read buffer. Any frame kept beyond the current dispatch (join cache,
latest-wins slots, reorder buffers) MUST be copied with `Frame::detach`, otherwise it pins the whole read
buffer (up to ~512 KiB) for its lifetime.

## 2. Priorities & queues

* P0: vehicle-critical state, timing, fuel, position, race status. P1: tyres, brakes, energy. P2: detail.
* Control frames (`priority = 0xFF`) go through a small reliable queue and are never dropped. A writer serves
  the reliable queue before the latest-wins slots, so a reliable frame is never overtaken by telemetry the
  relay received after it (low event/PONG latency; keeps §5 restart detection sound).
* Priority per type: TELEMETRY_DELTA, TELEMETRY_SNAPSHOT, SCORING_UPDATE, WEATHER_UPDATE, DRIVER_STATUS and
  RECORDING_STATUS are latest-wins and MUST use P0–P2 (defaults: WEATHER_UPDATE P2, the others P0; deltas are
  split into one frame per channel priority class). Every other type — including the stream-class
  SCHEMA_NEGOTIATE, SESSION_EVENT and SNAPSHOT_REQUEST, which must never be dropped — MUST use 0xFF.
* Live telemetry frames go through a per-receiver **latest-wins** slot per priority. When a newer frame of the
  same `msg_type`+priority arrives before the old one was written, the old one is dropped and counted in
  `relay_dropped`. Snapshots are never superseded by deltas (a pending snapshot is sent first).
* Consequence for receivers: frames of different keys (and reliable control-queue frames) legitimately arrive
  out of global sequence order. Liveness is therefore decided per key, never against one global highest
  sequence (§5).

## 3. Message types

| id   | name               | dir            | payload encoding | purpose |
|------|--------------------|----------------|------------------|---------|
| 0x01 | HELLO              | C→R, R→C       | postcard `HelloPayload` (§10) | version + role + capabilities |
| 0x02 | AUTH               | C→R            | postcard `Auth`  | password / access token / refresh token |
| 0x03 | AUTH_OK            | R→C            | postcard `AuthOk`| identity, role, new tokens |
| 0x04 | ACCOUNT_REGISTER   | C→R            | postcard `AccountRegister` | only if relay allows open registration |
| 0x10 | SESSION_CREATE     | D→R            | postcard `SessionCreate` | vehicle/track/session/LMU version/capabilities |
| 0x11 | SESSION_JOIN       | E→R            | postcard `SessionJoin`   | redeem a one-time connection code (§6, D-022) |
| 0x12 | SESSION_INFO       | R→C            | postcard `SessionInfo`   | session id, driver identity for the confirmation card, car, class, track, session type, start, schema, resume credential |
| 0x13 | SESSION_RESUME     | D→R, E→R       | postcard `SessionResume` | reconnect: driver resume token, or engineer resume ticket |
| 0x14 | SESSION_CLOSE      | D→R, R→E       | postcard `SessionClose`  | driver stopped / expired / engineer removed by the driver |
| 0x15 | CONNECT_CODE_REQUEST | D→R          | postcard `ConnectCodeRequest` (empty) | issue the connection code, or rotate it (**New code**) |
| 0x16 | CONNECT_CODE       | R→D            | postcard `ConnectCode`   | current code + expiry (or none), engineers connected / max |
| 0x17 | ENGINEER_LIST      | R→D            | postcard `EngineerList`  | engineers holding a slot: name, username, since, link state |
| 0x18 | ENGINEER_KICK      | D→R            | postcard `EngineerKick`  | disconnect one engineer (its ticket dies, slot freed) |
| 0x20 | SCHEMA_NEGOTIATE   | D→R→E          | postcard `SchemaNegotiate` | schema version + capability map (channel availability, source, native rate) |
| 0x21 | TELEMETRY_DELTA    | D→R→E          | TLV (§4)         | changed/high-rate channels |
| 0x22 | TELEMETRY_SNAPSHOT | D→R→E          | TLV (§4) full    | every channel incl. unavailable markers |
| 0x23 | SCORING_UPDATE     | D→R→E          | postcard `ScoringUpdate` | race/session scoring + competitor list (native scoring rate) |
| 0x24 | WEATHER_UPDATE     | D→R→E          | TLV (§4)         | weather channels (on change / ≤1 Hz) |
| 0x25 | SESSION_EVENT      | D→R→E          | postcard `SessionEventMsg` | lap/sector/pit/flag/LMU-lost/recording events |
| 0x26 | SNAPSHOT_REQUEST   | E→R→D          | postcard `SnapshotRequest` | ask for a fresh snapshot (after gap/reconnect) |
| 0x27 | DRIVER_STATUS      | D→R→E          | postcard `DriverStatus` | layered health: LMU source, processing, recording, rates (1 Hz) |
| 0x28 | RECORDING_STATUS   | D→R→E          | postcard `RecordingStatus` | recording active/failed/disk space |
| 0x30 | PING               | C→R            | postcard `Ping { id, t0 }` | heartbeat + clock sync |
| 0x31 | PONG               | R→C            | postcard `Pong { id, t0, t1, t2 }` | t1/t2 = relay rx/tx (ns since Unix epoch) |
| 0x3E | DISCONNECT         | any            | postcard `Disconnect { reason, message }` | graceful close |
| 0x3F | ERROR              | any            | postcard `ErrorMsg { code, message, fatal }` | |

There is deliberately **no** message type that can carry commands to LMU, the vehicle, inputs, or the
driver's operating system (spec §140, §200). Engineer→driver messages are limited to SNAPSHOT_REQUEST
and session control; the driver app ignores any other message type from the engineer direction.
Concretely, the only types a driver processes from the relay are HELLO (server form), AUTH_OK, SESSION_INFO,
CONNECT_CODE, ENGINEER_LIST, SNAPSHOT_REQUEST, PONG, DISCONNECT and ERROR (`protocol::frame::driver_accepts`,
`Message::driver_accepts`). CONNECT_CODE and ENGINEER_LIST originate at the relay; the relay forwards exactly
one type engineer→driver: SNAPSHOT_REQUEST. Engineers originate only HELLO, AUTH, ACCOUNT_REGISTER,
SESSION_JOIN, SESSION_RESUME, SNAPSHOT_REQUEST, PING, DISCONNECT and ERROR. There is no message to discover or
list drivers: an engineer reaches a driver only with a one-time code from that driver (D-022). The relay
enforces the "dir" column per authenticated role (`frame::check_client_frame`). Clients never talk to each
other directly.

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
* **Loss accounting** (`SequenceTracker`): receivers track `expected`, `received`, `lost` (gaps),
  `duplicated` (already seen), `out_of_order` (seq < highest but not seen), `too_old`, `restarts`.
  `observe(seq)` classifies First / InOrder / Gap{missing} / Late / Duplicate / TooOld / Restart. "Seen" is
  remembered for the 64 sequence numbers ending at the highest, so a frame ≥ 64 below the highest is `TooOld`
  (counted separately). `lost` is the number of currently missing sequences (a late frame decrements it).
  Example: 501, 502, 504 → lost = 1 (503); 100, 101, 103, 102, 104 → out_of_order = 1, lost = 0. This view
  does **not** decide what is displayed.
* **Live state** (`LiveGate`): "accept only if newer" applies **per latest-wins key** `(msg_type, priority)`
  (§2), not against one global highest sequence. A frame is applied iff its sequence is above the last one
  applied for its key. Reliable stream types (SCHEMA_NEGOTIATE, SESSION_EVENT) travel in the FIFO control
  queue, so the same rule delivers each of them exactly once and never drops one for being overtaken by
  telemetry. An applied TELEMETRY_SNAPSHOT carries every channel (weather included), so it supersedes every
  TELEMETRY_DELTA (any priority) and WEATHER_UPDATE with a sequence ≤ its own; a snapshot older than an already
  applied delta or weather update is not applied (it would roll those channels back; the next periodic snapshot
  restores the full state). Example: P1 delta 201 and P0 delta 202 arriving as 202, 201 are both applied.
* **Join replay**: frames the relay replays from its cache on join (§6) carry the REPLAYED trailer flag. The
  receiver applies them through the live gate but never feeds them to the sequence tracker; tracking starts at
  the first non-replayed frame. No false loss is reported and no replayed frame is classified late.
* **Session lifecycle**: a receiver resets tracking and live gating on every SESSION_INFO. After reconnect the
  sender continues its sequence space; a driver restart starts a new one (seq 1, 2, …) announced with
  `SESSION_RESUME{new_sequence_space: true}`, and the relay then sends SESSION_INFO to every engineer of the
  session **before** forwarding any frame of the new space (§6). Defence in depth: the tracker reports
  `Restart` (and resets) after `RESTART_DETECT_RUN` = 36 consecutive stale (Duplicate/TooOld) frames with
  strictly increasing sequences. A conforming relay never produces such a run: reliable frames go first (§2),
  and between two fresh frames at most one frame per other latest-wins key arrives (18 keys = 6 latest-wins
  types × P0–P2; the threshold is twice that).
* **SNAPSHOT_REQUEST** reaches the driver from several sequence spaces (each engineer's own, plus the relay's
  on join), so the driver neither sequence-tracks nor live-gates it. The driver serves at most one
  request-triggered snapshot per `SNAPSHOT_REQUEST_MIN_INTERVAL_MS` (any snapshot sent, periodic included,
  counts); requests inside the interval are coalesced into the next periodic snapshot. The relay forwards at
  most one SNAPSHOT_REQUEST per engineer per `SNAPSHOT_REQUEST_MIN_INTERVAL_MS` and drops (counts) the rest.
* `relay_dropped` lets the engineer distinguish intentional relay drops (backpressure) from upstream gaps.
* Implementation (`protocol::sequence`): `StreamReceiver::receive_frame` combines all of the above for one
  inbound stream (`Reception { arrival, live }`); `SnapshotRequestLimiter` implements the request rate rule.
  `ReorderBuffer` releases frames in order for archival use and gives up on a missing sequence once the
  buffered span reaches 64 frames (counted as `skipped`).

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
Driver:   SESSION_CREATE                   → SESSION_INFO{.., DriverToken} + CONNECT_CODE{code, 0/2}
                                             + ENGINEER_LIST{[]}
          SESSION_RESUME::Driver{token}    → SESSION_INFO + CONNECT_CODE + ENGINEER_LIST (current state)
          CONNECT_CODE_REQUEST             → CONNECT_CODE (fresh code; the unused one is invalidated)
          ENGINEER_KICK{connection_id}     → ENGINEER_LIST + CONNECT_CODE; that engineer gets
                                             SESSION_CLOSE{RemovedByDriver} and is disconnected
Engineer: SESSION_JOIN{code}               → SESSION_INFO{identity, EngineerTicket} → cached SCHEMA_NEGOTIATE
                                             + TELEMETRY_SNAPSHOT + SCORING_UPDATE (REPLAYED trailer flag)
                                             → live frames; the relay sends SNAPSHOT_REQUEST to the driver,
                                             and ENGINEER_LIST + CONNECT_CODE to the driver
                                           or ERROR{CodeInvalid | CodeExpired | SessionFull | SessionClosed
                                                    | NotAuthorized | RateLimited}
          SESSION_RESUME::Engineer{ticket} → as SESSION_JOIN, or ERROR{NotAuthorized} (ticket dead: only a
                                             new code connects)
```
Engineer access (D-022, normative):
* **Connection codes.** 8 random Crockford base32 characters (40 bits) issued by the relay, stored only as a
  hash, compared in constant time and never logged (`Secret` on the wire). Wire form is canonical: upper
  case, no separator (`messages::normalize_connect_code` turns input such as `7kq4-mz9t` into it, reading
  I/L as 1 and O as 0); the driver app shows `XXXX-XXXX`. A code is single use and expires
  `CONNECT_CODE_TTL_S` after issue if unused. The relay keeps at most one unused code per session.
  CONNECT_CODE_REQUEST issues one or rotates it (the unused code is invalidated at once). After
  SESSION_CREATE and after every redemption the relay issues a fresh code if a slot is free; while every slot
  is taken there is no code (`ConnectCode.code = None`). The relay sends CONNECT_CODE whenever the code or the
  slot usage changes, and in reply to every CONNECT_CODE_REQUEST.
* **Redemption.** The relay accepts SESSION_JOIN only if the connection is authenticated with role ENGINEER
  (else NotAuthorized), the code is valid, unused and unexpired (else CodeInvalid / CodeExpired), the session
  is live (else SessionClosed) and an engineer slot is free (else SessionFull, shown as "Driver already has
  2 engineers connected"). The relay remembers the hashes of a live session's used and replaced codes, so a
  join with such a code while every slot is taken gets SessionFull rather than CodeInvalid. Redemptions are
  rate-limited per account and per source IP (`limits::CODE_REDEMPTIONS_PER_MINUTE` / `_PER_HOUR`, else
  RateLimited). Every new engineer connection needs a fresh code: the first one, after an engineer app
  restart, after an explicit disconnect and after removal by the driver. Engineer apps never store a code,
  ticket or driver across app restarts, and never connect to a previous driver by themselves.
* **Identity confirmation.** After SESSION_JOIN the engineer app shows SESSION_INFO's driver display name and
  username, car, class, track, session type and start time, and shows the stream only after the user
  confirmed; **Cancel** sends DISCONNECT{ClientShutdown} (the code stays used). This is client-side: there is
  no confirmation message.
* **Slots.** At most `ServerLimits.max_engineers_per_session` engineers (default
  `limits::MAX_ENGINEERS_PER_SESSION` = 2) hold a slot in a session. A slot is held from redemption until the
  engineer sends DISCONNECT, the driver removes it, its resume ticket expires after a drop, or the session
  ends. A join by an account that already holds a slot in the session takes that slot over (the old
  connection, if still open, gets DISCONNECT{Replaced}; its ticket dies).
* **Network blips.** SESSION_INFO gives each engineer an in-memory resume ticket (random 256-bit, never
  written to disk). While the engineer app keeps running it reconnects by itself with
  `SESSION_RESUME::Engineer{session_id, ticket}` within `ENGINEER_TICKET_TTL_S` after the drop; the relay
  answers like a join (SESSION_INFO, which may carry a new ticket; replay; live). The ticket dies when the
  engineer app closes or sends DISCONNECT, when the driver removes the engineer, when the session ends or
  when it expires. Driver app restarts within the session grace window do not disconnect engineers.
* **Driver controls.** ENGINEER_LIST lists every engineer holding a slot (connection id, display name,
  username, connected since, link state `Healthy | Degraded | Reconnecting`) and is sent whenever it
  changes. ENGINEER_KICK{connection_id} (also for a `Reconnecting` entry, to free a dead slot) makes the
  relay send that engineer SESSION_CLOSE{RemovedByDriver}, close its connection, kill its ticket and free
  the slot. Unknown connection ids are ignored.

Stream lifecycle:
* Join replay (SESSION_JOIN and engineer SESSION_RESUME): the relay sends SESSION_INFO first, then the cached
  frames with relay trailer flag REPLAYED (original header and sequence, never otherwise modified), then live
  frames. Receivers handle them per §5.
* Driver restart: when the relay accepts `SESSION_RESUME::Driver{new_sequence_space: true}` it MUST reset its
  own per-driver sequence state, drop its cached and pending frames of the old space, and send SESSION_INFO to
  every engineer of the session **before** forwarding any frame of the new space. Without this, every
  engineer would discard the new space's frames as stale. A SESSION_INFO for the session an engineer already
  confirmed needs no new confirmation.

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
| SNAPSHOT_REQUEST_MIN_INTERVAL_MS | 250 | driver: min interval of request-triggered snapshots; relay: per-engineer SNAPSHOT_REQUEST rate (§5) |
| SESSION_DRIVER_GRACE_S    | 600     | relay keeps a session alive after driver disconnect |
| CONNECT_CODE_TTL_S        | 600     | an unused connection code expires (D-022) |
| ENGINEER_TICKET_TTL_S     | 600     | an engineer's resume ticket stays valid after its connection dropped (D-022) |
| MAX_ENGINEERS_PER_SESSION | 2       | engineer slots per driver session (`protocol::limits`, D-022) |
| CODE_REDEMPTIONS_PER_MINUTE / _PER_HOUR | 5 / 20 | SESSION_JOIN attempts per account and per source IP (`protocol::limits`) |

## 8. Clock synchronization

PING carries `t0` (client clock, ns). The relay stamps `t1` on receipt and `t2` on send (relay wall clock,
ns since Unix epoch); the client records `t3`. offset θ = ((t1−t0)+(t2−t3))/2, delay δ = (t3−t0)−(t2−t1).
Clients keep the last 16 samples and use the one with minimum δ; uncertainty = δ_min/2. The driver writes
`capture_relay_ns_est = capture_client_ns + θ_driver`. The engineer computes
`age = (now_engineer + θ_engineer) − capture_relay_ns_est`, uncertainty = δ_min,driver/2 + δ_min,engineer/2.
The driver publishes θ_driver and δ_min,driver/2 in `DriverStatus.clock` so the engineer can do this.

Implementation (`protocol::clock`): arithmetic in i128; θ uses floor division by 2; a sample is rejected
(not added to the window) if t3 < t0, t2 < t1, δ < 0, θ outside i64, or the PONG's `id`/`t0` do not match the
outstanding PING. On equal δ the newest sample wins.

## 9. Errors

`ErrorMsg.code`: BadMagic, Incompatible, Malformed, TooLarge, CrcMismatch, AuthFailed, TokenExpired,
NotAuthorized, SessionNotFound, SessionClosed, RateLimited, ServerBusy, Internal, CodeInvalid, CodeExpired,
SessionFull. `fatal=true` → sender closes the connection after sending. The listed order is the postcard
variant index (BadMagic = 0 … Internal = 12, CodeInvalid = 13, CodeExpired = 14, SessionFull = 15).
CodeInvalid / CodeExpired / SessionFull answer a SESSION_JOIN (§6): code unknown, malformed, used or
replaced / code expired unused / every engineer slot taken.
Framing failures map to: bad magic → BadMagic, major → Incompatible, payload_len → TooLarge, CRC →
CrcMismatch, reserved/priority/truncation → Malformed; payload (postcard/TLV) failures → Malformed.

## 10. Postcard payload definitions (normative for 1.0)

Encoding rules (postcard 1.x, D-005): integers `u16/u32/u64/i32/i64` are LEB128 varints (signed: zigzag),
`u8`/`bool` one byte, `f32`/`f64` little-endian IEEE, `String` = varint length + UTF-8, `Vec<T>`/maps = varint
count + elements, `Option<T>` = `0x00` | `0x01 T`, fixed arrays without length, struct fields in declaration
order without names, enum = varint variant index (declaration order) + variant fields, newtypes = inner
value. **Field and variant order is the wire format**. `Secret` is a plain `String` on the wire (its `Debug`
is redacted). Wire example: `Ping { id: 1, t0: 300 }` = `01 AC 02`.

Exception — LMU-derived enums (`telemetry_model::enums::*` and `SessionType`) are **not** encoded by variant
index: each is serialized as its raw LMU value in the LMU field's integer type (e.g. `CarFlag` as `u8`,
`SessionType` as `i32` zigzag varint) and decoded through `from_raw` / `from_lmu`, so reordering or adding
variants never changes the wire bytes and an unknown raw value survives the round trip.

Version evolution within major 1 (normative):
* A new minor may only **append bytes at the end of a payload** (new trailing fields of the top-level payload
  struct, or of the struct that ends it). Changes in the middle of a payload, or inside nested types that are
  followed by other fields, need a major bump.
* Every header carries the sender's minor; receivers decode with it (`Message::from_frame` →
  `Message::decode_from_minor`): same minor → strict, every byte consumed (trailing bytes → Malformed);
  sender minor **newer** → bytes after the known fields are ignored; sender minor **older** → decoded with
  that minor's layout (1.0 is the first minor; every minor that appends fields adds a tested per-minor decode
  path for the older layouts).
* New enum variants cannot be decoded by older peers: a sender never uses fields/variants newer than the
  negotiated minor (`min(own, peer)`).
* **Frozen for major 1** (exchanged before or without negotiation): `HelloPayload`, `Hello`, `ServerHello`,
  `ProtocolVersion`, `ClientRole`, `ServerLimits`, `Environment`, `ErrorMsg`, `ErrorCode` (including the
  D-022 codes 13–15, which are part of 1.0), `Disconnect`, `DisconnectReason`. Extending any of them requires
  a new major.

Types from `telemetry-model` are embedded verbatim (`SessionIdentity`, `VehicleCapabilities`,
`CapabilityMap`, `ScoringState`, `EventRecord`, `SessionType`, …): their field/variant order is therefore
part of this wire format, and changing them requires a `TELEMETRY_SCHEMA_VERSION` bump and/or a
`PROTOCOL_MINOR` bump.

Decoder resource bound: postcard never reads beyond the payload (≤ `MAX_PAYLOAD_BYTES`); serde's sequence
pre-allocation is capped by the remaining payload length and by serde's 1 MiB per-sequence limit, so a hostile
payload can cause at most a bounded, transient allocation (the frame decoder itself never allocates more than
`MAX_PAYLOAD_BYTES` at once, tested with a counting allocator).

```rust
// 0x01 HELLO — self-describing: variant 0 = client form, 1 = relay form.
enum HelloPayload { Client(Hello), Server(ServerHello) }
struct Hello { app_version: String, protocol: ProtocolVersion, role: ClientRole,
               schema_versions: Vec<u16>, client_id: String }
struct ProtocolVersion { major: u8, minor: u8 }
enum ClientRole { Driver, Engineer }
struct ServerHello { relay_version: String, protocol: ProtocolVersion, server_time_ns: u64 /*Unix ns*/,
                     limits: ServerLimits, env: Environment }
struct ServerLimits { max_payload_bytes: u32, heartbeat_interval_ms: u32, heartbeat_timeout_ms: u32,
                      max_engineers_per_session: u16, session_driver_grace_s: u32 }
enum Environment { Development, Staging, Production }
// 0x02–0x04
enum Auth { Password { username: String, password: Secret }, AccessToken { token: Secret },
            RefreshToken { token: Secret } }
struct AuthOk { account: AccountId, username: String, display_name: String, role: ClientRole,
                access_token: Secret, refresh_token: Secret,
                access_expires_at_ns: u64, refresh_expires_at_ns: u64 /*Unix ns*/ }
struct AccountId([u8; 16])                      // UUID bytes
struct AccountRegister { username: String, password: Secret, display_name: String }
// 0x10–0x18
struct SessionCreate { identity: telemetry_model::scoring::SessionIdentity, lmu_version: Option<String>,
                       app_version: String, schema_version: u16,
                       capabilities: telemetry_model::scoring::VehicleCapabilities }
struct SessionJoin { code: Secret /*canonical connection code (§6)*/ }
struct SessionInfo { session_id: u64, driver_display_name: String, driver_username: String,
                     car: Option<String>, vehicle_class: Option<String>, track: Option<String>,
                     session_type: Option<telemetry_model::enums::SessionType>, started_at_ns: u64 /*Unix ns*/,
                     schema_version: u16, resume: ResumeCredential }
enum ResumeCredential { DriverToken(Secret) /*driver copy*/, EngineerTicket(Secret) /*engineer copy*/ }
enum SessionResume { Driver { session_id: u64, resume_token: Secret, new_sequence_space: bool },
                     Engineer { session_id: u64, ticket: Secret } }
struct SessionClose { session_id: u64, reason: SessionCloseReason }
enum SessionCloseReason { DriverStopped, Expired, RemovedByDriver, RelayShutdown }
struct ConnectCodeRequest;                        // empty payload
struct ConnectCode { code: Option<IssuedCode>, engineers_connected: u16, engineers_max: u16 }
struct IssuedCode { code: Secret, expires_at_ns: u64 /*Unix ns*/ }
struct EngineerList { engineers: Vec<EngineerEntry> }
struct EngineerEntry { connection_id: u64, username: String, display_name: String,
                       connected_since_ns: u64 /*Unix ns*/, link: EngineerLink }
enum EngineerLink { Healthy, Degraded, Reconnecting }
struct EngineerKick { connection_id: u64 }
// 0x20–0x28 (0x21/0x22/0x24 are TLV, §4)
struct SchemaNegotiate { schema_version: u16, capabilities: telemetry_model::scoring::CapabilityMap,
                         vehicle_capabilities: VehicleCapabilities }
struct CaptureTime { mono_ns: u64, relay_ns_est: Option<i64> /*None = no clock sample yet*/ }
struct ScoringUpdate { capture: CaptureTime, scoring: telemetry_model::scoring::ScoringState }
struct SessionEventMsg { capture: CaptureTime, record: telemetry_model::events::EventRecord }
struct SnapshotRequest { reason: SnapshotReason }
enum SnapshotReason { EngineerJoined, SequenceGap, Reconnect, Manual }
struct DriverStatus { lmu_state: LmuSourceState, lmu_source: ComponentHealth, processing: ComponentHealth,
                      recording: ComponentHealth, network: ComponentHealth, rates: DriverRates,
                      clock: Option<ClockSummary> }
enum LmuSourceState { NotRunning, Connecting, Live, Paused, Stale, Error }
struct ComponentHealth { level: HealthLevel, detail: Option<String> }
enum HealthLevel { Ok, Degraded, Failed, Inactive }
struct DriverRates { telemetry_read_hz: Option<f32>, scoring_read_hz: Option<f32>,
                     frames_sent_hz: Option<f32>, bytes_sent_per_s: Option<u64>,
                     frames_dropped_total: u64 }          // None = not measured (never 0)
struct ClockSummary { offset_ns: i64, uncertainty_ns: u64 }
struct RecordingStatus { state: RecordingState, recording_id: Option<String>, bytes_written: u64,
                         frames_written: u64, frames_lost: u64, disk_free_bytes: Option<u64>,
                         error: Option<String> }
enum RecordingState { Off, Active, Failed }
// 0x30–0x3F
struct Ping { id: u64, t0: u64 }
struct Pong { id: u64, t0: u64, t1: u64, t2: u64 }
struct Disconnect { reason: DisconnectReason, message: Option<String> }
enum DisconnectReason { ClientShutdown, ServerShutdown, Replaced, IdleTimeout, SessionEnded }
struct ErrorMsg { code: ErrorCode, message: String, fatal: bool }
```
