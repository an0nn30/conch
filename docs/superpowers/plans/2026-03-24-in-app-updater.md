# In-App Updater Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add automatic update checking and in-place update installation on macOS and Windows via Tauri's updater plugin and GitHub Releases.

**Architecture:** New `updater.rs` module exposes check/install/restart Tauri commands. The `Update` object from the plugin is stored in managed state between check and install. Startup check runs as a delayed async task. Frontend uses existing toast system for notifications and overlay dialog pattern for restart prompt.

**Tech Stack:** `tauri-plugin-updater` v2, `tauri-plugin-process` v2, GitHub Releases `latest.json` endpoint

**Spec:** `docs/superpowers/specs/2026-03-24-in-app-updater-design.md`

---

## File Structure

| Action | Path | Responsibility |
|--------|------|---------------|
| Create | `crates/conch_tauri/src/updater.rs` | Update check, install, restart commands + managed state |
| Modify | `crates/conch_tauri/src/lib.rs` | Register plugin, commands, managed state, menu item, startup check |
| Modify | `crates/conch_core/src/config/conch.rs` | Add `check_for_updates` field |
| Modify | `crates/conch_tauri/Cargo.toml` | Add updater + process plugin deps |
| Modify | `crates/conch_tauri/tauri.conf.json` | Updater endpoint, pubkey, bundle config |
| Modify | `crates/conch_tauri/capabilities/default.json` | Updater + process permissions |
| Modify | `crates/conch_tauri/frontend/index.html` | Update event listeners, toast, restart dialog |
| Modify | `crates/conch_tauri/frontend/settings.js` | "Check for updates" toggle in Advanced |
| Modify | `.github/workflows/release.yml` | `cargo tauri build`, signing, latest.json |
| Modify | `README.md` | Document auto-updates feature |

---

### Task 1: Add `check_for_updates` config field

**Files:**
- Modify: `crates/conch_core/src/config/conch.rs`

- [ ] **Step 1: Write failing test**

Add to the existing test module in `crates/conch_core/src/config/conch.rs` (or the config test file):

```rust
#[test]
fn check_for_updates_defaults_to_true() {
    let config = ConchConfig::default();
    assert!(config.check_for_updates);
}

#[test]
fn check_for_updates_survives_round_trip() {
    let mut config = ConchConfig::default();
    config.check_for_updates = false;
    let toml = toml::to_string(&config).unwrap();
    let parsed: ConchConfig = toml::from_str(&toml).unwrap();
    assert!(!parsed.check_for_updates);
}

#[test]
fn check_for_updates_missing_defaults_true() {
    // Backward compat: old configs without this field get true
    let parsed: ConchConfig = toml::from_str("").unwrap();
    assert!(parsed.check_for_updates);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p conch_core --lib config`
Expected: FAIL — `check_for_updates` field doesn't exist

- [ ] **Step 3: Add the field to ConchConfig**

