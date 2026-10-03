# Gate 1 — Real LMU telemetry validation

**Status: PASSED for the data path and the channels listed in §3, from the live log.** The
capture-based report (`tools/lmu-verify` on the 15-minute recording) is still to be added; the items
it must settle are listed in §5. Nothing here relies on synthetic data.

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

## 4. New behaviour found (needs code)

- **Damaged car.** After a crash, FR `mDetached` was true for 10,291 frames and FL `mFlat` true for
  7,993 frames (FL pressure 0 while flat). Channels of a detached wheel must be Unavailable, not 0.
- **Reset frames.** 32 frames had pressure 0 on all wheels, engine temperatures 0, `mTCMax` 0 and the
  pit-lane bit set (a garage/reset transition). These must not be published as valid values.
- **Surface type 30** appeared (outside the documented enum); it is kept as `Unknown(30)`.
- **Brake temperature unit.** Now verified as Kelvin for 14200, so the normalizer can use Kelvin for
  verified versions instead of withholding until its runtime detector decides.

## 5. Still to verify from the capture (tools/lmu-verify) or a later session

Load-transfer confirmation of the lateral sign; tyre L/C/R → inner/middle/outer mapping (A-LMU-016);
wear direction (A-LMU-040); `mDeltaBest` sign (A-LMU-033); sector semantics (A-LMU-026); lap and sector
times against the timing screen; refuel and tyre change at the pit stop; virtual-energy use per lap; LMU
exit detection and frozen-data handling (A-LMU-056); LMGT3 and LMP2 sessions for A-LMU-012.
