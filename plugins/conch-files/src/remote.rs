//! Remote filesystem operations via SFTP (queried through conch-ssh RPC).

use std::ffi::CString;

use conch_plugin_sdk::HostApi;
use serde_json::json;

use crate::FileEntry;

/// Query the SSH plugin's `list_dir` service for a remote directory listing.
pub fn list_dir(api: &HostApi, session_id: u64, path: &str) -> Result<Vec<FileEntry>, String> {
    let target = CString::new("SSH Manager").unwrap();
    let method = CString::new("list_dir").unwrap();
    let args = json!({ "session_id": session_id, "path": path });
    let args_str = serde_json::to_string(&args).unwrap();

    let result_ptr = (api.query_plugin)(
        target.as_ptr(),
        method.as_ptr(),
        args_str.as_ptr() as *const _,
        args_str.len(),
    );

    if result_ptr.is_null() {
        return Err("query_plugin returned null".to_string());
    }

    let result_cstr = unsafe { std::ffi::CStr::from_ptr(result_ptr) };
    let result_str = result_cstr.to_string_lossy().to_string();
    (api.free_string)(result_ptr);

    let val: serde_json::Value =
        serde_json::from_str(&result_str).map_err(|e| format!("JSON parse error: {e}"))?;

    if val["status"] != "ok" {
        return Err(
            val["message"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string(),
        );
    }

    let entries_val = val["entries"]
        .as_array()
        .ok_or("missing entries array")?;

    let mut entries: Vec<FileEntry> = entries_val
        .iter()
        .map(|e| FileEntry {
            name: e["name"].as_str().unwrap_or("").to_string(),
            is_dir: e["is_dir"].as_bool().unwrap_or(false),
            size: e["size"].as_u64().unwrap_or(0),
            modified: e["mtime"].as_u64().filter(|&v| v > 0),
        })
        .collect();

    crate::local::sort_entries(&mut entries);
    Ok(entries)
}

/// Create a directory on the remote via SFTP.
pub fn mkdir(api: &HostApi, session_id: u64, path: &str) -> Result<(), String> {
    query_simple(api, session_id, "mkdir", json!({ "session_id": session_id, "path": path }))
}

/// Rename a file or directory on the remote via SFTP.
pub fn rename(api: &HostApi, session_id: u64, from: &str, to: &str) -> Result<(), String> {
    query_simple(api, session_id, "rename", json!({ "session_id": session_id, "from": from, "to": to }))
}

/// Delete a file or directory on the remote via SFTP.
pub fn delete(api: &HostApi, session_id: u64, path: &str, is_dir: bool) -> Result<(), String> {
    query_simple(api, session_id, "delete", json!({ "session_id": session_id, "path": path, "is_dir": is_dir }))
}

fn query_simple(api: &HostApi, _session_id: u64, method: &str, args: serde_json::Value) -> Result<(), String> {
    let target = CString::new("SSH Manager").unwrap();
    let method_c = CString::new(method).unwrap();
    let args_str = serde_json::to_string(&args).unwrap();

    let result_ptr = (api.query_plugin)(
        target.as_ptr(),
        method_c.as_ptr(),
        args_str.as_ptr() as *const _,
        args_str.len(),
    );

    if result_ptr.is_null() {
        return Err("query_plugin returned null".to_string());
    }

    let result_cstr = unsafe { std::ffi::CStr::from_ptr(result_ptr) };
    let result_str = result_cstr.to_string_lossy().to_string();
    (api.free_string)(result_ptr);

    let val: serde_json::Value =
        serde_json::from_str(&result_str).map_err(|e| format!("JSON parse error: {e}"))?;

    if val["status"] != "ok" {
        Err(val["message"].as_str().unwrap_or("unknown error").to_string())
    } else {
        Ok(())
    }
}
