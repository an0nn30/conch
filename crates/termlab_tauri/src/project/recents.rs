//! The recent-projects list and its persistence.
//!
//! Split out of `project/mod.rs` rather than appended to it: this is
//! persistence logic with its own rules (ordering, capping, pruning) and it
//! wants its own tests. The ordering rule is the whole reason it is a pure
//! function — a most-recent-first list that also prunes is exactly the kind of
//! thing that quietly loses an entry.

use serde::Serialize;
use termlab_core::config::{self, RecentProject};

/// The menu would be unusable longer than this, and a longer memory is what
/// "open recent" is not for.
pub(crate) const MAX_RECENTS: usize = 10;

/// Move `path` to the front, refresh its timestamp, prune every OTHER entry
/// whose path no longer exists, and cap the result.
///
/// `exists` is injected so the rule is testable without a filesystem. The
/// path being recorded is never asked about: it was just opened, so it exists
/// by construction, and asking would let a slow or flaky stat delete the very
/// entry this call is adding.
pub(crate) fn record_recent(
    list: &mut Vec<RecentProject>,
    path: &str,
    now_ms: u64,
    exists: impl Fn(&str) -> bool,
) {
    list.retain(|entry| entry.path != path && exists(&entry.path));
    list.insert(
        0,
        RecentProject {
            path: path.to_string(),
            last_opened_ms: now_ms,
        },
    );
    list.truncate(MAX_RECENTS);
}

/// Record `root` as the most recently opened project. Failures to write
/// state.toml are logged, never surfaced: a missing recents entry must not
/// stop a project from opening.
pub(crate) fn remember(root: &str, now_ms: u64) {
    let root = root.to_string();
    if let Err(e) = config::update_persistent_state(|state| {
        record_recent(&mut state.recent_projects, &root, now_ms, |p| {
            std::path::Path::new(p).is_dir()
        });
        true
    }) {
        log::warn!("project: could not record {root} in the recent projects: {e}");
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecentProjectInfo {
    pub path: String,
    pub name: String,
    pub last_opened_ms: u64,
}

/// The recents that still exist on disk, most recent first. Reading does not
/// prune — pruning happens on the next update, so a project on a volume that
/// happens to be unmounted right now comes back rather than being forgotten.
pub(crate) fn list_recents() -> Vec<RecentProjectInfo> {
    let state = config::load_persistent_state().unwrap_or_default();
    state
        .recent_projects
        .iter()
        .filter(|entry| std::path::Path::new(&entry.path).is_dir())
        .map(|entry| RecentProjectInfo {
            path: entry.path.clone(),
            name: super::project_name(std::path::Path::new(&entry.path)),
            last_opened_ms: entry.last_opened_ms,
        })
        .collect()
}

#[tauri::command]
pub(crate) fn project_recents() -> Vec<RecentProjectInfo> {
    list_recents()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_exist(_: &str) -> bool {
        true
    }

    #[test]
    fn recording_moves_a_project_to_the_front_without_duplicating_it() {
        let mut list = Vec::new();
        record_recent(&mut list, "/a", 1, all_exist);
        record_recent(&mut list, "/b", 2, all_exist);
        record_recent(&mut list, "/a", 3, all_exist);
        assert_eq!(
            list.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            vec!["/a", "/b"],
            "most recent first, one entry per project"
        );
        assert_eq!(list[0].last_opened_ms, 3, "the timestamp is refreshed");
    }

    #[test]
    fn the_list_is_capped_at_ten_and_drops_the_oldest() {
        let mut list = Vec::new();
        for i in 0..(MAX_RECENTS + 5) {
            record_recent(&mut list, &format!("/p{i}"), i as u64, all_exist);
        }
        assert_eq!(list.len(), MAX_RECENTS);
        assert_eq!(list[0].path, format!("/p{}", MAX_RECENTS + 4));
        assert!(
            !list.iter().any(|r| r.path == "/p0"),
            "the oldest entries fall off the end"
        );
    }

    #[test]
    fn a_project_whose_path_is_gone_is_pruned_on_the_next_update() {
        let mut list = Vec::new();
        record_recent(&mut list, "/gone", 1, all_exist);
        record_recent(&mut list, "/kept", 2, all_exist);
        record_recent(&mut list, "/new", 3, |p| p != "/gone");
        assert_eq!(
            list.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            vec!["/new", "/kept"],
            "the vanished project is pruned, the rest keeps its order"
        );
    }

    #[test]
    fn the_project_being_recorded_is_never_pruned_by_its_own_update() {
        // The predicate is asked about every OTHER entry; the path being
        // recorded was just opened, so it exists by construction.
        let mut list = Vec::new();
        record_recent(&mut list, "/fresh", 1, |_| false);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].path, "/fresh");
    }

    #[test]
    fn recent_project_info_serializes_as_camel_case() {
        let json = serde_json::to_string(&RecentProjectInfo {
            path: "/repo".into(),
            name: "repo".into(),
            last_opened_ms: 7,
        })
        .expect("serialize");
        assert!(json.contains("\"lastOpenedMs\":7"), "got {json}");
    }
}
