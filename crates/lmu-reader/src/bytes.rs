//! Bounds-checked little-endian field reads and string decoding (D-003).
//!
//! Every read names the [`Field`] it reads, checks the field size against the decoded type and checks
//! the byte range against the buffer. No transmute, no `repr(packed)` references, no panics.

use std::borrow::Cow;
use std::fmt;

use serde::{Serialize, Serializer};
use thiserror::Error;

use crate::offsets::{Field, vect3};

/// Decoding a copied block failed. With the fixed block sizes used by the reader this indicates a bug
/// or a corrupted capture file, never a property of live data.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// The block handed to a decoder does not have the struct size.
    #[error("{context}: block is {actual} bytes, expected exactly {expected}")]
    WrongSize {
        /// Struct being decoded.
        context: &'static str,
        /// Expected size in bytes.
        expected: usize,
        /// Actual size in bytes.
        actual: usize,
    },
    /// A read would leave the block.
    #[error("{context}: read of {len} bytes at offset {offset} exceeds the {available}-byte block")]
    OutOfBounds {
        /// Struct being decoded.
        context: &'static str,
        /// Offset of the read.
        offset: usize,
        /// Length of the read.
        len: usize,
        /// Size of the block.
        available: usize,
    },
    /// A field constant does not have the size the decoder expects for its type.
    #[error(
        "{context}: field at offset {offset} is {field_size} bytes but its decoder reads {expected}"
    )]
    FieldSize {
        /// Struct being decoded.
        context: &'static str,
        /// Offset of the field.
        offset: usize,
        /// Size from the offset table.
        field_size: usize,
        /// Size implied by the Rust type.
        expected: usize,
    },
}

/// A `TelemVect3`: three raw `f64` components in the frame documented on the field that holds it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Vec3 {
    /// `x` component.
    pub x: f64,
    /// `y` component.
    pub y: f64,
    /// `z` component.
    pub z: f64,
}

/// A fixed-size C `char[N]` decoded within its array bounds (A-LMU-045).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RawString {
    /// Bytes up to the first NUL (or the whole array if there is none), decoded as lossy UTF-8.
    pub text: String,
    /// `false` if the text contains control characters or invalid UTF-8 (replaced by U+FFFD).
    pub printable: bool,
    /// `false` if the array contained no NUL terminator (the text fills the whole array).
    pub nul_terminated: bool,
}

impl RawString {
    /// Decodes a `char[N]` array: stops at the first NUL, never reads past `bytes`.
    pub fn decode(bytes: &[u8]) -> Self {
        let nul = bytes.iter().position(|&b| b == 0);
        let content = match nul {
            Some(end) => bytes.get(..end).unwrap_or(bytes),
            None => bytes,
        };
        let text = String::from_utf8_lossy(content);
        let lossy = matches!(text, Cow::Owned(_));
        let printable = !lossy && !text.chars().any(char::is_control);
        Self {
            text: text.into_owned(),
            printable,
            nul_terminated: nul.is_some(),
        }
    }

    /// The decoded text.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// `true` if the decoded text is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

impl fmt::Display for RawString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Reserved / expansion bytes kept only for diagnostics (channel map section 20). They serialize as
/// `null` while all zero and as a lowercase hex string once LMU starts using them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedBytes(Vec<u8>);

impl ReservedBytes {
    /// Wraps a copy of the given bytes.
    pub fn new(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }

    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// `true` while LMU leaves the reserved bytes zeroed.
    pub fn is_all_zero(&self) -> bool {
        self.0.iter().all(|&b| b == 0)
    }

    /// Lowercase hex dump.
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }
}

impl Serialize for ReservedBytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.is_all_zero() {
            serializer.serialize_none()
        } else {
            serializer.serialize_str(&self.to_hex())
        }
    }
}

/// Lowercase hex dump of a byte slice.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // Writing into a String cannot fail.
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Size of one `f64` component of a `TelemVect3`.
const F64_SIZE: usize = std::mem::size_of::<f64>();

/// Reads typed little-endian fields from one struct-sized block.
pub(crate) struct FieldReader<'a> {
    buf: &'a [u8],
    context: &'static str,
}

macro_rules! scalar_reads {
    ($($name:ident: $ty:ty),* $(,)?) => {
        $(
            #[doc = concat!("Reads one little-endian `", stringify!($ty), "` field.")]
            pub(crate) fn $name(&self, field: Field) -> Result<$ty, DecodeError> {
                const N: usize = std::mem::size_of::<$ty>();
                self.expect_size(field, N)?;
                Ok(<$ty>::from_le_bytes(self.bytes::<N>(field.offset)?))
            }
        )*
    };
}

