use makepad_live_id::*;
use makepad_math::*;
use crate::value::*;
use crate::trap::*;
use crate::function::*;
use crate::vm::*;
use crate::opcode::*;
use crate::pod::*;
use crate::heap::*;
use crate::mod_pod::*;
use crate::mod_shader::*;
use crate::shader_backend::*;
use std::fmt::Write;
use crate::makepad_error_log::*;
use std::collections::BTreeSet;

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
pub struct ShaderPodArg{
    pub name: Option<LiveId>,
    pub ty: ShaderType,
    pub s: String
}

#[derive(Debug)]
pub enum ShaderMe{
    FnBody{
        ret: Option<ScriptPodType>,
        escaped: bool,  // true when all code paths have returned
    },
    LoopBody,
    ForLoop{
        var_id: LiveId,
    },
    IfBody{
        target_ip: u32,
        start_pos: usize,
        stack_depth: usize,
        phi: Option<String>,
        phi_type: Option<ShaderType>,
        has_return: bool,         // true if current branch has a return
        if_branch_returned: bool, // remembers if the if-branch returned (used when in else)
    },
    BuiltinCall{name:LiveId, fnptr: NativeId, args:Vec<(ShaderType, String)>},
    ScriptCall{out:String, name:LiveId, fnobj: ScriptObject, sself:ShaderType, args:Vec<ShaderType>},
    Pod{pod_ty:ScriptPodType, args: Vec<ShaderPodArg>},
    ArrayConstruct{args:Vec<String>, elem_ty:Option<ScriptPodType>},
    TextureBuiltin{
        method_id: LiveId,
        tex_type: TextureType,
        texture_expr: String,
        args: Vec<String>,
    },
}

#[derive(Debug, PartialEq, Clone)]
pub enum ShaderType{
    None,
    IoSelf(ScriptObject),
    PodType(ScriptPodType),
    Pod(ScriptPodType),
    PodPtr(ScriptPodType), // Pointer to pod type (used for uniform buffers in Metal)
    Texture(TextureType), // Texture type for method calls like .size()
    Id(LiveId),
    AbstractInt,
    AbstractFloat,
    Range{start:String, end:String, ty:ScriptPodType},
    Error(ScriptValue)
}

impl ShaderType{
    pub fn make_concrete(&self, builtins:&ScriptPodBuiltins)->Option<ScriptPodType>{
        match self{
            Self::Pod(ty) => Some(*ty),
            Self::PodPtr(ty) => Some(*ty),
            Self::Texture(_) => None, // Textures don't have a concrete pod type
            Self::Id(_id) => None,
            Self::None => None,
            Self::IoSelf(_) => None,
            Self::PodType(_) => None,
            Self::AbstractInt => Some(builtins.pod_i32),
            Self::AbstractFloat => Some(builtins.pod_f32),
            Self::Range{ty,..} => Some(*ty),
            Self::Error(_e) => None,
        }
    }
}

#[derive(Debug)]
pub struct ShaderFn{
    pub call_sig: String,
    pub overload: usize,
    pub name: LiveId,
    pub args: Vec<ScriptPodType>,
    pub fnobj: ScriptObject,
    pub out: String,
    pub ret: ScriptPodType,
}

#[derive(Debug)]
pub enum ShaderScopeItem{
    IoSelf(ScriptObject),
    Let{ty:ScriptPodType, shadow:usize},
    Var{ty:ScriptPodType, shadow:usize},
    PodType{ty:ScriptPodType, shadow:usize}
}

impl ShaderScopeItem{
    pub fn ty(&self)->ScriptPodType{
        match self{
            Self::IoSelf(_)=>ScriptPodType::VOID,
            Self::Let{ty,..}=>*ty,
            Self::Var{ty,..}=>*ty,
            Self::PodType{ty,..}=>*ty,
        }
    }
    
