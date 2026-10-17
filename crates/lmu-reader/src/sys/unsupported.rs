//! Non-Windows stand-in: LMU's kernel objects never exist, so nothing here can be constructed.
//! [`crate::LmuReader::open`] reports [`crate::LmuError::UnsupportedPlatform`] before reaching it.

use std::convert::Infallible;
use std::time::Duration;

use super::{GateWait, LockAttempt, LockOpen, OsError};
use crate::blocks::{ByteSource, OutOfBounds};

/// Never constructed on this platform.
pub(crate) struct SharedView(Infallible);

impl SharedView {
    /// `LMU_Data` never exists here.
    pub(crate) fn open(_name: &'static str) -> Result<Option<Self>, OsError> {
        Ok(None)
    }

    pub(crate) fn region_size(&self) -> usize {
        match self.0 {}
    }
}

impl ByteSource for SharedView {
    fn len(&self) -> usize {
        match self.0 {}
    }

    fn copy_to(&self, _offset: usize, _dst: &mut [u8]) -> Result<(), OutOfBounds> {
        match self.0 {}
    }
}

/// Never constructed on this platform.
pub(crate) struct FrameGate(Infallible);

impl FrameGate {
    pub(crate) fn open(_hold: &'static str, _data: &'static str) -> Result<Option<Self>, OsError> {
        Ok(None)
    }

    pub(crate) fn wait(&self, _hold_timeout: Duration, _data_timeout: Duration) -> GateWait {
        match self.0 {}
    }
}

/// Never constructed on this platform.
pub(crate) struct SharedLock(Infallible);

impl SharedLock {
    pub(crate) fn open(_mapping: &'static str, _event: &'static str) -> Result<LockOpen, OsError> {
        Ok(LockOpen::MappingMissing)
    }

    pub(crate) fn acquire(&self, _spins: u32, _wait: Duration) -> LockAttempt<'_> {
        match self.0 {}
    }

    pub(crate) fn busy_word(&self) -> i32 {
        match self.0 {}
    }

    pub(crate) fn waiters_word(&self) -> i32 {
        match self.0 {}
    }

    pub(crate) fn set_event_failures(&self) -> u64 {
        match self.0 {}
    }
}

/// Never constructed on this platform.
pub(crate) struct LockGuard<'a>(Infallible, std::marker::PhantomData<&'a ()>);

/// Never constructed on this platform.
pub(crate) struct ProcessWatch(Infallible);

impl ProcessWatch {
    pub(crate) fn open(_pid: u32) -> Result<Self, OsError> {
        Err(OsError {
            call: "OpenProcess",
            object: "Le Mans Ultimate.exe",
            source: std::io::Error::new(std::io::ErrorKind::Unsupported, "not Windows"),
        })
    }

    pub(crate) fn pid(&self) -> u32 {
        match self.0 {}
    }

    pub(crate) fn has_exited(&self) -> bool {
        match self.0 {}
    }
}

/// There is no LMU process on this platform.
pub(crate) fn find_lmu_process() -> Result<Option<u32>, OsError> {
    Ok(None)
}
