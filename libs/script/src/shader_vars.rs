//! Shader variable and field operations
//!
//! This module contains handle functions for variable declarations (let, var),
//! assignments, field access, and logging.

use std::fmt::Write;
use makepad_live_id::*;
use crate::value::*;
use crate::vm::*;
use crate::opcode::*;
use crate::trap::*;
use crate::shader::*;
use crate::shader_tables::*;
use crate::shader_backend::*;
use crate::mod_shader::*;
use crate::suggest::*;
use crate::*;

impl ShaderFnCompiler {
    pub(crate) fn handle_log(&mut self, vm: &ScriptVm) {
        let (ty, value_str) = self.stack.peek(self.trap.pass());
        let type_name = self.shader_type_to_string(vm, ty);
        if let Some(loc) = vm.code.ip_to_loc(self.trap.ip) {
            crate::makepad_error_log::log_with_level(
                &loc.file,
                loc.line,
                loc.col,
                loc.line,
                loc.col,
                format!("{}:{}", value_str, type_name),
                crate::makepad_error_log::LogLevel::Log,
            );
        }
    }

    pub(crate) fn shader_type_to_string(&self, vm: &ScriptVm, ty: &ShaderType) -> String {
        match ty {
            ShaderType::None => "none".to_string(),
            ShaderType::IoSelf(_) => "io".to_string(),
            ShaderType::PodType(pod_ty) | ShaderType::Pod(pod_ty) | ShaderType::PodPtr(pod_ty) => {
                if let Some(name) = vm.heap.pod_type_name(*pod_ty) {
                    name.to_string()
                } else {
                    format!("{:?}", pod_ty)
                }
            }
            ShaderType::Id(id) => {
                // Try to resolve the id to get its actual type
                if let Some((sc, _shadow)) = self.shader_scope.find_var(*id) {
                    let pod_ty = sc.ty();
                    if let Some(name) = vm.heap.pod_type_name(pod_ty) {
                        return name.to_string();
                    }
                }
                format!("id({})", id)
            }
            ShaderType::AbstractInt => "abstract_int".to_string(),
            ShaderType::AbstractFloat => "abstract_float".to_string(),
            ShaderType::Range { ty, .. } => {
                if let Some(name) = vm.heap.pod_type_name(*ty) {
                    format!("range<{}>", name)
                } else {
                    "range".to_string()
                }
            }
            ShaderType::Error(_) => "error".to_string(),
            ShaderType::Texture(tex_type) => format!("texture({:?})", tex_type),
            ShaderType::ScopeTexture { tex_type, .. } => format!("scope_texture({:?})", tex_type),
            ShaderType::ScopeObject(_) => "scope_object".to_string(),
            ShaderType::ScopeUniformBuffer { pod_ty, .. } => {
                if let Some(name) = vm.heap.pod_type_name(*pod_ty) {
                    format!("scope_uniform_buffer<{}>", name)
                } else {
                    "scope_uniform_buffer".to_string()
                }
            }
        }
    }

