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
    script works around that non-destructively: it MOVES
    `frontend\node_modules` out of the way for the duration of the bundle
    invocations only (never overriding `build.frontendDist` itself -- see the
    long comment above $NodeModulesDir below for why that override is a
    trap), then moves it back afterward via try/finally. A developer's real
    `node_modules` is restored even if the build fails partway through.

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
# Tauri from re-running npm ci -- which would recreate frontend/node_modules
# right after step 5a moves it out of the way, undoing that move), so
# Tauri's own beforeBuildCommand never fires in this script. -SkipBundle
# still runs this step so `cargo run`/`cargo build` afterward don't ship an
# editor that shows the "bundle missing" toast.
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

# --- 5a. Relocate node_modules for the duration of the bundle -------------
# tauri-cli 2.11.4 hard-errors if frontendDist contains a `node_modules`
# folder, which the committed `frontendDist: "frontend"` always will once
# `npm ci` has run (step 2 above). The fix is to get node_modules out of
# frontend/ for the bundle invocations below, then put it back -- NOT to
# override `build.frontendDist` to point somewhere else.
#
# An earlier version of this script did override frontendDist: it staged a
# copy of frontend/ (minus node_modules) into a git-ignored
# crates/termlab_tauri/frontend-dist/ and pointed build.frontendDist at that
# copy. That shipped a real regression. `build.frontendDist` deserializes
# into tauri_utils::config::FrontendDist, an untagged enum tried in THIS
# declared order: Url(url::Url), then Directory(PathBuf), then
# Files(Vec<PathBuf>). A Windows ABSOLUTE path such as
# 'C:\repo\crates\termlab_tauri\frontend-dist' is not rejected by
# url::Url::parse -- verified directly: it parses successfully as an opaque
# URL with scheme "c" (cannot-be-a-base, same class as "mailto:..."),
# because a single letter is syntactically a valid URL scheme. Serde's
# untagged matching takes the FIRST variant that parses, so an absolute
# frontendDist silently became FrontendDist::Url, never
# FrontendDist::Directory, with no error anywhere in the pipeline. Two
# things follow purely from that mis-typing, both in tauri's own source:
# (1) tauri-codegen's context.rs matches `FrontendDist::Url(_) =>
# Default::default()` for embedded assets -- nothing gets embedded, at all;
# (2) tauri's manager/mod.rs get_app_url() matches
# `Some(FrontendDist::Url(url)) => Some(url)` -- the window's start URL
# becomes that literal mangled path instead of the correct
# `tauri://localhost` custom protocol, and the webview's Chromium engine
# renders its own built-in directory-listing page for it. cargo tauri build
# still exits 0 through all of this, and both installers still clear a
# naive size floor (NSIS/WiX boilerplate plus the WebView2 loader is itself
# several MB) -- this is exactly the shipped "Index of C:\Users\..." bug. A
# later attempt fixed the immediate crash by making the override value
# RELATIVE ('frontend-dist') instead of absolute, which does avoid this
# specific url::Url trap, but the deeper problem is overriding
# build.frontendDist AT ALL: it is a code path this project never otherwise
# exercises, sitting directly on a serde-untagged parsing hazard, for no
# benefit over simply keeping node_modules out of the way.
#
# The approach below was run end-to-end on real Windows (ARM64, tauri-cli
# 2.11.4) against the STOCK, unmodified frontendDist: "frontend" and
# produced a working installer with the frontend actually embedded (see
# docs/superpowers/plans/task-1-baseline.md, step 5 and "Surprising
# findings": TermLab_3.0.0-rc.2_arm64-setup.exe at 9,778,153 bytes, versus
# 5,968,724 bytes from the broken frontendDist-override build -- the
# ~3.8 MB delta is the missing embedded frontend). It never touches
# build.frontendDist.
#
# node_modules is MOVED, not deleted or copied: this is a developer's
# working tree, not a disposable CI checkout, and `npm ci` regenerating it
# can take real time. It is moved under target\, which is already
# git-ignored and guaranteed to sit outside the frontend/ subtree tauri-cli
# inspects, rather than under $env:TEMP: a same-volume Move-Item is a fast
# rename, while $env:TEMP can be a different volume or a redirected
# profile path on some Windows setups, which would silently turn this into
# a slow copy-then-delete.
#
# bundle.resources ("frontend/themes/": "themes/") is resolved relative to
# tauri.conf.json's own directory (crates/termlab_tauri), independent of
# frontendDist, so none of this disturbs it.
$NodeModulesDir    = Join-Path $FrontendDir 'node_modules'
$NodeModulesBackup = Join-Path $RepoRoot 'target\windows-build-node_modules-backup'
$NodeModulesMoved  = $false

