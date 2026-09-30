# LMU Telemetry Research

**Date of verification: 2026-09-30.** Status: static research is complete. Runtime verification is pending (Gate 1, `tools/lmu-diag`).

This is a consolidated, reviewed summary of the research files in `docs/research/`:
[`lmu_abi_layout.json`](research/lmu_abi_layout.json), [`lmu_headers_fingerprint.json`](research/lmu_headers_fingerprint.json),
[`local_install_findings.md`](research/local_install_findings.md) and [`community_research.md`](research/community_research.md).
Where those files disagree, this document says which one is right and why.

Related documents:
- [LMU_CHANNEL_MAP.md](LMU_CHANNEL_MAP.md): field-by-field mapping to canonical channels.
- [ASSUMPTIONS.md](ASSUMPTIONS.md): every assumption, with its risk and fallback.
- [DECISIONS.md](DECISIONS.md) D-002 / D-003 / D-018: the data source choice, offset-based decoding, and the read-only rule.

> **License rule.** The Studio 397 headers say *"Redistribution or modification of this header is not permitted."*
> This document therefore contains no header text. It records only identifiers, sizes, offsets, enum values and
> short paraphrases of unit comments. The headers are referenced in place, from the LMU install, at build time.

### Evidence labels

| Label | Meaning |
|---|---|
| **VERIFIED** | Observed directly on this machine: a file was read, a hash computed, code compiled and run, or strings extracted from a binary. |
| **VERIFIED-SOURCE** | Read in an official Studio 397 publication, or in a named project's source code. For project code this proves what that project does, not what LMU does. |
| **INFERRED** | Reasoned from verified evidence but not observed end to end. |
| **CLAIMED** | Stated by a third party and not checked independently. |
| **UNVERIFIED** | Open question that needs a running game. |

LMU was **not running** at any point during this research. Nothing about live data is VERIFIED.

---

## 1. Exact interface discovered

LMU has its own **native shared-memory interface**. It is built into the game and needs no plugin. **VERIFIED:** the names below
are in `Le Mans Ultimate.exe` and in the installed SDK header. The V1.2 release notes (2025-12-08) announced it as
"new shared memory implementation" (VERIFIED-SOURCE).

| Kernel object | Name (exact) | Type | Created by | Client access |
|---|---|---|---|---|
| Data mapping | `LMU_Data` | pagefile-backed file mapping | LMU (the exe has `OpenFileMapping %s failed` / `CreateFileMapping failed` log strings) | `OpenFileMapping` + `MapViewOfFile`. The SDK example asks for `FILE_MAP_ALL_ACCESS`; **we use `FILE_MAP_READ`** (D-018) |
| Frame gate 1 | `LMU_Data_HoldEvent` | named event | LMU | `OpenEventA(SYNCHRONIZE)`, wait only |
| Frame gate 2 | `LMU_Data_DataEvent` | named event | LMU | `OpenEventA(SYNCHRONIZE)`, wait only |
| Lock word | `LMU_SharedMemoryLockData` | 8-byte file mapping `{ LONG waiters @0; LONG busy @4 }` | whoever comes first (the SDK helper calls `CreateFileMappingA`, create-or-open) | read/write (needed for the `Interlocked*` operations) |
| Lock wake-up | `LMU_SharedMemoryLockEvent` | **auto-reset** event, initially non-signalled | whoever comes first (`CreateEventA(NULL, FALSE, FALSE, …)`) | wait + `SetEvent` |
| Legacy notify | `LMU_Data_Event` | named event (exe string only) | LMU | **Not in the SDK header.** Purpose UNVERIFIED. Do not use. |

- All names have no `Global\` prefix, so they live in the **session-local** namespace. Our reader must run in the same Windows logon session as LMU. **INFERRED** from the strings.
  - The one global object in the exe is `Global\LMU_PluginsAdapter_Ready`.
- Layout: one `SharedMemoryLayout` wraps `SharedMemoryObjectOut data`, which holds four blocks: `generic`, `paths`, `scoring` and `telemetry`. See §4.
- Official consumer: the exe starts `PluginsAdapter.exe`, which hosts legacy `InternalsPlugin` DLLs out of process.
  - **VERIFIED:** `PluginsAdapter.exe` contains the lock and event names and the SDK example's error text "Cannot connect to shared memory update gate."
  - **INFERRED:** Studio 397's own plugin host reads `LMU_Data` the same way the SDK example does. That makes the SDK procedure (§5.3) the reference behaviour.
- **Not used (D-002):**
  - legacy rF2 `$rFactor2SMMP_*$` buffers;
  - CrewChief's `$LMU.3.8.SMMP_*$` buffers;
  - TelemetryTool `$TT_LMU_*$` buffers;
  - DAMPlugin MoTeC output.

  All of these come from third-party plugins in `<LMU>\Plugins\`.

## 2. Interface / header version and fingerprint

The SDK folder is `C:\Program Files (x86)\Steam\steamapps\common\Le Mans Ultimate\Support\SharedMemoryInterface\`.

| Header | SHA-256 (re-computed 2026-09-30, matches fingerprint file) | Bytes | LastWriteTime |
|---|---|---|---|
| `SharedMemoryInterface.hpp` | `a82833d5e9e277a7c3af8802518e35ea62f4ec7c7c7c9083650282e4edf1f8bf` | 12,695 | 2026-09-22 17:21:41 +05:30 |
| `InternalsPlugin.hpp` | `9b6ee8cf610fa5049b18df580a9a9bc9ebb91346fc466584d576a6442abcf68f` | 57,621 | 2026-03-31 18:52:53 +05:30 |
| `PluginObjects.hpp` | `f65f1d2226af1acb277f8337fb10d8955db384118fc36eef95ea446be058e247` | 3,551 | 2025-12-20 13:41:39 +05:30 |

- The headers have **no explicit version number**. The interface version is therefore identified by the triple of SHA-256 hashes (`docs/research/lmu_headers_fingerprint.json`, schema `lmu-headers-fingerprint/1`).
  - The layout JSON hash in the fingerprint is `f7728c96…d0a`.
  - At startup the reader should compare the installed header hashes with the committed fingerprint *if the headers are present*. A mismatch triggers a "re-run layout probe" warning (D-003).
- **Header history** (INFERRED from mtimes and release notes):
  - `InternalsPlugin.hpp` last changed with the 1.3.0.x release (2026-03-30 notes: "Added various new values to plugin system").
  - `SharedMemoryInterface.hpp` changed with **1.4.2** (2026-09-22 notes: "Fixed some threading issues with shared memory"). That change added the Hold/Data waiter.
- The layout was produced by `tools/layout-probe`: MSVC 19.51.36248 x64, `/std:c++17 /W4`, compiled against the installed headers.
  - Every self-check passed, and all 13 structs and 3 enums cross-check by name and order against the header text. **VERIFIED.**
  - A second compiler (clang 20, llvm-mingw) produced an identical layout. **VERIFIED.**

## 3. LMU game version tested

| Item | Value | Status |
|---|---|---|
| `Le Mans Ultimate.exe` FileVersion / ProductVersion | **1.4.2.0** (exe mtime 2026-09-25) | VERIFIED (re-read 2026-09-30) |
| Steam app / build | 2399420 / **25513257** (`appmanifest_2399420.acf`) | VERIFIED (re-read 2026-09-30) |
| Trace log / `Core\Version.txt` | `LMU-Retail:1.4200` / `1.4200`, `4200` | VERIFIED (local findings) |
| Release notes | V1.4.2 (2026-09-22) and hotfix **v1.4.2.1 (2026-09-24)** | VERIFIED-SOURCE |
| Is build 25513257 the 1.4.2.1 hotfix? | The exe still reports 1.4.2.0 but its mtime (2026-09-25) is after the hotfix date | **UNVERIFIED** |
| `SharedMemoryGeneric.gameVersion` at runtime | Probably `14200` (the TelemetryTool plugin logged "version 14.200") | INFERRED / **UNVERIFIED** |

**All research was static.** LMU was not running, so every statement about live values, rates and event behaviour is pending runtime verification. The checklist is in §13.

## 4. Shared-memory mapping name and layout

The mapping is **`LMU_Data`**. Offsets below come from `lmu_abi_layout.json` (MSVC, x64). Types follow Windows LLP64:
`long` and `unsigned long` are 4 bytes; pointers, `HWND` and `size_t` are 8 bytes.

| Region (absolute offset in the view) | Offset | Size | Notes |
|---|---:|---:|---|
| `data.generic` | 0 | 332 | `events[16]` (u32 each) @0, `gameVersion` (i32) @64, `FFBTorque` (f32) @68, `appInfo` @72 |
| `data.paths` | 332 | 1,300 | 5 × `char[260]`: `userData`, `customVariables`, `stewardResults`, `playerProfile`, `pluginsFolder` |
| `data.scoring.scoringInfo` | 1,632 | 548 | `ScoringInfoV01`: `mCurrentET` @1700, `mLapDist` @1720, `mNumVehicles` @1736, `mGamePhase` @1740 |
| *(padding)* | 2,180 | 4 | Default-pack hole before the 8-byte-aligned `size_t` |
| `data.scoring.scoringStreamSize` | 2,184 | 8 | u64. Capacity is 65,536 |
| `data.scoring.vehScoringInfo[104]` | 2,192 | 60,736 | Stride **584** |
| `data.scoring.scoringStream[65536]` | 62,928 | 65,536 | Text (newline-delimited results additions, per the header comment on `mResultsStream`) |
| `data.telemetry.activeVehicles` / `playerVehicleIdx` / `playerHasVehicle` | 128,464 / 128,465 / 128,466 | 1 each | u8 / u8 / bool, then a 1-byte hole |
| `data.telemetry.telemInfo[104]` | 128,468 | 196,352 | Stride **1,888**. `mWheel[4]` at +848, stride **260** |
| end of last member | 324,820 | | `sizeof(SharedMemoryLayout)` = **324,824** (4 bytes of tail padding), 80 × 4 KiB pages |

**Packing.** This was *half-right* in the initial briefing and in `community_research.md`.
- `InternalsPlugin.hpp` wraps its structs in `#pragma pack(push, 4)`, so doubles there are only 4-aligned (e.g. `TelemInfoV01.mDeltaTime` @4, `TelemWheelV01.mVerticalTireDeflection` @180).
- `SharedMemoryInterface.hpp` has **no** pragma, so the `SharedMemory*` wrapper structs use MSVC default packing. That produces the 4-byte hole at 2,180. **VERIFIED** with MSVC and clang.
- If LMU itself had been built with `/Zp4`, everything from `scoringStreamSize` onward would be 4 bytes earlier (total 324,816). **INFERRED** that LMU uses default packing, because the SDK is meant to be compiled as-is. **UNVERIFIED** against a live mapping; runtime checks are in §13.

