//! The blocking reader: attach to `LMU_Data`, wait for frames, copy under the SDK lock, process,
//! and watch liveness. Meant to run on a dedicated OS thread (research §5.3).
//!
//! ```text
//! loop {
//!     match reader.next_snapshot(Duration::from_millis(100)) {
//!         Ok(Some(snapshot)) => { /* hand to the normalizer via a bounded queue */ }
//!         Ok(None) => { /* no new frame yet; reader.state() says why */ }
//!         Err(e) => { /* report; IncompatibleInterface persists until LMU changes */ }
//!     }
//! }
//! ```
//!
//! Robustness rules implemented here:
//! * **Frame gate fallback (A-LMU-003).** The Hold wait is bounded by [`GATE_WITNESS_INTERVAL`];
//!   when it times out, a cheap witness read (ETs, counts, telemetry header) checks whether LMU
//!   published without signalling. Such frames are copied anyway and counted
//!   ([`ReaderStats::gate_missed_updates`]); if the gate delivers fewer than half of the new frames
//!   over a window, the connection switches to polling (degraded).
//! * **Late kernel objects.** A gate or lock that did not exist at attach time is re-opened every
//!   `retry_interval`, and the connection is upgraded in place.
//! * **Stale data (A-LMU-056).** The last frame of a connection that died is remembered; a mapping
//!   still holding exactly that frame is not attached to, whether or not an LMU process exists (it
//!   may be starting and not have published yet).

use std::io;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::blocks::{
    ByteSource, CopyPlan, OutOfBounds, RawBlocks, VerifiedCopy, Witness, copy_frame,
    copy_frame_verified, read_witness, witness_of,
};
use crate::clock::{MonoClock, duration_ns};
use crate::compat::CompatibilityReport;
use crate::error::LmuError;
use crate::offsets::abs;
use crate::processor::{FrameMeta, FrameOutcome, FrameProcessor};
use crate::snapshot::RawSnapshot;
use crate::state::{Consistency, FrameSource, LockMode, ObjectNames, ReaderState, SyncMode};
use crate::stats::{LOCK_HOLD_WARN_NS, ReaderStats};
use crate::sys::{
    self, FrameGate, GateWait, LockAttempt, LockOpen, OsError, ProcessWatch, SharedLock, SharedView,
};

/// `tracing` component name.
const COMPONENT: &str = "lmu-reader";

/// SDK `Lock` spin count before waiting (research §5.2).
pub const DEFAULT_LOCK_SPINS: u32 = 4_000;
/// Upper bound of the configurable spin count.
pub const MAX_LOCK_SPINS: u32 = 100_000;
/// Hard upper bound of the lock wait after spinning (≤ 5 ms, research §5.3).
pub const MAX_LOCK_WAIT: Duration = Duration::from_millis(5);
/// The lock counts as stuck (A-LMU-052) after being unavailable this long, while `busy` still reads
/// 1 and at least [`LOCK_STUCK_MIN_ATTEMPTS`] consecutive acquisitions failed.
pub const LOCK_STUCK_AFTER: Duration = Duration::from_secs(1);
/// Consecutive failed acquisitions needed before the lock can be reported stuck.
pub const LOCK_STUCK_MIN_ATTEMPTS: u32 = 2;
/// Longest timeout honoured by one [`LmuReader::next_snapshot`] call.
pub const MAX_CALL_TIMEOUT: Duration = Duration::from_secs(3_600);
/// Longest Hold-event wait before a witness read checks for a publish the gate missed (A-LMU-003).
pub const GATE_WITNESS_INTERVAL: Duration = Duration::from_millis(20);
/// Length of one frame-gate audit window (A-LMU-003).
pub const GATE_AUDIT_WINDOW: Duration = Duration::from_secs(2);
/// New frames a window needs before the gate is judged.
pub const GATE_AUDIT_MIN_FRAMES: u32 = 20;
/// Pause after a duplicate frame in gate mode. Guards against a gate that stays signalled (at most
/// ~1,000 copies/s) while adding at most 1 ms before the next real frame (frames are ~10 ms apart).
const DUPLICATE_BACKOFF: Duration = Duration::from_millis(1);
/// Minimum interval between repeated warnings of the same kind.
const WARN_INTERVAL: Duration = Duration::from_secs(10);

/// Accepted range of [`ReaderConfig::poll_interval`] (0 would take LMU's lock in a tight loop).
pub const POLL_INTERVAL_RANGE: (Duration, Duration) =
    (Duration::from_millis(1), Duration::from_secs(1));
/// Accepted range of [`ReaderConfig::gate_timeout`] (0 would busy-spin).
pub const GATE_TIMEOUT_RANGE: (Duration, Duration) =
    (Duration::from_millis(1), Duration::from_secs(1));
/// Accepted range of [`ReaderConfig::retry_interval`].
pub const RETRY_INTERVAL_RANGE: (Duration, Duration) =
    (Duration::from_millis(50), Duration::from_secs(60));
/// Accepted range of [`ReaderConfig::stale_after`].
pub const STALE_AFTER_RANGE: (Duration, Duration) =
    (Duration::from_millis(100), Duration::from_secs(60));
/// Accepted range of [`ReaderConfig::paths_refresh`].
pub const PATHS_REFRESH_RANGE: (Duration, Duration) =
    (Duration::from_secs(1), Duration::from_secs(3_600));
/// Upper bound of [`ReaderConfig::lock_free_attempts`] and [`ReaderConfig::incompatible_after`].
pub const MAX_ATTEMPTS: u32 = 100;

/// Reader tuning. The defaults follow the research contract (§5.3); every wait is bounded and every
/// value is clamped to a safe range by the reader (see the `*_RANGE` constants).
#[derive(Debug, Clone, Copy)]
pub struct ReaderConfig {
    /// Time base of `capture_mono_ns`. Share it with consumers to compute sample ages.
    pub clock: MonoClock,
    /// Bound of each frame-gate wait (also how often liveness is re-checked while waiting).
    pub gate_timeout: Duration,
    /// Copy interval in fallback mode (no frame gate).
    pub poll_interval: Duration,
    /// Interval between attach attempts while LMU is absent or incompatible, and between attempts
    /// to open a frame gate or lock that did not exist at attach time.
    pub retry_interval: Duration,
    /// No new frame for this long marks the connection [`ReaderState::Stalled`] and triggers the
    /// liveness check.
    pub stale_after: Duration,
    /// Compare-exchange spins before waiting for the lock.
    pub lock_spins: u32,
    /// Bounded lock wait after spinning (clamped to [`MAX_LOCK_WAIT`]).
    pub lock_wait: Duration,
    /// Copy attempts per frame in lock-free mode.
    pub lock_free_attempts: u32,
    /// Consecutive structurally failing frames before the reader stops as incompatible.
    pub incompatible_after: u32,
    /// Copy `data.paths` (PII) on the first frame of a connection and every `paths_refresh`.
    pub copy_paths: bool,
    /// Refresh interval of `data.paths` when `copy_paths` is set.
    pub paths_refresh: Duration,
    /// Copy `scoringStream` (up to 65,535 bytes) with every frame.
    pub copy_scoring_stream: bool,
    /// Kernel object names; leave at [`ObjectNames::LMU`] except in tests.
    pub object_names: ObjectNames,
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            object_names: ObjectNames::LMU,
            clock: MonoClock::new(),
            gate_timeout: Duration::from_millis(100),
            poll_interval: Duration::from_millis(10),
            retry_interval: Duration::from_secs(1),
            stale_after: Duration::from_secs(2),
            lock_spins: DEFAULT_LOCK_SPINS,
            lock_wait: MAX_LOCK_WAIT,
            lock_free_attempts: 5,
            incompatible_after: 3,
            copy_paths: false,
            paths_refresh: Duration::from_secs(30),
            copy_scoring_stream: false,
        }
    }
}

