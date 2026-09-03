#Requires -Version 5.1
<#
.SYNOPSIS
    Installs both Windows TermLab installers, asserts each one registered
    itself with Windows correctly, then uninstalls and asserts it cleaned up.

.DESCRIPTION
    Run on Windows after scripts/build-windows.ps1 has produced
    dist/*-setup.exe (and, when the workspace version allows it, dist/*.msi).
    This is committed rather than performed ad hoc so the check is repeatable
    on every release.

    PRIMARY TARGET: the NSIS *-setup.exe. It is where the Explorer
    context-menu hook lives (packaging/windows/installer-hooks.nsh), it is
    the artifact the auto-updater ships, and it is the only installer format
    that currently accepts this workspace's pre-release version string --
    MSI's ProductVersion must be numeric-only, and this workspace is at
    3.0.0-rc.2 (see docs/superpowers/plans/task-1-baseline.md).

    SECONDARY TARGET: the MSI, run through the identical checks, only if one
    exists in dist/. Its absence is expected at a pre-release version and is
    reported, not treated as a failure.

    Both passes share one assertion contract (Test-Registration / Test-Launch
    / Test-Cleanup below), because packaging/windows/registration.wxs and
    packaging/windows/installer-hooks.nsh are required to declare identical
    HKCU keys -- enforced by
    crates/termlab_tauri/tests/windows_registration_parity.rs. Only the
    install/uninstall *mechanics* (msiexec vs. the NSIS setup.exe/uninstall.exe)
    differ between the two passes; everything asserted about the resulting
    Windows state is the same function call for both.

    A failed *assertion* (a missing key, a wrong label, a leftover value) is
    recorded and the script keeps going, so one run reports every mismatch
    instead of stopping at the first. A failed *installer invocation* itself
    (msiexec / setup.exe / uninstall.exe returning a non-zero exit code) is
    treated as fatal instead: if the tool never actually installed or
    uninstalled anything, running dozens of registration checks against that
    would just produce a wall of confusing, symptomatic failures instead of
    one clear root cause.

.PARAMETER SetupExePath
    The NSIS installer to verify. Defaults to the newest *-setup.exe in dist/.

.PARAMETER MsiPath
    The MSI to verify. Defaults to the newest *.msi in dist/, if any. Pass an
    empty string explicitly to force-skip the MSI pass even if one exists.

.EXAMPLE
    pwsh scripts/tests/verify-windows-install.ps1

.EXAMPLE
    pwsh scripts/tests/verify-windows-install.ps1 -SetupExePath dist\TermLab_3.0.0-rc.2_arm64-setup.exe

.EXAMPLE
    pwsh scripts/tests/verify-windows-install.ps1 -MsiPath ''
    # Force-skip the MSI pass even if dist/*.msi exists.

.NOTES
    Measured, not guessed. The constants and registry keys below come from a
    real install performed on the project's Windows ARM64 VM plus a direct
    reading of packaging/windows/registration.wxs and installer-hooks.nsh
    (see docs/superpowers/plans/task-1-baseline.md). In particular:
      - The installed executable is lowercase termlab.exe, NOT TermLab.exe.
      - The install directory is %LOCALAPPDATA%\TermLab, NOT under
        Program Files or %LOCALAPPDATA%\Programs.
      - The uninstall registry entry's Publisher value is lowercase
        "termlab", not "TermLab" or "an0nn30".
      - The VM that produced these facts is ARM64, so its own artifacts are
        named *_arm64-*; CI produces *_x64-*. Nothing in this script
        hardcodes an architecture -- installers are located by glob, and
        every path used for assertions is read back out of the registry
        rather than assumed.
    This script has not been run: the Windows VM used for prior tasks is
    offline (confirmed 100% packet loss, no route to host) and this machine
    has no pwsh to even syntax-check it against. It was written and manually
    re-read for quoting/null-handling instead. Run it for real the moment the
    VM is back -- see the task-6 report for a short checklist.
#>
[CmdletBinding()]
param(
    [string]$SetupExePath,
    [string]$MsiPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# =============================================================================
# The Windows registration contract.
#
# Exactly the keys packaging/windows/registration.wxs (MSI) and
# packaging/windows/installer-hooks.nsh (NSIS) write, both entirely under
# HKCU -- the install is per-user and takes no UAC prompt, so both installers
# write these specific keys under HKCU unconditionally regardless of overall
# MSI install scope.
# =============================================================================
$ContextMenuRoots = @(
    'Software\Classes\Directory\shell\TermLab',
    'Software\Classes\Directory\Background\shell\TermLab',
    'Software\Classes\Drive\shell\TermLab'
)
$OtherKeys = @(
    'Software\Clients\Terminal\TermLab\Capabilities',
    'Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe'
)
$AllCustomKeys = $ContextMenuRoots + $OtherKeys
$RegisteredAppsKey = 'Software\RegisteredApplications'

# The standard per-user "Programs and Features" uninstall entry. This is
# written by the bundler itself (NSIS/WiX boilerplate), not by our hooks.
# task-1-baseline.md measured it under HKCU for the NSIS install; the MSI's
# own uninstall-entry hive was never measured (Task 1 never got a working MSI
# build), so the MSI pass also accepts HKLM here without failing -- only
# "found in neither hive" is a failure.
$UninstallSubKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\TermLab'

# =============================================================================
# Small helpers
# =============================================================================
$Failures = New-Object System.Collections.Generic.List[string]

function Assert-True {
    param([Parameter(Mandatory)][bool]$Condition, [Parameter(Mandatory)][string]$Message)
    if ($Condition) {
        Write-Host "  PASS  $Message" -ForegroundColor Green
    } else {
        Write-Host "  FAIL  $Message" -ForegroundColor Red
        $script:Failures.Add($Message)
    }
}

# Reads one named registry value safely. If the value does not exist,
# Get-ItemProperty -Name emits no object at all (suppressed here), so this
# returns plain $null instead of erroring on a missing property -- important
# under Set-StrictMode -Version Latest, which would otherwise turn "this
# value happens to be absent" into a script-ending error instead of a
# reportable failed assertion.
function Get-RegValue {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Name)
    return (Get-ItemProperty -Path $Path -Name $Name -ErrorAction SilentlyContinue).$Name
}

# Extracts the leading quoted path out of a command-line-shaped value, e.g.
# '"C:\...\termlab.exe" --working-directory "%V"' or an UninstallString like
# '"C:\...\uninstall.exe"'. Returns $null if the value is not a quoted path.
function Get-QuotedLeadingPath {
    param([string]$Value)
    if ($Value -and $Value -match '^"([^"]+)"') {
        return $Matches[1]
    }
    return $null
}

# Strips one pair of wrapping double quotes from a whole-value path, if
# present (some registry values, like InstallLocation, are not command
# lines and may or may not be quoted).
function Get-UnquotedPath {
    param([string]$Value)
    if ($Value -and $Value.Length -ge 2 -and $Value.StartsWith('"') -and $Value.EndsWith('"')) {
        return $Value.Substring(1, $Value.Length - 2)
    }
    return $Value
}

# DisplayIcon values are commonly "C:\...\termlab.exe,0" (an optional
# trailing ",<icon index>") and may or may not be quoted. Strips both so the
# result is a plain file path suitable for Test-Path.
function Get-IconPath {
    param([string]$Value)
    if (-not $Value) { return $null }
    $v = Get-UnquotedPath $Value
    $v = $v -replace ',\d+$', ''
    return $v
}

function Get-UninstallEntryPath {
    if (Test-Path "HKCU:\$UninstallSubKey") { return "HKCU:\$UninstallSubKey" }
    if (Test-Path "HKLM:\$UninstallSubKey") { return "HKLM:\$UninstallSubKey" }
    return $null
}

# Bounded local polling. An NSIS uninstaller commonly re-launches itself from
# a %TEMP% copy so it can delete its own install directory and exe, which
# means the *original* uninstall.exe process (the one Start-Process -Wait
# waits on) can exit before that background copy has actually finished
# deleting keys/files. These poll local registry state only, for at most
# $TimeoutSeconds -- this is unrelated to, and does not reintroduce, the
# instruction to never poll the (offline, network) VM host.
function Wait-UntilAbsent {
    param([Parameter(Mandatory)][string]$Path, [int]$TimeoutSeconds = 30)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        if (-not (Test-Path $Path)) { return $true }
        Start-Sleep -Seconds 1
    } while ((Get-Date) -lt $deadline)
    return (-not (Test-Path $Path))
}

function Wait-UntilRegValueAbsent {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Name, [int]$TimeoutSeconds = 30)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        if ($null -eq (Get-RegValue -Path $Path -Name $Name)) { return $true }
        Start-Sleep -Seconds 1
    } while ((Get-Date) -lt $deadline)
    return ($null -eq (Get-RegValue -Path $Path -Name $Name))
}

function Wait-UntilUninstallEntryAbsent {
    param([int]$TimeoutSeconds = 30)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        if (-not (Test-Path "HKCU:\$UninstallSubKey") -and -not (Test-Path "HKLM:\$UninstallSubKey")) { return $true }
        Start-Sleep -Seconds 1
    } while ((Get-Date) -lt $deadline)
    return (-not (Test-Path "HKCU:\$UninstallSubKey") -and -not (Test-Path "HKLM:\$UninstallSubKey"))
}

# =============================================================================
# Shared assertions -- identical for the NSIS pass and the MSI pass.
# =============================================================================
function Test-Registration {
    param([Parameter(Mandatory)][string]$PassName)

    foreach ($root in $ContextMenuRoots) {
        $key = "HKCU:\$root"
        Assert-True (Test-Path $key) "[$PassName] $root exists"
        if (Test-Path $key) {
            $label = Get-RegValue -Path $key -Name 'MUIVerb'
            Assert-True ($label -eq 'Open TermLab here') "[$PassName] $root MUIVerb is 'Open TermLab here' (got '$label')"
        }

        $cmdKey = "HKCU:\$root\command"
        Assert-True (Test-Path $cmdKey) "[$PassName] $root\command exists"
        if (Test-Path $cmdKey) {
            $cmd = Get-RegValue -Path $cmdKey -Name '(default)'
            Assert-True ($cmd -like '*--working-directory*') "[$PassName] $root command passes --working-directory (got '$cmd')"
            Assert-True ($cmd -like '*%V*') "[$PassName] $root command uses capital %V (got '$cmd')"

            # This is the assertion that catches a wrong install path or a
            # renamed binary: the command must point at a file that actually
            # exists on disk right now, not just look plausible as a string.
            $exe = Get-QuotedLeadingPath $cmd
            if ($exe) {
                Assert-True (Test-Path $exe) "[$PassName] $root command target exists on disk: $exe"
            } else {
                Assert-True $false "[$PassName] $root command is a quoted path (got '$cmd')"
            }
        }
    }

    foreach ($key in $OtherKeys) {
        Assert-True (Test-Path "HKCU:\$key") "[$PassName] $key exists"
    }

    $registered = Get-RegValue -Path "HKCU:\$RegisteredAppsKey" -Name 'TermLab'
    Assert-True ($registered -eq 'Software\Clients\Terminal\TermLab\Capabilities') `
        "[$PassName] $RegisteredAppsKey\TermLab points at the Capabilities key (got '$registered')"

    $entryPath = Get-UninstallEntryPath
    if (-not $entryPath) {
        Assert-True $false "[$PassName] $UninstallSubKey exists (checked HKCU and HKLM)"
    } else {
        $hive = ($entryPath -split ':')[0]
        Write-Host "  ($PassName uninstall entry found under $hive)"

        $installLocation = Get-UnquotedPath (Get-RegValue -Path $entryPath -Name 'InstallLocation')
        $displayIcon     = Get-RegValue -Path $entryPath -Name 'DisplayIcon'
        $iconPath        = Get-IconPath $displayIcon
        $uninstallString = Get-RegValue -Path $entryPath -Name 'UninstallString'
        $displayVersion  = Get-RegValue -Path $entryPath -Name 'DisplayVersion'
        $publisher       = Get-RegValue -Path $entryPath -Name 'Publisher'

        Assert-True ([bool]$installLocation -and (Test-Path $installLocation)) `
            "[$PassName] uninstall entry InstallLocation exists on disk (got '$installLocation')"
        Assert-True ([bool]$iconPath -and (Test-Path $iconPath)) `
            "[$PassName] uninstall entry DisplayIcon target exists on disk (got '$displayIcon')"
        Assert-True ([bool]$uninstallString) `
            "[$PassName] uninstall entry has an UninstallString (got '$uninstallString')"
        Assert-True ([bool]$displayVersion) `
            "[$PassName] uninstall entry has a DisplayVersion (got '$displayVersion')"
        Assert-True ($publisher -eq 'termlab') `
            "[$PassName] uninstall entry Publisher is 'termlab' (got '$publisher')"
    }
}

function Test-Launch {
    param([Parameter(Mandatory)][string]$PassName)

    $appPath = Get-RegValue -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe' -Name '(default)'
    if (-not $appPath -or -not (Test-Path $appPath)) {
        Assert-True $false "[$PassName] App Paths entry points at a real file (got '$appPath')"
        return
    }

    # NOTE for future readers: if this ever runs over an SSH-spawned
    # PowerShell session, that session is Windows session 0 while the real
    # interactive desktop is session 1 (recorded during Task 1 on the project
    # VM). A GUI app launched from session 0 is a known confounder there -- it
    # may behave abnormally even though the process itself is alive -- so
    # this only asserts the process survives, not that a window painted. If
    # this check ever fails with a clean, immediate exit code, suspect the
    # --working-directory wiring (crates/termlab_tauri, Tasks 2/3) before
    # suspecting the installer.
    $proc = Start-Process -FilePath $appPath -ArgumentList '--working-directory', 'C:\Windows' -PassThru
    Start-Sleep -Seconds 8
    $alive = -not $proc.HasExited
    if ($alive) {
        Assert-True $true "[$PassName] termlab --working-directory C:\Windows is still running after 8s"
        try {
            $proc.Kill()
            $proc.WaitForExit()
        } catch {
            Write-Host "  (warning: could not stop the launched termlab.exe process: $($_.Exception.Message))" -ForegroundColor Yellow
        }
    } else {
        Assert-True $false "[$PassName] termlab --working-directory C:\Windows is still running after 8s (it exited immediately with code $($proc.ExitCode))"
    }
}

function Test-Cleanup {
    param([Parameter(Mandatory)][string]$PassName)

    foreach ($root in $AllCustomKeys) {
        $key = "HKCU:\$root"
        $gone = Wait-UntilAbsent -Path $key -TimeoutSeconds 30
        Assert-True $gone "[$PassName] $root removed on uninstall"
    }

    $valueGone = Wait-UntilRegValueAbsent -Path "HKCU:\$RegisteredAppsKey" -Name 'TermLab' -TimeoutSeconds 30
    Assert-True $valueGone "[$PassName] $RegisteredAppsKey\TermLab value removed on uninstall"
    Assert-True (Test-Path "HKCU:\$RegisteredAppsKey") `
        "[$PassName] $RegisteredAppsKey key itself still exists (it is shared with every other installed app, so it must survive)"

    $entryGone = Wait-UntilUninstallEntryAbsent -TimeoutSeconds 30
    Assert-True $entryGone "[$PassName] $UninstallSubKey removed on uninstall (checked both hives)"
}

# =============================================================================
# Per-installer mechanics. Exit-code failures here are fatal (throw): if the
# tool itself did not install/uninstall, the registration checks that follow
# would just produce a cascade of symptomatic failures instead of one root
# cause.
# =============================================================================
function Invoke-NsisInstall {
    param([Parameter(Mandatory)][string]$Path)
    Write-Host "  running: `"$Path`" /S"
    $p = Start-Process -FilePath $Path -ArgumentList '/S' -PassThru -Wait
    if ($p.ExitCode -ne 0) { throw "NSIS installer '$Path' /S exited with code $($p.ExitCode)" }
}

function Invoke-NsisUninstall {
    $entryPath = Get-UninstallEntryPath
    $exePath = $null
    if ($entryPath) {
        $uninstallString = Get-RegValue -Path $entryPath -Name 'UninstallString'
        $exePath = Get-QuotedLeadingPath $uninstallString
        if (-not $exePath) { $exePath = Get-UnquotedPath $uninstallString }
    }
    if (-not $exePath -or -not (Test-Path $exePath)) {
        throw "could not resolve a valid uninstall.exe path from the registry UninstallString (got '$exePath') -- cannot proceed with the NSIS uninstall"
    }
    Write-Host "  running: `"$exePath`" /S"
    $p = Start-Process -FilePath $exePath -ArgumentList '/S' -PassThru -Wait
    if ($p.ExitCode -ne 0) { throw "NSIS uninstaller '$exePath' /S exited with code $($p.ExitCode)" }
}

function Invoke-Msi {
    param([Parameter(Mandatory)][string]$Verb, [Parameter(Mandatory)][string]$Path)
    $argString = "$Verb `"$Path`" /qn /norestart"
    Write-Host "  running: msiexec $argString"
    return Start-Process -FilePath 'msiexec.exe' -ArgumentList $argString -PassThru -Wait
}

function Invoke-MsiInstall {
    param([Parameter(Mandatory)][string]$Path)
    $p = Invoke-Msi -Verb '/i' -Path $Path
    if ($p.ExitCode -ne 0) { throw "msiexec /i '$Path' exited with code $($p.ExitCode)" }
}

function Invoke-MsiUninstall {
    param([Parameter(Mandatory)][string]$Path)
    $p = Invoke-Msi -Verb '/x' -Path $Path
    if ($p.ExitCode -ne 0) { throw "msiexec /x '$Path' exited with code $($p.ExitCode)" }
}

function Invoke-VerificationPass {
    param(
        [Parameter(Mandatory)][string]$PassName,
        [Parameter(Mandatory)][scriptblock]$InstallAction,
        [Parameter(Mandatory)][scriptblock]$UninstallAction
    )

    Write-Host ''
    Write-Host "===== $PassName =====" -ForegroundColor Magenta

    Write-Host "`n-- Install --" -ForegroundColor Cyan
    & $InstallAction

    Write-Host "`n-- Registration --" -ForegroundColor Cyan
    Test-Registration -PassName $PassName

    Write-Host "`n-- Launch --" -ForegroundColor Cyan
    Test-Launch -PassName $PassName

    Write-Host "`n-- Uninstall --" -ForegroundColor Cyan
    & $UninstallAction

    Write-Host "`n-- Cleanup --" -ForegroundColor Cyan
    Test-Cleanup -PassName $PassName
}

# =============================================================================
# Locate installers.
# =============================================================================
$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$DistDir  = Join-Path $RepoRoot 'dist'

if (-not $SetupExePath) {
    $candidate = Get-ChildItem -Path $DistDir -Filter '*-setup.exe' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $candidate) {
        throw "no *-setup.exe found in $DistDir. Run scripts\build-windows.ps1 first."
    }
    $SetupExePath = $candidate.FullName
}
if (-not (Test-Path $SetupExePath)) {
    throw "NSIS installer not found at $SetupExePath"
}

$SkipMsi = $false
if (-not $PSBoundParameters.ContainsKey('MsiPath')) {
    $candidate = Get-ChildItem -Path $DistDir -Filter '*.msi' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($candidate) {
        $MsiPath = $candidate.FullName
    } else {
        $SkipMsi = $true
    }
} elseif ([string]::IsNullOrEmpty($MsiPath)) {
    $SkipMsi = $true
} elseif (-not (Test-Path $MsiPath)) {
    throw "MSI not found at $MsiPath"
}

Write-Host "NSIS installer: $SetupExePath"
if ($SkipMsi) {
    Write-Host "MSI: none found in $DistDir -- skipping the MSI pass. This is expected at workspace version 3.0.0-rc.2: MSI ProductVersion cannot encode a pre-release identifier, so scripts/build-windows.ps1 may not have produced one. Not a failure." -ForegroundColor Yellow
} else {
    Write-Host "MSI installer: $MsiPath"
}

# =============================================================================
# Main
# =============================================================================
try {
    Invoke-VerificationPass -PassName 'NSIS' `
        -InstallAction { Invoke-NsisInstall -Path $SetupExePath } `
        -UninstallAction { Invoke-NsisUninstall }

    if ($SkipMsi) {
        Write-Host ''
        Write-Host '===== MSI =====' -ForegroundColor Magenta
        Write-Host "skipped: no MSI in $DistDir." -ForegroundColor Yellow
    } else {
        Invoke-VerificationPass -PassName 'MSI' `
            -InstallAction { Invoke-MsiInstall -Path $MsiPath } `
            -UninstallAction { Invoke-MsiUninstall -Path $MsiPath }
    }
} catch {
    Write-Host ''
    Write-Host "FATAL: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}

# =============================================================================
# Result
# =============================================================================
Write-Host ''
if ($Failures.Count -gt 0) {
    Write-Host "$($Failures.Count) check(s) failed:" -ForegroundColor Red
    $Failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}
Write-Host 'All install verification checks passed.' -ForegroundColor Green
exit 0
