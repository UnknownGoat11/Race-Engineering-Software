# Gate 1 — Real LMU telemetry validation

**Status: PASSED for the data path and the channels listed in §3 (live log), §6 (capture-based
`tools/lmu-verify` report on the 15-minute Hypercar recording) and §7 (LMGT3 race and LMGT3 pit-stop
practice, and all three captures combined).** In the combined run every check passes except the tyre
side mapping (A-LMU-016), which stays INCONCLUSIVE (§7.4). Nothing here relies on synthetic data.

## 1. Session

| Item | Value |
|---|---|
| Date | 2026-10-02 |
| LMU | 1.4.2.0 (Steam build 25513257), `generic.gameVersion` = 14200 |
| Session | Practice, single player, dry; ambient 18.0 °C, track 34.0 °C |
| Car | Hybrid Hypercar (telemetry `mVehicleClass` = 0) |
| LMU settings | Untouched (D-018) |
| Tools | `lmu-diag` release build (`compat`, `capture`, `live --log`), statistics scripts |

Artifacts (kept privately by the owner, never committed — they contain player names):
`gate1-compat-menu.txt`, `gate1-hypercar.lmuraw` (≈ 15 min, 300 MB, started in the garage with cold
brakes, laps and a pit stop with refuel and tyres), `gate1-live.jsonl` (207 s, 10,858 decoded frames,
1–2 laps, then a crash, Esc pause and LMU exit).

## 2. Gate-1 acceptance criteria

| Criterion | Result | Evidence |
|---|---|---|
| LMU running, app connects to the real source | PASS | `compat`: CONNECTED, `sync FRAME_GATE`, `lock SDK_LOCK`, mapping 327,680 bytes, every structural check passed |
| Real telemetry values received | PASS | 10,848 telemetry updates and 547 scoring updates in the live log |
| Values change correctly while driving | PASS | speed up to 78.0 m/s (281 km/h), rpm up to 8,611, gears −1…6, inputs move over their full ranges, lap 7 → 8 with a 1:40.440 lap |
| Units correct | PASS for §3 | see §3 |
| Timestamps correct | PASS | telemetry `mElapsedTime` step 0.010 s (median), `mDeltaTime` = 0.010 s, 0 regressions |
| Update frequency measured | PASS | telemetry median interval 9.99 ms (p5 9.09, p95 10.90) ≈ **100 Hz**; scoring median 200.0 ms ≈ **5 Hz** |
| Invalid / stale detected | PARTIAL | pause and exit produced telemetry gaps of 2.6–39.6 s; the reader's STALLED / WAITING_FOR_LMU transitions are not in the JSONL and are re-checked in the next session |
| Channel provenance documented | PASS (in progress for the rest) | this report, `docs/ASSUMPTIONS.md` statuses |
| Diagnostic logs demonstrate acquisition | PASS | the three artifacts above |

## 3. Verified facts (assumption → evidence)

1. **Layout (A-LMU-007, A-LMU-010).** All structural checks passed at runtime; 14200 is now in
   `VERIFIED_GAME_VERSIONS` (`crates/lmu-reader/src/compat.rs`).
2. **Frame gate and lock (A-LMU-003, A-LMU-004).** `FRAME_GATE` and `LOCKED` consistency in 100 % of
   frames; 0 duplicate-ET telemetry updates in 10,848, so one gate wait = one new physics frame.
3. **Rates (A-LMU-001, A-LMU-002).** Telemetry ≈ 100 Hz and scoring ≈ 5 Hz (see §2).
4. **Event queue (A-LMU-009 — CORRECTED).** Unused slots read **16 (`SME_MAX`)**, not 0. Every telemetry
   publish lists `UPDATE_TELEMETRY` (11) and `FFB` (15); 546 of 547 scoring publishes list
   `UPDATE_SCORING` (10). Quitting LMU produced `EXIT_REALTIME` (9), `END_SESSION` (7),
   `SET_ENVIRONMENT` (14), `UNLOAD` (5), `EXIT` (1), `SHUTDOWN` (3), `UNINIT_APPLICATION` (13) once each.
   The reader now treats 16 as an empty slot (it was reported as "12 unknown codes").
