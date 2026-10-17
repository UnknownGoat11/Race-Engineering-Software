//! Segment integrity assessment, startup recovery and orphan adoption.
//!
//! * [`assess_segment`] fully validates one segment file (never modifying it) and classifies the result.
//!   The writer uses it when it closes or abandons a segment; recovery and on-demand validation use it for
//!   every segment of a session.
//! * Startup recovery: every session still ACTIVE when a [`crate::RecordingStore`] opens was being recorded
//!   when the application stopped. Each of its segments (index rows plus any `seg-*.lmurec` file found in
//!   the session directory) is rescanned; the index gets the valid length, the chunk offsets of the valid
//!   prefix and an integrity status, and the session becomes INTERRUPTED with its last valid capture time.
//!   Files are left exactly as they are: nothing is truncated or rewritten.
//! * Orphan adoption: a session directory whose id is not in the index (index lost, or a crash before the
//!   index transaction committed) is added from its segment headers, then recovered the same way.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use crate::COMPONENT;
use crate::config::{
    RecorderConfig, SESSIONS_DIR_NAME, parse_segment_file_name, parse_session_dir_name,
    segment_file_name,
};
use crate::error::RecorderError;
use crate::format::{FILE_HEADER_LEN, FILE_MAGIC, FileHeader, FormatError};
use crate::index::{ChunkEntry, ChunkRows, Index, SegmentInfo, SessionEnd, SessionInfo};
use crate::scan::{ScanError, ScanStop, SegmentScan, read_full, validate_segment_file};
use crate::types::{
    EventCategory, EventRecord, IntegrityStatus, RetentionClass, SegmentStatus, SessionId,
    SessionMeta, SessionStatus, Severity,
};

/// What a full validation of one segment file found.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// The file header is valid and belongs to the expected session/segment; chunks were validated.
    Scanned(Box<SegmentScan>),
    /// The file does not exist.
    Missing,
    /// The file exists but holds no usable data: it ends inside its header, or the header is invalid or
    /// belongs to another session/segment.
    Unusable,
    /// The file could not be read (I/O error); nothing is known about its content.
    Unreadable,
}

/// Classified result of [`assess_segment`].
#[derive(Debug)]
pub(crate) struct Assessment {
    pub(crate) outcome: Outcome,
    pub(crate) integrity: IntegrityStatus,
    pub(crate) detail: Option<String>,
    /// Physical file length, when the file exists.
    pub(crate) file_len: Option<u64>,
}

impl Assessment {
    pub(crate) fn scan(&self) -> Option<&SegmentScan> {
        match &self.outcome {
            Outcome::Scanned(scan) => Some(scan),
            _ => None,
        }
    }
}

