# Storage design — local recording (`crates/recorder`)

Status: implemented for v0.x (decision D-006). This document is normative for the `.lmurec` byte layout
(§3) and the index schema (§5); the code cites these sections and its tests check the offsets.

## 1. Scope and decisions

* **What is recorded:** already-encoded records handed over by the driver pipeline: canonical telemetry as
  telemetry-model TLV blocks (the same bytes the network sends, PROTOCOL.md §4), scoring/session/capability
  payloads, weather TLV, markers and, optionally, raw LMU byte blocks. The recorder is payload-agnostic: it
  never decodes telemetry and does not depend on the frame types. Its input is
  `append(kind, capture_mono_ns, bytes)`.
* **How (D-006):** append-only `.lmurec` segment files made of self-delimiting, CRC-protected,
  zstd-compressed chunks of about one second each, plus a SQLite index of sessions, segments, chunk offsets,
  laps, stints and events. The files are the source of truth for recorded data; the index can be rebuilt from
  them (sessions/segments/chunks), except laps, stints, events and metadata updates, which exist only in the
  index.
* **Why not DuckDB/Parquet live, or one SQL row per sample:** those are poor at crash-tolerant incremental
  appends and cost far more per sample. A sequential append with a CRC per chunk is the cheapest write path
  and loses at most the unsynced tail on a crash.
* **LMU stays read-only (D-018):** the recorder writes only inside its configured recordings directory. The
  caller (app-core) chooses that directory (e.g. under `%LOCALAPPDATA%`), never inside the LMU installation or
  its `UserData`.

## 2. Local files

```text
<recordings_dir>/                         configurable (RecorderConfig::recordings_dir)
  recorder.lock                           exclusively locked (LockFileEx) by the owning process
  index.sqlite3  (+ -wal, -shm)           session index, SQLite in WAL mode (§5)
  index.sqlite3.corrupt-<UTC>-<8 hex>     a corrupt index moved aside (only after corruption, §6.4)
  sessions/
    <YYYYMMDDTHHMMSSZ>_<session uuid>/    one directory per session (UTC start, sortable)
      seg-000000.lmurec                   segment files (§3), zero-padded index, never overwritten
      seg-000001.lmurec
```

* One store owns a directory at a time: `RecordingStore::open` takes the lock with `File::try_lock`
  (released by the OS if the process dies) and fails with `DirectoryInUse` otherwise. The lock is held until
  the store **and every writer thread it started** are gone: a `RecordingSession` that outlives its store keeps
  the directory locked until `finish` returns (or its background finalization ends), so no other store can
  "recover" or delete a session that is still being recorded.
* Paths stored in the index are relative to `recordings_dir` and `/`-separated, so a recordings directory
  can be moved or copied as a whole.
* A segment file is created with `create_new` (never truncates an existing file); a name collision moves to the
  next index (up to 16 attempts).

## 3. Recording format (`.lmurec`, format version 1)

All integers are little-endian. CRC-32C is the Castagnoli CRC (crate `crc32c`, standard init/xor-out).

### 3.1 File header (96 bytes, at offset 0 of every segment)

| Offset | Size | Field                      | Notes                                                                 |
|-------:|-----:|----------------------------|-----------------------------------------------------------------------|
| 0      | 8    | magic                      | `89 4C 4D 55 52 45 43 0A` (`\x89LMUREC\n`, PNG-style: catches 7-bit and text-mode damage) |
| 8      | 2    | format_version (u16)       | 1. Checked before the CRC so a newer file is "unsupported", not "corrupt" |
| 10     | 2    | header_len (u16)           | 96                                                                    |
| 12     | 2    | telemetry_schema_version   | `telemetry_model::TELEMETRY_SCHEMA_VERSION` of the producer           |
| 14     | 2    | flags (u16)                | 0 in v1 (non-zero is rejected)                                         |
| 16     | 16   | session_id                 | UUID bytes in RFC 4122 order (as printed)                              |
| 32     | 4    | segment_index (u32)        | 0-based position of the segment in its session                         |
| 36     | 4    | reserved (u32)             | 0 (non-zero is rejected)                                               |
| 40     | 8    | created_unix_ns (i64)      | wall clock at segment creation, ns since the Unix epoch                |
| 48     | 8    | created_mono_ns (u64)      | capture clock read back-to-back with `created_unix_ns`; `u64::MAX` = no capture clock supplied |
| 56     | 1    | app_version_len (u8)       | ≤ 35                                                                   |
| 57     | 35   | app_version                | UTF-8, zero-padded                                                     |
| 92     | 4    | header_crc32c              | CRC-32C of bytes `[0, 92)`                                             |

