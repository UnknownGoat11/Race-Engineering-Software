//! Raw non-finite census: counts NaN / ±Inf per raw LMU field, exactly, before any mapping.
//!
//! The normalizer turns a non-finite raw value into an `Invalid` sample without saying why (its validator
//! never sees it), so NaN/Inf evidence is collected here instead: the decoded raw structs (`RawTelemetry`,
//! `RawScoringInfo`, the player's `RawVehicleScoring`) are walked with a minimal [`serde::Serializer`] that
//! looks only at floats and at reserved (expansion) bytes. Paths use the Rust field names of
//! `lmu_reader::raw_structs` (e.g. `telemetry.wheels[2].temperature[1]`). Memory is bounded by the number
//! of raw fields.

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;
use serde::ser::{self, Impossible};

/// Field names of reserved byte blocks (`ReservedBytes` serializes as a hex string once non-zero).
const RESERVED_FIELDS: [&str; 2] = ["expansion", "unused"];

/// Non-finite occurrences of one raw field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct NonFiniteCount {
    /// NaN values.
    pub nan: u64,
    /// +Inf values.
    pub pos_inf: u64,
    /// −Inf values.
    pub neg_inf: u64,
}

/// Census over a whole capture.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Census {
    /// Float values inspected per root (`telemetry`, `scoring_info`, `player_row`).
    pub floats_checked: BTreeMap<&'static str, u64>,
    /// Structs inspected per root.
    pub structs_checked: BTreeMap<&'static str, u64>,
    /// Non-finite values per raw field path (only fields that had one).
    pub non_finite: BTreeMap<String, NonFiniteCount>,
    /// Reserved / expansion byte blocks seen non-zero, per path (LMU started using reserved bytes?).
    pub reserved_non_zero: BTreeMap<String, u64>,
}

impl Census {
    /// Walks one raw struct under `root`.
    pub fn observe<T: Serialize>(&mut self, root: &'static str, value: &T) {
        *self.structs_checked.entry(root).or_insert(0) += 1;
        let mut walker = Walker {
            census: self,
            root,
            path: Vec::new(),
            floats: 0,
        };
        // The walker never fails on the raw structs; an error would only mean an unsupported shape.
        if let Err(e) = value.serialize(&mut walker) {
            tracing::warn!(component = crate::COMPONENT, root, error = %e, "raw census walk failed");
        }
        let floats = walker.floats;
        *self.floats_checked.entry(root).or_insert(0) += floats;
    }

    /// Total non-finite values.
    pub fn total_non_finite(&self) -> u64 {
        self.non_finite
            .values()
            .map(|c| c.nan + c.pos_inf + c.neg_inf)
            .sum()
    }
}

/// One path segment.
#[derive(Debug, Clone, Copy)]
enum Seg {
    Field(&'static str),
    Index(usize),
}

struct Walker<'c> {
    census: &'c mut Census,
    root: &'static str,
    path: Vec<Seg>,
    floats: u64,
}

impl Walker<'_> {
    fn path_string(&self) -> String {
        let mut s = String::from(self.root);
        for seg in &self.path {
            match seg {
                Seg::Field(name) => {
                    s.push('.');
                    s.push_str(name);
                }
                Seg::Index(i) => {
                    s.push('[');
                    s.push_str(&i.to_string());
                    s.push(']');
                }
            }
        }
        s
    }

    fn float(&mut self, v: f64) {
        self.floats += 1;
        if v.is_finite() {
            return;
        }
        let path = self.path_string();
        let entry = self.census.non_finite.entry(path).or_default();
        if v.is_nan() {
            entry.nan += 1;
        } else if v > 0.0 {
            entry.pos_inf += 1;
        } else {
            entry.neg_inf += 1;
        }
    }

    fn in_reserved_field(&self) -> bool {
        matches!(self.path.last(), Some(Seg::Field(name)) if RESERVED_FIELDS.contains(name))
    }
}

/// Error type of the walker (required by serde; never produced for the raw structs).
#[derive(Debug)]
pub struct WalkError(String);

impl fmt::Display for WalkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WalkError {}

impl ser::Error for WalkError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self(msg.to_string())
    }
}

/// Sequence / tuple walker: numbers its elements.
struct SeqWalker<'w, 'c> {
    walker: &'w mut Walker<'c>,
    index: usize,
}

impl SeqWalker<'_, '_> {
    fn element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), WalkError> {
        self.walker.path.push(Seg::Index(self.index));
        let result = value.serialize(&mut *self.walker);
        self.walker.path.pop();
        self.index += 1;
        result
    }
}

impl ser::SerializeSeq for SeqWalker<'_, '_> {
    type Ok = ();
    type Error = WalkError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), WalkError> {
        self.element(value)
    }
    fn end(self) -> Result<(), WalkError> {
        Ok(())
    }
}

impl ser::SerializeTuple for SeqWalker<'_, '_> {
    type Ok = ();
    type Error = WalkError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), WalkError> {
        self.element(value)
    }
    fn end(self) -> Result<(), WalkError> {
        Ok(())
    }
}

impl ser::SerializeTupleStruct for SeqWalker<'_, '_> {
    type Ok = ();
    type Error = WalkError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), WalkError> {
        self.element(value)
    }
    fn end(self) -> Result<(), WalkError> {
        Ok(())
    }
}

impl ser::SerializeTupleVariant for SeqWalker<'_, '_> {
    type Ok = ();
    type Error = WalkError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), WalkError> {
        self.element(value)
    }
    fn end(self) -> Result<(), WalkError> {
        Ok(())
    }
}

