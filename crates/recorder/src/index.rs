//! SQLite session index (`index.sqlite3` in the recordings directory).
//!
//! Tables: `sessions`, `segments`, `chunks` (offset index, one row per chunk, i.e. about one per second of
//! recording), `laps`, `stints` and `events`. The `.lmurec` files are the source of truth for recorded data:
//! sessions, segments and chunks can be rebuilt from them (startup recovery does that for interrupted
//! sessions and adopts orphan session directories). Metadata updates, laps, stints and events exist only
//! here. Schema and semantics: `docs/STORAGE_DESIGN.md` §5.
//!
//! Every capture time and counter is a `u64` in Rust and a signed 64-bit INTEGER in SQLite; conversions are
//! checked (the writer rejects capture times above [`crate::format::MAX_CAPTURE_NS`], so they never fail in
//! normal operation).

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::types::Type;
use rusqlite::{Connection, ErrorCode, OptionalExtension, Row, TransactionBehavior, params};
use thiserror::Error;

use crate::COMPONENT;
use crate::format::CHUNK_HEADER_LEN;
use crate::scan::ChunkLoc;
use crate::types::{
    EventCategory, EventRecord, IntegrityStatus, LapRecord, RetentionClass, SegmentStatus,
    SessionId, SessionMeta, SessionMetaUpdate, SessionStatus, Severity, StintRecord, TimeRange,
};

/// Version of the index schema (`PRAGMA user_version`).
pub const INDEX_SCHEMA_VERSION: i32 = 1;
/// `PRAGMA application_id` marking the file as an LMU Race Engineering recording index (ASCII `LMUR`).
pub const INDEX_APPLICATION_ID: i32 = 0x4C4D_5552;
/// How long a connection waits for another connection's write lock before failing with "busy".
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Schema version 1. STRICT tables reject values of the wrong type instead of silently coercing them.
const SCHEMA_V1: &str = "
CREATE TABLE sessions (
    id                         TEXT PRIMARY KEY NOT NULL,
    dir                        TEXT NOT NULL UNIQUE,
    status                     TEXT NOT NULL CHECK (status IN ('ACTIVE', 'COMPLETE', 'INTERRUPTED')),
    started_unix_ns            INTEGER NOT NULL,
    ended_unix_ns              INTEGER,
    recovered_unix_ns          INTEGER,
    first_capture_ns           INTEGER,
    last_capture_ns            INTEGER,
    format_version             INTEGER NOT NULL,
    app_version                TEXT NOT NULL,
    telemetry_schema_version   INTEGER NOT NULL,
    protocol_version           INTEGER,
    lmu_version                TEXT,
    interface_version          TEXT,
    calculation_engine_version TEXT,
    vehicle                    TEXT,
    track                      TEXT,
    session_type               TEXT,
    data_availability_json     TEXT CHECK (data_availability_json IS NULL OR json_valid(data_availability_json)),
    record_count               INTEGER NOT NULL DEFAULT 0,
    dropped_records            INTEGER NOT NULL DEFAULT 0,
    integrity_status           TEXT NOT NULL CHECK (integrity_status IN ('UNVERIFIED', 'OK', 'TRUNCATED', 'DAMAGED')),
    integrity_detail           TEXT,
    checksum_crc32c            INTEGER,
    retention_class            TEXT NOT NULL DEFAULT 'DEFAULT' CHECK (retention_class IN ('DEFAULT', 'KEEP'))
) STRICT;
CREATE INDEX sessions_started ON sessions (started_unix_ns);
CREATE INDEX sessions_status ON sessions (status);

CREATE TABLE segments (
    session_id       TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    segment_index    INTEGER NOT NULL,
    path             TEXT NOT NULL,
    status           TEXT NOT NULL CHECK (status IN ('OPEN', 'CLOSED', 'FAILED', 'RECOVERED', 'MISSING')),
    created_unix_ns  INTEGER NOT NULL,
    created_mono_ns  INTEGER,
    closed_unix_ns   INTEGER,
    byte_size        INTEGER NOT NULL,
    file_len         INTEGER,
    first_capture_ns INTEGER,
    last_capture_ns  INTEGER,
    record_count     INTEGER NOT NULL DEFAULT 0,
    chunk_count      INTEGER NOT NULL DEFAULT 0,
    file_crc32c      INTEGER,
    integrity_status TEXT NOT NULL CHECK (integrity_status IN ('UNVERIFIED', 'OK', 'TRUNCATED', 'DAMAGED')),
    integrity_detail TEXT,
    PRIMARY KEY (session_id, segment_index)
) STRICT;

CREATE TABLE chunks (
    session_id     TEXT NOT NULL,
    segment_index  INTEGER NOT NULL,
    chunk_seq      INTEGER NOT NULL,
    byte_offset    INTEGER NOT NULL,
    body_len       INTEGER NOT NULL,
    record_count   INTEGER NOT NULL,
    min_capture_ns INTEGER NOT NULL,
    max_capture_ns INTEGER NOT NULL,
    PRIMARY KEY (session_id, segment_index, chunk_seq),
    FOREIGN KEY (session_id, segment_index) REFERENCES segments (session_id, segment_index) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE laps (
    session_id       TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    lap_number       INTEGER NOT NULL,
    stint_number     INTEGER,
    start_capture_ns INTEGER NOT NULL,
    end_capture_ns   INTEGER,
    lap_time_s       REAL,
    sector1_s        REAL,
    sector2_s        REAL,
    sector3_s        REAL,
    valid            INTEGER CHECK (valid IN (0, 1)),
    pit_lap          INTEGER CHECK (pit_lap IN (0, 1)),
    metrics_json     TEXT CHECK (metrics_json IS NULL OR json_valid(metrics_json)),
    PRIMARY KEY (session_id, lap_number)
) STRICT;
CREATE INDEX laps_time ON laps (session_id, start_capture_ns);

CREATE TABLE stints (
    session_id       TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    stint_number     INTEGER NOT NULL,
    start_capture_ns INTEGER NOT NULL,
    end_capture_ns   INTEGER,
    start_lap        INTEGER,
    end_lap          INTEGER,
    driver           TEXT,
    metrics_json     TEXT CHECK (metrics_json IS NULL OR json_valid(metrics_json)),
    PRIMARY KEY (session_id, stint_number)
) STRICT;
CREATE INDEX stints_time ON stints (session_id, start_capture_ns);

CREATE TABLE events (
    id          INTEGER PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    capture_ns  INTEGER,
    unix_ns     INTEGER,
    lap         INTEGER,
    category    TEXT NOT NULL,
    severity    TEXT NOT NULL CHECK (severity IN ('INFO', 'WARNING', 'CRITICAL')),
    description TEXT NOT NULL,
    channel_id  INTEGER,
    value       REAL,
    threshold   REAL
) STRICT;
CREATE INDEX events_time ON events (session_id, capture_ns);
CREATE INDEX events_lap ON events (session_id, lap);
";

/// Errors from the session index.
#[derive(Debug, Error)]
pub enum IndexError {
    #[error("SQLite error while {op}: {source}")]
    Sqlite {
        op: &'static str,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "index schema version {found} is not supported by this build (it supports {supported})"
    )]
    UnsupportedSchema { found: i32, supported: i32 },
    #[error("{path} is not an LMU Race Engineering recording index (application_id {found:#010x})")]
    ForeignDatabase { path: PathBuf, found: i32 },
    #[error("{field} value {value} does not fit the index's signed 64-bit integer range")]
    OutOfRange { field: &'static str, value: u64 },
}

impl IndexError {
    /// True when SQLite reports the database file itself as corrupt or not a database (as opposed to a
    /// transient condition such as "busy").
    pub fn is_corruption(&self) -> bool {
        matches!(
            self,
            IndexError::Sqlite { source, .. }
                if matches!(source.sqlite_error_code(), Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase))
        )
    }
}

