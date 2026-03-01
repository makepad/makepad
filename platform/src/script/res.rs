use crate::makepad_network::HttpRequest;
use crate::script::vm::*;
use crate::*;
use makepad_script::id;
use makepad_script::*;
use std::cell::RefCell;
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::fs::File;
#[cfg(not(target_arch = "wasm32"))]
use std::io::Read;
#[cfg(target_arch = "wasm32")]
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum CxScriptResourceData {
    NotLoaded,
    Loading,
    Loaded(Rc<Vec<u8>>),
    Error(String),
}

#[derive(Clone)]
pub struct CxScriptResource {
    pub abs_path: String,
    pub dependency_path: Option<String>,
    pub web_url: Option<String>,
    pub data: CxScriptResourceData,
    pub handle: ScriptHandle,
}

impl CxScriptResource {
    pub fn is_error(&self) -> bool {
        matches!(self.data, CxScriptResourceData::Error(_))
    }
}

/// Tracks an in-flight HTTP request that will populate a resource
pub struct CxScriptHttpResource {
    pub request_id: LiveId,
    pub handle: ScriptHandle,
}

#[derive(Default)]
pub struct CxScriptResources {
    pub resources: Rc<RefCell<Vec<CxScriptResource>>>,
    pub handles_by_abs_path: Rc<RefCell<HashMap<String, ScriptHandle>>>,
    pub http_resources: Vec<CxScriptHttpResource>,
}

impl CxScriptResources {
    pub fn get_handle_by_abs_path(&self, abs_path: &str) -> Option<ScriptHandle> {
        self.handles_by_abs_path.borrow().get(abs_path).copied()
    }

    pub fn insert_resource(&self, resource: CxScriptResource) {
        self.handles_by_abs_path
            .borrow_mut()
            .insert(resource.abs_path.clone(), resource.handle);
        self.resources.borrow_mut().push(resource);
    }

    /// Get the data for a resource by handle
    pub fn get_data(&self, handle: ScriptHandle) -> Option<Rc<Vec<u8>>> {
        let resources = self.resources.borrow();
        if let Some(res) = resources.iter().find(|v| v.handle == handle) {
            if let CxScriptResourceData::Loaded(data) = &res.data {
                return Some(data.clone());
            }
        }
        None
    }

    /// Store HTTP response data into a resource by request_id.
    /// Returns true if a matching resource was found and updated.
    pub fn handle_http_response(&mut self, request_id: LiveId, data: Vec<u8>) -> bool {
        if let Some(idx) = self
            .http_resources
            .iter()
            .position(|r| r.request_id == request_id)
        {
            let handle = self.http_resources[idx].handle;
            self.http_resources.remove(idx);
            let mut resources = self.resources.borrow_mut();
            if let Some(res) = resources.iter_mut().find(|r| r.handle == handle) {
                res.data = CxScriptResourceData::Loaded(Rc::new(data));
                return true;
            }
        }
        false
    }

    /// Store HTTP error into a resource by request_id.
    /// Returns true if a matching resource was found and updated.
    pub fn handle_http_error(&mut self, request_id: LiveId, error: String) -> bool {
        if let Some(idx) = self
            .http_resources
            .iter()
            .position(|r| r.request_id == request_id)
        {
            let handle = self.http_resources[idx].handle;
            self.http_resources.remove(idx);
            let mut resources = self.resources.borrow_mut();
            if let Some(res) = resources.iter_mut().find(|r| r.handle == handle) {
                res.data = CxScriptResourceData::Error(error);
                return true;
            }
        }
        false
    }

    /// Check if a request_id belongs to an http_resource
    pub fn is_http_resource(&self, request_id: LiveId) -> bool {
        self.http_resources
            .iter()
            .any(|r| r.request_id == request_id)
    }
}

// ---------------------------------------------------------------------------
// Platform-specific resource loading
//
// Loading order depends on whether we are packaged or not:
//   - Desktop unpackaged (package_root is None):
//       1. dependency table (populated at init)
//       2. direct filesystem via abs_path
//       3. error
//   - Desktop packaged (package_root is Some):
//       1. dependency table
//       2. packaged path: package_root/dep_path on filesystem
//       3. error
//   - iOS/tvOS (always packaged, package_root = "makepad"):
//       1. dependency table
//       2. apple bundle: NSBundle.resourcePath / package_root / dep_path
//       3. error
//   - Android (always packaged, package_root = "makepad"):
//       1. dependency table (get_dependency calls JNI asset manager)
//       2. error
//   - Wasm (always packaged, package_root = ""):
//       1. dependency table (may have pre-loaded deps)
//       2. HTTP fetch via web_url (async)
//       3. error
// ---------------------------------------------------------------------------