5. **Brake temperature is Kelvin (A-LMU-015).** The cold/reset value was 291.15 on all four wheels =
   18.00 °C ambient + 273.15 exactly; hot values 556–739 K (≈ 283–466 °C).
6. **Tyre temperatures are Kelvin, pressure is kPa (A-LMU-049).** Front surface medians 329–338 K
   (56–65 °C), carcass 346–356 K; pressures 152–193 kPa while running. Water/oil 84/88 °C (°C).
7. **Local axes (A-LMU-024).** Forward is **−z** (`mLocalVel.z` < 0 in 100 % of 2,832 frames above
   10 m/s); accelerating gives **negative** `mLocalAccel.z` (correlation −0.83 with d|v|/dt). In 649
   smoothed cornering samples, sign(steering) = sign(yaw rate `mLocalRot.y`) in 95.7 % and
   `mLocalAccel.x` ≈ −v·yaw in 95.2 %: with steering −1…+1 = left…right (header), a right turn has
   positive yaw rate and negative `mLocalAccel.x`, i.e. +x = left and +y yaws right, as the header says.
   An independent load-transfer check is still to run on the capture (§5).
8. **Vertical acceleration excludes gravity (A-LMU-032).** At rest `mLocalAccel.y` median ≈ 0
   (p5 −0.017, p95 0.021 m/s²).
9. **Wheel rotation sign (A-LMU-039).** rotation × radius ÷ speed ≈ −1.0 (medians −1.007 … −1.055):
   negative when rolling forward; |rotation| × radius = surface speed.
10. **Fuel and scoring fuel fraction (A-LMU-018, A-LMU-044).** On this Hypercar scoring `mFuelFraction`
    is fuel on a 0–255 scale: 156/255 = 0.612 vs `mFuel`/`mFuelCapacity` = 67.4/110 = 0.613, and
    188/255 = 0.737 vs 81/110 = 0.736.
11. **Hybrid fields (A-LMU-013, A-LMU-014, A-LMU-038).** `mVirtualEnergy` 0.828–1.000 (fraction);
    `mBatteryChargeFraction` 0.017–1.000 (fraction); `mSoC` = 100 × battery fraction (1.666 vs 0.0167,
    7.635 vs 0.0764), so **percent**; motor state codes 0–3 seen; motor temperature 34.8–41.1 (°C).
12. **Steering wheel range (A-LMU-037).** `mPhysicalSteeringWheelRange` = **0** on this setup (not set),
    so the steering-wheel angle stays Unavailable (normalizer change of 2026-10-02);
    `mVisualSteeringWheelRange` = 719 (≈ 720° lock to lock).
13. **Aids (A-LMU-058).** TC 7/11; ABS max 0 (this car has no ABS).
14. **Pit lane.** `mCurrentSector` sign bit set for 32 frames; scoring `mInPits` true and pit state 5 for
    3 updates.

## 4. New behaviour found (handled in lmu-normalizer, 2026-10-05)

- **Damaged car.** After a crash, FR `mDetached` was true for 10,291 frames (its pressure and carcass
  temperature frozen) and FL `mFlat` true for 7,993 frames (FL pressure 0 while flat). → A-LMU-061: a
  detached wheel's tyre and wheel-motion channels are `UNAVAILABLE`; a flat tyre's 0 kPa is still published.
- **Reset frames.** 32 frames had 0 K tyre temperatures, 0 kPa on all wheels, engine temperatures 0, `mTCMax`
  0 and the pit-lane bit set (a garage/reset transition). → A-LMU-062: those readings are `UNAVAILABLE`, not
  values and not rejections.
- **Surface type 30** appeared (outside the documented enum); it is kept as `Unknown(30)`.
- **Brake temperature unit.** Kelvin is applied from the first frame for verified versions
  (`VERIFIED_BRAKE_TEMP_UNITS`: 14200); any evidence against it withholds the values again (conflict).
- **Sign notes.** The capability notes of `g_lat`, `g_long`, `g_vert` and `yaw_rate` now say "verified".

## 5. Still to verify from the capture (tools/lmu-verify) or a later session