impl ReaderConfig {
    /// The configuration with every value clamped to its safe range.
    pub fn clamped(mut self) -> Self {
        let clamp = |d: Duration, (min, max): (Duration, Duration)| d.clamp(min, max);
        self.gate_timeout = clamp(self.gate_timeout, GATE_TIMEOUT_RANGE);
        self.poll_interval = clamp(self.poll_interval, POLL_INTERVAL_RANGE);
        self.retry_interval = clamp(self.retry_interval, RETRY_INTERVAL_RANGE);
        self.stale_after = clamp(self.stale_after, STALE_AFTER_RANGE);
        self.paths_refresh = clamp(self.paths_refresh, PATHS_REFRESH_RANGE);
        self.lock_spins = self.lock_spins.min(MAX_LOCK_SPINS);
        self.lock_wait = self.lock_wait.min(MAX_LOCK_WAIT);
        self.lock_free_attempts = self.lock_free_attempts.clamp(1, MAX_ATTEMPTS);
        self.incompatible_after = self.incompatible_after.clamp(1, MAX_ATTEMPTS);
        self
    }
}

/// `now + d`, or `now` if that would overflow (all durations are clamped, so it never does).
fn instant_after(now: Instant, d: Duration) -> Instant {
    now.checked_add(d).unwrap_or(now)
}

/// Point-in-time view of the reader for diagnostics UIs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReaderDiagnostics {
    /// Connection state.
    pub state: ReaderState,
    /// Connection generation (increments on every attach).
    pub connection_id: u64,
    /// PID of `Le Mans Ultimate.exe` from the last process lookup.
    pub lmu_pid: Option<u32>,
    /// `VirtualQuery` region size of the current view.
    pub mapping_size: Option<usize>,
    /// Frame synchronisation of the current connection.
    pub sync_mode: Option<SyncMode>,
    /// Why the connection polls instead of using the frame gate, if it does.
    pub sync_reason: Option<String>,
    /// Copy protection of the current connection.
    pub lock_mode: Option<LockMode>,
    /// Time since the current connection was established.
    pub connected_for_ns: Option<u64>,
    /// Time since the last new frame (or since connecting).
    pub since_new_frame_ns: Option<u64>,
    /// Raw `busy` lock word (read-only observation).
    pub lock_busy_word: Option<i32>,
    /// Raw `waiters` lock word (read-only observation).
    pub lock_waiters_word: Option<i32>,
    /// How long the lock has been unavailable to us (cleared as soon as `busy` reads 0 again).
    pub lock_unavailable_ns: Option<u64>,
    /// `busy` still reads 1, at least [`LOCK_STUCK_MIN_ATTEMPTS`] consecutive acquisitions failed
    /// and the lock has been unavailable for more than [`LOCK_STUCK_AFTER`] (A-LMU-052: another
    /// program holds or abandoned it; restart LMU. We never reset it).
    pub lock_stuck: bool,
    /// `SetEvent` failures when releasing the lock.
    pub lock_set_event_failures: u64,
    /// Why the reader is waiting, in plain words.
    pub note: Option<String>,
    /// The last unexpected OS error, if any.
    pub last_error: Option<String>,
}

/// What woke the reader up for the copy in progress (frame-gate audit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wake {
    /// The initial copies right after attaching.
    Attach,
    /// The frame gate signalled.
    Gate,
    /// The gate stayed silent but a witness read saw new data.
    Witness,
    /// Polling mode.
    Poll,
}

/// Compares the frames the gate delivered with the frames only witness reads found (A-LMU-003).
#[derive(Debug, Clone, Copy)]
struct GateAudit {
    window_start: Instant,
    via_gate: u32,
    via_witness: u32,
}

impl GateAudit {
    fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            via_gate: 0,
            via_witness: 0,
        }
    }

    /// Records one new frame.
    fn record(&mut self, wake: Wake) {
        match wake {
            Wake::Gate => self.via_gate = self.via_gate.saturating_add(1),
            Wake::Witness => self.via_witness = self.via_witness.saturating_add(1),
            Wake::Attach | Wake::Poll => {}
        }
    }

    /// Closes the window once it is [`GATE_AUDIT_WINDOW`] old. Returns the reason to stop using the
    /// gate when it delivered fewer than half of at least [`GATE_AUDIT_MIN_FRAMES`] new frames.
    fn evaluate(&mut self, now: Instant) -> Option<String> {
        if now.duration_since(self.window_start) < GATE_AUDIT_WINDOW {
            return None;
        }
        let total = self.via_gate.saturating_add(self.via_witness);
        let verdict = (total >= GATE_AUDIT_MIN_FRAMES
            && u64::from(self.via_gate) * 2 < u64::from(total))
        .then(|| {
            format!(
                "the frame gate signalled only {} of {total} new frames (A-LMU-003); polling with witness reads",
                self.via_gate
            )
        });
        *self = Self::new(now);
        verdict
    }
}

/// `true` if the SDK lock should be reported as stuck (A-LMU-052).
fn lock_stuck(unavailable_for: Option<Duration>, failed_attempts: u32, busy: i32) -> bool {
    busy != 0
        && failed_attempts >= LOCK_STUCK_MIN_ATTEMPTS
        && unavailable_for.is_some_and(|d| d > LOCK_STUCK_AFTER)
}

/// One attachment to `LMU_Data`.
struct Connection {
    view: SharedView,
    gate: Option<FrameGate>,
    lock: Option<SharedLock>,
    sync_mode: SyncMode,
    sync_reason: Option<String>,
    lock_mode: LockMode,
    /// The gate events did not exist at attach time: retry opening them.
    gate_retry: bool,
    /// The lock objects did not exist at attach time: retry opening them.
    lock_retry: bool,
    next_object_retry: Instant,
    process: Option<ProcessWatch>,
    mapping_size: usize,
    connected_at: Instant,
    last_new_frame: Instant,
    last_poll: Option<Instant>,
    last_wake: Wake,
    audit: GateAudit,
    next_process_lookup: Instant,
    consecutive_rejections: u32,
    consecutive_duplicates: u32,
    lock_unavailable_since: Option<Instant>,
    lock_failed_attempts: u32,
    lock_stuck_reported: bool,
    last_paths: Option<Instant>,
}

impl Connection {
    /// Forgets a lock-unavailability episode (the lock was acquired, or `busy` reads 0).
    fn lock_available(&mut self) {
        self.lock_unavailable_since = None;
        self.lock_failed_attempts = 0;
        self.lock_stuck_reported = false;
    }
}

/// Outcome of waiting for the next frame.
enum FrameWait {
    /// Copy now.
    Ready,
    /// Nothing to copy yet (timeout, poll interval, or a failed wait).
    NotYet,
}

/// Outcome of one copy under the SDK lock.
enum LockedCopy {
    /// The lock was held for the copy. `consistent` is `Ok(false)` when the witness check of a
    /// polled copy saw a concurrent write.
    Copied {
        consistent: Result<bool, OutOfBounds>,
        wait_ns: u64,
        hold_ns: u64,
    },
    /// Spins and the bounded wait were exhausted.
    TimedOut,
    /// The wait on the lock event failed.
    Failed(io::Error),
}

/// The lock critical section: acquire, copy (witness-verified when `verify`), release. The guard
/// is dropped only after the copy returned, so the copy can never run outside the lock (tested
/// with an instrumented [`ByteSource`]).
fn copy_locked<S: ByteSource + ?Sized>(
    lock: &SharedLock,
    src: &S,
    plan: CopyPlan,
    work: &mut RawBlocks,
    verify: bool,
    config: &ReaderConfig,
) -> LockedCopy {
    let wait_started = Instant::now();
    match lock.acquire(config.lock_spins, config.lock_wait) {
        LockAttempt::Acquired(guard) => {
            // Critical section: bounded memcpy into preallocated buffers only. One verified attempt
            // at most, so the hold time stays bounded (a torn frame is retried at the next poll).
            let acquired = Instant::now();
            let consistent = if verify {
                copy_frame_verified(src, plan, work, 1)
                    .map(|v| matches!(v, VerifiedCopy::Consistent { .. }))
            } else {
                copy_frame(src, plan, work).map(|()| true)
            };
            drop(guard);
            LockedCopy::Copied {
                consistent,
                wait_ns: duration_ns(acquired.duration_since(wait_started)),
                hold_ns: duration_ns(acquired.elapsed()),
            }
        }
        LockAttempt::TimedOut => LockedCopy::TimedOut,
        LockAttempt::Failed(e) => LockedCopy::Failed(e),
    }
}

