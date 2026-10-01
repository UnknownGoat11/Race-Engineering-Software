//! Test-only builder for synthetic `LMU_Data` images.
//!
//! Compiled for this crate's tests and, behind the `test-support` feature, for other crates' tests
//! (e.g. normalizer fixtures). It writes little-endian values at the probed offsets of
//! [`crate::offsets`]; it never touches LMU and its output is never presented as real telemetry.
//! Writes outside the layout panic, which is the desired behaviour in a test.

use crate::offsets::{
    Field, MAX_SCORING_VEHICLES, MAX_TELEMETRY_VEHICLES, WHEEL_COUNT, abs, generic, layout,
    scoring_info, telem_info, telem_wheel, telemetry_data, veh_scoring,
};

/// A value with a fixed little-endian encoding.
pub trait LeValue: Copy {
    /// Encoded size in bytes.
    const SIZE: usize;
    /// Writes the encoding into `dst` (exactly [`LeValue::SIZE`] bytes).
    fn write_le(self, dst: &mut [u8]);
}

macro_rules! le_value {
    ($($t:ty),* $(,)?) => {
        $(
            impl LeValue for $t {
                const SIZE: usize = std::mem::size_of::<$t>();
                fn write_le(self, dst: &mut [u8]) {
                    dst.copy_from_slice(&self.to_le_bytes());
                }
            }
        )*
    };
}

le_value!(i8, u8, i16, u16, i32, u32, u64, f32, f64);

impl LeValue for bool {
    const SIZE: usize = 1;
    fn write_le(self, dst: &mut [u8]) {
        dst.copy_from_slice(&[u8::from(self)]);
    }
}

/// Synthetic `LMU_Data` image of `sizeof(SharedMemoryLayout)` bytes, zero-filled like a fresh mapping.
#[derive(Debug, Clone)]
pub struct LayoutBuilder {
    buf: Vec<u8>,
}

impl Default for LayoutBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutBuilder {
    /// A zero-filled image (what a freshly created mapping looks like).
    pub fn new() -> Self {
        Self {
            buf: vec![0; layout::SIZE],
        }
    }

    /// A plausible "player on track" frame; see [`LayoutBuilder::on_track`].
    pub fn on_track_frame() -> Self {
        let mut b = Self::new();
        b.on_track();
        b
    }

    /// The image bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Consumes the builder, returning the image.
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// Truncates the image (to simulate a mapping smaller than the layout).
    pub fn truncate(&mut self, len: usize) -> &mut Self {
        self.buf.truncate(len);
        self
    }

    /// Writes raw bytes at an absolute offset.
    pub fn put(&mut self, offset: usize, bytes: &[u8]) -> &mut Self {
        self.buf[offset..offset + bytes.len()].copy_from_slice(bytes);
        self
    }

    /// Writes a scalar into a field of the struct starting at absolute `base`.
    pub fn set<T: LeValue>(&mut self, base: usize, field: Field, value: T) -> &mut Self {
        assert_eq!(
            field.size,
            T::SIZE,
            "field at +{} has size {}, value has {}",
            field.offset,
            field.size,
            T::SIZE
        );
        self.set_elem(base, field, 0, value)
    }

    /// Writes element `index` of an array field of the struct starting at absolute `base`.
    pub fn set_elem<T: LeValue>(
        &mut self,
        base: usize,
        field: Field,
        index: usize,
        value: T,
    ) -> &mut Self {
        assert!(
            (index + 1) * T::SIZE <= field.size,
            "element {index} is outside the field at +{}",
            field.offset
        );
        let start = base + field.offset + index * T::SIZE;
        value.write_le(&mut self.buf[start..start + T::SIZE]);
        self
    }

    /// Writes a NUL-terminated string into a `char[N]` field (asserts that it fits with its NUL).
    pub fn set_str(&mut self, base: usize, field: Field, text: &str) -> &mut Self {
        assert!(
            text.len() < field.size,
            "{text:?} does not fit the {}-byte field",
            field.size
        );
        let start = base + field.offset;
        self.buf[start..start + field.size].fill(0);
        self.put(start, text.as_bytes())
    }

