//! The recent-projects list and its persistence.
//!
//! Split out of `project/mod.rs` rather than appended to it: this is
//! persistence logic with its own rules (ordering, capping, pruning) and it
//! wants its own tests. The ordering rule is the whole reason it is a pure
//! function — a most-recent-first list that also prunes is exactly the kind of
//! thing that quietly loses an entry.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use termlab_core::config::{self, LayoutConfig, RecentProject};

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

/// Drop every `project_layouts` entry whose project is no longer in
/// `recents` (fix round 1, F3). Extracted as its own pure function for the
/// same reason `record_recent` is: "list on the left, map on the right, keep
/// them in sync" is exactly the kind of logic that silently drifts once it's
/// inlined into a mutator closure. Without this, a deleted project's full
/// `LayoutConfig` sits in `state.toml` forever, rewritten on every single
/// `remember()` call from then on.
pub(crate) fn prune_layouts_to_recents(
    project_layouts: &mut HashMap<String, LayoutConfig>,
    recents: &[RecentProject],
) {
    let surviving: HashSet<&str> = recents.iter().map(|r| r.path.as_str()).collect();
    project_layouts.retain(|path, _| surviving.contains(path.as_str()));
}

/// Record `root` as the most recently opened project, and prune
/// `project_layouts` to match. Returns whether anything observably changed —
/// `remember`'s mutator uses this to skip the disk write entirely when
/// nothing did (fix round 1, F8; mirrors `save_window_metrics`'s "steady
/// state, no write on every launch" skip).
///
/// The common no-op case: a window that already holds the front-most recent
/// project gets refocused (`project_open`'s `focused_existing` path) with
/// nothing to prune. `record_recent` still bumps that entry's timestamp, but
/// nothing reads `last_opened_ms` for anything other than display, and
/// nothing observable about the persisted STRUCTURE (the ordered path list,
/// the layout key set) changed — so this reports `false` rather than the
/// timestamp-only diff. A genuine reorder, a prune (of either list), or a
/// brand new entry all change the structure and still report `true`.
///
/// Split from `remember` (rather than inlined into its `update_persistent_state`
/// closure) so this decision is unit-testable without any disk I/O.
pub(crate) fn record_recent_changed(
    recent_projects: &mut Vec<RecentProject>,
    project_layouts: &mut HashMap<String, LayoutConfig>,
    root: &str,
    now_ms: u64,
    exists: impl Fn(&str) -> bool,
) -> bool {
    let before_paths: Vec<String> = recent_projects.iter().map(|r| r.path.clone()).collect();
    let before_layout_keys: HashSet<String> = project_layouts.keys().cloned().collect();

    record_recent(recent_projects, root, now_ms, exists);
    prune_layouts_to_recents(project_layouts, recent_projects);

    let after_paths: Vec<String> = recent_projects.iter().map(|r| r.path.clone()).collect();
    let after_layout_keys: HashSet<String> = project_layouts.keys().cloned().collect();

    before_paths != after_paths || before_layout_keys != after_layout_keys
}