/// Attaches the operation name to a rusqlite error.
trait SqlContext<T> {
    fn ctx(self, op: &'static str) -> Result<T, IndexError>;
}

impl<T> SqlContext<T> for rusqlite::Result<T> {
    fn ctx(self, op: &'static str) -> Result<T, IndexError> {
        self.map_err(|source| IndexError::Sqlite { op, source })
    }
}

/// Session row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// Session id (also in every segment header).
    pub id: SessionId,
    /// Session directory relative to the recordings directory, `/`-separated.
    pub dir: String,
    /// Lifecycle status.
    pub status: SessionStatus,
    /// Wall clock when recording started (ns since the Unix epoch).
    pub started_unix_ns: i64,
    /// Wall clock of a normal finish; `None` while ACTIVE and for INTERRUPTED sessions (the true end time of
    /// an interrupted session is unknown; `last_capture_ns` is the last valid data).
    pub ended_unix_ns: Option<i64>,
    /// Wall clock at which startup recovery processed the session (INTERRUPTED sessions only).
    pub recovered_unix_ns: Option<i64>,
    /// Earliest valid capture time.
    pub first_capture_ns: Option<u64>,
    /// Last valid capture time (for an INTERRUPTED session: of the last record that survived).
    pub last_capture_ns: Option<u64>,
    /// `.lmurec` format version the session was written with.
    pub format_version: u16,
    /// Versions, vehicle, track, availability map.
    pub meta: SessionMeta,
    /// Records in the valid parts of all segments (producer records plus recorder notes).
    pub record_count: u64,
    /// Records the recorder knows it could not persist (overruns, write failures, disk full).
    pub dropped_records: u64,
    /// Byte-level integrity (worst over the segments).
    pub integrity: IntegrityStatus,
    /// Per-segment details of anything that is not OK.
    pub integrity_detail: Option<String>,
    /// CRC-32C over every segment's `(segment_index u32, byte_size u64, file_crc32c u32)` in index order,
    /// little-endian (docs/STORAGE_DESIGN.md §5.2). `None` until the session is finished or recovered.
    pub checksum_crc32c: Option<u32>,
    /// Retention class (KEEP protects the session from retention policies).
    pub retention: RetentionClass,
}

/// Segment row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentInfo {
    /// Session the segment belongs to.
    pub session_id: SessionId,
    /// 0-based position of the segment in its session.
    pub segment_index: u32,
    /// Segment file relative to the recordings directory, `/`-separated.
    pub path: String,
    /// Lifecycle status.
    pub status: SegmentStatus,
    /// Wall clock at creation (ns since the Unix epoch).
    pub created_unix_ns: i64,
    /// Capture clock at creation (anchors capture times to `created_unix_ns`), if a clock was supplied.
    pub created_mono_ns: Option<u64>,
    /// Wall clock when the writer closed or abandoned the segment.
    pub closed_unix_ns: Option<i64>,
    /// Valid bytes: while OPEN, the bytes known durable (synced); after close or recovery, the validated
    /// prefix (file header plus every valid chunk).
    pub byte_size: u64,
    /// Physical file length observed at the last validation.
    pub file_len: Option<u64>,
    /// Earliest capture time in the valid bytes.
    pub first_capture_ns: Option<u64>,
    /// Latest capture time in the valid bytes.
    pub last_capture_ns: Option<u64>,
    /// Records in the valid bytes (producer records plus recorder notes).
    pub record_count: u64,
    /// Chunks in the valid bytes.
    pub chunk_count: u64,
    /// CRC-32C of bytes `[0, byte_size)`.
    pub file_crc32c: Option<u32>,
    /// Byte-level integrity of the file.
    pub integrity: IntegrityStatus,
    /// What is wrong, when the integrity is not OK (or why the segment was abandoned).
    pub integrity_detail: Option<String>,
}

/// One chunk in the offset index (mirrors the chunk header, docs/STORAGE_DESIGN.md §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkEntry {
    /// Sequence of the chunk within its segment.
    pub chunk_seq: u32,
    /// Byte offset of the chunk header from the start of the segment file.
    pub offset: u64,
    /// Encoded body length (the chunk occupies `48 + body_len` bytes).
    pub body_len: u32,
    /// Records in the chunk.
    pub record_count: u32,
    /// Smallest capture time in the chunk.
    pub min_capture_ns: u64,
    /// Largest capture time in the chunk.
    pub max_capture_ns: u64,
}

impl ChunkEntry {
    /// Offset of the first byte after the chunk.
    pub fn end(&self) -> u64 {
        self.offset + CHUNK_HEADER_LEN as u64 + u64::from(self.body_len)
    }
}

impl From<&ChunkLoc> for ChunkEntry {
    fn from(loc: &ChunkLoc) -> Self {
        Self {
            chunk_seq: loc.header.chunk_seq,
            offset: loc.offset,
            body_len: loc.header.body_len,
            record_count: loc.header.record_count,
            min_capture_ns: loc.header.min_capture_ns,
            max_capture_ns: loc.header.max_capture_ns,
        }
    }
}

/// Filter for [`crate::RecordingStore::events`]. Unset fields do not filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventQuery {
    /// Capture-time range (events without a capture time are excluded when set).
    pub range: Option<TimeRange>,
    /// Only events of this lap.
    pub lap: Option<u32>,
    /// Only events of this category.
    pub category: Option<EventCategory>,
    /// Maximum number of rows.
    pub limit: Option<u32>,
}

/// What [`Index::put_segment`] does with the segment's chunk rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChunkRows {
    /// Leave the existing rows untouched.
    Keep,
    /// Replace every row of the segment with these.
    Replace(Vec<ChunkEntry>),
    /// Append these rows to the existing ones.
    Append(Vec<ChunkEntry>),
}

/// Incremental segment state written at every sync while a segment is OPEN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SegmentProgress {
    pub byte_size: u64,
    pub crc32c: u32,
    pub first_capture_ns: Option<u64>,
    pub last_capture_ns: Option<u64>,
    pub record_count: u64,
    pub chunk_count: u64,
}

/// Session-level counters written together with a [`SegmentProgress`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionProgress {
    pub first_capture_ns: Option<u64>,
    pub last_capture_ns: Option<u64>,
    pub record_count: u64,
    pub dropped_records: u64,
}

/// Final state of a session (normal finish or recovery).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionEnd {
    pub status: SessionStatus,
    pub ended_unix_ns: Option<i64>,
    pub recovered_unix_ns: Option<i64>,
    pub first_capture_ns: Option<u64>,
    pub last_capture_ns: Option<u64>,
    pub record_count: u64,
    pub dropped_records: u64,
    pub integrity: IntegrityStatus,
    pub integrity_detail: Option<String>,
    pub checksum_crc32c: Option<u32>,
}

