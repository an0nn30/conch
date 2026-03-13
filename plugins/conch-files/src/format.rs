//! Formatting helpers for file size, date, and extension labels.

/// Format a byte count as a human-readable size string.
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Format a unix timestamp as "YYYY-MM-DD HH:MM".
pub fn format_date(timestamp: u64) -> String {
    // Manual UTC conversion — avoids pulling in chrono.
    let secs = timestamp;
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;

    // Days since epoch to Y-M-D (simplified Gregorian).
    let (year, month, day) = days_to_ymd(days);

    format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Return a human-readable extension label for the file extension.
pub fn extension_label(name: &str, is_dir: bool) -> String {
    if is_dir {
        return "<DIR>".to_string();
    }

    let ext = match name.rsplit_once('.') {
        Some((_, e)) => e.to_lowercase(),
        None => return String::new(),
    };

    let label = match ext.as_str() {
        // Programming languages
        "rs" => "Rust Source",
        "py" => "Python",
        "js" => "JavaScript",
        "ts" => "TypeScript",
        "jsx" => "React JSX",
        "tsx" => "React TSX",
        "c" => "C Source",
        "h" => "C Header",
        "cpp" | "cc" | "cxx" => "C++ Source",
        "hpp" => "C++ Header",
        "go" => "Go Source",
        "java" => "Java Source",
        "rb" => "Ruby",
        "php" => "PHP",
        "swift" => "Swift",
        "kt" => "Kotlin",
        "lua" => "Lua Script",
        "sh" | "bash" | "zsh" => "Shell Script",
        "ps1" => "PowerShell",
        "sql" => "SQL",
        "r" => "R Script",
        "pl" => "Perl",
        "cs" => "C# Source",

        // Web / markup
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "scss" | "sass" => "Sass",
        "xml" => "XML",
        "svg" => "SVG Image",

        // Data / config
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "ini" | "cfg" => "Config",
        "csv" => "CSV",
        "env" => "Env File",

        // Documents
        "md" | "markdown" => "Markdown",
        "txt" => "Text",
        "pdf" => "PDF Document",
        "doc" | "docx" => "Word Document",
        "xls" | "xlsx" => "Excel Spreadsheet",
        "ppt" | "pptx" => "PowerPoint",
        "rtf" => "Rich Text",

        // Images
        "png" => "PNG Image",
        "jpg" | "jpeg" => "JPEG Image",
        "gif" => "GIF Image",
        "bmp" => "Bitmap",
        "ico" => "Icon",
        "webp" => "WebP Image",
        "tiff" | "tif" => "TIFF Image",

        // Audio / Video
        "mp3" => "MP3 Audio",
        "wav" => "WAV Audio",
        "flac" => "FLAC Audio",
        "ogg" => "Ogg Audio",
        "mp4" => "MP4 Video",
        "mkv" => "MKV Video",
        "avi" => "AVI Video",
        "mov" => "QuickTime Video",
        "webm" => "WebM Video",

        // Archives
        "zip" => "ZIP Archive",
        "tar" => "Tar Archive",
        "gz" | "gzip" => "Gzip Archive",
        "bz2" => "Bzip2 Archive",
        "xz" => "XZ Archive",
        "7z" => "7-Zip Archive",
        "rar" => "RAR Archive",
        "deb" => "Debian Package",
        "rpm" => "RPM Package",
        "dmg" => "Disk Image",

        // Executables / libraries
        "exe" => "Executable",
        "dll" => "DLL Library",
        "so" => "Shared Library",
        "dylib" => "Dynamic Library",
        "app" => "Application",
        "wasm" => "WebAssembly",

        // Build / project
        "lock" => "Lock File",
        "log" => "Log File",
        "bak" => "Backup",
        "tmp" | "temp" => "Temporary",
        "o" => "Object File",

        // Keys / certs
        "pem" => "PEM Certificate",
        "key" => "Key File",
        "crt" | "cer" => "Certificate",
        "pub" => "Public Key",

        _ => return ext.to_uppercase(),
    };

    label.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1048576), "1.0 MB");
        assert_eq!(format_size(1073741824), "1.0 GB");
    }

    #[test]
    fn test_format_date() {
        // 2024-01-01 00:00 UTC = 1704067200
        assert_eq!(format_date(1704067200), "2024-01-01 00:00");
    }

    #[test]
    fn test_extension_label() {
        assert_eq!(extension_label("test.rs", false), "Rust Source");
        assert_eq!(extension_label("test.py", false), "Python");
        assert_eq!(extension_label("folder", true), "<DIR>");
        assert_eq!(extension_label("file.xyz", false), "XYZ");
        assert_eq!(extension_label("noext", false), "");
    }
}
