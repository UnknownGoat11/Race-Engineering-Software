//! Win32 implementation. Every `unsafe` block of the crate is in this file, each with a SAFETY note.
//!
//! Access rights requested (D-018, A-LMU-046):
//! * `LMU_Data`: `FILE_MAP_READ` (handle and view).
//! * `LMU_Data_HoldEvent`, `LMU_Data_DataEvent`: `SYNCHRONIZE` (wait only; never set or reset).
//! * `LMU_SharedMemoryLockData`: opened only if it already exists (`OpenFileMappingA`), read + write
//!   because acquiring the reader lock is a compare-exchange on its `busy` word; never created,
//!   never reset.
//! * `LMU_SharedMemoryLockEvent`: `SYNCHRONIZE | EVENT_MODIFY_STATE` (wait, and `SetEvent` on release
//!   when other waiters are registered, exactly like the SDK's `Unlock`).
//! * The LMU process: `SYNCHRONIZE` only, to notice its exit.

use std::cell::Cell;
use std::ffi::CString;
use std::io;
use std::mem::size_of;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use ::windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use ::windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use ::windows::Win32::System::Memory::{
    FILE_MAP, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS,
    MapViewOfFile, OpenFileMappingA, OpenFileMappingW, UnmapViewOfFile, VirtualQuery,
};
use ::windows::Win32::System::Threading::{
    EVENT_MODIFY_STATE, OpenEventA, OpenProcess, PROCESS_SYNCHRONIZE, SYNCHRONIZATION_SYNCHRONIZE,
    SetEvent, WaitForSingleObject,
};
use ::windows::core::{HRESULT, HSTRING, PCSTR, PCWSTR};

use super::{GateWait, LockAttempt, LockOpen, OsError, is_lmu_exe};
use crate::blocks::{ByteSource, OutOfBounds};
use crate::offsets::{Field, lock_data};

/// Bits of an HRESULT that identify `FACILITY_WIN32` errors (`HRESULT_FROM_WIN32`).
const HRESULT_FACILITY_MASK: u32 = 0xFFFF_0000;
/// `HRESULT_FROM_WIN32` prefix.
const HRESULT_FACILITY_WIN32: u32 = 0x8007_0000;
/// The Win32 error code inside such an HRESULT.
const WIN32_CODE_MASK: u32 = 0x0000_FFFF;
/// Longest wait we ever request; `u32::MAX` would be `INFINITE`.
const MAX_WAIT_MS: u32 = u32::MAX - 1;
/// Nanoseconds per millisecond.
const NS_PER_MS: u128 = 1_000_000;

/// The Win32 error code of a `windows` error as an `io::Error` (so `kind()` works).
fn io_error(e: &::windows::core::Error) -> io::Error {
    let hr = e.code().0;
    let bits = u32::from_ne_bytes(hr.to_ne_bytes());
    let code = if bits & HRESULT_FACILITY_MASK == HRESULT_FACILITY_WIN32 {
        i32::try_from(bits & WIN32_CODE_MASK).unwrap_or(hr)
    } else {
        hr
    };
    io::Error::from_raw_os_error(code)
}

fn os_error(call: &'static str, object: &'static str, e: &::windows::core::Error) -> OsError {
    OsError {
        call,
        object,
        source: io_error(e),
    }
}

fn last_error(call: &'static str, object: &'static str) -> OsError {
    OsError {
        call,
        object,
        source: io::Error::last_os_error(),
    }
}

/// Maps "object does not exist" to `Ok(None)`.
fn opened(
    result: ::windows::core::Result<HANDLE>,
    call: &'static str,
    object: &'static str,
) -> Result<Option<OwnedHandle>, OsError> {
    match result {
        Ok(handle) => Ok(Some(OwnedHandle(handle))),
        Err(e) if e.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => Ok(None),
        Err(e) => Err(os_error(call, object, &e)),
    }
}

/// NUL-terminated ANSI copy of an object name.
fn ansi(name: &'static str) -> Result<CString, OsError> {
    CString::new(name).map_err(|e| OsError {
        call: "CString::new",
        object: name,
        source: io::Error::new(io::ErrorKind::InvalidInput, e),
    })
}

/// Milliseconds for `WaitForSingleObject`, rounded up so a sub-millisecond remainder still waits,
/// and never `INFINITE`.
fn millis_ceil(d: Duration) -> u32 {
    u32::try_from(d.as_nanos().div_ceil(NS_PER_MS)).map_or(MAX_WAIT_MS, |ms| ms.min(MAX_WAIT_MS))
}