Settled by the capture (§6): load-transfer confirmation of the lateral sign, wear direction (A-LMU-040),
sector semantics (A-LMU-026), lap and sector times, virtual-energy use per lap. Still open: tyre L/C/R →
inner/outer mapping (A-LMU-016), `mDeltaBest` sign (A-LMU-033), refuel and tyre change **at a pit-box stop**
(the capture only has garage returns), LMU exit detection and frozen-data handling (A-LMU-056), LMGT3 and
LMP2 sessions for A-LMU-012.

## 6. Capture-based verification (tools/lmu-verify, 2026-10-12, applied 2026-10-13)

`lmu-verify report gate1-hypercar.lmuraw` (LMU 1.4.2.0, gameVersion 14200, Peugeot Hypercar, Monza,
Practice 1, solo; 900 s, 87,109 snapshots, 86,895 published frames, every frame COMPATIBLE). The capture
is replayed through `lmu-reader` and `lmu-normalizer` exactly as the live app does. The reports (names
redacted) are kept outside the repository with the capture.

### 6.1 Verdicts

| Check | Verdict | Evidence | Assumptions |
|---|---|---|---|
| Native rates | PASS | telemetry 100.0 Hz, scoring 5.00 Hz (median intervals) | A-LMU-001, -002 |
| ET monotonic, one clock | PASS | 0 regressions; telemetry − scoring ET p99 0.003 s | A-LMU-031, -059 |
| Brake temperature unit | PASS | coldest − ambient median 276.8 over 2,601 cold samples (Kelvin); hot p99 625 °C | A-LMU-015 |
| Tyre temperature unit | PASS | running raw ≥ 289.5 K, wheel medians 46–61 °C; published = raw − 273.15 | A-LMU-049, -062 |
| Tyre pressure unit | PASS | running medians 135–139 kPa; published = raw × 1000 Pa | A-LMU-049 |
| Brake pressure fraction | INCONCLUSIVE | 0..0.537; at full pedal front ÷ (1 − bias) p99 0.83, rear ÷ bias p99 0.95; r = 0.77 with the pedal (hybrid regeneration blending suspected) | A-LMU-017 |
| Virtual energy 0..1 | PASS | 0.839..1.000; 3.6 % of the allowance per complete lap (4 laps) | A-LMU-013 |
| State of charge scale | PASS | `mSoC` = 100 × `mBatteryChargeFraction` (median ratio 1.0000): percent | A-LMU-014 |
| Longitudinal G sign | PASS | r = 0.97 with d(speed)/dt; +0.36 g at full throttle, −1.48 g braking | A-LMU-024 |
| Lateral G sign | PASS | agrees with yaw rate in 99.1 %, with load transfer in 96.9 % | A-LMU-024 |
| Steering sign | PASS | steering right ⇔ yawing right in 95.6 % | A-LMU-024 |
| Wheel speed / rotation sign | PASS | rotation < 0 rolling forward; wheel ÷ body speed 1.010–1.056 | A-LMU-039, -060 |
| Lap distance | PASS | 6 wraps = 6 on-track lap increments | channel map |
| Sectors and laps | PASS | (`mSector` + 2) mod 3 = telemetry sector in 100 %; lap and S1/S2 times match 100 % | A-LMU-026, -043 |
| Fuel | PASS | 18.6 L used driving, never rising on track; one refill (garage stall) | A-LMU-018 |
| Tyre wear direction | PASS | falls while driving (100 % of the change): 1.0 = new tyre | A-LMU-040 |
| Pit flags | INCONCLUSIVE | pit-lane bit = `mInPits` in 100 %; no pit-box service stop in the capture | A-LMU-029, -026 |
| Tyre side mapping | INCONCLUSIVE | inside tyres: inner edge warms 4/4; outside tyres: outer edge warms on only 2/4 | A-LMU-016 |
| `mPitState` values | FAIL at the time; PASS since §7 | value **5** (undocumented) in 415 scoring updates: garage stall and the drive out of it. The tool then compared against the header's 0..4; it now judges the observed 0..5 semantics (§7.2) | A-LMU-063 |

### 6.2 Findings applied to telemetry-model and lmu-normalizer (2026-10-13)

