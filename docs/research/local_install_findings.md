# LMU local installation: telemetry-relevant findings

Inspection date: 2026-09-30. LMU was **not running**. All LMU files were opened read-only; nothing under the LMU install was modified. DuckDB files were copied to a temp directory before being opened with Python `duckdb` 1.5.6 in `read_only=True` mode (temp venv outside the repo).

Install root (abbreviated below as `<LMU>`):
`C:\Program Files (x86)\Steam\steamapps\common\Le Mans Ultimate`

Evidence labels:

- **VERIFIED**: observed directly (file read, query run, bytes inspected, strings extracted from a binary).
- **INFERRED**: a conclusion drawn from verified evidence that has not been observed end to end, for example a behaviour implied by strings in the binary.
- **CLAIMED**: stated by a README, a comment, or a third party, and not checked independently.
- **UNVERIFIED**: open question. It needs a live game session to settle.

License note: nothing from `Support\SharedMemoryInterface\*.hpp` is copied here. Only identifier names are recorded.

Privacy note: driver names, Steam user IDs and machine IDs appear in logs, `.ld` headers and DuckDB metadata. They are deliberately redacted in this document.

---

## 0. Summary (most important facts)

| Topic | Finding | Label |
|---|---|---|
| Game version | Exe FileVersion `1.4.2.0`. Steam buildid `25513257`. Trace log reports `LMU-Retail:1.4200`. `Core\Version.txt` = `1.4200` / `4200`. The TelemetryTool plugin logged `-STARTUP- (version 14.200)` | VERIFIED |
| Value of `SharedMemoryGeneric.gameVersion` | Probably `14200` (see section 1) | INFERRED / UNVERIFIED |
| Native telemetry recorder | Built into `Le Mans Ultimate.exe` (`TelemetryManager` / `SlotTelemetryRecorder` in `Source\Telemetry\Telemetry.cpp`). It writes **DuckDB** through the bundled `duckdb.dll` (FileVersion `v1.4.0`) | VERIFIED (strings + DB files) |
| Recorder config | `<LMU>\UserData\Telemetry\config.json` has 2 top-level keys: `Channels` (56 entries, each `{Frequency, Name}`) and `Events` (42 entries, each `{Name}`). There is no enable flag, output-dir key or format key in this file | VERIFIED |
| Enable flag | `Settings.JSON` > `"Game Options"` > `"Automatically Record Telemetry": false`. Recording can also be toggled with the "Start/Stop Telemetry Recording" key bind | VERIFIED (key + description text) |
| Local recordings | **None were made on this machine.** `UserData\Telemetry\` contains only `config.json`. None of 26 trace logs contains "Telemetry recording started" | VERIFIED |
| Output location | `UserData\Telemetry\` next to the filename pieces `{}{}_{}_{}`, `{:%Y-%m-%dT%H_%M_%SZ}` and `duckdb` in the exe | INFERRED |
| Real LMU DuckDB files | 4 files exist under `UserData\player\Settings\<Track>\`. They come from a third-party setup pack ("GO ..."). They are genuine LMU recorder output (schema matches the exe's `CREATE TABLE` strings) | VERIFIED |
| DuckDB schema | One table per channel and one per event, plus `channelsList`, `eventsList` and `metadata` (104 tables = 61 + 40 + 3). Channels have **no timestamp column**: row *i* is at `t0 + i/frequency`. Events are `(ts DOUBLE, value...)`. 4-wheel data is in columns `value1..value4` = FL, FR, RL, RR | VERIFIED |
| MoTeC `.ld/.ldx` in `<LMU>\LOG` | Produced by the third-party **DAMPlugin v2.00** (`Plugins\DAMPlugin.dll`), not by LMU | VERIFIED |
| REST API | `Settings.JSON` > `"Miscellaneous"` > `"WebUI bind": "127.0.0.1"`, `"WebUI port": 6397`. The trace log says `Bind rest server to 127.0.0.1:6397`. 185 route strings are in the exe and 181 client calls are in the bundled UI (tables in section 7) | VERIFIED |
| WebSocket | UI JS connects to `ws://localhost:<WebUI port + 1>/websocket/{ui,events,replaymetrics,controlpanel,overlays}`, which is **6398** by default | VERIFIED (code) / UNVERIFIED (live) |
| Swagger | Bundled Swagger UI points to `http://localhost:6397/swagger-schema.json`. The exe contains the `/swagger-schema.json` string | VERIFIED (strings) / UNVERIFIED (live) |
| `PLUGIN_NOT_NEEDED_ON_v1505_OR_LATER.txt` | "v1505" means the third-party **Telemetry Tool 15.05**, not an LMU version | VERIFIED (readme text) |

---

## 1. Game version facts

| Source | Value | Label |
|---|---|---|
| `<LMU>\Le Mans Ultimate.exe` VersionInfo | FileVersion `1.4.2.0`, ProductVersion `1.4.2.0`, ProductName `Le Mans Ultimate`, CompanyName `Studio 397`. Size 29,915,136 bytes, mtime 2026-09-25 01:40:55 | VERIFIED |
| `C:\Program Files (x86)\Steam\steamapps\appmanifest_2399420.acf` | `"appid" "2399420"`, `"buildid" "25513257"`, `"TargetBuildID" "25513257"` | VERIFIED |
| `<LMU>\steam_appid.txt` | `2399420` | VERIFIED |
| `<LMU>\Core\Version.txt` | Two lines: `1.4200`, `4200` | VERIFIED |
| `<LMU>\UserData\Log\trace*.txt` line 3 (`main.cpp 543`) | `LMU-Retail:1.4200 UTC=... SteamBuild=25513257 SteamUser=<redacted> ...`. Identical in all 26 logs that have it | VERIFIED |
| `<LMU>\Internals_TelemetryTool_LMU_Output.txt` (written by the third-party TelemetryToolPluginForLMU) | `-STARTUP- (version 14.200)`, `Shmem $TT_LMU_Telemetry$ succesfully created ...`, `Shmem $TT_LMU_Scoring$ succesfully created ...` | VERIFIED |
| `InternalsPlugin.hpp` comment on `Startup(long version)` | Describes the argument as "version * 1000" | VERIFIED (read) |
| `SharedMemoryInterface.hpp` | `SharedMemoryGeneric` has a field `long gameVersion`. No scaling comment | VERIFIED (read) |
| `duckdb.dll` | FileVersion `v1.4.0` | VERIFIED |
| `cohtml.WindowsDesktop.dll` | Coherent Cohtml `2.2.4.1` (the HTML UI engine) | VERIFIED |

**Interpretation, INFERRED:** TelemetryTool prints `14.200` for LMU 1.4200. That means the plugin `Startup()` argument was `14200` (version × 10000, printed ÷ 1000), not 1.4200 × 1000 = 1420. So `SharedMemoryGeneric.gameVersion` most likely reads **14200** on this build. This is **UNVERIFIED** until it is read from a live `LMU_Data` mapping.

---

## 2. Native telemetry recorder configuration (`UserData\Telemetry\config.json`)

Path: `<LMU>\UserData\Telemetry\config.json`. 9,286 bytes, mtime 2026-02-01 00:22:25, SHA-256 `7005630F28978AA7B4D2DBEC230D60DD1666CD5F414D2BCE2769D2CF0D9C3D52`. **VERIFIED**

- Top-level keys: exactly `Channels` and `Events`. There is **no** enable flag, output-directory key, format key or version key in this file. **VERIFIED**
- `Channels`: 56 objects keyed by name. Each has exactly `{ "Frequency": <int Hz>, "Name": <same as key> }`. **VERIFIED**
- `Events`: 42 objects keyed by name. Each has exactly `{ "Name": <same as key> }` and no frequency, because events are recorded on change. **VERIFIED**
- Frequency histogram (channels): 1 Hz ×4, 2 Hz ×2, 5 Hz ×1, 7 Hz ×2, 10 Hz ×14, 20 Hz ×3, 50 Hz ×10, 100 Hz ×20. **VERIFIED**
- The exe contains the literal keys `config.json`, `Channels`, `Frequency`, `Events`, `dev_options.json` and the error string `Error while reading telemetry configuration file %s: %s`. So the game parses this file. **VERIFIED** (strings)
- Whether editing `config.json` changes what or how fast LMU records is **UNVERIFIED**. The 4 real recordings found (section 4) contain 5 channels that are *not* in this config (see table E), and they record `TC` and `SurfaceTypes` as channels although this config lists them under `Events`. Those recordings were made on another machine, possibly with a different config or game version, so this is not proof either way. The local `config.json` mtime (2026-02-01) predates the 1.4.2 exe (2026-09-25). **INFERRED**: the game wrote the file once and has not rewritten it since.