**Community size claim.** pyLMUSharedMemory and KoDriver quote **324,820** bytes. That is the sum of member sizes (all member offsets agree with ours); MSVC `sizeof` is 324,824 because of tail padding. Consequences:
- The difference is harmless for reading.
- Map with size `0` (the whole section) and require `VirtualQuery` region size ≥ 324,820.

**Useless fields.**
- `ScoringInfoV01.mResultsStream` (@96 in the struct) and `ScoringInfoV01.mVehicle` (@540) are **pointers into LMU's address space**.
- `ApplicationStateV01.mAppWindow` is an `HWND`.

Never dereference any of them.

## 5. Synchronisation and locking

### 5.1 Frame gate (`LMU_Data_HoldEvent` → `LMU_Data_DataEvent`), new in 1.4.2

- **VERIFIED (header code):**
  - The client opens both events with `SYNCHRONIZE` only, so it can wait on them but never set or reset them.
  - `WaitForNextFrame(ms)` waits on **Hold first, then Data**, each with the same timeout. It returns `true` only if both waits return `WAIT_OBJECT_0`. The header comment calls this "the only order that's race-free".
  - The example loop is: `WaitForNextFrame(200)` → `Lock()` → copy → `Unlock()` → process. A 200 ms timeout is used only to re-check whether LMU is still alive ("it's not a rate limit").
- **Semantics (INFERRED, UNVERIFIED):** a two-phase broadcast gate.
  - "Hold signalled" means the previous publish phase is over, so a waiter must now wait for the *next* Data signal.
  - "Data signalled" means a new frame has been published.
  - Waiting on Hold first stops a fast client from passing straight through a Data event still set from the previous frame.
  - Unknown: whether the events are manual- or auto-reset, and whether they fire per telemetry frame, per scoring update or per either (open question 2 in `community_research.md`).
- **Consequences:**
  - A `true` return **does not grant exclusive access**. The SDK example still takes the lock.
  - The worst-case wait is **2 × timeout**.
  - A slow client simply skips frames, which is acceptable (latest-wins).
  - `OpenEventA` fails if LMU is not running, **or if the build predates this interface** (the header comment says so). Our fallback for that case is a 10 ms poll plus a witness read (§5.3).

### 5.2 Lock (`LMU_SharedMemoryLockData` + `LMU_SharedMemoryLockEvent`)

**VERIFIED (header code).** A hand-rolled spin-then-wait lock over two 32-bit words in shared memory: `waiters` at +0 and `busy` at +4.

| Operation | Exact behaviour |
|---|---|
| `Lock(ms)` | 1. Up to **4,000** spins of `InterlockedCompareExchange(&busy, 1, 0)` with `YieldProcessor()`.<br>2. Then `InterlockedIncrement(&waiters)` and loop: CAS again, otherwise `WaitForSingleObject(lockEvent, ms)`. Anything other than `WAIT_OBJECT_0` gives up.<br>3. `InterlockedDecrement(&waiters)`.<br>Returns whether the lock was acquired. **`ms` applies to each wait, not to the whole call**, so total time can exceed `ms` when a wake-up loses the CAS race. |
| `TryLockSpin(n=1000)` | Up to `n` CAS spins. Never blocks. Commented as meant for "time-critical callers". |
| `Unlock()` | `InterlockedExchange(&busy, 0)`, then **a plain (non-interlocked) read** of `waiters`; `SetEvent(lockEvent)` if it is > 0. Wake-ups can therefore be lost, which the per-wait timeout covers. |
| `Reset()` | Writes both words to 0. Commented "only from the core application". `Init()` also calls it when the lock mapping was newly created (`GetLastError() != ERROR_ALREADY_EXISTS`). |