The first replay published 12,017 samples INVALID and withheld virtual energy for 31.8 % of the frames.
After the changes below the same replay publishes **14** samples INVALID (crash-impact and teleport
artefacts that are rightly rejected: three 66–83 g spikes with their combined-g and local-acceleration
twins, one 547 rad/s² spike, three pushrod-force spikes, one −0.014 s lap time at the garage teleport) and
virtual energy is VALID in 100 % of the frames.

- **Registry ranges** (real values; physical bounds with margin; the evidence is cited in `registry.rs`):
  `pit.state` 0..5 (`PitState::Garage`, A-LMU-063); `hybrid.motor_state` 0..4
  (`HybridMotorState::GarageRelease`: 4 for 1.64 s after each garage release, A-LMU-064); toe ±40° (the
  front toe includes the steering angle: ±0.35 rad seen, A-LMU-068); clutch speed ±30,000 rpm (−4,259 rpm
  sliding backwards in a spin); hybrid motor torque ±10,000 N·m (−3,676 N·m regenerating); tyre
  inner − outer spread ±500 °C (+188 °C on a locked tyre); per-wheel ride height up to 2 m (0.90 m with a
  wheel off the ground in a crash); tyre surface temperature rate 15,000 °C/s (sustained rises of
  4,950 °C/s on all three nodes in lock-ups at 40–60 m/s had been rejected as jumps).
- **Channels LMU does not fill** (A-LMU-065): `mFrontRideHeight`, `mRearRideHeight` and `mFrontWingHeight`
  read exactly 0, `mDrag` and both downforces exactly 0, `mTurboBoostPressure` exactly 101,325 Pa all
  session. A 0 m ride or wing height is impossible even at rest; 0 N of aero force or exactly ISA boost is
  impossible once the car moves; any value still bit-identical to its first reading after 200 m of driving
  is not a measurement. These are now UNAVAILABLE (notice `ChannelNotFilled`, capability note) and become
  VALID the moment LMU writes a different value. Constant channels that are real stay VALID: clutch pedal
  0 (paddle shift), rear brake bias (a setup value), `mDeltaTime` 0.01 s.
- **Gaps in a solo session** (A-LMU-066): the four `mTimeGap*` read exactly 0 with one car in the session;
  there is no car to measure a gap to, so they stay at "no data" (UNAVAILABLE) instead of a VALID 0 s. With
  other cars the raw gaps are published unchanged (multi-car semantics: A-LMU-034).
- **Virtual energy scale** (A-LMU-013): 0..1 is applied from the first frame for gameVersion 14200
  (`VERIFIED_VIRTUAL_ENERGY_SCALES`), like the verified brake unit; a value above 1.5 is then a conflict and
  withholds the channel (UNAVAILABLE) for the session. The `mSoC` scale is still decided from evidence.
- **Discontinuities** (A-LMU-067): the garage release (9.2 m in 12.5 ms) and the return to the garage (652 m
  in 7.5 ms with refuel, brake reset, new tyres and an energy refill) were rejected sample by sample by the
  rate guard. A teleport (step > 5 m and faster than 200 m/s, the car at rest ≤ 2 m/s afterwards, in the
  pit area) or a realtime entry now resets every rate history; standing still (≤ 1 m/s) in the pit area
  resets the fuel, virtual-energy and tyre channels (pit service). Uncorroborated jumps (on track, or still
  moving) are still rejected. After the change the replay has 0 rate rejections.

### 6.3 Still needed (next capture session)

- A **pit-box stop** with refuel and tyre change (pit flags, A-LMU-029; service steps in the pit box rather
  than in the garage, A-LMU-067).
- **Long corners in both directions** at steady speed (tyre side mapping, A-LMU-016).
- A **non-hybrid car** (LMGT3 / LMP2): virtual energy without hybrid (A-LMU-012), and whether `mDrag`,
  downforce, ride heights and turbo boost are filled on other cars (A-LMU-065).
- Brake pressure with regeneration off or on a non-hybrid car (A-LMU-017).
- A multi-car session for the gap semantics (A-LMU-034, A-LMU-066).

## 7. Second and third capture: LMGT3 race and LMGT3 pit stops (2026-10-13)

Two more recordings, replayed exactly like §6 (reports with names redacted kept outside the repository):

- **race1** — LMU 1.4.2.0 (gameVersion 14200), RACE, Sebring (5,820 m), LMGT3, 20 cars (AI), 23.5 min,
  standing grid start with a formation lap, 10 racing laps, no pit stop, ends with a return to the garage.