    /// Fills a `char[N]` field with exactly these bytes (no terminator is added).
    pub fn set_raw(&mut self, base: usize, field: Field, bytes: &[u8]) -> &mut Self {
        assert!(
            bytes.len() <= field.size,
            "{} bytes do not fit the {}-byte field",
            bytes.len(),
            field.size
        );
        self.put(base + field.offset, bytes)
    }

    /// Absolute start of `scoringInfo`.
    pub fn scoring_base() -> usize {
        abs::SCORING_INFO
    }

    /// Absolute start of `vehScoringInfo[row]`.
    pub fn vehicle_base(row: usize) -> usize {
        assert!(row < MAX_SCORING_VEHICLES, "row {row} out of range");
        abs::VEH_SCORING_INFO + row * veh_scoring::SIZE
    }

    /// Absolute start of `telemInfo[slot]`.
    pub fn telem_base(slot: usize) -> usize {
        assert!(slot < MAX_TELEMETRY_VEHICLES, "slot {slot} out of range");
        abs::TELEM_INFO + slot * telem_info::SIZE
    }

    /// Absolute start of `telemInfo[slot].mWheel[wheel]`.
    pub fn wheel_base(slot: usize, wheel: usize) -> usize {
        assert!(wheel < WHEEL_COUNT, "wheel {wheel} out of range");
        Self::telem_base(slot) + telem_info::WHEEL.offset + wheel * telem_wheel::SIZE
    }

    /// Sets `generic.gameVersion`.
    pub fn game_version(&mut self, version: i32) -> &mut Self {
        self.set(abs::GENERIC, generic::GAME_VERSION, version)
    }

    /// Sets slot `index` of the `generic.events[]` queue.
    pub fn event(&mut self, index: usize, code: u32) -> &mut Self {
        self.set_elem(abs::GENERIC, generic::EVENTS, index, code)
    }

    /// Sets raw `scoringInfo.mNumVehicles` (not clamped).
    pub fn num_vehicles(&mut self, count: i32) -> &mut Self {
        self.set(abs::SCORING_INFO, scoring_info::NUM_VEHICLES, count)
    }

    /// Sets `scoringInfo.mCurrentET`.
    pub fn scoring_et(&mut self, et: f64) -> &mut Self {
        self.set(abs::SCORING_INFO, scoring_info::CURRENT_ET, et)
    }

    /// Sets raw `scoringStreamSize` (not clamped).
    pub fn scoring_stream_size(&mut self, size: u64) -> &mut Self {
        self.put(abs::SCORING_STREAM_SIZE, &size.to_le_bytes())
    }

    /// Writes the 4 default-pack padding bytes at absolute 2180 (zero in a correct layout).
    pub fn scoring_padding(&mut self, bytes: [u8; 4]) -> &mut Self {
        self.put(abs::SCORING_PADDING.offset, &bytes)
    }

    /// Sets the raw telemetry header bytes (`activeVehicles`, `playerVehicleIdx`, `playerHasVehicle`).
    pub fn telemetry_header(&mut self, active: u8, player_idx: u8, has_vehicle: u8) -> &mut Self {
        self.set(abs::TELEMETRY, telemetry_data::ACTIVE_VEHICLES, active);
        self.set(
            abs::TELEMETRY,
            telemetry_data::PLAYER_VEHICLE_IDX,
            player_idx,
        );
        self.set(
            abs::TELEMETRY,
            telemetry_data::PLAYER_HAS_VEHICLE,
            has_vehicle,
        )
    }

    /// Sets `telemInfo[slot].mElapsedTime`.
    pub fn player_elapsed_time(&mut self, slot: usize, et: f64) -> &mut Self {
        self.set(Self::telem_base(slot), telem_info::ELAPSED_TIME, et)
    }

