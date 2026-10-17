# LMU Channel Map

**Date of verification: 2026-09-30.** Interface: LMU `LMU_Data` shared memory, headers fingerprinted in
[`research/lmu_headers_fingerprint.json`](research/lmu_headers_fingerprint.json). Game: 1.4.2.0, Steam build 25513257.
Research context is in [LMU_TELEMETRY_RESEARCH.md](LMU_TELEMETRY_RESEARCH.md); assumptions are in [ASSUMPTIONS.md](ASSUMPTIONS.md).

This table is the specification for `crates/lmu-normalizer` and for the channel registry in `telemetry-model::registry`.
Rules:
- Every LMU field name below exists in [`research/lmu_abi_layout.json`](research/lmu_abi_layout.json). A script checked this mechanically on 2026-09-30.
- No header text is reproduced. Raw-unit entries paraphrase header comments; "—" means the header states no unit.
- LMU was **not running**. Every live value, rate and sign is **UNVERIFIED** until `tools/lmu-diag` measures it.

## 0. Conventions

### 0.1 Canonical names

- Names are dotted `snake_case`: `<group>.<subgroup?>.<quantity>_<unit suffix>`.
  - The unit suffix is added for every dimensional quantity: `_m`, `_mps`, `_mps2`, `_pa`, `_c` (°C), `_rad`, `_radps`, `_radps2`, `_l`, `_s`, `_n`, `_nm`, `_w`, `_rpm`, `_g`, `_frac` (0..1).
  - Enums, booleans, counts and text have no suffix.
  - This convention is a proposal for the registry. The registry's `unit` field stays authoritative, and the `vehicle.speed` example in `channel.rs` becomes `vehicle.speed_mps`.
- Per-wheel names use `{w}` ∈ `fl`, `fr`, `rl`, `rr`. These map to `mWheel[0]`, `[1]`, `[2]`, `[3]`, following the header comment "front left, front right, rear left, rear right" (VERIFIED header). The DuckDB `value1..4` order FL, FR, RL, RR matches (VERIFIED on recordings).
- Per-car scoring records use `cars.<id>.<field>`, where `<id>` = that row's `mID`. These are rows in a standings table, not individual registry channels.

### 0.2 Internal units

Per `telemetry-model::units`:

| Quantity | Unit |
|---|---|
| Length | m |
| Speed | m/s |
| Acceleration | m/s² (derived G in g) |
| Pressure | **Pa** |
| Temperature | **°C** (LMU mixes K and °C; °C is the canonical internal unit) |
| Angle | **rad** |
| Fuel | **L** |
| Time | **s** |
| Force | N |
| Torque | N·m |
| Power | **W** |
| Engine speed | rpm |
| Ratios | fraction 0..1 |

Display units are converted only in `apps/desktop/src/lib/units.ts`.

### 0.3 Indices

| Symbol | Meaning | Validity rule |
|---|---|---|
| `p` | `telemetry.playerVehicleIdx` | Valid only if `telemetry.playerHasVehicle` and `p < min(telemetry.activeVehicles, 104)` |
| `k` | Player's scoring row: the `k` with `scoring.vehScoringInfo[k].mIsPlayer` | Must satisfy `k < min(scoring.scoringInfo.mNumVehicles, 104)` and `vehScoringInfo[k].mID == telemInfo[p].mID`. Otherwise every player scoring channel is UNAVAILABLE for that snapshot |
| `i` | Any scoring row | `i < min(mNumVehicles, 104)` |
| `w` | Wheel | 0..3 |

### 0.4 Column codes

- **Source:** `SM/tel` = `data.telemetry`; `SM/sco` = `data.scoring`; `SM/gen` = `data.generic`; `SM/paths` = `data.paths`; `CALC` = computed by `race-engines` / `lmu-normalizer` (Provenance DERIVED or ESTIMATED).
- **Frequency** (all **UNVERIFIED until measured**):
  - `TEL` = every telemetry update, nominally about 100 Hz (Settings "Plugin Mask" description, CLAIMED);
  - `SCO` = every scoring update, nominally about 5 Hz (header plugin comment, CLAIMED for `LMU_Data`);
  - `STATIC` = changes only on session or vehicle change;
  - `SLOW` = scoring-rate but slowly varying (weather).
- **Availability:**
  - `AVAILABLE` = present and meaningful for every car;
  - `VEHICLE-DEPENDENT` = meaningful only for some classes or cars (zero or garbage otherwise);
  - `UNAVAILABLE` = not provided by `LMU_Data`.
  - `UNIT?` in Notes = raw unit not stated by the header; the internal unit rests on an assumption.
- **Validation:**
  - **V0** applies to every row: floats must be finite (NaN or ±Inf → `INVALID`), and strings are decoded as bounded, NUL-terminated, lossy UTF-8.
  - Out-of-range values → `INVALID`; they are never clamped silently.
  - A "sentinel" value maps to `NONE`, not to `INVALID`.
- **Evidence tags in Notes:** [H] header comment (VERIFIED read); [D] DuckDB recordings (VERIFIED, different source); [C] community claim; [I] inference; [A-LMU-nnn] entry in ASSUMPTIONS.md.

### 0.5 Key offsets

Byte offsets from the start of the `LMU_Data` view (MSVC probe, VERIFIED compile; UNVERIFIED against a live mapping).

| Field | Offset in struct | Absolute (slot 0) | Absolute formula |
|---|---:|---:|---|
| `generic.events[16]` (u32) | 0 | 0 | — |
| `generic.gameVersion` (i32) | 64 | 64 | — |
| `scoring.scoringInfo.mCurrentET` (f64) | 68 | 1,700 | — |
| `scoring.scoringInfo.mLapDist` (f64, track length) | 88 | 1,720 | — |
| `scoring.scoringInfo.mNumVehicles` (i32) | 104 | 1,736 | — |
| `scoring.scoringInfo.mGamePhase` (u8) | 108 | 1,740 | — |
| `scoring.scoringStreamSize` (u64) | — | 2,184 | 4 bytes of padding at 2,180 |
| `scoring.vehScoringInfo[i]` | — | 2,192 | 2,192 + 584·i |
| `…vehScoringInfo[i].mLapDist` / `.mIsPlayer` / `.mLastLapTime` | 104 / 196 / 168 | 2,296 / 2,388 / 2,360 | 2,192 + 584·i + off |
| `telemetry.activeVehicles` / `playerVehicleIdx` / `playerHasVehicle` | 0 / 1 / 2 | 128,464 / 128,465 / 128,466 | — |
| `telemetry.telemInfo[p]` | — | 128,468 | 128,468 + 1,888·p |
| `…telemInfo[p].mElapsedTime` / `.mLocalVel` / `.mFuel` / `.mCurrentSector` | 12 / 184 / 524 / 600 | 128,480 / 128,652 / 128,992 / 129,068 | 128,468 + 1,888·p + off |
| `…telemInfo[p].mVirtualEnergy` / `.mSoC` / `.mBatteryChargeFraction` | 776 / 772 / 704 | 129,244 / 129,240 / 129,172 | same |
| `…telemInfo[p].mWheel[w]` | 848 | 129,316 | 128,468 + 1,888·p + 848 + 260·w |
| `…mWheel[w].mPressure` / `.mTemperature[3]` / `.mBrakeTemp` / `.mBrakePressure` | 120 / 128 / 24 / 32 | 129,436 / 129,444 / 129,340 / 129,348 | … + 260·w + off |

---

## 1. Session, application and paths

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `lmu.game_version` | `generic.gameVersion` | SM/gen | long (i32) | — | count (raw) | text | STATIC | AVAILABLE | none | > 0 | Probably 14200 for 1.4.2 (version × 10000) [I] [A-LMU-010]. Used for version-gating fields |
| `lmu.event_queue` | `generic.events[0..16]` | SM/gen | enum SharedMemoryEvent[16] (u32) | SME code | enum[] | — | TEL | AVAILABLE | none | each value 0..15 | Queue of SME codes, **not flags**. 0 = `SME_ENTER` and is indistinguishable from empty [H-code] [A-LMU-009]. Diagnostic only |
| `inputs.ffb_torque` | `generic.FFBTorque` | SM/gen | float | — | none (raw) | % | TEL | AVAILABLE | none | finite | UNIT?. Scale unknown (DuckDB "FFB Output" is ±100 %) [D]. Low priority |
| `app.options_location` | `generic.appInfo.mOptionsLocation` | SM/gen | unsigned char | enum | enum | text | EVT | AVAILABLE | none | 0..3 | 0 main UI, 1 track loading, 2 monitor, 3 on track [H]. Drives the PAUSED state |
| `app.options_page` | `generic.appInfo.mOptionsPage` | SM/gen | char[31] | text | text | text | EVT | AVAILABLE | bounded decode | printable | Diagnostic only |
| `session.user_data_path` | `paths.userData` | SM/paths | char[260] | path | text | — | STATIC | AVAILABLE | bounded decode | non-empty | Locates `UserData\Telemetry` for DuckDB import without touching the process. Relative or absolute is UNVERIFIED. **PII** (Windows user path): never transmitted |
| `session.track_name` | `scoring.scoringInfo.mTrackName` | SM/sco | char[64] | text | text | text | STATIC | AVAILABLE | bounded decode | non-empty in session | Telemetry also has `mTrackName` (same meaning) |
| `session.type` | `scoring.scoringInfo.mSession` | SM/sco | long (i32) | enum | enum | text | STATIC | AVAILABLE | none | 0..13 | 0 test day, 1–4 practice, 5–8 qualifying, 9 warm-up, 10–13 race [H] |
| `session.elapsed_s` | `scoring.scoringInfo.mCurrentET` | SM/sco | double | s (session time) | s | h:mm:ss | SCO | AVAILABLE | none | ≥ 0; non-decreasing within a session | Freshness key for scoring snapshots |
| `session.end_s` | `scoring.scoringInfo.mEndET` | SM/sco | double | s | s | h:mm:ss | STATIC | AVAILABLE | none | ≥ 0; `INT32_MIN` = not set → NONE | Gate 1 race: −2,147,483,648 (`INT32_MIN`) during the formation lap of a timed race, published as no data (A-LMU-043); transitional real values before the green flag |
| `session.time_remaining_s` | `scoring.scoringInfo.mSessionTimeRemaining` | SM/sco | float | — | s | h:mm:ss | SCO | AVAILABLE | none | ≥ 0 | UNIT? Assumed seconds [A-LMU-035]. Cross-check with `mEndET − mCurrentET` |
| `session.max_laps` | `scoring.scoringInfo.mMaxLaps` | SM/sco | long (i32) | laps | count | laps | STATIC | AVAILABLE | 2147483647 → NONE | ≥ 0 | INT32_MAX in timed races [C] |
| `session.num_vehicles` | `scoring.scoringInfo.mNumVehicles` | SM/sco | long (i32) | count | count | — | SCO | AVAILABLE | clamp view to 104 | 0..104 (> 104 → `INCOMPATIBLE` warning) | Bounds the scoring array [A-LMU-021] |
| `session.active_vehicles_telemetry` | `telemetry.activeVehicles` | SM/tel | unsigned char | count | count | — | TEL | AVAILABLE | clamp view to 104 | 0..104 | Bounds the telemetry array. Compare with `mNumVehicles` [C] |
| `session.player_has_vehicle` | `telemetry.playerHasVehicle` | SM/tel | bool | bool | bool | — | TEL | AVAILABLE | none | 0/1 | Gate for all `p` channels |
| `session.player_slot_index` | `telemetry.playerVehicleIdx` | SM/tel | unsigned char | index | index | — | TEL | AVAILABLE | none | < activeVehicles | Array index, not `mID` [A-LMU-020] |
| `session.game_phase` | `scoring.scoringInfo.mGamePhase` | SM/sco | unsigned char | enum | enum | text | SCO | AVAILABLE | none | 0..9 | 0 before session, 1 recon, 2 grid walk, 3 formation, 4 countdown, 5 green, 6 FCY / safety car, 7 stopped, 8 over, 9 paused [H] |
| `session.in_realtime` | `scoring.scoringInfo.mInRealtime` | SM/sco | bool | bool | bool | — | SCO | AVAILABLE | none | 0/1 | false = monitor or menus. Drives PAUSED |
| `session.start_light` | `scoring.scoringInfo.mStartLight` | SM/sco | unsigned char | frame # | count | — | SCO | AVAILABLE | none | 0..255 | Frame number depends on the track [H]. Relation to `mNumRedLights` UNVERIFIED |
| `session.num_red_lights` | `scoring.scoringInfo.mNumRedLights` | SM/sco | unsigned char | count | count | — | STATIC | AVAILABLE | none | 0..20 | |
| `session.start_time_of_day_s` | `scoring.scoringInfo.mStartET` | SM/sco | float | s since midnight | s | hh:mm | STATIC | AVAILABLE | none | 0..86400 | Header: seconds since midnight [H] |
| `session.time_of_day` | `scoring.scoringInfo.mTimeOfDay` | SM/sco | float | — | s (assumed) | hh:mm | SLOW | AVAILABLE | withheld (UNAVAILABLE) until a value > 24 is seen, then seconds since midnight (A-LMU-036); never guessed as hours | finite | UNIT? [A-LMU-036] |
| `session.fixed_setup` | `scoring.scoringInfo.mIsFixedSetup` | SM/sco | bool | bool | bool | — | STATIC | AVAILABLE | none | 0/1 | |
| `session.game_mode` | `scoring.scoringInfo.mGameMode` | SM/sco | unsigned char | enum | enum | text | STATIC | AVAILABLE | none | 0..3 | 1 server, 2 client, 3 server + client [H]; 0 presumably offline [I] |
| `session.max_players` | `scoring.scoringInfo.mMaxPlayers` | SM/sco | long (i32) | count | count | — | STATIC | AVAILABLE | none | 0..104 | |
| `session.server_name` | `scoring.scoringInfo.mServerName` | SM/sco | char[32] | text | text | — | STATIC | AVAILABLE | bounded decode | — | **Not transmitted by default** (privacy). Same for `mServerPublicIP`, `mServerPort`, `mIsPasswordProtected` |
| `rules.track_limits_steps_per_penalty` | `scoring.scoringInfo.mTrackLimitsStepsPerPenalty` | SM/sco | unsigned char | count | count | — | STATIC | AVAILABLE | none | ≥ 0 | Meaning follows the header naming; scale UNVERIFIED |
| `rules.track_limits_steps_per_point` | `scoring.scoringInfo.mTrackLimitsStepsPerPoint` | SM/sco | unsigned char | count | count | — | STATIC | AVAILABLE | none | ≥ 0 | |
| `track.length_m` | `scoring.scoringInfo.mLapDist` | SM/sco | double | m (distance around track) | m | km | STATIC | AVAILABLE | none | 500..30000 | Denominator for lap progress |
| `track.grip_level` | `scoring.scoringInfo.mTrackGripLevel` | SM/sco | unsigned char | — | enum (raw) | — | SLOW | AVAILABLE | none | 0..255 | UNIT? Scale / enum UNVERIFIED |