The header is written, flushed and `sync_data`ed when the segment is created, so an existing segment always
starts with a valid header unless the crash hit that very write (recovery reports such a file as
"created but never written", TRUNCATED). That covers a file shorter than the header and a file exactly one
header long whose header was never (fully) persisted: all zeros, or the file magic followed by zeros that
cover at least the header CRC (after a power loss NTFS can keep the extended length without the data). Any
other invalid header is DAMAGED.

### 3.2 Chunk header (48 bytes)

| Offset | Size | Field                | Notes                                                                       |
|-------:|-----:|----------------------|-----------------------------------------------------------------------------|
| 0      | 4    | magic                | ASCII `LRCH`                                                                |
| 4      | 4    | body_len (u32)       | encoded body length that follows the header ("chunk_len"), ≥ 1             |
| 8      | 4    | record_count (u32)   | records in the body, ≥ 1                                                    |
| 12     | 4    | chunk_seq (u32)      | 0-based sequence within the segment (must be contiguous)                    |
| 16     | 8    | min_capture_ns (u64) | smallest capture time in the chunk ("first capture ns")                     |
| 24     | 8    | max_capture_ns (u64) | largest capture time in the chunk ("last capture ns"); ≥ min; reaches the end of the gap of any gap note in the chunk (§3.4) |
| 32     | 4    | raw_len (u32)        | decoded body length, ≤ 64 MiB; `record_count × 13 ≤ raw_len`               |
| 36     | 4    | body_crc32c          | CRC-32C of the **encoded** body bytes                                       |
| 40     | 1    | codec (u8)           | 0 = stored (body is the raw records, `body_len == raw_len`), 1 = one zstd frame |
| 41     | 3    | reserved             | 0 (non-zero is rejected)                                                    |
| 44     | 4    | header_crc32c        | CRC-32C of bytes `[0, 44)` (covers `body_crc32c`)                           |

A chunk is `header ‖ body`; the next chunk starts right after the body. Because the header CRC is checked
before anything else, a header that decodes has a trustworthy `body_len`: that is what lets a scanner tell a
torn tail from damage in the middle of a file (§6.2). Zstd is used only when it shrinks the data; tiny or
incompressible chunks are stored.

### 3.3 Records (inside a decoded body)

| Offset | Size | Field             |
|-------:|-----:|-------------------|
| 0      | 1    | kind (u8)         |
| 1      | 8    | capture_ns (u64)  |
| 9      | 4    | len (u32), ≤ 8 MiB |
| 13     | len  | payload bytes     |

Records are packed back to back; a valid body is consumed exactly, holds exactly `record_count` records, and
every `capture_ns` lies in `[min_capture_ns, max_capture_ns]`.

Record kinds (append-only, codes never reused):

| Code | Kind                 | Payload                                                                   |
|-----:|----------------------|---------------------------------------------------------------------------|
| 0    | —                    | reserved, never valid                                                     |
| 1    | `TelemetryTlv`       | telemetry-model TLV block (PROTOCOL.md §4)                                |
| 2    | `ScoringPostcard`    | scoring/standings update, postcard (`SCORING_UPDATE` payload)             |
| 3    | `WeatherTlv`         | weather channels, TLV (`WEATHER_UPDATE` payload)                          |
| 4    | `SessionEvent`       | session event, postcard (`SESSION_EVENT` payload)                         |
| 5    | `RawLmuBlock`        | raw bytes copied from `LMU_Data` (producer-defined region header; producer redacts personal data) |
| 6    | `Marker`             | application marker / bookmark                                             |
| 7    | `CapabilityPostcard` | capability map / schema negotiation, postcard                             |
| 8    | `RecorderNote`       | written by the recorder only (§3.4); rejected from `append`               |
| 9–255| unknown              | written by a newer version: preserved and returned as `Unknown(code)`, never reinterpreted |

### 3.4 Recorder notes (gap notes, 28 bytes)

Whenever records are dropped (input queue overrun, write failure, critical disk space), the writer appends a
`RecorderNote` as soon as writing is possible again, so a gap is visible in the recording itself:

