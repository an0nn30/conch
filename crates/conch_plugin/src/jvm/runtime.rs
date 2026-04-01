//! JVM plugin manager — discovery, loading, and lifecycle for `.jar` plugins.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jfloat, jint, jobject};
use jni::{InitArgsBuilder, JNIEnv, JavaVM, NativeMethod};
use tokio::sync::mpsc;

use crate::HostApi;
use crate::bus::{PluginBus, PluginMail, QueryResponse};

// Types previously in `native/` — inlined here since native plugins were removed.

/// Metadata about a discovered/loaded plugin.
#[derive(Debug, Clone)]
pub struct PluginMeta {
    pub name: String,
    pub description: String,
    pub version: String,
    pub api_required: Option<String>,
    pub permissions: Vec<String>,
    pub plugin_type: conch_plugin_sdk::PluginType,
    pub panel_location: conch_plugin_sdk::PanelLocation,
}

/// Error type for plugin loading operations.
#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    AlreadyLoaded(String),
    NotLoaded(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::AlreadyLoaded(n) => write!(f, "plugin '{n}' already loaded"),
            Self::NotLoaded(n) => write!(f, "plugin '{n}' not loaded"),
        }
    }
}

impl std::error::Error for LoadError {}

/// A loaded and running plugin.
struct LoadedPlugin {
    meta: PluginMeta,
    sender: tokio::sync::mpsc::Sender<PluginMail>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl LoadedPlugin {
    fn join(&mut self) {
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Global trait-based HostApi. Set once by `JavaPluginManager::new()`.
/// JNI native methods call trait methods through this.
static TRAIT_HOST_API: OnceLock<Arc<dyn HostApi>> = OnceLock::new();

/// Per-thread plugin name so JNI native methods can attribute actions
/// (e.g. menu item registration) to the correct plugin instead of the
/// shared "java" HostApi name.
thread_local! {
    static CURRENT_PLUGIN_NAME: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The SDK JAR is embedded in the binary at compile time.
/// It's written to a temp file on first JVM startup.
static SDK_JAR_BYTES: &[u8] = include_bytes!("../../../../java-sdk/build/conch-plugin-sdk.jar");

/// Manages JVM plugin discovery, loading, and lifecycle.
pub struct JavaPluginManager {
    bus: Arc<PluginBus>,
    plugins: HashMap<String, LoadedPlugin>,
    jvm: Option<JavaVM>,
    /// Temp file holding the extracted SDK JAR. Kept alive for JVM lifetime.
    _sdk_jar_tempfile: Option<tempfile::NamedTempFile>,
}

// SAFETY: JavaVM is Send+Sync. The trait HostApi is stored in a global OnceLock.
unsafe impl Send for JavaPluginManager {}

impl JavaPluginManager {
    pub fn new(bus: Arc<PluginBus>, host_api: Arc<dyn HostApi>) -> Self {
        let _ = TRAIT_HOST_API.set(host_api);

        Self {
            bus,
            plugins: HashMap::new(),
            jvm: None,
            _sdk_jar_tempfile: None,
        }
    }

    /// Lazily create the JVM with the embedded SDK JAR on the classpath.
    fn ensure_jvm(&mut self) -> Result<&JavaVM, LoadError> {
        if let Some(ref jvm) = self.jvm {
            return Ok(jvm);
        }

        // Write the embedded SDK JAR to a temp file.
        use std::io::Write;
        let mut tmpfile = tempfile::Builder::new()
            .prefix("conch-plugin-sdk-")
            .suffix(".jar")
            .tempfile()
            .map_err(LoadError::Io)?;
        tmpfile.write_all(SDK_JAR_BYTES).map_err(LoadError::Io)?;
        tmpfile.flush().map_err(LoadError::Io)?;

        let classpath = format!("-Djava.class.path={}", tmpfile.path().display());
        log::info!(
            "jvm: starting JVM with embedded SDK JAR ({} bytes) at {}",
            SDK_JAR_BYTES.len(),
            tmpfile.path().display()
        );

        let jvm_args = InitArgsBuilder::new()
            .version(jni::JNIVersion::V8)
            .option(&classpath)
            .build()
            .map_err(|e| {
                LoadError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("JVM init args: {e}"),
                ))
            })?;

        let jvm = JavaVM::new(jvm_args).map_err(|e| {
            LoadError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("JVM creation failed: {e}"),
            ))
        })?;

        // Register native methods for HostApi.
        {
            let mut env = jvm.attach_current_thread().map_err(|e| {
                LoadError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("JNI attach failed: {e}"),
                ))
            })?;
            register_host_natives(&mut env)?;
        }

        log::info!("jvm: JVM started successfully");
        self._sdk_jar_tempfile = Some(tmpfile);
        self.jvm = Some(jvm);
        self.jvm.as_ref().ok_or_else(|| {
            LoadError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "JVM initialization failed unexpectedly",
            ))
        })
    }

    /// Scan a directory for `.jar` files and probe their metadata.
    pub fn discover(&mut self, dir: &Path) -> Vec<(PathBuf, PluginMeta)> {
        let mut found = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return found,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jar") {
                continue;
            }
            log::debug!("[jvm] probing JAR: {}", path.display());
            match self.probe_jar_metadata(&path) {
                Ok(meta) => {
                    log::debug!("[jvm] found plugin: {} v{}", meta.name, meta.version);
                    found.push((path, meta));
                }
                Err(e) => log::warn!("[jvm] FAILED to probe {}: {e}", path.display()),
            }
        }
        found
    }

    /// Get the plugin name from a JAR without fully loading it.
    pub fn probe_jar_name(&mut self, jar_path: &Path) -> Option<String> {
        self.probe_jar_metadata(jar_path).ok().map(|m| m.name)
    }

    /// Get the plugin API requirement from a JAR manifest (`Plugin-Api`).
    pub fn probe_jar_api_requirement(&self, jar_path: &Path) -> Option<String> {
        read_manifest_attr_from_jar(jar_path, "Plugin-Api")
            .ok()
            .flatten()
    }

    /// Get declared permission capabilities from JAR manifest (`Plugin-Permissions`).
    pub fn probe_jar_permissions(&self, jar_path: &Path) -> Vec<String> {
        read_manifest_attr_from_jar(jar_path, "Plugin-Permissions")
            .ok()
            .flatten()
            .map(|csv| parse_manifest_permissions(&csv))
            .unwrap_or_default()
    }

    /// Read plugin metadata from a JAR by loading it in the JVM.
    fn probe_jar_metadata(&mut self, jar_path: &Path) -> Result<PluginMeta, LoadError> {
        let class_name = read_plugin_class_from_jar(jar_path)?;
        let api_required = self.probe_jar_api_requirement(jar_path);
        let permissions = self.probe_jar_permissions(jar_path);

        let jvm = self.ensure_jvm()?;
        let mut env = jvm.attach_current_thread().map_err(|e| {
            LoadError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("JNI attach: {e}"),
            ))
        })?;

        // Clear any pending exception from a previous probe.
        let _ = env.exception_clear();

        // Load the JAR via URLClassLoader.
        let loader = match create_url_classloader(&mut env, jar_path) {
            Ok(l) => l,
            Err(e) => {
                describe_java_exception(&mut env);
                return Err(e);
            }
        };
        let plugin_obj = match instantiate_plugin(&mut env, &loader, &class_name) {
            Ok(o) => o,
            Err(e) => {
                describe_java_exception(&mut env);
                return Err(e);
            }
        };

        // Call getInfo().
        let info_obj =
            match env.call_method(&plugin_obj, "getInfo", "()Lconch/plugin/PluginInfo;", &[]) {
                Ok(v) => match v.l() {
                    Ok(o) => o,
                    Err(e) => {
                        describe_java_exception(&mut env);
                        return Err(jni_err(format!("getInfo obj: {e}")));
                    }
                },
                Err(e) => {
                    describe_java_exception(&mut env);
                    return Err(jni_err(format!("getInfo: {e}")));
                }
            };

        let mut meta = read_plugin_info(&mut env, &info_obj)?;
        meta.api_required = api_required;
        meta.permissions = permissions;
        Ok(meta)
    }

    /// Load and activate a Java plugin from a JAR.
    pub fn load_plugin(&mut self, jar_path: &Path) -> Result<PluginMeta, LoadError> {
        let class_name = read_plugin_class_from_jar(jar_path)?;
        let meta = self.probe_jar_metadata(jar_path)?;
        let name = meta.name.clone();

        if self.plugins.contains_key(&name) {
            return Err(LoadError::AlreadyLoaded(name));
        }

        self.ensure_jvm()?;
        let jvm = self.jvm.as_ref().ok_or_else(|| {
            LoadError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "JVM not available after ensure_jvm",
            ))
        })?;

        // Create the plugin object on this thread, convert to GlobalRef for the plugin thread.
        let plugin_global = {
            let mut env = jvm.attach_current_thread().map_err(|e| {
                LoadError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("JNI attach: {e}"),
                ))
            })?;
            let loader = create_url_classloader(&mut env, jar_path)?;
            let plugin_obj = instantiate_plugin(&mut env, &loader, &class_name)?;
            env.new_global_ref(&plugin_obj).map_err(|e| {
                LoadError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("global ref: {e}"),
                ))
            })?
        };

        // Register on the bus.
        let mailbox_rx = self.bus.register_plugin(&name);
        let sender = self.bus.sender_for(&name).ok_or_else(|| {
            LoadError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("bus sender not found for plugin '{name}'"),
            ))
        })?;

        let jvm_ptr = jvm as *const JavaVM as usize;
        let thread_name = name.clone();
        let thread_plugin_name = name.clone();
        let thread_meta = meta.clone();

        let handle = std::thread::Builder::new()
            .name(format!("plugin:{thread_name}"))
            .spawn(move || {
                let jvm = unsafe { &*(jvm_ptr as *const JavaVM) };
                java_plugin_thread(
                    jvm,
                    plugin_global,
                    mailbox_rx,
                    thread_plugin_name,
                    &thread_meta,
                );
            })
            .map_err(LoadError::Io)?;

        self.plugins.insert(
            name,
            LoadedPlugin {
                meta: meta.clone(),
                sender,
                thread_handle: Some(handle),
            },
        );

        log::info!("jvm: loaded plugin: {} v{}", meta.name, meta.version);
        Ok(meta)
    }

    pub fn unload_plugin(&mut self, name: &str) -> Result<(), LoadError> {
        let mut plugin = self
            .plugins
            .remove(name)
            .ok_or_else(|| LoadError::NotLoaded(name.to_string()))?;

        if plugin.sender.try_send(PluginMail::Shutdown).is_err() {
            log::warn!("jvm plugin [{name}]: failed to send shutdown");
        }

        plugin.join();
        self.bus.unregister_plugin(name);
        log::info!("jvm: unloaded plugin: {name}");
        Ok(())
    }

    pub fn loaded_plugins(&self) -> Vec<&PluginMeta> {
        self.plugins.values().map(|p| &p.meta).collect()
    }

    pub fn is_loaded(&self, name: &str) -> bool {
        self.plugins.contains_key(name)
    }

    pub fn loaded_count(&self) -> usize {
        self.plugins.len()
    }

    pub fn shutdown_all(&mut self) {
        let names: Vec<String> = self.plugins.keys().cloned().collect();
        for name in names {
            if let Err(e) = self.unload_plugin(&name) {
                log::error!("jvm: failed to unload {name}: {e}");
            }
        }
    }
}