    /// Writes one scoring row with the identity fields filled in.
    pub fn vehicle_row(
        &mut self,
        row: usize,
        id: i32,
        driver: &str,
        is_player: bool,
        place: u8,
    ) -> &mut Self {
        let base = Self::vehicle_base(row);
        self.set(base, veh_scoring::ID, id);
        self.set_str(base, veh_scoring::DRIVER_NAME, driver);
        self.set_str(base, veh_scoring::VEHICLE_NAME, "Synthetic Car");
        self.set_str(base, veh_scoring::VEHICLE_CLASS, "Hypercar");
        self.set(base, veh_scoring::IS_PLAYER, is_player);
        self.set(base, veh_scoring::PLACE, place)
    }

    /// Writes one telemetry slot with plausible, clearly synthetic values.
    pub fn telemetry_slot(&mut self, slot: usize, id: i32, et: f64) -> &mut Self {
        const DELTA_TIME_S: f64 = 0.01;
        const RPM: f64 = 7_500.0;
        const FUEL_L: f64 = 50.0;
        const FUEL_CAPACITY_L: f64 = 90.0;
        const GEAR: i32 = 4;
        const SPEED_MPS: f64 = 50.0;
        const TYRE_KPA: f64 = 170.0;
        const TYRE_K: f64 = 353.15;
        const BRAKE_RAW: f64 = 700.0;
        let base = Self::telem_base(slot);
        self.set(base, telem_info::ID, id);
        self.set(base, telem_info::DELTA_TIME, DELTA_TIME_S);
        self.set(base, telem_info::ELAPSED_TIME, et);
        self.set(base, telem_info::LAP_NUMBER, 3i32);
        self.set_str(base, telem_info::VEHICLE_NAME, "Synthetic Car #7");
        self.set_str(base, telem_info::TRACK_NAME, "Synthetic Ring");
        // Driving forward means -z in the local frame (+z points to the rear).
        self.set_elem(base, telem_info::LOCAL_VEL, 2, -SPEED_MPS);
        self.set(base, telem_info::GEAR, GEAR);
        self.set(base, telem_info::ENGINE_RPM, RPM);
        self.set(base, telem_info::FUEL, FUEL_L);
        self.set(base, telem_info::FUEL_CAPACITY, FUEL_CAPACITY_L);
        self.set(base, telem_info::FILTERED_THROTTLE, 1.0f64);
        self.set(base, telem_info::UNFILTERED_THROTTLE, 1.0f64);
        for wheel in 0..WHEEL_COUNT {
            let w = Self::wheel_base(slot, wheel);
            self.set(w, telem_wheel::PRESSURE, TYRE_KPA);
            self.set(w, telem_wheel::BRAKE_TEMP, BRAKE_RAW);
            self.set(w, telem_wheel::TIRE_CARCASS_TEMPERATURE, TYRE_K);
            for i in 0..3 {
                self.set_elem(w, telem_wheel::TEMPERATURE, i, TYRE_K);
            }
        }
        self
    }

    /// A plausible frame: two cars, the player in telemetry slot 0 / scoring row 0, ET 120 s.
    pub fn on_track(&mut self) -> &mut Self {
        const GAME_VERSION: i32 = 14_200;
        const ET: f64 = 120.0;
        const GREEN_FLAG_PHASE: u8 = 5;
        const RACE_SESSION: i32 = 10;
        self.game_version(GAME_VERSION);
        let s = Self::scoring_base();
        self.set_str(s, scoring_info::TRACK_NAME, "Synthetic Ring");
        self.set(s, scoring_info::SESSION, RACE_SESSION);
        self.scoring_et(ET);
        self.set(s, scoring_info::LAP_DIST, 13_626.0f64);
        self.set(s, scoring_info::GAME_PHASE, GREEN_FLAG_PHASE);
        self.set(s, scoring_info::IN_REALTIME, true);
        self.set_str(s, scoring_info::PLAYER_NAME, "Synthetic Driver");
        self.set(s, scoring_info::AMBIENT_TEMP, 22.0f64);
        self.set(s, scoring_info::TRACK_TEMP, 31.0f64);
        self.num_vehicles(2);
        self.vehicle_row(0, 7, "Synthetic Driver", true, 1);
        self.vehicle_row(1, 9, "Synthetic Rival", false, 2);
        self.telemetry_header(2, 0, 1);
        self.telemetry_slot(0, 7, ET);
        self.telemetry_slot(1, 9, ET)
    }
}