| Offset | Size | Field                                                                |
|-------:|-----:|----------------------------------------------------------------------|
| 0      | 1    | note_type = 1 (GAP)                                                   |
| 1      | 1    | reason: 1 overrun, 2 write failure, 3 disk space critical            |
| 2      | 1    | flags: bit 0 first_capture_ns present, bit 1 last_capture_ns present |
| 3      | 1    | reserved = 0                                                          |
| 4      | 8    | dropped_records (u64)                                                 |
| 12     | 8    | first_capture_ns (u64; 0 when absent)                                 |
| 20     | 8    | last_capture_ns (u64; 0 when absent)                                  |

The note record's own `capture_ns` is the capture time of the first dropped record. The header of the chunk
holding the note is widened so that its `max_capture_ns` reaches the note's `last_capture_ns` (record times
still lie inside `[min, max]`), and readers return a note whenever its gap `[first, last]` overlaps the
requested range (§7): a read that starts inside a gap still gets the note, so the gap is never silent.

### 3.5 Limits

| Limit                         | Value                                   | Why                                             |
|-------------------------------|-----------------------------------------|-------------------------------------------------|
| record payload                | 8 MiB (`MAX_RECORD_BYTES`)              | a full raw `LMU_Data` copy is ~325 KB; bounds a corrupt length |
| decoded chunk body            | 64 MiB (`MAX_CHUNK_RAW_BYTES`)          | bounds reader memory                            |
| encoded chunk body            | 64 MiB + 512 KiB + 4 KiB                | ⊇ zstd worst-case expansion (tested against `ZSTD_compressBound`) |
| capture time                  | `i64::MAX` ns (`MAX_CAPTURE_NS`)        | stored as SQLite signed INTEGER without loss    |
| app version string            | 35 bytes                                | fixed header slot                               |

### 3.6 Chunking and segment rotation (defaults in `RecorderConfig`)

* A chunk is sealed before a record that would make it span **≥ 1 s of capture time**
  (`chunk_max_capture_span`), when its first record is **1 s old** by the wall clock (`chunk_max_age`), or
  before it would exceed **1 MiB** uncompressed (`chunk_max_raw_bytes`). The capture-span rule makes chunk
  boundaries a function of the data (≈ 100 records per chunk at 100 Hz), independent of writer speed.
* zstd level **3** (configurable 1–3 only: the live path must stay cheap).
* A segment rotates after a chunk write when it reaches **256 MiB** or is **30 min** old, on
  `request_rotation()` (e.g. LMU elapsed time jumping backwards), at a disk-space suspension and after a
  write failure. A multi-hour race is therefore several files; the next segment is opened lazily by the next
  chunk.

### 3.7 Clocks

`capture_ns` is the producer's capture clock (monotonic ns at the moment LMU data was read); the recorder
never assumes which clock it is. With a capture clock supplied (`SessionOptions::with_capture_clock`), each
segment header stores `(created_unix_ns, created_mono_ns)` read back to back, which anchors capture times to
wall time per segment. Index times named `*_unix_ns` are wall clock; `*_capture_ns` are capture clock.

### 3.8 Compatibility rules

A reader rejects a different `format_version`, a different `header_len`, non-zero flags/reserved fields and
unknown codecs. Additive changes (new record kinds, new note types) do not need a new format version; any
change to the header or chunk layout does.

## 4. Write path

### 4.1 Threads and queues

* One writer **OS thread per recording session**. Producers call `RecordingSession::append` or a cloned
  `RecordSender::append` from any thread; the record is copied into a **bounded** crossbeam channel
  (2048 records ≈ 18 s at 110 records/s, and at most 64 MiB of payload). `append` **never blocks**: when the
  queue is full the record is dropped, counted (`records_overrun`) and reported (see §4.3).
* A separate bounded control queue (256) carries laps, stints, events, metadata updates, sync and rotation
  requests (these return `ControlQueueFull` rather than block). `sync` and `request_rotation` first process
  every record queued before them, so they cover everything appended earlier.
* Events to the application (`RecorderEvent`) go through a bounded queue (256); if the application does not
  drain it, further events are counted in `events_dropped` (they are still logged and indexed).
* Shutdown handshake: `finish` sets a closing flag; an append either sees the flag (rejected as `Stopped`)
  or is waited for (an in-flight counter, both sides `SeqCst`) before the final drain, so no accepted record
  can be stranded in the queue.

### 4.2 Durability

* Sealed chunks go through a 256 KiB write buffer. About every **1 s** (`sync_interval`) the writer flushes and
  calls `File::sync_data` (`FlushFileBuffers`), then records the newly durable chunks (offsets, byte size,
  CRC of the durable prefix, counters) in the index in one transaction. Never per sample.