/// A kernel handle closed on drop.
struct OwnedHandle(HANDLE);

// SAFETY: a kernel handle is a process-wide value with no thread affinity; it may be used and closed
// from any thread.
unsafe impl Send for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful Open*/Create* call, is owned exclusively by this
        // value and is closed exactly once. A failure leaks nothing we could recover.
        let closed = unsafe { CloseHandle(self.0) };
        if let Err(e) = closed {
            tracing::debug!(component = "lmu-reader", error = %e, "CloseHandle failed");
        }
    }
}

/// Waits on one handle.
fn wait_one(handle: &OwnedHandle, timeout: Duration) -> GateWait {
    // SAFETY: the handle is valid for the lifetime of `handle` and was opened with SYNCHRONIZE.
    let result = unsafe { WaitForSingleObject(handle.0, millis_ceil(timeout)) };
    if result == WAIT_OBJECT_0 {
        GateWait::Signaled
    } else if result == WAIT_TIMEOUT {
        GateWait::TimedOut
    } else {
        GateWait::Failed(io::Error::last_os_error())
    }
}

/// A mapped view of a whole section, unmapped on drop.
struct MappedView {
    base: MEMORY_MAPPED_VIEW_ADDRESS,
    /// `VirtualQuery` region size: the readable length of the view.
    len: usize,
}

// SAFETY: the view is ordinary process memory, valid until `UnmapViewOfFile`; it has no thread
// affinity and all access goes through raw-pointer copies or atomics.
unsafe impl Send for MappedView {}

impl MappedView {
    fn map(mapping: &OwnedHandle, access: FILE_MAP, object: &'static str) -> Result<Self, OsError> {
        // SAFETY: `mapping` is a valid file-mapping handle opened with at least `access`; offset 0 and
        // size 0 map the whole section.
        let base = unsafe { MapViewOfFile(mapping.0, access, 0, 0, 0) };
        if base.Value.is_null() {
            return Err(last_error("MapViewOfFile", object));
        }
        let mut view = Self { base, len: 0 };
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: `info` is a writable MEMORY_BASIC_INFORMATION and its size is passed; the address is
        // the start of the view mapped above.
        let written = unsafe {
            VirtualQuery(
                Some(view.base.Value.cast_const()),
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if written == 0 {
            return Err(last_error("VirtualQuery", object));
        }
        view.len = info.RegionSize;
        Ok(view)
    }

    fn copy_to(&self, offset: usize, dst: &mut [u8]) -> Result<(), OutOfBounds> {
        let oob = OutOfBounds {
            offset,
            len: dst.len(),
            available: self.len,
        };
        let end = offset.checked_add(dst.len()).ok_or(oob)?;
        if end > self.len {
            return Err(oob);
        }
        // SAFETY: [offset, end) lies inside the view (checked above), which stays mapped while `self`
        // lives; `dst` is a separate, exclusively borrowed buffer, so the ranges cannot overlap. No
        // Rust reference to the shared bytes is ever created: LMU may write them concurrently, which
        // the SDK lock (or the lock-free witness check) guards against; a torn raw copy is detected,
        // never trusted.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.base.Value.cast::<u8>().cast_const().add(offset),
                dst.as_mut_ptr(),
                dst.len(),
            );
        }
        Ok(())
    }
}

impl Drop for MappedView {
    fn drop(&mut self) {
        // SAFETY: `base` is the address returned by MapViewOfFile and is unmapped exactly once; no
        // reference into the view outlives `self`.
        let unmapped = unsafe { UnmapViewOfFile(self.base) };
        if let Err(e) = unmapped {
            tracing::debug!(component = "lmu-reader", error = %e, "UnmapViewOfFile failed");
        }
    }
}

/// The `LMU_Data` mapping, mapped read-only.
pub(crate) struct SharedView {
    view: MappedView,
    _mapping: OwnedHandle,
}

