//! Reader errors.

use std::io;

use thiserror::Error;

use crate::blocks::OutOfBounds;
use crate::bytes::DecodeError;
use crate::compat::CompatibilityReport;

/// Errors returned by [`crate::LmuReader`].
///
/// "LMU is not running" is not an error: the reader reports it through
/// [`crate::ReaderState::WaitingForLmu`] and `Ok(None)`.
#[derive(Debug, Error)]
pub enum LmuError {
    /// Structural compatibility checks failed. The reader decodes nothing until a re-check passes
    /// (D-003, spec §191); it never guesses at a changed layout. The message names the detected and
    /// expected values and what to do.
    #[error("{message}")]
    IncompatibleInterface {
        /// Actionable, human-readable description (detected vs expected).
        message: String,
        /// The full compatibility report at the time of the failure.
        report: Box<CompatibilityReport>,
    },

    /// A Windows API call failed for a reason other than "the object does not exist".
    #[error("{call}({object}) failed: {source}")]
    Os {
        /// The Win32 function.
        call: &'static str,
        /// The kernel object name or a short description of the target.
        object: &'static str,
        /// The OS error (Win32 error code preserved).
        #[source]
        source: io::Error,
    },

    /// A copied block could not be decoded. With the fixed block sizes this means an internal
    /// invariant was broken; the frame is dropped.
    #[error("decoding a copied LMU block failed: {0}")]
    Decode(#[from] DecodeError),

    /// A copy would have read outside the mapped view; the frame is dropped.
    #[error("copying from LMU_Data failed: {0}")]
    Copy(#[from] OutOfBounds),

    /// LMU shared memory exists only on Windows.
    #[error("LMU shared memory can only be read on Windows")]
    UnsupportedPlatform,
}

impl LmuError {
    /// `true` for [`LmuError::IncompatibleInterface`].
    pub fn is_incompatible(&self) -> bool {
        matches!(self, Self::IncompatibleInterface { .. })
    }
}
