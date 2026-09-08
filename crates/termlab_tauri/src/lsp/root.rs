use std::fs;
use std::io::{self, Read};
use std::path::Path;

use super::catalog::{BundledServerCatalog, RootStrategy};
use super::types::ProjectCandidate;

const MAX_MARKER_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LanguageId {
    JavaScript,
    TypeScript,
    Json,
    Python,
    Rust,
    Go,
    C,
    Cpp,
    Java,
}

#[derive(Debug)]
struct RankedCandidate {
    candidate: ProjectCandidate,
    distance: usize,
    priority: u8,
}

impl RankedCandidate {
    fn new(
        path: &Path,
        marker: impl Into<String>,
        reason: impl Into<String>,
        confidence: u8,
        is_fallback: bool,
        distance: usize,
        priority: u8,
    ) -> Self {
        let canonical_path = path.display().to_string();
        let display_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(&canonical_path)
            .to_owned();
        Self {
            candidate: ProjectCandidate {
                root_uri: file_uri(path),
                canonical_path,
                display_name,
                marker: marker.into(),
                reason: reason.into(),
                confidence,
                is_fallback,
            },
            distance,
            priority,
        }
    }
}

/// Discovers plausible project roots without choosing one or inspecting project contents.
///
/// The caller owns when discovery runs and whether any returned root is trusted; malformed
/// marker files are retained as lower-confidence evidence instead of becoming errors.
pub(crate) fn discover_project_roots(
    file: &Path,
    language: LanguageId,
) -> io::Result<Vec<ProjectCandidate>> {
    let strategy = BundledServerCatalog::new()
        .descriptor(language)
        .root_strategy;
    discover_project_roots_for_strategy(file, strategy)
}

/// Descriptor-driven root discovery. The future manager supplies an adapter
/// descriptor's strategy instead of branching on raw language names.
pub(crate) fn discover_project_roots_for_strategy(
    file: &Path,
    strategy: RootStrategy,
) -> io::Result<Vec<ProjectCandidate>> {
    let canonical_file = fs::canonicalize(file)?;
    let parent = canonical_file.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "a project root cannot be discovered for a volume root",
        )
    })?;

    let mut candidates = Vec::new();
    let mut directory = Some(parent);
    let mut distance = 0;
    while let Some(current) = directory {
        if distance == 0 {
            insert_candidate(
                &mut candidates,
                RankedCandidate::new(
                    current,
                    "This folder",
                    "Use this file's parent folder",
                    20,
                    true,
                    distance,
                    0,
                ),
            );
        }

        for candidate in marker_candidates(current, strategy, distance) {
            insert_candidate(&mut candidates, candidate);
        }

        directory = current.parent();
        distance += 1;
    }

    if strategy == RootStrategy::Json
        && candidates
            .iter()
            .any(|candidate| is_javascript_context_marker(&candidate.candidate.marker))
    {
        candidates.retain(|candidate| candidate.candidate.marker != ".git");
    }

    candidates.sort_by(|left, right| {
        let left_go_workspace = strategy == RootStrategy::Go && left.candidate.marker == "go.work";
        let right_go_workspace =
            strategy == RootStrategy::Go && right.candidate.marker == "go.work";
        left.candidate
            .is_fallback
            .cmp(&right.candidate.is_fallback)
            .then_with(|| right_go_workspace.cmp(&left_go_workspace))
            .then_with(|| left.distance.cmp(&right.distance))
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| left.candidate.marker.cmp(&right.candidate.marker))
    });

    Ok(candidates
        .into_iter()
        .map(|ranked| ranked.candidate)
        .collect())
}

fn is_javascript_context_marker(marker: &str) -> bool {
    matches!(
        marker,
        "tsconfig.json"
            | "jsconfig.json"
            | "package.json"
            | "package.json (workspace)"
            | "package.json (unreadable)"
            | "pnpm-workspace.yaml"
            | "lerna.json"
            | "nx.json"
    )
}

fn insert_candidate(candidates: &mut Vec<RankedCandidate>, next: RankedCandidate) {
    if next.candidate.is_fallback {
        candidates.push(next);
        return;
    }

    if let Some(existing) = candidates.iter_mut().find(|existing| {
        !existing.candidate.is_fallback
            && existing.candidate.canonical_path == next.candidate.canonical_path
    }) {
        if next.priority > existing.priority {
            *existing = next;
        }
    } else {
        candidates.push(next);
    }
}