/// Fully validates segment `segment_index` of `session` at `path`. Never modifies the file, never panics.
pub(crate) fn assess_segment(path: &Path, session: SessionId, segment_index: u32) -> Assessment {
    let file_len = fs::metadata(path).ok().map(|m| m.len());
    let (outcome, integrity, detail) = match validate_segment_file(path) {
        Err(ScanError::Io(e)) if e.kind() == ErrorKind::NotFound => (
            Outcome::Missing,
            IntegrityStatus::Damaged,
            Some("segment file is missing".to_owned()),
        ),
        Err(ScanError::Io(e)) => (
            Outcome::Unreadable,
            IntegrityStatus::Unverified,
            Some(format!("segment file could not be read: {e}")),
        ),
        Err(ScanError::Decompressor(e)) => (
            Outcome::Unreadable,
            IntegrityStatus::Unverified,
            Some(format!(
                "segment could not be validated (zstd context): {e}"
            )),
        ),
        Err(ScanError::Header(FormatError::HeaderTruncated { available })) => (
            Outcome::Unusable,
            IntegrityStatus::Truncated,
            Some(format!(
                "file ends inside its header ({available} of {FILE_HEADER_LEN} bytes): created but never written"
            )),
        ),
        Err(ScanError::Header(e)) if header_never_persisted(path) => (
            Outcome::Unusable,
            IntegrityStatus::Truncated,
            Some(format!(
                "file holds only an unpersisted {FILE_HEADER_LEN}-byte header ({e}): created but never written"
            )),
        ),
        Err(ScanError::Header(e)) => (
            Outcome::Unusable,
            IntegrityStatus::Damaged,
            Some(format!("segment file header is invalid: {e}")),
        ),
        Ok(scan)
            if scan.header.session_id != session.0
                || scan.header.segment_index != segment_index =>
        {
            (
                Outcome::Unusable,
                IntegrityStatus::Damaged,
                Some(format!(
                    "file header belongs to session {} segment {}, expected session {session} segment {segment_index}",
                    scan.header.session_id, scan.header.segment_index
                )),
            )
        }
        Ok(scan) => {
            let (integrity, detail) = match &scan.stop {
                ScanStop::CleanEof => (IntegrityStatus::Ok, None),
                stop @ ScanStop::TornTail { .. } => {
                    (IntegrityStatus::Truncated, Some(stop.describe()))
                }
                stop @ ScanStop::Corrupt { offset, .. } => {
                    let detail = match &scan.beyond_damage {
                        Some(b) => {
                            let end = b.end().unwrap_or(*offset);
                            let mut d = format!(
                                "{}; {} damaged chunk(s) at byte(s) {:?}, {} intact chunk(s) after the damage are still readable",
                                stop.describe(),
                                b.damaged.len(),
                                b.damaged.iter().map(|(o, _)| *o).collect::<Vec<_>>(),
                                b.intact_chunks()
                            );
                            let unreachable = scan.file_len.saturating_sub(end);
                            if unreachable > 0 {
                                d.push_str(&format!(
                                    "; {unreachable} bytes after byte {end} are not readable"
                                ));
                            }
                            d
                        }
                        None => format!(
                            "{}; {} bytes after it are not readable",
                            stop.describe(),
                            scan.file_len.saturating_sub(*offset)
                        ),
                    };
                    (IntegrityStatus::Damaged, Some(detail))
                }
            };
            (Outcome::Scanned(Box::new(scan)), integrity, detail)
        }
    };
    Assessment {
        outcome,
        integrity,
        detail,
        file_len,
    }
}

/// Size of the header CRC field, the last field of the file header.
const HEADER_CRC_BYTES: usize = 4;

/// Whether a segment file whose header does not decode is the trace of a crash right after the file was
/// created: it is exactly one header long (no chunk was ever written) and the header was never (fully)
/// persisted, i.e. it is all zeros, or it starts with the file magic and ends in zeros covering at least
/// the header CRC (after a power loss NTFS can keep the extended length with zero or partial data).
/// Anything else (including any read error) is not assumed to be harmless.
fn header_never_persisted(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    if !file
        .metadata()
        .is_ok_and(|m| m.len() == FILE_HEADER_LEN as u64)
    {
        return false;
    }
    let mut bytes = [0u8; FILE_HEADER_LEN];
    if !read_full(&mut file, &mut bytes).is_ok_and(|n| n == FILE_HEADER_LEN) {
        return false;
    }
    let zero_suffix = bytes.iter().rev().take_while(|&&b| b == 0).count();
    zero_suffix == FILE_HEADER_LEN
        || (bytes.starts_with(&FILE_MAGIC) && zero_suffix >= HEADER_CRC_BYTES)
}

/// Folds an assessment into a segment row. Returns the row and what to do with its chunk rows: the chunk
/// list of the valid prefix for a scanned file, none for a file without usable data, and "keep" when the
/// file could not be read at all.
pub(crate) fn apply_assessment(mut seg: SegmentInfo, a: &Assessment) -> (SegmentInfo, ChunkRows) {
    seg.file_len = a.file_len;
    seg.integrity = a.integrity;
    seg.integrity_detail = a.detail.clone();
    let rows = match &a.outcome {
        Outcome::Scanned(scan) => {
            // `byte_size` / `file_crc32c` describe the valid prefix; the counts, capture bounds and chunk
            // rows also cover the intact chunks beyond a damaged one, which readers still return.
            let beyond = scan.beyond_damage.as_ref();
            let min_of = |a: Option<u64>, b: Option<u64>| match (a, b) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let max_of = |a: Option<u64>, b: Option<u64>| match (a, b) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            seg.byte_size = scan.valid_len;
            seg.first_capture_ns =
                min_of(scan.min_capture_ns, beyond.and_then(|b| b.min_capture_ns));
            seg.last_capture_ns =
                max_of(scan.max_capture_ns, beyond.and_then(|b| b.max_capture_ns));
            seg.record_count = scan.record_count + beyond.map_or(0, |b| b.record_count);
            seg.chunk_count = scan.indexable_chunks().count() as u64;
            seg.file_crc32c = Some(scan.valid_crc32c);
            seg.created_mono_ns = seg
                .created_mono_ns
                .or_else(|| scan.header.created_mono_ns());
            ChunkRows::Replace(scan.indexable_chunks().map(ChunkEntry::from).collect())
        }
        Outcome::Missing | Outcome::Unusable => {
            seg.byte_size = 0;
            seg.first_capture_ns = None;
            seg.last_capture_ns = None;
            seg.record_count = 0;
            seg.chunk_count = 0;
            seg.file_crc32c = None;
            ChunkRows::Replace(Vec::new())
        }
        Outcome::Unreadable => ChunkRows::Keep,
    };
    (seg, rows)
}

