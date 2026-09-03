# Task 1 baseline — verified on the VM, 2026-09-03

VM: `dustin@192.168.1.125`, Windows 11 build 26200, **ARM64** (`aarch64-pc-windows-msvc`
host, `rustc 1.97.1`). This is a load-bearing fact — see "Surprising findings" below.
Repo path: `C:\Users\dustin\conch`. Synced to commit `00e99a4` (branch
`feat/windows-installer-registration`, pushed to origin during this task since it was
2 commits ahead of origin at the start).

## The three required constants

- **`INSTALLED_EXE_NAME` = `termlab.exe`** (lowercase — **not** `TermLab.exe`).
  Verified from a real NSIS install: `C:\Users\dustin\AppData\Local\TermLab\termlab.exe`
  exists; `TermLab.exe` (capitalized) does not exist anywhere under the install dir.
  The registry `DisplayIcon` value for the uninstall entry also points at
  `"C:\Users\dustin\AppData\Local\TermLab\termlab.exe"`.

  Tauri v2 does **not** rename the compiled binary to `mainBinaryName`/`productName`
  for this app. `tauri.conf.json` sets no explicit `mainBinaryName`, and the build log
  itself says `Built application at: C:\Users\dustin\conch\target\release\termlab.exe`
  — matching the Cargo `[[bin]] name = "termlab"`, not `productName: "TermLab"`.

- **`INSTALL_DIR` = `%LOCALAPPDATA%\TermLab`** (i.e.
  `C:\Users\dustin\AppData\Local\TermLab`) — **not**
  `%LOCALAPPDATA%\Programs\TermLab` as the brief guessed. Verified directory listing
  after a real silent NSIS install:

  ```
  C:\Users\dustin\AppData\Local\TermLab\themes
  C:\Users\dustin\AppData\Local\TermLab\termlab.exe
  C:\Users\dustin\AppData\Local\TermLab\uninstall.exe
  C:\Users\dustin\AppData\Local\TermLab\themes\TermLab Dark.toml
  C:\Users\dustin\AppData\Local\TermLab\themes\TermLab Light.toml
  ```

  Confirmed independently via the HKCU uninstall registry entry:
  ```
  KeyName: TermLab   (HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\TermLab)
  DisplayName     : TermLab
  InstallLocation : "C:\Users\dustin\AppData\Local\TermLab"
  DisplayIcon     : "C:\Users\dustin\AppData\Local\TermLab\termlab.exe"
  UninstallString : "C:\Users\dustin\AppData\Local\TermLab\uninstall.exe"
  DisplayVersion  : 3.0.0-rc.2
  Publisher       : termlab
  ```
  The uninstall entry lives under `HKCU` (not `HKLM`), consistent with
  `installMode: currentUser`. A per-user Start Menu shortcut was also created at
  `%APPDATA%\Microsoft\Windows\Start Menu\Programs\TermLab.lnk`. No entries or files
  were created under either `Program Files` path.

- **`MSI_VERSION_OK` = no.** Running the stock `cargo tauri build --config
  bundle-on.json` (`{"bundle":{"active":true}}`, default targets = all) compiled
  successfully in release mode, then failed during MSI packaging with:

  ```
  Info Verifying wix package
  Downloading https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip
  Info validating hash
  Info extracting WIX
  Error failed to bundle project: `optional pre-release identifier in app version must be numeric-only and cannot be greater than 65535 for msi target`
  ```

  No `bundle` directory was produced at all — the overall build command exits 1 and
  the whole run aborts (it does not fall through to attempt NSIS afterward). Task 5
  needs an explicit numeric-version step (e.g. compute a numeric `ProductVersion`
  override, or bundle only `["nsis"]` and skip MSI) if MSI packaging is wanted while
  the workspace version stays `3.0.0-rc.2`.

  By contrast, **NSIS accepts the same pre-release version string outright** —
  `DisplayVersion: 3.0.0-rc.2` shows up verbatim in the uninstall registry entry, and
  bundling succeeded producing `TermLab_3.0.0-rc.2_arm64-setup.exe`. If Task 5 only
  needs the NSIS installer (which is where the Explorer context-menu hook lives per
  the overall plan), the version problem may not need solving at all.

## Signing-key question (for Task 5)

**Answer: a local Windows bundle does not need a signing key for the installer
itself. The key is needed only because `createUpdaterArtifacts: true` is set in
`tauri.conf.json` and asks the bundler to also emit a signed `.sig` update
manifest alongside the installer.**

Evidence, in order:

1. First real bundle attempt (`bundle.active:true`, default targets, after working
   around the `frontendDist` issue below) got past MSI's version rejection... no,
   separately: the **NSIS-only** run (`{"bundle":{"active":true,"targets":["nsis"]}}`)
   completed packaging fully —
   `Finished 1 bundle at: ...\TermLab_3.0.0-rc.2_arm64-setup.exe` — and the
   **installer file was already written to disk** — before the build then failed
   with exit code 1 on:
   ```
   Error A public key has been found, but no private key. Make sure to set `TAURI_SIGNING_PRIVATE_KEY` environment variable.
   ```
   i.e. the signing failure happens strictly *after* the installer artifact exists;
   it only blocks generation of the separate updater signature file.