The tables below list every config entry. The "In recorded DuckDB?" columns cross-reference the 4 real DuckDB recordings (section 4). All 4 recordings have identical `channelsList` and `eventsList`. **VERIFIED**: for every one of the 56 channels in both places, the config frequency equals the recorded `channelsList.frequency`.

### A. config.json `Channels` (56 entries)

| # | Channel (key = `Name`) | Frequency (Hz) | In recorded DuckDB? | DB unit (channelsList.unit) | DB column shape / type |
|---|---|---|---|---|---|
| 1 | Ambient Temperature | 1 | yes (freq 1) | `C` | value FLOAT |
| 2 | Brake Pos | 50 | yes (freq 50) | `%` | value FLOAT |
| 3 | Brake Pos Unfiltered | 50 | yes (freq 50) | `%` | value FLOAT |
| 4 | Brake Thickness | 10 | yes (freq 10) | `%` | value1..value4 FLOAT |
| 5 | Brakes Air Temp | 50 | yes (freq 50) | `C` | value1..value4 FLOAT |
| 6 | Brakes Force | 50 | yes (freq 50) | `%` | value1..value4 FLOAT |
| 7 | Brakes Temp | 50 | yes (freq 50) | `C` | value1..value4 FLOAT |
| 8 | Clutch Pos | 50 | yes (freq 50) | `%` | value FLOAT |
| 9 | Clutch Pos Unfiltered | 50 | yes (freq 50) | `%` | value FLOAT |
| 10 | Clutch RPM | 100 | yes (freq 100) | `RPM` | value FLOAT |
| 11 | Engine Oil Temp | 7 | yes (freq 7) | `C` | value FLOAT |
| 12 | Engine RPM | 100 | yes (freq 100) | `RPM` | value FLOAT |
| 13 | Engine Water Temp | 7 | yes (freq 7) | `C` | value FLOAT |
| 14 | FFB Output | 100 | yes (freq 100) | `%` | value FLOAT |
| 15 | Front3rdDeflection | 100 | yes (freq 100) | `m` | value FLOAT |
| 16 | FrontRideHeight | 100 | yes (freq 100) | `m` | value FLOAT |
| 17 | Fuel Level | 20 | yes (freq 20) | `L` | value FLOAT |
| 18 | G Force Lat | 10 | yes (freq 10) | `G` | value FLOAT |
| 19 | G Force Long | 10 | yes (freq 10) | `G` | value FLOAT |
| 20 | G Force Vert | 10 | yes (freq 10) | `G` | value FLOAT |
| 21 | GPS Latitude | 10 | yes (freq 10) | `deg` | value FLOAT |
| 22 | GPS Longitude | 10 | yes (freq 10) | `deg` | value FLOAT |
| 23 | GPS Speed | 10 | yes (freq 10) | `m/s` | value FLOAT |
| 24 | GPS Time | 100 | yes (freq 100) | `s` | value DOUBLE |
| 25 | Ground Speed | 100 | yes (freq 100) | `km/h` | value FLOAT |
| 26 | Lap Dist | 10 | yes (freq 10) | `m` | value FLOAT |
| 27 | OverheatingState | 2 | yes (freq 2) | (empty) | value BOOLEAN |
| 28 | Path Lateral | 10 | yes (freq 10) | `m` | value FLOAT |
| 29 | Rear3rdDeflection | 100 | yes (freq 100) | `m` | value FLOAT |
| 30 | RearRideHeight | 100 | yes (freq 100) | `m` | value FLOAT |
| 31 | Regen Rate | 100 | yes (freq 100) | `kW` | value FLOAT |
| 32 | RideHeights | 100 | yes (freq 100) | `m` | value1..value4 FLOAT |
| 33 | SoC | 20 | yes (freq 20) | `%` | value FLOAT |
| 34 | Steering Pos | 100 | yes (freq 100) | `%` | value FLOAT |
| 35 | Steering Pos Unfiltered | 100 | yes (freq 100) | `%` | value FLOAT |
| 36 | Steering Shaft Torque | 100 | yes (freq 100) | `Nm` | value FLOAT |
| 37 | Susp Pos | 100 | yes (freq 100) | `m` | value1..value4 FLOAT |
| 38 | Throttle Pos | 50 | yes (freq 50) | `%` | value FLOAT |
| 39 | Throttle Pos Unfiltered | 50 | yes (freq 50) | `%` | value FLOAT |
| 40 | Time Behind Next | 2 | yes (freq 2) | `s` | value FLOAT |
| 41 | Total Dist | 10 | yes (freq 10) | `m` | value FLOAT |
| 42 | Track Edge | 10 | yes (freq 10) | `m` | value FLOAT |
| 43 | Track Temperature | 1 | yes (freq 1) | `C` | value FLOAT |
| 44 | Turbo Boost Pressure | 100 | yes (freq 100) | `Pa` | value FLOAT |
| 45 | Tyres Wear | 10 | yes (freq 10) | `%` | value1..value4 FLOAT |
| 46 | TyresCarcassTemp | 5 | yes (freq 5) | `C` | value1..value4 FLOAT |
| 47 | TyresPressure | 10 | yes (freq 10) | `kPa` | value1..value4 FLOAT |
| 48 | TyresRimTemp | 50 | yes (freq 50) | `C` | value1..value4 FLOAT |
| 49 | TyresRubberTemp | 10 | yes (freq 10) | `C` | value1..value4 FLOAT |
| 50 | TyresTempCentre | 100 | yes (freq 100) | `C` | value1..value4 FLOAT |
| 51 | TyresTempLeft | 100 | yes (freq 100) | `C` | value1..value4 FLOAT |
| 52 | TyresTempRight | 100 | yes (freq 100) | `C` | value1..value4 FLOAT |
| 53 | Virtual Energy | 20 | yes (freq 20) | `%` | value FLOAT |
| 54 | Wheel Speed | 100 | yes (freq 100) | `m/s` | value1..value4 FLOAT |
| 55 | Wind Heading | 1 | yes (freq 1) | `deg` | value FLOAT |
| 56 | Wind Speed | 1 | yes (freq 1) | `m/s` | value FLOAT |

### B. config.json `Events` (42 entries, no Frequency field)

