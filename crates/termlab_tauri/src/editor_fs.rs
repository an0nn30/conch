//! Filesystem access for the light editor.
//!
//! Every guard the editor applies lives here and nowhere else: the frontend
//! asks this module whether a file may be opened rather than carrying its own
//! copy of the size cap or the blocklist.

use std::fs;
use std::path::{Path, PathBuf};

/// Files above this never open. Matches the JVM editor's cap.
pub const MAX_EDIT_BYTES: u64 = 5 * 1024 * 1024;

/// How much of a file the binary sniff inspects.
const SNIFF_BYTES: usize = 8192;

/// Extensions we refuse outright, so a mis-click on an image or an archive
/// does not pull megabytes over SFTP just to be rejected after the fact.
pub const BLOCKED_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "svg", "zip", "tar", "gz", "tgz", "bz2",
    "xz", "7z", "rar", "jar", "war", "ear", "class", "exe", "dll", "so", "dylib", "pdf", "doc",
    "docx", "xls", "xlsx", "ppt", "pptx", "mp3", "mp4", "mov", "avi", "mkv", "wav", "flac", "pyc",
    "pyo",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenRejection {
    TooLarge { size: u64, max: u64 },
    BlockedExtension { ext: String },
    Binary { name: String },
}

impl OpenRejection {
    /// The user-facing message. Formatted here so every caller says the same
    /// thing about the same rejection.
    pub fn message(&self) -> String {
        match self {
            OpenRejection::TooLarge { size, max } => format!(
                "File too large ({:.1} MB). Maximum is {} MB.",
                *size as f64 / (1024.0 * 1024.0),
                max / (1024 * 1024)
            ),
            OpenRejection::BlockedExtension { ext } => {
                format!("Cannot edit binary file: .{ext}")
            }
            OpenRejection::Binary { name } => format!("Binary file detected: {name}"),
        }
    }
}

/// Decide whether a file may be opened, from its name and size alone — no I/O,
/// so the remote path can reject before transferring a byte.
pub fn guard_openable(name: &str, size: u64) -> Result<(), OpenRejection> {
    if size > MAX_EDIT_BYTES {
        return Err(OpenRejection::TooLarge {
            size,
            max: MAX_EDIT_BYTES,
        });
    }
    if let Some(ext) = extension_of(name)
        && BLOCKED_EXTENSIONS.contains(&ext.as_str())
    {
        return Err(OpenRejection::BlockedExtension { ext });
    }
    Ok(())
}

/// The lowercased characters after the final dot, when there is one that is
/// not the leading dot of a dotfile. `".bashrc"` has no extension by this
/// definition, which is what makes dotfiles editable.
fn extension_of(name: &str) -> Option<String> {
    let base = name.rsplit('/').next().unwrap_or(name);
    let idx = base.rfind('.')?;
    if idx == 0 {
        return None;
    }
    Some(base[idx + 1..].to_ascii_lowercase())
}

/// True if the head of a file contains a NUL, the cheap heuristic for "this is
/// not text". Only the first [`SNIFF_BYTES`] are examined, so a NUL past that
/// window is deliberately missed rather than costing a full scan.
pub fn looks_binary(head: &[u8]) -> bool {
    head.iter().take(SNIFF_BYTES).any(|b| *b == 0)
}

/// FNV-1a, 64-bit, rendered as 8 hex characters.
///
/// This disambiguates directory names on disk; it is not security, so a real
/// hash crate would be a dependency bought for nothing.
fn short_hash(input: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", hash)[..8].to_string()
}

/// The three path components of a remote edit's temp location:
/// (host directory, path directory, basename).
///
/// Split out from [`temp_path`] so the naming rules are testable without
/// touching the filesystem.
pub fn temp_path_parts(host_label: &str, remote_path: &str) -> (String, String, String) {
    let basename = remote_path
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("untitled")
        .to_string();
    (short_hash(host_label), short_hash(remote_path), basename)
}

pub fn temp_root() -> PathBuf {
    std::env::temp_dir().join("termlab-sftp-edits")
}