## 2. Vehicle identity (player; same fields exist for every telemetry slot)

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `vehicle.slot_id` | `telemetry.telemInfo[p].mID` | SM/tel | long (i32) | id | id | — | STATIC | AVAILABLE | none | ≥ 0 | Join key with scoring `mID`. Reused in multiplayer [H] |
| `vehicle.name` | `telemetry.telemInfo[p].mVehicleName` | SM/tel | char[64] | text | text | text | STATIC | AVAILABLE | bounded decode | non-empty | |
| `vehicle.model` | `telemetry.telemInfo[p].mVehicleModel` | SM/tel | char[30] | text | text | text | STATIC | AVAILABLE | bounded decode | — | Added in 1.3 [C, confirmed in the installed header] |
| `vehicle.class` | `telemetry.telemInfo[p].mVehicleClass` | SM/tel | enum IP_VehicleClass (u8) | enum | enum | text | STATIC | AVAILABLE | none | {0,2,3,4,5,6,8,255}; other → Unknown | Hypercar 0, LMP2_ELMS 2, LMP2 3, LMP3 4, GTE 5, GT3 6, PaceCar 8, Unknown 255 [H]. **Different type from scoring `mVehicleClass` (char[32])** |
| `vehicle.championship` | `telemetry.telemInfo[p].mVehicleChampionship` | SM/tel | enum IP_VehicleChampionship (u8) | enum | enum | text | STATIC | AVAILABLE | none | {0..3,16,17,255} | WEC_2023..2026 = 0..3, ELMS_2025 16, ELMS_2026 17 [H] |
| `vehicle.class_name` | `scoring.vehScoringInfo[k].mVehicleClass` | SM/sco | char[32] | text | text | text | STATIC | AVAILABLE | bounded decode | — | Display string (e.g. "Hyper", "GT3" [C]) |
| `vehicle.veh_filename` | `scoring.vehScoringInfo[k].mVehFilename` | SM/sco | char[32] | text | text | — | STATIC | AVAILABLE | bounded decode | — | Stable vehicle identifier |
| `vehicle.max_gears` | `telemetry.telemInfo[p].mMaxGears` | SM/tel | unsigned char | count | count | — | STATIC | AVAILABLE | none | 1..10 | |
| `vehicle.engine_max_rpm` | `telemetry.telemInfo[p].mEngineMaxRPM` | SM/tel | double | rpm (rev limit) | rpm | rpm | STATIC | AVAILABLE | none | 1000..25000 | |
| `fuel.capacity_l` | `telemetry.telemInfo[p].mFuelCapacity` | SM/tel | double | L | L | L | STATIC | AVAILABLE | none | 0..200 | |

