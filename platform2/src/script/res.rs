use crate::*;
use makepad_script::*;
use makepad_script::id;
use crate::script::vm::*;
use std::rc::Rc;
use std::cell::RefCell;

#[derive(Clone, Debug)]
pub enum CxScriptResourceData {
    Loading,
    Loaded(Vec<u8>),
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

pub struct CxScriptResourceGc {
    pub resources: Rc<RefCell<Vec<CxScriptResource>>>,
    pub handle: ScriptHandle,
}

impl ScriptHandleGc for CxScriptResourceGc {
    fn gc(&mut self) {
        self.resources.borrow_mut().retain(|v| v.handle != self.handle)
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

pub fn extend_std_module_with_res(vm: &mut ScriptVm) {
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
                        return vm.heap.new_string_with(|_heap, s| {
                            s.push_str(&path);
                        }).into()
                    }
                    _ if prop == id!(is_loading) => {
                        return matches!(res.data, CxScriptResourceData::Loading).into()
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
                            return vm.heap.new_string_with(|_heap, s| {
                                s.push_str(&err);
                            }).into()
                        }
                        return NIL
                    }
                    _ if prop == id!(data) => {
                        if let CxScriptResourceData::Loaded(ref data) = res.data {
                            let data = data.clone();
                            drop(resources);
                            return vm.heap.new_array_from_vec_u8(data).into()
                        }
                        return NIL
                    }
                    _ => {}
                }
            }
        }
        vm.thread.trap.err_invalid_prop_name()
    });
    
    // res.crate("self:path/to/file") or res.crate("crate_name:path/to/file")
    // Resolves a crate-relative path to an absolute path
    vm.add_method(res, id_lut!(crate), script_args_def!(path = NIL), move |vm, args| {
        let path = script_value!(vm, args.path);
        if !path.is_string_like() {
            return vm.thread.trap.err_invalid_arg_type()
        }
        
        let path_string = vm.heap.string_with(path, |_heap, s| s.to_string());
        
        if let Some(path_string) = path_string {
            // Parse "crate:path" format
            if let Some((crate_part, file_path)) = parse_crate_path(&path_string) {
                let abs_path = if crate_part == "self" {
                    // Get cargo_manifest_path from current script body
                    let bodies = vm.code.bodies.borrow();
                    let body_id = vm.thread.trap.ip.body as usize;
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
                    let manifests = vm.code.crate_manifests.borrow();
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
                        handle: ScriptHandle::ZERO
                    };
                    let handle = vm.heap.new_handle(res_type, Box::new(handle_gc));
                    
                    cx.script_data.resources.resources.borrow_mut().push(
                        CxScriptResource {
                            abs_path,
                            data: CxScriptResourceData::Loading,
                            handle,
                        }
                    );
                    
                    return handle.into()
                }
            }
        }
        
        vm.thread.trap.err_invalid_arg_type()
    });
}