* Data at risk on a crash: the open chunk (< 1 s) plus written-but-unsynced chunks (< 1 s), i.e. about 2 s.
  `RecordingSession::sync()` makes everything appended so far durable on demand.
* On close (rotation, suspension, finish) the segment is synced and, with `validate_on_close` (default), read
  back and fully validated: the scan must agree with the writer's byte count, record count and running
  CRC-32C, otherwise the segment is marked DAMAGED with both views in the detail.

### 4.3 Failure handling (nothing ever blocks or crashes the producer)

| Situation | Behaviour | Visible as |
|-----------|-----------|------------|
| Input queue full | record dropped at `append` (returns `AppendError::Overrun`) | `records_overrun`; `RecorderOverrun{dropped, total_dropped, first/last capture ns}` at most once per second; WARNING index event; gap note in the file; session `dropped_records` |
| Write / flush / sync / create error | segment abandoned: buffered bytes discarded, the file rescanned to establish exactly what reached it, row marked FAILED with the OS error; records dropped while failed; a new segment attempted every 5 s (`failure_retry_interval`) and once more at finish | `RecordingFailed{segment_index, path, operation, error, os_error, kind}`; CRITICAL index event; `records_dropped_write_failure`; `RecordingResumed{segment_index, dropped_records}`; gap note in the new segment |
| Free space < 5 GiB (`disk_warn_free_bytes`) | warning once per low-space episode | `DiskSpaceLow`; WARNING index event |
| Free space < 1 GiB (`disk_critical_free_bytes`) | segment closed cleanly; recording suspended; records dropped and counted until ≥ 2 GiB free (`disk_resume_free_bytes`, hysteresis) | `DiskSpaceCritical`, `DiskSpaceRecovered`; CRITICAL/INFO index events; `records_dropped_disk_full`; gap note |
| Free space cannot be measured | recording continues (a suspended recording resumes); write errors are then handled as above | log warning |
| Index update fails | recording continues; chunk rows kept pending for retry (up to 4096, then abandoned and rebuilt at close/finish from the files) | `IndexWriteFailed` once per failure streak; `index_errors` |
| Writer thread panics | appends rejected; the session stays ACTIVE and startup recovery handles it | `RecorderError::WriterPanicked` from `finish` |

Free space is measured with `GetDiskFreeSpaceExW` (bytes available to the user) every 10 s
(`disk_check_interval`); the probe is a trait so tests can simulate a filling disk.

Accounting invariant (tested): `records_accepted = records_written + records_dropped_write_failure +
records_dropped_disk_full` once a session is finished; `records_overrun` counts records never accepted.

### 4.4 Diagnostics

`RecorderStats` (state, every counter, queue length and bytes, free space, total and maximum flush + fsync
wall time — the first sign of a slow disk — and the last error) is a cheap snapshot for the `rec.*`
diagnostic channels (CHANNEL_IDS.md 0x0D40–0x0D46). `SessionSummary` (from `finish`) adds the
segments, integrity, checksum and the writer thread's CPU time.

## 5. Index (SQLite, `index.sqlite3`)

`rusqlite` with bundled SQLite. Pragmas: `journal_mode=WAL`, `synchronous=NORMAL` (a power cut may roll back
the last index transactions, never corrupt the database; recorded data does not depend on it),
`foreign_keys=ON`, `busy_timeout=5000`, `application_id=0x4C4D5552` ('LMUR'), `user_version=1`. A database
with another application id, unknown tables or a newer `user_version` is refused. All tables are `STRICT`;
JSON columns are checked with `json_valid`; caller-supplied JSON is also checked before it is queued
(`start_session` for the metadata, `record_lap` / `record_stint` / `update_meta` on the caller's thread,
returning `AppendError::InvalidJson` and counting `control_dropped`), so a malformed field is a caller error
reported at once instead of a lost lap, stint or metadata update (they exist only in the index and cannot be
rebuilt from the recording). Every connection belongs to one thread (the store has one, each
writer thread has its own). Capture times and counters are `u64` in Rust and checked conversions to SQLite's
signed 64-bit INTEGER.

### 5.1 Schema (version 1)

