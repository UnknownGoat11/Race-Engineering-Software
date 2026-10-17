# lmu-diag

Read-only diagnostic viewer for Le Mans Ultimate's native `LMU_Data` shared memory. It is the
Gate-1 tool: it shows what `crates/lmu-reader` decodes from a running LMU, so we can confirm the
layout, the units and the update rates before the rest of the pipeline relies on them.

## What it touches (D-018)

`lmu-diag` never launches, configures or writes to LMU or its files. It only:

- maps `LMU_Data` with `FILE_MAP_READ`;
- waits on `LMU_Data_HoldEvent` / `LMU_Data_DataEvent` (opened with `SYNCHRONIZE` only);
- takes the SDK reader lock (`LMU_SharedMemoryLockData` + `LMU_SharedMemoryLockEvent`) exactly as the
  SDK documents, only if LMU already created it, and only for the copy (about 1-2 µs). It never
  creates or resets the lock;
- finds `Le Mans Ultimate.exe` in the process list and opens it with `SYNCHRONIZE` only, to notice
  when it exits.

## Build

From the repository root, in PowerShell:

```powershell
cargo build -p lmu-diag --release
```

The binary is `target\release\lmu-diag.exe`. Run it **at the same elevation as LMU**: normally both
not elevated (A-LMU-011).

## Commands

| Command | Purpose |
|---|---|
| `lmu-diag compat [--wait-secs 10] [--json]` | Attach once and print the compatibility report. Exit code 0 = decodable, 2 = incompatible, 3 = LMU not running / nothing checked. |
| `lmu-diag live [--log decoded.jsonl] [--seconds N] [--refresh-ms 250] [--no-clear]` | Refreshing view (every 250 ms) of the reader state, rates, lock timing and decoded values. `--log` writes every decoded frame to a JSONL file. Quit with Ctrl+C (or close the window): the reader finishes the copy in progress, releases the LMU lock and the log is flushed before exit. |
| `lmu-diag capture --seconds N --out file.lmuraw` | Records the raw copied blocks and timestamps for offline normalizer tests. |

Add `--force` to overwrite an existing output file. Diagnostics logs go to stderr
(`--log-level info` or `--log-level lmu_reader=debug` for more).

**Privacy:** `--log` JSONL files and `.lmuraw` captures contain driver names, your player name and
server names. Keep them private. Do not commit them and do not post them publicly. Write them under
`D:\RaceEngineeringData` (`live --log` and `capture --out` refuse a path inside a git working tree
unless `--allow-in-git` is given). Frames dropped because the writer fell behind are counted: the
progress and final lines report them, and every JSONL line carries the cumulative `dropped_frames`.

## Gate-1 validation session

Do every step with LMU's settings untouched. If a step seems to need a settings change, note it
instead; the owner decides.

Every output file of this session goes to `D:\RaceEngineeringData\captures\gate1`, never into the
repository (the files contain names; see **Privacy**). Create the folder once, in the PowerShell
window you run the steps from:

```powershell
$gate1 = 'D:\RaceEngineeringData\captures\gate1'
New-Item -ItemType Directory -Force $gate1 | Out-Null
```

1. **Start LMU** normally from Steam and wait for the main menu.
2. **First check, in the menu.** In PowerShell:
   ```powershell
   .\target\release\lmu-diag.exe compat | Tee-Object "$gate1\gate1-compat-menu.txt"
   ```
   Expected: `state CONNECTED` (or `STALLED` if LMU publishes nothing in menus), `sync FRAME_GATE`,
   `lock SDK_LOCK`, and no `FAIL` lines. Write down the `gameVersion` value.
3. **Get on track.** Start a Practice session with a hybrid Hypercar. Leave the garage.
4. **Run the live view** in a second window (LMU windowed or borderless helps):
   ```powershell
   .\target\release\lmu-diag.exe live --log "$gate1\gate1-live.jsonl"
   ```
