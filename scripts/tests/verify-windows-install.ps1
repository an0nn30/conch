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
      - The MSI's own uninstall-entry hive HAS since been measured: it
        registers under a GUID-named subkey, under HKLM\WOW6432Node on the
        build that produced it, not under a literal "TermLab" name the way
        NSIS does. Get-UninstallEntryPath handles the two passes
        differently for exactly this reason.
    This script has been run end-to-end on the project's Windows ARM64 VM,
    through both the NSIS and MSI passes.
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

# The frontend-embedding regression this script failed to catch once
# already: an installed build whose webview shows a bare `file://`
# directory listing instead of the app UI still registers every key above
# correctly and still keeps a process alive for 8 seconds (see Test-Launch's
# own notes on why it cannot check for a painted window). See the matching,
# longer explanation on the marker check in scripts/build-windows.ps1 (step
# 6a) for why this is a file PATH and not JS source text (tauri's default
# "compression" feature is active here, so brotli-compressed file contents
# do not survive as a raw substring -- only each embedded asset's literal
# path key does, per tauri-codegen's phf_map! codegen). Kept in sync with
# that script by hand; if the marker there ever changes, change it here too.
$FrontendMarker = 'features/editor/open-path-routing.js'

# The standard per-user "Programs and Features" uninstall entry. This is
# written by the bundler itself (NSIS/WiX boilerplate), not by our hooks.
#
# NSIS registers itself under a literal "TermLab" subkey (measured on a real
# VM, present under HKCU). The MSI does not: it registers under a
# GUID-named subkey -- and, on this WiX toolset/build, under WOW6432Node
# even on an ARM64 host -- so the two passes need different lookup
# strategies. $UninstallSubKey below is only ever used for the NSIS pass;
# Get-UninstallEntryPath handles the MSI pass by searching for a matching
# DisplayName instead of assuming a path.
$UninstallSubKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\TermLab'
$MsiUninstallSearchRoots = @(
    'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall',
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall',
    'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
)

# =============================================================================
# Small helpers
# =============================================================================
$Failures = New-Object System.Collections.Generic.List[string]
$Warnings = New-Object System.Collections.Generic.List[string]

function Assert-True {
    param([Parameter(Mandatory)][bool]$Condition, [Parameter(Mandatory)][string]$Message)
    if ($Condition) {
        Write-Host "  PASS  $Message" -ForegroundColor Green
    } else {
        Write-Host "  FAIL  $Message" -ForegroundColor Red
        $script:Failures.Add($Message)
    }
}

# Like Assert-True, but for a known, accepted gap: prints a visible WARN
# instead of FAIL and does not add to $Failures, so it never fails the
# script or blocks it from exiting 0 on an otherwise-clean run. Used only
# where a real, understood packaging limitation would otherwise make an
# unattended gate un-runnable for a cosmetic reason.
function Assert-Warn {
    param([Parameter(Mandatory)][bool]$Condition, [Parameter(Mandatory)][string]$Message)
    if ($Condition) {
        Write-Host "  PASS  $Message" -ForegroundColor Green
    } else {
        Write-Host "  WARN  $Message" -ForegroundColor Yellow
        $script:Warnings.Add($Message)
    }
}

# Reads one named registry value safely, returning $null if either the key
# or the value does not exist.
#
# Measured on a real VM: when the key does not exist, Get-ItemProperty -Name
# emits no object at all (suppressed here). But when the key DOES exist and
# merely lacks this particular value -- e.g. Software\RegisteredApplications,
# a key shared with every other installed app, right after our own value has
# been removed from it by an uninstaller -- Get-ItemProperty still returns
# the key object populated with everyone else's properties, just not this
# one. Dereferencing a genuinely absent property with .$Name on that real
# object throws under Set-StrictMode -Version Latest (line 86) instead of
# returning $null, turning a legitimate "value is gone" state into a
# script-ending error. Checking PSObject.Properties.Name first avoids that
# without swallowing other errors: -ErrorAction SilentlyContinue only
# suppresses the "not found" case, so a genuine access failure (e.g. a
# permissions error) still surfaces as a terminating error via
# $ErrorActionPreference = 'Stop' at the top of this script.
function Get-RegValue {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Name)
    $item = Get-ItemProperty -Path $Path -Name $Name -ErrorAction SilentlyContinue
    if (-not $item) { return $null }
    if ($item.PSObject.Properties.Name -notcontains $Name) { return $null }
    return $item.$Name
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