impl Drop for JavaPluginManager {
    fn drop(&mut self) {
        if !self.plugins.is_empty() {
            self.shutdown_all();
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin thread
// ---------------------------------------------------------------------------

fn java_plugin_thread(
    jvm: &JavaVM,
    plugin: GlobalRef,
    mut mailbox: mpsc::Receiver<PluginMail>,
    plugin_name: String,
    meta: &PluginMeta,
) {
    let mut env = match jvm.attach_current_thread() {
        Ok(e) => e,
        Err(e) => {
            log::error!("jvm [{plugin_name}]: failed to attach thread: {e}");
            return;
        }
    };

    // Store the real plugin name so JNI native methods attribute actions
    // (menu items, notifications, etc.) to this plugin, not the shared
    // "java" HostApi name.
    CURRENT_PLUGIN_NAME.with(|n| *n.borrow_mut() = plugin_name.clone());

    // Auto-register tool window if this is a tool-window plugin.
    if meta.plugin_type == conch_plugin_sdk::PluginType::ToolWindow {
        if let Some(api) = TRAIT_HOST_API.get() {
            api.register_panel(meta.panel_location, &meta.name, None);
            log::info!(
                "jvm [{plugin_name}]: registered tool window at {:?}",
                meta.panel_location
            );
        }
    }

    // Call setup().
    if let Err(e) = env.call_method(&plugin, "setup", "()V", &[]) {
        log::error!("jvm [{plugin_name}]: setup failed: {e}");
        describe_java_exception(&mut env);
        return;
    }
    log::info!("jvm [{plugin_name}]: setup complete");

    // Event loop.
    while let Some(mail) = mailbox.blocking_recv() {
        match mail {
            PluginMail::RenderRequest { view_id, reply } => {
                let json = call_render(&mut env, &plugin, &plugin_name, view_id.as_deref());
                let _ = reply.send(json);
            }

            PluginMail::WidgetEvent { json } => {
                call_on_event(&mut env, &plugin, &json, &plugin_name);
            }

            PluginMail::BusEvent(msg) => {
                let event = conch_plugin_sdk::PluginEvent::BusEvent {
                    event_type: msg.event_type.clone(),
                    data: msg.data.clone(),
                };
                if let Ok(json) = serde_json::to_string(&event) {
                    call_on_event(&mut env, &plugin, &json, &plugin_name);
                }
            }

            PluginMail::BusQuery(req) => {
                let args_json = serde_json::to_string(&req.args).unwrap_or_else(|_| "null".into());
                let jmethod = match env.new_string(&req.method) {
                    Ok(s) => s,
                    Err(_) => {
                        let _ = req.reply.send(QueryResponse {
                            result: Ok(serde_json::Value::Null),
                        });
                        continue;
                    }
                };
                let jargs = match env.new_string(&args_json) {
                    Ok(s) => s,
                    Err(_) => {
                        let _ = req.reply.send(QueryResponse {
                            result: Ok(serde_json::Value::Null),
                        });
                        continue;
                    }
                };
                let result = env.call_method(
                    &plugin,
                    "onQuery",
                    "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                    &[JValue::Object(&jmethod), JValue::Object(&jargs)],
                );
                let response = match result {
                    Ok(v) => match v.l() {
                        Ok(obj) => {
                            let jstr = JString::from(obj);
                            env.get_string(&jstr)
                                .ok()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "null".to_string())
                        }
                        Err(_) => "null".to_string(),
                    },
                    Err(_) => "null".to_string(),
                };
                let value: serde_json::Value =
                    serde_json::from_str(&response).unwrap_or(serde_json::Value::Null);
                let _ = req.reply.send(QueryResponse { result: Ok(value) });
            }

            PluginMail::Shutdown => {
                log::info!("jvm [{plugin_name}]: shutting down");
                break;
            }
        }
    }

    // Call teardown().
    if let Err(e) = env.call_method(&plugin, "teardown", "()V", &[]) {
        log::warn!("jvm [{plugin_name}]: teardown failed: {e}");
    }
    log::info!("jvm [{plugin_name}]: thread exiting");
}

fn call_render(
    env: &mut JNIEnv,
    plugin: &GlobalRef,
    plugin_name: &str,
    view_id: Option<&str>,
) -> String {
    if let Some(view_id) = view_id {
        if let Ok(jview_id) = env.new_string(view_id) {
            match env.call_method(
                plugin,
                "renderView",
                "(Ljava/lang/String;)Ljava/lang/String;",
                &[JValue::Object(&jview_id)],
            ) {
                Ok(val) => {
                    if let Ok(obj) = val.l() {
                        let jstr = JString::from(obj);
                        if let Ok(s) = env.get_string(&jstr) {
                            return s.to_string_lossy().into_owned();
                        }
                    }
                }
                Err(_) => {
                    // Fall through to legacy render() below.
                }
            }
        }
    }

    match env.call_method(plugin, "render", "()Ljava/lang/String;", &[]) {
        Ok(val) => match val.l() {
            Ok(obj) => {
                let jstr = JString::from(obj);
                env.get_string(&jstr)
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "[]".to_string())
            }
            Err(_) => "[]".to_string(),
        },
        Err(e) => {
            log::warn!("jvm [{plugin_name}]: render failed: {e}");
            describe_java_exception(env);
            "[]".to_string()
        }
    }
}

fn call_on_event(env: &mut JNIEnv, plugin: &GlobalRef, json: &str, plugin_name: &str) {
    let jstr = match env.new_string(json) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("jvm [{plugin_name}]: failed to create event string: {e}");
            return;
        }
    };
    if let Err(e) = env.call_method(
        plugin,
        "onEvent",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&jstr)],
    ) {
        log::warn!("jvm [{plugin_name}]: onEvent failed: {e}");
        describe_java_exception(env);
    }
}