In `crates/conch_core/src/config/conch.rs`, add to the `ConchConfig` struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConchConfig {
    pub keyboard: KeyboardConfig,
    pub ui: UiConfig,
    pub plugins: PluginsConfig,
    pub check_for_updates: bool,
}
```

Update the `Default` impl:

```rust
impl Default for ConchConfig {
    fn default() -> Self {
        Self {
            keyboard: KeyboardConfig::default(),
            ui: UiConfig::default(),
            plugins: PluginsConfig::default(),
            check_for_updates: true,
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p conch_core --lib config`
Expected: All pass

- [ ] **Step 5: Commit**

```bash
git add crates/conch_core/src/config/conch.rs
git commit -m "Add check_for_updates config field (default: true)"
```

---

### Task 2: Add Cargo dependencies

**Files:**
- Modify: `crates/conch_tauri/Cargo.toml`

- [ ] **Step 1: Add dependencies**

Add after the existing `tauri-plugin-dialog` line:

```toml
tauri-plugin-updater = "2"
tauri-plugin-process = "2"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p conch_tauri`
Expected: Compiles (warnings ok)

- [ ] **Step 3: Commit**

```bash
git add crates/conch_tauri/Cargo.toml
git commit -m "Add tauri-plugin-updater and tauri-plugin-process dependencies"
```

---

### Task 3: Update Tauri config and capabilities

**Files:**
- Modify: `crates/conch_tauri/tauri.conf.json`
- Modify: `crates/conch_tauri/capabilities/default.json`

- [ ] **Step 1: Update tauri.conf.json**

Replace the `"bundle"` section and add `"plugins"` section. The `pubkey` will be a placeholder until keys are generated:

```json
{
  "$schema": "https://raw.githubusercontent.com/nicegui/nicegui/main/nicegui/static/tauri-v2.schema.json",
  "productName": "Conch (Tauri)",
  "identifier": "com.conch.tauri",
  "build": {
    "frontendDist": "frontend"
  },
  "app": {
    "withGlobalTauri": true,
    "windows": [
      {
        "title": "Conch",
        "width": 1200,
        "height": 800,
        "resizable": true,
        "decorations": true
      }
    ],
    "security": {
      "csp": null
    }
  },
  "bundle": {
    "active": false,
    "createUpdaterArtifacts": "v2Compatible"
  },
  "plugins": {
    "updater": {
      "endpoints": [
        "https://github.com/an0nn30/conch/releases/latest/download/latest.json"
      ],
      "pubkey": "PLACEHOLDER_REPLACE_WITH_REAL_PUBKEY"
    }
  }
}
```

Note: `bundle.active` stays `false` for development. The release pipeline sets it when building with `cargo tauri build`. The `createUpdaterArtifacts` is set now so it's ready.

- [ ] **Step 2: Update capabilities/default.json**

Add updater and process permissions to the `permissions` array:

```json
"updater:default",
"process:allow-restart"
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p conch_tauri`
Expected: Compiles

- [ ] **Step 4: Commit**

```bash
git add crates/conch_tauri/tauri.conf.json crates/conch_tauri/capabilities/default.json
git commit -m "Add updater config, endpoint, and capabilities"
```

---

### Task 4: Create updater.rs with Tauri commands

**Files:**
- Create: `crates/conch_tauri/src/updater.rs`

- [ ] **Step 1: Create updater.rs**

```rust
//! In-app update checking and installation (macOS/Windows only).

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

/// Holds the pending update between check and install steps.
/// The `Update` object from tauri-plugin-updater is opaque and not serializable,
/// so we store it in managed state.
pub(crate) struct PendingUpdate(pub Mutex<Option<tauri_plugin_updater::Update>>);

/// Serializable update info returned to the frontend.
#[derive(Serialize, Clone)]
pub(crate) struct UpdateInfo {
    pub version: String,
    pub body: Option<String>,
}

/// Serializable progress event emitted during download.
#[derive(Serialize, Clone)]
struct DownloadProgress {
    downloaded: u64,
    total: Option<u64>,
}

/// Check for an available update. Returns `Ok(Some(info))` if an update is
/// available, `Ok(None)` if up to date. Returns `Err` on network/API failure
/// so the frontend can show "Unable to check" for manual checks.
#[tauri::command]
pub(crate) async fn check_for_update(
    app: AppHandle,
    pending: tauri::State<'_, PendingUpdate>,
) -> Result<Option<UpdateInfo>, String> {
    // Note: verify the exact tauri-plugin-updater v2 API at implementation time.
    // It may be `app.updater()?.check().await` or `app.updater_builder().build()?.check().await`.
    let update = app
        .updater()
        .map_err(|e| format!("Failed to build updater: {e}"))?
        .check()
        .await
        .map_err(|e| format!("Update check failed: {e}"))?;

    match update {
        Some(update) => {
            let info = UpdateInfo {
                version: update.version.clone(),
                body: update.body.clone(),
            };
            *pending.0.lock() = Some(update);
            Ok(Some(info))
        }
        None => Ok(None),
    }
}

/// Download and install the pending update. Emits `update-progress` events
/// during download. Does NOT restart — the frontend handles that.
#[tauri::command]
pub(crate) async fn install_update(
    app: AppHandle,
    pending: tauri::State<'_, PendingUpdate>,
) -> Result<(), String> {
    let update = pending
        .0
        .lock()
        .take()
        .ok_or_else(|| "No pending update".to_string())?;

    let app_handle = app.clone();
    update
        .download_and_install(
            move |downloaded, total| {
                let _ = app_handle.emit(
                    "update-progress",
                    DownloadProgress {
                        downloaded,
                        total,
                    },
                );
            },
            || {
                log::info!("Update downloaded, ready to install on restart");
            },
        )
        .await
        .map_err(|e| format!("Update failed: {e}"))?;

    Ok(())
}

/// Restart the app. Requires tauri-plugin-process to be registered for
/// reliable relaunch on all platforms (fixes tauri-apps/tauri#13923).
#[tauri::command]
pub(crate) fn restart_app(app: AppHandle) {
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_info_serializes() {
        let info = UpdateInfo {
            version: "2.1.0".to_string(),
            body: Some("Bug fixes".to_string()),
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["version"], "2.1.0");
        assert_eq!(json["body"], "Bug fixes");
    }

    #[test]
    fn update_info_serializes_without_body() {
        let info = UpdateInfo {
            version: "2.1.0".to_string(),
            body: None,
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["version"], "2.1.0");
        assert!(json["body"].is_null());
    }

    #[test]
    fn download_progress_serializes() {
        let progress = DownloadProgress {
            downloaded: 1024,
            total: Some(2048),
        };
        let json = serde_json::to_value(&progress).unwrap();
        assert_eq!(json["downloaded"], 1024);
        assert_eq!(json["total"], 2048);
    }

    #[test]
    fn pending_update_starts_empty() {
        let pending = PendingUpdate(Mutex::new(None));
        assert!(pending.0.lock().is_none());
    }
}
```

- [ ] **Step 2: Add module declaration in lib.rs**

Near the top of `crates/conch_tauri/src/lib.rs`, alongside existing module declarations, add:

```rust
mod updater;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p conch_tauri --lib updater::tests`
Expected: All 4 tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/conch_tauri/src/updater.rs crates/conch_tauri/src/lib.rs
git commit -m "Add updater module with check, install, and restart commands"
```

---

### Task 5: Register plugins, commands, state, and startup check in lib.rs

**Files:**
- Modify: `crates/conch_tauri/src/lib.rs`

- [ ] **Step 1: Register plugins in the Tauri builder**

Find the existing `.plugin()` chain (near lines 863-864, around `tauri_plugin_shell::init()` and `tauri_plugin_dialog::init()`). Add:

```rust
.plugin(tauri_plugin_updater::Builder::new().build())
.plugin(tauri_plugin_process::init())
```

- [ ] **Step 2: Register managed state**

Near the existing `.manage()` calls (around line 865-871), add:

```rust
.manage(updater::PendingUpdate(parking_lot::Mutex::new(None)))
```

- [ ] **Step 3: Register commands in invoke_handler**

In the `tauri::generate_handler!` macro (around line 1085), add:

```rust
updater::check_for_update,
updater::install_update,
updater::restart_app,
```

- [ ] **Step 4: Add startup update check**

In the `.setup()` closure (around line 872), after existing initialization (vault timer, migration, etc.), add the startup check. Gate it behind `#[cfg(not(target_os = "linux"))]` equivalent runtime check:

```rust
// Auto-check for updates on startup (macOS/Windows only)
if cfg!(not(target_os = "linux")) {
    let check_enabled = config::load_user_config()
        .map(|c| c.conch.check_for_updates)
        .unwrap_or(true);
    if check_enabled {
        let app_handle = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            // Startup check swallows all errors silently
            let update = match app_handle.updater() {
                Ok(u) => u.check().await,
                Err(e) => { log::warn!("Startup updater init failed: {e}"); return; }
            };
            match update {
                Ok(Some(update)) => {
                    let info = updater::UpdateInfo {
                        version: update.version.clone(),
                        body: update.body.clone(),
                    };
                    let pending = app_handle.state::<updater::PendingUpdate>();
                    *pending.0.lock() = Some(update);
                    let _ = app_handle.emit("update-available", &info);
                }
                Ok(None) => log::debug!("No updates available"),
                Err(e) => log::warn!("Startup update check failed: {e}"),
            }
        });
    }
}
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p conch_tauri`
Expected: Compiles

- [ ] **Step 6: Commit**

```bash
git add crates/conch_tauri/src/lib.rs
git commit -m "Register updater plugin, commands, state, and startup check"
```

---

### Task 6: Add "Check for Updates" menu item

**Files:**
- Modify: `crates/conch_tauri/src/lib.rs`

- [ ] **Step 1: Add menu ID constant**

Near the existing `MENU_*_ID` constants at the top of `lib.rs`, add:

```rust
const MENU_CHECK_UPDATES_ID: &str = "check-for-updates";
```

- [ ] **Step 2: Add menu item to macOS app menu**

In `build_app_menu()`, find the macOS-specific block (around line 696-718) where the app submenu is built. After the "About Conch" or "Settings" item, add:

```rust
MenuItem::with_id(app, MENU_CHECK_UPDATES_ID, "Check for Updates", true, None::<&str>)?,
```

- [ ] **Step 3: Add Help menu on non-macOS**

In the non-macOS block (around line 720-730), after the existing menus, add a Help menu:

```rust
let help_menu = Submenu::with_id_and_items(
    app,
    "help",
    "Help",
    true,
    &[
        &MenuItem::with_id(app, MENU_CHECK_UPDATES_ID, "Check for Updates", true, None::<&str>)?,
    ],
)?;
```

And include `&help_menu` in the `Menu::with_items()` call.

- [ ] **Step 4: Handle the menu event**

In the `.on_menu_event()` handler (around line 992), add a match arm:

```rust
MENU_CHECK_UPDATES_ID => emit_menu_action_to_focused_window(app, "check-for-updates"),
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p conch_tauri`
Expected: Compiles

- [ ] **Step 6: Commit**

```bash
git add crates/conch_tauri/src/lib.rs
git commit -m "Add Check for Updates menu item (App menu on macOS, Help menu on Windows)"
```

---

### Task 7: Frontend — update event handling, toasts, and restart dialog

**Files:**
- Modify: `crates/conch_tauri/frontend/index.html`

- [ ] **Step 1: Add update-available event listener**

After the existing event listeners (around the area where `menu-action` listener is set up, near line 2021), add:

```javascript
// --- In-app updater ---
await listen('update-available', (event) => {
  const info = event.payload;
  if (!info || !info.version) return;
  showUpdateAvailableToast(info);
});

function showUpdateAvailableToast(info) {
  window.toast.show({
    level: 'info',
    title: 'Update Available',
    body: 'Conch v' + window.utils.esc(info.version) + ' is available.',
    duration: 0,  // persistent until dismissed
    action: {
      label: 'Update Now',
      callback: () => startUpdate(),
    },
  });
}
```

Note: `listen` (app-wide) is correct here — the startup check emits via `app_handle.emit()` which is app-scoped. This is a deliberate deviation from `listenOnCurrentWindow` used elsewhere because update notifications are app-level, not window-level.

- [ ] **Step 2: Add startUpdate() function and progress listener**

```javascript
let updateProgressToast = null;

async function startUpdate() {
  updateProgressToast = window.toast.show({
    level: 'info',
    title: 'Updating',
    body: 'Downloading update...',
    duration: 0,
  });

  try {
    await invoke('install_update');
    if (updateProgressToast) {
      window.toast.dismiss(updateProgressToast);
      updateProgressToast = null;
    }
    showRestartDialog();
  } catch (e) {
    if (updateProgressToast) {
      window.toast.dismiss(updateProgressToast);
      updateProgressToast = null;
    }
    window.toast.error('Update Failed', String(e));
  }
}
```

- [ ] **Step 3: Add update-progress listener to update the toast body**

```javascript
await listen('update-progress', (event) => {
  if (!updateProgressToast) return;
  const p = event.payload;
  const body = updateProgressToast.querySelector('.conch-toast-body');
  if (body && p.total) {
    const pct = Math.round((p.downloaded / p.total) * 100);
    body.textContent = 'Downloading update... ' + pct + '%';
  }
});
```

- [ ] **Step 4: Add restart dialog using ssh-overlay/ssh-form pattern**

```javascript
function showRestartDialog() {
  const overlay = document.createElement('div');
  overlay.className = 'ssh-overlay';
  overlay.id = 'update-restart-overlay';

  const dialog = document.createElement('div');
  dialog.className = 'ssh-form';
  dialog.style.width = '400px';

  const title = document.createElement('div');
  title.className = 'ssh-form-title';
  title.textContent = 'Update Ready';
  dialog.appendChild(title);

  const msg = document.createElement('div');
  msg.style.cssText = 'padding:16px 20px;color:var(--fg);font-size:13px';
  msg.textContent = 'The update has been installed. Restart now to apply?';
  dialog.appendChild(msg);

  const buttons = document.createElement('div');
  buttons.className = 'ssh-form-buttons';

  const laterBtn = document.createElement('button');
  laterBtn.className = 'ssh-form-btn';
  laterBtn.textContent = 'Restart Later';
  laterBtn.addEventListener('click', () => overlay.remove());

  const restartBtn = document.createElement('button');
  restartBtn.className = 'ssh-form-btn primary';
  restartBtn.textContent = 'Restart Now';
  restartBtn.addEventListener('click', () => {
    overlay.remove();
    invoke('restart_app');
  });

  buttons.appendChild(laterBtn);
  buttons.appendChild(restartBtn);
  dialog.appendChild(buttons);

  overlay.appendChild(dialog);
  overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) overlay.remove(); });
  document.body.appendChild(overlay);
}
```

- [ ] **Step 5: Handle manual menu check-for-updates action**

In the `handleMenuAction()` function (around line 1995), add an if-block (this function uses if-chains, not switch-case):

```javascript
if (action === 'check-for-updates') {
  invoke('check_for_update').then(info => {
    if (info) {
      showUpdateAvailableToast(info);
    } else {
      window.toast.info('Up to Date', "You're running the latest version.");
    }
  }).catch(() => {
    window.toast.warn('Update Check Failed', 'Unable to check for updates.');
  });
  return;
}
```

The `check_for_update` command returns `Err` on network/API failures, which the `.catch()` handles by showing a warning toast. For the startup check, errors are swallowed silently (handled in the Rust startup task).

- [ ] **Step 6: Commit**

```bash
git add crates/conch_tauri/frontend/index.html
git commit -m "Add frontend update notification, progress, and restart dialog"
```

---

### Task 8: Add "Check for updates" toggle in Settings > Advanced

**Files:**
- Modify: `crates/conch_tauri/frontend/settings.js`

- [ ] **Step 1: Add toggle to renderAdvanced()**

In the `renderAdvanced()` function (around line 1153), add a new section before the "Initial Window Size" section:

```javascript
// Updates section (macOS/Windows only)
addSectionLabel(c, 'Updates');

const updateSwitch = makeSwitch(
  pendingSettings.conch.check_for_updates !== false,
  (val) => { pendingSettings.conch.check_for_updates = val; }
);
addRow(c, 'Check for updates on startup', 'Automatically check for new versions when the app starts (macOS and Windows)', updateSwitch);

addDivider(c);
```

This uses the existing `makeSwitch(checked, onChange)` helper (around line 702 in settings.js) which handles the checkbox + slider DOM construction.

- [ ] **Step 2: Commit**

```bash
git add crates/conch_tauri/frontend/settings.js
git commit -m "Add check-for-updates toggle in Settings > Advanced"
```

---

### Task 9: Update release pipeline

**Files:**
- Modify: `.github/workflows/release.yml`

This is the most involved infrastructure task. The changes are:

- [ ] **Step 1: Install Tauri CLI in macOS and Windows jobs**

Both macOS and Windows jobs need the Tauri CLI. Add after the rust-cache step in each:

```yaml
- name: Install Tauri CLI
  run: cargo install tauri-cli --version "^2"
```

- [ ] **Step 2: Update macOS job to use `cargo tauri build`**

Replace the separate ARM64 + x86_64 `cargo build` steps and manual `lipo` + `.app` assembly + DMG creation with:

```yaml
- name: Build universal app with Tauri
  env:
    TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
  run: cargo tauri build --target universal-apple-darwin

- name: Upload DMG
  run: |
    ./.github/workflows/upload_asset.sh \
      ./target/universal-apple-darwin/release/bundle/dmg/*.dmg $GITHUB_TOKEN

- name: Upload update artifact (.tar.gz + .sig)
  run: |
    ./.github/workflows/upload_asset.sh \
      ./target/universal-apple-darwin/release/bundle/macos/*.tar.gz $GITHUB_TOKEN
    ./.github/workflows/upload_asset.sh \
      ./target/universal-apple-darwin/release/bundle/macos/*.tar.gz.sig $GITHUB_TOKEN
```

Remove the old `lipo`, `create-dmg`, and manual `.app` assembly steps.

Note: The exact output paths may vary — verify after first build. `cargo tauri build` puts artifacts under `target/<target>/release/bundle/`.

- [ ] **Step 3: Update Windows job to use `cargo tauri build`**

Replace the `cargo build` + manual WiX steps with:

```yaml
- name: Build with Tauri
  env:
    TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
  run: cargo tauri build

- name: Upload NSIS installer
  run: |
    ./.github/workflows/upload_asset.sh \
      ./target/release/bundle/nsis/*.exe $GITHUB_TOKEN

- name: Upload update artifact (.nsis.zip + .sig)
  run: |
    ./.github/workflows/upload_asset.sh \
      ./target/release/bundle/nsis/*.nsis.zip $GITHUB_TOKEN
    ./.github/workflows/upload_asset.sh \
      ./target/release/bundle/nsis/*.nsis.zip.sig $GITHUB_TOKEN
```

Remove the old WiX install, `wix build`, and portable zip steps. Also remove the manual `javac`/`jar` step and use `make -C java-sdk build` like the other jobs.

- [ ] **Step 4: Add latest.json generation job**

Add a new job that runs after macOS and Windows builds complete:

```yaml
generate-latest-json:
  needs: [macos, windows]
  runs-on: ubuntu-latest
  permissions:
    contents: write
  steps:
    - uses: actions/checkout@v4
      with:
        fetch-depth: 0

    - name: Extract version
      id: version
      run: |
        VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
        echo "version=$VERSION" >> "$GITHUB_OUTPUT"

    - name: Download signatures from release assets
      env:
        GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
      run: |
        TAG="${GITHUB_REF_NAME}"
        # Download .sig files from the draft release
        gh release download "$TAG" --pattern "*.sig" --dir sigs/

    - name: Generate latest.json
      run: |
        TAG="${GITHUB_REF_NAME}"
        VERSION="${{ steps.version.outputs.version }}"
        MAC_SIG=$(cat sigs/*.tar.gz.sig 2>/dev/null || echo "")
        WIN_SIG=$(cat sigs/*.nsis.zip.sig 2>/dev/null || echo "")

        cat > latest.json << ENDJSON
        {
          "version": "$VERSION",
          "notes": "See release notes at https://github.com/an0nn30/conch/releases/tag/$TAG",
          "pub_date": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
          "platforms": {
            "darwin-aarch64": {
              "signature": "$MAC_SIG",
              "url": "https://github.com/an0nn30/conch/releases/download/$TAG/Conch.app.tar.gz"
            },
            "darwin-x86_64": {
              "signature": "$MAC_SIG",
              "url": "https://github.com/an0nn30/conch/releases/download/$TAG/Conch.app.tar.gz"
            },
            "windows-x86_64": {
              "signature": "$WIN_SIG",
              "url": "https://github.com/an0nn30/conch/releases/download/$TAG/Conch-v${VERSION}-setup.exe.nsis.zip"
            }
          }
        }
        ENDJSON

    - name: Upload latest.json
      run: |
        ./.github/workflows/upload_asset.sh \
          ./latest.json $GITHUB_TOKEN
```

**Important:** The exact artifact filenames in URLs will depend on what `cargo tauri build` produces. These should be verified after the first successful build and adjusted accordingly.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "Update release pipeline for Tauri bundler and updater artifacts"
```

---

### Task 10: Update README

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add Auto-Updates to Features section**

After the "Settings Dialog" entry and before "Theming", add:

```markdown
**Auto-Updates** (macOS/Windows) — Checks for new versions on startup and notifies when an update is available. Download and install updates in-place from the app. Configurable via Settings > Advanced or check manually from the menu.
```

- [ ] **Step 2: Add to Configuration example**

In the `[conch.plugins]` TOML example block, add before or after:

```toml
[conch]
check_for_updates = true    # Check for new versions on startup (macOS/Windows)
```

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "Document auto-updates in README"
```

---

### Task 11: One-time setup (manual, not automated)

This task is performed by the developer, not by code changes.

- [ ] **Step 1: Generate signing keys**

```bash
cargo tauri signer generate -w ~/.tauri/conch.key
```

This outputs a public key. Copy it.

- [ ] **Step 2: Update tauri.conf.json with real public key**

Replace `"PLACEHOLDER_REPLACE_WITH_REAL_PUBKEY"` in `tauri.conf.json` with the actual public key from step 1.

- [ ] **Step 3: Add secrets to GitHub repo**

Go to GitHub repo > Settings > Secrets > Actions and add:
- `TAURI_SIGNING_PRIVATE_KEY` — contents of `~/.tauri/conch.key`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — the password used during generation

- [ ] **Step 4: Commit the public key**

```bash
git add crates/conch_tauri/tauri.conf.json
git commit -m "Add updater signing public key"
```

---

## Verification

After all tasks:

- [ ] `cargo test --workspace` — all tests pass
- [ ] `cargo clippy --workspace` — no new warnings
- [ ] `cargo check -p conch_tauri` — compiles
- [ ] Manual: launch app → after 3s, no crash (no real update available yet, check silently returns nothing)
- [ ] Manual: Window > Check for Updates (or Conch > Check for Updates on macOS) → shows "Up to Date" toast
- [ ] Manual: Settings > Advanced → "Check for updates on startup" toggle present
- [ ] Manual: disable toggle, restart app → no update check on startup
- [ ] Tag a test release to verify the full pipeline: `cargo tauri build` produces DMG + `.tar.gz` + `.sig` on macOS, NSIS `.exe` + `.nsis.zip` + `.sig` on Windows, and `latest.json` is uploaded correctly