/// A session with its segments and their chunk rows, read in one transaction.
pub(crate) struct SessionSnapshot {
    pub session: SessionInfo,
    pub segments: Vec<(SegmentInfo, Vec<ChunkEntry>)>,
}

/// One connection to the index. Not `Sync`: each thread (store, writer, reader) opens its own.
pub(crate) struct Index {
    conn: Connection,
}

impl Index {
    /// Opens (creating if needed) the index at `path`, enables WAL and foreign keys, and creates or checks
    /// the schema.
    pub(crate) fn open(path: &Path) -> Result<Self, IndexError> {
        let conn = Connection::open(path).ctx("opening the index")?;
        conn.busy_timeout(BUSY_TIMEOUT)
            .ctx("setting the busy timeout")?;
        let mode: String = conn
            .pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))
            .ctx("enabling WAL")?;
        if !mode.eq_ignore_ascii_case("wal") {
            // Possible on file systems without shared memory support; the rollback journal is still safe.
            tracing::warn!(component = COMPONENT, journal_mode = %mode, "index is not in WAL mode");
        }
        // NORMAL in WAL mode: a power cut may roll back the last few index transactions, never corrupt the
        // database. Recorded data does not depend on it (segment files are fsynced separately and recovery
        // rebuilds the index from them).
        conn.pragma_update(None, "synchronous", "NORMAL")
            .ctx("setting synchronous")?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .ctx("enabling foreign keys")?;
        let mut index = Self { conn };
        index.ensure_schema(path)?;
        Ok(index)
    }

    fn ensure_schema(&mut self, path: &Path) -> Result<(), IndexError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .ctx("starting the schema transaction")?;
        let version: i32 = tx
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .ctx("reading user_version")?;
        let app_id: i32 = tx
            .query_row("PRAGMA application_id", [], |r| r.get(0))
            .ctx("reading application_id")?;
        match version {
            0 => {
                let tables: i64 = tx
                    .query_row(
                        "SELECT count(*) FROM sqlite_schema WHERE type = 'table'",
                        [],
                        |r| r.get(0),
                    )
                    .ctx("inspecting an unversioned database")?;
                if app_id != 0 && app_id != INDEX_APPLICATION_ID || tables != 0 {
                    return Err(IndexError::ForeignDatabase {
                        path: path.to_owned(),
                        found: app_id,
                    });
                }
                tx.execute_batch(SCHEMA_V1).ctx("creating the schema")?;
                tx.pragma_update(None, "application_id", INDEX_APPLICATION_ID)
                    .ctx("setting application_id")?;
                tx.pragma_update(None, "user_version", INDEX_SCHEMA_VERSION)
                    .ctx("setting user_version")?;
            }
            INDEX_SCHEMA_VERSION => {
                if app_id != INDEX_APPLICATION_ID {
                    return Err(IndexError::ForeignDatabase {
                        path: path.to_owned(),
                        found: app_id,
                    });
                }
            }
            found => {
                return Err(IndexError::UnsupportedSchema {
                    found,
                    supported: INDEX_SCHEMA_VERSION,
                });
            }
        }
        tx.commit().ctx("committing the schema")
    }

    // ----------------------------------------------------------------------------------------------
    // Sessions
    // ----------------------------------------------------------------------------------------------

    /// Inserts a session row as given (new ACTIVE sessions and sessions adopted from disk).
    pub(crate) fn insert_session(&mut self, s: &SessionInfo) -> Result<(), IndexError> {
        let m = &s.meta;
        self.conn
            .execute(
                "INSERT INTO sessions (id, dir, status, started_unix_ns, ended_unix_ns, recovered_unix_ns,
                    first_capture_ns, last_capture_ns, format_version, app_version, telemetry_schema_version,
                    protocol_version, lmu_version, interface_version, calculation_engine_version, vehicle, track,
                    session_type, data_availability_json, record_count, dropped_records, integrity_status,
                    integrity_detail, checksum_crc32c, retention_class)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                    ?21, ?22, ?23, ?24, ?25)",
                params![
                    s.id.to_string(),
                    s.dir,
                    s.status.as_str(),
                    s.started_unix_ns,
                    s.ended_unix_ns,
                    s.recovered_unix_ns,
                    opt_i64("first_capture_ns", s.first_capture_ns)?,
                    opt_i64("last_capture_ns", s.last_capture_ns)?,
                    s.format_version,
                    m.app_version,
                    m.telemetry_schema_version,
                    m.protocol_version,
                    m.lmu_version,
                    m.interface_version,
                    m.calculation_engine_version,
                    m.vehicle,
                    m.track,
                    m.session_type,
                    m.data_availability_json,
                    to_i64("record_count", s.record_count)?,
                    to_i64("dropped_records", s.dropped_records)?,
                    s.integrity.as_str(),
                    s.integrity_detail,
                    s.checksum_crc32c,
                    s.retention.as_str(),
                ],
            )
            .ctx("inserting a session")?;
        Ok(())
    }

    /// Applies a partial metadata update (`None` fields keep their stored value).
    pub(crate) fn update_session_meta(
        &mut self,
        id: SessionId,
        u: &SessionMetaUpdate,
    ) -> Result<(), IndexError> {
        self.conn
            .execute(
                "UPDATE sessions SET
                    protocol_version = COALESCE(?2, protocol_version),
                    lmu_version = COALESCE(?3, lmu_version),
                    interface_version = COALESCE(?4, interface_version),
                    calculation_engine_version = COALESCE(?5, calculation_engine_version),
                    vehicle = COALESCE(?6, vehicle),
                    track = COALESCE(?7, track),
                    session_type = COALESCE(?8, session_type),
                    data_availability_json = COALESCE(?9, data_availability_json)
                 WHERE id = ?1",
                params![
                    id.to_string(),
                    u.protocol_version,
                    u.lmu_version,
                    u.interface_version,
                    u.calculation_engine_version,
                    u.vehicle,
                    u.track,
                    u.session_type,
                    u.data_availability_json,
                ],
            )
            .ctx("updating session metadata")?;
        Ok(())
    }

    /// Writes the final state of a session (normal finish or recovery).
    pub(crate) fn end_session(&mut self, id: SessionId, e: &SessionEnd) -> Result<(), IndexError> {
        self.conn
            .execute(
                "UPDATE sessions SET status = ?2, ended_unix_ns = ?3, recovered_unix_ns = ?4, first_capture_ns = ?5,
                    last_capture_ns = ?6, record_count = ?7, dropped_records = ?8, integrity_status = ?9,
                    integrity_detail = ?10, checksum_crc32c = ?11
                 WHERE id = ?1",
                params![
                    id.to_string(),
                    e.status.as_str(),
                    e.ended_unix_ns,
                    e.recovered_unix_ns,
                    opt_i64("first_capture_ns", e.first_capture_ns)?,
                    opt_i64("last_capture_ns", e.last_capture_ns)?,
                    to_i64("record_count", e.record_count)?,
                    to_i64("dropped_records", e.dropped_records)?,
                    e.integrity.as_str(),
                    e.integrity_detail,
                    e.checksum_crc32c,
                ],
            )
            .ctx("ending a session")?;
        Ok(())
    }

    /// Sets the retention class of a session.
    pub(crate) fn set_retention(
        &mut self,
        id: SessionId,
        class: RetentionClass,
    ) -> Result<bool, IndexError> {
        let n = self
            .conn
            .execute(
                "UPDATE sessions SET retention_class = ?2 WHERE id = ?1",
                params![id.to_string(), class.as_str()],
            )
            .ctx("setting the retention class")?;
        Ok(n == 1)
    }

    /// Deletes a session and (by cascade) its segments, chunks, laps, stints and events.
    pub(crate) fn delete_session(&mut self, id: SessionId) -> Result<bool, IndexError> {
        let n = self
            .conn
            .execute(
                "DELETE FROM sessions WHERE id = ?1",
                params![id.to_string()],
            )
            .ctx("deleting a session")?;
        Ok(n == 1)
    }

    /// One session, if present.
    pub(crate) fn session(&self, id: SessionId) -> Result<Option<SessionInfo>, IndexError> {
        self.conn
            .query_row(
                &format!("{SESSION_SELECT} WHERE id = ?1"),
                params![id.to_string()],
                session_from_row,
            )
            .optional()
            .ctx("reading a session")
    }

    /// Every session, oldest first.
    pub(crate) fn sessions(&self) -> Result<Vec<SessionInfo>, IndexError> {
        let mut stmt = self
            .conn
            .prepare(&format!("{SESSION_SELECT} ORDER BY started_unix_ns, id"))
            .ctx("listing sessions")?;
        let rows = stmt
            .query_map([], session_from_row)
            .ctx("listing sessions")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .ctx("listing sessions")
    }

    /// Ids of the sessions with `status`, oldest first.
    pub(crate) fn session_ids_with_status(
        &self,
        status: SessionStatus,
    ) -> Result<Vec<SessionId>, IndexError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM sessions WHERE status = ?1 ORDER BY started_unix_ns, id")
            .ctx("listing sessions by status")?;
        let rows = stmt
            .query_map(params![status.as_str()], |r| session_id_at(r, 0))
            .ctx("listing sessions by status")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .ctx("listing sessions by status")
    }

    /// Whether SQLite's `json_valid` accepts `text` (the check the schema enforces on JSON columns).
    pub(crate) fn json_valid(&self, text: &str) -> Result<bool, IndexError> {
        self.conn
            .query_row("SELECT json_valid(?1)", params![text], |r| {
                r.get::<_, i64>(0)
            })
            .map(|v| v == 1)
            .ctx("validating JSON")
    }

    /// Whether a session row exists.
    pub(crate) fn session_exists(&self, id: SessionId) -> Result<bool, IndexError> {
        self.conn
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                params![id.to_string()],
                |_| Ok(()),
            )
            .optional()
            .map(|o| o.is_some())
            .ctx("checking a session")
    }

    // ----------------------------------------------------------------------------------------------
    // Segments and chunks
    // ----------------------------------------------------------------------------------------------

    /// Inserts or fully replaces a segment row and applies `chunks` to its chunk rows, in one transaction so
    /// the chunk rows and `byte_size` always agree.
    pub(crate) fn put_segment(
        &mut self,
        s: &SegmentInfo,
        chunks: ChunkRows,
    ) -> Result<(), IndexError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .ctx("starting put_segment")?;
        tx.execute(
            "INSERT INTO segments (session_id, segment_index, path, status, created_unix_ns, created_mono_ns,
                closed_unix_ns, byte_size, file_len, first_capture_ns, last_capture_ns, record_count, chunk_count,
                file_crc32c, integrity_status, integrity_detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT (session_id, segment_index) DO UPDATE SET
                path = excluded.path, status = excluded.status, created_unix_ns = excluded.created_unix_ns,
                created_mono_ns = excluded.created_mono_ns, closed_unix_ns = excluded.closed_unix_ns,
                byte_size = excluded.byte_size, file_len = excluded.file_len,
                first_capture_ns = excluded.first_capture_ns, last_capture_ns = excluded.last_capture_ns,
                record_count = excluded.record_count, chunk_count = excluded.chunk_count,
                file_crc32c = excluded.file_crc32c, integrity_status = excluded.integrity_status,
                integrity_detail = excluded.integrity_detail",
            params![
                s.session_id.to_string(),
                s.segment_index,
                s.path,
                s.status.as_str(),
                s.created_unix_ns,
                opt_i64("created_mono_ns", s.created_mono_ns)?,
                s.closed_unix_ns,
                to_i64("byte_size", s.byte_size)?,
                opt_i64("file_len", s.file_len)?,
                opt_i64("first_capture_ns", s.first_capture_ns)?,
                opt_i64("last_capture_ns", s.last_capture_ns)?,
                to_i64("record_count", s.record_count)?,
                to_i64("chunk_count", s.chunk_count)?,
                s.file_crc32c,
                s.integrity.as_str(),
                s.integrity_detail,
            ],
        )
        .ctx("writing a segment")?;
        match &chunks {
            ChunkRows::Keep => {}
            ChunkRows::Replace(rows) => {
                tx.execute(
                    "DELETE FROM chunks WHERE session_id = ?1 AND segment_index = ?2",
                    params![s.session_id.to_string(), s.segment_index],
                )
                .ctx("replacing chunk rows")?;
                insert_chunks(&tx, s.session_id, s.segment_index, rows)?;
            }
            ChunkRows::Append(rows) => insert_chunks(&tx, s.session_id, s.segment_index, rows)?,
        }
        tx.commit().ctx("committing put_segment")
    }

    /// Records sync progress of an OPEN segment: appends `new_chunks` to its chunk rows and updates the
    /// segment and session counters, atomically.
    pub(crate) fn record_progress(
        &mut self,
        id: SessionId,
        segment_index: u32,
        seg: &SegmentProgress,
        session: &SessionProgress,
        new_chunks: &[ChunkEntry],
    ) -> Result<(), IndexError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .ctx("starting record_progress")?;
        insert_chunks(&tx, id, segment_index, new_chunks)?;
        tx.execute(
            "UPDATE segments SET byte_size = ?3, file_crc32c = ?4, first_capture_ns = ?5, last_capture_ns = ?6,
                record_count = ?7, chunk_count = ?8
             WHERE session_id = ?1 AND segment_index = ?2",
            params![
                id.to_string(),
                segment_index,
                to_i64("byte_size", seg.byte_size)?,
                seg.crc32c,
                opt_i64("first_capture_ns", seg.first_capture_ns)?,
                opt_i64("last_capture_ns", seg.last_capture_ns)?,
                to_i64("record_count", seg.record_count)?,
                to_i64("chunk_count", seg.chunk_count)?,
            ],
        )
        .ctx("updating segment progress")?;
        tx.execute(
            "UPDATE sessions SET first_capture_ns = ?2, last_capture_ns = ?3, record_count = ?4, dropped_records = ?5
             WHERE id = ?1",
            params![
                id.to_string(),
                opt_i64("first_capture_ns", session.first_capture_ns)?,
                opt_i64("last_capture_ns", session.last_capture_ns)?,
                to_i64("record_count", session.record_count)?,
                to_i64("dropped_records", session.dropped_records)?,
            ],
        )
        .ctx("updating session progress")?;
        tx.commit().ctx("committing record_progress")
    }

    /// Segments of a session in segment order.
    pub(crate) fn segments(&self, id: SessionId) -> Result<Vec<SegmentInfo>, IndexError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT session_id, segment_index, path, status, created_unix_ns, created_mono_ns, closed_unix_ns,
                    byte_size, file_len, first_capture_ns, last_capture_ns, record_count, chunk_count, file_crc32c,
                    integrity_status, integrity_detail
                 FROM segments WHERE session_id = ?1 ORDER BY segment_index",
            )
            .ctx("listing segments")?;
        let rows = stmt
            .query_map(params![id.to_string()], |r| {
                Ok(SegmentInfo {
                    session_id: session_id_at(r, 0)?,
                    segment_index: int_at(r, 1)?,
                    path: r.get(2)?,
                    status: text_at(r, 3, SegmentStatus::parse)?,
                    created_unix_ns: r.get(4)?,
                    created_mono_ns: opt_int_at(r, 5)?,
                    closed_unix_ns: r.get(6)?,
                    byte_size: int_at(r, 7)?,
                    file_len: opt_int_at(r, 8)?,
                    first_capture_ns: opt_int_at(r, 9)?,
                    last_capture_ns: opt_int_at(r, 10)?,
                    record_count: int_at(r, 11)?,
                    chunk_count: int_at(r, 12)?,
                    file_crc32c: opt_int_at(r, 13)?,
                    integrity: text_at(r, 14, IntegrityStatus::parse)?,
                    integrity_detail: r.get(15)?,
                })
            })
            .ctx("listing segments")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .ctx("listing segments")
    }

    /// The session, its segments and their chunk rows as one consistent snapshot (a read transaction), even
    /// while a writer is updating them through another connection.
    pub(crate) fn session_snapshot(
        &self,
        id: SessionId,
    ) -> Result<Option<SessionSnapshot>, IndexError> {
        let tx = self
            .conn
            .unchecked_transaction()
            .ctx("starting a read transaction")?;
        let Some(session) = self.session(id)? else {
            return Ok(None);
        };
        let mut segments = Vec::new();
        for seg in self.segments(id)? {
            let chunks = self.chunks(id, seg.segment_index)?;
            segments.push((seg, chunks));
        }
        tx.commit().ctx("ending a read transaction")?;
        Ok(Some(SessionSnapshot { session, segments }))
    }

    /// Chunk rows of one segment in sequence order (a contiguous prefix of the segment's chunks).
    pub(crate) fn chunks(
        &self,
        id: SessionId,
        segment_index: u32,
    ) -> Result<Vec<ChunkEntry>, IndexError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT chunk_seq, byte_offset, body_len, record_count, min_capture_ns, max_capture_ns
                 FROM chunks WHERE session_id = ?1 AND segment_index = ?2 ORDER BY chunk_seq",
            )
            .ctx("listing chunks")?;
        let rows = stmt
            .query_map(params![id.to_string(), segment_index], |r| {
                Ok(ChunkEntry {
                    chunk_seq: int_at(r, 0)?,
                    offset: int_at(r, 1)?,
                    body_len: int_at(r, 2)?,
                    record_count: int_at(r, 3)?,
                    min_capture_ns: int_at(r, 4)?,
                    max_capture_ns: int_at(r, 5)?,
                })
            })
            .ctx("listing chunks")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .ctx("listing chunks")
    }

    // ----------------------------------------------------------------------------------------------
    // Laps, stints, events
    // ----------------------------------------------------------------------------------------------

    /// Inserts or replaces lap `lap.lap_number` (laps are updated as sector times arrive).
    pub(crate) fn put_lap(&mut self, id: SessionId, lap: &LapRecord) -> Result<(), IndexError> {
        self.conn
            .execute(
                "INSERT INTO laps (session_id, lap_number, stint_number, start_capture_ns, end_capture_ns, lap_time_s,
                    sector1_s, sector2_s, sector3_s, valid, pit_lap, metrics_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT (session_id, lap_number) DO UPDATE SET
                    stint_number = excluded.stint_number, start_capture_ns = excluded.start_capture_ns,
                    end_capture_ns = excluded.end_capture_ns, lap_time_s = excluded.lap_time_s,
                    sector1_s = excluded.sector1_s, sector2_s = excluded.sector2_s, sector3_s = excluded.sector3_s,
                    valid = excluded.valid, pit_lap = excluded.pit_lap, metrics_json = excluded.metrics_json",
                params![
                    id.to_string(),
                    lap.lap_number,
                    lap.stint_number,
                    to_i64("start_capture_ns", lap.start_capture_ns)?,
                    opt_i64("end_capture_ns", lap.end_capture_ns)?,
                    finite(lap.lap_time_s),
                    finite(lap.sector_times_s[0]),
                    finite(lap.sector_times_s[1]),
                    finite(lap.sector_times_s[2]),
                    lap.valid,
                    lap.pit_lap,
                    lap.metrics_json,
                ],
            )
            .ctx("writing a lap")?;
        Ok(())
    }

    /// Inserts or replaces stint `stint.stint_number`.
    pub(crate) fn put_stint(
        &mut self,
        id: SessionId,
        stint: &StintRecord,
    ) -> Result<(), IndexError> {
        self.conn
            .execute(
                "INSERT INTO stints (session_id, stint_number, start_capture_ns, end_capture_ns, start_lap, end_lap,
                    driver, metrics_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (session_id, stint_number) DO UPDATE SET
                    start_capture_ns = excluded.start_capture_ns, end_capture_ns = excluded.end_capture_ns,
                    start_lap = excluded.start_lap, end_lap = excluded.end_lap, driver = excluded.driver,
                    metrics_json = excluded.metrics_json",
                params![
                    id.to_string(),
                    stint.stint_number,
                    to_i64("start_capture_ns", stint.start_capture_ns)?,
                    opt_i64("end_capture_ns", stint.end_capture_ns)?,
                    stint.start_lap,
                    stint.end_lap,
                    stint.driver,
                    stint.metrics_json,
                ],
            )
            .ctx("writing a stint")?;
        Ok(())
    }

    /// Appends an event; returns its row id.
    pub(crate) fn insert_event(
        &mut self,
        id: SessionId,
        e: &EventRecord,
    ) -> Result<i64, IndexError> {
        self.conn
            .execute(
                "INSERT INTO events (session_id, capture_ns, unix_ns, lap, category, severity, description, channel_id,
                    value, threshold)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    id.to_string(),
                    opt_i64("capture_ns", e.capture_ns)?,
                    e.unix_ns,
                    e.lap,
                    e.category.as_str(),
                    e.severity.as_str(),
                    e.description,
                    e.channel_id,
                    finite(e.value),
                    finite(e.threshold),
                ],
            )
            .ctx("inserting an event")?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Laps of a session in lap order.
    pub(crate) fn laps(&self, id: SessionId) -> Result<Vec<LapRecord>, IndexError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT lap_number, stint_number, start_capture_ns, end_capture_ns, lap_time_s, sector1_s, sector2_s,
                    sector3_s, valid, pit_lap, metrics_json
                 FROM laps WHERE session_id = ?1 ORDER BY lap_number",
            )
            .ctx("listing laps")?;
        let rows = stmt
            .query_map(params![id.to_string()], |r| {
                Ok(LapRecord {
                    lap_number: int_at(r, 0)?,
                    stint_number: opt_int_at(r, 1)?,
                    start_capture_ns: int_at(r, 2)?,
                    end_capture_ns: opt_int_at(r, 3)?,
                    lap_time_s: r.get(4)?,
                    sector_times_s: [r.get(5)?, r.get(6)?, r.get(7)?],
                    valid: r.get(8)?,
                    pit_lap: r.get(9)?,
                    metrics_json: r.get(10)?,
                })
            })
            .ctx("listing laps")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .ctx("listing laps")
    }

    /// Stints of a session in stint order.
    pub(crate) fn stints(&self, id: SessionId) -> Result<Vec<StintRecord>, IndexError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT stint_number, start_capture_ns, end_capture_ns, start_lap, end_lap, driver, metrics_json
                 FROM stints WHERE session_id = ?1 ORDER BY stint_number",
            )
            .ctx("listing stints")?;
        let rows = stmt
            .query_map(params![id.to_string()], |r| {
                Ok(StintRecord {
                    stint_number: int_at(r, 0)?,
                    start_capture_ns: int_at(r, 1)?,
                    end_capture_ns: opt_int_at(r, 2)?,
                    start_lap: opt_int_at(r, 3)?,
                    end_lap: opt_int_at(r, 4)?,
                    driver: r.get(5)?,
                    metrics_json: r.get(6)?,
                })
            })
            .ctx("listing stints")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .ctx("listing stints")
    }

    /// Events of a session matching `q`, in capture-time order (events without a capture time first, in
    /// insertion order).
    pub(crate) fn events(
        &self,
        id: SessionId,
        q: &EventQuery,
    ) -> Result<Vec<EventRecord>, IndexError> {
        // Stored capture times are ≤ i64::MAX, so clamping the bounds keeps the filter exact.
        let start = q
            .range
            .map(|r| i64::try_from(r.start_ns).unwrap_or(i64::MAX));
        let end = q.range.map(|r| i64::try_from(r.end_ns).unwrap_or(i64::MAX));
        let limit = q.limit.map_or(-1, i64::from);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT capture_ns, unix_ns, lap, category, severity, description, channel_id, value, threshold
                 FROM events
                 WHERE session_id = ?1
                   AND (?2 IS NULL OR capture_ns >= ?2) AND (?3 IS NULL OR capture_ns <= ?3)
                   AND (?4 IS NULL OR lap = ?4) AND (?5 IS NULL OR category = ?5)
                 ORDER BY capture_ns IS NOT NULL, capture_ns, id
                 LIMIT ?6",
            )
            .ctx("listing events")?;
        let category = q.category.as_ref().map(EventCategory::as_str);
        let rows = stmt
            .query_map(
                params![id.to_string(), start, end, q.lap, category, limit],
                |r| {
                    let category: String = r.get(3)?;
                    Ok(EventRecord {
                        capture_ns: opt_int_at(r, 0)?,
                        unix_ns: r.get(1)?,
                        lap: opt_int_at(r, 2)?,
                        category: EventCategory::parse(&category),
                        severity: text_at(r, 4, Severity::parse)?,
                        description: r.get(5)?,
                        channel_id: opt_int_at(r, 6)?,
                        value: r.get(7)?,
                        threshold: r.get(8)?,
                    })
                },
            )
            .ctx("listing events")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .ctx("listing events")
    }
}

