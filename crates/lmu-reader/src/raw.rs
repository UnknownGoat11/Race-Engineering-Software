//! Raw enums and small raw-semantics helpers that sit next to the generated structs.
//!
//! These only *name* raw values; they never convert units or guess. Unknown raw values are kept as
//! `Other(raw)` so nothing is lost.

use serde::Serialize;

use crate::offsets::{sme, vehicle_championship, vehicle_class};

/// A `SharedMemoryEvent` code from the `generic.events[]` queue.
///
/// The queue is a list of codes, not 16 flags, and `SME_ENTER` (0) is indistinguishable from an empty
/// slot (A-LMU-009). Do not use it for freshness; the reader uses ET changes instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum SharedMemoryEvent {
    /// `SME_ENTER` (also what an empty slot reads as).
    Enter,
    /// `SME_EXIT`.
    Exit,
    /// `SME_STARTUP`.
    Startup,
    /// `SME_SHUTDOWN`.
    Shutdown,
    /// `SME_LOAD`.
    Load,
    /// `SME_UNLOAD`.
    Unload,
    /// `SME_START_SESSION`.
    StartSession,
    /// `SME_END_SESSION`.
    EndSession,
    /// `SME_ENTER_REALTIME`.
    EnterRealtime,
    /// `SME_EXIT_REALTIME`.
    ExitRealtime,
    /// `SME_UPDATE_SCORING`.
    UpdateScoring,
    /// `SME_UPDATE_TELEMETRY`.
    UpdateTelemetry,
    /// `SME_INIT_APPLICATION`.
    InitApplication,
    /// `SME_UNINIT_APPLICATION`.
    UninitApplication,
    /// `SME_SET_ENVIRONMENT`.
    SetEnvironment,
    /// `SME_FFB`.
    Ffb,
    /// Any other value (not a known code; layout or version change?).
    Other(u32),
}

impl SharedMemoryEvent {
    /// Names a raw queue value.
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            sme::ENTER => Self::Enter,
            sme::EXIT => Self::Exit,
            sme::STARTUP => Self::Startup,
            sme::SHUTDOWN => Self::Shutdown,
            sme::LOAD => Self::Load,
            sme::UNLOAD => Self::Unload,
            sme::START_SESSION => Self::StartSession,
            sme::END_SESSION => Self::EndSession,
            sme::ENTER_REALTIME => Self::EnterRealtime,
            sme::EXIT_REALTIME => Self::ExitRealtime,
            sme::UPDATE_SCORING => Self::UpdateScoring,
            sme::UPDATE_TELEMETRY => Self::UpdateTelemetry,
            sme::INIT_APPLICATION => Self::InitApplication,
            sme::UNINIT_APPLICATION => Self::UninitApplication,
            sme::SET_ENVIRONMENT => Self::SetEnvironment,
            sme::FFB => Self::Ffb,
            other => Self::Other(other),
        }
    }

    /// `true` for a value below `SME_MAX`.
    pub fn is_known(self) -> bool {
        !matches!(self, Self::Other(_))
    }
}

/// `IP_VehicleClass` (telemetry `mVehicleClass`, A-LMU-055).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum VehicleClass {
    /// Hypercar.
    Hypercar,
    /// LMP2 (ELMS spec).
    Lmp2Elms,
    /// LMP2.
    Lmp2,
    /// LMP3.
    Lmp3,
    /// GTE.
    Gte,
    /// GT3.
    Gt3,
    /// Pace / safety car.
    PaceCar,
    /// `Unknown` (255) as reported by LMU.
    Unknown,
    /// A value the probed header does not define.
    Other(u8),
}

impl VehicleClass {
    /// Names a raw value.
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            vehicle_class::HYPERCAR => Self::Hypercar,
            vehicle_class::LMP2_ELMS => Self::Lmp2Elms,
            vehicle_class::LMP2 => Self::Lmp2,
            vehicle_class::LMP3 => Self::Lmp3,
            vehicle_class::GTE => Self::Gte,
            vehicle_class::GT3 => Self::Gt3,
            vehicle_class::PACE_CAR => Self::PaceCar,
            vehicle_class::UNKNOWN => Self::Unknown,
            other => Self::Other(other),
        }
    }
}

/// `IP_VehicleChampionship` (telemetry `mVehicleChampionship`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum VehicleChampionship {
    /// WEC 2023.
    Wec2023,
    /// WEC 2024.
    Wec2024,
    /// WEC 2025.
    Wec2025,
    /// WEC 2026.
    Wec2026,
    /// ELMS 2025.
    Elms2025,
    /// ELMS 2026.
    Elms2026,
    /// `Unknown` (255) as reported by LMU.
    Unknown,
    /// A value the probed header does not define.
    Other(u8),
}

impl VehicleChampionship {
    /// Names a raw value.
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            vehicle_championship::WEC_2023 => Self::Wec2023,
            vehicle_championship::WEC_2024 => Self::Wec2024,
            vehicle_championship::WEC_2025 => Self::Wec2025,
            vehicle_championship::WEC_2026 => Self::Wec2026,
            vehicle_championship::ELMS_2025 => Self::Elms2025,
            vehicle_championship::ELMS_2026 => Self::Elms2026,
            vehicle_championship::UNKNOWN => Self::Unknown,
            other => Self::Other(other),
        }
    }
}

/// Bit 31 of `TelemInfoV01.mCurrentSector`: set while the car is in the pit lane (A-LMU-026).
pub const CURRENT_SECTOR_PIT_BIT: u32 = 0x8000_0000;

/// Splits raw `mCurrentSector` into (zero-based sector bits, pit-lane flag), e.g. `0x8000_0002` is
/// sector index 2 while entering the pits.
pub fn split_current_sector(raw: i32) -> (u32, bool) {
    let bits = u32::from_le_bytes(raw.to_le_bytes());
    (
        bits & !CURRENT_SECTOR_PIT_BIT,
        bits & CURRENT_SECTOR_PIT_BIT != 0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_named_and_unknown_values_kept() {
        assert_eq!(
            SharedMemoryEvent::from_raw(11),
            SharedMemoryEvent::UpdateTelemetry
        );
        assert_eq!(
            SharedMemoryEvent::from_raw(10),
            SharedMemoryEvent::UpdateScoring
        );
        assert_eq!(SharedMemoryEvent::from_raw(0), SharedMemoryEvent::Enter);
        assert_eq!(
            SharedMemoryEvent::from_raw(16),
            SharedMemoryEvent::Other(16)
        );
        assert!(!SharedMemoryEvent::from_raw(99).is_known());
    }

    #[test]
    fn vehicle_enums_keep_unknown_values() {
        assert_eq!(VehicleClass::from_raw(0), VehicleClass::Hypercar);
        assert_eq!(VehicleClass::from_raw(6), VehicleClass::Gt3);
        assert_eq!(VehicleClass::from_raw(1), VehicleClass::Other(1));
        assert_eq!(VehicleClass::from_raw(255), VehicleClass::Unknown);
        assert_eq!(
            VehicleChampionship::from_raw(17),
            VehicleChampionship::Elms2026
        );
        assert_eq!(
            VehicleChampionship::from_raw(4),
            VehicleChampionship::Other(4)
        );
    }

    #[test]
    fn current_sector_pit_bit() {
        assert_eq!(split_current_sector(1), (1, false));
        assert_eq!(
            split_current_sector(i32::from_le_bytes(0x8000_0002u32.to_le_bytes())),
            (2, true)
        );
    }
}