/// Try to load a resource from the packaged location on iOS/tvOS.
#[cfg(any(target_os = "ios", target_os = "tvos"))]
fn load_packaged_resource(cx: &Cx, dep_path: &str) -> Option<Rc<Vec<u8>>> {
    let bundle_path = if let Some(root) = cx.package_root.as_deref() {
        format!("{}/{}", root, dep_path)
    } else {
        dep_path.to_string()
    };
    cx.apple_bundle_load_file(&bundle_path).ok()
}

/// Try to load a resource from the packaged location on desktop.
/// Returns None when not in packaged mode (package_root is None).
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "android", target_os = "ios", target_os = "tvos"))
))]
fn load_packaged_resource(cx: &Cx, dep_path: &str) -> Option<Rc<Vec<u8>>> {
    let root = cx.package_root.as_deref()?;
    let full_path = format!("{}/{}", root, dep_path);
    let mut file = File::open(&full_path).ok()?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).ok()?;
    Some(Rc::new(data))
}

/// Load a file directly from the filesystem (desktop/mobile only, not wasm).
#[cfg(not(target_arch = "wasm32"))]
fn load_file_direct(abs_path: &str) -> Option<Result<Rc<Vec<u8>>, String>> {
    let mut file = File::open(abs_path).ok()?;
    let mut data = Vec::new();
    match file.read_to_end(&mut data) {
        Ok(_) => Some(Ok(Rc::new(data))),
        Err(e) => Some(Err(format!("Failed to read file: {}", e))),
    }
}

impl Cx {
    /// Load all script resources that are still pending.
    ///
    /// Each platform uses a different loading strategy:
    /// - Desktop unpackaged: dependency table, then direct filesystem
    /// - Desktop packaged: dependency table, then package_root-relative file
    /// - iOS/tvOS: dependency table, then apple bundle
    /// - Android: dependency table only (JNI asset manager is inside get_dependency)
    /// - Wasm: dependency table, then async HTTP fetch
    pub fn load_all_script_resources(&mut self) {
        // On wasm, skip loading if we haven't received ToWasmInit yet (os_type is Unknown).
        #[cfg(target_arch = "wasm32")]
        if matches!(self.os_type(), crate::cx::OsType::Unknown) {
            return;
        }

        // On wasm, resolve web_url for resources that don't have one yet.
        #[cfg(target_arch = "wasm32")]
        let crate_manifests = self.script_data.crate_manifests.borrow().clone();

        let is_packaged = self.package_root.is_some();

        // Collect HTTP requests to issue after releasing the borrow (wasm only).
        #[cfg(target_arch = "wasm32")]
        let mut pending_http = Vec::<(LiveId, ScriptHandle, String)>::new();

        {
            let mut resources = self.script_data.resources.resources.borrow_mut();
            for res in resources.iter_mut() {
                if !matches!(res.data, CxScriptResourceData::NotLoaded) {
                    continue;
                }

                // --- Wasm: resolve web_url from crate manifests if missing ---
                #[cfg(target_arch = "wasm32")]
                if res.web_url.is_none() {
                    if let Some(dep_path) = resolve_dependency_path_from_manifests(
                        &res.abs_path,
                        None,
                        None,
                        &crate_manifests,
                    ) {
                        res.dependency_path = Some(dep_path.clone());
                        res.web_url = Some(format!("/{}", dep_path));
                    }
                }

                // --- Step 1: Try the dependency table ---
                // On android this calls through to the JNI asset manager.
                if let Some(dep_path) = res.dependency_path.as_deref() {
                    if let Ok(data) = self.get_dependency(dep_path) {
                        res.data = CxScriptResourceData::Loaded(data);
                        continue;
                    }
                }

                // --- Step 2: Platform-specific loading ---

                // Wasm: issue an async HTTP request for the web_url.
                #[cfg(target_arch = "wasm32")]
                {
                    if let Some(url) = res.web_url.clone() {
                        let request_id = LiveId::unique();
                        res.data = CxScriptResourceData::Loading;
                        pending_http.push((request_id, res.handle, url));
                        continue;
                    }
                }

                // Native platforms: try packaged or direct file depending on mode.
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if is_packaged {
                        // Packaged mode: only load from the packaged location.
                        // Android: get_dependency above already tried JNI — nothing more to do.
                        #[cfg(not(target_os = "android"))]
                        if let Some(dep_path) = res.dependency_path.as_deref() {
                            if let Some(data) = load_packaged_resource(self, dep_path) {
                                res.data = CxScriptResourceData::Loaded(data);
                                continue;
                            }
                        }
                    } else {
                        // Unpackaged (dev) mode: load directly from the filesystem.
                        if let Some(result) = load_file_direct(&res.abs_path) {
                            res.data = match result {
                                Ok(data) => CxScriptResourceData::Loaded(data),
                                Err(e) => CxScriptResourceData::Error(e),
                            };
                            continue;
                        }
                    }
                }

                // --- Step 3: Error ---
                res.data = CxScriptResourceData::Error(format!(
                    "Failed to load resource: {} (dep: {:?}, packaged: {})",
                    res.abs_path, res.dependency_path, is_packaged,
                ));
            }
        }