if (Test-Path $NodeModulesBackup) {
    Remove-Item -Recurse -Force $NodeModulesBackup
}
if (Test-Path $NodeModulesDir) {
    Write-Host "==> Relocating $NodeModulesDir out of frontendDist for the bundle" -ForegroundColor Cyan
    Move-Item -Path $NodeModulesDir -Destination $NodeModulesBackup
    $NodeModulesMoved = $true
} else {
    Write-Host "==> No node_modules under $FrontendDir to relocate (unexpected after npm ci, but not fatal)" -ForegroundColor Yellow
}

try {
    # --- 5b. Signing ---------------------------------------------------------
    # createUpdaterArtifacts: true (set in tauri.conf.json, unconditionally,
    # for CI's sake) asks the bundler to also emit a signed .sig update
    # manifest. A developer who just wants an installer should not be forced
    # to generate a signing key, so when no key is present in the
    # environment this script disables updater artifacts for its own local
    # bundle invocations only, via --config. tauri.conf.json itself is never
    # touched, so CI (which does export TAURI_SIGNING_PRIVATE_KEY) still
    # gets the .sig files release.yml uploads.
    $HasSigningKey = [bool]$env:TAURI_SIGNING_PRIVATE_KEY
    if ($HasSigningKey) {
        Write-Host '==> TAURI_SIGNING_PRIVATE_KEY is set  -  updater .sig files will be produced' -ForegroundColor Cyan
    } else {
        Write-Host '==> No TAURI_SIGNING_PRIVATE_KEY in the environment  -  disabling updater artifacts for this build' -ForegroundColor Yellow
    }

    # A .sig from an earlier build (e.g. a throwaway signing-key experiment)
    # can be left sitting next to the installer in target/release/bundle/
    # from a previous run. If this run does not itself produce a fresh one,
    # step 6 below must never mistake that leftover for this build's
    # signature -- a mismatched .sig silently breaks the auto-updater for
    # whoever receives it. Delete any pre-existing .sig from both bundle
    # output directories before building, so anything found there
    # afterward can only have been written by this run.
    $BundleRoot = Join-Path $RepoRoot 'target\release\bundle'
    Remove-Item -Path (Join-Path $BundleRoot 'nsis\*.sig') -Force -ErrorAction SilentlyContinue
    Remove-Item -Path (Join-Path $BundleRoot 'msi\*.sig')  -Force -ErrorAction SilentlyContinue
    $BuildStartTime = Get-Date

    # Runs one `cargo tauri build --config <file>` invocation. A non-zero
    # exit here is NOT immediately fatal: createUpdaterArtifacts can fail
    # signing *after* the installer has already been written to disk. The
    # real pass/fail check is the artifact-existence verification in step
    # 6, which runs after both invocations regardless of their exit codes.
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

    # --- NSIS, at the real (possibly pre-release) version ------------------
    # This keeps TermLab_<version>_<arch>-setup.exe exactly as CI already
    # uploads it; the auto-updater depends on that name never changing.
    # beforeBuildCommand is still overridden to "" here: without it, Tauri
    # would re-run `npm ci` itself before bundling and recreate
    # node_modules inside frontend/, right back in the way it was just
    # moved out of. frontendDist is deliberately absent -- the stock
    # tauri.conf.json value ("frontend") is used as-is.
    Write-JsonConfig -Path $NsisConfigPath -Object @{
        build  = @{
            beforeBuildCommand = ''
        }
        bundle = $NsisBundleConfig
    }
    Invoke-Bundle -What 'Bundle NSIS installer' -ConfigPath $NsisConfigPath

    # --- MSI, with a numeric-only version override --------------------------
    # MSI ProductVersion must be at most three dot-separated integers, so
    # the pre-release suffix is stripped for this invocation only.
    # Cargo.toml is never modified.
    $CargoVersion = (Select-String -Path (Join-Path $RepoRoot 'Cargo.toml') -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
    if (-not $CargoVersion) {
        throw 'could not find a version line (version = "...") in Cargo.toml'
    }
    $NumericVersion = ($CargoVersion -split '-')[0]
    Write-Host "==> Cargo version $CargoVersion, MSI ProductVersion $NumericVersion" -ForegroundColor Cyan

    Write-JsonConfig -Path $MsiConfigPath -Object @{
        version = $NumericVersion
        build   = @{
            beforeBuildCommand = ''
        }
        bundle  = $MsiBundleConfig
    }
    Invoke-Bundle -What 'Bundle MSI installer' -ConfigPath $MsiConfigPath

    Remove-Item -Force $NsisConfigPath, $MsiConfigPath -ErrorAction SilentlyContinue
} finally {
    # Always restore node_modules, even if a step above threw or the script
    # was interrupted. A developer left without node_modules because a
    # build failed midway is a bad outcome, but a recoverable one -- `npm
    # ci` regenerates it -- so this is a best-effort restore, not a second
    # source of fatal errors.
    if ($NodeModulesMoved) {
        Write-Host "==> Restoring $NodeModulesDir" -ForegroundColor Cyan
        if (Test-Path $NodeModulesDir) {
            Remove-Item -Recurse -Force $NodeModulesDir
        }
        Move-Item -Path $NodeModulesBackup -Destination $NodeModulesDir
    }
}

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
# This is a coarse tripwire only, NOT the check that catches a missing
# frontend -- see the size sanity check (6a) and the marker check (6b) below,
# which are the real tests for that specific regression.
$MinBytes = 5MB
foreach ($artifact in @($Setup, $Msi)) {
    if ($artifact.Length -lt $MinBytes) {
        throw "$($artifact.Name) is only $($artifact.Length) bytes, well under the $MinBytes floor  -  treating as a failed build"
    }
}

# --- 6a. Size sanity check (secondary signal, not the primary gate) -------
# The exact regression this branch shipped once already: a broken build (an
# overridden build.frontendDist that silently swallowed the embedded
# frontend -- see the long comment in step 5a above) produced a real,
# installable, non-empty NSIS installer -- TermLab_3.0.0-rc.2_arm64-setup.exe
# at 5,968,724 bytes -- versus a known-good build of the identical version,
# with the frontend actually embedded, at 9,778,153 bytes (both measured on
# the same Windows ARM64 VM; see docs/superpowers/plans/task-1-baseline.md).
# Both sizes clear the coarse $MinBytes floor above, so that floor alone
# would never have caught this. A floor set between the two -- well above
# the broken size, comfortably below the good one -- would have.
#
# This is deliberately a SECOND, independent signal alongside the marker
# check in 6b below, not the primary gate: installer size drifts with
# unrelated changes (a new vendored dependency, new plugin sources) in ways
# a marker string does not, so a genuine size increase over time is
# expected and should not need to keep raising this floor. Treat a future
# failure here as reason to re-check with the marker test first, not to
# reflexively raise the floor. Applies to the NSIS setup.exe only -- it is
# the artifact this size was actually measured against; the MSI's own
# baseline size was not.
$FrontendEmbeddedSizeFloor = 8MB
if ($Setup.Length -lt $FrontendEmbeddedSizeFloor) {
    throw "$($Setup.Name) is only $($Setup.Length) bytes, under the $FrontendEmbeddedSizeFloor floor set between the known-good (9,778,153 bytes) and known-broken (5,968,724 bytes) sizes for the missing-frontend regression  -  treating as a failed build"
}

# --- 6b. Verify the frontend was actually embedded -------------------------
# The regression this guards against shipped once already: an ABSOLUTE
# build.frontendDist override (see the long comment in step 5a above) made
# Tauri skip embedding the frontend into the binary entirely, and nothing
# else in this script would have noticed. cargo tauri build still exits 0,
# both installers still land on disk, and both are still comfortably over
# the $MinBytes floor above (NSIS/WiX boilerplate plus the WebView2 loader
# is itself several MB) -- the missing frontend is only a few MB out of
# tens, which is why 6a's floor is needed in addition to $MinBytes, and
# why this marker check exists as the real test: a size floor alone cannot
# tell "frontend embedded" from "frontend missing" as reliably as checking
# for actual frontend content.
#
# The check: read the compiled termlab.exe -- BEFORE NSIS/WiX compress it
# into an installer, since NSIS's LZMA and the MSI's cabinet compression
# would hide a plain-text marker from a raw byte search of the installer
# itself -- and look for a string that only exists in the frontend source.
#
# Marker choice, and why it is a file PATH rather than JS source text:
# termlab_tauri's `tauri = { version = "2", features = [] }` in Cargo.toml
# does NOT set `default-features = false`, so tauri's default feature set
# -- which includes "compression" (see tauri 2.10.3's own Cargo.toml
# [features] default list) -- is still active. tauri-codegen therefore
# brotli-compresses every embedded FILE'S CONTENTS at build time
# (crates/tauri-codegen-2.5.5/src/embedded_assets.rs, compress_file, gated
# on `cfg(feature = "compression")`), so a plain-text identifier from
# inside a .js file's body (e.g. a variable name) does NOT survive into
# termlab.exe as a raw substring -- this was verified directly: brotli-
# compressing crates/termlab_tauri/frontend/app/features/editor/
# open-path-routing.js at quality 2, 9, and 11 never reproduces the
# variable name `termlabOpenPathRouting` in the compressed bytes.
#
# What DOES survive uncompressed is each embedded asset's KEY -- its
# repo-relative path, e.g. "app/features/editor/open-path-routing.js" --
# because tauri-codegen's `ToTokens` impl for `EmbeddedAssets` (same file,
# `to_tokens`) emits the key as a literal `&str` in the generated
# `phf::phf_map! { #key => ... }` source, with no `cfg(feature =
# "compression")` guard anywhere near that emission. `phf::Map` (phf
# 0.11.3's src/map.rs) stores `entries: &'static [(K, V)]`, i.e. the actual
# key strings, not just a hash of them, so this literal path string is
# live, referenced data in the final binary, not a compile-time-only
# artifact rustc could discard. Only the bytes AFTER that key (the
# brotli-compressed file body, reached via `include_bytes!`) are opaque.
#
# 'features/editor/open-path-routing.js' is used rather than the bare
# filename or the full "app/..." key so the check is immune to any leading
# "/" or "app/" prefix-formatting variance in how AssetKey normalizes
# paths, while still being long enough to be distinctive: an
# application-specific directory+file path with no reason to occur by
# coincidence in Tauri/WRY/WebView2 runtime strings or unrelated Rust code
# compiled into the same binary. The underlying file is loaded by
# frontend/index.html as a plain <script src="app/features/editor/
# open-path-routing.js"> tag, and nothing in this repo's build pipeline
# (npm run build:vendor only touches vendored dependencies like CodeMirror)
# renames or relocates first-party app files, so this path reaches the
# embedded asset key unchanged. If this file is ever renamed or moved,
# update the marker here in the same change, per this repo's rule that
# plugin/frontend-surface changes and their checks move together.
$FrontendMarker = 'features/editor/open-path-routing.js'
$TermlabExe = Join-Path $RepoRoot 'target\release\termlab.exe'
if (-not (Test-Path $TermlabExe)) {
    throw "expected compiled binary not found at $TermlabExe  -  cannot verify the frontend was embedded"
}
# Read raw bytes and decode as ISO-8859-1 (Latin-1), which maps every byte
# value 0-255 to exactly one char with no substitution or multi-byte
# decoding -- unlike UTF-8, it can never throw or lossily collapse bytes
# that are not valid text, which matters because most of this file is
# compiled machine code, not text. An ASCII marker's bytes survive that
# round-trip completely unchanged, so a plain substring search on the
# resulting string finds it wherever it sits inside the binary.
$ExeBytes = [System.IO.File]::ReadAllBytes($TermlabExe)
$ExeText  = [System.Text.Encoding]::GetEncoding('ISO-8859-1').GetString($ExeBytes)
if (-not $ExeText.Contains($FrontendMarker)) {
    throw "termlab.exe at $TermlabExe does not contain the frontend marker '$FrontendMarker'  -  the frontend assets were not embedded. Check that build.frontendDist is not being overridden in either --config payload above: this script relies on the stock tauri.conf.json frontendDist ('frontend'), since overriding it at all risks the FrontendDist::Url mis-parse described in step 5a, which builds and bundles cleanly but ships an app that opens a directory listing instead of the UI."
}
Write-Host "==> Frontend marker '$FrontendMarker' found in $TermlabExe  -  frontend assets are embedded" -ForegroundColor Green

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