| # | Event (key = `Name`) | In recorded DuckDB? | DB unit (eventsList.unit) | DB column shape / type |
|---|---|---|---|---|
| 1 | ABS | yes, as event | (empty) | ts DOUBLE, value BOOLEAN |
| 2 | ABSLevel | yes, as event | (empty) | ts DOUBLE, value UTINYINT |
| 3 | AntiStall Activated | yes, as event | (empty) | ts DOUBLE, value BOOLEAN |
| 4 | Best LapTime | yes, as event | `s` | ts DOUBLE, value FLOAT |
| 5 | Best Sector1 | yes, as event | `s` | ts DOUBLE, value FLOAT |
| 6 | Best Sector2 | yes, as event | `s` | ts DOUBLE, value FLOAT |
| 7 | Brake Bias Rear | yes, as event | (empty) | ts DOUBLE, value FLOAT |
| 8 | Brake Migration | yes, as event | (empty) | ts DOUBLE, value FLOAT |
| 9 | CloudDarkness | yes, as event | `%` | ts DOUBLE, value BOOLEAN |
| 10 | Current LapTime | yes, as event | `s` | ts DOUBLE, value FLOAT |
| 11 | Current Sector | yes, as event | (empty) | ts DOUBLE, value UTINYINT |
| 12 | Current Sector1 | yes, as event | `s` | ts DOUBLE, value FLOAT |
| 13 | Current Sector2 | yes, as event | `s` | ts DOUBLE, value FLOAT |
| 14 | Engine Max RPM | yes, as event | `RPM` | ts DOUBLE, value FLOAT |
| 15 | Finish Status | yes, as event | (empty) | ts DOUBLE, value UTINYINT |
| 16 | FrontFlapActivated | yes, as event | (empty) | ts DOUBLE, value BOOLEAN |
| 17 | FuelMixtureMap | yes, as event | (empty) | ts DOUBLE, value UINTEGER |
| 18 | Gear | yes, as event | (empty) | ts DOUBLE, value TINYINT |
| 19 | Headlights State | yes, as event | `On/Off` | ts DOUBLE, value BOOLEAN |
| 20 | In Pits | yes, as event | (empty) | ts DOUBLE, value UTINYINT |
| 21 | Lap | yes, as event | (empty) | ts DOUBLE, value USMALLINT |
| 22 | Lap Time | yes, as event | `s` | ts DOUBLE, value FLOAT |
| 23 | Last Sector1 | yes, as event | `s` | ts DOUBLE, value FLOAT |
| 24 | Last Sector2 | yes, as event | `s` | ts DOUBLE, value FLOAT |
| 25 | LastImpactMagnitude | yes, as event | (empty) | ts DOUBLE, value BOOLEAN |
| 26 | LaunchControlActive | yes, as event | (empty) | ts DOUBLE, value BOOLEAN |
| 27 | Minimum Path Wetness | yes, as event | `%` | ts DOUBLE, value FLOAT |
| 28 | OffpathWetness | yes, as event | `%` | ts DOUBLE, value BOOLEAN |
| 29 | RearFlapActivated | yes, as event | (empty) | ts DOUBLE, value BOOLEAN |
| 30 | RearFlapLegalStatus | yes, as event | (empty) | ts DOUBLE, value BOOLEAN |
| 31 | Sector1 Flag | yes, as event | (empty) | ts DOUBLE, value UTINYINT |
| 32 | Sector2 Flag | yes, as event | (empty) | ts DOUBLE, value UTINYINT |
| 33 | Sector3 Flag | yes, as event | (empty) | ts DOUBLE, value UTINYINT |
| 34 | Speed Limiter | yes, as event | (empty) | ts DOUBLE, value BOOLEAN |
| 35 | SurfaceTypes | yes, but as a **channel** (5 Hz) | (empty) | value1..value4 UTINYINT |
| 36 | TC | yes, but as a **channel** (100 Hz) | (empty) | value BOOLEAN |
| 37 | TCCut | yes, as event | (empty) | ts DOUBLE, value UTINYINT |
| 38 | TCLevel | yes, as event | (empty) | ts DOUBLE, value UTINYINT |
| 39 | TCSlipAngle | yes, as event | (empty) | ts DOUBLE, value UTINYINT |
| 40 | TyresCompound | yes, as event | (empty) | ts DOUBLE, value1..value4 UINTEGER |
| 41 | WheelsDetached | yes, as event | (empty) | ts DOUBLE, value1..value4 UTINYINT |
| 42 | Yellow Flag State | yes, as event | (empty) | ts DOUBLE, value UTINYINT |

### E. Channels present in recordings but not in the local config.json

| channelName | frequency (Hz) | unit | columns |
|---|---|---|---|
| Brakes Core Temp | 50 | `C` | value1..value4 FLOAT |
| Steering Pos Filtered | 100 | `%` | value FLOAT |
| Steering Shaft Torque Unfiltered | 10 | `Nm` | value FLOAT |
| SurfaceTypes | 5 | (empty) | value1..value4 UTINYINT (listed under `Events` in config.json) |
| TC | 100 | (empty) | value BOOLEAN (listed under `Events` in config.json) |

### F. More channel names found only in the exe string table

These strings sit in the same exe region as the other channel names and units: `Air Density` (`kg/m3`), `Ambient Pressure`, `Humidity`, `Rain`, `Air Specific Heat` (`J/(kg*K)`). They are in neither the config nor the recordings. Whether they can be enabled by adding them to `config.json` is **UNVERIFIED**.

---

## 3. Recording enable flags and related keys in `Settings.JSON` / `Multiplayer.JSON`

File: `<LMU>\UserData\player\Settings.JSON` (51,120 bytes, mtime 2026-09-30 11:15:08). Case-insensitive search for telemetry, record, duckdb, shared, memory, rest, http, port, 6397, plugin, web and motec. Exact keys and values. **VERIFIED**

