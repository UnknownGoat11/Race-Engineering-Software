# LMU telemetry interfaces: community and official research

- **Retrieved:** 2026-09-30. Every URL below was fetched on that date unless a row says otherwise.
- **Installed build:** Le Mans Ultimate FileVersion 1.4.2.0 (exe mtime 2026-09-25), Steam app 2399420, build 25513257.
- **Scope:** the native shared memory `LMU_Data`, the in-game REST/WebUI server, native DuckDB recording, community projects, and known pitfalls.
- **Licence rule we follow:** nothing from the Studio 397 headers is copied here. We record only names, offsets, units and short (under 15 word) comment quotes as facts. No third-party code is copied either. Where projects are named, it is so we can check their claims against ours, not to reuse their code.

## Evidence labels

| Label | Meaning |
|---|---|
| **VERIFIED-LOCAL** | I saw it on this machine: the installed header, `Settings.JSON`, the telemetry `config.json`, file versions, or strings inside the shipped binaries. LMU was **not running**, so no live data was observed. |
| **VERIFIED-SOURCE** | Quoted from an official Studio 397 or Motorsport Games source (release notes, guide, news), or read directly in a project's source code. When it is project code, it proves what that project *does*. It does not prove what LMU does. |
| **CLAIMED** | A third party states it (README, forum, changelog, their own measurements), and I could not check it independently. |
| **INFERENCE** | My own reasoning from the evidence. It must be tested before we rely on it. |

---

## 0. Key takeaways for LMU Race Engineering

1. **LMU has its own shared memory, `LMU_Data`, and needs no third-party plugin.**
   - The V1.2 release notes (2025-12-08) say Studio 397 "Added SharedMemory Header file to support folder for new shared memory implementation". (VERIFIED-SOURCE)
   - The header is at `Support\SharedMemoryInterface\`. (VERIFIED-LOCAL)
2. **Telemetry rate: plan for about 100 Hz, but measure it.**
   - `UserData\player\Settings.JSON` has `"Plugin Mask": 4`, commented "100Hz default for performance reasons". (VERIFIED-LOCAL)
   - LMU Pitwall measured about 100 Hz telemetry and about 5 Hz scoring. (CLAIMED)
   - The header gives no telemetry rate. For scoring it says "approximately five times per second". (VERIFIED-LOCAL)
   - I found no source for 400 Hz or 50 Hz on `LMU_Data`. The 50 FPS figure belongs to the legacy rF2 plugin.
3. **Synchronisation is new as of 1.4.2.**
   - The header now dated 2026-09-22 adds `LMU_Data_HoldEvent` + `LMU_Data_DataEvent` and a `WaitForNextFrame` helper. The same day's release notes say "Fixed some threading issues with shared memory". (VERIFIED-LOCAL, VERIFIED-SOURCE)
   - None of the community readers I inspected use these events yet:
     - TinyPedal and pyLMUSharedMemory poll without the lock.
     - LMU Pitwall uses a "witness read".
     - apex-lmu and the `lmu-shared-memory` crate use the named lock.
   - We would be early adopters. Test carefully.
4. **REST API on `http://127.0.0.1:6397`.** It fills gaps that shared memory does not cover: weather forecast, virtual-energy history, pit menu, damage/wear, pit-stop time estimate.
   - Studio 397 does not document the REST API. Its schema is at `/swagger-schema.json`: 179 paths, 79 GET. (CLAIMED)
   - `Settings.JSON` has `"WebUI bind": "127.0.0.1"` and `"WebUI port": 6397`. (VERIFIED-LOCAL)
   - Known hazards (CLAIMED by TinyPedal and CrewChief):
     - Polling `RepairAndRefuel` often can make the MFD flicker.
     - Some endpoints crash the game.
     - The paths changed with game updates.
     - `localhost` resolution broke after 1.3.3.
5. **Native DuckDB recording** has been in the game since V1.2 and is documented officially.
   - Files go to `UserData\Telemetry\*.duckdb`, and rates are configured per channel in `UserData\Telemetry\config.json`. (VERIFIED-SOURCE, VERIFIED-LOCAL)
   - The game bundles `duckdb.dll` v1.4.0. (VERIFIED-LOCAL)
   - The file layout is one table per channel/event plus `metadata`, `channelsList` and `eventsList`. (CLAIMED)
6. **Anti-cheat:**
   - Easy Anti-Cheat ships with 1.2+ and online play requires it. The official post says EAC "blocks access to write to memory locations". (VERIFIED-SOURCE)
   - Reading `LMU_Data` is the mechanism Studio 397 ships an SDK for. No official statement forbids external readers online, but none explicitly allows them either (see §5.7).
   - Do not open the LMU process with anything beyond `SYNCHRONIZE`. CrewChief reports that EAC blocks reading the exe path. (CLAIMED)
7. **Build-environment note:**
   - The header `#include <windows.h>`, but this machine has **no Windows SDK**. `C:\Program Files (x86)\Windows Kits\10` is absent and vcvars64 reports "vswhere.exe is not recognized". (VERIFIED-LOCAL)
   - Any compile-the-header size check needs the Windows 10/11 SDK installed first.
   - I could not independently verify struct sizes. See pyLMUSharedMemory's claimed sizes in §1.5.

---

## 1. Native shared memory interface (`LMU_Data`)

### 1.1 Timeline (official release notes)

**Source:** Zendesk JSON API for the Release Notes category 13278904445967 at `guide.lemansultimate.com/api/v2/help_center/...` (the HTML pages return 403 to WebFetch). I searched all 51 articles for telemetry keywords. Status: VERIFIED-SOURCE.

| Version | Notes article date | Relevant official line(s) (short quotes) |
|---|---|---|
| V1.0.0.0 | 2025-07-18 | Release. |
| **V1.2 (Update 2)** | 2025-12-08 | "Added SharedMemory Header file to support folder for new shared memory implementation"; "Added Telemetry Recording to DuckDB"; "Disabled online access if Easy Anti-Cheat is not enabled". |
| V1.2.1 | 2025-12-16 | "Signed PluginAdapter.exe to prevent Anti Virus issues"; "Fixed broken timestamps on telemetry events". |
| V1.2.2.0 | 2026-01-19 | "Added toggle for Enable Plugins to Settings page". |
| V1.2.3.0 | 2026-02-09 | "Fixed last 10 seconds of telemetry not saving to file" (DuckDB). |
| **V1.3.0.0** | 2026-03-30 | "Added various new values to plugin system"; "Various fixes to telemetry output". The release news (via racecontrol.gg, 2026-03-30) says new, highly requested telemetry parameters were added to the shared memory file. |
| V1.3.3 | 2026-06-04 | Nothing about telemetry in the notes. TinyPedal 2.47.0 says hostname auto-resolve is "required for accessing LMU Rest API after v1.3.3" (CLAIMED). |
| V1.4 | 2026-07-27 | Nothing about telemetry or shared memory found by keyword search. |
| **V1.4.2** | 2026-09-22 | "Fixed some threading issues with shared memory"; "Blocked online mode when EAC ProductID is invalid". |
| v1.4.2.1 | 2026-09-24 | Hotfix; nothing about telemetry. |

**Official V1.2 news post** (lemansultimate.com, published 2025-12-04; fetched with curl because WebFetch got 403):
- EAC "blocks access to write to memory locations".
- "we had to rewrite some of our plugin system".
- The DuckDB telemetry system was "Designed for use internally to validate our physics direction".

### 1.2 Facts from the installed SDK (VERIFIED-LOCAL)

