# LMU shared-memory ABI layout probe

This tool gets the byte layout of Le Mans Ultimate's native shared-memory interface
(mapping `LMU_Data`) from the headers LMU installs. The Rust reader can then be checked
against a compiler's actual output instead of hand-counted offsets.

It compiles `probe.cpp` against the SDK headers in the LMU install directory and records,
for every struct reachable from `SharedMemoryObjectOut` (and the lock's private `LockData`):

- `sizeof` and `alignof` of the struct, plus `offsetof`, `sizeof`, type, kind and array dims of every field
- padding holes, and whether alignment explains each one
- `SME_*`, `IP_VehicleClass` and `IP_VehicleChampionship` values, `SME_MAX` and `MAX_PATH`
- the names of the mapping, events and lock
- absolute offsets of key paths inside `SharedMemoryLayout` (for example `data.telemetry.telemInfo`)

Outputs, written by `build.ps1`:

| File | Content |
|---|---|
| `docs/research/lmu_abi_layout.json` | The layout data. This is the source of truth for the Rust reader. |
| `docs/research/lmu_headers_fingerprint.json` | SHA-256, size and LastWriteTime of the three headers; LMU exe version and Steam build id; the hash of the layout JSON; and the exact build command used. |

## License constraint: the headers are referenced, never copied

`SharedMemoryInterface.hpp`, `InternalsPlugin.hpp` and `PluginObjects.hpp`
(`<LMU>\Support\SharedMemoryInterface\`) are Studio 397 / Motorsport Games files. Their
license says *"Redistribution or modification of this header is not permitted."* So:

- **Never copy these headers, or large verbatim parts of them, into this repository.**
  The probe reaches them only through the include path (`/I <LMU>\Support\SharedMemoryInterface`).
- Recording facts such as names, offsets, sizes, enum values and units is fine; that is what the JSON holds.
- The build output (`probe.exe`, objects, generated assert files) goes to `%TEMP%`, not the repo.

## Usage

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\layout-probe\build.ps1
# options:
#   -LmuDir "D:\SteamLibrary\steamapps\common\Le Mans Ultimate"   (default: Steam default path)
#   -OutFile <json>  -FingerprintFile <json>  -BuildDir <dir>
#   -Toolchain auto|msvc|clang   (default auto: MSVC if usable, else clang)
#   -ClangPath <clang++.exe>  -EmitAssertsFile <file.cpp>  -SkipCrossCheck
```

Toolchains:

- **msvc** (the reference). `build.ps1` finds Visual Studio / Build Tools with `vswhere`
  (it also tries `-all`, because `-latest` skips incomplete installs), imports
  `VC\Auxiliary\Build\vcvars64.bat`, then runs
  `cl.exe /nologo /std:c++17 /EHsc /W4 /I <sdk> probe.cpp`. This needs the Windows SDK
  (`windows.h`, UCRT). Environment changes are undone when the script exits.
- **clang**: an llvm-mingw `clang++` targeting `x86_64-w64-mingw32`, for example the copy that
  Windhawk ships in `C:\Program Files\Windhawk\Compiler`. It is the fallback when MSVC or the
  Windows SDK is missing. For these structs its layout matched MSVC byte for byte on
  2026-09-30. Because MSVC is the reference, confirm any clang-only result with the assert file
  (below) as soon as MSVC is available.

Exit status is non-zero if any self-check or the cross-check fails.

## How completeness is checked

The fields are listed by hand in `probe.cpp`, one `offsetof`/`sizeof` pair each. Two independent checks guard that list:

1. **Byte coverage and alignment** (in `probe.cpp`). Every byte not covered by a listed field
   is reported as padding. An interior hole is accepted only if it equals the alignment gap
   before the next field, which is `min(alignof(next), pack)`. A tail hole is accepted only if
   it rounds the struct up to its `alignof`. A missing field of 4 or more bytes shows up as an
   unexplained hole and fails the run. *Limitation:* a forgotten 1–3 byte field sitting just
   before an aligned member looks exactly like padding.
2. **Name check** (`crosscheck_fields.py`, standard-library Python). It parses the struct and
   enum bodies of the installed headers in place and requires that the JSON's field and
   enumerator names match the headers exactly and in order. This closes the gap left by
   check 1. `build.ps1` runs it automatically when `py` or `python` is on PATH.

Both checks were negative-tested: removing `mLapNumber` and `mRearTireCompoundIndex` from the
probe made check 1 flag the first, and check 2 flag the second.

## Cross-compiler / cross-machine check (`--emit-asserts`)

`probe.exe --emit-asserts --out layout_asserts.cpp` (or `build.ps1 -EmitAssertsFile ...`)
writes a translation unit with one `static_assert` per recorded size, alignment, offset and
enum value. It contains no header content; it `#include`s the installed headers. If it
compiles with another compiler, header version or machine, that compiler's layout is
identical. On 2026-09-30 the MSVC-generated asserts compiled with clang, the clang-generated
asserts compiled with MSVC, and a deliberately wrong value was rejected (control).

## Layout facts the Rust reader must honour (see the JSON for everything)

- `InternalsPlugin.hpp` wraps its structs in `#pragma pack(push, 4)`. Doubles there are only
  4-byte aligned. For example `TelemWheelV01::mVerticalTireDeflection` sits at offset 180.
  Mirror these structs with `#[repr(C, packed(4))]`.
- `SharedMemoryInterface.hpp` has **no** `#pragma pack`, so the `SharedMemory*` wrapper structs
  use the compiler's default packing (max alignment 8). That gives two padding holes that
  matter:
  - `SharedMemoryScoringData`: 4 bytes after `scoringInfo` (548 → 552), because
    `size_t scoringStreamSize` is 8-aligned.
  - `SharedMemoryTelemetryData`: 1 byte after `playerHasVehicle` (3 → 4).

  `SharedMemoryObjectOut` also has 4 bytes of tail padding. If LMU were built with `/Zp4`,
  everything from `scoring.scoringStreamSize` onward would sit 4 bytes earlier.
  `SharedMemoryLayout` would then be 324816 bytes instead of 324824. The default packing is
  assumed, not yet confirmed against a live `LMU_Data` mapping.
- `long` / `unsigned long` are 4 bytes (Windows LLP64); pointers, `HWND` and `size_t` are 8.

Re-run the probe whenever `lmu_headers_fingerprint.json` stops matching the installed
headers, for example after an LMU update.