- **pitstop1** — PRACTICE, Laguna Seca (3,590 m), LMGT3, solo, 413 s: two real **pit-box stops** (one with
  +90.6 L of fuel, one with a tyre change), the session running out with the car in the box (the car is moved
  to its garage), a session restart and a garage exit.

### 7.1 Findings and what changed

| Finding (evidence) | Change | Assumption |
|---|---|---|
| Lap distance negative before the first line crossing: −1,586.8 m in the garage, −1,023.4 m on the grid, −272 m leaving the garage; 4.1 % of race samples rejected | `track.lap_dist_m` / `race.total_distance_m` accept down to −1 track length (below → INVALID); lap progress uses the distance modulo the track length | A-LMU-070 |
| The 11th lap-distance "wrap" (11 vs 10 lap increments): LMU re-bases the pre-start distance across the line, −1,023.4 → +4,796.9 m at the formation start and +5,726 → −87.4 m at the green flag, without counting a lap | lmu-verify counts start re-bases separately; 10 wraps / 10 increments, all paired | A-LMU-070 |
| Gaps negative in 98 % of samples: LMU signs the car/place **ahead** negative (`mTimeGapPlaceAhead` = −`mTimeBehindNext` within 0.02 s; car-ahead gap = lap-distance difference ÷ speed), the one behind positive | Published as magnitudes (ahead = −raw); a wrong sign ≤ 0.05 s = alongside → 0 s; a larger wrong sign → INVALID (1,065 samples in the formation lap, 453 during overtakes when the car classified ahead is briefly behind on track); place gaps NONE for P1 / the last car | A-LMU-034 |
| `mEndET` = −2,147,483,648 (`INT32_MIN`) during the formation lap of the timed race (end time not set yet) | Published as no data (UNAVAILABLE), not INVALID | A-LMU-043 |
| `mPitState` at real stops: 0 → 1 (request, on track) → 2 (entering, pit lane) → **4 while stopped in the box** (fuel 15.4 → 106.0 L during 4) → **5 while leaving the box** → 0 at the pit exit; 3 never; 5 also in the garage | `PitState`: 3 `HeaderStopped`, 4 `Stopped`, 5 `Exiting` (the uncommitted `Garage` variant removed); range 0..5 | A-LMU-029, A-LMU-063 |
| LMGT3, race **and** practice: tyre load, lateral / longitudinal force, sliding fraction and engine torque exactly 0 all session (the Hypercar filled them) ⇒ class-dependent, not session-dependent | Same fill detector as A-LMU-065: UNAVAILABLE while unchanged (an unchanged bit-exact 0 on a moving car at once, any unchanged value after 200 m), engine power withheld with the torque, VALID from the first change. The clutch pedal (also 0) stays VALID: a 0 pedal is a real reading | A-LMU-069 |
| Camber ±0.36..0.50 rad in kerb strikes and an impact (4 samples per wheel) | Camber range ±0.6 rad | channel map |
| Session ran out with the car in the box: moved 19.6 m into the garage in 27 ms, fuel and lap counter reset, and ET published 315.830 before 315.820 | lmu-verify: a ≤ 1 s ET step back within 0.5 s of a teleport is explained; a lap ended at a discontinuity is partial (it had been judged as using 79 % VE) | A-LMU-067 |

Replay result: race1 INVALID samples 305,298 → 1,538 (all wrong-sign place gaps, above); pitstop1 16,826 → 2
(the −0.013 s lap time at the teleport); the Hypercar unchanged at 14.

### 7.2 Check logic changes in tools/lmu-verify (no threshold weakened)

- `g_lat_sign`: four tyre loads of exactly 0 N (or unavailable) give no load-transfer evidence ("n/a"); the
  verdict then rests on the yaw evidence (99.8 % race, 99.4 % pit-stop practice; g_lat ÷ v·yaw/g0 median 0.96).
  The load-transfer agreement stays from the Hypercar (96.9 %).
- `pit_flags`: the pit-state rule is judged only in the pit lane and box (not in the garage stall, where LMU
  reports 0 at a session start and 5 on a return) and only when a pit-box stop happened; every serviced box
  stop must show state 4. Without a box stop the verdict is INCONCLUSIVE.