/// Opens the frame gate: `(gate, mode, retry later, reason when polling)`.
fn open_gate(names: ObjectNames) -> (Option<FrameGate>, SyncMode, bool, Option<String>) {
    match FrameGate::open(names.hold_event, names.data_event) {
        Ok(Some(gate)) => (Some(gate), SyncMode::FrameGate, false, None),
        Ok(None) => (
            None,
            SyncMode::Polling,
            true,
            Some("frame-gate events do not exist (yet); retrying".to_owned()),
        ),
        Err(e) => {
            tracing::warn!(component = COMPONENT, error = %e, "frame gate unavailable; polling instead");
            (
                None,
                SyncMode::Polling,
                false,
                Some(format!("frame gate cannot be opened: {e}")),
            )
        }
    }
}

/// Opens the SDK lock: `(lock, mode, retry later)`. Missing objects are retried; access errors are
/// not (they persist until LMU or this program restarts).
fn open_lock(names: ObjectNames) -> (Option<SharedLock>, LockMode, bool) {
    let lock_free = |reason: String| LockMode::LockFree { reason };
    match SharedLock::open(names.lock_mapping, names.lock_event) {
        Ok(LockOpen::Ready(lock)) => (Some(lock), LockMode::SdkLock, false),
        Ok(LockOpen::MappingMissing) => (
            None,
            lock_free(format!(
                "lock mapping {} does not exist (yet)",
                names.lock_mapping
            )),
            true,
        ),
        Ok(LockOpen::EventMissing) => (
            None,
            lock_free(format!(
                "lock event {} does not exist (yet)",
                names.lock_event
            )),
            true,
        ),
        Err(e) => {
            let hint = if e.is_access_denied() {
                " (run LMU and this program at the same elevation, A-LMU-011)"
            } else {
                ""
            };
            (None, lock_free(format!("{e}{hint}")), false)
        }
    }
}

/// Rate limiter for repeated warnings.
#[derive(Debug, Default)]
struct Throttle(Option<Instant>);

impl Throttle {
    fn allow(&mut self, now: Instant) -> bool {
        let ok = self
            .0
            .is_none_or(|last| now.duration_since(last) >= WARN_INTERVAL);
        if ok {
            self.0 = Some(now);
        }
        ok
    }
}

/// How the reader finds the LMU process (replaceable in tests).
type ProcessLookup = fn() -> Result<Option<u32>, OsError>;

/// Blocking reader of LMU's `LMU_Data` shared memory. Not `Sync`; owned by one thread.
pub struct LmuReader {
    config: ReaderConfig,
    state: ReaderState,
    conn: Option<Connection>,
    work: RawBlocks,
    processor: FrameProcessor,
    stats: ReaderStats,
    connection_id: u64,
    next_attempt: Instant,
    pending: Option<RawSnapshot>,
    incompatible: Option<Box<CompatibilityReport>>,
    frozen: Option<Witness>,
    lmu_pid: Option<u32>,
    note: Option<String>,
    last_error: Option<String>,
    warn_lock: Throttle,
    warn_gate: Throttle,
    process_lookup: ProcessLookup,
}

impl LmuReader {
    /// Creates the reader and makes the first attach attempt. LMU not running is not an error: the
    /// reader starts in [`ReaderState::WaitingForLmu`] and keeps retrying inside
    /// [`LmuReader::next_snapshot`]. Every configuration value is clamped
    /// ([`ReaderConfig::clamped`]).
    ///
    /// # Errors
    /// [`LmuError::UnsupportedPlatform`] when not running on Windows.
    pub fn open(config: ReaderConfig) -> Result<Self, LmuError> {
        Self::open_with(config, sys::find_lmu_process)
    }

    fn open_with(config: ReaderConfig, process_lookup: ProcessLookup) -> Result<Self, LmuError> {
        if !cfg!(windows) {
            return Err(LmuError::UnsupportedPlatform);
        }
        let mut reader = Self::new(config);
        reader.process_lookup = process_lookup;
        reader.next_attempt = instant_after(Instant::now(), reader.config.retry_interval);
        match reader.try_connect() {
            Ok(first) => reader.pending = first,
            // Recorded in the state / diagnostics and reported again by `next_snapshot`.
            Err(LmuError::IncompatibleInterface { .. }) => {}
            Err(e) => {
                tracing::warn!(component = COMPONENT, error = %e, "first attach attempt failed; retrying")
            }
        }
        Ok(reader)
    }

    fn new(config: ReaderConfig) -> Self {
        Self {
            config: config.clamped(),
            state: ReaderState::WaitingForLmu,
            conn: None,
            work: RawBlocks::new(),
            processor: FrameProcessor::new(),
            stats: ReaderStats::default(),
            connection_id: 0,
            next_attempt: Instant::now(),
            pending: None,
            incompatible: None,
            frozen: None,
            lmu_pid: None,
            note: None,
            last_error: None,
            warn_lock: Throttle::default(),
            warn_gate: Throttle::default(),
            process_lookup: sys::find_lmu_process,
        }
    }

    /// Current state.
    pub fn state(&self) -> ReaderState {
        self.state
    }

    /// Counters, rates and timing histograms.
    pub fn stats(&self) -> &ReaderStats {
        &self.stats
    }

    /// The configuration in effect (clamped).
    pub fn config(&self) -> &ReaderConfig {
        &self.config
    }

    /// The raw blocks behind the most recently returned snapshot (e.g. for `.lmuraw` captures; use
    /// [`RawBlocks::to_owned_blocks`] to queue them).
    pub fn last_raw_blocks(&self) -> Option<&RawBlocks> {
        self.processor.last_blocks()
    }

    /// Compatibility report of the current (or, while incompatible, the failed) connection.
    pub fn compatibility(&self) -> CompatibilityReport {
        if self.conn.is_none()
            && let Some(report) = &self.incompatible
        {
            return (**report).clone();
        }
        self.current_report()
    }

    /// The processor's report plus the modes of the current connection.
    fn current_report(&self) -> CompatibilityReport {
        let mut report = self.processor.report();
        if let Some(conn) = &self.conn {
            report.sync_mode = Some(conn.sync_mode);
            report.lock_mode = Some(conn.lock_mode.clone());
            report.mapping_size = Some(conn.mapping_size);
        }
        report
    }

    /// Point-in-time diagnostics.
    pub fn diagnostics(&self) -> ReaderDiagnostics {
        let now = Instant::now();
        let conn = self.conn.as_ref();
        let lock = conn.and_then(|c| c.lock.as_ref());
        let busy = lock.map(SharedLock::busy_word);
        // A free lock word ends an unavailability episode even if no acquisition was attempted
        // since (gate silent in menus or pause).
        let unavailable = conn
            .filter(|_| busy != Some(0))
            .and_then(|c| c.lock_unavailable_since)
            .map(|t| now.duration_since(t));
        ReaderDiagnostics {
            state: self.state,
            connection_id: self.connection_id,
            lmu_pid: conn
                .and_then(|c| c.process.as_ref().map(ProcessWatch::pid))
                .or(self.lmu_pid),
            mapping_size: conn.map(|c| c.mapping_size),
            sync_mode: conn.map(|c| c.sync_mode),
            sync_reason: conn.and_then(|c| c.sync_reason.clone()),
            lock_mode: conn.map(|c| c.lock_mode.clone()),
            connected_for_ns: conn.map(|c| duration_ns(now.duration_since(c.connected_at))),
            since_new_frame_ns: conn.map(|c| duration_ns(now.duration_since(c.last_new_frame))),
            lock_busy_word: busy,
            lock_waiters_word: lock.map(SharedLock::waiters_word),
            lock_unavailable_ns: unavailable.map(duration_ns),
            lock_stuck: lock_stuck(
                unavailable,
                conn.map_or(0, |c| c.lock_failed_attempts),
                busy.unwrap_or(0),
            ),
            lock_set_event_failures: lock.map_or(0, SharedLock::set_event_failures),
            note: self.note.clone(),
            last_error: self.last_error.clone(),
        }
    }

    /// Detaches from LMU (closes every handle). The next call to [`LmuReader::next_snapshot`]
    /// attaches again.
    pub fn close(&mut self) {
        if self.conn.is_some() {
            self.disconnect("closed by the application", false);
        }
    }