2. Generated a throwaway key (`cargo tauri signer generate -w throwaway.key
   --password "throwaway-research-key-123"`), set
   `TAURI_SIGNING_PRIVATE_KEY`/`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` for that shell,
   and reran the same NSIS-only config. Build exited 0:
   ```
   Finished 1 bundle at:
       C:\Users\dustin\conch\target\release\bundle\nsis\TermLab_3.0.0-rc.2_arm64-setup.exe
       Warn The updater secret key from `TAURI_SIGNING_PRIVATE_KEY` does not match the public key from `plugins > updater > pubkey`. ...
   Finished 1 updater signature at:
       C:\Users\dustin\conch\target\release\bundle\nsis\TermLab_3.0.0-rc.2_arm64-setup.exe.sig
   ```
   The mismatch warning is expected (throwaway key vs. the real committed pubkey) and
   harmless for local verification purposes — it does not fail the build.

**Recommendation for Task 5's build script:** don't require a signing key by
default. Either (a) override `bundle.createUpdaterArtifacts: false` via the script's
own `--config` merge for local/dev builds, or (b) accept that the script's `cargo
tauri build` invocation may report a non-zero exit even though the installer was
produced, and check for the installer file's existence rather than relying solely on
the process exit code. Only ask for/require a real `TAURI_SIGNING_PRIVATE_KEY` when
the developer explicitly wants updater artifacts too.

Throwaway key material (`throwaway.key`, `throwaway.key.pub`) was deleted from the VM
after this test. No key material was committed.

## Surprising findings Tasks 4/5/6 should know about

1. **The VM is ARM64, not x64.** `$env:PROCESSOR_ARCHITECTURE` = `ARM64`,
   `systeminfo` reports `System Type: ARM64-based PC`, and `rustc -vV` reports
   `host: aarch64-pc-windows-msvc`. Every artifact produced in this task is named
   with `arm64` (`TermLab_3.0.0-rc.2_arm64-setup.exe`), not `x64` as both the task
   brief and `docs/superpowers/plans/2026-09-03-windows-installer-registration.md`
   assumed. Task 5's build script and Task 4/6's verification assertions must not
   hardcode `x64` in installer filenames or architecture checks — they need to
   either detect the host architecture or handle both. This also means CI (which
   presumably runs on `windows-latest`, an x64 GitHub-hosted runner) will produce
   differently-named artifacts than this VM. `INSTALLED_EXE_NAME` (`termlab.exe`)
   and `INSTALL_DIR` (`%LOCALAPPDATA%\TermLab`) are almost certainly
   architecture-independent (they come from `tauri.conf.json`/NSIS template
   defaults, not from the target triple), but nothing in this task's evidence
   confirms that for x64 specifically — no x64 build was performed.

2. **The stock CI-equivalent build command fails before touching Rust at all**,
   for a reason unrelated to versioning or signing. Running the exact command CI
   uses (`cargo tauri build --config '{"bundle":{"active":true}}'`, tauri-cli
   2.11.4) fails immediately after the `beforeBuildCommand` step with:
   ```
   Error The configured frontendDist includes the `["node_modules"]` folder. Please isolate your web assets on a separate folder and update `tauri.conf.json > build > frontendDist`.
   ```
   `tauri.conf.json` sets `"frontendDist": "frontend"` (resolves to
   `crates/termlab_tauri/frontend`), and `beforeBuildCommand` runs
   `npm --prefix crates/termlab_tauri/frontend ci`, which installs `node_modules`
   directly inside that same directory. `node_modules` is gitignored but tauri-cli
   2.11.4 validates the actual directory contents at bundle time, not `.gitignore`,
   and rejects it. **This is the exact command
   `.github/workflows/release.yml` runs for both the `windows` and `macos` jobs**
   (`cargo install tauri-cli --version "^2"` with no `--locked`/pin, so CI always
   gets whatever the newest 2.x is at run time). This strongly suggests the
   existing release workflow would fail today on a fresh run, independent of
   anything in this branch — it is not something Task 1 introduced, and it is not
   in scope to fix here, but Task 5 (and whoever owns CI) should know before
   assuming the workflow currently works. I did not modify `tauri.conf.json`,
   `beforeBuildCommand`, or any workflow file; for research purposes only, I
   worked around this locally on the VM by relocating (not deleting)
   `crates\termlab_tauri\frontend\node_modules` out of the tree and overriding
   `build.beforeBuildCommand` to `""` via the same disposable `--config` JSON file
   mechanism the brief already sanctions, then moved `node_modules` back
   afterward. No tracked file was changed.

3. The compiled binary is genuinely never renamed away from `termlab.exe` at any
   stage of this pipeline (build, patch-for-bundle-type, or NSIS packaging) despite
   `productName: "TermLab"`. Any Task 4/6 registry value, shortcut target, or
   verification assertion that assumes `TermLab.exe` will be wrong. The NSIS
   installer, uninstaller, Start Menu shortcut, and registry `DisplayIcon` all
   consistently point at `termlab.exe`.

4. `Publisher` in the uninstall registry entry is `termlab` (lowercase), not
   `TermLab` or `an0nn30` — worth knowing if Task 6 asserts on that field.

5. Running installers via a plain SSH-spawned PowerShell process did not reliably
   work: the SSH session runs in Windows Session 0 (`query session` shows the SSH
   process attached to `services`/session 0, while the real interactive desktop is
   session 1). The first silent-install attempt (`Start-Process ... -Wait`)
   appears to have actually completed (the install directory and registry key were
   found afterward) but the SSH connection itself dropped mid-command
   (`Host is down`) — possibly coincidental, but worth noting for Task 5 if its
   script or CI ever drives an install non-interactively over a remote session. A
   second attempt via a Task Scheduler "run interactively as the logged-on user"
   task did not visibly do anything new (no new temp files, no new install) —
   inconclusive, not further investigated since the first attempt's on-disk result
   was already sufficient evidence.

## Steps performed, in order (commands + outcomes)

1. `git fetch origin && git checkout feat/windows-installer-registration && git
   reset --hard origin/feat/windows-installer-registration` — VM was behind (at
   `030a242`); the local worktree here had 2 unpushed commits (`5c2892c`, `00e99a4`,
   the `--working-directory` CLI work from Tasks 2/3). Pushed them
   (`git push -u origin feat/windows-installer-registration`), then re-synced the
   VM. Confirmed `git log --oneline -1` on the VM = `00e99a4`.
2. Confirmed JDK 21 (`javac 21.0.12.1`, `jar 21.0.12.1`) and `cargo tauri --version`
   = `tauri-cli 2.11.4` (both already set up before this task started).
3. Built the Java SDK jar (`javac` + `jar cf java-sdk\build\termlab-plugin-sdk.jar`)
   — succeeded.
4. Wrote `bundle-on.json` = `{"bundle":{"active":true}}` and ran
   `cargo tauri build --config bundle-on.json`. This is the **stock** command from
   the brief (mirroring CI). Failed on the `frontendDist`/`node_modules` error
   above — no Rust compilation was reached.
5. Worked around it (see finding 2) by moving
   `crates\termlab_tauri\frontend\node_modules` to
   `C:\Users\dustin\node_modules_backup` and overriding
   `{"build":{"beforeBuildCommand":""},"bundle":{"active":true}}`. This build
   compiled the full release binary (`Finished release profile [optimized]
   target(s) in 4m 52s`) and then failed at MSI packaging on the version error
   above (`MSI_VERSION_OK = false`).
6. Re-ran with `{"build":{"beforeBuildCommand":""},"bundle":{"active":true,
   "targets":["nsis"]}}` (binary already compiled, so this repackaging step took
   ~1 minute). NSIS packaging succeeded
   (`TermLab_3.0.0-rc.2_arm64-setup.exe`, 9,778,153 bytes), then the overall
   command still exited 1 on the updater-signing error (expected per the brief's
   corrections — `createUpdaterArtifacts: true` + configured `pubkey`, no private
   key on the VM).
7. Installed the produced NSIS installer silently
   (`Start-Process ...TermLab_3.0.0-rc.2_arm64-setup.exe -ArgumentList "/S" -Wait`)
   and recorded the real on-disk layout and registry entry (see constants above).
8. Generated a throwaway signing key and reran step 6's NSIS-only build with
   `TAURI_SIGNING_PRIVATE_KEY`/`_PASSWORD` set, confirming exit code 0 and a
   `.sig` file alongside the installer (see signing-key section above).
9. Uninstalled TermLab via its own `uninstall.exe /S`; confirmed
   `%LOCALAPPDATA%\TermLab`, the HKCU uninstall registry key, and the Start Menu
   shortcut are all gone.
10. Deleted the throwaway key files, all `bundle-on*.json` / `tauri-build-output*.log`
    scratch files, all `.ps1` helper scripts copied to the VM, and a stray
    generated `crates/termlab_tauri/gen/schemas/windows-schema.json`. Moved
    `node_modules` back to `crates\termlab_tauri\frontend\node_modules`.
    Confirmed `git status --short` on the VM is empty.

## Bundle output paths (final successful state, NSIS + throwaway-signed updater artifact)

```
C:\Users\dustin\conch\target\release\bundle\nsis\TermLab_3.0.0-rc.2_arm64-setup.exe      (9,776,307 bytes)
C:\Users\dustin\conch\target\release\bundle\nsis\TermLab_3.0.0-rc.2_arm64-setup.exe.sig  (428 bytes)
```

No `bundle\msi\` directory was ever produced in this task — MSI packaging never got
past the version-validation error.