- There is **no owner tracking and no abandonment detection** (unlike a Win32 mutex). A process killed while holding `busy = 1` leaves the lock held.
- A process killed while waiting leaves `waiters` permanently > 0. That is benign: every later `Unlock` just issues a spurious `SetEvent`.
- **The game uses the same lock** (VERIFIED strings in `Le Mans Ultimate.exe`, next to `SharedMemoryManager.cpp`):
  - `SharedMemoryManager::FlushTimeCritical` and `Failed to acquire SharedMemoryLock in FlushTimeCritical`
  - `SharedMemoryManager::Flush` and `SharedMemory lock timed out, backing off...`
  - `Cannot create SharedMemoryLock!`

### 5.3 Correct read procedure for an external reader (our implementation contract)

1. **Detect.** `OpenFileMappingW(FILE_MAP_READ, FALSE, L"LMU_Data")`.
   - If it fails, LMU is not running. Stay in `WAITING_FOR_LMU` and retry every 1 s.
   - Otherwise `MapViewOfFile(h, FILE_MAP_READ, 0, 0, 0)`. `VirtualQuery` the view and require region size ≥ 324,820; otherwise report `INCOMPATIBLE`.
2. **Gate.** `OpenEventA(SYNCHRONIZE, FALSE, "LMU_Data_HoldEvent")` and `…DataEvent`.
   - If either fails while `LMU_Data` exists, the build is pre-1.4.2. Use **fallback mode**: a 10 ms poll plus a witness read (the ET / vehicle count must be unchanged before and after the copy, up to 5 retries), and flag the source `DEGRADED`.
3. **Lock.** Only *after* `LMU_Data` exists, so LMU has had the chance to create the lock first:
   - `CreateFileMappingA(INVALID_HANDLE_VALUE, NULL, PAGE_READWRITE, 0, 8, "LMU_SharedMemoryLockData")`. Record whether `GetLastError() == ERROR_ALREADY_EXISTS`; expected yes, and log a warning if not.
   - Map it with `FILE_MAP_READ | FILE_MAP_WRITE`.
   - `CreateEventA(NULL, FALSE, FALSE, "LMU_SharedMemoryLockEvent")`.
   - **Never call `Reset()`.** A newly created mapping is zero-filled anyway.
   - In Rust, operate on the two words through `AtomicI32` at the mapped address (`compare_exchange(0, 1, SeqCst, SeqCst)`, `swap(0, SeqCst)`, `fetch_add` / `fetch_sub`). These compile to the same `lock cmpxchg` / `xchg` as the `Interlocked*` calls.
4. **Loop** (dedicated OS thread):
   1. `WaitForNextFrame(100 ms)`, meaning Hold then Data. On timeout, increment `gate_timeouts`. After more than 2 s of consecutive timeouts, run the liveness check (step 5).
   2. **Acquire:** `TryLockSpin(4000)`, then at most **one** `Lock`-style wait of **≤ 5 ms**. If still not acquired, increment `lock_timeouts`, **skip this frame**, and go back to 4.1. Never block the reader for long.
   3. **Under the lock (critical section, no allocation, no logging, no I/O, no panics):**
      - read `generic.events[16]`, `telemetry.activeVehicles` / `playerVehicleIdx` / `playerHasVehicle`, `scoringInfo.mNumVehicles` and `scoringStreamSize`;
      - clamp the counts to `[0, 104]` and the stream size to `[0, 65535]`;
      - `memcpy` generic, paths, `scoringInfo`, `vehScoringInfo[0..n_s]`, `scoringStream[0..len]`, the telemetry header and `telemInfo[0..n_t]` into a **preallocated** buffer.

      Copying the whole 324,820 bytes is also acceptable (about 32.5 MB/s at 100 Hz). The clamped copy keeps hold time lowest.
   4. **Release:** `swap(busy, 0)`, then `SetEvent` if `waiters > 0`. Use a drop guard so unwinding also releases.
   5. **Outside the lock:** stamp `capture_mono_ns`, record lock-wait and lock-hold durations, then decode, validate and normalise (see ARCHITECTURE §4).