    /// Blocks until a new frame is available or `timeout` expires.
    ///
    /// * `Ok(Some(snapshot))`: a frame that differs from the previous one (duplicates are counted in
    ///   [`ReaderStats::duplicate_frames`] and skipped).
    /// * `Ok(None)`: no new frame within `timeout` (LMU absent, idle, paused, or between frames).
    /// * `Err(IncompatibleInterface)`: returned on every call while the structural checks fail; the
    ///   reader re-checks every `retry_interval` and recovers by itself.
    /// * `Err(Os)`: an attach attempt in this call failed unexpectedly (e.g. access denied).
    ///
    /// # Errors
    /// See above.
    pub fn next_snapshot(&mut self, timeout: Duration) -> Result<Option<RawSnapshot>, LmuError> {
        if let Some(first) = self.pending.take() {
            return Ok(Some(first));
        }
        let deadline = instant_after(Instant::now(), timeout.min(MAX_CALL_TIMEOUT));
        loop {
            let now = Instant::now();
            self.stats.tick(self.config.clock.ns_at(now));

            if self.conn.is_none() {
                if now >= self.next_attempt {
                    self.next_attempt = instant_after(now, self.config.retry_interval);
                    if let Some(snapshot) = self.try_connect()? {
                        return Ok(Some(snapshot));
                    }
                    if self.conn.is_some() {
                        continue;
                    }
                }
                let now = Instant::now();
                if now >= deadline {
                    return match self.state {
                        ReaderState::Incompatible => Err(self.incompatible_error()),
                        _ => Ok(None),
                    };
                }
                sleep(
                    deadline
                        .min(self.next_attempt)
                        .saturating_duration_since(now),
                );
                continue;
            }

            if let Some(reason) = self.liveness_problem(now) {
                self.disconnect(&reason, true);
                continue;
            }
            self.retry_missing_objects(now);
            let remaining = deadline.saturating_duration_since(now);
            if remaining.is_zero() {
                return Ok(None);
            }
            if let FrameWait::NotYet = self.wait_for_frame(remaining) {
                continue;
            }
            if let Some(snapshot) = self.copy_and_process()? {
                return Ok(Some(snapshot));
            }
        }
    }

    fn incompatible_error(&self) -> LmuError {
        let report = self.compatibility();
        LmuError::IncompatibleInterface {
            message: report.incompatibility_message(),
            report: Box::new(report),
        }
    }

    fn set_waiting(&mut self, note: &str) {
        self.state = ReaderState::WaitingForLmu;
        // A failed report from an earlier attach no longer describes anything that is running.
        self.incompatible = None;
        if self.note.as_deref() != Some(note) {
            tracing::info!(component = COMPONENT, note, "waiting for LMU");
            self.note = Some(note.to_owned());
        }
    }

    /// One attach attempt. On success the first frame is copied immediately (no gate wait) so the
    /// structural checks run before anything is returned.
    fn try_connect(&mut self) -> Result<Option<RawSnapshot>, LmuError> {
        let lmu_pid = match (self.process_lookup)() {
            Ok(pid) => pid,
            Err(e) => {
                tracing::debug!(component = COMPONENT, error = %e, "process lookup failed");
                None
            }
        };
        self.lmu_pid = lmu_pid;
        let names = self.config.object_names;
        let view = match SharedView::open(names.data_mapping) {
            Ok(Some(view)) => view,
            Ok(None) => {
                self.set_waiting(match lmu_pid {
                    None => "LMU is not running (LMU_Data does not exist)",
                    Some(_) => "LMU is running but LMU_Data does not exist yet (starting up?)",
                });
                return Ok(None);
            }
            Err(e) => return Err(self.connect_failed(e)),
        };

        // A section kept alive by another program after LMU exited still holds the frozen frame we
        // detached from: never present it as live (A-LMU-056), even when an LMU process exists
        // again (it may be starting and not have published yet). Cleared by the first new frame.
        if let Some(frozen) = self.frozen
            && read_witness(&view).ok() == Some(frozen)
        {
            self.set_waiting(match lmu_pid {
                None => {
                    "LMU is not running (LMU_Data still exists with frozen data, kept open by another program)"
                }
                Some(_) => {
                    "LMU is starting: LMU_Data still holds the previous session's last frame (not updated yet)"
                }
            });
            return Ok(None);
        }

        let mapping_size = view.region_size();
        self.processor.reset();
        if mapping_size < abs::MAPPING_END {
            // Nothing is copied from a view smaller than the layout; the view is closed on return.
            self.processor.record_mapping_too_small(mapping_size);
            return Err(self.enter_incompatible());
        }

        let (gate, sync_mode, gate_retry, sync_reason) = open_gate(names);
        // The lock is opened only after the data mapping exists, so LMU had the chance to create it.
        let (lock, lock_mode, lock_retry) = open_lock(names);
        let process = lmu_pid.and_then(|pid| match ProcessWatch::open(pid) {
            Ok(watch) => Some(watch),
            Err(e) => {
                tracing::debug!(component = COMPONENT, error = %e, "cannot watch the LMU process; using staleness only");
                None
            }
        });

        let now = Instant::now();
        self.connection_id += 1;
        self.stats.connects += 1;
        self.note = None;
        self.last_error = None;
        self.state = ReaderState::Connected;
        tracing::info!(
            component = COMPONENT,
            connection = self.connection_id,
            mapping_size,
            sync = sync_mode.as_str(),
            lock = %lock_mode.label(),
            pid = ?lmu_pid,
            "attached to LMU_Data (read-only)"
        );
        self.conn = Some(Connection {
            view,
            gate,
            lock,
            sync_mode,
            sync_reason,
            lock_mode,
            gate_retry,
            lock_retry,
            next_object_retry: instant_after(now, self.config.retry_interval),
            process,
            mapping_size,
            connected_at: now,
            last_new_frame: now,
            last_poll: None,
            last_wake: Wake::Attach,
            audit: GateAudit::new(now),
            next_process_lookup: now,
            consecutive_rejections: 0,
            consecutive_duplicates: 0,
            lock_unavailable_since: None,
            lock_failed_attempts: 0,
            lock_stuck_reported: false,
            last_paths: None,
        });

        // Initial structural check: a few immediate copies until one is checked.
        for _ in 0..self.config.incompatible_after {
            if let Some(snapshot) = self.copy_and_process()? {
                return Ok(Some(snapshot));
            }
            if self.processor.compat().frames_checked() > 0
                && !self.processor.compat().structural_failure()
            {
                break;
            }
            sleep(self.config.poll_interval);
        }
        Ok(None)
    }

    fn connect_failed(&mut self, e: OsError) -> LmuError {
        self.stats.connect_failures += 1;
        let hint = if e.is_access_denied() {
            "; LMU probably runs at a different elevation: run both elevated or both not (A-LMU-011)"
        } else {
            ""
        };
        let text = format!("{e}{hint}");
        if self.last_error.as_deref() != Some(text.as_str()) {
            tracing::warn!(component = COMPONENT, error = %text, "cannot attach to LMU_Data");
            self.last_error = Some(text);
        }
        self.set_waiting("LMU_Data exists but cannot be opened");
        e.into_lmu()
    }

    /// Stops decoding: closes every handle and reports the failed structural checks.
    fn enter_incompatible(&mut self) -> LmuError {
        let report = self.current_report();
        let message = report.incompatibility_message();
        if self.state != ReaderState::Incompatible {
            tracing::error!(component = COMPONENT, %message, "LMU shared-memory interface incompatible; decoding stopped");
        }
        if self.conn.take().is_some() {
            self.stats.disconnects += 1;
        }
        self.state = ReaderState::Incompatible;
        self.note = Some("structural compatibility checks failed; nothing is decoded".to_owned());
        self.next_attempt = instant_after(Instant::now(), self.config.retry_interval);
        self.incompatible = Some(Box::new(report.clone()));
        LmuError::IncompatibleInterface {
            message,
            report: Box::new(report),
        }
    }

    fn disconnect(&mut self, reason: &str, remember_frozen: bool) {
        if remember_frozen {
            self.frozen = self.processor.last_blocks().and_then(witness_of);
        }
        if self.conn.take().is_some() {
            self.stats.disconnects += 1;
        }
        self.processor.reset();
        self.state = ReaderState::WaitingForLmu;
        self.next_attempt = Instant::now();
        tracing::info!(component = COMPONENT, reason, "detached from LMU_Data");
    }