        // Wasm: fire HTTP requests outside the borrow.
        #[cfg(target_arch = "wasm32")]
        for (request_id, handle, url) in pending_http {
            self.script_data
                .resources
                .http_resources
                .push(CxScriptHttpResource { request_id, handle });
            self.http_request(request_id, HttpRequest::new(url, Default::default()));
        }
    }
}

pub struct CxScriptResourceGc {
    pub resources: Rc<RefCell<Vec<CxScriptResource>>>,
    pub handles_by_abs_path: Rc<RefCell<HashMap<String, ScriptHandle>>>,
    pub handle: ScriptHandle,
}

impl ScriptHandleGc for CxScriptResourceGc {
    fn gc(&mut self) {
        let mut removed_paths = Vec::new();
        self.resources.borrow_mut().retain(|v| {
            if v.handle == self.handle {
                removed_paths.push(v.abs_path.clone());
                false
            } else {
                true
            }
        });
        if !removed_paths.is_empty() {
            let mut handles_by_abs_path = self.handles_by_abs_path.borrow_mut();
            for abs_path in removed_paths {
                if handles_by_abs_path.get(&abs_path).copied() == Some(self.handle) {
                    handles_by_abs_path.remove(&abs_path);
                }
            }
        }
    }
    fn set_handle(&mut self, handle: ScriptHandle) {
        self.handle = handle
    }
}

// ---------------------------------------------------------------------------
// Crate path resolution
// ---------------------------------------------------------------------------

/// Parses a crate path like "self:resources/file.jpg" or "other_crate:path/file.ext"
/// Returns (crate_part, file_path)
fn parse_crate_path(path: &str) -> Option<(&str, &str)> {
    let mut split = path.splitn(2, ':');
    let crate_part = split.next()?;
    let file_path = split.next()?;
    Some((crate_part, file_path))
}

/// Accept both `self:path` and `self://path` syntax.
fn strip_crate_resource_leading_slashes(file_path: &str) -> &str {
    file_path.trim_start_matches('/')
}

fn normalize_dependency_file_path(path: &str) -> Option<String> {
    let mut stack: Vec<&str> = Vec::new();
    let normalized = path.replace('\\', "/");
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() {
                    return None;
                }
            }
            other => stack.push(other),
        }
    }
    Some(stack.join("/"))
}

#[cfg(target_arch = "wasm32")]
fn normalize_path(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            std::path::Component::RootDir => out.push(comp.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            std::path::Component::Normal(part) => out.push(part),
        }
    }
    Some(out)
}

#[cfg(target_arch = "wasm32")]
fn normalize_manifest_relative_path(path: &Path) -> Option<String> {
    normalize_dependency_file_path(&path.to_string_lossy().replace('\\', "/"))
}

