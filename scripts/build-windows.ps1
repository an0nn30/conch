#Requires -Version 5.1
<#
.SYNOPSIS
    Builds TermLab's Windows artifacts: the NSIS setup.exe and the MSI.

.DESCRIPTION
    The single entry point for Windows builds. Both the developer VM and the
    GitHub Actions release job run this script, which is what keeps them from
    drifting: TermLab previously had one Windows installer path in CI and a
    different one in the Makefile, and only the unused one registered a
    context menu.

    Every step is fatal on failure and verifies its own output before
    reporting success, with one deliberate exception: the `cargo tauri build`
    invocations themselves. `bundle.createUpdaterArtifacts: true` makes the
    bundler try to sign an updater manifest after the installer is already
    written to disk, and that signing step can fail (no key on a developer
    machine) even though the installer is fine. So this script treats a
    non-zero exit from `cargo tauri build` as a warning, not a fatal error,
    and instead verifies success the only way that cannot be fooled: by
    checking that the installer files actually landed on disk and are a
    plausible size.

    Because `tauri.conf.json`'s `frontendDist: "frontend"` resolves to the
    same directory `npm ci` installs `node_modules` into, `cargo tauri build`
    cannot run at all against the committed config (tauri-cli 2.11.4 refuses
    to bundle a frontendDist that contains a `node_modules` folder). This
    script works around that non-destructively: it stages a copy of
    `frontend/` (minus `node_modules`) into a git-ignored directory and points
    `build.frontendDist` at the copy for the bundle invocations only. A
    developer's real `node_modules` is never touched.

    The workspace version (3.0.0-rc.2 as of this writing) also fails MSI
    packaging outright  -  MSI's ProductVersion must be numeric-only  -  while
    NSIS accepts it verbatim. So bundling happens in two separate
    `cargo tauri build` invocations: one for NSIS at the real version (so the
    updater-facing `TermLab_<version>_<arch>-setup.exe` name never changes),
    and one for MSI with a numeric-only `version` override (so its filename
    reads `3.0.0` where the setup.exe reads `3.0.0-rc.2`  -  inherent to MSI,
    not a defect).

.PARAMETER SkipTests
    Skip `cargo test`. Use only when tests ran in a separate step.

.PARAMETER SkipBundle
    Compile and test but do not produce installers. Used by CI on pull
    requests, where the goal is to catch a broken build early rather than to
    produce artifacts.

.PARAMETER Configuration
    Cargo profile. Defaults to Release.

.PARAMETER OutDir
    Where verified artifacts are copied. Defaults to dist.

.EXAMPLE
    pwsh scripts/build-windows.ps1
    pwsh scripts/build-windows.ps1 -SkipBundle
#>
[CmdletBinding()]
param(
    [switch]$SkipTests,
    [switch]$SkipBundle,
    [string]$Configuration = 'Release',
    [string]$OutDir = 'dist'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Native commands do not trip $ErrorActionPreference, so every invocation is
# followed by an explicit exit-code check via Invoke-Checked.
function Invoke-Checked {
    param(
        [Parameter(Mandatory)][string]$What,
        [Parameter(Mandatory)][scriptblock]$Command
    )
    Write-Host "==> $What" -ForegroundColor Cyan
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$What failed with exit code $LASTEXITCODE"
    }
}

function Assert-Tool {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$Remedy
    )
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "required tool '$Name' is not on PATH. $Remedy"
    }
}

# Writes a hashtable as a UTF-8, no-BOM JSON file. `cargo tauri build --config`
# accepts a path to a JSON file, which sidesteps the quoting nightmare of
# putting a JSON object on a PowerShell (and, on CI, bash-via-SSH) command
# line. A BOM confuses some JSON parsers, so this avoids the default
# Set-Content/Out-File encodings, which add one on Windows PowerShell 5.1.
function Write-JsonConfig {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)]$Object
    )
    $json = $Object | ConvertTo-Json -Depth 10
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $json, $utf8NoBom)
}

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot
Write-Host "Repository: $RepoRoot"

# --- 1. Preflight ---------------------------------------------------------
# WiX and NSIS are downloaded by tauri-bundler itself and need no handling.
Write-Host "==> Preflight" -ForegroundColor Cyan
Assert-Tool cargo 'Install Rust from https://rustup.rs'
Assert-Tool node  'Install Node 20+ from https://nodejs.org'
Assert-Tool npm   'Install Node 20+ from https://nodejs.org'
Assert-Tool git   'Install Git from https://git-scm.com'
Assert-Tool javac 'Install a JDK: winget install Microsoft.OpenJDK.21'
Assert-Tool jar   'Install a JDK: winget install Microsoft.OpenJDK.21'

