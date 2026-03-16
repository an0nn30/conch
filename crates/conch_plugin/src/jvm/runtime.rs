//! JVM plugin manager — discovery, loading, and lifecycle for `.jar` plugins.

use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;

use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jfloat, jint, jobject};
use jni::{InitArgsBuilder, JNIEnv, JavaVM, NativeMethod};
use tokio::sync::mpsc;

use crate::bus::{PluginBus, PluginMail, QueryResponse};
use crate::native::lifecycle::LoadedPlugin;
use crate::native::{LoadError, PluginMeta};

/// Global pointer to the HostApi vtable. Set once by `JavaPluginManager::new()`.
/// JNI native methods read this to call back into the host.
static HOST_API_PTR: AtomicPtr<conch_plugin_sdk::HostApi> =
    AtomicPtr::new(std::ptr::null_mut());

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
    _host_api_box: Box<conch_plugin_sdk::HostApi>,
}

// SAFETY: JavaVM is Send+Sync. The HostApi pointer is stable (owned by _host_api_box).
unsafe impl Send for JavaPluginManager {}

impl JavaPluginManager {
    pub fn new(bus: Arc<PluginBus>, host_api: conch_plugin_sdk::HostApi) -> Self {
        let mut boxed = Box::new(host_api);
        let ptr: *mut conch_plugin_sdk::HostApi = &mut *boxed;
        HOST_API_PTR.store(ptr, Ordering::Release);

        Self {
            bus,
            plugins: HashMap::new(),
            jvm: None,
            _sdk_jar_tempfile: None,
            _host_api_box: boxed,
        }
    }

