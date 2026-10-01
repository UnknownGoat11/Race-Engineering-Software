//! Scalar channel values and per-sample quality.
//!
//! These types are the currency shared by the normalizer, protocol TLV codec, recorder and UI bridge.
//! A missing channel is represented by `Quality::Unavailable` / `Value::None` — never by a zero.

use serde::{Deserialize, Serialize};

/// Wire/storage value type tag (low 5 bits of the TLV tag byte, see docs/PROTOCOL.md §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ValueType {
    None = 0,
    F32 = 1,
    F64 = 2,
    I32 = 3,
    U32 = 4,
    Bool = 5,
    U8 = 6,
    I8 = 7,
    Str = 8,
    U64 = 9,
}

impl ValueType {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::None,
            1 => Self::F32,
            2 => Self::F64,
            3 => Self::I32,
            4 => Self::U32,
            5 => Self::Bool,
            6 => Self::U8,
            7 => Self::I8,
            8 => Self::Str,
            9 => Self::U64,
            _ => return None,
        })
    }
}

/// Maximum encoded length of a string value (bytes of UTF-8).
pub const MAX_STR_VALUE_BYTES: usize = 64;

/// A single channel value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    None,
    F32(f32),
    F64(f64),
    I32(i32),
    U32(u32),
    Bool(bool),
    U8(u8),
    I8(i8),
    Str(String),
    U64(u64),
}

impl Value {
    pub fn value_type(&self) -> ValueType {
        match self {
            Value::None => ValueType::None,
            Value::F32(_) => ValueType::F32,
            Value::F64(_) => ValueType::F64,
            Value::I32(_) => ValueType::I32,
            Value::U32(_) => ValueType::U32,
            Value::Bool(_) => ValueType::Bool,
            Value::U8(_) => ValueType::U8,
            Value::I8(_) => ValueType::I8,
            Value::Str(_) => ValueType::Str,
            Value::U64(_) => ValueType::U64,
        }
    }

    /// Numeric view used by graphs/exports. Strings and `None` yield `None`.
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Value::F32(v) => Some(v as f64),
            Value::F64(v) => Some(v),
            Value::I32(v) => Some(v as f64),
            Value::U32(v) => Some(v as f64),
            Value::Bool(v) => Some(if v { 1.0 } else { 0.0 }),
            Value::U8(v) => Some(v as f64),
            Value::I8(v) => Some(v as f64),
            Value::U64(v) => Some(v as f64),
            Value::None | Value::Str(_) => None,
        }
    }
}

/// Per-sample quality (high 3 bits of the TLV tag byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[repr(u8)]
pub enum Quality {
    #[default]
    Valid = 0,
    /// Value was read but failed validation (NaN, out of range, impossible jump…).
    Invalid = 1,
    /// Previously valid, not refreshed within its staleness budget.
    Stale = 2,
    /// Source exists in principle but provides no value right now (e.g. hybrid on a GT3 car).
    Unavailable = 3,
    /// The current source can never provide this channel.
    NotSupported = 4,
}

impl Quality {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Valid,
            1 => Self::Invalid,
            2 => Self::Stale,
            3 => Self::Unavailable,
            4 => Self::NotSupported,
            _ => return None,
        })
    }
}

/// A value together with its quality. `Sample::unavailable()` is the canonical "no data".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub value: Value,
    pub quality: Quality,
}

impl Sample {
    pub fn valid(value: Value) -> Self {
        Self {
            value,
            quality: Quality::Valid,
        }
    }
    pub fn unavailable() -> Self {
        Self {
            value: Value::None,
            quality: Quality::Unavailable,
        }
    }
    pub fn invalid() -> Self {
        Self {
            value: Value::None,
            quality: Quality::Invalid,
        }
    }
    pub fn not_supported() -> Self {
        Self {
            value: Value::None,
            quality: Quality::NotSupported,
        }
    }
    /// Previously valid but not refreshed within its staleness budget. Carries no value (a stale value
    /// must never be mistaken for a measurement downstream).
    pub fn stale() -> Self {
        Self {
            value: Value::None,
            quality: Quality::Stale,
        }
    }
    pub fn is_valid(&self) -> bool {
        self.quality == Quality::Valid && self.value != Value::None
    }
}
