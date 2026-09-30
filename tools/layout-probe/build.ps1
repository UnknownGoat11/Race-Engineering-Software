<#
.SYNOPSIS
  Builds and runs the LMU shared-memory ABI layout probe against the *installed* SDK headers.

.DESCRIPTION
  Compiles probe.cpp with the installed LMU SDK directory on the include path (the headers
  are never copied), runs it, writes the layout JSON, fingerprints the three headers
  (SHA-256 + LastWriteTime) and, if Python is available, cross-checks the probe's hand-written
  field lists against the header text (crosscheck_fields.py).

  Toolchains:
    msvc   vswhere -> vcvars64.bat -> cl.exe /std:c++17 /EHsc /W4. Needs the Windows SDK
           (windows.h + UCRT). This is the reference toolchain (LMU itself is an MSVC x64 build).
    clang  An llvm-mingw clang++ (x86_64-w64-mingw32 target, Windows LLP64 data model). Same
           layout rules for these plain #pragma-pack structs; used when MSVC or the Windows SDK
           is missing. Confirm against MSVC later with -EmitAssertsFile (see README.md).
    auto   (default) msvc if fully usable, otherwise clang (with a warning).

  Environment changes made by vcvars are undone before the script returns.

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File tools\layout-probe\build.ps1
.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File tools\layout-probe\build.ps1 -Toolchain msvc -LmuDir "D:\SteamLibrary\steamapps\common\Le Mans Ultimate"
#>
[CmdletBinding()]
param(
    [string]$LmuDir = "C:\Program Files (x86)\Steam\steamapps\common\Le Mans Ultimate",
    [string]$OutFile,
    [string]$FingerprintFile,
    [string]$EmitAssertsFile,
    [string]$BuildDir = (Join-Path $env:TEMP "lmu-layout-probe-build"),
    [ValidateSet("auto", "msvc", "clang")]
    [string]$Toolchain = "auto",
    [string]$ClangPath,
    [string[]]$ClangArgs = @("--target=x86_64-w64-mingw32", "-rtlib=compiler-rt", "-unwindlib=libunwind",
                             "-stdlib=libc++", "-fuse-ld=lld", "-static"),
    [switch]$SkipCrossCheck
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if (-not $OutFile) { $OutFile = Join-Path $repoRoot "docs\research\lmu_abi_layout.json" }
if (-not $FingerprintFile) { $FingerprintFile = Join-Path $repoRoot "docs\research\lmu_headers_fingerprint.json" }
$OutFile = [System.IO.Path]::GetFullPath($OutFile)
$FingerprintFile = [System.IO.Path]::GetFullPath($FingerprintFile)
$probeSrc = Join-Path $PSScriptRoot "probe.cpp"
$sdkDir = (Join-Path $LmuDir "Support\SharedMemoryInterface").TrimEnd('\')
$headerNames = @("SharedMemoryInterface.hpp", "InternalsPlugin.hpp", "PluginObjects.hpp")
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

# Runs a native tool with output to the host; returns its exit code. (Windows PowerShell 5.1
# turns native stderr into terminating errors under "Stop", so relax it for the call.)
function Invoke-Native([string]$File, [string[]]$Arguments) {
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try { & $File @Arguments | Out-Host } finally { $ErrorActionPreference = $prev }
    return $LASTEXITCODE
}

# ---------------------------------------------------------------------------- MSVC discovery
function Find-VcVars {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    $paths = @()
    if (Test-Path -LiteralPath $vswhere) {
        $req = "Microsoft.VisualStudio.Component.VC.Tools.x86.x64"
        $paths += @(& $vswhere -latest -products * -requires $req -property installationPath)
        # -latest/-requires skip instances whose installation is incomplete; fall back to -all.
        $paths += @(& $vswhere -all -products * -property installationPath)
    }
    foreach ($p in ($paths | Where-Object { $_ } | Select-Object -Unique)) {
        # vcvars64.bat = x64-hosted x64 tools; vcvarsx86_amd64.bat = x86-hosted x64 tools (same target).
        foreach ($bat in @("vcvars64.bat", "vcvarsx86_amd64.bat")) {
            $candidate = Join-Path $p "VC\Auxiliary\Build\$bat"
            if (Test-Path -LiteralPath $candidate) { return $candidate }
        }
    }
    return $null
}

function Import-VcVarsEnvironment([string]$vcvars) {
    $cmdFile = Join-Path $BuildDir "vcvars-env.cmd"
    # vcvarsall.bat itself looks for vswhere.exe on PATH; put the installer dir there first.
    $installerDir = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer"
    $cmd = "@set `"PATH=$installerDir;%PATH%`"`r`n@call `"$vcvars`" >nul`r`n@if errorlevel 1 exit /b 1`r`n@set`r`n"
    [System.IO.File]::WriteAllText($cmdFile, $cmd, $utf8NoBom)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try { $lines = & cmd.exe /d /c $cmdFile } finally { $ErrorActionPreference = $prev }
    if ($LASTEXITCODE -ne 0) { return $false }
    foreach ($line in $lines) {
        $i = "$line".IndexOf('=')
        if ($i -gt 0) { Set-Item -LiteralPath ("Env:" + "$line".Substring(0, $i)) -Value "$line".Substring($i + 1) }
    }
    return $true
}

# Returns $null when MSVC + Windows SDK are usable, otherwise the reason.
function Test-MsvcUsable {
    $script:vcvars = Find-VcVars
    if (-not $script:vcvars) { return "no vcvars64.bat / vcvarsx86_amd64.bat found via vswhere" }
    if (-not (Import-VcVarsEnvironment $script:vcvars)) { return "$script:vcvars returned an error" }
    if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) { return "cl.exe not on PATH after $script:vcvars" }
    $include = "$env:INCLUDE"
    foreach ($d in ($include -split ';' | Where-Object { $_ })) {
        if (Test-Path -LiteralPath (Join-Path $d "windows.h")) { return $null }
    }
    return "windows.h not found on INCLUDE (Windows SDK not installed?)"
}

function Find-Clang {
    if ($ClangPath) { return $ClangPath }
    $cands = @(
        (Join-Path $env:ProgramFiles "Windhawk\Compiler\bin\clang++.exe"),  # Windhawk ships llvm-mingw
        "C:\llvm-mingw\bin\clang++.exe"
    )
    foreach ($c in $cands) { if (Test-Path -LiteralPath $c) { return $c } }
    $onPath = Get-Command x86_64-w64-mingw32-clang++.exe -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    return $null
}

# ---------------------------------------------------------------------------- main
foreach ($h in $headerNames) {
    if (-not (Test-Path -LiteralPath (Join-Path $sdkDir $h))) {
        throw "SDK header not found: $(Join-Path $sdkDir $h). Pass -LmuDir <LMU install dir>."
    }
}
New-Item -ItemType Directory -Force -Path $BuildDir | Out-Null
foreach ($f in @($OutFile, $FingerprintFile)) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $f) | Out-Null
}