/// Struct walker: names its fields.
struct StructWalker<'w, 'c> {
    walker: &'w mut Walker<'c>,
}

impl StructWalker<'_, '_> {
    fn field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), WalkError> {
        self.walker.path.push(Seg::Field(key));
        let result = value.serialize(&mut *self.walker);
        self.walker.path.pop();
        result
    }
}

impl ser::SerializeStruct for StructWalker<'_, '_> {
    type Ok = ();
    type Error = WalkError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), WalkError> {
        self.field(key, value)
    }
    fn end(self) -> Result<(), WalkError> {
        Ok(())
    }
}

impl ser::SerializeStructVariant for StructWalker<'_, '_> {
    type Ok = ();
    type Error = WalkError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), WalkError> {
        self.field(key, value)
    }
    fn end(self) -> Result<(), WalkError> {
        Ok(())
    }
}

impl<'w, 'c> ser::Serializer for &'w mut Walker<'c> {
    type Ok = ();
    type Error = WalkError;
    type SerializeSeq = SeqWalker<'w, 'c>;
    type SerializeTuple = SeqWalker<'w, 'c>;
    type SerializeTupleStruct = SeqWalker<'w, 'c>;
    type SerializeTupleVariant = SeqWalker<'w, 'c>;
    type SerializeMap = Impossible<(), WalkError>;
    type SerializeStruct = StructWalker<'w, 'c>;
    type SerializeStructVariant = StructWalker<'w, 'c>;

    fn serialize_bool(self, _: bool) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_i8(self, _: i8) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_i16(self, _: i16) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_i32(self, _: i32) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_i64(self, _: i64) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_u8(self, _: u8) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_u16(self, _: u16) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_u32(self, _: u32) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_u64(self, _: u64) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_f32(self, v: f32) -> Result<(), WalkError> {
        self.float(f64::from(v));
        Ok(())
    }
    fn serialize_f64(self, v: f64) -> Result<(), WalkError> {
        self.float(v);
        Ok(())
    }
    fn serialize_char(self, _: char) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_str(self, _: &str) -> Result<(), WalkError> {
        // `ReservedBytes` serializes as a hex string only once its bytes are non-zero.
        if self.in_reserved_field() {
            let path = self.path_string();
            *self.census.reserved_non_zero.entry(path).or_insert(0) += 1;
        }
        Ok(())
    }
    fn serialize_bytes(self, _: &[u8]) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_none(self) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<(), WalkError> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<(), WalkError> {
        Ok(())
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), WalkError> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), WalkError> {
        value.serialize(self)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<SeqWalker<'w, 'c>, WalkError> {
        Ok(SeqWalker {
            walker: self,
            index: 0,
        })
    }
    fn serialize_tuple(self, _: usize) -> Result<SeqWalker<'w, 'c>, WalkError> {
        Ok(SeqWalker {
            walker: self,
            index: 0,
        })
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<SeqWalker<'w, 'c>, WalkError> {
        Ok(SeqWalker {
            walker: self,
            index: 0,
        })
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<SeqWalker<'w, 'c>, WalkError> {
        Ok(SeqWalker {
            walker: self,
            index: 0,
        })
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Impossible<(), WalkError>, WalkError> {
        Err(WalkError(
            "maps are not part of the raw LMU structs".to_owned(),
        ))
    }
    fn serialize_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<StructWalker<'w, 'c>, WalkError> {
        Ok(StructWalker { walker: self })
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<StructWalker<'w, 'c>, WalkError> {
        Ok(StructWalker { walker: self })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lmu_reader::offsets::{scoring_info, telem_info};
    use lmu_reader::raw_structs::{decode_scoring_info, decode_telemetry};

    #[test]
    fn counts_non_finite_values_per_raw_field() {
        let mut t = decode_telemetry(&vec![0; telem_info::SIZE]).expect("zeroed slot decodes");
        t.engine_rpm = f64::NAN;
        t.wheels[2].temperature[1] = f64::INFINITY;
        t.soc = f32::NEG_INFINITY;
        let mut census = Census::default();
        census.observe("telemetry", &t);
        census.observe("telemetry", &t);
        assert_eq!(
            census.non_finite.get("telemetry.engine_rpm"),
            Some(&NonFiniteCount {
                nan: 2,
                pos_inf: 0,
                neg_inf: 0
            })
        );
        assert_eq!(
            census
                .non_finite
                .get("telemetry.wheels[2].temperature[1]")
                .map(|c| c.pos_inf),
            Some(2)
        );
        assert_eq!(
            census.non_finite.get("telemetry.soc").map(|c| c.neg_inf),
            Some(2)
        );
        assert_eq!(census.total_non_finite(), 6);
        assert_eq!(census.structs_checked.get("telemetry"), Some(&2));
        assert!(census.floats_checked.get("telemetry").copied().unwrap_or(0) > 200);
        assert!(census.reserved_non_zero.is_empty());
    }

    #[test]
    fn reserved_bytes_in_use_are_reported() {
        let mut bytes = vec![0; scoring_info::SIZE];
        let start = scoring_info::EXPANSION.offset;
        bytes[start] = 0xAB;
        let info = decode_scoring_info(&bytes).expect("decodes");
        let mut census = Census::default();
        census.observe("scoring_info", &info);
        assert_eq!(
            census.reserved_non_zero.get("scoring_info.expansion"),
            Some(&1)
        );
        assert_eq!(census.total_non_finite(), 0);
    }
}
