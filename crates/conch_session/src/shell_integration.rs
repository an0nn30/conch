/// Parse OSC 7 (current working directory) from terminal output.
///
/// Format: `\x1b]7;file://hostname/path\x07` or `\x1b]7;file://hostname/path\x1b\\`
pub fn parse_osc7(payload: &str) -> Option<String> {
    let payload = payload.strip_prefix("7;").or_else(|| {
        // Some terminals send just the URI after the OSC number
        if payload.starts_with("file://") {
            Some(payload)
        } else {
            None
        }
    })?;

    // Strip file:// prefix
    let rest = payload.strip_prefix("file://")?;

    // Skip hostname (everything up to the next /)
    let path_start = rest.find('/')?;
    let path = &rest[path_start..];

    // URL-decode the path
    Some(url_decode(path))
}

fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().and_then(|c| hex_val(c));
            let lo = chars.next().and_then(|c| hex_val(c));
            if let (Some(h), Some(l)) = (hi, lo) {
                result.push((h << 4 | l) as char);
            }
        } else {
            result.push(b as char);
        }
    }
    result
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Scan raw terminal bytes for OSC 7 sequences, return the last path found.
///
/// This bypasses alacritty_terminal's VTE parser (which drops OSC 7) by
/// scanning the raw SSH output before it reaches the terminal.
pub fn extract_osc7_from_bytes(data: &[u8]) -> Option<String> {
    process_osc7_from_bytes(data, false).0
}

/// Scan raw terminal bytes for OSC 7 sequences, extract the last path found,
/// and return a copy of the data with all OSC 7 sequences stripped out.
///
/// This matches the Java Conch app's `OscParser` architecture: OSC 7 sequences
/// are intercepted and removed from the byte stream before reaching the terminal
/// emulator. This prevents any VTE side-effects from the unhandled OSC 7
/// (which can interfere with zsh's PROMPT_SP cursor-position detection).
pub fn extract_and_strip_osc7(data: &[u8]) -> (Option<String>, Vec<u8>) {
    let result = process_osc7_from_bytes(data, true);
    (result.0, result.1.unwrap_or_else(|| data.to_vec()))
}

/// Internal: scan for OSC 7 sequences, optionally building a stripped copy.
fn process_osc7_from_bytes(data: &[u8], strip: bool) -> (Option<String>, Option<Vec<u8>>) {
    let marker = b"\x1b]7;";
    let mut result = None;
    let mut cleaned: Option<Vec<u8>> = if strip { Some(Vec::with_capacity(data.len())) } else { None };
    let mut search_from = 0;
    let mut copy_from = 0; // for stripping: start of uncopied region

    loop {
        let Some(offset) = data[search_from..]
            .windows(marker.len())
            .position(|w| w == marker)
        else {
            break;
        };
        let seq_start = search_from + offset;
        let payload_start = seq_start + marker.len();
        // Find BEL (0x07) or ST (ESC \) terminator
        let mut end = payload_start;
        let mut found = false;
        while end < data.len() {
            if data[end] == 0x07 {
                found = true;
                break;
            }
            if data[end] == 0x1b && data.get(end + 1) == Some(&b'\\') {
                found = true;
                break;
            }
            end += 1;
        }
        if found {
            if let Ok(payload) = std::str::from_utf8(&data[payload_start..end]) {
                if let Some(path) = parse_osc7(payload) {
                    result = Some(path);
                }
            }
            // Determine where the sequence ends (past the terminator)
            let seq_end = if data[end] == 0x07 {
                end + 1
            } else {
                end + 2 // ESC \ is two bytes
            };
            // Copy the non-OSC-7 bytes before this sequence
            if let Some(ref mut buf) = cleaned {
                buf.extend_from_slice(&data[copy_from..seq_start]);
                copy_from = seq_end;
            }
            search_from = seq_end;
        } else {
            break;
        }
    }
    // Copy remaining bytes after the last OSC 7 sequence
    if let Some(ref mut buf) = cleaned {
        buf.extend_from_slice(&data[copy_from..]);
    }
    (result, cleaned)
}

/// Generate the PROMPT_COMMAND snippet for bash that emits OSC 7.
pub fn bash_osc7_prompt_command() -> &'static str {
    r#"__conch_osc7() { printf '\e]7;file://%s%s\a' "$(hostname)" "$(pwd)"; }; PROMPT_COMMAND="__conch_osc7${PROMPT_COMMAND:+;$PROMPT_COMMAND}""#
}

/// Generate the precmd hook for zsh that emits OSC 7.
pub fn zsh_osc7_precmd() -> &'static str {
    r#"__conch_osc7() { printf '\e]7;file://%s%s\a' "$(hostname)" "$(pwd)"; }; precmd_functions+=(__conch_osc7)"#
}