impl SharedView {
    /// Opens and maps the data mapping (`LMU_Data`) with `FILE_MAP_READ`. `Ok(None)` if it does not
    /// exist (LMU is not running).
    pub(crate) fn open(name: &'static str) -> Result<Option<Self>, OsError> {
        let wide = HSTRING::from(name);
        // SAFETY: FFI call with a valid NUL-terminated wide string that outlives the call; read-only
        // access requested.
        let result =
            unsafe { OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR::from_raw(wide.as_ptr())) };
        let Some(mapping) = opened(result, "OpenFileMappingW", name)? else {
            return Ok(None);
        };
        let view = MappedView::map(&mapping, FILE_MAP_READ, name)?;
        Ok(Some(Self {
            view,
            _mapping: mapping,
        }))
    }

    /// `VirtualQuery` region size of the view.
    pub(crate) fn region_size(&self) -> usize {
        self.view.len
    }
}

impl ByteSource for SharedView {
    fn len(&self) -> usize {
        self.view.len
    }

    fn copy_to(&self, offset: usize, dst: &mut [u8]) -> Result<(), OutOfBounds> {
        self.view.copy_to(offset, dst)
    }
}

/// Opens an existing event with `SYNCHRONIZE` only.
fn open_wait_only_event(name: &'static str) -> Result<Option<OwnedHandle>, OsError> {
    let c_name = ansi(name)?;
    // SAFETY: FFI call with a valid NUL-terminated name that outlives the call. SYNCHRONIZE only: the
    // handle can be waited on, never set or reset (D-018).
    let result = unsafe {
        OpenEventA(
            SYNCHRONIZATION_SYNCHRONIZE,
            false,
            PCSTR::from_raw(c_name.as_ptr().cast()),
        )
    };
    opened(result, "OpenEventA", name)
}

/// The LMU 1.4.2+ frame gate (`LMU_Data_HoldEvent` then `LMU_Data_DataEvent`).
pub(crate) struct FrameGate {
    hold: OwnedHandle,
    data: OwnedHandle,
}

impl FrameGate {
    /// Opens both events. `Ok(None)` if either is missing (pre-1.4.2 build or LMU gone).
    pub(crate) fn open(
        hold_name: &'static str,
        data_name: &'static str,
    ) -> Result<Option<Self>, OsError> {
        let hold = open_wait_only_event(hold_name)?;
        let data = open_wait_only_event(data_name)?;
        Ok(hold.zip(data).map(|(hold, data)| Self { hold, data }))
    }

    /// The SDK's `WaitForNextFrame`: Hold first (for at most `hold_timeout`), then Data ("the only
    /// race-free order", for at most `data_timeout` once Hold fired). Hold can be bounded tightly so
    /// the caller regains control to check for missed publishes, while a frame whose Hold already
    /// fired still gets the full `data_timeout`. Signalled does not grant exclusive access: the copy
    /// still takes the lock.
    pub(crate) fn wait(&self, hold_timeout: Duration, data_timeout: Duration) -> GateWait {
        match wait_one(&self.hold, hold_timeout) {
            GateWait::Signaled => wait_one(&self.data, data_timeout),
            other => other,
        }
    }
}

/// The SDK reader lock: two 32-bit words (`waiters` @0, `busy` @4) in `LMU_SharedMemoryLockData`
/// plus the auto-reset `LMU_SharedMemoryLockEvent`.
pub(crate) struct SharedLock {
    view: MappedView,
    event: OwnedHandle,
    _mapping: OwnedHandle,
    set_event_failures: Cell<u64>,
}

