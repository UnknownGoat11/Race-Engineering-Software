# Channel IDs

<!-- GENERATED from crates/telemetry-model/src/registry.rs. Do not edit by hand: run `LMURE_BLESS=1 cargo test -p telemetry-model --test registry_docs` to regenerate. -->

Telemetry schema version **1**, **511** channels. Single source of truth: `telemetry-model::registry` (D-017). IDs are append-only and never reused; per-wheel channels use a stride of 4 (`FL, FR, RL, RR` = base + 0..3). Field semantics, raw units and evidence: [LMU_CHANNEL_MAP.md](LMU_CHANNEL_MAP.md).

Canonical names are the map's row names, except the rows below; `cars.<id>.*` rows are per-car standings fields (`scoring::CompetitorEntry`), not channels. `tests/registry_docs.rs` checks this cross-reference in both directions.

| LMU_CHANNEL_MAP row | Registry |
|---|---|
| `vehicle.orientation_row0` | `vehicle.orientation_row0_x`, `vehicle.orientation_row0_y`, `vehicle.orientation_row0_z` (scalar channels in internal units) |
| `vehicle.orientation_row1` | `vehicle.orientation_row1_x`, `vehicle.orientation_row1_y`, `vehicle.orientation_row1_z` (scalar channels in internal units) |
| `vehicle.orientation_row2` | `vehicle.orientation_row2_x`, `vehicle.orientation_row2_y`, `vehicle.orientation_row2_z` (scalar channels in internal units) |
| `track.map_xy_m` | `track.map_x_m`, `track.map_y_m` (scalar channels in internal units) |
| `vehicle.gps_lat_deg` | `vehicle.gps_lat_rad` (scalar channels in internal units) |
| `vehicle.gps_lon_deg` | `vehicle.gps_lon_rad` (scalar channels in internal units) |
| `lmu.event_queue` | no channel: diagnostic only (SME event codes, shown by lmu-diag); not telemetry |
| `session.user_data_path` | no channel: personal data (Windows user path): used locally to find LMU's DuckDB recordings, never transmitted |
| `session.server_name` | no channel: server identity is not transmitted (privacy) |
| `weather.forecast.*` | no channel: LMU REST group (CLAIMED); not modelled as channels |
| `pit.menu.*` | no channel: LMU REST group (CLAIMED); not modelled as channels |

Columns: **Prov** M = measured (LMU, unit conversion only), D = derived, E = estimated. **Prio** network class. **Freq** PHY telemetry rate, SCO scoring rate, ENV slow, EVT event, STA static, DIA ~1 Hz diagnostics. **Int** C = continuous (may be interpolated), D = discrete (never interpolated). **Avail** A = always, V = vehicle-dependent, N = not provided by any live source (sent as NOT_SUPPORTED), X = experimental (not published until verified live). **Range** inclusive plausibility range in internal units (outside = INVALID). **Max rate** largest plausible change per second (larger jumps = INVALID).

| Group | ID range | Channels |
|---|---|---:|
| Vehicle & inputs | 0x0001–0x00FF | 64 |
| Powertrain | 0x0100–0x01FF | 18 |
| Fuel | 0x0200–0x02FF | 3 |
| Tyres | 0x0300–0x03FF | 129 |
| Brakes | 0x0400–0x04FF | 31 |
| Dynamics | 0x0500–0x05FF | 81 |
| Hybrid & energy | 0x0600–0x06FF | 10 |
| Timing | 0x0700–0x07FF | 29 |
| Session, race & scoring | 0x0800–0x08FF | 38 |
| Weather & track conditions | 0x0900–0x09FF | 15 |
| Track position & map | 0x0A00–0x0AFF | 18 |
| Flags & pit | 0x0B00–0x0BFF | 20 |
| Derived fuel / stint / energy (engine outputs) | 0x0C00–0x0CFF | 18 |
| Application, network & recording diagnostics | 0x0D00–0x0DFF | 37 |

## 0x0000 Vehicle & inputs

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0001 | `vehicle.speed_mps` | `Speed` | Speed | m/s | F32 | CALC | D | P0 | PHY | C | A | 0 … 150 | 1500 | `formula:speed_from_local_vel` |
| 0x0002 | `vehicle.speed_forward_mps` | `SpeedForward` | Forward speed (negative reversing) | m/s | F32 | CALC | D | P1 | PHY | C | A | -150 … 150 | 1500 | `formula:forward_speed_from_local_vel` |
| 0x0010 | `inputs.throttle_raw_frac` | `Throttle` | Throttle pedal | 0..1 | F32 | SM/tel | M | P0 | PHY | C | A | -0.01 … 1.01 |  | `telemetry.telemInfo[player].mUnfilteredThrottle` |
| 0x0011 | `inputs.brake_raw_frac` | `Brake` | Brake pedal | 0..1 | F32 | SM/tel | M | P0 | PHY | C | A | -0.01 … 1.01 |  | `telemetry.telemInfo[player].mUnfilteredBrake` |
| 0x0012 | `inputs.clutch_raw_frac` | `Clutch` | Clutch pedal | 0..1 | F32 | SM/tel | M | P1 | PHY | C | A | -0.01 … 1.01 |  | `telemetry.telemInfo[player].mUnfilteredClutch` |
| 0x0013 | `inputs.steering_raw` | `Steering` | Steering input (+ right) | 0..1 | F32 | SM/tel | M | P0 | PHY | C | A | -1.01 … 1.01 |  | `telemetry.telemInfo[player].mUnfilteredSteering` |
| 0x0014 | `inputs.throttle_filtered_frac` | `ThrottleFiltered` | Throttle after aids | 0..1 | F32 | SM/tel | M | P1 | PHY | C | A | -0.01 … 1.01 |  | `telemetry.telemInfo[player].mFilteredThrottle` |
| 0x0015 | `inputs.brake_filtered_frac` | `BrakeFiltered` | Brake after aids | 0..1 | F32 | SM/tel | M | P1 | PHY | C | A | -0.01 … 1.01 |  | `telemetry.telemInfo[player].mFilteredBrake` |
| 0x0016 | `inputs.clutch_filtered_frac` | `ClutchFiltered` | Clutch after aids | 0..1 | F32 | SM/tel | M | P2 | PHY | C | A | -0.01 … 1.01 |  | `telemetry.telemInfo[player].mFilteredClutch` |
| 0x0017 | `inputs.steering_filtered` | `SteeringFiltered` | Steering after aids | 0..1 | F32 | SM/tel | M | P1 | PHY | C | A | -1.01 … 1.01 |  | `telemetry.telemInfo[player].mFilteredSteering` |
| 0x0018 | `inputs.steering_shaft_torque_nm` | `SteeringTorque` | Steering shaft torque | N·m | F32 | SM/tel | M | P2 | PHY | C | A | -500 … 500 |  | `telemetry.telemInfo[player].mSteeringShaftTorque` |
| 0x0019 | `inputs.steering_wheel_range_physical` | `SteeringRangePhysical` | Steering wheel range (physical) | rad | F32 | SM/tel | M | P2 | STA | D | A | 1.570796 … 25.132741 |  | `telemetry.telemInfo[player].mPhysicalSteeringWheelRange` |
| 0x001A | `inputs.steering_wheel_range_visual` | `SteeringRangeVisual` | Steering wheel range (visual) | rad | F32 | SM/tel | M | P2 | STA | D | A | 1.570796 … 25.132741 |  | `telemetry.telemInfo[player].mVisualSteeringWheelRange` |
| 0x001B | `inputs.steering_wheel_angle_rad` | `SteeringAngle` | Steering wheel angle | rad | F32 | CALC | D | P1 | PHY | C | A | -12.566371 … 12.566371 |  | `formula:steering_wheel_angle` |
| 0x001C | `inputs.ffb_torque` | `FFBTorque` | Force-feedback torque (raw scale) | — | F32 | SM/gen | M | P2 | PHY | C | A |  |  | `generic.FFBTorque` |
| 0x0020 | `aids.tc_active` | `TCActive` | TC intervening | — | Bool | SM/tel | M | P1 | PHY | D | V |  |  | `telemetry.telemInfo[player].mTCActive` |
| 0x0021 | `aids.abs_active` | `ABSActive` | ABS intervening | — | Bool | SM/tel | M | P1 | PHY | D | V |  |  | `telemetry.telemInfo[player].mABSActive` |
| 0x0022 | `aids.tc_level` | `TCLevel` | TC setting | count | U8 | SM/tel | M | P1 | PHY | D | V |  |  | `telemetry.telemInfo[player].mTC` |
| 0x0023 | `aids.tc_level_max` | `TCLevelMax` | TC setting max | count | U8 | SM/tel | M | P2 | STA | D | V |  |  | `telemetry.telemInfo[player].mTCMax` |
| 0x0024 | `aids.tc_slip` | `TCSlip` | TC slip setting | count | U8 | SM/tel | M | P1 | PHY | D | V |  |  | `telemetry.telemInfo[player].mTCSlip` |
| 0x0025 | `aids.tc_slip_max` | `TCSlipMax` | TC slip setting max | count | U8 | SM/tel | M | P2 | STA | D | V |  |  | `telemetry.telemInfo[player].mTCSlipMax` |
| 0x0026 | `aids.tc_cut` | `TCCut` | TC cut setting | count | U8 | SM/tel | M | P1 | PHY | D | V |  |  | `telemetry.telemInfo[player].mTCCut` |
| 0x0027 | `aids.tc_cut_max` | `TCCutMax` | TC cut setting max | count | U8 | SM/tel | M | P2 | STA | D | V |  |  | `telemetry.telemInfo[player].mTCCutMax` |
| 0x0028 | `aids.abs_level` | `ABSLevel` | ABS setting | count | U8 | SM/tel | M | P1 | PHY | D | V |  |  | `telemetry.telemInfo[player].mABS` |
| 0x0029 | `aids.abs_level_max` | `ABSLevelMax` | ABS setting max | count | U8 | SM/tel | M | P2 | STA | D | V |  |  | `telemetry.telemInfo[player].mABSMax` |
| 0x002A | `aids.tc_duty_frac` | `TCDuty` | TC active share of samples | 0..1 | F32 | CALC | D | P1 | EVT | D | V | 0 … 1 |  | `formula:tc_active_duty_cycle` |
| 0x002B | `aids.abs_duty_frac` | `ABSDuty` | ABS active share of samples | 0..1 | F32 | CALC | D | P1 | EVT | D | V | 0 … 1 |  | `formula:abs_active_duty_cycle` |
| 0x002C | `aids.tc_intervention_frac` | `TCIntervention` | TC intervention amount | 0..1 | F32 | SM/tel | M | P1 | PHY | C | N |  |  | `unavailable: LMU exposes only the mTCActive boolean` |
| 0x002D | `aids.abs_intervention_frac` | `ABSIntervention` | ABS intervention amount | 0..1 | F32 | SM/tel | M | P1 | PHY | C | N |  |  | `unavailable: LMU exposes only the mABSActive boolean` |
| 0x0030 | `chassis.arb_front` | `ARBFront` | Front anti-roll bar setting | count | U8 | SM/tel | M | P2 | PHY | D | V |  |  | `telemetry.telemInfo[player].mFrontAntiSway` |
| 0x0031 | `chassis.arb_front_max` | `ARBFrontMax` | Front anti-roll bar setting max | count | U8 | SM/tel | M | P2 | STA | D | V |  |  | `telemetry.telemInfo[player].mFrontAntiSwayMax` |
| 0x0032 | `chassis.arb_rear` | `ARBRear` | Rear anti-roll bar setting | count | U8 | SM/tel | M | P2 | PHY | D | V |  |  | `telemetry.telemInfo[player].mRearAntiSway` |
| 0x0033 | `chassis.arb_rear_max` | `ARBRearMax` | Rear anti-roll bar setting max | count | U8 | SM/tel | M | P2 | STA | D | V |  |  | `telemetry.telemInfo[player].mRearAntiSwayMax` |
| 0x0034 | `aero.front_flap_active` | `FrontFlapActive` | Front flap active | — | Bool | SM/tel | M | P2 | PHY | D | V |  |  | `telemetry.telemInfo[player].mFrontFlapActivated` |
| 0x0035 | `aero.rear_flap_active` | `RearFlapActive` | Rear flap active | — | Bool | SM/tel | M | P1 | PHY | D | V |  |  | `telemetry.telemInfo[player].mRearFlapActivated` |
| 0x0036 | `aero.rear_flap_legal_status` | `RearFlapStatus` | Rear flap legal status | enum | U8 | SM/tel | M | P2 | PHY | D | V | 0 … 2 |  | `telemetry.telemInfo[player].mRearFlapLegalStatus` |
| 0x0037 | `vehicle.headlights_on` | `Headlights` | Headlights | — | Bool | SM/tel | M | P2 | PHY | D | A |  |  | `telemetry.telemInfo[player].mHeadlights` |
| 0x0038 | `vehicle.wiper_state` | `WiperState` | Wiper state (raw) | enum | U8 | SM/tel | M | P2 | PHY | D | A |  |  | `telemetry.telemInfo[player].mWiperState` |
| 0x0040 | `vehicle.slot_id` | `VehicleSlotId` | Vehicle slot id | count | I32 | SM/tel | M | P1 | STA | D | A | ≥ 0 |  | `telemetry.telemInfo[player].mID` |
| 0x0041 | `vehicle.name` | `VehicleName` | Vehicle name | text | Str | SM/tel | M | P1 | STA | D | A |  |  | `telemetry.telemInfo[player].mVehicleName` |
| 0x0042 | `vehicle.model` | `VehicleModel` | Vehicle model | text | Str | SM/tel | M | P1 | STA | D | A |  |  | `telemetry.telemInfo[player].mVehicleModel` |
| 0x0043 | `vehicle.class` | `VehicleClass` | Vehicle class | enum | U8 | SM/tel | M | P1 | STA | D | A |  |  | `telemetry.telemInfo[player].mVehicleClass` |
| 0x0044 | `vehicle.championship` | `Championship` | Championship | enum | U8 | SM/tel | M | P2 | STA | D | A |  |  | `telemetry.telemInfo[player].mVehicleChampionship` |
| 0x0045 | `vehicle.class_name` | `VehicleClassName` | Vehicle class name | text | Str | SM/sco | M | P1 | STA | D | A |  |  | `scoring.vehScoringInfo[player].mVehicleClass` |
| 0x0046 | `vehicle.veh_filename` | `VehicleFile` | Vehicle file | text | Str | SM/sco | M | P2 | STA | D | A |  |  | `scoring.vehScoringInfo[player].mVehFilename` |
| 0x0050 | `damage.dent_severity_0` | `DentSeverity0` | Dent severity location 0 | enum | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 2 |  | `telemetry.telemInfo[player].mDentSeverity[0]` |
| 0x0051 | `damage.dent_severity_1` | `DentSeverity1` | Dent severity location 1 | enum | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 2 |  | `telemetry.telemInfo[player].mDentSeverity[1]` |
| 0x0052 | `damage.dent_severity_2` | `DentSeverity2` | Dent severity location 2 | enum | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 2 |  | `telemetry.telemInfo[player].mDentSeverity[2]` |
| 0x0053 | `damage.dent_severity_3` | `DentSeverity3` | Dent severity location 3 | enum | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 2 |  | `telemetry.telemInfo[player].mDentSeverity[3]` |
| 0x0054 | `damage.dent_severity_4` | `DentSeverity4` | Dent severity location 4 | enum | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 2 |  | `telemetry.telemInfo[player].mDentSeverity[4]` |
| 0x0055 | `damage.dent_severity_5` | `DentSeverity5` | Dent severity location 5 | enum | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 2 |  | `telemetry.telemInfo[player].mDentSeverity[5]` |
| 0x0056 | `damage.dent_severity_6` | `DentSeverity6` | Dent severity location 6 | enum | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 2 |  | `telemetry.telemInfo[player].mDentSeverity[6]` |
| 0x0057 | `damage.dent_severity_7` | `DentSeverity7` | Dent severity location 7 | enum | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 2 |  | `telemetry.telemInfo[player].mDentSeverity[7]` |
| 0x0058 | `damage.parts_detached` | `PartsDetached` | Body parts detached | — | Bool | SM/tel | M | P1 | PHY | D | A |  |  | `telemetry.telemInfo[player].mDetached` |
| 0x0059 | `damage.last_impact_et_s` | `LastImpactTime` | Last impact time | s | F64 | SM/tel | M | P2 | PHY | D | A |  |  | `telemetry.telemInfo[player].mLastImpactET` |
| 0x005A | `damage.last_impact_magnitude` | `LastImpactMagnitude` | Last impact magnitude (raw) | — | F32 | SM/tel | M | P2 | PHY | D | A | ≥ 0 |  | `telemetry.telemInfo[player].mLastImpactMagnitude` |
| 0x005B | `damage.last_impact_pos_x_m` | `LastImpactPosX` | Last impact position x | m | F64 | SM/tel | M | P2 | PHY | D | A |  |  | `telemetry.telemInfo[player].mLastImpactPos.x` |
| 0x005C | `damage.last_impact_pos_y_m` | `LastImpactPosY` | Last impact position y | m | F64 | SM/tel | M | P2 | PHY | D | A |  |  | `telemetry.telemInfo[player].mLastImpactPos.y` |
| 0x005D | `damage.last_impact_pos_z_m` | `LastImpactPosZ` | Last impact position z | m | F64 | SM/tel | M | P2 | PHY | D | A |  |  | `telemetry.telemInfo[player].mLastImpactPos.z` |
| 0x005E | `damage.aero_frac` | `DamageAero` | Aero damage | 0..1 | F32 | REST | M | P2 | ENV | C | N |  |  | `unavailable: REST wearables (CLAIMED); shared memory has only mDentSeverity[8]` |
| 0x0060 | `damage.suspension_fl_frac` | `DamageSuspension_FL` | Suspension damage FL | 0..1 | F32 | REST | M | P2 | ENV | C | N |  |  | `unavailable: REST wearables (CLAIMED)` |
| 0x0061 | `damage.suspension_fr_frac` | `DamageSuspension_FR` | Suspension damage FR | 0..1 | F32 | REST | M | P2 | ENV | C | N |  |  | `unavailable: REST wearables (CLAIMED)` |
| 0x0062 | `damage.suspension_rl_frac` | `DamageSuspension_RL` | Suspension damage RL | 0..1 | F32 | REST | M | P2 | ENV | C | N |  |  | `unavailable: REST wearables (CLAIMED)` |
| 0x0063 | `damage.suspension_rr_frac` | `DamageSuspension_RR` | Suspension damage RR | 0..1 | F32 | REST | M | P2 | ENV | C | N |  |  | `unavailable: REST wearables (CLAIMED)` |

