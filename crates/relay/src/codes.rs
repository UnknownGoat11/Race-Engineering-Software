//! One-time connection codes (D-022, PROTOCOL.md §6).
//!
//! * A code is 8 Crockford base32 characters = 40 bits from the OS CSPRNG
//!   ([`generate_connect_code`]); every character carries exactly 5 random bits, so all 2^40 codes
//!   are equally likely.
//! * The relay keeps only a **keyed** digest of a code (HMAC-SHA256 under a random per-process key,
//!   [`CodeHasher`]). A plain SHA-256 of a 40-bit value could be brute-forced from a memory dump;
//!   without the key it cannot. Codes live in relay memory only and never outlive the process.
//! * Comparison is constant-time ([`crate::secrets::TokenHash::ct_eq`]). Codes are never logged:
//!   they travel as [`Secret`] whose `Debug` is redacted.

use hmac::{Hmac, Mac};
use protocol::messages::{CONNECT_CODE_ALPHABET, CONNECT_CODE_LEN, Secret};
use sha2::Sha256;
use thiserror::Error;

use crate::secrets::{DIGEST_BYTES, RandomError, TokenHash, random_bytes};

/// Random bytes behind one code: 8 characters × 5 bits = 40 bits = 5 bytes.
const CODE_RANDOM_BYTES: usize = 5;
/// Bits per Crockford base32 character.
const BITS_PER_CHAR: u32 = 5;
/// Mask of one base32 digit.
const CHAR_MASK: u64 = 0b1_1111;

/// Generates a fresh canonical connection code (upper case, no separator).
pub fn generate_connect_code() -> Result<Secret, RandomError> {
    let bytes = random_bytes::<CODE_RANDOM_BYTES>()?;
    let mut value = bytes
        .iter()
        .fold(0u64, |acc, &b| (acc << u8::BITS) | u64::from(b));
    let mut chars = [0u8; CONNECT_CODE_LEN];
    for slot in chars.iter_mut().rev() {
        // In bounds: the index is masked to 0..32 and the alphabet has 32 entries.
        *slot = CONNECT_CODE_ALPHABET[(value & CHAR_MASK) as usize];
        value >>= BITS_PER_CHAR;
    }
    // The alphabet is ASCII: one byte is one char.
    Ok(Secret::new(
        chars.iter().map(|&b| char::from(b)).collect::<String>(),
    ))
}

/// Why a [`CodeHasher`] could not be created.
#[derive(Debug, Error)]
pub enum CodeHasherError {
    #[error(transparent)]
    Random(#[from] RandomError),
    /// HMAC refused the key (impossible for HMAC-SHA256, which accepts any key length).
    #[error("the HMAC implementation refused the code key")]
    Key,
}

/// Keyed digest of connection codes (HMAC-SHA256 under a random per-process key).
#[derive(Clone)]
pub struct CodeHasher {
    /// HMAC state already keyed; cloned per digest.
    keyed: Hmac<Sha256>,
}

impl std::fmt::Debug for CodeHasher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CodeHasher(<key redacted>)")
    }
}

impl CodeHasher {
    /// A hasher with a fresh random key. Codes hashed by another instance never match.
    pub fn new() -> Result<Self, CodeHasherError> {
        let key = random_bytes::<DIGEST_BYTES>()?;
        let keyed =
            <Hmac<Sha256> as Mac>::new_from_slice(&key).map_err(|_| CodeHasherError::Key)?;
        Ok(Self { keyed })
    }

    /// Digest of a canonical code (see `protocol::messages::normalize_connect_code`).
    pub fn digest(&self, canonical_code: &Secret) -> TokenHash {
        let mut mac = self.keyed.clone();
        mac.update(canonical_code.expose().as_bytes());
        let out = mac.finalize().into_bytes();
        let mut digest = [0u8; DIGEST_BYTES];
        digest.copy_from_slice(&out);
        TokenHash::from_bytes(digest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::messages::normalize_connect_code;
    use std::collections::HashSet;

    #[test]
    fn codes_are_canonical_crockford() {
        for _ in 0..200 {
            let code = generate_connect_code().expect("rng");
            assert_eq!(code.expose().len(), CONNECT_CODE_LEN);
            assert!(
                code.expose()
                    .bytes()
                    .all(|b| CONNECT_CODE_ALPHABET.contains(&b))
            );
            // Already canonical: normalization is the identity.
            let normalized = normalize_connect_code(code.expose()).expect("valid");
            assert_eq!(normalized.expose(), code.expose());
        }
    }

    #[test]
    fn codes_are_random() {
        let codes: HashSet<String> = (0..1_000)
            .map(|_| generate_connect_code().expect("rng").expose().to_owned())
            .collect();
        // 1000 draws from 2^40: a collision has probability ~5e-7.
        assert_eq!(codes.len(), 1_000);
        // Every alphabet character appears in some position (8000 characters drawn).
        let seen: HashSet<u8> = codes.iter().flat_map(|c| c.bytes()).collect();
        assert_eq!(seen.len(), CONNECT_CODE_ALPHABET.len());
    }

    #[test]
    fn digests_are_keyed() {
        let a = CodeHasher::new().expect("rng");
        let b = CodeHasher::new().expect("rng");
        let code = Secret::new("7KQ4MZ9T");
        assert!(a.digest(&code).ct_eq(&a.digest(&code)));
        assert!(!a.digest(&code).ct_eq(&b.digest(&code)));
        assert!(!a.digest(&code).ct_eq(&a.digest(&Secret::new("7KQ4MZ9V"))));
        assert_eq!(format!("{a:?}"), "CodeHasher(<key redacted>)");
    }
}