/// Validation of an indexed segment: only the verdict (integrity, detail) and the observed file length are
/// updated; the writer-recorded reference (durable length, its CRC, counts, capture bounds) is kept.
fn with_assessment_verdict(mut seg: SegmentInfo, a: &Assessment) -> SegmentInfo {
    seg.file_len = a.file_len;
    seg.integrity = a.integrity;
    seg.integrity_detail = a.detail.clone();
    seg.created_mono_ns = seg
        .created_mono_ns
        .or_else(|| a.scan().and_then(|s| s.header.created_mono_ns()));
    seg
}

/// Session checksum: CRC-32C over `segment_index u32 | byte_size u64 | file_crc32c u32` (little-endian,
/// `file_crc32c` 0 when unknown) of every segment in index order (docs/STORAGE_DESIGN.md §5.2).
pub(crate) fn session_checksum(segments: &[SegmentInfo]) -> u32 {
    segments.iter().fold(0u32, |crc, s| {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&s.segment_index.to_le_bytes());
        b[4..12].copy_from_slice(&s.byte_size.to_le_bytes());
        b[12..16].copy_from_slice(&s.file_crc32c.unwrap_or(0).to_le_bytes());
        crc32c::crc32c_append(crc, &b)
    })
}

/// Outcome of startup recovery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Sessions that were ACTIVE when the store opened (now INTERRUPTED) and adopted orphan directories.
    pub sessions: Vec<RecoveredSession>,
    /// Sessions or directories that could not be processed; they are left untouched and retried at the
    /// next startup.
    pub failures: Vec<RecoveryFailure>,
    /// When the index file was corrupt: where it was moved (it is kept for inspection, never deleted). A
    /// new index was created and every session directory re-adopted from its segment files; laps, stints,
    /// events and metadata updates that existed only in the old index are not in the new one.
    pub index_set_aside: Option<PathBuf>,
}

/// One recovered (or re-validated) session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredSession {
    /// The session.
    pub session_id: SessionId,
    /// Session directory relative to the recordings directory.
    pub dir: String,
    /// True when the directory had no index entry and was adopted from disk.
    pub adopted: bool,
    /// Every segment as validated (rows as now stored in the index).
    pub segments: Vec<SegmentInfo>,
    /// Records in the valid parts of all segments.
    pub record_count: u64,
    /// Earliest valid capture time.
    pub first_capture_ns: Option<u64>,
    /// Last valid capture time.
    pub last_capture_ns: Option<u64>,
    /// Worst integrity over the segments.
    pub integrity: IntegrityStatus,
    /// Per-segment details of anything that is not OK.
    pub integrity_detail: Option<String>,
    /// Session checksum over the validated segments (docs/STORAGE_DESIGN.md §5.2).
    pub checksum_crc32c: u32,
}

/// A session or directory recovery could not process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryFailure {
    /// The session, when known.
    pub session_id: Option<SessionId>,
    /// The directory or file involved.
    pub path: PathBuf,
    /// What went wrong.
    pub error: String,
}

/// How [`rescan_session`] treats the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rescan {
    /// Startup recovery: the session becomes INTERRUPTED and chunk rows are rebuilt from the files.
    Recover { adopted: bool },
    /// On-demand validation of a finished session: status and chunk rows are kept, integrity is updated.
    Validate,
}

