use crate::makepad_network::HttpRequest;
use crate::script::vm::*;
use crate::*;
use makepad_script::id;
use makepad_script::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
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

    /// Load all resources that haven't been loaded yet (skips Loading/HTTP resources)
    pub fn load_all_resources(&self) {
        let mut resources = self.resources.borrow_mut();
        for res in resources.iter_mut() {
            if matches!(res.data, CxScriptResourceData::NotLoaded) {
                match File::open(&res.abs_path) {
                    Ok(mut file) => {
                        let mut data = Vec::new();
                        match file.read_to_end(&mut data) {
                            Ok(_) => {
                                res.data = CxScriptResourceData::Loaded(Rc::new(data));
                            }
                            Err(e) => {
                                res.data = CxScriptResourceData::Error(format!(
                                    "Failed to read file: {}",
                                    e
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        res.data =
                            CxScriptResourceData::Error(format!("Failed to open file: {}", e));
                    }
                }
            }
        }
    }
}

impl Cx {
    /// Load all script resources that are still pending.
    ///
    /// On web we first try wasm dependencies (`dependency_path`) and fall back to
    /// async HTTP fetch via `web_url`.
    pub fn load_all_script_resources(&mut self) {
        let mut pending_http = Vec::<(LiveId, ScriptHandle, String)>::new();
        let is_web = self.os_type().is_web();

        {
            let mut resources = self.script_data.resources.resources.borrow_mut();
            for res in resources.iter_mut() {
                if !matches!(res.data, CxScriptResourceData::NotLoaded) {
                    continue;
                }

                if let Some(dep_path) = res.dependency_path.as_deref() {
                    if let Ok(data) = self.get_dependency(dep_path) {
                        res.data = CxScriptResourceData::Loaded(data);
                        continue;
                    }
                }

                if is_web {
                    if let Some(url) = res.web_url.clone() {
                        let request_id = LiveId::unique();
                        res.data = CxScriptResourceData::Loading;
                        pending_http.push((request_id, res.handle, url));
                        continue;
                    }
                }

                // Try loading from the filesystem (works on desktop/simulator)
                if let Ok(mut file) = File::open(&res.abs_path) {
                    let mut data = Vec::new();
                    match file.read_to_end(&mut data) {
                        Ok(_) => {
                            res.data = CxScriptResourceData::Loaded(Rc::new(data));
                        }
                        Err(e) => {
                            res.data =
                                CxScriptResourceData::Error(format!("Failed to read file: {}", e));
                        }
                    }
                    continue;
                }

                // On iOS/tvOS device, fall back to loading from the app bundle.
                // The bundle has resources at {bundle}/makepad/{crate_name}/{file_path}.
                #[cfg(any(target_os = "ios", target_os = "tvos"))]
                {
                    if let Some(dep_path) = res.dependency_path.as_deref() {
                        let bundle_path = if let Some(root) = self.package_root.as_deref() {
                            format!("{}/{}", root, dep_path)
                        } else {
                            dep_path.to_string()
                        };
                        if let Ok(data) = self.apple_bundle_load_file(&bundle_path) {
                            res.data = CxScriptResourceData::Loaded(data);
                            continue;
                        }
                    }
                }

                res.data = CxScriptResourceData::Error(format!(
                    "Failed to open resource: {}",
                    res.abs_path
                ));
            }
        }

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

/// Parses a crate path like "self:resources/file.jpg" or "other_crate:path/file.ext"
/// Returns (crate_part, file_path)
fn parse_crate_path(path: &str) -> Option<(&str, &str)> {
    let mut split = path.splitn(2, ':');
    let crate_part = split.next()?;
    let file_path = split.next()?;
    Some((crate_part, file_path))
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

    // res.load_all() - loads all pending resources from disk
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
                    let (abs_path, crate_name) = if crate_part == "self" {
                        // Get cargo_manifest_path from current script body
                        let bodies = vm.bx.code.bodies.borrow();
                        let body_id = vm.thread().trap.ip.body as usize;
                        if let Some(body) = bodies.get(body_id) {
                            if let ScriptSource::Mod(script_mod) = &body.source {
                                let mut final_path = script_mod.cargo_manifest_path.clone();
                                final_path.push('/');
                                final_path.push_str(file_path);
                                let self_crate_name = script_mod
                                    .module_path
                                    .split("::")
                                    .next()
                                    .unwrap_or("")
                                    .replace('-', "_");
                                (Some(final_path), Some(self_crate_name))
                            } else {
                                (None, None)
                            }
                        } else {
                            (None, None)
                        }
                    } else {
                        // Look up crate name in the manifest table on ScriptCode
                        let crate_name = crate_part.replace('-', "_");
                        let manifests = vm.bx.code.crate_manifests.borrow();
                        if let Some(manifest_path) = manifests.get(&crate_name) {
                            let mut final_path = manifest_path.clone();
                            final_path.push('/');
                            final_path.push_str(file_path);
                            (Some(final_path), Some(crate_name))
                        } else {
                            (None, None)
                        }
                    };
                    if let Some(abs_path) = abs_path {
                        let cx = vm.host.cx_mut();
                        if let Some(existing) =
                            cx.script_data.resources.get_handle_by_abs_path(&abs_path)
                        {
                            return existing.into();
                        }

                        let dependency_path = if let Some(crate_name) = crate_name {
                            normalize_dependency_file_path(file_path)
                                .map(|file_path| format!("{}/{}", crate_name, file_path))
                        } else {
                            None
                        };
                        let web_url = dependency_path.as_ref().map(|path| format!("/{}", path));

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