$envSnapshot = @{}
Get-ChildItem Env: | ForEach-Object { $envSnapshot[$_.Name] = $_.Value }

try {
    # ------------------------------------------------------------------------ build
    $exe = Join-Path $BuildDir "probe.exe"
    if (Test-Path -LiteralPath $exe) { Remove-Item -LiteralPath $exe -Force }
    $used = $null
    $buildCommand = $null

    if ($Toolchain -eq "msvc" -or $Toolchain -eq "auto") {
        $why = Test-MsvcUsable
        if ($why) {
            if ($Toolchain -eq "msvc") { throw "MSVC toolchain not usable: $why" }
            Write-Warning "MSVC toolchain not usable ($why); falling back to clang."
        } else {
            $obj = Join-Path $BuildDir "probe.obj"
            $clArgs = @("/nologo", "/std:c++17", "/EHsc", "/W4", "/I", $sdkDir, "/Fo:$obj", "/Fe:$exe", $probeSrc)
            $buildCommand = "cl.exe " + ($clArgs -join ' ') + "   [env: $script:vcvars]"
            Write-Host "> $buildCommand"
            if ((Invoke-Native "cl.exe" $clArgs) -ne 0) { throw "cl.exe failed" }
            $used = "msvc"
        }
    }
    if (-not $used) {
        $clang = Find-Clang
        if (-not $clang) {
            throw "No usable toolchain: install the Windows SDK (MSVC) or pass -ClangPath <llvm-mingw clang++.exe>."
        }
        $cArgs = @($ClangArgs) + @("-std=c++17", "-Wall", "-Wextra", "-I", $sdkDir, $probeSrc, "-o", $exe)
        $buildCommand = "`"$clang`" " + ($cArgs -join ' ')
        Write-Host "> $buildCommand"
        if ((Invoke-Native $clang $cArgs) -ne 0) { throw "clang++ failed" }
        $used = "clang"
    }

    # ------------------------------------------------------------------------ run
    $probeExit = Invoke-Native $exe @("--out", $OutFile)
    if ($probeExit -eq 1 -or -not (Test-Path -LiteralPath $OutFile)) { throw "probe.exe failed (usage/IO error)" }
    $layout = Get-Content -LiteralPath $OutFile -Raw | ConvertFrom-Json
    if ($probeExit -ne 0 -or -not $layout.checks.all_ok) {
        throw "probe self-checks FAILED: $($layout.checks.errors -join '; ')  (see $OutFile)"
    }
    Write-Host "Wrote $OutFile (toolchain: $used; sizeof(SharedMemoryLayout) = $($layout.absolute_offsets.sizeof_SharedMemoryLayout))"

    if ($EmitAssertsFile) {
        $EmitAssertsFile = [System.IO.Path]::GetFullPath($EmitAssertsFile)
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $EmitAssertsFile) | Out-Null
        if ((Invoke-Native $exe @("--emit-asserts", "--out", $EmitAssertsFile)) -ne 0) { throw "probe.exe --emit-asserts failed" }
        Write-Host "Wrote $EmitAssertsFile"
    }

    # ------------------------------------------------------------------------ fingerprint
    $headers = [ordered]@{}
    foreach ($h in $headerNames) {
        $item = Get-Item -LiteralPath (Join-Path $sdkDir $h)
        $headers[$h] = [ordered]@{
            sha256          = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            bytes           = $item.Length
            last_write_time = $item.LastWriteTime.ToString("yyyy-MM-ddTHH:mm:ss.fffzzz")
            last_write_utc  = $item.LastWriteTimeUtc.ToString("yyyy-MM-ddTHH:mm:ss.fffZ")
        }
    }
    $game = [ordered]@{ exe_file_version = $null; steam_app_id = 2399420; steam_buildid = $null }
    $gameExe = Join-Path $LmuDir "Le Mans Ultimate.exe"
    if (Test-Path -LiteralPath $gameExe) { $game.exe_file_version = (Get-Item -LiteralPath $gameExe).VersionInfo.FileVersion }
    $manifest = Join-Path (Split-Path -Parent (Split-Path -Parent $LmuDir)) "appmanifest_2399420.acf"
    if (Test-Path -LiteralPath $manifest) {
        $m = Select-String -LiteralPath $manifest -Pattern '"buildid"\s+"(\d+)"' | Select-Object -First 1
        if ($m) { $game.steam_buildid = $m.Matches[0].Groups[1].Value }
    }
    $fingerprint = [ordered]@{
        schema        = "lmu-headers-fingerprint/1"
        generated_utc = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
        sdk_dir       = $sdkDir
        note          = "Headers are referenced from the LMU install at build time and never copied (their license forbids redistribution or modification)."
        headers       = $headers
        game          = $game
        layout_json   = [ordered]@{
            file      = [System.IO.Path]::GetFileName($OutFile)
            sha256    = (Get-FileHash -LiteralPath $OutFile -Algorithm SHA256).Hash.ToLowerInvariant()
            toolchain = $used
            build     = $buildCommand
        }
    }
    [System.IO.File]::WriteAllText($FingerprintFile, ($fingerprint | ConvertTo-Json -Depth 6) + "`n", $utf8NoBom)
    Write-Host "Wrote $FingerprintFile"

    # ------------------------------------------------------------------------ cross-check
    if (-not $SkipCrossCheck) {
        $py = Get-Command py.exe -ErrorAction SilentlyContinue
        $pyArgs = @("-3")
        if (-not $py) { $py = Get-Command python.exe -ErrorAction SilentlyContinue; $pyArgs = @() }
        if (-not $py) {
            Write-Warning "Python not found; skipped crosscheck_fields.py (field names vs header text)."
        } else {
            $ccArgs = $pyArgs + @("-B", (Join-Path $PSScriptRoot "crosscheck_fields.py"), $OutFile, $sdkDir)
            if ((Invoke-Native $py.Source $ccArgs) -ne 0) {
                throw "crosscheck_fields.py FAILED: probe field lists do not match the installed headers"
            }
        }
    }
}
finally {
    # Undo vcvars environment changes.
    foreach ($name in @(Get-ChildItem Env: | ForEach-Object { $_.Name })) {
        if (-not $envSnapshot.ContainsKey($name)) { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
    }
    foreach ($kv in $envSnapshot.GetEnumerator()) {
        Set-Item -LiteralPath ("Env:" + $kv.Key) -Value $kv.Value
    }
}