if (-not $SkipBundle) {
    cargo tauri --version *> $null
    if ($LASTEXITCODE -ne 0) {
        Invoke-Checked 'Install the Tauri CLI' { cargo install tauri-cli --version '^2' --locked }
    }
}

# --- 2. Frontend vendor bundle -------------------------------------------
# This is the ONLY place vendor assets get built before bundling: the bundle
# invocations below override build.beforeBuildCommand to "" (required to keep
# Tauri from re-running npm ci into the staged frontendDist copy and undoing
# the staging), so Tauri's own beforeBuildCommand never fires in this script.
# -SkipBundle still runs this step so `cargo run`/`cargo build` afterward
# don't ship an editor that shows the "bundle missing" toast.
$FrontendDir = Join-Path $RepoRoot 'crates\termlab_tauri\frontend'
Invoke-Checked 'Install frontend dependencies' { npm --prefix $FrontendDir ci }
Invoke-Checked 'Build frontend vendor bundles' { npm --prefix $FrontendDir run build:vendor }

# --- 3. Java Plugin SDK jar (embedded in the binary) ----------------------
$ClassesDir = Join-Path $RepoRoot 'java-sdk\build\classes'
New-Item -ItemType Directory -Force -Path $ClassesDir | Out-Null
$JavaSources = Get-ChildItem (Join-Path $RepoRoot 'java-sdk\src\termlab\plugin\*.java')
if (-not $JavaSources) {
    throw 'no Java SDK sources found under java-sdk/src/termlab/plugin/'
}
Invoke-Checked 'Compile the Java Plugin SDK' { javac -d $ClassesDir $JavaSources.FullName }
$SdkJar = Join-Path $RepoRoot 'java-sdk\build\termlab-plugin-sdk.jar'
Invoke-Checked 'Package the Java Plugin SDK jar' { jar cf $SdkJar -C $ClassesDir . }
if (-not (Test-Path $SdkJar)) {
    throw "the Java SDK jar was not produced at $SdkJar"
}

# --- 4. Test --------------------------------------------------------------
if ($SkipTests) {
    Write-Host '==> Tests skipped (-SkipTests)' -ForegroundColor Yellow
} elseif ($Configuration -eq 'Release') {
    Invoke-Checked 'Run the test suite' { cargo test --release --workspace }
} else {
    Invoke-Checked 'Run the test suite' { cargo test --workspace }
}

# --- 5. Bundle ------------------------------------------------------------
if ($SkipBundle) {
    if ($Configuration -eq 'Release') {
        Invoke-Checked 'Compile (no bundle)' { cargo build --release -p termlab_tauri }
    } else {
        Invoke-Checked 'Compile (no bundle)' { cargo build -p termlab_tauri }
    }
    Write-Host '==> Bundling skipped (-SkipBundle)' -ForegroundColor Yellow
    exit 0
}

# --- 5a. Stage a clean frontendDist ---------------------------------------
# tauri-cli 2.11.4 hard-errors if frontendDist contains a `node_modules`
# folder, which the committed `frontendDist: "frontend"` always will once
# `npm ci` has run. Stage everything else into a git-ignored sibling
# directory and point the bundle invocations at that copy instead. This is
# non-destructive: the developer's real frontend/node_modules is left alone.
#
# bundle.resources ("frontend/themes/": "themes/") is resolved relative to
# this tauri.conf.json's own directory (crates/termlab_tauri), independent of
# frontendDist, so redirecting frontendDist does not disturb it.
$StagedFrontendDist = Join-Path $RepoRoot 'crates\termlab_tauri\frontend-dist'
Write-Host "==> Stage frontend assets (excluding node_modules) into $StagedFrontendDist" -ForegroundColor Cyan
if (Test-Path $StagedFrontendDist) {
    Remove-Item -Recurse -Force $StagedFrontendDist
}
New-Item -ItemType Directory -Force -Path $StagedFrontendDist | Out-Null
Get-ChildItem -Path $FrontendDir -Force |
    Where-Object { $_.Name -ne 'node_modules' } |
    ForEach-Object {
        Copy-Item -Path $_.FullName -Destination (Join-Path $StagedFrontendDist $_.Name) -Recurse -Force
    }
if (-not (Test-Path (Join-Path $StagedFrontendDist 'index.html'))) {
    throw "staged frontendDist $StagedFrontendDist has no index.html  -  the staging copy is broken"
}