The folder is `Support\SharedMemoryInterface\`. File mtimes (Steam appears to keep depot mtimes: the install date is 2026-08-11, yet one file is dated March):

| File | mtime | Meaning |
|---|---|---|
| `InternalsPlugin.hpp` | 2026-03-31 | Unchanged since the 1.3.0.x hotfix (INFERENCE) |
| `PluginObjects.hpp` | 2025-12-20 | — |
| `SharedMemoryInterface.hpp` | 2026-09-22 | Matches V1.4.2 |

All three carry "Redistribution or modification of this header is not permitted."

**Named objects**

| Object | Name | What it is |
|---|---|---|
| Data mapping | `LMU_Data` | The layout wraps one `SharedMemoryObjectOut` (generic, paths, scoring, telemetry). |
| Frame events (new) | `LMU_Data_HoldEvent`, `LMU_Data_DataEvent` | The header opens both with `OpenEventA(SYNCHRONIZE, ...)`. `WaitForNextFrame(ms)` waits Hold, then Data, with the same timeout. A header comment calls this "the only order that's race-free". |
| Lock mapping | `LMU_SharedMemoryLockData` | `{ LONG waiters; LONG busy; }` |
| Lock event | `LMU_SharedMemoryLockEvent` | Auto-reset event (`CreateEventA(NULL, FALSE, FALSE, ...)`) |

**How the lock works (header code):**
- **Acquire:** spin up to 4000 times with `InterlockedCompareExchange(busy, 1, 0)`. Then increment `waiters` and loop on `WaitForSingleObject(event, timeout)`.
- **Release:** set `busy` to 0, then `SetEvent` if `waiters > 0`.
- **Creation:** the helper *creates* the lock mapping and event (`CreateFileMappingA` / `CreateEventA`). So a reader that starts before LMU creates them itself.

**Header usage example (paraphrased):**
- The example is a child process that receives the LMU PID on its command line.
- It builds the lock and the waiter, then opens `LMU_Data` via `OpenFileMapping(FILE_MAP_ALL_ACCESS, ...)`.
- It polls `WaitForSingleObject(hParent, 0)` to notice LMU exiting, and calls `WaitForNextFrame(200)` in a loop.

**`SharedMemoryEvent` enum** (16 entries, one `uint32` each in `generic.events[]`):
- `SME_ENTER`, `EXIT`, `STARTUP`, `SHUTDOWN`, `LOAD`, `UNLOAD`, `START_SESSION`, `END_SESSION`, `ENTER_REALTIME`, `EXIT_REALTIME`, `UPDATE_SCORING`, `UPDATE_TELEMETRY`, `INIT_APPLICATION`, `UNINIT_APPLICATION`, `SET_ENVIRONMENT`, `FFB`.
- The header's copy helper maps them to parts:
  - `UPDATE_SCORING` → scoring
  - `UPDATE_TELEMETRY` → telemetry
  - `ENTER`, `EXIT`, `SET_ENVIRONMENT` → paths

**Containers**

| Struct | Contents |
|---|---|
| `SharedMemoryScoringData` | `ScoringInfoV01`, `size_t scoringStreamSize`, `VehicleScoringInfoV01 vehScoringInfo[104]`, `char scoringStream[65536]` |
| `SharedMemoryTelemetryData` | `uint8_t activeVehicles`, `uint8_t playerVehicleIdx`, `bool playerHasVehicle`, `TelemInfoV01 telemInfo[104]` |
| `SharedMemoryPathData` | five `char[MAX_PATH]`: `userData`, `customVariables`, `stewardResults`, `playerProfile`, `pluginsFolder` |
| `SharedMemoryGeneric` | `events[16]`, `long gameVersion`, `float FFBTorque`, `ApplicationStateV01 appInfo` |

`SharedMemoryPathData` gives us the UserData path **without touching the LMU process**. This matters because of the EAC issue in §5.7.

**Packing:** `#pragma pack(push, 4)`.
- `ScoringInfoV01` still contains raw pointers (`mResultsStream`, `mVehicle`). These are 8 bytes on x64 and meaningless in another process.

**Unit comments in the header (short quotes):**

| Field | Header comment |
|---|---|
| `mBrakePressure` | "currently 0.0-1.0 ... will convert to true brake pressure (kPa) in future" |
| `mTemperature[3]` | "Kelvin ... left/center/right (not to be confused with inside/center/outside!)" |
| `mBrakeTemp` | "Celsius" (but see §5.2: the community treats it as Kelvin) |
| `mCurrentSector` | pit lane stored in the sign bit, example "0x80000002" |
| `mSector` | "0=sector3, 1=sector1, 2=sector2 (don't ask why)" |
| `mInPits` | "not always accurate for remote vehicles" |
| `mSectorFlag[3]` | "not sure if sector 0 is first or last, so test" |
| `mFuelFraction` (vehicle scoring) | `unsigned char`, 0x00 = 0% to 0xFF = 100% of fuel *or battery* |

**Binary strings** (VERIFIED-LOCAL: extracted from the shipped binaries):
- `Le Mans Ultimate.exe` contains `LMU_Data`, `LMU_Data_HoldEvent`, `LMU_Data_DataEvent`, **`LMU_Data_Event`**, `LMU_SharedMemoryLockData`, `LMU_SharedMemoryLockEvent`, `Plugin Mask`, `PluginsAdapter.exe` and `Global\LMU_PluginsAdapter_Ready`, plus messages such as "Could not start PluginsAdapter process".
- `PluginsAdapter.exe` (mtime 2026-09-25, 162,744 bytes) contains:
  - the same shared-memory names;
  - "Cannot connect to shared memory update gate.";
  - the plugin-DLL entry points `GetPluginName`, `GetPluginVersion`, `CreatePluginObject`;
  - `CustomPluginVariables.JSON`.
- **INFERENCE:** since 1.2, legacy `InternalsPlugin` DLLs run *out of process* inside `PluginsAdapter.exe`, which is fed from `LMU_Data`. This fits the official "rewrite some of our plugin system" for EAC and the header's "child.exe <LMU-pid>" example. So even legacy plugins (rF2SMMP, CrewChief's `LMU.3.8.SharedMemoryMapPlugin.dll`) are now downstream of `LMU_Data`.
- `LMU_Data_Event` is **not** in the current header. apex-lmu's bridge README describes `LMU_Data_Event` as "an update notification". **INFERENCE:** this is the pre-1.4.2 single event, kept in the exe for compatibility. Unverified whether it is still signalled.

### 1.3 Update rates: what each source says

| Rate | Source | Status |
|---|---|---|
| Scoring about 5 Hz | Header comment on `UpdateScoring`: "approximately five times per second" | VERIFIED-LOCAL (plugin callback). The link to the `LMU_Data` write rate is an INFERENCE. |
| Scoring 5 Hz | LMU Pitwall README ("100 Hz telemetry, 5 Hz scoring") and its `lmu-probe` tool, which times `mCurrentET` at a 200 Hz sampling rate | CLAIMED (measured by them) |
| Scoring about 5 Hz | TinyPedal code comment "(5fps refresh rate from API)" | VERIFIED-SOURCE (their comment) |
| Telemetry rate | Header: no rate stated for `UpdateTelemetry` | VERIFIED-LOCAL (absence) |
| Telemetry 100 Hz default | `Settings.JSON`: `"Plugin Mask": 4`, comment "Plugin mask to control plugin refresh rate. 100Hz default for performance reasons. Faster rates might be helpful for external hardware". What the other mask values mean is **unknown**. | VERIFIED-LOCAL |
| Telemetry 100 Hz, "10 ms write interval" | LMU Pitwall `reader.rs` comments; `lmu-probe` times player `mElapsedTime` | CLAIMED |
| Telemetry poll 10 ms | TinyPedal `lmu_sharedmemory.py` sets `update_delay = 0.01` while active and 0.5 s when idle | VERIFIED-SOURCE (reader behaviour, not the producer rate) |
| 10–100 Hz sampling | apex-lmu bridge README | CLAIMED |
| "higher refresh rate of native shared memory" (than the plugin) | SimHub statement quoted in a web-search snippet about the SimHub migration; I could not open the primary post | CLAIMED, weak |
| 50 FPS telemetry, 5 FPS scoring, 400 FPS FFB/graphics | TheIronWolf rF2SharedMemoryMapPlugin README ("Telemetry - 50FPS", "Scoring - 5FPS") | VERIFIED-SOURCE, but for the **legacy plugin only** |
| 400 Hz telemetry on `LMU_Data` | No source found | Treat as unsupported |