impl SharedLock {
    /// Opens the existing lock mapping and event (never creates either, never resets the words).
    pub(crate) fn open(
        mapping_name: &'static str,
        event_name: &'static str,
    ) -> Result<LockOpen, OsError> {
        let c_mapping = ansi(mapping_name)?;
        // SAFETY: FFI call with a valid NUL-terminated name that outlives the call. OpenFileMappingA
        // only opens an existing mapping. Write access is required for the compare-exchange on
        // `busy`, exactly as the SDK reader does.
        let result = unsafe {
            OpenFileMappingA(
                (FILE_MAP_READ | FILE_MAP_WRITE).0,
                false,
                PCSTR::from_raw(c_mapping.as_ptr().cast()),
            )
        };
        let Some(mapping) = opened(result, "OpenFileMappingA", mapping_name)? else {
            return Ok(LockOpen::MappingMissing);
        };
        let c_event = ansi(event_name)?;
        // SAFETY: as above; SYNCHRONIZE to wait for the lock, EVENT_MODIFY_STATE to wake waiters on
        // release (the SDK's Unlock).
        let result = unsafe {
            OpenEventA(
                SYNCHRONIZATION_SYNCHRONIZE | EVENT_MODIFY_STATE,
                false,
                PCSTR::from_raw(c_event.as_ptr().cast()),
            )
        };
        let Some(event) = opened(result, "OpenEventA", event_name)? else {
            return Ok(LockOpen::EventMissing);
        };
        let view = MappedView::map(&mapping, FILE_MAP_READ | FILE_MAP_WRITE, mapping_name)?;
        if view.len < lock_data::SIZE || !view.base.Value.cast::<i32>().is_aligned() {
            return Err(OsError {
                call: "MapViewOfFile",
                object: mapping_name,
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "lock view too small or misaligned",
                ),
            });
        }
        Ok(LockOpen::Ready(Self {
            view,
            event,
            _mapping: mapping,
            set_event_failures: Cell::new(0),
        }))
    }

    fn word(&self, field: Field) -> &AtomicI32 {
        // SAFETY: the view is at least `lock_data::SIZE` bytes and 4-byte aligned (both checked in
        // `open`), so base + 0 / base + 4 is a valid, aligned i32 inside the view for as long as `self`
        // (and therefore the returned reference) lives. LMU and every other reader access these words
        // only with Interlocked* (atomic) operations, as `AtomicI32::from_ptr` requires.
        unsafe {
            AtomicI32::from_ptr(
                self.view
                    .base
                    .Value
                    .cast::<u8>()
                    .add(field.offset)
                    .cast::<i32>(),
            )
        }
    }

    fn try_cas(&self) -> bool {
        self.word(lock_data::BUSY)
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// The SDK's `Lock`, bounded: `spins` compare-exchanges of `busy` 0 → 1, then `waiters++`, waits
    /// on the lock event (re-trying the exchange after every wake-up) for at most `wait` in total,
    /// then `waiters--`. Never blocks longer than `wait`.
    pub(crate) fn acquire(&self, spins: u32, wait: Duration) -> LockAttempt<'_> {
        for _ in 0..spins {
            if self.try_cas() {
                return LockAttempt::Acquired(LockGuard { lock: self });
            }
            std::hint::spin_loop();
        }
        let waiters = self.word(lock_data::WAITERS);
        waiters.fetch_add(1, Ordering::SeqCst);
        let deadline = Instant::now() + wait;
        let outcome = loop {
            if self.try_cas() {
                break Ok(true);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break Ok(false);
            }
            // SAFETY: the event handle is valid while `self` lives and was opened with SYNCHRONIZE.
            let result = unsafe { WaitForSingleObject(self.event.0, millis_ceil(remaining)) };
            if result == WAIT_OBJECT_0 {
                continue;
            }
            if result == WAIT_TIMEOUT {
                break Ok(false);
            }
            break Err(io::Error::last_os_error());
        };
        waiters.fetch_sub(1, Ordering::SeqCst);
        match outcome {
            Ok(true) => LockAttempt::Acquired(LockGuard { lock: self }),
            Ok(false) => LockAttempt::TimedOut,
            Err(e) => LockAttempt::Failed(e),
        }
    }

    /// The SDK's `Unlock`: `busy = 0`, then wake one waiter if any is registered.
    fn release(&self) {
        self.word(lock_data::BUSY).swap(0, Ordering::SeqCst);
        if self.word(lock_data::WAITERS).load(Ordering::SeqCst) > 0 {
            // SAFETY: the event handle is valid while `self` lives and was opened with
            // EVENT_MODIFY_STATE.
            let set = unsafe { SetEvent(self.event.0) };
            if set.is_err() {
                // A lost wake-up only delays a waiter until its own timeout (the SDK has the same
                // property); counted for diagnostics.
                self.set_event_failures
                    .set(self.set_event_failures.get().saturating_add(1));
            }
        }
    }

    /// Current raw `busy` word (read-only diagnostics: 1 for a long time means another process holds
    /// or abandoned the lock, A-LMU-052; we never reset it).
    pub(crate) fn busy_word(&self) -> i32 {
        self.word(lock_data::BUSY).load(Ordering::Relaxed)
    }

    /// Current raw `waiters` word (diagnostics).
    pub(crate) fn waiters_word(&self) -> i32 {
        self.word(lock_data::WAITERS).load(Ordering::Relaxed)
    }

    /// `SetEvent` failures on release.
    pub(crate) fn set_event_failures(&self) -> u64 {
        self.set_event_failures.get()
    }
}

/// Holds the SDK lock; releases it on drop (including during unwinding).
pub(crate) struct LockGuard<'a> {
    lock: &'a SharedLock,
}

