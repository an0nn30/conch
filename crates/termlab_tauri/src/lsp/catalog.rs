use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::root::LanguageId;
use super::types::LspUnavailableReason;

const ARM64: u32 = 0x0100_000c;
const X86_64: u32 = 0x0100_0007;
const MACHO_64_LE: u32 = 0xfeed_facf;
const LC_BUILD_VERSION: u32 = 0x32;
const PLATFORM_MACOS: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PocAvailability {
    Bundled,
    NotBundledYet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PackagedMetadata {
    pub version: &'static str,
    pub upstream_url: &'static str,
    pub license: &'static str,
    pub notices_file: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandArgument {
    Literal(&'static str),
    ResourcePath(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgramLayout {
    Node {
        executable_relative_path: &'static str,
        arguments: &'static [CommandArgument],
        required_files: &'static [&'static str],
    },
    Native {
        executable_relative_path: &'static str,
        arguments: &'static [CommandArgument],
    },
    Java {
        executable_relative_path: &'static str,
        arguments: &'static [CommandArgument],
        required_files: &'static [&'static str],
    },
}

impl ProgramLayout {
    fn executable_relative_path(self) -> &'static str {
        match self {
            Self::Node {
                executable_relative_path,
                ..
            }
            | Self::Native {
                executable_relative_path,
                ..
            }
            | Self::Java {
                executable_relative_path,
                ..
            } => executable_relative_path,
        }
    }

    fn arguments(self) -> &'static [CommandArgument] {
        match self {
            Self::Node { arguments, .. }
            | Self::Native { arguments, .. }
            | Self::Java { arguments, .. } => arguments,
        }
    }

    fn required_files(self) -> &'static [&'static str] {
        match self {
            Self::Node { required_files, .. } | Self::Java { required_files, .. } => required_files,
            Self::Native { .. } => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdapterDescriptor {
    pub adapter_id: &'static str,
    pub display_name: &'static str,
    pub languages: &'static [LanguageId],
    pub extensions: &'static [&'static str],
    pub file_names: &'static [&'static str],
    pub lsp_language_ids: &'static [&'static str],
    pub root_markers: &'static [&'static str],
    pub program: ProgramLayout,
    pub initialization_options_json: &'static str,
    pub workspace_configuration_json: &'static str,
    pub completion_trigger_characters: &'static [&'static str],
    pub metadata: PackagedMetadata,
    pub availability: PocAvailability,
    pub maximum_startup_time: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedServerCommand {
    pub adapter_id: &'static str,
    pub program: PathBuf,
    pub args: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdapterCachePaths {
    pub cache_dir: PathBuf,
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CatalogUnavailable {
    NotBundledYet {
        adapter_id: String,
    },
    MissingResource {
        adapter_id: String,
        relative_path: PathBuf,
    },
    ResourceIsNotAFile {
        adapter_id: String,
        relative_path: PathBuf,
    },
    ProgramNotExecutable {
        adapter_id: String,
        relative_path: PathBuf,
    },
    ResourceOutsideRoot {
        adapter_id: String,
        relative_path: PathBuf,
    },
    UnsupportedArchitecture {
        adapter_id: String,
        expected: String,
        actual: String,
    },
    UnsupportedPlatform {
        adapter_id: String,
        expected: String,
        actual: String,
    },
    InvalidExecutable {
        adapter_id: String,
        relative_path: PathBuf,
    },
    NoResourceRoot,
}

impl CatalogUnavailable {
    pub(crate) fn lsp_reason(&self) -> LspUnavailableReason {
        match self {
            Self::NotBundledYet { adapter_id } => LspUnavailableReason::NotBundledYet {
                adapter_id: adapter_id.clone(),
            },
            Self::UnsupportedPlatform {
                expected, actual, ..
            } => LspUnavailableReason::UnsupportedPlatform {
                expected: expected.clone(),
                actual: actual.clone(),
            },
            Self::UnsupportedArchitecture {
                expected, actual, ..
            } => LspUnavailableReason::UnsupportedArchitecture {
                expected: expected.clone(),
                actual: actual.clone(),
            },
            _ => LspUnavailableReason::ResourceUnavailable {
                adapter_id: self.adapter_id().to_owned(),
                detail: self.to_string(),
            },
        }
    }

    fn adapter_id(&self) -> &str {
        match self {
            Self::NotBundledYet { adapter_id }
            | Self::MissingResource { adapter_id, .. }
            | Self::ResourceIsNotAFile { adapter_id, .. }
            | Self::ProgramNotExecutable { adapter_id, .. }
            | Self::ResourceOutsideRoot { adapter_id, .. }
            | Self::UnsupportedArchitecture { adapter_id, .. }
            | Self::UnsupportedPlatform { adapter_id, .. }
            | Self::InvalidExecutable { adapter_id, .. } => adapter_id,
            Self::NoResourceRoot => "unknown",
        }
    }
}

impl std::fmt::Display for CatalogUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotBundledYet { adapter_id } => {
                write!(formatter, "{adapter_id} is not bundled yet")
            }
            Self::MissingResource { relative_path, .. } => {
                write!(
                    formatter,
                    "bundled resource {} is missing",
                    relative_path.display()
                )
            }
            Self::ResourceIsNotAFile { relative_path, .. } => {
                write!(
                    formatter,
                    "bundled resource {} is not a file",
                    relative_path.display()
                )
            }
            Self::ProgramNotExecutable { relative_path, .. } => write!(
                formatter,
                "bundled program {} is not executable",
                relative_path.display()
            ),
            Self::ResourceOutsideRoot { relative_path, .. } => write!(
                formatter,
                "bundled resource {} escapes the resource root",
                relative_path.display()
            ),
            Self::UnsupportedArchitecture {
                expected, actual, ..
            } => write!(formatter, "expected {expected} program but found {actual}"),
            Self::UnsupportedPlatform {
                expected, actual, ..
            } => write!(formatter, "expected {expected} program but found {actual}"),
            Self::InvalidExecutable { relative_path, .. } => write!(
                formatter,
                "bundled program {} is not a supported Mach-O executable",
                relative_path.display()
            ),
            Self::NoResourceRoot => write!(formatter, "no bundled LSP resource root is available"),
        }
    }
}

