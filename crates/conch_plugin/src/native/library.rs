//! Low-level shared library loading and symbol resolution.
//!
//! [`PluginLibrary`] wraps a `libloading::Library` and resolves all six C ABI
//! symbols that every Conch native plugin must export.

use std::ffi::{c_char, c_void};
use std::path::Path;

use conch_plugin_sdk::{HostApi, PluginInfo};

use super::{LoadError, PluginMeta};

// ---------------------------------------------------------------------------
// Function pointer type aliases (matching the declare_plugin! signatures)
// ---------------------------------------------------------------------------

type InfoFn = unsafe extern "C" fn() -> PluginInfo;
type SetupFn = unsafe extern "C" fn(*const HostApi) -> *mut c_void;
type EventFn = unsafe extern "C" fn(*mut c_void, *const c_char, usize);
type RenderFn = unsafe extern "C" fn(*mut c_void) -> *const c_char;
type TeardownFn = unsafe extern "C" fn(*mut c_void);
type QueryFn =
    unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, usize) -> *mut c_char;

// ---------------------------------------------------------------------------
// PluginLibrary
// ---------------------------------------------------------------------------

/// A loaded shared library with all six plugin symbols resolved.
///
/// The `Library` is kept alive so the function pointers remain valid.
/// `PluginLibrary` is `Send` (can be moved to the plugin thread) but not
/// `Sync` (should not be shared across threads).
pub struct PluginLibrary {
    /// Kept alive to prevent the OS from unloading the library.
    _library: libloading::Library,

    pub(crate) info_fn: InfoFn,
    pub(crate) setup_fn: SetupFn,
    pub(crate) event_fn: EventFn,
    pub(crate) render_fn: RenderFn,
    pub(crate) teardown_fn: TeardownFn,
    pub(crate) query_fn: QueryFn,
}

// SAFETY: All fields are function pointers (trivially Send) and an owned
// Library (which is Send). We only use PluginLibrary from one thread at a time.
unsafe impl Send for PluginLibrary {}

impl PluginLibrary {
    /// Open a shared library and resolve all required symbols.
    ///
    /// # Safety
    ///
    /// Loading a shared library can execute arbitrary code in its
    /// constructors. Only load libraries from trusted sources.
    pub unsafe fn load(path: &Path) -> Result<Self, LoadError> {
        let library = unsafe { libloading::Library::new(path)? };

        let info_fn = Self::resolve_fn::<InfoFn>(&library, b"conch_plugin_info\0")?;
        let setup_fn = Self::resolve_fn::<SetupFn>(&library, b"conch_plugin_setup\0")?;
        let event_fn = Self::resolve_fn::<EventFn>(&library, b"conch_plugin_event\0")?;
        let render_fn = Self::resolve_fn::<RenderFn>(&library, b"conch_plugin_render\0")?;
        let teardown_fn = Self::resolve_fn::<TeardownFn>(&library, b"conch_plugin_teardown\0")?;
        let query_fn = Self::resolve_fn::<QueryFn>(&library, b"conch_plugin_query\0")?;

        Ok(Self {
            _library: library,
            info_fn,
            setup_fn,
            event_fn,
            render_fn,
            teardown_fn,
            query_fn,
        })
    }

    /// Call `conch_plugin_info()` and return an owned copy of the metadata.
    ///
    /// # Safety
    ///
    /// The plugin's `conch_plugin_info` must return a valid `PluginInfo` with
    /// valid string pointers.
    pub unsafe fn read_info(&self) -> PluginMeta {
        let raw = unsafe { (self.info_fn)() };
        unsafe { PluginMeta::from_raw(&raw) }
    }

    /// Resolve a single symbol from the library.
    fn resolve_fn<T: Copy>(
        library: &libloading::Library,
        symbol_name: &[u8],
    ) -> Result<T, LoadError> {
        unsafe {
            let sym: libloading::Symbol<'_, T> = library.get(symbol_name).map_err(|_| {
                // Strip the trailing NUL for the error message.
                let name =
                    std::str::from_utf8(&symbol_name[..symbol_name.len() - 1]).unwrap_or("???");
                LoadError::SymbolNotFound(
                    // SAFETY: We only ever pass static byte-string literals.
                    std::mem::transmute::<&str, &'static str>(name),
                )
            })?;
            Ok(*sym)
        }
    }
}

/// Scan a directory for shared libraries that match the platform's extension.
///
/// Does **not** load or validate the libraries — just returns paths.
pub fn discover_library_paths(dir: &Path) -> Result<Vec<std::path::PathBuf>, LoadError> {
    let ext = std::env::consts::DLL_EXTENSION;
    let mut paths = Vec::new();

    if !dir.is_dir() {
        return Ok(paths);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == ext) && path.is_file() {
            paths.push(path);
        }
    }

    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discover_empty_dir() {
        let dir = std::env::temp_dir().join("conch_test_discover_empty");
        let _ = std::fs::create_dir(&dir);
        let paths = discover_library_paths(&dir).unwrap();
        assert!(paths.is_empty());
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn discover_nonexistent_dir_returns_empty() {
        let paths = discover_library_paths(Path::new("/nonexistent/path")).unwrap();
        assert!(paths.is_empty());
    }

    #[test]
    fn discover_filters_by_extension() {
        let dir = std::env::temp_dir().join("conch_test_discover_ext");
        let _ = std::fs::create_dir_all(&dir);

        let ext = std::env::consts::DLL_EXTENSION;
        let good = dir.join(format!("plugin.{ext}"));
        let bad = dir.join("plugin.txt");
        std::fs::write(&good, b"fake").unwrap();
        std::fs::write(&bad, b"fake").unwrap();

        let paths = discover_library_paths(&dir).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].file_name().unwrap().to_str().unwrap(), format!("plugin.{ext}"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_nonexistent_library_errors() {
        let result = unsafe { PluginLibrary::load(Path::new("/nonexistent/plugin.dylib")) };
        assert!(result.is_err());
    }

    #[test]
    fn load_error_from_libloading() {
        let err = LoadError::Library(libloading::Error::DlOpen {
            desc: c"test error".into(),
        });
        assert!(err.to_string().contains("test error"));
    }
}