fn marker_candidates(
    directory: &Path,
    strategy: RootStrategy,
    distance: usize,
) -> Vec<RankedCandidate> {
    match strategy {
        RootStrategy::JavaScript => javascript_candidates(directory, distance),
        RootStrategy::Json => {
            let javascript = javascript_candidates(directory, distance);
            if javascript.is_empty() {
                repository_candidates(directory, distance)
            } else {
                javascript
            }
        }
        RootStrategy::Python => python_candidates(directory, distance),
        RootStrategy::Rust => rust_candidates(directory, distance),
        RootStrategy::Go => go_candidates(directory, distance),
        RootStrategy::Clangd => c_cpp_candidates(directory, distance),
        RootStrategy::Java => java_candidates(directory, distance),
    }
}

fn javascript_candidates(directory: &Path, distance: usize) -> Vec<RankedCandidate> {
    let mut candidates = Vec::new();
    add_simple_marker(
        &mut candidates,
        directory,
        "tsconfig.json",
        "TypeScript project configuration",
        100,
        distance,
        100,
    );
    add_simple_marker(
        &mut candidates,
        directory,
        "jsconfig.json",
        "JavaScript project configuration",
        100,
        distance,
        99,
    );
    add_package_json_marker(&mut candidates, directory, distance);
    add_simple_marker(
        &mut candidates,
        directory,
        "pnpm-workspace.yaml",
        "pnpm workspace root",
        100,
        distance,
        105,
    );
    add_simple_marker(
        &mut candidates,
        directory,
        "lerna.json",
        "Lerna workspace root",
        100,
        distance,
        104,
    );
    add_simple_marker(
        &mut candidates,
        directory,
        "nx.json",
        "Nx workspace root",
        100,
        distance,
        103,
    );
    candidates
}

fn python_candidates(directory: &Path, distance: usize) -> Vec<RankedCandidate> {
    let mut candidates = Vec::new();
    for (marker, reason, priority) in [
        ("pyproject.toml", "Python project configuration", 100),
        ("setup.cfg", "Python setup configuration", 95),
        ("setup.py", "Python package root", 94),
        ("tox.ini", "Python tox configuration", 93),
        ("Pipfile", "Python Pipenv root", 92),
        ("poetry.lock", "Python Poetry root", 91),
        ("uv.lock", "Python uv root", 90),
    ] {
        add_simple_marker(
            &mut candidates,
            directory,
            marker,
            reason,
            100,
            distance,
            priority,
        );
    }
    candidates.extend(repository_candidates(directory, distance));
    candidates
}

fn rust_candidates(directory: &Path, distance: usize) -> Vec<RankedCandidate> {
    let path = directory.join("Cargo.toml");
    let Some(contents) = read_marker(&path) else {
        return Vec::new();
    };
    let (marker, reason, confidence, priority) = match contents {
        Ok(contents) => match contents.parse::<toml::Value>() {
            Ok(value) if value.get("package").is_some() => {
                ("Cargo.toml (package)", "Rust crate manifest", 100, 105)
            }
            Ok(value) if value.get("workspace").is_some() => (
                "Cargo.toml (workspace)",
                "Rust workspace manifest",
                100,
                100,
            ),
            Ok(_) => (
                "Cargo.toml (unrecognized)",
                "Rust manifest has no package or workspace table",
                45,
                45,
            ),
            Err(_) => (
                "Cargo.toml (unreadable)",
                "Rust manifest could not be parsed",
                40,
                40,
            ),
        },
        Err(()) => (
            "Cargo.toml (unreadable)",
            "Rust manifest could not be parsed",
            40,
            40,
        ),
    };
    vec![RankedCandidate::new(
        directory, marker, reason, confidence, false, distance, priority,
    )]
}

fn go_candidates(directory: &Path, distance: usize) -> Vec<RankedCandidate> {
    let mut candidates = Vec::new();
    add_simple_marker(
        &mut candidates,
        directory,
        "go.work",
        "Go workspace",
        100,
        distance,
        100,
    );
    add_simple_marker(
        &mut candidates,
        directory,
        "go.mod",
        "Go module",
        95,
        distance,
        95,
    );
    candidates
}

fn c_cpp_candidates(directory: &Path, distance: usize) -> Vec<RankedCandidate> {
    let mut candidates = Vec::new();
    add_simple_marker(
        &mut candidates,
        directory,
        "compile_commands.json",
        "C/C++ compilation database",
        100,
        distance,
        100,
    );
    add_simple_marker(
        &mut candidates,
        directory,
        ".clangd",
        "clangd project configuration",
        100,
        distance,
        99,
    );
    add_simple_marker(
        &mut candidates,
        directory,
        "CMakeLists.txt",
        "CMake project root",
        100,
        distance,
        98,
    );
    candidates.extend(repository_candidates(directory, distance));
    candidates
}

