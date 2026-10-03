//! The recordings directory: exclusive lock, session index, startup recovery, session creation, queries,
//! validation, retention and readers.

use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::COMPONENT;
use crate::clock::{format_utc_compact, unix_now_ns};
use crate::config::{
    INDEX_FILE_NAME, RecorderConfig, SESSIONS_DIR_NAME, parse_session_dir_name, session_dir_name,
};
use crate::error::RecorderError;
use crate::format::FORMAT_VERSION;
use crate::index::{ChunkEntry, EventQuery, Index, SegmentInfo, SessionInfo};
use crate::reader::{SegmentPlan, SessionReader};
use crate::recovery::{RecoveredSession, RecoveryReport, Rescan, recover, rescan_session};
use crate::retention::{RetentionFailure, RetentionPolicy, RetentionReport};
use crate::types::{
    EventRecord, IntegrityStatus, LapRecord, RetentionClass, SessionId, SessionMeta, SessionStatus,
    StintRecord,
};
use crate::writer::{self, ExitGuard, RecordingSession, SessionOptions, SessionSpawn};

/// Sessions being recorded by this store (their writers are running).
type ActiveSessions = Arc<Mutex<HashSet<SessionId>>>;

/// One recordings directory (see the crate documentation for the layout). `Send + Sync`: share it behind an
/// `Arc`. At most one store (in any process) can hold a directory at a time.
pub struct RecordingStore {
    cfg: RecorderConfig,
    /// The store's own index connection (each writer thread has its own).
    index: Mutex<Index>,
    active: ActiveSessions,
    recovery: RecoveryReport,
    /// Exclusively locked for the lifetime of the store; the OS releases it if the process dies.
    _lock: File,
}

impl fmt::Debug for RecordingStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordingStore")
            .field("recordings_dir", &self.cfg.recordings_dir)
            .field("recovery", &self.recovery)
            .finish_non_exhaustive()
    }
}