    /// Lazily create the JVM with the embedded SDK JAR on the classpath.
    fn ensure_jvm(&mut self) -> Result<&JavaVM, LoadError> {
        if self.jvm.is_some() {
            return Ok(self.jvm.as_ref().unwrap());
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
        log::info!("jvm: starting JVM with embedded SDK JAR ({} bytes) at {}", SDK_JAR_BYTES.len(), tmpfile.path().display());

        let jvm_args = InitArgsBuilder::new()
            .version(jni::JNIVersion::V8)
            .option(&classpath)
            .build()
            .map_err(|e| LoadError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("JVM init args: {e}"),
            )))?;

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
        Ok(self.jvm.as_ref().unwrap())
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
            eprintln!("[jvm] probing JAR: {}", path.display());
            match self.probe_jar_metadata(&path) {
                Ok(meta) => {
                    eprintln!("[jvm] found plugin: {} v{}", meta.name, meta.version);
                    found.push((path, meta));
                }
                Err(e) => eprintln!("[jvm] FAILED to probe {}: {e}", path.display()),
            }
        }
        found
    }

    /// Read plugin metadata from a JAR by loading it in the JVM.
    fn probe_jar_metadata(&mut self, jar_path: &Path) -> Result<PluginMeta, LoadError> {
        let class_name = read_plugin_class_from_jar(jar_path)?;

        let jvm = self.ensure_jvm()?;
        let mut env = jvm.attach_current_thread().map_err(|e| {
            LoadError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("JNI attach: {e}")))
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
        let info_obj = match env.call_method(&plugin_obj, "getInfo", "()Lconch/plugin/PluginInfo;", &[]) {
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

        let meta = read_plugin_info(&mut env, &info_obj)?;
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
        let jvm = self.jvm.as_ref().unwrap();

        // Create the plugin object on this thread, convert to GlobalRef for the plugin thread.
        let plugin_global = {
            let mut env = jvm.attach_current_thread().map_err(|e| {
                LoadError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("JNI attach: {e}")))
            })?;
            let loader = create_url_classloader(&mut env, jar_path)?;
            let plugin_obj = instantiate_plugin(&mut env, &loader, &class_name)?;
            env.new_global_ref(&plugin_obj).map_err(|e| {
                LoadError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("global ref: {e}")))
            })?
        };

        // Register on the bus.
        let mailbox_rx = self.bus.register_plugin(&name);
        let sender = self.bus.sender_for(&name).unwrap();

        let jvm_ptr = jvm as *const JavaVM as usize;
        let host_api_addr = HOST_API_PTR.load(std::sync::atomic::Ordering::Acquire) as usize;
        let thread_name = name.clone();
        let thread_plugin_name = name.clone();
        let thread_meta = meta.clone();

        let handle = std::thread::Builder::new()
            .name(format!("plugin:{thread_name}"))
            .spawn(move || {
                let jvm = unsafe { &*(jvm_ptr as *const JavaVM) };
                let host_api = host_api_addr as *const conch_plugin_sdk::HostApi;
                java_plugin_thread(jvm, plugin_global, mailbox_rx, thread_plugin_name, host_api, &thread_meta);
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
    host_api: *const conch_plugin_sdk::HostApi,
    meta: &crate::native::PluginMeta,
) {
    let mut env = match jvm.attach_current_thread() {
        Ok(e) => e,
        Err(e) => {
            log::error!("jvm [{plugin_name}]: failed to attach thread: {e}");
            return;
        }
    };

    // Auto-register panel if this is a panel plugin (like Lua plugins do).
    if meta.plugin_type == conch_plugin_sdk::PluginType::Panel && !host_api.is_null() {
        let panel_name = CString::new(meta.name.as_str()).unwrap_or_default();
        unsafe {
            ((*host_api).register_panel)(meta.panel_location, panel_name.as_ptr(), std::ptr::null());
        }
        log::info!("jvm [{plugin_name}]: registered panel at {:?}", meta.panel_location);
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
            PluginMail::RenderRequest { reply } => {
                let json = call_render(&mut env, &plugin, &plugin_name);
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
                let _ = req.reply.send(QueryResponse {
                    result: Err("Java plugins do not support queries yet".into()),
                });
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

fn call_render(env: &mut JNIEnv, plugin: &GlobalRef, plugin_name: &str) -> String {
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
    if let Err(e) = env.call_method(plugin, "onEvent", "(Ljava/lang/String;)V", &[JValue::Object(&jstr)]) {
        log::warn!("jvm [{plugin_name}]: onEvent failed: {e}");
        describe_java_exception(env);
    }
}

// ---------------------------------------------------------------------------
// JNI helpers
// ---------------------------------------------------------------------------

/// Read `Plugin-Class` from a JAR's META-INF/MANIFEST.MF.
fn read_plugin_class_from_jar(jar_path: &Path) -> Result<String, LoadError> {
    let file = std::fs::File::open(jar_path)
        .map_err(|e| LoadError::Io(e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| LoadError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;

    let manifest = archive
        .by_name("META-INF/MANIFEST.MF")
        .map_err(|_| LoadError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "JAR missing META-INF/MANIFEST.MF",
        )))?;

    let content = std::io::read_to_string(manifest)
        .map_err(|e| LoadError::Io(e))?;

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
        .new_object("java/net/URL", "(Ljava/lang/String;)V", &[JValue::Object(&url_str)])
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
        "panel" => conch_plugin_sdk::PluginType::Panel,
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
        plugin_type,
        panel_location,
        dependencies: vec![],
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
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V".into(),
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
            name: "writeToPty".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: native_host_write_to_pty as *mut std::ffi::c_void,
        },
        NativeMethod {
            name: "newTab".into(),
            sig: "(Ljava/lang/String;Z)V".into(),
            fn_ptr: native_host_new_tab as *mut std::ffi::c_void,
        },
    ];

    env.register_native_methods(class, methods).map_err(jni_err)?;
    log::info!("jvm: registered HostApi native methods");
    Ok(())
}

/// JNI implementation of `HostApi.log(int level, String message)`.
extern "system" fn native_host_log(mut env: JNIEnv, _class: JClass, level: jint, message: JString) {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() {
        return;
    }

    let msg: String = match env.get_string(&message) {
        Ok(s) => s.to_string_lossy().into_owned(),
        Err(_) => return,
    };

    let c_msg = CString::new(msg).unwrap_or_default();
    unsafe { ((*host_api).log)(level as u8, c_msg.as_ptr()) };
}

/// JNI implementation of `HostApi.registerMenuItem(String menu, String label, String action)`.
extern "system" fn native_host_register_menu_item(
    mut env: JNIEnv,
    _class: JClass,
    menu: JString,
    label: JString,
    action: JString,
) {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() {
        return;
    }

    let menu_str = match env.get_string(&menu) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let label_str = match env.get_string(&label) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let action_str = match env.get_string(&action) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };

    let c_menu = CString::new(menu_str).unwrap_or_default();
    let c_label = CString::new(label_str).unwrap_or_default();
    let c_action = CString::new(action_str).unwrap_or_default();

    unsafe {
        ((*host_api).register_menu_item)(
            c_menu.as_ptr(),
            c_label.as_ptr(),
            c_action.as_ptr(),
            std::ptr::null(),
        );
    }
}

/// JNI implementation of `HostApi.registerMenuItemWithKeybind`.
extern "system" fn native_host_register_menu_item_keybind(
    mut env: JNIEnv, _class: JClass,
    menu: JString, label: JString, action: JString, keybind: JString,
) {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() { return; }
    let menu_str = match env.get_string(&menu) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let label_str = match env.get_string(&label) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let action_str = match env.get_string(&action) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let keybind_str = match env.get_string(&keybind) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let c_menu = CString::new(menu_str).unwrap_or_default();
    let c_label = CString::new(label_str).unwrap_or_default();
    let c_action = CString::new(action_str).unwrap_or_default();
    let c_keybind = CString::new(keybind_str).unwrap_or_default();
    unsafe { ((*host_api).register_menu_item)(c_menu.as_ptr(), c_label.as_ptr(), c_action.as_ptr(), c_keybind.as_ptr()); }
}

/// JNI implementation of `HostApi.notify`.
extern "system" fn native_host_notify(
    mut env: JNIEnv, _class: JClass,
    title: JString, body: JString, level: JString, duration_ms: jint,
) {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() { return; }
    let title_str = env.get_string(&title).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let body_str = match env.get_string(&body) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let level_str = env.get_string(&level).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|_| "info".into());
    let json = serde_json::json!({
        "title": title_str,
        "body": body_str,
        "level": level_str,
        "duration_ms": if duration_ms < 0 { serde_json::Value::Null } else { serde_json::json!(duration_ms) },
    });
    let json_str = json.to_string();
    let c_json = CString::new(json_str.clone()).unwrap_or_default();
    unsafe { ((*host_api).notify)(c_json.as_ptr(), json_str.len()); }
}