impl Drop for LockGuard<'_> {
    fn drop(&mut self) {
        self.lock.release();
    }
}

/// The LMU process opened with `SYNCHRONIZE` only.
pub(crate) struct ProcessWatch {
    handle: OwnedHandle,
    pid: u32,
}

impl ProcessWatch {
    /// Opens the process for waiting only (never stronger rights, A-LMU-046).
    pub(crate) fn open(pid: u32) -> Result<Self, OsError> {
        // SAFETY: plain FFI call; SYNCHRONIZE is the only right requested.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }
            .map_err(|e| os_error("OpenProcess", "Le Mans Ultimate.exe", &e))?;
        Ok(Self {
            handle: OwnedHandle(handle),
            pid,
        })
    }

    /// The process id.
    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    /// `true` once the process has exited.
    pub(crate) fn has_exited(&self) -> bool {
        matches!(wait_one(&self.handle, Duration::ZERO), GateWait::Signaled)
    }
}

/// Finds `Le Mans Ultimate.exe` with a ToolHelp snapshot (no process handle is opened).
pub(crate) fn find_lmu_process() -> Result<Option<u32>, OsError> {
    // SAFETY: plain FFI call; the returned handle is owned (and closed) below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|e| os_error("CreateToolhelp32Snapshot", "process list", &e))?;
    let snapshot = OwnedHandle(snapshot);
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    // SAFETY: valid snapshot handle and a writable, correctly sized PROCESSENTRY32W.
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_ok();
    while more {
        if is_lmu_exe(&entry.szExeFile) {
            return Ok(Some(entry.th32ProcessID));
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot.0, &mut entry) }.is_ok();
    }
    Ok(None)
}