/// Phase 2 of SSH shell integration injection (sent after `stty -echo` takes effect).
///
/// Matches the Java Conch app's injection exactly:
/// - Defines `__conch_osc7` that emits OSC 7 to stdout (no /dev/tty redirect)
/// - Bash: hooks via `PROMPT_COMMAND`
/// - Zsh: hooks via `precmd_functions`
///
/// The OSC 7 bytes are stripped from the terminal byte stream by
/// `extract_and_strip_osc7()` in the SSH bridge (matching Java's OscParser
/// architecture), so the terminal emulator never sees them.
pub fn ssh_osc7_injection() -> &'static str {
    concat!(
        r#"__conch_osc7(){ printf '\033]7;file://%s\007' "$PWD"; };"#,
        r#" [ -n "$BASH_VERSION" ] && PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND;}__conch_osc7";"#,
        r#" [ -n "$ZSH_VERSION" ] && precmd_functions+=(__conch_osc7);"#,
        r#" __conch_osc7;"#,
        // Re-enable echo, then erase the visible stty -echo line + prompt
        r#" stty echo; printf '\033[2K\033[1A\033[2K\r'"#,
        "\n",
    )
}

/// Get the current working directory of a process by PID (macOS only).
///
/// Uses `proc_pidinfo` with `PROC_PIDVNODEPATHINFO` to read the CWD
/// without shelling out.
#[cfg(target_os = "macos")]
pub fn get_process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    use std::ffi::CStr;
    use std::path::PathBuf;

    const PROC_PIDVNODEPATHINFO: i32 = 9;
    // sizeof(proc_vnodepathinfo) = 2 * sizeof(vnode_info_path)
    // vnode_info_path = vnode_info(152) + path(1024) = 1176
    // proc_vnodepathinfo = 2 * 1176 = 2352
    const BUF_SIZE: usize = 2352;
    // Offset of pvi_cdir.vip_path within proc_vnodepathinfo:
    // vinfo_stat(136) + vi_type(4) + vi_pad(4) + vi_fsid(8) = 152
    const CWD_PATH_OFFSET: usize = 152;

    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut u8,
            buffersize: i32,
        ) -> i32;
    }

    let mut buf = vec![0u8; BUF_SIZE];
    let ret = unsafe {
        proc_pidinfo(
            pid as i32,
            PROC_PIDVNODEPATHINFO,
            0,
            buf.as_mut_ptr(),
            BUF_SIZE as i32,
        )
    };
    if ret <= 0 {
        return None;
    }
    let path_bytes = &buf[CWD_PATH_OFFSET..];
    let c_str = unsafe { CStr::from_ptr(path_bytes.as_ptr() as *const std::ffi::c_char) };
    let path = PathBuf::from(c_str.to_string_lossy().into_owned());
    if path.as_os_str().is_empty() {
        None
    } else {
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_osc7() {
        let payload = "file://myhost/Users/dustin/projects";
        assert_eq!(
            parse_osc7(payload),
            Some("/Users/dustin/projects".to_string())
        );
    }

    #[test]
    fn test_parse_osc7_with_prefix() {
        let payload = "7;file://myhost/tmp/hello%20world";
        assert_eq!(
            parse_osc7(payload),
            Some("/tmp/hello world".to_string())
        );
    }

    #[test]
    fn test_extract_osc7_bel_terminator() {
        let data = b"some output\x1b]7;file://host/tmp\x07more output";
        assert_eq!(
            extract_osc7_from_bytes(data),
            Some("/tmp".to_string())
        );
    }

    #[test]
    fn test_extract_osc7_st_terminator() {
        let data = b"\x1b]7;file://host/home/user\x1b\\trailing";
        assert_eq!(
            extract_osc7_from_bytes(data),
            Some("/home/user".to_string())
        );
    }

    #[test]
    fn test_extract_osc7_returns_last() {
        let data = b"\x1b]7;file://h/first\x07middle\x1b]7;file://h/second\x07end";
        assert_eq!(
            extract_osc7_from_bytes(data),
            Some("/second".to_string())
        );
    }

    #[test]
    fn test_extract_osc7_no_match() {
        assert_eq!(extract_osc7_from_bytes(b"plain text"), None);
    }

    #[test]
    fn test_strip_osc7_removes_sequences() {
        let data = b"before\x1b]7;file://h/tmp\x07after";
        let (path, cleaned) = extract_and_strip_osc7(data);
        assert_eq!(path, Some("/tmp".to_string()));
        assert_eq!(cleaned, b"beforeafter");
    }

    #[test]
    fn test_strip_osc7_multiple() {
        let data = b"a\x1b]7;file://h/first\x07b\x1b]7;file://h/second\x07c";
        let (path, cleaned) = extract_and_strip_osc7(data);
        assert_eq!(path, Some("/second".to_string()));
        assert_eq!(cleaned, b"abc");
    }

    #[test]
    fn test_strip_osc7_no_match_passthrough() {
        let data = b"plain text";
        let (path, cleaned) = extract_and_strip_osc7(data);
        assert_eq!(path, None);
        assert_eq!(cleaned, b"plain text");
    }

    #[test]
    fn test_strip_osc7_st_terminator() {
        let data = b"X\x1b]7;file://h/path\x1b\\Y";
        let (path, cleaned) = extract_and_strip_osc7(data);
        assert_eq!(path, Some("/path".to_string()));
        assert_eq!(cleaned, b"XY");
    }
}