/// JNI implementation of `HostApi.setStatus`.
extern "system" fn native_host_set_status(
    mut env: JNIEnv, _class: JClass,
    text: JString, level: jint, progress: jfloat,
) {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() { return; }
    let text_str = match env.get_string(&text) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let c_text = CString::new(text_str).unwrap_or_default();
    unsafe { ((*host_api).set_status)(c_text.as_ptr(), level as u8, progress); }
}

/// JNI implementation of `HostApi.clipboardSet`.
extern "system" fn native_host_clipboard_set(
    mut env: JNIEnv, _class: JClass, text: JString,
) {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() { return; }
    let text_str = match env.get_string(&text) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let c_text = CString::new(text_str).unwrap_or_default();
    unsafe { ((*host_api).clipboard_set)(c_text.as_ptr()); }
}

/// JNI implementation of `HostApi.clipboardGet`.
extern "system" fn native_host_clipboard_get(
    mut env: JNIEnv, _class: JClass,
) -> jobject {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() { return std::ptr::null_mut(); }
    let ptr = unsafe { ((*host_api).clipboard_get)() };
    if ptr.is_null() { return std::ptr::null_mut(); }
    let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    unsafe { ((*host_api).free_string)(ptr); }
    match env.new_string(&s) {
        Ok(js) => js.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// JNI implementation of `HostApi.getConfig`.
extern "system" fn native_host_get_config(
    mut env: JNIEnv, _class: JClass, key: JString,
) -> jobject {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() { return std::ptr::null_mut(); }
    let key_str = match env.get_string(&key) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return std::ptr::null_mut() };
    let c_key = CString::new(key_str).unwrap_or_default();
    let ptr = unsafe { ((*host_api).get_config)(c_key.as_ptr()) };
    if ptr.is_null() { return std::ptr::null_mut(); }
    let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    unsafe { ((*host_api).free_string)(ptr); }
    match env.new_string(&s) {
        Ok(js) => js.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// JNI implementation of `HostApi.setConfig`.
extern "system" fn native_host_set_config(
    mut env: JNIEnv, _class: JClass, key: JString, value: JString,
) {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() { return; }
    let key_str = match env.get_string(&key) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let value_str = match env.get_string(&value) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let c_key = CString::new(key_str).unwrap_or_default();
    let c_value = CString::new(value_str).unwrap_or_default();
    unsafe { ((*host_api).set_config)(c_key.as_ptr(), c_value.as_ptr()); }
}

/// JNI implementation of `HostApi.prompt`.
extern "system" fn native_host_prompt(
    mut env: JNIEnv, _class: JClass, message: JString, default_value: JString,
) -> jobject {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() {
        log::error!("jvm: HostApi.prompt called but HOST_API_PTR is null");
        return std::ptr::null_mut();
    }
    let msg = match env.get_string(&message) {
        Ok(s) => s.to_string_lossy().into_owned(),
        Err(e) => {
            log::error!("jvm: HostApi.prompt: failed to read message arg: {e}");
            describe_java_exception(&mut env);
            return std::ptr::null_mut();
        }
    };
    let default = match env.get_string(&default_value) {
        Ok(s) => s.to_string_lossy().into_owned(),
        Err(e) => {
            log::error!("jvm: HostApi.prompt: failed to read defaultValue arg: {e}");
            describe_java_exception(&mut env);
            String::new()
        }
    };
    log::debug!("jvm: HostApi.prompt: msg={msg:?} default={default:?}");
    let c_msg = CString::new(msg).unwrap_or_default();
    let c_default = CString::new(default).unwrap_or_default();
    let ptr = unsafe { ((*host_api).show_prompt)(c_msg.as_ptr(), c_default.as_ptr()) };
    if ptr.is_null() {
        log::debug!("jvm: HostApi.prompt: user cancelled (null result)");
        return std::ptr::null_mut();
    }
    let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    unsafe { ((*host_api).free_string)(ptr); }
    log::debug!("jvm: HostApi.prompt: result={s:?}");
    match env.new_string(&s) {
        Ok(js) => js.into_raw(),
        Err(e) => {
            log::error!("jvm: HostApi.prompt: failed to create result string: {e}");
            describe_java_exception(&mut env);
            std::ptr::null_mut()
        }
    }
}

/// JNI implementation of `HostApi.confirm`.
extern "system" fn native_host_confirm(
    mut env: JNIEnv, _class: JClass, message: JString,
) -> jboolean {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() {
        log::error!("jvm: HostApi.confirm called but HOST_API_PTR is null");
        return 0;
    }
    let msg = match env.get_string(&message) {
        Ok(s) => s.to_string_lossy().into_owned(),
        Err(e) => {
            log::error!("jvm: HostApi.confirm: failed to read message arg: {e}");
            describe_java_exception(&mut env);
            return 0;
        }
    };
    let c_msg = CString::new(msg).unwrap_or_default();
    let result = unsafe { ((*host_api).show_confirm)(c_msg.as_ptr()) };
    if result { 1 } else { 0 }
}

/// JNI implementation of `HostApi.alert`.
extern "system" fn native_host_alert(
    mut env: JNIEnv, _class: JClass, title: JString, message: JString,
) {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() { return; }
    let title_str = match env.get_string(&title) { Ok(s) => s.to_string_lossy().into_owned(), Err(e) => { log::error!("jvm: HostApi.alert: {e}"); describe_java_exception(&mut env); return; } };
    let msg_str = match env.get_string(&message) { Ok(s) => s.to_string_lossy().into_owned(), Err(e) => { log::error!("jvm: HostApi.alert: {e}"); describe_java_exception(&mut env); return; } };
    let c_title = CString::new(title_str).unwrap_or_default();
    let c_msg = CString::new(msg_str).unwrap_or_default();
    unsafe { ((*host_api).show_alert)(c_title.as_ptr(), c_msg.as_ptr()); }
}

/// JNI implementation of `HostApi.showError`.
extern "system" fn native_host_show_error(
    mut env: JNIEnv, _class: JClass, title: JString, message: JString,
) {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() { return; }
    let title_str = match env.get_string(&title) { Ok(s) => s.to_string_lossy().into_owned(), Err(e) => { log::error!("jvm: HostApi.showError: {e}"); describe_java_exception(&mut env); return; } };
    let msg_str = match env.get_string(&message) { Ok(s) => s.to_string_lossy().into_owned(), Err(e) => { log::error!("jvm: HostApi.showError: {e}"); describe_java_exception(&mut env); return; } };
    let c_title = CString::new(title_str).unwrap_or_default();
    let c_msg = CString::new(msg_str).unwrap_or_default();
    unsafe { ((*host_api).show_error)(c_title.as_ptr(), c_msg.as_ptr()); }
}

/// JNI implementation of `HostApi.subscribe`.
extern "system" fn native_host_subscribe(
    mut env: JNIEnv, _class: JClass, event_type: JString,
) {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() { return; }
    let et = match env.get_string(&event_type) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let c_et = CString::new(et).unwrap_or_default();
    unsafe { ((*host_api).subscribe)(c_et.as_ptr()); }
}

/// JNI implementation of `HostApi.publishEvent`.
extern "system" fn native_host_publish_event(
    mut env: JNIEnv, _class: JClass, event_type: JString, data_json: JString,
) {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() { return; }
    let et = match env.get_string(&event_type) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let data = match env.get_string(&data_json) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    let c_et = CString::new(et).unwrap_or_default();
    let c_data = CString::new(data.clone()).unwrap_or_default();
    unsafe { ((*host_api).publish_event)(c_et.as_ptr(), c_data.as_ptr(), data.len()); }
}

/// JNI implementation of `HostApi.writeToPty`.
extern "system" fn native_host_write_to_pty(
    mut env: JNIEnv, _class: JClass, text: JString,
) {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() { return; }
    let text_str = match env.get_string(&text) { Ok(s) => s.to_string_lossy().into_owned(), Err(_) => return };
    unsafe { ((*host_api).write_to_pty)(text_str.as_ptr(), text_str.len()); }
}

/// JNI implementation of `HostApi.newTab`.
extern "system" fn native_host_new_tab(
    mut env: JNIEnv, _class: JClass, command: JString, plain: jboolean,
) {
    let host_api = HOST_API_PTR.load(Ordering::Acquire);
    if host_api.is_null() { return; }
    let cmd_ptr = if command.is_null() {
        std::ptr::null()
    } else {
        match env.get_string(&command) {
            Ok(s) => {
                let owned = s.to_string_lossy().into_owned();
                let c = CString::new(owned).unwrap_or_default();
                let ptr = c.as_ptr();
                // Keep CString alive through the FFI call.
                unsafe { ((*host_api).new_tab)(ptr, plain != 0); }
                return;
            }
            Err(_) => std::ptr::null(),
        }
    };
    unsafe { ((*host_api).new_tab)(cmd_ptr, plain != 0); }
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
    LoadError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
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