impl std::error::Error for CatalogUnavailable {}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct BundledServerCatalog;

impl BundledServerCatalog {
    pub(crate) const fn new() -> Self {
        Self
    }

    pub(crate) fn descriptor(&self, language: LanguageId) -> &'static AdapterDescriptor {
        DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.languages.contains(&language))
            .expect("every curated LanguageId has an immutable catalog descriptor")
    }

    pub(crate) fn resolve(
        &self,
        language: LanguageId,
        resource_root: &Path,
    ) -> Result<ResolvedServerCommand, CatalogUnavailable> {
        let descriptor = self.descriptor(language);
        if descriptor.availability == PocAvailability::NotBundledYet {
            return Err(CatalogUnavailable::NotBundledYet {
                adapter_id: descriptor.adapter_id.to_owned(),
            });
        }

        let root = canonical_directory(resource_root, descriptor.adapter_id)?;
        let program_relative_path = Path::new(descriptor.program.executable_relative_path());
        let program = resource_path(&root, program_relative_path, descriptor.adapter_id)?;
        require_file(&program, program_relative_path, descriptor.adapter_id)?;
        require_executable(&program, program_relative_path, descriptor.adapter_id)?;
        validate_macos_arm64_executable(&program, program_relative_path, descriptor.adapter_id)?;

        for required in descriptor.program.required_files() {
            let required_path = Path::new(required);
            let resource = resource_path(&root, required_path, descriptor.adapter_id)?;
            require_file(&resource, required_path, descriptor.adapter_id)?;
        }

        let args = descriptor
            .program
            .arguments()
            .iter()
            .map(|argument| match argument {
                CommandArgument::Literal(value) => Ok(PathBuf::from(value)),
                CommandArgument::ResourcePath(relative_path) => {
                    let relative_path = Path::new(relative_path);
                    let absolute = resource_path(&root, relative_path, descriptor.adapter_id)?;
                    require_file(&absolute, relative_path, descriptor.adapter_id)?;
                    Ok(absolute)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ResolvedServerCommand {
            adapter_id: descriptor.adapter_id,
            program,
            args,
        })
    }

    pub(crate) fn cache_paths(
        &self,
        language: LanguageId,
        canonical_project_root: &Path,
        termlab_cache_root: &Path,
    ) -> Result<AdapterCachePaths, CatalogUnavailable> {
        let descriptor = self.descriptor(language);
        let cache_root = canonical_directory(termlab_cache_root, descriptor.adapter_id)?;
        let canonical_project_root = canonical_project_root.canonicalize().map_err(|_| {
            CatalogUnavailable::MissingResource {
                adapter_id: descriptor.adapter_id.to_owned(),
                relative_path: canonical_project_root.to_owned(),
            }
        })?;
        let mut hash = Sha256::new();
        hash.update(descriptor.adapter_id.as_bytes());
        hash.update([0]);
        hash.update(canonical_project_root.as_os_str().as_encoded_bytes());
        let root_key = format!("{:x}", hash.finalize());
        let base = cache_root
            .join("lsp")
            .join(descriptor.adapter_id)
            .join(root_key);

        Ok(AdapterCachePaths {
            cache_dir: base.join("cache"),
            data_dir: base.join("data"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceResolutionMode {
    Production,
    DebugOrTest,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResourceRootCandidates<'a> {
    pub test_injected_root: Option<&'a Path>,
    pub packaged_root: Option<&'a Path>,
    pub debug_environment_root: Option<&'a Path>,
    pub debug_checkout_root: Option<&'a Path>,
}

pub(crate) struct ResourceRootResolver;

impl ResourceRootResolver {
    /// Resolves only explicitly supplied candidates. AppHandle integration
    /// supplies the packaged resource path later, keeping this policy pure.
    pub(crate) fn resolve(
        candidates: ResourceRootCandidates<'_>,
        mode: ResourceResolutionMode,
    ) -> Result<PathBuf, CatalogUnavailable> {
        let root = candidates
            .test_injected_root
            .or(candidates.packaged_root)
            .or_else(|| {
                (mode == ResourceResolutionMode::DebugOrTest)
                    .then_some(candidates.debug_environment_root)
                    .flatten()
            })
            .or_else(|| {
                (mode == ResourceResolutionMode::DebugOrTest)
                    .then_some(candidates.debug_checkout_root)
                    .flatten()
            })
            .ok_or(CatalogUnavailable::NoResourceRoot)?;
        canonical_directory(root, "resource-root").map_err(|_| CatalogUnavailable::NoResourceRoot)
    }

    /// Applies the production/debug policy without requiring a Tauri
    /// `AppHandle`; the caller supplies its packaged resource path.
    pub(crate) fn resolve_runtime(
        test_injected_root: Option<&Path>,
        packaged_root: Option<&Path>,
    ) -> Result<PathBuf, CatalogUnavailable> {
        let debug_environment_root = debug_environment_root();
        let debug_checkout_root = debug_checkout_root();
        Self::resolve(
            ResourceRootCandidates {
                test_injected_root,
                packaged_root,
                debug_environment_root: debug_environment_root.as_deref(),
                debug_checkout_root: debug_checkout_root.as_deref(),
            },
            if cfg!(any(debug_assertions, test)) {
                ResourceResolutionMode::DebugOrTest
            } else {
                ResourceResolutionMode::Production
            },
        )
    }
}

fn canonical_directory(path: &Path, adapter_id: &str) -> Result<PathBuf, CatalogUnavailable> {
    let canonical = path
        .canonicalize()
        .map_err(|_| CatalogUnavailable::MissingResource {
            adapter_id: adapter_id.to_owned(),
            relative_path: path.to_owned(),
        })?;
    if canonical.is_dir() {
        Ok(canonical)
    } else {
        Err(CatalogUnavailable::ResourceIsNotAFile {
            adapter_id: adapter_id.to_owned(),
            relative_path: path.to_owned(),
        })
    }
}

fn resource_path(
    root: &Path,
    relative_path: &Path,
    adapter_id: &str,
) -> Result<PathBuf, CatalogUnavailable> {
    if relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CatalogUnavailable::ResourceOutsideRoot {
            adapter_id: adapter_id.to_owned(),
            relative_path: relative_path.to_owned(),
        });
    }

    let candidate = root.join(relative_path);
    if let Ok(canonical) = candidate.canonicalize() {
        if !canonical.starts_with(root) {
            return Err(CatalogUnavailable::ResourceOutsideRoot {
                adapter_id: adapter_id.to_owned(),
                relative_path: relative_path.to_owned(),
            });
        }
        return Ok(canonical);
    }
    Ok(candidate)
}

fn require_file(
    path: &Path,
    relative_path: &Path,
    adapter_id: &str,
) -> Result<(), CatalogUnavailable> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(CatalogUnavailable::ResourceIsNotAFile {
            adapter_id: adapter_id.to_owned(),
            relative_path: relative_path.to_owned(),
        }),
        Err(_) => Err(CatalogUnavailable::MissingResource {
            adapter_id: adapter_id.to_owned(),
            relative_path: relative_path.to_owned(),
        }),
    }
}

fn require_executable(
    path: &Path,
    relative_path: &Path,
    adapter_id: &str,
) -> Result<(), CatalogUnavailable> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
        {
            return Ok(());
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Err(CatalogUnavailable::ProgramNotExecutable {
        adapter_id: adapter_id.to_owned(),
        relative_path: relative_path.to_owned(),
    })
}

fn validate_macos_arm64_executable(
    path: &Path,
    relative_path: &Path,
    adapter_id: &str,
) -> Result<(), CatalogUnavailable> {
    let mut bytes = [0u8; 4096];
    let mut file = fs::File::open(path).map_err(|_| CatalogUnavailable::MissingResource {
        adapter_id: adapter_id.to_owned(),
        relative_path: relative_path.to_owned(),
    })?;
    let count = file
        .read(&mut bytes)
        .map_err(|_| CatalogUnavailable::InvalidExecutable {
            adapter_id: adapter_id.to_owned(),
            relative_path: relative_path.to_owned(),
        })?;
    let bytes = &bytes[..count];
    if read_u32(bytes, 0) != Some(MACHO_64_LE) {
        return Err(CatalogUnavailable::InvalidExecutable {
            adapter_id: adapter_id.to_owned(),
            relative_path: relative_path.to_owned(),
        });
    }
    let cpu_type = read_u32(bytes, 4).ok_or_else(|| CatalogUnavailable::InvalidExecutable {
        adapter_id: adapter_id.to_owned(),
        relative_path: relative_path.to_owned(),
    })?;
    if cpu_type != ARM64 {
        return Err(CatalogUnavailable::UnsupportedArchitecture {
            adapter_id: adapter_id.to_owned(),
            expected: "arm64".to_owned(),
            actual: cpu_type_name(cpu_type).to_owned(),
        });
    }
    let ncmds = read_u32(bytes, 16).ok_or_else(|| CatalogUnavailable::InvalidExecutable {
        adapter_id: adapter_id.to_owned(),
        relative_path: relative_path.to_owned(),
    })?;
    let mut offset = 32usize;
    let mut platform = None;
    for _ in 0..ncmds {
        let command =
            read_u32(bytes, offset).ok_or_else(|| CatalogUnavailable::InvalidExecutable {
                adapter_id: adapter_id.to_owned(),
                relative_path: relative_path.to_owned(),
            })?;
        let size =
            read_u32(bytes, offset + 4).ok_or_else(|| CatalogUnavailable::InvalidExecutable {
                adapter_id: adapter_id.to_owned(),
                relative_path: relative_path.to_owned(),
            })? as usize;
        if size < 8 || offset.checked_add(size).is_none_or(|end| end > bytes.len()) {
            return Err(CatalogUnavailable::InvalidExecutable {
                adapter_id: adapter_id.to_owned(),
                relative_path: relative_path.to_owned(),
            });
        }
        if command == LC_BUILD_VERSION {
            platform = read_u32(bytes, offset + 8);
            break;
        }
        offset += size;
    }
    match platform {
        Some(PLATFORM_MACOS) => Ok(()),
        Some(actual) => Err(CatalogUnavailable::UnsupportedPlatform {
            adapter_id: adapter_id.to_owned(),
            expected: "macOS".to_owned(),
            actual: platform_name(actual).to_owned(),
        }),
        None => Err(CatalogUnavailable::InvalidExecutable {
            adapter_id: adapter_id.to_owned(),
            relative_path: relative_path.to_owned(),
        }),
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .map(u32::from_le_bytes)
}

fn cpu_type_name(cpu_type: u32) -> &'static str {
    match cpu_type {
        ARM64 => "arm64",
        X86_64 => "x86_64",
        _ => "unknown",
    }
}

fn platform_name(platform: u32) -> &'static str {
    match platform {
        PLATFORM_MACOS => "macOS",
        2 => "iOS",
        3 => "tvOS",
        4 => "watchOS",
        _ => "unknown",
    }
}

#[cfg(any(debug_assertions, test))]
fn debug_environment_root() -> Option<PathBuf> {
    std::env::var_os("TERMLAB_LSP_RESOURCE_DIR").map(PathBuf::from)
}

#[cfg(not(any(debug_assertions, test)))]
fn debug_environment_root() -> Option<PathBuf> {
    None
}

#[cfg(debug_assertions)]
fn debug_checkout_root() -> Option<PathBuf> {
    Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/lsp/dist/arm64"))
}

#[cfg(not(debug_assertions))]
fn debug_checkout_root() -> Option<PathBuf> {
    None
}

const TYPESCRIPT_ARGUMENTS: &[CommandArgument] = &[
    CommandArgument::ResourcePath("typescript/node_modules/typescript-language-server/lib/cli.mjs"),
    CommandArgument::Literal("--stdio"),
];

const TYPESCRIPT_REQUIRED_FILES: &[&str] = &[
    "typescript/node_modules/typescript-language-server/lib/cli.mjs",
    "typescript/node_modules/typescript/lib/typescript.js",
];

const JAVASCRIPT_TYPESCRIPT_LANGUAGES: &[LanguageId] =
    &[LanguageId::JavaScript, LanguageId::TypeScript];

const DESCRIPTORS: &[AdapterDescriptor] = &[
    AdapterDescriptor {
        adapter_id: "typescript",
        display_name: "TypeScript and JavaScript",
        languages: JAVASCRIPT_TYPESCRIPT_LANGUAGES,
        extensions: &["js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts"],
        file_names: &[],
        lsp_language_ids: &[
            "javascript",
            "javascriptreact",
            "typescript",
            "typescriptreact",
        ],
        root_markers: &[
            "tsconfig.json",
            "jsconfig.json",
            "package.json",
            "pnpm-workspace.yaml",
            "lerna.json",
            "nx.json",
        ],
        program: ProgramLayout::Node {
            executable_relative_path: "node/bin/node",
            arguments: TYPESCRIPT_ARGUMENTS,
            required_files: TYPESCRIPT_REQUIRED_FILES,
        },
        initialization_options_json: "{}",
        workspace_configuration_json: "{\"typescript\":{},\"javascript\":{}}",
        completion_trigger_characters: &[".", "'", "\"", "/", "@", "<"],
        metadata: PackagedMetadata {
            version: "typescript-language-server 6.0.0; typescript 7.0.2; node 24.19.0",
            upstream_url: "https://github.com/typescript-language-server/typescript-language-server",
            license: "MIT; TypeScript Apache-2.0; Node MIT",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::Bundled,
        maximum_startup_time: Duration::from_secs(15),
    },
    AdapterDescriptor {
        adapter_id: "json",
        display_name: "JSON",
        languages: &[LanguageId::Json],
        extensions: &["json", "jsonc"],
        file_names: &[],
        lsp_language_ids: &["json", "jsonc"],
        root_markers: &["package.json", "tsconfig.json", "jsconfig.json", ".git"],
        program: ProgramLayout::Node {
            executable_relative_path: "node/bin/node",
            arguments: &[
                CommandArgument::ResourcePath(
                    "json/node_modules/vscode-langservers-extracted/bin/vscode-json-languageserver",
                ),
                CommandArgument::Literal("--stdio"),
            ],
            required_files: &[
                "json/node_modules/vscode-langservers-extracted/bin/vscode-json-languageserver",
            ],
        },
        initialization_options_json: "{}",
        workspace_configuration_json: "{\"json\":{}}",
        completion_trigger_characters: &["\"", ":", ","],
        metadata: PackagedMetadata {
            version: "not bundled",
            upstream_url: "https://github.com/microsoft/vscode-languageserver-node",
            license: "MIT",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::NotBundledYet,
        maximum_startup_time: Duration::from_secs(15),
    },
    AdapterDescriptor {
        adapter_id: "python",
        display_name: "Python",
        languages: &[LanguageId::Python],
        extensions: &["py", "pyi"],
        file_names: &[],
        lsp_language_ids: &["python"],
        root_markers: &[
            "pyproject.toml",
            "setup.cfg",
            "setup.py",
            "tox.ini",
            "Pipfile",
            "poetry.lock",
            "uv.lock",
            ".git",
        ],
        program: ProgramLayout::Node {
            executable_relative_path: "node/bin/node",
            arguments: &[
                CommandArgument::ResourcePath("python/node_modules/pyright/langserver.index.js"),
                CommandArgument::Literal("--stdio"),
            ],
            required_files: &["python/node_modules/pyright/langserver.index.js"],
        },
        initialization_options_json: "{}",
        workspace_configuration_json: "{\"python\":{}}",
        completion_trigger_characters: &[".", "'", "\"", "/", "@"],
        metadata: PackagedMetadata {
            version: "not bundled",
            upstream_url: "https://github.com/microsoft/pyright",
            license: "MIT",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::NotBundledYet,
        maximum_startup_time: Duration::from_secs(20),
    },
    AdapterDescriptor {
        adapter_id: "rust",
        display_name: "Rust",
        languages: &[LanguageId::Rust],
        extensions: &["rs"],
        file_names: &[],
        lsp_language_ids: &["rust"],
        root_markers: &["Cargo.toml"],
        program: ProgramLayout::Native {
            executable_relative_path: "rust-analyzer/rust-analyzer",
            arguments: &[],
        },
        initialization_options_json: "{}",
        workspace_configuration_json: "{\"rust-analyzer\":{}}",
        completion_trigger_characters: &[".", ":", "<"],
        metadata: PackagedMetadata {
            version: "2026-08-24",
            upstream_url: "https://github.com/rust-lang/rust-analyzer",
            license: "MIT OR Apache-2.0",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::Bundled,
        maximum_startup_time: Duration::from_secs(20),
    },
    AdapterDescriptor {
        adapter_id: "go",
        display_name: "Go",
        languages: &[LanguageId::Go],
        extensions: &["go"],
        file_names: &["go.mod", "go.work"],
        lsp_language_ids: &["go"],
        root_markers: &["go.work", "go.mod"],
        program: ProgramLayout::Native {
            executable_relative_path: "gopls/gopls",
            arguments: &[],
        },
        initialization_options_json: "{}",
        workspace_configuration_json: "{\"gopls\":{}}",
        completion_trigger_characters: &["."],
        metadata: PackagedMetadata {
            version: "not bundled",
            upstream_url: "https://github.com/golang/tools/tree/master/gopls",
            license: "BSD-3-Clause",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::NotBundledYet,
        maximum_startup_time: Duration::from_secs(20),
    },
    AdapterDescriptor {
        adapter_id: "clangd",
        display_name: "C and C++",
        languages: &[LanguageId::C, LanguageId::Cpp],
        extensions: &["c", "h", "cc", "cp", "cpp", "cxx", "hpp", "hh", "hxx"],
        file_names: &["compile_commands.json"],
        lsp_language_ids: &["c", "cpp"],
        root_markers: &[
            "compile_commands.json",
            "compile_flags.txt",
            "CMakeLists.txt",
            "meson.build",
            ".git",
        ],
        program: ProgramLayout::Native {
            executable_relative_path: "clangd/clangd",
            arguments: &[],
        },
        initialization_options_json: "{}",
        workspace_configuration_json: "{\"clangd\":{}}",
        completion_trigger_characters: &[".", ":", ">"],
        metadata: PackagedMetadata {
            version: "not bundled",
            upstream_url: "https://clangd.llvm.org/",
            license: "Apache-2.0 WITH LLVM-exception",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::NotBundledYet,
        maximum_startup_time: Duration::from_secs(30),
    },
    AdapterDescriptor {
        adapter_id: "java",
        display_name: "Java",
        languages: &[LanguageId::Java],
        extensions: &["java"],
        file_names: &[
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
        lsp_language_ids: &["java"],
        root_markers: &[
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
            ".git",
        ],
        program: ProgramLayout::Java {
            executable_relative_path: "jre/bin/java",
            arguments: &[
                CommandArgument::Literal("-jar"),
                CommandArgument::ResourcePath("jdtls/plugins/org.eclipse.equinox.launcher.jar"),
            ],
            required_files: &["jdtls/plugins/org.eclipse.equinox.launcher.jar"],
        },
        initialization_options_json: "{}",
        workspace_configuration_json: "{\"java\":{}}",
        completion_trigger_characters: &["."],
        metadata: PackagedMetadata {
            version: "not bundled",
            upstream_url: "https://projects.eclipse.org/projects/eclipse.jdt.ls",
            license: "EPL-2.0",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::NotBundledYet,
        maximum_startup_time: Duration::from_secs(45),
    },
];

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::{
        BundledServerCatalog, CatalogUnavailable, ResourceResolutionMode, ResourceRootCandidates,
        ResourceRootResolver, resource_path,
    };
    use crate::lsp::root::LanguageId;

    #[test]
    fn typescript_command_uses_the_private_bundled_node_runtime() {
        let resources = ResourceTree::new();
        let catalog = BundledServerCatalog::new();

        let command = catalog
            .resolve(LanguageId::TypeScript, resources.root())
            .expect("resolve TypeScript command");
        let root = resources.canonical_root();

        assert_eq!(command.adapter_id, "typescript");
        assert_eq!(command.program, root.join("node/bin/node"));
        assert_eq!(
            command.args,
            vec![
                root.join("typescript/node_modules/typescript-language-server/lib/cli.mjs"),
                PathBuf::from("--stdio"),
            ]
        );
        assert!(command.program.is_absolute());
        assert!(command.args[0].is_absolute());
        assert!(command.program.starts_with(&root));
        assert!(command.args[0].starts_with(&root));
    }

    #[test]
    fn javascript_and_typescript_share_the_typescript_adapter() {
        let catalog = BundledServerCatalog::new();

        assert_eq!(
            catalog.descriptor(LanguageId::JavaScript).adapter_id,
            catalog.descriptor(LanguageId::TypeScript).adapter_id
        );
    }

    #[test]
    fn rust_uses_its_bundled_native_server() {
        let resources = ResourceTree::new();
        let catalog = BundledServerCatalog::new();

        let command = catalog
            .resolve(LanguageId::Rust, resources.root())
            .expect("resolve Rust command");
        let root = resources.canonical_root();

        assert_eq!(command.adapter_id, "rust");
        assert_eq!(command.program, root.join("rust-analyzer/rust-analyzer"));
        assert!(command.args.is_empty());
        assert!(command.program.is_absolute());
        assert!(command.program.starts_with(&root));
    }

    #[test]
    fn curated_but_unbundled_languages_return_not_bundled_yet() {
        let resources = ResourceTree::new();
        let catalog = BundledServerCatalog::new();

        for (language, adapter_id) in [
            (LanguageId::Json, "json"),
            (LanguageId::Python, "python"),
            (LanguageId::Go, "go"),
            (LanguageId::C, "clangd"),
            (LanguageId::Cpp, "clangd"),
            (LanguageId::Java, "java"),
        ] {
            assert_eq!(
                catalog.resolve(language, resources.root()),
                Err(CatalogUnavailable::NotBundledYet {
                    adapter_id: adapter_id.to_owned(),
                })
            );
        }
    }

    #[test]
    fn resolution_never_consults_path_for_a_bundled_program() {
        let resources = ResourceTree::new();
        let catalog = BundledServerCatalog::new();

        let command = catalog
            .resolve(LanguageId::Rust, resources.root())
            .expect("resolve bundled Rust command");

        assert_eq!(
            command.program,
            resources
                .canonical_root()
                .join("rust-analyzer/rust-analyzer")
        );
        assert!(command.program.is_absolute());
    }

    #[test]
    fn missing_or_non_executable_program_is_reported_as_unavailable() {
        let resources = ResourceTree::new();
        let catalog = BundledServerCatalog::new();
        fs::remove_file(resources.root().join("rust-analyzer/rust-analyzer"))
            .expect("remove bundled program");

        assert_eq!(
            catalog.resolve(LanguageId::Rust, resources.root()),
            Err(CatalogUnavailable::MissingResource {
                adapter_id: "rust".to_owned(),
                relative_path: PathBuf::from("rust-analyzer/rust-analyzer"),
            })
        );

        resources.install_rust_analyzer(false);
        assert_eq!(
            catalog.resolve(LanguageId::Rust, resources.root()),
            Err(CatalogUnavailable::ProgramNotExecutable {
                adapter_id: "rust".to_owned(),
                relative_path: PathBuf::from("rust-analyzer/rust-analyzer"),
            })
        );
    }

    #[test]
    fn resolver_rejects_non_files_and_paths_that_escape_the_resource_root() {
        let resources = ResourceTree::new();
        let root = resources.canonical_root();
        let catalog = BundledServerCatalog::new();
        fs::remove_file(root.join("rust-analyzer/rust-analyzer")).expect("remove program");
        fs::create_dir(root.join("rust-analyzer/rust-analyzer"))
            .expect("replace program with directory");

        assert_eq!(
            catalog.resolve(LanguageId::Rust, &root),
            Err(CatalogUnavailable::ResourceIsNotAFile {
                adapter_id: "rust".to_owned(),
                relative_path: PathBuf::from("rust-analyzer/rust-analyzer"),
            })
        );
        assert_eq!(
            resource_path(&root, Path::new("../outside"), "rust"),
            Err(CatalogUnavailable::ResourceOutsideRoot {
                adapter_id: "rust".to_owned(),
                relative_path: PathBuf::from("../outside"),
            })
        );
    }

    #[test]
    fn rejects_programs_with_the_wrong_architecture_or_platform() {
        let resources = ResourceTree::new();
        let catalog = BundledServerCatalog::new();

        resources.install_rust_analyzer_with_header(0x0100_0007, 1, true);
        assert_eq!(
            catalog.resolve(LanguageId::Rust, resources.root()),
            Err(CatalogUnavailable::UnsupportedArchitecture {
                adapter_id: "rust".to_owned(),
                expected: "arm64".to_owned(),
                actual: "x86_64".to_owned(),
            })
        );

        resources.install_rust_analyzer_with_header(0x0100_000c, 2, true);
        assert_eq!(
            catalog.resolve(LanguageId::Rust, resources.root()),
            Err(CatalogUnavailable::UnsupportedPlatform {
                adapter_id: "rust".to_owned(),
                expected: "macOS".to_owned(),
                actual: "iOS".to_owned(),
            })
        );
    }

    #[test]
    fn release_resolution_ignores_debug_environment_and_checkout_roots() {
        let packaged = ResourceTree::new();
        let debug_environment = ResourceTree::new();
        let debug_checkout = ResourceTree::new();

        let root = ResourceRootResolver::resolve(
            ResourceRootCandidates {
                test_injected_root: None,
                packaged_root: Some(packaged.root()),
                debug_environment_root: Some(debug_environment.root()),
                debug_checkout_root: Some(debug_checkout.root()),
            },
            ResourceResolutionMode::Production,
        )
        .expect("resolve packaged resource root in production");

        assert_eq!(root, packaged.canonical_root());
    }

    #[test]
    fn an_explicit_test_root_fails_closed_instead_of_falling_back() {
        let temporary = TempDir::new().expect("temporary directory");
        let packaged = ResourceTree::new();

        assert_eq!(
            ResourceRootResolver::resolve(
                ResourceRootCandidates {
                    test_injected_root: Some(&temporary.path().join("missing")),
                    packaged_root: Some(packaged.root()),
                    debug_environment_root: None,
                    debug_checkout_root: None,
                },
                ResourceResolutionMode::Production,
            ),
            Err(CatalogUnavailable::NoResourceRoot)
        );
    }

    #[test]
    fn cache_paths_are_deterministic_adapter_scoped_and_outside_the_project() {
        let project = TempDir::new().expect("project directory");
        let second_project = TempDir::new().expect("second project directory");
        let cache = TempDir::new().expect("cache directory");
        let canonical_project = project.path().canonicalize().expect("canonical project");
        let second_canonical_project = second_project
            .path()
            .canonicalize()
            .expect("second canonical project");
        let canonical_cache = cache.path().canonicalize().expect("canonical cache");
        let catalog = BundledServerCatalog::new();

        let typescript = catalog
            .cache_paths(LanguageId::TypeScript, &canonical_project, &canonical_cache)
            .expect("TypeScript cache paths");
        let rust = catalog
            .cache_paths(LanguageId::Rust, &canonical_project, &canonical_cache)
            .expect("Rust cache paths");
        let second_typescript = catalog
            .cache_paths(
                LanguageId::TypeScript,
                &second_canonical_project,
                &canonical_cache,
            )
            .expect("second TypeScript cache paths");

        assert_eq!(
            typescript,
            catalog
                .cache_paths(LanguageId::TypeScript, &canonical_project, &canonical_cache)
                .expect("repeat TypeScript cache paths")
        );
        assert!(typescript.cache_dir.starts_with(&canonical_cache));
        assert!(typescript.data_dir.starts_with(&canonical_cache));
        assert!(!typescript.cache_dir.starts_with(&canonical_project));
        assert_ne!(typescript.cache_dir, rust.cache_dir);
        assert_ne!(typescript.data_dir, rust.data_dir);
        assert_ne!(typescript.cache_dir, second_typescript.cache_dir);
        assert_ne!(typescript.data_dir, second_typescript.data_dir);
    }

    struct ResourceTree {
        directory: TempDir,
    }

    impl ResourceTree {
        fn new() -> Self {
            let tree = Self {
                directory: TempDir::new().expect("resource directory"),
            };
            tree.install_node();
            tree.install_typescript_files();
            tree.install_rust_analyzer(true);
            tree
        }

        fn root(&self) -> &Path {
            self.directory.path()
        }

        fn canonical_root(&self) -> PathBuf {
            self.root().canonicalize().expect("canonical resource root")
        }

        fn install_node(&self) {
            write_macho_binary(&self.root().join("node/bin/node"), 0x0100_000c, 1, true);
        }

        fn install_typescript_files(&self) {
            write_file(
                &self
                    .root()
                    .join("typescript/node_modules/typescript-language-server/lib/cli.mjs"),
                b"export {};\n",
            );
            write_file(
                &self
                    .root()
                    .join("typescript/node_modules/typescript/lib/typescript.js"),
                b"module.exports = {};\n",
            );
        }

        fn install_rust_analyzer(&self, executable: bool) {
            self.install_rust_analyzer_with_header(0x0100_000c, 1, executable);
        }

        fn install_rust_analyzer_with_header(
            &self,
            cpu_type: u32,
            platform: u32,
            executable: bool,
        ) {
            write_macho_binary(
                &self.root().join("rust-analyzer/rust-analyzer"),
                cpu_type,
                platform,
                executable,
            );
        }
    }

    fn write_file(path: &Path, contents: &[u8]) {
        fs::create_dir_all(path.parent().expect("parent directory")).expect("create parent");
        fs::write(path, contents).expect("write file");
    }

    fn write_macho_binary(path: &Path, cpu_type: u32, platform: u32, executable: bool) {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0xfeed_facfu32.to_le_bytes());
        bytes.extend_from_slice(&cpu_type.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&24u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0x32u32.to_le_bytes());
        bytes.extend_from_slice(&24u32.to_le_bytes());
        bytes.extend_from_slice(&platform.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        write_file(path, &bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(
                path,
                fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
            )
            .expect("set executable permission");
        }
    }
}