/// Recovers every ACTIVE session and adopts orphan session directories. Per-session problems are reported
/// in [`RecoveryReport::failures`] and never abort recovery of the others.
pub(crate) fn recover(cfg: &RecorderConfig, index: &mut Index, now_unix_ns: i64) -> RecoveryReport {
    let mut report = RecoveryReport::default();
    match index.session_ids_with_status(SessionStatus::Active) {
        Ok(ids) => {
            for id in ids {
                match rescan_session(
                    cfg,
                    index,
                    id,
                    Rescan::Recover { adopted: false },
                    now_unix_ns,
                ) {
                    Ok(r) => report.sessions.push(r),
                    Err(e) => {
                        tracing::error!(component = COMPONENT, session = %id, error = %e, "session recovery failed");
                        report.failures.push(RecoveryFailure {
                            session_id: Some(id),
                            path: cfg.recordings_dir.clone(),
                            error: e.to_string(),
                        });
                    }
                }
            }
        }
        Err(e) => {
            report.failures.push(RecoveryFailure {
                session_id: None,
                path: cfg.index_path(),
                error: e.to_string(),
            });
        }
    }
    adopt_orphans(cfg, index, now_unix_ns, &mut report);
    for s in &report.sessions {
        tracing::warn!(
            component = COMPONENT,
            session = %s.session_id,
            adopted = s.adopted,
            records = s.record_count,
            last_capture_ns = ?s.last_capture_ns,
            integrity = %s.integrity,
            detail = s.integrity_detail.as_deref().unwrap_or(""),
            "recovered interrupted recording session"
        );
    }
    report
}

/// Re-validates every segment of session `id` (index rows plus segment files found on disk) and updates
/// the index. See [`Rescan`].
pub(crate) fn rescan_session(
    cfg: &RecorderConfig,
    index: &mut Index,
    id: SessionId,
    mode: Rescan,
    now_unix_ns: i64,
) -> Result<RecoveredSession, RecorderError> {
    let info = index
        .session(id)?
        .ok_or(RecorderError::SessionNotFound(id))?;
    let rows = index.segments(id)?;
    let dir = cfg.resolve(Path::new(&info.dir));
    let on_disk = list_segment_files(&dir)
        .map_err(|e| RecorderError::io("listing segment files", &dir, e))?;
    let mut all: BTreeSet<u32> = rows.iter().map(|r| r.segment_index).collect();
    all.extend(on_disk.keys().copied());

    let mut segments = Vec::with_capacity(all.len());
    for segment_index in all {
        let row = rows.iter().find(|r| r.segment_index == segment_index);
        let rel_path = row.map_or_else(
            || format!("{}/{}", info.dir, segment_file_name(segment_index)),
            |r| r.path.clone(),
        );
        let a = assess_segment(&cfg.resolve(Path::new(&rel_path)), id, segment_index);
        let base = match row {
            Some(r) => r.clone(),
            None => orphan_segment_row(id, segment_index, rel_path, &a, info.started_unix_ns),
        };
        let (mut seg, chunk_rows) = match (mode, row) {
            // Validation never rewrites what the writer recorded as durable: that row stays the reference
            // for every later validation (otherwise damage found once would read as a plain torn tail the
            // next time, and capture bounds would shrink while the chunk rows still reach past them).
            (Rescan::Validate, Some(r)) => {
                (with_assessment_verdict(r.clone(), &a), ChunkRows::Keep)
            }
            _ => apply_assessment(base, &a),
        };
        seg.status = match (&a.outcome, row.map(|r| r.status)) {
            (Outcome::Missing, _) => SegmentStatus::Missing,
            (_, None | Some(SegmentStatus::Open | SegmentStatus::Missing)) => {
                SegmentStatus::Recovered
            }
            (_, Some(status)) => status,
        };
        if let (Some(row), Some(scan)) = (row, a.scan()) {
            check_durable_prefix(row, scan, &mut seg);
        }
        let chunk_rows = match mode {
            Rescan::Recover { .. } => chunk_rows,
            Rescan::Validate => ChunkRows::Keep,
        };
        index.put_segment(&seg, chunk_rows)?;
        segments.push(seg);
    }

    let mut integrity = IntegrityStatus::Ok;
    let mut details = Vec::new();
    for s in &segments {
        integrity = integrity.worst(s.integrity);
        if let Some(d) = &s.integrity_detail {
            details.push(format!("segment {}: {d}", s.segment_index));
        }
    }
    if segments.is_empty() {
        integrity = IntegrityStatus::Truncated;
        details.push("no segment file was created".to_owned());
    }
    let mut record_count = segments.iter().map(|s| s.record_count).sum();
    let mut first_capture_ns = segments.iter().filter_map(|s| s.first_capture_ns).min();
    let mut last_capture_ns = segments.iter().filter_map(|s| s.last_capture_ns).max();
    if mode == Rescan::Validate {
        // The session's extent stays what was recorded (see the per-segment rows above).
        record_count = info.record_count;
        first_capture_ns = info.first_capture_ns.or(first_capture_ns);
        last_capture_ns = info.last_capture_ns.or(last_capture_ns);
    }
    let checksum = session_checksum(&segments);
    let detail = (!details.is_empty()).then(|| details.join("; "));
    let (status, ended_unix_ns, recovered_unix_ns) = match mode {
        Rescan::Recover { .. } => (SessionStatus::Interrupted, None, Some(now_unix_ns)),
        Rescan::Validate => (info.status, info.ended_unix_ns, info.recovered_unix_ns),
    };
    index.end_session(
        id,
        &SessionEnd {
            status,
            ended_unix_ns,
            recovered_unix_ns,
            first_capture_ns,
            last_capture_ns,
            record_count,
            dropped_records: info.dropped_records,
            integrity,
            integrity_detail: detail.clone(),
            checksum_crc32c: Some(checksum),
        },
    )?;
    let adopted = matches!(mode, Rescan::Recover { adopted: true });
    if let Rescan::Recover { .. } = mode {
        let what = if adopted {
            "session directory adopted from disk (it had no index entry)"
        } else {
            "session was not finished (the application stopped while recording)"
        };
        let last = last_capture_ns.map_or_else(
            || "no valid data".to_owned(),
            |ns| format!("last valid capture ns {ns}"),
        );
        let description = format!(
            "{what}; recovered {record_count} records, {last}; integrity {integrity}{}",
            detail
                .as_deref()
                .map(|d| format!(": {d}"))
                .unwrap_or_default()
        );
        let event = EventRecord {
            capture_ns: last_capture_ns,
            unix_ns: Some(now_unix_ns),
            lap: None,
            category: EventCategory::Recorder,
            severity: Severity::Warning,
            description,
            channel_id: None,
            value: None,
            threshold: None,
        };
        index.insert_event(id, &event)?;
    }
    Ok(RecoveredSession {
        session_id: id,
        dir: info.dir,
        adopted,
        segments,
        record_count,
        first_capture_ns,
        last_capture_ns,
        integrity,
        integrity_detail: detail,
        checksum_crc32c: checksum,
    })
}