## 0x0100 Powertrain

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0100 | `powertrain.gear` | `Gear` | Gear (-1 R, 0 N) | enum | I32 | SM/tel | M | P0 | PHY | D | A | -1 … 10 |  | `telemetry.telemInfo[player].mGear` |
| 0x0101 | `powertrain.engine_rpm` | `RPM` | Engine speed | rpm | F32 | SM/tel | M | P0 | PHY | C | A | -1000 … 30000 |  | `telemetry.telemInfo[player].mEngineRPM` |
| 0x0102 | `vehicle.engine_max_rpm` | `RPMMax` | Rev limit | rpm | F32 | SM/tel | M | P1 | STA | D | A | 1000 … 25000 |  | `telemetry.telemInfo[player].mEngineMaxRPM` |
| 0x0103 | `vehicle.max_gears` | `GearsMax` | Number of forward gears | count | U8 | SM/tel | M | P2 | STA | D | A | 1 … 10 |  | `telemetry.telemInfo[player].mMaxGears` |
| 0x0104 | `powertrain.clutch_rpm` | `ClutchRPM` | Clutch speed | rpm | F32 | SM/tel | M | P2 | PHY | C | A | -1000 … 30000 |  | `telemetry.telemInfo[player].mClutchRPM` |
| 0x0105 | `powertrain.engine_torque_nm` | `EngineTorque` | Engine torque | N·m | F32 | SM/tel | M | P2 | PHY | C | A | -1000 … 3000 |  | `telemetry.telemInfo[player].mEngineTorque` |
| 0x0106 | `powertrain.water_temp_c` | `WaterTemp` | Water temperature | °C | F32 | SM/tel | M | P1 | PHY | C | A | -40 … 200 | 50 | `telemetry.telemInfo[player].mEngineWaterTemp` |
| 0x0107 | `powertrain.oil_temp_c` | `OilTemp` | Oil temperature | °C | F32 | SM/tel | M | P1 | PHY | C | A | -40 … 200 | 50 | `telemetry.telemInfo[player].mEngineOilTemp` |
| 0x0108 | `powertrain.turbo_boost_pa` | `TurboBoost` | Turbo boost pressure (absolute, assumed) | Pa | F32 | SM/tel | M | P2 | PHY | C | V | 0 … 500000 |  | `telemetry.telemInfo[player].mTurboBoostPressure` |
| 0x0109 | `powertrain.overheating` | `Overheating` | Overheating warning | — | Bool | SM/tel | M | P1 | PHY | D | A |  |  | `telemetry.telemInfo[player].mOverheating` |
| 0x010A | `powertrain.ignition_starter` | `IgnitionStarter` | Ignition / starter | enum | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 2 |  | `telemetry.telemInfo[player].mIgnitionStarter` |
| 0x010B | `powertrain.anti_stall_active` | `AntiStall` | Anti-stall active | — | Bool | SM/tel | M | P2 | PHY | D | A |  |  | `telemetry.telemInfo[player].mAntiStallActivated` |
| 0x010C | `powertrain.speed_limiter_on` | `SpeedLimiter` | Speed limiter (legacy) | — | Bool | SM/tel | M | P0 | PHY | D | A |  |  | `telemetry.telemInfo[player].mSpeedLimiter` |
| 0x010D | `powertrain.speed_limiter_active` | `SpeedLimiterActive` | Speed limiter active | — | Bool | SM/tel | M | P0 | PHY | D | A |  |  | `telemetry.telemInfo[player].mSpeedLimiterActive` |
| 0x010E | `powertrain.speed_limiter_available` | `SpeedLimiterAvailable` | Speed limiter available | — | Bool | SM/tel | M | P2 | STA | D | A |  |  | `telemetry.telemInfo[player].mSpeedLimiterAvailable` |
| 0x010F | `powertrain.motor_map` | `MotorMap` | Motor map setting | count | U8 | SM/tel | M | P1 | PHY | D | V |  |  | `telemetry.telemInfo[player].mMotorMap` |
| 0x0110 | `powertrain.motor_map_max` | `MotorMapMax` | Motor map setting max | count | U8 | SM/tel | M | P2 | STA | D | V |  |  | `telemetry.telemInfo[player].mMotorMapMax` |
| 0x0111 | `powertrain.engine_power_w` | `EnginePower` | Engine power | W | F32 | CALC | D | P2 | PHY | C | A | -1000000 … 1500000 |  | `formula:engine_power_from_torque_rpm` |

## 0x0200 Fuel

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0200 | `fuel.level_l` | `Fuel` | Fuel | L | F32 | SM/tel | M | P0 | PHY | C | A | 0 … 201 | 50 | `telemetry.telemInfo[player].mFuel` |
| 0x0201 | `fuel.capacity_l` | `FuelCapacity` | Fuel capacity | L | F32 | SM/tel | M | P1 | STA | D | A | 0 … 200 |  | `telemetry.telemInfo[player].mFuelCapacity` |
| 0x0202 | `fuel.level_frac` | `FuelFraction` | Fuel level (share of capacity) | 0..1 | F32 | CALC | D | P1 | PHY | C | A | 0 … 1.01 |  | `formula:fuel_level_fraction` |