5. **Drive 3 or more laps**, including one pit-lane entry and exit. Check these lines:
   - `LMU`: `CONNECTED`, `sync FRAME_GATE`, `lock SDK_LOCK`.
   - `Compat`: `SUPPORTED` or `SUPPORTED_WITH_WARNINGS`. The only expected warning is
     `generic.gameVersion ... UNKNOWN_VERSION` (no version is verified yet). Note any other `WARN`
     or `FAIL` line exactly.
   - `Rates`: the telemetry and scoring Hz (the research expects about 100 Hz and 5 Hz; these are
     UNVERIFIED, write down what you see). `sample age` should stay small (under 20 ms).
   - `Lock`: `hold p99` should be under 50 us, `timeouts` near 0, and `LOCK STUCK` must never appear.
   - `Frames`: note the `duplicates` and `generic-only` counts. They show how the frame gate behaves.
   - `Events`: note which codes appear.
   - Values. Compare each with the game:
     - **Speed (km/h)**: matches the in-game speedometer. `z` is negative when driving forward.
     - **rpm and gear**: match the dash.
     - **Inputs**: throttle and brake are 0..1. Steering sign: is turning left `+` or `-`?
     - **Fuel**: matches the MFD, in litres.
     - **Tyres**: pressures around 150-200 kPa; tread °C plausible (60-110 °C when warm).
     - **Brake temperature unit**: look at the `unit hint` row **in the garage at session start,
       before driving**. It should say `K plausible (cold, near ambient)` or `C plausible`. Record
       which one (A-LMU-015).
     - **Signs**: in a right-hand corner, `G lat` should be negative (`+` = left) and `Yaw` should be
       negative. Write down what you see.
     - **G vert at rest**: in the garage, near 0 g or near +1 g? (A-LMU-032)
     - **Hybrid**: motor state changes between propulsion (2) and regeneration (3); note the ranges of
       battery, SoC and virtual energy.
     - **Aids**: TC and ABS levels match the car's settings.
     - **Pits**: `in pit lane yes` in the pit lane; the sector changes S1 → S2 → S3.
     - **Laps**: after a lap, the last lap and sector times match the timing screen.
     - **Session**: the `Weather` and `Session` lines (phase 5 = green) look right.
6. **Pause and exit.** Press Esc (pause) for more than 2 s. The state may go `STALLED` and must
   return to `CONNECTED` when you resume. Then quit to the menu and exit LMU. Within about 3 s the
   state must become `WAITING_FOR_LMU`, and the view must not keep showing the old values as live.
   Start LMU again: the view reconnects by itself.
7. **Capture** 2 minutes of driving, including a pit entry and exit:
   ```powershell
   .\target\release\lmu-diag.exe capture --seconds 120 --out "$gate1\gate1-hypercar.lmuraw"
   ```
8. **Repeat** steps 3-7 with an LMGT3 and an LMP2 car: virtual energy, SoC and hybrid fields differ
   by class (A-LMU-012/013/014).
9. **Optional:** run the reader's live integration test while on track:
   ```powershell
   cargo test -p lmu-reader --test live_mapping -- --ignored --nocapture
   ```

Hand over the files in `D:\RaceEngineeringData\captures\gate1` (`gate1-compat-menu.txt`, the JSONL
and the `.lmuraw` files) and your notes, privately. The
lead uses them to record the verified `gameVersion` in `crates/lmu-reader/src/compat.rs`
(`VERIFIED_GAME_VERSIONS`) and to update the A-LMU entries in `docs/ASSUMPTIONS.md`.

## Troubleshooting

| What you see | What it means / what to do |
|---|---|
| `WAITING_FOR_LMU`, note "LMU is not running" | LMU is not running, or it runs in a different logon session. |
| `last error: ... Access is denied` | LMU and lmu-diag run at different elevation. Run both elevated or both not elevated. |
| `INCOMPATIBLE`, `FAIL` lines | LMU changed its shared-memory layout (probably after an update). Nothing is decoded. Re-run `tools/layout-probe` and `crates/lmu-reader/codegen/gen_layout.py`, or wait for an update. |
| `LOCK STUCK` | Another program died while holding LMU's lock. Restart LMU. lmu-diag never resets the lock. |
| `sync POLLING (degraded)` | The frame-gate events are missing (an LMU build older than 1.4.2?). Data is still read, every 10 ms. |
| `lock LOCK_FREE ...` | The lock objects are missing or not accessible. Copies are verified by re-reading instead. |

## Output formats

- **JSONL (`--log`)**: one JSON object per decoded frame: `seq`, `capture_mono_ns`,
  `telemetry_updated`, `scoring_updated`, `support`, `version_verified`, `source`, `generic`,
  `telemetry_header`, `player` (the raw `TelemInfoV01` incl. wheels; `null` when the player's `mID`
  could not be verified), `player_identity`, `scoring` (only on frames where it changed, and on
  the first frame after any dropped frame), and `dropped_frames` (cumulative count of frames
  dropped before this line; a jump means the log has a gap).
  Values are raw LMU units. NaN and infinite values appear as `null` (JSON has no NaN).
- **`.lmuraw` (`capture`)**: a length-prefixed binary file of the raw blocks exactly as copied
  (all active telemetry slots, not only the player's), with timestamps, connection ids and lock
  provenance. Format version 2, tied to the layout fingerprint of the build that wrote it; the
  format is documented in `crates/lmu-reader/src/capture.rs`.
  `lmu_reader::CaptureReplay` replays it through the same decoder as the live reader.