**Recommendation (INFERENCE):**
- Drive reads from `WaitForNextFrame`, falling back to a 10 ms poll.
- Log the real rate from successive player `mElapsedTime` / `mDeltaTime` values.
- Show the measured rate in the UI.
- Do not hardcode 100 Hz.

### 1.4 How existing readers stay consistent

| Project | Lock? | Hold/Data events? | Consistency strategy | Evidence |
|---|---|---|---|---|
| pyLMUSharedMemory / TinyPedal | No | No | Copy the whole mapping only if `SME_UPDATE_SCORING` or `SME_UPDATE_TELEMETRY` is non-zero **and** `scoringInfo.mNumVehicles == telemetry.activeVehicles` ("helps avoid data desync") | VERIFIED-SOURCE (`lmu_mmap.py` @ 2026-09-22) |
| LMU Pitwall (Rust) | No | No | "Witness read": snapshot `mCurrentET`, player `mElapsedTime` and both car counts before and after a partial copy; retry up to 5 times. They observed a scoring/telemetry car-count disagreement in "0.04–0.08% of samples", always while cars were joining. | VERIFIED-SOURCE (code); the rates are CLAIMED |
| `lmu-shared-memory` crate v0.1.0 (Rust, MIT) | Yes (`try_lock`, 64 spins) | No | Guarded copy | VERIFIED-SOURCE (`src/shared_memory.rs`) |
| apex-lmu (Go bridge) | Yes (100 ms wait) | No (documents `LMU_Data_Event` only) | Copy under the lock; liveness via `OpenProcess(SYNCHRONIZE)` on the LMU PID | VERIFIED-SOURCE |

**Takeaways:**
- None of these use the 1.4.2 `HoldEvent`/`DataEvent` pair yet.
- **INFERENCE:** holding the SDK lock blocks LMU's own writer, since the header's lock is shared by producer and consumer. Copy as fast as possible (memcpy into a private buffer), release, then decode.

### 1.5 Struct changes by version

**1.3 additions.** Source: pyLMUSharedMemory commit `0e335d1719` on 2026-03-31, "updated LMU API to 1.3 spec". It carves these fields out of `mExpansion` (CLAIMED as "1.3"). I confirmed every field below in the installed 1.4.2 header (VERIFIED-LOCAL).

- **`TelemWheelV01`:**
  - `float mOptimalTemp`
  - `uchar mCompoundIndex`
  - `uchar mCompoundType` (pyLMU: 0 soft, 1 medium, 2 hard, 3 wet)
  - `mExpansion[18]`
- **`TelemInfoV01`:**
  - `mLapInvalidated`, `mABSActive`, `mTCActive`, `mSpeedLimiterActive`, `mWiperState`
  - `mTC`/`mTCMax`, `mTCSlip`/`mTCSlipMax`, `mTCCut`/`mTCCutMax`, `mABS`/`mABSMax`, `mMotorMap`/`mMotorMapMax`, `mMigration`/`mMigrationMax`, `mFrontAntiSway`/`mFrontAntiSwayMax`, `mRearAntiSway`/`mRearAntiSwayMax`
  - `mLiftAndCoastProgress`, `mTrackLimitsSteps`
  - `float mRegen` (header: kW), `float mSoC`, `float mVirtualEnergy`
  - `float mTimeGapCarAhead`, `mTimeGapCarBehind`, `mTimeGapPlaceAhead`, `mTimeGapPlaceBehind`
  - `char mVehicleModel[30]`
  - `IP_VehicleClass mVehicleClass` (uint8 enum)
  - `IP_VehicleChampionship mVehicleChampionship`
  - `mExpansion[20]`
  - The header gives **no units** for `mSoC`, `mVirtualEnergy` or the gap fields. pyLMU names `mSoC` `mStateOfCharge`, "battery state of charge (percent)", and says `mVirtualEnergy` is a "fraction" (CLAIMED).
- **`ScoringInfoV01`:**
  - `float mSessionTimeRemaining`, `float mTimeOfDay`, `bool mIsFixedSetup`
  - `uint8 mTrackGripLevel`, `uint8 mCloudCoverage`
  - `uint8 mTrackLimitsStepsPerPenalty`, `uint8 mTrackLimitsStepsPerPoint`
  - `mExpansion[187]`
- **Enums** (VERIFIED-LOCAL):
  - `IP_VehicleClass`: Hypercar=0x00, LMP2_ELMS=0x02, LMP2=0x03, LMP3=0x04, GTE=0x05, GT3=0x06, PaceCar=0x08, Unknown=0xFF.
  - `IP_VehicleChampionship`: WEC_2023..WEC_2026=0x00..0x03, ELMS_2025=0x10, ELMS_2026=0x11, Unknown=0xFF.

**Struct sizes claimed by pyLMUSharedMemory `tests/read_lmu_api.py`** (CLAIMED; not compiled here because the Windows SDK is missing):

| Struct | Bytes |
|---|---|
| Wheel | 260 |
| VehicleTelemetry | 1888 |
| VehicleScoring | 584 |
| ScoringInfo | 548 |
| ApplicationState | 260 |
| ScoringData | 126,832 |
| TelemetryData | 196,356 |
| PathData | 1,300 |
| Event array | 64 |
| Generic | 332 |
| **ObjectOut / Layout** | **324,820** |

KoDriver's `docs/lmu-windows-telemetry.md` gives the same 324,820-byte total (CLAIMED). Our Rust bindings should assert these sizes at compile time.

**1.4 / 1.4.1 / 1.4.2:**
- The release notes show no struct change.
- `InternalsPlugin.hpp` mtime 2026-03-31 suggests it has not changed since 1.3 (INFERENCE).
- pyLMU commits from 2026-09-14 to 2026-09-22 are refactors ("embed type annotation…", "cleanup"), not layout changes (VERIFIED-SOURCE, commit messages).
- **What changed in 1.4.2** was `SharedMemoryInterface.hpp`, which gained the Hold/Data waiter and "threading" fixes.

---

## 2. Maintained projects that read LMU telemetry

Last-updated values are GitHub `pushed_at` (API), fetched 2026-09-30.

**Reuse column:** we never copy code, only use projects as references.
- **GPL-3.0:** incompatible with a proprietary app. Even for an MIT app it would force GPL on the combined work. Treat as facts-only.
- **MIT / Apache-2.0:** compatible if attribution is kept (plus NOTICE for Apache).
- **Header transcriptions:** any struct definitions that transcribe the S397 header may carry S397's "no redistribution/modification" restriction whatever the project's own licence. Always generate our own bindings from the installed header.

