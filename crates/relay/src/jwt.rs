//! Access tokens (D-008): compact JWS / JWT signed with Ed25519 (`alg` = `EdDSA`, RFC 8037).
//!
//! * Implemented directly on `ed25519-dalek` (no generic JWT library): the relay is both issuer and
//!   only verifier, so exactly one algorithm is accepted. A header with any other `alg`, an unknown
//!   header field (e.g. `crit`) or a foreign `kid` is refused before the signature is checked, which
//!   rules out algorithm-confusion attacks.
//! * Signatures are verified with `verify_strict` (rejects small-order keys and malleable
//!   signatures).
//! * The signing key is a 32-byte Ed25519 seed kept in a file (`JWT_KEY_FILE`, created with
//!   `lmu-relay gen-jwt-key`): one line of unpadded base64url. Replacing the key invalidates every
//!   outstanding access token; clients then fall back to their refresh token transparently.

use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use ed25519_dalek::{
    SECRET_KEY_LENGTH, SIGNATURE_LENGTH, Signature, Signer, SigningKey, VerifyingKey,
};
use hmac::{Hmac, Mac};
use protocol::messages::Secret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::secrets::{RandomError, b64url, b64url_decode, random_bytes};

/// The only accepted JWS algorithm.
pub const JWT_ALGORITHM: &str = "EdDSA";
/// JWT `typ` header value.
pub const JWT_TYPE: &str = "JWT";
/// `iss` claim of every access token.
pub const JWT_ISSUER: &str = "lmu-relay";
/// Longest access token accepted for verification (ours are ~400 characters).
pub const MAX_ACCESS_TOKEN_CHARS: usize = 2_048;
/// Tolerated clock skew for `iat` in the future (seconds).
pub const MAX_IAT_SKEW_S: u64 = 60;
/// Bytes of the public-key digest used as `kid`.
const KID_BYTES: usize = 8;
/// Random bytes of a token id (`jti`).
const JTI_BYTES: usize = 16;
/// Number of dot-separated segments of a compact JWS.
const JWS_SEGMENTS: usize = 3;
/// Length of a key derived with [`AccessTokenKeys::derive_key`] (one SHA-256 output).
pub const DERIVED_KEY_BYTES: usize = 32;

/// Claims of an access token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessClaims {
    /// Issuer, always [`JWT_ISSUER`].
    pub iss: String,
    /// Account id (hyphenated UUID).
    pub sub: String,
    /// Account role at issue time (`driver` / `engineer` / `admin`).
    pub role: String,
    /// Refresh-token family (login) the token belongs to; revoking the family revokes the token.
    pub fam: String,
    /// Issued at, seconds since the Unix epoch.
    pub iat: u64,
    /// Expiry, seconds since the Unix epoch.
    pub exp: u64,
    /// Random token id.
    pub jti: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JwsHeader {
    alg: String,
    typ: String,
    kid: String,
}