macro_rules! array_reads {
    ($($name:ident: $ty:ty),* $(,)?) => {
        $(
            #[doc = concat!("Reads a `[", stringify!($ty), "; N]` field element by element.")]
            pub(crate) fn $name<const N: usize>(&self, field: Field) -> Result<[$ty; N], DecodeError> {
                const ELEM: usize = std::mem::size_of::<$ty>();
                self.expect_size(field, ELEM * N)?;
                let mut out = [<$ty>::default(); N];
                for (i, slot) in out.iter_mut().enumerate() {
                    *slot = <$ty>::from_le_bytes(self.bytes::<ELEM>(field.offset + i * ELEM)?);
                }
                Ok(out)
            }
        )*
    };
}

impl<'a> FieldReader<'a> {
    /// Wraps a block that must be exactly `expected` bytes long.
    pub(crate) fn new(
        buf: &'a [u8],
        expected: usize,
        context: &'static str,
    ) -> Result<Self, DecodeError> {
        if buf.len() != expected {
            return Err(DecodeError::WrongSize {
                context,
                expected,
                actual: buf.len(),
            });
        }
        Ok(Self { buf, context })
    }

    fn out_of_bounds(&self, offset: usize, len: usize) -> DecodeError {
        DecodeError::OutOfBounds {
            context: self.context,
            offset,
            len,
            available: self.buf.len(),
        }
    }

    fn expect_size(&self, field: Field, expected: usize) -> Result<(), DecodeError> {
        if field.size == expected {
            Ok(())
        } else {
            Err(DecodeError::FieldSize {
                context: self.context,
                offset: field.offset,
                field_size: field.size,
                expected,
            })
        }
    }

    /// The bytes of a whole field (for nested structs and strings).
    pub(crate) fn sub(&self, field: Field) -> Result<&'a [u8], DecodeError> {
        let end = field
            .offset
            .checked_add(field.size)
            .ok_or_else(|| self.out_of_bounds(field.offset, field.size))?;
        self.buf
            .get(field.offset..end)
            .ok_or_else(|| self.out_of_bounds(field.offset, field.size))
    }

    fn bytes<const N: usize>(&self, offset: usize) -> Result<[u8; N], DecodeError> {
        let end = offset
            .checked_add(N)
            .ok_or_else(|| self.out_of_bounds(offset, N))?;
        let slice = self
            .buf
            .get(offset..end)
            .ok_or_else(|| self.out_of_bounds(offset, N))?;
        <[u8; N]>::try_from(slice).map_err(|_| self.out_of_bounds(offset, N))
    }

    scalar_reads!(u8: u8, i8: i8, i16: i16, u16: u16, i32: i32, u32: u32, f32: f32, f64: f64, u64: u64);
    array_reads!(u8_array: u8, i8_array: i8, u32_array: u32, f32_array: f32, f64_array: f64);

    /// Reads a C++ `bool` byte (any non-zero value is `true`; layout checks flag values other than 0/1).
    pub(crate) fn bool(&self, field: Field) -> Result<bool, DecodeError> {
        Ok(self.u8(field)? != 0)
    }

    /// Reads a `TelemVect3`.
    pub(crate) fn vec3(&self, field: Field) -> Result<Vec3, DecodeError> {
        self.expect_size(field, vect3::SIZE)?;
        self.vec3_at(field.offset)
    }

    fn vec3_at(&self, offset: usize) -> Result<Vec3, DecodeError> {
        Ok(Vec3 {
            x: f64::from_le_bytes(self.bytes::<F64_SIZE>(offset + vect3::X.offset)?),
            y: f64::from_le_bytes(self.bytes::<F64_SIZE>(offset + vect3::Y.offset)?),
            z: f64::from_le_bytes(self.bytes::<F64_SIZE>(offset + vect3::Z.offset)?),
        })
    }

    /// Reads a `TelemVect3[N]`.
    pub(crate) fn vec3_array<const N: usize>(
        &self,
        field: Field,
    ) -> Result<[Vec3; N], DecodeError> {
        self.expect_size(field, vect3::SIZE * N)?;
        let mut out = [Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }; N];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self.vec3_at(field.offset + i * vect3::SIZE)?;
        }
        Ok(out)
    }

    /// Reads a `char[N]` field.
    pub(crate) fn string(&self, field: Field) -> Result<RawString, DecodeError> {
        Ok(RawString::decode(self.sub(field)?))
    }

    /// Reads reserved bytes.
    pub(crate) fn reserved(&self, field: Field) -> Result<ReservedBytes, DecodeError> {
        Ok(ReservedBytes::new(self.sub(field)?))
    }
}