// ---------------------------------------------------------------------------
// JNI helpers
// ---------------------------------------------------------------------------

/// Read `Plugin-Class` from a JAR's META-INF/MANIFEST.MF.
fn read_plugin_class_from_jar(jar_path: &Path) -> Result<String, LoadError> {
    let content = read_manifest_content_from_jar(jar_path)?;
    for line in content.lines() {
        if let Some(class) = line.strip_prefix("Plugin-Class:") {
            return Ok(class.trim().to_string());
        }
    }

    Err(LoadError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("no Plugin-Class in manifest of {}", jar_path.display()),
    )))
}

fn read_manifest_content_from_jar(jar_path: &Path) -> Result<String, LoadError> {
    let file = std::fs::File::open(jar_path).map_err(|e| LoadError::Io(e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        LoadError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        ))
    })?;

    let manifest = archive.by_name("META-INF/MANIFEST.MF").map_err(|_| {
        LoadError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "JAR missing META-INF/MANIFEST.MF",
        ))
    })?;

    std::io::read_to_string(manifest).map_err(LoadError::Io)
}

fn read_manifest_attr_from_jar(jar_path: &Path, key: &str) -> Result<Option<String>, LoadError> {
    let content = read_manifest_content_from_jar(jar_path)?;
    let prefix = format!("{key}:");
    for line in content.lines() {
        if let Some(value) = line.strip_prefix(&prefix) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed.to_string()));
            }
            return Ok(None);
        }
    }
    Ok(None)
}