/// Why an access token was refused (or could not be issued).
#[derive(Debug, Error)]
pub enum JwtError {
    #[error("access token longer than {MAX_ACCESS_TOKEN_CHARS} characters")]
    TooLong,
    #[error("access token is not a compact JWS: {0}")]
    Malformed(&'static str),
    #[error("access token header or claims are not valid JSON")]
    Json(#[source] serde_json::Error),
    #[error("access token uses algorithm {0:?}; only EdDSA is accepted")]
    Algorithm(String),
    #[error("access token was signed by another key")]
    KeyId,
    #[error("access token signature is invalid")]
    Signature,
    #[error("access token issuer is not {JWT_ISSUER}")]
    Issuer,
    #[error("access token expired")]
    Expired,
    #[error("access token is issued in the future")]
    NotYetValid,
    #[error(transparent)]
    Random(#[from] RandomError),
}

/// HMAC refused the signing key as a derivation key (impossible for HMAC-SHA256, which accepts any
/// key length).
#[derive(Debug, Error)]
#[error("the signing key cannot be used for key derivation")]
pub struct KeyDerivationError;

/// Errors reading or creating the signing key file.
#[derive(Debug, Error)]
pub enum KeyFileError {
    #[error("cannot read the JWT key file {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot create the JWT key file {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the JWT key file {path} does not hold a base64url Ed25519 seed of 32 bytes")]
    Format { path: PathBuf },
    #[error(transparent)]
    Random(#[from] RandomError),
}

/// The relay's access-token signing key.
pub struct AccessTokenKeys {
    signing: SigningKey,
    verifying: VerifyingKey,
    kid: String,
}

impl fmt::Debug for AccessTokenKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessTokenKeys")
            .field("kid", &self.kid)
            .finish_non_exhaustive()
    }
}

impl AccessTokenKeys {
    /// Keys from a 32-byte Ed25519 seed.
    pub fn from_seed(seed: [u8; SECRET_KEY_LENGTH]) -> Self {
        let signing = SigningKey::from_bytes(&seed);
        let verifying = signing.verifying_key();
        let digest = Sha256::digest(verifying.as_bytes());
        let kid = b64url(&digest[..KID_BYTES]);
        Self {
            signing,
            verifying,
            kid,
        }
    }

    /// A fresh random key (development without `JWT_KEY_FILE`, tests). Tokens signed with it die
    /// with the process.
    pub fn generate() -> Result<Self, RandomError> {
        Ok(Self::from_seed(random_bytes::<SECRET_KEY_LENGTH>()?))
    }

    /// Loads the seed from `path` (one line of unpadded base64url).
    pub fn load(path: &Path) -> Result<Self, KeyFileError> {
        let text = fs::read_to_string(path).map_err(|source| KeyFileError::Read {
            path: path.to_owned(),
            source,
        })?;
        let seed = b64url_decode(text.trim())
            .and_then(|bytes| <[u8; SECRET_KEY_LENGTH]>::try_from(bytes.as_slice()).ok())
            .ok_or_else(|| KeyFileError::Format {
                path: path.to_owned(),
            })?;
        Ok(Self::from_seed(seed))
    }

    /// Creates a new key file at `path` (never overwrites an existing file). On Unix the file is
    /// readable by its owner only.
    pub fn create_file(path: &Path) -> Result<(), KeyFileError> {
        let seed = random_bytes::<SECRET_KEY_LENGTH>()?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let write_err = |source| KeyFileError::Write {
            path: path.to_owned(),
            source,
        };
        let mut file = options.open(path).map_err(write_err)?;
        writeln!(file, "{}", b64url(&seed)).map_err(write_err)?;
        file.sync_all().map_err(write_err)?;
        Ok(())
    }

    /// Key id (`kid` header): base64url of the first bytes of SHA-256(public key).
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// A 32-byte secret for another purpose, derived from the signing key as
    /// HMAC-SHA256(seed, `label`) (domain-separated by `label`). It changes exactly when the
    /// signing key changes (e.g. the engineer resume-ticket key, [`crate::tickets`]).
    pub fn derive_key(&self, label: &[u8]) -> Result<[u8; DERIVED_KEY_BYTES], KeyDerivationError> {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.signing.to_bytes())
            .map_err(|_| KeyDerivationError)?;
        mac.update(label);
        let out = mac.finalize().into_bytes();
        let mut key = [0u8; DERIVED_KEY_BYTES];
        // Both are 32 bytes: HMAC-SHA256 output.
        key.copy_from_slice(&out);
        Ok(key)
    }

    /// Signs `claims` into a compact JWS.
    pub fn sign(&self, claims: &AccessClaims) -> Result<Secret, JwtError> {
        let header = JwsHeader {
            alg: JWT_ALGORITHM.to_owned(),
            typ: JWT_TYPE.to_owned(),
            kid: self.kid.clone(),
        };
        let header = serde_json::to_vec(&header).map_err(JwtError::Json)?;
        let claims = serde_json::to_vec(claims).map_err(JwtError::Json)?;
        let signing_input = format!("{}.{}", b64url(&header), b64url(&claims));
        let signature = self.signing.sign(signing_input.as_bytes());
        Ok(Secret::new(format!(
            "{signing_input}.{}",
            b64url(&signature.to_bytes())
        )))
    }

    /// Verifies a compact JWS at wall-clock second `now_s` and returns its claims.
    pub fn verify(&self, token: &str, now_s: u64) -> Result<AccessClaims, JwtError> {
        if token.len() > MAX_ACCESS_TOKEN_CHARS {
            return Err(JwtError::TooLong);
        }
        let mut parts = token.split('.');
        let (Some(header_b64), Some(claims_b64), Some(signature_b64), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(JwtError::Malformed("expected 3 segments"));
        };
        let header_bytes =
            b64url_decode(header_b64).ok_or(JwtError::Malformed("header is not base64url"))?;
        let header: JwsHeader = serde_json::from_slice(&header_bytes).map_err(JwtError::Json)?;
        if header.alg != JWT_ALGORITHM {
            return Err(JwtError::Algorithm(header.alg));
        }
        if header.typ != JWT_TYPE {
            return Err(JwtError::Malformed("typ is not JWT"));
        }
        if header.kid != self.kid {
            return Err(JwtError::KeyId);
        }
        let signature_bytes = b64url_decode(signature_b64)
            .and_then(|b| <[u8; SIGNATURE_LENGTH]>::try_from(b.as_slice()).ok())
            .ok_or(JwtError::Malformed("signature is not 64 base64url bytes"))?;
        let signature = Signature::from_bytes(&signature_bytes);
        // The signing input is the exact received text of the first two segments.
        let signing_input_len = header_b64.len() + 1 + claims_b64.len();
        let signing_input = token
            .get(..signing_input_len)
            .ok_or(JwtError::Malformed("segment bounds"))?;
        self.verifying
            .verify_strict(signing_input.as_bytes(), &signature)
            .map_err(|_| JwtError::Signature)?;
        let claims_bytes =
            b64url_decode(claims_b64).ok_or(JwtError::Malformed("claims are not base64url"))?;
        let claims: AccessClaims = serde_json::from_slice(&claims_bytes).map_err(JwtError::Json)?;
        if claims.iss != JWT_ISSUER {
            return Err(JwtError::Issuer);
        }
        if claims.iat > now_s.saturating_add(MAX_IAT_SKEW_S) {
            return Err(JwtError::NotYetValid);
        }
        if now_s >= claims.exp {
            return Err(JwtError::Expired);
        }
        Ok(claims)
    }
}

/// A fresh random token id for `jti`.
pub fn new_jti() -> Result<String, RandomError> {
    Ok(b64url(&random_bytes::<JTI_BYTES>()?))
}

/// Number of segments of a compact JWS (used by callers to tell a JWT from an opaque token).
pub fn looks_like_jws(token: &str) -> bool {
    token.split('.').count() == JWS_SEGMENTS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(iat: u64, exp: u64) -> AccessClaims {
        AccessClaims {
            iss: JWT_ISSUER.into(),
            sub: "8f0e7c52-6f3c-4a8e-9a4b-2d1f0c9e7b11".into(),
            role: "engineer".into(),
            fam: "1b4e28ba-2fa1-11d2-883f-0016d3cca427".into(),
            iat,
            exp,
            jti: new_jti().expect("rng"),
        }
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let keys = AccessTokenKeys::generate().expect("rng");
        let c = claims(1_000, 1_900);
        let token = keys.sign(&c).expect("sign");
        assert!(looks_like_jws(token.expose()));
        assert_eq!(keys.verify(token.expose(), 1_000).expect("valid"), c);
        assert!(matches!(
            keys.verify(token.expose(), 1_900),
            Err(JwtError::Expired)
        ));
        assert!(matches!(
            keys.verify(token.expose(), 1_000 - MAX_IAT_SKEW_S - 1),
            Err(JwtError::NotYetValid)
        ));
    }

    #[test]
    fn foreign_key_and_tampering_are_refused() {
        let keys = AccessTokenKeys::generate().expect("rng");
        let other = AccessTokenKeys::generate().expect("rng");
        let token = other.sign(&claims(1_000, 2_000)).expect("sign");
        assert!(matches!(
            keys.verify(token.expose(), 1_500),
            Err(JwtError::KeyId)
        ));

        let token = keys.sign(&claims(1_000, 2_000)).expect("sign");
        let mut parts: Vec<String> = token.expose().split('.').map(str::to_owned).collect();
        // Raise the expiry in the claims without re-signing.
        let mut forged = claims(1_000, 9_999_999);
        forged.jti = "x".into();
        parts[1] = b64url(&serde_json::to_vec(&forged).expect("json"));
        let tampered = parts.join(".");
        assert!(matches!(
            keys.verify(&tampered, 1_500),
            Err(JwtError::Signature)
        ));
    }

    #[test]
    fn other_algorithms_are_refused_before_the_signature() {
        let keys = AccessTokenKeys::generate().expect("rng");
        let header = format!(r#"{{"alg":"none","typ":"JWT","kid":"{}"}}"#, keys.kid());
        let body = serde_json::to_vec(&claims(1, 9)).expect("json");
        let token = format!("{}.{}.", b64url(header.as_bytes()), b64url(&body));
        assert!(matches!(
            keys.verify(&token, 2),
            Err(JwtError::Algorithm(alg)) if alg == "none"
        ));
        // An unknown header field (e.g. `crit`) is refused as well.
        let header = format!(
            r#"{{"alg":"EdDSA","typ":"JWT","kid":"{}","crit":["x"]}}"#,
            keys.kid()
        );
        let token = format!("{}.{}.AA", b64url(header.as_bytes()), b64url(&body));
        assert!(matches!(keys.verify(&token, 2), Err(JwtError::Json(_))));
        assert!(matches!(keys.verify("a.b", 2), Err(JwtError::Malformed(_))));
        assert!(matches!(
            keys.verify(&"x".repeat(MAX_ACCESS_TOKEN_CHARS + 1), 2),
            Err(JwtError::TooLong)
        ));
    }

    #[test]
    fn key_file_roundtrip_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("lmu-relay-jwt-{}", new_jti().expect("rng")));
        fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("jwt.key");
        AccessTokenKeys::create_file(&path).expect("create");
        let a = AccessTokenKeys::load(&path).expect("load");
        let b = AccessTokenKeys::load(&path).expect("load");
        assert_eq!(a.kid(), b.kid());
        assert!(matches!(
            AccessTokenKeys::create_file(&path),
            Err(KeyFileError::Write { .. })
        ));
        fs::write(&path, "not-a-key").expect("write");
        assert!(matches!(
            AccessTokenKeys::load(&path),
            Err(KeyFileError::Format { .. })
        ));
        let _ = fs::remove_dir_all(&dir);
        assert!(!format!("{a:?}").contains("signing"));
    }

    #[test]
    fn derived_keys_follow_the_signing_key_and_the_label() {
        let a = AccessTokenKeys::from_seed([7u8; SECRET_KEY_LENGTH]);
        let same = AccessTokenKeys::from_seed([7u8; SECRET_KEY_LENGTH]);
        let other = AccessTokenKeys::from_seed([8u8; SECRET_KEY_LENGTH]);
        let key = a.derive_key(b"label-1").expect("derive");
        assert_eq!(key, same.derive_key(b"label-1").expect("derive"));
        assert_ne!(key, a.derive_key(b"label-2").expect("derive"));
        assert_ne!(key, other.derive_key(b"label-1").expect("derive"));
        assert_ne!(key, [7u8; SECRET_KEY_LENGTH], "never the seed itself");
    }
}
