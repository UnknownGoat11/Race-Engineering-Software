//! Secret material: CSPRNG output, opaque tokens, SHA-256 digests and constant-time comparison.
//!
//! * Every token, ticket, code and key comes from the operating system CSPRNG ([`OsRng`]); a
//!   failure of the OS generator is an error, never a fallback to a weaker source.
//! * Refresh tokens, driver resume tokens and engineer resume tickets are 256-bit random values,
//!   transported as unpadded base64url and stored (in PostgreSQL or relay memory) only as their
//!   SHA-256 digest ([`TokenHash`]).
//! * [`TokenHash`] has a redacted `Debug` and compares in constant time ([`TokenHash::ct_eq`]).

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use protocol::messages::Secret;
use rand::TryRngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;

/// Random bytes of every opaque token (256 bits).
pub const TOKEN_BYTES: usize = 32;
/// Size of a SHA-256 digest.
pub const DIGEST_BYTES: usize = 32;
/// Longest token string the relay accepts before hashing it (base64url of 32 bytes is 43
/// characters; JWT access tokens are checked separately). Longer input is rejected unread.
pub const MAX_OPAQUE_TOKEN_CHARS: usize = 128;

/// The operating system's random number generator failed.
#[derive(Debug, Error)]
#[error("the operating system random number generator failed")]
pub struct RandomError(#[source] rand::rand_core::OsError);

/// `N` bytes from the operating system CSPRNG.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], RandomError> {
    let mut buf = [0u8; N];
    OsRng.try_fill_bytes(&mut buf).map_err(RandomError)?;
    Ok(buf)
}

/// A random `u64` from the operating system CSPRNG.
pub fn random_u64() -> Result<u64, RandomError> {
    Ok(u64::from_le_bytes(random_bytes::<8>()?))
}

/// SHA-256 digest of a secret (refresh token, resume token, ticket). The secret itself is never
/// stored; lookups and comparisons use this digest.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenHash([u8; DIGEST_BYTES]);

impl TokenHash {
    /// Digest of `secret`'s UTF-8 bytes.
    pub fn of(secret: &str) -> Self {
        Self(Sha256::digest(secret.as_bytes()).into())
    }

    /// Wraps a digest read from storage. `None` unless exactly [`DIGEST_BYTES`] long.
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        <[u8; DIGEST_BYTES]>::try_from(bytes).ok().map(Self)
    }

    /// Wraps a raw digest (e.g. an HMAC output).
    pub const fn from_bytes(bytes: [u8; DIGEST_BYTES]) -> Self {
        Self(bytes)
    }

    /// Raw digest bytes (for storage).
    pub const fn as_bytes(&self) -> &[u8; DIGEST_BYTES] {
        &self.0
    }

    /// Constant-time equality: the running time does not depend on where the digests differ.
    pub fn ct_eq(&self, other: &TokenHash) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenHash(<redacted>)")
    }
}

/// A fresh 256-bit opaque token (unpadded base64url, 43 characters) and its digest.
pub fn new_token() -> Result<(Secret, TokenHash), RandomError> {
    let token = URL_SAFE_NO_PAD.encode(random_bytes::<TOKEN_BYTES>()?);
    let hash = TokenHash::of(&token);
    Ok((Secret::new(token), hash))
}

/// Digest of a token presented by a client, or `None` if it cannot be one of ours (empty or longer
/// than [`MAX_OPAQUE_TOKEN_CHARS`]). Unknown-but-plausible tokens still hash; the lookup fails.
pub fn presented_token_hash(token: &Secret) -> Option<TokenHash> {
    let value = token.expose();
    if value.is_empty() || value.len() > MAX_OPAQUE_TOKEN_CHARS {
        return None;
    }
    Some(TokenHash::of(value))
}

/// Constant-time comparison of two secrets of possibly different length. Both are hashed first,
/// so neither the content nor the length leaks through timing.
pub fn ct_eq_secret(a: &str, b: &str) -> bool {
    TokenHash::of(a).ct_eq(&TokenHash::of(b))
}

/// Unpadded base64url encoding.
pub fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Unpadded base64url decoding (`None` on invalid input).
pub fn b64url_decode(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_256_bit_base64url_and_unique() {
        let (a, ha) = new_token().expect("rng");
        let (b, hb) = new_token().expect("rng");
        assert_eq!(a.expose().len(), 43);
        assert_ne!(a.expose(), b.expose());
        assert!(!ha.ct_eq(&hb));
        assert!(ha.ct_eq(&TokenHash::of(a.expose())));
        assert_eq!(
            b64url_decode(a.expose()).map(|v| v.len()),
            Some(TOKEN_BYTES)
        );
    }

    #[test]
    fn digest_debug_is_redacted() {
        let h = TokenHash::of("secret-value");
        assert_eq!(format!("{h:?}"), "TokenHash(<redacted>)");
    }

    #[test]
    fn presented_tokens_are_bounded() {
        assert!(presented_token_hash(&Secret::new("")).is_none());
        assert!(
            presented_token_hash(&Secret::new("x".repeat(MAX_OPAQUE_TOKEN_CHARS + 1))).is_none()
        );
        assert!(presented_token_hash(&Secret::new("abc")).is_some());
    }

    #[test]
    fn secret_comparison() {
        assert!(ct_eq_secret("same", "same"));
        assert!(!ct_eq_secret("same", "same-but-longer"));
        assert!(!ct_eq_secret("", "x"));
    }

    #[test]
    fn from_slice_checks_length() {
        assert!(TokenHash::from_slice(&[0u8; 31]).is_none());
        assert!(TokenHash::from_slice(&[0u8; 32]).is_some());
    }
}