fn java_candidates(directory: &Path, distance: usize) -> Vec<RankedCandidate> {
    let mut candidates = Vec::new();
    add_simple_marker(
        &mut candidates,
        directory,
        "pom.xml",
        "Maven project root",
        100,
        distance,
        100,
    );
    add_simple_marker(
        &mut candidates,
        directory,
        "settings.gradle",
        "Gradle workspace root",
        100,
        distance,
        99,
    );
    add_simple_marker(
        &mut candidates,
        directory,
        "settings.gradle.kts",
        "Gradle workspace root",
        100,
        distance,
        99,
    );
    add_simple_marker(
        &mut candidates,
        directory,
        "build.gradle",
        "Gradle project root",
        95,
        distance,
        98,
    );
    add_simple_marker(
        &mut candidates,
        directory,
        "build.gradle.kts",
        "Gradle project root",
        95,
        distance,
        98,
    );
    add_simple_marker(
        &mut candidates,
        directory,
        ".project",
        "Eclipse project root",
        95,
        distance,
        97,
    );
    candidates.extend(repository_candidates(directory, distance));
    candidates
}

fn repository_candidates(directory: &Path, distance: usize) -> Vec<RankedCandidate> {
    if directory.join(".git").exists() {
        vec![RankedCandidate::new(
            directory,
            ".git",
            "Repository root",
            70,
            false,
            distance,
            70,
        )]
    } else {
        Vec::new()
    }
}

fn add_package_json_marker(
    candidates: &mut Vec<RankedCandidate>,
    directory: &Path,
    distance: usize,
) {
    let path = directory.join("package.json");
    let Some(contents) = read_marker(&path) else {
        return;
    };
    let (marker, reason, confidence, priority) = match contents {
        Ok(contents) => match serde_json::from_str::<serde_json::Value>(&contents) {
            Ok(value) if value.get("workspaces").is_some() => (
                "package.json (workspace)",
                "JavaScript workspace root",
                100,
                102,
            ),
            Ok(_) => ("package.json", "JavaScript package root", 95, 95),
            Err(_) => (
                "package.json (unreadable)",
                "JavaScript package marker could not be parsed",
                40,
                40,
            ),
        },
        Err(()) => (
            "package.json (unreadable)",
            "JavaScript package marker could not be parsed",
            40,
            40,
        ),
    };
    candidates.push(RankedCandidate::new(
        directory, marker, reason, confidence, false, distance, priority,
    ));
}

fn add_simple_marker(
    candidates: &mut Vec<RankedCandidate>,
    directory: &Path,
    marker: &str,
    reason: &str,
    confidence: u8,
    distance: usize,
    priority: u8,
) {
    match read_marker(&directory.join(marker)) {
        Some(Ok(_)) => candidates.push(RankedCandidate::new(
            directory, marker, reason, confidence, false, distance, priority,
        )),
        Some(Err(())) => candidates.push(RankedCandidate::new(
            directory,
            format!("{marker} (unreadable)"),
            format!("{reason} could not be read"),
            40,
            false,
            distance,
            40,
        )),
        None => {}
    }
}

fn read_marker(path: &Path) -> Option<Result<String, ()>> {
    if !path.is_file() {
        return None;
    }
    let mut contents = Vec::new();
    let result = fs::File::open(path)
        .and_then(|mut file| {
            file.by_ref()
                .take(MAX_MARKER_BYTES + 1)
                .read_to_end(&mut contents)
        })
        .map_err(|_| ());
    match result {
        Ok(count) if count <= MAX_MARKER_BYTES as usize => {
            Some(String::from_utf8(contents).map_err(|_| ()))
        }
        Ok(_) | Err(()) => Some(Err(())),
    }
}

fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            uri.push(byte as char);
        } else {
            use std::fmt::Write;
            write!(&mut uri, "%{byte:02X}").expect("writing to a string cannot fail");
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::{LanguageId, discover_project_roots};

    fn write(root: &Path, relative: &str, contents: &str) -> PathBuf {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("fixture file parent"))
            .expect("create fixture parent");
        fs::write(&path, contents).expect("write fixture file");
        path
    }

    fn write_bytes(root: &Path, relative: &str, contents: &[u8]) -> PathBuf {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("fixture file parent"))
            .expect("create fixture parent");
        fs::write(&path, contents).expect("write fixture file");
        path
    }

    fn candidates(file: &Path, language: LanguageId) -> Vec<(String, String, u8, bool)> {
        discover_project_roots(file, language)
            .expect("discover roots")
            .into_iter()
            .map(|candidate| {
                (
                    candidate.marker,
                    candidate.reason,
                    candidate.confidence,
                    candidate.is_fallback,
                )
            })
            .collect()
    }

    #[test]
    fn rust_returns_crate_and_workspace_candidates() {
        let temp = TempDir::new().expect("temporary fixture");
        let root = temp.path();
        write(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/widget\"]\n",
        );
        let file = write(
            root,
            "crates/widget/lib.rs",
            "pub fn answer() -> u32 { 42 }\n",
        );
        write(
            root,
            "crates/widget/Cargo.toml",
            "[package]\nname = \"widget\"\nversion = \"0.1.0\"\n",
        );

        assert_eq!(
            candidates(&file, LanguageId::Rust),
            vec![
                (
                    "Cargo.toml (package)".into(),
                    "Rust crate manifest".into(),
                    100,
                    false,
                ),
                (
                    "Cargo.toml (workspace)".into(),
                    "Rust workspace manifest".into(),
                    100,
                    false,
                ),
                (
                    "This folder".into(),
                    "Use this file's parent folder".into(),
                    20,
                    true,
                ),
            ]
        );
    }

    #[test]
    fn typescript_keeps_nested_configuration_package_and_workspace_roots() {
        let temp = TempDir::new().expect("temporary fixture");
        let root = temp.path();
        write(
            root,
            "package.json",
            "{\"private\": true, \"workspaces\": [\"apps/*\"]}",
        );
        write(root, "pnpm-workspace.yaml", "packages:\n  - apps/*\n");
        write(root, "apps/package.json", "{\"name\": \"apps\"}");
        write(
            root,
            "apps/web/tsconfig.json",
            "{\"compilerOptions\": {}}\n",
        );
        let file = write(root, "apps/web/src/main.ts", "export const answer = 42;\n");

        assert_eq!(
            candidates(&file, LanguageId::TypeScript),
            vec![
                (
                    "tsconfig.json".into(),
                    "TypeScript project configuration".into(),
                    100,
                    false,
                ),
                (
                    "package.json".into(),
                    "JavaScript package root".into(),
                    95,
                    false,
                ),
                (
                    "pnpm-workspace.yaml".into(),
                    "pnpm workspace root".into(),
                    100,
                    false,
                ),
                (
                    "This folder".into(),
                    "Use this file's parent folder".into(),
                    20,
                    true,
                ),
            ]
        );
    }

    #[test]
    fn language_markers_return_literal_orders_and_reasons() {
        struct Case {
            language: LanguageId,
            file: &'static str,
            markers: &'static [(&'static str, &'static str)],
            expected: &'static [(&'static str, &'static str)],
        }

        let cases = [
            Case {
                language: LanguageId::Python,
                file: "python/src/tool.py",
                markers: &[("python/pyproject.toml", "[project]\nname = \"tool\"\n")],
                expected: &[("pyproject.toml", "Python project configuration")],
            },
            Case {
                language: LanguageId::Go,
                file: "go/module/main.go",
                markers: &[
                    ("go/go.work", "go 1.22\nuse ./module\n"),
                    ("go/module/go.mod", "module example.com/module\n"),
                ],
                expected: &[("go.work", "Go workspace"), ("go.mod", "Go module")],
            },
            Case {
                language: LanguageId::Cpp,
                file: "cpp/sub/main.cpp",
                markers: &[
                    ("cpp/compile_commands.json", "[]\n"),
                    ("cpp/.git/HEAD", "ref: refs/heads/main\n"),
                    (
                        "cpp/sub/CMakeLists.txt",
                        "cmake_minimum_required(VERSION 3.20)\n",
                    ),
                ],
                expected: &[
                    ("CMakeLists.txt", "CMake project root"),
                    ("compile_commands.json", "C/C++ compilation database"),
                ],
            },
            Case {
                language: LanguageId::Java,
                file: "java/module/Main.java",
                markers: &[
                    ("java/settings.gradle", "rootProject.name = 'java'\n"),
                    ("java/module/pom.xml", "<project/>\n"),
                ],
                expected: &[
                    ("pom.xml", "Maven project root"),
                    ("settings.gradle", "Gradle workspace root"),
                ],
            },
            Case {
                language: LanguageId::Json,
                file: "json/config/settings.json",
                markers: &[
                    ("json/tsconfig.json", "{\"compilerOptions\": {}}\n"),
                    (".git/HEAD", "ref: refs/heads/main\n"),
                ],
                expected: &[("tsconfig.json", "TypeScript project configuration")],
            },
        ];

        for case in cases {
            let temp = TempDir::new().expect("temporary fixture");
            for (path, contents) in case.markers {
                write(temp.path(), path, contents);
            }
            let file = write(temp.path(), case.file, "fixture\n");

            let roots = candidates(&file, case.language);
            assert_eq!(
                roots
                    .iter()
                    .filter(|(_, _, _, is_fallback)| !is_fallback)
                    .map(|(marker, reason, _, _)| (marker.as_str(), reason.as_str()))
                    .collect::<Vec<_>>(),
                case.expected,
                "{}",
                case.file,
            );
        }
    }

    #[test]
    fn canonicalizes_symlinked_paths_and_deduplicates_roots() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let temp = TempDir::new().expect("temporary fixture");
            let real = temp.path().join("real-project");
            fs::create_dir_all(real.join("src")).expect("create real project");
            write(&real, "package.json", "{\"name\": \"real-project\"}");
            let file = write(&real, "src/main.js", "export {};\n");
            let alias = temp.path().join("alias");
            symlink(&real, &alias).expect("create project symlink");

            let roots = discover_project_roots(&alias.join("src/main.js"), LanguageId::JavaScript)
                .expect("discover roots");
            assert_eq!(roots.len(), 2);
            assert_eq!(
                roots[0].canonical_path,
                real.canonicalize().unwrap().display().to_string()
            );
            assert_eq!(roots[0].marker, "package.json");
            assert_eq!(
                roots[1].canonical_path,
                file.parent()
                    .unwrap()
                    .canonicalize()
                    .unwrap()
                    .display()
                    .to_string()
            );
            assert_eq!(roots[1].marker, "This folder");
        }
    }

    #[test]
    fn malformed_marker_remains_a_low_confidence_candidate() {
        let temp = TempDir::new().expect("temporary fixture");
        let file = write(temp.path(), "project/main.ts", "export {};\n");
        write(temp.path(), "project/package.json", "{ not valid json");

        assert_eq!(
            candidates(&file, LanguageId::TypeScript),
            vec![
                (
                    "package.json (unreadable)".into(),
                    "JavaScript package marker could not be parsed".into(),
                    40,
                    false,
                ),
                (
                    "This folder".into(),
                    "Use this file's parent folder".into(),
                    20,
                    true,
                )
            ]
        );
    }

    #[test]
    fn marked_parent_keeps_a_distinct_this_folder_fallback() {
        let temp = TempDir::new().expect("temporary fixture");
        let file = write(temp.path(), "project/main.ts", "export {};\n");
        write(
            temp.path(),
            "project/tsconfig.json",
            "{\"compilerOptions\": {}}\n",
        );

        assert_eq!(
            candidates(&file, LanguageId::TypeScript),
            vec![
                (
                    "tsconfig.json".into(),
                    "TypeScript project configuration".into(),
                    100,
                    false,
                ),
                (
                    "This folder".into(),
                    "Use this file's parent folder".into(),
                    20,
                    true,
                ),
            ]
        );
    }

    #[test]
    fn unreadable_simple_markers_lower_confidence_without_aborting_discovery() {
        let cases = [
            ("oversized", vec![b'x'; 65_537]),
            ("non_utf8", vec![0xff, 0xfe, 0xfd]),
        ];

        for (name, contents) in cases {
            let temp = TempDir::new().expect("temporary fixture");
            let file = write(temp.path(), "project/main.ts", "export {};\n");
            write_bytes(temp.path(), "project/tsconfig.json", &contents);

            assert_eq!(
                candidates(&file, LanguageId::TypeScript),
                vec![
                    (
                        "tsconfig.json (unreadable)".into(),
                        "TypeScript project configuration could not be read".into(),
                        40,
                        false,
                    ),
                    (
                        "This folder".into(),
                        "Use this file's parent folder".into(),
                        20,
                        true,
                    ),
                ],
                "{name}",
            );
        }
    }

    #[test]
    fn loose_file_keeps_its_parent_as_the_explicit_fallback() {
        let temp = TempDir::new().expect("temporary fixture");
        let file = write(temp.path(), "notes/todo.txt", "remember this\n");
        let parent = file.parent().unwrap().canonicalize().unwrap();

        let roots = discover_project_roots(&file, LanguageId::Json).expect("discover roots");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].canonical_path, parent.display().to_string());
        assert_eq!(roots[0].marker, "This folder");
        assert_eq!(roots[0].reason, "Use this file's parent folder");
        assert_eq!(roots[0].confidence, 20);
        assert!(roots[0].is_fallback);
    }
}
