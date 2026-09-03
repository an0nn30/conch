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
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
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

#[test]
fn all_three_context_menu_roots_are_registered() {
    let wix = wix_keys(&read("registration.wxs"));
    for root in [
        r"Software\Classes\Directory\shell\TermLab",
        r"Software\Classes\Directory\Background\shell\TermLab",
        r"Software\Classes\Drive\shell\TermLab",
    ] {
        assert!(
            wix.contains(root),
            "missing context-menu root {root}; the entry would not appear \
             for that kind of right-click"
        );
        assert!(
            wix.contains(&format!(r"{root}\command")),
            "missing command subkey for {root}; the entry would appear but do nothing"
        );
    }
}