/// Little-endian scalar read at an absolute position of a byte slice (block-level helpers).
pub(crate) fn read_le<const N: usize>(buf: &[u8], offset: usize) -> Option<[u8; N]> {
    let end = offset.checked_add(N)?;
    buf.get(offset..end)
        .and_then(|s| <[u8; N]>::try_from(s).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_stops_at_nul_and_flags_termination() {
        let s = RawString::decode(b"Le Mans\0garbage");
        assert_eq!(s.text, "Le Mans");
        assert!(s.printable && s.nul_terminated);

        let full = RawString::decode(b"ABCD");
        assert_eq!(full.text, "ABCD");
        assert!(full.printable);
        assert!(
            !full.nul_terminated,
            "no NUL inside the array must be flagged"
        );

        let empty = RawString::decode(&[0u8; 8]);
        assert!(empty.is_empty() && empty.printable && empty.nul_terminated);
    }

    #[test]
    fn string_is_lossy_utf8_and_flags_non_printable() {
        let utf8 = RawString::decode("Jos\u{e9} M\u{fc}ller\0".as_bytes());
        assert_eq!(utf8.text, "Jos\u{e9} M\u{fc}ller");
        assert!(utf8.printable);

        let invalid = RawString::decode(&[b'A', 0xFF, b'B', 0]);
        assert_eq!(invalid.text, "A\u{fffd}B");
        assert!(!invalid.printable, "invalid UTF-8 must be flagged");

        let control = RawString::decode(&[b'A', 0x01, b'B', 0]);
        assert!(!control.printable, "control characters must be flagged");
    }

    #[test]
    fn reader_rejects_wrong_block_size_and_wrong_field_size() {
        let buf = [0u8; 8];
        assert!(matches!(
            FieldReader::new(&buf, 9, "T"),
            Err(DecodeError::WrongSize {
                expected: 9,
                actual: 8,
                ..
            })
        ));
        let r = FieldReader::new(&buf, 8, "T").unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(
            r.f64(Field::new(0, 4)),
            Err(DecodeError::FieldSize { .. })
        ));
        assert!(matches!(
            r.u32(Field::new(6, 4)),
            Err(DecodeError::OutOfBounds {
                offset: 6,
                len: 4,
                ..
            })
        ));
        assert!(matches!(
            r.sub(Field::new(usize::MAX, 2)),
            Err(DecodeError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn scalar_and_array_reads_are_little_endian_and_keep_nan() {
        let mut buf = [0u8; 32];
        buf[0..4].copy_from_slice(&(-7i32).to_le_bytes());
        buf[4..12].copy_from_slice(&f64::NAN.to_le_bytes());
        buf[12..16].copy_from_slice(&f32::INFINITY.to_le_bytes());
        buf[16] = 2;
        buf[17] = 0xFE;
        buf[24..32].copy_from_slice(&0x0102_0304_0506_0708u64.to_le_bytes());
        let r = FieldReader::new(&buf, 32, "T").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.i32(Field::new(0, 4)), Ok(-7));
        assert!(r.f64(Field::new(4, 8)).is_ok_and(f64::is_nan));
        assert_eq!(r.f32(Field::new(12, 4)), Ok(f32::INFINITY));
        assert_eq!(r.bool(Field::new(16, 1)), Ok(true));
        assert_eq!(r.i8(Field::new(17, 1)), Ok(-2));
        assert_eq!(r.u64(Field::new(24, 8)), Ok(0x0102_0304_0506_0708));
        assert_eq!(r.u8_array::<2>(Field::new(16, 2)), Ok([2, 0xFE]));
        assert!(
            r.f64_array::<2>(Field::new(24, 8)).is_err(),
            "array size must match the field size"
        );
    }

    #[test]
    fn reserved_bytes_serialize_null_when_zero() {
        let zero = ReservedBytes::new(&[0, 0, 0]);
        assert_eq!(serde_json::to_string(&zero).ok().as_deref(), Some("null"));
        let used = ReservedBytes::new(&[0, 0xAB]);
        assert_eq!(
            serde_json::to_string(&used).ok().as_deref(),
            Some("\"00ab\"")
        );
        assert!(!used.is_all_zero());
    }
}
