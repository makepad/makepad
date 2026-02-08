use crate::script::vm::*;
use crate::*;
use makepad_script::id;
use makepad_script::*;
use std::cell::RefCell;
use std::fs::File;
use std::io::Read;
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum CxScriptResourceData {
    NotLoaded,
    Loaded(Rc<Vec<u8>>),
    Error(String),
}

#[derive(Clone)]
pub struct CxScriptResource {
    pub abs_path: String,
    pub data: CxScriptResourceData,
    pub handle: ScriptHandle,
}

#[derive(Default)]
pub struct CxScriptResources {
    pub resources: Rc<RefCell<Vec<CxScriptResource>>>,
}

impl CxScriptResources {
    /// Get the data for a resource by handle
    pub fn get_data(&self, handle: ScriptHandle) -> Option<Rc<Vec<u8>>> {
        let resources = self.resources.borrow();
        if let Some(res) = resources.iter().find(|v| v.handle == handle) {
            match &res.data {
                CxScriptResourceData::Loaded(data) => return Some(data.clone()),
                CxScriptResourceData::NotLoaded => {
                    println!(
                        "get_data: handle {:?} path={:?} NOT YET LOADED",
                        handle, res.abs_path
                    );
                }
                CxScriptResourceData::Error(e) => {
                    println!(
                        "get_data: handle {:?} path={:?} ERROR: {}",
                        handle, res.abs_path, e
                    );
                }
            }
        } else {
            println!(
                "get_data: handle {:?} NOT FOUND in resources list ({} entries)",
                handle,
                resources.len()
            );
        }
        None
    }

    /// Load all resources that haven't been loaded yet
    pub fn load_all_resources(&self) {
        let mut resources = self.resources.borrow_mut();
        for res in resources.iter_mut() {
            if matches!(res.data, CxScriptResourceData::NotLoaded) {
                println!(
                    "load_all_resources: loading {:?} handle={:?}",
                    res.abs_path, res.handle
                );
                match File::open(&res.abs_path) {
                    Ok(mut file) => {
                        let mut data = Vec::new();
                        match file.read_to_end(&mut data) {
                            Ok(_) => {
                                println!("  -> loaded {} bytes", data.len());
                                res.data = CxScriptResourceData::Loaded(Rc::new(data));
                            }
                            Err(e) => {
                                println!("  -> read error: {}", e);
                                res.data = CxScriptResourceData::Error(format!(
                                    "Failed to read file: {}",
                                    e
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        println!("  -> open error: {}", e);
                        res.data =
                            CxScriptResourceData::Error(format!("Failed to open file: {}", e));
                    }
                }
            }
        }
    }
}

pub struct CxScriptResourceGc {
    pub resources: Rc<RefCell<Vec<CxScriptResource>>>,
    pub handle: ScriptHandle,
}

impl ScriptHandleGc for CxScriptResourceGc {
    fn gc(&mut self) {
        self.resources
            .borrow_mut()
            .retain(|v| v.handle != self.handle)
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
            cx.script_data.resources.load_all_resources();
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
                let handle_gc = CxScriptResourceGc {
                    resources: cx.script_data.resources.resources.clone(),
                    handle: ScriptHandle::ZERO,
                };
                let handle = vm.bx.heap.new_handle(res_type, Box::new(handle_gc));

                cx.script_data
                    .resources
                    .resources
                    .borrow_mut()
                    .push(CxScriptResource {
                        abs_path,
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
                    let abs_path = if crate_part == "self" {
                        // Get cargo_manifest_path from current script body
                        let bodies = vm.bx.code.bodies.borrow();
                        let body_id = vm.thread().trap.ip.body as usize;
                        if let Some(body) = bodies.get(body_id) {
                            if let ScriptSource::Mod(script_mod) = &body.source {
                                let mut final_path = script_mod.cargo_manifest_path.clone();
                                final_path.push('/');
                                final_path.push_str(file_path);
                                Some(final_path)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        // Look up crate name in the manifest table on ScriptCode
                        let crate_name = crate_part.replace('-', "_");
                        let manifests = vm.bx.code.crate_manifests.borrow();
                        if let Some(manifest_path) = manifests.get(&crate_name) {
                            let mut final_path = manifest_path.clone();
                            final_path.push('/');
                            final_path.push_str(file_path);
                            Some(final_path)
                        } else {
                            None
                        }
                    };
                    if let Some(abs_path) = abs_path {
                        let cx = vm.host.cx_mut();
                        let handle_gc = CxScriptResourceGc {
                            resources: cx.script_data.resources.resources.clone(),
                            handle: ScriptHandle::ZERO,
                        };
                        let handle = vm.bx.heap.new_handle(res_type, Box::new(handle_gc));

                        cx.script_data
                            .resources
                            .resources
                            .borrow_mut()
                            .push(CxScriptResource {
                                abs_path,
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
}