```sql
sessions (id TEXT PK, dir TEXT UNIQUE, status IN ('ACTIVE','COMPLETE','INTERRUPTED'),
          started_unix_ns, ended_unix_ns, recovered_unix_ns, first_capture_ns, last_capture_ns,
          format_version, app_version, telemetry_schema_version, protocol_version, lmu_version,
          interface_version, calculation_engine_version, vehicle, track, session_type,
          data_availability_json (json_valid), record_count, dropped_records,
          integrity_status IN ('UNVERIFIED','OK','TRUNCATED','DAMAGED'), integrity_detail,
          checksum_crc32c, retention_class IN ('DEFAULT','KEEP'))
  INDEX sessions_started (started_unix_ns), sessions_status (status)

segments (session_id FK→sessions ON DELETE CASCADE, segment_index, path,
          status IN ('OPEN','CLOSED','FAILED','RECOVERED','MISSING'), created_unix_ns, created_mono_ns,
          closed_unix_ns, byte_size, file_len, first_capture_ns, last_capture_ns, record_count, chunk_count,
          file_crc32c, integrity_status, integrity_detail, PK (session_id, segment_index))

chunks   (session_id, segment_index, chunk_seq, byte_offset, body_len, record_count,
          min_capture_ns, max_capture_ns, PK (session_id, segment_index, chunk_seq),
          FK (session_id, segment_index)→segments ON DELETE CASCADE)  WITHOUT ROWID

laps     (session_id FK, lap_number, stint_number, start_capture_ns, end_capture_ns, lap_time_s,
          sector1_s, sector2_s, sector3_s, valid (0/1), pit_lap (0/1), metrics_json, PK (session_id, lap_number))
  INDEX laps_time (session_id, start_capture_ns)

stints   (session_id FK, stint_number, start_capture_ns, end_capture_ns, start_lap, end_lap, driver,
          metrics_json, PK (session_id, stint_number))
  INDEX stints_time (session_id, start_capture_ns)

events   (id INTEGER PK, session_id FK, capture_ns, unix_ns, lap, category, severity IN
          ('INFO','WARNING','CRITICAL'), description, channel_id, value, threshold)
  INDEX events_time (session_id, capture_ns), events_lap (session_id, lap)
```

Unknown values are NULL, never a placeholder 0 (non-finite floats are stored as NULL). Event categories:
ALERT (with channel id / value / threshold), LAP, PIT, FLAG, SESSION, RECORDER (overruns, write failures,
disk space, recovery), LMU, NETWORK, USER, or any other upper-case code.

The `chunks` table is the sparse offset index: one row per chunk (≈ one per second of recording, ~3,600 rows
per hour), giving chunk-level seek by capture time without reading the files.

### 5.2 Integrity and checksums

* `segments.byte_size`: while OPEN, the bytes known durable (synced); after close/recovery/validation, the
  validated prefix (file header plus every valid chunk). `file_crc32c` is the CRC-32C of bytes
  `[0, byte_size)`; `file_len` is the physical length seen at the last validation.
* Session checksum (`sessions.checksum_crc32c`): CRC-32C over, for every segment in index order,
  `segment_index u32 | byte_size u64 | file_crc32c u32` (little-endian; `file_crc32c` 0 when unknown).
* Integrity status (byte-level; dropped records are counted separately in `dropped_records`):
  `OK` every byte validated; `TRUNCATED` valid data followed by an incomplete final write (crash) or a
  segment created but never written; `DAMAGED` corruption before the end of a segment, a missing segment, a
  durable prefix that changed, or a mismatch with what the writer wrote; `UNVERIFIED` not validated (yet).
  The session status is the worst over its segments, with per-segment details joined in `integrity_detail`.

### 5.3 Lifecycle

`start_session` inserts the session (ACTIVE, UNVERIFIED) → the writer registers each segment (OPEN) → every
sync appends chunk rows and updates counters → each close stores the validated row (CLOSED / FAILED) →
`finish` writes COMPLETE, end time, totals, integrity and checksum. If index writes failed during the
session, `finish` first rebuilds every segment and chunk row from the files. If the final update itself
fails, the session stays ACTIVE and the next startup recovers it (as INTERRUPTED; the data is intact).
`validate_session` re-validates a finished session on demand (status and chunk rows unchanged). It updates
only the verdict (integrity, detail, observed `file_len`, checksum): the writer-recorded reference of each
segment (`byte_size`, `file_crc32c`, counts, capture bounds) and the session's extent are kept, so every later
validation compares against the same durable truth and damage found once is reported again (it never decays
into TRUNCATED), and the capture bounds used by replay do not shrink.

## 6. Crash tolerance and recovery

### 6.1 What a crash can leave

