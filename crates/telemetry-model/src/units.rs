//! Unit policy (spec §105): SI internally, conversions in exactly one place.
//!
//! Internal units: metre, metre/second, pascal, degree Celsius (temperatures are displayed in °C and
//! LMU mixes Kelvin/Celsius, so Celsius is the canonical internal temperature unit), radian, litre,
//! second, newton, watt, joule, rpm (engine speed is conventionally rpm), fraction (0..1).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    None,
    Metre,
    MetrePerSecond,
    MetrePerSecond2,
    Pascal,
    Celsius,
    Radian,
    RadianPerSecond,
    Litre,
    LitrePerLap,
    Second,
    Newton,
    NewtonMetre,
    Watt,
    Joule,
    Rpm,
    /// Dimensionless ratio, normally 0.0..=1.0 (signed -1.0..=1.0 for steering input).
    Fraction,
    /// Standard gravity multiples (derived G-force channels).
    G,
    Lap,
    Count,
    Enum,
    Text,
    Nanosecond,
    BitsPerSecond,
    Hertz,
    /// Angular acceleration.
    RadianPerSecond2,
    /// Fraction of a resource (e.g. virtual energy) used per lap.
    FractionPerLap,
    /// Data size (recording/disk diagnostics).
    Byte,
    /// Density (air density).
    KilogramPerCubicMetre,
}

impl Unit {
    /// Short symbol for the internal unit.
    pub fn symbol(self) -> &'static str {
        match self {
            Unit::None | Unit::Enum | Unit::Text | Unit::Count => "",
            Unit::Metre => "m",
            Unit::MetrePerSecond => "m/s",
            Unit::MetrePerSecond2 => "m/s²",
            Unit::Pascal => "Pa",
            Unit::Celsius => "°C",
            Unit::Radian => "rad",
            Unit::RadianPerSecond => "rad/s",
            Unit::Litre => "L",
            Unit::LitrePerLap => "L/lap",
            Unit::Second => "s",
            Unit::Newton => "N",
            Unit::NewtonMetre => "N·m",
            Unit::Watt => "W",
            Unit::Joule => "J",
            Unit::Rpm => "rpm",
            Unit::Fraction => "",
            Unit::G => "g",
            Unit::Lap => "lap",
            Unit::Nanosecond => "ns",
            Unit::BitsPerSecond => "bit/s",
            Unit::Hertz => "Hz",
            Unit::RadianPerSecond2 => "rad/s²",
            Unit::FractionPerLap => "/lap",
            Unit::Byte => "B",
            Unit::KilogramPerCubicMetre => "kg/m³",
        }
    }
}

/// Standard gravity, m/s² (ISO 80000-3).
pub const STANDARD_GRAVITY: f64 = 9.806_65;
/// Offset between Kelvin and Celsius.
pub const KELVIN_OFFSET: f64 = 273.15;
/// Pascals per kilopascal.
pub const PA_PER_KPA: f64 = 1_000.0;
/// Watts per kilowatt.
pub const W_PER_KW: f64 = 1_000.0;
/// Centimetres per metre.
pub const CM_PER_M: f64 = 100.0;
/// Radians per degree.
pub const RAD_PER_DEG: f64 = std::f64::consts::PI / 180.0;
/// Full scale of an unsigned byte used as a 0..=1 fraction (e.g. `mFuelFraction`: 0x00 = 0 %, 0xFF = 100 %).
pub const U8_FRACTION_FULL_SCALE: f64 = 255.0;

/// Raw-source → internal conversions. The normalizer must use these and nothing else.
pub mod convert {
    use super::{
        CM_PER_M, KELVIN_OFFSET, PA_PER_KPA, RAD_PER_DEG, STANDARD_GRAVITY, U8_FRACTION_FULL_SCALE,
        W_PER_KW,
    };

    #[inline]
    pub fn kelvin_to_celsius(k: f64) -> f64 {
        k - KELVIN_OFFSET
    }
    #[inline]
    pub fn kpa_to_pa(kpa: f64) -> f64 {
        kpa * PA_PER_KPA
    }
    #[inline]
    pub fn accel_to_g(mps2: f64) -> f64 {
        mps2 / STANDARD_GRAVITY
    }
    /// Magnitude of a 3-vector.
    #[inline]
    pub fn magnitude(x: f64, y: f64, z: f64) -> f64 {
        (x * x + y * y + z * z).sqrt()
    }
    #[inline]
    pub fn kw_to_w(kw: f64) -> f64 {
        kw * W_PER_KW
    }
    #[inline]
    pub fn cm_to_m(cm: f64) -> f64 {
        cm / CM_PER_M
    }
    #[inline]
    pub fn deg_to_rad(deg: f64) -> f64 {
        deg * RAD_PER_DEG
    }
    /// Byte-encoded fraction (0 → 0.0, 255 → 1.0).
    #[inline]
    pub fn u8_to_fraction(raw: u8) -> f64 {
        f64::from(raw) / U8_FRACTION_FULL_SCALE
    }
}

#[cfg(test)]
mod tests {
    use super::Unit;
    use super::convert::*;

    #[test]
    fn conversions() {
        assert!((kelvin_to_celsius(373.15) - 100.0).abs() < 1e-9);
        assert!((kpa_to_pa(180.0) - 180_000.0).abs() < 1e-9);
        assert!((accel_to_g(9.806_65) - 1.0).abs() < 1e-12);
        assert!((magnitude(3.0, 4.0, 0.0) - 5.0).abs() < 1e-12);
        assert!((kw_to_w(120.0) - 120_000.0).abs() < 1e-9);
        assert!((cm_to_m(33.0) - 0.33).abs() < 1e-12);
        assert!((deg_to_rad(180.0) - std::f64::consts::PI).abs() < 1e-12);
        assert!((u8_to_fraction(255) - 1.0).abs() < 1e-12);
        assert!(u8_to_fraction(0).abs() < 1e-12);
    }

    #[test]
    fn new_units_have_symbols() {
        assert_eq!(Unit::RadianPerSecond2.symbol(), "rad/s²");
        assert_eq!(Unit::Byte.symbol(), "B");
    }
}