/// Record `root` as the most recently opened project. Failures to write
/// state.toml are logged, never surfaced: a missing recents entry must not
/// stop a project from opening.
pub(crate) fn remember(root: &str, now_ms: u64) {
    let root = root.to_string();
    if let Err(e) = config::update_persistent_state(|state| {
        record_recent_changed(
            &mut state.recent_projects,
            &mut state.project_layouts,
            &root,
            now_ms,
            |p| std::path::Path::new(p).is_dir(),
        )
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

/// Every recorded recent, most recent first, exactly as persisted — no
/// filesystem check.
///
/// RULING (fix round 1, F4): this used to stat every path with
/// `Path::is_dir()` to filter out dead entries. `list_recents()` is what
/// BOTH `recent_projects_submenu` (native menu construction, on the main
/// thread, at every app launch AND every `rebuild_menu`) and
/// `project_recents` (the palette) read from — a hung network mount would
/// block app LAUNCH itself. Menu/palette construction must never stat.
///
/// The menu and palette now show entries exactly as recorded; a click on one
/// whose path has since vanished flows straight into `project_open`, which
/// already reports the failure through a toast (see menu-actions.js /
/// command-palette-runtime.js). The validating stat still happens — just
/// moved to `record_recent`, called from `remember()`, which only runs on an
/// actual open action (already off the startup path, and already doing
/// comparable I/O via `canonical_root`) — so a genuinely deleted project is
/// still pruned, just on the NEXT open rather than at read time.
pub(crate) fn list_recents() -> Vec<RecentProjectInfo> {
    recents_info_from(&config::load_persistent_state().unwrap_or_default())
}

/// The pure projection `list_recents` wraps — split out so the "no stat, no
/// filter, straight 1:1 mapping" contract is unit-testable directly, without
/// touching disk.
fn recents_info_from(state: &config::PersistentState) -> Vec<RecentProjectInfo> {
    state
        .recent_projects
        .iter()
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

    // --- F3: project_layouts is pruned to match the surviving recents -----

    #[test]
    fn prune_layouts_to_recents_drops_entries_for_projects_no_longer_recent() {
        let mut layouts = HashMap::new();
        layouts.insert("/kept".to_string(), LayoutConfig::default());
        layouts.insert("/gone".to_string(), LayoutConfig::default());
        let recents = vec![RecentProject {
            path: "/kept".into(),
            last_opened_ms: 1,
        }];
        prune_layouts_to_recents(&mut layouts, &recents);
        assert_eq!(layouts.len(), 1);
        assert!(layouts.contains_key("/kept"));
        assert!(!layouts.contains_key("/gone"));
    }

    #[test]
    fn prune_layouts_to_recents_keeps_every_entry_still_recent() {
        let mut layouts = HashMap::new();
        layouts.insert("/a".to_string(), LayoutConfig::default());
        layouts.insert("/b".to_string(), LayoutConfig::default());
        let recents = vec![
            RecentProject { path: "/a".into(), last_opened_ms: 1 },
            RecentProject { path: "/b".into(), last_opened_ms: 2 },
        ];
        prune_layouts_to_recents(&mut layouts, &recents);
        assert_eq!(layouts.len(), 2);
    }

    // --- record_recent_changed: F3's prune wired in, F8's no-op skip ------

    #[test]
    fn record_recent_changed_reports_true_and_prunes_when_a_stale_layout_exists() {
        let mut list = vec![RecentProject {
            path: "/repo".into(),
            last_opened_ms: 1,
        }];
        let mut layouts = HashMap::new();
        layouts.insert("/stale".to_string(), LayoutConfig::default());

        let changed = record_recent_changed(&mut list, &mut layouts, "/repo", 2, all_exist);

        assert!(changed, "pruning a stale project_layouts entry is a real change");
        assert!(
            !layouts.contains_key("/stale"),
            "the layout for a project no longer in recents disappears on the next remember()"
        );
    }

    #[test]
    fn record_recent_changed_reports_true_when_the_order_actually_changes() {
        let mut list = vec![
            RecentProject { path: "/a".into(), last_opened_ms: 1 },
            RecentProject { path: "/b".into(), last_opened_ms: 2 },
        ];
        let mut layouts = HashMap::new();
        let changed = record_recent_changed(&mut list, &mut layouts, "/b", 3, all_exist);
        assert!(changed, "moving /b to the front is a real, save-worthy change");
    }

    #[test]
    fn record_recent_changed_reports_false_when_already_front_and_nothing_pruned() {
        let mut list = vec![RecentProject {
            path: "/repo".into(),
            last_opened_ms: 1,
        }];
        let mut layouts = HashMap::new();
        layouts.insert("/repo".to_string(), LayoutConfig::default());

        let changed = record_recent_changed(&mut list, &mut layouts, "/repo", 2, all_exist);

        assert!(
            !changed,
            "reopening the already-front project with nothing pruned is a no-op \
             worth skipping — the timestamp bump alone is not worth a disk write"
        );
    }

    // --- F4: reading recents never stats -----------------------------------

    #[test]
    fn recents_info_from_does_not_filter_by_the_filesystem() {
        // (F4 ruling) `recents_info_from` — what both `list_recents` (menu,
        // palette) and the click-time path go through — has no `exists`/
        // `is_dir` parameter to inject, unlike `record_recent` above. A path
        // that plainly doesn't exist on this machine still maps through
        // unfiltered: no stat happens here at all.
        let state = config::PersistentState {
            recent_projects: vec![
                RecentProject { path: "/a".into(), last_opened_ms: 1 },
                RecentProject {
                    path: "/definitely/does/not/exist/anywhere".into(),
                    last_opened_ms: 2,
                },
            ],
            ..config::PersistentState::default()
        };
        let info = recents_info_from(&state);
        assert_eq!(
            info.len(),
            2,
            "every recorded entry maps through, dead path or not — filtering \
             happens at click time via project_open's own error path, not here"
        );
        assert_eq!(info[1].path, "/definitely/does/not/exist/anywhere");
    }
}