/// Test-only stand-in for LMU's kernel objects, created under unique per-process test names (never
/// LMU's names), so the real open / map / gate / lock code can be exercised without LMU.
#[cfg(test)]
pub(crate) mod fake {
    use ::windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, INVALID_HANDLE_VALUE};
    use ::windows::Win32::System::Memory::{CreateFileMappingW, PAGE_READWRITE};
    use ::windows::Win32::System::Threading::CreateEventW;

    use super::*;
    use crate::state::ObjectNames;

    /// A unique, leaked (test-lifetime) object name.
    pub(crate) fn unique(tag: &str, object: &str) -> &'static str {
        Box::leak(format!("lmu-reader-test-{}-{tag}-{object}", std::process::id()).into_boxed_str())
    }

    fn create_mapping(name: &'static str, size: usize) -> (OwnedHandle, MappedView) {
        let wide = HSTRING::from(name);
        let size = u32::try_from(size).expect("mapping size fits u32");
        // SAFETY: test-only FFI call creating a pagefile-backed section under a unique test name.
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                size,
                PCWSTR::from_raw(wide.as_ptr()),
            )
        }
        .expect("CreateFileMappingW");
        // SAFETY: reads this thread's last-error value, set by the call above.
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let handle = OwnedHandle(handle);
        assert!(
            !existed,
            "test mapping {name} already exists: two tests share a tag"
        );
        let view =
            MappedView::map(&handle, FILE_MAP_READ | FILE_MAP_WRITE, name).expect("map test view");
        (handle, view)
    }

    fn create_event(name: &'static str) -> OwnedHandle {
        let wide = HSTRING::from(name);
        // SAFETY: test-only FFI call creating an auto-reset, non-signalled event under a unique name.
        let handle = unsafe { CreateEventW(None, false, false, PCWSTR::from_raw(wide.as_ptr())) }
            .expect("CreateEventW");
        OwnedHandle(handle)
    }

    /// Fake data mapping, optional gate events and optional lock objects.
    pub(crate) struct FakeLmu {
        pub(crate) names: ObjectNames,
        data: (OwnedHandle, MappedView),
        gate: Option<(OwnedHandle, OwnedHandle)>,
        lock: Option<(OwnedHandle, MappedView, OwnedHandle)>,
    }

    impl FakeLmu {
        /// Creates the objects and writes `image` into the data mapping (sized to the image).
        pub(crate) fn new(tag: &str, image: &[u8], with_gate: bool, with_lock: bool) -> Self {
            let names = ObjectNames {
                data_mapping: unique(tag, "data"),
                hold_event: unique(tag, "hold"),
                data_event: unique(tag, "data-event"),
                lock_mapping: unique(tag, "lock"),
                lock_event: unique(tag, "lock-event"),
            };
            let data = create_mapping(names.data_mapping, image.len());
            let gate = with_gate.then(|| {
                (
                    create_event(names.hold_event),
                    create_event(names.data_event),
                )
            });
            let lock = with_lock.then(|| {
                let (handle, view) = create_mapping(names.lock_mapping, lock_data::SIZE);
                (handle, view, create_event(names.lock_event))
            });
            let fake = Self {
                names,
                data,
                gate,
                lock,
            };
            fake.write(0, image);
            fake
        }

        /// Creates the gate events later (LMU creating them after `LMU_Data`).
        pub(crate) fn add_gate(&mut self) {
            assert!(self.gate.is_none(), "fake already has a gate");
            self.gate = Some((
                create_event(self.names.hold_event),
                create_event(self.names.data_event),
            ));
        }

        /// Creates the lock objects later (LMU creating them after `LMU_Data`).
        pub(crate) fn add_lock(&mut self) {
            assert!(self.lock.is_none(), "fake already has a lock");
            let (handle, view) = create_mapping(self.names.lock_mapping, lock_data::SIZE);
            self.lock = Some((handle, view, create_event(self.names.lock_event)));
        }

        /// Writes bytes into the data mapping (as LMU's writer would).
        pub(crate) fn write(&self, offset: usize, bytes: &[u8]) {
            let view = &self.data.1;
            assert!(
                offset + bytes.len() <= view.len,
                "write outside the fake mapping"
            );
            // SAFETY: test-only write inside our own read-write view; bounds asserted above.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    view.base.Value.cast::<u8>().add(offset),
                    bytes.len(),
                );
            }
        }

        /// Publishes one frame: Hold, then Data.
        pub(crate) fn publish(&self) {
            let (hold, data) = self.gate.as_ref().expect("fake has no gate");
            // SAFETY: valid event handles created by this fake.
            unsafe {
                SetEvent(hold.0).expect("SetEvent hold");
                SetEvent(data.0).expect("SetEvent data");
            }
        }

        fn word(&self, field: Field) -> &AtomicI32 {
            let (_, view, _) = self.lock.as_ref().expect("fake has no lock");
            // SAFETY: our own page-sized, page-aligned read-write view; every user accesses the
            // words atomically.
            unsafe {
                AtomicI32::from_ptr(view.base.Value.cast::<u8>().add(field.offset).cast::<i32>())
            }
        }

        /// Sets the `busy` word (simulates another process holding the lock).
        pub(crate) fn set_busy(&self, value: i32) {
            self.word(lock_data::BUSY).store(value, Ordering::SeqCst);
        }

        /// The `busy` word.
        pub(crate) fn busy(&self) -> i32 {
            self.word(lock_data::BUSY).load(Ordering::SeqCst)
        }

        /// Sets the `waiters` word (simulates a registered waiter in another process).
        pub(crate) fn set_waiters(&self, value: i32) {
            self.word(lock_data::WAITERS).store(value, Ordering::SeqCst);
        }

        /// The `waiters` word.
        pub(crate) fn waiters(&self) -> i32 {
            self.word(lock_data::WAITERS).load(Ordering::SeqCst)
        }

        /// `true` if the lock event is signalled (consumes the auto-reset signal).
        pub(crate) fn take_lock_event(&self) -> bool {
            let (_, _, event) = self.lock.as_ref().expect("fake has no lock");
            matches!(wait_one(event, Duration::ZERO), GateWait::Signaled)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{FakeLmu, unique};
    use super::*;
    use crate::offsets::layout;

    fn image() -> Vec<u8> {
        (0..layout::SIZE)
            .map(|i| u8::try_from(i % 251).expect("byte"))
            .collect()
    }

    #[test]
    fn view_maps_read_only_copies_and_checks_bounds() {
        let image = image();
        let fake = FakeLmu::new("view", &image, false, false);
        let view = SharedView::open(fake.names.data_mapping)
            .expect("open")
            .expect("exists");
        assert!(view.region_size() >= layout::SIZE, "page-rounded region");
        let mut copy = vec![0u8; layout::SIZE];
        view.copy_to(0, &mut copy).expect("copy");
        assert_eq!(copy, image);
        let mut tail = [0u8; 8];
        assert!(
            view.copy_to(view.region_size() - 4, &mut tail).is_err(),
            "never reads past the view"
        );
        assert!(view.copy_to(usize::MAX, &mut tail).is_err());
    }

    #[test]
    fn missing_objects_are_reported_as_absent_not_as_errors() {
        let missing = unique("missing", "x");
        assert!(SharedView::open(missing).expect("open").is_none());
        assert!(FrameGate::open(missing, missing).expect("open").is_none());
        assert!(matches!(
            SharedLock::open(missing, missing).expect("open"),
            LockOpen::MappingMissing
        ));
        let fake = FakeLmu::new("lock-no-event", &[0u8; 64], false, false);
        // A data mapping under the lock-mapping name, but no event.
        assert!(matches!(
            SharedLock::open(fake.names.data_mapping, missing).expect("open"),
            LockOpen::EventMissing
        ));
    }

    #[test]
    fn gate_waits_hold_then_data_and_times_out() {
        let fake = FakeLmu::new("gate", &[0u8; 64], true, false);
        let gate = FrameGate::open(fake.names.hold_event, fake.names.data_event)
            .expect("open")
            .expect("events");
        assert!(matches!(
            gate.wait(Duration::from_millis(10), Duration::from_millis(10)),
            GateWait::TimedOut
        ));
        fake.publish();
        assert!(matches!(
            gate.wait(Duration::from_millis(500), Duration::from_millis(500)),
            GateWait::Signaled
        ));
        assert!(
            matches!(
                gate.wait(Duration::from_millis(10), Duration::from_millis(10)),
                GateWait::TimedOut
            ),
            "one publish, one frame"
        );
    }

    fn open_lock(fake: &FakeLmu) -> SharedLock {
        match SharedLock::open(fake.names.lock_mapping, fake.names.lock_event).expect("open") {
            LockOpen::Ready(lock) => lock,
            _ => panic!("lock objects missing"),
        }
    }

    #[test]
    fn lock_acquire_and_release_follow_the_sdk_protocol() {
        let fake = FakeLmu::new("lock", &[0u8; 64], false, true);
        let lock = open_lock(&fake);
        match lock.acquire(10, Duration::from_millis(1)) {
            LockAttempt::Acquired(guard) => {
                assert_eq!(fake.busy(), 1, "held while the guard lives");
                drop(guard);
            }
            _ => panic!("free lock not acquired"),
        }
        assert_eq!(fake.busy(), 0, "released");
        assert!(!fake.take_lock_event(), "no waiters: no wake-up");

        fake.set_waiters(1);
        let LockAttempt::Acquired(guard) = lock.acquire(10, Duration::from_millis(1)) else {
            panic!("free lock not acquired");
        };
        drop(guard);
        assert!(fake.take_lock_event(), "release wakes a registered waiter");
        assert_eq!(
            fake.waiters(),
            1,
            "other processes' waiter count is not touched"
        );
    }

    #[test]
    fn lock_held_elsewhere_times_out_quickly_and_is_never_reset() {
        let fake = FakeLmu::new("lock-held", &[0u8; 64], false, true);
        let lock = open_lock(&fake);
        fake.set_busy(1);
        let started = Instant::now();
        assert!(matches!(
            lock.acquire(100, Duration::from_millis(5)),
            LockAttempt::TimedOut
        ));
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "bounded wait: {:?}",
            started.elapsed()
        );
        assert_eq!(fake.busy(), 1, "a lock held by someone else is never reset");
        assert_eq!(fake.waiters(), 0, "our waiter registration is undone");
        assert_eq!(lock.busy_word(), 1);
    }

    #[test]
    fn process_lookup_runs() {
        // LMU may or may not be running; the lookup itself must succeed.
        assert!(find_lmu_process().is_ok());
    }

    #[test]
    fn millis_round_up_and_never_mean_infinite() {
        assert_eq!(millis_ceil(Duration::ZERO), 0);
        assert_eq!(millis_ceil(Duration::from_micros(1)), 1);
        assert_eq!(millis_ceil(Duration::from_millis(5)), 5);
        assert_eq!(millis_ceil(Duration::MAX), MAX_WAIT_MS);
    }

    #[test]
    fn hresult_win32_codes_map_to_io_kinds() {
        let not_found =
            ::windows::core::Error::from_hresult(HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0));
        assert_eq!(io_error(&not_found).kind(), io::ErrorKind::NotFound);
        assert_eq!(io_error(&not_found).raw_os_error(), Some(2));
    }
}
