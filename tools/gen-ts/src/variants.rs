//! Enumerates the variants of the model's unit-only enums without re-typing them.
//!
//! Every enum exported to TypeScript derives `serde::Deserialize`, and serde lets a unit-only enum be
//! deserialized from its variant *index*. Asking for index 0, 1, 2, … until serde refuses yields every
//! variant in declaration order, so a variant added in Rust appears in the generated tables automatically.

use std::fmt::Debug;

use serde::Deserialize;
use serde::de::value::{Error as ValueError, U32Deserializer};

/// Every variant of the unit-only enum `T`, in declaration order.
pub fn all<T>() -> Vec<T>
where
    T: for<'de> Deserialize<'de>,
{
    let mut out = Vec::new();
    let mut index: u32 = 0;
    while let Ok(variant) = T::deserialize(U32Deserializer::<ValueError>::new(index)) {
        out.push(variant);
        index += 1;
    }
    assert!(
        !out.is_empty(),
        "enum {} produced no variants from serde variant indices",
        std::any::type_name::<T>()
    );
    out
}

/// Rust variant name (the derived `Debug` output of a unit variant).
pub fn name<T: Debug>(variant: T) -> String {
    let name = format!("{variant:?}");
    assert!(
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()),
        "variant {name:?} of {} is not a plain identifier",
        std::any::type_name::<T>()
    );
    name
}

#[cfg(test)]
mod tests {
    use telemetry_model::{Quality, Unit, ValueType};

    use super::*;

    #[test]
    fn enumerates_in_declaration_order() {
        let types = all::<ValueType>();
        assert_eq!(types.first(), Some(&ValueType::None));
        assert_eq!(types.last(), Some(&ValueType::U64));
        for (i, vt) in types.iter().enumerate() {
            assert_eq!(*vt as usize, i, "ValueType codes are dense and in order");
        }
        assert_eq!(all::<Quality>().len(), 5);
        let units = all::<Unit>();
        assert_eq!(units.first(), Some(&Unit::None));
        assert!(units.contains(&Unit::KilogramPerCubicMetre));
        assert_eq!(name(Unit::MetrePerSecond), "MetrePerSecond");
    }
}