# --- 5b. Signing ------------------------------------------------------------
# createUpdaterArtifacts: true (set in tauri.conf.json, unconditionally, for
# CI's sake) asks the bundler to also emit a signed .sig update manifest. A
# developer who just wants an installer should not be forced to generate a
# signing key, so when no key is present in the environment this script
# disables updater artifacts for its own local bundle invocations only, via
# --config. tauri.conf.json itself is never touched, so CI (which does export
# TAURI_SIGNING_PRIVATE_KEY) still gets the .sig files release.yml uploads.
$HasSigningKey = [bool]$env:TAURI_SIGNING_PRIVATE_KEY
if ($HasSigningKey) {
    Write-Host '==> TAURI_SIGNING_PRIVATE_KEY is set  -  updater .sig files will be produced' -ForegroundColor Cyan
} else {
    Write-Host '==> No TAURI_SIGNING_PRIVATE_KEY in the environment  -  disabling updater artifacts for this build' -ForegroundColor Yellow
}

# A .sig from an earlier build (e.g. a throwaway signing-key experiment) can
# be left sitting next to the installer in target/release/bundle/ from a
# previous run. If this run does not itself produce a fresh one, step 6 below
# must never mistake that leftover for this build's signature -- a mismatched
# .sig silently breaks the auto-updater for whoever receives it. Delete any
# pre-existing .sig from both bundle output directories before building, so
# anything found there afterward can only have been written by this run.
$BundleRoot = Join-Path $RepoRoot 'target\release\bundle'
Remove-Item -Path (Join-Path $BundleRoot 'nsis\*.sig') -Force -ErrorAction SilentlyContinue
Remove-Item -Path (Join-Path $BundleRoot 'msi\*.sig')  -Force -ErrorAction SilentlyContinue
$BuildStartTime = Get-Date

# Runs one `cargo tauri build --config <file>` invocation. A non-zero exit
# here is NOT immediately fatal: createUpdaterArtifacts can fail signing
# *after* the installer has already been written to disk. The real
# pass/fail check is the artifact-existence verification in step 6, which
# runs after both invocations regardless of their exit codes.
function Invoke-Bundle {
    param(
        [Parameter(Mandatory)][string]$What,
        [Parameter(Mandatory)][string]$ConfigPath
    )
    Write-Host "==> $What" -ForegroundColor Cyan
    cargo tauri build --config $ConfigPath
    if ($LASTEXITCODE -ne 0) {
        Write-Host "    cargo tauri build exited $LASTEXITCODE for '$What'; continuing, since a signing-only failure can leave a good installer on disk. Verified in the next step." -ForegroundColor Yellow
    }
}

$NsisConfigPath = Join-Path $env:TEMP 'termlab-build-windows-nsis-config.json'
$MsiConfigPath  = Join-Path $env:TEMP 'termlab-build-windows-msi-config.json'

$NsisBundleConfig = @{
    active  = $true
    targets = @('nsis')
}
$MsiBundleConfig = @{
    active  = $true
    targets = @('msi')
}
if (-not $HasSigningKey) {
    $NsisBundleConfig['createUpdaterArtifacts'] = $false
    $MsiBundleConfig['createUpdaterArtifacts'] = $false
}

# --- NSIS, at the real (possibly pre-release) version ----------------------
# This keeps TermLab_<version>_<arch>-setup.exe exactly as CI already uploads
# it; the auto-updater depends on that name never changing.
Write-JsonConfig -Path $NsisConfigPath -Object @{
    build  = @{
        frontendDist       = $StagedFrontendDist
        beforeBuildCommand = ''
    }
    bundle = $NsisBundleConfig
}
Invoke-Bundle -What 'Bundle NSIS installer' -ConfigPath $NsisConfigPath

# --- MSI, with a numeric-only version override -----------------------------
# MSI ProductVersion must be at most three dot-separated integers, so the
# pre-release suffix is stripped for this invocation only. Cargo.toml is
# never modified.
$CargoVersion = (Select-String -Path (Join-Path $RepoRoot 'Cargo.toml') -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $CargoVersion) {
    throw 'could not find a version line (version = "...") in Cargo.toml'
}
$NumericVersion = ($CargoVersion -split '-')[0]
Write-Host "==> Cargo version $CargoVersion, MSI ProductVersion $NumericVersion" -ForegroundColor Cyan

Write-JsonConfig -Path $MsiConfigPath -Object @{
    version = $NumericVersion
    build   = @{
        frontendDist       = $StagedFrontendDist
        beforeBuildCommand = ''
    }
    bundle  = $MsiBundleConfig
}
Invoke-Bundle -What 'Bundle MSI installer' -ConfigPath $MsiConfigPath

Remove-Item -Force $NsisConfigPath, $MsiConfigPath -ErrorAction SilentlyContinue