## 0x0300 Tyres

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0300 | `tyres.fl.pressure_pa` | `TyrePressure_FL` | Tyre pressure FL | Pa | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 600000 | 2000000 | `telemetry.telemInfo[player].mWheel[0].mPressure` |
| 0x0301 | `tyres.fr.pressure_pa` | `TyrePressure_FR` | Tyre pressure FR | Pa | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 600000 | 2000000 | `telemetry.telemInfo[player].mWheel[1].mPressure` |
| 0x0302 | `tyres.rl.pressure_pa` | `TyrePressure_RL` | Tyre pressure RL | Pa | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 600000 | 2000000 | `telemetry.telemInfo[player].mWheel[2].mPressure` |
| 0x0303 | `tyres.rr.pressure_pa` | `TyrePressure_RR` | Tyre pressure RR | Pa | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 600000 | 2000000 | `telemetry.telemInfo[player].mWheel[3].mPressure` |
| 0x0304 | `tyres.fl.temp_surface_left_c` | `TyreTemp_FL_Left` | Tyre surface temp FL left edge (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `telemetry.telemInfo[player].mWheel[0].mTemperature[0]` |
| 0x0305 | `tyres.fr.temp_surface_left_c` | `TyreTemp_FR_Left` | Tyre surface temp FR left edge (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `telemetry.telemInfo[player].mWheel[1].mTemperature[0]` |
| 0x0306 | `tyres.rl.temp_surface_left_c` | `TyreTemp_RL_Left` | Tyre surface temp RL left edge (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `telemetry.telemInfo[player].mWheel[2].mTemperature[0]` |
| 0x0307 | `tyres.rr.temp_surface_left_c` | `TyreTemp_RR_Left` | Tyre surface temp RR left edge (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `telemetry.telemInfo[player].mWheel[3].mTemperature[0]` |
| 0x0308 | `tyres.fl.temp_surface_center_c` | `TyreTemp_FL_Middle` | Tyre surface temp FL middle | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `telemetry.telemInfo[player].mWheel[0].mTemperature[1]` |
| 0x0309 | `tyres.fr.temp_surface_center_c` | `TyreTemp_FR_Middle` | Tyre surface temp FR middle | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `telemetry.telemInfo[player].mWheel[1].mTemperature[1]` |
| 0x030A | `tyres.rl.temp_surface_center_c` | `TyreTemp_RL_Middle` | Tyre surface temp RL middle | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `telemetry.telemInfo[player].mWheel[2].mTemperature[1]` |
| 0x030B | `tyres.rr.temp_surface_center_c` | `TyreTemp_RR_Middle` | Tyre surface temp RR middle | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `telemetry.telemInfo[player].mWheel[3].mTemperature[1]` |
| 0x030C | `tyres.fl.temp_surface_right_c` | `TyreTemp_FL_Right` | Tyre surface temp FL right edge (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `telemetry.telemInfo[player].mWheel[0].mTemperature[2]` |
| 0x030D | `tyres.fr.temp_surface_right_c` | `TyreTemp_FR_Right` | Tyre surface temp FR right edge (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `telemetry.telemInfo[player].mWheel[1].mTemperature[2]` |
| 0x030E | `tyres.rl.temp_surface_right_c` | `TyreTemp_RL_Right` | Tyre surface temp RL right edge (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `telemetry.telemInfo[player].mWheel[2].mTemperature[2]` |
| 0x030F | `tyres.rr.temp_surface_right_c` | `TyreTemp_RR_Right` | Tyre surface temp RR right edge (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `telemetry.telemInfo[player].mWheel[3].mTemperature[2]` |
| 0x0310 | `tyres.fl.temp_inner_layer_left_c` | `TyreLayerTemp_FL_Left` | Tyre inner-layer temp FL left (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 1000 | `telemetry.telemInfo[player].mWheel[0].mTireInnerLayerTemperature[0]` |
| 0x0311 | `tyres.fr.temp_inner_layer_left_c` | `TyreLayerTemp_FR_Left` | Tyre inner-layer temp FR left (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 1000 | `telemetry.telemInfo[player].mWheel[1].mTireInnerLayerTemperature[0]` |
| 0x0312 | `tyres.rl.temp_inner_layer_left_c` | `TyreLayerTemp_RL_Left` | Tyre inner-layer temp RL left (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 1000 | `telemetry.telemInfo[player].mWheel[2].mTireInnerLayerTemperature[0]` |
| 0x0313 | `tyres.rr.temp_inner_layer_left_c` | `TyreLayerTemp_RR_Left` | Tyre inner-layer temp RR left (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 1000 | `telemetry.telemInfo[player].mWheel[3].mTireInnerLayerTemperature[0]` |
| 0x0314 | `tyres.fl.temp_inner_layer_center_c` | `TyreLayerTemp_FL_Middle` | Tyre inner-layer temp FL middle | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 1000 | `telemetry.telemInfo[player].mWheel[0].mTireInnerLayerTemperature[1]` |
| 0x0315 | `tyres.fr.temp_inner_layer_center_c` | `TyreLayerTemp_FR_Middle` | Tyre inner-layer temp FR middle | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 1000 | `telemetry.telemInfo[player].mWheel[1].mTireInnerLayerTemperature[1]` |
| 0x0316 | `tyres.rl.temp_inner_layer_center_c` | `TyreLayerTemp_RL_Middle` | Tyre inner-layer temp RL middle | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 1000 | `telemetry.telemInfo[player].mWheel[2].mTireInnerLayerTemperature[1]` |
| 0x0317 | `tyres.rr.temp_inner_layer_center_c` | `TyreLayerTemp_RR_Middle` | Tyre inner-layer temp RR middle | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 1000 | `telemetry.telemInfo[player].mWheel[3].mTireInnerLayerTemperature[1]` |
| 0x0318 | `tyres.fl.temp_inner_layer_right_c` | `TyreLayerTemp_FL_Right` | Tyre inner-layer temp FL right (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 1000 | `telemetry.telemInfo[player].mWheel[0].mTireInnerLayerTemperature[2]` |
| 0x0319 | `tyres.fr.temp_inner_layer_right_c` | `TyreLayerTemp_FR_Right` | Tyre inner-layer temp FR right (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 1000 | `telemetry.telemInfo[player].mWheel[1].mTireInnerLayerTemperature[2]` |
| 0x031A | `tyres.rl.temp_inner_layer_right_c` | `TyreLayerTemp_RL_Right` | Tyre inner-layer temp RL right (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 1000 | `telemetry.telemInfo[player].mWheel[2].mTireInnerLayerTemperature[2]` |
| 0x031B | `tyres.rr.temp_inner_layer_right_c` | `TyreLayerTemp_RR_Right` | Tyre inner-layer temp RR right (car frame) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 1000 | `telemetry.telemInfo[player].mWheel[3].mTireInnerLayerTemperature[2]` |
| 0x031C | `tyres.fl.temp_carcass_c` | `TyreCarcassTemp_FL` | Tyre carcass temp FL | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 500 | `telemetry.telemInfo[player].mWheel[0].mTireCarcassTemperature` |
| 0x031D | `tyres.fr.temp_carcass_c` | `TyreCarcassTemp_FR` | Tyre carcass temp FR | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 500 | `telemetry.telemInfo[player].mWheel[1].mTireCarcassTemperature` |
| 0x031E | `tyres.rl.temp_carcass_c` | `TyreCarcassTemp_RL` | Tyre carcass temp RL | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 500 | `telemetry.telemInfo[player].mWheel[2].mTireCarcassTemperature` |
| 0x031F | `tyres.rr.temp_carcass_c` | `TyreCarcassTemp_RR` | Tyre carcass temp RR | °C | F32 | SM/tel | M | P1 | PHY | C | A | -73.15 … 426.85 | 500 | `telemetry.telemInfo[player].mWheel[3].mTireCarcassTemperature` |
| 0x0320 | `tyres.fl.temp_optimal_c` | `TyreOptimalTemp_FL` | Tyre optimal temp FL | °C | F32 | SM/tel | M | P2 | STA | D | A | 0 … 200 |  | `telemetry.telemInfo[player].mWheel[0].mOptimalTemp` |
| 0x0321 | `tyres.fr.temp_optimal_c` | `TyreOptimalTemp_FR` | Tyre optimal temp FR | °C | F32 | SM/tel | M | P2 | STA | D | A | 0 … 200 |  | `telemetry.telemInfo[player].mWheel[1].mOptimalTemp` |
| 0x0322 | `tyres.rl.temp_optimal_c` | `TyreOptimalTemp_RL` | Tyre optimal temp RL | °C | F32 | SM/tel | M | P2 | STA | D | A | 0 … 200 |  | `telemetry.telemInfo[player].mWheel[2].mOptimalTemp` |
| 0x0323 | `tyres.rr.temp_optimal_c` | `TyreOptimalTemp_RR` | Tyre optimal temp RR | °C | F32 | SM/tel | M | P2 | STA | D | A | 0 … 200 |  | `telemetry.telemInfo[player].mWheel[3].mOptimalTemp` |
| 0x0324 | `tyres.fl.wear_frac` | `TyreWear_FL` | Tyre wear FL | 0..1 | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 1 | 0.5 | `telemetry.telemInfo[player].mWheel[0].mWear` |
| 0x0325 | `tyres.fr.wear_frac` | `TyreWear_FR` | Tyre wear FR | 0..1 | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 1 | 0.5 | `telemetry.telemInfo[player].mWheel[1].mWear` |
| 0x0326 | `tyres.rl.wear_frac` | `TyreWear_RL` | Tyre wear RL | 0..1 | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 1 | 0.5 | `telemetry.telemInfo[player].mWheel[2].mWear` |
| 0x0327 | `tyres.rr.wear_frac` | `TyreWear_RR` | Tyre wear RR | 0..1 | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 1 | 0.5 | `telemetry.telemInfo[player].mWheel[3].mWear` |
| 0x0328 | `tyres.fl.sliding_frac` | `TyreSliding_FL` | Tyre contact patch sliding FL | 0..1 | F32 | SM/tel | M | P2 | PHY | C | A | 0 … 1 |  | `telemetry.telemInfo[player].mWheel[0].mGripFract` |
| 0x0329 | `tyres.fr.sliding_frac` | `TyreSliding_FR` | Tyre contact patch sliding FR | 0..1 | F32 | SM/tel | M | P2 | PHY | C | A | 0 … 1 |  | `telemetry.telemInfo[player].mWheel[1].mGripFract` |
| 0x032A | `tyres.rl.sliding_frac` | `TyreSliding_RL` | Tyre contact patch sliding RL | 0..1 | F32 | SM/tel | M | P2 | PHY | C | A | 0 … 1 |  | `telemetry.telemInfo[player].mWheel[2].mGripFract` |
| 0x032B | `tyres.rr.sliding_frac` | `TyreSliding_RR` | Tyre contact patch sliding RR | 0..1 | F32 | SM/tel | M | P2 | PHY | C | A | 0 … 1 |  | `telemetry.telemInfo[player].mWheel[3].mGripFract` |
| 0x032C | `tyres.fl.load_n` | `TyreLoad_FL` | Tyre load FL | N | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 60000 |  | `telemetry.telemInfo[player].mWheel[0].mTireLoad` |
| 0x032D | `tyres.fr.load_n` | `TyreLoad_FR` | Tyre load FR | N | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 60000 |  | `telemetry.telemInfo[player].mWheel[1].mTireLoad` |
| 0x032E | `tyres.rl.load_n` | `TyreLoad_RL` | Tyre load RL | N | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 60000 |  | `telemetry.telemInfo[player].mWheel[2].mTireLoad` |
| 0x032F | `tyres.rr.load_n` | `TyreLoad_RR` | Tyre load RR | N | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 60000 |  | `telemetry.telemInfo[player].mWheel[3].mTireLoad` |
| 0x0330 | `tyres.fl.lateral_force_n` | `TyreLatForce_FL` | Tyre lateral force FL | N | F32 | SM/tel | M | P2 | PHY | C | A | -60000 … 60000 |  | `telemetry.telemInfo[player].mWheel[0].mLateralForce` |
| 0x0331 | `tyres.fr.lateral_force_n` | `TyreLatForce_FR` | Tyre lateral force FR | N | F32 | SM/tel | M | P2 | PHY | C | A | -60000 … 60000 |  | `telemetry.telemInfo[player].mWheel[1].mLateralForce` |
| 0x0332 | `tyres.rl.lateral_force_n` | `TyreLatForce_RL` | Tyre lateral force RL | N | F32 | SM/tel | M | P2 | PHY | C | A | -60000 … 60000 |  | `telemetry.telemInfo[player].mWheel[2].mLateralForce` |
| 0x0333 | `tyres.rr.lateral_force_n` | `TyreLatForce_RR` | Tyre lateral force RR | N | F32 | SM/tel | M | P2 | PHY | C | A | -60000 … 60000 |  | `telemetry.telemInfo[player].mWheel[3].mLateralForce` |
| 0x0334 | `tyres.fl.longitudinal_force_n` | `TyreLongForce_FL` | Tyre longitudinal force FL | N | F32 | SM/tel | M | P2 | PHY | C | A | -60000 … 60000 |  | `telemetry.telemInfo[player].mWheel[0].mLongitudinalForce` |
| 0x0335 | `tyres.fr.longitudinal_force_n` | `TyreLongForce_FR` | Tyre longitudinal force FR | N | F32 | SM/tel | M | P2 | PHY | C | A | -60000 … 60000 |  | `telemetry.telemInfo[player].mWheel[1].mLongitudinalForce` |
| 0x0336 | `tyres.rl.longitudinal_force_n` | `TyreLongForce_RL` | Tyre longitudinal force RL | N | F32 | SM/tel | M | P2 | PHY | C | A | -60000 … 60000 |  | `telemetry.telemInfo[player].mWheel[2].mLongitudinalForce` |
| 0x0337 | `tyres.rr.longitudinal_force_n` | `TyreLongForce_RR` | Tyre longitudinal force RR | N | F32 | SM/tel | M | P2 | PHY | C | A | -60000 … 60000 |  | `telemetry.telemInfo[player].mWheel[3].mLongitudinalForce` |
| 0x0338 | `tyres.fl.lateral_patch_vel_mps` | `TyreLatPatchVel_FL` | Tyre lateral patch velocity FL | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[0].mLateralPatchVel` |
| 0x0339 | `tyres.fr.lateral_patch_vel_mps` | `TyreLatPatchVel_FR` | Tyre lateral patch velocity FR | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[1].mLateralPatchVel` |
| 0x033A | `tyres.rl.lateral_patch_vel_mps` | `TyreLatPatchVel_RL` | Tyre lateral patch velocity RL | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[2].mLateralPatchVel` |
| 0x033B | `tyres.rr.lateral_patch_vel_mps` | `TyreLatPatchVel_RR` | Tyre lateral patch velocity RR | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[3].mLateralPatchVel` |
| 0x033C | `tyres.fl.longitudinal_patch_vel_mps` | `TyreLongPatchVel_FL` | Tyre longitudinal patch velocity FL | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[0].mLongitudinalPatchVel` |
| 0x033D | `tyres.fr.longitudinal_patch_vel_mps` | `TyreLongPatchVel_FR` | Tyre longitudinal patch velocity FR | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[1].mLongitudinalPatchVel` |
| 0x033E | `tyres.rl.longitudinal_patch_vel_mps` | `TyreLongPatchVel_RL` | Tyre longitudinal patch velocity RL | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[2].mLongitudinalPatchVel` |
| 0x033F | `tyres.rr.longitudinal_patch_vel_mps` | `TyreLongPatchVel_RR` | Tyre longitudinal patch velocity RR | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[3].mLongitudinalPatchVel` |
| 0x0340 | `tyres.fl.lateral_ground_vel_mps` | `TyreLatGroundVel_FL` | Tyre lateral ground velocity FL | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[0].mLateralGroundVel` |
| 0x0341 | `tyres.fr.lateral_ground_vel_mps` | `TyreLatGroundVel_FR` | Tyre lateral ground velocity FR | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[1].mLateralGroundVel` |
| 0x0342 | `tyres.rl.lateral_ground_vel_mps` | `TyreLatGroundVel_RL` | Tyre lateral ground velocity RL | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[2].mLateralGroundVel` |
| 0x0343 | `tyres.rr.lateral_ground_vel_mps` | `TyreLatGroundVel_RR` | Tyre lateral ground velocity RR | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[3].mLateralGroundVel` |
| 0x0344 | `tyres.fl.longitudinal_ground_vel_mps` | `TyreLongGroundVel_FL` | Tyre longitudinal ground velocity FL | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[0].mLongitudinalGroundVel` |
| 0x0345 | `tyres.fr.longitudinal_ground_vel_mps` | `TyreLongGroundVel_FR` | Tyre longitudinal ground velocity FR | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[1].mLongitudinalGroundVel` |
| 0x0346 | `tyres.rl.longitudinal_ground_vel_mps` | `TyreLongGroundVel_RL` | Tyre longitudinal ground velocity RL | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[2].mLongitudinalGroundVel` |
| 0x0347 | `tyres.rr.longitudinal_ground_vel_mps` | `TyreLongGroundVel_RR` | Tyre longitudinal ground velocity RR | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 |  | `telemetry.telemInfo[player].mWheel[3].mLongitudinalGroundVel` |
| 0x0348 | `tyres.fl.vertical_deflection_m` | `TyreDeflection_FL` | Tyre vertical deflection FL | m | F32 | SM/tel | M | P2 | PHY | C | A | -0.2 … 0.2 |  | `telemetry.telemInfo[player].mWheel[0].mVerticalTireDeflection` |
| 0x0349 | `tyres.fr.vertical_deflection_m` | `TyreDeflection_FR` | Tyre vertical deflection FR | m | F32 | SM/tel | M | P2 | PHY | C | A | -0.2 … 0.2 |  | `telemetry.telemInfo[player].mWheel[1].mVerticalTireDeflection` |
| 0x034A | `tyres.rl.vertical_deflection_m` | `TyreDeflection_RL` | Tyre vertical deflection RL | m | F32 | SM/tel | M | P2 | PHY | C | A | -0.2 … 0.2 |  | `telemetry.telemInfo[player].mWheel[2].mVerticalTireDeflection` |
| 0x034B | `tyres.rr.vertical_deflection_m` | `TyreDeflection_RR` | Tyre vertical deflection RR | m | F32 | SM/tel | M | P2 | PHY | C | A | -0.2 … 0.2 |  | `telemetry.telemInfo[player].mWheel[3].mVerticalTireDeflection` |
| 0x034C | `tyres.fl.static_radius_m` | `TyreRadius_FL` | Tyre static radius FL (1 cm resolution) | m | F32 | SM/tel | M | P2 | STA | D | A | 0.2 … 0.6 |  | `telemetry.telemInfo[player].mWheel[0].mStaticUndeflectedRadius` |
| 0x034D | `tyres.fr.static_radius_m` | `TyreRadius_FR` | Tyre static radius FR (1 cm resolution) | m | F32 | SM/tel | M | P2 | STA | D | A | 0.2 … 0.6 |  | `telemetry.telemInfo[player].mWheel[1].mStaticUndeflectedRadius` |
| 0x034E | `tyres.rl.static_radius_m` | `TyreRadius_RL` | Tyre static radius RL (1 cm resolution) | m | F32 | SM/tel | M | P2 | STA | D | A | 0.2 … 0.6 |  | `telemetry.telemInfo[player].mWheel[2].mStaticUndeflectedRadius` |
| 0x034F | `tyres.rr.static_radius_m` | `TyreRadius_RR` | Tyre static radius RR (1 cm resolution) | m | F32 | SM/tel | M | P2 | STA | D | A | 0.2 … 0.6 |  | `telemetry.telemInfo[player].mWheel[3].mStaticUndeflectedRadius` |
| 0x0350 | `tyres.fl.compound_index` | `TyreCompoundIndex_FL` | Tyre compound index FL | count | U8 | SM/tel | M | P1 | STA | D | A |  |  | `telemetry.telemInfo[player].mWheel[0].mCompoundIndex` |
| 0x0351 | `tyres.fr.compound_index` | `TyreCompoundIndex_FR` | Tyre compound index FR | count | U8 | SM/tel | M | P1 | STA | D | A |  |  | `telemetry.telemInfo[player].mWheel[1].mCompoundIndex` |
| 0x0352 | `tyres.rl.compound_index` | `TyreCompoundIndex_RL` | Tyre compound index RL | count | U8 | SM/tel | M | P1 | STA | D | A |  |  | `telemetry.telemInfo[player].mWheel[2].mCompoundIndex` |
| 0x0353 | `tyres.rr.compound_index` | `TyreCompoundIndex_RR` | Tyre compound index RR | count | U8 | SM/tel | M | P1 | STA | D | A |  |  | `telemetry.telemInfo[player].mWheel[3].mCompoundIndex` |
| 0x0354 | `tyres.fl.compound_type` | `TyreCompoundType_FL` | Tyre compound type FL | enum | U8 | SM/tel | M | P1 | STA | D | A |  |  | `telemetry.telemInfo[player].mWheel[0].mCompoundType` |
| 0x0355 | `tyres.fr.compound_type` | `TyreCompoundType_FR` | Tyre compound type FR | enum | U8 | SM/tel | M | P1 | STA | D | A |  |  | `telemetry.telemInfo[player].mWheel[1].mCompoundType` |
| 0x0356 | `tyres.rl.compound_type` | `TyreCompoundType_RL` | Tyre compound type RL | enum | U8 | SM/tel | M | P1 | STA | D | A |  |  | `telemetry.telemInfo[player].mWheel[2].mCompoundType` |
| 0x0357 | `tyres.rr.compound_type` | `TyreCompoundType_RR` | Tyre compound type RR | enum | U8 | SM/tel | M | P1 | STA | D | A |  |  | `telemetry.telemInfo[player].mWheel[3].mCompoundType` |
| 0x0358 | `tyres.fl.flat` | `TyreFlat_FL` | Tyre flat FL | — | Bool | SM/tel | M | P0 | PHY | D | A |  |  | `telemetry.telemInfo[player].mWheel[0].mFlat` |
| 0x0359 | `tyres.fr.flat` | `TyreFlat_FR` | Tyre flat FR | — | Bool | SM/tel | M | P0 | PHY | D | A |  |  | `telemetry.telemInfo[player].mWheel[1].mFlat` |
| 0x035A | `tyres.rl.flat` | `TyreFlat_RL` | Tyre flat RL | — | Bool | SM/tel | M | P0 | PHY | D | A |  |  | `telemetry.telemInfo[player].mWheel[2].mFlat` |
| 0x035B | `tyres.rr.flat` | `TyreFlat_RR` | Tyre flat RR | — | Bool | SM/tel | M | P0 | PHY | D | A |  |  | `telemetry.telemInfo[player].mWheel[3].mFlat` |
| 0x035C | `tyres.fl.surface_type` | `SurfaceType_FL` | Surface under FL | enum | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 6 |  | `telemetry.telemInfo[player].mWheel[0].mSurfaceType` |
| 0x035D | `tyres.fr.surface_type` | `SurfaceType_FR` | Surface under FR | enum | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 6 |  | `telemetry.telemInfo[player].mWheel[1].mSurfaceType` |
| 0x035E | `tyres.rl.surface_type` | `SurfaceType_RL` | Surface under RL | enum | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 6 |  | `telemetry.telemInfo[player].mWheel[2].mSurfaceType` |
| 0x035F | `tyres.rr.surface_type` | `SurfaceType_RR` | Surface under RR | enum | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 6 |  | `telemetry.telemInfo[player].mWheel[3].mSurfaceType` |
| 0x0360 | `tyres.fl.terrain_name` | `TerrainName_FL` | Terrain material FL | text | Str | SM/tel | M | P2 | PHY | D | A |  |  | `telemetry.telemInfo[player].mWheel[0].mTerrainName` |
| 0x0361 | `tyres.fr.terrain_name` | `TerrainName_FR` | Terrain material FR | text | Str | SM/tel | M | P2 | PHY | D | A |  |  | `telemetry.telemInfo[player].mWheel[1].mTerrainName` |
| 0x0362 | `tyres.rl.terrain_name` | `TerrainName_RL` | Terrain material RL | text | Str | SM/tel | M | P2 | PHY | D | A |  |  | `telemetry.telemInfo[player].mWheel[2].mTerrainName` |
| 0x0363 | `tyres.rr.terrain_name` | `TerrainName_RR` | Terrain material RR | text | Str | SM/tel | M | P2 | PHY | D | A |  |  | `telemetry.telemInfo[player].mWheel[3].mTerrainName` |
| 0x0364 | `tyres.fl.temp_surface_inner_c` | `TyreTemp_FL_Inner` | Tyre surface temp FL inner edge (A-LMU-016) | °C | F32 | CALC | D | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `formula:tyre_surface_inner_edge` |
| 0x0365 | `tyres.fr.temp_surface_inner_c` | `TyreTemp_FR_Inner` | Tyre surface temp FR inner edge (A-LMU-016) | °C | F32 | CALC | D | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `formula:tyre_surface_inner_edge` |
| 0x0366 | `tyres.rl.temp_surface_inner_c` | `TyreTemp_RL_Inner` | Tyre surface temp RL inner edge (A-LMU-016) | °C | F32 | CALC | D | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `formula:tyre_surface_inner_edge` |
| 0x0367 | `tyres.rr.temp_surface_inner_c` | `TyreTemp_RR_Inner` | Tyre surface temp RR inner edge (A-LMU-016) | °C | F32 | CALC | D | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `formula:tyre_surface_inner_edge` |
| 0x0368 | `tyres.fl.temp_surface_outer_c` | `TyreTemp_FL_Outer` | Tyre surface temp FL outer edge (A-LMU-016) | °C | F32 | CALC | D | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `formula:tyre_surface_outer_edge` |
| 0x0369 | `tyres.fr.temp_surface_outer_c` | `TyreTemp_FR_Outer` | Tyre surface temp FR outer edge (A-LMU-016) | °C | F32 | CALC | D | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `formula:tyre_surface_outer_edge` |
| 0x036A | `tyres.rl.temp_surface_outer_c` | `TyreTemp_RL_Outer` | Tyre surface temp RL outer edge (A-LMU-016) | °C | F32 | CALC | D | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `formula:tyre_surface_outer_edge` |
| 0x036B | `tyres.rr.temp_surface_outer_c` | `TyreTemp_RR_Outer` | Tyre surface temp RR outer edge (A-LMU-016) | °C | F32 | CALC | D | P1 | PHY | C | A | -73.15 … 426.85 | 3000 | `formula:tyre_surface_outer_edge` |
| 0x036C | `tyres.fl.temp_surface_avg_c` | `TyreTemp_FL_Avg` | Tyre surface temp FL average | °C | F32 | CALC | D | P2 | PHY | C | A | -73.15 … 426.85 |  | `formula:tyre_surface_mean` |
| 0x036D | `tyres.fr.temp_surface_avg_c` | `TyreTemp_FR_Avg` | Tyre surface temp FR average | °C | F32 | CALC | D | P2 | PHY | C | A | -73.15 … 426.85 |  | `formula:tyre_surface_mean` |
| 0x036E | `tyres.rl.temp_surface_avg_c` | `TyreTemp_RL_Avg` | Tyre surface temp RL average | °C | F32 | CALC | D | P2 | PHY | C | A | -73.15 … 426.85 |  | `formula:tyre_surface_mean` |
| 0x036F | `tyres.rr.temp_surface_avg_c` | `TyreTemp_RR_Avg` | Tyre surface temp RR average | °C | F32 | CALC | D | P2 | PHY | C | A | -73.15 … 426.85 |  | `formula:tyre_surface_mean` |
| 0x0370 | `tyres.fl.inner_outer_spread_c` | `TyreTempSpread_FL` | Tyre inner-minus-outer FL | °C | F32 | CALC | D | P2 | PHY | C | A | -100 … 100 |  | `formula:tyre_inner_minus_outer` |
| 0x0371 | `tyres.fr.inner_outer_spread_c` | `TyreTempSpread_FR` | Tyre inner-minus-outer FR | °C | F32 | CALC | D | P2 | PHY | C | A | -100 … 100 |  | `formula:tyre_inner_minus_outer` |
| 0x0372 | `tyres.rl.inner_outer_spread_c` | `TyreTempSpread_RL` | Tyre inner-minus-outer RL | °C | F32 | CALC | D | P2 | PHY | C | A | -100 … 100 |  | `formula:tyre_inner_minus_outer` |
| 0x0373 | `tyres.rr.inner_outer_spread_c` | `TyreTempSpread_RR` | Tyre inner-minus-outer RR | °C | F32 | CALC | D | P2 | PHY | C | A | -100 … 100 |  | `formula:tyre_inner_minus_outer` |
| 0x0374 | `tyres.fl.temp_rim_c` | `TyreRimTemp_FL` | Rim temp FL | °C | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (TyresRimTemp)` |
| 0x0375 | `tyres.fr.temp_rim_c` | `TyreRimTemp_FR` | Rim temp FR | °C | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (TyresRimTemp)` |
| 0x0376 | `tyres.rl.temp_rim_c` | `TyreRimTemp_RL` | Rim temp RL | °C | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (TyresRimTemp)` |
| 0x0377 | `tyres.rr.temp_rim_c` | `TyreRimTemp_RR` | Rim temp RR | °C | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (TyresRimTemp)` |
| 0x0378 | `tyres.fl.compound_name` | `TyreCompound_FL` | Tyre compound name FL | text | Str | SM/tel | M | P2 | PHY | D | N |  |  | `unavailable: compound names exist per axle only (tyres.front_compound_name)` |
| 0x0379 | `tyres.fr.compound_name` | `TyreCompound_FR` | Tyre compound name FR | text | Str | SM/tel | M | P2 | PHY | D | N |  |  | `unavailable: compound names exist per axle only (tyres.front_compound_name)` |
| 0x037A | `tyres.rl.compound_name` | `TyreCompound_RL` | Tyre compound name RL | text | Str | SM/tel | M | P2 | PHY | D | N |  |  | `unavailable: compound names exist per axle only (tyres.rear_compound_name)` |
| 0x037B | `tyres.rr.compound_name` | `TyreCompound_RR` | Tyre compound name RR | text | Str | SM/tel | M | P2 | PHY | D | N |  |  | `unavailable: compound names exist per axle only (tyres.rear_compound_name)` |
| 0x03E0 | `tyres.front_compound_name` | `TyreCompoundFront` | Front tyre compound | text | Str | SM/tel | M | P1 | STA | D | A |  |  | `telemetry.telemInfo[player].mFrontTireCompoundName` |
| 0x03E1 | `tyres.rear_compound_name` | `TyreCompoundRear` | Rear tyre compound | text | Str | SM/tel | M | P1 | STA | D | A |  |  | `telemetry.telemInfo[player].mRearTireCompoundName` |
| 0x03E2 | `tyres.front_compound_index` | `TyreCompoundIndexFront` | Front tyre compound index | count | U8 | SM/tel | M | P2 | STA | D | A |  |  | `telemetry.telemInfo[player].mFrontTireCompoundIndex` |
| 0x03E3 | `tyres.rear_compound_index` | `TyreCompoundIndexRear` | Rear tyre compound index | count | U8 | SM/tel | M | P2 | STA | D | A |  |  | `telemetry.telemInfo[player].mRearTireCompoundIndex` |
| 0x03E4 | `tyres.sets_available` | `TyreSetsAvailable` | Tyre sets available | count | U8 | REST | M | P2 | ENV | D | N |  |  | `unavailable: REST pit menu (CLAIMED)` |

## 0x0400 Brakes

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0400 | `brakes.fl.temp_c` | `BrakeTemp_FL` | Brake temp FL (unit detected, A-LMU-015) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -50 … 1500 | 3000 | `telemetry.telemInfo[player].mWheel[0].mBrakeTemp` |
| 0x0401 | `brakes.fr.temp_c` | `BrakeTemp_FR` | Brake temp FR (unit detected, A-LMU-015) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -50 … 1500 | 3000 | `telemetry.telemInfo[player].mWheel[1].mBrakeTemp` |
| 0x0402 | `brakes.rl.temp_c` | `BrakeTemp_RL` | Brake temp RL (unit detected, A-LMU-015) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -50 … 1500 | 3000 | `telemetry.telemInfo[player].mWheel[2].mBrakeTemp` |
| 0x0403 | `brakes.rr.temp_c` | `BrakeTemp_RR` | Brake temp RR (unit detected, A-LMU-015) | °C | F32 | SM/tel | M | P1 | PHY | C | A | -50 … 1500 | 3000 | `telemetry.telemInfo[player].mWheel[3].mBrakeTemp` |
| 0x0404 | `brakes.fl.pressure_frac` | `BrakePressure_FL` | Brake pressure FL (fraction, not Pa) | 0..1 | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 1.01 |  | `telemetry.telemInfo[player].mWheel[0].mBrakePressure` |
| 0x0405 | `brakes.fr.pressure_frac` | `BrakePressure_FR` | Brake pressure FR (fraction, not Pa) | 0..1 | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 1.01 |  | `telemetry.telemInfo[player].mWheel[1].mBrakePressure` |
| 0x0406 | `brakes.rl.pressure_frac` | `BrakePressure_RL` | Brake pressure RL (fraction, not Pa) | 0..1 | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 1.01 |  | `telemetry.telemInfo[player].mWheel[2].mBrakePressure` |
| 0x0407 | `brakes.rr.pressure_frac` | `BrakePressure_RR` | Brake pressure RR (fraction, not Pa) | 0..1 | F32 | SM/tel | M | P1 | PHY | C | A | 0 … 1.01 |  | `telemetry.telemInfo[player].mWheel[3].mBrakePressure` |
| 0x0408 | `brakes.fl.line_pressure_pa` | `BrakeLinePressure_FL` | Brake line pressure FL | Pa | F32 | SM/tel | M | P2 | PHY | C | N |  |  | `unavailable: mBrakePressure is a 0..1 fraction; kPa promised for a future LMU version` |
| 0x0409 | `brakes.fr.line_pressure_pa` | `BrakeLinePressure_FR` | Brake line pressure FR | Pa | F32 | SM/tel | M | P2 | PHY | C | N |  |  | `unavailable: mBrakePressure is a 0..1 fraction; kPa promised for a future LMU version` |
| 0x040A | `brakes.rl.line_pressure_pa` | `BrakeLinePressure_RL` | Brake line pressure RL | Pa | F32 | SM/tel | M | P2 | PHY | C | N |  |  | `unavailable: mBrakePressure is a 0..1 fraction; kPa promised for a future LMU version` |
| 0x040B | `brakes.rr.line_pressure_pa` | `BrakeLinePressure_RR` | Brake line pressure RR | Pa | F32 | SM/tel | M | P2 | PHY | C | N |  |  | `unavailable: mBrakePressure is a 0..1 fraction; kPa promised for a future LMU version` |
| 0x040C | `brakes.fl.wear_frac` | `BrakeWear_FL` | Brake wear FL | 0..1 | F32 | REST | M | P2 | ENV | C | N |  |  | `unavailable: REST RepairAndRefuel wearables.brakes (CLAIMED)` |
| 0x040D | `brakes.fr.wear_frac` | `BrakeWear_FR` | Brake wear FR | 0..1 | F32 | REST | M | P2 | ENV | C | N |  |  | `unavailable: REST RepairAndRefuel wearables.brakes (CLAIMED)` |
| 0x040E | `brakes.rl.wear_frac` | `BrakeWear_RL` | Brake wear RL | 0..1 | F32 | REST | M | P2 | ENV | C | N |  |  | `unavailable: REST RepairAndRefuel wearables.brakes (CLAIMED)` |
| 0x040F | `brakes.rr.wear_frac` | `BrakeWear_RR` | Brake wear RR | 0..1 | F32 | REST | M | P2 | ENV | C | N |  |  | `unavailable: REST RepairAndRefuel wearables.brakes (CLAIMED)` |
| 0x0410 | `brakes.fl.thickness_m` | `BrakeThickness_FL` | Brake disc thickness FL | m | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (Brake Thickness)` |
| 0x0411 | `brakes.fr.thickness_m` | `BrakeThickness_FR` | Brake disc thickness FR | m | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (Brake Thickness)` |
| 0x0412 | `brakes.rl.thickness_m` | `BrakeThickness_RL` | Brake disc thickness RL | m | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (Brake Thickness)` |
| 0x0413 | `brakes.rr.thickness_m` | `BrakeThickness_RR` | Brake disc thickness RR | m | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (Brake Thickness)` |
| 0x0414 | `brakes.fl.temp_core_c` | `BrakeCoreTemp_FL` | Brake core temp FL | °C | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (Brakes Core Temp)` |
| 0x0415 | `brakes.fr.temp_core_c` | `BrakeCoreTemp_FR` | Brake core temp FR | °C | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (Brakes Core Temp)` |
| 0x0416 | `brakes.rl.temp_core_c` | `BrakeCoreTemp_RL` | Brake core temp RL | °C | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (Brakes Core Temp)` |
| 0x0417 | `brakes.rr.temp_core_c` | `BrakeCoreTemp_RR` | Brake core temp RR | °C | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (Brakes Core Temp)` |
| 0x0418 | `brakes.fl.temp_air_c` | `BrakeAirTemp_FL` | Brake air temp FL | °C | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (Brakes Air Temp)` |
| 0x0419 | `brakes.fr.temp_air_c` | `BrakeAirTemp_FR` | Brake air temp FR | °C | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (Brakes Air Temp)` |
| 0x041A | `brakes.rl.temp_air_c` | `BrakeAirTemp_RL` | Brake air temp RL | °C | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (Brakes Air Temp)` |
| 0x041B | `brakes.rr.temp_air_c` | `BrakeAirTemp_RR` | Brake air temp RR | °C | F32 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only (Brakes Air Temp)` |
| 0x0480 | `brakes.bias_rear_frac` | `BrakeBiasRear` | Brake bias (rear share) | 0..1 | F32 | SM/tel | M | P1 | PHY | C | A | 0.2 … 0.8 |  | `telemetry.telemInfo[player].mRearBrakeBias` |
| 0x0481 | `brakes.migration` | `BrakeMigration` | Brake migration setting | count | U8 | SM/tel | M | P2 | PHY | D | V |  |  | `telemetry.telemInfo[player].mMigration` |
| 0x0482 | `brakes.migration_max` | `BrakeMigrationMax` | Brake migration setting max | count | U8 | SM/tel | M | P2 | STA | D | V |  |  | `telemetry.telemInfo[player].mMigrationMax` |

## 0x0500 Dynamics

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0500 | `vehicle.local_vel_x_mps` | `LocalVelX` | Local velocity x (LMU frame, + left) | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 | 1500 | `telemetry.telemInfo[player].mLocalVel.x` |
| 0x0501 | `vehicle.local_vel_y_mps` | `LocalVelY` | Local velocity y (LMU frame, + up) | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 | 1500 | `telemetry.telemInfo[player].mLocalVel.y` |
| 0x0502 | `vehicle.local_vel_z_mps` | `LocalVelZ` | Local velocity z (LMU frame, + rear) | m/s | F32 | SM/tel | M | P2 | PHY | C | A | -150 … 150 | 1500 | `telemetry.telemInfo[player].mLocalVel.z` |
| 0x0503 | `vehicle.local_accel_x_mps2` | `LocalAccelX` | Local acceleration x (LMU frame) | m/s² | F32 | SM/tel | M | P2 | PHY | C | A | -490 … 490 |  | `telemetry.telemInfo[player].mLocalAccel.x` |
| 0x0504 | `vehicle.local_accel_y_mps2` | `LocalAccelY` | Local acceleration y (LMU frame) | m/s² | F32 | SM/tel | M | P2 | PHY | C | A | -490 … 490 |  | `telemetry.telemInfo[player].mLocalAccel.y` |
| 0x0505 | `vehicle.local_accel_z_mps2` | `LocalAccelZ` | Local acceleration z (LMU frame) | m/s² | F32 | SM/tel | M | P2 | PHY | C | A | -490 … 490 |  | `telemetry.telemInfo[player].mLocalAccel.z` |
| 0x0506 | `vehicle.local_rot_x_radps` | `LocalRotX` | Local rotation rate x (LMU frame, + pitch up) | rad/s | F32 | SM/tel | M | P2 | PHY | C | A | -20 … 20 |  | `telemetry.telemInfo[player].mLocalRot.x` |
| 0x0507 | `vehicle.local_rot_y_radps` | `LocalRotY` | Local rotation rate y (LMU frame, + yaw right) | rad/s | F32 | SM/tel | M | P2 | PHY | C | A | -20 … 20 |  | `telemetry.telemInfo[player].mLocalRot.y` |
| 0x0508 | `vehicle.local_rot_z_radps` | `LocalRotZ` | Local rotation rate z (LMU frame, + roll right) | rad/s | F32 | SM/tel | M | P2 | PHY | C | A | -20 … 20 |  | `telemetry.telemInfo[player].mLocalRot.z` |
| 0x0509 | `vehicle.local_rot_accel_x_radps2` | `LocalRotAccelX` | Local rotational acceleration x (LMU frame) | rad/s² | F32 | SM/tel | M | P2 | PHY | C | A | -500 … 500 |  | `telemetry.telemInfo[player].mLocalRotAccel.x` |
| 0x050A | `vehicle.local_rot_accel_y_radps2` | `LocalRotAccelY` | Local rotational acceleration y (LMU frame) | rad/s² | F32 | SM/tel | M | P2 | PHY | C | A | -500 … 500 |  | `telemetry.telemInfo[player].mLocalRotAccel.y` |
| 0x050B | `vehicle.local_rot_accel_z_radps2` | `LocalRotAccelZ` | Local rotational acceleration z (LMU frame) | rad/s² | F32 | SM/tel | M | P2 | PHY | C | A | -500 … 500 |  | `telemetry.telemInfo[player].mLocalRotAccel.z` |
| 0x050C | `vehicle.orientation_row0_x` | `OriRow0X` | Orientation matrix row 0 x | — | F32 | SM/tel | M | P2 | PHY | C | A | -1.01 … 1.01 |  | `telemetry.telemInfo[player].mOri[0].x` |
| 0x050D | `vehicle.orientation_row0_y` | `OriRow0Y` | Orientation matrix row 0 y | — | F32 | SM/tel | M | P2 | PHY | C | A | -1.01 … 1.01 |  | `telemetry.telemInfo[player].mOri[0].y` |
| 0x050E | `vehicle.orientation_row0_z` | `OriRow0Z` | Orientation matrix row 0 z | — | F32 | SM/tel | M | P2 | PHY | C | A | -1.01 … 1.01 |  | `telemetry.telemInfo[player].mOri[0].z` |
| 0x050F | `vehicle.orientation_row1_x` | `OriRow1X` | Orientation matrix row 1 x | — | F32 | SM/tel | M | P2 | PHY | C | A | -1.01 … 1.01 |  | `telemetry.telemInfo[player].mOri[1].x` |
| 0x0510 | `vehicle.orientation_row1_y` | `OriRow1Y` | Orientation matrix row 1 y | — | F32 | SM/tel | M | P2 | PHY | C | A | -1.01 … 1.01 |  | `telemetry.telemInfo[player].mOri[1].y` |
| 0x0511 | `vehicle.orientation_row1_z` | `OriRow1Z` | Orientation matrix row 1 z | — | F32 | SM/tel | M | P2 | PHY | C | A | -1.01 … 1.01 |  | `telemetry.telemInfo[player].mOri[1].z` |
| 0x0512 | `vehicle.orientation_row2_x` | `OriRow2X` | Orientation matrix row 2 x | — | F32 | SM/tel | M | P2 | PHY | C | A | -1.01 … 1.01 |  | `telemetry.telemInfo[player].mOri[2].x` |
| 0x0513 | `vehicle.orientation_row2_y` | `OriRow2Y` | Orientation matrix row 2 y | — | F32 | SM/tel | M | P2 | PHY | C | A | -1.01 … 1.01 |  | `telemetry.telemInfo[player].mOri[2].y` |
| 0x0514 | `vehicle.orientation_row2_z` | `OriRow2Z` | Orientation matrix row 2 z | — | F32 | SM/tel | M | P2 | PHY | C | A | -1.01 … 1.01 |  | `telemetry.telemInfo[player].mOri[2].z` |
| 0x0520 | `dynamics.g_long` | `G_Long` | Longitudinal G (+ accelerating) | g | F32 | CALC | D | P1 | PHY | C | A | -50 … 50 |  | `formula:g_long_from_local_accel` |
| 0x0521 | `dynamics.g_lat` | `G_Lat` | Lateral G (+ toward the left, ISO) | g | F32 | CALC | D | P1 | PHY | C | A | -50 … 50 |  | `formula:g_lat_from_local_accel` |
| 0x0522 | `dynamics.g_vert` | `G_Vert` | Vertical G | g | F32 | CALC | D | P2 | PHY | C | A | -50 … 50 |  | `formula:g_vert_from_local_accel` |
| 0x0523 | `dynamics.g_combined` | `G_Combined` | Combined lateral/longitudinal G | g | F32 | CALC | D | P2 | PHY | C | A | 0 … 50 |  | `formula:g_combined_lat_long` |
| 0x0524 | `dynamics.yaw_rate_radps` | `YawRate` | Yaw rate (+ left, ISO) | rad/s | F32 | CALC | D | P1 | PHY | C | A | -20 … 20 |  | `formula:yaw_rate_iso` |
| 0x0525 | `dynamics.pitch_rate_radps` | `PitchRate` | Pitch rate (+ nose down, ISO) | rad/s | F32 | CALC | D | P2 | PHY | C | A | -20 … 20 |  | `formula:pitch_rate_iso` |
| 0x0526 | `dynamics.roll_rate_radps` | `RollRate` | Roll rate (+ right, ISO) | rad/s | F32 | CALC | D | P2 | PHY | C | A | -20 … 20 |  | `formula:roll_rate_iso` |
| 0x0530 | `chassis.front_ride_height_m` | `RideHeightFront` | Front ride height | m | F32 | SM/tel | M | P1 | PHY | C | A | -0.05 … 0.3 |  | `telemetry.telemInfo[player].mFrontRideHeight` |
| 0x0531 | `chassis.rear_ride_height_m` | `RideHeightRear` | Rear ride height | m | F32 | SM/tel | M | P1 | PHY | C | A | -0.05 … 0.3 |  | `telemetry.telemInfo[player].mRearRideHeight` |
| 0x0532 | `chassis.front_3rd_deflection_m` | `ThirdSpringDeflFront` | Front third-spring deflection | m | F32 | SM/tel | M | P2 | PHY | C | V | -0.2 … 0.2 |  | `telemetry.telemInfo[player].mFront3rdDeflection` |
| 0x0533 | `chassis.rear_3rd_deflection_m` | `ThirdSpringDeflRear` | Rear third-spring deflection | m | F32 | SM/tel | M | P2 | PHY | C | V | -0.2 … 0.2 |  | `telemetry.telemInfo[player].mRear3rdDeflection` |
| 0x0534 | `aero.front_wing_height_m` | `FrontWingHeight` | Front wing height | m | F32 | SM/tel | M | P2 | PHY | C | A | -0.05 … 0.5 |  | `telemetry.telemInfo[player].mFrontWingHeight` |
| 0x0535 | `aero.drag_n` | `Drag` | Drag force | N | F32 | SM/tel | M | P2 | PHY | C | A | -1000 … 30000 |  | `telemetry.telemInfo[player].mDrag` |
| 0x0536 | `aero.front_downforce_n` | `DownforceFront` | Front downforce | N | F32 | SM/tel | M | P2 | PHY | C | A | -5000 … 60000 |  | `telemetry.telemInfo[player].mFrontDownforce` |
| 0x0537 | `aero.rear_downforce_n` | `DownforceRear` | Rear downforce | N | F32 | SM/tel | M | P2 | PHY | C | A | -5000 … 60000 |  | `telemetry.telemInfo[player].mRearDownforce` |
| 0x0538 | `chassis.steer_angle_road_wheel_rad` | `SteerAngleRoadWheel` | Road-wheel steer angle | rad | F32 | SM/tel | M | P2 | PHY | C | N |  |  | `unavailable: LMU exposes only the steering input and wheel range` |
| 0x0540 | `suspension.fl.deflection_m` | `SuspPos_FL` | Suspension deflection FL | m | F32 | SM/tel | M | P1 | PHY | C | A | -0.3 … 0.3 |  | `telemetry.telemInfo[player].mWheel[0].mSuspensionDeflection` |
| 0x0541 | `suspension.fr.deflection_m` | `SuspPos_FR` | Suspension deflection FR | m | F32 | SM/tel | M | P1 | PHY | C | A | -0.3 … 0.3 |  | `telemetry.telemInfo[player].mWheel[1].mSuspensionDeflection` |
| 0x0542 | `suspension.rl.deflection_m` | `SuspPos_RL` | Suspension deflection RL | m | F32 | SM/tel | M | P1 | PHY | C | A | -0.3 … 0.3 |  | `telemetry.telemInfo[player].mWheel[2].mSuspensionDeflection` |
| 0x0543 | `suspension.rr.deflection_m` | `SuspPos_RR` | Suspension deflection RR | m | F32 | SM/tel | M | P1 | PHY | C | A | -0.3 … 0.3 |  | `telemetry.telemInfo[player].mWheel[3].mSuspensionDeflection` |
| 0x0544 | `suspension.fl.ride_height_m` | `RideHeight_FL` | Ride height FL | m | F32 | SM/tel | M | P1 | PHY | C | A | -0.05 … 0.3 |  | `telemetry.telemInfo[player].mWheel[0].mRideHeight` |
| 0x0545 | `suspension.fr.ride_height_m` | `RideHeight_FR` | Ride height FR | m | F32 | SM/tel | M | P1 | PHY | C | A | -0.05 … 0.3 |  | `telemetry.telemInfo[player].mWheel[1].mRideHeight` |
| 0x0546 | `suspension.rl.ride_height_m` | `RideHeight_RL` | Ride height RL | m | F32 | SM/tel | M | P1 | PHY | C | A | -0.05 … 0.3 |  | `telemetry.telemInfo[player].mWheel[2].mRideHeight` |
| 0x0547 | `suspension.rr.ride_height_m` | `RideHeight_RR` | Ride height RR | m | F32 | SM/tel | M | P1 | PHY | C | A | -0.05 … 0.3 |  | `telemetry.telemInfo[player].mWheel[3].mRideHeight` |
| 0x0548 | `suspension.fl.pushrod_force_n` | `PushrodForce_FL` | Pushrod force FL | N | F32 | SM/tel | M | P2 | PHY | C | A | -20000 … 150000 |  | `telemetry.telemInfo[player].mWheel[0].mSuspForce` |
| 0x0549 | `suspension.fr.pushrod_force_n` | `PushrodForce_FR` | Pushrod force FR | N | F32 | SM/tel | M | P2 | PHY | C | A | -20000 … 150000 |  | `telemetry.telemInfo[player].mWheel[1].mSuspForce` |
| 0x054A | `suspension.rl.pushrod_force_n` | `PushrodForce_RL` | Pushrod force RL | N | F32 | SM/tel | M | P2 | PHY | C | A | -20000 … 150000 |  | `telemetry.telemInfo[player].mWheel[2].mSuspForce` |
| 0x054B | `suspension.rr.pushrod_force_n` | `PushrodForce_RR` | Pushrod force RR | N | F32 | SM/tel | M | P2 | PHY | C | A | -20000 … 150000 |  | `telemetry.telemInfo[player].mWheel[3].mSuspForce` |
| 0x054C | `wheels.fl.camber_rad` | `Camber_FL` | Camber FL | rad | F32 | SM/tel | M | P2 | PHY | C | A | -0.35 … 0.35 |  | `telemetry.telemInfo[player].mWheel[0].mCamber` |
| 0x054D | `wheels.fr.camber_rad` | `Camber_FR` | Camber FR | rad | F32 | SM/tel | M | P2 | PHY | C | A | -0.35 … 0.35 |  | `telemetry.telemInfo[player].mWheel[1].mCamber` |
| 0x054E | `wheels.rl.camber_rad` | `Camber_RL` | Camber RL | rad | F32 | SM/tel | M | P2 | PHY | C | A | -0.35 … 0.35 |  | `telemetry.telemInfo[player].mWheel[2].mCamber` |
| 0x054F | `wheels.rr.camber_rad` | `Camber_RR` | Camber RR | rad | F32 | SM/tel | M | P2 | PHY | C | A | -0.35 … 0.35 |  | `telemetry.telemInfo[player].mWheel[3].mCamber` |
| 0x0550 | `wheels.fl.toe_rad` | `Toe_FL` | Toe FL | rad | F32 | SM/tel | M | P2 | PHY | C | A | -0.2 … 0.2 |  | `telemetry.telemInfo[player].mWheel[0].mToe` |
| 0x0551 | `wheels.fr.toe_rad` | `Toe_FR` | Toe FR | rad | F32 | SM/tel | M | P2 | PHY | C | A | -0.2 … 0.2 |  | `telemetry.telemInfo[player].mWheel[1].mToe` |
| 0x0552 | `wheels.rl.toe_rad` | `Toe_RL` | Toe RL | rad | F32 | SM/tel | M | P2 | PHY | C | A | -0.2 … 0.2 |  | `telemetry.telemInfo[player].mWheel[2].mToe` |
| 0x0553 | `wheels.rr.toe_rad` | `Toe_RR` | Toe RR | rad | F32 | SM/tel | M | P2 | PHY | C | A | -0.2 … 0.2 |  | `telemetry.telemInfo[player].mWheel[3].mToe` |
| 0x0554 | `wheels.fl.rotation_radps` | `WheelRot_FL` | Wheel rotation FL | rad/s | F32 | SM/tel | M | P1 | PHY | C | A | -1500 … 1500 |  | `telemetry.telemInfo[player].mWheel[0].mRotation` |
| 0x0555 | `wheels.fr.rotation_radps` | `WheelRot_FR` | Wheel rotation FR | rad/s | F32 | SM/tel | M | P1 | PHY | C | A | -1500 … 1500 |  | `telemetry.telemInfo[player].mWheel[1].mRotation` |
| 0x0556 | `wheels.rl.rotation_radps` | `WheelRot_RL` | Wheel rotation RL | rad/s | F32 | SM/tel | M | P1 | PHY | C | A | -1500 … 1500 |  | `telemetry.telemInfo[player].mWheel[2].mRotation` |
| 0x0557 | `wheels.rr.rotation_radps` | `WheelRot_RR` | Wheel rotation RR | rad/s | F32 | SM/tel | M | P1 | PHY | C | A | -1500 … 1500 |  | `telemetry.telemInfo[player].mWheel[3].mRotation` |
| 0x0558 | `wheels.fl.y_location_m` | `WheelYLocation_FL` | Wheel height relative to body FL | m | F32 | SM/tel | M | P2 | PHY | C | A | -1 … 1 |  | `telemetry.telemInfo[player].mWheel[0].mWheelYLocation` |
| 0x0559 | `wheels.fr.y_location_m` | `WheelYLocation_FR` | Wheel height relative to body FR | m | F32 | SM/tel | M | P2 | PHY | C | A | -1 … 1 |  | `telemetry.telemInfo[player].mWheel[1].mWheelYLocation` |
| 0x055A | `wheels.rl.y_location_m` | `WheelYLocation_RL` | Wheel height relative to body RL | m | F32 | SM/tel | M | P2 | PHY | C | A | -1 … 1 |  | `telemetry.telemInfo[player].mWheel[2].mWheelYLocation` |
| 0x055B | `wheels.rr.y_location_m` | `WheelYLocation_RR` | Wheel height relative to body RR | m | F32 | SM/tel | M | P2 | PHY | C | A | -1 … 1 |  | `telemetry.telemInfo[player].mWheel[3].mWheelYLocation` |
| 0x055C | `wheels.fl.detached` | `WheelDetached_FL` | Wheel detached FL | — | Bool | SM/tel | M | P1 | PHY | D | A |  |  | `telemetry.telemInfo[player].mWheel[0].mDetached` |
| 0x055D | `wheels.fr.detached` | `WheelDetached_FR` | Wheel detached FR | — | Bool | SM/tel | M | P1 | PHY | D | A |  |  | `telemetry.telemInfo[player].mWheel[1].mDetached` |
| 0x055E | `wheels.rl.detached` | `WheelDetached_RL` | Wheel detached RL | — | Bool | SM/tel | M | P1 | PHY | D | A |  |  | `telemetry.telemInfo[player].mWheel[2].mDetached` |
| 0x055F | `wheels.rr.detached` | `WheelDetached_RR` | Wheel detached RR | — | Bool | SM/tel | M | P1 | PHY | D | A |  |  | `telemetry.telemInfo[player].mWheel[3].mDetached` |
| 0x0560 | `wheels.fl.speed_mps` | `WheelSpeed_FL` | Wheel speed FL | m/s | F32 | CALC | E | P1 | PHY | C | A | 0 … 300 |  | `formula:wheel_speed_from_rotation_radius` |
| 0x0561 | `wheels.fr.speed_mps` | `WheelSpeed_FR` | Wheel speed FR | m/s | F32 | CALC | E | P1 | PHY | C | A | 0 … 300 |  | `formula:wheel_speed_from_rotation_radius` |
| 0x0562 | `wheels.rl.speed_mps` | `WheelSpeed_RL` | Wheel speed RL | m/s | F32 | CALC | E | P1 | PHY | C | A | 0 … 300 |  | `formula:wheel_speed_from_rotation_radius` |
| 0x0563 | `wheels.rr.speed_mps` | `WheelSpeed_RR` | Wheel speed RR | m/s | F32 | CALC | E | P1 | PHY | C | A | 0 … 300 |  | `formula:wheel_speed_from_rotation_radius` |
| 0x0564 | `tyres.fl.slip_ratio` | `SlipRatio_FL` | Slip ratio FL (experimental) | 0..1 | F32 | CALC | D | P2 | PHY | C | X | -2 … 2 |  | `formula:slip_ratio_patch_over_ground` |
| 0x0565 | `tyres.fr.slip_ratio` | `SlipRatio_FR` | Slip ratio FR (experimental) | 0..1 | F32 | CALC | D | P2 | PHY | C | X | -2 … 2 |  | `formula:slip_ratio_patch_over_ground` |
| 0x0566 | `tyres.rl.slip_ratio` | `SlipRatio_RL` | Slip ratio RL (experimental) | 0..1 | F32 | CALC | D | P2 | PHY | C | X | -2 … 2 |  | `formula:slip_ratio_patch_over_ground` |
| 0x0567 | `tyres.rr.slip_ratio` | `SlipRatio_RR` | Slip ratio RR (experimental) | 0..1 | F32 | CALC | D | P2 | PHY | C | X | -2 … 2 |  | `formula:slip_ratio_patch_over_ground` |
| 0x0568 | `tyres.fl.slip_angle_rad` | `SlipAngle_FL` | Slip angle FL (experimental) | rad | F32 | CALC | D | P2 | PHY | C | X | -1.570796 … 1.570796 |  | `formula:slip_angle_from_ground_vel` |
| 0x0569 | `tyres.fr.slip_angle_rad` | `SlipAngle_FR` | Slip angle FR (experimental) | rad | F32 | CALC | D | P2 | PHY | C | X | -1.570796 … 1.570796 |  | `formula:slip_angle_from_ground_vel` |
| 0x056A | `tyres.rl.slip_angle_rad` | `SlipAngle_RL` | Slip angle RL (experimental) | rad | F32 | CALC | D | P2 | PHY | C | X | -1.570796 … 1.570796 |  | `formula:slip_angle_from_ground_vel` |
| 0x056B | `tyres.rr.slip_angle_rad` | `SlipAngle_RR` | Slip angle RR (experimental) | rad | F32 | CALC | D | P2 | PHY | C | X | -1.570796 … 1.570796 |  | `formula:slip_angle_from_ground_vel` |

## 0x0600 Hybrid & energy

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0600 | `energy.virtual_energy_frac` | `VirtualEnergy` | Virtual energy (scale detected) | 0..1 | F32 | SM/tel | M | P0 | PHY | C | V | 0 … 1 | 1 | `telemetry.telemInfo[player].mVirtualEnergy` |
| 0x0601 | `energy.soc_frac` | `BatterySoC` | Battery state of charge (scale detected) | 0..1 | F32 | SM/tel | M | P1 | PHY | C | V | 0 … 1 | 1 | `telemetry.telemInfo[player].mSoC` |
| 0x0602 | `energy.battery_charge_frac` | `BatteryCharge` | Battery charge | 0..1 | F32 | SM/tel | M | P1 | PHY | C | V | 0 … 1 | 1 | `telemetry.telemInfo[player].mBatteryChargeFraction` |
| 0x0603 | `energy.regen_power_w` | `RegenPower` | Regeneration power | W | F32 | SM/tel | M | P1 | PHY | C | V | -1000000 … 1000000 |  | `telemetry.telemInfo[player].mRegen` |
| 0x0604 | `hybrid.motor_torque_nm` | `HybridMotorTorque` | Hybrid motor torque (- regenerating) | N·m | F32 | SM/tel | M | P2 | PHY | C | V | -2000 … 2000 |  | `telemetry.telemInfo[player].mElectricBoostMotorTorque` |
| 0x0605 | `hybrid.motor_rpm` | `HybridMotorRPM` | Hybrid motor speed | rpm | F32 | SM/tel | M | P2 | PHY | C | V | -50000 … 50000 |  | `telemetry.telemInfo[player].mElectricBoostMotorRPM` |
| 0x0606 | `hybrid.motor_temp_c` | `HybridMotorTemp` | Hybrid motor temperature | °C | F32 | SM/tel | M | P1 | PHY | C | V | -40 … 250 | 100 | `telemetry.telemInfo[player].mElectricBoostMotorTemperature` |
| 0x0607 | `hybrid.motor_water_temp_c` | `HybridWaterTemp` | Hybrid cooler temperature | °C | F32 | SM/tel | M | P1 | PHY | C | V | -40 … 200 | 100 | `telemetry.telemInfo[player].mElectricBoostWaterTemperature` |
| 0x0608 | `hybrid.motor_state` | `HybridMotorState` | Hybrid motor state | enum | U8 | SM/tel | M | P1 | PHY | D | V | 0 … 3 |  | `telemetry.telemInfo[player].mElectricBoostMotorState` |
| 0x0609 | `energy.lift_and_coast_progress` | `LiftAndCoast` | Lift-and-coast progress (raw) | count | U8 | SM/tel | M | P2 | PHY | D | V |  |  | `telemetry.telemInfo[player].mLiftAndCoastProgress` |

## 0x0700 Timing

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0700 | `time.source_et_s` | `Time` | Elapsed session time (telemetry) | s | F64 | SM/tel | M | P0 | PHY | C | A | 0 … 10000000 |  | `telemetry.telemInfo[player].mElapsedTime` |
| 0x0701 | `time.delta_s` | `UpdateDeltaTime` | Physics update interval | s | F32 | SM/tel | M | P2 | PHY | C | A | 0.000122 … 0.5 |  | `telemetry.telemInfo[player].mDeltaTime` |
| 0x0702 | `lap.number` | `Lap` | Lap number | count | I32 | SM/tel | M | P0 | PHY | D | A | 0 … 100000 |  | `telemetry.telemInfo[player].mLapNumber` |
| 0x0703 | `lap.start_et_s` | `LapStartTime` | Lap start time | s | F64 | SM/tel | M | P1 | PHY | D | A | 0 … 10000000 |  | `telemetry.telemInfo[player].mLapStartET` |
| 0x0704 | `lap.invalidated` | `LapInvalid` | Lap invalidated | — | Bool | SM/tel | M | P0 | PHY | D | A |  |  | `telemetry.telemInfo[player].mLapInvalidated` |
| 0x0705 | `timing.delta_best_s` | `DeltaBest` | Delta to best lap (LMU) | s | F64 | SM/tel | M | P0 | PHY | D | A | -600 … 600 |  | `telemetry.telemInfo[player].mDeltaBest` |
| 0x0706 | `timing.gap_car_ahead_s` | `GapCarAhead` | Gap to car ahead on track | s | F32 | SM/tel | M | P0 | PHY | C | A | 0 … 3600 |  | `telemetry.telemInfo[player].mTimeGapCarAhead` |
| 0x0707 | `timing.gap_car_behind_s` | `GapCarBehind` | Gap to car behind on track | s | F32 | SM/tel | M | P0 | PHY | C | A | 0 … 3600 |  | `telemetry.telemInfo[player].mTimeGapCarBehind` |
| 0x0708 | `timing.gap_place_ahead_s` | `GapPlaceAhead` | Gap to the next place | s | F32 | SM/tel | M | P0 | PHY | C | A | 0 … 3600 |  | `telemetry.telemInfo[player].mTimeGapPlaceAhead` |
| 0x0709 | `timing.gap_place_behind_s` | `GapPlaceBehind` | Gap to the previous place | s | F32 | SM/tel | M | P0 | PHY | C | A | 0 … 3600 |  | `telemetry.telemInfo[player].mTimeGapPlaceBehind` |
| 0x070A | `timing.best_lap_s` | `BestLapTime` | Best lap time | s | F64 | SM/sco | M | P0 | SCO | D | A | 0.001 … 3600 |  | `scoring.vehScoringInfo[player].mBestLapTime` |
| 0x070B | `timing.last_lap_s` | `LastLapTime` | Last lap time | s | F64 | SM/sco | M | P0 | SCO | D | A | 0.001 … 3600 |  | `scoring.vehScoringInfo[player].mLastLapTime` |
| 0x070C | `timing.best_sector1_s` | `BestSector1` | Best sector 1 | s | F64 | SM/sco | M | P1 | SCO | D | A | 0.001 … 3600 |  | `scoring.vehScoringInfo[player].mBestSector1` |
| 0x070D | `timing.best_sector12_s` | `BestSector12` | Best sector 1+2 (cumulative) | s | F64 | SM/sco | M | P1 | SCO | D | A | 0.001 … 3600 |  | `scoring.vehScoringInfo[player].mBestSector2` |
| 0x070E | `timing.last_sector1_s` | `LastSector1` | Last sector 1 | s | F64 | SM/sco | M | P1 | SCO | D | A | 0.001 … 3600 |  | `scoring.vehScoringInfo[player].mLastSector1` |
| 0x070F | `timing.last_sector12_s` | `LastSector12` | Last sector 1+2 (cumulative) | s | F64 | SM/sco | M | P1 | SCO | D | A | 0.001 … 3600 |  | `scoring.vehScoringInfo[player].mLastSector2` |
| 0x0710 | `timing.cur_sector1_s` | `CurSector1` | Current lap sector 1 | s | F64 | SM/sco | M | P1 | SCO | D | A | 0.001 … 3600 |  | `scoring.vehScoringInfo[player].mCurSector1` |
| 0x0711 | `timing.cur_sector12_s` | `CurSector12` | Current lap sector 1+2 (cumulative) | s | F64 | SM/sco | M | P1 | SCO | D | A | 0.001 … 3600 |  | `scoring.vehScoringInfo[player].mCurSector2` |
| 0x0712 | `timing.best_lap_sector1_s` | `BestLapSector1` | Sector 1 of the best lap | s | F32 | SM/sco | M | P2 | SCO | D | A | 0.001 … 3600 |  | `scoring.vehScoringInfo[player].mBestLapSector1` |
| 0x0713 | `timing.best_lap_sector12_s` | `BestLapSector12` | Sector 1+2 of the best lap (cumulative) | s | F32 | SM/sco | M | P2 | SCO | D | A | 0.001 … 3600 |  | `scoring.vehScoringInfo[player].mBestLapSector2` |
| 0x0714 | `timing.time_into_lap_s` | `TimeIntoLap` | Time into lap (LMU estimate) | s | F64 | SM/sco | M | P1 | SCO | D | A | 0 … 3600 |  | `scoring.vehScoringInfo[player].mTimeIntoLap` |
| 0x0715 | `timing.estimated_lap_s` | `EstimatedLapTime` | Estimated lap time (LMU) | s | F64 | SM/sco | M | P2 | SCO | D | A | 0.001 … 3600 |  | `scoring.vehScoringInfo[player].mEstimatedLapTime` |
| 0x0716 | `timing.lap_start_et_scoring_s` | `LapStartTimeScoring` | Lap start time (scoring) | s | F64 | SM/sco | M | P2 | SCO | D | A | 0 … 10000000 |  | `scoring.vehScoringInfo[player].mLapStartET` |
| 0x0720 | `lap.current_time_s` | `LapTime` | Current lap time | s | F64 | CALC | D | P0 | PHY | D | A | 0 … 3600 |  | `formula:current_lap_time` |
| 0x0721 | `timing.last_sector2_s` | `LastSector2` | Last sector 2 | s | F64 | CALC | D | P1 | SCO | D | A | 0.001 … 3600 |  | `formula:last_sector2_from_cumulative` |
| 0x0722 | `timing.last_sector3_s` | `LastSector3` | Last sector 3 | s | F64 | CALC | D | P1 | SCO | D | A | 0.001 … 3600 |  | `formula:last_sector3_from_lap_time` |
| 0x0723 | `timing.best_sector2_s` | `BestSector2` | Best sector 2 (approximation) | s | F64 | CALC | E | P1 | SCO | D | A | 0.001 … 3600 |  | `formula:best_sector2_from_cumulative` |
| 0x0724 | `timing.delta_personal_best_s` | `DeltaPB` | Delta to personal best (calculated) | s | F64 | CALC | D | P1 | PHY | D | A | -600 … 600 |  | `formula:delta_distance_time_trace` |
| 0x0725 | `timing.predicted_lap_s` | `PredictedLapTime` | Predicted lap time | s | F64 | CALC | E | P1 | PHY | D | A | 0 … 3600 |  | `formula:predicted_lap_best_plus_delta` |

## 0x0800 Session, race & scoring

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0800 | `session.type` | `SessionType` | Session type | enum | I32 | SM/sco | M | P0 | STA | D | A | 0 … 13 |  | `scoring.scoringInfo.mSession` |
| 0x0801 | `session.elapsed_s` | `SessionTime` | Elapsed session time (scoring) | s | F64 | SM/sco | M | P0 | SCO | C | A | 0 … 10000000 |  | `scoring.scoringInfo.mCurrentET` |
| 0x0802 | `session.end_s` | `SessionEndTime` | Session end time | s | F64 | SM/sco | M | P1 | STA | D | A | 0 … 10000000 |  | `scoring.scoringInfo.mEndET` |
| 0x0803 | `session.time_remaining_s` | `SessionTimeRemaining` | Session time remaining (LMU) | s | F32 | SM/sco | M | P0 | SCO | C | A | 0 … 10000000 |  | `scoring.scoringInfo.mSessionTimeRemaining` |
| 0x0804 | `session.max_laps` | `SessionMaxLaps` | Session lap limit | count | I32 | SM/sco | M | P1 | STA | D | A | 0 … 100000 |  | `scoring.scoringInfo.mMaxLaps` |
| 0x0805 | `session.num_vehicles` | `NumVehicles` | Vehicles in session | count | I32 | SM/sco | M | P1 | SCO | D | A | 0 … 104 |  | `scoring.scoringInfo.mNumVehicles` |
| 0x0806 | `session.active_vehicles_telemetry` | `NumVehiclesTelemetry` | Vehicles with telemetry | count | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 104 |  | `telemetry.activeVehicles` |
| 0x0807 | `session.player_has_vehicle` | `PlayerHasVehicle` | Player has a vehicle | — | Bool | SM/tel | M | P1 | PHY | D | A |  |  | `telemetry.playerHasVehicle` |
| 0x0808 | `session.player_slot_index` | `PlayerSlotIndex` | Player telemetry slot index | count | U8 | SM/tel | M | P2 | PHY | D | A | 0 … 103 |  | `telemetry.playerVehicleIdx` |
| 0x0809 | `session.game_phase` | `GamePhase` | Game phase | enum | U8 | SM/sco | M | P0 | SCO | D | A | 0 … 9 |  | `scoring.scoringInfo.mGamePhase` |
| 0x080A | `session.in_realtime` | `InRealtime` | In realtime (driving) | — | Bool | SM/sco | M | P0 | SCO | D | A |  |  | `scoring.scoringInfo.mInRealtime` |
| 0x080B | `session.start_light` | `StartLight` | Start light frame | count | U8 | SM/sco | M | P1 | SCO | D | A |  |  | `scoring.scoringInfo.mStartLight` |
| 0x080C | `session.num_red_lights` | `NumRedLights` | Number of red lights | count | U8 | SM/sco | M | P2 | STA | D | A | 0 … 20 |  | `scoring.scoringInfo.mNumRedLights` |
| 0x080D | `session.start_time_of_day_s` | `StartTimeOfDay` | Session start time of day | s | F32 | SM/sco | M | P2 | STA | D | A | 0 … 86400 |  | `scoring.scoringInfo.mStartET` |
| 0x080E | `session.time_of_day` | `TimeOfDay` | Time of day (scale detected, A-LMU-036) | s | F32 | SM/sco | M | P2 | ENV | D | A |  |  | `scoring.scoringInfo.mTimeOfDay` |
| 0x080F | `session.fixed_setup` | `FixedSetup` | Fixed setup | — | Bool | SM/sco | M | P2 | STA | D | A |  |  | `scoring.scoringInfo.mIsFixedSetup` |
| 0x0810 | `session.game_mode` | `GameMode` | Game mode | enum | U8 | SM/sco | M | P2 | STA | D | A | 0 … 3 |  | `scoring.scoringInfo.mGameMode` |
| 0x0811 | `session.max_players` | `MaxPlayers` | Maximum players | count | I32 | SM/sco | M | P2 | STA | D | A | 0 … 104 |  | `scoring.scoringInfo.mMaxPlayers` |
| 0x0812 | `rules.track_limits_steps_per_penalty` | `TLStepsPerPenalty` | Track-limit steps per penalty | count | U8 | SM/sco | M | P2 | STA | D | A |  |  | `scoring.scoringInfo.mTrackLimitsStepsPerPenalty` |
| 0x0813 | `rules.track_limits_steps_per_point` | `TLStepsPerPoint` | Track-limit steps per point | count | U8 | SM/sco | M | P2 | STA | D | A |  |  | `scoring.scoringInfo.mTrackLimitsStepsPerPoint` |
| 0x0814 | `session.time_remaining_calc_s` | `SessionTimeRemainingCalc` | Session time remaining (end - current) | s | F64 | CALC | D | P1 | SCO | C | A | 0 … 10000000 |  | `formula:session_end_minus_current` |
| 0x0815 | `session.track_name` | `TrackName` | Track name | text | Str | SM/sco | M | P1 | STA | D | A |  |  | `scoring.scoringInfo.mTrackName` |
| 0x0820 | `race.place` | `Position` | Position (overall) | count | U8 | SM/sco | M | P0 | SCO | D | A | 1 … 104 |  | `scoring.vehScoringInfo[player].mPlace` |
| 0x0821 | `race.class_place` | `ClassPosition` | Position (class) | count | U8 | CALC | D | P0 | SCO | D | A | 1 … 104 |  | `formula:class_place_by_place_within_class` |
| 0x0822 | `race.laps_completed` | `LapsCompleted` | Laps completed | count | I32 | SM/sco | M | P0 | SCO | D | A | 0 … 100000 |  | `scoring.vehScoringInfo[player].mTotalLaps` |
| 0x0823 | `race.finish_status` | `FinishStatus` | Finish status | enum | I8 | SM/sco | M | P0 | SCO | D | A | 0 … 3 |  | `scoring.vehScoringInfo[player].mFinishStatus` |
| 0x0824 | `race.time_behind_next_s` | `TimeBehindNext` | Time behind the next place | s | F64 | SM/sco | M | P0 | SCO | C | A | 0 … 10000000 |  | `scoring.vehScoringInfo[player].mTimeBehindNext` |
| 0x0825 | `race.laps_behind_next` | `LapsBehindNext` | Laps behind the next place | count | I32 | SM/sco | M | P0 | SCO | D | A | 0 … 100000 |  | `scoring.vehScoringInfo[player].mLapsBehindNext` |
| 0x0826 | `race.time_behind_leader_s` | `TimeBehindLeader` | Time behind the leader | s | F64 | SM/sco | M | P0 | SCO | C | A | 0 … 10000000 |  | `scoring.vehScoringInfo[player].mTimeBehindLeader` |
| 0x0827 | `race.laps_behind_leader` | `LapsBehindLeader` | Laps behind the leader | count | I32 | SM/sco | M | P0 | SCO | D | A | 0 … 100000 |  | `scoring.vehScoringInfo[player].mLapsBehindLeader` |
| 0x0828 | `race.qualification` | `QualifyingPosition` | Qualifying position | count | I32 | SM/sco | M | P2 | STA | D | A | 1 … 104 |  | `scoring.vehScoringInfo[player].mQualification` |
| 0x0829 | `race.control` | `Control` | Vehicle control | enum | I8 | SM/sco | M | P1 | SCO | D | A | -1 … 3 |  | `scoring.vehScoringInfo[player].mControl` |
| 0x082A | `race.server_scored` | `ServerScored` | Server scored | — | Bool | SM/sco | M | P2 | SCO | D | A |  |  | `scoring.vehScoringInfo[player].mServerScored` |
| 0x082B | `race.count_lap_flag` | `CountLapFlag` | Lap counting | enum | U8 | SM/sco | M | P1 | SCO | D | A | 0 … 2 |  | `scoring.vehScoringInfo[player].mCountLapFlag` |
| 0x082C | `race.num_penalties` | `Penalties` | Outstanding penalties | count | I32 | SM/sco | M | P0 | SCO | D | A | 0 … 100 |  | `scoring.vehScoringInfo[player].mNumPenalties` |
| 0x082D | `race.total_distance_m` | `Distance` | Total distance | m | F64 | CALC | D | P1 | SCO | C | A | 0 … 100000000 |  | `formula:laps_times_length_plus_lap_dist` |
| 0x082E | `race.penalty_details` | `PenaltyDetails` | Penalty details | text | Str | SM/sco | M | P2 | SCO | D | N |  |  | `unavailable: only the outstanding count mNumPenalties exists` |
| 0x082F | `race.safety_car_position` | `SafetyCarPosition` | Safety car lap distance | m | F64 | SM/sco | M | P2 | SCO | C | N |  |  | `unavailable: only mGamePhase == 6 signals a safety car` |

## 0x0900 Weather & track conditions

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0900 | `weather.ambient_temp_c` | `AirTemp` | Ambient temperature | °C | F32 | SM/sco | M | P1 | ENV | C | A | -30 … 60 |  | `scoring.scoringInfo.mAmbientTemp` |
| 0x0901 | `weather.track_temp_c` | `TrackTemp` | Track temperature | °C | F32 | SM/sco | M | P1 | ENV | C | A | -30 … 80 |  | `scoring.scoringInfo.mTrackTemp` |
| 0x0902 | `weather.raining_frac` | `Rain` | Rain intensity | 0..1 | F32 | SM/sco | M | P1 | ENV | C | A | 0 … 1 |  | `scoring.scoringInfo.mRaining` |
| 0x0903 | `weather.dark_cloud_frac` | `DarkCloud` | Cloud darkness | 0..1 | F32 | SM/sco | M | P2 | ENV | C | A | 0 … 1 |  | `scoring.scoringInfo.mDarkCloud` |
| 0x0904 | `weather.cloud_coverage` | `CloudCoverage` | Cloud coverage (raw) | enum | U8 | SM/sco | M | P2 | ENV | D | A |  |  | `scoring.scoringInfo.mCloudCoverage` |
| 0x0905 | `weather.path_wetness_min_frac` | `TrackWetnessMin` | Racing line wetness min | 0..1 | F32 | SM/sco | M | P1 | ENV | C | A | 0 … 1 |  | `scoring.scoringInfo.mMinPathWetness` |
| 0x0906 | `weather.path_wetness_max_frac` | `TrackWetnessMax` | Racing line wetness max | 0..1 | F32 | SM/sco | M | P1 | ENV | C | A | 0 … 1 |  | `scoring.scoringInfo.mMaxPathWetness` |
| 0x0907 | `weather.path_wetness_avg_frac` | `TrackWetness` | Racing line wetness average | 0..1 | F32 | SM/sco | M | P1 | ENV | C | A | 0 … 1 |  | `scoring.scoringInfo.mAvgPathWetness` |
| 0x0908 | `weather.wind_x_mps` | `WindX` | Wind x (world frame, assumed) | m/s | F32 | SM/sco | M | P2 | ENV | C | A | -60 … 60 |  | `scoring.scoringInfo.mWind.x` |
| 0x0909 | `weather.wind_y_mps` | `WindY` | Wind y (world frame, assumed) | m/s | F32 | SM/sco | M | P2 | ENV | C | A | -60 … 60 |  | `scoring.scoringInfo.mWind.y` |
| 0x090A | `weather.wind_z_mps` | `WindZ` | Wind z (world frame, assumed) | m/s | F32 | SM/sco | M | P2 | ENV | C | A | -60 … 60 |  | `scoring.scoringInfo.mWind.z` |
| 0x090B | `track.grip_level` | `TrackGripLevel` | Track grip level (raw) | enum | U8 | SM/sco | M | P2 | ENV | D | A |  |  | `scoring.scoringInfo.mTrackGripLevel` |
| 0x0910 | `weather.humidity_frac` | `Humidity` | Humidity | 0..1 | F32 | REST | M | P2 | ENV | C | N |  |  | `unavailable: REST weather forecast WNV_HUMIDITY (CLAIMED)` |
| 0x0911 | `weather.air_pressure_pa` | `AirPressure` | Air pressure | Pa | F32 | REST | M | P2 | ENV | C | N |  |  | `unavailable: not exposed by LMU` |
| 0x0912 | `weather.air_density` | `AirDensity` | Air density | kg/m³ | F32 | REST | M | P2 | ENV | C | N |  |  | `unavailable: not exposed by LMU` |

## 0x0A00 Track position & map

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0A00 | `track.lap_dist_m` | `LapDistance` | Lap distance | m | F64 | SM/sco | M | P0 | SCO | D | A | -50 … 30050 |  | `scoring.vehScoringInfo[player].mLapDist` |
| 0x0A01 | `track.length_m` | `TrackLength` | Track length | m | F64 | SM/sco | M | P1 | STA | D | A | 500 … 30000 |  | `scoring.scoringInfo.mLapDist` |
| 0x0A02 | `lap.progress_frac` | `LapProgress` | Lap progress | 0..1 | F32 | CALC | E | P0 | PHY | D | A | 0 … 1 |  | `formula:lap_progress_scoring_anchored` |
| 0x0A03 | `track.sector_index` | `Sector` | Sector (0 = S1) | count | U8 | CALC | D | P0 | PHY | D | A | 0 … 2 |  | `formula:sector_from_current_sector` |
| 0x0A04 | `track.sector_index_scoring` | `SectorScoring` | Sector from scoring (0 = S1) | count | U8 | CALC | D | P2 | SCO | D | A | 0 … 2 |  | `formula:sector_from_scoring_sector` |
| 0x0A05 | `track.current_sector_raw` | `SectorRaw` | Current sector (raw, pit lane in sign bit) | — | I32 | SM/tel | M | P2 | PHY | D | A |  |  | `telemetry.telemInfo[player].mCurrentSector` |
| 0x0A06 | `track.sector_scoring_raw` | `SectorScoringRaw` | Scoring sector (raw, 0 = S3) | — | I8 | SM/sco | M | P2 | SCO | D | A | 0 … 2 |  | `scoring.vehScoringInfo[player].mSector` |
| 0x0A07 | `track.limits_steps` | `TrackLimitsSteps` | Track-limit steps | count | U8 | SM/tel | M | P1 | PHY | D | A |  |  | `telemetry.telemInfo[player].mTrackLimitsSteps` |
| 0x0A08 | `track.path_lateral_m` | `PathLateral` | Lateral offset from centre path | m | F32 | SM/sco | M | P2 | SCO | C | A | -100 … 100 |  | `scoring.vehScoringInfo[player].mPathLateral` |
| 0x0A09 | `track.edge_m` | `TrackEdge` | Track edge on the car's side | m | F32 | SM/sco | M | P2 | SCO | C | A | -100 … 100 |  | `scoring.vehScoringInfo[player].mTrackEdge` |
| 0x0A0A | `vehicle.pos_x_m` | `PosX` | World position x | m | F64 | SM/tel | M | P1 | PHY | C | A | -100000 … 100000 | 200 | `telemetry.telemInfo[player].mPos.x` |
| 0x0A0B | `vehicle.pos_y_m` | `PosY` | World position y (up) | m | F64 | SM/tel | M | P2 | PHY | C | A | -100000 … 100000 | 200 | `telemetry.telemInfo[player].mPos.y` |
| 0x0A0C | `vehicle.pos_z_m` | `PosZ` | World position z | m | F64 | SM/tel | M | P1 | PHY | C | A | -100000 … 100000 | 200 | `telemetry.telemInfo[player].mPos.z` |
| 0x0A0D | `vehicle.heading_rad` | `Heading` | Heading (A-LMU-025) | rad | F32 | CALC | D | P1 | PHY | D | A | -3.141593 … 3.141593 |  | `formula:heading_from_orientation` |
| 0x0A0E | `track.map_x_m` | `MapX` | Map x (screen right, A-LMU-025) | m | F64 | CALC | D | P0 | PHY | C | A | -100000 … 100000 | 200 | `formula:map_xy_from_world_pos` |
| 0x0A0F | `track.map_y_m` | `MapY` | Map y (screen up, A-LMU-025) | m | F64 | CALC | D | P0 | PHY | C | A | -100000 … 100000 | 200 | `formula:map_xy_from_world_pos` |
| 0x0A10 | `vehicle.gps_lat_rad` | `GPSLatitude` | GPS latitude | rad | F64 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only` |
| 0x0A11 | `vehicle.gps_lon_rad` | `GPSLongitude` | GPS longitude | rad | F64 | DuckDB | M | P2 | PHY | C | N |  |  | `unavailable: DuckDB recordings only` |

## 0x0B00 Flags & pit

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0B00 | `flags.yellow_flag_state` | `YellowFlagState` | Full-course yellow state | enum | I8 | SM/sco | M | P0 | SCO | D | A | -1 … 7 |  | `scoring.scoringInfo.mYellowFlagState` |
| 0x0B01 | `flags.sector_flag_0` | `SectorFlag0` | Sector flag [0] (raw, order UNVERIFIED) | enum | I8 | SM/sco | M | P1 | SCO | D | A |  |  | `scoring.scoringInfo.mSectorFlag[0]` |
| 0x0B02 | `flags.sector_flag_1` | `SectorFlag1` | Sector flag [1] (raw, order UNVERIFIED) | enum | I8 | SM/sco | M | P1 | SCO | D | A |  |  | `scoring.scoringInfo.mSectorFlag[1]` |
| 0x0B03 | `flags.sector_flag_2` | `SectorFlag2` | Sector flag [2] (raw, order UNVERIFIED) | enum | I8 | SM/sco | M | P1 | SCO | D | A |  |  | `scoring.scoringInfo.mSectorFlag[2]` |
| 0x0B04 | `flags.car_flag` | `CarFlag` | Car flag | enum | U8 | SM/sco | M | P0 | SCO | D | A |  |  | `scoring.vehScoringInfo[player].mFlag` |
| 0x0B05 | `flags.under_yellow` | `UnderYellow` | Under full-course yellow | — | Bool | SM/sco | M | P0 | SCO | D | A |  |  | `scoring.vehScoringInfo[player].mUnderYellow` |
| 0x0B06 | `flags.individual_phase` | `IndividualPhase` | Individual phase | enum | U8 | SM/sco | M | P1 | SCO | D | A | 0 … 11 |  | `scoring.vehScoringInfo[player].mIndividualPhase` |
| 0x0B07 | `flags.full_course_yellow` | `FullCourseYellow` | Full-course yellow / safety car | — | Bool | CALC | D | P0 | SCO | D | A |  |  | `formula:fcy_from_phase_and_yellow_state` |
| 0x0B08 | `flags.blue` | `BlueFlag` | Blue flag | — | Bool | CALC | D | P0 | SCO | D | A |  |  | `formula:blue_from_car_flag` |
| 0x0B09 | `flags.local_yellow_s1` | `LocalYellowS1` | Local yellow sector 1 (experimental) | — | Bool | CALC | D | P0 | SCO | D | X |  |  | `formula:local_yellow_from_sector_flags` |
| 0x0B0A | `flags.local_yellow_s2` | `LocalYellowS2` | Local yellow sector 2 (experimental) | — | Bool | CALC | D | P0 | SCO | D | X |  |  | `formula:local_yellow_from_sector_flags` |
| 0x0B0B | `flags.local_yellow_s3` | `LocalYellowS3` | Local yellow sector 3 (experimental) | — | Bool | CALC | D | P0 | SCO | D | X |  |  | `formula:local_yellow_from_sector_flags` |
| 0x0B20 | `pit.in_pits` | `InPits` | In pits | — | Bool | SM/sco | M | P0 | SCO | D | A |  |  | `scoring.vehScoringInfo[player].mInPits` |
| 0x0B21 | `pit.state` | `PitState` | Pit state | enum | U8 | SM/sco | M | P0 | SCO | D | A | 0 … 4 |  | `scoring.vehScoringInfo[player].mPitState` |
| 0x0B22 | `pit.in_garage_stall` | `InGarage` | In garage stall | — | Bool | SM/sco | M | P1 | SCO | D | A |  |  | `scoring.vehScoringInfo[player].mInGarageStall` |
| 0x0B23 | `pit.lap_dist_m` | `PitLapDistance` | Pit box lap distance | m | F32 | SM/sco | M | P2 | STA | D | A | 0 … 30000 |  | `scoring.vehScoringInfo[player].mPitLapDist` |
| 0x0B24 | `pit.num_stops` | `PitStops` | Pit stops | count | I32 | SM/sco | M | P0 | SCO | D | A | 0 … 100 |  | `scoring.vehScoringInfo[player].mNumPitstops` |
| 0x0B25 | `pit.in_pit_lane` | `InPitLane` | In pit lane | — | Bool | CALC | D | P0 | PHY | D | A |  |  | `formula:pit_lane_from_current_sector_sign_bit` |
| 0x0B26 | `pit.scheduled_stops` | `PitScheduledStops` | Scheduled pit stops | count | U8 | SM/tel | M | P2 | STA | D | A |  |  | `telemetry.telemInfo[player].mScheduledStops` |
| 0x0B30 | `pit.stop_time_estimate_s` | `PitStopTimeEstimate` | Pit stop time estimate | s | F32 | REST | M | P2 | ENV | C | N |  |  | `unavailable: REST pit menu (CLAIMED)` |

## 0x0C00 Derived fuel / stint / energy (engine outputs)

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0C00 | `fuel.used_last_lap_l` | `FuelUsedLastLap` | Fuel used last lap | L | F32 | CALC | D | P1 | EVT | D | A | 0 … 50 |  | `formula:fuel_used_last_lap` |
| 0x0C01 | `fuel.used_per_lap_l` | `FuelPerLap` | Fuel per lap (window average) | L/lap | F32 | CALC | E | P0 | EVT | D | A | 0 … 50 |  | `formula:fuel_per_lap_window_average` |
| 0x0C02 | `fuel.used_this_lap_l` | `FuelUsedThisLap` | Fuel used this lap | L | F32 | CALC | D | P1 | PHY | D | A | 0 … 50 |  | `formula:fuel_used_since_lap_start` |
| 0x0C03 | `fuel.laps_remaining` | `FuelLapsRemaining` | Laps of fuel remaining | lap | F32 | CALC | E | P0 | PHY | C | A | 0 … 100000 |  | `formula:fuel_laps_remaining` |
| 0x0C04 | `fuel.time_remaining_s` | `FuelTimeRemaining` | Time of fuel remaining | s | F32 | CALC | E | P1 | PHY | C | A | 0 … 10000000 |  | `formula:fuel_time_remaining` |
| 0x0C05 | `fuel.to_finish_l` | `FuelToFinish` | Fuel needed to finish | L | F32 | CALC | E | P0 | PHY | C | A | 0 … 20000 |  | `formula:fuel_to_finish` |
| 0x0C06 | `fuel.to_add_l` | `FuelToAdd` | Fuel to add at the next stop | L | F32 | CALC | E | P0 | PHY | C | A | 0 … 200 |  | `formula:fuel_to_add_next_stop` |
| 0x0C07 | `fuel.margin_laps` | `FuelMarginLaps` | Fuel margin (+ spare, - short) | lap | F32 | CALC | E | P1 | PHY | C | A | -100000 … 100000 |  | `formula:fuel_margin_laps` |
| 0x0C10 | `energy.ve_used_last_lap_frac` | `VEUsedLastLap` | Virtual energy used last lap | 0..1 | F32 | CALC | D | P1 | EVT | D | V | 0 … 0.5 |  | `formula:ve_used_last_lap` |
| 0x0C11 | `energy.ve_used_per_lap_frac` | `VEPerLap` | Virtual energy per lap (window average) | /lap | F32 | CALC | E | P0 | EVT | D | V | 0 … 0.5 |  | `formula:ve_per_lap_window_average` |
| 0x0C12 | `energy.ve_used_this_lap_frac` | `VEUsedThisLap` | Virtual energy used this lap | 0..1 | F32 | CALC | D | P1 | PHY | D | V | 0 … 1 |  | `formula:ve_used_since_lap_start` |
| 0x0C13 | `energy.ve_laps_remaining` | `VELapsRemaining` | Laps of virtual energy remaining | lap | F32 | CALC | E | P0 | PHY | C | V | 0 … 100000 |  | `formula:ve_laps_remaining` |
| 0x0C20 | `stint.number` | `StintNumber` | Stint number | count | U32 | CALC | D | P1 | EVT | D | A | 1 … 1000 |  | `formula:stint_counter_pit_exits` |
| 0x0C21 | `stint.laps` | `StintLaps` | Laps in this stint | lap | U32 | CALC | D | P1 | EVT | D | A | 0 … 100000 |  | `formula:laps_since_pit_exit` |
| 0x0C22 | `stint.time_s` | `StintTime` | Time in this stint | s | F64 | CALC | D | P1 | PHY | D | A | 0 … 10000000 |  | `formula:time_since_pit_exit` |
| 0x0C23 | `stint.max_laps_estimate` | `StintMaxLaps` | Stint length limit (fuel or energy) | lap | F32 | CALC | E | P1 | PHY | C | A | 0 … 100000 |  | `formula:stint_max_laps_min_fuel_ve` |
| 0x0C24 | `stint.tyre_set_laps` | `TyreSetLaps` | Laps on the current tyre set | lap | U32 | CALC | D | P1 | EVT | D | A | 0 … 100000 |  | `formula:laps_since_tyre_change` |
| 0x0C25 | `race.laps_remaining_estimate` | `RaceLapsRemaining` | Race laps remaining | lap | F32 | CALC | E | P0 | PHY | C | A | 0 … 100000 |  | `formula:race_laps_remaining` |

## 0x0D00 Application, network & recording diagnostics

| ID | Canonical name | Export | Label | Unit | Type | Source | Prov | Prio | Freq | Int | Avail | Range | Max rate/s | Origin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0x0D00 | `lmu.game_version` | `GameVersion` | LMU game version | count | I32 | SM/gen | M | P1 | STA | D | A | 1 … 2147483647 |  | `generic.gameVersion` |
| 0x0D01 | `app.options_location` | `OptionsLocation` | LMU UI location | enum | U8 | SM/gen | M | P1 | EVT | D | A | 0 … 3 |  | `generic.appInfo.mOptionsLocation` |
| 0x0D02 | `app.options_page` | `OptionsPage` | LMU options page | text | Str | SM/gen | M | P2 | EVT | D | A |  |  | `generic.appInfo.mOptionsPage` |
| 0x0D03 | `source.telemetry_rate_hz` | `SrcTelemetryRate` | LMU telemetry rate (measured) | Hz | F32 | APP | D | P1 | DIA | C | A | 0 … 1000 |  | `app:distinct_elapsed_time_per_second` |
| 0x0D04 | `source.scoring_rate_hz` | `SrcScoringRate` | LMU scoring rate (measured) | Hz | F32 | APP | D | P1 | DIA | C | A | 0 … 100 |  | `app:distinct_current_et_per_second` |
| 0x0D05 | `source.telemetry_age_s` | `SrcTelemetryAge` | Age of the newest LMU telemetry | s | F32 | APP | D | P1 | DIA | C | A | ≥ 0 |  | `app:telemetry_age` |
| 0x0D06 | `source.scoring_age_s` | `SrcScoringAge` | Age of the newest LMU scoring | s | F32 | APP | D | P1 | DIA | C | A | ≥ 0 |  | `app:scoring_age` |
| 0x0D07 | `source.et_regressions` | `SrcETRegressions` | Elapsed-time regressions | count | U32 | APP | D | P2 | DIA | D | A |  |  | `app:et_regression_count` |
| 0x0D08 | `source.duplicate_frames` | `SrcDuplicateFrames` | Duplicate LMU frames | count | U64 | APP | D | P2 | DIA | D | A |  |  | `app:duplicate_frame_count` |
| 0x0D09 | `source.invalid_samples` | `SrcInvalidSamples` | Samples rejected by validation | count | U64 | APP | D | P2 | DIA | D | A |  |  | `app:invalid_sample_count` |
| 0x0D0A | `source.read_duration_s` | `SrcReadDuration` | Shared-memory copy duration | s | F32 | APP | D | P2 | DIA | C | A | ≥ 0 |  | `app:snapshot_copy_duration` |
| 0x0D10 | `app.pipeline_latency_s` | `AppPipelineLatency` | Capture-to-publish latency | s | F32 | APP | D | P2 | DIA | C | A | ≥ 0 |  | `app:capture_to_publish` |
| 0x0D11 | `app.calculation_engine_version` | `CalcEngineVersion` | Calculation engine version (race-engines formulas) | count | U32 | APP | D | P1 | STA | D | A |  |  | `app:race_engines_version` |
| 0x0D12 | `app.normalizer_formula_version` | `NormalizerFormulaVersion` | Formula version (lmu-normalizer formulas) | count | U32 | APP | D | P1 | STA | D | A |  |  | `app:lmu_normalizer_formula_version` |
| 0x0D20 | `net.connection_state` | `NetState` | Connection state | enum | U8 | APP | D | P1 | EVT | D | A |  |  | `app:transport_connection_state` |
| 0x0D21 | `net.stream_age_s` | `NetStreamAge` | Stream age (relay clock) | s | F32 | APP | E | P2 | DIA | C | A |  |  | `app:stream_age_relay_clock` |
| 0x0D22 | `net.latency_s` | `NetLatency` | One-way latency (estimated) | s | F32 | APP | E | P2 | DIA | C | A |  |  | `app:one_way_latency` |
| 0x0D23 | `net.rtt_s` | `NetRTT` | Round-trip time | s | F32 | APP | D | P2 | DIA | C | A | ≥ 0 |  | `app:ping_rtt` |
| 0x0D24 | `net.jitter_s` | `NetJitter` | Inter-arrival jitter | s | F32 | APP | D | P2 | DIA | C | A | ≥ 0 |  | `app:interarrival_jitter` |
| 0x0D25 | `net.packet_loss_frac` | `NetPacketLoss` | Packet loss | 0..1 | F32 | APP | D | P2 | DIA | C | A | 0 … 1 |  | `app:sequence_gap_fraction` |
| 0x0D26 | `net.frames_lost` | `NetFramesLost` | Frames lost | count | U64 | APP | D | P2 | DIA | D | A |  |  | `app:sequence_gaps` |
| 0x0D27 | `net.frames_out_of_order` | `NetFramesOutOfOrder` | Frames out of order | count | U64 | APP | D | P2 | DIA | D | A |  |  | `app:sequence_out_of_order` |
| 0x0D28 | `net.frames_duplicated` | `NetFramesDuplicated` | Frames duplicated | count | U64 | APP | D | P2 | DIA | D | A |  |  | `app:sequence_duplicates` |
| 0x0D29 | `net.relay_dropped` | `NetRelayDropped` | Frames dropped by the relay | count | U64 | APP | D | P2 | DIA | D | A |  |  | `app:relay_dropped_counter` |
| 0x0D2A | `net.tx_rate_bps` | `NetTxRate` | Transmit rate | bit/s | F32 | APP | D | P2 | DIA | C | A | ≥ 0 |  | `app:tx_bits_per_second` |
| 0x0D2B | `net.rx_rate_bps` | `NetRxRate` | Receive rate | bit/s | F32 | APP | D | P2 | DIA | C | A | ≥ 0 |  | `app:rx_bits_per_second` |
| 0x0D2C | `net.telemetry_frame_rate_hz` | `NetFrameRate` | Telemetry frame rate on the link | Hz | F32 | APP | D | P2 | DIA | C | A | ≥ 0 |  | `app:telemetry_frames_per_second` |
| 0x0D2D | `net.send_queue_depth` | `NetQueueDepth` | Send queue depth | count | U32 | APP | D | P2 | DIA | D | A |  |  | `app:send_queue_len` |
| 0x0D2E | `net.clock_offset_s` | `NetClockOffset` | Clock offset to the relay (estimated) | s | F64 | APP | E | P2 | DIA | C | A |  |  | `app:ntp_style_offset` |
| 0x0D2F | `net.reconnects` | `NetReconnects` | Reconnects | count | U32 | APP | D | P2 | DIA | D | A |  |  | `app:reconnect_count` |
| 0x0D40 | `rec.state` | `RecState` | Recording state | enum | U8 | APP | D | P2 | EVT | D | A |  |  | `app:recorder_state` |
| 0x0D41 | `rec.bytes_written` | `RecBytes` | Recording size | B | U64 | APP | D | P2 | DIA | D | A |  |  | `app:recorder_bytes_written` |
| 0x0D42 | `rec.write_rate_bps` | `RecWriteRate` | Recording write rate | bit/s | F32 | APP | D | P2 | DIA | C | A | ≥ 0 |  | `app:recorder_write_rate` |
| 0x0D43 | `rec.queue_depth` | `RecQueueDepth` | Recording queue depth | count | U32 | APP | D | P2 | DIA | D | A |  |  | `app:recorder_queue_len` |
| 0x0D44 | `rec.frames_dropped` | `RecFramesDropped` | Frames dropped by the recorder | count | U64 | APP | D | P2 | DIA | D | A |  |  | `app:recorder_dropped` |
| 0x0D45 | `rec.disk_free_bytes` | `RecDiskFree` | Free disk space | B | U64 | APP | D | P2 | DIA | D | A |  |  | `app:recorder_disk_free` |
| 0x0D46 | `rec.duration_s` | `RecDuration` | Recording duration | s | F64 | APP | D | P2 | DIA | C | A | ≥ 0 |  | `app:recorder_duration` |
