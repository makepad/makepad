use crate::function::*;
use crate::makepad_error_log::*;
use crate::mod_pod::*;
use crate::mod_shader::*;
use crate::opcode::*;
use crate::pod::ScriptPodTy;
use crate::shader_backend::*;
use crate::suggest::*;
use crate::trap::*;
use crate::value::*;
use crate::vm::*;
use crate::*;
use makepad_live_id::*;
use makepad_math::*;
use std::fmt::Write;

// Re-export types from shader_output
pub use crate::shader_output::*;

/// Writes a float value for shader output, using scientific notation when needed.
/// This prevents very large numbers like 1e20 from being output as 100000000000000000000.0
/// which would break shader parsers like Metal.
fn write_shader_float(out: &mut String, v: f64) {
    let abs_v = v.abs();
    // Use scientific notation for very large or very small numbers
    if abs_v != 0.0 && (abs_v >= 1e15 || abs_v < 1e-6) {
        write!(out, "{:e}", v).ok();
    } else {
        write!(out, "{}", v).ok();
    }
}

#[derive(Debug)]
pub struct ShaderPodArg {
    pub name: Option<LiveId>,
    pub ty: ShaderType,
    pub s: String,
}

#[derive(Debug)]
pub enum ShaderMe {
    FnBody {
        ret: Option<ScriptPodType>,
        escaped: bool, // true when all code paths have returned
    },
    LoopBody,
    ForLoop {
        var_id: LiveId,
    },
    IfBody {
        target_ip: u32,
        start_pos: usize,
        stack_depth: usize,
        phi: Option<String>,
        phi_type: Option<ShaderType>,
        has_return: bool,            // true if current branch has a return
        if_branch_returned: bool,    // remembers if the if-branch returned (used when in else)
        phi_assigned_by_inner: bool, // true if an inner if assigned to this phi (for match/else-if chains)
        created_unreachable: bool, // true if this IfBody was created while in unreachable code (no code emitted)
    },
    /// Logic operation (&&, ||) - for short-circuit bytecode handling in shaders
    /// Shaders evaluate both operands and combine with the operator
    LogicOp {
        target_ip: u32,
        op: &'static str,       // "&&" or "||"
        first_operand: String,  // The first operand expression
        first_type: ShaderType, // The type of the first operand
    },
    BuiltinCall {
        name: LiveId,
        fnptr: NativeId,
        args: Vec<(ShaderType, String)>,
    },
    /// Pod builtin method call: x.mix(y, a) -> mix(x, y, a)
    PodBuiltinMethod {
        name: LiveId,
        self_ty: ScriptPodType,
        args: Vec<(ShaderType, String)>,
    },
    ScriptCall {
        out: String,
        name: LiveId,
        fnobj: ScriptObject,
        sself: ShaderType,
        args: Vec<ShaderType>,
    },
    Pod {
        pod_ty: ScriptPodType,
        args: Vec<ShaderPodArg>,
    },
    ArrayConstruct {
        args: Vec<String>,
        elem_ty: Option<ScriptPodType>,
    },
    TextureBuiltin {
        method_id: LiveId,
        tex_type: TextureType,
        texture_expr: String,
        args: Vec<String>,
    },
}

#[derive(Debug, PartialEq, Clone)]
pub enum ShaderType {
    None,
    IoSelf(ScriptObject),
    PodType(ScriptPodType),
    Pod(ScriptPodType),
    PodPtr(ScriptPodType), // Pointer to pod type (used for uniform buffers in Metal)
    Texture(TextureType),  // Texture type for method calls like .size()
    Id(LiveId),
    AbstractInt,
    AbstractFloat,
    Range {
        start: String,
        end: String,
        ty: ScriptPodType,
    },
    Error(ScriptValue),
    /// A script scope object that we're accessing properties from.
    /// Properties are flattened into ScopeUniforms.
    ScopeObject(ScriptObject),
    /// A uniform buffer defined in the script scope (e.g., `let buf = shader.uniform_buffer(...)`)
    /// Contains the uniform buffer object and its pod type.
    ScopeUniformBuffer {
        obj: ScriptObject,
        pod_ty: ScriptPodType,
    },
    /// A texture defined in the script scope (e.g., `let tex = shader.texture_2d(float)`)
    /// Contains the texture object and its type.
    ScopeTexture {
        obj: ScriptObject,
        tex_type: TextureType,
        shader_name: LiveId,
    },
}

impl ShaderType {
    pub fn make_concrete(&self, builtins: &ScriptPodBuiltins) -> Option<ScriptPodType> {
        match self {
            Self::Pod(ty) => Some(*ty),
            Self::PodPtr(ty) => Some(*ty),
            Self::Texture(_) => None, // Textures don't have a concrete pod type
            Self::ScopeTexture { .. } => None, // Scope textures don't have a concrete pod type
            Self::Id(_id) => None,
            Self::None => None,
            Self::IoSelf(_) => None,
            Self::ScopeObject(_) => None, // Scope objects don't have a concrete pod type
            Self::ScopeUniformBuffer { pod_ty, .. } => Some(*pod_ty),
            Self::PodType(_) => None,
            Self::AbstractInt => Some(builtins.pod_i32),
            Self::AbstractFloat => Some(builtins.pod_f32),
            Self::Range { ty, .. } => Some(*ty),
            Self::Error(_e) => None,
        }
    }
}

#[derive(Debug)]
pub enum ShaderScopeItem {
    IoSelf(ScriptObject),
    ScopeObject(ScriptObject),
    Let { ty: ScriptPodType, shadow: usize },
    Var { ty: ScriptPodType, shadow: usize },
    PodType { ty: ScriptPodType, shadow: usize },
}

impl ShaderScopeItem {
    pub fn ty(&self) -> ScriptPodType {
        match self {
            Self::IoSelf(_) => ScriptPodType::VOID,
            Self::ScopeObject(_) => ScriptPodType::VOID,
            Self::Let { ty, .. } => *ty,
            Self::Var { ty, .. } => *ty,
            Self::PodType { ty, .. } => *ty,
        }
    }

    pub fn shadow(&self) -> usize {
        match self {
            Self::IoSelf(_) => 0,
            Self::ScopeObject(_) => 0,
            Self::Let { shadow, .. } => *shadow,
            Self::Var { shadow, .. } => *shadow,
            Self::PodType { shadow, .. } => *shadow,
        }
    }
}

#[derive(Default)]
pub struct ShaderScope {
    pub shader_scope: Vec<LiveIdMap<LiveId, ShaderScopeItem>>,
}

#[derive(Default)]
pub struct ShaderFnCompiler {
    pub out: String,
    pub stack: ShaderStack,
    pub script_scope: ScriptObject,
    pub shader_scope: ShaderScope,
    pub mes: Vec<ShaderMe>,
    pub trap: ScriptTrapInner,
    pub debug: bool,
    /// Skip the next POP_TO_ME opcode - used when closing an if that had a return
    pub skip_next_pop_to_me: bool,
}

#[derive(Default)]
pub struct ShaderStack {
    pub(crate) stack_limit: usize,
    pub(crate) types: Vec<ShaderType>,
    pub(crate) strings: Vec<String>,
    pub(crate) free: Vec<String>,
}