    pub(crate) fn handle_assign(&mut self, vm: &mut ScriptVm) {
        let (_value_ty, value) = self.stack.pop(self.trap.pass());
        let (id_ty, _id) = self.stack.pop(self.trap.pass());
        if let ShaderType::Id(id) = id_ty {
            if let Some((var, shadow)) = self.shader_scope.find_var(id) {
                if !matches!(var, ShaderScopeItem::Var { .. }) {
                    script_err_let_is_immutable!(self.trap, "cannot assign to let binding {:?}", id);
                }
                let mut s = self.stack.new_string();
                if shadow > 0 {
                    write!(s, "_s{}{}", shadow, id).ok();
                } else {
                    write!(s, "{}", id).ok();
                }
                write!(s, " = {}", value).ok();
                self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), s);
            } else {
                script_err_not_found!(self.trap, "variable {:?} not found in shader scope{}", id, suggest_from_live_ids(id, &self.shader_scope.all_var_names()));
                self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
            }
        } else {
            script_err_not_assignable!(self.trap, "shader assign target is not an id");
            self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
        }
        self.stack.free_string(value);
    }

    pub(crate) fn handle_assign_field(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (value_ty, value_s) = self.pop_resolved(vm, output);
        let (field_ty, field_s) = self.stack.pop(self.trap.pass());
        let (instance_ty, instance_s) = self.pop_resolved(vm, output);

        if let ShaderType::Id(field_id) = field_ty {
            if let ShaderType::Pod(pod_ty) = instance_ty {
                if let Some(ret_ty) = vm.heap.pod_field_type(pod_ty, field_id, &vm.code.builtins.pod) {
                    let val_ty = value_ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                    if val_ty != ret_ty {
                        script_err_pod_type_not_matching!(self.trap, "field {:?} type mismatch", field_id);
                    }

                    let mut s = self.stack.new_string();
                    write!(s, "{}.{} = {}", instance_s, field_id, value_s).ok();
                    self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), s);
                } else {
                    script_err_not_found!(self.trap, "field {:?} not found on pod{}", field_id, suggest_pod_field(&vm.heap, pod_ty, field_id));
                    self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                }
            } else if let ShaderType::PodPtr(pod_ty) = instance_ty {
                // Pointer type (e.g., uniform buffer in Metal) - use -> for field access
                if let Some(ret_ty) = vm.heap.pod_field_type(pod_ty, field_id, &vm.code.builtins.pod) {
                    let val_ty = value_ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                    if val_ty != ret_ty {
                        script_err_pod_type_not_matching!(self.trap, "field {:?} type mismatch on pod ptr", field_id);
                    }

                    let mut s = self.stack.new_string();
                    write!(s, "{}->{} = {}", instance_s, field_id, value_s).ok();
                    self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), s);
                } else {
                    script_err_not_found!(self.trap, "field {:?} not found on pod ptr{}", field_id, suggest_pod_field(&vm.heap, pod_ty, field_id));
                    self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                }
            } else if let ShaderType::IoSelf(obj) = instance_ty {
                let value = vm.heap.value(obj, field_id.into(), self.trap.pass());
                if let Some(value_obj) = value.as_object() {
                    if let Some(io_type) = vm.heap.as_shader_io(value_obj) {
                        let allowed = match io_type {
                            SHADER_IO_VARYING => output.mode == ShaderMode::Vertex,
                            SHADER_IO_VERTEX_POSITION => output.mode == ShaderMode::Vertex,
                            io_type if io_type.0 >= SHADER_IO_FRAGMENT_OUTPUT_0.0 && io_type.0 <= SHADER_IO_FRAGMENT_OUTPUT_MAX.0 => {
                                output.mode == ShaderMode::Fragment
                            }
                            _ => false,
                        };

                        if !allowed {
                            script_err_assign_not_allowed!(self.trap, "cannot assign to shader io in this mode");
                            self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                            self.stack.free_string(value_s);
                            self.stack.free_string(field_s);
                            self.stack.free_string(instance_s);
                            return;
                        }

                        // we need to find the type of the field
                        let proto = vm.heap.proto(value.as_object().unwrap());
                        let ty = Self::type_from_value(vm, proto);
                        let concrete_ty = match ty {
                            ShaderType::Pod(pt) => Some(pt),
                            ShaderType::PodType(pt) => Some(pt),
                            _ => None,
                        };

                        if let Some(pod_ty) = concrete_ty {
                            let val_ty = value_ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                            if val_ty != pod_ty {
                                script_err_pod_type_not_matching!(self.trap, "shader io field type mismatch");
                            }

                            let (kind, prefix) = output.backend.get_shader_io_kind_and_prefix(output.mode, io_type);

                            if !output.io.iter().any(|io| io.name == field_id) {
                                output.io.push(ShaderIo {
                                    kind,
                                    name: field_id,
                                    ty: pod_ty,
                                    buffer_index: None,
                                });
                            }
                            let mut s = self.stack.new_string();
                            match prefix {
                                ShaderIoPrefix::Prefix(prefix) => write!(s, "{}{} = {}", prefix, field_id, value_s).ok(),
                                ShaderIoPrefix::Full(full) => write!(s, "{} = {}", full, value_s).ok(),
                                ShaderIoPrefix::FullOwned(full) => write!(s, "{} = {}", full, value_s).ok(),
                            };
                            self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), s);
                            self.stack.free_string(field_s);
                            self.stack.free_string(instance_s);
                            self.stack.free_string(value_s);
                            return;
                        }
                    }
                }
                script_err_no_matching_shader_type!(self.trap, "no matching shader type for IoSelf field");
                self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
            } else {
                script_err_no_matching_shader_type!(self.trap, "no matching shader type for instance");
                self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
            }
        } else {
            script_err_unexpected!(self.trap, "field assign requires Id field type");
            self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
        }
        self.stack.free_string(value_s);
        self.stack.free_string(field_s);
        self.stack.free_string(instance_s);
    }

    pub(crate) fn handle_assign_index(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (value_ty, value_s) = self.pop_resolved(vm, output);
        let (index_ty, index_s) = self.pop_resolved(vm, output);
        let (instance_ty, instance_s) = self.pop_resolved(vm, output);

        if let ShaderType::Pod(pod_ty) = instance_ty {
            let builtins = &vm.code.builtins.pod;
            let elem_ty = type_table_elem_type(&vm.heap.pod_types[pod_ty.index as usize].ty, self.trap.pass(), builtins);

            if let Some(ret_ty) = elem_ty {
                match index_ty {
                    ShaderType::AbstractInt => {}
                    ShaderType::Pod(t) if t == builtins.pod_i32 || t == builtins.pod_u32 => {}
                    _ => {
                        script_err_pod_type_not_matching!(self.trap, "index type must be int or uint");
                    }
                }

                let val_ty = value_ty.make_concrete(builtins).unwrap_or(builtins.pod_void);
                if val_ty != ret_ty {
                    script_err_pod_type_not_matching!(self.trap, "value type mismatch in index assign");
                }

                let mut s = self.stack.new_string();
                write!(s, "{}[{}] = {}", instance_s, index_s, value_s).ok();
                self.stack.push(self.trap.pass(), ShaderType::Pod(builtins.pod_void), s);
            } else {
                script_err_not_assignable!(self.trap, "index assign not supported for this type");
                self.stack.push(self.trap.pass(), ShaderType::Pod(builtins.pod_void), String::new());
            }
        } else {
            script_err_no_matching_shader_type!(self.trap, "index assign requires Pod type");
            self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
        }
        self.stack.free_string(value_s);
        self.stack.free_string(index_s);
        self.stack.free_string(instance_s);
    }

    pub(crate) fn handle_assign_me(&mut self, vm: &mut ScriptVm) {
        let (val_ty, val_s) = self.stack.pop(self.trap.pass());
        let (id_ty, id_s) = self.stack.pop(self.trap.pass());
        if let ShaderType::Id(id) = id_ty {
            if let Some(ShaderMe::Pod { args, .. }) = self.mes.last_mut() {
                if let Some(last) = args.last() {
                    if last.name.is_none() {
                        script_err_use_only_named_or_ordered_pod_fields!(self.trap, "mixing named and ordered args");
                    }
                }
                args.push(ShaderPodArg {
                    name: Some(id),
                    ty: val_ty,
                    s: val_s,
                });
            } else {
                script_err_unexpected!(self.trap, "assign_me requires Pod on me stack");
                self.stack.free_string(val_s);
            }
            self.stack.free_string(id_s);
        } else {
            script_err_unexpected!(self.trap, "assign_me requires Id type");
            self.stack.free_string(val_s);
            self.stack.free_string(id_s);
            self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
        }
    }

    pub(crate) fn type_from_value(vm: &ScriptVm, value: ScriptValue) -> ShaderType {
        if let Some(pod_ty) = vm.code.builtins.pod.value_to_exact_type(value) {
            return ShaderType::Pod(pod_ty);
        }
        // Check if it's a color - colors map to vec4f
        if value.is_color() {
            return ShaderType::Pod(vm.code.builtins.pod.pod_vec4f);
        }
        if let Some(pod_ty) = vm.heap.pod_type(value) {
            return ShaderType::PodType(pod_ty);
        }
        if let Some(pod) = value.as_pod() {
            let pod = &vm.heap.pods[pod.index as usize];
            return ShaderType::Pod(pod.ty);
        }
        if let Some(pod_ty) = value.as_pod_type() {
            return ShaderType::Pod(pod_ty);
        }
        ShaderType::None
    }

    /// Find the highest (most ancestral) shader IO definition for a field in the prototype chain.
    /// This ensures that if a parent defines `x: shader.uniform(vec4f)` and a child overrides
    /// with `x: #ffff`, we still use the uniform type from the parent.
    /// Returns (value_object, shader_io_type) if found, or None if no shader IO marker exists.
    pub(crate) fn find_highest_shader_io(
        vm: &ScriptVm,
        io_self: ScriptObject,
        field_id: LiveId,
        _trap: ScriptTrap,
    ) -> Option<(ScriptObject, ShaderIoType)> {
        let mut result: Option<(ScriptObject, ShaderIoType)> = None;
        let mut current: Option<ScriptObject> = Some(io_self);

        // Walk up the prototype chain
        while let Some(obj) = current {
            // Check if this object has the field directly (not inherited)
            let obj_data = vm.heap.object_data(obj);
            if let Some(map_value) = obj_data.map.get(&field_id.into()) {
                if let Some(value_obj) = map_value.value.as_object() {
                    if let Some(io_type) = vm.heap.as_shader_io(value_obj) {
                        // Found a shader IO marker - keep track of it (will be overwritten by higher ones)
                        result = Some((value_obj, io_type));
                    }
                }
            }

            // Move to parent prototype
            current = vm.heap.proto(obj).as_object();
        }

        result
    }
    

    /// Get the value for a field, preferring inherited shader IO markers over local values.
    /// If a shader IO marker exists higher in the prototype chain, returns that.
    /// Otherwise returns the normal (lowest/most derived) value.
    pub(crate) fn get_io_self_field_value(
        vm: &ScriptVm,
        io_self: ScriptObject,
        field_id: LiveId,
        trap: ScriptTrap,
    ) -> (ScriptValue, Option<ShaderIoType>) {
        // First, try to find the highest shader IO definition
        if let Some((io_obj, io_type)) = Self::find_highest_shader_io(vm, io_self, field_id, trap) {
            return (io_obj.into(), Some(io_type));
        }

        // No shader IO marker found - get the normal value (for RustInstance fields)
        let value = vm.heap.value(io_self, field_id.into(), trap);
        (value, None)
    }

    pub(crate) fn handle_field(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (field_ty, field_s) = self.stack.pop(self.trap.pass());
        let (instance_ty, instance_s) = self.pop_resolved(vm, output);

        if let ShaderType::Id(field_id) = field_ty {
            if let ShaderType::Pod(pod_ty) = instance_ty {
                if let Some(ret_ty) = vm.heap.pod_field_type(pod_ty, field_id, &vm.code.builtins.pod) {
                    let mut s = self.stack.new_string();
                    write!(s, "{}.{}", instance_s, field_id).ok();
                    self.stack.push(self.trap.pass(), ShaderType::Pod(ret_ty), s);
                } else {
                    script_err_not_found!(self.trap, "field {:?} not found on Pod{}", field_id, suggest_pod_field(&vm.heap, pod_ty, field_id));
                    self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                }
                self.stack.free_string(field_s);
                self.stack.free_string(instance_s);
                return;
            } else if let ShaderType::PodPtr(pod_ty) = instance_ty {
                // Pointer type (e.g., uniform buffer in Metal) - use -> for field access
                if let Some(ret_ty) = vm.heap.pod_field_type(pod_ty, field_id, &vm.code.builtins.pod) {
                    let mut s = self.stack.new_string();
                    write!(s, "{}->{}", instance_s, field_id).ok();
                    self.stack.push(self.trap.pass(), ShaderType::Pod(ret_ty), s);
                } else {
                    script_err_not_found!(self.trap, "field {:?} not found on PodPtr{}", field_id, suggest_pod_field(&vm.heap, pod_ty, field_id));
                    self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                }
                self.stack.free_string(field_s);
                self.stack.free_string(instance_s);
                return;
            } else if let ShaderType::Texture(tex_type) = instance_ty {
                // Field/method access on a texture - push texture and field name for method call handling
                // The field name (like "size") will be used as the method name
                self.stack.push(self.trap.pass(), ShaderType::Texture(tex_type), instance_s);
                self.stack.push(self.trap.pass(), ShaderType::Id(field_id), field_s);
                return;
            } else if let ShaderType::ScopeObject(obj) = instance_ty {
                // Field access on a scope object (e.g., test_obj.p2 or test_obj.objfn or test_obj.sub_obj)
                // Look up the field value
                let value = vm.heap.value(obj, field_id.into(), self.trap.pass());
                
                if !value.is_nil() && self.trap.err.borrow().is_empty() {
                    // Check if this is an object
                    if let Some(value_obj) = value.as_object() {
                        // Check if this is a shader_io type - not supported for scope objects
                        if vm.heap.as_shader_io(value_obj).is_some() {
                            script_err_opcode_not_supported_in_shader!(self.trap, "shader_io not supported on scope objects");
                            self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                            self.stack.free_string(field_s);
                            self.stack.free_string(instance_s);
                            return;
                        }
                        
                        // Check if it's a function - if so, push it for calling
                        if vm.heap.as_fn(value_obj).is_some() {
                            // Push the ScopeObject as the receiver and the function for calling
                            // Similar to how IoSelf handles method calls
                            self.stack.push(self.trap.pass(), ShaderType::ScopeObject(obj), instance_s);
                            self.stack.push(self.trap.pass(), ShaderType::Id(field_id), field_s);
                            return;
                        }
                        
                        // It's a regular sub-object (like test_obj.sub_obj) - return it as ScopeObject
                        // so that further field access can continue (e.g., test_obj.sub_obj.test_p1)
                        let empty_s = self.stack.new_string();
                        self.stack.push(self.trap.pass(), ShaderType::ScopeObject(value_obj), empty_s);
                        self.stack.free_string(field_s);
                        self.stack.free_string(instance_s);
                        return;
                    }
                    
                    // Get the pod type from the value - it's a regular property (primitive or pod)
                    if let Some(pod_ty) = self.get_scope_value_pod_type(vm, value) {
                        // Check if we already have this scope uniform
                        let existing = output.scope_uniforms.iter().find(|su| 
                            su.source_obj == obj && su.key == field_id
                        );
                        
                        let shader_name = if let Some(existing) = existing {
                            existing.shader_name
                        } else {
                            // Generate unique name if there's a collision (use obj as source)
                            let shader_name = self.generate_scope_uniform_name(output, field_id, obj);
                            output.scope_uniforms.push(ScopeUniformSource {
                                source_obj: obj,
                                key: field_id,
                                shader_name,
                                ty: pod_ty,
                            });
                            // Also add to IO list
                            if !output.io.iter().any(|io| io.name == shader_name && matches!(io.kind, ShaderIoKind::ScopeUniform)) {
                                vm.heap.pod_type_name_if_not_set(pod_ty, shader_name);
                                output.io.push(ShaderIo {
                                    kind: ShaderIoKind::ScopeUniform,
                                    name: shader_name,
                                    ty: pod_ty,
                                    buffer_index: None,
                                });
                            }
                            shader_name
                        };
                        
                        let mut s = self.stack.new_string();
                        let (_, prefix) = output.backend.get_shader_io_kind_and_prefix(output.mode, SHADER_IO_SCOPE_UNIFORM);
                        match prefix {
                            ShaderIoPrefix::Prefix(prefix) => write!(s, "{}{}", prefix, shader_name).ok(),
                            ShaderIoPrefix::Full(full) => write!(s, "{}", full).ok(),
                            ShaderIoPrefix::FullOwned(full) => write!(s, "{}", full).ok(),
                        };
                        self.stack.push(self.trap.pass(), ShaderType::Pod(pod_ty), s);
                        self.stack.free_string(field_s);
                        self.stack.free_string(instance_s);
                        return;
                    }
                }
                
                // Value not found on prototype - try to get the type from type-check structure
                self.trap.err.take(); // Clear any error from value lookup
                if let Some(field_type_id) = vm.heap.field_type_from_type_check(obj, field_id) {
                    // Found field type in type-check structure - convert to pod type
                    if let Some(pod_ty) = vm.heap.type_id_to_pod_type(field_type_id, &vm.code.builtins.pod) {
                        // Check if we already have this scope uniform
                        let existing = output.scope_uniforms.iter().find(|su| 
                            su.source_obj == obj && su.key == field_id
                        );
                        
                        let shader_name = if let Some(existing) = existing {
                            existing.shader_name
                        } else {
                            let shader_name = self.generate_scope_uniform_name(output, field_id, obj);
                            output.scope_uniforms.push(ScopeUniformSource {
                                source_obj: obj,
                                key: field_id,
                                shader_name,
                                ty: pod_ty,
                            });
                            if !output.io.iter().any(|io| io.name == shader_name && matches!(io.kind, ShaderIoKind::ScopeUniform)) {
                                vm.heap.pod_type_name_if_not_set(pod_ty, shader_name);
                                output.io.push(ShaderIo {
                                    kind: ShaderIoKind::ScopeUniform,
                                    name: shader_name,
                                    ty: pod_ty,
                                    buffer_index: None,
                                });
                            }
                            shader_name
                        };
                        
                        let mut s = self.stack.new_string();
                        let (_, prefix) = output.backend.get_shader_io_kind_and_prefix(output.mode, SHADER_IO_SCOPE_UNIFORM);
                        match prefix {
                            ShaderIoPrefix::Prefix(prefix) => write!(s, "{}{}", prefix, shader_name).ok(),
                            ShaderIoPrefix::Full(full) => write!(s, "{}", full).ok(),
                            ShaderIoPrefix::FullOwned(full) => write!(s, "{}", full).ok(),
                        };
                        self.stack.push(self.trap.pass(), ShaderType::Pod(pod_ty), s);
                        self.stack.free_string(field_s);
                        self.stack.free_string(instance_s);
                        return;
                    }
                }
                
                // Field not found in type-check structure either
                script_err_not_found!(self.trap, "field {:?} not found on ScopeObject{}", field_id, suggest_property(&vm.heap, obj, field_id.into()));
                self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                self.stack.free_string(field_s);
                self.stack.free_string(instance_s);
                return;
            } else if let ShaderType::ScopeUniformBuffer { obj, pod_ty } = instance_ty {
                // Field access on a scope-level uniform buffer (e.g., test_buf.p3)
                // Look up the field on the pod type
                if let Some(ret_ty) = vm.heap.pod_field_type(pod_ty, field_id, &vm.code.builtins.pod) {
                    // Check if we already have this scope uniform buffer registered
                    let existing = output.scope_uniform_buffers.iter().find(|sub| sub.obj == obj);
                    
                    let shader_name = if let Some(existing) = existing {
                        existing.shader_name
                    } else {
                        // Generate unique names for this scope uniform buffer:
                        // - shader_name: identifier used in shader code (us_{id})
                        // - struct_type_name: the struct type name (IoScopeUniformBuf{id})
                        let (shader_name, struct_type_name) = self.generate_scope_uniform_buffer_names(output, obj);
                        
                        // Add to scope_uniform_buffers for runtime tracking
                        output.scope_uniform_buffers.push(ScopeUniformBufferSource {
                            obj,
                            pod_ty,
                            shader_name,
                        });
                        
                        // Add to IO list as UniformBuffer
                        // Set the struct type name (not the shader identifier name)
                        vm.heap.pod_type_name_if_not_set(pod_ty, struct_type_name);
                        output.io.push(ShaderIo {
                            kind: ShaderIoKind::UniformBuffer,
                            name: shader_name,
                            ty: pod_ty,
                            buffer_index: None,
                        });
                        
                        shader_name
                    };
                    
                    // Generate field access code
                    // Note: Don't use the backend prefix since our name already has `us_` prefix
                    let mut s = self.stack.new_string();
                    // For Metal, uniform buffers are pointers, use ->
                    if matches!(output.backend, ShaderBackend::Metal) {
                        write!(s, "{}->{}", shader_name, field_id).ok();
                    } else {
                        write!(s, "{}.{}", shader_name, field_id).ok();
                    }
                    self.stack.push(self.trap.pass(), ShaderType::Pod(ret_ty), s);
                } else {
                    script_err_not_found!(self.trap, "field {:?} not found on ScopeUniformBuffer{}", field_id, suggest_pod_field(&vm.heap, pod_ty, field_id));
                    self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                }
                self.stack.free_string(field_s);
                self.stack.free_string(instance_s);
                return;
            } else if let ShaderType::IoSelf(obj) = instance_ty {
                // Look up field value, preferring the highest shader IO marker in the prototype chain
                let (value, maybe_io_type) = Self::get_io_self_field_value(vm, obj, field_id, self.trap.pass());

                if let Some(io_type) = maybe_io_type {
                    // Found a shader IO marker (uniform, varying, texture, etc.)
                    let value_obj = value.as_object().unwrap();
                    let proto = vm.heap.proto(value_obj);
                    let ty = Self::type_from_value(vm, proto);
                    let concrete_ty = match ty {
                        ShaderType::Pod(pt) => Some(pt),
                        ShaderType::PodType(pt) => Some(pt),
                        _ => None,
                    };

                    let (kind, prefix) = output.backend.get_shader_io_kind_and_prefix(output.mode, io_type);

                    // Handle texture types specially - they don't have a concrete pod type
                    if let ShaderIoKind::Texture(tex_type) = &kind {
                        if !output.io.iter().any(|io| io.name == field_id) {
                            output.io.push(ShaderIo {
                                kind: kind.clone(),
                                name: field_id,
                                ty: ScriptPodType::VOID, // Textures don't have a pod type
                                buffer_index: None,
                            });
                        }
                        let mut s = self.stack.new_string();
                        match prefix {
                            ShaderIoPrefix::Prefix(prefix) => write!(s, "{}{}", prefix, field_id).ok(),
                            ShaderIoPrefix::Full(full) => write!(s, "{}", full).ok(),
                            ShaderIoPrefix::FullOwned(full) => write!(s, "{}", full).ok(),
                        };
                        self.stack.push(self.trap.pass(), ShaderType::Texture(*tex_type), s);
                        self.stack.free_string(field_s);
                        self.stack.free_string(instance_s);
                        return;
                    }

                    if let Some(pod_ty) = concrete_ty {
                        // lets see if our podtype has a name. ifnot use pod_ty
                        vm.heap.pod_type_name_if_not_set(pod_ty, field_id);
                        if !output.io.iter().any(|io| io.name == field_id) {
                            output.io.push(ShaderIo {
                                kind: kind.clone(),
                                name: field_id,
                                ty: pod_ty,
                                buffer_index: None,
                            });
                        }
                        let mut s = self.stack.new_string();
                        match prefix {
                            ShaderIoPrefix::Prefix(prefix) => write!(s, "{}{}", prefix, field_id).ok(),
                            ShaderIoPrefix::Full(full) => write!(s, "{}", full).ok(),
                            ShaderIoPrefix::FullOwned(full) => write!(s, "{}", full).ok(),
                        };
                        // UniformBuffer in Metal is a pointer, use PodPtr for correct -> access
                        let shader_ty = if matches!(kind, ShaderIoKind::UniformBuffer) && matches!(output.backend, ShaderBackend::Metal) {
                            ShaderType::PodPtr(pod_ty)
                        } else {
                            ShaderType::Pod(pod_ty)
                        };
                        self.stack.push(self.trap.pass(), shader_ty, s);
                        self.stack.free_string(field_s);
                        self.stack.free_string(instance_s);
                        return;
                    }
                }

                // No shader IO marker found - clear any trap error from value lookup
                // before checking RustInstance fields (which don't depend on prototype values)
                self.trap.err.take();
                
                // Check if this is a RustInstance field
                // RustInstance fields are pre-collected into output.io, so just look it up there
                if let Some(io) = output.io.iter().find(|io| io.name == field_id && matches!(io.kind, ShaderIoKind::RustInstance)) {
                    let pod_ty = io.ty;
                    let (_, prefix) = output.backend.get_shader_io_kind_and_prefix(output.mode, SHADER_IO_RUST_INSTANCE);
                    let mut s = self.stack.new_string();
                    match prefix {
                        ShaderIoPrefix::Prefix(prefix) => write!(s, "{}{}", prefix, field_id).ok(),
                        ShaderIoPrefix::Full(full) => write!(s, "{}", full).ok(),
                        ShaderIoPrefix::FullOwned(full) => write!(s, "{}", full).ok(),
                    };
                    self.stack.push(self.trap.pass(), ShaderType::Pod(pod_ty), s);
                    self.stack.free_string(field_s);
                    self.stack.free_string(instance_s);
                    return;
                }
                
                // Not a RustInstance field - check if the actual value has a pod type
                // (This path handles dynamically defined script fields)
                let actual_value = vm.heap.value(obj, field_id.into(), self.trap.pass());
                let ty = Self::type_from_value(vm, actual_value);
                let concrete_ty = match ty {
                    ShaderType::Pod(pt) => Some(pt),
                    ShaderType::PodType(pt) => Some(pt),
                    _ => None,
                };

                if let Some(pod_ty) = concrete_ty {
                    // This is a script-defined pod value
                    let (kind, prefix) = output.backend.get_shader_io_kind_and_prefix(output.mode, SHADER_IO_RUST_INSTANCE);
                    vm.heap.pod_type_name_if_not_set(pod_ty, field_id);
                    if !output.io.iter().any(|io| io.name == field_id) {
                        output.io.push(ShaderIo {
                            kind,
                            name: field_id,
                            ty: pod_ty,
                            buffer_index: None,
                        });
                    }
                    let mut s = self.stack.new_string();
                    match prefix {
                        ShaderIoPrefix::Prefix(prefix) => write!(s, "{}{}", prefix, field_id).ok(),
                        ShaderIoPrefix::Full(full) => write!(s, "{}", full).ok(),
                        ShaderIoPrefix::FullOwned(full) => write!(s, "{}", full).ok(),
                    };
                    self.stack.push(self.trap.pass(), ShaderType::Pod(pod_ty), s);
                    self.stack.free_string(field_s);
                    self.stack.free_string(instance_s);
                    return;
                }
                
                // Fallback: Look up field type from type-check structure
                self.trap.err.take(); // Clear any error
                if let Some(field_type_id) = vm.heap.field_type_from_type_check(obj, field_id) {
                    if let Some(pod_ty) = vm.heap.type_id_to_pod_type(field_type_id, &vm.code.builtins.pod) {
                        let (kind, prefix) = output.backend.get_shader_io_kind_and_prefix(output.mode, SHADER_IO_RUST_INSTANCE);
                        vm.heap.pod_type_name_if_not_set(pod_ty, field_id);
                        if !output.io.iter().any(|io| io.name == field_id) {
                            output.io.push(ShaderIo {
                                kind,
                                name: field_id,
                                ty: pod_ty,
                                buffer_index: None,
                            });
                        }
                        let mut s = self.stack.new_string();
                        match prefix {
                            ShaderIoPrefix::Prefix(prefix) => write!(s, "{}{}", prefix, field_id).ok(),
                            ShaderIoPrefix::Full(full) => write!(s, "{}", full).ok(),
                            ShaderIoPrefix::FullOwned(full) => write!(s, "{}", full).ok(),
                        };
                        self.stack.push(self.trap.pass(), ShaderType::Pod(pod_ty), s);
                        self.stack.free_string(field_s);
                        self.stack.free_string(instance_s);
                        return;
                    }
                }
            }
        }
        script_err_not_found!(self.trap, "field not found on shader type");
        self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
        self.stack.free_string(field_s);
        self.stack.free_string(instance_s);
    }

    pub(crate) fn handle_let_dyn(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        if opargs.is_nil() {
            script_err_have_to_initialise_variable!(self.trap, "shader let requires initializer");
            self.stack.pop(self.trap.pass());
        } else {
            let (ty_value, value) = self.pop_resolved(vm, output);
            let (ty_id, _id) = self.stack.pop(self.trap.pass());
            if let ShaderType::Id(id) = ty_id {
                // lets define our let type
                if let Some(ty) = ty_value.make_concrete(&vm.code.builtins.pod) {
                    let shadow = self.shader_scope.define_let(id, ty);
                    match output.backend {
                        ShaderBackend::Wgsl => {
                            if shadow > 0 {
                                write!(self.out, "let _s{}{} = {};\n", shadow, id, value).ok();
                            } else {
                                write!(self.out, "let {} = {};\n", id, value).ok();
                            }
                        }
                        ShaderBackend::Metal | ShaderBackend::Hlsl | ShaderBackend::Glsl => {
                            let type_name = if let Some(name) = vm.heap.pod_type_name(ty) {
                                output.backend.map_pod_name(name)
                            } else {
                                id!(unknown)
                            };
                            if shadow > 0 {
                                write!(self.out, "{} _s{}{} = {};\n", type_name, shadow, id, value).ok();
                            } else {
                                write!(self.out, "{} {} = {};\n", type_name, id, value).ok();
                            }
                        }
                    }
                } else {
                    script_err_no_matching_shader_type!(self.trap, "cannot determine shader type for let");
                }
            } else {
                script_err_unexpected!(self.trap, "let requires Id");
            }
        }
    }

    pub(crate) fn handle_var_dyn(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        if opargs.is_nil() {
            script_err_have_to_initialise_variable!(self.trap, "shader var requires initializer");
            self.stack.pop(self.trap.pass());
        } else {
            let (ty_value, value) = self.pop_resolved(vm, output);
            let (ty_id, _id) = self.stack.pop(self.trap.pass());
            if let ShaderType::Id(id) = ty_id {
                // lets define our let type
                if let Some(ty) = ty_value.make_concrete(&vm.code.builtins.pod) {
                    let shadow = self.shader_scope.define_var(id, ty);
                    match output.backend {
                        ShaderBackend::Wgsl => {
                            if shadow > 0 {
                                write!(self.out, "var _s{}{} = {};\n", shadow, id, value).ok();
                            } else {
                                write!(self.out, "var {} = {};\n", id, value).ok();
                            }
                        }
                        ShaderBackend::Metal | ShaderBackend::Hlsl | ShaderBackend::Glsl => {
                            let type_name = if let Some(name) = vm.heap.pod_type_name(ty) {
                                output.backend.map_pod_name(name)
                            } else {
                                id!(unknown)
                            };
                            if shadow > 0 {
                                write!(self.out, "{} _s{}{} = {};\n", type_name, shadow, id, value).ok();
                            } else {
                                write!(self.out, "{} {} = {};\n", type_name, id, value).ok();
                            }
                        }
                    }
                } else {
                    script_err_no_matching_shader_type!(self.trap, "cannot determine shader type for var");
                }
            } else {
                script_err_unexpected!(self.trap, "var requires Id");
            }
        }
    }
}