| Section | Key | Value | Description string (`<key>#`) |
|---|---|---|---|
| `Game Options` | `Automatically Record Telemetry` | `false` | "Enables automatic telemetry recording every time you drive. Recordings can be manually run when this is off or stopped via the Telemetry Recording key bind." |
| `Game Options` | `Enable external plugins` | `true` | "When no extra plugins are required you can switch it off to avoid performance overhead" |
| `Game Options` | `Plugin Mask` | `4` | "Plugin mask to control plugin refresh rate. 100Hz default for performance reasons. Faster rates might be helpful for external hardware". The meaning of value `4` is **UNVERIFIED** |
| `Game Options` | `RealTimeLogging` | `0` | "0=off, 1=Accurate" |
| `Game Options` | `Record Replays` | `true` | "whether to record replays or not" |
| `Game Options` | `Record Hotlap` | `false` | "whether to record hotlap or not (must have replay recording on)" |
| `Game Options` | `Custom Replay Folder` | `<LMU>\UserData\Replays\` | |
| `Miscellaneous` | `WebUI bind` | `"127.0.0.1"` | "IP address to bind the WebUI to.  Use 'localhost' for local access only, or '0.0.0.0' for open access for broadcasting" |
| `Miscellaneous` | `WebUI port` | `6397` | "Port for the WebUI" |
| `Miscellaneous` | `UI Dev Loose File Path` | `""` | "Folder to use for loose files during development" |

No key contains "duckdb", "shared" or "memory". **VERIFIED**

`<LMU>\UserData\player\Multiplayer.JSON` > `"Multiplayer General Options"`. These are **multiplayer** ports, not the WebUI/REST port. **VERIFIED**
- `"HTTP Server Port": 64297` ("Used for file sharing and for clients to get the initial server config")
- `"Simulation Port": 54297`
- `"Bind IP": "0.0.0.0"`
- `"Multiplayer Server Options"` > `"HTTP Server Document Root": "Packages"`

Key binding: the exe contains `Start/Stop Telemetry Recording`, `Start/Stop telemetry recording`, `Telemetry recording started`, `Telemetry recording stopped` and `TelemetryOnIndicator`. **VERIFIED** (strings). No telemetry binding was found in `UserData\player\current controls.json`, `keyboard.json` or `gamepad.json`, so the bind is probably unassigned. **INFERRED**

The exe also contains `RecordAllCarsTelemetry` next to the recorder strings and the class name `SlotTelemetryRecorder`. This suggests a (dev?) option to record every car slot, possibly set through `dev_options.json`. **INFERRED / UNVERIFIED**

---

## 4. Where native recordings live and what format they use

### 4.1 Search results

Recursive search of `<LMU>` for `*.duckdb, *.db, *.parquet, *.wal, *.sqlite, *.csv, *.ld, *.ldx`. **VERIFIED**

- No `*.db`, `*.parquet`, `*.wal`, `*.sqlite` or `*.csv` files.
- `*.duckdb`: exactly 4 files, all in setup folders:
  - `UserData\player\Settings\Spa\GO V1.4.2 M4EVO LMGT3 SPA Q Esport DuckDB.duckdb` (19,509,248 B)
  - `UserData\player\Settings\Lemans\GO V1.4 M4EVO LMGT3 LMS Q Esport V2 DuckDB.duckdb` (11,825,152 B)
  - `UserData\player\Settings\Portimao\GO V1.4.1 M4EVO LMGT3 PTM Q Esport DuckDB.duckdb` (8,744,960 B)
  - `UserData\player\Settings\Sebring\GO V1.4 M4EVO LMGT3 SBR Endu Esport DuckDB.duckdb` (7,974,912 B)
- Each sits beside `.svm` setups, a `... LMU Replay.Vcr` and a `... MOTEC.ld` with the same name prefix. They belong to a **third-party setup package** ("GO" setups): the setup vendor ships a reference lap as native DuckDB, MoTeC and replay. They are **not** setup files with an odd extension. **VERIFIED** (contents). The provenance is **INFERRED** from names and co-located files.
- `*.ld` / `*.ldx`: 67 + 67 files in `<LMU>\LOG\` (2026-09-27 to 2026-09-30) plus the 4 `... MOTEC.ld` files above (section 6).

### 4.2 Are these real LMU recordings? Yes. VERIFIED

- Magic bytes: offset 8 = `DUCK`, u64 at offset 12 = `64` (DuckDB storage version number), library string at offset 52 = `v1.4.0`. This matches the bundled `duckdb.dll` v1.4.0.
- DuckDB `pragma_database_size()` block_size = **16384**. The exe contains the strings `default_block_size` and `16384` next to the recorder code.
- The exe contains these literal SQL statements, and the files match them exactly:
  - `CREATE TABLE metadata(key VARCHAR PRIMARY KEY,value VARCHAR)` / `INSERT INTO metadata VALUES('{}', '{}');`
  - `CREATE TABLE channelsList(channelName VARCHAR PRIMARY KEY, frequency INT, unit VARCHAR)` / `INSERT INTO channelsList VALUES('{}', '{}', '{}');`
  - `CREATE TABLE eventsList(eventName VARCHAR PRIMARY KEY, unit VARCHAR)` / `INSERT INTO eventsList VALUES('{}', '{}');`
  - `CREATE TABLE '{}'({});` (channel tables, no ts) and `CREATE TABLE '{}'(ts DOUBLE,{});` (event tables)
  - Column-spec strings: `value FLOAT`, `value DOUBLE`, `value UTINYINT`, `value UINTEGER`, `value BOOLEAN`, `value TINYINT`, `value USMALLINT`, `value1 FLOAT, value2 FLOAT, value3 FLOAT, value4 FLOAT`, `value1 UTINYINT, ...`, `value1 UINTEGER, ...`
  - Other SQL: `BEGIN TRANSACTION;`, `COMMIT;`, `CALL checkpoint();`. Imported DuckDB C API: `duckdb_open_ext`, `duckdb_set_config`, `duckdb_appender_create`, `duckdb_append_{bool,double,float,int8,uint8,uint16,uint32}`, `duckdb_appender_flush`, and others. Data is written through the **Appender API**.

### 4.3 Where LMU writes its own recordings

- No local recording exists. `<LMU>\UserData\Telemetry\` contains only `config.json`. No trace log (`UserData\Log\trace*.txt`, 26 files) contains "Telemetry recording started" or "telemetry file". Every session logs only `Telemetry.cp 557: Telemetry initialized...` and `Telemetry.cp 576: Shutting down telemetry manager.` **VERIFIED**
- Expected output directory: **`<LMU>\UserData\Telemetry\`**. The string `UserData\Telemetry\` sits in the same exe string block as the recorder. The filename format pieces there are `{}{}_{}_{}`, `{:%Y-%m-%dT%H_%M_%SZ}` (this matches the `RecordingTime` metadata value format, e.g. `2026-09-25T12_52_05Z`) and `duckdb`. **INFERRED**: file = `UserData\Telemetry\<a>_<b>_<timestamp>.duckdb`. The exact components are **UNVERIFIED**; one manual recording would settle this.
- To create one: set `"Automatically Record Telemetry": true`, or bind and press "Start/Stop Telemetry Recording". **CLAIMED** by the setting's own description text.

---

## 5. LMU DuckDB telemetry schema (from the 4 real files)

All numbers are from the Spa file unless stated otherwise. The schema, table set, `channelsList` and `eventsList` are **identical in all 4 files**. **VERIFIED**

### 5.1 Tables

- 104 tables in schema `main`: 61 channel tables + 40 event tables + `channelsList` + `eventsList` + `metadata`. There are no views, indexes or sequences. **VERIFIED**
- Table names are the channel or event display names, **including spaces** (e.g. `"Brake Pos"`, `"GPS Time"`). They must be double-quoted in SQL. **VERIFIED**

| Table | Columns | Rows (Spa) |
|---|---|---|
| `metadata` | `key VARCHAR` (PK), `value VARCHAR` | 12 |
| `channelsList` | `channelName VARCHAR` (PK), `frequency INTEGER`, `unit VARCHAR` | 61 |
| `eventsList` | `eventName VARCHAR` (PK), `unit VARCHAR` | 40 |
| channel table, e.g. `"Engine RPM"` | `value <TYPE>` or `value1..value4 <TYPE>`, **no timestamp** | frequency × duration |
| event table, e.g. `"Gear"` | `ts DOUBLE`, then `value <TYPE>` or `value1..value4 <TYPE>` | one row per change |

### 5.2 `metadata` keys (12). VERIFIED

| key | Example value | Notes |
|---|---|---|
| `Version` | `1` | schema version |
| `DriverName` | (redacted) | |
| `SteamID` | `0` | 0 in all 4 files |
| `RecordingTime` | `2026-09-25T12_52_05Z` | UTC wall clock, format `{:%Y-%m-%dT%H_%M_%SZ}` |
| `SessionTime` | `12:00:00` (Le Mans file: `12:37:06`) | format `{:%H:%M:%S}`. Probably in-game time of day (**INFERRED**) |
| `SessionType` | `Practice` | |
| `TrackName` | `Circuit de Spa-Francorchamps` | |
| `TrackLayout` | `Circuit de Spa-Francorchamps` | |
| `WeatherConditions` | `Clear` | |
| `CarName` | `BMW GT3 Custom Team 2026 #397` | |
| `CarClass` | `GT3` | |
| `CarSetup` | JSON object, ~38 KB, 172 keys (`VM_ANTILOCKBRAKESYSTEMMAP`, `VM_BRAKE_BALANCE`, `VM_BRAKE_DUCTS`, ...) | Each entry has `available, diffComparisonValue, isFreeSetting, key, lastSavedStringValue, maxValue, minValue, numChangesValue, stringValue, value` and sometimes `caption`. The same `VM_*` keys appear in the REST route `/rest/garage/(VM_.*)` |

The 4 files: Spa (Practice, 716.98 s), Circuit de la Sarthe (503.05 s), Algarve International Circuit (334.78 s), Sebring International Raceway (280.85 s). All are BMW GT3, recorded 2026-07-31 to 2026-09-25.

### 5.3 Frequency representation. VERIFIED

- Per channel in `channelsList.frequency` (INTEGER Hz). Events have no frequency.
- Row count = frequency × duration within ±2 rows for every channel except the two **7 Hz** channels (Engine Oil Temp, Engine Water Temp). Those have 5032 rows over 716.98 s at Spa and 1971 over 280.85 s at Sebring, an effective rate of **7.018 Hz** in both.
- **INFERRED**: 7.018 Hz ≈ 400/57. The recorder probably decimates a 400 Hz loop by `round(400/f)` ticks. Every other configured rate (1, 2, 5, 10, 20, 50, 100) divides 400 exactly. Consumers should compute each channel's effective rate as `rows / duration` rather than trust the nominal value.

### 5.4 Timestamp representation. VERIFIED

- **Channel tables have no time column.** Sample *i*, in insertion order (use `ORDER BY rowid`), is at `t0 + i / frequency`.
- `t0` = first value of the `"GPS Time"` channel (100 Hz, DOUBLE, unit `s`) = the earliest event `ts` = the `ts` of the first `"Lap"` event. For Spa, `t0 = 1922.825`.
- `"GPS Time"` is an explicit 100 Hz clock. Consecutive differences are exactly 0.01 s (min 0.00999999999976, max 0.0100000000002).
- The time base is **session elapsed seconds** (e.g. Spa 1922.825 → 2639.795; Sebring 865.295 → 1146.135), not wall clock.
- Event `ts` uses the same base. Rows are monotonically non-decreasing (checked for `ABS` in all 4 files). Each event table has an initial row at `t0` carrying the starting state, then one row per change (e.g. `Gear` 448 rows, `ABS` 3817 rows).
- Alignment check: for each `Lap` event, the 10 Hz `Lap Dist` sample at index `round((ts − t0)·10)` is where Lap Dist wraps from ~6978 m to ~0 m. This held within ±1 sample for all 5 lap changes at Spa and all 3 at Sebring.

### 5.5 Wheel channel naming. VERIFIED

Four-corner channels use columns `value1, value2, value3, value4` = **FL, FR, RL, RR**. Evidence:

- **Front/rear split:**
  - `Brakes Force` maxima are 0.495 for value1/value2 and 0.505 for value3/value4. This matches the `Brake Bias Rear` event (0.505) and the setup string `49.5:50.5`.
  - `RideHeights` value1/value2 correlate with `FrontRideHeight` (0.88 to 0.95), and value3/value4 correlate with `RearRideHeight` (0.92 to 0.95). Checked at Spa and Le Mans.
  - `Brake Thickness` max is 3.6 for value1/value2 and 3.2 for value3/value4.
- **Left/right split:**
  - Wheel-speed differences (value1 − value2) and (value3 − value4) correlate at +0.47 with the GPS-derived clockwise heading rate at both Spa and Le Mans. In a right turn the left (outside) wheels spin faster, so value1 and value3 are the left wheels. This assumes the synthetic GPS axes point north/east.
  - An independent check uses tyre edge temperatures. For value1/value3, `TyresTempRight` is hotter than `TyresTempLeft` (63.5 vs 55.9 °C avg), and for value2/value4 the opposite holds. With negative camber the inboard edge runs hotter, and the inboard edge of a left wheel is its right edge.
