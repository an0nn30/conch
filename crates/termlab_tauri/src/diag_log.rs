//! An on-disk diagnostic log the FRONTEND can write to.
//!
//! `env_logger` sends the Rust side's own lines to stderr, which in a bundled
//! `.app` launched from Finder or `open` goes nowhere a user can reach. The
//! webview is worse off still: `console.log` in a WKWebView needs Safari's
//! Web Inspector attached, so a bug that only shows up in the built app has
//! historically been diagnosed through toasts.
//!
//! This module is the durable half of that: `app_diag_log` appends one
//! timestamped line per call to `~/.config/termlab/logs/frontend.log`, so a bug
//! report is a file the owner can attach rather than a screenshot of a toast
//! that has already faded.
//!
//! Rules the implementation follows, all of them because this is a DIAGNOSTIC
//! and must never become the thing that breaks:
//!
//!   * it never panics and never returns an error the frontend has to handle —
//!     a failed write is a `log::debug!` and nothing more;
//!   * it is capped. Past `MAX_BYTES` the file is truncated to its most recent
//!     `KEEP_BYTES` on a line boundary, so a runaway caller costs a bounded
//!     amount of disk rather than filling it;
//!   * the message is sanitized to one line, so one call is one record and a
//!     newline in a path or an error string cannot forge a second entry.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;

/// Rotate once the file passes this.
const MAX_BYTES: u64 = 1024 * 1024;

/// How much of the tail survives a rotation. Deliberately well under
/// `MAX_BYTES`, so a rotation is rare rather than happening on every line once
/// the cap is first reached.
const KEEP_BYTES: usize = 512 * 1024;

/// Longest single record. A frontend line is a diagnostic, not a payload dump.
const MAX_MESSAGE: usize = 2000;

/// Serializes appends from the several webview windows a session can have
/// open. Each holds its own IPC thread, and two interleaved appends would
/// otherwise be able to split a record.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn log_dir() -> PathBuf {
    termlab_core::config::config_dir().join("logs")
}

pub(crate) fn frontend_log_path() -> PathBuf {
    log_dir().join("frontend.log")
}