macro_rules! push_fmt {
    ($self:ident, $ty:expr, $fmt_str:literal, $($args:expr),*) => {{
        let s = free_fmt!($self, $fmt_str, $($args),*);
        $self.stack.push($self.trap.pass(), $ty, s);
    }};
}

macro_rules! free_fmt {
    ($self:ident, $fmt_str:literal, $($args:expr),*) => {{
        let mut s = $self.stack.new_string();
        write!(s, $fmt_str, $($args),*).ok();
        s
    }};
}

impl ShaderScope {
    pub fn enter_scope(&mut self) {
        self.shader_scope.push(Default::default());
    }

    pub fn exit_scope(&mut self) {
        self.shader_scope.pop();
    }

    pub fn find_var(&self, id: LiveId) -> Option<(&ShaderScopeItem, usize)> {
        for scope in self.shader_scope.iter().rev() {
            if let Some(item) = scope.get(&id) {
                return Some((item, item.shadow()));
            }
        }
        None
    }

    /// Collect all variable names in scope for suggestions
    pub fn all_var_names(&self) -> Vec<LiveId> {
        let mut names = Vec::new();
        for scope in self.shader_scope.iter() {
            for key in scope.keys() {
                if !names.contains(key) {
                    names.push(*key);
                }
            }
        }
        names
    }

    pub fn define_io_self(&mut self, sself: ScriptObject) {
        let scope = self.shader_scope.last_mut().unwrap();
        scope.insert(id!(self), ShaderScopeItem::IoSelf(sself));
    }

    pub fn define_scope_object(&mut self, sself: ScriptObject) {
        let scope = self.shader_scope.last_mut().unwrap();
        scope.insert(id!(self), ShaderScopeItem::ScopeObject(sself));
    }

    pub fn define_var(&mut self, id: LiveId, ty: ScriptPodType) -> usize {
        let scope = self.shader_scope.last_mut().unwrap();
        if let Some(item) = scope.get_mut(&id) {
            let shadow = item.shadow() + 1;
            *item = ShaderScopeItem::Var { ty, shadow };
            shadow
        } else {
            scope.insert(id, ShaderScopeItem::Var { ty, shadow: 0 });
            0
        }
    }

    pub fn define_let(&mut self, id: LiveId, ty: ScriptPodType) -> usize {
        let scope = self.shader_scope.last_mut().unwrap();
        if let Some(item) = scope.get_mut(&id) {
            let shadow = item.shadow() + 1;
            *item = ShaderScopeItem::Let { ty, shadow };
            shadow
        } else {
            scope.insert(id, ShaderScopeItem::Let { ty, shadow: 0 });
            0
        }
    }

    pub fn define_pod_type(&mut self, id: LiveId, ty: ScriptPodType) {
        let scope = self.shader_scope.last_mut().unwrap();
        if let Some(item) = scope.get_mut(&id) {
            let shadow = item.shadow() + 1;
            *item = ShaderScopeItem::PodType { ty, shadow };
        } else {
            scope.insert(id, ShaderScopeItem::PodType { ty, shadow: 0 });
        }
    }
}

impl ShaderStack {
    pub fn pop(&mut self, trap: ScriptTrap) -> (ShaderType, String) {
        if let Some(s) = self.types.pop() {
            return (s, self.strings.pop().unwrap());
        } else {
            script_err_stack!(trap, "shader stack underflow");
            (ShaderType::Error(NIL), String::new())
        }
    }

    pub fn peek(&self, trap: ScriptTrap) -> (&ShaderType, &String) {
        if let Some(ty) = self.types.last() {
            return (ty, self.strings.last().unwrap());
        } else {
            script_err_stack!(trap, "shader stack underflow");
            static EMPTY: (ShaderType, String) = (ShaderType::None, String::new());
            (&EMPTY.0, &EMPTY.1)
        }
    }

    pub fn push(&mut self, trap: ScriptTrap, ty: ShaderType, s: String) {
        if self.types.len() > self.stack_limit {
            script_err_stack!(trap, "shader stack overflow");
        } else {
            self.types.push(ty);
            self.strings.push(s);
        }
    }

    pub fn new_string(&mut self) -> String {
        if let Some(s) = self.free.pop() {
            s
        } else {
            String::new()
        }
    }

    pub fn free_string(&mut self, s: String) {
        let mut s = s;
        s.clear();
        self.free.push(s);
    }
}

impl ShaderFnCompiler {
    pub fn new(script_scope: ScriptObject) -> Self {
        ShaderFnCompiler {
            script_scope,
            stack: ShaderStack {
                stack_limit: 1000000,
                ..Default::default()
            },
            mes: vec![],
            shader_scope: ShaderScope {
                shader_scope: vec![Default::default()],
            },
            ..Default::default()
        }
    }

    pub fn compile_fn(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        fnip: ScriptIp,
    ) -> ScriptPodType {
        //output.backend = ShaderBackend::Wgsl;
        output.backend.register_ids();

        self.mes.push(ShaderMe::FnBody {
            ret: None,
            escaped: false,
        });
        // alright lets go trace the opcodes
        self.trap.ip = fnip;

        // Calculate function end position from the FN_BODY_DYN opcode that precedes the function body
        // fnip.index points to the first opcode AFTER FN_BODY_DYN
        // FN_BODY_DYN's opargs contains the jump offset from its position to the end of the function
        let fn_end_index = {
            let bodies = vm.bx.code.bodies.borrow();
            let body = &bodies[self.trap.ip.body as usize];
            let fn_body_opcode = body.parser.opcodes[(fnip.index - 1) as usize];
            if let Some((_opcode, args)) = fn_body_opcode.as_opcode() {
                (fnip.index - 1) + args.to_u32()
            } else {
                // Fallback to opcodes.len() if we can't find FN_BODY_DYN
                body.parser.opcodes.len() as u32
            }
        };

        while self.trap.ip.index < fn_end_index {
            // Re-borrow bodies at each iteration to allow mutable vm access in method calls
            let opcode = {
                let bodies = vm.bx.code.bodies.borrow();
                let body = &bodies[self.trap.ip.body as usize];
                body.parser.opcodes[self.trap.ip.index as usize]
            };

            // Skip processing when in unreachable code (after a return in current branch)
            // But still need to process control flow opcodes to maintain structure
            if self.is_unreachable() {
                if let Some((op, args)) = opcode.as_opcode() {
                    // Only process control flow opcodes when unreachable
                    match op {
                        Opcode::IF_TEST => self.handle_if_test_unreachable(args),
                        // IF_ELSE is special: it transitions to the else branch which IS reachable
                        // (if the parent scope is reachable). Check if parent scope is unreachable.
                        Opcode::IF_ELSE => {
                            if self.is_parent_scope_unreachable() {
                                self.handle_if_else_unreachable(args);
                            } else {
                                self.handle_if_else(vm, args);
                            }
                        }
                        _ => {}
                    }
                }
                self.trap.goto_next();
                self.handle_if_else_phi_unreachable();
            } else if let Some((opcode, args)) = opcode.as_opcode() {
                self.opcode(vm, output, opcode, args);
                self.trap.goto_next();
                self.handle_logic_phi(vm, output);
                self.handle_if_else_phi(vm, output);
            } else {
                // id or immediate value
                self.push_immediate(opcode, &vm.bx.code.builtins.pod, &output.backend);
                self.trap.goto_next();
                self.handle_logic_phi(vm, output);
                self.handle_if_else_phi(vm, output);
            }
            // alright lets see if we have a trap, ifso we can log it
            if let Some(err) = self.trap.err.borrow_mut().pop_front() {
                output.has_errors = true;
                if let Some(ptr) = err.value.as_err() {
                    if let Some(loc2) = vm.bx.code.ip_to_loc(ptr.ip) {
                        log_with_level(
                            &loc2.file,
                            loc2.line,
                            loc2.col,
                            loc2.line,
                            loc2.col,
                            format!("{} ({}:{})", err.message, err.origin_file, err.origin_line),
                            LogLevel::Error,
                        );
                    }
                }
            }
            // The trap handling for Return is no longer needed since we use fn_end_index
            // to determine when to stop. The trap may still be set by handle_return but
            // we ignore it and continue processing to properly close all control structures.
            self.trap.on.take();
        }
        let value = self.mes.pop();
        if let Some(ShaderMe::FnBody { ret, .. }) = value {
            return ret.unwrap_or(vm.bx.code.builtins.pod.pod_void);
        }
        panic!("Unexpected ME at end {:?}", value)
    }