- `pit_state_values`: values must be within the observed 0..5 and the pit-lane states 2..5 must not appear on
  track (beyond the 0.5 s flag guard).
- `lap_distance`: a negative distance is allowed before the first crossing and in the pit area (down to −1
  lap); start re-bases are not line crossings; garage teleports are not steps back.
- `elapsed_time` / `virtual_energy`: see the last row of §7.1.

### 7.3 Verdicts (lmu-verify v4)

| Check | Hypercar (§6) | race1 | pitstop1 | Combined (all three) |
|---|---|---|---|---|
| Native rates | PASS | PASS (99.8 / 5.00 Hz) | PASS (100.0 / 5.00 Hz) | PASS |
| ET monotonic, one clock | PASS | PASS | PASS (1 regression explained by the teleport, 1 session restart) | PASS |
| Brake temperature unit | PASS | PASS (cold − ambient 273.15) | PASS | PASS |
| Tyre temperature unit | PASS | PASS | PASS | PASS |
| Tyre pressure unit | PASS | PASS | PASS | PASS |
| Brake pressure fraction | INCONCLUSIVE | **PASS** (front ÷ (1 − bias) 1.000, rear ÷ bias 1.000, r = 0.963) | PASS (0.942 / 0.967, r = 0.873) | PASS |
| Virtual energy 0..1 | PASS | PASS (4.36 %/lap, 8 laps) | INCONCLUSIVE (no complete lap) | PASS |
| State of charge | PASS | INCONCLUSIVE (no hybrid) | INCONCLUSIVE (no hybrid) | PASS |
| Longitudinal G sign | PASS | PASS | PASS | PASS |
| Lateral G sign | PASS | PASS (yaw 99.8 %, load n/a) | PASS (yaw 99.4 %, load n/a) | PASS |
| Steering sign | PASS | PASS | PASS | PASS |
| Wheel speed / rotation sign | PASS | PASS | PASS | PASS |
| Lap distance | PASS | PASS (10/10 paired, 2 start re-bases) | PASS | PASS |
| Sectors and laps | PASS | PASS | INCONCLUSIVE (no timed lap) | PASS |
| Fuel | PASS | INCONCLUSIVE (no refill) | PASS (pit-box +90.6 L, garage refill) | PASS |
| Pit flags | INCONCLUSIVE | INCONCLUSIVE (no box stop) | **PASS** (bit = mInPits 100 %, 2 box stops showing 4, states {2, 4, 5}) | PASS |
| `mPitState` values | PASS | PASS | PASS | PASS |
| Tyre side mapping | INCONCLUSIVE | INCONCLUSIVE | INCONCLUSIVE | INCONCLUSIVE |
| Tyre wear direction | PASS | PASS | PASS | PASS |

Before these changes race1 had four FAILs (`g_lat_sign`, `lap_distance`, `pit_flags`, `pit_state_values`) and
pitstop1 five (`elapsed_time`, `virtual_energy`, `g_lat_sign`, `lap_distance`, `pit_state_values`); each was a
tool or model fault explained above, not an LMU contradiction. No check that passed before regressed.

### 7.4 Settled and still open

- **Settled:** brake pressure = pedal × brake balance (A-LMU-017; the Hypercar's lower peaks fit hybrid
  regeneration blending); VE is a 0..1 fraction on LMGT3 too, 4.36 %/lap at Sebring (A-LMU-012, A-LMU-013);
  class LMGT3 (telemetry) / GT3 (scoring); pit states (A-LMU-029); gap sign (A-LMU-034); fuel refills in the box
  (A-LMU-018).
- **Still open:** tyre side mapping (A-LMU-016): unloaded inside tyres warm the inner edge 4/4, loaded outside
  tyres the outer edge only 3/4 (FL Δ 0.70; 2/4 at Laguna Seca), so the rule (4/4) is not met; `mDeltaBest`
  sign (A-LMU-033); LMU exit and frozen data (A-LMU-056); LMP2 / LMP3 (A-LMU-012); whether the LMGT3 zero
  channels are filled on other GT3 cars (A-LMU-069); state of charge on a non-hybrid is n/a.