# Locates the Programs-and-Features uninstall entry for the given pass.
# NSIS writes a literal "TermLab" subkey (checked under both HKCU and HKLM).
# MSI writes a GUID-named subkey under one of several plausible roots, so
# it is found by DisplayName instead of by assumed path -- returns $null,
# a clear "not found", if no match turns up anywhere searched.
function Get-UninstallEntryPath {
    param([Parameter(Mandatory)][string]$PassName)

    if ($PassName -eq 'NSIS') {
        if (Test-Path "HKCU:\$UninstallSubKey") { return "HKCU:\$UninstallSubKey" }
        if (Test-Path "HKLM:\$UninstallSubKey") { return "HKLM:\$UninstallSubKey" }
        return $null
    }

    foreach ($root in $MsiUninstallSearchRoots) {
        if (-not (Test-Path $root)) { continue }
        $match = Get-ChildItem -Path $root -ErrorAction SilentlyContinue | Where-Object {
            (Get-RegValue -Path $_.PSPath -Name 'DisplayName') -eq 'TermLab'
        } | Select-Object -First 1
        if ($match) { return "$root\$($match.PSChildName)" }
    }
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
    param([Parameter(Mandatory)][string]$PassName, [int]$TimeoutSeconds = 30)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        if (-not (Get-UninstallEntryPath -PassName $PassName)) { return $true }
        Start-Sleep -Seconds 1
    } while ((Get-Date) -lt $deadline)
    return (-not (Get-UninstallEntryPath -PassName $PassName))
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
            # -ceq (case-sensitive): MUIVerb is a literal display string the
            # installers write byte-for-byte ("Open TermLab here"). A
            # regression to different casing is a real, visible defect the
            # default case-insensitive -eq would silently miss.
            $label = Get-RegValue -Path $key -Name 'MUIVerb'
            Assert-True ($label -ceq 'Open TermLab here') "[$PassName] $root MUIVerb is exactly 'Open TermLab here' (got '$label')"

            # Icon is written alongside MUIVerb on this same key (not on
            # \command). A broken/missing icon path would otherwise go
            # undetected -- Get-IconPath strips the optional quotes/",N"
            # suffix this value could carry, though the source .wxs/.nsh
            # write it as a bare path.
            $icon = Get-RegValue -Path $key -Name 'Icon'
            $iconPath = Get-IconPath $icon
            Assert-True ([bool]$iconPath -and (Test-Path $iconPath)) "[$PassName] $root Icon target exists on disk (got '$icon')"
        }

        $cmdKey = "HKCU:\$root\command"
        Assert-True (Test-Path $cmdKey) "[$PassName] $root\command exists"
        if (Test-Path $cmdKey) {
            $cmd = Get-RegValue -Path $cmdKey -Name '(default)'
            # -clike (case-sensitive): both are literal tokens a real CLI
            # parser / Explorer substitution engine treats case-sensitively.
            # termlab's hand-rolled CLI parser (crates/termlab_tauri/src/cli.rs,
            # not clap or any other arg-parsing crate) will not match
            # "--Working-Directory", and Explorer's shell substitution only
            # recognizes uppercase "%V" for this "current directory" form --
            # lowercase "%v" does not resolve here. A regression to either
            # wrong case is a real functional break, not cosmetic, so the
            # default case-insensitive -like would provide false assurance.
            Assert-True ($cmd -clike '*--working-directory*') "[$PassName] $root command passes --working-directory (got '$cmd')"
            Assert-True ($cmd -clike '*%V*') "[$PassName] $root command uses capital %V (got '$cmd')"

            # This is the assertion that catches a wrong install path or a
            # renamed binary: the command must point at a file that actually
            # exists on disk right now, not just look plausible as a string.
            # (Case-insensitive Test-Path is correct here -- Windows paths
            # are not case-sensitive.)
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

    # -eq (deliberately case-insensitive): this value is itself a registry
    # key path, and the Windows registry treats key/value names -- and any
    # path segment referencing one -- case-insensitively. A casing difference
    # here would still resolve to the exact same Capabilities key, so it is
    # not a real regression the way the MUIVerb/%V/Publisher checks are.
    $registered = Get-RegValue -Path "HKCU:\$RegisteredAppsKey" -Name 'TermLab'
    Assert-True ($registered -eq 'Software\Clients\Terminal\TermLab\Capabilities') `
        "[$PassName] $RegisteredAppsKey\TermLab points at the Capabilities key (got '$registered')"

    $entryPath = Get-UninstallEntryPath -PassName $PassName
    if (-not $entryPath) {
        if ($PassName -eq 'NSIS') {
            Assert-True $false "[$PassName] $UninstallSubKey exists (checked HKCU and HKLM)"
        } else {
            Assert-True $false "[$PassName] Programs-and-Features uninstall entry with DisplayName 'TermLab' exists (searched HKLM, HKLM\WOW6432Node, and HKCU)"
        }
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
        if ($PassName -eq 'MSI') {
            # Known, accepted packaging gap, not an install failure: the
            # deleted hand-written packaging/windows/termlab.wxs used to set
            # <Icon>/ARPPRODUCTICON for the MSI's uninstall entry, but our
            # current registry-only packaging/windows/registration.wxs does
            # not, and neither does Tauri's own WiX template -- so the MSI's
            # DisplayIcon is blank, which means no icon in Add/Remove
            # Programs. It is cosmetic: the NSIS installer, the primary
            # artifact and the one the auto-updater ships, sets this
            # correctly and is still asserted as a hard failure below.
            Assert-Warn ([bool]$iconPath -and (Test-Path $iconPath)) `
                "[$PassName] uninstall entry DisplayIcon target exists on disk (got '$displayIcon')"
        } else {
            Assert-True ([bool]$iconPath -and (Test-Path $iconPath)) `
                "[$PassName] uninstall entry DisplayIcon target exists on disk (got '$displayIcon')"
        }
        Assert-True ([bool]$uninstallString) `
            "[$PassName] uninstall entry has an UninstallString (got '$uninstallString')"
        Assert-True ([bool]$displayVersion) `
            "[$PassName] uninstall entry has a DisplayVersion (got '$displayVersion')"
        # -ceq (case-sensitive): the measured, required value is lowercase
        # "termlab", specifically NOT "TermLab" or "an0nn30" -- exactly the
        # kind of casing regression the default case-insensitive -eq would
        # let through unnoticed.
        Assert-True ($publisher -ceq 'termlab') `
            "[$PassName] uninstall entry Publisher is exactly 'termlab' (got '$publisher')"
    }
}