## 3. Timing (player)

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `time.source_et_s` | `telemetry.telemInfo[p].mElapsedTime` | SM/tel | double | s (game session time) | s | h:mm:ss.000 | TEL | AVAILABLE | none | ≥ 0; non-decreasing within a session (a reset means a new session) | Frame timestamp (`source_et`, ARCHITECTURE §8). Freshness key [A-LMU-031] |
| `time.delta_s` | `telemetry.telemInfo[p].mDeltaTime` | SM/tel | double | s (since last update) | s | ms | TEL | AVAILABLE | none | (0, 0.5] | Its distribution gives the physics update rate |
| `lap.number` | `telemetry.telemInfo[p].mLapNumber` | SM/tel | long (i32) | count | count | — | TEL | AVAILABLE | none | ≥ 0 | Compare with scoring `mTotalLaps` (completed laps) |
| `lap.start_et_s` | `telemetry.telemInfo[p].mLapStartET` | SM/tel | double | s | s | — | TEL | AVAILABLE | < 0 → NONE | −1 or 0..`mElapsedTime` | −1 before the first lap [C] |
| `lap.invalidated` | `telemetry.telemInfo[p].mLapInvalidated` | SM/tel | bool | bool | bool | — | TEL | AVAILABLE | none | 0/1 | 1.3+ |
| `timing.delta_best_s` | `telemetry.telemInfo[p].mDeltaBest` | SM/tel | double | — | s | ±s.000 | TEL | AVAILABLE | none | abs ≤ 600 | UNIT? Sign convention (negative = faster?) UNVERIFIED [A-LMU-033] |
| `timing.gap_car_ahead_s` | `telemetry.telemInfo[p].mTimeGapCarAhead` | SM/tel | float | — | s | s.000 | TEL | AVAILABLE | none | 0..3600 (or NONE) | LMU signs it **negative** (the car ahead is −x s); published as the magnitude −raw. Wrong sign ≤ 0.05 s = alongside → 0; larger → INVALID. NONE alone in the session (Gate 1, A-LMU-034, A-LMU-066) |
| `timing.gap_car_behind_s` | `telemetry.telemInfo[p].mTimeGapCarBehind` | SM/tel | float | — | s | s.000 | TEL | AVAILABLE | none | 0..3600 (or NONE) | Positive (raw as-is); wrong sign ≤ 0.05 s → 0, larger → INVALID; NONE alone in the session (A-LMU-034, A-LMU-066) |
| `timing.gap_place_ahead_s` | `telemetry.telemInfo[p].mTimeGapPlaceAhead` | SM/tel | float | — | s | s.000 | TEL | AVAILABLE | none | 0..3600 (or NONE) | LMU signs it **negative** (= −`mTimeBehindNext`, Gate 1 race); published −raw. NONE for P1 or alone. Pre-green and during overtakes (the car classified ahead is briefly behind on track) the sign is wrong → INVALID (A-LMU-034). **NONE outside race sessions**: in practice / qualifying LMU fills it with the signed road gap to the car classified (by best lap) one place ahead, up to +124 s (Gate 1 stint1) |
| `timing.gap_place_behind_s` | `telemetry.telemInfo[p].mTimeGapPlaceBehind` | SM/tel | float | — | s | s.000 | TEL | AVAILABLE | none | 0..3600 (or NONE) | Positive (= next car's `mTimeBehindNext`). NONE for the last car or alone. Garbage during the formation lap (equals the car-ahead gap, down to −2.2 s) → INVALID (A-LMU-034). **NONE outside race sessions** (road gap, see above) |
| `timing.best_lap_s` | `scoring.vehScoringInfo[k].mBestLapTime` | SM/sco | double | s | s | m:ss.000 | SCO | AVAILABLE | ≤ 0 → NONE | 0 < t < 3600 | Sentinel ≤ 0 = no time [C] |
| `timing.last_lap_s` | `scoring.vehScoringInfo[k].mLastLapTime` | SM/sco | double | s | s | m:ss.000 | SCO | AVAILABLE | ≤ 0 → NONE | 0 < t < 3600 | |
| `timing.best_sector1_s` | `scoring.vehScoringInfo[k].mBestSector1` | SM/sco | double | s | s | s.000 | SCO | AVAILABLE | ≤ 0 → NONE | > 0 | Best S1 on its own |
| `timing.best_sector12_s` | `scoring.vehScoringInfo[k].mBestSector2` | SM/sco | double | s (S1 + S2) | s | s.000 | SCO | AVAILABLE | ≤ 0 → NONE | > `best_sector1` | **Cumulative** S1 + S2 [H]. Best S2 on its own is CALC |
| `timing.last_sector1_s` | `scoring.vehScoringInfo[k].mLastSector1` | SM/sco | double | s | s | s.000 | SCO | AVAILABLE | ≤ 0 → NONE | > 0 | |
| `timing.last_sector12_s` | `scoring.vehScoringInfo[k].mLastSector2` | SM/sco | double | s (cumulative) | s | s.000 | SCO | AVAILABLE | ≤ 0 → NONE | > `last_sector1` | Cumulative [H] |
| `timing.cur_sector1_s` | `scoring.vehScoringInfo[k].mCurSector1` | SM/sco | double | s | s | s.000 | SCO | AVAILABLE | ≤ 0 → NONE | > 0 | "if valid" [H] |
| `timing.cur_sector12_s` | `scoring.vehScoringInfo[k].mCurSector2` | SM/sco | double | s (cumulative) | s | s.000 | SCO | AVAILABLE | ≤ 0 → NONE | > `cur_sector1` | Cumulative [H] |
| `timing.best_lap_sector1_s` | `scoring.vehScoringInfo[k].mBestLapSector1` | SM/sco | float | s | s | s.000 | SCO | AVAILABLE | ≤ 0 → NONE | > 0 | S1 **of the best lap** (not the best S1) [H] |
| `timing.best_lap_sector12_s` | `scoring.vehScoringInfo[k].mBestLapSector2` | SM/sco | float | s (cumulative, assumed) | s | s.000 | SCO | AVAILABLE | ≤ 0 → NONE | > `best_lap_sector1` | Cumulative by analogy [I] |
| `timing.time_into_lap_s` | `scoring.vehScoringInfo[k].mTimeIntoLap` | SM/sco | double | s (estimated) | s | s.0 | SCO | AVAILABLE | none | ≥ 0 | LMU's estimate [H]. Negative (−5.9 s, REST `timeIntoLap`) while waiting in the pit lane after a garage exit (2026-10-13): published NONE |
| `timing.estimated_lap_s` | `scoring.vehScoringInfo[k].mEstimatedLapTime` | SM/sco | double | s | s | m:ss.000 | SCO | AVAILABLE | none | > 0 | Used by LMU for gaps [H] |
| `timing.lap_start_et_scoring_s` | `scoring.vehScoringInfo[k].mLapStartET` | SM/sco | double | s | s | — | SCO | AVAILABLE | < 0 → NONE | ≥ −1 | Cross-check with the telemetry `mLapStartET` |

## 4. Driver inputs (player)

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `inputs.throttle_raw_frac` | `telemetry.telemInfo[p].mUnfilteredThrottle` | SM/tel | double | 0.0–1.0 | fraction | % | TEL | AVAILABLE | none | −0.01..1.01 | Pedal as pressed [H] |
| `inputs.brake_raw_frac` | `telemetry.telemInfo[p].mUnfilteredBrake` | SM/tel | double | 0.0–1.0 | fraction | % | TEL | AVAILABLE | none | −0.01..1.01 | |
| `inputs.clutch_raw_frac` | `telemetry.telemInfo[p].mUnfilteredClutch` | SM/tel | double | 0.0–1.0 | fraction | % | TEL | AVAILABLE | none | −0.01..1.01 | |
| `inputs.steering_raw` | `telemetry.telemInfo[p].mUnfilteredSteering` | SM/tel | double | −1.0–1.0 (left to right) | fraction (signed) | % | TEL | AVAILABLE | none | −1.01..1.01 | **Positive = right** [H]. DuckDB correlates + with right turns [D] |
| `inputs.throttle_filtered_frac` | `telemetry.telemInfo[p].mFilteredThrottle` | SM/tel | double | 0.0–1.0 | fraction | % | TEL | AVAILABLE | none | −0.01..1.01 | After TC, limiter, auto-blip etc. [H] |
| `inputs.brake_filtered_frac` | `telemetry.telemInfo[p].mFilteredBrake` | SM/tel | double | 0.0–1.0 | fraction | % | TEL | AVAILABLE | none | −0.01..1.01 | After ABS (?) [H]. Not a line pressure |
| `inputs.clutch_filtered_frac` | `telemetry.telemInfo[p].mFilteredClutch` | SM/tel | double | 0.0–1.0 | fraction | % | TEL | AVAILABLE | none | −0.01..1.01 | |
| `inputs.steering_filtered` | `telemetry.telemInfo[p].mFilteredSteering` | SM/tel | double | −1.0–1.0 | fraction (signed) | % | TEL | AVAILABLE | none | −1.01..1.01 | Includes speed-sensitive steering [H] |
| `inputs.steering_shaft_torque_nm` | `telemetry.telemInfo[p].mSteeringShaftTorque` | SM/tel | double | — | N·m | N·m | TEL | AVAILABLE | none | abs < 500 | UNIT? DuckDB labels it Nm [D] |
| `inputs.steering_wheel_range_physical` | `telemetry.telemInfo[p].mPhysicalSteeringWheelRange` | SM/tel | float | — | rad (after conversion) | ° | STATIC | AVAILABLE | assumed degrees lock-to-lock × π/180 | 0 < x ≤ 1440° | UNIT? [A-LMU-037] |
| `inputs.steering_wheel_range_visual` | `telemetry.telemInfo[p].mVisualSteeringWheelRange` | SM/tel | float | — | rad | ° | STATIC | AVAILABLE | same as above | same | UNIT? Graphics only |

## 5. Powertrain (player)

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `powertrain.gear` | `telemetry.telemInfo[p].mGear` | SM/tel | long (i32) | −1 R, 0 N, 1+ | enum | R/N/1.. | TEL | AVAILABLE | none | −1..`mMaxGears` | [H] |
| `powertrain.engine_rpm` | `telemetry.telemInfo[p].mEngineRPM` | SM/tel | double | rpm | rpm | rpm | TEL | AVAILABLE | none | 0..1.2 × `mEngineMaxRPM` | |
| `powertrain.clutch_rpm` | `telemetry.telemInfo[p].mClutchRPM` | SM/tel | double | rpm | rpm | rpm | TEL | AVAILABLE | none | −30000..30000 | ≈ engine rpm when engaged [D]. Negative when the car slides backwards (Gate 1: −4,259 rpm in a spin, −1,456 rpm in practice) |
| `powertrain.engine_torque_nm` | `telemetry.telemInfo[p].mEngineTorque` | SM/tel | double | — (incl. additive torque) | N·m | N·m | TEL | AVAILABLE | none | −1000..3000 | UNIT? Assumed N·m [I]. Exactly 0 all session on LMGT3 (Gate 1 race and practice): withheld (UNAVAILABLE) while bit-identical to its first value with the car moving (> 2 m/s) or after 200 m of driving, VALID from the first change (A-LMU-069) |
| `powertrain.water_temp_c` | `telemetry.telemInfo[p].mEngineWaterTemp` | SM/tel | double | °C | °C | °C | TEL | AVAILABLE | none | −40..200 | [H] |
| `powertrain.oil_temp_c` | `telemetry.telemInfo[p].mEngineOilTemp` | SM/tel | double | °C | °C | °C | TEL | AVAILABLE | none | −40..200 | [H] |
| `powertrain.turbo_boost_pa` | `telemetry.telemInfo[p].mTurboBoostPressure` | SM/tel | double | — ("if available") | Pa (absolute, assumed) | kPa / bar | TEL | VEHICLE-DEPENDENT | raw 0 → NOT_SUPPORTED; unit latched on the first non-zero sample: 50..500 → kPa × 1000 (`TurboBoostKpa` notice), otherwise Pa as-is | 0..500000 | UNIT? [A-LMU-027]. Exactly 101,325 Pa on the Hypercar (Gate 1): ISA value while moving → UNAVAILABLE until it changes (A-LMU-065) |
| `powertrain.overheating` | `telemetry.telemInfo[p].mOverheating` | SM/tel | bool | bool | bool | icon | TEL | AVAILABLE | none | 0/1 | Whether the overheating icon is shown [H] |
| `powertrain.ignition_starter` | `telemetry.telemInfo[p].mIgnitionStarter` | SM/tel | unsigned char | enum | enum | text | TEL | AVAILABLE | none | 0..2 | 0 off, 1 ignition, 2 ignition + starter [H] |
| `powertrain.anti_stall_active` | `telemetry.telemInfo[p].mAntiStallActivated` | SM/tel | unsigned char | bool | bool | — | TEL | AVAILABLE | ≠ 0 → true | 0/1 | |
| `powertrain.speed_limiter_on` | `telemetry.telemInfo[p].mSpeedLimiter` | SM/tel | unsigned char | bool | bool | — | TEL | AVAILABLE | ≠ 0 → true | 0/1 | Legacy field [H] |
| `powertrain.speed_limiter_active` | `telemetry.telemInfo[p].mSpeedLimiterActive` | SM/tel | bool | bool | bool | — | TEL | AVAILABLE | none | 0/1 | 1.3 field. Difference from `mSpeedLimiter` UNVERIFIED |
| `powertrain.speed_limiter_available` | `telemetry.telemInfo[p].mSpeedLimiterAvailable` | SM/tel | unsigned char | bool | bool | — | STATIC | AVAILABLE | ≠ 0 → true | 0/1 | |
| `powertrain.motor_map` | `telemetry.telemInfo[p].mMotorMap` | SM/tel | unsigned char (uint8_t) | level | count | — | TEL | VEHICLE-DEPENDENT | none | ≤ `mMotorMapMax` | Engine or hybrid map? UNVERIFIED (DuckDB has "FuelMixtureMap") |
| `powertrain.motor_map_max` | `telemetry.telemInfo[p].mMotorMapMax` | SM/tel | unsigned char | level | count | — | STATIC | VEHICLE-DEPENDENT | none | 0..255 | 0 presumably means "not adjustable" [I] |

## 6. Fuel (player)

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `fuel.level_l` | `telemetry.telemInfo[p].mFuel` | SM/tel | double | L | L | L | TEL | AVAILABLE | none | 0..`mFuelCapacity` + 1 | Litres [H] [A-LMU-018]. DuckDB "Fuel Level" is also L [D] |
| `fuel.capacity_l` | `telemetry.telemInfo[p].mFuelCapacity` | SM/tel | double | L | L | L | STATIC | AVAILABLE | none | 0..200 | (Also listed in §2) |
| `pit.scheduled_stops` | `telemetry.telemInfo[p].mScheduledStops` | SM/tel | unsigned char | count | count | — | STATIC | AVAILABLE | none | 0..255 | [H] |

## 7. Hybrid and energy (player)

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `energy.virtual_energy_frac` | `telemetry.telemInfo[p].mVirtualEnergy` | SM/tel | float | — | fraction | % | TEL | VEHICLE-DEPENDENT | UNAVAILABLE while the scale is undecided (values in 0..1.5 fit both scales); a value > 1.5 → ÷100 (latched per session, `FractionScalePercent` notice); two laps of changing non-zero values in 0..1.5 → 0..1 (`FractionScaleFraction` notice); a value outside −0.01..101 → INVALID for the session with the raw value in a `FractionScaleUnrecognized` notice (never another unit). Data-driven availability: 0 before any non-zero value → UNAVAILABLE; no non-zero value within 300 samples → NOT_SUPPORTED (A-LMU-057) | 0..1 after conversion | UNIT? pyLMU says fraction [C]; DuckDB "Virtual Energy" in % (75–100 on a GT3) [D]. Only Hypercar and LMGT3 use VE (official guide) [A-LMU-012/013] |
| `energy.soc_frac` | `telemetry.telemInfo[p].mSoC` | SM/tel | float | — | fraction | % | TEL | VEHICLE-DEPENDENT | same scale detector as VE; hybrid-gated (UNAVAILABLE while undecided with motor state 0, NOT_SUPPORTED once the car is decided non-hybrid) | 0..1 | UNIT? pyLMU: "percent" [C]. 0 on non-hybrids [D] [A-LMU-014] |
| `energy.battery_charge_frac` | `telemetry.telemInfo[p].mBatteryChargeFraction` | SM/tel | double | fraction 0.0–1.0 | fraction | % | TEL | VEHICLE-DEPENDENT | none | 0..1 | [H]. Relation to `mSoC` UNVERIFIED |
| `energy.regen_power_w` | `telemetry.telemInfo[p].mRegen` | SM/tel | float | kW | W | kW | TEL | VEHICLE-DEPENDENT | × 1000 | abs ≤ 1.0e6 W | kW per header comment [H]. Sign UNVERIFIED |
| `hybrid.motor_torque_nm` | `telemetry.telemInfo[p].mElectricBoostMotorTorque` | SM/tel | double | — (negative when regenerating) | N·m | N·m | TEL | VEHICLE-DEPENDENT | none | abs < 2000 | UNIT? [H] sign note |
| `hybrid.motor_rpm` | `telemetry.telemInfo[p].mElectricBoostMotorRPM` | SM/tel | double | rpm | rpm | rpm | TEL | VEHICLE-DEPENDENT | none | abs < 50000 | |
| `hybrid.motor_temp_c` | `telemetry.telemInfo[p].mElectricBoostMotorTemperature` | SM/tel | double | — | °C (assumed) | °C | TEL | VEHICLE-DEPENDENT | if > 200 assume K and −273.15 (latched) | −40..250 °C | UNIT? [A-LMU-038] |
| `hybrid.motor_water_temp_c` | `telemetry.telemInfo[p].mElectricBoostWaterTemperature` | SM/tel | double | — (0 if no cooler) | °C (assumed) | °C | TEL | VEHICLE-DEPENDENT | as above; exactly 0 → NONE | −40..200 | [H] "0 otherwise" |
| `hybrid.motor_state` | `telemetry.telemInfo[p].mElectricBoostMotorState` | SM/tel | unsigned char | enum | enum | text | TEL | VEHICLE-DEPENDENT | none (always published). Hybrid channels: UNAVAILABLE while the hybrid capability is undecided and the state is 0, NOT_SUPPORTED once decided absent (A-LMU-057) | 0..4 (4 observed, A-LMU-064) | 0 unavailable (no hybrid activity in this sample), 1 inactive, 2 propulsion, 3 regeneration [H], 4 garage release (observed). 0 alone does not make hybrid channels NOT_SUPPORTED (A-LMU-057) |
| `energy.lift_and_coast_progress` | `telemetry.telemInfo[p].mLiftAndCoastProgress` | SM/tel | unsigned char (uint8_t) | — | count (raw) | — | TEL | VEHICLE-DEPENDENT | none (scale unknown) | 0..255 | UNIT? Semantics UNVERIFIED |

## 8. Driver aids and adjustable settings (player)

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `aids.tc_active` | `telemetry.telemInfo[p].mTCActive` | SM/tel | bool | bool | bool | lamp | TEL | VEHICLE-DEPENDENT | none | 0/1 | **Boolean: TC is intervening now.** No intervention amount exists |
| `aids.abs_active` | `telemetry.telemInfo[p].mABSActive` | SM/tel | bool | bool | bool | lamp | TEL | VEHICLE-DEPENDENT | none | 0/1 | **Boolean: ABS is intervening now** |
| `aids.tc_level` | `telemetry.telemInfo[p].mTC` | SM/tel | unsigned char (uint8_t) | setting level | count | — | TEL | VEHICLE-DEPENDENT | none | ≤ `mTCMax` | **Setting** (map position), not activity |
| `aids.tc_level_max` | `telemetry.telemInfo[p].mTCMax` | SM/tel | unsigned char | level | count | — | STATIC | VEHICLE-DEPENDENT | none | 0..255 | 0 = no TC [I] |
| `aids.tc_slip` | `telemetry.telemInfo[p].mTCSlip` | SM/tel | unsigned char | setting level | count | — | TEL | VEHICLE-DEPENDENT | none | ≤ `mTCSlipMax` | DuckDB event "TCSlipAngle" [D] |
| `aids.tc_slip_max` | `telemetry.telemInfo[p].mTCSlipMax` | SM/tel | unsigned char | level | count | — | STATIC | VEHICLE-DEPENDENT | none | 0..255 | |
| `aids.tc_cut` | `telemetry.telemInfo[p].mTCCut` | SM/tel | unsigned char | setting level | count | — | TEL | VEHICLE-DEPENDENT | none | ≤ `mTCCutMax` | |
| `aids.tc_cut_max` | `telemetry.telemInfo[p].mTCCutMax` | SM/tel | unsigned char | level | count | — | STATIC | VEHICLE-DEPENDENT | none | 0..255 | |
| `aids.abs_level` | `telemetry.telemInfo[p].mABS` | SM/tel | unsigned char | setting level | count | — | TEL | VEHICLE-DEPENDENT | none | ≤ `mABSMax` | **Setting**, not activity |
| `aids.abs_level_max` | `telemetry.telemInfo[p].mABSMax` | SM/tel | unsigned char | level | count | — | STATIC | VEHICLE-DEPENDENT | none | 0..255 | 0 = no ABS [I] |
| `brakes.migration` | `telemetry.telemInfo[p].mMigration` | SM/tel | unsigned char | setting level | count | — | TEL | VEHICLE-DEPENDENT | none | ≤ `mMigrationMax` | Brake migration |
| `brakes.migration_max` | `telemetry.telemInfo[p].mMigrationMax` | SM/tel | unsigned char | level | count | — | STATIC | VEHICLE-DEPENDENT | none | 0..255 | |
| `brakes.bias_rear_frac` | `telemetry.telemInfo[p].mRearBrakeBias` | SM/tel | double | fraction of braking on the rear | fraction | % F:R | TEL | AVAILABLE | display front = 1 − x | 0.2..0.8 | [H]. DuckDB "Brake Bias Rear" 0.505 matches "49.5:50.5" [D] |
| `chassis.arb_front` | `telemetry.telemInfo[p].mFrontAntiSway` | SM/tel | unsigned char | setting level | count | — | TEL | VEHICLE-DEPENDENT | none | ≤ `mFrontAntiSwayMax` | |
| `chassis.arb_front_max` | `telemetry.telemInfo[p].mFrontAntiSwayMax` | SM/tel | unsigned char | level | count | — | STATIC | VEHICLE-DEPENDENT | none | 0..255 | |
| `chassis.arb_rear` | `telemetry.telemInfo[p].mRearAntiSway` | SM/tel | unsigned char | setting level | count | — | TEL | VEHICLE-DEPENDENT | none | ≤ `mRearAntiSwayMax` | |
| `chassis.arb_rear_max` | `telemetry.telemInfo[p].mRearAntiSwayMax` | SM/tel | unsigned char | level | count | — | STATIC | VEHICLE-DEPENDENT | none | 0..255 | |
| `aero.front_flap_active` | `telemetry.telemInfo[p].mFrontFlapActivated` | SM/tel | unsigned char | bool | bool | — | TEL | VEHICLE-DEPENDENT | ≠ 0 → true | 0/1 | |
| `aero.rear_flap_active` | `telemetry.telemInfo[p].mRearFlapActivated` | SM/tel | unsigned char | bool | bool | — | TEL | VEHICLE-DEPENDENT | ≠ 0 → true | 0/1 | DRS-style flap |
| `aero.rear_flap_legal_status` | `telemetry.telemInfo[p].mRearFlapLegalStatus` | SM/tel | unsigned char | enum | enum | — | TEL | VEHICLE-DEPENDENT | none | 0..2 | 0 disallowed, 1 criteria detected, 2 allowed [H] |
| `vehicle.headlights_on` | `telemetry.telemInfo[p].mHeadlights` | SM/tel | bool | bool | bool | — | TEL | AVAILABLE | none | 0/1 | |
| `vehicle.wiper_state` | `telemetry.telemInfo[p].mWiperState` | SM/tel | unsigned char (uint8_t) | enum | enum | — | TEL | AVAILABLE | none | 0..255 | Enum values UNVERIFIED |

## 9. Dynamics: raw vectors (player). LMU local frame: +x left, +y up, +z rear [H]

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `vehicle.pos_{x,y,z}_m` | `telemetry.telemInfo[p].mPos` (`.x/.y/.z`) | SM/tel | struct TelemVect3 (3 × double) | m (world) | m | m | TEL | AVAILABLE | none | each abs < 1.0e5 | World frame is **left-handed, +y up** [H]. Map uses x and z [A-LMU-025] |
| `vehicle.local_vel_{x,y,z}_mps` | `telemetry.telemInfo[p].mLocalVel` | SM/tel | struct TelemVect3 | m/s (local) | m/s | — | TEL | AVAILABLE | none (kept in the LMU frame) | each abs < 150 | Forward motion is **−z** [H]. Raw; use derived channels for display |
| `vehicle.local_accel_{x,y,z}_mps2` | `telemetry.telemInfo[p].mLocalAccel` | SM/tel | struct TelemVect3 | m/s² (local) | m/s² | — | TEL | AVAILABLE | none | each abs < 490 (50 g); > 60 m/s² outside an impact is SUSPECT | Whether it includes gravity is UNVERIFIED [A-LMU-032] |
| `vehicle.orientation_row{0,1,2}` | `telemetry.telemInfo[p].mOri[0..3]` | SM/tel | struct TelemVect3[3] | unitless (rotation matrix rows) | none | — | TEL | AVAILABLE | none | each row norm 1 ± 0.01 | Row r · local vector = world component r [H] |
| `vehicle.local_rot_{x,y,z}_radps` | `telemetry.telemInfo[p].mLocalRot` | SM/tel | struct TelemVect3 | rad/s (local) | rad/s | — | TEL | AVAILABLE | none | each abs < 20 | +x pitch up, **+y yaw right**, +z roll right [H] |
| `vehicle.local_rot_accel_{x,y,z}_radps2` | `telemetry.telemInfo[p].mLocalRotAccel` | SM/tel | struct TelemVect3 | rad/s² | rad/s² | — | TEL | AVAILABLE | none | each abs < 500 | |

## 10. Chassis, aero and suspension (player)

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `chassis.front_ride_height_m` | `telemetry.telemInfo[p].mFrontRideHeight` | SM/tel | double | — | m | mm | TEL | AVAILABLE | none | −0.05..0.3 | Exactly 0 on Hypercar and LMGT3 (Gate 1): a 0 m first value is impossible → UNAVAILABLE until it changes (A-LMU-065) |
| `chassis.rear_ride_height_m` | `telemetry.telemInfo[p].mRearRideHeight` | SM/tel | double | — | m | mm | TEL | AVAILABLE | none | −0.05..0.3 | As the front row (A-LMU-065) |
| `aero.front_wing_height_m` | `telemetry.telemInfo[p].mFrontWingHeight` | SM/tel | double | — | m | mm | TEL | AVAILABLE | none | −0.05..0.5 | Exactly 0 on Hypercar and LMGT3 (Gate 1) → UNAVAILABLE until it changes (A-LMU-065) |
| `chassis.front_3rd_deflection_m` | `telemetry.telemInfo[p].mFront3rdDeflection` | SM/tel | double | — | m | mm | TEL | VEHICLE-DEPENDENT | none | abs < 0.2 | UNIT? [D] m. 0 if the car has no third spring [I] |
| `chassis.rear_3rd_deflection_m` | `telemetry.telemInfo[p].mRear3rdDeflection` | SM/tel | double | — | m | mm | TEL | VEHICLE-DEPENDENT | none | abs < 0.2 | UNIT? |
| `aero.drag_n` | `telemetry.telemInfo[p].mDrag` | SM/tel | double | — | N (assumed) | N | TEL | AVAILABLE | none | 0..30000 | UNIT? [A-LMU-028]. Exactly 0 on Hypercar and LMGT3 (Gate 1): 0 N while moving > 2 m/s → UNAVAILABLE until it changes (A-LMU-065) |
| `aero.front_downforce_n` | `telemetry.telemInfo[p].mFrontDownforce` | SM/tel | double | — | N (assumed) | N | TEL | AVAILABLE | none | −5000..60000 | UNIT? Exactly 0 on Hypercar and LMGT3 (Gate 1) → UNAVAILABLE while moving until it changes (A-LMU-065) |
| `aero.rear_downforce_n` | `telemetry.telemInfo[p].mRearDownforce` | SM/tel | double | — | N (assumed) | N | TEL | AVAILABLE | none | −5000..60000 | UNIT? As the front row (A-LMU-065) |
| `suspension.{w}.deflection_m` | `telemetry.telemInfo[p].mWheel[w].mSuspensionDeflection` | SM/tel | double | m | m | mm | TEL | AVAILABLE | none | abs < 0.3 | [H]. DuckDB "Susp Pos" [D] |
| `suspension.{w}.ride_height_m` | `telemetry.telemInfo[p].mWheel[w].mRideHeight` | SM/tel | double | m | m | mm | TEL | AVAILABLE | none | −0.05..2.0 | [H]. A wheel off the ground reads up to 0.9 m (Gate 1 crash); 2 m bound |
| `suspension.{w}.pushrod_force_n` | `telemetry.telemInfo[p].mWheel[w].mSuspForce` | SM/tel | double | N (pushrod load) | N | N | TEL | AVAILABLE | none | −2.0e4..1.5e5 | [H] |
| `wheels.{w}.camber_rad` | `telemetry.telemInfo[p].mWheel[w].mCamber` | SM/tel | double | rad (header: positive is left for left-side wheels, right for right-side wheels) | rad | ° | TEL | AVAILABLE | none | abs < 0.6 | [H]. Gate 1 race: ±0.36..0.50 rad in kerb strikes / an impact, so the bound is 0.6 rad. Sign convention still to verify |
| `wheels.{w}.toe_rad` | `telemetry.telemInfo[p].mWheel[w].mToe` | SM/tel | double | — (angle w.r.t. vehicle) | rad (assumed) | ° | TEL | AVAILABLE | none | abs < 0.698 (40°) | UNIT? Sign UNVERIFIED. The front toe includes the road-wheel steering angle (Gate 1: ±0.35 rad, A-LMU-068) |
| `wheels.{w}.rotation_radps` | `telemetry.telemInfo[p].mWheel[w].mRotation` | SM/tel | double | rad/s | rad/s | — | TEL | AVAILABLE | none | abs < 400 | Sign when driving forward UNVERIFIED [A-LMU-039] |
| `wheels.{w}.y_location_m` | `telemetry.telemInfo[p].mWheel[w].mWheelYLocation` | SM/tel | double | — (relative to vehicle y) | m | mm | TEL | AVAILABLE | none | abs < 1 | UNIT? |
| `wheels.{w}.detached` | `telemetry.telemInfo[p].mWheel[w].mDetached` | SM/tel | bool | bool | bool | — | TEL | AVAILABLE | none | 0/1 | |

## 11. Tyres (player, per wheel)

**Tyre temperature orientation.** `mTemperature[3]` and `mTireInnerLayerTemperature[3]` are ordered **left / centre / right in car coordinates**. The header explicitly warns this is *not* inside/centre/outside. The header's frame puts **+x out of the car's LEFT side** (driver's view). Therefore (ASSUMPTION **A-LMU-016**, needs runtime verification):

| Wheel | index 0 (left edge) | index 1 | index 2 (right edge) |
|---|---|---|---|
| FL `mWheel[0]` | **outer** | centre | **inner** |
| FR `mWheel[1]` | **inner** | centre | **outer** |
| RL `mWheel[2]` | **outer** | centre | **inner** |
| RR `mWheel[3]` | **inner** | centre | **outer** |

- **Supporting evidence (INFERRED):** in the four DuckDB recordings, the left wheels (`value1`, `value3`) have a hotter `TyresTempRight` than `TyresTempLeft` (63.5 vs 55.9 °C), and the right wheels show the opposite [D]. With negative camber the inboard edge runs hotter. This is consistent with the table, *provided* DuckDB's Left/Right follow the same car-frame convention as shared memory.
- **Live check:** in a long corner with negative camber, the inner-edge temperature must be the hotter one.

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `tyres.{w}.pressure_pa` | `telemetry.telemInfo[p].mWheel[w].mPressure` | SM/tel | double | kPa | Pa | kPa (psi optional) | TEL | AVAILABLE | kPa × 1000 → Pa | 0..600 kPa raw | [H]. DuckDB 127–176 kPa [D] |
| `tyres.{w}.temp_surface_left_c` | `telemetry.telemInfo[p].mWheel[w].mTemperature[0]` | SM/tel | double | K | °C | °C | TEL | AVAILABLE | K − 273.15 | 200..700 K raw | **Car-frame left edge** [H]. See the inner/outer table |
| `tyres.{w}.temp_surface_center_c` | `telemetry.telemInfo[p].mWheel[w].mTemperature[1]` | SM/tel | double | K | °C | °C | TEL | AVAILABLE | K − 273.15 | 200..700 K | |
| `tyres.{w}.temp_surface_right_c` | `telemetry.telemInfo[p].mWheel[w].mTemperature[2]` | SM/tel | double | K | °C | °C | TEL | AVAILABLE | K − 273.15 | 200..700 K | **Car-frame right edge** |
| `tyres.{w}.temp_inner_layer_left_c` | `telemetry.telemInfo[p].mWheel[w].mTireInnerLayerTemperature[0]` | SM/tel | double | K | °C | °C | TEL | AVAILABLE | K − 273.15 | 200..700 K | Innermost rubber layer before the carcass [H]. Same L/C/R orientation (assumed) |
| `tyres.{w}.temp_inner_layer_center_c` | `telemetry.telemInfo[p].mWheel[w].mTireInnerLayerTemperature[1]` | SM/tel | double | K | °C | °C | TEL | AVAILABLE | K − 273.15 | 200..700 K | |
| `tyres.{w}.temp_inner_layer_right_c` | `telemetry.telemInfo[p].mWheel[w].mTireInnerLayerTemperature[2]` | SM/tel | double | K | °C | °C | TEL | AVAILABLE | K − 273.15 | 200..700 K | |
| `tyres.{w}.temp_carcass_c` | `telemetry.telemInfo[p].mWheel[w].mTireCarcassTemperature` | SM/tel | double | K | °C | °C | TEL | AVAILABLE | K − 273.15 | 200..700 K | Rough carcass average [H] |
| `tyres.{w}.temp_optimal_c` | `telemetry.telemInfo[p].mWheel[w].mOptimalTemp` | SM/tel | float | — | °C (assumed) | °C | STATIC | AVAILABLE | if > 200 treat as K (− 273.15) | 0..200 °C after conversion | UNIT? pyLMU claims °C [C] [A-LMU-038]. 1.3+ |
| `tyres.{w}.wear_frac` | `telemetry.telemInfo[p].mWheel[w].mWear` | SM/tel | double | 0.0–1.0 fraction of maximum | fraction | % | TEL | AVAILABLE | none | 0..1 | Not necessarily proportional to grip loss [H]. Direction (1 = new or 1 = fully worn?) UNVERIFIED [A-LMU-040]. UNAVAILABLE until a non-zero value is seen (0 is ambiguous; A-LMU-057) |
| `tyres.{w}.sliding_frac` | `telemetry.telemInfo[p].mWheel[w].mGripFract` | SM/tel | double | fraction of the patch sliding (approx.) | fraction | % | TEL | AVAILABLE | none | 0..1 | Name is misleading: it is the *sliding* fraction [H]. Exactly 0 all session on LMGT3 → UNAVAILABLE while an unchanged 0 with the car moving (> 2 m/s) or after 200 m unchanged, VALID from the first change (A-LMU-069) |
| `tyres.{w}.load_n` | `telemetry.telemInfo[p].mWheel[w].mTireLoad` | SM/tel | double | N | N | N | TEL | AVAILABLE | none | 0..6.0e4 | [H]. Exactly 0 all session on LMGT3 (Gate 1 race and practice; filled on the Hypercar): 0 N on a moving car → UNAVAILABLE until it changes (A-LMU-069) |
| `tyres.{w}.lateral_force_n` | `telemetry.telemInfo[p].mWheel[w].mLateralForce` | SM/tel | double | N | N | N | TEL | AVAILABLE | none | abs < 6.0e4 | Sign convention UNVERIFIED. Exactly 0 on LMGT3 → UNAVAILABLE while an unchanged 0 with the car moving (> 2 m/s) or after 200 m unchanged (A-LMU-069) |
| `tyres.{w}.longitudinal_force_n` | `telemetry.telemInfo[p].mWheel[w].mLongitudinalForce` | SM/tel | double | N | N | N | TEL | AVAILABLE | none | abs < 6.0e4 | Sign UNVERIFIED. Exactly 0 on LMGT3 → UNAVAILABLE while an unchanged 0 with the car moving (> 2 m/s) or after 200 m unchanged (A-LMU-069) |
| `tyres.{w}.lateral_patch_vel_mps` | `telemetry.telemInfo[p].mWheel[w].mLateralPatchVel` | SM/tel | double | — | m/s (assumed) | — | TEL | AVAILABLE | none | abs < 150 | UNIT? Input for slip (experimental) |
| `tyres.{w}.longitudinal_patch_vel_mps` | `telemetry.telemInfo[p].mWheel[w].mLongitudinalPatchVel` | SM/tel | double | — | m/s (assumed) | — | TEL | AVAILABLE | none | abs < 150 | UNIT? |
| `tyres.{w}.lateral_ground_vel_mps` | `telemetry.telemInfo[p].mWheel[w].mLateralGroundVel` | SM/tel | double | — | m/s (assumed) | — | TEL | AVAILABLE | none | abs < 150 | UNIT? The header comment is identical to the patch variant |
| `tyres.{w}.longitudinal_ground_vel_mps` | `telemetry.telemInfo[p].mWheel[w].mLongitudinalGroundVel` | SM/tel | double | — | m/s (assumed) | — | TEL | AVAILABLE | none | abs < 150 | UNIT? |
| `tyres.{w}.vertical_deflection_m` | `telemetry.telemInfo[p].mWheel[w].mVerticalTireDeflection` | SM/tel | double | — | m (assumed) | mm | TEL | AVAILABLE | none | abs < 0.2 | UNIT? |
| `tyres.{w}.static_radius_m` | `telemetry.telemInfo[p].mWheel[w].mStaticUndeflectedRadius` | SM/tel | unsigned char | cm | m | mm | STATIC | AVAILABLE | cm ÷ 100; raw 0 → NONE (and no wheel speed) (A-LMU-060) | 20..60 cm raw | [H]. **1 cm resolution (u8)** |
| `tyres.{w}.compound_index` | `telemetry.telemInfo[p].mWheel[w].mCompoundIndex` | SM/tel | unsigned char | index | index | — | STATIC | AVAILABLE | none | 0..255 | 1.3+ |
| `tyres.{w}.compound_type` | `telemetry.telemInfo[p].mWheel[w].mCompoundType` | SM/tel | unsigned char | enum | enum | text | STATIC | AVAILABLE | none | 0..3 known; other → Unknown | 0 soft, 1 medium, 2 hard, 3 wet [C pyLMU] [A-LMU-041] |
| `tyres.{w}.flat` | `telemetry.telemInfo[p].mWheel[w].mFlat` | SM/tel | bool | bool | bool | — | TEL | AVAILABLE | none | 0/1 | |
| `tyres.{w}.surface_type` | `telemetry.telemInfo[p].mWheel[w].mSurfaceType` | SM/tel | unsigned char | enum | enum | text | TEL | AVAILABLE | none | 0..6 | 0 dry, 1 wet, 2 grass, 3 dirt, 4 gravel, 5 rumble strip, 6 special [H] |
| `tyres.{w}.terrain_name` | `telemetry.telemInfo[p].mWheel[w].mTerrainName` | SM/tel | char[16] | text | text | — | TEL | AVAILABLE | bounded decode | — | TDF material prefix [H] |
| `tyres.front_compound_name` | `telemetry.telemInfo[p].mFrontTireCompoundName` | SM/tel | char[18] | text | text | text | STATIC | AVAILABLE | bounded decode | — | **Per axle only**. There is no per-wheel name |
| `tyres.rear_compound_name` | `telemetry.telemInfo[p].mRearTireCompoundName` | SM/tel | char[18] | text | text | text | STATIC | AVAILABLE | bounded decode | — | |
| `tyres.front_compound_index` | `telemetry.telemInfo[p].mFrontTireCompoundIndex` | SM/tel | unsigned char | index within brand | index | — | STATIC | AVAILABLE | none | 0..255 | [H] |
| `tyres.rear_compound_index` | `telemetry.telemInfo[p].mRearTireCompoundIndex` | SM/tel | unsigned char | index within brand | index | — | STATIC | AVAILABLE | none | 0..255 | |

## 12. Brakes (player, per wheel)

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `brakes.{w}.temp_c` | `telemetry.telemInfo[p].mWheel[w].mBrakeTemp` | SM/tel | double | **°C per header; K per community** | °C | °C | TEL | AVAILABLE | **Unit detector** (A-LMU-015, `lmu-normalizer` `detect::BrakeTempUnitDetector`): while the car is cold (first 10 s after the car is seen *entering* realtime, speed < 5 m/s, the 4 wheels within 5 units of each other), K if all 4 wheels are in 250..350 and the coldest is within ±25 of `mAmbientTemp + 273.15`; °C if all 4 are in −30..80 and within ±25 of ambient; also °C if all 4 are non-zero and below 200, K if any is in 1500..1773. 20 agreeing votes latch the unit per session. **Until decided:** withheld as UNAVAILABLE (raw value in a `BrakeTempUnitUnverified` notice, capability note "unit unverified"); the documented SUSPECT policy awaits a SUSPECT quality in telemetry-model | −50..1500 °C after conversion | [H] says Celsius. TinyPedal subtracts 273.15, KoDriver measured ≈285 cold, race-engineer says K [C] |
| `brakes.{w}.pressure_frac` | `telemetry.telemInfo[p].mWheel[w].mBrakePressure` | SM/tel | double | 0.0–1.0 (driver input × brake balance) | fraction | % | TEL | AVAILABLE | none (fraction). The first value > 1.5 raises the `BrakePressureUnitChanged` notice; all four channels are then INVALID for the session | 0..1.01. **If > 1.5 is ever seen, raise `UNIT_CHANGED` and stop mapping** | [H]: "will convert to true brake pressure (kPa) in future". **Not a pressure** [A-LMU-017] |

## 13. Damage (player)

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `damage.dent_severity_{0..7}` | `telemetry.telemInfo[p].mDentSeverity[0..8]` | SM/tel | unsigned char[8] | 0 none, 1 some, 2 more | enum | icon | TEL | AVAILABLE | none | 0..2 | 8 locations around the car [H]. Location order UNVERIFIED |
| `damage.parts_detached` | `telemetry.telemInfo[p].mDetached` | SM/tel | bool | bool | bool | — | TEL | AVAILABLE | none | 0/1 | Any part other than a wheel [H] |
| `damage.last_impact_et_s` | `telemetry.telemInfo[p].mLastImpactET` | SM/tel | double | s | s | — | TEL | AVAILABLE | none | ≤ `mElapsedTime` | |
| `damage.last_impact_magnitude` | `telemetry.telemInfo[p].mLastImpactMagnitude` | SM/tel | double | — | none (raw) | — | TEL | AVAILABLE | none | ≥ 0 | UNIT? |
| `damage.last_impact_pos_{x,y,z}_m` | `telemetry.telemInfo[p].mLastImpactPos` | SM/tel | struct TelemVect3 | — | m (assumed) | — | TEL | AVAILABLE | none | finite | Frame (world or local) UNVERIFIED |

## 14. Track position and pit (player)

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `track.current_sector_raw` | `telemetry.telemInfo[p].mCurrentSector` | SM/tel | long (i32) | zero-based sector, pit lane in the sign bit | raw | — | TEL | AVAILABLE | see CALC `track.sector_index` / `pit.in_pit_lane` | (raw & 0x7FFFFFFF) in 0..2 | Example from the header: 0x80000002 = entering pits from the third sector [H] |
| `track.limits_steps` | `telemetry.telemInfo[p].mTrackLimitsSteps` | SM/tel | unsigned char (uint8_t) | normalised points × steps per point | count | — | TEL | AVAILABLE | none | 0..255 | [H]. Use with `mTrackLimitsStepsPerPoint` |
| `track.lap_dist_m` | `scoring.vehScoringInfo[k].mLapDist` | SM/sco | double | m (distance around track) | m | m | SCO | AVAILABLE | none | −`track.length_m` − 50..`track.length_m` + 50 | Only in scoring (~5 Hz). **Negative before the first line crossing and in the pit area** (Gate 1 race: −1,586.8 m garage, −1,023.4 m grid; LMU re-bases it by ±1 lap at the formation start and at the green flag without counting a lap; A-LMU-070). Below −1 lap → INVALID |
| `track.path_lateral_m` | `scoring.vehScoringInfo[k].mPathLateral` | SM/sco | double | m (from an approximate centre path) | m | m | SCO | AVAILABLE | none | abs < 100 | "very approximate" [H]. Sign UNVERIFIED |
| `track.edge_m` | `scoring.vehScoringInfo[k].mTrackEdge` | SM/sco | double | m | m | m | SCO | AVAILABLE | none | abs < 100 | Track edge on the car's side [H] |
| `track.sector_scoring_raw` | `scoring.vehScoringInfo[k].mSector` | SM/sco | signed char | 0 = S3, 1 = S1, 2 = S2 | raw | — | SCO | AVAILABLE | see CALC | 0..2 | "don't ask why" [H] |
| `pit.in_pits` | `scoring.vehScoringInfo[k].mInPits` | SM/sco | bool | bool | bool | — | SCO | AVAILABLE | none | 0/1 | Between pit entrance and exit. Unreliable for remote cars [H] |
| `pit.state` | `scoring.vehScoringInfo[k].mPitState` | SM/sco | unsigned char | enum | enum | text | SCO | AVAILABLE | none | 0..5 | **Observed** (Gate 1 pit-box stops, A-LMU-029): 0 none, 1 request (on track), 2 entering (pit lane), 4 stopped in the box (service), 5 leaving the box / in or leaving the garage stall; 3 never seen. Header: 3 stopped, 4 exiting |
| `pit.in_garage_stall` | `scoring.vehScoringInfo[k].mInGarageStall` | SM/sco | bool | bool | bool | — | SCO | AVAILABLE | none | 0/1 | |
| `pit.lap_dist_m` | `scoring.vehScoringInfo[k].mPitLapDist` | SM/sco | float | m (pit location as lap distance) | m | m | STATIC | AVAILABLE | none | 0..`track.length_m` | |
| `pit.num_stops` | `scoring.vehScoringInfo[k].mNumPitstops` | SM/sco | short | count | count | — | SCO | AVAILABLE | none | 0..100 | |
| `race.num_penalties` | `scoring.vehScoringInfo[k].mNumPenalties` | SM/sco | short | count (outstanding) | count | — | SCO | AVAILABLE | none | 0..100 | Count only. No reason or type |
| `race.place` | `scoring.vehScoringInfo[k].mPlace` | SM/sco | unsigned char | 1-based | count | P# | SCO | AVAILABLE | 0 (not classified yet) → no data | 1..104 | Overall position. The class position is CALC |
| `race.laps_completed` | `scoring.vehScoringInfo[k].mTotalLaps` | SM/sco | short | laps | count | — | SCO | AVAILABLE | none | 0..10000 | |
| `race.finish_status` | `scoring.vehScoringInfo[k].mFinishStatus` | SM/sco | signed char | enum | enum | text | SCO | AVAILABLE | none | 0..3 | 0 none, 1 finished, 2 DNF, 3 DQ [H] |
| `race.time_behind_next_s` | `scoring.vehScoringInfo[k].mTimeBehindNext` | SM/sco | double | s | s | +s.000 | SCO | AVAILABLE | none | ≥ 0 (NONE outside a race) | To the car one place ahead [H]. Race sessions only: in practice / qualifying LMU writes a road gap or 0 (A-LMU-034) |
| `race.laps_behind_next` | `scoring.vehScoringInfo[k].mLapsBehindNext` | SM/sco | long (i32) | laps | count | — | SCO | AVAILABLE | none | ≥ 0 (NONE outside a race) | A-LMU-034 |
| `race.time_behind_leader_s` | `scoring.vehScoringInfo[k].mTimeBehindLeader` | SM/sco | double | s | s | +s.000 | SCO | AVAILABLE | none | ≥ 0 (NONE outside a race) | A-LMU-034 |
| `race.laps_behind_leader` | `scoring.vehScoringInfo[k].mLapsBehindLeader` | SM/sco | long (i32) | laps | count | — | SCO | AVAILABLE | none | ≥ 0 (NONE outside a race) | A-LMU-034 |
| `race.qualification` | `scoring.vehScoringInfo[k].mQualification` | SM/sco | long (i32) | 1-based | count | — | STATIC | AVAILABLE | −1 → NONE | −1 or 1..104 | [H] |
| `race.control` | `scoring.vehScoringInfo[k].mControl` | SM/sco | signed char | enum | enum | text | SCO | AVAILABLE | none | −1..3 | −1 nobody, 0 local player, 1 local AI, 2 remote, 3 replay [H] |
| `race.server_scored` | `scoring.vehScoringInfo[k].mServerScored` | SM/sco | unsigned char | bool | bool | — | SCO | AVAILABLE | ≠ 0 → true | 0/1 | |
| `race.count_lap_flag` | `scoring.vehScoringInfo[k].mCountLapFlag` | SM/sco | unsigned char | enum | enum | — | SCO | AVAILABLE | none | 0..2 | 0 count nothing, 1 lap only, 2 lap and time [H] |

## 15. Flags

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `flags.yellow_flag_state` | `scoring.scoringInfo.mYellowFlagState` | SM/sco | signed char | enum | enum | text | SCO | AVAILABLE | none | −1..7 | −1 invalid, 0 none, 1 pending, 2 pits closed, 3 pit lead lap, 4 pits open, 5 last lap, 6 resume, 7 race halt [H]. Full course only |
| `flags.sector_flag_{0,1,2}` | `scoring.scoringInfo.mSectorFlag[0..3]` | SM/sco | signed char[3] | — | enum (raw) | — | SCO | AVAILABLE | see CALC `flags.local_yellow_s{n}` | finite | **Index order unknown** (the header author says "test"). Values 1 = yellow, 11 = clear [C LMU Pitwall; DuckDB events show 11 [D]] [A-LMU-030] |
| `flags.car_flag` | `scoring.vehScoringInfo[k].mFlag` | SM/sco | unsigned char | enum | enum | text | SCO | AVAILABLE | none | {0, 6} expected | Currently only 0 green or 6 blue [H] |
| `flags.under_yellow` | `scoring.vehScoringInfo[k].mUnderYellow` | SM/sco | bool | bool | bool | — | SCO | AVAILABLE | none | 0/1 | Taken the full-course caution at the S/F line [H] |
| `flags.individual_phase` | `scoring.vehScoringInfo[k].mIndividualPhase` | SM/sco | unsigned char | enum | enum | text | SCO | AVAILABLE | none | 0..11 | Game phases plus 9 after formation, 10 under yellow, 11 under blue (unused) [H] |

## 16. Weather

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `weather.ambient_temp_c` | `scoring.scoringInfo.mAmbientTemp` | SM/sco | double | °C | °C | °C | SLOW | AVAILABLE | none | −30..60 | [H] |
| `weather.track_temp_c` | `scoring.scoringInfo.mTrackTemp` | SM/sco | double | °C | °C | °C | SLOW | AVAILABLE | none | −30..80 | [H] |
| `weather.raining_frac` | `scoring.scoringInfo.mRaining` | SM/sco | double | 0.0–1.0 severity | fraction | % | SLOW | AVAILABLE | none | 0..1 | [H] |
| `weather.dark_cloud_frac` | `scoring.scoringInfo.mDarkCloud` | SM/sco | double | 0.0–1.0 | fraction | % | SLOW | AVAILABLE | none | 0..1 | "cloud darkness?" (the header itself is unsure) |
| `weather.cloud_coverage` | `scoring.scoringInfo.mCloudCoverage` | SM/sco | unsigned char (uint8_t) | — | enum (raw) | — | SLOW | AVAILABLE | none | 0..255 | UNIT? 1.3+ |
| `weather.path_wetness_min_frac` | `scoring.scoringInfo.mMinPathWetness` | SM/sco | double | 0.0–1.0 | fraction | % | SLOW | AVAILABLE | none | 0..1 | Main racing path [H] |
| `weather.path_wetness_max_frac` | `scoring.scoringInfo.mMaxPathWetness` | SM/sco | double | 0.0–1.0 | fraction | % | SLOW | AVAILABLE | none | 0..1 | |
| `weather.path_wetness_avg_frac` | `scoring.scoringInfo.mAvgPathWetness` | SM/sco | double | 0.0–1.0 | fraction | % | SLOW | AVAILABLE | none | 0..1 | |
| `weather.wind_{x,y,z}_mps` | `scoring.scoringInfo.mWind` | SM/sco | struct TelemVect3 | "wind speed" (vector, no unit) | m/s (assumed) | km/h | SLOW | AVAILABLE | none | each abs < 60 | UNIT? Frame assumed world [A-LMU-042] |

## 17. Competitors (per scoring row `i`, keyed by `mID`)

All rows: Source `SM/sco`, Frequency `SCO`, index `i < min(mNumVehicles, 104)`. Fields with the same meaning as the player's own (§3, §14, §15) use the same conversion and validation.

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `cars.<id>.id` | `scoring.vehScoringInfo[i].mID` | SM/sco | long (i32) | id | id | — | SCO | AVAILABLE | none | ≥ 0; unique within the snapshot | Key. Duplicates → drop the snapshot's standings as SUSPECT |
| `cars.<id>.driver_name` | `scoring.vehScoringInfo[i].mDriverName` | SM/sco | char[32] | text (UTF-8, CLAIMED) | text | text | SCO | AVAILABLE | bounded lossy UTF-8 | — | **PII**, but needed for standings. Transmitted only inside the authorised session |
| `cars.<id>.vehicle_name` | `scoring.vehScoringInfo[i].mVehicleName` | SM/sco | char[64] | text | text | text | STATIC | AVAILABLE | bounded decode | — | |
| `cars.<id>.class_name` | `scoring.vehScoringInfo[i].mVehicleClass` | SM/sco | char[32] | text | text | text | STATIC | AVAILABLE | bounded decode | — | Class grouping key for class positions |
| `cars.<id>.place` | `scoring.vehScoringInfo[i].mPlace` | SM/sco | unsigned char | 1-based | count | P# | SCO | AVAILABLE | none | 1..104 | |
| `cars.<id>.laps_completed` | `scoring.vehScoringInfo[i].mTotalLaps` | SM/sco | short | laps | count | — | SCO | AVAILABLE | none | ≥ 0 | |
| `cars.<id>.lap_dist_m` | `scoring.vehScoringInfo[i].mLapDist` | SM/sco | double | m | m | — | SCO | AVAILABLE | none | −50..length + 50 | Used for the track-map position and relative gaps |
| `cars.<id>.sector_raw` | `scoring.vehScoringInfo[i].mSector` | SM/sco | signed char | 0 = S3, 1 = S1, 2 = S2 | raw | — | SCO | AVAILABLE | CALC remap | 0..2 | |
| `cars.<id>.best_lap_s` / `last_lap_s` | `scoring.vehScoringInfo[i].mBestLapTime` / `.mLastLapTime` | SM/sco | double | s | s | m:ss.000 | SCO | AVAILABLE | ≤ 0 → NONE | 0..3600 | |
| `cars.<id>.best/last/cur_sector1_s`, `…_sector12_s` | `scoring.vehScoringInfo[i].mBestSector1`, `.mBestSector2`, `.mLastSector1`, `.mLastSector2`, `.mCurSector1`, `.mCurSector2` | SM/sco | double | s (S2 cumulative) | s | s.000 | SCO | AVAILABLE | ≤ 0 → NONE | > 0 | As in §3 |
| `cars.<id>.time_behind_next_s`, `time_behind_leader_s` | `scoring.vehScoringInfo[i].mTimeBehindNext` / `.mTimeBehindLeader` | SM/sco | double | s | s | +s.000 | SCO | AVAILABLE | none | ≥ 0 (NONE outside a race) | A-LMU-034 |
| `cars.<id>.laps_behind_next`, `laps_behind_leader` | `scoring.vehScoringInfo[i].mLapsBehindNext` / `.mLapsBehindLeader` | SM/sco | long (i32) | laps | count | — | SCO | AVAILABLE | none | ≥ 0 (NONE outside a race) | A-LMU-034 |
| `cars.<id>.in_pits` / `pit_state` / `num_pitstops` | `scoring.vehScoringInfo[i].mInPits` / `.mPitState` / `.mNumPitstops` | SM/sco | bool / unsigned char / short | — | bool / enum / count | — | SCO | AVAILABLE | none | as in §14 | `mInPits` unreliable for remote cars [H]. Prefer `mPitState` |
| `cars.<id>.num_penalties` | `scoring.vehScoringInfo[i].mNumPenalties` | SM/sco | short | count | count | — | SCO | AVAILABLE | none | ≥ 0 | |
| `cars.<id>.finish_status` / `control` / `is_player` | `scoring.vehScoringInfo[i].mFinishStatus` / `.mControl` / `.mIsPlayer` | SM/sco | signed char / signed char / bool | enum | enum / bool | — | SCO | AVAILABLE | none | as in §14 | Exactly one row with `mIsPlayer` expected while driving |
| `cars.<id>.flag` / `under_yellow` / `individual_phase` / `count_lap_flag` | `scoring.vehScoringInfo[i].mFlag` / `.mUnderYellow` / `.mIndividualPhase` / `.mCountLapFlag` | SM/sco | u8 / bool / u8 / u8 | enum | enum | — | SCO | AVAILABLE | none | as in §14–15 | Blue flag = `mFlag == 6` |
| `cars.<id>.fuel_frac` | `scoring.vehScoringInfo[i].mFuelFraction` | SM/sco | unsigned char | 0x00 = 0 % … 0xFF = 100 % of fuel **or battery** | fraction | % | SCO | AVAILABLE | ÷ 255 | 0..255 raw | [H]. Whether it is VE, fuel or battery depends on the car [A-LMU-044] |
| `cars.<id>.pit_group` | `scoring.vehScoringInfo[i].mPitGroup` | SM/sco | char[24] | text (team name, usually) | text | text | STATIC | AVAILABLE | bounded decode | — | |
| `cars.<id>.veh_filename` | `scoring.vehScoringInfo[i].mVehFilename` | SM/sco | char[32] | text | text | — | STATIC | AVAILABLE | bounded decode | — | |
| `cars.<id>.estimated_lap_s` / `time_into_lap_s` | `scoring.vehScoringInfo[i].mEstimatedLapTime` / `.mTimeIntoLap` | SM/sco | double | s | s | — | SCO | AVAILABLE | none | ≥ 0 | |
| `cars.<id>.lap_start_et_s` | `scoring.vehScoringInfo[i].mLapStartET` | SM/sco | double | s | s | — | SCO | AVAILABLE | < 0 → NONE | ≥ −1 | |
| `cars.<id>.pos_{x,y,z}_m` | `scoring.vehScoringInfo[i].mPos` | SM/sco | struct TelemVect3 | m (world) | m | — | SCO | AVAILABLE | none | each abs < 1.0e5 | Track-map dots at scoring rate |
| `cars.<id>.local_vel_{x,y,z}_mps` | `scoring.vehScoringInfo[i].mLocalVel` | SM/sco | struct TelemVect3 | m/s (local) | m/s | — | SCO | AVAILABLE | none | each abs < 150 | Speed = CALC. Also available: `mLocalAccel`, `mOri`, `mLocalRot`, `mLocalRotAccel` (same semantics as §9) |
| `cars.<id>.headlights` | `scoring.vehScoringInfo[i].mHeadlights` | SM/sco | unsigned char | status | bool | — | SCO | AVAILABLE | ≠ 0 → true | 0..255 | |
| `cars.<id>.qualification` / `server_scored` / `in_garage_stall` | `scoring.vehScoringInfo[i].mQualification` / `.mServerScored` / `.mInGarageStall` | SM/sco | long / u8 / bool | — | count / bool / bool | — | SCO | AVAILABLE | as in §14 | as in §14 | |
| `cars.<id>.pit_lap_dist_m` | `scoring.vehScoringInfo[i].mPitLapDist` | SM/sco | float | m | m | — | STATIC | AVAILABLE | none | 0..length | |
| `cars.<id>.best_lap_sector1_s` / `best_lap_sector12_s` | `scoring.vehScoringInfo[i].mBestLapSector1` / `.mBestLapSector2` | SM/sco | float | s | s | — | SCO | AVAILABLE | ≤ 0 → NONE | > 0 | |
| `cars.<id>.drs_state` | `scoring.vehScoringInfo[i].mDRSState` | SM/sco | bool | bool | bool | — | SCO | VEHICLE-DEPENDENT | none | 0/1 | Rear flap state [H] |
| `cars.<id>.attack_mode` | `scoring.vehScoringInfo[i].mAttackMode` | SM/sco | short | — | raw | — | SCO | VEHICLE-DEPENDENT | none | finite | UNIT? Semantics UNVERIFIED. Diagnostic only |
| `cars.<id>.telemetry.*` | `telemetry.telemInfo[j]` where `telemInfo[j].mID == <id>`, `j < min(activeVehicles, 104)` | SM/tel | struct TelemInfoV01 | as §2–§13 | as §2–§13 | — | TEL | VEHICLE-DEPENDENT | as §2–§13 | as §2–§13 | Full telemetry for other cars exists (CLAIMED to cover the whole grid online, e.g. `mVirtualEnergy`). **Not streamed by default** (bandwidth); the engines use VE and fuel for strategy [A-LMU-022] |

## 18. Derived channels (CALCULATED)

Provenance is `DERIVED` unless marked `ESTIMATED`. The formulas live in `race-engines` / `lmu-normalizer` with a formula id and `calculation_engine_version` (D-010).

**Internal body frame.** Our body frame is **ISO 8855** (right-handed: X forward, Y left, Z up). From the LMU local frame (+x left, +y up, +z rear, per the header):
- `X_iso = −z_lmu`, `Y_iso = +x_lmu`, `Z_iso = +y_lmu`.
- For rates, using the header's stated rotation senses (+x pitches up, +y yaws right, +z rolls right): yaw rate `r = −rot.y` (positive = yawing **left**), pitch rate `q = −rot.x` (positive = nose **down**), roll rate `p = +rot.z` (positive = rolling **right**).
- All signs are ASSUMPTION **A-LMU-024** until checked in a known corner. The header's own ISO remark is internally inconsistent and is not used.
- Implemented as named sign constants in `lmu-normalizer::convention` (unit-tested): **positive `dynamics.g_lat` = acceleration towards the car's left; positive `dynamics.g_long` = acceleration (throttle), negative = braking**; positive yaw rate = yawing left. The capability notes of these channels say "sign unverified" until Gate-1.

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `vehicle.speed_mps` | from `telemetry.telemInfo[p].mLocalVel` | CALC | f64 | m/s | m/s | km/h | TEL | AVAILABLE | `sqrt(vx² + vy² + vz²)` | 0..150 | DERIVED. Matches DuckDB "GPS Speed" in m/s [D] |
| `vehicle.speed_forward_mps` | `telemetry.telemInfo[p].mLocalVel.z` | CALC | f64 | m/s | m/s | km/h | TEL | AVAILABLE | `−vz` | −30..150 | Negative when reversing [A-LMU-024] |
| `dynamics.g_long` | `telemetry.telemInfo[p].mLocalAccel.z` | CALC | f64 | m/s² | g | g | TEL | AVAILABLE | `−az / 9.80665` | abs < 6 (SUSPECT above) | **+ = accelerating, − = braking** |
| `dynamics.g_lat` | `telemetry.telemInfo[p].mLocalAccel.x` | CALC | f64 | m/s² | g | g | TEL | AVAILABLE | `+ax / 9.80665` | abs < 6 | **+ = acceleration toward the car's left** (a left-hand corner, ISO). The UI may flip it for a "right = +" display. The DuckDB G Lat sign is also UNVERIFIED [A-LMU-024] |
| `dynamics.g_vert` | `telemetry.telemInfo[p].mLocalAccel.y` | CALC | f64 | m/s² | g | g | TEL | AVAILABLE | `+ay / 9.80665` | abs < 10 | Whether ≈ 0 or ≈ +1 at rest (gravity included?) is UNVERIFIED [A-LMU-032] |
| `dynamics.g_combined` | derived from `g_lat`, `g_long` | CALC | f64 | — | g | g | TEL | AVAILABLE | `sqrt(g_lat² + g_long²)` | 0..6 | Friction-circle plot |
| `dynamics.yaw_rate_radps` | `telemetry.telemInfo[p].mLocalRot.y` | CALC | f64 | rad/s | rad/s | °/s | TEL | AVAILABLE | `r = −rot_y` | abs < 5 | ISO sign: + = yaw left (counter-clockwise from above) [A-LMU-024] |
| `dynamics.pitch_rate_radps` | `telemetry.telemInfo[p].mLocalRot.x` | CALC | f64 | rad/s | rad/s | °/s | TEL | AVAILABLE | `q = −rot_x` | abs < 5 | + = nose down (ISO) |
| `dynamics.roll_rate_radps` | `telemetry.telemInfo[p].mLocalRot.z` | CALC | f64 | rad/s | rad/s | °/s | TEL | AVAILABLE | `p = +rot_z` | abs < 5 | + = roll right (ISO) |
| `vehicle.heading_rad` | `telemetry.telemInfo[p].mOri` | CALC | f64 | — | rad | ° | TEL | AVAILABLE | forward in world `f = (−mOri[0].z, −mOri[1].z, −mOri[2].z)`; `heading = atan2(f.x, f.z)` | −π..π | Map arrow. Zero reference and direction UNVERIFIED [A-LMU-025] |
| `track.map_xy_m` | `telemetry.telemInfo[p].mPos.x`, `.mPos.z` | CALC | f64 × 2 | m | m | px | TEL | AVAILABLE | top view of a left-handed y-up world: `screen_right = +x`, `screen_up = +z` | finite | ASSUMPTION A-LMU-025 (verify on the clockwise Le Mans circuit) |
| `lap.progress_frac` | `scoring.vehScoringInfo[k].mLapDist`, `scoring.scoringInfo.mLapDist` | CALC | f64 | m / m | fraction | % | SCO (ESTIMATED at TEL) | AVAILABLE | `mLapDist / track_length`, clamped to [0, 1), plus `speed_forward × (telemetry ET − scoring mCurrentET) / track_length` (ESTIMATED, at most 1 s, else NONE); wraps to the new lap when the extrapolation crosses the line (A-LMU-059) | 0..1 | Telemetry has no lap distance |
| `lap.current_time_s` | `telemetry.telemInfo[p].mElapsedTime`, `.mLapStartET` | CALC | f64 | s | s | m:ss.000 | TEL | AVAILABLE | `mElapsedTime − mLapStartET` if `mLapStartET ≥ 0` and not after the ET, else NONE | 0..3600 | "no current laptime" in scoring [H]. `mLapStartET` lies after the ET by 0.01 s on a return to the garage and by 42.85 s after the session-end ET rewind (Gate 1 stint1, A-LMU-031): NONE, never negative |
| `race.total_distance_m` | `scoring.vehScoringInfo[k].mTotalLaps`, `.mLapDist`, `scoring.scoringInfo.mLapDist` | CALC | f64 | m | m | km | SCO | AVAILABLE | `mTotalLaps × track_length + mLapDist` | −`track.length_m`..1e8 | Negative before the first crossing (A-LMU-070); no data when the lap distance is below −1 lap |
| `pit.in_pit_lane` | `telemetry.telemInfo[p].mCurrentSector` | CALC | bool | — | bool | — | TEL | AVAILABLE | `((mCurrentSector as u32) >> 31) == 1` | — | Sign bit = pit lane [H] |
| `track.sector_index` | `telemetry.telemInfo[p].mCurrentSector` | CALC | u8 | — | index (0 = S1) | S1..S3 | TEL | AVAILABLE | `mCurrentSector & 0x7FFFFFFF` | 0..2 | Zero-based [H] |
| `track.sector_index_scoring` | `scoring.vehScoringInfo[i].mSector` | CALC | u8 | — | index (0 = S1) | S1..S3 | SCO | AVAILABLE | `(mSector + 2) mod 3` (1 → 0, 2 → 1, 0 → 2) | 0..2 | Same encoding as telemetry after remap. Cross-check both |
| `timing.last_sector2_s` | `scoring.vehScoringInfo[k].mLastSector2`, `.mLastSector1` | CALC | f64 | s | s | s.000 | SCO | AVAILABLE | `mLastSector2 − mLastSector1` if both > 0 | > 0 | S2 alone |
| `timing.last_sector3_s` | `scoring.vehScoringInfo[k].mLastLapTime`, `.mLastSector2` | CALC | f64 | s | s | s.000 | SCO | AVAILABLE | `mLastLapTime − mLastSector2` if both > 0 | > 0 | |
| `timing.best_sector2_s` | `scoring.vehScoringInfo[k].mBestSector2`, `.mBestSector1` | CALC | f64 | s | s | s.000 | SCO | AVAILABLE | `mBestSector2 − mBestSector1` | > 0 | Caution: `mBestSector2` is the best *cumulative* S1 + S2, which may come from a different lap than the best S1, so this is an **approximation** (ESTIMATED). Prefer tracking S2 per lap in `race-engines` |
| `fuel.level_frac` | `telemetry.telemInfo[p].mFuel`, `.mFuelCapacity` | CALC | f64 | — | fraction | % | TEL | AVAILABLE | `mFuel / mFuelCapacity` if capacity > 0 | 0..1.01 | |
| `fuel.used_per_lap_l` | `telemetry.telemInfo[p].mFuel` at lap boundaries | CALC | f64 | L | L/lap | L | EVT (lap) | AVAILABLE | `fuel(lap start) − fuel(lap end)`; excludes laps with refuelling or pit | 0..50 | ESTIMATED (window average) |
| `energy.ve_used_per_lap_frac` | `telemetry.telemInfo[p].mVirtualEnergy` at lap boundaries | CALC | f64 | — | fraction/lap | % | EVT (lap) | VEHICLE-DEPENDENT | difference of the normalised VE at lap boundaries | 0..0.5 | ESTIMATED. Hypercar / LMGT3 only |
| `powertrain.engine_power_w` | `telemetry.telemInfo[p].mEngineTorque`, `.mEngineRPM` | CALC | f64 | N·m, rpm | W | kW / hp | TEL | AVAILABLE | `τ × rpm × 2π / 60` | −1.0e6..1.5e6 | DERIVED. Depends on the torque unit assumption; withheld with the torque (A-LMU-069) |
| `tyres.{w}.temp_surface_inner_c` | `telemetry.telemInfo[p].mWheel[w].mTemperature[..]` | CALC | f64 | K | °C | °C | TEL | AVAILABLE | FL / RL: `[2]`; FR / RR: `[0]`; then − 273.15 | as raw | **ASSUMPTION A-LMU-016** (see §11) |
| `tyres.{w}.temp_surface_outer_c` | `telemetry.telemInfo[p].mWheel[w].mTemperature[..]` | CALC | f64 | K | °C | °C | TEL | AVAILABLE | FL / RL: `[0]`; FR / RR: `[2]`; then − 273.15 | as raw | **ASSUMPTION A-LMU-016** |
| `tyres.{w}.temp_surface_avg_c` | `telemetry.telemInfo[p].mWheel[w].mTemperature[0..3]` | CALC | f64 | K | °C | °C | TEL | AVAILABLE | `mean([0], [1], [2]) − 273.15` | as raw | |
| `tyres.{w}.inner_outer_spread_c` | derived | CALC | f64 | — | °C (difference) | °C | TEL | AVAILABLE | `inner − outer` | abs < 500 | Camber indicator. Inherits A-LMU-016. Gate 1: up to +188 °C on a locked tyre; the inputs are validated individually |
| `wheels.{w}.speed_mps` | `telemetry.telemInfo[p].mWheel[w].mRotation`, `.mStaticUndeflectedRadius` | CALC | f64 | rad/s, cm | m/s | km/h | TEL | AVAILABLE | `abs(mRotation) × radius_cm / 100` | 0..150 | ESTIMATED (static radius, 1 cm resolution, ±3 %). Direction from the sign is UNVERIFIED [A-LMU-039] |
| `tyres.{w}.slip_ratio` | `…mWheel[w].mLongitudinalPatchVel`, `.mLongitudinalGroundVel` | CALC | f64 | m/s | fraction | % | TEL | AVAILABLE | **EXPERIMENTAL:** `mLongitudinalPatchVel / max(abs(mLongitudinalGroundVel), 1.0)` | abs < 2 | Field semantics are UNVERIFIED (identical header comments). **Not shipped until validated live** |
| `tyres.{w}.slip_angle_rad` | `…mWheel[w].mLateralGroundVel`, `.mLongitudinalGroundVel` | CALC | f64 | m/s | rad | ° | TEL | AVAILABLE | **EXPERIMENTAL:** `atan2(mLateralGroundVel, max(abs(mLongitudinalGroundVel), 1.0))` | abs < 0.5 | Same caveat |
| `aids.tc_duty_frac` / `aids.abs_duty_frac` | `telemetry.telemInfo[p].mTCActive` / `.mABSActive` | CALC | f64 | bool | fraction | % | EVT (per lap or corner) | VEHICLE-DEPENDENT | share of TEL samples with the flag set over a window | 0..1 | Stands in for the missing intervention amount |
| `inputs.steering_wheel_angle_rad` | `telemetry.telemInfo[p].mUnfilteredSteering`, `.mPhysicalSteeringWheelRange` | CALC | f64 | −1..1, range | rad | ° | TEL | AVAILABLE | `steer × (range_deg / 2) × π/180` | abs < 12.6 | **ASSUMPTION A-LMU-037**: the range is full lock-to-lock in degrees |
| `session.time_remaining_calc_s` | `scoring.scoringInfo.mEndET`, `.mCurrentET` | CALC | f64 | s | s | h:mm:ss | SCO | AVAILABLE | `max(0, mEndET − mCurrentET)`; no data when `mEndET` is not finite or ≤ 0 (lap-race sentinel UNVERIFIED) | ≥ 0 | Cross-check with `mSessionTimeRemaining` |
| `flags.local_yellow_s{1,2,3}` | `scoring.scoringInfo.mSectorFlag[..]` | CALC | bool | — | bool | flag | SCO | AVAILABLE | `value == 1` (CLAIMED yellow); the index → sector mapping is decided live | — | **Not shipped until the order is verified** [A-LMU-030] |
| `flags.full_course_yellow` | `scoring.scoringInfo.mGamePhase`, `.mYellowFlagState` | CALC | bool | — | bool | flag | SCO | AVAILABLE | `mGamePhase == 6` OR `mYellowFlagState` in 1..=6 | — | [H] |
| `flags.blue` | `scoring.vehScoringInfo[k].mFlag` | CALC | bool | — | bool | flag | SCO | AVAILABLE | `mFlag == 6` | — | [H] |
| `race.class_place` | `scoring.vehScoringInfo[i].mPlace`, `.mVehicleClass` | CALC | u8 | — | count | P# | SCO | AVAILABLE | rank by `mPlace` within the same class string | 1..104 | |
| `cars.<id>.speed_mps` | `scoring.vehScoringInfo[i].mLocalVel` | CALC | f64 | m/s | m/s | km/h | SCO | AVAILABLE | `sqrt(vx² + vy² + vz²)` | 0..150 | Scoring rate only (or TEL via the matching `telemInfo[j]`) |
| `cars.<id>.gap_to_player_s` | `scoring.vehScoringInfo[i].mTimeBehindLeader`, `.mLapsBehindLeader` (player and car) | CALC | f64 | s | s | ±s.000 | SCO | AVAILABLE | difference of `time_behind_leader` when laps are equal; otherwise shown as ±N laps | abs < 3600 | ESTIMATED |
| `tyres.{w}.pressure_delta_pa` | `telemetry.telemInfo[p].mWheel[w].mPressure` | CALC | f64 | kPa | Pa | bar | TEL | AVAILABLE | current − value at the stint start (race-engines `tyre_delta_vs_stint_start`) | abs ≤ 600 kPa | Stint start = tyre engine stint boundary or an ET regression |
| `tyres.{w}.temp_inner_layer_avg_c` | `…mWheel[w].mTireInnerLayerTemperature[0..3]` | CALC | f64 | K | °C | °C | TEL | AVAILABLE | `mean([0], [1], [2]) − 273.15` (`tyre_layer_temp_mean_of_three`) | as raw | |
| `tyres.{w}.temp_surface_delta_c` / `temp_carcass_delta_c` | derived (`tyres.{w}.temp_surface_avg_c`, `.temp_carcass_c`) | CALC | f64 | — | °C (difference) | °C | TEL | AVAILABLE | current − value at the stint start (`tyre_delta_vs_stint_start`) | abs < 500 | |
| `tyres.{w}.temp_surface_peak_c` | derived (`tyres.{w}.temp_surface_avg_c`) | CALC | f64 | — | °C | °C | TEL | AVAILABLE | highest surface average since the stint start (`tyre_surface_avg_peak_since_stint_start`) | as raw | |
| `tyres.{w}.temp_surface_rolling_avg_c` | derived (`tyres.{w}.temp_surface_avg_c`) | CALC | f64 | — | °C | °C | TEL | AVAILABLE | mean over a 10 s ET window (`tyre_surface_avg_time_window_mean`) | as raw | ESTIMATED (window) |
| `tyres.{w}.inner_middle_delta_c` / `middle_outer_delta_c` | derived (inner, centre, outer surface) | CALC | f64 | — | °C (difference) | °C | TEL | AVAILABLE | `inner − middle` / `middle − outer` (`tyre_inner_minus_middle`, `tyre_middle_minus_outer`) | abs < 500 | Inherits A-LMU-016 |
| `tyres.{w}.temp_band` / `pressure_band` / `edge_spread_band` | derived (surface average, pressure, abs(inner − outer)) | CALC | u8 | — | enum | low / ok / high | TEL | AVAILABLE | 0 below, 1 within, 2 above the engineer's thresholds (`tyre_threshold_band`) | 0..2 | No threshold or no value → NONE (there is no universal correct value) |
| `tyres.balance_front_rear_temp_c` / `balance_left_right_temp_c` | derived (four `tyres.{w}.temp_surface_avg_c`) | CALC | f64 | — | °C (difference) | °C | TEL | AVAILABLE | front-axle − rear-axle mean / left-side − right-side mean (`tyre_front_minus_rear_axle_mean`, `tyre_left_minus_right_side_mean`) | abs < 500 | Needs all four wheels |
| `tyres.balance_front_rear_pressure_pa` / `balance_left_right_pressure_pa` | derived (four `tyres.{w}.pressure_pa`) | CALC | f64 | — | Pa (difference) | bar | TEL | AVAILABLE | as the temperature balances | abs ≤ 600 kPa | Needs all four wheels |
| `brakes.{w}.temp_peak_c` | derived (`brakes.{w}.temp_c`) | CALC | f64 | — | °C | °C | TEL | AVAILABLE | highest brake temperature since the stint start (`brake_temp_peak_since_stint_start`) | −50..1500 | Inherits A-LMU-015 |
| `brakes.{w}.temp_band` | derived (`brakes.{w}.temp_c`) | CALC | u8 | — | enum | low / ok / high | TEL | AVAILABLE | 0 below, 1 within, 2 above the engineer's thresholds (`brake_threshold_band`) | 0..2 | No threshold or no value → NONE |
| `brakes.temp_front_minus_rear_c` | derived (four `brakes.{w}.temp_c`) | CALC | f64 | — | °C (difference) | °C | TEL | AVAILABLE | front-axle mean − rear-axle mean (`brake_front_minus_rear_axle_mean`) | abs ≤ 1550 | Deliberately no temperature ratio |
| `brakes.bias_front_frac` | `telemetry.telemInfo[p].mRearBrakeBias` | CALC | f64 | fraction | fraction | % | TEL | AVAILABLE | `1 − mRearBrakeBias` (`brake_bias_front_one_minus_rear`) | 0.2..0.8 | |
| `track.map_lap_dist_m` | `telemetry.telemInfo[p].mPos.x`, `.mPos.z` + generated track map | CALC | f64 | m | m | m | TEL | AVAILABLE | projection on the nearest segment of the validated track map (`track_projection_nearest_segment`) | 0..30 000 | NONE until a map exists for the layout |
| `race.class_gap_ahead_s` / `class_gap_behind_s` | `scoring.vehScoringInfo[i].mTimeBehindLeader`, `.mLapsBehindLeader` (player and class neighbour) | CALC | f64 | s | s | ±s.000 | SCO | AVAILABLE | difference of `time_behind_leader` when on the same lap (`gap_from_time_behind_leader`); race sessions only | ≥ 0 | ESTIMATED |
| `race.class_gap_ahead_laps` / `class_gap_behind_laps` | `scoring.vehScoringInfo[i].mLapsBehindLeader` | CALC | i32 | laps | count | +N L | SCO | AVAILABLE | difference of `laps_behind_leader` (`gap_from_time_behind_leader`) | ≥ 0 | ESTIMATED |
| `race.best_lap_gap_ahead_s` / `best_lap_gap_behind_s` / `class_best_lap_gap_ahead_s` / `class_best_lap_gap_behind_s` | `scoring.vehScoringInfo[i].mBestLapTime` | CALC | f64 | s | s | +s.000 | SCO | AVAILABLE | `back.best − front.best` (`gap_best_lap_difference`); practice, qualifying, warm-up only | ≥ 0 | LMU's classification gaps are road gaps there (A-LMU-034) |
| `flags.summary` | derived (game phase, yellow state, player flag, `mUnderYellow`, finish status) | CALC | u8 | — | enum | flag | SCO | AVAILABLE | most important flag: 4 red, 5 chequered, 2 FCY / SC, 3 blue, 1 local yellow, 0 green (`race_flag_summary_priority`) | 0..5 | NONE without flag information |
| `flags.formation` | `scoring.scoringInfo.mGamePhase`, `scoring.vehScoringInfo[k].mIndividualPhase` | CALC | bool | — | bool | flag | SCO | AVAILABLE | `mGamePhase == 3` or `mIndividualPhase == 3` (`formation_from_game_or_individual_phase`) | — | |
| `source.discontinuity` | `telemetry.telemInfo[p].mPos`, `.mElapsedTime`, `scoring.scoringInfo.mInRealtime`, `.mGamePhase` | CALC | u8 | — | enum | — | TEL | AVAILABLE | 0 none, 1 garage teleport, 2 teleport ET step-back (dropped sample, reported on the next frame), 3 session-end rewind, 4 realtime entry (`discontinuity_teleport_rewind_entry`) | 0..4 | A-LMU-031 / A-LMU-067. Laps with a discontinuity are never bests or references (DEFERRED 19) |
| `source.telemetry_rate_hz` | frame arrival times | CALC | f64 | — | Hz | Hz | 1 Hz | AVAILABLE | count of distinct `mElapsedTime` per second | 0..1000 | Diagnostics (Source::Application) |
| `source.scoring_rate_hz` | frame arrival times | CALC | f64 | — | Hz | Hz | 1 Hz | AVAILABLE | count of distinct `mCurrentET` per second | 0..100 | Diagnostics |

## 19. Channels a race engineer would want that LMU does NOT provide (UNAVAILABLE)

These rows exist so the UI and capability map can say "not provided by LMU" instead of showing 0.

| Internal Name | LMU Field | Source | Type | Raw Unit | Internal Unit | Display Unit | Frequency | Availability | Conversion | Validation | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `brakes.{w}.line_pressure_pa` | — (`mBrakePressure` is 0..1) | — | — | — | Pa | bar | — | UNAVAILABLE | — | — | Header promises kPa "in future". Re-check each LMU update |
| `brakes.{w}.wear_frac` / `thickness_m` | — | — | — | — | — | — | — | UNAVAILABLE | — | — | REST `RepairAndRefuel` `wearables.brakes` (CLAIMED); DuckDB `Brake Thickness` |
| `brakes.{w}.temp_core_c` / `temp_air_c` | — | — | — | — | °C | °C | — | UNAVAILABLE | — | — | DuckDB only (`Brakes Core Temp`, `Brakes Air Temp`) |
| `tyres.{w}.temp_rim_c` | — | — | — | — | °C | °C | — | UNAVAILABLE | — | — | DuckDB only (`TyresRimTemp`) |
| `tyres.{w}.compound_name` | — (axle names only) | — | — | — | text | text | — | UNAVAILABLE | — | — | Use `tyres.front/rear_compound_name` by axle |
| `aids.tc_intervention_frac` / `aids.abs_intervention_frac` | — (only the `mTCActive` / `mABSActive` booleans) | — | — | — | fraction | % | — | UNAVAILABLE | — | — | See the CALC duty-cycle rows |
| `damage.aero_frac`, `damage.suspension_{w}_frac` | — (only `mDentSeverity[8]`) | — | — | — | fraction | % | — | UNAVAILABLE | — | — | REST `wearables` (CLAIMED) |
| `weather.humidity_frac`, `weather.air_pressure_pa`, `weather.air_density` | — | — | — | — | — | — | — | UNAVAILABLE | — | — | Exe strings only; REST forecast has `WNV_HUMIDITY` (CLAIMED) |
| `weather.forecast.*` | — | — | — | — | — | — | — | UNAVAILABLE | — | — | REST `/rest/sessions/weather` |
| `pit.menu.*`, `pit.stop_time_estimate_s`, `tyres.sets_available` | — | — | — | — | — | — | — | UNAVAILABLE | — | — | REST (§9 of the research doc) |
| `chassis.steer_angle_road_wheel_rad` | — | — | — | — | rad | ° | — | UNAVAILABLE | — | — | Only the steering *input* and the wheel range |
| `race.penalty_details` | — (only the count `mNumPenalties`) | — | — | — | — | — | — | UNAVAILABLE | — | — | |
| `race.safety_car_position` | — | — | — | — | — | — | — | UNAVAILABLE | — | — | Only `mGamePhase == 6` |
| `vehicle.gps_lat_deg` / `gps_lon_deg` | — | — | — | — | — | — | — | UNAVAILABLE | — | — | DuckDB only, and synthetic |

## 20. Fields intentionally not mapped

- **Pointers into LMU's address space.** Never dereference:
  - `scoring.scoringInfo.mResultsStream` (char*) and `scoring.scoringInfo.mVehicle` (VehicleScoringInfoV01*);
  - `generic.appInfo.mAppWindow` (HWND).
- **Reserved bytes:** `…mExpansion` in `TelemInfoV01` [20], `TelemWheelV01` [18], `ScoringInfoV01` [187], `VehicleScoringInfoV01` [4] and `ApplicationStateV01` [204]; also `telemetry.telemInfo[p].mUnused[2]`. `lmu-diag` dumps them only when non-zero.
- **Graphics only:** `telemetry.telemInfo[p].mPhysicsToGraphicsOffset[3]`.
- **Window geometry:** `generic.appInfo.mWidth`, `.mHeight`, `.mRefreshRate`, `.mWindowed`. Diagnostic only.
- **Undocumented:** `scoring.vehScoringInfo[i].mUpgradePack[16]` (coded upgrades). Diagnostic only.
- **Personal data** (never transmitted; not stored in recordings by default):
  - `scoring.vehScoringInfo[i].mSteamID` (u64);
  - `scoring.scoringInfo.mPlayerName`, `.mPlrFileName`;
  - `.mServerPublicIP`, `.mServerPort`, `.mIsPasswordProtected`;
  - `paths.customVariables`, `paths.stewardResults`, `paths.playerProfile`, `paths.pluginsFolder`.
- **Results text:** `scoring.scoringStream` / `scoringStreamSize`. Newline-delimited results text (per the `mResultsStream` comment). It is captured raw (clamped to 65,535 bytes) for the recorder; parsing is deferred until its format has been observed live.
