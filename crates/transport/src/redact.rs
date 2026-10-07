//! [`Redacted`]: a wrapper whose `Debug` and `Display` output never shows the wrapped value.
//!
//! Tokens, resume tickets and connection codes travel through structs that derive `Debug` and end
//! up in `tracing` fields. Wrapping them makes an accidental `{:?}` harmless (D-014: secrets are
//! never logged). [`protocol::messages::Secret`] already redacts its `Debug`; use `Redacted` for
//! anything else that must not appear in logs.

use std::fmt;

/// Text printed instead of a redacted value.
const REDACTED: &str = "<redacted>";

/// A value whose `Debug` and `Display` print `<redacted>`.
///
/// Access the value explicitly with [`Redacted::expose`]; call sites must not log it.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    /// Wraps a value.
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// The wrapped value. Do not log it.
    pub const fn expose(&self) -> &T {
        &self.0
    }

    /// Unwraps the value. Do not log it.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> From<T> for Redacted<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_never_show_the_value() {
        let token = Redacted::new(String::from("super-secret-token"));
        assert_eq!(format!("{token:?}"), REDACTED);
        assert_eq!(format!("{token}"), REDACTED);
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Holder {
            token: Redacted<String>,
        }
        let holder = Holder {
            token: token.clone(),
        };
        assert!(!format!("{holder:?}").contains("super-secret-token"));
        assert_eq!(token.expose(), "super-secret-token");
        assert_eq!(token.into_inner(), "super-secret-token");
    }
}