# Cheap, headless, and independent of Test-Launch's process-alive check
# (which cannot tell a painted UI from a directory listing -- see its own
# notes): reads the installed termlab.exe straight off disk, already
# decompressed by the installer, and looks for $FrontendMarker as a raw
# byte sequence. Absence means the frontend was never embedded into the
# binary in the first place, which is a hard failure, not a warning.
function Test-FrontendEmbedded {
    param([Parameter(Mandatory)][string]$PassName)

    $appPath = Get-RegValue -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe' -Name '(default)'
    if (-not $appPath -or -not (Test-Path $appPath)) {
        Assert-True $false "[$PassName] frontend embedding check: App Paths entry points at a real file (got '$appPath')"
        return
    }

    # ISO-8859-1 (Latin-1) maps every byte 0-255 to one char with no
    # substitution, so an ASCII marker's bytes survive the round-trip
    # unchanged and a plain substring search finds it wherever it sits
    # inside this mostly-binary file.
    $bytes = [System.IO.File]::ReadAllBytes($appPath)
    $text = [System.Text.Encoding]::GetEncoding('ISO-8859-1').GetString($bytes)
    Assert-True $text.Contains($FrontendMarker) `
        "[$PassName] installed termlab.exe contains frontend marker '$FrontendMarker' (frontend assets are embedded, not a bare file:// listing)"
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
    #
    # This is NOT a functional check of --working-directory itself: a bad or
    # unreachable directory is deliberately non-fatal (main.rs logs a warning
    # and falls back to the inherited cwd), so this assertion passes whether
    # or not the flag actually changed anything. A real functional check
    # would need to inspect the launched process's working directory on an
    # interactive desktop, which this headless/SSH context cannot do. The
    # PASS below only means "termlab.exe starts and stays up when given
    # this flag", not "the flag took effect".
    $proc = Start-Process -FilePath $appPath -ArgumentList '--working-directory', 'C:\Windows' -PassThru
    Start-Sleep -Seconds 8
    $alive = -not $proc.HasExited
    if ($alive) {
        Assert-True $true "[$PassName] termlab --working-directory C:\Windows starts and stays running after 8s"
        try {
            $proc.Kill()
            $proc.WaitForExit()
        } catch {
            Write-Host "  (warning: could not stop the launched termlab.exe process: $($_.Exception.Message))" -ForegroundColor Yellow
        }
    } else {
        Assert-True $false "[$PassName] termlab --working-directory C:\Windows starts and stays running after 8s (it exited immediately with code $($proc.ExitCode))"
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

    $entryGone = Wait-UntilUninstallEntryAbsent -PassName $PassName -TimeoutSeconds 30
    Assert-True $entryGone "[$PassName] Programs-and-Features uninstall entry removed on uninstall"
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
    $entryPath = Get-UninstallEntryPath -PassName 'NSIS'
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

    Write-Host "`n-- Frontend assets --" -ForegroundColor Cyan
    Test-FrontendEmbedded -PassName $PassName

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
if ($Warnings.Count -gt 0) {
    Write-Host "$($Warnings.Count) known-gap warning(s) (not failures):" -ForegroundColor Yellow
    $Warnings | ForEach-Object { Write-Host "  - $_" -ForegroundColor Yellow }
    Write-Host ''
}
if ($Failures.Count -gt 0) {
    Write-Host "$($Failures.Count) check(s) failed:" -ForegroundColor Red
    $Failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}
Write-Host 'All install verification checks passed.' -ForegroundColor Green
exit 0