    /// Decides whether the connection is dead (LMU exited or frozen data).
    fn liveness_problem(&mut self, now: Instant) -> Option<String> {
        let stale_after = self.config.stale_after;
        let retry = self.config.retry_interval;
        let lookup = self.process_lookup;
        let conn = self.conn.as_mut()?;
        if conn.process.as_ref().is_some_and(ProcessWatch::has_exited) {
            return Some("LMU process exited".to_owned());
        }
        let idle = now.duration_since(conn.last_new_frame);
        if idle < stale_after {
            return None;
        }
        if self.state == ReaderState::Connected {
            tracing::info!(
                component = COMPONENT,
                idle_ms = idle.as_millis(),
                "no new LMU frame; stalled (menus, pause or LMU exit?)"
            );
            self.state = ReaderState::Stalled;
        }
        if conn.process.is_some() || now < conn.next_process_lookup {
            // The watched process is alive (menus, pause, loading): keep the handles.
            return None;
        }
        conn.next_process_lookup = instant_after(now, retry);
        match lookup() {
            Ok(Some(pid)) => {
                self.lmu_pid = Some(pid);
                conn.process = ProcessWatch::open(pid).ok();
                None
            }
            Ok(None) => Some(format!(
                "no new frame for {} ms and LMU is not running",
                idle.as_millis()
            )),
            Err(e) => Some(format!(
                "no new frame for {} ms and the process lookup failed ({e})",
                idle.as_millis()
            )),
        }
    }

    /// Opens a frame gate or lock that did not exist at attach time and upgrades the connection in
    /// place (LMU may create them after `LMU_Data`).
    fn retry_missing_objects(&mut self, now: Instant) {
        let names = self.config.object_names;
        let retry = self.config.retry_interval;
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        if !(conn.gate_retry || conn.lock_retry) || now < conn.next_object_retry {
            return;
        }
        conn.next_object_retry = instant_after(now, retry);
        if conn.gate_retry {
            let (gate, sync_mode, retry_again, reason) = open_gate(names);
            if gate.is_some() {
                tracing::info!(
                    component = COMPONENT,
                    "frame-gate events appeared; switching from polling to the frame gate"
                );
                self.stats.object_upgrades += 1;
                conn.gate = gate;
                conn.sync_mode = sync_mode;
                conn.audit = GateAudit::new(now);
            }
            conn.gate_retry = retry_again;
            conn.sync_reason = reason;
        }
        if conn.lock_retry {
            let (lock, lock_mode, retry_again) = open_lock(names);
            if lock.is_some() {
                tracing::info!(
                    component = COMPONENT,
                    "SDK lock objects appeared; copies are now made under the lock"
                );
                self.stats.object_upgrades += 1;
                conn.lock = lock;
                conn.lock_available();
            }
            conn.lock_mode = lock_mode;
            conn.lock_retry = retry_again;
        }
    }

    fn wait_for_frame(&mut self, remaining: Duration) -> FrameWait {
        let gate_timeout = self.config.gate_timeout;
        let poll_interval = self.config.poll_interval;
        let clock = self.config.clock;
        let Some(conn) = self.conn.as_mut() else {
            return FrameWait::NotYet;
        };
        let Some(gate) = &conn.gate else {
            // Fallback (no gate): copy every poll interval; the copy itself verifies consistency.
            let now = Instant::now();
            if let Some(last) = conn.last_poll {
                let due = instant_after(last, poll_interval);
                if now < due {
                    sleep(due.saturating_duration_since(now).min(remaining));
                    if Instant::now() < due {
                        return FrameWait::NotYet;
                    }
                }
            }
            conn.last_poll = Some(Instant::now());
            conn.last_wake = Wake::Poll;
            self.stats.polls += 1;
            return FrameWait::Ready;
        };
        if conn.consecutive_duplicates > 0 {
            // The gate fired but nothing changed: pause briefly before waiting again, in case the
            // events stay signalled (their reset mode is UNVERIFIED, research §5.1).
            sleep(DUPLICATE_BACKOFF.min(remaining));
        }
        let started = Instant::now();
        let data_timeout = remaining.min(gate_timeout);
        match gate.wait(data_timeout.min(GATE_WITNESS_INTERVAL), data_timeout) {
            GateWait::Signaled => {
                let now = Instant::now();
                self.stats.gate_wakeups += 1;
                self.stats.gate_rate.record(clock.ns_at(now));
                self.stats
                    .gate_wait
                    .record(duration_ns(now.duration_since(started)));
                conn.last_wake = Wake::Gate;
                FrameWait::Ready
            }
            GateWait::TimedOut => {
                self.stats.gate_timeouts += 1;
                if conn.lock.as_ref().is_some_and(|l| l.busy_word() == 0) {
                    conn.lock_available();
                }
                // A-LMU-003: did LMU publish without signalling? A cheap, lock-free witness read
                // (racy bytes only decide whether to copy; the copy itself takes the lock).
                let current = read_witness(&conn.view).ok();
                let last = self.processor.last_blocks().and_then(witness_of);
                let changed = match (current, last) {
                    (Some(current), Some(last)) => current != last,
                    (Some(_), None) => true,
                    (None, _) => false,
                };
                if changed {
                    conn.last_wake = Wake::Witness;
                    FrameWait::Ready
                } else {
                    FrameWait::NotYet
                }
            }
            GateWait::Failed(e) => {
                self.stats.gate_failures += 1;
                if self.warn_gate.allow(Instant::now()) {
                    tracing::warn!(component = COMPONENT, error = %e, "frame gate wait failed");
                }
                sleep(poll_interval.min(remaining));
                FrameWait::NotYet
            }
        }
    }

