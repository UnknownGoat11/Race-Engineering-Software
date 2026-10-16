# lmu-verify

Offline Gate-1 evidence tool. It turns real Le Mans Ultimate captures into a reproducible verification
report, so channel units, signs, rates and provenance are decided from evidence (docs/ASSUMPTIONS.md
`A-LMU-*`, docs/LMU_CHANNEL_MAP.md), not from guesses.

**Read-only toward LMU (D-018).** `lmu-verify report` only reads `.lmuraw` files written by
`lmu-diag capture`; it never touches LMU, its install folder, its UserData or its process. The optional
`rest-probe` sends one-shot HTTP `GET` requests to `127.0.0.1:6397`, and only when you run it.

## Build

```powershell
cargo build -p lmu-verify --release
```

## Gate-1 commands

1. **Record** (LMU running, settings untouched; see tools/lmu-diag/README.md for the full session). Start
   the capture **in the garage before driving**, so the brakes are cold, then drive at least 3 laps
   including a pit stop with refuel and new tyres, and both left and right corners:

   ```powershell
   .\target\release\lmu-diag.exe capture --seconds 900 --out gate1-hypercar.lmuraw
   ```

2. **Verify** (LMU can be closed; this step never needs it):

   ```powershell
   .\target\release\lmu-verify.exe report gate1-hypercar.lmuraw --out gate1-report
   ```

   Several captures (for example one per car class) go in one report:
   `lmu-verify report a.lmuraw b.lmuraw c.lmuraw --out gate1-report`.
   `--json` prints `report.json` to stdout instead of the short summary.

The command writes `gate1-report/report.md` and `gate1-report/report.json` and prints the check table.
It exits with 0 when the report was written (whatever the verdicts), 1 when the output cannot be
written. A 300 MB, 15-minute capture takes about 15 s in a release build.

**Privacy.** Captures hold driver, player and server names. The report never prints them: every string
taken from a capture is redacted to `<name>`, and file names are printed without their directories.
Keep the captures themselves private, as with lmu-diag.

## Reading the report

`report.md` (and the same data in `report.json`) has, in this order:

1. **Gate-1 checks**: one row per check with its verdict over all captures (any FAIL fails; otherwise
   any PASS passes), the assumption ids it settles and the decisive numbers.
   - **PASS**: the evidence supports the documented assumption or convention.
   - **FAIL**: the evidence contradicts it; investigate before relying on the channel.
   - **INCONCLUSIVE**: not enough (or not clear enough) evidence. The detail says what was missing.
2. **Decisions this evidence supports**: which assumptions and channel-map rows can move to VERIFIED, which
   are contradicted and which stay open. A PASS names only the `gameVersion`s of the captures that passed
   that check, each counted only when one positive version covers ≥ 90 % of its snapshots (a capture
   with mixed or unknown versions is not counted; a newer build whose capture was INCONCLUSIVE is never
   named). A check lists a published channel-map row only when it compares that row with its raw source
   (exact within float tolerance), so a mapping fault there fails the check. Then the registry ranges that
   real values fell outside of, and the continuous channels that never changed while the car moved
   (candidates for UNAVAILABLE / NOT_SUPPORTED, or a mapping fault). The tool only prints suggestions;
   it never edits docs.
3. **Per capture**:
   - *Session facts*: game version, layout fingerprint and compatibility verdict, reader sync and lock
     modes, player identification, track / car / session, ambient and track temperature, event codes
     (undocumented values counted under `Other`, the first 16 distinct ones listed), normalizer notices.
   - *Rates & timing*: telemetry and scoring update intervals on the capture clock (median, p5, p95,
     max), gaps above 3× the median, ET monotonicity (regressions and their explanation), duplicate
     frames, telemetry-vs-scoring ET and ET-vs-capture-clock drift.
   - *Gate-1 checks* with every evidence line.
   - *Per-channel table* for every registry channel the normalizer published: source / provenance,
     share of each quality, distinct-value update rate, min / p1 / median / p99 / max, rejections by
     reason, values outside the registry range, longest constant run while moving and the frozen flag.
   - *Raw NaN / ±Inf census* of the raw LMU fields.

