//! Guards every `.ps1` script in the repo against two PowerShell footguns
//! that pure code review keeps missing, because neither one looks wrong on
//! the page:
//!
//! 1. `\"` inside a PowerShell string is NOT an escape sequence — the
//!    backslash is a literal character and the `"` still terminates the
//!    string, so the parser sees garbage for the rest of the line (or file,
//!    if that string never gets its intended closing quote). PowerShell
//!    escapes a quote with a backtick (`` `" ``) or by doubling it (`""`),
//!    never with a backslash.
//! 2. Windows PowerShell 5.1 (the interpreter actually installed as
//!    `powershell.exe` on the target machines — the dev VM and GitHub
//!    Actions `windows-latest` runners) reads a `.ps1` file with no
//!    byte-order mark using the process's OEM codepage, not UTF-8. Any
//!    non-ASCII character (an em-dash, a curly quote, ...) gets silently
//!    mangled on read, which can corrupt whatever string literal it sits
//!    inside. Keeping every script pure ASCII sidesteps the codepage
//!    question entirely, on any machine, in any locale — a BOM would also
//!    fix it, but only until a future edit or tool strips the BOM back off
//!    and silently reintroduces the bug.
//!
//! Both bugs shipped once in `scripts/build-windows.ps1` at the same time
//! and passed three rounds of human/agent code review, because reading the
//! source doesn't reveal either one — you have to know the specific
//! escaping and codepage rules. This test runs on every platform (it is
//! plain text scanning, no PowerShell required) so CI catches it on Linux
//! and macOS too, not just when someone happens to run the script on
//! Windows.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Directories that are never worth walking into: VCS internals, dependency
/// build output, and other trees that can contain vendored or generated
/// `.ps1`-shaped files this test has no business asserting on.
fn is_ignored_dir(name: &str) -> bool {
    matches!(
        name,
        "target"
            | "node_modules"
            | "dist"
            | "build"
            | ".git"
            | ".superpowers"
            | ".worktrees"
            | "vendor"
    ) || name.starts_with('.')
}

/// Every `.ps1` file under the repo root, found by a plain recursive walk
/// (no extra crates — this mirrors the rest of this test suite's
/// std-only, plain-text-parsing style).
fn find_ps1_files() -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![repo_root()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !is_ignored_dir(&name) {
                    stack.push(path);
                }
            } else if file_type.is_file() && path.extension().and_then(|e| e.to_str()) == Some("ps1") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn display(path: &Path, repo_root: &Path) -> String {
    path.strip_prefix(repo_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn assert_hygienic(path: &Path, repo_root: &Path) {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let rel = display(path, repo_root);

    // --- No non-ASCII bytes ------------------------------------------------
    if let Some(bad_offset) = bytes.iter().position(|b| *b >= 0x80) {
        // Best-effort: show a small window of context, tolerating the fact
        // that the surrounding bytes may not be valid UTF-8 either.
        let start = bad_offset.saturating_sub(20);
        let end = (bad_offset + 20).min(bytes.len());
        let context = String::from_utf8_lossy(&bytes[start..end]);
        panic!(
            "{rel} contains a non-ASCII byte at offset {bad_offset} (context: {context:?}).\n\
             Windows PowerShell 5.1 (the interpreter this script actually runs under on \
             the dev VM and CI's windows-latest runner) reads a BOM-less .ps1 file using \
             the process's OEM codepage, not UTF-8 — a non-ASCII character (em-dash, curly \
             quote, ...) gets silently mangled on read and can corrupt the string literal \
             it sits inside. Replace it with an ASCII equivalent (e.g. ' - ' for an \
             em-dash) rather than adding a BOM: a BOM only works until some future edit \
             or tool strips it back off."
        );
    }

    // --- No backslash-escaped quotes ---------------------------------------
    // read as a &str is safe now that the non-ASCII check above passed.
    let text = std::str::from_utf8(&bytes)
        .unwrap_or_else(|e| panic!("{rel} is not valid UTF-8/ASCII after the byte check: {e}"));
    for (line_no, line) in text.lines().enumerate() {
        if line.contains("\\\"") {
            panic!(
                "{rel}:{} contains a \\\" sequence: {line:?}\n\
                 PowerShell has no backslash-escape for double quotes: the backslash is a \
                 literal character and the following \" still terminates the string, so \
                 the parser sees garbage for the remainder of the line (or file, if that \
                 string never gets a real closing quote). Escape an embedded double quote \
                 with a backtick (`\") or by doubling it (\"\"), or restructure the string \
                 to avoid embedding one at all.",
                line_no + 1
            );
        }
    }
}

#[test]
fn every_ps1_script_is_ascii_and_has_no_backslash_escaped_quotes() {
    let root = repo_root();
    let scripts = find_ps1_files();
    assert!(
        !scripts.is_empty(),
        "found no .ps1 files under {} — the walk is probably broken, since \
         scripts/build-windows.ps1 is known to exist",
        root.display()
    );
    for script in &scripts {
        assert_hygienic(script, &root);
    }
}

#[cfg(test)]
mod teeth {
    //! Proves the hygiene checks actually fire on real violations, using a
    //! scratch file rather than mutating a tracked script.

    use std::io::Write;

    fn scratch_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("termlab-ps1-hygiene-teeth-{name}-{}.ps1", std::process::id()))
    }

    fn write_scratch(path: &std::path::Path, contents: &[u8]) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(contents).unwrap();
    }

    #[test]
    fn catches_a_non_ascii_byte() {
        let path = scratch_path("emdash");
        // "packaging \xe2\x80\x94 numeric-only" — a real em-dash (U+2014).
        write_scratch(&path, "Write-Host 'packaging \u{2014} numeric-only'".as_bytes());

        let result = std::panic::catch_unwind(|| super::assert_hygienic(&path, &super::repo_root()));
        std::fs::remove_file(&path).ok();

        assert!(
            result.is_err(),
            "assert_hygienic should have panicked on an em-dash but did not"
        );
        let message = *result.unwrap_err().downcast::<String>().unwrap();
        assert!(
            message.contains("non-ASCII"),
            "panic message should explain the non-ASCII/codepage problem, got: {message}"
        );
    }

    #[test]
    fn catches_a_backslash_escaped_quote() {
        let path = scratch_path("backslash-quote");
        write_scratch(
            &path,
            b"throw \"could not find a \\\"thing\\\" here\"",
        );

        let result = std::panic::catch_unwind(|| super::assert_hygienic(&path, &super::repo_root()));
        std::fs::remove_file(&path).ok();

        assert!(
            result.is_err(),
            "assert_hygienic should have panicked on a \\\" sequence but did not"
        );
        let message = *result.unwrap_err().downcast::<String>().unwrap();
        assert!(
            message.contains("backslash-escape"),
            "panic message should explain the backslash-escape problem, got: {message}"
        );
    }

    #[test]
    fn a_clean_ascii_file_passes() {
        let path = scratch_path("clean");
        write_scratch(
            &path,
            b"Write-Host 'packaging - numeric-only' -ForegroundColor Cyan\nthrow 'no embedded quotes here'\n",
        );

        let result = std::panic::catch_unwind(|| super::assert_hygienic(&path, &super::repo_root()));
        std::fs::remove_file(&path).ok();

        assert!(
            result.is_ok(),
            "a clean ASCII file with no backslash-escaped quotes should not panic"
        );
    }
}