#[cfg(target_arch = "wasm32")]
fn resolve_dependency_path_from_manifests(
    abs_path: &str,
    default_crate_name: Option<&str>,
    default_manifest_path: Option<&str>,
    manifests: &HashMap<String, String>,
) -> Option<String> {
    let abs_norm = normalize_path(Path::new(abs_path))?;
    let mut best: Option<(usize, String)> = None;

    let mut candidates = Vec::<(String, String)>::new();
    if let (Some(crate_name), Some(manifest_path)) = (default_crate_name, default_manifest_path) {
        candidates.push((crate_name.to_string(), manifest_path.to_string()));
    }
    for (crate_name, manifest_path) in manifests {
        candidates.push((crate_name.clone(), manifest_path.clone()));
    }

    for (crate_name, manifest_path) in candidates {
        let Some(manifest_norm) = normalize_path(Path::new(&manifest_path)) else {
            continue;
        };
        let Ok(rel) = abs_norm.strip_prefix(&manifest_norm) else {
            continue;
        };
        let Some(rel_norm) = normalize_manifest_relative_path(rel) else {
            continue;
        };
        let dep_path = format!("{}/{}", crate_name, rel_norm);
        let manifest_len = manifest_norm.to_string_lossy().len();
        match &best {
            Some((best_len, _)) if *best_len >= manifest_len => {}
            _ => best = Some((manifest_len, dep_path)),
        }
    }

    best.map(|(_, dep_path)| dep_path)
}

// ---------------------------------------------------------------------------
// Crate resource path resolution (wasm vs native)
// ---------------------------------------------------------------------------

/// On wasm, resolve crate resource paths and compute the web_url for HTTP fetching.
/// The abs_path uses normalized Path joining to handle .. segments correctly.
#[cfg(target_arch = "wasm32")]
fn resolve_crate_resource_paths(
    vm: &mut ScriptVm,
    crate_part: &str,
    file_path: &str,
) -> Option<(String, Option<String>, Option<String>)> {
    let file_path = strip_crate_resource_leading_slashes(file_path);
    let manifests = vm.bx.code.crate_manifests.borrow().clone();
    let (abs_path, default_crate_name, default_manifest_path) = if crate_part == "self" {
        let bodies = vm.bx.code.bodies.borrow();
        let body_id = vm.thread().trap.ip.body as usize;
        let body = bodies.get(body_id)?;
        let script_mod = match &body.source {
            ScriptSource::Mod(script_mod) => script_mod,
            _ => return None,
        };
        let abs_path = normalize_path(&Path::new(&script_mod.cargo_manifest_path).join(file_path))
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| {
                let mut fallback = script_mod.cargo_manifest_path.clone();
                fallback.push('/');
                fallback.push_str(file_path);
                fallback
            });
        let crate_name = script_mod
            .module_path
            .split("::")
            .next()
            .unwrap_or("")
            .replace('-', "_");
        (
            abs_path,
            Some(crate_name),
            Some(script_mod.cargo_manifest_path.clone()),
        )
    } else {
        let crate_name = crate_part.replace('-', "_");
        let manifest_path = manifests.get(&crate_name)?.clone();
        let abs_path = normalize_path(&Path::new(&manifest_path).join(file_path))
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| {
                let mut fallback = manifest_path.clone();
                fallback.push('/');
                fallback.push_str(file_path);
                fallback
            });
        (abs_path, Some(crate_name), Some(manifest_path))
    };

    let dependency_path = resolve_dependency_path_from_manifests(
        &abs_path,
        default_crate_name.as_deref(),
        default_manifest_path.as_deref(),
        &manifests,
    );
    let web_url = dependency_path.as_ref().map(|path| format!("/{}", path));
    if web_url.is_none() {
        crate::log!(
            "crate_resource_unmapped crate_part={} file_path={} abs_path={}",
            crate_part,
            file_path,
            abs_path
        );
    }
    Some((abs_path, dependency_path, web_url))
}

/// On native platforms, resolve crate resource paths.
/// Returns (abs_path, dependency_path). web_url is always None on native.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_crate_resource_paths(
    vm: &mut ScriptVm,
    crate_part: &str,
    file_path: &str,
) -> Option<(String, Option<String>, Option<String>)> {
    let file_path = strip_crate_resource_leading_slashes(file_path);
    let (abs_path, crate_name) = if crate_part == "self" {
        let bodies = vm.bx.code.bodies.borrow();
        let body_id = vm.thread().trap.ip.body as usize;
        let body = bodies.get(body_id)?;
        let script_mod = match &body.source {
            ScriptSource::Mod(script_mod) => script_mod,
            _ => return None,
        };
        let mut abs_path = script_mod.cargo_manifest_path.clone();
        abs_path.push('/');
        abs_path.push_str(file_path);
        let crate_name = script_mod
            .module_path
            .split("::")
            .next()
            .unwrap_or("")
            .replace('-', "_");
        (abs_path, crate_name)
    } else {
        let crate_name = crate_part.replace('-', "_");
        let manifests = vm.bx.code.crate_manifests.borrow();
        let manifest_path = manifests.get(&crate_name)?.clone();
        let mut abs_path = manifest_path;
        abs_path.push('/');
        abs_path.push_str(file_path);
        (abs_path, crate_name)
    };

    let dependency_path = normalize_dependency_file_path(file_path)
        .map(|file_path| format!("{}/{}", crate_name, file_path));
    Some((abs_path, dependency_path, None))
}

