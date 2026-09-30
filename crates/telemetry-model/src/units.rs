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
    /// 0.0..=1.0
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
        }
    }
}

/// Standard gravity, m/s² (ISO 80000-3).
pub const STANDARD_GRAVITY: f64 = 9.806_65;
/// Offset between Kelvin and Celsius.
pub const KELVIN_OFFSET: f64 = 273.15;
/// Pascals per kilopascal.
pub const PA_PER_KPA: f64 = 1_000.0;

/// Raw-source → internal conversions. The normalizer must use these and nothing else.
pub mod convert {
    use super::{KELVIN_OFFSET, PA_PER_KPA, STANDARD_GRAVITY};

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
}

#[cfg(test)]
mod tests {
    use super::convert::*;

    #[test]
    fn conversions() {
        assert!((kelvin_to_celsius(373.15) - 100.0).abs() < 1e-9);
        assert!((kpa_to_pa(180.0) - 180_000.0).abs() < 1e-9);
        assert!((accel_to_g(9.806_65) - 1.0).abs() < 1e-12);
        assert!((magnitude(3.0, 4.0, 0.0) - 5.0).abs() < 1e-12);
    }
}
