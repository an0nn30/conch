use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;

use async_trait::async_trait;

/// Map a discovered source entry onto its destination path.
/// POSIX remote paths use '/'; local paths use the platform separator.
pub fn map_destination(
    source_root: &str,
    dest_root: &str,
    entry_path: &str,
    remote_dest: bool,
) -> Result<String, String> {
    let src = normalize_root(source_root);
    let dest = normalize_root(dest_root);

    let relative: &str = if entry_path == src {
        ""
    } else {
        let prefix = if src == "/" {
            "/".to_string()
        } else {
            format!("{src}/")
        };
        match entry_path.strip_prefix(prefix.as_str()) {
            Some(rest) => rest,
            None => {
                return Err(format!(
                    "entry path '{entry_path}' is not under source root '{source_root}'"
                ));
            }
        }
    };

    if remote_dest {
        Ok(join_posix(&dest, relative))
    } else {
        let mut path = PathBuf::from(&dest);
        if !relative.is_empty() {
            for segment in relative.split('/') {
                path.push(segment);
            }
        }
        Ok(path.to_string_lossy().into_owned())
    }
}

/// Trim a trailing '/' from a root path, collapsing back to "/" if that
/// leaves nothing (i.e. the root itself was "/").
fn normalize_root(root: &str) -> String {
    let trimmed = root.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn join_posix(dest: &str, relative: &str) -> String {
    if relative.is_empty() {
        dest.to_string()
    } else if dest == "/" {
        format!("/{relative}")
    } else {
        format!("{dest}/{relative}")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum WalkEntry {
    Dir { path: String },
    File { path: String, size: u64 },
    SkippedSymlink { path: String },
}

#[async_trait]
pub trait TreeLister: Send + Sync {
    /// Immediate children of `path`, each tagged dir/file/symlink with size.
    async fn list(&self, path: &str) -> Result<Vec<WalkEntry>, String>;
}

/// Breadth-first walk: emits the root's children downward, parents before
/// children, yielding between directories. Calls `on_entry` for every entry
/// (including empty dirs and skipped symlinks). Stops early if `cancelled()`.
pub async fn walk_tree<F, Fut, C>(
    lister: &dyn TreeLister,
    root: &str,
    mut on_entry: F,
    cancelled: C,
) -> Result<(), String>
where
    F: FnMut(WalkEntry) -> Fut,
    Fut: Future<Output = Result<(), String>>,
    C: Fn() -> bool,
{
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(root.to_string());

    while let Some(dir) = queue.pop_front() {
        if cancelled() {
            return Ok(());
        }

        let children = lister
            .list(&dir)
            .await
            .map_err(|err| format!("{dir}: {err}"))?;

        for entry in children {
            if cancelled() {
                return Ok(());
            }

            if let WalkEntry::Dir { path } = &entry {
                queue.push_back(path.clone());
            }

            on_entry(entry).await?;
        }

        tokio::task::yield_now().await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    struct FakeLister {
        tree: BTreeMap<String, Vec<WalkEntry>>,
        errors: BTreeMap<String, String>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeLister {
        fn new(tree: BTreeMap<String, Vec<WalkEntry>>) -> Self {
            Self {
                tree,
                errors: BTreeMap::new(),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn with_error(mut self, path: &str, message: &str) -> Self {
            self.errors.insert(path.to_string(), message.to_string());
            self
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl TreeLister for FakeLister {
        async fn list(&self, path: &str) -> Result<Vec<WalkEntry>, String> {
            self.calls.lock().unwrap().push(path.to_string());
            if let Some(message) = self.errors.get(path) {
                return Err(message.clone());
            }
            Ok(self.tree.get(path).cloned().unwrap_or_default())
        }
    }

    #[test]
    fn maps_nested_paths_both_separators() {
        // Basic POSIX join.
        assert_eq!(
            map_destination("/a/b", "/home/x", "/a/b/c/d.txt", true).unwrap(),
            "/home/x/c/d.txt"
        );

        // Same relative path, platform join.
        let expected_platform = {
            let mut path = PathBuf::from("/home/x");
            path.push("c");
            path.push("d.txt");
            path.to_string_lossy().into_owned()
        };
        assert_eq!(
            map_destination("/a/b", "/home/x", "/a/b/c/d.txt", false).unwrap(),
            expected_platform
        );

        // The root itself maps straight to dest_root.
        assert_eq!(
            map_destination("/a/b", "/home/x", "/a/b", true).unwrap(),
            "/home/x"
        );

        // Entries outside the source root are rejected.
        assert!(map_destination("/a/b", "/home/x", "/a/c/d.txt", true).is_err());

        // Unicode path segments are preserved verbatim.
        assert_eq!(
            map_destination("/a/b", "/home/x", "/a/b/café/döc.txt", true).unwrap(),
            "/home/x/café/döc.txt"
        );

        // Trailing slashes on both roots are normalized away.
        assert_eq!(
            map_destination("/a/b/", "/home/x/", "/a/b/c/d.txt", true).unwrap(),
            "/home/x/c/d.txt"
        );
    }

    fn three_level_tree() -> BTreeMap<String, Vec<WalkEntry>> {
        let mut tree = BTreeMap::new();
        tree.insert(
            "/root".to_string(),
            vec![
                WalkEntry::Dir {
                    path: "/root/a".into(),
                },
                WalkEntry::File {
                    path: "/root/f0.txt".into(),
                    size: 10,
                },
                WalkEntry::Dir {
                    path: "/root/b".into(),
                },
            ],
        );
        tree.insert(
            "/root/a".to_string(),
            vec![
                WalkEntry::File {
                    path: "/root/a/f1.txt".into(),
                    size: 1,
                },
                WalkEntry::Dir {
                    path: "/root/a/aa".into(),
                },
            ],
        );
        tree.insert(
            "/root/b".to_string(),
            vec![WalkEntry::File {
                path: "/root/b/f2.txt".into(),
                size: 2,
            }],
        );
        tree.insert(
            "/root/a/aa".to_string(),
            vec![WalkEntry::File {
                path: "/root/a/aa/f3.txt".into(),
                size: 3,
            }],
        );
        tree
    }

    #[tokio::test]
    async fn walk_emits_parents_before_children_breadth_first() {
        let lister = FakeLister::new(three_level_tree());
        let emitted: RefCell<Vec<WalkEntry>> = RefCell::new(Vec::new());

        walk_tree(
            &lister,
            "/root",
            |entry| {
                emitted.borrow_mut().push(entry);
                async { Ok(()) }
            },
            || false,
        )
        .await
        .unwrap();

        let emitted = emitted.into_inner();
        assert_eq!(
            emitted,
            vec![
                WalkEntry::Dir {
                    path: "/root/a".into()
                },
                WalkEntry::File {
                    path: "/root/f0.txt".into(),
                    size: 10
                },
                WalkEntry::Dir {
                    path: "/root/b".into()
                },
                WalkEntry::File {
                    path: "/root/a/f1.txt".into(),
                    size: 1
                },
                WalkEntry::Dir {
                    path: "/root/a/aa".into()
                },
                WalkEntry::File {
                    path: "/root/b/f2.txt".into(),
                    size: 2
                },
                WalkEntry::File {
                    path: "/root/a/aa/f3.txt".into(),
                    size: 3
                },
            ]
        );
    }

    #[tokio::test]
    async fn walk_includes_empty_dirs_and_hidden_files() {
        let mut tree = BTreeMap::new();
        tree.insert(
            "/r".to_string(),
            vec![
                WalkEntry::Dir {
                    path: "/r/empty".into(),
                },
                WalkEntry::File {
                    path: "/r/.hidden".into(),
                    size: 5,
                },
            ],
        );
        tree.insert("/r/empty".to_string(), Vec::new());
        let lister = FakeLister::new(tree);
        let emitted: RefCell<Vec<WalkEntry>> = RefCell::new(Vec::new());

        walk_tree(
            &lister,
            "/r",
            |entry| {
                emitted.borrow_mut().push(entry);
                async { Ok(()) }
            },
            || false,
        )
        .await
        .unwrap();

        assert_eq!(
            emitted.into_inner(),
            vec![
                WalkEntry::Dir {
                    path: "/r/empty".into()
                },
                WalkEntry::File {
                    path: "/r/.hidden".into(),
                    size: 5
                },
            ]
        );
        assert_eq!(lister.call_count(), 2, "empty dir must still be listed");
    }

    #[tokio::test]
    async fn walk_skips_symlinks_with_marker() {
        let mut tree = BTreeMap::new();
        tree.insert(
            "/r".to_string(),
            vec![WalkEntry::SkippedSymlink {
                path: "/r/link".into(),
            }],
        );
        let lister = FakeLister::new(tree);
        let emitted: RefCell<Vec<WalkEntry>> = RefCell::new(Vec::new());

        walk_tree(
            &lister,
            "/r",
            |entry| {
                emitted.borrow_mut().push(entry);
                async { Ok(()) }
            },
            || false,
        )
        .await
        .unwrap();

        assert_eq!(
            emitted.into_inner(),
            vec![WalkEntry::SkippedSymlink {
                path: "/r/link".into()
            }]
        );
        assert_eq!(
            lister.call_count(),
            1,
            "a skipped symlink must never be descended into"
        );
    }

    #[tokio::test]
    async fn walk_stops_when_cancelled() {
        let mut tree = BTreeMap::new();
        tree.insert(
            "/r".to_string(),
            vec![
                WalkEntry::File {
                    path: "/r/a.txt".into(),
                    size: 1,
                },
                WalkEntry::File {
                    path: "/r/b.txt".into(),
                    size: 2,
                },
                WalkEntry::Dir {
                    path: "/r/c".into(),
                },
                WalkEntry::File {
                    path: "/r/d.txt".into(),
                    size: 4,
                },
            ],
        );
        tree.insert(
            "/r/c".to_string(),
            vec![WalkEntry::File {
                path: "/r/c/e.txt".into(),
                size: 5,
            }],
        );
        let lister = FakeLister::new(tree);
        let emitted: RefCell<Vec<WalkEntry>> = RefCell::new(Vec::new());
        let seen = RefCell::new(0usize);

        walk_tree(
            &lister,
            "/r",
            |entry| {
                emitted.borrow_mut().push(entry);
                *seen.borrow_mut() += 1;
                async { Ok(()) }
            },
            || *seen.borrow() >= 2,
        )
        .await
        .unwrap();

        assert_eq!(
            emitted.into_inner(),
            vec![
                WalkEntry::File {
                    path: "/r/a.txt".into(),
                    size: 1
                },
                WalkEntry::File {
                    path: "/r/b.txt".into(),
                    size: 2
                },
            ]
        );
        assert_eq!(
            lister.call_count(),
            1,
            "cancellation must stop further lister.list calls"
        );
    }

    #[tokio::test]
    async fn walk_surfaces_lister_error_with_path() {
        let mut tree = BTreeMap::new();
        tree.insert(
            "/r".to_string(),
            vec![
                WalkEntry::Dir {
                    path: "/r/bad".into(),
                },
                WalkEntry::File {
                    path: "/r/ok.txt".into(),
                    size: 1,
                },
            ],
        );
        let lister = FakeLister::new(tree).with_error("/r/bad", "permission denied");
        let emitted: RefCell<Vec<WalkEntry>> = RefCell::new(Vec::new());

        let result = walk_tree(
            &lister,
            "/r",
            |entry| {
                emitted.borrow_mut().push(entry);
                async { Ok(()) }
            },
            || false,
        )
        .await;

        let err = result.expect_err("subdirectory listing failure must propagate");
        assert!(
            err.contains("/r/bad"),
            "error must mention the failing path: {err}"
        );
        assert_eq!(
            emitted.into_inner(),
            vec![
                WalkEntry::Dir {
                    path: "/r/bad".into()
                },
                WalkEntry::File {
                    path: "/r/ok.txt".into(),
                    size: 1
                },
            ],
            "entries discovered before the failure must already be emitted"
        );
    }
}