fn script_value_to_u8_bytes(vm: &mut ScriptVm, value: ScriptValue) -> Option<Vec<u8>> {
    if let Some(array) = value.as_array() {
        if let ScriptArrayStorage::U8(data) = vm.bx.heap.array_storage(array) {
            return Some(data.clone());
        }
    }
    None
}

pub fn script_mod(vm: &mut ScriptVm) {
    let res = vm.new_module(id!(res));
    let res_type = vm.new_handle_type(id_lut!(res));

    // Get the path of the resource
    vm.set_handle_getter(res_type, |vm, pself, prop| {
        if let Some(handle) = pself.as_handle() {
            let cx = vm.host.cx_mut();
            let resources = cx.script_data.resources.resources.borrow();
            if let Some(res) = resources.iter().find(|v| v.handle == handle) {
                match prop {
                    _ if prop == id!(path) => {
                        let path = res.abs_path.clone();
                        drop(resources);
                        return vm
                            .new_string_with(|_vm, s| {
                                s.push_str(&path);
                            })
                            .into();
                    }
                    _ if prop == id!(is_loaded) => {
                        return matches!(res.data, CxScriptResourceData::Loaded(_)).into()
                    }
                    _ if prop == id!(is_error) => {
                        return matches!(res.data, CxScriptResourceData::Error(_)).into()
                    }
                    _ if prop == id!(error) => {
                        if let CxScriptResourceData::Error(ref e) = res.data {
                            let err = e.clone();
                            drop(resources);
                            return vm
                                .new_string_with(|_vm, s| {
                                    s.push_str(&err);
                                })
                                .into();
                        }
                        return NIL;
                    }
                    _ if prop == id!(data) => {
                        if let CxScriptResourceData::Loaded(ref data) = res.data {
                            let data: Vec<u8> = (**data).clone();
                            drop(resources);
                            return vm.bx.heap.new_array_from_vec_u8(data).into();
                        }
                        return NIL;
                    }
                    _ => {}
                }
            }
        }
        script_err_not_found!(vm.trap(), "invalid res prop")
    });

    // res.load_all() - loads all pending resources via the active platform backend
    vm.add_method(
        res,
        id_lut!(load_all_resources),
        script_args_def!(value = NIL),
        move |vm, args| {
            let value = script_value!(vm, args.value);
            let cx = vm.host.cx_mut();
            cx.load_all_script_resources();
            value
        },
    );

    // res.file("/absolute/path/to/file")
    // Uses an absolute file path directly
    vm.add_method(
        res,
        id_lut!(file_resource),
        script_args_def!(path = NIL),
        move |vm, args| {
            let path = script_value!(vm, args.path);
            if !path.is_string_like() {
                return script_err_type_mismatch!(vm.trap(), "invalid res arg type");
            }

            if let Some(abs_path) = vm.string_with(path, |_vm, s| s.to_string()) {
                let cx = vm.host.cx_mut();
                if let Some(existing) = cx.script_data.resources.get_handle_by_abs_path(&abs_path) {
                    return existing.into();
                }

                let handle_gc = CxScriptResourceGc {
                    resources: cx.script_data.resources.resources.clone(),
                    handles_by_abs_path: cx.script_data.resources.handles_by_abs_path.clone(),
                    handle: ScriptHandle::ZERO,
                };
                let handle = vm.bx.heap.new_handle(res_type, Box::new(handle_gc));

                cx.script_data.resources.insert_resource(CxScriptResource {
                    abs_path,
                    dependency_path: None,
                    web_url: None,
                    data: CxScriptResourceData::NotLoaded,
                    handle,
                });

                return handle.into();
            }

            script_err_type_mismatch!(vm.trap(), "invalid res arg type")
        },
    );

    // res.crate("self:path/to/file") or res.crate("crate_name:path/to/file")
    // Resolves a crate-relative path to an absolute path
    vm.add_method(
        res,
        id_lut!(crate_resource),
        script_args_def!(path = NIL),
        move |vm, args| {
            let path = script_value!(vm, args.path);
            if !path.is_string_like() {
                return script_err_type_mismatch!(vm.trap(), "invalid res arg type");
            }

            let path_string = vm.string_with(path, |_vm, s| s.to_string());

            if let Some(path_string) = path_string {
                // Parse "crate:path" format
                if let Some((crate_part, file_path)) = parse_crate_path(&path_string) {
                    if let Some((abs_path, dependency_path, web_url)) =
                        resolve_crate_resource_paths(vm, crate_part, file_path)
                    {
                        let cx = vm.host.cx_mut();
                        if let Some(existing) =
                            cx.script_data.resources.get_handle_by_abs_path(&abs_path)
                        {
                            return existing.into();
                        }

                        let handle_gc = CxScriptResourceGc {
                            resources: cx.script_data.resources.resources.clone(),
                            handles_by_abs_path: cx
                                .script_data
                                .resources
                                .handles_by_abs_path
                                .clone(),
                            handle: ScriptHandle::ZERO,
                        };
                        let handle = vm.bx.heap.new_handle(res_type, Box::new(handle_gc));

                        cx.script_data.resources.insert_resource(CxScriptResource {
                            abs_path,
                            dependency_path,
                            web_url,
                            data: CxScriptResourceData::NotLoaded,
                            handle,
                        });

                        return handle.into();
                    }
                }
            }

            script_err_type_mismatch!(vm.trap(), "invalid res arg type")
        },
    );

    // res.http_resource("https://example.com/file.svg")
    // Loads a resource from an HTTP URL asynchronously
    vm.add_method(
        res,
        id_lut!(http_resource),
        script_args_def!(url = NIL),
        move |vm, args| {
            let url = script_value!(vm, args.url);
            if !url.is_string_like() {
                return script_err_type_mismatch!(vm.trap(), "invalid res arg type");
            }

            if let Some(url_string) = vm.string_with(url, |_vm, s| s.to_string()) {
                let cx = vm.host.cx_mut();
                if let Some(existing) = cx.script_data.resources.get_handle_by_abs_path(&url_string)
                {
                    return existing.into();
                }
                let handle_gc = CxScriptResourceGc {
                    resources: cx.script_data.resources.resources.clone(),
                    handles_by_abs_path: cx.script_data.resources.handles_by_abs_path.clone(),
                    handle: ScriptHandle::ZERO,
                };
                let handle = vm.bx.heap.new_handle(res_type, Box::new(handle_gc));

                // Create the resource in Loading state
                cx.script_data.resources.insert_resource(CxScriptResource {
                    abs_path: url_string.clone(),
                    dependency_path: None,
                    web_url: None,
                    data: CxScriptResourceData::Loading,
                    handle,
                });

                // Fire the HTTP request
                let request_id = LiveId::unique();
                cx.script_data
                    .resources
                    .http_resources
                    .push(CxScriptHttpResource { request_id, handle });
                cx.http_request(request_id, HttpRequest::new(url_string, Default::default()));

                return handle.into();
            }

            script_err_type_mismatch!(vm.trap(), "invalid res arg type")
        },
    );

    // res.binary_resource(bytes_u8_array)
    // Creates an in-memory resource directly from bytes.
    vm.add_method(
        res,
        id_lut!(binary_resource),
        script_args_def!(data = NIL),
        move |vm, args| {
            let data = script_value!(vm, args.data);
            let Some(bytes) = script_value_to_u8_bytes(vm, data) else {
                return script_err_type_mismatch!(
                    vm.trap(),
                    "binary_resource expects a U8 byte array"
                );
            };

            let cx = vm.host.cx_mut();
            let handle_gc = CxScriptResourceGc {
                resources: cx.script_data.resources.resources.clone(),
                handles_by_abs_path: cx.script_data.resources.handles_by_abs_path.clone(),
                handle: ScriptHandle::ZERO,
            };
            let handle = vm.bx.heap.new_handle(res_type, Box::new(handle_gc));

            cx.script_data.resources.insert_resource(CxScriptResource {
                abs_path: format!("binary://{}", LiveId::unique().0),
                dependency_path: None,
                web_url: None,
                data: CxScriptResourceData::Loaded(Rc::new(bytes)),
                handle,
            });

            handle.into()
        },
    );
}