/// Checks that the bytes the index recorded as durable (`row.byte_size`, CRC `row.file_crc32c`) are still
/// present and byte-identical. Detects storage that acknowledged a sync it did not perform, and damage to
/// data that was already safely written.
fn check_durable_prefix(row: &SegmentInfo, scan: &SegmentScan, seg: &mut SegmentInfo) {
    if row.byte_size < FILE_HEADER_LEN as u64 {
        return;
    }
    let problem = if scan.valid_len < row.byte_size {
        Some(format!(
            "the index recorded {} bytes as durable but only {} bytes are valid",
            row.byte_size, scan.valid_len
        ))
    } else if let Some(expected) = row.file_crc32c {
        match scan.prefix_crc32c(row.byte_size) {
            Some(actual) if actual == expected => None,
            Some(actual) => Some(format!(
                "durable prefix [0, {}) changed: CRC-32C {expected:#010x} recorded, {actual:#010x} found",
                row.byte_size
            )),
            None => Some(format!(
                "recorded durable length {} is not a chunk boundary in the file",
                row.byte_size
            )),
        }
    } else {
        None
    };
    if let Some(p) = problem {
        seg.integrity = IntegrityStatus::Damaged;
        seg.integrity_detail = Some(match seg.integrity_detail.take() {
            Some(d) => format!("{p}; {d}"),
            None => p,
        });
    }
}