### What each check needs from the capture

| Check | Settles | Needs |
|---|---|---|
| `update_rates` | A-LMU-001, 002 | ≥ 10 s of telemetry and ≥ 50 scoring updates |
| `elapsed_time` | A-LMU-031, 059 | as above; any unexplained ET regression fails. Explained: a restart below 10 s (new session), an identity change within 2 s, a garage-teleport ET step-back (≤ 0.05 s, `TELEPORT_ET_STEP_BACK_MAX_S`) that the normalizer itself dropped (disposition DUPLICATE_ET with `teleport_et_step_backs` incremented, A-LMU-031 kind (b)), or a session-end rewind (session over, out of realtime, new ET ≥ `mEndET`: LMU closing a finished session, A-LMU-031; the normalizer's rule, applied to player and scoring ET) |
| `brake_temp_unit` | A-LMU-015 | ≥ 10 cold samples in the garage before driving (coldest − ambient ≈ 273 K ⇒ Kelvin) |
| `tyre_temperature_unit` | A-LMU-049, 062 | ≥ 200 samples per wheel above 10 m/s |
| `tyre_pressure_unit` | A-LMU-049 | ≥ 200 samples per wheel above 10 m/s |
| `brake_pressure_fraction` | A-LMU-017 | hard braking; ≥ 50 full-pedal samples while rolling (peak = pedal × balance) |
| `virtual_energy` | A-LMU-013 | a car with virtual energy and ≥ 2 complete laps. A lap is complete only when it starts and ends at an on-track line crossing with nothing but driving in between: the lap the capture starts in, the lap after a discontinuity, the out-lap from the garage, laps containing a garage teleport (the normalizer's teleport or teleport ET step-back, or a jump > 50 m), a VE refill or a VE reset (a drop > 0.05 in one sample), laps ended or started by a teleport, and laps ended within 2 s before a discontinuity (session end with the car in the box) are listed as partial and never judged |
| `state_of_charge` | A-LMU-014 | a hybrid with the battery in use, ≥ 5 s |
| `g_long_sign`, `g_lat_sign`, `steering_sign` | A-LMU-024 | acceleration, hard braking and clear cornering both ways. `g_lat_sign` cross-checks the load transfer only when tyre loads are filled (four loads of exactly 0 N on LMGT3 are "n/a", A-LMU-069); the yaw evidence then decides |
| `wheel_speed` | A-LMU-039, 060 | ≥ 100 straight, steady-speed samples per wheel |
| `lap_distance` | channel map, A-LMU-070 | at least one lap completed on track. A negative distance (down to −1 lap) is allowed before the first line crossing and in the pit area; LMU's start re-bases (±1 lap at the formation start and the green flag, no lap counted) are listed and not counted as wraps |
| `sectors_and_laps` | A-LMU-026, 043 | ≥ 10 s on track and completed sectors / laps |
| `fuel` | A-LMU-018 | driving plus a refuel |
| `pit_flags`, `pit_state_values` | A-LMU-029, 026, 063 | a pit-lane entry and exit and a real pit-box stop (not a garage visit): without one `pit_flags` is INCONCLUSIVE. Judged against the states LMU 1.4.2.0 was seen to use: 2 entering, **4 stopped in the box**, **5 leaving the box / garage** (the header's 3 never appeared); `mPitState` ≠ 0 is required in the pit lane outside the garage stall and every serviced box stop must show 4. `pit_state_values` fails on a value above 5 or on pit-lane states (2..5) seen on track |
| `tyre_side_mapping` | A-LMU-016 | ≥ 3 s of sustained cornering (> 1 g) each way per wheel; INCONCLUSIVE unless all four wheels agree |
| `tyre_wear_direction` | A-LMU-040 | ≥ 5 s of driving with measurable wear. PASS only for the documented direction (≥ 95 % of the change downwards: 1.0 = new, falling); a consistent rise FAILs as a contradiction of A-LMU-040 |

## Robustness and memory

- A truncated or corrupt capture never aborts the run: the replay stops at the first bad frame record,
  everything before it is analysed, and the report says where and why it stopped (`STOPPED EARLY
  (TRUNCATED) at frame N after M snapshots`). A missing or non-capture file is reported the same way.
- Memory is bounded for captures of any length: every statistic is streaming (`src/stats.rs`).
  Percentiles are exact while a series has at most 1,024 distinct values (every integer and enum
  channel) and P² estimates (Jain & Chlamtac 1985, five markers per quantile) otherwise; update
  intervals use log-linear histograms (exact below 1 µs, ≤ 0.2 % bucket width above). Lists in the
  report (gaps, stops, regressions, examples) are capped.
- Output is deterministic: fixed section and check order, registry order for channels, sorted maps,
  no wall-clock timestamps. The same captures give byte-identical files.

## REST probe (opt-in)

```powershell
.\target\release\lmu-verify.exe rest-probe --out gate1-rest [--skip <endpoint>]... [--include-pit-menu] [--include-crash-suspects] [--spacing-ms 500] [--keep-raw-bodies]
```

Only when you run it, and only while LMU is running, it requests each GET endpoint of
docs/LMU_TELEMETRY_RESEARCH.md §9 **once** on `http://127.0.0.1:6397`:

- **Loopback only, by construction**: the target is always `127.0.0.1`; there is no option to change it.
- **GET only**: the client has a single request builder with the `GET` method; no other method or body
  is ever sent.
- 1 s timeout per request; at least `--spacing-ms` (default 500, minimum 200) between requests.
- `/rest/garage/UIScreen/RepairAndRefuel` is **skipped** unless `--include-pit-menu` (polling it can make
  the in-game pit menu flicker).
- The **crash suspects** `/rest/garage/getVehicleCondition`, `/rest/garage/UIScreen/TireManagement`,
  `/rest/garage/getPlayerGarageData` and `/rest/garage/PitMenu/receivePitMenu` are **skipped** unless
  `--include-crash-suspects`: on 2026-10-13 they timed out (1 s) or reset the connection right before LMU
  crashed (research §9.1). Not recommended.
- `--skip <endpoint>` (exact path, repeatable) leaves any other endpoint out.
- Each response is saved to `--out` (bodies capped at 2 MiB, also when sent in many small chunks) with
  `index.json` holding status, latency, a header subset and a shape summary (key paths, JSON types, array
  lengths, example values, with name-like values redacted). A table is printed.
- **Privacy.** Standings and session bodies hold driver, player and server names. By default a JSON body
  is saved pretty-printed with every string under a name-like key (`driver`, `player`, `name`, `user`,
  `steam`, `server`, `nick`, `email`, and everything below such a key) replaced by `<name>`; a non-JSON
  body cannot be redacted and is not saved (`index.json` says so). **Object keys** are redacted too (also in
  the `index.json` shape paths): a key that is not schema-like (anything but ASCII letters, digits, `_`, `-`,
  `.`, or digits only) or that equals / contains a name seen as a value anywhere in the probe becomes
  `<name>`, `<name>#2`, ... `/rest/strategy/usage` is keyed by the player name, so every top-level key of
  its body is replaced unconditionally, even when no standings were fetched. `--keep-raw-bodies` saves
  every body verbatim for local inspection: do not share that folder.

## Tests

`cargo test -p lmu-verify` drives every check through PASS, FAIL and INCONCLUSIVE with synthetic
captures built with lmu-reader's test builder (a kinematic car on a 1.2 km synthetic track, each fault
breaking one convention), checks truncated / corrupt / missing captures and deterministic, redacted
output, and runs the REST probe against an in-process HTTP server on 127.0.0.1. The synthetic data
exists only in the tests.