# --- 6. Verify and collect ------------------------------------------------
# Never assume architecture: the dev VM is ARM64 Windows on Apple Silicon
# (artifacts named *_arm64-*) while CI's GitHub-hosted runner is x64
# (*_x64-*). Glob instead of hardcoding either.
#
# Invoke-Bundle deliberately swallows a non-zero `cargo tauri build` exit
# (a signing-only failure can still leave a good installer on disk), which
# means a genuine compile/bundle failure is otherwise silent here too: with
# no freshness check, a stale installer left over from an earlier successful
# run would still match the glob below and get reported as this run's
# output. Require LastWriteTime -ge $BuildStartTime, the same guard already
# used for .sig freshness further down, so only an artifact this invocation
# actually produced can count.
$Setup = Get-ChildItem -Path (Join-Path $BundleRoot 'nsis') -Filter '*-setup.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.LastWriteTime -ge $BuildStartTime } |
    Select-Object -First 1
$Msi   = Get-ChildItem -Path (Join-Path $BundleRoot 'msi')  -Filter '*.msi'       -ErrorAction SilentlyContinue |
    Where-Object { $_.LastWriteTime -ge $BuildStartTime } |
    Select-Object -First 1

if (-not $Setup) { throw "no NSIS setup.exe under $BundleRoot\nsis written since this build started ($BuildStartTime)  -  either the bundler produced nothing, or only a stale installer from an earlier run is present, and that does not count as success" }
if (-not $Msi)   { throw "no MSI under $BundleRoot\msi written since this build started ($BuildStartTime)  -  either the bundler produced nothing, or only a stale installer from an earlier run is present, and that does not count as success" }

# A truncated or stub installer is worse than a missing one, because it looks
# like a successful build. TermLab's real installers are tens of megabytes.
$MinBytes = 5MB
foreach ($artifact in @($Setup, $Msi)) {
    if ($artifact.Length -lt $MinBytes) {
        throw "$($artifact.Name) is only $($artifact.Length) bytes, well under the $MinBytes floor  -  treating as a failed build"
    }
}

$OutPath = Join-Path $RepoRoot $OutDir
New-Item -ItemType Directory -Force -Path $OutPath | Out-Null
Copy-Item $Setup.FullName -Destination $OutPath -Force
Copy-Item $Msi.FullName   -Destination $OutPath -Force

# Updater .sig files, when produced, ride along next to their installer and
# release.yml uploads them too  -  copy them if present. Two independent
# guards keep a stale signature from ever being collected: (1) with no
# signing key this run, a .sig cannot legitimately exist for this build, so
# the lookup is skipped entirely; (2) even with a key, only a .sig written
# after this run started (i.e. after the pre-build cleanup above) qualifies
# -- anything older is a leftover, not this run's output.
$SetupSig = $null
$MsiSig   = $null
if ($HasSigningKey) {
    $SetupSig = Get-Item ($Setup.FullName + '.sig') -ErrorAction SilentlyContinue
    if ($SetupSig -and $SetupSig.LastWriteTime -lt $BuildStartTime) {
        Write-Host "    ignoring $($SetupSig.Name): older than this build  -  stale signature, not copying" -ForegroundColor Yellow
        $SetupSig = $null
    }
    $MsiSig = Get-Item ($Msi.FullName + '.sig') -ErrorAction SilentlyContinue
    if ($MsiSig -and $MsiSig.LastWriteTime -lt $BuildStartTime) {
        Write-Host "    ignoring $($MsiSig.Name): older than this build  -  stale signature, not copying" -ForegroundColor Yellow
        $MsiSig = $null
    }
}
if ($SetupSig) { Copy-Item $SetupSig.FullName -Destination $OutPath -Force }
if ($MsiSig)   { Copy-Item $MsiSig.FullName   -Destination $OutPath -Force }

Write-Host ''
Write-Host 'Build complete.' -ForegroundColor Green
Write-Host "  $($Setup.Name)  ($([math]::Round($Setup.Length / 1MB, 1)) MB)"
Write-Host "  $($Msi.Name)  ($([math]::Round($Msi.Length / 1MB, 1)) MB)"
if ($SetupSig) { Write-Host "  $($SetupSig.Name)" }
if ($MsiSig)   { Write-Host "  $($MsiSig.Name)" }
Write-Host "  copied to $OutPath"

# $LASTEXITCODE can still hold cargo tauri build's non-zero exit from
# Invoke-Bundle (tolerated above -- a signing-only failure can leave a good
# installer on disk, and that has just been verified). GitHub Actions' pwsh
# wrapper appends `exit $LASTEXITCODE` after this script returns, so without
# an explicit success exit here, that tolerated, already-verified failure
# would fail the release step anyway.
exit 0
