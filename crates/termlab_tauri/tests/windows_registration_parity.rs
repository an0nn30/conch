//! Guards the two Windows installer registration sources against drift.
//!
//! `packaging/windows/registration.wxs` (consumed by the MSI bundler) and
//! `packaging/windows/installer-hooks.nsh` (consumed by the NSIS bundler)
//! must register the same keys. Historically the project had two Windows
//! installer definitions and only one of them grew a context menu, so the
//! shipped installer had none. This test is what stops that recurring.

use std::collections::BTreeSet;
use std::path::PathBuf;

fn packaging_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("packaging")
        .join("windows")
}

fn read(name: &str) -> String {
    let path = packaging_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn tauri_conf() -> serde_json::Value {
    let path = crate_dir().join("tauri.conf.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

/// Every `Component Id` declared in the WiX fragment.
fn wix_component_ids(text: &str) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("<Component ") && !trimmed.starts_with("<Component>") {
            continue;
        }
        let id = attribute(trimmed, "Id")
            .unwrap_or_else(|| panic!("<Component> without an Id attribute: {trimmed}"));
        ids.insert(id);
    }
    ids
}

/// Pull the value of `name="..."` out of a single line of XML.
fn attribute(line: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Every `Key` declared by a `<RegistryKey>` element in the WiX fragment.
fn wix_keys(text: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("<RegistryKey") {
            continue;
        }
        assert!(
            trimmed.contains(r#"Root="HKCU""#),
            "registration must be per-user; found a non-HKCU key: {trimmed}"
        );
        let key = attribute(trimmed, "Key")
            .unwrap_or_else(|| panic!("<RegistryKey> without a Key attribute: {trimmed}"));
        keys.insert(key);
    }
    keys
}

/// Every key path targeted by `command` (e.g. `WriteRegStr`) in the NSIS hook.
fn nsis_keys(text: &str, command: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(';') {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix(command) else {
            continue;
        };
        let rest = rest.trim_start();
        let rest = rest.strip_prefix("HKCU").unwrap_or_else(|| {
            panic!("registration must be per-user; found a non-HKCU write: {trimmed}")
        });
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix('"')
            .unwrap_or_else(|| panic!("key path must be quoted: {trimmed}"));
        let end = rest
            .find('"')
            .unwrap_or_else(|| panic!("unterminated key path: {trimmed}"));
        keys.insert(rest[..end].to_string());
    }
    keys
}

#[test]
fn both_bundlers_register_the_same_keys() {
    let wix = wix_keys(&read("registration.wxs"));
    let nsis = nsis_keys(&read("installer-hooks.nsh"), "WriteRegStr");

    let only_in_wix: Vec<_> = wix.difference(&nsis).collect();
    let only_in_nsis: Vec<_> = nsis.difference(&wix).collect();

    assert!(
        only_in_wix.is_empty() && only_in_nsis.is_empty(),
        "the MSI and setup.exe must register identical keys.\n\
         only in registration.wxs: {only_in_wix:?}\n\
         only in installer-hooks.nsh: {only_in_nsis:?}"
    );
}

#[test]
fn uninstall_removes_every_key_the_install_wrote() {
    let hooks = read("installer-hooks.nsh");
    let written = nsis_keys(&hooks, "WriteRegStr");

    // Software\\RegisteredApplications is shared with every other installed
    // application, so uninstall removes our *value* from it rather than the
    // key. Both removal forms therefore count as cleanup.
    let mut deleted = nsis_keys(&hooks, "DeleteRegKey");
    deleted.extend(nsis_keys(&hooks, "DeleteRegValue"));

    for key in &written {
        let covered = deleted
            .iter()
            .any(|d| key == d || key.starts_with(&format!("{d}\\")));
        assert!(
            covered,
            "uninstall leaves {key} behind; no DeleteRegKey covers it"
        );
    }
}

#[test]
fn the_context_menu_invokes_the_flag_the_cli_actually_accepts() {
    for source in ["registration.wxs", "installer-hooks.nsh"] {
        let text = read(source);
        assert!(
            text.contains("--working-directory"),
            "{source} must invoke --working-directory; any other flag exits 2"
        );
        assert!(
            text.contains("%V"),
            "{source} must use capital %V, which is the substitution that \
             resolves for Directory, Directory\\Background and Drive alike"
        );
        assert!(
            !text.contains("\"%v\""),
            "{source} uses lowercase %v, which does not resolve for Directory\\shell"
        );
        assert!(
            text.contains("Open TermLab here"),
            "{source} must use the agreed context-menu label"
        );
    }
}

/// The complete key set the task requires both bundlers to declare:
/// the three context-menu roots and their `\command` subkeys, plus the
/// Default Programs, RegisteredApplications, and App Paths keys.
///
/// This is stated literally (not derived from either file) because
/// `both_bundlers_register_the_same_keys` only diffs the two files' sets
/// against *each other* — a key dropped from both files at once would
/// shrink both sets identically and that test would still pass. Only an
/// assertion against a hardcoded expected set catches that.
fn required_keys() -> BTreeSet<String> {
    [
        r"Software\Classes\Directory\shell\TermLab",
        r"Software\Classes\Directory\shell\TermLab\command",
        r"Software\Classes\Directory\Background\shell\TermLab",
        r"Software\Classes\Directory\Background\shell\TermLab\command",
        r"Software\Classes\Drive\shell\TermLab",
        r"Software\Classes\Drive\shell\TermLab\command",
        r"Software\Clients\Terminal\TermLab\Capabilities",
        r"Software\RegisteredApplications",
        r"Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

#[test]
fn both_files_register_the_complete_required_key_set() {
    let required = required_keys();
    let wix = wix_keys(&read("registration.wxs"));
    let nsis = nsis_keys(&read("installer-hooks.nsh"), "WriteRegStr");

    let missing_from_wix: Vec<_> = required.difference(&wix).collect();
    assert!(
        missing_from_wix.is_empty(),
        "registration.wxs is missing required keys: {missing_from_wix:?}; \
         the corresponding feature would not exist in the MSI at all"
    );

    let missing_from_nsis: Vec<_> = required.difference(&nsis).collect();
    assert!(
        missing_from_nsis.is_empty(),
        "installer-hooks.nsh is missing required keys: {missing_from_nsis:?}; \
         the corresponding feature would not exist in the setup.exe at all"
    );
}

/// Everything above this line proves the two registration *sources* agree
/// with each other, but neither reads `tauri.conf.json` — the config that
/// actually wires those sources into the bundlers. Deleting
/// `bundle.windows.wix.fragmentPaths`, `componentRefs`, or
/// `nsis.installerHooks` from that file leaves both installers registering
/// nothing while every test above still passes.
#[test]
fn tauri_conf_wires_the_wix_fragment_into_the_bundle() {
    let conf = tauri_conf();
    let fragment_paths = conf["bundle"]["windows"]["wix"]["fragmentPaths"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "tauri.conf.json bundle.windows.wix.fragmentPaths is missing or not an array; \
                 without it registration.wxs is never compiled into the MSI at all"
            )
        });
    let declares_registration_wxs = fragment_paths
        .iter()
        .filter_map(|v| v.as_str())
        .any(|p| p.ends_with("registration.wxs"));
    assert!(
        declares_registration_wxs,
        "tauri.conf.json bundle.windows.wix.fragmentPaths does not reference \
         registration.wxs (got {fragment_paths:?}); the MSI would ship with no \
         context menu, no default-app entry, and no App Paths key"
    );
}

#[test]
fn tauri_conf_wires_the_nsis_hooks_into_the_bundle() {
    let conf = tauri_conf();
    let installer_hooks = conf["bundle"]["windows"]["nsis"]["installerHooks"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "tauri.conf.json bundle.windows.nsis.installerHooks is missing or not a \
                 string; without it installer-hooks.nsh is never compiled into the \
                 setup.exe at all"
            )
        });
    assert!(
        installer_hooks.ends_with("installer-hooks.nsh"),
        "tauri.conf.json bundle.windows.nsis.installerHooks does not point at \
         installer-hooks.nsh (got '{installer_hooks}'); the setup.exe would ship with \
         no context menu, no default-app entry, and no App Paths key"
    );
}

#[test]
fn every_wix_component_is_referenced_by_component_refs_and_vice_versa() {
    let conf = tauri_conf();
    let component_refs: BTreeSet<String> = conf["bundle"]["windows"]["wix"]["componentRefs"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "tauri.conf.json bundle.windows.wix.componentRefs is missing or not an \
                 array; with none referenced, none of registration.wxs's components \
                 would be included in the MSI"
            )
        })
        .iter()
        .filter_map(|v| v.as_str())
        .map(String::from)
        .collect();

    let components = wix_component_ids(&read("registration.wxs"));

    let declared_but_missing: Vec<_> = components.difference(&component_refs).collect();
    assert!(
        declared_but_missing.is_empty(),
        "registration.wxs declares components with no matching \
         componentRefs entry in tauri.conf.json: {declared_but_missing:?}; \
         a <Component> not referenced there is silently dropped from the MSI"
    );

    let referenced_but_absent: Vec<_> = component_refs.difference(&components).collect();
    assert!(
        referenced_but_absent.is_empty(),
        "tauri.conf.json's componentRefs names components that do not exist in \
         registration.wxs: {referenced_but_absent:?}; the MSI bundle would fail \
         to build"
    );
}
