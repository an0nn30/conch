//! Local filesystem operations.

use crate::FileEntry;

/// List directory entries at the given path using `std::fs`.
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
        });
    }

    sort_entries(&mut entries);
    Ok(entries)
}

/// Sort entries: directories first, then alphabetically (case-insensitive).
pub fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}
