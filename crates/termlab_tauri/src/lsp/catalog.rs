use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::root::LanguageId;
use super::types::LspUnavailableReason;

const ARM64: u32 = 0x0100_000c;
const X86_64: u32 = 0x0100_0007;
const MACHO_64_LE: u32 = 0xfeed_facf;
const LC_BUILD_VERSION: u32 = 0x32;
const PLATFORM_MACOS: u32 = 1;
const MH_EXECUTE: u32 = 2;
const INSTALLED_RECEIPT_SCHEMA: u32 = 1;

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
pub(crate) struct AdapterTimeouts {
    pub initialize: Duration,
    pub shutdown: Duration,
    pub smoke_test: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootStrategy {
    JavaScript,
    Json,
    Python,
    Rust,
    Go,
    Clangd,
    Java,
}

impl RootStrategy {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::JavaScript => "javascript",
            Self::Json => "json",
            Self::Python => "python",
            Self::Rust => "rust",
            Self::Go => "go",
            Self::Clangd => "clangd",
            Self::Java => "java",
        }
    }

    pub(crate) const fn markers(self) -> &'static [&'static str] {
        match self {
            Self::JavaScript => &[
                "tsconfig.json",
                "jsconfig.json",
                "package.json",
                "pnpm-workspace.yaml",
                "lerna.json",
                "nx.json",
            ],
            Self::Json => &["package.json", "tsconfig.json", "jsconfig.json", ".git"],
            Self::Python => &[
                "pyproject.toml",
                "setup.cfg",
                "setup.py",
                "tox.ini",
                "Pipfile",
                "poetry.lock",
                "uv.lock",
                ".git",
            ],
            Self::Rust => &["Cargo.toml"],
            Self::Go => &["go.work", "go.mod"],
            Self::Clangd => &[
                "compile_commands.json",
                "compile_flags.txt",
                ".clangd",
                "CMakeLists.txt",
                "meson.build",
                ".git",
            ],
            Self::Java => &[
                "pom.xml",
                "settings.gradle",
                "settings.gradle.kts",
                "build.gradle",
                "build.gradle.kts",
                ".project",
                ".git",
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilePattern {
    Extension(&'static str),
    FileName(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileBinding {
    pub pattern: FilePattern,
    pub language: LanguageId,
    pub lsp_language_id: &'static str,
}

impl FileBinding {
    fn matches(self, path: &Path) -> bool {
        match self.pattern {
            FilePattern::Extension(extension) => path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(extension)),
            FilePattern::FileName(file_name) => path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(file_name)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedFileBinding {
    pub adapter_id: &'static str,
    pub language: LanguageId,
    pub lsp_language_id: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriggerNormalizationPolicy {
    MergeWithServer,
    StaticOnly,
}

impl TriggerNormalizationPolicy {
    fn normalize(self, static_triggers: &[&str], server_triggers: &[String]) -> Vec<String> {
        let mut normalized = Vec::new();
        let mut append = |trigger: &str| {
            if !trigger.is_empty() && !normalized.iter().any(|value| value == trigger) {
                normalized.push(trigger.to_owned());
            }
        };
        for trigger in static_triggers {
            append(trigger);
        }
        if self == Self::MergeWithServer {
            for trigger in server_triggers {
                append(trigger);
            }
        }
        normalized
    }
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
    pub file_bindings: &'static [FileBinding],
    pub root_strategy: RootStrategy,
    pub program: ProgramLayout,
    pub initialization_options_json: &'static str,
    pub workspace_configuration_json: &'static str,
    pub completion_trigger_characters: &'static [&'static str],
    pub trigger_normalization: TriggerNormalizationPolicy,
    pub metadata: PackagedMetadata,
    pub availability: PocAvailability,
    pub timeouts: AdapterTimeouts,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostOperatingSystem {
    MacOs,
    Linux,
    Windows,
    Other,
}

impl HostOperatingSystem {
    const fn name(self) -> &'static str {
        match self {
            Self::MacOs => "macOS",
            Self::Linux => "linux",
            Self::Windows => "windows",
            Self::Other => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostArchitecture {
    Arm64,
    X86_64,
    Other,
}

impl HostArchitecture {
    const fn name(self) -> &'static str {
        match self {
            Self::Arm64 => "arm64",
            Self::X86_64 => "x86_64",
            Self::Other => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HostPlatform {
    operating_system: HostOperatingSystem,
    architecture: HostArchitecture,
}

impl HostPlatform {
    pub(crate) const fn new(
        operating_system: HostOperatingSystem,
        architecture: HostArchitecture,
    ) -> Self {
        Self {
            operating_system,
            architecture,
        }
    }

    fn current() -> Self {
        Self::new(
            if cfg!(target_os = "macos") {
                HostOperatingSystem::MacOs
            } else if cfg!(target_os = "linux") {
                HostOperatingSystem::Linux
            } else if cfg!(target_os = "windows") {
                HostOperatingSystem::Windows
            } else {
                HostOperatingSystem::Other
            },
            if cfg!(target_arch = "aarch64") {
                HostArchitecture::Arm64
            } else if cfg!(target_arch = "x86_64") {
                HostArchitecture::X86_64
            } else {
                HostArchitecture::Other
            },
        )
    }
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
    MissingReceipt {
        adapter_id: String,
    },
    CorruptResource {
        adapter_id: String,
        relative_path: PathBuf,
    },
    CacheRootInsideProject {
        adapter_id: String,
        cache_root: PathBuf,
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
            Self::MissingReceipt { adapter_id } => LspUnavailableReason::MissingResource {
                adapter_id: adapter_id.clone(),
                relative_path: "manifest.json".to_owned(),
            },
            Self::MissingResource {
                adapter_id,
                relative_path,
            } => LspUnavailableReason::MissingResource {
                adapter_id: adapter_id.clone(),
                relative_path: relative_path.display().to_string(),
            },
            Self::CorruptResource {
                adapter_id,
                relative_path,
            }
            | Self::ResourceIsNotAFile {
                adapter_id,
                relative_path,
            }
            | Self::ProgramNotExecutable {
                adapter_id,
                relative_path,
            }
            | Self::ResourceOutsideRoot {
                adapter_id,
                relative_path,
            }
            | Self::InvalidExecutable {
                adapter_id,
                relative_path,
            } => LspUnavailableReason::CorruptResource {
                adapter_id: adapter_id.clone(),
                relative_path: relative_path.display().to_string(),
            },
            Self::CacheRootInsideProject { .. } | Self::NoResourceRoot => {
                LspUnavailableReason::CorruptResource {
                    adapter_id: self.adapter_id().to_owned(),
                    relative_path: "resource-root".to_owned(),
                }
            }
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
            Self::MissingReceipt { adapter_id }
            | Self::CorruptResource { adapter_id, .. }
            | Self::CacheRootInsideProject { adapter_id, .. } => adapter_id,
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
            Self::MissingReceipt { .. } => write!(formatter, "bundled LSP receipt is missing"),
            Self::CorruptResource { relative_path, .. } => write!(
                formatter,
                "bundled resource {} failed integrity validation",
                relative_path.display()
            ),
            Self::CacheRootInsideProject { cache_root, .. } => write!(
                formatter,
                "TermLab cache root {} is inside the project source tree",
                cache_root.display()
            ),
            Self::NoResourceRoot => write!(formatter, "no bundled LSP resource root is available"),
        }
    }
}

impl std::error::Error for CatalogUnavailable {}

/// Versioned, packaging-generated receipt stored at `lsp/manifest.json`.
/// It deliberately names every installed file by relative path and immutable
/// digest so runtime resolution can fail closed without executing a program.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstalledLspReceipt {
    pub schema: u32,
    pub platform: String,
    pub architecture: String,
    pub artifacts: Vec<ReceiptArtifact>,
    pub files: Vec<ReceiptFile>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ReceiptArtifact {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReceiptFile {
    pub relative_path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct BundledServerCatalog;

impl BundledServerCatalog {
    pub(crate) const fn new() -> Self {
        Self
    }

    pub(crate) fn descriptor(&self, language: LanguageId) -> &'static AdapterDescriptor {
        DESCRIPTORS
            .iter()
            .find(|descriptor| {
                descriptor
                    .file_bindings
                    .iter()
                    .any(|binding| binding.language == language)
            })
            .expect("every curated LanguageId has an immutable catalog descriptor")
    }

    pub(crate) fn file_binding(&self, path: &Path) -> Option<ResolvedFileBinding> {
        DESCRIPTORS.iter().find_map(|descriptor| {
            descriptor
                .file_bindings
                .iter()
                .copied()
                .find(|binding| binding.matches(path))
                .map(|binding| ResolvedFileBinding {
                    adapter_id: descriptor.adapter_id,
                    language: binding.language,
                    lsp_language_id: binding.lsp_language_id,
                })
        })
    }

    pub(crate) fn normalize_completion_triggers(
        &self,
        language: LanguageId,
        server_triggers: &[String],
    ) -> Vec<String> {
        let descriptor = self.descriptor(language);
        descriptor
            .trigger_normalization
            .normalize(descriptor.completion_trigger_characters, server_triggers)
    }

    pub(crate) fn resolve(
        &self,
        language: LanguageId,
        resource_root: &Path,
    ) -> Result<ResolvedServerCommand, CatalogUnavailable> {
        self.resolve_for_host(language, resource_root, HostPlatform::current())
    }

    pub(crate) fn resolve_for_host(
        &self,
        language: LanguageId,
        resource_root: &Path,
        host: HostPlatform,
    ) -> Result<ResolvedServerCommand, CatalogUnavailable> {
        let descriptor = self.descriptor(language);
        if descriptor.availability == PocAvailability::NotBundledYet {
            return Err(CatalogUnavailable::NotBundledYet {
                adapter_id: descriptor.adapter_id.to_owned(),
            });
        }

        validate_host(host, descriptor.adapter_id)?;

        let root = canonical_directory(resource_root, descriptor.adapter_id)?;
        let receipt = load_receipt(&root, descriptor.adapter_id)?;
        validate_receipt_identity(&receipt, descriptor.adapter_id)?;
        let program_relative_path = Path::new(descriptor.program.executable_relative_path());
        let program = resource_path(&root, program_relative_path, descriptor.adapter_id)?;
        require_file(&program, program_relative_path, descriptor.adapter_id)?;
        validate_receipt_file(
            &receipt,
            &program,
            program_relative_path,
            descriptor.adapter_id,
        )?;
        require_executable(&program, program_relative_path, descriptor.adapter_id)?;
        validate_macos_arm64_executable(&program, program_relative_path, descriptor.adapter_id)?;

        for required in descriptor.program.required_files() {
            let required_path = Path::new(required);
            let resource = resource_path(&root, required_path, descriptor.adapter_id)?;
            require_file(&resource, required_path, descriptor.adapter_id)?;
            validate_receipt_file(&receipt, &resource, required_path, descriptor.adapter_id)?;
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
                    validate_receipt_file(
                        &receipt,
                        &absolute,
                        relative_path,
                        descriptor.adapter_id,
                    )?;
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
        let canonical_project_root =
            canonical_directory(canonical_project_root, descriptor.adapter_id)?;
        if cache_root.starts_with(&canonical_project_root) {
            return Err(CatalogUnavailable::CacheRootInsideProject {
                adapter_id: descriptor.adapter_id.to_owned(),
                cache_root,
            });
        }
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

fn validate_host(host: HostPlatform, adapter_id: &str) -> Result<(), CatalogUnavailable> {
    if host.operating_system != HostOperatingSystem::MacOs {
        return Err(CatalogUnavailable::UnsupportedPlatform {
            adapter_id: adapter_id.to_owned(),
            expected: "macOS".to_owned(),
            actual: host.operating_system.name().to_owned(),
        });
    }
    if host.architecture != HostArchitecture::Arm64 {
        return Err(CatalogUnavailable::UnsupportedArchitecture {
            adapter_id: adapter_id.to_owned(),
            expected: "arm64".to_owned(),
            actual: host.architecture.name().to_owned(),
        });
    }
    Ok(())
}

fn load_receipt(root: &Path, adapter_id: &str) -> Result<InstalledLspReceipt, CatalogUnavailable> {
    let receipt_path = root.join("manifest.json");
    let contents = match fs::read(&receipt_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CatalogUnavailable::MissingReceipt {
                adapter_id: adapter_id.to_owned(),
            });
        }
        Err(_) => {
            return Err(CatalogUnavailable::CorruptResource {
                adapter_id: adapter_id.to_owned(),
                relative_path: PathBuf::from("manifest.json"),
            });
        }
    };
    serde_json::from_slice(&contents).map_err(|_| CatalogUnavailable::CorruptResource {
        adapter_id: adapter_id.to_owned(),
        relative_path: PathBuf::from("manifest.json"),
    })
}

fn validate_receipt_identity(
    receipt: &InstalledLspReceipt,
    adapter_id: &str,
) -> Result<(), CatalogUnavailable> {
    if receipt.schema != INSTALLED_RECEIPT_SCHEMA
        || receipt.platform != "macos"
        || receipt.architecture != "arm64"
        || receipt
            .artifacts
            .iter()
            .any(|artifact| artifact.id.is_empty() || artifact.version.is_empty())
        || ![
            ("node", "24.19.0"),
            ("typescript-language-server", "6.0.0"),
            ("typescript", "7.0.2"),
            ("rust-analyzer", "2026-08-24"),
        ]
        .iter()
        .all(|(id, version)| {
            receipt
                .artifacts
                .iter()
                .any(|artifact| artifact.id == *id && artifact.version == *version)
        })
        || receipt
            .files
            .iter()
            .any(|file| !is_safe_receipt_path(&file.relative_path) || !is_sha256(&file.sha256))
    {
        return Err(CatalogUnavailable::CorruptResource {
            adapter_id: adapter_id.to_owned(),
            relative_path: PathBuf::from("manifest.json"),
        });
    }
    Ok(())
}

fn validate_receipt_file(
    receipt: &InstalledLspReceipt,
    path: &Path,
    relative_path: &Path,
    adapter_id: &str,
) -> Result<(), CatalogUnavailable> {
    let relative_path_text = relative_path.to_string_lossy();
    let files = receipt
        .files
        .iter()
        .filter(|file| file.relative_path == relative_path_text);
    let mut files = files.peekable();
    let Some(file) = files.next() else {
        return Err(CatalogUnavailable::CorruptResource {
            adapter_id: adapter_id.to_owned(),
            relative_path: relative_path.to_owned(),
        });
    };
    if files.next().is_some()
        || file.size == 0
        || file.size
            != fs::metadata(path)
                .map(|metadata| metadata.len())
                .unwrap_or(u64::MAX)
        || !is_sha256(&file.sha256)
    {
        return Err(CatalogUnavailable::CorruptResource {
            adapter_id: adapter_id.to_owned(),
            relative_path: relative_path.to_owned(),
        });
    }
    let contents = fs::read(path).map_err(|_| CatalogUnavailable::MissingResource {
        adapter_id: adapter_id.to_owned(),
        relative_path: relative_path.to_owned(),
    })?;
    if format!("{:x}", Sha256::digest(contents)) != file.sha256 {
        return Err(CatalogUnavailable::CorruptResource {
            adapter_id: adapter_id.to_owned(),
            relative_path: relative_path.to_owned(),
        });
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_safe_receipt_path(value: &str) -> bool {
    let path = Path::new(value);
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
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
    let bytes = fs::read(path).map_err(|_| CatalogUnavailable::MissingResource {
        adapter_id: adapter_id.to_owned(),
        relative_path: relative_path.to_owned(),
    })?;
    if read_u32(&bytes, 0) != Some(MACHO_64_LE) {
        return Err(corrupt_executable(adapter_id, relative_path));
    }
    let cpu_type =
        read_u32(&bytes, 4).ok_or_else(|| corrupt_executable(adapter_id, relative_path))?;
    if cpu_type != ARM64 {
        return Err(CatalogUnavailable::UnsupportedArchitecture {
            adapter_id: adapter_id.to_owned(),
            expected: "arm64".to_owned(),
            actual: cpu_type_name(cpu_type).to_owned(),
        });
    }
    if read_u32(&bytes, 12) != Some(MH_EXECUTE) {
        return Err(corrupt_executable(adapter_id, relative_path));
    }
    let ncmds =
        read_u32(&bytes, 16).ok_or_else(|| corrupt_executable(adapter_id, relative_path))?;
    let sizeofcmds =
        read_u32(&bytes, 20).ok_or_else(|| corrupt_executable(adapter_id, relative_path))?;
    if ncmds == 0 || ncmds > 1_024 || sizeofcmds < ncmds.saturating_mul(8) {
        return Err(corrupt_executable(adapter_id, relative_path));
    }
    let command_table_end = 32usize
        .checked_add(sizeofcmds as usize)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| corrupt_executable(adapter_id, relative_path))?;
    let mut offset = 32usize;
    let mut platform = None;
    for _ in 0..ncmds {
        let command = read_u32(&bytes, offset)
            .ok_or_else(|| corrupt_executable(adapter_id, relative_path))?;
        let size = read_u32(&bytes, offset + 4)
            .ok_or_else(|| corrupt_executable(adapter_id, relative_path))?
            as usize;
        let command_end = offset
            .checked_add(size)
            .filter(|end| size >= 8 && *end <= command_table_end)
            .ok_or_else(|| corrupt_executable(adapter_id, relative_path))?;
        if command == LC_BUILD_VERSION {
            if size < 24 {
                return Err(corrupt_executable(adapter_id, relative_path));
            }
            platform = read_u32(&bytes, offset + 8);
        }
        offset = command_end;
    }
    if offset != command_table_end {
        return Err(corrupt_executable(adapter_id, relative_path));
    }
    match platform {
        Some(PLATFORM_MACOS) => Ok(()),
        Some(actual) => Err(CatalogUnavailable::UnsupportedPlatform {
            adapter_id: adapter_id.to_owned(),
            expected: "macOS".to_owned(),
            actual: platform_name(actual).to_owned(),
        }),
        None => Err(corrupt_executable(adapter_id, relative_path)),
    }
}

fn corrupt_executable(adapter_id: &str, relative_path: &Path) -> CatalogUnavailable {
    CatalogUnavailable::CorruptResource {
        adapter_id: adapter_id.to_owned(),
        relative_path: relative_path.to_owned(),
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

const TYPESCRIPT_FILE_BINDINGS: &[FileBinding] = &[
    FileBinding {
        pattern: FilePattern::Extension("js"),
        language: LanguageId::JavaScript,
        lsp_language_id: "javascript",
    },
    FileBinding {
        pattern: FilePattern::Extension("mjs"),
        language: LanguageId::JavaScript,
        lsp_language_id: "javascript",
    },
    FileBinding {
        pattern: FilePattern::Extension("cjs"),
        language: LanguageId::JavaScript,
        lsp_language_id: "javascript",
    },
    FileBinding {
        pattern: FilePattern::Extension("jsx"),
        language: LanguageId::JavaScript,
        lsp_language_id: "javascriptreact",
    },
    FileBinding {
        pattern: FilePattern::Extension("ts"),
        language: LanguageId::TypeScript,
        lsp_language_id: "typescript",
    },
    FileBinding {
        pattern: FilePattern::Extension("mts"),
        language: LanguageId::TypeScript,
        lsp_language_id: "typescript",
    },
    FileBinding {
        pattern: FilePattern::Extension("cts"),
        language: LanguageId::TypeScript,
        lsp_language_id: "typescript",
    },
    FileBinding {
        pattern: FilePattern::Extension("tsx"),
        language: LanguageId::TypeScript,
        lsp_language_id: "typescriptreact",
    },
];

const JSON_FILE_BINDINGS: &[FileBinding] = &[
    FileBinding {
        pattern: FilePattern::Extension("json"),
        language: LanguageId::Json,
        lsp_language_id: "json",
    },
    FileBinding {
        pattern: FilePattern::Extension("jsonc"),
        language: LanguageId::Json,
        lsp_language_id: "jsonc",
    },
    FileBinding {
        pattern: FilePattern::FileName("package.json"),
        language: LanguageId::Json,
        lsp_language_id: "json",
    },
];

const PYTHON_FILE_BINDINGS: &[FileBinding] = &[
    FileBinding {
        pattern: FilePattern::Extension("py"),
        language: LanguageId::Python,
        lsp_language_id: "python",
    },
    FileBinding {
        pattern: FilePattern::Extension("pyi"),
        language: LanguageId::Python,
        lsp_language_id: "python",
    },
];

const RUST_FILE_BINDINGS: &[FileBinding] = &[FileBinding {
    pattern: FilePattern::Extension("rs"),
    language: LanguageId::Rust,
    lsp_language_id: "rust",
}];

const GO_FILE_BINDINGS: &[FileBinding] = &[FileBinding {
    pattern: FilePattern::Extension("go"),
    language: LanguageId::Go,
    lsp_language_id: "go",
}];

const CLANGD_FILE_BINDINGS: &[FileBinding] = &[
    FileBinding {
        pattern: FilePattern::Extension("c"),
        language: LanguageId::C,
        lsp_language_id: "c",
    },
    FileBinding {
        pattern: FilePattern::Extension("h"),
        language: LanguageId::C,
        lsp_language_id: "c",
    },
    FileBinding {
        pattern: FilePattern::Extension("cc"),
        language: LanguageId::Cpp,
        lsp_language_id: "cpp",
    },
    FileBinding {
        pattern: FilePattern::Extension("cp"),
        language: LanguageId::Cpp,
        lsp_language_id: "cpp",
    },
    FileBinding {
        pattern: FilePattern::Extension("cpp"),
        language: LanguageId::Cpp,
        lsp_language_id: "cpp",
    },
    FileBinding {
        pattern: FilePattern::Extension("cxx"),
        language: LanguageId::Cpp,
        lsp_language_id: "cpp",
    },
    FileBinding {
        pattern: FilePattern::Extension("hpp"),
        language: LanguageId::Cpp,
        lsp_language_id: "cpp",
    },
    FileBinding {
        pattern: FilePattern::Extension("hh"),
        language: LanguageId::Cpp,
        lsp_language_id: "cpp",
    },
    FileBinding {
        pattern: FilePattern::Extension("hxx"),
        language: LanguageId::Cpp,
        lsp_language_id: "cpp",
    },
];

const JAVA_FILE_BINDINGS: &[FileBinding] = &[FileBinding {
    pattern: FilePattern::Extension("java"),
    language: LanguageId::Java,
    lsp_language_id: "java",
}];

const DESCRIPTORS: &[AdapterDescriptor] = &[
    AdapterDescriptor {
        adapter_id: "typescript",
        display_name: "TypeScript and JavaScript",
        file_bindings: TYPESCRIPT_FILE_BINDINGS,
        root_strategy: RootStrategy::JavaScript,
        program: ProgramLayout::Node {
            executable_relative_path: "node/bin/node",
            arguments: TYPESCRIPT_ARGUMENTS,
            required_files: TYPESCRIPT_REQUIRED_FILES,
        },
        initialization_options_json: "{}",
        workspace_configuration_json: "{\"typescript\":{},\"javascript\":{}}",
        completion_trigger_characters: &[".", "'", "\"", "/", "@", "<"],
        trigger_normalization: TriggerNormalizationPolicy::MergeWithServer,
        metadata: PackagedMetadata {
            version: "typescript-language-server 6.0.0; typescript 7.0.2; node 24.19.0",
            upstream_url: "https://github.com/typescript-language-server/typescript-language-server",
            license: "MIT; TypeScript Apache-2.0; Node MIT",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::Bundled,
        timeouts: AdapterTimeouts {
            initialize: Duration::from_secs(60),
            shutdown: Duration::from_secs(3),
            smoke_test: Duration::from_secs(10),
        },
    },
    AdapterDescriptor {
        adapter_id: "json",
        display_name: "JSON",
        file_bindings: JSON_FILE_BINDINGS,
        root_strategy: RootStrategy::Json,
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
        trigger_normalization: TriggerNormalizationPolicy::MergeWithServer,
        metadata: PackagedMetadata {
            version: "not bundled",
            upstream_url: "https://github.com/microsoft/vscode-languageserver-node",
            license: "MIT",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::NotBundledYet,
        timeouts: AdapterTimeouts {
            initialize: Duration::from_secs(60),
            shutdown: Duration::from_secs(3),
            smoke_test: Duration::from_secs(10),
        },
    },
    AdapterDescriptor {
        adapter_id: "python",
        display_name: "Python",
        file_bindings: PYTHON_FILE_BINDINGS,
        root_strategy: RootStrategy::Python,
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
        trigger_normalization: TriggerNormalizationPolicy::MergeWithServer,
        metadata: PackagedMetadata {
            version: "not bundled",
            upstream_url: "https://github.com/microsoft/pyright",
            license: "MIT",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::NotBundledYet,
        timeouts: AdapterTimeouts {
            initialize: Duration::from_secs(60),
            shutdown: Duration::from_secs(3),
            smoke_test: Duration::from_secs(10),
        },
    },
    AdapterDescriptor {
        adapter_id: "rust",
        display_name: "Rust",
        file_bindings: RUST_FILE_BINDINGS,
        root_strategy: RootStrategy::Rust,
        program: ProgramLayout::Native {
            executable_relative_path: "rust-analyzer/rust-analyzer",
            arguments: &[],
        },
        initialization_options_json: "{}",
        workspace_configuration_json: "{\"rust-analyzer\":{}}",
        completion_trigger_characters: &[".", ":", "<"],
        trigger_normalization: TriggerNormalizationPolicy::MergeWithServer,
        metadata: PackagedMetadata {
            version: "2026-08-24",
            upstream_url: "https://github.com/rust-lang/rust-analyzer",
            license: "MIT OR Apache-2.0",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::Bundled,
        timeouts: AdapterTimeouts {
            initialize: Duration::from_secs(60),
            shutdown: Duration::from_secs(3),
            smoke_test: Duration::from_secs(10),
        },
    },
    AdapterDescriptor {
        adapter_id: "go",
        display_name: "Go",
        file_bindings: GO_FILE_BINDINGS,
        root_strategy: RootStrategy::Go,
        program: ProgramLayout::Native {
            executable_relative_path: "gopls/gopls",
            arguments: &[],
        },
        initialization_options_json: "{}",
        workspace_configuration_json: "{\"gopls\":{}}",
        completion_trigger_characters: &["."],
        trigger_normalization: TriggerNormalizationPolicy::MergeWithServer,
        metadata: PackagedMetadata {
            version: "not bundled",
            upstream_url: "https://github.com/golang/tools/tree/master/gopls",
            license: "BSD-3-Clause",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::NotBundledYet,
        timeouts: AdapterTimeouts {
            initialize: Duration::from_secs(60),
            shutdown: Duration::from_secs(3),
            smoke_test: Duration::from_secs(10),
        },
    },
    AdapterDescriptor {
        adapter_id: "clangd",
        display_name: "C and C++",
        file_bindings: CLANGD_FILE_BINDINGS,
        root_strategy: RootStrategy::Clangd,
        program: ProgramLayout::Native {
            executable_relative_path: "clangd/clangd",
            arguments: &[],
        },
        initialization_options_json: "{}",
        workspace_configuration_json: "{\"clangd\":{}}",
        completion_trigger_characters: &[".", ":", ">"],
        trigger_normalization: TriggerNormalizationPolicy::MergeWithServer,
        metadata: PackagedMetadata {
            version: "not bundled",
            upstream_url: "https://clangd.llvm.org/",
            license: "Apache-2.0 WITH LLVM-exception",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::NotBundledYet,
        timeouts: AdapterTimeouts {
            initialize: Duration::from_secs(60),
            shutdown: Duration::from_secs(3),
            smoke_test: Duration::from_secs(10),
        },
    },
    AdapterDescriptor {
        adapter_id: "java",
        display_name: "Java",
        file_bindings: JAVA_FILE_BINDINGS,
        root_strategy: RootStrategy::Java,
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
        trigger_normalization: TriggerNormalizationPolicy::StaticOnly,
        metadata: PackagedMetadata {
            version: "not bundled",
            upstream_url: "https://projects.eclipse.org/projects/eclipse.jdt.ls",
            license: "EPL-2.0",
            notices_file: "THIRD_PARTY_NOTICES.md",
        },
        availability: PocAvailability::NotBundledYet,
        timeouts: AdapterTimeouts {
            initialize: Duration::from_secs(120),
            shutdown: Duration::from_secs(3),
            smoke_test: Duration::from_secs(20),
        },
    },
];

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::{
        BundledServerCatalog, CatalogUnavailable, HostArchitecture, HostOperatingSystem,
        HostPlatform, ResourceResolutionMode, ResourceRootCandidates, ResourceRootResolver,
        resource_path,
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
    fn typed_file_bindings_unambiguously_choose_javascript_and_typescript_lsp_ids() {
        let catalog = BundledServerCatalog::new();

        for (name, language_id) in [
            ("component.js", "javascript"),
            ("component.jsx", "javascriptreact"),
            ("component.ts", "typescript"),
            ("component.tsx", "typescriptreact"),
        ] {
            let binding = catalog
                .file_binding(Path::new(name))
                .expect("typed file binding");
            assert_eq!(binding.adapter_id, "typescript", "{name}");
            assert_eq!(binding.lsp_language_id, language_id, "{name}");
        }
        assert_eq!(
            catalog.descriptor(LanguageId::Rust).root_strategy.id(),
            "rust"
        );
        assert_eq!(
            catalog.descriptor(LanguageId::Json).root_strategy.id(),
            "json"
        );
        assert_eq!(catalog.descriptor(LanguageId::Go).root_strategy.id(), "go");
        assert!(
            catalog
                .descriptor(LanguageId::C)
                .root_strategy
                .markers()
                .contains(&".clangd")
        );
        assert!(
            catalog
                .descriptor(LanguageId::Java)
                .root_strategy
                .markers()
                .contains(&".project")
        );
        assert_eq!(
            catalog.normalize_completion_triggers(
                LanguageId::TypeScript,
                &[".".into(), "?".into(), "?".into()],
            ),
            vec![".", "'", "\"", "/", "@", "<", "?"]
        );
    }

    #[test]
    fn descriptors_own_initialize_shutdown_and_smoke_test_timeouts() {
        let catalog = BundledServerCatalog::new();

        let typescript = catalog.descriptor(LanguageId::TypeScript);
        assert_eq!(typescript.timeouts.initialize, Duration::from_secs(60));
        assert_eq!(typescript.timeouts.shutdown, Duration::from_secs(3));
        assert_eq!(typescript.timeouts.smoke_test, Duration::from_secs(10));
        assert_eq!(
            catalog.descriptor(LanguageId::Java).timeouts.initialize,
            Duration::from_secs(120)
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
        resources.write_receipt();
        assert_eq!(
            catalog.resolve(LanguageId::Rust, resources.root()),
            Err(CatalogUnavailable::ProgramNotExecutable {
                adapter_id: "rust".to_owned(),
                relative_path: PathBuf::from("rust-analyzer/rust-analyzer"),
            })
        );
    }

    #[test]
    fn resolution_requires_a_versioned_receipt_and_rejects_corrupt_required_files() {
        let resources = ResourceTree::new_without_receipt();
        let catalog = BundledServerCatalog::new();

        assert_eq!(
            catalog.resolve(LanguageId::Rust, resources.root()),
            Err(CatalogUnavailable::MissingReceipt {
                adapter_id: "rust".to_owned(),
            })
        );

        resources.write_receipt();
        fs::write(
            resources
                .root()
                .join("typescript/node_modules/typescript/lib/typescript.js"),
            b"corrupt runtime",
        )
        .expect("corrupt TypeScript runtime");
        assert_eq!(
            catalog.resolve(LanguageId::TypeScript, resources.root()),
            Err(CatalogUnavailable::CorruptResource {
                adapter_id: "typescript".to_owned(),
                relative_path: PathBuf::from(
                    "typescript/node_modules/typescript/lib/typescript.js"
                ),
            })
        );

        let empty_cli_resources = ResourceTree::new();
        fs::write(
            empty_cli_resources
                .root()
                .join("typescript/node_modules/typescript-language-server/lib/cli.mjs"),
            b"",
        )
        .expect("empty TypeScript language server CLI");
        empty_cli_resources.write_receipt();
        assert_eq!(
            catalog.resolve(LanguageId::TypeScript, empty_cli_resources.root()),
            Err(CatalogUnavailable::CorruptResource {
                adapter_id: "typescript".to_owned(),
                relative_path: PathBuf::from(
                    "typescript/node_modules/typescript-language-server/lib/cli.mjs",
                ),
            })
        );

        assert_eq!(
            CatalogUnavailable::MissingReceipt {
                adapter_id: "rust".to_owned(),
            }
            .lsp_reason(),
            crate::lsp::types::LspUnavailableReason::MissingResource {
                adapter_id: "rust".to_owned(),
                relative_path: "manifest.json".to_owned(),
            }
        );
    }

    #[test]
    fn validation_rejects_header_only_truncated_and_non_executable_macho_programs() {
        let resources = ResourceTree::new();
        let catalog = BundledServerCatalog::new();

        resources.install_rust_analyzer_bytes(&macho_header_only(), true);
        resources.write_receipt();
        assert_eq!(
            catalog.resolve(LanguageId::Rust, resources.root()),
            Err(CatalogUnavailable::CorruptResource {
                adapter_id: "rust".to_owned(),
                relative_path: PathBuf::from("rust-analyzer/rust-analyzer"),
            })
        );

        resources.install_rust_analyzer_bytes(&macho_with_file_type(1), true);
        resources.write_receipt();
        assert_eq!(
            catalog.resolve(LanguageId::Rust, resources.root()),
            Err(CatalogUnavailable::CorruptResource {
                adapter_id: "rust".to_owned(),
                relative_path: PathBuf::from("rust-analyzer/rust-analyzer"),
            })
        );
    }

    #[test]
    fn resolution_fails_before_resource_lookup_on_an_unsupported_host() {
        let resources = ResourceTree::new();
        let catalog = BundledServerCatalog::new();

        assert_eq!(
            catalog.resolve_for_host(
                LanguageId::Rust,
                resources.root(),
                HostPlatform::new(HostOperatingSystem::Linux, HostArchitecture::Arm64),
            ),
            Err(CatalogUnavailable::UnsupportedPlatform {
                adapter_id: "rust".to_owned(),
                expected: "macOS".to_owned(),
                actual: "linux".to_owned(),
            })
        );
        assert_eq!(
            catalog.resolve_for_host(
                LanguageId::Rust,
                resources.root(),
                HostPlatform::new(HostOperatingSystem::MacOs, HostArchitecture::X86_64),
            ),
            Err(CatalogUnavailable::UnsupportedArchitecture {
                adapter_id: "rust".to_owned(),
                expected: "arm64".to_owned(),
                actual: "x86_64".to_owned(),
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
        resources.write_receipt();
        assert_eq!(
            catalog.resolve(LanguageId::Rust, resources.root()),
            Err(CatalogUnavailable::UnsupportedArchitecture {
                adapter_id: "rust".to_owned(),
                expected: "arm64".to_owned(),
                actual: "x86_64".to_owned(),
            })
        );

        resources.install_rust_analyzer_with_header(0x0100_000c, 2, true);
        resources.write_receipt();
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

    #[test]
    fn cache_paths_reject_a_cache_root_inside_the_project_even_via_a_symlink() {
        let project = TempDir::new().expect("project directory");
        let cache_link = TempDir::new().expect("link holder");
        let nested_cache = project.path().join(".termlab-cache");
        fs::create_dir(&nested_cache).expect("nested cache directory");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&nested_cache, cache_link.path().join("cache"))
            .expect("cache symlink");

        let catalog = BundledServerCatalog::new();
        let result = catalog.cache_paths(
            LanguageId::Rust,
            &project.path().canonicalize().expect("canonical project"),
            &cache_link.path().join("cache"),
        );

        assert!(matches!(
            result,
            Err(CatalogUnavailable::CacheRootInsideProject { .. })
        ));
    }

    struct ResourceTree {
        directory: TempDir,
    }

    impl ResourceTree {
        fn new() -> Self {
            let tree = Self::new_without_receipt();
            tree.write_receipt();
            tree
        }

        fn new_without_receipt() -> Self {
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

        fn install_rust_analyzer_bytes(&self, bytes: &[u8], executable: bool) {
            write_file(&self.root().join("rust-analyzer/rust-analyzer"), bytes);
            set_executable(&self.root().join("rust-analyzer/rust-analyzer"), executable);
        }

        fn write_receipt(&self) {
            write_receipt(self.root());
        }
    }

    fn write_file(path: &Path, contents: &[u8]) {
        fs::create_dir_all(path.parent().expect("parent directory")).expect("create parent");
        fs::write(path, contents).expect("write file");
    }

    fn write_macho_binary(path: &Path, cpu_type: u32, platform: u32, executable: bool) {
        write_file(path, &macho_bytes(cpu_type, 2, platform));
        set_executable(path, executable);
    }

    fn macho_header_only() -> Vec<u8> {
        let mut bytes = macho_bytes(0x0100_000c, 2, 1);
        bytes.truncate(32);
        bytes
    }

    fn macho_with_file_type(file_type: u32) -> Vec<u8> {
        macho_bytes(0x0100_000c, file_type, 1)
    }

    fn macho_bytes(cpu_type: u32, file_type: u32, platform: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0xfeed_facfu32.to_le_bytes());
        bytes.extend_from_slice(&cpu_type.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&file_type.to_le_bytes());
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
        bytes
    }

    fn set_executable(path: &Path, executable: bool) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(
                path,
                fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
            )
            .expect("set executable permission");
        }
        #[cfg(not(unix))]
        let _ = (path, executable);
    }

    fn write_receipt(root: &Path) {
        let files = [
            "node/bin/node",
            "typescript/node_modules/typescript-language-server/lib/cli.mjs",
            "typescript/node_modules/typescript/lib/typescript.js",
            "rust-analyzer/rust-analyzer",
        ]
        .into_iter()
        .map(|relative_path| {
            let bytes = fs::read(root.join(relative_path)).expect("receipt file contents");
            serde_json::json!({
                "relativePath": relative_path,
                "sha256": format!("{:x}", Sha256::digest(&bytes)),
                "size": bytes.len(),
            })
        })
        .collect::<Vec<_>>();
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": 1,
                "platform": "macos",
                "architecture": "arm64",
                "artifacts": [
                    { "id": "node", "version": "24.19.0" },
                    { "id": "typescript-language-server", "version": "6.0.0" },
                    { "id": "typescript", "version": "7.0.2" },
                    { "id": "rust-analyzer", "version": "2026-08-24" },
                ],
                "files": files,
            }))
            .expect("receipt JSON"),
        )
        .expect("write receipt");
    }
}