fn parse_manifest_permissions(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Create a URLClassLoader for a JAR file.
fn create_url_classloader<'a>(
    env: &mut JNIEnv<'a>,
    jar_path: &Path,
) -> Result<JObject<'a>, LoadError> {
    let abs_path = jar_path.canonicalize().map_err(LoadError::Io)?;
    let uri_str = format!("file://{}", abs_path.display());

    // new java.net.URL(uriStr)
    let url_str = env.new_string(&uri_str).map_err(jni_err)?;
    let url = env
        .new_object(
            "java/net/URL",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&url_str)],
        )
        .map_err(jni_err)?;

    // Create URL[] { url }
    let url_class = env.find_class("java/net/URL").map_err(jni_err)?;
    let url_array = env.new_object_array(1, url_class, &url).map_err(jni_err)?;

    // new URLClassLoader(urls)
    let loader = env
        .new_object(
            "java/net/URLClassLoader",
            "([Ljava/net/URL;)V",
            &[JValue::Object(&url_array)],
        )
        .map_err(jni_err)?;

    Ok(loader)
}

/// Load and instantiate the plugin class from a URLClassLoader.
fn instantiate_plugin<'a>(
    env: &mut JNIEnv<'a>,
    loader: &JObject<'a>,
    class_name: &str,
) -> Result<JObject<'a>, LoadError> {
    // loader.loadClass(className)
    let jname = env.new_string(class_name).map_err(jni_err)?;
    let cls_obj = env
        .call_method(
            loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&jname)],
        )
        .map_err(jni_err)?
        .l()
        .map_err(jni_err)?;

    let cls = JClass::from(cls_obj);

    // cls.getDeclaredConstructor().newInstance()
    let constructor = env
        .call_method(
            &cls,
            "getDeclaredConstructor",
            "([Ljava/lang/Class;)Ljava/lang/reflect/Constructor;",
            &[JValue::Object(&JObject::null())],
        )
        .map_err(jni_err)?
        .l()
        .map_err(jni_err)?;

    let instance = env
        .call_method(
            &constructor,
            "newInstance",
            "([Ljava/lang/Object;)Ljava/lang/Object;",
            &[JValue::Object(&JObject::null())],
        )
        .map_err(jni_err)?
        .l()
        .map_err(jni_err)?;

    Ok(instance)
}

