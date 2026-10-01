//! Operating-system access to LMU's kernel objects. All `unsafe` code of the crate lives in the
//! Windows implementation below this module.
//!
//! D-018: LMU is strictly read-only. The only objects touched are the ones the SDK documents for
//! readers: `LMU_Data` (mapped `FILE_MAP_READ`), the two gate events (`SYNCHRONIZE` only), the
//! *existing* lock mapping and lock event (the owner-approved reader handshake: never created, never
//! reset), and optionally the LMU process (`SYNCHRONIZE` only, to notice its exit).

use std::io;

#[cfg(windows)]
mod win32;
#[cfg(all(test, windows))]
pub(crate) use self::win32::fake;
#[cfg(windows)]
pub(crate) use self::win32::{
    FrameGate, LockGuard, ProcessWatch, SharedLock, SharedView, find_lmu_process,
};

#[cfg(not(windows))]
mod unsupported;
#[cfg(not(windows))]
pub(crate) use self::unsupported::{
    FrameGate, LockGuard, ProcessWatch, SharedLock, SharedView, find_lmu_process,
};

use crate::error::LmuError;

/// Executable name of the game (ToolHelp process detection).
pub(crate) const LMU_EXE_NAME: &str = "Le Mans Ultimate.exe";

/// A failed OS call, with the call and object named.
#[derive(Debug)]
pub(crate) struct OsError {
    pub(crate) call: &'static str,
    pub(crate) object: &'static str,
    pub(crate) source: io::Error,
}

impl OsError {
    /// Converts into the public error type.
    pub(crate) fn into_lmu(self) -> LmuError {
        LmuError::Os {
            call: self.call,
            object: self.object,
            source: self.source,
        }
    }

    /// `true` for `ERROR_ACCESS_DENIED` (typically an elevation mismatch, A-LMU-011).
    pub(crate) fn is_access_denied(&self) -> bool {
        self.source.kind() == io::ErrorKind::PermissionDenied
    }
}

impl std::fmt::Display for OsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({}) failed: {}", self.call, self.object, self.source)
    }
}

/// Result of one frame-gate wait.
#[derive(Debug)]
pub(crate) enum GateWait {
    /// Hold, then Data were signalled: a new frame was published.
    Signaled,
    /// One of the waits timed out.
    TimedOut,
    /// `WaitForSingleObject` failed.
    Failed(io::Error),
}

/// Result of opening the SDK lock objects.
pub(crate) enum LockOpen {
    /// Both objects exist and are mapped / opened.
    Ready(SharedLock),
    /// `LMU_SharedMemoryLockData` does not exist (we never create it).
    MappingMissing,
    /// `LMU_SharedMemoryLockEvent` does not exist (we never create it).
    EventMissing,
}

/// Result of one bounded lock acquisition.
pub(crate) enum LockAttempt<'a> {
    /// The lock is held until the guard is dropped.
    Acquired(LockGuard<'a>),
    /// Spins and the bounded wait were exhausted; the frame is skipped.
    TimedOut,
    /// The wait on the lock event failed.
    Failed(io::Error),
}

/// `true` if a NUL-terminated UTF-16 executable name (as in `PROCESSENTRY32W`) is LMU's.
pub(crate) fn is_lmu_exe(raw: &[u16]) -> bool {
    let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    let name = String::from_utf16_lossy(raw.get(..len).unwrap_or(raw));
    name.eq_ignore_ascii_case(LMU_EXE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str) -> Vec<u16> {
        let mut v: Vec<u16> = s.encode_utf16().collect();
        v.extend([0, b'x'.into(), b'y'.into()]);
        v
    }

    #[test]
    fn exe_name_match_is_case_insensitive_and_stops_at_nul() {
        assert!(is_lmu_exe(&wide("Le Mans Ultimate.exe")));
        assert!(is_lmu_exe(&wide("LE MANS ULTIMATE.EXE")));
        assert!(!is_lmu_exe(&wide("Le Mans Ultimate.exe.bak")));
        assert!(!is_lmu_exe(&wide("LMU.exe")));
        let unterminated: Vec<u16> = LMU_EXE_NAME.encode_utf16().collect();
        assert!(is_lmu_exe(&unterminated));
    }
}