5. **Liveness and exit.** Our open handle **keeps the `LMU_Data` section alive after LMU exits**. The data then freezes and the events never fire.
   - After more than 2 s with no frame *and* a non-advancing `mElapsedTime` / `mCurrentET`, close every handle and go back to step 1. If `OpenFileMapping` then fails, LMU is gone.
   - `OpenProcess(SYNCHRONIZE)` on the LMU PID (the SDK example's method) is optional. CrewChief reports EAC blocks stronger access rights (CLAIMED), so never request more than `SYNCHRONIZE`.
6. **Freshness.** Do not trust `events[]` alone (see §11). A telemetry frame is new if the player `mElapsedTime` changed; a scoring block is new if `mCurrentET` changed. Both flags are attached to the snapshot.

### 5.4 What happens if we hold the lock too long

- **Other readers stall.** `PluginsAdapter.exe` (every legacy plugin), TinyPedal, SimHub and similar tools block in `Lock()` or fail their `TryLockSpin`. **VERIFIED** (header code).
- **LMU drops or delays its own publishes.** INFERRED from the exe strings in §5.2:
  - The time-critical flush almost certainly uses the non-blocking `TryLockSpin` (the header documents it for "time-critical callers"). When it fails, that frame is **not published** and a "Failed to acquire SharedMemoryLock in FlushTimeCritical" line is logged.
  - The normal flush waits with a timeout and then "backs off".
  - Every consumer sees missing frames, and LMU's trace log fills with errors.
  - Physics is probably *not* stalled, because the time-critical path does not block, but that is **UNVERIFIED**.
- **If our process dies while holding the lock,** `busy` stays 1 and nothing in the SDK recovers it. Every client and LMU's writer keep failing until LMU restarts, or until LMU calls `Reset()` itself (UNVERIFIED).
  - Mitigation: a microsecond-scale critical section with no fallible code inside.
  - `lmu-diag` reports a lock word stuck at 1 for more than 1 s ("held by an unknown process").
  - We **never force-reset** the lock (D-018).
- **Budget:** hold ≤ 50 µs per frame (INFERRED from memcpy cost), measured and reported by `lmu-diag` as a histogram. Any hold > 1 ms is logged as a warning.

## 6. Refresh rates (CLAIMED / UNVERIFIED until the runtime diagnostic measures them)

| Stream | Figure | Evidence | Status |
|---|---|---|---|
| Telemetry (`SME_UPDATE_TELEMETRY`, `telemInfo[]`) | **~100 Hz nominal** | `Settings.JSON` → `"Plugin Mask": 4`, described as "100Hz default for performance reasons" (VERIFIED, re-read 2026-09-30). LMU Pitwall claims it measured ~100 Hz (CLAIMED). The header states no telemetry rate. | **UNVERIFIED**. What mask value `4` means is unknown |
| Scoring (`SME_UPDATE_SCORING`, `scoringInfo` + `vehScoringInfo[]`) | **~5 Hz nominal** | Header comment on the plugin `UpdateScoring` callback: "approximately five times per second" (VERIFIED, but it describes plugin callbacks, not `LMU_Data`). LMU Pitwall measured 5 Hz (CLAIMED). TinyPedal comment "5fps" (VERIFIED-SOURCE) | **UNVERIFIED** |
| 50 Hz / 400 Hz | Not an `LMU_Data` figure | Legacy rF2 plugin README only ("Telemetry - 50FPS"). No source ties 400 Hz to `LMU_Data` | Not applicable to `LMU_Data` |
| Internal recorder loop | Probably 400 Hz | The DuckDB 7 Hz channels run at 7.018 Hz ≈ 400/57 | INFERRED. Not the shared-memory rate |

**How `lmu-diag` will measure the rates (never hardcode them):**
- the wake-up rate of `WaitForNextFrame`;
- how often `SME_UPDATE_TELEMETRY` / `SME_UPDATE_SCORING` appear in `events[]`;
- distinct player `mElapsedTime` values per wall-clock second, and the `mDeltaTime` distribution;
- distinct `mCurrentET` values per second;
- whether the rates change with `Plugin Mask`, as a manual user experiment (D-018 forbids us editing settings).

The measured rates go into the capability map (PROTOCOL `SCHEMA_NEGOTIATE`) and the UI.

## 7. Available channels (grouped)

The full mapping, with type, raw unit, conversion and validation per field, is in [LMU_CHANNEL_MAP.md](LMU_CHANNEL_MAP.md). Notation:
- `p` = player telemetry slot;
- `k` = player scoring row;
- `w` = wheel, 0 FL, 1 FR, 2 RL, 3 RR (header order);
- telemetry fields are `TelemInfoV01`; scoring fields are `VehicleScoringInfoV01` / `ScoringInfoV01`.

| Group | Fields (exact names) |
|---|---|
| **Inputs** | `mUnfilteredThrottle`, `mUnfilteredBrake`, `mUnfilteredClutch` (0..1), `mUnfilteredSteering` (-1..1); `mFilteredThrottle`, `mFilteredBrake`, `mFilteredClutch`, `mFilteredSteering`; `mSteeringShaftTorque`; `mPhysicalSteeringWheelRange`, `mVisualSteeringWheelRange`; `generic.FFBTorque` |
| **Powertrain** | `mGear`, `mMaxGears`, `mEngineRPM`, `mEngineMaxRPM`, `mClutchRPM`, `mEngineTorque`, `mEngineWaterTemp`, `mEngineOilTemp` (°C per header), `mTurboBoostPressure`, `mOverheating`, `mIgnitionStarter`, `mAntiStallActivated`, `mSpeedLimiter`, `mSpeedLimiterActive`, `mSpeedLimiterAvailable`, `mMotorMap`/`mMotorMapMax` |
| **Fuel** | `mFuel` (L), `mFuelCapacity` (L), `mScheduledStops`; per car in scoring: `mFuelFraction` (0–255, "fuel or battery") |
| **Tyres** (per wheel) | `mPressure` (kPa); `mTemperature[3]` (K, left/centre/right *in car coordinates*); `mTireInnerLayerTemperature[3]` (K); `mTireCarcassTemperature` (K); `mOptimalTemp`; `mWear` (0..1); `mGripFract`; `mTireLoad`; `mLateralForce`, `mLongitudinalForce`; `mLateralPatchVel`, `mLongitudinalPatchVel`; `mLateralGroundVel`, `mLongitudinalGroundVel`; `mVerticalTireDeflection`; `mStaticUndeflectedRadius` (cm); `mCompoundIndex`, `mCompoundType`; `mFlat`; `mSurfaceType`; `mTerrainName`. Per axle: `mFrontTireCompoundName`, `mRearTireCompoundName`, `mFrontTireCompoundIndex`, `mRearTireCompoundIndex` |
| **Brakes** | `mWheel[w].mBrakeTemp` (header says °C, community measured K; see §11); `mWheel[w].mBrakePressure` (**0..1 normalised**, not kPa); `mRearBrakeBias` (fraction on the rear); `mMigration`/`mMigrationMax` |
| **Suspension / chassis / aero** | `mSuspensionDeflection`, `mRideHeight`, `mSuspForce`, `mCamber`, `mToe`, `mWheelYLocation`, `mRotation` (per wheel); `mFront3rdDeflection`, `mRear3rdDeflection`; `mFrontRideHeight`, `mRearRideHeight`, `mFrontWingHeight`; `mDrag`, `mFrontDownforce`, `mRearDownforce` (units not stated); `mFrontAntiSway`/`mFrontAntiSwayMax`, `mRearAntiSway`/`mRearAntiSwayMax`; `mFrontFlapActivated`, `mRearFlapActivated`, `mRearFlapLegalStatus` |
| **Dynamics** | `mPos`, `mLocalVel`, `mLocalAccel`, `mOri[3]`, `mLocalRot`, `mLocalRotAccel` (m, m/s, m/s², rad/s, rad/s²; local frame +x left, +y up, +z rear) |
| **Driver aids** | `mTCActive`, `mABSActive` (**booleans = intervening now**); `mTC`/`mTCMax`, `mTCSlip`/`mTCSlipMax`, `mTCCut`/`mTCCutMax`, `mABS`/`mABSMax` (**setting levels**) |
| **Hybrid / energy** | `mVirtualEnergy`, `mSoC`, `mBatteryChargeFraction` (0..1), `mRegen` (kW per header), `mElectricBoostMotorTorque`, `mElectricBoostMotorRPM`, `mElectricBoostMotorTemperature`, `mElectricBoostWaterTemperature`, `mElectricBoostMotorState` (0 unavailable, 1 inactive, 2 propulsion, 3 regen), `mLiftAndCoastProgress` |
| **Timing** | Telemetry: `mElapsedTime`, `mDeltaTime`, `mLapNumber`, `mLapStartET`, `mLapInvalidated`, `mDeltaBest`, `mTimeGapCarAhead`, `mTimeGapCarBehind`, `mTimeGapPlaceAhead`, `mTimeGapPlaceBehind`. Scoring: `mBestSector1`, `mBestSector2`, `mBestLapTime`, `mLastSector1`, `mLastSector2`, `mLastLapTime`, `mCurSector1`, `mCurSector2`, `mBestLapSector1`, `mBestLapSector2`, `mTimeIntoLap`, `mEstimatedLapTime`, `mLapStartET`; session `mCurrentET`, `mEndET`, `mSessionTimeRemaining`, `mMaxLaps` |
| **Scoring / competitors** (per car, `k < mNumVehicles`) | `mID`, `mDriverName`, `mVehicleName`, `mVehicleClass` (**char[32] string**), `mPlace`, `mTotalLaps`, `mLapDist`, `mTimeBehindNext`/`mLapsBehindNext`, `mTimeBehindLeader`/`mLapsBehindLeader`, `mNumPitstops`, `mNumPenalties`, `mFinishStatus`, `mControl`, `mIsPlayer`, `mServerScored`, `mQualification`, `mPitGroup`, `mVehFilename`, `mUpgradePack`, `mAttackMode`, `mDRSState`, plus `mPos`/`mLocalVel`/`mOri` at scoring rate. Full telemetry for every active car is also in `telemInfo[0..activeVehicles]` (CLAIMED to cover the whole grid online) |
| **Weather** | `mAmbientTemp`, `mTrackTemp` (°C), `mRaining`, `mDarkCloud` (0..1), `mCloudCoverage`, `mMinPathWetness`, `mMaxPathWetness`, `mAvgPathWetness` (0..1), `mWind` (vector), `mTimeOfDay`, `mTrackGripLevel` |
| **Flags** | `mGamePhase` (0–9; 5 green, 6 FCY / safety car, 7 stopped, 8 over), `mYellowFlagState` (-1..7), `mSectorFlag[3]`, `mStartLight`, `mNumRedLights`; per car `mFlag` (0 green, 6 blue), `mUnderYellow`, `mIndividualPhase`, `mCountLapFlag` |
| **Pit** | `mCurrentSector` sign bit (pit lane); per car `mInPits`, `mPitState` (0 none, 1 request, 2 entering, 3 stopped, 4 exiting), `mInGarageStall`, `mPitLapDist`, `mNumPitstops`; `mScheduledStops` |
| **Track position** | `mCurrentSector` (low bits, 0-based); per car `mSector` (0=S3, 1=S1, 2=S2), `mLapDist`, `mPathLateral`, `mTrackEdge`; track length `scoringInfo.mLapDist`; `mPos` for the map; `mTrackLimitsSteps` and `mTrackLimitsStepsPerPenalty`, `mTrackLimitsStepsPerPoint`; per-wheel `mSurfaceType`, `mTerrainName` |
| **Vehicle identity** | `mVehicleName`, `mVehicleModel`, `mVehicleClass` (`IP_VehicleClass` enum: Hypercar 0, LMP2_ELMS 2, LMP2 3, LMP3 4, GTE 5, GT3 6, PaceCar 8, Unknown 255), `mVehicleChampionship` (WEC_2023..2026 = 0..3, ELMS_2025 16, ELMS_2026 17, Unknown 255), `mID`; scoring `mVehFilename`, `mVehicleClass` (string) |
| **Session / app** | `mTrackName`, `mSession` (0 testday, 1–4 practice, 5–8 qual, 9 warmup, 10–13 race), `mNumVehicles`, `mInRealtime`, `mGameMode`, `mMaxPlayers`, `mIsFixedSetup`, `mStartET`; `generic.gameVersion`; `appInfo.mOptionsLocation` (0 main UI, 1 loading, 2 monitor, 3 on track); `paths.userData` (locates `UserData\Telemetry` for DuckDB import without touching the LMU process) |
| **Damage** | `mDentSeverity[8]` (0/1/2), `mDetached` (car), `mWheel[w].mDetached`, `mWheel[w].mFlat`, `mLastImpactET`, `mLastImpactMagnitude`, `mLastImpactPos` |

**Personal data.** Fields that must not leave the driver PC without consent:
- `mDriverName`, `mPlayerName`, `mPlrFileName`, **`mSteamID`**;
- `mServerName`, `mServerPublicIP`, `mServerPort`;
- `paths.*` (these contain the Windows user path).

## 8. Unavailable channels (checked field by field against the layout)

| Wanted | Shared-memory reality | Closest alternative |
|---|---|---|
| Real brake line pressure (kPa / Pa) | **UNAVAILABLE.** `mBrakePressure` is 0..1 from pedal × bias. The header says it will become kPa "in future" | Display as a fraction. Version-gate the field |
| Brake pad / disc wear or thickness | **UNAVAILABLE** | DuckDB `Brake Thickness` (unit unclear); REST `RepairAndRefuel` → `wearables.brakes` (CLAIMED) |
| Brake disc core vs air temperature | **UNAVAILABLE.** There is only one `mBrakeTemp` per wheel | DuckDB `Brakes Core Temp`, `Brakes Air Temp` |
| Tyre rim temperature | **UNAVAILABLE** | DuckDB `TyresRimTemp` |
| TC / ABS intervention *amount* | **UNAVAILABLE.** Only booleans `mTCActive` / `mABSActive` (active or not) and setting levels `mTC`, `mABS` | Duty cycle derived from the booleans (% of samples active) |
| Tyre compound *name* per wheel | **UNAVAILABLE** per wheel. Names are per axle (`mFrontTireCompoundName`, `mRearTireCompoundName`, `char[18]`); per-wheel `mCompoundIndex` / `mCompoundType` exist | Map each wheel to its axle name |
| Aero load per axle | **AVAILABLE, unit unverified**: `mFrontDownforce`, `mRearDownforce`, `mDrag` (probably N, INFERRED) | Validate against speed² at runtime |
| Aero / body damage as a number | **UNAVAILABLE.** Only `mDentSeverity[8]` (0/1/2) and `mDetached` | REST `wearables.body.aero` (CLAIMED) |
| Suspension damage | **UNAVAILABLE** | REST `wearables.suspension` (CLAIMED) |
| Engine power | **UNAVAILABLE** directly | Derived: `mEngineTorque × mEngineRPM × 2π/60` |
| Road-wheel steer angle | **UNAVAILABLE** | Steering-wheel angle derived from input × `mPhysicalSteeringWheelRange` (assumption) |
| Slip angle / slip ratio | **UNAVAILABLE** directly | Experimental derivation from patch and ground velocities (unverified definitions) |
| Humidity, air pressure, air density | **UNAVAILABLE** (the names appear only as unused exe strings) | REST `/rest/sessions/weather` `WNV_HUMIDITY` (CLAIMED) |
| Weather *forecast* | **UNAVAILABLE** | REST `/rest/sessions/weather` |
| Pit-menu state, pit-stop time estimate, tyre-set inventory | **UNAVAILABLE** | REST (§9) |
| Safety-car position / SC flag | **UNAVAILABLE** as a dedicated field. `mGamePhase == 6` means "full course yellow / safety car" | Use the phase |
| Penalty details / reason | Only the count `mNumPenalties` | REST (not yet surveyed) |
| Current lap time | Not a field (the header explains it "instantly becomes last") | Derived: `mElapsedTime − mLapStartET` |
| Lap distance at telemetry rate | Only in scoring (~5 Hz) | Interpolate with speed × dt (ESTIMATED) |
| GPS latitude / longitude | **UNAVAILABLE** | Only DuckDB, where it is synthetic (≈60°N 0°E for every track) |

## 9. Supplementary REST endpoints

- **Server:** `http://127.0.0.1:6397`. VERIFIED: `Settings.JSON` has `"WebUI bind": "127.0.0.1"` and `"WebUI port": 6397`, and trace logs say `Bind rest server to 127.0.0.1:6397`.
- **WebSocket:** `ws://…:6398` (port + 1) with `/websocket/ui` topics `SessionInfo`, `LiveStandings`, `InputData`, and `/websocket/events`. VERIFIED in the UI JavaScript; not tested live.
- **Swagger:** `/swagger-schema.json` (OpenAPI 2.0) is the authoritative description of the running build. VERIFIED that the string is in the exe; its 179 paths / 79 GET are CLAIMED.
- **Response schemas are UNVERIFIED**, because the game was not running.

**Policy** (D-002, D-018):
- **GET only**, local `127.0.0.1` only (not `localhost`: resolution broke after 1.3.3, CLAIMED);
- feature flag `ENABLE_LMU_REST`, ≤ 1 Hz, 1 s timeout, back-off when data is unchanged or on errors;
- never ask the user to bind `0.0.0.0`: that would expose about 100 unauthenticated write routes;
- never call write routes.

| Endpoint (GET) | Why we want it | Hazards / notes |
|---|---|---|
| `/rest/watch/standings` | Per-car standings incl. `veFraction`, `fuelFraction`, `pitState` strings (CLAIMED) | Cross-check against shared-memory scoring |
| `/rest/watch/sessionInfo` | Session and time info (CLAIMED fields) | — |
| `/rest/watch/standings/history`, `/rest/watch/trackmap` | Standings history; track-map geometry | Shapes unknown |
| `/rest/sessions/GetGameState` | Game / pit / multi-stint state strings | Logged 4xx when called in menus |
| `/rest/sessions/weather` | **Forecast** (`WNV_*` nodes) | One-shot per session plus a slow refresh |
| `/rest/garage/UIScreen/RepairAndRefuel` | Wear / damage (`wearables`), `fuelInfo` (max/current VE), pit menu, `pitStopLength` | **Polling it often makes the in-game MFD pit menu flicker** (CLAIMED, acknowledged by the dev in 2025). Keep ≤ 0.2 Hz with back-off |
| `/rest/strategy/usage` | Per-lap fuel / VE / tyre usage history | — |
| `/rest/strategy/pitstop-estimate` | Pit-stop time breakdown | Found in the exe, not in the UI bundle |
| `/rest/strategy/overall` | Strategy summary | Empty in single-car tests (CLAIMED) |
| `/rest/garage/getVehicleCondition`, `/rest/garage/tireinfo`, `/rest/garage/brakeinfo`, `/rest/garage/PitMenu/receivePitMenu` | Condition, tyre, brake and pit-menu data | Shapes unknown |
| `/rest/sessions/TireInventory`, `/rest/garage/UIScreen/TireManagement` | Tyre-set inventory | Shapes unknown |
| `/rest/garage/getPlayerGarageData` | Setup `VM_*` values | **Frozen snapshot while driving** (CLAIMED) |

Known REST hazards (CLAIMED):
- some URIs crash the game (e.g. `garage/Pitstop/getPitstopTimes`);
- paths changed with game updates (June 2025, and again later);
- responses are `text/plain` with no charset but contain UTF-8;
- remaining VE from REST updates only at lap end, which is why TinyPedal and CrewChief switched to shared memory `mVirtualEnergy`.

## 10. Native DuckDB recording (offline import only)

**VERIFIED from 4 real third-party recordings, plus strings in the exe:**
- The recorder is built into `Le Mans Ultimate.exe`. It writes DuckDB through the bundled `duckdb.dll` **v1.4.0** (storage version 64, block size 16,384) using the Appender API.
- It is enabled by `Settings.JSON` → `"Automatically Record Telemetry"` (currently `false`) or by an unbound "Start/Stop Telemetry Recording" key. Per **D-018** we never change this; the user does.
- Channel set and rates come from `UserData\Telemetry\config.json`: 56 channels at 1–100 Hz and 42 events. Whether editing it takes effect is UNVERIFIED. **Always read each file's own `channelsList`**, because recordings contain channels missing from the local config.
- **Output:** `UserData\Telemetry\<…>_<…>_<%Y-%m-%dT%H_%M_%SZ>.duckdb`. The directory is INFERRED and the exact filename is UNVERIFIED; no local recording exists yet.
- **Schema:**
  - one table per channel and one per event, **names contain spaces and must be quoted**;
  - `channelsList(channelName, frequency INT, unit)`, `eventsList(eventName, unit)`;
  - `metadata(key, value)` with 12 keys, including `CarSetup` JSON (172 `VM_*` keys).
- **Time:**
  - Channel tables have **no timestamp**. Sample *i* is at `t0 + i/frequency`, where `t0` = the first `GPS Time` value (session-elapsed seconds).
  - Event tables are `(ts DOUBLE, value…)`: an initial row, then one row per change.
  - Effective rate = rows / duration (the 7 Hz channels actually run at 7.018 Hz).
- Per-wheel columns `value1..value4` = **FL, FR, RL, RR**. VERIFIED by brake-bias, ride-height, wheel-speed and tyre-edge-temperature checks.
- **Unit label mismatches:**
  - `Brakes Force` is labelled `%` but holds a fraction;
  - `Brake Thickness` is labelled `%` but holds about 3.6;
  - `Wind Heading` is labelled `deg` but holds about 5.5 (radians?).

  UNVERIFIED which is right.
- GPS lat/lon are **synthetic** (a track-local projection near 60°N 0°E).
- **Fixtures:** the 4 local files belong to a third-party setup pack and contain a driver name. **Never commit them.** Record our own session instead.
- **Import:** use the Rust `duckdb` crate with bundled libduckdb ≥ 1.4, open `read_only`, and retry if the file is locked. Find the folder from `LMU_Data.paths.userData`.

## 11. Known limitations

1. **`generic.events[16]` is a queue of SME codes, not 16 flags.** VERIFIED from how the header's copy helper switches on each value.
   - `SME_ENTER` = 0, so zero-filled unused slots are indistinguishable from `SME_ENTER`. The SDK helper would copy `paths` on every frame.
   - How empty slots are marked is UNVERIFIED. The event queue is advisory; freshness comes from ET changes (§5.3 step 6).
2. **The SDK copy helper does not clamp** `mNumVehicles` (i32) or `activeVehicles` (u8, up to 255) to 104 slots, and writes a NUL at `scoringStream[scoringStreamSize]`, which is **out of bounds when the size is 65,536**. VERIFIED (header code). Our reader clamps all three.
3. **Array validity:**
   - `telemInfo[i]` is meaningful only for `i < min(activeVehicles, 104)`; `vehScoringInfo[i]` only for `i < min(mNumVehicles, 104)`.
   - Slots beyond the count may hold stale data from a previous session (INFERRED).
   - Telemetry and scoring arrays are **not guaranteed to share an order**. Join them on `mID`, which is reused in multiplayer after someone leaves.
4. **Player identification:**
   - Telemetry uses `playerVehicleIdx`, valid only if `playerHasVehicle`. Scoring has no index; find `mIsPlayer`.
   - The two must agree on `mID`; otherwise the player is `UNAVAILABLE` for that frame.
5. **Count disagreement:** while cars join, scoring and telemetry counts can briefly differ, CLAIMED at 0.04–0.08 % of samples. Flag that frame's joins as suspect.
6. **Units disputed or missing in the header:**
   - `mBrakeTemp`: the header says °C; three community sources say K (a cold reading of about 285).
   - No units given for `mSoC`, `mVirtualEnergy`, `mTimeGapCarAhead`, `mTimeGapCarBehind`, `mTimeGapPlaceAhead`, `mTimeGapPlaceBehind`, `mDeltaBest`, `mOptimalTemp`, `mTurboBoostPressure`, `mDrag`, `mFrontDownforce`, `mRearDownforce`, the steering ranges, `mTimeOfDay` or `mWind`.
   - Each has a runtime detection rule or an assumption in [ASSUMPTIONS.md](ASSUMPTIONS.md).
7. **Tyre temperature arrays are left/centre/right in the car frame, not inner/outer.** The header says so explicitly. The inner/outer mapping depends on the wheel side (see the channel map; ASSUMPTION A-LMU-016).
8. **Sectors:**
   - `mSector` uses 0 = sector 3 (VERIFIED header).
   - `mCurrentSector` is zero-based with **pit lane in bit 31** (VERIFIED header).
   - The order of the `mSectorFlag[3]` indices is **unknown even to the header author**. Values 1 = yellow and 11 = clear are CLAIMED.
9. **`mInPits` is "not always accurate for remote vehicles"** (header). Cross-check with `mPitState`.
10. **Sentinels** (CLAIMED): `mMaxLaps` = INT32_MAX in timed races; lap and sector times ≤ 0 mean "none"; `mLapStartET` = −1 before the first lap.
11. **Class-dependent data:**
    - Virtual energy exists only for Hypercar and LMGT3 (VERIFIED-SOURCE, official guide).
    - Hybrid fields mean something only for hybrid Hypercars.
    - LMP2 and LMP3 read 0 (CLAIMED).
12. **Same name, different type:** telemetry `mVehicleClass` is a u8 enum; scoring `mVehicleClass` is `char[32]`. Strings are NUL-terminated only by convention, so decode within bounds with UTF-8 lossy decoding.
13. **The header's own remark about ISO coordinates is internally inconsistent.** It calls "+x forward, +y right, +z up" right-handed, which it is not. We rely only on the explicit axis and rotation definitions (+x left, +y up, +z rear; +x pitches up, +y yaws right, +z rolls right) and verify the signs live.
14. **`LMU_Data` may require "Enable external plugins" = ON** (CLAIMED by LMU Pitwall; contradicted implicitly by TinyPedal and SimHub guides). The local setting is `true`. UNVERIFIED.
15. **Elevation:** the reader and LMU should both run elevated or both non-elevated (CLAIMED "Elevate both or neither"). A medium-integrity process may be unable to open the lock mapping for writing if a high-integrity LMU created it. INFERRED from Windows integrity rules.

## 12. Compatibility risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| **Struct change on patch day.** 1.3 carved new fields out of the `mExpansion` reserves (in `TelemInfoV01`, the block from `mLapInvalidated` @745 to `mVehicleChampionship` @827; also `mOptimalTemp`, `mCompoundIndex`, `mCompoundType` in `TelemWheelV01` and `mSessionTimeRemaining`…`mTrackLimitsStepsPerPoint` in `ScoringInfoV01`). LMU could extend or reshuffle again | Medium per major update | Wrong values that look plausible | Fingerprint check (header SHA-256s) when headers are present. Re-run the probe on every update. Runtime sanity checks (§13). On mismatch the reader stops decoding (D-003, spec §191) |
| `mExpansion` bytes (ScoringInfo 187, TelemInfo 20, Wheel 18, VehicleScoring 4) gain meaning without size changes | Medium | New data ignored (safe) | Diagnostic dumps the expansion bytes when non-zero |
| **104-vehicle arrays**: count > 104, stale slots, slot reuse | Low / Medium | OOB reads (if not clamped), ghost cars | Clamp to 104, join on `mID`, ignore rows beyond the counts |
| **`pack(4)` / default-pack mix**: a Rust `#[repr(C)]` mirror without `packed(4)` would misplace ~25 doubles in `TelemInfoV01`; `/Zp4` in LMU would shift the scoring tail by 4 | High if done naively | Garbage decode | Explicit offsets from the probe JSON with unit tests (D-003). Runtime check that the padding at 2,180 is zero and `scoringStreamSize` at 2,184 is ≤ 65,536 |
| `mBrakePressure` switches to kPa silently (announced "in future") | Medium | Wrong scale | Range detector: values > 1.5 mean kPa. Gate the field on `gameVersion` |
| `mBrakeTemp` unit flip (°C / K) | Already ambiguous | ±273 °C error | Cold-start detector (A-LMU-015) |
| Frame gate semantics change again (1.4.2 was the first version with Hold/Data) | Medium | Reader stalls or spins | 100 ms gate timeout; polling fallback with witness read |
| Lock abandoned by another tool (stuck `busy`) | Low | No data for anyone | Our lock wait is bounded (≤ 5 ms); `lmu-diag` detects and reports it; we never force-reset |
| EAC policy changes (reading `LMU_Data` online) | Low | Online sessions unusable | Read-only access, no process handles beyond `SYNCHRONIZE`; follow official notices |
| REST path churn and crashes | High (history of changes) | Supplementary data missing | Feature flag, GET only, per-endpoint health, graceful `UNAVAILABLE` |
| DuckDB version bump (> 1.4 storage format) | Medium | Import fails | Bundle a recent libduckdb; readable error on version mismatch |

## 13. Runtime verification checklist (Gate 1, `tools/lmu-diag`, LMU running)

1. Confirm the `LMU_Data` region size is ≥ 324,820, and record the exact size.
2. Layout sanity:
   - the 4 bytes at **2,180** are zero;
   - the u64 at **2,184** is ≤ 65,536;
   - `mNumVehicles` at **1,736** is in 0..104;
   - `vehScoringInfo[0].mDriverName` at **2,196** is a printable NUL-terminated string when the count is > 0;
   - `telemInfo[p].mDeltaTime` at **128,472 + 1,888·p** is in (0, 0.1];
   - `telemInfo[p].mVehicleName` at **128,500 + 1,888·p** is printable.
3. Read `generic.gameVersion` at **64** and record the value.
4. Log `events[16]` raw for 60 s: which codes appear and how empty slots look.
5. Measure the telemetry rate, scoring rate, gate wake-up rate, lock wait/hold histograms, and `gate_timeouts` / `lock_timeouts`.
6. Check whether each event (Hold/Data) behaves as manual- or auto-reset, observable from the wake-up pattern of two concurrent waiters, and whether `LMU_Data_Event` is still signalled.
7. Units:
   - `mBrakeTemp` at a cold start;
   - `mSoC` / `mVirtualEnergy` / `mBatteryChargeFraction` ranges on a hybrid Hypercar, an LMGT3 and an LMP2;
   - `mTimeGapCarAhead` / `mTimeGapPlaceAhead` against `mTimeBehindNext`;
   - `mTurboBoostPressure` at idle (≈ 101 kPa?);
   - `mOptimalTemp` magnitude.
8. Signs:
   - `mLocalAccel.x` and `mLocalRot.y` in a known right-hand corner;
   - `mLocalVel.z` < 0 when driving forward;
   - `mRotation` sign when driving forward;
   - tyre inner/outer with negative camber.
9. Map: plot `mPos.x` against `mPos.z` for a lap of Le Mans (clockwise) and confirm it is not mirrored.
10. Test with "Enable external plugins" OFF (the user changes the setting); test online with EAC; test elevated vs non-elevated.

## 14. References

"Last updated" is the GitHub `pushed_at` value or the page date, as retrieved by the community investigation on 2026-09-30. "LMU version tested" is what the project states or what its evidence implies.

| Project | URL | License | Last updated | LMU version tested | Relevant finding | Code reusable? | Why |
|---|---|---|---|---|---|---|---|
| Studio 397 Shared Memory SDK (installed headers) | `<LMU>\Support\SharedMemoryInterface\` | Proprietary: "Redistribution or modification … not permitted" | 2026-09-22 (`SharedMemoryInterface.hpp`) | 1.4.2.0 (installed) | Authoritative names, layout, lock and gate code, unit comments | **No** (reference in place at build time only) | License forbids redistribution. We commit only facts (probe JSON) |
| LMU Release Notes (official) | https://guide.lemansultimate.com/hc/en-gb/categories/13278904445967-Release-Notes | Proprietary docs | 2026-09-24 (v1.4.2.1) | 1.0 → 1.4.2.1 | V1.2: new shared memory, DuckDB, EAC; V1.3: new values; V1.4.2: shared-memory threading fixes | n/a | Documentation |
| LMU guide: Telemetry Recording (official) | https://guide.lemansultimate.com/hc/en-gb/articles/14524956311695-Telemetry-Recording | Proprietary docs | 2026-09-23 | 1.2+ | DuckDB recorder, `config.json` rates, `UserData\Telemetry` | n/a | Documentation |
| LMU V1.2 news post (official) | https://lemansultimate.com/le-mans-ultimate-releases-v1-2-update-with-huge-feature-list-and-latest-elms-content/ | Proprietary | 2025-12-04 | 1.2 | EAC "blocks access to write to memory locations"; plugin system rewritten | n/a | Documentation |
| TinyPedal | https://github.com/TinyPedal/TinyPedal | GPL-3.0 | 2026-09-30 | Native API since 2.36.0; 1.3 support in 2.45.0 | No-lock polling; treats `mBrakeTemp` as K; REST intervals (200 ms, back-off 5 s); MFD flicker and crash reports | **No** | GPL-3.0 is incompatible with our licensing. Facts only |
| pyLMUSharedMemory | https://github.com/TinyPedal/pyLMUSharedMemory | MIT | 2026-09-22 | 1.3 spec (no 1.4.2 events) | Claimed sizes (324,820 = member sum); 1.3 field additions; "copy if events set and counts match" | **No** | MIT, but its structs transcribe the S397 header, so the header restriction applies. Cross-check only |
| LMU Pitwall | https://github.com/Swizzjack/lmu-pitwall | MIT | 2026-08-03 | ~1.4.0.x (probe runs 2026-07-29/30) | Rust + React bridge like ours; `FILE_MAP_READ`, witness read; ~100 Hz / 5 Hz measured; `mSectorFlag` 1 / 11; "Enable Plugins" must be ON | **No** (reference only) | MIT-compatible, but the struct file likely transcribes the header, and we implement our own offset decoder (D-003) |
| lmu-shared-memory (crate) | https://github.com/unblvvv/lmu-shared-memory | MIT | 2026-09-05 | Unstated | Uses the named lock (64-spin `try_lock`) | **No** (not as a dependency) | Immature (v0.1.0, ~1 % documented), header-derived, no gate support |
| apex-lmu | https://github.com/ralfboltshauser/apex-lmu | GPL-3.0 | 2026-09-29 | Not established | Lock with 100 ms wait; `OpenProcess(SYNCHRONIZE)` liveness; mentions `LMU_Data_Event` | **No** | GPL-3.0 |
| KoDriver | https://github.com/ai-kurou/KoDriver | GPL-3.0 | 2026-09-30 | Live REST probing 2026-09 | Best REST survey (65 GET called live); `mBrakeTemp` about 285 K cold | **No** | GPL-3.0. Facts only |
| race-engineer (Alexander-Gro) | https://github.com/Alexander-Gro/race-engineer | Apache-2.0 | 2026-06-17 | 1.3.3.x ("Verified Live 2026-06-16") | REST notes: `getPlayerGarageData` frozen, `[::1]` refused, swagger | **No** (reference only) | Compatible licence, but it uses legacy plugin buffers, so no code is relevant |
| CrewChiefV4 | https://gitlab.com/mr_belowski/CrewChiefV4 | MIT | 2026-09-29 | Up to 4.19.4.11 | Still plugin-based; REST opt-in; "Anticheat prevents reading path of LMU.exe" | **No** | Depends on a third-party plugin (we must not) |
| rF2SharedMemoryMapPlugin (legacy) | https://github.com/TheIronWolfModding/rF2SharedMemoryMapPlugin | GPL-3.0 | 2024-12-31 | rF2 / LMU < 1.2 | 50 FPS telemetry / 5 FPS scoring (**plugin only**) | **No** | GPL-3.0; obsolete for LMU ≥ 1.2 |
| go-lmu-api | https://github.com/snipem/go-lmu-api | None (all rights reserved) | 2026-02-17 | 1.2.x | Client generated from swagger ("all 179 endpoints") | **No** | No licence |
| lmu-telemetry-analyzer | https://github.com/mzluzifer/lmu-telemetry-analyzer | MIT | 2026-06-05 | 1.2+ DuckDB | DuckDB layout documented | **No** (not needed) | We verified the schema ourselves from real files |
| LMU-Telemetry-Lab | https://github.com/rabbit20031225/LMU-Telemetry-Lab | MIT | 2026-08-14 | DuckDB | DuckDB analysis plus live HUD | Not evaluated | Not inspected in depth |
| duckdb_motorsport_telemetry | https://github.com/tobi/duckdb_motorsport_telemetry | MIT | 2026-09-24 | n/a | MoTeC LD / PDS / VBOX parsers (Rust) | **Maybe** (as a dependency after review) | MIT; relevant only if we add `.ld` export |
| LMU-telemetry (alelosbrigia) | https://github.com/alelosbrigia/LMU-telemetry | Custom non-commercial | 2025-12-21 | 1.2 | DuckDB → MoTeC | **No** | Non-commercial licence |
| LMU-DuckDB-Telem-to-MoTec | https://github.com/Ion-Gavriliuc-SimRacing/LMU-DuckDB-Telem-to-MoTec | None | 2025-12-10 | 1.2 | DuckDB → CSV → `.ld` | **No** | No licence |
| SimHub | https://github.com/SHWotever/SimHub (issues only) | Proprietary | 2026-09-17 | 9.11.x | Reads native memory; "Elevate both or neither" | **No** | Closed source |
| isiMotor-Pulse-Plugin | https://github.com/marcgardent/isiMotor-Pulse-Plugin | Apache-2.0 | 2026-09-14 | Unstated | Plugin with per-channel rate limiters | **No** | It is a plugin, and we must not depend on plugins |
| le-mans-ultimate-telemetry (NikMusy) | https://github.com/NikMusy/le-mans-ultimate-telemetry | MIT | 2026-05-30 | Legacy plugin | 50 Hz WebSocket from rF2SMMP | **No** | Plugin-based |
| irdashies (LMU PRs #744 / #747) | https://github.com/tariknz/irdashies | MIT | 2026-09-28 | Unknown | Electron LMU shared-memory bridge | Not evaluated | Not inspected |
| LMU-Overlay (gut0to) | https://github.com/gut0to/LMU-Overlay | MIT | 2026-09-22 | Not tested (LMU was not installed by its author) | Rust crates | **No** | Low confidence |
| LMUTools (JeGoBE8900) | https://github.com/JeGoBE8900/LMUTools | Apache-2.0 | 2025-10-15 (archived) | Pre-1.2 | — | **No** | Archived, outdated |
| Telemetry Tool plugin for LMU | readme in `<LMU>\Plugins\` | Unknown | readme v2.03, 2025-09-28 | — | `$TT_LMU_*$` buffers; "not needed on Telemetry Tool v15.05+" | **No** | Third-party plugin |
| DAMPlugin v2.00 | `<LMU>\Plugins\DAMPlugin.dll` (local) | Unknown | local files 2026-09-27..30 | 1.4.2 (local logs) | Source of the MoTeC `.ld` files in `<LMU>\LOG` | **No** | Third-party plugin |
| MyLMU | https://mylmu.app/docs/telemetry/getting-started/recording-in-lmu | Commercial service | 2026-07-24 | DuckDB era | Uses only LMU's `.duckdb` recordings (~20 MB / 15 min, CLAIMED) | **No** | Commercial |

---

*Verification date: 2026-09-30. Verified by static inspection of LMU 1.4.2.0 (build 25513257); header hashes and exe version were re-checked on this date. LMU was not running. Runtime verification is pending (§13).*