/// Locks a mutex, ignoring poisoning (the protected state stays consistent: a set of ids, or a SQLite
/// connection whose open transaction is rolled back when its guard unwinds).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl RecordingStore {
    /// Opens (creating if needed) the recordings directory `cfg.recordings_dir`: takes its exclusive lock,
    /// opens the index and runs startup recovery (see [`RecoveryReport`]). Recovery never modifies a
    /// recording file.
    ///
    /// Fails with [`RecorderError::DirectoryInUse`] when another store holds the directory.
    pub fn open(cfg: RecorderConfig) -> Result<Self, RecorderError> {
        cfg.validate()?;
        let root = cfg.recordings_dir.clone();
        fs::create_dir_all(&root)
            .map_err(|e| RecorderError::io("creating the recordings directory", &root, e))?;
        let lock_file = acquire_lock(&cfg)?;
        let sessions = cfg.sessions_dir();
        fs::create_dir_all(&sessions)
            .map_err(|e| RecorderError::io("creating the sessions directory", &sessions, e))?;
        let (mut index, index_set_aside) = open_index(&cfg)?;
        let mut recovery = recover(&cfg, &mut index, unix_now_ns());
        recovery.index_set_aside = index_set_aside;
        tracing::info!(
            component = COMPONENT,
            dir = %root.display(),
            recovered = recovery.sessions.len(),
            failures = recovery.failures.len(),
            "recordings directory opened"
        );
        Ok(Self {
            cfg,
            index: Mutex::new(index),
            active: Arc::default(),
            recovery,
            _lock: lock_file,
        })
    }

    /// The configuration the store was opened with.
    pub fn config(&self) -> &RecorderConfig {
        &self.cfg
    }

    /// What startup recovery did when the store was opened.
    pub fn recovery_report(&self) -> &RecoveryReport {
        &self.recovery
    }

    fn index(&self) -> MutexGuard<'_, Index> {
        lock(&self.index)
    }

    /// Starts recording a new session: registers it (ACTIVE) in the index, creates its directory and first
    /// segment, and starts its writer thread. Errors leave nothing behind (no index row, no directory).
    pub fn start_session(
        &self,
        meta: SessionMeta,
        options: SessionOptions,
    ) -> Result<RecordingSession, RecorderError> {
        meta.validate().map_err(RecorderError::InvalidMeta)?;
        let id = SessionId::new_random();
        let started_unix_ns = unix_now_ns();
        let rel_dir = format!(
            "{SESSIONS_DIR_NAME}/{}",
            session_dir_name(started_unix_ns, id)
        );
        let dir = self.cfg.resolve(Path::new(&rel_dir));
        {
            let mut index = self.index();
            if let Some(json) = &meta.data_availability_json
                && !index.json_valid(json)?
            {
                return Err(RecorderError::InvalidMeta(
                    "data_availability_json is not valid JSON".to_owned(),
                ));
            }
            index.insert_session(&SessionInfo {
                id,
                dir: rel_dir.clone(),
                status: SessionStatus::Active,
                started_unix_ns,
                ended_unix_ns: None,
                recovered_unix_ns: None,
                first_capture_ns: None,
                last_capture_ns: None,
                format_version: FORMAT_VERSION,
                meta: meta.clone(),
                record_count: 0,
                dropped_records: 0,
                integrity: IntegrityStatus::Unverified,
                integrity_detail: None,
                checksum_crc32c: None,
                retention: RetentionClass::Default,
            })?;
        }
        let writer_index = match Index::open(&self.cfg.index_path()) {
            Ok(index) => index,
            Err(e) => {
                self.discard_failed_start(id, &dir);
                return Err(e.into());
            }
        };
        lock(&self.active).insert(id);
        let active = Arc::clone(&self.active);
        let on_exit = ExitGuard::new(move || {
            lock(&active).remove(&id);
        });
        let spawn = SessionSpawn {
            config: self.cfg.clone(),
            session_id: id,
            rel_dir,
            dir: dir.clone(),
            app_version: meta.app_version,
            schema_version: meta.telemetry_schema_version,
            index: writer_index,
            options,
            on_exit,
        };
        writer::spawn(spawn).inspect_err(|e| {
            tracing::error!(component = COMPONENT, session = %id, error = %e, "could not start a recording session");
            self.discard_failed_start(id, &dir);
        })
    }

    /// Removes the traces of a session whose start failed before anything was recorded.
    fn discard_failed_start(&self, id: SessionId, dir: &Path) {
        if let Err(e) = self.index().delete_session(id) {
            tracing::warn!(component = COMPONENT, session = %id, error = %e, "could not remove the index row of a failed session start");
        }
        // `remove_dir` only removes an empty directory: anything recorded is never deleted here.
        match fs::remove_dir(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                component = COMPONENT,
                session = %id,
                dir = %dir.display(),
                error = %e,
                "session directory of a failed start left in place (it is adopted at the next startup)"
            ),
        }
    }

    /// Whether a session is being recorded by this store right now.
    pub fn is_recording(&self, id: SessionId) -> bool {
        lock(&self.active).contains(&id)
    }

    /// Sessions being recorded by this store right now.
    pub fn recording_sessions(&self) -> Vec<SessionId> {
        lock(&self.active).iter().copied().collect()
    }

    /// Every session in the index, oldest first.
    pub fn sessions(&self) -> Result<Vec<SessionInfo>, RecorderError> {
        Ok(self.index().sessions()?)
    }

    /// One session, if it exists.
    pub fn session(&self, id: SessionId) -> Result<Option<SessionInfo>, RecorderError> {
        Ok(self.index().session(id)?)
    }

    /// Segments of a session in order.
    pub fn segments(&self, id: SessionId) -> Result<Vec<SegmentInfo>, RecorderError> {
        Ok(self.index().segments(id)?)
    }

    /// Chunk offset index of one segment (a contiguous prefix of its chunks).
    pub fn chunks(
        &self,
        id: SessionId,
        segment_index: u32,
    ) -> Result<Vec<ChunkEntry>, RecorderError> {
        Ok(self.index().chunks(id, segment_index)?)
    }

    /// Laps of a session in lap order.
    pub fn laps(&self, id: SessionId) -> Result<Vec<LapRecord>, RecorderError> {
        Ok(self.index().laps(id)?)
    }

    /// Stints of a session in stint order.
    pub fn stints(&self, id: SessionId) -> Result<Vec<StintRecord>, RecorderError> {
        Ok(self.index().stints(id)?)
    }

    /// Events of a session matching `query`.
    pub fn events(
        &self,
        id: SessionId,
        query: &EventQuery,
    ) -> Result<Vec<EventRecord>, RecorderError> {
        Ok(self.index().events(id, query)?)
    }

    /// Opens a session for reading (also allowed while it is being recorded: the reader then sees the data
    /// handed to the OS so far).
    pub fn open_reader(&self, id: SessionId) -> Result<SessionReader, RecorderError> {
        let snapshot = self
            .index()
            .session_snapshot(id)?
            .ok_or(RecorderError::SessionNotFound(id))?;
        let plans = snapshot
            .segments
            .into_iter()
            .map(|(info, chunks)| SegmentPlan { info, chunks })
            .collect();
        Ok(SessionReader::new(
            self.cfg.recordings_dir.clone(),
            snapshot.session,
            plans,
        ))
    }

    /// Fully re-validates a finished session against its files (every chunk: CRCs, decompression, framing;
    /// every durable prefix the index recorded) and stores the result (integrity status, detail, checksum).
    /// Never modifies a recording file. Status and chunk rows are kept.
    pub fn validate_session(&self, id: SessionId) -> Result<RecoveredSession, RecorderError> {
        if self.is_recording(id) {
            return Err(RecorderError::SessionBusy(id));
        }
        let mut index = self.index();
        let info = index
            .session(id)?
            .ok_or(RecorderError::SessionNotFound(id))?;
        if info.status == SessionStatus::Active {
            return Err(RecorderError::InvalidSessionState {
                id,
                status: info.status,
                required: "COMPLETE or INTERRUPTED",
            });
        }
        rescan_session(&self.cfg, &mut index, id, Rescan::Validate, unix_now_ns())
    }

    /// Sets a session's retention class (KEEP protects it from every retention policy).
    pub fn set_retention(&self, id: SessionId, class: RetentionClass) -> Result<(), RecorderError> {
        if self.index().set_retention(id, class)? {
            Ok(())
        } else {
            Err(RecorderError::SessionNotFound(id))
        }
    }

    /// Deletes a finished session: its directory (segment files) and its index rows. An explicit user
    /// action: it also deletes sessions marked KEEP. Refuses sessions that are being recorded or ACTIVE.
    pub fn delete_session(&self, id: SessionId) -> Result<(), RecorderError> {
        self.delete_session_checked(id, false)
    }

    fn delete_session_checked(
        &self,
        id: SessionId,
        for_retention: bool,
    ) -> Result<(), RecorderError> {
        if self.is_recording(id) {
            return Err(RecorderError::SessionBusy(id));
        }
        let mut index = self.index();
        let info = index
            .session(id)?
            .ok_or(RecorderError::SessionNotFound(id))?;
        if info.status == SessionStatus::Active {
            return Err(RecorderError::InvalidSessionState {
                id,
                status: info.status,
                required: "COMPLETE or INTERRUPTED",
            });
        }
        if for_retention && info.retention != RetentionClass::Default {
            return Err(RecorderError::RetentionKeep(id));
        }
        let dir = self.checked_session_dir(&info)?;
        match fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(RecorderError::io("deleting a session directory", &dir, e)),
        }
        index.delete_session(id)?;
        tracing::info!(component = COMPONENT, session = %id, dir = %dir.display(), "session deleted");
        Ok(())
    }

    /// The session directory, accepted only in the form `sessions/<this session's directory name>`, so an
    /// index row can never make the store delete anything else.
    fn checked_session_dir(&self, info: &SessionInfo) -> Result<PathBuf, RecorderError> {
        let rel = Path::new(&info.dir);
        let mut parts = rel.components();
        let valid = match (parts.next(), parts.next(), parts.next()) {
            (Some(Component::Normal(first)), Some(Component::Normal(name)), None) => {
                first == SESSIONS_DIR_NAME
                    && name.to_str().and_then(parse_session_dir_name) == Some(info.id)
            }
            _ => false,
        };
        if !valid {
            return Err(RecorderError::PathOutsideRoot(rel.to_owned()));
        }
        Ok(self.cfg.resolve(rel))
    }

    /// Runs a retention policy. Only finished sessions with retention class DEFAULT that are not being
    /// recorded are offered to the policy; nothing else can be deleted through this call. With [`KeepAll`]
    /// (the default policy) nothing is deleted.
    ///
    /// [`KeepAll`]: crate::KeepAll
    pub fn apply_retention(
        &self,
        policy: &dyn RetentionPolicy,
    ) -> Result<RetentionReport, RecorderError> {
        let candidates: Vec<SessionInfo> = self
            .sessions()?
            .into_iter()
            .filter(|s| {
                s.status != SessionStatus::Active
                    && s.retention == RetentionClass::Default
                    && !self.is_recording(s.id)
            })
            .collect();
        let selected = policy.select(&candidates, unix_now_ns());
        let mut report = RetentionReport::default();
        let mut seen = HashSet::new();
        for id in selected {
            if !seen.insert(id) {
                continue;
            }
            if !candidates.iter().any(|c| c.id == id) {
                report.refused.push(id);
                continue;
            }
            match self.delete_session_checked(id, true) {
                Ok(()) => report.deleted.push(id),
                Err(e) => {
                    tracing::warn!(component = COMPONENT, session = %id, error = %e, "retention could not delete a session");
                    report.failed.push(RetentionFailure {
                        session_id: id,
                        error: e.to_string(),
                    });
                }
            }
        }
        if !report.deleted.is_empty() || !report.refused.is_empty() || !report.failed.is_empty() {
            tracing::info!(
                component = COMPONENT,
                deleted = report.deleted.len(),
                refused = report.refused.len(),
                failed = report.failed.len(),
                "retention policy applied"
            );
        }
        Ok(report)
    }
}