Only appends happen, so a crash (process kill, BSOD, power loss) can leave: a torn final chunk (header or body
incomplete), a final region that the file system allocated but never wrote (zeros), a final chunk whose body
was only partially persisted, a segment file with only a partial header, a session row still ACTIVE and
segment rows still OPEN with the last synced state. Synced data is intact unless the storage lied about the
sync or was damaged later — both are detected (§6.2, durable-prefix check).

### 6.2 Segment scan (`scan::validate_segment`)

Sequential, never modifies the file, never panics, memory bounded by one chunk. Validates the file header,
then each chunk: header magic and CRC, contiguous `chunk_seq`, body length, body CRC, decompression to exactly
`raw_len`, record framing, record count and per-record capture range. It stops at the first chunk that fails
and classifies the stop:

| Stop | Condition | Segment integrity |
|------|-----------|-------------------|
| `CleanEof` | every byte belongs to a valid chunk | OK |
| `TornTail{IncompleteHeader}` | the file ends inside a chunk header | TRUNCATED |
| `TornTail{IncompleteBody}` | header valid, file ends inside the body | TRUNCATED |
| `TornTail{ZeroFilled}` | only zero bytes from the stop offset to EOF | TRUNCATED |
| `TornTail{InvalidFinalChunk}` | the last chunk is complete in length but invalid and nothing (or only zeros) follows | TRUNCATED |
| `Corrupt` | an invalid chunk with non-zero data after it (mid-file damage), a sequence gap, or a CRC-valid body that does not decode | DAMAGED |

The scan reports the valid length, the valid chunks with their offsets, the CRC of the valid prefix and of
every chunk-boundary prefix, record/note counts, the dropped records announced by gap notes, and the capture
range. Recovery then checks the **durable prefix**: the bytes the index recorded as synced must still be a
chunk boundary of the valid prefix with the same CRC; otherwise the segment is DAMAGED ("the index recorded N
bytes as durable but only M bytes are valid").

**Stepping over a damaged body.** When the stop is `Corrupt` at a chunk whose header CRC passed (so its
`body_len` is what the writer wrote) but whose body fails its CRC or does not decode, the scan does not give
up on the rest of the segment: it steps over that chunk and keeps validating the following chunks while their
headers stay intact and in sequence (`SegmentScan::beyond_damage`: those chunks, damaged ones included, the
offsets and causes of the damaged ones, and the record counts and capture range of the intact ones). The
valid-prefix fields above are unchanged. Every consumer (startup recovery, close-time validation, abandoned
segments, index rebuilds) then indexes the valid prefix **plus** every chunk beyond the damage, and counts the
intact ones in the segment's records and capture bounds; readers report each damaged chunk as an error item
and return the intact ones. `byte_size` / `file_crc32c` stay those of the valid prefix. A damaged chunk
header (its length cannot be trusted) still ends the scan: nothing after it can be located.

### 6.3 Startup recovery (`RecordingStore::open`)

For every ACTIVE session (it was being recorded when the application stopped): every segment (index rows plus
any `seg-*.lmurec` file found in its directory) is rescanned; the index gets the valid length, the chunk rows of
the valid prefix (plus the chunks beyond a damaged body, §6.2) and the integrity status; the session becomes **INTERRUPTED** with `last_capture_ns` = the
last valid capture time, `recovered_unix_ns` set and `ended_unix_ns` NULL (the true end is unknown), and a
RECORDER warning event describing what was recovered. **Files are left exactly as they are**: nothing is
truncated, rewritten or deleted; readers simply stop at the valid length. Per-session problems are reported in
`RecoveryReport::failures` and retried at the next startup; they never block the others.

### 6.4 Orphans and a corrupt index

* A session directory whose id is not in the index (index lost, or a crash between the directory creation
  and the index commit) is **adopted**: a session row is created from its first valid segment header, then it
  is recovered as above (`adopted = true`).
* A corrupt index file (SQLite reports `DatabaseCorrupt`/`NotADatabase`) is **moved aside** (never deleted)
  together with its `-wal`/`-shm` files, a new index is created and every session directory is adopted.
  Laps, stints, events and metadata updates that existed only in the old index are not recovered
  automatically; the set-aside file is kept for inspection (`RecoveryReport::index_set_aside`).

## 7. Reading and replay

* `RecordingStore::open_reader(id)` takes a consistent snapshot (one read transaction) of the session, its
  segments and chunk rows; `SessionReader::records(range)` streams records whose capture time is in the
  inclusive range, segment by segment and chunk by chunk.
* **Chunk-level seek:** segments and chunks whose capture range does not overlap the request are skipped
  without being read; a range that starts late in a session reads from the first overlapping chunk.
* **Bounded memory:** one encoded body and one decoded body are reused; the chunk map is one row per chunk of
  the current segment. A multi-hour session is never loaded as a whole.
* **Verification before use:** every chunk's header must decode (CRC) and match its index row; the body must
  pass its CRC, decompress to the exact length and frame exactly. A failing chunk yields an `Err` item and
  reading continues with the next chunk, so intact data after a damaged region is still returned; records of a
  failed chunk are never returned. Chunks beyond the indexed rows (a segment still being written, rows lost to
  index failures) are found by walking CRC-checked chunk headers up to the segment's valid length (or, for an
  OPEN segment, up to its current length: a session can be read while it is being recorded).