    pub fn shadow(&self)->usize{
        match self{
            Self::IoSelf(_)=>0,
            Self::Let{shadow,..}=>*shadow,
            Self::Var{shadow,..}=>*shadow,
            Self::PodType{shadow,..}=>*shadow,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SamplerFilter {
    Nearest,
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SamplerAddress {
    Repeat,
    ClampToEdge,
    ClampToZero,
    MirroredRepeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SamplerCoord {
    Normalized,
    Pixel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShaderSampler {
    pub filter: SamplerFilter,
    pub address: SamplerAddress,
    pub coord: SamplerCoord,
}

impl Default for ShaderSampler {
    fn default() -> Self {
        Self {
            filter: SamplerFilter::Linear,
            address: SamplerAddress::Repeat,
            coord: SamplerCoord::Normalized,
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct ShaderSamplerOptions{
}

#[derive(Debug, Default, Clone)]
pub struct ShaderStorageFlags(u32);
impl ShaderStorageFlags{
    pub fn set_read(&mut self){self.0 |= 1}
    pub fn set_write(&mut self){self.0 |= 1}
    pub fn is_read(&self)->bool{self.0 & 1 != 0}
    pub fn is_write(&self)->bool{self.0 & 2 != 0}
    pub fn is_readwrite(&self)->bool{self.0 & 3 == 3}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureType {
    Texture1d,
    Texture1dArray,
    Texture2d,
    Texture2dArray,
    Texture3d,
    Texture3dArray,
    TextureCube,
    TextureCubeArray,
    TextureDepth,
    TextureDepthArray,
}

#[derive(Debug, Clone)]
pub enum ShaderIoKind{
    StorageBuffer(ShaderStorageFlags),
    UniformBuffer,
    Sampler(ShaderSamplerOptions),
    Texture(TextureType),
    Varying,
    VertexBuffer,
    VertexPosition,
    FragmentOutput(u8),
    RustInstance,
    Uniform,
    DynInstance,
}

#[allow(unused)]
#[derive(Debug)]
pub struct ShaderIo{
    pub kind: ShaderIoKind,
    pub name: LiveId,
    pub ty: ScriptPodType,
    /// Buffer index assigned during Metal/backend code generation (for uniform buffers, etc.)
    pub buffer_index: Option<usize>,
}

impl ShaderIo {
    pub fn kind(&self) -> &ShaderIoKind {
        &self.kind
    }
    
    pub fn name(&self) -> LiveId {
        self.name
    }
    
    pub fn ty(&self) -> ScriptPodType {
        self.ty
    }
    
    pub fn buffer_index(&self) -> Option<usize> {
        self.buffer_index
    }
}


#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub enum ShaderMode{
    Vertex,
    #[default]
    Fragment,
    Compute
}

#[derive(Default, Debug)]
pub struct ShaderOutput{
    pub mode: ShaderMode,
    pub backend: ShaderBackend,
    pub io: Vec<ShaderIo>,
    pub recur_block: Vec<ScriptObject>,
    pub structs: BTreeSet<ScriptPodType>,
    pub functions: Vec<ShaderFn>,
    pub samplers: Vec<ShaderSampler>,
}

/// Mapping of uniform buffer type names to their assigned buffer indices
#[derive(Default, Debug, Clone)]
pub struct UniformBufferBindings {
    /// Maps Pod type name (e.g. DrawCallUniforms) to buffer index
    pub bindings: Vec<(LiveId, usize)>,
}

impl UniformBufferBindings {
    /// Look up buffer index by Pod type name
    pub fn get_by_type_name(&self, type_name: LiveId) -> Option<usize> {
        self.bindings.iter().find(|(name, _)| *name == type_name).map(|(_, idx)| *idx)
    }
} 

impl ShaderOutput{
    /// Pre-collect ALL Rust instance fields in the correct order for struct layout.
    /// Walks from deepest prototype to io_self, collecting ALL rust type properties.
    /// Dyn instance fields are NOT pre-collected - they are added during compilation
    /// as encountered, and their order doesn't matter.
    /// 
    /// IoInstance struct layout: Dyn fields first (any order), Rust fields last (must match Repr(C))
    /// RustInstance fields are pushed in the correct order, so no sorting is needed later.
    pub fn pre_collect_rust_instance_io(&mut self, vm: &mut ScriptVm, io_self: ScriptObject) {
        // First, collect all prototypes in order (deepest first)
        let mut proto_chain = Vec::new();
        let mut current = io_self;
        proto_chain.push(current);
        while let Some(proto_obj) = vm.heap.proto(current).as_object() {
            proto_chain.push(proto_obj);
            current = proto_obj;
        }
        // Reverse so deepest (root) prototype comes first
        proto_chain.reverse();
        
        // Walk from deepest prototype to io_self
        // Only collect Rust type properties - dyn properties are added during compilation
        for proto_obj in proto_chain {
            let obj_data = vm.heap.object_data(proto_obj);
            let ty_index = obj_data.tag.as_type_index();
            
            if let Some(ty_index) = ty_index {
                // Collect the ordered props first
                let type_check = vm.heap.type_check(ty_index);
                let ordered_props: Vec<_> = type_check.props.iter_ordered().collect();
                
                for (field_id, _type_id) in ordered_props {
                    // Get the value and its pod type - we emit ALL rust fields
                    let value = vm.heap.value(proto_obj, field_id.into(), &vm.thread.trap);
                    if let Some(pod_ty) = Self::get_pod_type_from_value(vm, value) {
                        if !self.io.iter().any(|io| io.name == field_id) {
                            vm.heap.pod_type_name_if_not_set(pod_ty, field_id);
                            self.io.push(ShaderIo {
                                kind: ShaderIoKind::RustInstance,
                                name: field_id,
                                ty: pod_ty,
                                buffer_index: None,
                            });
                        }
                    }
                }
            }
        }
    }
    
    /// Pre-collect fragment outputs from the shader object prototype chain.
    /// Walks the prototype chain and finds all properties marked with SHADER_IO_FRAGMENT_OUTPUT_*.
    pub fn pre_collect_fragment_outputs(&mut self, vm: &mut ScriptVm, io_self: ScriptObject) {
        use crate::mod_shader::{SHADER_IO_FRAGMENT_OUTPUT_0, SHADER_IO_FRAGMENT_OUTPUT_MAX};
        
        // Walk the prototype chain
        let mut current = Some(io_self);
        while let Some(obj) = current {
            // Iterate over all key-value pairs in this object
            let obj_data = vm.heap.object_data(obj);
            for kv in &obj_data.vec {
                if let Some(value_obj) = kv.value.as_object() {
                    if let Some(io_type) = vm.heap.as_shader_io(value_obj) {
                        // Check if it's a fragment output
                        if io_type.0 >= SHADER_IO_FRAGMENT_OUTPUT_0.0 && io_type.0 <= SHADER_IO_FRAGMENT_OUTPUT_MAX.0 {
                            let index = (io_type.0 - SHADER_IO_FRAGMENT_OUTPUT_0.0) as u8;
                            
                            // Get the pod type from the prototype of the fragment output object
                            let proto_value = vm.heap.proto(value_obj);
                            if let Some(pod_ty) = Self::get_pod_type_from_value(vm, proto_value) {
                                // Check if we already have this fragment output index
                                let already_exists = self.io.iter().any(|io| {
                                    matches!(io.kind, ShaderIoKind::FragmentOutput(idx) if idx == index)
                                });
                                
                                if !already_exists {
                                    if let Some(key_id) = kv.key.as_id() {
                                        self.io.push(ShaderIo {
                                            kind: ShaderIoKind::FragmentOutput(index),
                                            name: key_id,
                                            ty: pod_ty,
                                            buffer_index: None,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // Move to next prototype
            current = vm.heap.proto(obj).as_object();
        }
    }
    
    fn get_pod_type_from_value(vm: &ScriptVm, value: ScriptValue) -> Option<ScriptPodType> {
        // Check if it's a primitive type (f32, f64, bool, etc.)
        if let Some(pod_ty) = vm.code.builtins.pod.value_to_exact_type(value) {
            return Some(pod_ty);
        }
        // Check if it's a pod type object
        if let Some(pod_ty) = vm.heap.pod_type(value) {
            return Some(pod_ty);
        }
        // Check if it's a pod instance
        if let Some(pod) = value.as_pod() {
            let pod = &vm.heap.pods[pod.index as usize];
            return Some(pod.ty);
        }
        // Check if it's a pod type reference
        if let Some(pod_ty) = value.as_pod_type() {
            return Some(pod_ty);
        }
        None
    }
    
    pub fn create_struct_defs(&mut self, vm:&ScriptVm, out:&mut String){
        for io in &self.io{
            let ty = io.ty;
            if let ScriptPodTy::Struct{..} = vm.heap.pod_type_ref(ty).ty{
                self.structs.insert(ty);
            }
        }
        self.backend.pod_struct_defs(vm.heap, &self.structs, out);
    }
    
    pub fn create_functions(&self, out: &mut String) {
        for fns in &self.functions {
            writeln!(out, "{}{{\n{}}}\n", fns.call_sig, fns.out).ok();
        }
    }
    
    /// Find the vertex buffer object from io_self by looking for SHADER_IO_VERTEX_BUFFER type
    pub fn find_vertex_buffer_object(&self, vm: &ScriptVm, io_self: ScriptObject) -> Option<ScriptObject> {
        // Walk the prototype chain looking for vertex buffer properties
        let mut current = Some(io_self);
        while let Some(obj) = current {
            let obj_data = vm.heap.object_data(obj);
            
            // Check map properties
            if let Some(ret) = obj_data.map_iter_ret(|_key, value| {
                if let Some(value_obj) = value.as_object() {
                    if let Some(io_type) = vm.heap.as_shader_io(value_obj) {
                        if io_type == SHADER_IO_VERTEX_BUFFER {
                            return Some(value_obj);
                        }
                    }
                }
                None
            }) {
                return Some(ret);
            }
            
            // Move to next prototype
            current = vm.heap.proto(obj).as_object();
        }
        None
    }
    
    /// Assign buffer indices to uniform buffers starting from `start_index`.
    /// Returns the UniformBufferBindings and the next available buffer index.
    /// Also sets the buffer_index field on each ShaderIo.
    pub fn assign_uniform_buffer_indices(&mut self, heap: &ScriptHeap, start_index: usize) -> (UniformBufferBindings, usize) {
        let mut bindings = UniformBufferBindings::default();
        let mut buf_idx = start_index;
        
        for io in &mut self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                // Get the Pod type name for this uniform buffer
                let pod_type = heap.pod_type_ref(io.ty);
                if let Some(type_name) = pod_type.name {
                    bindings.bindings.push((type_name, buf_idx));
                }
                io.buffer_index = Some(buf_idx);
                buf_idx += 1;
            }
        }
        
        (bindings, buf_idx)
    }
    
    /// Get the UniformBufferBindings from the current IO state.
    /// This should be called after `assign_uniform_buffer_indices` has been called.
    pub fn get_uniform_buffer_bindings(&self, heap: &ScriptHeap) -> UniformBufferBindings {
        let mut bindings = UniformBufferBindings::default();
        
        for io in &self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                if let Some(buf_idx) = io.buffer_index {
                    let pod_type = heap.pod_type_ref(io.ty);
                    if let Some(type_name) = pod_type.name {
                        bindings.bindings.push((type_name, buf_idx));
                    }
                }
            }
        }
        
        bindings
    }
    
    /// Get or create a sampler with the given properties, returns the sampler index
    pub fn get_or_create_sampler(&mut self, sampler: ShaderSampler) -> usize {
        // Check if we already have this sampler
        if let Some(idx) = self.samplers.iter().position(|s| *s == sampler) {
            return idx;
        }
        // Create new sampler
        let idx = self.samplers.len();
        self.samplers.push(sampler);
        idx
    }
}


#[derive(Default)]
pub struct ShaderScope{
    pub shader_scope: Vec<LiveIdMap<LiveId, ShaderScopeItem>>,
}

#[derive(Default)]
pub struct ShaderFnCompiler{
    pub out: String,
    pub stack: ShaderStack,
    pub script_scope: ScriptObject,
    pub shader_scope: ShaderScope,
    pub mes: Vec<ShaderMe>,
    pub trap: ScriptTrap,
}

#[derive(Default)]
pub struct ShaderStack{
    pub(crate) stack_limit: usize,
    pub(crate) types: Vec<ShaderType>,
    pub(crate) strings: Vec<String>,
    pub(crate) free: Vec<String>,
}

macro_rules! push_fmt {
    ($self:ident, $ty:expr, $fmt_str:literal, $($args:expr),*) => {{
        let s = free_fmt!($self, $fmt_str, $($args),*);
        $self.stack.push(&$self.trap, $ty, s);
    }};
}

macro_rules! free_fmt {
    ($self:ident, $fmt_str:literal, $($args:expr),*) => {{
        let mut s = $self.stack.new_string();
        write!(s, $fmt_str, $($args),*).ok();
        s
    }};
}

impl ShaderScope{
        
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
    
    pub fn define_io_self(&mut self, sself:ScriptObject) {
        let scope = self.shader_scope.last_mut().unwrap();
        scope.insert(id!(self),ShaderScopeItem::IoSelf(sself) );
    }
    
    pub fn define_var(&mut self, id: LiveId, ty: ScriptPodType) -> usize {
        let scope = self.shader_scope.last_mut().unwrap();
        if let Some(item) = scope.get_mut(&id) {
            let shadow = item.shadow() + 1;
            *item = ShaderScopeItem::Var{ty, shadow};
            shadow
        } else {
            scope.insert(id, ShaderScopeItem::Var{ty, shadow:0});
            0
        }
    }

    pub fn define_let(&mut self, id: LiveId, ty: ScriptPodType) -> usize {
        let scope = self.shader_scope.last_mut().unwrap();
        if let Some(item) = scope.get_mut(&id) {
            let shadow = item.shadow() + 1;
            *item = ShaderScopeItem::Let{ty, shadow};
            shadow
        } else {
            scope.insert(id, ShaderScopeItem::Let{ty, shadow:0});
            0
        }
    }

    pub fn define_pod_type(&mut self, id: LiveId, ty: ScriptPodType) {
        let scope = self.shader_scope.last_mut().unwrap();
        if let Some(item) = scope.get_mut(&id) {
            let shadow = item.shadow() + 1;
            *item = ShaderScopeItem::PodType{ty, shadow};
        } else {
            scope.insert(id, ShaderScopeItem::PodType{ty, shadow:0});
        }
    }
}

impl ShaderStack{    
    pub fn pop(&mut self, trap:&ScriptTrap)->(ShaderType,String){
        if let Some(s) = self.types.pop(){
            return (s,self.strings.pop().unwrap())
        }
        else{
            trap.err_stack_underflow();
            (ShaderType::Error(NIL), String::new())
        }
    }
    
    pub fn peek(&self, trap:&ScriptTrap)->(&ShaderType, &String){
        if let Some(ty) = self.types.last(){
            return (ty, self.strings.last().unwrap())
        }
        else{
            trap.err_stack_underflow();
            static EMPTY: (ShaderType, String) = (ShaderType::None, String::new());
            (&EMPTY.0, &EMPTY.1)
        }
    }
    
    pub fn push(&mut self, trap:&ScriptTrap, ty:ShaderType, s:String){
        if self.types.len() > self.stack_limit{
            trap.err_stack_overflow();
        }
        else{
            self.types.push(ty);
            self.strings.push(s);
        }
    }
    
    pub fn new_string(&mut self)->String{
        if let Some(s) = self.free.pop(){
            s
        }
        else{
            String::new()
        }
    }
    
    pub fn free_string(&mut self, s:String){
        let mut s = s;
        s.clear();
        self.free.push(s);
    }
}


impl ShaderFnCompiler{
    
    pub fn new(script_scope:ScriptObject)->Self{
        ShaderFnCompiler{
            script_scope,
            stack: ShaderStack{
                stack_limit: 1000000,
                ..Default::default()
            },
            mes: vec![],
            shader_scope: ShaderScope{shader_scope:vec![Default::default()]},
            ..Default::default()
        }
    }
    
    pub fn compile_fn(&mut self, vm:&mut ScriptVm, output:&mut ShaderOutput, fnip:ScriptIp)->ScriptPodType{
        //output.backend = ShaderBackend::Wgsl;
        output.backend.register_ids();
        
        self.mes.push(ShaderMe::FnBody{
            ret: None,
            escaped: false,
        });
        // alright lets go trace the opcodes
        self.trap.ip = fnip;
        self.trap.in_rust = true;
        let bodies = vm.code.bodies.borrow();
        let body = &bodies[self.trap.ip.body as usize];
        
        // Calculate function end position from the FN_BODY_DYN opcode that precedes the function body
        // fnip.index points to the first opcode AFTER FN_BODY_DYN
        // FN_BODY_DYN's opargs contains the jump offset from its position to the end of the function
        let fn_body_opcode = body.parser.opcodes[(fnip.index - 1) as usize];
        let fn_end_index = if let Some((_opcode, args)) = fn_body_opcode.as_opcode() {
            (fnip.index - 1) + args.to_u32()
        } else {
            // Fallback to opcodes.len() if we can't find FN_BODY_DYN
            body.parser.opcodes.len() as u32
        };
        
        while self.trap.ip.index < fn_end_index {
            let opcode = body.parser.opcodes[self.trap.ip.index as usize];
            
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
                                self.handle_if_else(args);
                            }
                        }
                        _ => {}
                    }
                }
                self.trap.goto_next();
                self.handle_if_else_phi_unreachable();
            } else if let Some((opcode, args)) = opcode.as_opcode(){
                self.opcode(vm, output, opcode, args);
                self.trap.goto_next();
                self.handle_if_else_phi(vm, output);
            }
            else{ // id or immediate value
                self.push_immediate(opcode, &vm.code.builtins.pod, &output.backend);
                self.trap.goto_next();
                self.handle_if_else_phi(vm, output);
            }
            // alright lets see if we have a trap, ifso we can log it
            if let Some(err) = self.trap.err.take(){
                if let Some(ptr) = err.value.as_err(){
                    if let Some(loc2) = vm.code.ip_to_loc(ptr.ip){
                        log_with_level(&loc2.file, loc2.line, loc2.col, loc2.line, loc2.col, format!("{}", err.value), LogLevel::Error);
                    }
                }
            }
            // The trap handling for Return is no longer needed since we use fn_end_index
            // to determine when to stop. The trap may still be set by handle_return but
            // we ignore it and continue processing to properly close all control structures.
            self.trap.on.take();
        }
        let value = self.mes.pop();
        if let Some(ShaderMe::FnBody{ret, ..}) = value{
            return ret.unwrap_or(vm.code.builtins.pod.pod_void)
        }
        panic!("Unexpected ME at end {:?}", value)
    }

    pub(crate) fn pop_resolved(&mut self, _vm:&ScriptVm)->(ShaderType,String){
        let (ty, s) = self.stack.pop(&self.trap);
        // if ty is an id, look it up
        match ty{
            ShaderType::Id(id)=>{
                // look it up on our scope
                if let Some((sc, shadow)) = self.shader_scope.find_var(id){
                    let mut s2 = self.stack.new_string();
                    if let ShaderScopeItem::IoSelf(obj) = sc{
                        return (ShaderType::IoSelf(*obj), s2)
                    }
                    if shadow > 0 {
                        write!(s2, "_s{}{}", shadow, id).ok();
                    }
                    else if id == id!(self) {
                        write!(s2, "_self").ok();
                    }
                    else{
                        write!(s2, "{}", id).ok();
                    }
                    self.stack.free_string(s);
                    return (ShaderType::Pod(sc.ty()), s2)
                }
                self.trap.err_not_found();
                self.stack.free_string(s);
                return (ShaderType::Error(NIL), self.stack.new_string())
            },
            _=>(ty, s),
        }
    }
    
    
    fn push_immediate(&mut self, value:ScriptValue, builtins:&ScriptPodBuiltins, backend:&ShaderBackend){
        if let Some(v) = value.as_f64(){ // abstract int or float
            let mut s = self.stack.new_string();
            write_shader_float(&mut s, v);
            return self.stack.push(&self.trap, ShaderType::AbstractFloat, s);
        }
        if let Some(v) = value.as_u40(){
            return push_fmt!(self, ShaderType::AbstractInt, "{}", v);
        }
        if let Some(id) = value.as_id(){
            return push_fmt!(self, ShaderType::Id(id), "{}", id);
        }
        if let Some(v) = value.as_f32(){
            let mut s = self.stack.new_string();
            write_shader_float(&mut s, v as f64);
            s.push('f');
            return self.stack.push(&self.trap, ShaderType::Pod(builtins.pod_f32), s);
        }
        if let Some(v) = value.as_f16(){
            let mut s = self.stack.new_string();
            write_shader_float(&mut s, v as f64);
            s.push('h');
            return self.stack.push(&self.trap, ShaderType::Pod(builtins.pod_f16), s);
        }
        if let Some(v) = value.as_u32(){
            return push_fmt!(self, ShaderType::Pod(builtins.pod_u32), "{}u", v);
        }
        if let Some(v) = value.as_i32(){
            return push_fmt!(self, ShaderType::Pod(builtins.pod_i32), "{}i", v);
        }
        if let Some(v) = value.as_bool(){
            return push_fmt!(self, ShaderType::Pod(builtins.pod_bool), "{}", v);
        }
        if let Some(v) = value.as_color(){
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
            return self.stack.push(&self.trap, ShaderType::Pod(builtins.pod_vec4f), s);
        }
        self.trap.err_no_matching_shader_type();
    }

    pub(crate) fn ensure_struct_name(&self, vm: &mut ScriptVm, output: &mut ShaderOutput, pod_ty: ScriptPodType, used_name: LiveId) -> LiveId {
        if let Some(name) = vm.heap.pod_type_name(pod_ty) {
            if name != used_name && used_name != id!(self) && used_name != id!(vec2) && used_name != id!(vec3) && used_name != id!(vec4) {
                self.trap.err_struct_name_not_consistent();
            }
            return name;
        }
        output.structs.insert(pod_ty);
        vm.heap.pod_type_name_set(pod_ty, used_name);
        used_name
    }

    fn opcode(&mut self, vm:&mut ScriptVm, output: &mut ShaderOutput, opcode: Opcode, opargs:OpcodeArgs){
        match opcode{
// Arithmetic
            Opcode::NOT=>{}
            Opcode::NEG=>self.handle_neg(vm, opargs, "-"),
            Opcode::MUL=>self.handle_arithmetic(vm, opargs, "*", false),
            Opcode::DIV=>self.handle_arithmetic(vm, opargs, "/", false),
            Opcode::MOD=>self.handle_arithmetic(vm, opargs, "%", false),
            Opcode::ADD=>self.handle_arithmetic(vm, opargs, "+", false),
            Opcode::SUB=>self.handle_arithmetic(vm, opargs, "-", false),
            Opcode::SHL=>self.handle_arithmetic(vm, opargs, ">>", true),
            Opcode::SHR=>self.handle_arithmetic(vm, opargs, "<<", true),
            Opcode::AND=>self.handle_arithmetic(vm, opargs, "&", true),
            Opcode::OR=>self.handle_arithmetic(vm, opargs, "|", true),
            Opcode::XOR=>self.handle_arithmetic(vm, opargs, "^", true),
                        
// ASSIGN
            Opcode::ASSIGN=>self.handle_assign(vm),
            Opcode::ASSIGN_ADD=>{self.handle_arithmetic_assign(vm, opargs, "+=", false);},
            Opcode::ASSIGN_SUB=>{self.handle_arithmetic_assign(vm, opargs, "-=", false);},
            Opcode::ASSIGN_MUL=>{self.handle_arithmetic_assign(vm, opargs, "*=", false);},
            Opcode::ASSIGN_DIV=>{self.handle_arithmetic_assign(vm, opargs, "/=", false);},
            Opcode::ASSIGN_MOD=>{self.handle_arithmetic_assign(vm, opargs, "%=", false);},
            Opcode::ASSIGN_AND=>{self.handle_arithmetic_assign(vm, opargs, "&=", true);},
            Opcode::ASSIGN_OR=>{self.handle_arithmetic_assign(vm, opargs, "|=", true);},
            Opcode::ASSIGN_XOR=>{self.handle_arithmetic_assign(vm, opargs, "^=", true);},
            Opcode::ASSIGN_SHL=>{self.handle_arithmetic_assign(vm, opargs, ">>=", true);},
            Opcode::ASSIGN_SHR=>{self.handle_arithmetic_assign(vm, opargs, "<<=", true);},
            Opcode::ASSIGN_IFNIL=>{self.trap.err_not_impl();},
// ASSIGN FIELD                       
            Opcode::ASSIGN_FIELD=>self.handle_assign_field(vm, output),
            Opcode::ASSIGN_FIELD_ADD=>{self.handle_arithmetic_field_assign(vm, opargs, "+=", false);},
            Opcode::ASSIGN_FIELD_SUB=>{self.handle_arithmetic_field_assign(vm, opargs, "-=", false);},
            Opcode::ASSIGN_FIELD_MUL=>{self.handle_arithmetic_field_assign(vm, opargs, "*=", false);},
            Opcode::ASSIGN_FIELD_DIV=>{self.handle_arithmetic_field_assign(vm, opargs, "/=", false);},
            Opcode::ASSIGN_FIELD_MOD=>{self.handle_arithmetic_field_assign(vm, opargs, "%=", false);},
            Opcode::ASSIGN_FIELD_AND=>{self.handle_arithmetic_field_assign(vm, opargs, "&=", true);},
            Opcode::ASSIGN_FIELD_OR=>{self.handle_arithmetic_field_assign(vm, opargs, "|=", true);},
            Opcode::ASSIGN_FIELD_XOR=>{self.handle_arithmetic_field_assign(vm, opargs, "^=", true);},
            Opcode::ASSIGN_FIELD_SHL=>{self.handle_arithmetic_field_assign(vm, opargs, ">>=", true);},
            Opcode::ASSIGN_FIELD_SHR=>{self.handle_arithmetic_field_assign(vm, opargs, "<<=", true);},
            Opcode::ASSIGN_FIELD_IFNIL=>{self.trap.err_not_impl();},
                                    
            Opcode::ASSIGN_INDEX=>self.handle_assign_index(vm),
            Opcode::ASSIGN_INDEX_ADD=>{self.handle_arithmetic_index_assign(vm, opargs, "+=", false);},
            Opcode::ASSIGN_INDEX_SUB=>{self.handle_arithmetic_index_assign(vm, opargs, "-=", false);},
            Opcode::ASSIGN_INDEX_MUL=>{self.handle_arithmetic_index_assign(vm, opargs, "*=", false);},
            Opcode::ASSIGN_INDEX_DIV=>{self.handle_arithmetic_index_assign(vm, opargs, "/=", false);},
            Opcode::ASSIGN_INDEX_MOD=>{self.handle_arithmetic_index_assign(vm, opargs, "%=", false);},
            Opcode::ASSIGN_INDEX_AND=>{self.handle_arithmetic_index_assign(vm, opargs, "&=", true);},
            Opcode::ASSIGN_INDEX_OR=>{self.handle_arithmetic_index_assign(vm, opargs, "|=", true);},
            Opcode::ASSIGN_INDEX_XOR=>{self.handle_arithmetic_index_assign(vm, opargs, "^=", true);},
            Opcode::ASSIGN_INDEX_SHL=>{self.handle_arithmetic_index_assign(vm, opargs, ">>=", true);},
            Opcode::ASSIGN_INDEX_SHR=>{self.handle_arithmetic_index_assign(vm, opargs, "<<=", true);},
            Opcode::ASSIGN_INDEX_IFNIL=>{self.trap.err_not_impl();},
// ASSIGN ME            
            Opcode::ASSIGN_ME=>self.handle_assign_me(vm),
                                    
            Opcode::ASSIGN_ME_BEFORE | Opcode::ASSIGN_ME_AFTER=>{self.trap.err_opcode_not_supported_in_shader();},
                                    
            Opcode::ASSIGN_ME_BEGIN=>{self.trap.err_opcode_not_supported_in_shader();},
            
// CONCAT  
            Opcode::CONCAT=>{self.trap.err_opcode_not_supported_in_shader();},
// EQUALITY
            Opcode::EQ=>{self.handle_eq(vm, opargs, "==");},
            Opcode::NEQ=>{self.handle_eq(vm, opargs, "!=");},
                        
            Opcode::LT=>{self.handle_eq(vm, opargs, "<");},
            Opcode::GT=>{self.handle_eq(vm, opargs, ">");},
            Opcode::LEQ=>{self.handle_eq(vm, opargs, "<=");},
            Opcode::GEQ=>{self.handle_eq(vm, opargs, ">=");},
                        
            Opcode::LOGIC_AND =>{self.handle_logic(vm, opargs, "&&");},
            Opcode::LOGIC_OR =>{self.handle_logic(vm, opargs, "||");},
            Opcode::NIL_OR =>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::SHALLOW_EQ =>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::SHALLOW_NEQ=>{self.trap.err_opcode_not_supported_in_shader();},
            // Object/Array begin
            Opcode::BEGIN_PROTO=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::BEGIN_PROTO_ME=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::END_PROTO=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::BEGIN_BARE=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::END_BARE=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::BEGIN_ARRAY=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::END_ARRAY=>{self.trap.err_opcode_not_supported_in_shader();},
// Calling
            Opcode::CALL_ARGS=>{
                self.handle_call_args(vm, output, opargs);
            },
            Opcode::CALL_EXEC | Opcode::METHOD_CALL_EXEC=>{
                self.handle_call_exec(vm, output);
            },
            Opcode::METHOD_CALL_ARGS=>{
                self.handle_method_call_args(vm, output, opargs);
            },
// Fn def
            Opcode::FN_ARGS=>{self.trap.err_not_impl();},
            Opcode::FN_LET_ARGS=>{self.trap.err_not_impl();},
            Opcode::FN_ARG_DYN=>{self.trap.err_not_impl();},
            Opcode::FN_ARG_TYPED=>{self.trap.err_not_impl();},
            Opcode::FN_BODY_DYN=>{self.trap.err_not_impl();},
            Opcode::FN_BODY_TYPED=>{self.trap.err_not_impl();},
            Opcode::RETURN=>self.handle_return(vm, opargs),
            Opcode::RETURN_IF_ERR=>{self.trap.err_opcode_not_supported_in_shader();},
// IF            
            Opcode::IF_TEST=>self.handle_if_test(opargs),
                        
            Opcode::IF_ELSE=>self.handle_if_else(opargs),
// Use            
            Opcode::USE=>{self.trap.err_opcode_not_supported_in_shader();},
// Field            
            Opcode::FIELD=>self.handle_field(vm, output),
            Opcode::FIELD_NIL=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::ME_FIELD=>{self.trap.err_not_impl();},
            Opcode::PROTO_FIELD=>self.handle_field(vm, output),
                        
            Opcode::POP_TO_ME=>{
                self.pop_to_me(vm);    
            },
// Array index            
            Opcode::ARRAY_INDEX=>{self.trap.err_not_impl();},
// Let                   
            Opcode::LET_DYN=>self.handle_let_dyn(vm, output, opargs),
            Opcode::LET_TYPED=>{self.trap.err_not_impl();},
            Opcode::VAR_DYN=>self.handle_var_dyn(vm, output, opargs),
            Opcode::VAR_TYPED=>{self.trap.err_not_impl();},
// Tree search            
            Opcode::SEARCH_TREE=>{self.trap.err_opcode_not_supported_in_shader();},
// Log            
            Opcode::LOG=>{self.handle_log(vm);},
// Me/Scope
            Opcode::ME=>{self.trap.err_opcode_not_supported_in_shader();},
                        
            Opcode::SCOPE=>{self.trap.err_opcode_not_supported_in_shader();},
// For            
            Opcode::FOR_1 =>self.handle_for_1(),
            Opcode::FOR_2 =>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::FOR_3=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::LOOP=>{self.trap.err_not_impl();},
            Opcode::FOR_END=>self.handle_for_end(),
            Opcode::BREAK=>{self.trap.err_not_impl();},
            Opcode::BREAKIFNOT=>{self.trap.err_not_impl();},
            Opcode::CONTINUE=>{self.trap.err_not_impl();},
// Range            
            Opcode::RANGE=>self.handle_range(vm),
// Is            
            Opcode::IS=>{self.trap.err_opcode_not_supported_in_shader();},
// Try / OK            
            Opcode::OK_TEST=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::OK_END=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::TRY_TEST=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::TRY_ERR=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::TRY_OK=>{self.trap.err_opcode_not_supported_in_shader();},
            opcode=>{
                self.trap.err_opcode_not_supported_in_shader();
                println!("UNDEFINED OPCODE {}", opcode);
                self.trap.goto_next();
                // unknown instruction
            }
        }
        self.maybe_pop_to_me(vm, opargs);
    }
    
    pub(crate) fn pop_to_me(&mut self, vm:&ScriptVm){
        if let Some(me) = self.mes.last_mut(){
            match me{
                ShaderMe::FnBody{ .. } | ShaderMe::ForLoop{..} | ShaderMe::IfBody{..}=>{
                    let (_ty,s) = self.stack.pop(&self.trap);
                    self.out.push_str(&s);
                    self.out.push_str(";\n");
                    self.stack.free_string(s);
                }
                ShaderMe::Pod{pod_ty:_, args}=>{
                    let (ty, s) = self.stack.pop(&self.trap);
                    
                    if let Some(last) = args.last(){
                         let last_was_named = last.name.is_some();
                         if last_was_named {
                             self.trap.err_use_only_named_or_ordered_pod_fields();
                         }
                    }
                    
                    args.push(ShaderPodArg{
                        name: None,
                        ty,
                        s
                    });
                }
                ShaderMe::ArrayConstruct{args, elem_ty}=>{
                    let (ty, s) = self.stack.pop(&self.trap);
                    let arg_ty = if let ShaderType::Id(id) = ty {
                         if let Some((v, _name)) = self.shader_scope.find_var(id){
                             v.ty()
                         }
                         else{
                             self.trap.err_not_found();
                             vm.code.builtins.pod.pod_void
                         }
                    }
                    else if let Some(ty) = ty.make_concrete(&vm.code.builtins.pod){
                        ty
                    }
                    else{
                        self.trap.err_no_matching_shader_type();
                        vm.code.builtins.pod.pod_void
                    };
                    
                    if let Some(elem_ty) = elem_ty {
                        if *elem_ty != arg_ty {
                             self.trap.err_pod_type_not_matching();
                        }
                    }
                    else {
                        *elem_ty = Some(arg_ty);
                    }
                    args.push(s);
                }
                ShaderMe::TextureBuiltin{args, ..}=>{
                    let (_ty, s) = self.stack.pop(&self.trap);
                    args.push(s);
                }
                ShaderMe::ScriptCall{out, args, ..}=>{
                    let (ty, s) = self.stack.pop(&self.trap);
                    //let has_self = if let ShaderType::Pod(_) = sself{true} else {false};
                    if out.len() > 0{
                        out.push_str(", ");
                    }
                    // Store the ShaderType directly - we'll resolve AbstractInt/AbstractFloat
                    // against the function's declared parameter types later
                    if let ShaderType::Id(id) = &ty{
                         if let Some((v, _name)) = self.shader_scope.find_var(*id){
                             args.push(ShaderType::Pod(v.ty()));
                         }
                         else{
                             self.trap.err_not_found();
                             args.push(ty);
                         }
                    }
                    else{
                        args.push(ty);
                    }
                    out.push_str(&s);
                    self.stack.free_string(s);
                }
                ShaderMe::BuiltinCall{args, ..}=>{
                    let (ty, s) = self.stack.pop(&self.trap);
                    // Resolve Id to Pod type, but keep AbstractInt/AbstractFloat as-is
                    let resolved_ty = if let ShaderType::Id(id) = &ty {
                        if let Some((v, _name)) = self.shader_scope.find_var(*id) {
                            ShaderType::Pod(v.ty())
                        } else {
                            self.trap.err_not_found();
                            ty
                        }
                    } else {
                        ty
                    };
                    args.push((resolved_ty, s));
                }
                _=>todo!()
            }
        }
    }
    
    pub(crate) fn maybe_pop_to_me(&mut self, vm:&ScriptVm, opargs:OpcodeArgs){
        if opargs.is_pop_to_me(){
            self.pop_to_me(vm);
        }
    }
}