    pub(crate) fn pop_resolved(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
    ) -> (ShaderType, String) {
        let (ty, s) = self.stack.pop(self.trap.pass());
        // if ty is an id, look it up
        match ty {
            ShaderType::Id(id) => {
                // First, look it up on our shader scope (local variables)
                if let Some((sc, shadow)) = self.shader_scope.find_var(id) {
                    let mut s2 = self.stack.new_string();
                    if let ShaderScopeItem::IoSelf(obj) = sc {
                        return (ShaderType::IoSelf(*obj), s2);
                    }
                    if let ShaderScopeItem::ScopeObject(obj) = sc {
                        // `self` is a ScopeObject - return it for field access handling
                        return (ShaderType::ScopeObject(*obj), s2);
                    }
                    if shadow > 0 {
                        write!(s2, "_s{}{}", shadow, id).ok();
                    } else if id == id!(self) {
                        write!(s2, "_self").ok();
                    } else {
                        write!(s2, "{}", id).ok();
                    }
                    self.stack.free_string(s);
                    return (ShaderType::Pod(sc.ty()), s2);
                }

                // Not found in shader scope - try script scope for scope uniforms
                let value = vm
                    .bx
                    .heap
                    .scope_value(self.script_scope, id.into(), self.trap.pass());
                if !value.is_nil() && self.trap.err.borrow().is_empty() {
                    // Check if this is a shader_io type
                    if let Some(value_obj) = value.as_object() {
                        if let Some(io_type) = vm.bx.heap.as_shader_io(value_obj) {
                            // Uniform buffers from scope are supported
                            if io_type == SHADER_IO_UNIFORM_BUFFER {
                                // Get the pod type from the prototype
                                let proto_value = vm.bx.heap.proto(value_obj);
                                if let Some(pod_ty) = vm.bx.heap.pod_type(proto_value) {
                                    self.stack.free_string(s);
                                    return (
                                        ShaderType::ScopeUniformBuffer {
                                            obj: value_obj,
                                            pod_ty,
                                        },
                                        self.stack.new_string(),
                                    );
                                } else if let Some(pod_ty) = proto_value.as_pod_type() {
                                    self.stack.free_string(s);
                                    return (
                                        ShaderType::ScopeUniformBuffer {
                                            obj: value_obj,
                                            pod_ty,
                                        },
                                        self.stack.new_string(),
                                    );
                                }
                            }
                            // Textures from scope are supported
                            let tex_type = match io_type {
                                SHADER_IO_TEXTURE_1D => Some(TextureType::Texture1d),
                                SHADER_IO_TEXTURE_1D_ARRAY => Some(TextureType::Texture1dArray),
                                SHADER_IO_TEXTURE_2D => Some(TextureType::Texture2d),
                                SHADER_IO_TEXTURE_2D_ARRAY => Some(TextureType::Texture2dArray),
                                SHADER_IO_TEXTURE_3D => Some(TextureType::Texture3d),
                                SHADER_IO_TEXTURE_3D_ARRAY => Some(TextureType::Texture3dArray),
                                SHADER_IO_TEXTURE_CUBE => Some(TextureType::TextureCube),
                                SHADER_IO_TEXTURE_CUBE_ARRAY => Some(TextureType::TextureCubeArray),
                                SHADER_IO_TEXTURE_DEPTH => Some(TextureType::TextureDepth),
                                SHADER_IO_TEXTURE_DEPTH_ARRAY => {
                                    Some(TextureType::TextureDepthArray)
                                }
                                _ => None,
                            };
                            if let Some(tex_type) = tex_type {
                                // Check if we already have this scope texture registered
                                let existing =
                                    output.scope_textures.iter().find(|st| st.obj == value_obj);

                                let shader_name = if let Some(existing) = existing {
                                    existing.shader_name
                                } else {
                                    // Generate unique name for this scope texture
                                    let shader_name =
                                        self.generate_scope_texture_name(output, id, value_obj);

                                    // Add to scope_textures for runtime tracking
                                    output.scope_textures.push(ScopeTextureSource {
                                        obj: value_obj,
                                        tex_type,
                                        shader_name,
                                    });

                                    // Add to IO list as Texture
                                    if !output.io.iter().any(|io| {
                                        io.name == shader_name
                                            && matches!(io.kind, ShaderIoKind::Texture(_))
                                    }) {
                                        output.io.push(ShaderIo {
                                            kind: ShaderIoKind::Texture(tex_type),
                                            name: shader_name,
                                            ty: ScriptPodType::VOID, // Textures don't have a pod type
                                            buffer_index: None,
                                        });
                                    }

                                    shader_name
                                };

                                // Generate the texture expression with proper prefix
                                let mut s2 = self.stack.new_string();
                                let (_, prefix) = output
                                    .backend
                                    .get_shader_io_kind_and_prefix(output.mode, io_type);
                                match prefix {
                                    ShaderIoPrefix::Prefix(prefix) => {
                                        write!(s2, "{}{}", prefix, shader_name).ok()
                                    }
                                    ShaderIoPrefix::Full(full) => write!(s2, "{}", full).ok(),
                                    ShaderIoPrefix::FullOwned(full) => write!(s2, "{}", full).ok(),
                                };

                                self.stack.free_string(s);
                                return (
                                    ShaderType::ScopeTexture {
                                        obj: value_obj,
                                        tex_type,
                                        shader_name,
                                    },
                                    s2,
                                );
                            }
                            // Other shader_io types are not supported in scope
                            script_err_shader!(self.trap, "opcode not supported");
                            self.stack.free_string(s);
                            return (ShaderType::Error(NIL), self.stack.new_string());
                        }
                        // Check if this is an object we can walk for properties
                        // Return ScopeObject so handle_field can process property access
                        self.stack.free_string(s);
                        return (ShaderType::ScopeObject(value_obj), self.stack.new_string());
                    }

                    // It's a direct value - add as scope uniform
                    if let Some(pod_ty) = self.get_scope_value_pod_type(vm, value) {
                        // Check if we already have this scope uniform
                        let existing = output
                            .scope_uniforms
                            .iter()
                            .find(|su| su.source_obj == self.script_scope && su.key == id);

                        let shader_name = if let Some(existing) = existing {
                            existing.shader_name
                        } else {
                            // Generate unique name if there's a collision (use script_scope as source obj)
                            let shader_name =
                                self.generate_scope_uniform_name(output, id, self.script_scope);
                            output.scope_uniforms.push(ScopeUniformSource {
                                source_obj: self.script_scope,
                                key: id,
                                shader_name,
                                ty: pod_ty,
                            });
                            // Also add to IO list
                            if !output.io.iter().any(|io| {
                                io.name == shader_name
                                    && matches!(io.kind, ShaderIoKind::ScopeUniform)
                            }) {
                                vm.bx.heap.pod_type_name_if_not_set(pod_ty, shader_name);
                                output.io.push(ShaderIo {
                                    kind: ShaderIoKind::ScopeUniform,
                                    name: shader_name,
                                    ty: pod_ty,
                                    buffer_index: None,
                                });
                            }
                            shader_name
                        };

                        let mut s2 = self.stack.new_string();
                        let (_, prefix) = output
                            .backend
                            .get_shader_io_kind_and_prefix(output.mode, SHADER_IO_SCOPE_UNIFORM);
                        match prefix {
                            ShaderIoPrefix::Prefix(prefix) => {
                                write!(s2, "{}{}", prefix, shader_name).ok()
                            }
                            ShaderIoPrefix::Full(full) => write!(s2, "{}", full).ok(),
                            ShaderIoPrefix::FullOwned(full) => write!(s2, "{}", full).ok(),
                        };
                        self.stack.free_string(s);
                        return (ShaderType::Pod(pod_ty), s2);
                    }
                }

                // Clear any error from scope_value lookup failure
                self.trap.err.take();
                script_err_not_found!(
                    self.trap,
                    "shader variable {:?} not found{}",
                    id,
                    suggest_from_live_ids(id, &self.shader_scope.all_var_names())
                );
                self.stack.free_string(s);
                return (ShaderType::Error(NIL), self.stack.new_string());
            }
            _ => (ty, s),
        }
    }