    /// Copies one frame (under the lock, or witness-verified without it) and processes it.
    fn copy_and_process(&mut self) -> Result<Option<RawSnapshot>, LmuError> {
        let config = self.config;
        let Some(conn) = self.conn.as_mut() else {
            return Ok(None);
        };
        let now = Instant::now();
        let plan = CopyPlan {
            paths: config.copy_paths
                && conn
                    .last_paths
                    .is_none_or(|t| now.duration_since(t) >= config.paths_refresh),
            scoring_stream: config.copy_scoring_stream,
        };
        if plan.scoring_stream {
            // Allocate outside the lock (no-op once allocated).
            self.work.enable_scoring_stream();
        }

        let consistency = match &conn.lock {
            Some(lock) => {
                // Without the frame gate (older builds, or a degraded gate) the copy is also
                // witness-checked, in case that build's writer does not take the lock.
                let verify = conn.gate.is_none();
                match copy_locked(lock, &conn.view, plan, &mut self.work, verify, &config) {
                    LockedCopy::Copied {
                        consistent,
                        wait_ns,
                        hold_ns,
                    } => {
                        self.stats.lock_acquired += 1;
                        self.stats.lock_wait.record(wait_ns);
                        self.stats.record_lock_hold(hold_ns);
                        conn.lock_available();
                        if hold_ns > LOCK_HOLD_WARN_NS && self.warn_lock.allow(Instant::now()) {
                            tracing::warn!(
                                component = COMPONENT,
                                held_us = hold_ns / 1_000,
                                "LMU lock held longer than 1 ms"
                            );
                        }
                        match consistent {
                            Ok(true) => {}
                            Ok(false) => {
                                self.stats.torn_copies += 1;
                                return Ok(None);
                            }
                            Err(e) => {
                                self.stats.copy_errors += 1;
                                return Err(e.into());
                            }
                        }
                        Consistency::Locked
                    }
                    LockedCopy::TimedOut => {
                        self.stats.lock_timeouts += 1;
                        conn.lock_failed_attempts = conn.lock_failed_attempts.saturating_add(1);
                        let since = *conn.lock_unavailable_since.get_or_insert(now);
                        let busy = lock.busy_word();
                        if !conn.lock_stuck_reported
                            && lock_stuck(
                                Some(now.duration_since(since)),
                                conn.lock_failed_attempts,
                                busy,
                            )
                        {
                            conn.lock_stuck_reported = true;
                            tracing::warn!(
                                component = COMPONENT,
                                busy,
                                "LMU shared-memory lock is stuck (held by another program); restart LMU. It is never reset by us (A-LMU-052)"
                            );
                        }
                        return Ok(None);
                    }
                    LockedCopy::Failed(e) => {
                        self.stats.lock_failures += 1;
                        if self.warn_lock.allow(Instant::now()) {
                            tracing::warn!(component = COMPONENT, error = %e, "waiting for the LMU lock failed; frame skipped");
                        }
                        return Ok(None);
                    }
                }
            }
            None => match copy_frame_verified(
                &conn.view,
                plan,
                &mut self.work,
                config.lock_free_attempts,
            ) {
                Ok(VerifiedCopy::Consistent { attempts }) => {
                    self.stats.lock_free_copies += 1;
                    self.stats.copy_retries += u64::from(attempts.saturating_sub(1));
                    Consistency::VerifiedLockFree { attempts }
                }
                Ok(VerifiedCopy::Torn { attempts }) => {
                    self.stats.torn_copies += 1;
                    self.stats.copy_retries += u64::from(attempts.saturating_sub(1));
                    return Ok(None);
                }
                Err(e) => {
                    self.stats.copy_errors += 1;
                    return Err(e.into());
                }
            },
        };

        let capture_mono_ns = config.clock.now_ns();
        self.stats.copies += 1;
        let meta = FrameMeta {
            capture_mono_ns,
            connection_id: self.connection_id,
            source: FrameSource {
                sync: Some(conn.sync_mode),
                consistency,
            },
            mapping_size: Some(conn.mapping_size),
        };
        let outcome = match self
            .processor
            .process(&mut self.work, &meta, &mut self.stats)
        {
            Ok(outcome) => outcome,
            Err(e) => {
                self.stats.decode_errors += 1;
                return Err(e.into());
            }
        };
        match outcome {
            FrameOutcome::Snapshot(snapshot) => {
                let now = Instant::now();
                conn.consecutive_rejections = 0;
                conn.consecutive_duplicates = 0;
                conn.last_new_frame = now;
                if snapshot.paths.is_some() {
                    conn.last_paths = Some(now);
                }
                if conn.sync_mode == SyncMode::FrameGate {
                    match conn.last_wake {
                        Wake::Gate => self.stats.gate_new_frames += 1,
                        Wake::Witness => self.stats.gate_missed_updates += 1,
                        Wake::Attach | Wake::Poll => {}
                    }
                    conn.audit.record(conn.last_wake);
                    if let Some(reason) = conn.audit.evaluate(now) {
                        tracing::warn!(component = COMPONENT, %reason, "frame gate unreliable; switching to polling (degraded)");
                        self.stats.gate_degradations += 1;
                        conn.gate = None;
                        conn.gate_retry = false;
                        conn.sync_mode = SyncMode::Polling;
                        conn.sync_reason = Some(reason);
                        conn.last_poll = None;
                    }
                }
                if self.state != ReaderState::Connected {
                    tracing::info!(component = COMPONENT, "receiving LMU frames");
                    self.state = ReaderState::Connected;
                }
                if self.incompatible.take().is_some() {
                    tracing::info!(
                        component = COMPONENT,
                        "structural checks pass again; decoding resumed"
                    );
                }
                self.frozen = None;
                Ok(Some(*snapshot))
            }
            FrameOutcome::Duplicate => {
                conn.consecutive_duplicates = conn.consecutive_duplicates.saturating_add(1);
                Ok(None)
            }
            FrameOutcome::Rejected => {
                conn.consecutive_rejections += 1;
                if conn.consecutive_rejections >= config.incompatible_after {
                    return Err(self.enter_incompatible());
                }
                Ok(None)
            }
        }
    }
}