* **Order:** recording order. Capture times are non-decreasing when producers append in capture order, except
  recorder notes, which carry the capture time of the first record of their gap.
* **Gap notes in range reads:** a gap note is returned whenever its gap `[first_capture_ns, last_capture_ns]`
  overlaps the requested range, even when its own capture time lies before the range start; its chunk's header
  covers the whole gap (§3.4), so chunk-level seek reaches it.
* **Replay clock:** `ReplayClock` maps wall time to capture time over `[start_ns, end_ns]` with speeds
  0.25×, 0.5×, 1×, 2×, 4× (exact integer ratios, no drift), play/pause, seek (clamped), continuous speed
  changes and `wall_time_until(target)` for scheduling. It is pure (the caller passes `Instant`s); UI
  integration comes later.

## 8. Retention

Nothing is ever deleted by default. Hooks:

* per-session retention class `DEFAULT` / `KEEP` (`set_retention`); KEEP sessions are never offered to a
  policy;
* `RetentionPolicy::select(candidates, now)` chooses from eligible sessions only (finished, DEFAULT, not
  being recorded); `KeepAll` (default) selects nothing; `MaxAge` is an opt-in example;
* `RecordingStore::apply_retention(&policy)` deletes the selected sessions and refuses any id it did not offer;
* `delete_session(id)` is the explicit user action (also for KEEP sessions; never for a session being
  recorded). A deletion only ever removes `sessions/<that session's directory name>` (checked against the id)
  and then its index rows.

## 9. Export plan (not implemented in this phase)

* **CSV and Parquet** (D-006), produced by streaming a `SessionReader` and decoding TLV blocks with
  `telemetry_model::codec`: one row per telemetry record (capture ns, source ET), one column per channel from
  the registry (value plus quality), SI units as stored (°C for temperatures; display units are never applied
  to exports). Unavailable/invalid values are empty cells / Parquet nulls, never 0. Scoring, events and laps
  export as separate tables; gap notes are exported as explicit gap rows.
* Parquet is written after the fact (never live), chunk by chunk with bounded memory, behind a separate tool
  or feature so the desktop app does not carry the Arrow/Parquet dependency weight. DuckDB reads Parquet
  directly, so no native DuckDB export is planned (stated in D-006).

## 10. Database use

* **SQLite** (rusqlite, bundled): the local index only — sessions, segments, chunk offsets, laps, stints,
  events. Never one row per sample.
* **DuckDB:** not used by the recorder. It appears only as LMU's own telemetry format, read strictly
  read-only by research tooling (D-018).
* **PostgreSQL:** relay metadata only (D-007); recorded telemetry never leaves the driver PC through this
  crate.

## 11. Benchmarks

Measured on the development PC (AMD Ryzen 7 7435HS, 8 cores / 16 threads; NVMe SSD WD SN740; Windows 11;
release build; recordings in `%TEMP%`), with production defaults and synthetic but realistic payloads:
150-channel telemetry TLV records from the shared codec (1,028 bytes each; mostly f32 signals with
sensor-like noise, integer counters, flags, a few unavailable channels). Other builds were running on the
machine at the same time, so CPU and wall figures are given as the range over runs. Run:

```text
cargo test -p recorder --release --test throughput -- --ignored --nocapture --test-threads 1
# longer real-time run (more precise CPU share): set RECORDER_BENCH_REAL_TIME_SECONDS=300
```