/// Write via a sibling temp file and a rename, so a write that fails partway
/// leaves the original file exactly as it was.
pub fn write_text_file(path: &str, contents: &str) -> Result<(), String> {
    let target = Path::new(path);
    let tmp = target.with_extension(format!(
        "{}termlab-tmp",
        target
            .extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    fs::write(&tmp, contents).map_err(|e| {
        // `fs::write` is create-then-write_all: a write that fails partway
        // still leaves the created file on disk. Clean it up so a failed
        // write never leaves litter that also blocks the empty-parent climb
        // in `editor_temp_cleanup`.
        let _ = fs::remove_file(&tmp);
        format!("Could not write {path}: {e}")
    })?;
    fs::rename(&tmp, target).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Could not replace {path}: {e}")
    })
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn editor_can_open(name: String, size: f64) -> Result<(), String> {
    let size = if size < 0.0 { 0 } else { size as u64 };
    guard_openable(&name, size).map_err(|r| r.message())
}

#[tauri::command]
pub(crate) fn editor_read_file(path: String) -> Result<String, String> {
    let p = Path::new(&path);
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    let meta = fs::metadata(p).map_err(|e| format!("Could not open {path}: {e}"))?;
    guard_openable(&name, meta.len()).map_err(|r| r.message())?;

    let bytes = fs::read(p).map_err(|e| format!("Could not read {path}: {e}"))?;
    if looks_binary(&bytes) {
        return Err(OpenRejection::Binary { name }.message());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[tauri::command]
pub(crate) fn editor_write_file(path: String, contents: String) -> Result<(), String> {
    write_text_file(&path, &contents)
}

#[tauri::command]
pub(crate) fn editor_temp_path(host_label: String, remote_path: String) -> Result<String, String> {
    let (host_dir, path_dir, basename) = temp_path_parts(&host_label, &remote_path);
    let dir = temp_root().join(host_dir).join(path_dir);
    fs::create_dir_all(&dir).map_err(|_| "Cannot create temp file for editing".to_string())?;
    Ok(dir.join(basename).to_string_lossy().into_owned())
}

/// Delete a temp file and any parent directories it leaves empty, without
/// escaping the temp root — a caller passing an arbitrary path must not be
/// able to delete outside it.
///
/// `Path::starts_with` is a lexical, component-wise prefix check: it does not
/// resolve `..`, so a path like `<root>/../etc/passwd` lexically starts with
/// `root` even though the OS would delete a file outside it. Rather than
/// `canonicalize` (which fails when the target has already been removed), we
/// reject outright any path containing a `..` component at all, then apply
/// the lexical prefix check to what is left.
#[tauri::command]
pub(crate) fn editor_temp_cleanup(path: String) -> Result<(), String> {
    let root = temp_root();
    let target = PathBuf::from(&path);
    if target
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("Refusing to clean a path outside the editor temp root".into());
    }
    if !target.starts_with(&root) {
        return Err("Refusing to clean a path outside the editor temp root".into());
    }
    let _ = fs::remove_file(&target);
    let mut parent = target.parent().map(|p| p.to_path_buf());
    while let Some(dir) = parent {
        if dir == root || !dir.starts_with(&root) {
            break;
        }
        if fs::remove_dir(&dir).is_err() {
            break; // not empty — stop climbing
        }
        parent = dir.parent().map(|p| p.to_path_buf());
    }
    Ok(())
}

/// Delete everything under the temp root.
///
/// Two callers, both in Rust — this is deliberately not a `#[tauri::command]`,
/// because handing the frontend "delete every remote edit in flight" is not a
/// capability it needs:
///
/// * app setup (`lib.rs`), which clears orphans left by a previous crash;
/// * `close_guard::finish_exit`, which runs only on a completed Quit or
///   Restart poll.
///
/// So "at shutdown" means those two exits and nothing else. Closing the last
/// window does not sweep, and neither does macOS's Dock → Quit, which bypasses
/// `finish_exit` entirely (see Known Limitation 9 in the design spec). Both
/// leave the temp tree behind until the next launch clears it.
pub(crate) fn editor_temp_sweep() -> Result<(), String> {
    let _ = fs::remove_dir_all(temp_root());
    Ok(())
}

/// Largest image inlined into a preview. Images become base64 `data:` URIs, so
/// the cost is ~4/3 of this in the webview per image; a cap keeps one oversized
/// asset from stalling a render.
pub(crate) const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

fn image_extension(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let dot = base.rfind('.')?;
    if dot == 0 {
        return None;
    }
    Some(base[dot + 1..].to_ascii_lowercase())
}

pub(crate) fn is_image_name(name: &str) -> bool {
    matches!(
        image_extension(name).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico")
    )
}

pub(crate) fn check_image_size(bytes: u64) -> Result<(), String> {
    if bytes > MAX_IMAGE_BYTES {
        return Err(format!(
            "image is {bytes} bytes, over the {MAX_IMAGE_BYTES} byte preview limit"
        ));
    }
    Ok(())
}

/// Read a local image for the markdown preview, base64-encoded.
///
/// Returns the payload only — the caller builds the `data:` URI, because it
/// already knows the MIME type from the same filename.
#[tauri::command]
pub(crate) fn editor_read_image_base64(path: String) -> Result<String, String> {
    use base64::Engine;

    if !is_image_name(&path) {
        return Err(format!("not a recognised image file: {path}"));
    }
    let meta = std::fs::metadata(&path).map_err(|e| format!("{path}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{path}: not a file"));
    }
    check_image_size(meta.len())?;

    let bytes = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A save into a read-only directory has to fail loudly. The close guards
    /// treat a rejected write as "do not close this tab", so a write that
    /// swallowed the error would hand them consent to discard the file.
    #[cfg(unix)]
    #[test]
    fn write_into_a_read_only_directory_is_an_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("termlab-ro-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test dir");
        let file = dir.join("scratch-1.txt");

        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).expect("chmod 500");
        let result = editor_write_file(file.to_string_lossy().into_owned(), "unsaved text".into());
        // Restore before asserting so a failure still leaves a removable dir.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).expect("chmod 700");

        let error = result.expect_err("a write into a 0500 directory must fail");
        assert!(
            error.starts_with("Could not write "),
            "the message names the file that could not be written: {error}"
        );
        assert!(!file.exists(), "and nothing was left behind");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn size_cap_is_five_megabytes() {
        assert!(guard_openable("a.txt", MAX_EDIT_BYTES).is_ok());
        assert!(matches!(
            guard_openable("a.txt", MAX_EDIT_BYTES + 1),
            Err(OpenRejection::TooLarge { .. })
        ));
    }

    #[test]
    fn blocklist_covers_every_listed_extension_case_insensitively() {
        for ext in BLOCKED_EXTENSIONS {
            let lower = format!("file.{ext}");
            let upper = format!("file.{}", ext.to_uppercase());
            assert!(
                matches!(
                    guard_openable(&lower, 10),
                    Err(OpenRejection::BlockedExtension { .. })
                ),
                "{lower} should be blocked"
            );
            assert!(
                matches!(
                    guard_openable(&upper, 10),
                    Err(OpenRejection::BlockedExtension { .. })
                ),
                "{upper} should be blocked"
            );
        }
    }

    #[test]
    fn ordinary_names_are_allowed() {
        for name in [
            "a.txt",
            "b.rs",
            "Makefile",
            ".gitignore",
            "a.tar.txt",
            "no_extension",
        ] {
            assert!(guard_openable(name, 10).is_ok(), "{name} should be allowed");
        }
    }

    #[test]
    fn multi_dot_names_are_judged_by_the_last_extension() {
        // .tar.gz is blocked by `gz`; .gz.txt is not blocked at all.
        assert!(matches!(
            guard_openable("archive.tar.gz", 10),
            Err(OpenRejection::BlockedExtension { .. })
        ));
        assert!(guard_openable("notes.gz.txt", 10).is_ok());
    }

    #[test]
    fn binary_sniff_looks_at_the_first_8192_bytes_only() {
        assert!(looks_binary(&[0x00]));

        let mut at_8191 = vec![b'a'; 8192];
        at_8191[8191] = 0x00;
        assert!(looks_binary(&at_8191));

        // One byte past the window: must be missed.
        let mut at_8192 = vec![b'a'; 8193];
        at_8192[8192] = 0x00;
        assert!(!looks_binary(&at_8192));

        assert!(!looks_binary(b""));
        assert!(!looks_binary(b"short text"));
    }

    #[test]
    fn temp_paths_separate_hosts_and_paths_and_keep_the_basename() {
        let a = temp_path_parts("host-a", "/etc/nginx.conf");
        let b = temp_path_parts("host-b", "/etc/nginx.conf");
        let c = temp_path_parts("host-a", "/opt/nginx.conf");

        assert_ne!(a.0, b.0, "different hosts must not share a directory");
        assert_eq!(a.0, c.0, "same host must share its directory");
        assert_ne!(
            a.1, c.1,
            "different remote paths must not share a directory"
        );
        assert_eq!(a.2, "nginx.conf");

        assert_eq!(temp_path_parts("h", "/a/.bashrc").2, ".bashrc");
        assert_eq!(temp_path_parts("h", "/a/x.tar.gz").2, "x.tar.gz");
    }

    #[test]
    fn write_uses_a_temp_file_so_a_failed_write_cannot_truncate_the_original() {
        let dir = std::env::temp_dir().join("termlab-editor-write-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("f.txt");
        std::fs::write(&target, "original").unwrap();

        // Writing to a path whose parent does not exist must fail without
        // touching the existing file.
        let bad = dir.join("missing-dir").join("f.txt");
        assert!(write_text_file(bad.to_str().unwrap(), "new").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");

        write_text_file(target.to_str().unwrap(), "replaced").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "replaced");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_refuses_paths_outside_the_temp_root() {
        let outside = std::env::temp_dir().join("termlab-not-an-edit.txt");
        std::fs::write(&outside, "keep me").unwrap();

        assert!(editor_temp_cleanup(outside.to_string_lossy().into_owned()).is_err());
        assert!(outside.exists(), "a path outside the root must survive");

        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn cleanup_rejects_dot_dot_traversal_outside_the_temp_root() {
        let root = temp_root();
        let escape_dir = std::env::temp_dir().join("termlab-cleanup-escape-test");
        let _ = std::fs::remove_dir_all(&escape_dir);
        std::fs::create_dir_all(&escape_dir).unwrap();
        let victim = escape_dir.join("victim.txt");
        std::fs::write(&victim, "keep me").unwrap();

        // Lexically this path starts with `root` — a naive `starts_with`
        // check would accept it — but it resolves outside `root` once the
        // OS follows the `..` component.
        let escaping_path = root
            .join("..")
            .join("termlab-cleanup-escape-test")
            .join("victim.txt");

        assert!(editor_temp_cleanup(escaping_path.to_string_lossy().into_owned()).is_err());
        assert!(
            victim.exists(),
            "a path escaping the root via .. must survive"
        );

        let _ = std::fs::remove_dir_all(&escape_dir);
    }

    #[test]
    fn image_extensions_are_recognised() {
        for name in [
            "a.png", "b.JPG", "c.jpeg", "d.gif", "e.webp", "f.svg", "g.bmp",
        ] {
            assert!(is_image_name(name), "{name} must be treated as an image");
        }
    }

    #[test]
    fn non_image_extensions_are_rejected() {
        for name in ["a.txt", "b.rs", "c.md", "d", "e.png.exe"] {
            assert!(
                !is_image_name(name),
                "{name} must not be treated as an image"
            );
        }
    }

    #[test]
    fn oversized_images_are_refused() {
        assert!(
            check_image_size(MAX_IMAGE_BYTES + 1).is_err(),
            "an image over the cap must be refused rather than inlined"
        );
        assert!(check_image_size(1024).is_ok());
    }
}