const SESSION_SELECT: &str = "SELECT id, dir, status, started_unix_ns, ended_unix_ns, recovered_unix_ns,
    first_capture_ns, last_capture_ns, format_version, app_version, telemetry_schema_version, protocol_version,
    lmu_version, interface_version, calculation_engine_version, vehicle, track, session_type, data_availability_json,
    record_count, dropped_records, integrity_status, integrity_detail, checksum_crc32c, retention_class
    FROM sessions";

fn session_from_row(r: &Row<'_>) -> rusqlite::Result<SessionInfo> {
    Ok(SessionInfo {
        id: session_id_at(r, 0)?,
        dir: r.get(1)?,
        status: text_at(r, 2, SessionStatus::parse)?,
        started_unix_ns: r.get(3)?,
        ended_unix_ns: r.get(4)?,
        recovered_unix_ns: r.get(5)?,
        first_capture_ns: opt_int_at(r, 6)?,
        last_capture_ns: opt_int_at(r, 7)?,
        format_version: int_at(r, 8)?,
        meta: SessionMeta {
            app_version: r.get(9)?,
            telemetry_schema_version: int_at(r, 10)?,
            protocol_version: opt_int_at(r, 11)?,
            lmu_version: r.get(12)?,
            interface_version: r.get(13)?,
            calculation_engine_version: r.get(14)?,
            vehicle: r.get(15)?,
            track: r.get(16)?,
            session_type: r.get(17)?,
            data_availability_json: r.get(18)?,
        },
        record_count: int_at(r, 19)?,
        dropped_records: int_at(r, 20)?,
        integrity: text_at(r, 21, IntegrityStatus::parse)?,
        integrity_detail: r.get(22)?,
        checksum_crc32c: opt_int_at(r, 23)?,
        retention: text_at(r, 24, RetentionClass::parse)?,
    })
}