- Wheel channels: `Brake Thickness, Brakes Air Temp, Brakes Core Temp, Brakes Force, Brakes Temp, RideHeights, SurfaceTypes, Susp Pos, Tyres Wear, TyresCarcassTemp, TyresPressure, TyresRimTemp, TyresRubberTemp, TyresTempCentre, TyresTempLeft, TyresTempRight, Wheel Speed`. Event tables with four values: `TyresCompound, WheelsDetached`.

### 5.6 Value ranges and unit sanity (Spa)

Ranges are **VERIFIED**. The interpretation notes are **INFERRED**.

- Pedals `Throttle/Brake/Clutch Pos`: 0 to 100 (%). `Steering Pos`: −65 to +68 (%). Positive correlates with right turns (+0.43). `FFB Output`: −100 to +100.
- `Ground Speed`: km/h, max 265.3. `GPS Speed` and `Wheel Speed`: m/s, max ~73.7 (= 265 km/h, consistent). Wheel Speed is positive when moving forward.
- `TyresPressure`: kPa, 127 to 176. `Turbo Boost Pressure`: Pa, ~101,300 (≈ ambient). `Fuel Level`: L, 0.59 to 21. `Virtual Energy`: %, 75 to 100. `SoC` and `Regen Rate`: 0 (GT3).
- **Unit label mismatches:**
  - `Brakes Force` is labelled `%` but holds fractions ≤ 0.505.
  - `Brake Thickness` is labelled `%` but values are 3.6/3.2 (cm?).
  - `Wind Heading` is labelled `deg` but the constant is 5.498 (radians?).
  - The correct units for these are **UNVERIFIED**.
- **`GPS Latitude/Longitude` are synthetic.** All 4 tracks sit around **60.0°N, 0.0°E** (Spa 59.990 to 60.009 / −0.010 to 0.013), not at their real locations. **INFERRED**: this is a track-local projection.
- `G Force Lat` had near-zero correlation with the heading rate, so its sign convention is **UNVERIFIED**.
- Sector flag events had value 11. `Clutch RPM` ≈ `Engine RPM`.

---

## 6. MoTeC `.ld` / `.ldx` files in `<LMU>\LOG\`: produced by DAMPlugin (third party)

**VERIFIED:**

- `<LMU>\LOG\` holds 67 `.ld` + 67 `.ldx` files named like `2026-09-30 - 02-19-02 - Fuji Speedway - P1.ld`.
- `<LMU>\UserData\player\DAMPlugin.ini` header reads `DAMPlugin v2.00 Parameters file` and contains:
  - `Active on startup = 1`, `Activation key = M`, `Key modifier = 2` (Ctrl)
  - `Log dir = "<DATA_PATH>\LOG\"`, `Log Filename Format = "<date> - <time> - <track> - <session>"`
  - `Capture partial log = 1` (vehicle must have moved), `Data rate = 3`
  - Base channel groups Controls, Scoring, Suspension, Tyres, Vehicle, Weather and Wheels all `= 1`. All extra channel groups are `= 0`.
- `<LMU>\DAMPlugin_Output.txt` repeats `Startup config / Log dir: .\\LOG\ / Logging data rate: Normal (3)` (236 times) and `Log build finished` (379 times).
- The filename format and directory match exactly. `.ld` headers carry device string `ADL`, date, time, driver (redacted) and track name. `.ldx` is XML `<LDXFile Version="1.6">` with `Total Laps`, `Fastest Time`, `Fastest Lap`.
- Channel naming uses MoTeC/DAMPlugin style (`Ground Speed`, `Throttle Pos`, `Wheel Rot Speed FL`, `Tyre Wear FL`, ...). This differs from LMU's DuckDB naming. The third-party `... MOTEC.ld` files in the setup folders use the same DAMPlugin naming, at 100 Hz on all channels (data rate 5).
- `DAMPlugin.dll` lives in `<LMU>\Plugins\` and is enabled in `UserData\player\CustomPluginVariables.JSON`. It is a **third-party plugin and must not be a dependency** of our app.

---

## 7. LMU REST API (internal HTTP server used by the HTML UI)

### 7.1 Server, port and transport. VERIFIED unless noted

- Configured in `Settings.JSON` > `Miscellaneous` > `WebUI bind` = `127.0.0.1`, `WebUI port` = `6397`. The exe's own help text for `WebUI bind` says ipv6 addresses are not supported.
- Every game start logs `RestService. 889: Creating rest server`, `914: Bind rest server to 127.0.0.1:6397`, `917: rest server started` and, on exit, `739: Stopping rest server` (18 starts across logs).
- Failed UI requests logged by `CoherentReso 383` include:
  - `GET http://127.0.0.1:6397/rest/replay/isactive` → 400
  - `/rest/watch/getIncidentsList/1` → 404
  - `/webdata/overlays` → 400
  - `/start/locales/en/credits.json` → 404
  - Other failed paths: `/rest/watch/focus`, `/rest/garage/setup/notes/<setup>`, `/rest/garage/PitMenu/receivePitMenu`, `/rest/garage/summary`, `/rest/sessions/GetGameState`, `/rest/garage/UIScreen/RepairAndRefuel`, `/rest/race/car/<id>/image?type=IMAGE_MEDIUM`, `/start/bgImageUrl`
  - These are only the requests that *failed*. Successful calls are not logged.
- UI assets: `<LMU>\Bin\UI.zip` (1.2 GB, 4,456 entries, not encrypted), with top-level folders `start/` (React/Vite game UI), `overlays/` (AngularJS broadcast overlays), `swagger/`, `materialeditor/` and `images/`. The exe static-route regex `(/start/).*|(/swagger/).*|(/materialeditor/).*|(/overlays/).*` shows these are served over HTTP from port 6397.
- **Swagger:** `UI.zip:swagger/swagger-initializer.js` sets `url: "http://localhost:6397/swagger-schema.json"`, and the exe contains `/swagger-schema.json`. **INFERRED**: with the game running, `http://localhost:6397/swagger/index.html` and `/swagger-schema.json` expose the authoritative OpenAPI description. **UNVERIFIED** live, and this is the first thing to check once LMU runs.
- **WebSocket.** `UI.zip:start/assets/app-CjYE-9lu.js` defines ``const wsPort = `${Number(location.port) + 1}`; const baseUrl = `ws://localhost:${wsPort}/`;``. The overlays use `'ws://' + location.hostname + ':' + (Number(location.port) + 1) + '/websocket/...'`. So the default is **ws://localhost:6398** (**UNVERIFIED** live). Endpoints:
  - `/websocket/ui`: the client sends `{"messageType":"SUB","topic":"<topic>"}`. Topics handled: `SessionInfo`, `LiveStandings`, `InputData`. Messages carry `{topic, body}`.
  - `/websocket/events`: messages `{type, body}` with type ∈ `navStateChange`, `driverSwapNotifications`, `pitSettingChange`, `liveryEditorNotifications`, `gameState`.
  - `/websocket/replaymetrics`, `/websocket/controlpanel`, `/websocket/overlays` (the last two come from `overlays/src/*.js`). The exe contains `/websocket/controlpanel`, `/websocket/replaymetrics`, `LiveStandings`, `SessionInfo`, `navStateChange`, `pitSettingChange` and `driverSwapNotifications` (strings).
- Client wrappers in the UI bundle (ky): `getJson`, `postJson`, `postJsonAndGetJson`, `getText`, `postText`, `getHtml`, `putJson`, `putText`, `del`, `deleteAndGetJson`. Base constants resolved from the bundle:
  - `restServiceUrl=/rest/`, `navigationRestUrl=/navigation/`, `webdataServiceUrl=/webdata/`
  - `garageRestUrl=/rest/garage/`, `pitStrategyUrl=/rest/garage/PitStrategy/`
  - `optionsRestUrl=/rest/options/`, `sessionsRestUrl=/rest/sessions/`, `watchRestUrl=/rest/watch/`, `strategyRestUrl=/rest/strategy`, `raceRestUrl=/rest/race/`, `replayRestUrl=/rest/replay/`
  - `profileRestUrl=/rest/profile/`, `multiplayerRestUrl=/rest/multiplayer/`, `chatRestUrl=/rest/chat/`, `hudRestUrl=/rest/hud`, `materialRestUrl=/rest/materialeditor/`, `startServices=/rest/start/`