/// Read PluginInfo fields from a Java PluginInfo object.
fn read_plugin_info(env: &mut JNIEnv, info: &JObject) -> Result<PluginMeta, LoadError> {
    let name = get_string_field(env, info, "name")?;
    let description = get_string_field(env, info, "description")?;
    let version = get_string_field(env, info, "version")?;
    let plugin_type_str = get_string_field(env, info, "pluginType")?;
    let panel_location_str = get_string_field(env, info, "panelLocation")?;

    let plugin_type = match plugin_type_str.as_str() {
        "tool_window" | "panel" => conch_plugin_sdk::PluginType::ToolWindow,
        _ => conch_plugin_sdk::PluginType::Action,
    };
    let panel_location = match panel_location_str.as_str() {
        "left" => conch_plugin_sdk::PanelLocation::Left,
        "right" => conch_plugin_sdk::PanelLocation::Right,
        "bottom" => conch_plugin_sdk::PanelLocation::Bottom,
        _ => conch_plugin_sdk::PanelLocation::None,
    };

    Ok(PluginMeta {
        name,
        description,
        version,
        api_required: None,
        permissions: Vec::new(),
        plugin_type,
        panel_location,
    })
}

fn get_string_field(env: &mut JNIEnv, obj: &JObject, field: &str) -> Result<String, LoadError> {
    let val = env
        .get_field(obj, field, "Ljava/lang/String;")
        .map_err(jni_err)?
        .l()
        .map_err(jni_err)?;
    let jstr = JString::from(val);
    env.get_string(&jstr)
        .map(|s| s.to_string_lossy().into_owned())
        .map_err(jni_err)
}

// ---------------------------------------------------------------------------
// JNI native method registration
// ---------------------------------------------------------------------------

/// Register native method implementations for `conch.plugin.HostApi`.
fn register_host_natives(env: &mut JNIEnv) -> Result<(), LoadError> {
    let class = env.find_class("conch/plugin/HostApi").map_err(jni_err)?;

    let methods: &[NativeMethod] = &[
        NativeMethod {
            name: "checkPermission".into(),
            sig: "(Ljava/lang/String;)Z".into(),
            fn_ptr: native_host_check_permission as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "log".into(),
            sig: "(ILjava/lang/String;)V".into(),
            fn_ptr: native_host_log as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "registerMenuItem".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V".into(),
            fn_ptr: native_host_register_menu_item as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "registerMenuItemWithKeybind".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"
                .into(),
            fn_ptr: native_host_register_menu_item_keybind as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "notify".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;I)V".into(),
            fn_ptr: native_host_notify as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "setStatus".into(),
            sig: "(Ljava/lang/String;IF)V".into(),
            fn_ptr: native_host_set_status as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "clipboardSet".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: native_host_clipboard_set as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "clipboardGet".into(),
            sig: "()Ljava/lang/String;".into(),
            fn_ptr: native_host_clipboard_get as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "getTheme".into(),
            sig: "()Ljava/lang/String;".into(),
            fn_ptr: native_host_get_theme as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "getConfig".into(),
            sig: "(Ljava/lang/String;)Ljava/lang/String;".into(),
            fn_ptr: native_host_get_config as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "setConfig".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;)V".into(),
            fn_ptr: native_host_set_config as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "showForm".into(),
            sig: "(Ljava/lang/String;)Ljava/lang/String;".into(),
            fn_ptr: native_host_show_form as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "prompt".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;".into(),
            fn_ptr: native_host_prompt as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "confirm".into(),
            sig: "(Ljava/lang/String;)Z".into(),
            fn_ptr: native_host_confirm as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "alert".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;)V".into(),
            fn_ptr: native_host_alert as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "showError".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;)V".into(),
            fn_ptr: native_host_show_error as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "subscribe".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: native_host_subscribe as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "publishEvent".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;)V".into(),
            fn_ptr: native_host_publish_event as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "queryPlugin".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"
                .into(),
            fn_ptr: native_host_query_plugin as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "registerService".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: native_host_register_service as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "openDockedView".into(),
            sig: "(Ljava/lang/String;)Ljava/lang/String;".into(),
            fn_ptr: native_host_open_docked_view as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "closeDockedView".into(),
            sig: "(Ljava/lang/String;)Z".into(),
            fn_ptr: native_host_close_docked_view as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "focusDockedView".into(),
            sig: "(Ljava/lang/String;)Z".into(),
            fn_ptr: native_host_focus_docked_view as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "writeToPty".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: native_host_write_to_pty as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "newTab".into(),
            sig: "(Ljava/lang/String;Z)V".into(),
            fn_ptr: native_host_new_tab as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "getActiveSession".into(),
            sig: "()Ljava/lang/String;".into(),
            fn_ptr: native_host_get_active_session as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "execActiveSession".into(),
            sig: "(Ljava/lang/String;)Ljava/lang/String;".into(),
            fn_ptr: native_host_exec_active_session as *mut std::ffi::c_void,
        },
    ];

    env.register_native_methods(class, methods)
        .map_err(jni_err)?;
    log::info!("jvm: registered HostApi native methods");
    Ok(())
}