/// Opens and exclusively locks the lock file of the recordings directory.
fn acquire_lock(cfg: &RecorderConfig) -> Result<File, RecorderError> {
    let path = cfg.lock_path();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| RecorderError::io("opening the lock file", &path, e))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(RecorderError::DirectoryInUse {
            path: cfg.recordings_dir.clone(),
        }),
        Err(TryLockError::Error(e)) => Err(RecorderError::io(
            "locking the recordings directory",
            &path,
            e,
        )),
    }
}

/// Opens the index. A corrupt index file is moved aside (never deleted) and replaced by a new one; startup
/// recovery then re-adopts every session directory from its segment files. Returns where the corrupt file
/// went, if that happened.
fn open_index(cfg: &RecorderConfig) -> Result<(Index, Option<PathBuf>), RecorderError> {
    let path = cfg.index_path();
    match Index::open(&path) {
        Ok(index) => Ok((index, None)),
        Err(e) if e.is_corruption() => {
            let suffix = uuid::Uuid::new_v4().simple().to_string();
            let aside = cfg.recordings_dir.join(format!(
                "{INDEX_FILE_NAME}.corrupt-{}-{}",
                format_utc_compact(unix_now_ns()),
                &suffix[..8]
            ));
            tracing::error!(
                component = COMPONENT,
                path = %path.display(),
                aside = %aside.display(),
                error = %e,
                "the session index is corrupt; it is set aside and rebuilt from the segment files"
            );
            fs::rename(&path, &aside)
                .map_err(|io| RecorderError::io("setting a corrupt index aside", &path, io))?;
            for sidecar_suffix in ["-wal", "-shm"] {
                let from = with_suffix(&path, sidecar_suffix);
                match fs::rename(&from, with_suffix(&aside, sidecar_suffix)) {
                    Ok(()) => {}
                    Err(io) if io.kind() == ErrorKind::NotFound => {}
                    Err(io) => {
                        return Err(RecorderError::io(
                            "setting a corrupt index aside",
                            &from,
                            io,
                        ));
                    }
                }
            }
            Ok((Index::open(&path)?, Some(aside)))
        }
        Err(e) => Err(e.into()),
    }
}

/// `path` with `suffix` appended to its file name (SQLite's `-wal` / `-shm` side files).
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}