- The `HttpServer.WindowsDesktop.dll` (Coherent Labs, pdb `...\cohtml\Build\bin_WinDesktop_x64\Release\HttpServer.WindowsDesktop.pdb`) embeds the Chrome DevTools frontend (`localhost:9229`). It contains **no** LMU `/rest/` routes. The routes live in `Le Mans Ultimate.exe`.
- Not LMU-local: the UI also calls the online back end with `fetch("/api/v1/player")`, `"/api/v1/statistics"` and `api/v1/notification` relative to a remote host. The trace also shows `https://raceos.gg/api/v1/team/...`. These are irrelevant to us.

### 7.2 Most relevant endpoints for a race-engineering tool

This list is chosen by name. **Response schemas are UNVERIFIED** because the game was not running.

- **Live timing and standings:** `GET /rest/watch/standings`, `GET /rest/watch/standings/history`, `GET /rest/watch/sessionInfo`, `GET /rest/watch/trackmap`, `GET /rest/race/track/{id}/trackmap`, and ws `/websocket/ui` topics `LiveStandings`, `SessionInfo`, `InputData`.
- **Game state:** `GET /rest/sessions/GetGameState`, `GET /navigation/state`, and ws `/websocket/events` (`gameState`, `navStateChange`).
- **Car and strategy:** `GET /rest/garage/getVehicleCondition`, `GET /rest/garage/tireinfo`, `GET /rest/garage/brakeinfo`, `GET /rest/garage/summary`, `GET /rest/garage/PitMenu/receivePitMenu`, `GET /rest/strategy/usage`, `GET /rest/strategy/overall`, `/rest/strategy/pitstop-estimate` (exe only), `GET /rest/sessions/TireInventory`, `GET /rest/sessions/weather`, `GET /rest/garage/UIScreen/RepairAndRefuel`, `GET /rest/garage/UIScreen/TireManagement`.
  - Pit menu entries carry a field `"PMC Value"`. The UI enum `PitModCategory` names `PMC_DRIVER`=5, value 6 = virtual energy, `PMC_FL_PRESSURE`=25 ... `PMC_RR_PRESSURE`=28, and so on up to `PMC_MAXIMUM`=37. **VERIFIED** in JS.
- **Write endpoints exist.** Examples: `POST /rest/garage/PitMenu/loadPitMenu` and `POST /rest/garage/PitStrategy/ApplyEntirePitStrategy`. A remote engineer tool must treat them as out of scope unless deliberately designed and user-approved.

### 7.3 All REST route strings found in `Le Mans Ultimate.exe`, cross-referenced with the bundled UI

Extraction method: an ASCII string scan of the exe for `/rest/`, `/navigation` and `/webdata`. That gives 185 distinct strings. `(...)`, `.*`, `/?` and `{param}` are the server's own route patterns. The "Methods" column shows which HTTP verbs the bundled UI JS uses, from `start/assets/app-CjYE-9lu.js` (app), `overlays/src/services/BroadcastService.js`, `overlays/src/overlays.js`, `overlays/src/controlpanel.js` (overlays) and `materialeditor/services/MaterialEditorService.js`. **VERIFIED** (static). The exe list is **not exhaustive**: some routes are assembled at runtime. The UI calls these endpoints, which have no literal exe string:
- `POST /rest/garage/PitStrategy/ApplyEntirePitStrategy` (the exe only has `PitStrategy`)
- `GET/POST /rest/race/series`
- `POST /rest/sessions/Championship/championshipAction/{CHAMP_CONTINUE|CHAMP_ENTERRACE|CHAMP_SAVE|CHAMP_SETTINGS|CHAMP_STOP|CHAMP_STARTNEW}`
- `GET /rest/sessions/Championship/championshipState`
- `GET /rest/sessions/TireInventory`
- `POST /rest/sessions/ai/SkipToMyNextStint`
- `GET /rest/sessions/opponents/filter`
- `POST /rest/sessions/getRaceResults` (overlays)
- `GET /rest/watch/activeCamera` (overlays)

`POST /rest/options/{valueType}` resolves to the exe routes `/rest/options/float` and `/rest/options/long`.

