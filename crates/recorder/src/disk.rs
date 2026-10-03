//! Free disk space probing.

use std::io;
use std::path::Path;

/// Reports the free space available for recordings. A trait so tests can simulate a filling disk.
pub trait DiskSpaceProbe: Send {
    /// Bytes available to the current user on the volume containing `path` (an existing directory).
    fn available_bytes(&self, path: &Path) -> io::Result<u64>;
}

/// The operating system's view of free space (`GetDiskFreeSpaceExW` on Windows).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDiskSpace;

#[cfg(windows)]
impl DiskSpaceProbe for SystemDiskSpace {
    fn available_bytes(&self, path: &Path) -> io::Result<u64> {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        use windows::core::PCWSTR;

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut available: u64 = 0;
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call; `available` is a valid,
        // writable u64 for the duration of the call; the two optional out-parameters are passed as None
        // (NULL), which the API permits. The function only reads the path and writes the counters.
        unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut available), None, None) }
            .map_err(io::Error::from)?;
        Ok(available)
    }
}

#[cfg(not(windows))]
impl DiskSpaceProbe for SystemDiskSpace {
    fn available_bytes(&self, _path: &Path) -> io::Result<u64> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "disk space probing is implemented for Windows only",
        ))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn system_probe_reports_space_for_temp_dir() {
        let free = SystemDiskSpace
            .available_bytes(&std::env::temp_dir())
            .expect("probe");
        assert!(free > 0);
        assert!(
            SystemDiskSpace
                .available_bytes(Path::new("Z:\\definitely\\not\\here\\"))
                .is_err()
        );
    }
}