    /// Get the pod type from a scope value, if it's a supported type
    pub(crate) fn get_scope_value_pod_type(
        &self,
        vm: &ScriptVm,
        value: ScriptValue,
    ) -> Option<ScriptPodType> {
        // Check if it's a primitive type (f32, f64, bool, etc.)
        if let Some(pod_ty) = vm.bx.code.builtins.pod.value_to_exact_type(value) {
            return Some(pod_ty);
        }
        // Check if it's a color - colors map to vec4f
        if value.is_color() {
            return Some(vm.bx.code.builtins.pod.pod_vec4f);
        }
        // Check if it's a pod instance
        if let Some(pod) = value.as_pod() {
            let pod = &vm.bx.heap.pods[pod];
            return Some(pod.ty);
        }
        None
    }

    /// Generate a unique name for a scope uniform, handling collisions.
    /// Uses the source object's index to create unique names when there are collisions.
    pub(crate) fn generate_scope_uniform_name(
        &self,
        output: &ShaderOutput,
        base_name: LiveId,
        source_obj: ScriptObject,
    ) -> LiveId {
        // First, ensure base_name is actually in the LUT. If not, use a default.
        let base_name_str = base_name.as_string(|s| s.map(|s| s.to_string()));
        let base_name_str = base_name_str.unwrap_or_else(|| "scope_uni".to_string());

        // Re-register the base name to ensure it's in the LUT
        let base_name =
            LiveId::from_str_with_lut(&base_name_str).unwrap_or_else(|_| id!(scope_uni));

        // Check if name is already used
        let name_used = output.io.iter().any(|io| io.name == base_name)
            || output
                .scope_uniforms
                .iter()
                .any(|su| su.shader_name == base_name);

        if !name_used {
            return base_name;
        }

        // Name collision - use the object index to create a unique name
        // Format: base_name_objN where N is the object index
        // Use from_str_with_lut to register the name in the LiveId lookup table
        let unique_name_str = format!("{}_obj{}", base_name_str, source_obj.index);
        let unique_name = LiveId::from_str_with_lut(&unique_name_str)
            .unwrap_or_else(|_| LiveId::from_str(&unique_name_str));

        // Check if this unique name is also used (very unlikely but possible)
        let unique_name_used = output.io.iter().any(|io| io.name == unique_name)
            || output
                .scope_uniforms
                .iter()
                .any(|su| su.shader_name == unique_name);

        if !unique_name_used {
            return unique_name;
        }

        // Fallback: add counter suffix
        for i in 1..100 {
            let new_name_str = format!("{}_obj{}_{}", base_name_str, source_obj.index, i);
            let new_name = LiveId::from_str_with_lut(&new_name_str)
                .unwrap_or_else(|_| LiveId::from_str(&new_name_str));
            let new_name_used = output.io.iter().any(|io| io.name == new_name)
                || output
                    .scope_uniforms
                    .iter()
                    .any(|su| su.shader_name == new_name);
            if !new_name_used {
                return new_name;
            }
        }

        // Final fallback - should never reach here
        unique_name
    }

    /// Generate names for a scope uniform buffer.
    /// These are uniform buffers defined in the script scope, e.g., `let buf = shader.uniform_buffer(...)`
    /// Returns (shader_name, struct_type_name):
    /// - shader_name: identifier used in shader code, e.g., `scopebuf_{obj_index}`
    /// - struct_type_name: the struct type name, e.g., `IoScopeUniformBuf{obj_index}`
    pub(crate) fn generate_scope_uniform_buffer_names(
        &self,
        output: &ShaderOutput,
        obj: ScriptObject,
    ) -> (LiveId, LiveId) {
        // Generate the shader identifier name: scopebuf_{index}
        let shader_name_str = format!("scopebuf_{}", obj.index);
        let shader_name = LiveId::from_str_with_lut(&shader_name_str)
            .unwrap_or_else(|_| LiveId::from_str(&shader_name_str));

        // Generate the struct type name: IoScopeUniformBuf{index}
        let struct_name_str = format!("IoScopeUniformBuf{}", obj.index);
        let struct_name = LiveId::from_str_with_lut(&struct_name_str)
            .unwrap_or_else(|_| LiveId::from_str(&struct_name_str));

        // Check if shader name is already used (shouldn't happen since obj.index is unique, but just in case)
        let name_used = output.io.iter().any(|io| io.name == shader_name)
            || output
                .scope_uniform_buffers
                .iter()
                .any(|sub| sub.shader_name == shader_name);

        if !name_used {
            return (shader_name, struct_name);
        }

        // Name collision - add counter suffix
        for i in 0..100 {
            let new_shader_name_str = format!("scopebuf_{}_{}", obj.index, i);
            let new_shader_name = LiveId::from_str_with_lut(&new_shader_name_str)
                .unwrap_or_else(|_| LiveId::from_str(&new_shader_name_str));
            let new_struct_name_str = format!("IoScopeUniformBuf{}_{}", obj.index, i);
            let new_struct_name = LiveId::from_str_with_lut(&new_struct_name_str)
                .unwrap_or_else(|_| LiveId::from_str(&new_struct_name_str));

            let new_name_used = output.io.iter().any(|io| io.name == new_shader_name)
                || output
                    .scope_uniform_buffers
                    .iter()
                    .any(|sub| sub.shader_name == new_shader_name);
            if !new_name_used {
                return (new_shader_name, new_struct_name);
            }
        }

        // Final fallback - should never reach here
        (shader_name, struct_name)
    }