| # | Route string in `Le Mans Ultimate.exe` | Methods the bundled UI uses (source) |
|---|---|---|
| 1 | `/rest/cancelSteamAuth` | -- (not referenced by bundled UI JS) |
| 2 | `/rest/chat/` | GET (app), POST (app) |
| 3 | `/rest/garage/` | PUT (app) |
| 4 | `/rest/garage/(VM_.*)` | POST (app) |
| 5 | `/rest/garage/(WM_.*)-(.*)` | -- (not referenced by bundled UI JS) |
| 6 | `/rest/garage/PitMenu/loadPitMenu` | POST (app) |
| 7 | `/rest/garage/PitMenu/receivePitMenu` | GET (app) |
| 8 | `/rest/garage/SetCurrentVehicle` | POST (app) |
| 9 | `/rest/garage/SetPreviewSaveFile` | POST (app) |
| 10 | `/rest/garage/UIScreen/CarSetupOverview` | GET (app) |
| 11 | `/rest/garage/UIScreen/CoopOverview` | GET (app) |
| 12 | `/rest/garage/UIScreen/DriverHandOffStintEnd` | -- (not referenced by bundled UI JS) |
| 13 | `/rest/garage/UIScreen/DriverHandOffStintStart` | GET (app) |
| 14 | `/rest/garage/UIScreen/PitCarReview` | GET (app) |
| 15 | `/rest/garage/UIScreen/RepairAndRefuel` | GET (app) |
| 16 | `/rest/garage/UIScreen/SessionSetup` | GET (app) |
| 17 | `/rest/garage/UIScreen/TireManagement` | GET (app) |
| 18 | `/rest/garage/brakeinfo` | GET (app) |
| 19 | `/rest/garage/drive` | POST (app) |
| 20 | `/rest/garage/getPlayerGarageData` | GET (app) |
| 21 | `/rest/garage/getVehicleCondition` | GET (app) |
| 22 | `/rest/garage/isRefreshInProgress` | GET (app) |
| 23 | `/rest/garage/refreshSetups` | POST (app) |
| 24 | `/rest/garage/setup` | GET (app), POST (app), PUT (app) |
| 25 | `/rest/garage/setup/(.*)` | DELETE (app) |
| 26 | `/rest/garage/setup/compare` | POST (app) |
| 27 | `/rest/garage/setup/default` | POST (app) |
| 28 | `/rest/garage/setup/notes` | POST (app) |
| 29 | `/rest/garage/setup/notes/(.*)` | GET (app) |
| 30 | `/rest/garage/showOnlyRelevantSetups` | GET (app), POST (app) |
| 31 | `/rest/garage/summary` | GET (app) |
| 32 | `/rest/garage/tireinfo` | GET (app) |
| 33 | `/rest/garage/toRaceMenu` | -- (not referenced by bundled UI JS) |
| 34 | `/rest/hud` | GET (app) |
| 35 | `/rest/hud/toggle/{component}` | POST (app) |
| 36 | `/rest/hud/toggleAllComponents/{visible}` | POST (app) |
| 37 | `/rest/liveryeditor/setCamera/{camera}` | POST (app) |
| 38 | `/rest/liveryeditor/showRegionTexture/{active}` | POST (app) |
| 39 | `/rest/liveryeditor/submitCustomSkin` | -- (not referenced by bundled UI JS) |
| 40 | `/rest/materialeditor/download/{materialGuid}` | -- (not referenced by bundled UI JS) |
| 41 | `/rest/materialeditor/liveryeditor/getCustomSkinInfo` | GET (app) |
| 42 | `/rest/materialeditor/liveryeditor/reloadCustomSkin` | POST (app) |
| 43 | `/rest/materialeditor/{materialGuid}` | GET (materialeditor), PUT (materialeditor) |
| 44 | `/rest/materialeditor/{materialGuid}/persist` | POST (materialeditor) |
| 45 | `/rest/materialeditor/{materialGuid}/shader` | PUT (materialeditor) |
| 46 | `/rest/materialeditor/{materialGuid}/{map}` | -- (not referenced by bundled UI JS) |
| 47 | `/rest/multiplayer/cancelJoinRequest` | -- (not referenced by bundled UI JS) |
| 48 | `/rest/multiplayer/exitVehicle` | POST (app) |
| 49 | `/rest/multiplayer/join` | GET (app) |
| 50 | `/rest/multiplayer/join/state` | GET (app) |
| 51 | `/rest/multiplayer/steam/status` | GET (app) |
| 52 | `/rest/multiplayer/takeControlOfVehicle` | POST (app) |
| 53 | `/rest/multiplayer/teams` | GET (app) |
| 54 | `/rest/options/ApplyVideoOptions` | POST (app) |
| 55 | `/rest/options/UIScreen/Controls` | GET (app) |
| 56 | `/rest/options/assign/cancel` | POST (app) |
| 57 | `/rest/options/assign/changestatus` | GET (app) |
| 58 | `/rest/options/assign/confirm` | POST (app) |
| 59 | `/rest/options/commandline` | GET (app) |
| 60 | `/rest/options/display` | GET (overlays) |
| 61 | `/rest/options/float` | -- (not referenced by bundled UI JS) |
| 62 | `/rest/options/getAllHapticsDevices` | GET (app) |
| 63 | `/rest/options/getAllResolutions` | GET (app) |
| 64 | `/rest/options/getAllSoundDevices` | GET (app) |
| 65 | `/rest/options/getLanguage` | GET (app) |
| 66 | `/rest/options/graphics/confirmgraphics` | POST (app) |
| 67 | `/rest/options/graphics/resetgraphics` | POST (app) |
| 68 | `/rest/options/liveInputs` | GET (app) |
| 69 | `/rest/options/long` | -- (not referenced by bundled UI JS) |
| 70 | `/rest/options/occlusionCullingSupported` | GET (app) |
| 71 | `/rest/options/resetVRView` | GET (app) |
| 72 | `/rest/options/setConfigControl` | POST (app) |
| 73 | `/rest/options/setControls` | POST (app) |
| 74 | `/rest/options/setHapticsDevice` | POST (app) |
| 75 | `/rest/options/setInMenuGfxEffects` | PUT (app) |
| 76 | `/rest/options/setInputAxisProperties` | POST (app) |
| 77 | `/rest/options/setSoundDevice` | POST (app) |
| 78 | `/rest/options/settings` | GET (app) |
| 79 | `/rest/options/unsetConfigControl` | POST (app) |
| 80 | `/rest/profile/` | GET (app) |
| 81 | `/rest/profile/DLC/viewDLC` | POST (app) |
| 82 | `/rest/profile/eacActive` | GET (app) |
| 83 | `/rest/profile/firstRun` | GET (app) |
| 84 | `/rest/profile/getAuthSessionTicket` | GET (app) |
| 85 | `/rest/profile/inDevMode` | GET (app) |
| 86 | `/rest/profile/profileInfo/getProfileInfo` | GET (app) |
| 87 | `/rest/profile/profileInfo/setProfileInfo` | POST (app) |
| 88 | `/rest/race/car` | GET (app) |
| 89 | `/rest/race/car/` | -- (not referenced by bundled UI JS) |
| 90 | `/rest/race/car/{id}/image` | -- (not referenced by bundled UI JS) |
| 91 | `/rest/race/getAllowedToStartRacing` | GET (app) |
| 92 | `/rest/race/startRace` | -- (not referenced by bundled UI JS) |
| 93 | `/rest/race/track` | POST (app) |
| 94 | `/rest/race/track/` | -- (not referenced by bundled UI JS) |
| 95 | `/rest/race/track/{id}/trackmap` | GET (app) |
| 96 | `/rest/replay/CameraController/getCameraInfo` | -- (not referenced by bundled UI JS) |
| 97 | `/rest/replay/CameraController/setCamera` | -- (not referenced by bundled UI JS) |
| 98 | `/rest/replay/CameraController/setCameraByName/{cameraName}` | POST (app) |
| 99 | `/rest/replay/isActive` | GET (app) |
| 100 | `/rest/replay/toggleactive` | POST (app) |
| 101 | `/rest/sessions/?` | GET (app) |
| 102 | `/rest/sessions/Championship/getCurrentChampTemplate` | POST (app) |
| 103 | `/rest/sessions/Championship/getGrid` | POST (app) |
| 104 | `/rest/sessions/Championship/setCurrentChampionshipTemplate` | POST (app) |
| 105 | `/rest/sessions/Coop/setCoopDriverID` | POST (app) |
| 106 | `/rest/sessions/FFtoRaceEnd` | POST (app) |
| 107 | `/rest/sessions/GetGameState` | GET (app) |
| 108 | `/rest/sessions/GetSessionsInfoForEvent` | GET (app) |
| 109 | `/rest/sessions/MultiStintRace/Drive` | POST (app) |
| 110 | `/rest/sessions/MultiStintRace/UnPause` | POST (app) |
| 111 | `/rest/sessions/MultiStintRace/setDriverInfo` | POST (app) |
| 112 | `/rest/sessions/SaveLoad/compressSaveFile` | POST (app) |
| 113 | `/rest/sessions/SaveLoad/decompressSaveFile` | POST (app) |
| 114 | `/rest/sessions/SaveLoad/deleteSaveFile` | POST (app) |
| 115 | `/rest/sessions/SaveLoad/doesBackupExistForThisSession` | POST (app) |
| 116 | `/rest/sessions/SaveLoad/generateSaveFileFromSessionPreset` | POST (app) |
| 117 | `/rest/sessions/SaveLoad/getEveryLocalSave` | POST (app) |
| 118 | `/rest/sessions/SaveLoad/getNumSaves` | -- (not referenced by bundled UI JS) |
| 119 | `/rest/sessions/SaveLoad/getSaveJSON` | GET (app) |
| 120 | `/rest/sessions/SaveLoad/isSaveNameValid` | POST (app) |
| 121 | `/rest/sessions/SaveLoad/loadGame` | POST (app) |
| 122 | `/rest/sessions/SaveLoad/saveGame` | -- (not referenced by bundled UI JS) |
| 123 | `/rest/sessions/SaveLoad/saveLastBackup` | POST (app) |
| 124 | `/rest/sessions/SaveLoad/saveTemplateToFile` | -- (not referenced by bundled UI JS) |
| 125 | `/rest/sessions/SessionPresets/applyPreset` | POST (app) |
| 126 | `/rest/sessions/SessionPresets/getDefaultPresetForTrack` | POST (app) |
| 127 | `/rest/sessions/SessionPresets/requestPreset` | POST (app) |
| 128 | `/rest/sessions/ai/TakeDriverControl` | POST (app) |
| 129 | `/rest/sessions/ai/forcePlayerVehAiPit` | -- (not referenced by bundled UI JS) |
| 130 | `/rest/sessions/amount` | GET (app) |
| 131 | `/rest/sessions/clearEventNotification` | -- (not referenced by bundled UI JS) |
| 132 | `/rest/sessions/continueGame` | POST (app) |
| 133 | `/rest/sessions/getAllAvailableVehicles` | POST (app) |
| 134 | `/rest/sessions/getAllVehicles` | GET (app/overlays) |
| 135 | `/rest/sessions/getTracksAll` | GET (app) |
| 136 | `/rest/sessions/getTracksInSeries` | GET (app) |
| 137 | `/rest/sessions/notifyInPauseSettings` | POST (app) |
| 138 | `/rest/sessions/opponents` | GET (app), POST (app) |
| 139 | `/rest/sessions/opponents/all` | DELETE (app) |
| 140 | `/rest/sessions/playVOTrigger` | POST (app) |
| 141 | `/rest/sessions/playerSettings/backupPlayerSettings` | POST (app) |
| 142 | `/rest/sessions/playerSettings/restorePlayerSettingsFromBackup` | POST (app) |
| 143 | `/rest/sessions/raceControlVerification` | POST (app) |
| 144 | `/rest/sessions/restartStintAvailable` | POST (app) |
| 145 | `/rest/sessions/resumePitStop` | POST (app) |
| 146 | `/rest/sessions/returnToMonitor` | POST (app) |
| 147 | `/rest/sessions/saveload/getSaveFileJSONFromFilename` | POST (app) |
| 148 | `/rest/sessions/setEventNotification` | POST (app) |
| 149 | `/rest/sessions/setHudOnWatchScreen` | POST (app) |
| 150 | `/rest/sessions/settings` | POST (app) |
| 151 | `/rest/sessions/weather` | GET (app) |
| 152 | `/rest/sessions/weather/{session}/{node}/{setting}` | -- (not referenced by bundled UI JS) |
| 153 | `/rest/sessions/weather/{session}/{preset}` | POST (app) |
| 154 | `/rest/sessions/{session}/sessions` | POST (app) |
| 155 | `/rest/start/openExternalBrowserToURL` | POST (app) |
| 156 | `/rest/strategy/overall` | GET (app) |
| 157 | `/rest/strategy/pitstop-estimate` | -- (not referenced by bundled UI JS) |
| 158 | `/rest/strategy/usage` | GET (app) |
| 159 | `/rest/watch/focus` | GET (app/overlays) |
| 160 | `/rest/watch/focus/{cameraType}/{trackSideGroup}/{shouldAdvance}` | PUT (app/overlays) |
| 161 | `/rest/watch/focus/{slotid}` | PUT (app/overlays) |
| 162 | `/rest/watch/focusBackward` | PUT (app) |
| 163 | `/rest/watch/focusForward` | PUT (app) |
| 164 | `/rest/watch/getBookmarkedTimestamps` | GET (app) |
| 165 | `/rest/watch/getIncidentsList/{minTimeBetweenContacts}` | GET (app) |
| 166 | `/rest/watch/play/{id}` | GET (app) |
| 167 | `/rest/watch/replay/getReplayFolder` | GET (app) |
| 168 | `/rest/watch/replay/setCurrentMetadata` | PUT (app) |
| 169 | `/rest/watch/replay/setReplayFolder` | PUT (app) |
| 170 | `/rest/watch/replay/setReplayUIVisible` | POST (app) |
| 171 | `/rest/watch/replayCommand/{command}` | PUT (app) |
| 172 | `/rest/watch/replays` | GET (app) |
| 173 | `/rest/watch/replaytime/{time}` | PUT (app/overlays) |
| 174 | `/rest/watch/sessionInfo` | GET (overlays) |
| 175 | `/rest/watch/standings` | GET (overlays) |
| 176 | `/rest/watch/standings/history` | GET (app/overlays) |
| 177 | `/rest/watch/trackmap` | GET (overlays) |
| 178 | `/navigation/GetLoadingScreen` | GET (app) |
| 179 | `/navigation/action/{action}` | POST (app) |
| 180 | `/navigation/getReferrer` | -- (not referenced by bundled UI JS) |
| 181 | `/navigation/openLiveryEditor` | POST (app) |
| 182 | `/navigation/sendToLog` | POST (app) |
| 183 | `/navigation/setReferrer` | -- (not referenced by bundled UI JS) |
| 184 | `/navigation/state` | GET (app) |
| 185 | `/webdata/.*` | GET (app/overlays), POST (overlays) |