/// Row for a segment file found on disk but missing from the index.
fn orphan_segment_row(
    id: SessionId,
    segment_index: u32,
    path: String,
    a: &Assessment,
    fallback_unix_ns: i64,
) -> SegmentInfo {
    let header = a.scan().map(|s| &s.header);
    SegmentInfo {
        session_id: id,
        segment_index,
        path,
        status: SegmentStatus::Recovered,
        // Without a readable header the creation time is unknown; the session start is the best bound.
        created_unix_ns: header.map_or(fallback_unix_ns, |h| h.created_unix_ns),
        created_mono_ns: header.and_then(FileHeader::created_mono_ns),
        closed_unix_ns: None,
        byte_size: 0,
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

/// Segment files (`seg-NNNNNN.lmurec`) in `dir` by index. A missing directory yields an empty map.
pub(crate) fn list_segment_files(dir: &Path) -> io::Result<BTreeMap<u32, PathBuf>> {
    let mut out = BTreeMap::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        if let Some(index) = entry.file_name().to_str().and_then(parse_segment_file_name) {
            out.insert(index, entry.path());
        }
    }
    Ok(out)
}

fn adopt_orphans(
    cfg: &RecorderConfig,
    index: &mut Index,
    now_unix_ns: i64,
    report: &mut RecoveryReport,
) {
    let sessions_dir = cfg.sessions_dir();
    let entries = match fs::read_dir(&sessions_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == ErrorKind::NotFound => return,
        Err(e) => {
            report.failures.push(RecoveryFailure {
                session_id: None,
                path: sessions_dir,
                error: e.to_string(),
            });
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                report.failures.push(RecoveryFailure {
                    session_id: None,
                    path: sessions_dir.clone(),
                    error: e.to_string(),
                });
                continue;
            }
        };
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(id) = parse_session_dir_name(&name) else {
            continue;
        };
        match index.session_exists(id) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                report.failures.push(RecoveryFailure {
                    session_id: Some(id),
                    path,
                    error: e.to_string(),
                });
                continue;
            }
        }
        match adopt_session(cfg, index, id, &name, now_unix_ns) {
            Ok(Some(r)) => report.sessions.push(r),
            Ok(None) => {}
            Err(e) => {
                tracing::error!(component = COMPONENT, session = %id, error = %e, "adopting an orphan session failed");
                report.failures.push(RecoveryFailure {
                    session_id: Some(id),
                    path,
                    error: e.to_string(),
                });
            }
        }
    }
}

/// Adds an orphan session directory to the index (metadata from its first valid segment header) and
/// recovers it. Directories without any valid segment header are left alone (`Ok(None)`).
fn adopt_session(
    cfg: &RecorderConfig,
    index: &mut Index,
    id: SessionId,
    dir_name: &str,
    now_unix_ns: i64,
) -> Result<Option<RecoveredSession>, RecorderError> {
    let rel_dir = format!("{SESSIONS_DIR_NAME}/{dir_name}");
    let dir = cfg.resolve(Path::new(&rel_dir));
    let files = list_segment_files(&dir)
        .map_err(|e| RecorderError::io("listing segment files", &dir, e))?;
    let header = files.iter().find_map(|(&segment_index, path)| {
        read_file_header(path)
            .ok()
            .filter(|h| h.session_id == id.0 && h.segment_index == segment_index)
    });
    let Some(h) = header else {
        tracing::warn!(
            component = COMPONENT,
            dir = %dir.display(),
            "session directory has no valid segment; not adopting it"
        );
        return Ok(None);
    };
    let info = SessionInfo {
        id,
        dir: rel_dir,
        status: SessionStatus::Active,
        started_unix_ns: h.created_unix_ns,
        ended_unix_ns: None,
        recovered_unix_ns: None,
        first_capture_ns: None,
        last_capture_ns: None,
        format_version: h.format_version,
        meta: SessionMeta {
            app_version: h.app_version,
            telemetry_schema_version: h.telemetry_schema_version,
            ..SessionMeta::default()
        },
        record_count: 0,
        dropped_records: 0,
        integrity: IntegrityStatus::Unverified,
        integrity_detail: None,
        checksum_crc32c: None,
        retention: RetentionClass::Default,
    };
    index.insert_session(&info)?;
    rescan_session(
        cfg,
        index,
        id,
        Rescan::Recover { adopted: true },
        now_unix_ns,
    )
    .map(Some)
}