| Measure (100 Hz × 150 channels) | Result |
|---------------------------------|--------|
| Payload per recorded hour (raw chunk bodies) | 374.8 MB (360,000 records) |
| **On disk per recorded hour** | **142.6 MB** (paced runs: 144.0–144.9 MB/h); 3,600 chunks |
| Compression ratio (zstd level 3) | 2.63× |
| Writer throughput, flat out | 66,000–116,000 records/s (660–1,160× real time) |
| Writer CPU for one hour of data, flat out (framing, zstd, CRC, writes) | 2.8–4.8 s |
| **Writer CPU share in real time** (paced 100 Hz, 1 sync/s, per-record wake-ups) | **0.42 % of one core** (300 s run; 15 s CPU per hour); 60 s runs measured 0.6–1.4 % |
| flush + `sync_data` wall time per sync | mean 1.6 ms, max 47 ms (299 syncs) |
| Readback of a full hour (verify + decompress + iterate) | 0.6–0.9 s (390,000–590,000 records/s) |

Notes: the CPU share is measured with `GetThreadTimes`, which Windows samples per scheduler tick (15.6 ms),
so short runs of a mostly idle thread are imprecise; the 300 s run (80 samples) is the reference. A 20 s
paced experiment measured 8 ticks (≈ 0.6 %) with the default 1 s sync and 1 tick (≈ 0.08 %) with the
periodic sync disabled: the once-per-second fsync (and its index update) dominates the recorder's cost, which
is the price of the ≤ ~2 s data-loss window. A real session at this rate is ≈ 3.4 GB per 24 h race (`segment_max_bytes` 256 MiB and the 30 min age
limit give about two segments per hour). Compression depends on the entropy of real signals; the synthetic
payload model is in `tests/common/mod.rs`.

## 12. Verification

`cargo test -p recorder` covers, among others:

* byte layout: exact offsets of both headers, every single-bit flip in a chunk header detected, every bit of
  the file header, zstd worst case within the body bound (`format`);
* kill mid-write: a crashed session whose last chunk was handed to the OS but never synced, truncated at
  **every byte offset** of that chunk, then recovered — every complete chunk is returned, the file is never
  modified, integrity is OK at chunk boundaries and TRUNCATED otherwise (`scenario_tests`, `scan`);
* a real process death (child process exits mid-recording without any destructor) recovered at the next open
  (`tests/crash_process.rs`);
* corrupted CRC in a middle chunk: the scan stops the valid prefix there, reports offset and cause, and steps
  over the damaged body to the intact chunks after it (`scan`); validation marks the session DAMAGED; readers
  report that chunk and return everything else (`tests/store_api.rs`); corruption inside the durable prefix
  of a crashed session: recovery keeps every intact chunk after the damage readable (`scenario_tests`);
  re-validation keeps reporting damage in the final chunk and keeps the capture bounds (`tests/store_api.rs`);
  an unpersisted (zero / partial) header is TRUNCATED, not DAMAGED (`recovery`);
* a range read starting inside a gap returns the gap note (`scenario_tests`); invalid lap/stint/metadata
  JSON is refused at the call (`tests/store_api.rs`); a session that outlives its store keeps the directory
  locked (`tests/store_api.rs`); a failed writer-thread start leaves no directory behind (`scenario_tests`);
* deterministic overrun (stalled writer) and exact overrun accounting under four unthrottled producers;
* injected write/create failures (OS error 112): FAILED segment, `RecordingFailed` with the OS error, counted
  drops, retry, `RecordingResumed`, gap note; simulated filling disk: suspension, hysteresis, resume;
* segment rotation by size, age and request; index lifecycle (ACTIVE → COMPLETE / INTERRUPTED), startup
  recovery, orphan adoption, corrupt-index set-aside; retention hooks; single directory owner; replay clock.

## 13. Limitations and open items

* The directory entry of a new segment is not fsynced separately (Windows offers no portable directory fsync;
  NTFS journals metadata). A power loss right after creation could lose the just-created, header-only
  segment file; recovery then reports it MISSING (session DAMAGED) although it held no records.
* Index failure injection is not tested (SQLite errors are hard to provoke deterministically); the code
  paths (pending chunk rows, rebuild at finish, `IndexWriteFailed`) are reviewed but not exercised by tests.
* The writer's CPU time is reported on Windows only (`GetThreadTimes`).
* `validate_session` holds the store's index connection for the duration of the scan (writers use their
  own connections and are not blocked).
* `validate_session` keeps the writer-recorded segment rows (§5.3): after it finds damage, `byte_size` is
  still the recorded durable length and the chunk rows are kept, so readers still reach intact chunks after
  the damage and later validations report the same damage.