/// Helper to get the trait HostApi from the global.
fn get_api() -> Option<&'static Arc<dyn HostApi>> {
    TRAIT_HOST_API.get()
}

/// Helper to extract a Java string, returning None on failure.
fn jstr(env: &mut JNIEnv, s: &JString) -> Option<String> {
    env.get_string(s)
        .ok()
        .map(|s| s.to_string_lossy().into_owned())
}

/// JNI implementation of `HostApi.log(int level, String message)`.
extern "system" fn native_host_log(mut env: JNIEnv, _class: JClass, level: jint, message: JString) {
    let Some(api) = get_api() else { return };
    let Some(msg) = jstr(&mut env, &message) else {
        return;
    };
    api.log(level as u8, &msg);
}

extern "system" fn native_host_check_permission(
    mut env: JNIEnv,
    _class: JClass,
    capability: JString,
) -> jboolean {
    let Some(api) = get_api() else { return 0 };
    let Some(cap) = jstr(&mut env, &capability) else {
        return 0;
    };
    if api.check_permission(&cap) { 1 } else { 0 }
}

/// Read the current plugin name from the thread-local (set in java_plugin_thread).
fn current_plugin_name() -> String {
    CURRENT_PLUGIN_NAME.with(|n| n.borrow().clone())
}

extern "system" fn native_host_register_menu_item(
    mut env: JNIEnv,
    _class: JClass,
    menu: JString,
    label: JString,
    action: JString,
) {
    let Some(api) = get_api() else { return };
    let Some(m) = jstr(&mut env, &menu) else {
        return;
    };
    let Some(l) = jstr(&mut env, &label) else {
        return;
    };
    let Some(a) = jstr(&mut env, &action) else {
        return;
    };
    let name = current_plugin_name();
    if name.is_empty() {
        api.register_menu_item(&m, &l, &a, None);
    } else {
        api.register_menu_item_as(&name, &m, &l, &a, None);
    }
}

extern "system" fn native_host_register_menu_item_keybind(
    mut env: JNIEnv,
    _class: JClass,
    menu: JString,
    label: JString,
    action: JString,
    keybind: JString,
) {
    let Some(api) = get_api() else { return };
    let Some(m) = jstr(&mut env, &menu) else {
        return;
    };
    let Some(l) = jstr(&mut env, &label) else {
        return;
    };
    let Some(a) = jstr(&mut env, &action) else {
        return;
    };
    let kb = jstr(&mut env, &keybind);
    let name = current_plugin_name();
    if name.is_empty() {
        api.register_menu_item(&m, &l, &a, kb.as_deref());
    } else {
        api.register_menu_item_as(&name, &m, &l, &a, kb.as_deref());
    }
}

extern "system" fn native_host_notify(
    mut env: JNIEnv,
    _class: JClass,
    title: JString,
    body: JString,
    level: JString,
    duration_ms: jint,
) {
    let Some(api) = get_api() else { return };
    let title_str = jstr(&mut env, &title).unwrap_or_default();
    let Some(body_str) = jstr(&mut env, &body) else {
        return;
    };
    let level_str = jstr(&mut env, &level).unwrap_or_else(|| "info".into());
    let json = serde_json::json!({
        "title": title_str,
        "body": body_str,
        "level": level_str,
        "duration_ms": if duration_ms < 0 { serde_json::Value::Null } else { serde_json::json!(duration_ms) },
    });
    api.notify(&json.to_string());
}