fn read_file_header(path: &Path) -> Result<FileHeader, RecorderError> {
    let mut file = File::open(path).map_err(|e| RecorderError::io("opening a segment", path, e))?;
    let mut buf = [0u8; FILE_HEADER_LEN];
    let n = read_full(&mut file, &mut buf)
        .map_err(|e| RecorderError::io("reading a segment header", path, e))?;
    FileHeader::decode(&buf[..n]).map_err(|source| RecorderError::Format {
        path: path.to_owned(),
        offset: 0,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(index: u32, size: u64, crc: Option<u32>) -> SegmentInfo {
        SegmentInfo {
            session_id: SessionId(uuid::Uuid::nil()),
            segment_index: index,
            path: String::new(),
            status: SegmentStatus::Closed,
            created_unix_ns: 0,
            created_mono_ns: None,
            closed_unix_ns: None,
            byte_size: size,
            file_len: None,
            first_capture_ns: None,
            last_capture_ns: None,
            record_count: 0,
            chunk_count: 0,
            file_crc32c: crc,
            integrity: IntegrityStatus::Ok,
            integrity_detail: None,
        }
    }

    #[test]
    fn session_checksum_covers_every_segment_field() {
        let base = vec![seg(0, 1000, Some(1)), seg(1, 2000, Some(2))];
        let c = session_checksum(&base);
        let mut expected_bytes = Vec::new();
        for (i, size, crc) in [(0u32, 1000u64, 1u32), (1, 2000, 2)] {
            expected_bytes.extend_from_slice(&i.to_le_bytes());
            expected_bytes.extend_from_slice(&size.to_le_bytes());
            expected_bytes.extend_from_slice(&crc.to_le_bytes());
        }
        assert_eq!(c, crc32c::crc32c(&expected_bytes), "documented layout");
        for variant in [
            vec![seg(0, 1000, Some(1)), seg(1, 2001, Some(2))],
            vec![seg(0, 1000, Some(1)), seg(1, 2000, Some(3))],
            vec![seg(0, 1000, Some(1)), seg(2, 2000, Some(2))],
            vec![seg(0, 1000, Some(1))],
        ] {
            assert_ne!(session_checksum(&variant), c);
        }
        assert_eq!(session_checksum(&[]), 0);
    }

    #[test]
    fn missing_file_is_assessed_as_missing() {
        let a = assess_segment(
            Path::new("Z:\\definitely\\not\\here\\seg-000000.lmurec"),
            SessionId(uuid::Uuid::nil()),
            0,
        );
        assert!(matches!(a.outcome, Outcome::Missing | Outcome::Unreadable));
        assert_ne!(a.integrity, IntegrityStatus::Ok);
    }

    #[test]
    fn an_unpersisted_header_is_truncated_not_damaged() {
        let tmp = crate::test_support::TempDir::new("unpersisted-header");
        let path = tmp.path().join(segment_file_name(3));
        let id = SessionId(uuid::Uuid::from_u128(9));
        let assess = |bytes: &[u8]| {
            fs::write(&path, bytes).expect("write");
            assess_segment(&path, id, 3)
        };

        // Power loss right after creation: the length survived, the data did not.
        let a = assess(&[0u8; FILE_HEADER_LEN]);
        assert!(matches!(a.outcome, Outcome::Unusable));
        assert_eq!(a.integrity, IntegrityStatus::Truncated, "{:?}", a.detail);
        assert!(
            a.detail
                .as_deref()
                .is_some_and(|d| d.contains("created but never written"))
        );

        // Partially persisted header: magic present, the rest (CRC included) still zero.
        let mut partial = [0u8; FILE_HEADER_LEN];
        partial[..FILE_MAGIC.len()].copy_from_slice(&FILE_MAGIC);
        partial[FILE_MAGIC.len()] = 1;
        assert_eq!(assess(&partial).integrity, IntegrityStatus::Truncated);

        // Garbage of header length, a zero header followed by data, or a header with a wrong CRC are
        // damage, not a harmless crash trace.
        let mut garbage = [0xA5u8; FILE_HEADER_LEN];
        assert_eq!(assess(&garbage).integrity, IntegrityStatus::Damaged);
        garbage[..FILE_MAGIC.len()].copy_from_slice(&FILE_MAGIC);
        assert_eq!(assess(&garbage).integrity, IntegrityStatus::Damaged);
        let mut longer = vec![0u8; FILE_HEADER_LEN];
        longer.extend_from_slice(&[1u8; 64]);
        assert_eq!(assess(&longer).integrity, IntegrityStatus::Damaged);
    }
}