| Project | URL | Licence | Last updated | LMU version tested | Relevant finding | Code reusable? |
|---|---|---|---|---|---|---|
| **TinyPedal** | https://github.com/TinyPedal/TinyPedal | GPL-3.0 | 2026-09-30 | Built-in API since 2.36.0 (2025-12-13); "LMU's 1.3 updates" (2.45.0, 2026-04-01) | Native `LMU_Data` via pyLMU; REST task list and intervals (§3); treats `mBrakeTemp` as Kelvin; maps `mSector` 0→S3; deprecates the legacy rF2 plugin API; changelog records REST flicker and crash issues. | **No** (GPL). Facts only. |
| **pyLMUSharedMemory** | https://github.com/TinyPedal/pyLMUSharedMemory | MIT | 2026-09-22 | 1.3 spec (2026-03-31); no 1.4.2 event support | ctypes structs and claimed sizes; no-lock "copy when events set and vehicle counts match" read strategy. | Licence-compatible with attribution, **but** its struct file transcribes the S397 header; use it only to cross-check sizes and names. |
| **rF2SharedMemoryMapPlugin** (TheIronWolf, legacy) | https://github.com/TheIronWolfModding/rF2SharedMemoryMapPlugin | GPL-3.0 | 2024-12-31 | rF2; used with LMU before 1.2 | Legacy `$rFactor2SMMP_*$` buffers with `mVersionUpdateBegin/End`; README rates: Telemetry 50 FPS, Scoring 5 FPS. **Not needed for LMU ≥1.2.** | **No** (GPL); obsolete for us. |
| **CrewChiefV4** | https://gitlab.com/mr_belowski/CrewChiefV4 | MIT (GitLab API) | 2026-09-29 | Changelog up to 4.19.4.11 | Still reads its own plugin buffers `$LMU.3.8.SMMP_*$` (`LMUData.cs`) via `LMU.3.8.SharedMemoryMapPlugin.dll` (present in our Plugins folder). REST base `http://localhost:6397` with an opt-in toggle. Changelog: "Enable LMU's problematic REST API (default unchecked)"; "Don't hammer REST API if it fails, may fix reports of causing LMU to lag"; "Shared memory update, used for Virtual Energy instead of REST API"; "Anticheat prevents reading path of LMU.exe". | Licence-compatible, but plugin-based; reference only. |
| **SimHub** | https://github.com/SHWotever/SimHub (issue tracker only) | Proprietary (no licence on repo) | 2026-09-17 | 9.11.x | Moving to native shared memory (search snippet, CLAIMED). Issue #2264 (2026-05-16, LMU 1.3.000): rF2 plugin DLL not loading. OESimRacing (2026-08-04) says SimHub reads native memory with nothing to enable in game, "Elevate both or neither". | **No** (closed source). |
| **LMU Pitwall** | https://github.com/Swizzjack/lmu-pitwall | MIT | 2026-08-03 | Probe runs 2026-07-29 (Monza) and 2026-07-30 (Sebring), so V1.4.0.x | Rust bridge plus React/PWA. Reads `LMU_Data` with `FILE_MAP_READ`, no lock, witness read. REST: `/rest/strategy/usage`, `/rest/garage/UIScreen/RepairAndRefuel`, `/rest/sessions/weather` (ureq, 2 s timeout). `tools/lmu-probe` for layout validation. Claims (a) "Enable Plugins" must be ON for `LMU_Data`, (b) `mSectorFlag` 11 = clear, 1 = yellow, (c) no safety-car flag in `LMU_Data`. | MIT, but the architecture is closest to ours; reference only (their struct file may transcribe the header). |
| **`lmu-shared-memory` crate** | https://github.com/unblvvv/lmu-shared-memory (docs.rs `lmu-shared-memory` 0.1.0, 2026-09-02) | MIT | 2026-09-05 | Unstated | Safe Rust reader using the named lock (64-spin `try_lock`); about 1% of the API documented. | Compatible, but immature and header-derived; reference only. |
| **apex-lmu** | https://github.com/ralfboltshauser/apex-lmu | GPL-3.0 (GitHub API; its README says GPL-3.0-or-later) | 2026-09-29 | "compatibility with a current installed LMU build has not yet been established" | Go bridge: named lock with 100 ms wait; 10–100 Hz sampling; `OpenProcess(SYNCHRONIZE)` for liveness; mentions `LMU_Data_Event`; reads DuckDB read-only; `.apexrec` capture format. | **No** (GPL). |
| **KoDriver** | https://github.com/ai-kurou/KoDriver | GPL-3.0 | 2026-09-30 | Real-hardware REST probing, 2026-09 | Best public REST survey: `docs/lmu-windows-rest-api.md` (Japanese; 65 GET paths called live). Also `docs/lmu-windows-telemetry.md` (`LMU_Data` offsets; measured `mBrakeTemp` ≈ 280–290 K when cold). | **No** (GPL). Facts only. |
| **race-engineer** (Alexander-Gro) | https://github.com/Alexander-Gro/race-engineer | Apache-2.0 | 2026-06-17 | "Verified Live 2026-06-16" (V1.3.3.x) | `docs/03-LMU-INTEGRATION.md`: REST endpoints, `/swagger-schema.json`, `getPlayerGarageData` "is a FROZEN garage snapshot", IPv6 `[::1]` refused. Uses the *legacy plugin* SHM names, not `LMU_Data`. | Compatible (Apache-2.0); reference only. |
| **go-lmu-api** | https://github.com/snipem/go-lmu-api | None (all rights reserved) | 2026-02-17 | 1.2.x era | Generates a Go client from `/swagger-schema.json`: "all 179 endpoints". | **No** (no licence). |
| **isiMotor-Pulse-Plugin** | https://github.com/marcgardent/isiMotor-Pulse-Plugin | Apache-2.0 | 2026-09-14 | Unstated | Third-party *plugin* (TCP/ZeroMQ/flatbuffers) for LMU/rF2 with per-channel rate limiters. Plugin-based, so not for us. | Compatible, but we must not depend on plugins. |
| **lmu-telemetry-analyzer** (mzluzifer; fork joevinsirach) | https://github.com/mzluzifer/lmu-telemetry-analyzer | MIT | 2026-06-05 (fork 2026-09-24) | 1.2+ DuckDB | Documents the DuckDB layout: a table per channel/event (`value` or `value1..4` = FL, FR, RL, RR), plus `metadata` (incl. `CarSetup` JSON), `channelsList(channelName, frequency, unit)`, `eventsList(eventName, unit)`; event tables have `ts`. Reads via `duckdb.exe -readonly`. | Compatible; reference for the schema. |
| **LMU-Telemetry-Lab** | https://github.com/rabbit20031225/LMU-Telemetry-Lab (fork of kolipkaza/lmu-telemetry-lab) | MIT | 2026-08-14 | DuckDB | DuckDB analysis plus live HUD. | Compatible; not inspected in depth. |
| **LMU-telemetry** (DuckDB → MoTeC) | https://github.com/alelosbrigia/LMU-telemetry | Custom **non-commercial** | 2025-12-21 | 1.2 | DuckDB → a single `.ld`; per-group resampling. | **No** (non-commercial). |
| **LMU-DuckDB-Telem-to-MoTec** | https://github.com/Ion-Gavriliuc-SimRacing/LMU-DuckDB-Telem-to-MoTec | None | 2025-12-10 | 1.2 | DuckDB → CSV resampled at a 0.01 s step → MotecLogGenerator → `.ld`/`.ldx`. | **No** (no licence). |
| **duckdb_motorsport_telemetry** | https://github.com/tobi/duckdb_motorsport_telemetry | MIT | 2026-09-24 | n/a | DuckDB extension plus Rust parsers for the MoTeC LD, Cosworth PDS and VBOX formats. Useful if we export `.ld`. | Compatible (MIT); candidate dependency after review. |
| **le-mans-ultimate-telemetry** (NikMusy) | https://github.com/NikMusy/le-mans-ultimate-telemetry | MIT | 2026-05-30 | Legacy plugin | Needs rF2SMMP; streams at 50 Hz over WebSocket. | Compatible, but plugin-based. |
| **irdashies** (LMU PRs #744/#747) | https://github.com/tariknz/irdashies | MIT | 2026-09-28 | Unknown | Electron/TypeScript LMU shared-memory bridge in PRs. Not inspected. | Compatible; not inspected. |
| **LMU-Overlay** (gut0to) | https://github.com/gut0to/LMU-Overlay | MIT | 2026-09-22 | Its own research doc says LMU was not installed on the dev machine | Rust crates `lmu-telemetry`, `telemetry-engine`. | Compatible; low confidence. |
| **JeGoBE8900/LMUTools** | https://github.com/JeGoBE8900/LMUTools | Apache-2.0 | 2025-10-15 (**archived**) | Pre-1.2 | — | Archived; skip. |
| **Telemetry Tool plugin for LMU** (Telemetry Tool author) | readme in our `Plugins` folder | Unknown | Readme v2.03, 2025-09-28 | — | Creates `$TT_LMU_Telemetry$` / `$TT_LMU_Scoring$`. The readme says Telemetry Tool 15.05 "is now using the Shared Memory directly", which explains `PLUGIN_NOT_NEEDED_ON_v1505_OR_LATER.txt` (VERIFIED-LOCAL). | Not applicable. |

**Also checked:**
- **MyLMU** (https://mylmu.app, commercial web service): uses "LMU's built-in .duckdb recordings only"; about 20 MB per 15 min (CLAIMED, page dated 2026-07-24).
- **DR Sim Manager docs:** still document the rF2 plugin route (outdated).
- **lemansultimate.wiki.gg/wiki/Telemetry:** last edited 2025-10-27. Outdated and partly wrong (claims `.ld/.csv` exports). Do not use.

---

## 3. LMU REST API (in-game WebUI server)

**Base URL and binding:**
- `Settings.JSON` → `"WebUI port": 6397`.
- `"WebUI bind": "127.0.0.1"`, commented "Use 'localhost' for local access only, or '0.0.0.0' for open access for broadcasting". (VERIFIED-LOCAL)
- The official Broadcast Overlay guide (updated 2026-09-05) uses `http://localhost:6397/overlays/controlpanel.html` and `/overlays/overlays.html`. It says you can put another computer's IP in place of `localhost`. (VERIFIED-SOURCE)
- Access from other LAN machines times out with the default bind (KoDriver and race-engineer, CLAIMED; matches the default above).

**Official docs:** none published. `/swagger-schema.json` (OpenAPI 2.0; Swagger UI at `/swagger/index.html`) is the de facto reference: 179 paths, 79 GET, 107 paths with write methods (KoDriver measured this on real hardware; go-lmu-api reports the same; CLAIMED).
- No auth, plain HTTP, no WebSocket/push.
- `Content-Type: text/plain` with **no charset**. Bodies are UTF-8 and must be decoded explicitly (KoDriver, CLAIMED).

**Endpoints relevant to telemetry.** All are GET. Latency is not measured in any source I found (LMU was not running here, so I did not measure either). "Source" is where the path is used or documented.

| Path | Returns (summary) | Rate / latency notes | Source(s) | Status |
|---|---|---|---|---|
| `/rest/watch/standings` | Array, one record per car: driver/team/vehicle/class, position, laps, `lapDistance`, sector times, `timeBehindLeader/Next/ClassLeader`, `carPosition`/`carVelocity`/`carAcceleration`, `fuelFraction`, `veFraction`, `pitState` (string), `flag`, `penalties`, `pathLateral`, `trackEdge`, `slotID`, etc. | — | KoDriver (live sample); race-engineer; go-lmu-api standings TUI | CLAIMED |
| `/rest/watch/standings/history` | Standings history | — | KoDriver | CLAIMED |
| `/rest/watch/sessionInfo` | One object: `session` (e.g. `PRACTICE1`), `trackName`, `ambientTemp`, `currentEventTime`, `startEventTime`, `endEventTime`, `maxTime`, `raceCompletion.timeCompletion`, `sectorFlag[3]` (strings), `yellowFlagState`, `lapDistance` | — | KoDriver (live sample) | CLAIMED |
| `/rest/watch/trackmap` | Track-map coordinates | — | KoDriver | CLAIMED |
| `/rest/sessions` | `SESSSET_*` settings objects (`currentValue`, `stringValue`, …), e.g. `SESSSET_race_timescale`, `SESSSET_private_qual` | TinyPedal: one-shot, min interval 0.1 s | TinyPedal `lmu_restapi.py` | VERIFIED-SOURCE (usage) |
| `/rest/sessions/weather` | Forecast by session key (`PRACTICE`/`QUALIFY`/`RACE`), with nodes `START`, `NODE_25`, `NODE_50`, `NODE_75`, `FINISH` and `WNV_*` values (`WNV_SKY`, `WNV_TEMPERATURE`, `WNV_RAIN_CHANCE`, `WNV_HUMIDITY`, `WNV_WINDSPEED`, `WNV_WINDDIRECTION`) | TinyPedal one-shot; Pitwall 2 s timeout | TinyPedal; LMU Pitwall `garage_api.rs`; KoDriver | VERIFIED-SOURCE (usage) |
| `/rest/sessions/GetGameState` | Enum strings: `gamePhase`, `PitState`, `MultiStintState`, `timeOfDay`, `closeestWeatherNode` (typo is real per KoDriver) | — | race-engineer; KoDriver | CLAIMED |
| `/rest/garage/getPlayerGarageData` | Garage/setup values (`VM_*`, e.g. `VM_STEER_LOCK`, `VM_VIRTUAL_ENERGY`, TC/ABS map, brake balance). **Frozen snapshot: does not update while driving.** | TinyPedal one-shot | TinyPedal; race-engineer ("FROZEN garage snapshot") | VERIFIED-SOURCE (usage) / CLAIMED (frozen) |
| `/rest/garage/UIScreen/RepairAndRefuel` | `wearables` (`body.aero`, per-wheel `brakes`, `suspension`, 0..1 damage/wear), `fuelInfo` (incl. `maxVirtualEnergy`, `currentVirtualEnergy`), `pitMenu`, `pitRecommendations`, `pitStopLength`, `weatherForecast`, `sessionTime` | TinyPedal: repeated, min 0.2 s with backoff up to 5 s. **High-frequency polling can make the in-game MFD pit menu flicker**, "acknowledged by game developer, but not yet fixed" (TinyPedal 2.30.1, June 2025). | TinyPedal; LMU Pitwall; race-engineer; KoDriver | VERIFIED-SOURCE (usage) / CLAIMED (flicker) |
| `/rest/strategy/usage` | Object keyed by driver display name → array of `{fuel, lap, pit, stint, tyres[4], ve}` (per-lap VE/fuel history) | — | LMU Pitwall; KoDriver (live sample) | VERIFIED-SOURCE (usage) |
| `/rest/strategy/pitstop-estimate` | Pit-stop time breakdown: `total`, `damage`, `fuel`, … | TinyPedal: repeated, min 1.0 s | TinyPedal; KoDriver | VERIFIED-SOURCE (usage) |
| `/rest/strategy/overall` | Strategy summary (empty body in KoDriver's single-car test) | — | KoDriver | CLAIMED |
| `/rest/garage/UIScreen/TireManagement` | Tyre-set inventory and conditions | — | race-engineer; KoDriver | CLAIMED |
| `/rest/garage/getVehicleCondition`, `/rest/garage/tireinfo`, `/rest/garage/brakeinfo`, `/rest/garage/PitMenu/receivePitMenu` | Vehicle condition, tyre and brake info, pit-menu state | — | KoDriver | CLAIMED |
| `/rest/options/liveInputs` | Live processed inputs (throttle, brakes, clutch, steer) plus raw device values | — | KoDriver | CLAIMED |
| `/swagger-schema.json` | OpenAPI 2.0 schema for the running build | — | race-engineer; KoDriver; go-lmu-api | CLAIMED |

**Polling guidance and hazards** (all CLAIMED unless marked):
- **TinyPedal defaults** (VERIFIED-SOURCE, `template/setting_api.py`):
  - `url_host "localhost"`, `url_port 6397`
  - `restapi_update_interval 200` ms (floor 100 ms)
  - `connection_timeout 1` s, 3 retries, 1 s retry delay
  - per-endpoint back-off to at most 5 s when the data hash is unchanged
- **Game crashes:** TinyPedal 2.29.1 says some URIs "(such as "garage/Pitstop/getPitstopTimes") are buggy and can crash game". KoDriver also saw an LMU crash while experimenting with write endpoints.
- **Path churn:** TinyPedal 2.30.1 updated its URLs "due to changes from June update". CrewChief 4.19.1.8 says "S397 changed the REST API again".
- **Hostname:** TinyPedal 2.47.0 needed hostname auto-resolve after v1.3.3. race-engineer saw IPv6 `[::1]` refused. **Use `127.0.0.1` explicitly** (matches VERIFIED-LOCAL `WebUI bind`).
- **Poll rate:** community practice is about 1–5 Hz with caching (KoDriver, race-engineer). REST is not a telemetry-rate channel.
- **Remaining VE:** TinyPedal (2.32.1, July 2025) says remaining VE from REST "is updated only when driver completes a lap". TinyPedal 2.50.0 (2026-08-31) switched to the native `mVirtualEnergy`. CrewChief did likewise.

**Design implication (INFERENCE):**
- Our sim-PC agent should call REST **locally** at 127.0.0.1 and only with GET.
- Never ask users to set `WebUI bind` to `0.0.0.0`: that would expose about 100 unauthenticated write endpoints to the LAN.

---

## 4. Native telemetry recording (DuckDB)

**Official guide:** "Telemetry Recording", Zendesk article 14524956311695, created 2025-12-04, updated 2026-09-23. Status: VERIFIED-SOURCE.

**Availability, enabling, configuration and viewing**
- **Since:** "In version 1.2 on PC".
- **Enable:**
  - *Gameplay* settings → automatic recording, or
  - a control binding (none by default).
  - The HUD shows "Telemetry recording started" above the tachometer.
- **Local setting:** `Settings.JSON` has `"Automatically Record Telemetry": false` (VERIFIED-LOCAL).
- **Configuration:** `UserData/Telemetry/config.json` lets you remove channels/events or change each `"Frequency"`. The guide warns this "may have some performance implications".
- **Viewing:** the guide suggests the DuckDB CLI (`duckdb -ui`). Converting to other formats is "not directly supported by Studio 397".

**Output location:** `UserData\Telemetry\*.duckdb`.
- No `.duckdb` files exist on this machine yet (VERIFIED-LOCAL).
- The file naming format changed in 1.2 ("Change Telemetry File Naming Format"). The format itself is undocumented.

**Engine:** `duckdb.dll` FileVersion **v1.4.0** ships with the game (VERIFIED-LOCAL). **INFERENCE:** the Rust `duckdb` crate must be a version that can read files written by DuckDB 1.4.0, so pin ≥ 1.4 and test.

**Schema** (CLAIMED, from lmu-telemetry-analyzer README and code):
- One table per channel, named exactly as the channel.
  - Scalar channels have a `value` column.
  - Per-wheel channels have `value1..value4` = FL, FR, RL, RR.
  - Row order (`rowid`) is the sample index at the channel's frequency.
- One table per event, with `ts` and `value`.
- Meta tables:
  - `metadata(key, value)`, which includes key `CarSetup` holding the full setup as JSON;
  - `channelsList(channelName, frequency, unit)`;
  - `eventsList(eventName, unit)`.
- Treat `channelsList.frequency` as authoritative per file.
- V1.2.1 fixed "broken timestamps on telemetry events"; V1.2.3.0 fixed a missing final 10 s.

**Channel rates in the local `config.json`** (VERIFIED-LOCAL; file mtime 2026-02-01; 56 channels, 42 events):

| Hz | Channels |
|---|---|
| 100 | Clutch RPM, Engine RPM, FFB Output, Front3rdDeflection, FrontRideHeight, GPS Time, Ground Speed, Rear3rdDeflection, RearRideHeight, Regen Rate, RideHeights, Steering Pos, Steering Pos Unfiltered, Steering Shaft Torque, Susp Pos, Turbo Boost Pressure, TyresTempCentre, TyresTempLeft, TyresTempRight, Wheel Speed |
| 50 | Brake Pos, Brake Pos Unfiltered, Brakes Air Temp, Brakes Force, Brakes Temp, Clutch Pos, Clutch Pos Unfiltered, Throttle Pos, Throttle Pos Unfiltered, TyresRimTemp |
| 20 | Fuel Level, SoC, Virtual Energy |
| 10 | Brake Thickness, G Force Lat/Long/Vert, GPS Latitude, GPS Longitude, GPS Speed, Lap Dist, Path Lateral, Total Dist, Track Edge, Tyres Wear, TyresPressure, TyresRubberTemp |
| 7 | Engine Oil Temp, Engine Water Temp |
| 5 | TyresCarcassTemp |
| 2 | OverheatingState, Time Behind Next |
| 1 | Ambient Temperature, Track Temperature, Wind Heading, Wind Speed |

**Events** (value-on-change, no frequency):
- ABS, ABSLevel, AntiStall Activated, Brake Bias Rear, Brake Migration
- Best LapTime, Best Sector1/2, Current LapTime, Current Sector, Current Sector1/2, Last Sector1/2, Lap, Lap Time
- CloudDarkness, Minimum Path Wetness, OffpathWetness
- Engine Max RPM, Finish Status, FuelMixtureMap, Gear, Headlights State, In Pits
- FrontFlapActivated, RearFlapActivated, RearFlapLegalStatus
- LastImpactMagnitude, LaunchControlActive
- Sector1/2/3 Flag, Speed Limiter, SurfaceTypes
- TC, TCCut, TCLevel, TCSlipAngle, TyresCompound, WheelsDetached, Yellow Flag State

**Differences between the official example and the local file** (VERIFIED):
- The official guide's example also lists Drag, FrontDownForce, ReadDownForce, FrontWingHeight, Lateral/Longitudinal Acceleration, Steered Angle and Yaw Rate, and spells "Brake Tickness".
- The local file lacks those, and has "Brakes Air Temp", "TyresRimTemp" and "Brake Thickness" instead.
- Shared channels have identical frequencies.
- **INFERENCE:** `config.json` is not regenerated on game update, so the channel set depends on when it was created. Always read `channelsList` from each file.

**Known consumers:** MyLMU (web), lmu-telemetry-analyzer, LMU-Telemetry-Lab, apex-lmu (read-only), alelosbrigia LMU-telemetry (→ MoTeC, non-commercial), Ion-Gavriliuc converter (→ CSV → MotecLogGenerator), and tobi/duckdb_motorsport_telemetry for LD I/O.

**File locking (INFERENCE):** the file is probably locked for writing while LMU records to it. Open finished files read-only, as the analyzers do with `-readonly` and apex's "opened read-only". Retry if the file is locked.

---

## 5. Known pitfalls

### 5.1 Class-specific channels

- **Which classes use Virtual Energy:** Hypercar (LMH/LMDh, hybrid) and LMGT3 (fuel-only). Some LMH cars are non-hybrid. (VERIFIED-SOURCE: official guide "What is Virtual Energy? (NRG)", updated 2026-09-28)
- **LMP2 and LMP3** have no VE, so expect `mVirtualEnergy`, `mBatteryChargeFraction` and `mSoC` to be 0 or meaningless.
  - LMU Pitwall probe: VE and battery fields "have no meaning in an LMP3 and read zero there" (CLAIMED).
  - TinyPedal: "estimated total stint laps cannot be shown on vehicle without virtual energy" for LMP2 (CLAIMED).
- **Hybrid fields** (`mElectricBoostMotor*`, `mRegen`, `mSoC`, `mBatteryChargeFraction`) only mean something on hybrid Hypercars. `mElectricBoostMotorState` 0 = "unavailable" (VERIFIED-LOCAL header).
- **Class detection:**
  - Prefer the `TelemInfoV01.mVehicleClass` enum (`IP_VehicleClass`).
  - Do not confuse it with `VehicleScoringInfoV01.mVehicleClass`, which is a `char[32]` string.
  - Same name, different types (VERIFIED-LOCAL).
  - Observed class strings include "Hyper", "LMP2" and "GT3" (race-engineer, CLAIMED).
- **Three "battery/energy" representations:**
  - `mBatteryChargeFraction` (header: fraction 0–1)
  - `mSoC` (no unit in the header; pyLMU says percent)
  - scoring `mFuelFraction` (0–255 = fuel *or battery*)
  - REST adds `veFraction`, `fuelInfo.currentVirtualEnergy`, and `VM_VIRTUAL_ENERGY` (a percentage) (KoDriver, CLAIMED).
  - Verify all of these live before use.

### 5.2 Temperatures and units

- **Tyre surface `mTemperature[3]`:** Kelvin, ordered **left / centre / right in car coordinates, not inner/outer**. The header warns explicitly (VERIFIED-LOCAL). It defines +x as "out the left side of the car (from the driver's perspective)".
  - **INFERENCE (to verify live, e.g. with negative camber so the inner edge runs hotter):**
    - Left-side wheels (FL, RL): `[0]` = outer, `[2]` = inner.
    - Right-side wheels (FR, RR): `[0]` = inner, `[2]` = outer.
  - TinyPedal labels its unchanged `[0],[1],[2]` output "inner,center,outer" for all four wheels (VERIFIED-SOURCE). Community labels are unreliable here.
  - CrewChief 4.19.3.2: "Tyre temperatures read from internal values, not the surface" (CLAIMED).
- **`mBrakeTemp`:** the header says "Celsius", **but**:
  - TinyPedal's LMU reader subtracts 273.15 (treats it as Kelvin) (VERIFIED-SOURCE);
  - KoDriver measured 280–290 at a cold start (Kelvin) (CLAIMED);
  - race-engineer says brakes are Kelvin (CLAIMED);
  - LMU Pitwall lists "whether `mBrakeTemp` is still Kelvin" as an open V1.4 question.
  - → Detect at runtime with a plausibility window (ambient ≈ 280–320 K vs 5–50 °C) and flag it.
- **Other temperatures:** `mTireCarcassTemperature` and `mTireInnerLayerTemperature[3]` are Kelvin. Engine water/oil are Celsius. `mOptimalTemp` has no unit in the header; pyLMU says Celsius (CLAIMED).
- **`mBrakePressure`:** 0.0–1.0 from pedal input and bias, **not kPa**. The header says this "will convert to true brake pressure (kPa) in future" (VERIFIED-LOCAL). Units could change silently in a future update, so version-gate this field.

### 5.3 Sectors, pit lane, flags

- **`VehicleScoringInfoV01.mSector`:** 0 = sector 3, 1 = sector 1, 2 = sector 2 (VERIFIED-LOCAL). TinyPedal remaps it the same way (VERIFIED-SOURCE).
- **`TelemInfoV01.mCurrentSector`:** zero-based, **pit lane in the sign bit**, e.g. `0x80000002` = entering the pits from the 3rd sector (VERIFIED-LOCAL). Mask with `& 0x7FFFFFFF` and test bit 31.
- **`mInPits`:** "not always accurate for remote vehicles" (VERIFIED-LOCAL). Cross-check with `mPitState` (0 none, 1 request, 2 entering, 3 stopped, 4 exiting).
- **`mSectorFlag[3]`:**
  - The header itself is unsure of the ordering (VERIFIED-LOCAL).
  - LMU Pitwall reports LMU uses **1 = yellow, 11 = clear** ("unlike rF2") (CLAIMED).
  - TinyPedal tests `== 1` for yellow (VERIFIED-SOURCE).
- **Other flag fields:** `mFlag` is "currently only 0=green or 6=blue" (VERIFIED-LOCAL).
- **Game phase:** `mGamePhase` 5 = green (race-engineer, CLAIMED); 6 = full-course yellow (LMU Pitwall, CLAIMED).
- **No safety-car flags:** `LMU_Data` has no equivalent of the plugin's safety-car flags (LMU Pitwall, CLAIMED).
- **Sentinels** (LMU Pitwall probe, race-engineer, CLAIMED):
  - `mMaxLaps = INT32_MAX` in timed races;
  - lap times `0` or `-1` until the first line crossing;
  - `mLapStartET = -1` before the first lap.

### 5.4 Vehicle indexing and consistency

- There are 104 slots. Use `telemetry.playerVehicleIdx` / `playerHasVehicle` for the player (VERIFIED-LOCAL header fields).
- Scoring has no equivalent index; find the row with `mIsPlayer` (LMU Pitwall, VERIFIED-SOURCE comment).
- Match telemetry to scoring by `mID`, not by array index, because slots are reused as drivers join and leave online.
- Scoring and telemetry counts can briefly disagree while cars join (LMU Pitwall, CLAIMED). Validate `mNumVehicles == activeVehicles` before joining the two, as pyLMU does.

### 5.5 Pointers and raw strings

- Ignore `ScoringInfoV01.mResultsStream` and `mVehicle` (pointers). Use `scoring.scoringStream` / `scoringStreamSize` instead (VERIFIED-LOCAL).
- **Encoding:**
  - Driver names can contain UTF-8 (race-engineer says the plugin emits UTF-8; CLAIMED). Decode leniently and do not assume Latin-1.
  - REST bodies are UTF-8 but carry no charset header (KoDriver, CLAIMED).

### 5.6 The "Enable Plugins" toggle and plugin interference

- `Settings.JSON` has `"Enable external plugins": true` with the comment "When no extra plugins are required you can switch it off to avoid performance overhead" (VERIFIED-LOCAL).
- **Open question:** LMU Pitwall says `LMU_Data` needs this ON (CLAIMED). TinyPedal/SimHub guides say nothing needs enabling. Test with LMU running.
- Third-party plugins in `Plugins\` run in `PluginsAdapter.exe` (INFERENCE §1.2).
  - The official Known Issues article advises disabling plugins via `CustomPluginVariables.JSON` if they cause crashes.
  - V1.2 fixed plugins triggering brake replacement in the MFD pit menu.
  - Our app should work with plugins disabled.

### 5.7 Anti-cheat (EAC) and online use

**Facts:**
- EAC is included "from 1.2 onwards on PC" (official EAC article, updated 2026-09-30).
- Online access is disabled without EAC (V1.2 notes); 1.4.2 also blocks online play on an invalid EAC ProductID.
- The official news says EAC "blocks access to write to memory locations and will crash the game if players start interfering".
- The Known Issues article tells users to have no programs "that attempts to alter the game using DLLs, hooks, or other methods".
- (All VERIFIED-SOURCE)

**Is reading `LMU_Data` allowed online?**
- No explicit official statement either way exists (no official doc found).
- Evidence for "yes":
  - Studio 397 created `LMU_Data` alongside EAC and ships an SDK plus a reader example.
  - The mapping is a named section that any process may open.
  - Community tools (TinyPedal, SimHub, LMU Pitwall's online "Run C", which "proves `mVirtualEnergy` arrives for the whole grid") read it during online races.
  - (CLAIMED / INFERENCE)
- **Treat it as permitted, read-only, and never inject, hook, or write game memory.**
- Open the mapping with `FILE_MAP_READ` where possible. The lock mapping needs write access for the `Interlocked` ops.

**Process access:**
- CrewChief 4.19.4.2: "Anticheat prevents reading path of LMU.exe" (CLAIMED). Do not rely on `QueryFullProcessImageName` or `OpenProcess(PROCESS_QUERY_*)`.
- apex-lmu opens the LMU PID with `SYNCHRONIZE` only, to watch for exit (VERIFIED-SOURCE; whether it works under EAC is unconfirmed).
- Get paths from `LMU_Data.paths.userData` instead.

**Elevation:** SimHub guides advise "Elevate both or neither" for the game and the reader (CLAIMED). Named-object visibility across integrity levels should be tested.

**Offline:**
- The guide says you can run without EAC by launching the exe directly or uninstalling EAC (official). The V1.2 news says to launch through Steam "at least for now".
- Offline sessions are fine for our development testing.

### 5.8 Official "Engineer Mode" (context, not a data source)

- LMU has an in-game Engineer Mode: team members can join and set pit strategy.
- It has been improved through V1.4.2, which "Added penalty options to engineer pit menu". (VERIFIED-SOURCE, release notes)
- The official V1.2 news describes it as letting drivers "configure their team mate's pit stops".
- This is the built-in competitor or complement to our remote race-engineering app. It exposes no public API.

---

## 6. Open questions to settle with LMU running

1. The real `LMU_Data` telemetry and scoring rates at `Plugin Mask` = 4, and what the other mask values do.
2. `HoldEvent`/`DataEvent` behaviour:
   - Are they signalled every telemetry frame or every scoring frame?
   - Manual- or auto-reset?
   - Is `LMU_Data_Event` still signalled?
   - Does waiting on them need the lock as well?
3. Whether `LMU_Data` exists with "Enable external plugins" OFF.
4. Whether `mBrakeTemp` is K or °C.
5. The tyre L/C/R ↔ inner/outer mapping per side.
6. Units and ranges of `mSoC`, `mVirtualEnergy` and the `mTimeGap*` fields.
7. Struct sizes: compile against the header once the Windows SDK is installed, and compare with pyLMU's claimed 324,820 bytes.
8. REST latency and whether GET polling at ≤1 Hz causes any MFD flicker on 1.4.2.
9. `OpenProcess(SYNCHRONIZE)` on `Le Mans Ultimate.exe` under EAC.
10. DuckDB file naming and whether files can be opened read-only while recording.

---

## 7. Sources (all retrieved 2026-09-30)

**Official (Studio 397 / Motorsport Games)**
- Release notes category (Zendesk API, 51 articles): https://guide.lemansultimate.com/hc/en-gb/categories/13278904445967-Release-Notes. API used: `https://guide.lemansultimate.com/api/v2/help_center/en-gb/categories/13278904445967/articles.json`
- V1.2 (Update 2): https://guide.lemansultimate.com/hc/en-gb/articles/14556121957775-V1-2-Update-2
- V1.4.2: https://guide.lemansultimate.com/hc/en-gb/articles/17713037697807-V1-4-2-Update-1-4-Patch-2
- Telemetry Recording: https://guide.lemansultimate.com/hc/en-gb/articles/14524956311695-Telemetry-Recording
- EAC uninstall/reinstall: https://guide.lemansultimate.com/hc/en-gb/articles/14524562770447
- Run without EAC: https://guide.lemansultimate.com/hc/en-gb/articles/14590657733903
- Known Issues and Advice: https://guide.lemansultimate.com/hc/en-gb/articles/13240843908623-Known-Issues-and-Advice
- What is Virtual Energy? (NRG): https://guide.lemansultimate.com/hc/en-gb/articles/13152376674191
- Broadcast Overlay setup: https://guide.lemansultimate.com/hc/en-gb/articles/13162981242895
- V1.2 news post (2025-12-04): https://lemansultimate.com/le-mans-ultimate-releases-v1-2-update-with-huge-feature-list-and-latest-elms-content/
- V1.3 coverage quoting shared-memory additions (2026-03-30): https://news.racecontrol.gg/?p=1511. The official V1.3 post https://lemansultimate.com/le-mans-ultimate-releases-v1-3-update-with-final-elms-content-performance-updates/ returned 403.

**Local files (VERIFIED-LOCAL)**
- `Support\SharedMemoryInterface\*.hpp`
- `UserData\player\Settings.JSON`
- `UserData\player\Multiplayer.JSON`
- `UserData\Telemetry\config.json`
- `Plugins\TelemetryToolPluginForLMU readme.txt`
- File versions and strings of `Le Mans Ultimate.exe`, `PluginsAdapter.exe` and `duckdb.dll`

**Community code and docs**
- TinyPedal: https://github.com/TinyPedal/TinyPedal
  - `tinypedal/adapter/lmu_restapi.py`, `_restapi.py`, `lmu_sharedmemory.py`, `lmu_reader.py`
  - `tinypedal/template/setting_api.py`
  - `docs/changelog.txt`
- pyLMUSharedMemory: https://github.com/TinyPedal/pyLMUSharedMemory (`lmu_mmap.py`, `lmu_data.py`, `tests/read_lmu_api.py`, commit `0e335d1719`)
- rF2SharedMemoryMapPlugin README: https://github.com/TheIronWolfModding/rF2SharedMemoryMapPlugin
- CrewChiefV4: https://gitlab.com/mr_belowski/CrewChiefV4
  - `CrewChiefV4/LMU/LMUData.cs`, `LMU_REST_API.cs`
  - Changelog: https://thecrewchief.org/downloads/change_log_for_auto_updated.html
- LMU Pitwall: https://github.com/Swizzjack/lmu-pitwall (`bridge/src/shared_memory/reader.rs`, `bridge/src/garage_api.rs`, `tools/lmu-probe/README.md`)
- lmu-shared-memory crate: https://docs.rs/lmu-shared-memory/latest/lmu_shared_memory/ and https://github.com/unblvvv/lmu-shared-memory
- apex-lmu: https://github.com/ralfboltshauser/apex-lmu (`bridge/README.md`, `bridge/lmu_memory_windows.go`)
- KoDriver: https://github.com/ai-kurou/KoDriver
  - `docs/lmu-windows-rest-api.md`, `docs/lmu-windows-telemetry.md`
  - PR: https://github.com/ai-kurou/KoDriver/pull/1491
- race-engineer: https://github.com/Alexander-Gro/race-engineer/blob/main/docs/03-LMU-INTEGRATION.md
- go-lmu-api: https://github.com/snipem/go-lmu-api
- lmu-telemetry-analyzer: https://github.com/mzluzifer/lmu-telemetry-analyzer (`README.en.md`, `lmu-bridge.js`)
- LMU-telemetry (MoTeC): https://github.com/alelosbrigia/LMU-telemetry
- LMU-DuckDB-Telem-to-MoTec: https://github.com/Ion-Gavriliuc-SimRacing/LMU-DuckDB-Telem-to-MoTec
- duckdb_motorsport_telemetry: https://github.com/tobi/duckdb_motorsport_telemetry
- LMU-Telemetry-Lab: https://github.com/rabbit20031225/LMU-Telemetry-Lab
- le-mans-ultimate-telemetry: https://github.com/NikMusy/le-mans-ultimate-telemetry
- isiMotor-Pulse-Plugin: https://github.com/marcgardent/isiMotor-Pulse-Plugin
- LMU forum thread "Shared memory data structure" (2026-04-02, no S397 staff posts): https://community.lemansultimate.com/index.php?threads/shared-memory-data-structure.16608/
- SimHub issue #2264: https://github.com/SHWotever/SimHub/issues/2264
- OESimRacing SimHub LMU guide (2026-08-04): https://oesimracing.com/simhub/games/le-mans-ultimate/
- MyLMU recording docs (2026-07-24): https://mylmu.app/docs/telemetry/getting-started/recording-in-lmu
- DR Sim Manager LMU page (outdated, plugin-based): https://docs.departedreality.com/dr-sim-manager/general/sources/le-mans-ultimate/
- LMU wiki Telemetry (outdated, 2025-10-27): https://lemansultimate.wiki.gg/wiki/Telemetry