UI-side call list: the full list of 181 (method, path) pairs extracted from `app-CjYE-9lu.js` is reproducible with the method in the appendix. It matches the table above except for the items listed before the table.

---

## 8. Shared memory and plugins observed in the install

- Trace log at every start: `SharedMemory 187: SharedMemoryManager initialized.`, `SharedMemory 162: PluginsAdapter: Initialized`, `SharedMemory 179: SharedMemory initialized`. **VERIFIED**
- Exe strings: `LMU_Data`, `LMU_Data_HoldEvent`, `LMU_Data_DataEvent`, **`LMU_Data_Event`** (not in the SDK header's defines; purpose **UNVERIFIED**), `LMU_SharedMemoryLockData`, `LMU_SharedMemoryLockEvent`, `SharedMemory lock timed out, backing off...`, `Global\LMU_PluginsAdapter_Ready`, `PluginsAdapter.exe`. **VERIFIED**
  - **INFERRED**: third-party plugins run out of process in `<LMU>\PluginsAdapter.exe`.
  - **INFERRED**: the `LMU_Data` shared memory is created by the game itself, so it needs no plugin.
- SDK headers: `<LMU>\Support\SharedMemoryInterface\SharedMemoryInterface.hpp` (12,695 B, 2026-09-22), `InternalsPlugin.hpp` (57,621 B, 2026-03-31), `PluginObjects.hpp` (3,551 B, 2025-12-20). They define `LMU_SHARED_MEMORY_FILE "LMU_Data"`, `LMU_SHARED_MEMORY_HOLD_EVENT "LMU_Data_HoldEvent"` and `LMU_SHARED_MEMORY_DATA_EVENT "LMU_Data_DataEvent"`. The lock event is named `LMU_SharedMemoryLockEvent`. `SharedMemoryGeneric` has `long gameVersion`. `SharedMemoryPathData` has `char userData/customVariables/stewardResults/playerProfile/pluginsFolder[MAX_PATH]`. The SME enum runs `SME_ENTER ... SME_FFB, SME_MAX`. **VERIFIED** (names only)
- `<LMU>\UserData\player\CustomPluginVariables.JSON`: all six plugins have `" Enabled": 1`. **VERIFIED**
  - `ChatTransceiver.dll`, `DAMPlugin.dll`, `DRE_LMU_SMMP64.dll`, `LMU.3.8.SharedMemoryMapPlugin.dll` (`UnsubscribedBuffersMask: 160`, `EnableHWControlInput: 1`), `TelemetryToolPluginForLMU.dll` (`Mode: 0`, `Verbose: 0`), `TrackTitanRf2Plugin.dll`
  - None of these is required by our app.

### 8.1 `PLUGIN_NOT_NEEDED_ON_v1505_OR_LATER.txt`. VERIFIED

- It is a 0-byte marker file in `<LMU>\Plugins\` (mtime 2026-03-22), shipped with `TelemetryToolPluginForLMU.dll` and `TelemetryToolPluginForLMU readme.txt` (same mtime).
- The readme (plugin v2.03, 2025-09-28) says the plugin is no longer needed with Telemetry Tool 15.05, because that tool now reads LMU's shared memory directly.
- So **"v1505" is the third-party Telemetry Tool application version 15.05, not an LMU version**. The plugin itself exposes `$TT_LMU_Telemetry$`, `$TT_LMU_Scoring$` and `$TT_LMU_Plugin$` (double-buffered, 12-byte headers, 120 slots). This is **CLAIMED** by the readme. Our app must not depend on it.

---

## 9. Implications for LMU Race Engineering (INFERRED)

1. **Live data:** use native `LMU_Data` shared memory (built into the game), plus the REST/WebSocket API on 127.0.0.1:6397/6398 for standings, session, pit menu and strategy data. No third-party plugin is needed. The REST server binds to 127.0.0.1 by default, so a remote engineer app needs our own relay. We should not ask users to set `WebUI bind` to `0.0.0.0`.
2. **Post-session import:** we can read LMU's native DuckDB recordings directly with the Rust `duckdb` crate. The files were written by DuckDB v1.4.0 (storage version 64), so the crate must bundle libduckdb ≥ 1.4 (Python duckdb 1.5.6 read them fine). Rebuild channel time as `t0 + rowid/frequency`, with `t0` = first `GPS Time`. Use `rows/duration` for the effective rate (the 7 Hz channels run at ~7.018 Hz). Map `value1..4` → FL, FR, RL, RR. Quote table names.
3. **Test fixtures:** the 4 third-party DuckDB files are someone else's content and contain a driver name, so **do not copy them into the repo**. Record one of our own sessions with "Automatically Record Telemetry" enabled and use that, with the name redacted if needed.
4. **Open items for a live session:**
   - `gameVersion` value
   - Exact recording filename/directory
   - Whether `config.json` edits take effect
   - The swagger schema at `/swagger-schema.json`
   - REST response schemas and WebSocket payloads
   - `Plugin Mask` semantics
   - Units for `Brakes Force`, `Brake Thickness` and `Wind Heading`
   - `G Force Lat` sign

---

## Appendix: method (reproducible, read-only)

- The config was parsed with PowerShell `ConvertFrom-Json`.
- DuckDB files were copied to a temp dir and opened with `duckdb.connect(path, read_only=True)`. Queries used: `duckdb_tables()`, `duckdb_columns()`, `pragma_database_size()`, `SELECT * FROM metadata/channelsList/eventsList`, per-table `count(*)`/min/max, and `ORDER BY rowid` for sample order.
- Exe/DLL strings came from a Python regex scan for printable ASCII (≥ 3–4 chars) and UTF-16LE runs.
- `UI.zip` was read in memory with Python `zipfile`, with no extraction into the LMU folder. The regex extracted `const X = "/rest/..."` / `` `${base}...` `` constants, resolved them transitively, and captured the first argument of each ky wrapper call.
- `.ld` channel headers were parsed from the MoTeC channel-meta linked list (pointer at file offset 8).
- Scripts were kept in the session scratchpad, outside the repo, and are not committed.