    /// Generate a unique name for a scope texture.
    /// These are textures defined in the script scope, e.g., `let tex = shader.texture_2d(float)`
    pub(crate) fn generate_scope_texture_name(
        &self,
        output: &ShaderOutput,
        base_name: LiveId,
        obj: ScriptObject,
    ) -> LiveId {
        // First, ensure base_name is actually in the LUT. If not, use a default.
        let base_name_str = base_name.as_string(|s| s.map(|s| s.to_string()));
        let base_name_str = base_name_str.unwrap_or_else(|| format!("scope_tex_{}", obj.index));

        // Re-register the base name to ensure it's in the LUT
        let base_name =
            LiveId::from_str_with_lut(&base_name_str).unwrap_or_else(|_| id!(scope_tex));

        // Check if name is already used
        let name_used = output
            .io
            .iter()
            .any(|io| io.name == base_name && matches!(io.kind, ShaderIoKind::Texture(_)))
            || output
                .scope_textures
                .iter()
                .any(|st| st.shader_name == base_name);

        if !name_used {
            return base_name;
        }

        // Name collision - use the object index to create a unique name
        let unique_name_str = format!("{}_obj{}", base_name_str, obj.index);
        let unique_name = LiveId::from_str_with_lut(&unique_name_str)
            .unwrap_or_else(|_| LiveId::from_str(&unique_name_str));

        // Check if this unique name is also used
        let unique_name_used = output
            .io
            .iter()
            .any(|io| io.name == unique_name && matches!(io.kind, ShaderIoKind::Texture(_)))
            || output
                .scope_textures
                .iter()
                .any(|st| st.shader_name == unique_name);

        if !unique_name_used {
            return unique_name;
        }

        // Fallback: add counter suffix
        for i in 1..100 {
            let new_name_str = format!("{}_obj{}_{}", base_name_str, obj.index, i);
            let new_name = LiveId::from_str_with_lut(&new_name_str)
                .unwrap_or_else(|_| LiveId::from_str(&new_name_str));
            let new_name_used = output
                .io
                .iter()
                .any(|io| io.name == new_name && matches!(io.kind, ShaderIoKind::Texture(_)))
                || output
                    .scope_textures
                    .iter()
                    .any(|st| st.shader_name == new_name);
            if !new_name_used {
                return new_name;
            }
        }

        // Final fallback
        unique_name
    }

    fn push_immediate(
        &mut self,
        value: ScriptValue,
        builtins: &ScriptPodBuiltins,
        backend: &ShaderBackend,
    ) {
        if let Some(v) = value.as_f64() {
            // abstract int or float
            let mut s = self.stack.new_string();
            write_shader_float(&mut s, v);
            return self
                .stack
                .push(self.trap.pass(), ShaderType::AbstractFloat, s);
        }
        if let Some(v) = value.as_u40() {
            return push_fmt!(self, ShaderType::AbstractInt, "{}", v);
        }
        if let Some(id) = value.as_id() {
            return push_fmt!(self, ShaderType::Id(id), "{}", id);
        }
        if let Some(v) = value.as_f32() {
            let mut s = self.stack.new_string();
            write_shader_float(&mut s, v as f64);
            s.push('f');
            return self
                .stack
                .push(self.trap.pass(), ShaderType::Pod(builtins.pod_f32), s);
        }
        if let Some(v) = value.as_f16() {
            let mut s = self.stack.new_string();
            write_shader_float(&mut s, v as f64);
            s.push('h');
            return self
                .stack
                .push(self.trap.pass(), ShaderType::Pod(builtins.pod_f16), s);
        }
        if let Some(v) = value.as_u32() {
            return push_fmt!(self, ShaderType::Pod(builtins.pod_u32), "{}u", v);
        }
        if let Some(v) = value.as_i32() {
            return push_fmt!(self, ShaderType::Pod(builtins.pod_i32), "{}i", v);
        }
        if let Some(v) = value.as_bool() {
            return push_fmt!(self, ShaderType::Pod(builtins.pod_bool), "{}", v);
        }
        if let Some(v) = value.as_color() {
            let v = Vec4f::from_u32(v);
            let name = backend.map_pod_name(id!(vec4f));
            let mut s = self.stack.new_string();
            write!(s, "{}(", name).ok();
            write_shader_float(&mut s, v.x as f64);
            s.push(',');
            write_shader_float(&mut s, v.y as f64);
            s.push(',');
            write_shader_float(&mut s, v.z as f64);
            s.push(',');
            write_shader_float(&mut s, v.w as f64);
            s.push(')');
            return self
                .stack
                .push(self.trap.pass(), ShaderType::Pod(builtins.pod_vec4f), s);
        }
        script_err_shader!(self.trap, "no matching shader type");
    }

    pub(crate) fn ensure_struct_name(
        &self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        pod_ty: ScriptPodType,
        used_name: LiveId,
    ) -> LiveId {
        // Always insert struct types into output.structs so they get defined in shader output
        if let ScriptPodTy::Struct { .. } = vm.bx.heap.pod_type_ref(pod_ty).ty {
            output.structs.insert(pod_ty);
        }

        if let Some(name) = vm.bx.heap.pod_type_name(pod_ty) {
            if name != used_name
                && used_name != id!(self)
                && used_name != id!(vec2)
                && used_name != id!(vec3)
                && used_name != id!(vec4)
            {
                script_err_inconsistent!(self.trap, "struct name not consistent");
            }
            return name;
        }
        vm.bx.heap.pod_type_name_set(pod_ty, used_name);
        used_name
    }