/// Collapse a caller-supplied field to something safe to put in a line-oriented
/// file: control characters (newlines included) become spaces, and the result
/// is bounded.
fn sanitize(value: &str, max: usize) -> String {
    let mut out = String::with_capacity(value.len().min(max));
    for ch in value.chars() {
        if out.chars().count() >= max {
            out.push('…');
            break;
        }
        if ch.is_control() {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    let trimmed = out.trim();
    if trimmed.is_empty() {
        "-".to_string()
    } else {
        trimmed.to_string()
    }
}

/// One record, exactly as it lands in the file (with its trailing newline).
///
/// Pure, so the format is testable without touching the filesystem.
pub(crate) fn format_line(timestamp: &str, level: &str, category: &str, message: &str) -> String {
    format!(
        "{} [{}] {}: {}\n",
        sanitize(timestamp, 64),
        sanitize(level, 16).to_ascii_lowercase(),
        sanitize(category, 48),
        sanitize(message, MAX_MESSAGE),
    )
}

/// What should remain in the file once `existing` has grown past the cap.
///
/// Returns `None` when nothing needs to be dropped. Otherwise the tail from the
/// first line boundary at or after `len - KEEP_BYTES`, prefixed with a marker
/// so a reader can see the file was trimmed rather than that it starts
/// mid-session.
///
/// Pure, so the cap is testable without writing a megabyte to disk.
pub(crate) fn rotated(existing: &str) -> Option<String> {
    if existing.len() as u64 <= MAX_BYTES {
        return None;
    }
    let cut = existing.len().saturating_sub(KEEP_BYTES);
    // Never split a record: start at the byte after the next newline at or
    // past `cut`. Cutting on a newline is also what keeps the slice on a UTF-8
    // boundary when a record carries multi-byte characters.
    let start = existing[cut..]
        .find('\n')
        .map(|offset| cut + offset + 1)
        .unwrap_or(existing.len());
    let mut kept = String::with_capacity(existing.len() - start + 64);
    kept.push_str("--- earlier entries trimmed ---\n");
    kept.push_str(&existing[start..]);
    Some(kept)
}

/// Append one record to the session's log. Never panics; a failure is swallowed
/// after a debug line.
pub(crate) fn append(level: &str, category: &str, message: &str) {
    append_to(&frontend_log_path(), level, category, message);
}

/// The same, against an explicit file. Split out so the create/rotate/append
/// behaviour is testable against a temporary directory rather than against the
/// developer's own `~/.config/termlab`.
pub(crate) fn append_to(path: &Path, level: &str, category: &str, message: &str) {
    let timestamp = timestamp_now();
    let line = format_line(&timestamp, level, category, message);
    let _guard = WRITE_LOCK.lock();
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            log::debug!("diagnostic log directory unavailable: {error}");
            return;
        }
    }
    if let Ok(metadata) = fs::metadata(path) {
        if metadata.len() > MAX_BYTES {
            if let Ok(existing) = fs::read_to_string(path) {
                if let Some(kept) = rotated(&existing) {
                    if let Err(error) = fs::write(path, kept) {
                        log::debug!("diagnostic log could not be trimmed: {error}");
                    }
                }
            }
        }
    }
    let opened = fs::OpenOptions::new().create(true).append(true).open(path);
    match opened {
        Ok(mut file) => {
            if let Err(error) = file.write_all(line.as_bytes()) {
                log::debug!("diagnostic log write failed: {error}");
            }
        }
        Err(error) => log::debug!("diagnostic log could not be opened: {error}"),
    }
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ`, built from the wall clock without pulling in a
/// date library: the crate already depends on nothing that formats time, and a
/// log timestamp does not justify one.
fn timestamp_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_timestamp(now.as_secs(), now.subsec_millis())
}

/// Civil time from a Unix timestamp, UTC. Split out from `timestamp_now` so it
/// is testable against known epochs.
pub(crate) fn format_timestamp(epoch_secs: u64, millis: u32) -> String {
    let days = (epoch_secs / 86_400) as i64;
    let seconds_of_day = epoch_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60,
    )
}

/// Howard Hinnant's `civil_from_days`, the standard shift-to-March algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The frontend's entry point. Fire-and-forget by design: it resolves with
/// nothing, so a caller never has to await it or handle a rejection.
#[tauri::command]
pub(crate) fn app_diag_log(level: String, category: String, message: String) {
    append(&level, &category, &message);
}

/// Where the log lives, so the app can tell the user what to attach to a bug
/// report without the frontend hard-coding a path.
#[tauri::command]
pub(crate) fn app_diag_log_path() -> String {
    frontend_log_path().to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_line_is_one_record_per_call() {
        let line = format_line(
            "2026-09-07T12:00:00.000Z",
            "info",
            "vim-nav",
            "back: navigated",
        );
        assert_eq!(
            line, "2026-09-07T12:00:00.000Z [info] vim-nav: back: navigated\n",
            "the record is timestamp, level, category, message"
        );
        assert_eq!(
            line.matches('\n').count(),
            1,
            "one call must produce exactly one line"
        );
    }

    #[test]
    fn newlines_in_a_message_cannot_forge_a_second_record() {
        let line = format_line("t", "info", "vim-nav", "first\nsecond\r\nthird");
        assert_eq!(
            line.matches('\n').count(),
            1,
            "a control character in the message becomes a space, not a record boundary"
        );
        assert!(line.contains("first second  third"), "got {line:?}");
    }

    #[test]
    fn empty_fields_become_a_placeholder_rather_than_a_gap() {
        let line = format_line("t", "", "", "");
        assert_eq!(line, "t [-] -: -\n", "empty fields are visible as '-'");
    }

    #[test]
    fn level_is_normalized_to_lowercase() {
        let line = format_line("t", "WARN", "vim-nav", "x");
        assert!(line.contains("[warn]"), "got {line:?}");
    }

    #[test]
    fn a_long_message_is_bounded() {
        let message = "x".repeat(MAX_MESSAGE * 3);
        let line = format_line("t", "info", "vim-nav", &message);
        assert!(
            line.chars().count() < MAX_MESSAGE + 64,
            "an oversized message is truncated, got {} chars",
            line.chars().count()
        );
        assert!(line.contains('…'), "truncation is visible");
    }

    #[test]
    fn a_small_file_is_not_rotated() {
        assert_eq!(
            rotated("one\ntwo\n"),
            None,
            "under the cap nothing is dropped"
        );
    }

    #[test]
    fn rotation_keeps_the_tail_on_a_line_boundary() {
        let record = "2026-09-07T12:00:00.000Z [info] vim-nav: entry\n";
        let mut existing = String::new();
        while existing.len() as u64 <= MAX_BYTES {
            existing.push_str(record);
        }
        let kept = rotated(&existing).expect("past the cap the file is trimmed");
        assert!(
            kept.starts_with("--- earlier entries trimmed ---\n"),
            "a reader can see the file was trimmed"
        );
        assert!(
            (kept.len() as u64) < MAX_BYTES,
            "the trimmed file is under the cap, got {}",
            kept.len()
        );
        let body = &kept["--- earlier entries trimmed ---\n".len()..];
        assert!(
            body.starts_with(record),
            "the kept tail starts at a record boundary"
        );
        assert!(body.ends_with(record), "the newest record survives");
    }

    #[test]
    fn rotation_of_a_capped_file_without_newlines_keeps_nothing_partial() {
        let existing = "x".repeat((MAX_BYTES + 10) as usize);
        let kept = rotated(&existing).expect("past the cap");
        assert_eq!(
            kept, "--- earlier entries trimmed ---\n",
            "with no record boundary to cut on, nothing partial is kept"
        );
    }

    #[test]
    fn timestamps_render_known_epochs() {
        assert_eq!(
            format_timestamp(0, 0),
            "1970-01-01T00:00:00.000Z",
            "the epoch itself"
        );
        assert_eq!(
            format_timestamp(1_788_796_800, 42),
            "2026-09-07T16:00:00.042Z",
            "a date in the bug report's own week"
        );
        assert_eq!(
            format_timestamp(1_709_164_800, 0),
            "2024-02-29T00:00:00.000Z",
            "a leap day"
        );
    }

    /// A private directory under the OS temp root, removed by the test that
    /// made it. No `HOME` games: `append_to` takes the path precisely so a test
    /// never has to reach into the developer's own config directory.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("termlab-diag-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn appending_creates_the_directory_and_the_file() {
        let dir = scratch("create");
        let path = dir.join("nested").join("frontend.log");
        append_to(
            &path,
            "info",
            "vim-nav",
            "2 vim ran the mapping — termlabJumpBack",
        );
        let written = fs::read_to_string(&path).expect("the log was created");
        assert!(
            written.contains("[info] vim-nav: 2 vim ran the mapping — termlabJumpBack"),
            "got {written:?}"
        );
        assert!(written.ends_with('\n'), "records are newline terminated");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn appending_twice_keeps_both_records_in_order() {
        let dir = scratch("append");
        let path = dir.join("frontend.log");
        append_to(&path, "info", "vim-nav", "first");
        append_to(&path, "warn", "vim-nav", "second");
        let written = fs::read_to_string(&path).expect("the log exists");
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), 2, "one record per call, got {written:?}");
        assert!(lines[0].ends_with("vim-nav: first"), "got {:?}", lines[0]);
        assert!(lines[1].ends_with("vim-nav: second"), "got {:?}", lines[1]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_past_the_cap_is_trimmed_before_the_next_append() {
        let dir = scratch("rotate");
        let path = dir.join("frontend.log");
        fs::create_dir_all(&dir).expect("scratch directory");
        let record = "2026-09-07T12:00:00.000Z [info] vim-nav: old\n";
        let mut oversized = String::new();
        while oversized.len() as u64 <= MAX_BYTES {
            oversized.push_str(record);
        }
        fs::write(&path, &oversized).expect("seed an oversized log");
        append_to(&path, "info", "vim-nav", "new");
        let written = fs::read_to_string(&path).expect("the log exists");
        assert!(
            (written.len() as u64) < MAX_BYTES,
            "the file is back under the cap, got {}",
            written.len()
        );
        assert!(
            written.starts_with("--- earlier entries trimmed ---\n"),
            "the trim is visible to a reader"
        );
        assert!(
            written.ends_with("vim-nav: new\n"),
            "and the new record landed"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_that_cannot_be_written_is_swallowed() {
        let dir = scratch("unwritable");
        fs::create_dir_all(&dir).expect("scratch directory");
        let blocker = dir.join("frontend.log");
        // A DIRECTORY where the log file should be: opening it for append
        // fails, and the diagnostic must not take the caller down with it.
        fs::create_dir_all(&blocker).expect("blocking directory");
        append_to(&blocker, "info", "vim-nav", "this cannot be written");
        assert!(blocker.is_dir(), "nothing was clobbered either");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_log_lives_under_the_config_directory() {
        let path = frontend_log_path();
        assert!(
            path.ends_with("logs/frontend.log"),
            "got {}",
            path.display()
        );
        assert!(
            path.starts_with(termlab_core::config::config_dir()),
            "the log belongs beside config.toml, got {}",
            path.display()
        );
    }
}