extern "system" fn native_host_set_status(
    mut env: JNIEnv,
    _class: JClass,
    text: JString,
    level: jint,
    progress: jfloat,
) {
    let Some(api) = get_api() else { return };
    let text_str = jstr(&mut env, &text);
    api.set_status(text_str.as_deref(), level as u8, progress);
}

extern "system" fn native_host_clipboard_set(mut env: JNIEnv, _class: JClass, text: JString) {
    let Some(api) = get_api() else { return };
    let Some(t) = jstr(&mut env, &text) else {
        return;
    };
    api.clipboard_set(&t);
}

extern "system" fn native_host_clipboard_get(mut env: JNIEnv, _class: JClass) -> jobject {
    let Some(api) = get_api() else {
        return std::ptr::null_mut();
    };
    match api.clipboard_get() {
        Some(s) => env
            .new_string(&s)
            .map(|js| js.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

extern "system" fn native_host_get_theme(mut env: JNIEnv, _class: JClass) -> jobject {
    let Some(api) = get_api() else {
        return std::ptr::null_mut();
    };
    match api.get_theme() {
        Some(s) => env
            .new_string(&s)
            .map(|js| js.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

extern "system" fn native_host_get_config(
    mut env: JNIEnv,
    _class: JClass,
    key: JString,
) -> jobject {
    let Some(api) = get_api() else {
        return std::ptr::null_mut();
    };
    let Some(k) = jstr(&mut env, &key) else {
        return std::ptr::null_mut();
    };
    match api.get_config(&k) {
        Some(s) => env
            .new_string(&s)
            .map(|js| js.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

extern "system" fn native_host_set_config(
    mut env: JNIEnv,
    _class: JClass,
    key: JString,
    value: JString,
) {
    let Some(api) = get_api() else { return };
    let Some(k) = jstr(&mut env, &key) else {
        return;
    };
    let Some(v) = jstr(&mut env, &value) else {
        return;
    };
    api.set_config(&k, &v);
}

extern "system" fn native_host_show_form(
    mut env: JNIEnv,
    _class: JClass,
    form_json: JString,
) -> jobject {
    let Some(api) = get_api() else {
        return std::ptr::null_mut();
    };
    let Some(json) = jstr(&mut env, &form_json) else {
        return std::ptr::null_mut();
    };
    match api.show_form(&json) {
        Some(s) => env
            .new_string(&s)
            .map(|js| js.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

extern "system" fn native_host_prompt(
    mut env: JNIEnv,
    _class: JClass,
    message: JString,
    default_value: JString,
) -> jobject {
    let Some(api) = get_api() else {
        return std::ptr::null_mut();
    };
    let Some(msg) = jstr(&mut env, &message) else {
        return std::ptr::null_mut();
    };
    let default = jstr(&mut env, &default_value).unwrap_or_default();
    match api.show_prompt(&msg, &default) {
        Some(s) => env
            .new_string(&s)
            .map(|js| js.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

extern "system" fn native_host_confirm(
    mut env: JNIEnv,
    _class: JClass,
    message: JString,
) -> jboolean {
    let Some(api) = get_api() else { return 0 };
    let Some(msg) = jstr(&mut env, &message) else {
        return 0;
    };
    if api.show_confirm(&msg) { 1 } else { 0 }
}

extern "system" fn native_host_alert(
    mut env: JNIEnv,
    _class: JClass,
    title: JString,
    message: JString,
) {
    let Some(api) = get_api() else { return };
    let Some(t) = jstr(&mut env, &title) else {
        return;
    };
    let Some(m) = jstr(&mut env, &message) else {
        return;
    };
    api.show_alert(&t, &m);
}

extern "system" fn native_host_show_error(
    mut env: JNIEnv,
    _class: JClass,
    title: JString,
    message: JString,
) {
    let Some(api) = get_api() else { return };
    let Some(t) = jstr(&mut env, &title) else {
        return;
    };
    let Some(m) = jstr(&mut env, &message) else {
        return;
    };
    api.show_error(&t, &m);
}

extern "system" fn native_host_subscribe(mut env: JNIEnv, _class: JClass, event_type: JString) {
    let Some(api) = get_api() else { return };
    let Some(et) = jstr(&mut env, &event_type) else {
        return;
    };
    api.subscribe(&et);
}

extern "system" fn native_host_publish_event(
    mut env: JNIEnv,
    _class: JClass,
    event_type: JString,
    data_json: JString,
) {
    let Some(api) = get_api() else { return };
    let Some(et) = jstr(&mut env, &event_type) else {
        return;
    };
    let Some(data) = jstr(&mut env, &data_json) else {
        return;
    };
    api.publish_event(&et, &data);
}

extern "system" fn native_host_query_plugin(
    mut env: JNIEnv,
    _class: JClass,
    target: JString,
    method: JString,
    args_json: JString,
) -> jobject {
    let Some(api) = get_api() else {
        return std::ptr::null_mut();
    };
    let Some(t) = jstr(&mut env, &target) else {
        return std::ptr::null_mut();
    };
    let Some(m) = jstr(&mut env, &method) else {
        return std::ptr::null_mut();
    };
    let Some(a) = jstr(&mut env, &args_json) else {
        return std::ptr::null_mut();
    };
    match api.query_plugin(&t, &m, &a) {
        Some(s) => env
            .new_string(&s)
            .map(|js| js.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

extern "system" fn native_host_register_service(mut env: JNIEnv, _class: JClass, name: JString) {
    let Some(api) = get_api() else { return };
    let Some(svc) = jstr(&mut env, &name) else {
        return;
    };
    api.register_service(&svc);
}

extern "system" fn native_host_open_docked_view(
    mut env: JNIEnv,
    _class: JClass,
    request_json: JString,
) -> jobject {
    let Some(api) = get_api() else {
        return std::ptr::null_mut();
    };
    let Some(req) = jstr(&mut env, &request_json) else {
        return std::ptr::null_mut();
    };
    match api.open_docked_view(&req) {
        Some(s) => env
            .new_string(&s)
            .map(|js| js.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

extern "system" fn native_host_close_docked_view(
    mut env: JNIEnv,
    _class: JClass,
    view_id: JString,
) -> jboolean {
    let Some(api) = get_api() else { return 0 };
    let Some(view_id) = jstr(&mut env, &view_id) else {
        return 0;
    };
    if api.close_docked_view(&view_id) {
        1
    } else {
        0
    }
}

extern "system" fn native_host_focus_docked_view(
    mut env: JNIEnv,
    _class: JClass,
    view_id: JString,
) -> jboolean {
    let Some(api) = get_api() else { return 0 };
    let Some(view_id) = jstr(&mut env, &view_id) else {
        return 0;
    };
    if api.focus_docked_view(&view_id) {
        1
    } else {
        0
    }
}

extern "system" fn native_host_write_to_pty(mut env: JNIEnv, _class: JClass, text: JString) {
    let Some(api) = get_api() else { return };
    let Some(t) = jstr(&mut env, &text) else {
        return;
    };
    api.write_to_pty(t.as_bytes());
}

extern "system" fn native_host_new_tab(
    mut env: JNIEnv,
    _class: JClass,
    command: JString,
    plain: jboolean,
) {
    let Some(api) = get_api() else { return };
    let cmd = if command.is_null() {
        None
    } else {
        jstr(&mut env, &command)
    };
    api.new_tab(cmd.as_deref(), plain != 0);
}

extern "system" fn native_host_get_active_session(mut env: JNIEnv, _class: JClass) -> jobject {
    let Some(api) = get_api() else {
        return std::ptr::null_mut();
    };
    match api.get_active_session() {
        Some(s) => env
            .new_string(&s)
            .map(|js| js.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

extern "system" fn native_host_exec_active_session(
    mut env: JNIEnv,
    _class: JClass,
    command: JString,
) -> jobject {
    let Some(api) = get_api() else {
        return std::ptr::null_mut();
    };
    let Some(cmd) = jstr(&mut env, &command) else {
        return std::ptr::null_mut();
    };
    match api.exec_active_session(&cmd) {
        Some(s) => env
            .new_string(&s)
            .map(|js| js.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

// ---------------------------------------------------------------------------
// Error conversion
// ---------------------------------------------------------------------------

/// Log any pending Java exception as an error, then clear it.
fn describe_java_exception(env: &mut JNIEnv) {
    if !env.exception_check().unwrap_or(false) {
        return;
    }

    // Try to extract the exception message for structured logging.
    if let Ok(throwable) = env.exception_occurred() {
        env.exception_clear().ok();
        // Call throwable.toString() to get the exception class + message.
        match env.call_method(&throwable, "toString", "()Ljava/lang/String;", &[]) {
            Ok(val) => {
                if let Ok(obj) = val.l() {
                    let jstr = JString::from(obj);
                    if let Ok(s) = env.get_string(&jstr) {
                        log::error!("jvm: Java exception: {}", s.to_string_lossy());
                        return;
                    }
                }
            }
            Err(_) => {
                env.exception_clear().ok();
            }
        }
        log::error!("jvm: Java exception occurred (could not extract message)");
    } else {
        env.exception_describe().ok();
        env.exception_clear().ok();
    }
}

fn jni_err<E: std::fmt::Display>(e: E) -> LoadError {
    LoadError::Io(std::io::Error::new(
        std::io::ErrorKind::Other,
        e.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_manifest_from_test_jar() {
        // This test only works if the java-hello plugin has been built.
        let jar = PathBuf::from("../../plugins/java-hello/build/hello-plugin.jar");
        if !jar.exists() {
            return; // Skip if not built.
        }
        let class = read_plugin_class_from_jar(&jar).unwrap();
        assert_eq!(class, "conch.plugin.hello.HelloPlugin");
    }
}