/// Sleeps unless `d` is zero.
fn sleep(d: Duration) {
    if !d.is_zero() {
        std::thread::sleep(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_is_clamped_to_the_safety_bounds() {
        let reader = LmuReader::new(ReaderConfig {
            lock_wait: Duration::from_secs(1),
            lock_free_attempts: 0,
            incompatible_after: 0,
            poll_interval: Duration::ZERO,
            gate_timeout: Duration::ZERO,
            retry_interval: Duration::MAX,
            stale_after: Duration::MAX,
            paths_refresh: Duration::ZERO,
            lock_spins: u32::MAX,
            ..ReaderConfig::default()
        });
        let c = reader.config();
        assert_eq!(c.lock_wait, MAX_LOCK_WAIT);
        assert_eq!(c.lock_free_attempts, 1);
        assert_eq!(c.incompatible_after, 1);
        assert_eq!(c.poll_interval, POLL_INTERVAL_RANGE.0);
        assert_eq!(c.gate_timeout, GATE_TIMEOUT_RANGE.0);
        assert_eq!(c.retry_interval, RETRY_INTERVAL_RANGE.1);
        assert_eq!(c.stale_after, STALE_AFTER_RANGE.1);
        assert_eq!(c.paths_refresh, PATHS_REFRESH_RANGE.0);
        assert_eq!(c.lock_spins, MAX_LOCK_SPINS);
        assert_eq!(reader.state(), ReaderState::WaitingForLmu);
        assert!(
            reader.compatibility().overall.is_none(),
            "nothing checked yet"
        );
        assert!(!reader.diagnostics().lock_stuck);
        // Duration::MAX no longer reaches an Instant addition (it used to panic in `open`).
        assert!(instant_after(Instant::now(), Duration::MAX) <= Instant::now());
    }

    #[test]
    fn throttle_limits_repeats() {
        let mut t = Throttle::default();
        let now = Instant::now();
        assert!(t.allow(now));
        assert!(!t.allow(now + Duration::from_secs(1)));
        assert!(t.allow(now + WARN_INTERVAL));
    }

    #[test]
    fn reader_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<LmuReader>();
    }

    #[test]
    fn lock_is_reported_stuck_only_while_busy_and_repeatedly_unavailable() {
        let long = Some(LOCK_STUCK_AFTER + Duration::from_millis(1));
        assert!(lock_stuck(long, LOCK_STUCK_MIN_ATTEMPTS, 1));
        assert!(
            !lock_stuck(long, LOCK_STUCK_MIN_ATTEMPTS, 0),
            "lock is free now"
        );
        assert!(!lock_stuck(long, 1, 1), "a single timeout is not stuck");
        assert!(!lock_stuck(Some(LOCK_STUCK_AFTER), 10, 1));
        assert!(!lock_stuck(None, 10, 1));
    }

    #[test]
    fn gate_audit_degrades_only_a_gate_that_misses_most_frames() {
        let start = Instant::now();
        let after_window = start + GATE_AUDIT_WINDOW;

        let mut audit = GateAudit::new(start);
        for _ in 0..5 {
            audit.record(Wake::Gate);
        }
        for _ in 0..45 {
            audit.record(Wake::Witness);
        }
        assert!(audit.evaluate(start).is_none(), "window not closed yet");
        let reason = audit
            .evaluate(after_window)
            .expect("gate delivered 5 of 50");
        assert!(reason.contains("5 of 50"), "{reason}");

        let mut audit = GateAudit::new(start);
        for _ in 0..30 {
            audit.record(Wake::Gate);
            audit.record(Wake::Witness);
        }
        assert!(audit.evaluate(after_window).is_none(), "half is enough");

        let mut audit = GateAudit::new(start);
        for _ in 0..(GATE_AUDIT_MIN_FRAMES - 1) {
            audit.record(Wake::Witness);
        }
        assert!(
            audit.evaluate(after_window).is_none(),
            "too few frames to judge"
        );
    }
}

/// End-to-end tests of the real Win32 path against fake objects under test-only names.
#[cfg(all(test, windows))]
mod win32_tests {
    use std::cell::Cell;
    use std::sync::Arc;

    use super::*;
    use crate::blocks::OutOfBounds;
    use crate::compat::{CheckId, Outcome, SupportLevel};
    use crate::offsets::{abs, telem_info};
    use crate::sys::fake::FakeLmu;
    use crate::testutil::LayoutBuilder;

    const WAIT: Duration = Duration::from_millis(500);

    fn config(fake: &FakeLmu) -> ReaderConfig {
        ReaderConfig {
            object_names: fake.names,
            ..ReaderConfig::default()
        }
    }

    /// Process lookup that never finds LMU (deterministic whether or not LMU runs here).
    fn no_lmu() -> Result<Option<u32>, OsError> {
        Ok(None)
    }

    /// Process lookup that "finds LMU" as this test process (alive, waitable with SYNCHRONIZE).
    fn lmu_is_this_process() -> Result<Option<u32>, OsError> {
        Ok(Some(std::process::id()))
    }

    fn open(config: ReaderConfig) -> LmuReader {
        LmuReader::open_with(config, no_lmu).expect("open")
    }

    fn player_et_offset() -> usize {
        LayoutBuilder::telem_base(0) + telem_info::ELAPSED_TIME.offset
    }

    fn next(reader: &mut LmuReader) -> Option<RawSnapshot> {
        reader.next_snapshot(WAIT).expect("no reader error")
    }

    #[test]
    fn reads_frames_through_the_gate_under_the_lock() {
        let fake = FakeLmu::new(
            "reader-gate-lock",
            LayoutBuilder::on_track_frame().bytes(),
            true,
            true,
        );
        let mut reader = open(config(&fake));
        assert_eq!(reader.state(), ReaderState::Connected);

        let first = next(&mut reader).expect("initial frame is copied on attach");
        assert_eq!(first.seq, 1);
        assert_eq!(first.player_et(), Some(120.0));
        assert_eq!(first.source.consistency, Consistency::Locked);
        assert_eq!(first.source.sync, Some(SyncMode::FrameGate));
        assert_eq!(first.vehicle_telemetry.len(), 2);

        fake.write(player_et_offset(), &120.01f64.to_le_bytes());
        fake.publish();
        let second = next(&mut reader).expect("published frame");
        assert!(second.telemetry_updated && !second.scoring_updated);
        assert_eq!(second.player_et(), Some(120.01));
        assert!(Arc::ptr_eq(&first.scoring, &second.scoring));

        fake.publish(); // nothing changed: a duplicate, not returned
        assert!(
            reader
                .next_snapshot(Duration::from_millis(300))
                .expect("ok")
                .is_none()
        );

        let stats = reader.stats();
        assert!(stats.lock_acquired >= 3, "{stats:?}");
        assert_eq!(stats.lock_hold.count(), stats.lock_acquired);
        assert!(stats.duplicate_frames >= 1);
        assert!(stats.gate_wakeups >= 2);
        assert_eq!(stats.gate_new_frames, 1);
        assert_eq!(fake.busy(), 0, "lock released after every copy");
        assert_eq!(fake.waiters(), 0);
        let diagnostics = reader.diagnostics();
        assert_eq!(diagnostics.lock_mode, Some(LockMode::SdkLock));
        assert_eq!(
            diagnostics.mapping_size.map(|n| n >= abs::MAPPING_END),
            Some(true)
        );
        let report = reader.compatibility();
        assert!(report.is_decodable());
        assert_eq!(report.sync_mode, Some(SyncMode::FrameGate));
        assert_eq!(
            report
                .checks
                .iter()
                .find(|c| c.id == CheckId::MappingSize)
                .and_then(|c| c.outcome),
            Some(Outcome::Pass),
            "a section of sizeof(SharedMemoryLayout) is within the expected region size"
        );
    }

    /// A source that records the fake's `busy` word on every read the copy makes.
    struct ObservingSource<'a> {
        bytes: &'a [u8],
        fake: &'a FakeLmu,
        reads: Cell<u32>,
        reads_without_lock: Cell<u32>,
    }

    impl ByteSource for ObservingSource<'_> {
        fn len(&self) -> usize {
            self.bytes.len()
        }

        fn copy_to(&self, offset: usize, dst: &mut [u8]) -> Result<(), OutOfBounds> {
            self.reads.set(self.reads.get() + 1);
            if self.fake.busy() != 1 {
                self.reads_without_lock
                    .set(self.reads_without_lock.get() + 1);
            }
            self.bytes.copy_to(offset, dst)
        }
    }

    #[test]
    fn every_read_of_the_copy_happens_while_the_lock_is_held() {
        let image = LayoutBuilder::on_track_frame();
        let fake = FakeLmu::new("reader-lock-order", image.bytes(), false, true);
        let LockOpen::Ready(lock) =
            SharedLock::open(fake.names.lock_mapping, fake.names.lock_event).expect("open lock")
        else {
            panic!("fake lock objects exist");
        };
        let config = ReaderConfig::default();
        for verify in [false, true] {
            let src = ObservingSource {
                bytes: image.bytes(),
                fake: &fake,
                reads: Cell::new(0),
                reads_without_lock: Cell::new(0),
            };
            let mut work = RawBlocks::new();
            let outcome = copy_locked(&lock, &src, CopyPlan::default(), &mut work, verify, &config);
            assert!(matches!(
                outcome,
                LockedCopy::Copied {
                    consistent: Ok(true),
                    ..
                }
            ));
            assert!(src.reads.get() >= 5, "verify={verify}");
            assert_eq!(
                src.reads_without_lock.get(),
                0,
                "verify={verify}: a read ran outside the lock"
            );
            assert_eq!(fake.busy(), 0, "released afterwards");
        }
    }

    #[test]
    fn a_lock_held_elsewhere_skips_frames_and_is_never_reset() {
        let fake = FakeLmu::new(
            "reader-lock-held",
            LayoutBuilder::on_track_frame().bytes(),
            true,
            true,
        );
        let mut reader = open(config(&fake));
        assert!(next(&mut reader).is_some());

        fake.set_busy(1);
        fake.write(player_et_offset(), &121.0f64.to_le_bytes());
        fake.publish();
        assert!(
            reader
                .next_snapshot(Duration::from_millis(300))
                .expect("ok")
                .is_none()
        );
        assert!(reader.stats().lock_timeouts >= 1);
        assert_eq!(fake.busy(), 1, "never reset by the reader");
        assert_eq!(fake.waiters(), 0, "waiter registration undone");
        assert!(reader.diagnostics().lock_unavailable_ns.is_some());

        fake.set_busy(0);
        fake.publish();
        let s = next(&mut reader).expect("frame after the lock is free again");
        assert_eq!(s.player_et(), Some(121.0));
    }

    #[test]
    fn a_transient_lock_timeout_is_cleared_without_a_new_acquisition() {
        let fake = FakeLmu::new(
            "reader-lock-transient",
            LayoutBuilder::on_track_frame().bytes(),
            true,
            true,
        );
        let mut reader = open(config(&fake));
        assert!(next(&mut reader).is_some());
        fake.set_busy(1);
        fake.publish(); // the gate fires, the copy cannot get the lock
        assert!(
            reader
                .next_snapshot(Duration::from_millis(50))
                .expect("ok")
                .is_none()
        );
        assert!(reader.diagnostics().lock_unavailable_ns.is_some());
        let lock_timeouts = reader.stats().lock_timeouts;
        assert!(lock_timeouts >= 1);

        // The other holder releases; then nothing is published (pause): no acquisition attempt.
        fake.set_busy(0);
        assert!(
            reader.diagnostics().lock_unavailable_ns.is_none(),
            "a free busy word ends the episode"
        );
        assert!(
            reader
                .next_snapshot(Duration::from_millis(100))
                .expect("ok")
                .is_none()
        );
        assert_eq!(
            reader.stats().lock_timeouts,
            lock_timeouts,
            "no new attempt"
        );
        let conn = reader.conn.as_ref().expect("connected");
        assert!(
            conn.lock_unavailable_since.is_none(),
            "cleared on a gate timeout"
        );
        assert_eq!(conn.lock_failed_attempts, 0);
        assert!(!reader.diagnostics().lock_stuck);
    }

    #[test]
    fn without_gate_and_lock_it_polls_and_verifies_lock_free() {
        let fake = FakeLmu::new(
            "reader-poll",
            LayoutBuilder::on_track_frame().bytes(),
            false,
            false,
        );
        let mut reader = open(config(&fake));
        let first = next(&mut reader).expect("initial frame");
        assert_eq!(first.source.sync, Some(SyncMode::Polling));
        assert_eq!(
            first.source.consistency,
            Consistency::VerifiedLockFree { attempts: 1 }
        );
        assert!(matches!(
            reader.diagnostics().lock_mode,
            Some(LockMode::LockFree { .. })
        ));

        fake.write(player_et_offset(), &122.0f64.to_le_bytes());
        let s = next(&mut reader).expect("polled frame");
        assert_eq!(s.player_et(), Some(122.0));
        assert!(reader.stats().polls >= 1);
    }

    #[test]
    fn a_gate_and_lock_created_after_attaching_upgrade_the_connection() {
        let mut fake = FakeLmu::new(
            "reader-upgrade",
            LayoutBuilder::on_track_frame().bytes(),
            false,
            false,
        );
        let mut reader = open(ReaderConfig {
            retry_interval: RETRY_INTERVAL_RANGE.0,
            ..config(&fake)
        });
        assert!(next(&mut reader).is_some());
        let d = reader.diagnostics();
        assert_eq!(d.sync_mode, Some(SyncMode::Polling));
        assert!(matches!(d.lock_mode, Some(LockMode::LockFree { .. })));

        fake.add_gate();
        fake.add_lock();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let _ = reader.next_snapshot(Duration::from_millis(60));
            let d = reader.diagnostics();
            if d.sync_mode == Some(SyncMode::FrameGate) && d.lock_mode == Some(LockMode::SdkLock) {
                break;
            }
        }
        let d = reader.diagnostics();
        assert_eq!(d.sync_mode, Some(SyncMode::FrameGate));
        assert_eq!(d.lock_mode, Some(LockMode::SdkLock));
        assert_eq!(d.sync_reason, None);
        assert_eq!(reader.stats().object_upgrades, 2);

        fake.write(player_et_offset(), &124.0f64.to_le_bytes());
        fake.publish();
        let s = next(&mut reader).expect("frame through the gate");
        assert_eq!(s.player_et(), Some(124.0));
        assert_eq!(s.source.consistency, Consistency::Locked);
        assert_eq!(s.source.sync, Some(SyncMode::FrameGate));
        assert_eq!(s.connection_id, 1, "upgraded in place, not re-attached");
    }

    #[test]
    fn a_publish_without_a_gate_signal_is_still_read() {
        let fake = FakeLmu::new(
            "reader-silent-gate",
            LayoutBuilder::on_track_frame().bytes(),
            true,
            true,
        );
        let mut reader = open(config(&fake));
        assert!(next(&mut reader).is_some());
        // LMU writes a new frame but the gate never fires (A-LMU-003).
        fake.write(player_et_offset(), &125.0f64.to_le_bytes());
        let s = next(&mut reader).expect("found by the witness read");
        assert_eq!(s.player_et(), Some(125.0));
        assert_eq!(s.source.consistency, Consistency::Locked);
        assert_eq!(reader.stats().gate_missed_updates, 1);
        assert_eq!(reader.stats().gate_new_frames, 0);
    }

    #[test]
    fn polling_with_the_lock_copies_under_the_lock() {
        let fake = FakeLmu::new(
            "reader-poll-lock",
            LayoutBuilder::on_track_frame().bytes(),
            false,
            true,
        );
        let mut reader = open(config(&fake));
        let first = next(&mut reader).expect("initial frame");
        assert_eq!(first.source.sync, Some(SyncMode::Polling));
        assert_eq!(first.source.consistency, Consistency::Locked);
        fake.write(player_et_offset(), &123.0f64.to_le_bytes());
        assert_eq!(next(&mut reader).and_then(|s| s.player_et()), Some(123.0));
        assert_eq!(fake.busy(), 0);
        assert_eq!(reader.stats().torn_copies, 0);
    }

    #[test]
    fn a_shifted_layout_stops_decoding() {
        let mut image = LayoutBuilder::on_track_frame();
        image.scoring_padding([0xAA, 0, 0, 0]);
        let fake = FakeLmu::new("reader-incompatible", image.bytes(), true, true);
        let mut reader = open(config(&fake));
        assert_eq!(reader.state(), ReaderState::Incompatible);
        let err = reader
            .next_snapshot(Duration::from_millis(50))
            .expect_err("must not decode");
        assert!(err.is_incompatible());
        assert!(
            err.to_string()
                .contains("padding bytes at offset 2180: detected [aa, 00, 00, 00]"),
            "{err}"
        );
        let report = reader.compatibility();
        assert_eq!(report.overall, Some(SupportLevel::Unsupported));
        assert_eq!(reader.stats().snapshots, 0, "nothing was decoded");
        assert_eq!(fake.busy(), 0);
    }

    #[test]
    fn garbage_in_the_player_slot_stops_decoding() {
        let mut image = LayoutBuilder::on_track_frame();
        let slot = LayoutBuilder::telem_base(0);
        image.set(slot, telem_info::DELTA_TIME, f64::NAN);
        image.set(slot, telem_info::ID, -77i32);
        image.set_raw(slot, telem_info::TRACK_NAME, &[0x1b, b'[', b'2', b'J']);
        let fake = FakeLmu::new("reader-garbage-slot", image.bytes(), true, true);
        let reader = open(config(&fake));
        assert_eq!(reader.state(), ReaderState::Incompatible);
        assert_eq!(reader.stats().snapshots, 0, "never delivered");
        assert!(
            reader
                .compatibility()
                .incompatibility_message()
                .contains("A-LMU-007")
        );
    }

    #[test]
    fn a_too_small_mapping_is_incompatible() {
        let fake = FakeLmu::new("reader-small", &[0u8; 4_096], true, true);
        let reader = open(config(&fake));
        assert_eq!(reader.state(), ReaderState::Incompatible);
        let report = reader.compatibility();
        let check = report
            .checks
            .iter()
            .find(|c| c.id == CheckId::MappingSize)
            .expect("check");
        assert_eq!(check.outcome, Some(Outcome::Fail));
        assert!(check.detected.contains("4096"), "{}", check.detected);
    }

    fn frozen_config(fake: &FakeLmu) -> ReaderConfig {
        ReaderConfig {
            stale_after: STALE_AFTER_RANGE.0,
            retry_interval: RETRY_INTERVAL_RANGE.0,
            gate_timeout: Duration::from_millis(20),
            ..config(fake)
        }
    }

    /// Lets the connection go stale with no LMU process: detached, frozen frame remembered.
    fn detach_as_lmu_exited(reader: &mut LmuReader) {
        assert!(
            reader
                .next_snapshot(Duration::from_millis(600))
                .expect("ok")
                .is_none()
        );
        assert_eq!(reader.state(), ReaderState::WaitingForLmu);
        assert!(reader.stats().disconnects >= 1);
    }

    #[test]
    fn frozen_data_without_lmu_detaches_and_is_not_presented_as_live() {
        let fake = FakeLmu::new(
            "reader-frozen",
            LayoutBuilder::on_track_frame().bytes(),
            true,
            true,
        );
        let mut reader = open(frozen_config(&fake));
        assert!(next(&mut reader).is_some());
        detach_as_lmu_exited(&mut reader);
        assert!(
            reader
                .diagnostics()
                .note
                .is_some_and(|n| n.contains("frozen")),
            "{:?}",
            reader.diagnostics().note
        );

        // Data changes again (LMU back): attach and deliver.
        fake.write(player_et_offset(), &130.0f64.to_le_bytes());
        let s = next(&mut reader).expect("fresh frame after re-attach");
        assert_eq!(s.player_et(), Some(130.0));
        assert_eq!(reader.state(), ReaderState::Connected);
    }

    #[test]
    fn frozen_data_is_not_presented_as_live_while_a_new_lmu_starts() {
        let fake = FakeLmu::new(
            "reader-frozen-restart",
            LayoutBuilder::on_track_frame().bytes(),
            true,
            true,
        );
        let mut reader = open(frozen_config(&fake));
        assert!(next(&mut reader).is_some());
        detach_as_lmu_exited(&mut reader);
        let connects = reader.stats().connects;

        // A new LMU process exists but has not published yet: the mapping still holds the old
        // session's last frame. It must not be handed out as a fresh first sample.
        reader.process_lookup = lmu_is_this_process;
        assert!(
            reader
                .next_snapshot(Duration::from_millis(300))
                .expect("ok")
                .is_none()
        );
        assert_eq!(reader.state(), ReaderState::WaitingForLmu);
        assert_eq!(reader.stats().connects, connects, "not attached");
        assert!(
            reader
                .diagnostics()
                .note
                .is_some_and(|n| n.contains("starting")),
            "{:?}",
            reader.diagnostics().note
        );

        // The new LMU publishes: attach and deliver.
        fake.write(player_et_offset(), &0.5f64.to_le_bytes());
        let s = next(&mut reader).expect("first frame of the new session");
        assert_eq!(s.player_et(), Some(0.5));
        assert_eq!(reader.diagnostics().lmu_pid, Some(std::process::id()));
    }
}