fn insert_chunks(
    tx: &rusqlite::Transaction<'_>,
    id: SessionId,
    segment_index: u32,
    chunks: &[ChunkEntry],
) -> Result<(), IndexError> {
    if chunks.is_empty() {
        return Ok(());
    }
    let mut stmt = tx
        .prepare_cached(
            "INSERT INTO chunks (session_id, segment_index, chunk_seq, byte_offset, body_len, record_count,
                min_capture_ns, max_capture_ns)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .ctx("preparing chunk insert")?;
    let sid = id.to_string();
    for c in chunks {
        stmt.execute(params![
            sid,
            segment_index,
            c.chunk_seq,
            to_i64("chunk byte_offset", c.offset)?,
            c.body_len,
            c.record_count,
            to_i64("min_capture_ns", c.min_capture_ns)?,
            to_i64("max_capture_ns", c.max_capture_ns)?,
        ])
        .ctx("inserting a chunk row")?;
    }
    Ok(())
}

/// SQLite stores NaN as NULL; map non-finite values to NULL explicitly so the behaviour is documented.
fn finite(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite())
}

fn to_i64(field: &'static str, value: u64) -> Result<i64, IndexError> {
    i64::try_from(value).map_err(|_| IndexError::OutOfRange { field, value })
}

fn opt_i64(field: &'static str, value: Option<u64>) -> Result<Option<i64>, IndexError> {
    value.map(|v| to_i64(field, v)).transpose()
}