    fn opcode(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opcode: Opcode,
        opargs: OpcodeArgs,
    ) {
        match opcode {
            // Arithmetic
            Opcode::NOT => {}
            Opcode::NEG => self.handle_neg(vm, output, opargs, "-"),
            Opcode::MUL => self.handle_arithmetic(vm, output, opargs, "*", false),
            Opcode::DIV => self.handle_arithmetic(vm, output, opargs, "/", false),
            Opcode::MOD => self.handle_arithmetic(vm, output, opargs, "%", false),
            Opcode::ADD => self.handle_arithmetic(vm, output, opargs, "+", false),
            Opcode::SUB => self.handle_arithmetic(vm, output, opargs, "-", false),
            Opcode::SHL => self.handle_arithmetic(vm, output, opargs, ">>", true),
            Opcode::SHR => self.handle_arithmetic(vm, output, opargs, "<<", true),
            Opcode::AND => self.handle_arithmetic(vm, output, opargs, "&", true),
            Opcode::OR => self.handle_arithmetic(vm, output, opargs, "|", true),
            Opcode::XOR => self.handle_arithmetic(vm, output, opargs, "^", true),

            // ASSIGN
            Opcode::ASSIGN => self.handle_assign(vm),
            Opcode::ASSIGN_ADD => {
                self.handle_arithmetic_assign(vm, output, opargs, "+=", false);
            }
            Opcode::ASSIGN_SUB => {
                self.handle_arithmetic_assign(vm, output, opargs, "-=", false);
            }
            Opcode::ASSIGN_MUL => {
                self.handle_arithmetic_assign(vm, output, opargs, "*=", false);
            }
            Opcode::ASSIGN_DIV => {
                self.handle_arithmetic_assign(vm, output, opargs, "/=", false);
            }
            Opcode::ASSIGN_MOD => {
                self.handle_arithmetic_assign(vm, output, opargs, "%=", false);
            }
            Opcode::ASSIGN_AND => {
                self.handle_arithmetic_assign(vm, output, opargs, "&=", true);
            }
            Opcode::ASSIGN_OR => {
                self.handle_arithmetic_assign(vm, output, opargs, "|=", true);
            }
            Opcode::ASSIGN_XOR => {
                self.handle_arithmetic_assign(vm, output, opargs, "^=", true);
            }
            Opcode::ASSIGN_SHL => {
                self.handle_arithmetic_assign(vm, output, opargs, ">>=", true);
            }
            Opcode::ASSIGN_SHR => {
                self.handle_arithmetic_assign(vm, output, opargs, "<<=", true);
            }
            Opcode::ASSIGN_IFNIL => {
                script_err_not_impl!(self.trap, "ASSIGN_IFNIL: null-coalescing assignment `x ??= default` not supported in shaders");
            }
            // ASSIGN FIELD
            Opcode::ASSIGN_FIELD => self.handle_assign_field(vm, output),
            Opcode::ASSIGN_FIELD_ADD => {
                self.handle_arithmetic_field_assign(vm, output, opargs, "+=", false);
            }
            Opcode::ASSIGN_FIELD_SUB => {
                self.handle_arithmetic_field_assign(vm, output, opargs, "-=", false);
            }
            Opcode::ASSIGN_FIELD_MUL => {
                self.handle_arithmetic_field_assign(vm, output, opargs, "*=", false);
            }
            Opcode::ASSIGN_FIELD_DIV => {
                self.handle_arithmetic_field_assign(vm, output, opargs, "/=", false);
            }
            Opcode::ASSIGN_FIELD_MOD => {
                self.handle_arithmetic_field_assign(vm, output, opargs, "%=", false);
            }
            Opcode::ASSIGN_FIELD_AND => {
                self.handle_arithmetic_field_assign(vm, output, opargs, "&=", true);
            }
            Opcode::ASSIGN_FIELD_OR => {
                self.handle_arithmetic_field_assign(vm, output, opargs, "|=", true);
            }
            Opcode::ASSIGN_FIELD_XOR => {
                self.handle_arithmetic_field_assign(vm, output, opargs, "^=", true);
            }
            Opcode::ASSIGN_FIELD_SHL => {
                self.handle_arithmetic_field_assign(vm, output, opargs, ">>=", true);
            }
            Opcode::ASSIGN_FIELD_SHR => {
                self.handle_arithmetic_field_assign(vm, output, opargs, "<<=", true);
            }
            Opcode::ASSIGN_FIELD_IFNIL => {
                script_err_not_impl!(self.trap, "ASSIGN_FIELD_IFNIL: null-coalescing field assignment `obj.x ??= default` not supported in shaders");
            }

            Opcode::ASSIGN_INDEX => self.handle_assign_index(vm, output),
            Opcode::ASSIGN_INDEX_ADD => {
                self.handle_arithmetic_index_assign(vm, output, opargs, "+=", false);
            }
            Opcode::ASSIGN_INDEX_SUB => {
                self.handle_arithmetic_index_assign(vm, output, opargs, "-=", false);
            }
            Opcode::ASSIGN_INDEX_MUL => {
                self.handle_arithmetic_index_assign(vm, output, opargs, "*=", false);
            }
            Opcode::ASSIGN_INDEX_DIV => {
                self.handle_arithmetic_index_assign(vm, output, opargs, "/=", false);
            }
            Opcode::ASSIGN_INDEX_MOD => {
                self.handle_arithmetic_index_assign(vm, output, opargs, "%=", false);
            }
            Opcode::ASSIGN_INDEX_AND => {
                self.handle_arithmetic_index_assign(vm, output, opargs, "&=", true);
            }
            Opcode::ASSIGN_INDEX_OR => {
                self.handle_arithmetic_index_assign(vm, output, opargs, "|=", true);
            }
            Opcode::ASSIGN_INDEX_XOR => {
                self.handle_arithmetic_index_assign(vm, output, opargs, "^=", true);
            }
            Opcode::ASSIGN_INDEX_SHL => {
                self.handle_arithmetic_index_assign(vm, output, opargs, ">>=", true);
            }
            Opcode::ASSIGN_INDEX_SHR => {
                self.handle_arithmetic_index_assign(vm, output, opargs, "<<=", true);
            }
            Opcode::ASSIGN_INDEX_IFNIL => {
                script_err_not_impl!(self.trap, "ASSIGN_INDEX_IFNIL: null-coalescing index assignment `arr[i] ??= default` not supported in shaders");
            }
            // ASSIGN ME
            Opcode::ASSIGN_ME => self.handle_assign_me(vm),
            Opcode::ASSIGN_ME_VEC => {
                script_err_shader!(
                    self.trap,
                    "ASSIGN_ME_VEC: vec assignment `:=` not supported in shaders"
                );
            }

            Opcode::ASSIGN_ME_BEFORE | Opcode::ASSIGN_ME_AFTER => {
                script_err_shader!(
                    self.trap,
                    "ASSIGN_ME_BEFORE/AFTER: `++x` / `x++` not supported in shaders, use `x += 1`"
                );
            }

            Opcode::ASSIGN_ME_BEGIN => {
                script_err_shader!(
                    self.trap,
                    "ASSIGN_ME_BEGIN: compound assignment not supported in shaders"
                );
            }

            // CONCAT
            Opcode::CONCAT => {
                script_err_shader!(
                    self.trap,
                    "CONCAT: string concatenation `a + b` not supported in shaders"
                );
            }
            // EQUALITY
            Opcode::EQ => {
                self.handle_eq(vm, output, opargs, "==");
            }
            Opcode::NEQ => {
                self.handle_eq(vm, output, opargs, "!=");
            }

            Opcode::LT => {
                self.handle_eq(vm, output, opargs, "<");
            }
            Opcode::GT => {
                self.handle_eq(vm, output, opargs, ">");
            }
            Opcode::LEQ => {
                self.handle_eq(vm, output, opargs, "<=");
            }
            Opcode::GEQ => {
                self.handle_eq(vm, output, opargs, ">=");
            }

            Opcode::LOGIC_AND_TEST => {
                self.handle_logic_test(vm, output, opargs, "&&");
            }
            Opcode::LOGIC_OR_TEST => {
                self.handle_logic_test(vm, output, opargs, "||");
            }
            Opcode::NIL_OR_TEST => {
                script_err_shader!(
                    self.trap,
                    "NIL_OR_TEST: null-coalescing `a |? b` not supported in shaders"
                );
            }
            Opcode::SHALLOW_EQ => {
                script_err_shader!(
                    self.trap,
                    "SHALLOW_EQ: shallow equality `===` not supported in shaders"
                );
            }
            Opcode::SHALLOW_NEQ => {
                script_err_shader!(
                    self.trap,
                    "SHALLOW_NEQ: shallow inequality `!==` not supported in shaders"
                );
            }
            // Object/Array begin
            Opcode::BEGIN_PROTO => {
                script_err_shader!(
                    self.trap,
                    "BEGIN_PROTO: object literal `{{...}}` not supported in shaders"
                );
            }
            Opcode::PROTO_INHERIT_READ => {
                script_err_shader!(
                    self.trap,
                    "PROTO_INHERIT: prototype inheritance not supported in shaders"
                );
            }
            Opcode::PROTO_INHERIT_WRITE => {
                script_err_shader!(
                    self.trap,
                    "PROTO_INHERIT: prototype inheritance not supported in shaders"
                );
            }
            Opcode::SCOPE_INHERIT_READ => {
                script_err_shader!(
                    self.trap,
                    "SCOPE_INHERIT: scope inheritance not supported in shaders"
                );
            }
            Opcode::SCOPE_INHERIT_WRITE => {
                script_err_shader!(
                    self.trap,
                    "SCOPE_INHERIT: scope inheritance not supported in shaders"
                );
            }
            Opcode::FIELD_INHERIT_READ => {
                script_err_shader!(
                    self.trap,
                    "FIELD_INHERIT: field inheritance not supported in shaders"
                );
            }
            Opcode::FIELD_INHERIT_WRITE => {
                script_err_shader!(
                    self.trap,
                    "FIELD_INHERIT: field inheritance not supported in shaders"
                );
            }
            Opcode::INDEX_INHERIT_READ => {
                script_err_shader!(
                    self.trap,
                    "INDEX_INHERIT: index inheritance not supported in shaders"
                );
            }
            Opcode::INDEX_INHERIT_WRITE => {
                script_err_shader!(
                    self.trap,
                    "INDEX_INHERIT: index inheritance not supported in shaders"
                );
            }
            Opcode::END_PROTO => {
                script_err_shader!(
                    self.trap,
                    "END_PROTO: object literal `{{...}}` not supported in shaders"
                );
            }
            Opcode::BEGIN_BARE => {
                script_err_shader!(
                    self.trap,
                    "BEGIN_BARE: bare object `{{..}}` not supported in shaders"
                );
            }
            Opcode::END_BARE => {
                script_err_shader!(self.trap, "END_BARE: bare object not supported in shaders");
            }
            Opcode::BEGIN_ARRAY => {
                script_err_shader!(self.trap, "BEGIN_ARRAY: dynamic array `[...]` not supported in shaders, use fixed-size arrays");
            }
            Opcode::END_ARRAY => {
                script_err_shader!(
                    self.trap,
                    "END_ARRAY: dynamic array not supported in shaders"
                );
            }
            // Calling
            Opcode::CALL_ARGS => {
                self.handle_call_args(vm, output, opargs);
            }
            Opcode::CALL_EXEC | Opcode::METHOD_CALL_EXEC => {
                self.handle_call_exec(vm, output);
            }
            Opcode::METHOD_CALL_ARGS => {
                self.handle_method_call_args(vm, output, opargs);
            }
            // Fn def
            Opcode::FN_ARGS => {
                script_err_not_impl!(
                    self.trap,
                    "FN_ARGS: nested function definitions `fn foo() {{}}` not supported in shaders"
                );
            }
            Opcode::FN_LET_ARGS => {
                script_err_not_impl!(self.trap, "FN_LET_ARGS: nested function definitions `let foo = fn() {{}}` not supported in shaders");
            }
            Opcode::FN_ARG_DYN => {
                script_err_not_impl!(
                    self.trap,
                    "FN_ARG_DYN: dynamic function arguments not supported in shaders"
                );
            }
            Opcode::FN_ARG_TYPED => {
                script_err_not_impl!(
                    self.trap,
                    "FN_ARG_TYPED: typed function arguments not supported in shaders"
                );
            }
            Opcode::FN_BODY_DYN => {
                script_err_not_impl!(
                    self.trap,
                    "FN_BODY_DYN: function body definition not supported in shaders"
                );
            }
            Opcode::FN_BODY_TYPED => {
                script_err_not_impl!(
                    self.trap,
                    "FN_BODY_TYPED: typed function body not supported in shaders"
                );
            }
            Opcode::RETURN => self.handle_return(vm, output, opargs),
            Opcode::RETURN_IF_ERR => {
                script_err_shader!(
                    self.trap,
                    "RETURN_IF_ERR: error propagation `?` not supported in shaders"
                );
            }
            // IF
            Opcode::IF_TEST => self.handle_if_test(opargs),

            Opcode::IF_ELSE => self.handle_if_else(vm, opargs),
            // Use
            Opcode::USE => {
                script_err_shader!(self.trap, "USE: `use` imports not supported in shaders");
            }
            // Field
            Opcode::FIELD => self.handle_field(vm, output),
            Opcode::FIELD_NIL => {
                script_err_shader!(
                    self.trap,
                    "FIELD_NIL: optional chaining `obj?.field` not supported in shaders"
                );
            }
            Opcode::ME_FIELD => {
                script_err_not_impl!(
                    self.trap,
                    "ME_FIELD: `me.field` access not supported in shaders"
                );
            }
            Opcode::PROTO_FIELD => self.handle_field(vm, output),

            Opcode::POP_TO_ME => {
                self.pop_to_me(vm);
            }
            // Array index
            Opcode::ARRAY_INDEX => self.handle_array_index(vm, output),
            // Let
            Opcode::LET_DYN => self.handle_let_dyn(vm, output, opargs),
            Opcode::LET_TYPED => {
                script_err_not_impl!(self.trap, "LET_TYPED: typed let `let x: Type = ...` not yet supported in shaders, use `let x = Type(...)`");
            }
            Opcode::VAR_DYN => self.handle_var_dyn(vm, output, opargs),
            Opcode::VAR_TYPED => {
                script_err_not_impl!(self.trap, "VAR_TYPED: typed var `var x: Type = ...` not yet supported in shaders, use `var x = Type(...)`");
            }
            // Tree search
            Opcode::SEARCH_TREE => {
                script_err_shader!(
                    self.trap,
                    "SEARCH_TREE: tree search `#identifier` not supported in shaders"
                );
            }
            // Log
            Opcode::LOG => {
                self.handle_log(vm);
            }
            // Me/Scope
            Opcode::ME => {
                script_err_shader!(self.trap, "ME: `me` keyword not supported in shaders");
            }

            Opcode::SCOPE => {
                script_err_shader!(self.trap, "SCOPE: `scope` keyword not supported in shaders");
            }
            // For
            Opcode::FOR_1 => self.handle_for_1(vm),
            Opcode::FOR_2 => {
                script_err_shader!(self.trap, "FOR_2: `for k, v in obj` iteration not supported in shaders, use `for i in 0..n`");
            }
            Opcode::FOR_3 => {
                script_err_shader!(self.trap, "FOR_3: `for i, k, v in obj` iteration not supported in shaders, use `for i in 0..n`");
            }
            Opcode::LOOP => {
                script_err_not_impl!(
                    self.trap,
                    "LOOP: infinite `loop {{}}` not supported in shaders, use `for i in 0..n`"
                );
            }
            Opcode::FOR_END => self.handle_for_end(),
            Opcode::BREAK => {
                script_err_not_impl!(self.trap, "BREAK: `break` not yet supported in shaders");
            }
            Opcode::BREAKIFNOT => {
                script_err_not_impl!(
                    self.trap,
                    "BREAKIFNOT: conditional `break` not yet supported in shaders"
                );
            }
            Opcode::CONTINUE => {
                script_err_not_impl!(
                    self.trap,
                    "CONTINUE: `continue` not yet supported in shaders"
                );
            }
            // Range
            Opcode::RANGE => self.handle_range(vm),
            // Is
            Opcode::IS => {
                script_err_shader!(
                    self.trap,
                    "IS: type check `x is Type` not supported in shaders"
                );
            }
            // Try / OK
            Opcode::OK_TEST => {
                script_err_shader!(
                    self.trap,
                    "OK_TEST: `ok {{}}` block not supported in shaders"
                );
            }
            Opcode::OK_END => {
                script_err_shader!(
                    self.trap,
                    "OK_END: `ok {{}}` block not supported in shaders"
                );
            }
            Opcode::TRY_TEST => {
                script_err_shader!(
                    self.trap,
                    "TRY_TEST: `try {{}}` block not supported in shaders"
                );
            }
            Opcode::TRY_ERR => {
                script_err_shader!(
                    self.trap,
                    "TRY_ERR: `try {{}} catch {{}}` not supported in shaders"
                );
            }
            Opcode::TRY_OK => {
                script_err_shader!(
                    self.trap,
                    "TRY_OK: `try {{}}` block not supported in shaders"
                );
            }
            opcode => {
                script_err_shader!(
                    self.trap,
                    "unknown opcode {:?} not supported in shaders",
                    opcode
                );
                self.trap.goto_next();
                // unknown instruction
            }
        }
        self.maybe_pop_to_me(vm, opargs);
    }

