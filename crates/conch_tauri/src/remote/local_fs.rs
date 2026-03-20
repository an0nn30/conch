//! Local filesystem operations — same interface as sftp.rs.
//!
//! Both modules return `FileEntry` so the frontend can treat local and
//! remote file browsers identically.

use conch_remote::sftp::FileEntry;

/// List directory entries at the given local path.
pub fn list_dir(path: &str) -> Result<Vec<FileEntry>, String> {
    let dir = std::fs::read_dir(path).map_err(|e| format!("{e}"))?;

    let mut entries = Vec::new();
    for entry in dir {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().to_string();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        entries.push(FileEntry {
            name,
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified,
            permissions: None,
        });
    }

    sort_entries(&mut entries);
    Ok(entries)
}

/// Stat a single local path.
pub fn stat(path: &str) -> Result<FileEntry, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{e}"))?;
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    Ok(FileEntry {
        name,
        is_dir: meta.is_dir(),
        size: meta.len(),
        modified,
        permissions: None,
    })
}

/// Create a directory.
pub fn mkdir(path: &str) -> Result<(), String> {
    std::fs::create_dir(path).map_err(|e| format!("{e}"))
}

/// Rename a file or directory.
pub fn rename(from: &str, to: &str) -> Result<(), String> {
    std::fs::rename(from, to).map_err(|e| format!("{e}"))
}

/// Delete a file or directory.
pub fn remove(path: &str, is_dir: bool) -> Result<(), String> {
    if is_dir {
        std::fs::remove_dir_all(path).map_err(|e| format!("{e}"))
    } else {
        std::fs::remove_file(path).map_err(|e| format!("{e}"))
    }
}

/// Sort entries: directories first, then alphabetically (case-insensitive).
fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_dirs_first() {
        let mut entries = vec![
            FileEntry {
                name: "zebra.txt".into(),
                is_dir: false,
                size: 10,
                modified: None,
                permissions: None,
            },
            FileEntry {
                name: "alpha_dir".into(),
                is_dir: true,
                size: 0,
                modified: None,
                permissions: None,
            },
            FileEntry {
                name: "beta.txt".into(),
                is_dir: false,
                size: 20,
                modified: None,
                permissions: None,
            },
        ];
        sort_entries(&mut entries);
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].name, "alpha_dir");
        assert_eq!(entries[1].name, "beta.txt");
        assert_eq!(entries[2].name, "zebra.txt");
    }

    #[test]
    fn sort_case_insensitive() {
        let mut entries = vec![
            FileEntry {
                name: "Zebra".into(),
                is_dir: false,
                size: 0,
                modified: None,
                permissions: None,
            },
            FileEntry {
                name: "alpha".into(),
                is_dir: false,
                size: 0,
                modified: None,
                permissions: None,
            },
        ];
        sort_entries(&mut entries);
        assert_eq!(entries[0].name, "alpha");
        assert_eq!(entries[1].name, "Zebra");
    }

    #[test]
    fn list_dir_current() {
        // Should be able to list the current directory without error.
        let result = list_dir(".");
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn list_dir_nonexistent() {
        let result = list_dir("/nonexistent_path_that_does_not_exist_12345");
        assert!(result.is_err());
    }

    #[test]
    fn stat_current_dir() {
        let result = stat(".");
        assert!(result.is_ok());
        assert!(result.unwrap().is_dir);
    }
}