fn conversion_error(idx: usize, ty: Type, detail: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, ty, detail.into())
}

/// Reads an INTEGER column into any integer type, failing (not truncating) when it does not fit.
fn int_at<T: TryFrom<i64>>(r: &Row<'_>, idx: usize) -> rusqlite::Result<T> {
    let v: i64 = r.get(idx)?;
    T::try_from(v)
        .map_err(|_| conversion_error(idx, Type::Integer, format!("value {v} out of range")))
}

fn opt_int_at<T: TryFrom<i64>>(r: &Row<'_>, idx: usize) -> rusqlite::Result<Option<T>> {
    let v: Option<i64> = r.get(idx)?;
    v.map(|v| {
        T::try_from(v)
            .map_err(|_| conversion_error(idx, Type::Integer, format!("value {v} out of range")))
    })
    .transpose()
}

fn text_at<T>(r: &Row<'_>, idx: usize, parse: fn(&str) -> Option<T>) -> rusqlite::Result<T> {
    let s: String = r.get(idx)?;
    parse(&s).ok_or_else(|| conversion_error(idx, Type::Text, format!("unknown code {s:?}")))
}

fn session_id_at(r: &Row<'_>, idx: usize) -> rusqlite::Result<SessionId> {
    let s: String = r.get(idx)?;
    s.parse()
        .map_err(|e| conversion_error(idx, Type::Text, format!("invalid session id {s:?}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn meta() -> SessionMeta {
        SessionMeta {
            app_version: "0.1.0".into(),
            telemetry_schema_version: 1,
            ..Default::default()
        }
    }

    fn session(id: SessionId, started: i64) -> SessionInfo {
        SessionInfo {
            id,
            dir: format!("sessions/{id}"),
            status: SessionStatus::Active,
            started_unix_ns: started,
            ended_unix_ns: None,
            recovered_unix_ns: None,
            first_capture_ns: None,
            last_capture_ns: None,
            format_version: 1,
            meta: meta(),
            record_count: 0,
            dropped_records: 0,
            integrity: IntegrityStatus::Unverified,
            integrity_detail: None,
            checksum_crc32c: None,
            retention: RetentionClass::Default,
        }
    }

    fn segment(id: SessionId, index: u32) -> SegmentInfo {
        SegmentInfo {
            session_id: id,
            segment_index: index,
            path: format!("sessions/{id}/seg-{index:06}.lmurec"),
            status: SegmentStatus::Open,
            created_unix_ns: 5,
            created_mono_ns: None,
            closed_unix_ns: None,
            byte_size: 96,
            file_len: None,
            first_capture_ns: None,
            last_capture_ns: None,
            record_count: 0,
            chunk_count: 0,
            file_crc32c: None,
            integrity: IntegrityStatus::Unverified,
            integrity_detail: None,
        }
    }

    fn chunk(seq: u32, offset: u64) -> ChunkEntry {
        ChunkEntry {
            chunk_seq: seq,
            offset,
            body_len: 10,
            record_count: 2,
            min_capture_ns: u64::from(seq) * 100,
            max_capture_ns: u64::from(seq) * 100 + 50,
        }
    }

    #[test]
    fn schema_is_created_once_and_reopened() {
        let dir = TempDir::new("index-schema");
        let path = dir.path().join("index.sqlite3");
        let id = SessionId::new_random();
        {
            let mut idx = Index::open(&path).expect("create");
            idx.insert_session(&session(id, 1)).expect("insert");
        }
        let idx = Index::open(&path).expect("reopen");
        assert_eq!(idx.session(id).expect("read").expect("present").id, id);
        let version: i32 = idx
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("version");
        assert_eq!(version, INDEX_SCHEMA_VERSION);
        let second = Index::open(&path).expect("concurrent second connection");
        assert!(second.session_exists(id).expect("exists"));
    }

    #[test]
    fn foreign_and_newer_databases_are_refused() {
        let dir = TempDir::new("index-foreign");
        let foreign = dir.path().join("foreign.sqlite3");
        {
            let c = Connection::open(&foreign).expect("open");
            c.execute_batch("CREATE TABLE something (x INTEGER);")
                .expect("create");
        }
        assert!(matches!(
            Index::open(&foreign),
            Err(IndexError::ForeignDatabase { .. })
        ));
        let newer = dir.path().join("newer.sqlite3");
        {
            Index::open(&newer).expect("create");
            let c = Connection::open(&newer).expect("open");
            c.pragma_update(None, "user_version", INDEX_SCHEMA_VERSION + 1)
                .expect("bump");
        }
        assert!(matches!(
            Index::open(&newer),
            Err(IndexError::UnsupportedSchema { .. })
        ));
        let garbage = dir.path().join("garbage.sqlite3");
        std::fs::write(&garbage, vec![0x5au8; 8192]).expect("write");
        let err = Index::open(&garbage).err().expect("garbage must fail");
        assert!(err.is_corruption(), "{err}");
    }

    #[test]
    fn session_lifecycle_rows() {
        let dir = TempDir::new("index-session");
        let mut idx = Index::open(&dir.path().join("i.sqlite3")).expect("open");
        let a = SessionId::new_random();
        let b = SessionId::new_random();
        idx.insert_session(&session(b, 20)).expect("insert b");
        idx.insert_session(&session(a, 10)).expect("insert a");
        assert_eq!(
            idx.sessions()
                .expect("list")
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec![a, b]
        );
        idx.update_session_meta(
            a,
            &SessionMetaUpdate {
                track: Some("Spa".into()),
                data_availability_json: Some(r#"{"tyres":true}"#.into()),
                ..Default::default()
            },
        )
        .expect("meta");
        idx.update_session_meta(
            a,
            &SessionMetaUpdate {
                vehicle: Some("GT3".into()),
                ..Default::default()
            },
        )
        .expect("meta 2");
        let bad_json = SessionMetaUpdate {
            data_availability_json: Some("{not json".into()),
            ..Default::default()
        };
        assert!(
            idx.update_session_meta(a, &bad_json).is_err(),
            "invalid JSON is rejected by the schema"
        );
        let end = SessionEnd {
            status: SessionStatus::Complete,
            ended_unix_ns: Some(99),
            recovered_unix_ns: None,
            first_capture_ns: Some(1),
            last_capture_ns: Some(i64::MAX as u64),
            record_count: 42,
            dropped_records: 3,
            integrity: IntegrityStatus::Ok,
            integrity_detail: None,
            checksum_crc32c: Some(u32::MAX),
        };
        idx.end_session(a, &end).expect("end");
        let s = idx.session(a).expect("read").expect("present");
        assert_eq!(s.meta.track.as_deref(), Some("Spa"));
        assert_eq!(s.meta.vehicle.as_deref(), Some("GT3"));
        assert_eq!(
            s.meta.data_availability_json.as_deref(),
            Some(r#"{"tyres":true}"#)
        );
        assert_eq!(
            (s.status, s.record_count, s.dropped_records),
            (SessionStatus::Complete, 42, 3)
        );
        assert_eq!(s.last_capture_ns, Some(i64::MAX as u64));
        assert_eq!(s.checksum_crc32c, Some(u32::MAX));
        assert_eq!(
            idx.session_ids_with_status(SessionStatus::Active)
                .expect("active"),
            vec![b]
        );
        assert!(idx.set_retention(b, RetentionClass::Keep).expect("keep"));
        assert_eq!(
            idx.session(b).expect("read").expect("present").retention,
            RetentionClass::Keep
        );
        let mut too_big = end.clone();
        too_big.last_capture_ns = Some(u64::MAX);
        assert!(matches!(
            idx.end_session(a, &too_big),
            Err(IndexError::OutOfRange { .. })
        ));
    }

    #[test]
    fn segments_chunks_progress_and_replacement() {
        let dir = TempDir::new("index-segments");
        let mut idx = Index::open(&dir.path().join("i.sqlite3")).expect("open");
        let id = SessionId::new_random();
        idx.insert_session(&session(id, 1)).expect("session");
        idx.put_segment(&segment(id, 0), ChunkRows::Keep)
            .expect("segment");
        let progress = SegmentProgress {
            byte_size: 96 + 58 * 2,
            crc32c: 7,
            first_capture_ns: Some(0),
            last_capture_ns: Some(150),
            record_count: 4,
            chunk_count: 2,
        };
        let sp = SessionProgress {
            first_capture_ns: Some(0),
            last_capture_ns: Some(150),
            record_count: 4,
            dropped_records: 1,
        };
        idx.record_progress(id, 0, &progress, &sp, &[chunk(0, 96), chunk(1, 154)])
            .expect("progress");
        idx.record_progress(id, 0, &progress, &sp, &[])
            .expect("no new chunks");
        assert_eq!(
            idx.chunks(id, 0).expect("chunks"),
            vec![chunk(0, 96), chunk(1, 154)]
        );
        let seg = &idx.segments(id).expect("segments")[0];
        assert_eq!(
            (seg.byte_size, seg.record_count, seg.file_crc32c),
            (212, 4, Some(7))
        );
        let sess = idx.session(id).expect("read").expect("present");
        assert_eq!(
            (
                sess.record_count,
                sess.dropped_records,
                sess.last_capture_ns
            ),
            (4, 1, Some(150))
        );
        // A duplicate chunk row is a constraint violation, and the failed transaction changes nothing.
        assert!(
            idx.record_progress(id, 0, &progress, &sp, &[chunk(2, 212), chunk(1, 154)])
                .is_err()
        );
        assert_eq!(idx.chunks(id, 0).expect("chunks").len(), 2);
        // Closing replaces the chunk rows with the validated list; appending extends them.
        let mut closed = segment(id, 0);
        closed.status = SegmentStatus::Closed;
        closed.integrity = IntegrityStatus::Ok;
        idx.put_segment(&closed, ChunkRows::Replace(vec![chunk(0, 96)]))
            .expect("close");
        assert_eq!(idx.chunks(id, 0).expect("chunks"), vec![chunk(0, 96)]);
        assert_eq!(
            idx.segments(id).expect("segments")[0].status,
            SegmentStatus::Closed
        );
        idx.put_segment(&closed, ChunkRows::Append(vec![chunk(1, 154)]))
            .expect("append");
        assert_eq!(
            idx.chunks(id, 0).expect("chunks"),
            vec![chunk(0, 96), chunk(1, 154)]
        );
        idx.put_segment(&closed, ChunkRows::Keep).expect("keep");
        assert_eq!(idx.chunks(id, 0).expect("chunks").len(), 2);
        // Chunks of a segment that is not in the index are refused (foreign key).
        assert!(
            idx.record_progress(id, 9, &progress, &sp, &[chunk(0, 96)])
                .is_err()
        );
    }

    #[test]
    fn laps_stints_events_and_cascade_delete() {
        let dir = TempDir::new("index-laps");
        let mut idx = Index::open(&dir.path().join("i.sqlite3")).expect("open");
        let id = SessionId::new_random();
        idx.insert_session(&session(id, 1)).expect("session");
        idx.put_segment(&segment(id, 0), ChunkRows::Replace(vec![chunk(0, 96)]))
            .expect("segment");
        let mut lap = LapRecord {
            lap_number: 1,
            start_capture_ns: 100,
            ..Default::default()
        };
        idx.put_lap(id, &lap).expect("lap start");
        lap.end_capture_ns = Some(200);
        lap.lap_time_s = Some(93.25);
        lap.sector_times_s = [Some(30.0), Some(f64::NAN), None];
        lap.valid = Some(true);
        idx.put_lap(id, &lap).expect("lap update");
        let laps = idx.laps(id).expect("laps");
        assert_eq!(laps.len(), 1);
        assert_eq!(laps[0].lap_time_s, Some(93.25));
        assert_eq!(
            laps[0].sector_times_s,
            [Some(30.0), None, None],
            "NaN is stored as unknown"
        );
        assert_eq!(laps[0].valid, Some(true));
        assert_eq!(laps[0].pit_lap, None);
        let stint = StintRecord {
            stint_number: 1,
            start_capture_ns: 100,
            driver: Some("A".into()),
            ..Default::default()
        };
        idx.put_stint(id, &stint).expect("stint");
        assert_eq!(idx.stints(id).expect("stints"), vec![stint]);

        let mut alert = EventRecord::new(
            150,
            EventCategory::Alert,
            Severity::Warning,
            "tyre temp high",
        );
        alert.lap = Some(1);
        alert.channel_id = Some(0x0301);
        alert.value = Some(112.5);
        alert.threshold = Some(110.0);
        idx.insert_event(id, &alert).expect("alert");
        idx.insert_event(
            id,
            &EventRecord::new(50, EventCategory::Pit, Severity::Info, "pit in"),
        )
        .expect("pit");
        let mut no_time = EventRecord::new(
            0,
            EventCategory::Recorder,
            Severity::Critical,
            "write failed",
        );
        no_time.capture_ns = None;
        no_time.unix_ns = Some(123);
        idx.insert_event(id, &no_time).expect("recorder");
        let all = idx.events(id, &EventQuery::default()).expect("all");
        assert_eq!(
            all.iter().map(|e| e.capture_ns).collect::<Vec<_>>(),
            vec![None, Some(50), Some(150)]
        );
        assert_eq!(all[2], alert);
        let ranged = EventQuery {
            range: TimeRange::new(100, u64::MAX),
            ..Default::default()
        };
        assert_eq!(
            idx.events(id, &ranged).expect("ranged"),
            vec![alert.clone()]
        );
        let by_lap = EventQuery {
            lap: Some(1),
            ..Default::default()
        };
        assert_eq!(idx.events(id, &by_lap).expect("lap").len(), 1);
        let by_cat = EventQuery {
            category: Some(EventCategory::Pit),
            limit: Some(5),
            ..Default::default()
        };
        assert_eq!(
            idx.events(id, &by_cat).expect("cat")[0].description,
            "pit in"
        );
        let limited = EventQuery {
            limit: Some(1),
            ..Default::default()
        };
        assert_eq!(idx.events(id, &limited).expect("limit").len(), 1);

        assert!(idx.delete_session(id).expect("delete"));
        assert!(idx.session(id).expect("read").is_none());
        assert!(idx.segments(id).expect("segments").is_empty());
        assert!(idx.chunks(id, 0).expect("chunks").is_empty());
        assert!(idx.laps(id).expect("laps").is_empty());
        assert!(
            idx.events(id, &EventQuery::default())
                .expect("events")
                .is_empty()
        );
        assert!(!idx.delete_session(id).expect("second delete is a no-op"));
    }
}
