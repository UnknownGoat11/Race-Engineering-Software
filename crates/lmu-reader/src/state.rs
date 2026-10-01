//! Reader state and the synchronisation / consistency modes a frame was read with.

use std::fmt;

use serde::Serialize;

use crate::offsets::names;

/// Kernel object names the reader opens (session-local namespace, A-LMU-011).
///
/// Production code always uses [`ObjectNames::LMU`] (the default). Tests point the reader at objects
/// they created themselves, so the Win32 path can be exercised without LMU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ObjectNames {
    /// The data mapping (`LMU_Data`), opened with `FILE_MAP_READ`.
    pub data_mapping: &'static str,
    /// First frame-gate event (`LMU_Data_HoldEvent`), `SYNCHRONIZE` only.
    pub hold_event: &'static str,
    /// Second frame-gate event (`LMU_Data_DataEvent`), `SYNCHRONIZE` only.
    pub data_event: &'static str,
    /// The lock-word mapping (`LMU_SharedMemoryLockData`), opened only if it exists.
    pub lock_mapping: &'static str,
    /// The lock wake-up event (`LMU_SharedMemoryLockEvent`), opened only if it exists.
    pub lock_event: &'static str,
}

impl ObjectNames {
    /// LMU's object names (from the SDK header, see [`crate::offsets::names`]).
    pub const LMU: Self = Self {
        data_mapping: names::DATA_MAPPING,
        hold_event: names::HOLD_EVENT,
        data_event: names::DATA_EVENT,
        lock_mapping: names::LOCK_MAPPING,
        lock_event: names::LOCK_EVENT,
    };
}

impl Default for ObjectNames {
    fn default() -> Self {
        Self::LMU
    }
}

/// Connection state of an [`crate::LmuReader`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReaderState {
    /// `LMU_Data` does not exist (LMU not running) or could not be opened; retried every second.
    WaitingForLmu,
    /// Attached, structural checks pass, and new frames arrive.
    Connected,
    /// Attached, but no new frame for longer than the stale threshold (menus, pause, or LMU hanging).
    /// Snapshots resume as soon as data changes again.
    Stalled,
    /// Structural compatibility checks failed: nothing is decoded until a re-check passes (D-003).
    Incompatible,
}

impl ReaderState {
    /// Upper-case label used in diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WaitingForLmu => "WAITING_FOR_LMU",
            Self::Connected => "CONNECTED",
            Self::Stalled => "STALLED",
            Self::Incompatible => "INCOMPATIBLE",
        }
    }
}

impl fmt::Display for ReaderState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How the reader learns that a new frame was published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SyncMode {
    /// `LMU_Data_HoldEvent` then `LMU_Data_DataEvent` (LMU 1.4.2+ frame gate).
    FrameGate,
    /// The gate events are missing (pre-1.4.2 build?), or the gate delivered fewer than half of
    /// the frames the witness reads saw (A-LMU-003): ~10 ms polling. Reported as degraded.
    Polling,
}

impl SyncMode {
    /// Upper-case label used in diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FrameGate => "FRAME_GATE",
            Self::Polling => "POLLING (degraded)",
        }
    }
}

/// How copies are protected against concurrent writes by LMU.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LockMode {
    /// The SDK reader lock (`LMU_SharedMemoryLockData` + `LMU_SharedMemoryLockEvent`) is held for the
    /// duration of the copy only.
    SdkLock,
    /// The lock objects could not be opened; copies are verified with before/after witness reads
    /// (ET, vehicle count, telemetry header) instead.
    LockFree {
        /// Why the lock is not used.
        reason: String,
    },
}

impl LockMode {
    /// Short label used in diagnostics.
    pub fn label(&self) -> String {
        match self {
            Self::SdkLock => "SDK_LOCK".to_owned(),
            Self::LockFree { reason } => format!("LOCK_FREE double-copy ({reason})"),
        }
    }
}

/// How the bytes of one frame were made consistent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "kind")]
pub enum Consistency {
    /// Copied while holding the SDK lock.
    Locked,
    /// Copied without the lock; the witness values were identical before and after the copy.
    VerifiedLockFree {
        /// Copy attempts needed (1 = first try).
        attempts: u32,
    },
    /// Loaded from a `.lmuraw` capture file.
    Replayed {
        /// The capturing reader copied the frame under the SDK lock (`false`: witness-verified
        /// lock-free copy, i.e. degraded).
        locked: bool,
    },
}

/// Where a frame came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct FrameSource {
    /// Frame synchronisation in effect (`None` for replays of captures without that information).
    pub sync: Option<SyncMode>,
    /// Copy consistency.
    pub consistency: Consistency,
}
