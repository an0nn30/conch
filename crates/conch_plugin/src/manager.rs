use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Metadata parsed from a Lua plugin header.
#[derive(Debug, Clone)]
pub struct PluginMeta {
    pub name: String,
    pub description: String,
    pub version: String,
    pub path: PathBuf,
}

/// Discover plugins in the given directory.
pub fn discover_plugins(dir: &Path) -> Result<Vec<PluginMeta>> {
    let mut plugins = Vec::new();

    if !dir.exists() {
        return Ok(plugins);
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "lua") {
            if let Ok(meta) = parse_plugin_header(&path) {
                plugins.push(meta);
            }
        }
    }

    Ok(plugins)
}

fn parse_plugin_header(path: &Path) -> Result<PluginMeta> {
    let contents = fs::read_to_string(path)?;
    let mut name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let mut description = String::new();
    let mut version = String::from("0.0.0");

    for line in contents.lines() {
        let line = line.trim();
        if !line.starts_with("--") {
            break;
        }
        let comment = line.trim_start_matches('-').trim();
        if let Some(val) = comment.strip_prefix("plugin-name:") {
            name = val.trim().to_string();
        } else if let Some(val) = comment.strip_prefix("plugin-description:") {
            description = val.trim().to_string();
        } else if let Some(val) = comment.strip_prefix("plugin-version:") {
            version = val.trim().to_string();
        }
    }

    Ok(PluginMeta {
        name,
        description,
        version,
        path: path.to_path_buf(),
    })
}