    pub(crate) fn pop_to_me(&mut self, vm: &ScriptVm) {
        // Skip if we just closed an if block that had a return without a value
        if self.skip_next_pop_to_me {
            self.skip_next_pop_to_me = false;
            return;
        }
        if let Some(me) = self.mes.last_mut() {
            match me {
                ShaderMe::FnBody { .. } | ShaderMe::ForLoop { .. } | ShaderMe::IfBody { .. } => {
                    let (_ty, s) = self.stack.pop(self.trap.pass());
                    self.out.push_str(&s);
                    self.out.push_str(";\n");
                    self.stack.free_string(s);
                }
                ShaderMe::Pod { pod_ty: _, args } => {
                    let (ty, s) = self.stack.pop(self.trap.pass());

                    if let Some(last) = args.last() {
                        let last_was_named = last.name.is_some();
                        if last_was_named {
                            script_err_pod!(self.trap, "mixing named and ordered fields");
                        }
                    }

                    args.push(ShaderPodArg { name: None, ty, s });
                }
                ShaderMe::ArrayConstruct { args, elem_ty } => {
                    let (ty, s) = self.stack.pop(self.trap.pass());
                    let arg_ty = if let ShaderType::Id(id) = ty {
                        if let Some((v, _name)) = self.shader_scope.find_var(id) {
                            v.ty()
                        } else {
                            script_err_not_found!(
                                self.trap,
                                "shader variable {:?} not found{}",
                                id,
                                suggest_from_live_ids(id, &self.shader_scope.all_var_names())
                            );
                            vm.bx.code.builtins.pod.pod_void
                        }
                    } else if let Some(ty) = ty.make_concrete(&vm.bx.code.builtins.pod) {
                        ty
                    } else {
                        script_err_shader!(self.trap, "no matching shader type");
                        vm.bx.code.builtins.pod.pod_void
                    };

                    if let Some(elem_ty) = elem_ty {
                        if *elem_ty != arg_ty {
                            script_err_pod!(
                                self.trap,
                                "array element type mismatch: expected {}, got {}",
                                format_pod_type_name(&vm.bx.heap, *elem_ty),
                                format_pod_type_name(&vm.bx.heap, arg_ty)
                            );
                        }
                    } else {
                        *elem_ty = Some(arg_ty);
                    }
                    args.push(s);
                }
                ShaderMe::TextureBuiltin { args, .. } => {
                    let (_ty, s) = self.stack.pop(self.trap.pass());
                    args.push(s);
                }
                ShaderMe::ScriptCall { out, args, .. } => {
                    let (ty, s) = self.stack.pop(self.trap.pass());
                    //let has_self = if let ShaderType::Pod(_) = sself{true} else {false};
                    if out.len() > 0 {
                        out.push_str(", ");
                    }
                    // Store the ShaderType directly - we'll resolve AbstractInt/AbstractFloat
                    // against the function's declared parameter types later
                    if let ShaderType::Id(id) = &ty {
                        if let Some((v, _name)) = self.shader_scope.find_var(*id) {
                            args.push(ShaderType::Pod(v.ty()));
                        } else {
                            script_err_not_found!(
                                self.trap,
                                "shader variable {:?} not found{}",
                                id,
                                suggest_from_live_ids(*id, &self.shader_scope.all_var_names())
                            );
                            args.push(ty);
                        }
                    } else {
                        args.push(ty);
                    }
                    out.push_str(&s);
                    self.stack.free_string(s);
                }
                ShaderMe::BuiltinCall { args, .. } | ShaderMe::PodBuiltinMethod { args, .. } => {
                    let (ty, s) = self.stack.pop(self.trap.pass());
                    // Resolve Id to Pod type, but keep AbstractInt/AbstractFloat as-is
                    let resolved_ty = if let ShaderType::Id(id) = &ty {
                        if let Some((v, _name)) = self.shader_scope.find_var(*id) {
                            ShaderType::Pod(v.ty())
                        } else {
                            script_err_not_found!(
                                self.trap,
                                "shader variable {:?} not found{}",
                                id,
                                suggest_from_live_ids(*id, &self.shader_scope.all_var_names())
                            );
                            ty
                        }
                    } else {
                        ty
                    };
                    args.push((resolved_ty, s));
                }
                _ => todo!(),
            }
        }
    }

    pub(crate) fn maybe_pop_to_me(&mut self, vm: &ScriptVm, opargs: OpcodeArgs) {
        if opargs.is_pop_to_me() {
            self.pop_to_me(vm);
        }
    }
}
