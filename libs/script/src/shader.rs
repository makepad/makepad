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
use crate::shader_tables::*;
use crate::shader_builtins::*;
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
    fn make_concrete(&self, builtins:&ScriptPodBuiltins)->Option<ScriptPodType>{
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
    fn ty(&self)->ScriptPodType{
        match self{
            Self::IoSelf(_)=>ScriptPodType::VOID,
            Self::Let{ty,..}=>*ty,
            Self::Var{ty,..}=>*ty,
            Self::PodType{ty,..}=>*ty,
        }
    }
    
    fn shadow(&self)->usize{
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


#[derive(Debug, Default, Clone, Copy, PartialEq)]
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
    stack_limit: usize,
    types: Vec<ShaderType>,
    strings: Vec<String>,
    free: Vec<String>,
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
        
    fn enter_scope(&mut self) {
        self.shader_scope.push(Default::default());
    }
    
    fn exit_scope(&mut self) {
        self.shader_scope.pop();
    }
    
    fn find_var(&self, id: LiveId) -> Option<(&ShaderScopeItem, usize)> {
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
    
    fn define_var(&mut self, id: LiveId, ty: ScriptPodType) -> usize {
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

    fn define_let(&mut self, id: LiveId, ty: ScriptPodType) -> usize {
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

    fn define_pod_type(&mut self, id: LiveId, ty: ScriptPodType) {
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
    fn pop(&mut self, trap:&ScriptTrap)->(ShaderType,String){
        if let Some(s) = self.types.pop(){
            return (s,self.strings.pop().unwrap())
        }
        else{
            trap.err_stack_underflow();
            (ShaderType::Error(NIL), String::new())
        }
    }
    
    fn peek(&self, trap:&ScriptTrap)->(&ShaderType, &String){
        if let Some(ty) = self.types.last(){
            return (ty, self.strings.last().unwrap())
        }
        else{
            trap.err_stack_underflow();
            static EMPTY: (ShaderType, String) = (ShaderType::None, String::new());
            (&EMPTY.0, &EMPTY.1)
        }
    }
    
    fn push(&mut self, trap:&ScriptTrap, ty:ShaderType, s:String){
        if self.types.len() > self.stack_limit{
            trap.err_stack_overflow();
        }
        else{
            self.types.push(ty);
            self.strings.push(s);
        }
    }
    
    fn new_string(&mut self)->String{
        if let Some(s) = self.free.pop(){
            s
        }
        else{
            String::new()
        }
    }
    
    fn free_string(&mut self, s:String){
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

    fn pop_resolved(&mut self, _vm:&ScriptVm)->(ShaderType,String){
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

    fn handle_neg(&mut self, vm:&ScriptVm, _opargs:OpcodeArgs, op:&str){
        let (t1, s1) = self.pop_resolved(vm);
        let mut s = self.stack.new_string();
        write!(s, "({}{})", op, s1).ok();
        let ty = type_table_neg(&t1, &self.trap, &vm.code.builtins.pod);
        self.stack.push(&self.trap, ty, s);
    }

    fn handle_eq(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str){
        let (t2, s2) = if opargs.is_u32(){
             let mut s = self.stack.new_string();
             write!(s, "{}", opargs.to_u32()).ok();
             (ShaderType::AbstractInt, s)
        }else{
             self.pop_resolved(vm)
        };
        let (t1, s1) = self.pop_resolved(vm);
        let mut s = self.stack.new_string();
        write!(s, "({} {} {})", s1, op, s2).ok();
        let ty = type_table_eq(&t1, &t2, &self.trap, &vm.code.builtins.pod);
        self.stack.push(&self.trap, ty, s);
    }

    fn handle_logic(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str){
        let (t2, s2) = if opargs.is_u32(){
             let mut s = self.stack.new_string();
             write!(s, "{}", opargs.to_u32()).ok();
             (ShaderType::AbstractInt, s)
        }else{
             self.pop_resolved(vm)
        };
        let (t1, s1) = self.pop_resolved(vm);
        let mut s = self.stack.new_string();
        write!(s, "({} {} {})", s1, op, s2).ok();
        let ty = type_table_logic(&t1, &t2, &self.trap, &vm.code.builtins.pod);
        self.stack.push(&self.trap, ty, s);
    }
    
    fn handle_log(&mut self, vm:&ScriptVm){
        let (ty, value_str) = self.stack.peek(&self.trap);
        let type_name = self.shader_type_to_string(vm, ty);
        if let Some(loc) = vm.code.ip_to_loc(self.trap.ip){
            log_with_level(&loc.file, loc.line, loc.col, loc.line, loc.col, format!("{}:{}", value_str, type_name), LogLevel::Log);
        }
    }
    
    fn shader_type_to_string(&self, vm:&ScriptVm, ty:&ShaderType)->String{
        match ty{
            ShaderType::None => "none".to_string(),
            ShaderType::IoSelf(_) => "io".to_string(),
            ShaderType::PodType(pod_ty) | ShaderType::Pod(pod_ty) | ShaderType::PodPtr(pod_ty) => {
                if let Some(name) = vm.heap.pod_type_name(*pod_ty){
                    name.to_string()
                }
                else{
                    format!("{:?}", pod_ty)
                }
            },
            ShaderType::Id(id) => {
                // Try to resolve the id to get its actual type
                if let Some((sc, _shadow)) = self.shader_scope.find_var(*id){
                    let pod_ty = sc.ty();
                    if let Some(name) = vm.heap.pod_type_name(pod_ty){
                        return name.to_string()
                    }
                }
                format!("id({})", id)
            },
            ShaderType::AbstractInt => "abstract_int".to_string(),
            ShaderType::AbstractFloat => "abstract_float".to_string(),
            ShaderType::Range{ty, ..} => {
                if let Some(name) = vm.heap.pod_type_name(*ty){
                    format!("range<{}>", name)
                }
                else{
                    "range".to_string()
                }
            },
            ShaderType::Error(_) => "error".to_string(),
            ShaderType::Texture(tex_type) => format!("texture({:?})", tex_type),
        }
    }

    fn handle_arithmetic(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str, is_int: bool){
        let (t2, s2) = if opargs.is_u32(){
            let mut s = self.stack.new_string();
            write!(s, "{}", opargs.to_u32()).ok();
            (ShaderType::AbstractInt, s)
        }else{
            self.pop_resolved(vm)
        };
        let (t1, s1) = self.pop_resolved(vm);
        let mut s = self.stack.new_string();
        write!(s, "({} {} {})", s1, op, s2).ok();
        let ty = if is_int {
            type_table_int_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod)
        } else {
            type_table_float_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod)
        };
        self.stack.push(&self.trap, ty, s);
    }

    fn handle_arithmetic_assign(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str, is_int: bool){
        let (t2, s2) = if opargs.is_u32(){
            let mut s = self.stack.new_string();
            write!(s, "{}", opargs.to_u32()).ok();
            (ShaderType::AbstractInt, s)
        }else{
            self.pop_resolved(vm)
        };
        let (id_ty, id_s) = self.stack.pop(&self.trap);
        if let ShaderType::Id(id) = id_ty{
            if let Some((var, shadow)) = self.shader_scope.find_var(id){
                if !matches!(var, ShaderScopeItem::Var{..}){
                    self.trap.err_let_is_immutable();
                }
                let t1 = ShaderType::Pod(var.ty());
                let _ty = if is_int {
                    type_table_int_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod)
                } else {
                    type_table_float_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod)
                };
                
                let mut s = self.stack.new_string();
                if shadow > 0 {
                    write!(s, "_s{}{}", shadow, id).ok();
                }
                else{
                    write!(s, "{}", id).ok();
                }
                write!(s, " {} {}", op, s2).ok();
                self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), s);
            }
            else{
                self.trap.err_not_found();
                self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
            }
        }
        else{
            self.trap.err_not_assignable();
            self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
        }
        self.stack.free_string(s2);
        self.stack.free_string(id_s);
    }
    
    fn handle_arithmetic_field_assign(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str, is_int: bool){
        let (t2, s2) = if opargs.is_u32(){
            let mut s = self.stack.new_string();
            write!(s, "{}", opargs.to_u32()).ok();
            (ShaderType::AbstractInt, s)
        }else{
            self.pop_resolved(vm)
        };
        
        let (field_ty, field_s) = self.stack.pop(&self.trap);
        let (instance_ty, instance_s) = self.pop_resolved(vm);
        
        if let ShaderType::Id(field_id) = field_ty {
            if let ShaderType::Pod(pod_ty) = instance_ty {
                if let Some(ret_ty) = vm.heap.pod_field_type(pod_ty, field_id, &vm.code.builtins.pod) {
                    let t1 = ShaderType::Pod(ret_ty);
                    let op_res_ty = if is_int {
                        type_table_int_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod)
                    } else {
                        type_table_float_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod)
                    };
                    
                    let val_ty = op_res_ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                    if val_ty != ret_ty{
                         self.trap.err_pod_type_not_matching();
                    }

                    let mut s = self.stack.new_string();
                    write!(s, "{0}.{1} {2} {3}", instance_s, field_id, op, s2).ok();
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), s);
                }
                else{
                    self.trap.err_not_found();
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                }
            }
            else if let ShaderType::PodPtr(pod_ty) = instance_ty {
                // Pointer type (e.g., uniform buffer in Metal) - use -> for field access
                if let Some(ret_ty) = vm.heap.pod_field_type(pod_ty, field_id, &vm.code.builtins.pod) {
                    let t1 = ShaderType::Pod(ret_ty);
                    let op_res_ty = if is_int {
                        type_table_int_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod)
                    } else {
                        type_table_float_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod)
                    };
                    
                    let val_ty = op_res_ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                    if val_ty != ret_ty{
                         self.trap.err_pod_type_not_matching();
                    }

                    let mut s = self.stack.new_string();
                    write!(s, "{0}->{1} {2} {3}", instance_s, field_id, op, s2).ok();
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), s);
                }
                else{
                    self.trap.err_not_found();
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                }
            }
            else{
                self.trap.err_no_matching_shader_type();
                self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
            }
        }
        else{
            self.trap.err_unexpected();
            self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
        }
        self.stack.free_string(s2);
        self.stack.free_string(field_s);
        self.stack.free_string(instance_s);
    }

    fn handle_arithmetic_index_assign(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str, is_int: bool){
        let (t2, s2) = if opargs.is_u32(){
            let mut s = self.stack.new_string();
            write!(s, "{}", opargs.to_u32()).ok();
            (ShaderType::AbstractInt, s)
        }else{
            self.pop_resolved(vm)
        };
        
        let (index_ty, index_s) = self.pop_resolved(vm);
        let (instance_ty, instance_s) = self.pop_resolved(vm);
        
        if let ShaderType::Pod(pod_ty) = instance_ty {
            let builtins = &vm.code.builtins.pod;
            let elem_ty = type_table_elem_type(&vm.heap.pod_types[pod_ty.index as usize].ty, &self.trap, builtins);

            if let Some(ret_ty) = elem_ty {
                match index_ty {
                    ShaderType::AbstractInt => {},
                    ShaderType::Pod(t) if t == builtins.pod_i32 || t == builtins.pod_u32 => {},
                    _ => {self.trap.err_pod_type_not_matching();} 
                }
                
                let t1 = ShaderType::Pod(ret_ty);
                let op_res_ty = if is_int {
                    type_table_int_arithmetic(&t1, &t2, &self.trap, builtins)
                } else {
                    type_table_float_arithmetic(&t1, &t2, &self.trap, builtins)
                };
                
                let val_ty = op_res_ty.make_concrete(builtins).unwrap_or(builtins.pod_void);
                if val_ty != ret_ty{
                     self.trap.err_pod_type_not_matching();
                }

                let mut s = self.stack.new_string();
                write!(s, "{}[{}] {} {}", instance_s, index_s, op, s2).ok();
                self.stack.push(&self.trap, ShaderType::Pod(builtins.pod_void), s);
            }
            else{
                self.trap.err_not_assignable();
                self.stack.push(&self.trap, ShaderType::Pod(builtins.pod_void), String::new());
            }
        }
        else{
            self.trap.err_no_matching_shader_type();
            self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
        }
        self.stack.free_string(s2);
        self.stack.free_string(index_s);
        self.stack.free_string(instance_s);
    }
    
    /// Check if we're currently in unreachable code (after a return in the current branch)
    fn is_unreachable(&self) -> bool {
        // Check if ANY IfBody in the scope chain has returned (making subsequent code unreachable)
        // or if the FnBody is fully escaped
        for me in self.mes.iter().rev() {
            match me {
                ShaderMe::IfBody { has_return, .. } => {
                    if *has_return {
                        return true;
                    }
                    // Continue checking parent scopes
                }
                ShaderMe::FnBody { escaped, .. } => {
                    return *escaped;
                }
                _ => {}
            }
        }
        false
    }
    
    /// Check if the PARENT scope is unreachable (skipping the innermost IfBody)
    /// Used for IF_ELSE to determine if the else branch should generate code
    fn is_parent_scope_unreachable(&self) -> bool {
        let mut skipped_first_if = false;
        for me in self.mes.iter().rev() {
            match me {
                ShaderMe::IfBody { has_return, .. } => {
                    if !skipped_first_if {
                        // Skip the innermost IfBody (the one we're transitioning out of)
                        skipped_first_if = true;
                        continue;
                    }
                    if *has_return {
                        return true;
                    }
                }
                ShaderMe::FnBody { escaped, .. } => {
                    return *escaped;
                }
                _ => {}
            }
        }
        false
    }
    
    fn handle_if_else_phi(&mut self, vm:&ScriptVm, output: &ShaderOutput){
        if let Some(ShaderMe::IfBody{target_ip, phi, start_pos, stack_depth, phi_type, has_return, if_branch_returned}) = self.mes.last(){
            if self.trap.ip.index >= *target_ip{
                // Check if both branches returned (escape analysis)
                let both_returned = *if_branch_returned && *has_return;
                
                if self.stack.types.len() > *stack_depth{
                    let (ty, val) = self.stack.pop(&self.trap);
                    if let Some(phi) = phi{
                        if let Some(phi_type) = phi_type{
                            self.out.push_str(&format!("{} = {};\n", phi, val));
                            // declare the phi at start
                            let ty = type_table_if_else(phi_type, &ty, &self.trap, &vm.code.builtins.pod);
                            let ty = ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                            let ty_name = if let Some(name) = vm.heap.pod_type_name(ty){
                                output.backend.map_pod_name(name)
                            }
                            else{
                                id!(unknown)
                            };
                            let mut s = self.stack.new_string();
                            write!(s, "let {phi}:{ty_name};\n").ok();                            
                            self.out.insert_str(*start_pos, &s);
                            self.stack.free_string(s);
                            let mut s = self.stack.new_string();
                            write!(s, "{}", phi).ok();
                            self.stack.push(&self.trap, ShaderType::Pod(ty), s);
                        }
                    }
                    self.stack.free_string(val);
                }
                self.out.push_str("}\n");
                self.shader_scope.exit_scope();
                self.mes.pop();
                
                // If both branches returned, propagate escape status up
                if both_returned {
                    // Find the parent and mark it as having returned/escaped
                    if let Some(parent) = self.mes.last_mut() {
                        match parent {
                            ShaderMe::IfBody { has_return, .. } => {
                                *has_return = true;
                            }
                            ShaderMe::FnBody { escaped, .. } => {
                                *escaped = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    
    fn ensure_struct_name(&self, vm: &mut ScriptVm, output: &mut ShaderOutput, pod_ty: ScriptPodType, used_name: LiveId) -> LiveId {
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

    fn handle_pod_type_call(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs, pod_ty:ScriptPodType, name:LiveId){
        if let ScriptPodTy::ArrayBuilder = &vm.heap.pod_types[pod_ty.index as usize].ty {
            self.mes.push(ShaderMe::ArrayConstruct {
                args: Vec::new(),
                elem_ty: None,
            });
            self.maybe_pop_to_me(vm, opargs);
            return;
        }

        // alright lets see what Id we got
        let _name = self.ensure_struct_name(vm, output, pod_ty, name);
        //write!(out, "{}(", name).ok();
        
        self.mes.push(ShaderMe::Pod {
            pod_ty: pod_ty,
            args: Vec::new()
        });
        
        self.maybe_pop_to_me(vm, opargs);
    }

    fn handle_call_args(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        let (ty, _s) = self.stack.pop(&self.trap);
        if let ShaderType::Id(name) = ty {
            // Check shader scope for PodType
            if let Some((ShaderScopeItem::PodType{ty, ..}, _)) = self.shader_scope.find_var(name) {
                 self.handle_pod_type_call(vm, output, opargs, *ty, name);
                 return;
            }
            
            // alright lets look it up on our script scope
            let value = vm.heap.scope_value(self.script_scope, name.into(), &self.trap);
            // lets check if our obj is a PodType
            if let Some(pod_ty) = vm.heap.pod_type(value) {
                self.handle_pod_type_call(vm, output, opargs, pod_ty, name);
                return;
            }
            
            if let Some(fnobj) = value.as_object() {
                if let Some(fnptr) = vm.heap.as_fn(fnobj) {
                    match fnptr {
                        // another script fn
                        ScriptFnPtr::Script(_fnptr) => {
                            let mut out = self.stack.new_string();
                            write!(out, "{}", output.backend.get_io_all(output.mode)).ok();
                            self.mes.push(ShaderMe::ScriptCall {
                                name,
                                out,
                                fnobj,
                                sself: ShaderType::None,
                                args: Default::default(),
                            });
                        }
                        // builtin shader fns
                        ScriptFnPtr::Native(fnptr) => {
                            self.mes.push(ShaderMe::BuiltinCall {
                                name,
                                fnptr,
                                args: Default::default()
                            });
                            self.maybe_pop_to_me(vm, opargs);
                            return;
                        }
                    }
                    
                    self.maybe_pop_to_me(vm, opargs);
                    return;
                }
            }
        }
        self.trap.err_not_fn();
    }

    fn handle_array_construct(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, args: Vec<String>, elem_ty: Option<ScriptPodType>) {
        let elem_ty = elem_ty.unwrap_or(vm.code.builtins.pod.pod_f32);
        let count = args.len();
        
        let elem_data = vm.heap.pod_types[elem_ty.index as usize].clone();
        let elem_inline = ScriptPodTypeInline{
            self_ref: elem_ty,
            data: elem_data
        };
        
        let align_of = elem_inline.data.ty.align_of();
        let raw_size = elem_inline.data.ty.size_of();
        let stride = if raw_size % align_of != 0 { raw_size + (align_of - (raw_size % align_of)) } else { raw_size };
        let total_size = stride * count;
        
        let array_ty = vm.heap.new_pod_array_type(ScriptPodTy::FixedArray{
            align_of,
            size_of: total_size,
            len: count,
            ty: Box::new(elem_inline)
        }, NIL);
        
        let mut out = self.stack.new_string();
        
        if let Some(name) = vm.heap.pod_type_name(elem_ty) {
             if matches!(vm.heap.pod_types[elem_ty.index as usize].ty, ScriptPodTy::Struct{..}) {
                 output.structs.insert(elem_ty);
             }
             match output.backend{
                 ShaderBackend::Wgsl=>{
                     let name = output.backend.map_pod_name(name);
                     write!(out, "array<{}, {}>", name, count).ok();
                     write!(out, "(").ok();
                 }
                 ShaderBackend::Metal | ShaderBackend::Hlsl =>{
                     write!(out, "{{").ok();
                 }
                 ShaderBackend::Glsl=>{
                      let name = output.backend.map_pod_name(name);
                      write!(out, "{}[{}]", name, count).ok(); // array constructor
                      write!(out, "(").ok();
                 }
             }
        }
        else {
            self.trap.err_no_matching_shader_type();
            match output.backend{
                 ShaderBackend::Wgsl=>{
                     write!(out, "(").ok();
                 }
                 ShaderBackend::Metal | ShaderBackend::Hlsl =>{
                     write!(out, "{{").ok();
                 }
                 ShaderBackend::Glsl=>{
                     write!(out, "(").ok(); // Should not happen if type not found
                 }
             }
        }
        
        for (i, s) in args.iter().enumerate() {
            if i > 0 { out.push_str(", "); }
            out.push_str(s);
        }
        
        match output.backend{
             ShaderBackend::Wgsl | ShaderBackend::Glsl =>{
                 out.push_str(")");
             }
             ShaderBackend::Metal | ShaderBackend::Hlsl =>{
                 out.push_str("}");
             }
        }
        
        for s in args {
            self.stack.free_string(s);
        }
        
        self.stack.push(&self.trap, ShaderType::Pod(array_ty), out);
    }

    fn handle_pod_construct(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, pod_ty: ScriptPodType, args: Vec<ShaderPodArg>) {
         let mut offset = ScriptPodOffset::default();
         let pod_ty_data = &vm.heap.pod_types[pod_ty.index as usize];

         let mut out = self.stack.new_string();
         if let Some(name) = vm.heap.pod_type_name(pod_ty) {
             let name = output.backend.map_pod_name(name);
             match output.backend{
                 ShaderBackend::Wgsl=>{
                    write!(out, "{}(", name).ok();
                 }
                 ShaderBackend::Metal | ShaderBackend::Hlsl =>{
                    if let ScriptPodTy::Struct{..} = &pod_ty_data.ty{
                        write!(out, "{{").ok();
                    }
                    else{
                        write!(out, "{}(", name).ok();
                    }
                 }
                 ShaderBackend::Glsl =>{
                    write!(out, "{}(", name).ok();
                 }
             }
         }
         else {
             self.trap.err_no_matching_shader_type();
         }
         
         if let Some(first) = args.first(){
             if first.name.is_some(){ // Named args
                  if let ScriptPodTy::Struct{fields, ..} = &pod_ty_data.ty {
                       for (i, field) in fields.iter().enumerate(){
                           if i > 0 { out.push_str(", "); }
                           
                           // Find the arg with sself name
                           if let Some(arg) = args.iter().find(|a| a.name.unwrap() == field.name) {
                                // Check type
                                match &arg.ty{
                                    ShaderType::Pod(arg_pod_ty) => {
                                         if *arg_pod_ty != field.ty.self_ref {
                                              self.trap.err_pod_type_not_matching();
                                         }
                                    },
                                    ShaderType::Id(id) => {
                                         if let Some((v, _name)) = self.shader_scope.find_var(*id){
                                              if v.ty() != field.ty.self_ref {
                                                   self.trap.err_pod_type_not_matching();
                                              }
                                         }
                                         else{
                                              self.trap.err_not_found();
                                         }
                                    },
                                    ShaderType::AbstractInt => {
                                         let builtins = &vm.code.builtins.pod;
                                         if field.ty.self_ref != builtins.pod_i32 && field.ty.self_ref != builtins.pod_u32 && field.ty.self_ref != builtins.pod_f32 {
                                              self.trap.err_pod_type_not_matching();
                                         }
                                    },
                                    ShaderType::AbstractFloat => {
                                          let builtins = &vm.code.builtins.pod;
                                          if field.ty.self_ref != builtins.pod_f32 {
                                               self.trap.err_pod_type_not_matching();
                                          }
                                    },
                                     _ => {}
                                }
                                out.push_str(&arg.s);
                           }
                           else {
                                self.trap.err_invalid_constructor_arg();
                           }
                       }
                       
                       if args.len() != fields.len() {
                            self.trap.err_invalid_arg_count();
                       }
                  }
                  else {
                      self.trap.err_unexpected();
                  }
             }
             else { // Positional args
                  for (i, arg) in args.iter().enumerate() {
                       if i > 0 { out.push_str(", "); }
                       match &arg.ty{
                            ShaderType::Pod(pod_ty_field) | ShaderType::PodPtr(pod_ty_field)=>{
                                vm.heap.pod_check_constructor_arg(pod_ty, *pod_ty_field, &mut offset, &self.trap);
                            }
                            ShaderType::Id(id)=>{
                                if let Some((v, _name)) = self.shader_scope.find_var(*id){
                                    vm.heap.pod_check_constructor_arg(pod_ty, v.ty(), &mut offset, &self.trap);
                                }
                                else{
                                    self.trap.err_not_found();
                                }
                            }
                            ShaderType::AbstractInt | ShaderType::AbstractFloat=>{
                                vm.heap.pod_check_abstract_constructor_arg(pod_ty, &mut offset, &self.trap);
                            }
                            ShaderType::None|ShaderType::Range{..}|ShaderType::Error(_)|ShaderType::IoSelf(_)|ShaderType::PodType(_)|ShaderType::Texture(_)=>{}
                        }
                        out.push_str(&arg.s);
                  }
                  vm.heap.pod_check_constructor_arg_count(pod_ty, &offset, &self.trap);
             }
         }
         else {
              vm.heap.pod_check_constructor_arg_count(pod_ty, &offset, &self.trap);
         }
         
         match output.backend{
             ShaderBackend::Wgsl=>{
                out.push_str(")");
             }
             ShaderBackend::Metal | ShaderBackend::Hlsl =>{
                if let ScriptPodTy::Struct{..} = &pod_ty_data.ty{
                    out.push_str("}");
                }
                else{
                    out.push_str(")");
                }
             }
             ShaderBackend::Glsl =>{
                 out.push_str(")");
             }
         }
         
         for arg in args {
             self.stack.free_string(arg.s);
         }
         
         self.stack.push(&self.trap, ShaderType::Pod(pod_ty), out);
    }

    pub fn compile_shader_def(vm: &mut ScriptVm, output: &mut ShaderOutput, name: LiveId, fnobj: ScriptObject, sself: ShaderType, args: Vec<ShaderType>) -> (ScriptPodType, String) {
        let mut method_name_prefix = String::new();
        if let ShaderType::PodType(ty) = sself{
            if let Some(name) = vm.heap.pod_type_name(ty) {
                write!(method_name_prefix, "{}_", name).ok();
            } 
        }
        else if let ShaderType::Pod(ty) = sself {
            if let Some(name) = vm.heap.pod_type_name(ty) {
                write!(method_name_prefix, "{}_", name).ok();
            } 
        }
        else if let ShaderType::IoSelf(_) = sself {
            write!(method_name_prefix, "io_").ok();
        }
        
        // First pass: resolve AbstractInt/AbstractFloat against declared parameter types
        let builtins = &vm.code.builtins.pod;
        let argc = vm.heap.vec_len(fnobj);
        let mut resolved_args: Vec<ScriptPodType> = Vec::new();
        let mut argi = 0;
        for i in 0..argc {
            let kv = vm.heap.vec_key_value(fnobj, i, &vm.thread.trap);
            if kv.key == id!(self).into() {
                continue;
            }
            if argi >= args.len() {
                break;
            }
            let arg = &args[argi];
            // Get declared parameter type from kv.value
            // Try both direct pod_type value and object-based pod_type
            let declared_ty = kv.value.as_pod_type().or_else(|| vm.heap.pod_type(kv.value));
            
            let resolved = match arg {
                ShaderType::AbstractInt | ShaderType::AbstractFloat => {
                    // Use declared type if available, otherwise fall back to default
                    if let Some(declared) = declared_ty {
                        declared
                    } else {
                        arg.make_concrete(builtins).unwrap_or(builtins.pod_void)
                    }
                }
                _ => arg.make_concrete(builtins).unwrap_or(builtins.pod_void)
            };
            resolved_args.push(resolved);
            argi += 1;
        }
        
        // lets see if we already have fnobj with our argstypes
        if let Some(fun) = output.functions.iter().find(|v| {
            v.fnobj == fnobj && v.args == resolved_args
        }) {
            let mut fn_name = String::new();
            if fun.overload != 0 {
                write!(fn_name, "_f{}{}{}", fun.overload, method_name_prefix, name).ok();
            }
            else{
                write!(fn_name, "{}{}", method_name_prefix, name).ok();
            }
            write!(fn_name, "(").ok(); // Add opening paren to match new function path
            return (fun.ret, fn_name);
        } 
        
        let overload = output.functions.iter().filter(|v| { v.name == name }).count();
        
        let mut compiler = ShaderFnCompiler::new(fnobj);
        let mut call_sig = String::new();
            
        let mut fn_name = String::new();
        let mut fn_args = String::new();
            
        if overload != 0 {
            write!(fn_name, "_f{}{}{}", overload, method_name_prefix, name).ok();
        } else {
            write!(fn_name, "{}{}", method_name_prefix, name).ok();
        }
        
        let mut has_self = false;
        write!(fn_args, "{}", output.backend.get_io_all_decl(output.mode)).ok();
        if let ShaderType::Pod(ty) = sself {
            has_self = true;
            match output.backend {
                ShaderBackend::Wgsl => {
                    if fn_args.len()>0{write!(fn_args,", ").ok();}
                    write!(fn_args, "_self:ptr<function, ").ok();
                    if let Some(name) = vm.heap.pod_type_name(ty) {
                        let name = output.backend.map_pod_name(name);
                        write!(fn_args, "{}", name).ok();
                    }
                    write!(fn_args, ">").ok();
                }
                ShaderBackend::Metal => {
                    if let Some(name) = vm.heap.pod_type_name(ty) {
                        let name = output.backend.map_pod_name(name);
                        if fn_args.len()>0{write!(fn_args,", ").ok();}
                        write!(fn_args, "thread {}& _self", name).ok();
                    }
                }
                ShaderBackend::Hlsl => {
                    if let Some(name) = vm.heap.pod_type_name(ty) {
                        let name = output.backend.map_pod_name(name);
                        if fn_args.len()>0{write!(fn_args,", ").ok();}
                        write!(fn_args, "inout {} _self", name).ok();
                    }
                }
                ShaderBackend::Glsl => {
                        if let Some(name) = vm.heap.pod_type_name(ty) {
                        let name = output.backend.map_pod_name(name);
                        if fn_args.len()>0{write!(fn_args,", ").ok();}
                        write!(fn_args, "inout {} _self", name).ok();
                    }
                }
            }
            compiler.shader_scope.define_let(id!(self), ty);
        }
        else if let ShaderType::PodType(ty) = sself{
            compiler.shader_scope.define_pod_type(id!(self), ty);
        }
        else if let ShaderType::IoSelf(obj) = sself{
            if fn_args.len()>0{write!(fn_args, ", ").ok();}
            write!(fn_args, "{}", output.backend.get_io_self_decl(output.mode)).ok();
            compiler.shader_scope.define_io_self(obj);
        }
        
        let argc = vm.heap.vec_len(fnobj);
        let mut argi = 0;
        for i in 0..argc {
            let kv = vm.heap.vec_key_value(fnobj, i, &vm.thread.trap);
            
            if kv.key == id!(self).into() {
                if !has_self || argi != 0{
                    vm.thread.trap.err_invalid_arg_name();
                }
                continue;
            }
            
            if let Some(id) = kv.key.as_id() {
                if fn_args.len()>0{write!(fn_args,", ").ok();}
                if argi >= resolved_args.len(){
                    vm.thread.trap.err_invalid_arg_count();
                    break;
                }
                let arg_ty = resolved_args[argi];
                    
                match output.backend {
                    ShaderBackend::Wgsl => {
                        write!(fn_args, "{}:", id).ok();
                        if let Some(name) = vm.heap.pod_type_name(arg_ty) {
                            let name = output.backend.map_pod_name(name);
                            write!(fn_args, "{}", name).ok();
                        } else {
                             // todo!()
                        }
                    }
                    ShaderBackend::Metal | ShaderBackend::Hlsl | ShaderBackend::Glsl => {
                        if let Some(name) = vm.heap.pod_type_name(arg_ty) {
                            let name = output.backend.map_pod_name(name);
                            write!(fn_args, "{} {}", name, id).ok();
                        } else {
                             // todo!()
                        }
                    }
                }
                compiler.shader_scope.define_let(id, arg_ty);
            }
            argi += 1;
        }
        if argi < resolved_args.len(){
            vm.thread.trap.err_invalid_arg_count();
        }
            
        if let Some(fnptr) = vm.heap.as_fn(fnobj) {
            if let ScriptFnPtr::Script(fnip) = fnptr {
                if output.recur_block.iter().any(|v| *v == fnobj) {
                    vm.thread.trap.err_recursion_not_allowed();
                    (vm.code.builtins.pod.pod_void, fn_name)
                } else {
                    output.recur_block.push(fnobj);
                    let ret = compiler.compile_fn(vm, output, fnip);
                    output.recur_block.pop();
                        
                    match output.backend {
                        ShaderBackend::Wgsl => {
                            write!(call_sig, "fn {}({})", fn_name, fn_args).ok();
                            if let Some(name) = vm.heap.pod_type_name(ret) {
                                if name != id!(void){
                                    let name = output.backend.map_pod_name(name);
                                    write!(call_sig, "->{}", name).ok();
                                }
                            }
                        }
                        ShaderBackend::Metal | ShaderBackend::Hlsl | ShaderBackend::Glsl => {
                            let ret_name = if let Some(name) = vm.heap.pod_type_name(ret) {
                                output.backend.map_pod_name(name)
                            } else {
                                id!(void)
                            };
                            write!(call_sig, "{} {}({})", ret_name, fn_name, fn_args).ok();
                        }
                    }
                        
                    output.functions.push(ShaderFn {
                        overload,
                        call_sig,
                        name,
                        args: resolved_args,
                        fnobj,
                        out: compiler.out,
                        ret
                    });
                    write!(fn_name,"(").ok();
                    (ret, fn_name)
                }
            } else { panic!() }
        } else { panic!() }
    }

    fn handle_script_call(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, mut out: String, name: LiveId, fnobj: ScriptObject, sself: ShaderType, args: Vec<ShaderType>) {
        // we should compare number of arguments (needs to be exact)
        // Note: fn_name already includes "(" at the end from compile_shader_def
        let (ret, fn_name) = Self::compile_shader_def(vm, output, name, fnobj, sself, args);
        out.insert_str(0, &fn_name);
        out.push_str(")");
        self.stack.push(&self.trap, ShaderType::Pod(ret), out);
    }


    fn handle_call_exec(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        if let Some(me) = self.mes.pop() {
            match me {
                ShaderMe::ArrayConstruct { args, elem_ty } => {
                    self.handle_array_construct(vm, output, args, elem_ty);
                }
                ShaderMe::Pod { pod_ty, args } => {
                    self.handle_pod_construct(vm, output, pod_ty, args);
                }
                ShaderMe::ScriptCall { out, name, fnobj, sself, args } => {
                    self.handle_script_call(vm, output, out, name, fnobj, sself, args);
                }
                ShaderMe::TextureBuiltin { method_id, tex_type: _, texture_expr, args } => {
                    self.handle_texture_builtin_exec(vm, output, method_id, texture_expr, args);
                }
                ShaderMe::BuiltinCall { name, fnptr: _, args } => {
                    let builtins = &vm.code.builtins.pod;
                    
                    // Helper to check if a pod type is float-based
                    let is_float_type = |pt: ScriptPodType| -> bool {
                        let pod_ty = &vm.heap.pod_types[pt.index as usize];
                        match &pod_ty.ty {
                            ScriptPodTy::F32 | ScriptPodTy::F16 => true,
                            ScriptPodTy::Vec(v) => matches!(v.elem_ty(), ScriptPodTy::F32 | ScriptPodTy::F16),
                            ScriptPodTy::Mat(_) => true, // Matrices are float-based
                            _ => false
                        }
                    };
                    
                    // Check if any arg is a float type - if so, abstract ints should be floats
                    let has_float = args.iter().any(|(ty, _)| {
                        match ty {
                            ShaderType::Pod(pt) => is_float_type(*pt),
                            ShaderType::AbstractFloat => true,
                            _ => false
                        }
                    });
                    
                    // Build concrete args for type_table_builtin and format output
                    let mut concrete_args = Vec::new();
                    let mut out = self.stack.new_string();
                    let mapped_name = output.backend.map_builtin_name(name);
                    write!(out, "{}(", mapped_name).ok();
                    
                    for (i, (ty, s)) in args.into_iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        
                        match &ty {
                            ShaderType::AbstractInt | ShaderType::AbstractFloat => {
                                if has_float {
                                    // Format as float literal
                                    concrete_args.push(builtins.pod_f32);
                                    // Check if s is a simple integer that needs .0 suffix
                                    if s.chars().all(|c| c.is_ascii_digit() || c == '-') {
                                        out.push_str(&s);
                                        out.push_str(".0");
                                    } else {
                                        out.push_str(&s);
                                    }
                                } else {
                                    concrete_args.push(ty.make_concrete(builtins).unwrap_or(builtins.pod_void));
                                    out.push_str(&s);
                                }
                            }
                            ShaderType::Pod(pt) => {
                                concrete_args.push(*pt);
                                out.push_str(&s);
                            }
                            _ => {
                                concrete_args.push(ty.make_concrete(builtins).unwrap_or(builtins.pod_void));
                                out.push_str(&s);
                            }
                        }
                        self.stack.free_string(s);
                    }
                    
                    out.push_str(")");
                    let ret = type_table_builtin(name, &concrete_args, builtins, &self.trap);
                    self.stack.push(&self.trap, ShaderType::Pod(ret), out);
                }
                _ => { self.trap.err_not_impl(); }
            }
        }
    }

    fn handle_texture_builtin_exec(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        method_id: LiveId,
        texture_expr: String,
        args: Vec<String>,
    ) {
        // Handle texture methods - these are virtual methods that transpile to backend-specific code
        match method_id {
            id!(size) => {
                // size() returns vec2f with the texture dimensions
                let mut s = self.stack.new_string();
                match output.backend {
                    ShaderBackend::Metal => {
                        // Metal: float2(texture.get_width(), texture.get_height())
                        write!(s, "float2({}.get_width(), {}.get_height())", texture_expr, texture_expr).ok();
                    }
                    ShaderBackend::Wgsl => {
                        // WGSL: textureDimensions(texture) - returns vec2<u32>, cast to vec2<f32>
                        write!(s, "vec2f(0.0, 0.0)").ok(); // Placeholder
                    }
                    ShaderBackend::Hlsl => {
                        // HLSL: texture.GetDimensions() 
                        write!(s, "float2(0.0, 0.0)").ok(); // Placeholder
                    }
                    ShaderBackend::Glsl => {
                        // GLSL: textureSize(texture, 0)
                        write!(s, "vec2(0.0, 0.0)").ok(); // Placeholder
                    }
                }
                self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_vec2f), s);
            }
            id!(sample_2d) => {
                // sample_2d(coord) samples the texture at normalized coordinates
                // Returns vec4f
                if args.len() != 1 {
                    self.trap.err_invalid_arg_count();
                    let empty = self.stack.new_string();
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_vec4f), empty);
                } else {
                    let coord = &args[0];
                    let mut s = self.stack.new_string();
                    
                    // Get or create the default sampler (linear, repeat, normalized)
                    let sampler = ShaderSampler::default();
                    let sampler_idx = output.get_or_create_sampler(sampler);
                    
                    match output.backend {
                        ShaderBackend::Metal => {
                            // Metal: texture.sample(sampler, coord)
                            write!(s, "{}.sample(_s{}, {})", texture_expr, sampler_idx, coord).ok();
                        }
                        ShaderBackend::Wgsl => {
                            // WGSL: textureSample(texture, sampler, coord)
                            write!(s, "vec4f(0.0, 0.0, 0.0, 0.0)").ok(); // Placeholder
                        }
                        ShaderBackend::Hlsl => {
                            // HLSL: texture.Sample(sampler, coord)
                            write!(s, "float4(0.0, 0.0, 0.0, 0.0)").ok(); // Placeholder
                        }
                        ShaderBackend::Glsl => {
                            // GLSL: texture(sampler2D, coord)
                            write!(s, "vec4(0.0, 0.0, 0.0, 0.0)").ok(); // Placeholder
                        }
                    }
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_vec4f), s);
                }
            }
            _ => {
                self.trap.err_not_found();
            }
        }
        self.stack.free_string(texture_expr);
        for arg in args {
            self.stack.free_string(arg);
        }
    }

    fn handle_method_call_args(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        let (method_ty, method_s) = self.stack.pop(&self.trap);
        let (self_ty, self_s) = self.stack.pop(&self.trap);
        self.stack.free_string(method_s);
        
        if let ShaderType::Id(method_id) = method_ty {
            // Handle method calls on Texture types (e.g., texture.size())
            if let ShaderType::Texture(tex_type) = self_ty {
                self.handle_texture_method_call_args(vm, output, opargs, method_id, tex_type, self_s);
                return;
            }
            
            if let ShaderType::Id(self_id) = self_ty {
                // Try to resolve as variable on shader scope - extract info before mutable borrows
                let scope_info = self.shader_scope.find_var(self_id).map(|(var, _)| {
                    match var {
                        ShaderScopeItem::IoSelf(obj) => (Some(*obj), None),
                        _ => (None, Some(var.ty())),
                    }
                });
                
                if let Some((io_self_obj, pod_ty_opt)) = scope_info {
                    // Method call on IoSelf
                    if let Some(obj) = io_self_obj {
                        if self.handle_io_self_method_call_args(vm, output, opargs, method_id, obj, &self_s) {
                            self.stack.free_string(self_s);
                            return;
                        }
                    }
                    
                    // Method call on a Pod instance
                    if let Some(pod_ty) = pod_ty_opt {
                        let self_s_slice = if self_id == id!(self) { "_self" } else { &self_s };
                        if self.handle_pod_method_call_args(vm, output, opargs, method_id, pod_ty, self_s_slice, &self_s) {
                            return;
                        }
                    }
                }
                else {
                    // Try to resolve as PodType static method in script scope
                    if self.handle_pod_type_method_call_args(vm, output, opargs, method_id, self_id, &self_s) {
                        return;
                    }
                }
            }
        }
        self.stack.free_string(self_s);
        self.trap.err_not_impl();
    }
    
    fn handle_io_self_method_call_args(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        method_id: LiveId,
        obj: ScriptObject,
        _self_s: &str,
    ) -> bool {
        let fnobj = vm.heap.value(obj, method_id.into(), &self.trap);
        if let Some(fnobj) = fnobj.as_object() {
            if let Some(fnptr) = vm.heap.as_fn(fnobj) {
                match fnptr {
                    ScriptFnPtr::Script(_fnptr) => {
                        let mut out = self.stack.new_string();
                        write!(out, "{}", output.backend.get_io_all(output.mode)).ok();
                        if out.len() > 0 { write!(out, ", ").ok(); }
                        write!(out, "{}", output.backend.get_io_self(output.mode)).ok();
                        self.mes.push(ShaderMe::ScriptCall {
                            name: method_id,
                            out,
                            fnobj,
                            sself: ShaderType::IoSelf(obj),
                            args: vec![],
                        });
                    }
                    ScriptFnPtr::Native(_) => { todo!() }
                }
                self.maybe_pop_to_me(vm, opargs);
                return true;
            }
        }
        false
    }
    
    fn handle_pod_method_call_args(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        method_id: LiveId,
        pod_ty: ScriptPodType,
        self_s_slice: &str,
        self_s: &String,
    ) -> bool {
        let pod_ty_data = &vm.heap.pod_types[pod_ty.index as usize];
        let fnobj = vm.heap.value(pod_ty_data.object, method_id.into(), &self.trap);
        
        if let Some(fnobj) = fnobj.as_object() {
            if let Some(fnptr) = vm.heap.as_fn(fnobj) {
                match fnptr {
                    ScriptFnPtr::Script(_fnptr) => {
                        let mut out = self.stack.new_string();
                        write!(out, "{}", output.backend.get_io_all(output.mode)).ok();
                        match output.backend {
                            ShaderBackend::Wgsl => {
                                if out.len() > 0 { write!(out, ", ").ok(); }
                                write!(out, "&{}", self_s_slice).ok();
                            }
                            ShaderBackend::Metal => {
                                // Metal uses references (thread T&), not pointers
                                // Pass the variable directly without &
                                if out.len() > 0 { write!(out, ", ").ok(); }
                                write!(out, "{}", self_s_slice).ok();
                            }
                            ShaderBackend::Hlsl | ShaderBackend::Glsl => {
                                if out.len() > 0 { write!(out, ", ").ok(); }
                                write!(out, "{}", self_s_slice).ok();
                            }
                        }
                        self.mes.push(ShaderMe::ScriptCall {
                            name: method_id,
                            out,
                            fnobj,
                            sself: ShaderType::Pod(pod_ty),
                            args: vec![],
                        });
                    }
                    ScriptFnPtr::Native(fnptr) => {
                        // Store self as first argument
                        let mut self_arg = self.stack.new_string();
                        write!(self_arg, "{}", self_s_slice).ok();
                        self.mes.push(ShaderMe::BuiltinCall {
                            name: method_id,
                            fnptr,
                            args: vec![(ShaderType::Pod(pod_ty), self_arg)]
                        });
                    }
                }
                self.stack.free_string(self_s.clone());
                self.maybe_pop_to_me(vm, opargs);
                return true;
            }
        }
        false
    }
    
    fn handle_pod_type_method_call_args(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        method_id: LiveId,
        self_id: LiveId,
        self_s: &String,
    ) -> bool {
        let value = vm.heap.scope_value(self.script_scope, self_id.into(), &self.trap);
        if let Some(pod_ty) = vm.heap.pod_type(value) {
            self.ensure_struct_name(vm, output, pod_ty, self_id);
            // It is a PodType. Look up static method.
            let pod_ty_data = &vm.heap.pod_types[pod_ty.index as usize];
            let fnobj = vm.heap.value(pod_ty_data.object, method_id.into(), &self.trap);
            
            if let Some(fnobj) = fnobj.as_object() {
                if let Some(fnptr) = vm.heap.as_fn(fnobj) {
                    match fnptr {
                        ScriptFnPtr::Script(_fnptr) => {
                            let mut out = self.stack.new_string();
                            write!(out, "{}", output.backend.get_io_all(output.mode)).ok();
                            self.mes.push(ShaderMe::ScriptCall {
                                name: method_id,
                                out,
                                fnobj,
                                sself: ShaderType::PodType(pod_ty),
                                args: Default::default(),
                            });
                        }
                        ScriptFnPtr::Native(fnptr) => {
                            self.mes.push(ShaderMe::BuiltinCall {
                                name: method_id,
                                fnptr,
                                args: Default::default()
                            });
                        }
                    }
                    self.stack.free_string(self_s.clone());
                    self.maybe_pop_to_me(vm, opargs);
                    return true;
                }
            }
        }
        false
    }
    
    fn handle_texture_method_call_args(
        &mut self, 
        _vm: &mut ScriptVm, 
        _output: &mut ShaderOutput, 
        _opargs: OpcodeArgs,
        method_id: LiveId, 
        tex_type: TextureType,
        texture_expr: String
    ) {
        // Push TextureBuiltin to collect arguments - actual code gen happens in handle_call_exec
        self.mes.push(ShaderMe::TextureBuiltin {
            method_id,
            tex_type,
            texture_expr,
            args: vec![],
        });
    }
    
    fn handle_assign(&mut self, vm: &mut ScriptVm) {
        let (_value_ty, value) = self.stack.pop(&self.trap);
        let (id_ty, _id) = self.stack.pop(&self.trap);
        if let ShaderType::Id(id) = id_ty {
            if let Some((var, shadow)) = self.shader_scope.find_var(id) {
                if !matches!(var, ShaderScopeItem::Var{..}) {
                    self.trap.err_let_is_immutable();
                }
                let mut s = self.stack.new_string();
                if shadow > 0 {
                    write!(s, "_s{}{}", shadow, id).ok();
                }
                else{
                    write!(s, "{}", id).ok();
                }
                write!(s, " = {}", value).ok();
                self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), s);
            } else {
                self.trap.err_not_found();
                self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
            }
        } else {
            self.trap.err_not_assignable();
            self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
        }
        self.stack.free_string(value);
    }

    fn handle_assign_field(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        
        let (value_ty, value_s) = self.pop_resolved(vm);
        let (field_ty, field_s) = self.stack.pop(&self.trap);
        let (instance_ty, instance_s) = self.pop_resolved(vm);

        if let ShaderType::Id(field_id) = field_ty {
            if let ShaderType::Pod(pod_ty) = instance_ty {
                if let Some(ret_ty) = vm.heap.pod_field_type(pod_ty, field_id, &vm.code.builtins.pod) {
                    let val_ty = value_ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                    if val_ty != ret_ty {
                        self.trap.err_pod_type_not_matching();
                    }

                    let mut s = self.stack.new_string();
                    write!(s, "{}.{} = {}", instance_s, field_id, value_s).ok();
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), s);
                } else {
                    self.trap.err_not_found();
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                }
            }
            else if let ShaderType::PodPtr(pod_ty) = instance_ty {
                // Pointer type (e.g., uniform buffer in Metal) - use -> for field access
                if let Some(ret_ty) = vm.heap.pod_field_type(pod_ty, field_id, &vm.code.builtins.pod) {
                    let val_ty = value_ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                    if val_ty != ret_ty {
                        self.trap.err_pod_type_not_matching();
                    }

                    let mut s = self.stack.new_string();
                    write!(s, "{}->{} = {}", instance_s, field_id, value_s).ok();
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), s);
                } else {
                    self.trap.err_not_found();
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                }
            }
            else if let ShaderType::IoSelf(obj) = instance_ty{
                let value = vm.heap.value(obj, field_id.into(), &self.trap);
                if let Some(value_obj) = value.as_object(){
                    if let Some(io_type) = vm.heap.as_shader_io(value_obj) {
                                                
                        let allowed = match io_type {
                            SHADER_IO_VARYING => output.mode == ShaderMode::Vertex,
                            SHADER_IO_VERTEX_POSITION => output.mode == ShaderMode::Vertex,
                            io_type if io_type.0 >= SHADER_IO_FRAGMENT_OUTPUT_0.0 && io_type.0 <= SHADER_IO_FRAGMENT_OUTPUT_MAX.0 => output.mode == ShaderMode::Fragment,
                            _ => false
                        };
                        
                        if !allowed {
                            self.trap.err_assign_not_allowed();
                            self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
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
                             _ => None
                         };
                                                  
                         if let Some(pod_ty) = concrete_ty {
                             let val_ty = value_ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                             if val_ty != pod_ty {
                                 self.trap.err_pod_type_not_matching();
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
                             self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), s);
                             self.stack.free_string(field_s);
                             self.stack.free_string(instance_s);
                             self.stack.free_string(value_s);
                             return
                         }
                    }
                }
                self.trap.err_no_matching_shader_type();
                self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
            }
            else {
                self.trap.err_no_matching_shader_type();
                self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
            }
        } else {
            self.trap.err_unexpected();
            self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
        }
        self.stack.free_string(value_s);
        self.stack.free_string(field_s);
        self.stack.free_string(instance_s);
    }

    fn handle_assign_index(&mut self, vm: &mut ScriptVm) {
        let (value_ty, value_s) = self.pop_resolved(vm);
        let (index_ty, index_s) = self.pop_resolved(vm);
        let (instance_ty, instance_s) = self.pop_resolved(vm);

        if let ShaderType::Pod(pod_ty) = instance_ty {
            let builtins = &vm.code.builtins.pod;
            let elem_ty = type_table_elem_type(&vm.heap.pod_types[pod_ty.index as usize].ty, &self.trap, builtins);

            if let Some(ret_ty) = elem_ty {
                match index_ty {
                    ShaderType::AbstractInt => {}
                    ShaderType::Pod(t) if t == builtins.pod_i32 || t == builtins.pod_u32 => {}
                    _ => {
                        self.trap.err_pod_type_not_matching();
                    }
                }

                let val_ty = value_ty.make_concrete(builtins).unwrap_or(builtins.pod_void);
                if val_ty != ret_ty {
                    self.trap.err_pod_type_not_matching();
                }

                let mut s = self.stack.new_string();
                write!(s, "{}[{}] = {}", instance_s, index_s, value_s).ok();
                self.stack.push(&self.trap, ShaderType::Pod(builtins.pod_void), s);
            } else {
                self.trap.err_not_assignable();
                self.stack.push(&self.trap, ShaderType::Pod(builtins.pod_void), String::new());
            }
        } else {
            self.trap.err_no_matching_shader_type();
            self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
        }
        self.stack.free_string(value_s);
        self.stack.free_string(index_s);
        self.stack.free_string(instance_s);
    }

    fn handle_assign_me(&mut self, vm: &mut ScriptVm) {
        let (val_ty, val_s) = self.stack.pop(&self.trap);
        let (id_ty, id_s) = self.stack.pop(&self.trap);
        if let ShaderType::Id(id) = id_ty {
            if let Some(ShaderMe::Pod { args, .. }) = self.mes.last_mut() {
                if let Some(last) = args.last() {
                    if last.name.is_none() {
                        self.trap.err_use_only_named_or_ordered_pod_fields();
                    }
                }
                args.push(ShaderPodArg {
                    name: Some(id),
                    ty: val_ty,
                    s: val_s,
                });
            } else {
                self.trap.err_unexpected();
                self.stack.free_string(val_s);
            }
            self.stack.free_string(id_s);
        } else {
            self.trap.err_unexpected();
            self.stack.free_string(val_s);
            self.stack.free_string(id_s);
            self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
        }
    }

    fn handle_return(&mut self, vm: &mut ScriptVm, opargs: OpcodeArgs) {
        // Check if we're already escaped (all code paths have returned)
        let already_escaped = self.mes.iter().rev()
            .find_map(|me| match me {
                ShaderMe::FnBody { escaped, .. } => Some(*escaped),
                _ => None,
            })
            .unwrap_or(false);
        
        if already_escaped {
            // Still need to consume the stack value if present
            if !opargs.is_nil() {
                let (_ty, s) = self.stack.pop(&self.trap);
                self.stack.free_string(s);
            }
            return;
        }
        
        // Check if we're inside an IfBody before taking mutable borrow
        let inside_if = self.mes.iter().any(|me| matches!(me, ShaderMe::IfBody { .. }));
        
        // Find our FnBody to record return type
        if let Some(me) = self.mes.iter_mut().rev().find(|v| matches!(v, ShaderMe::FnBody { .. })) {
            if let ShaderMe::FnBody { ret, escaped } = me {
                // we can also return a void
                let (ty, s) = if opargs.is_nil() {
                    (vm.code.builtins.pod.pod_void, self.stack.new_string())
                } else {
                    let (ty, s) = self.stack.pop(&self.trap);
                    let ty = ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                    (ty, s)
                };
                if let Some(ret) = ret {
                    if ty != *ret {
                        self.trap.err_return_type_changed();
                    }
                }
                *ret = Some(ty);

                if ty == vm.code.builtins.pod.pod_void {
                    self.out.push_str(&s);
                    self.out.push_str(";\nreturn;\n");
                } else {
                    self.out.push_str("return ");
                    self.out.push_str(&s);
                    self.out.push_str(";\n");
                }

                self.stack.free_string(s);
                
                // If not inside an IfBody (return at function level), mark function as escaped
                if !inside_if {
                    *escaped = true;
                }
            }
        }
        
        // Mark the innermost IfBody as having a return
        if let Some(me) = self.mes.iter_mut().rev().find(|v| matches!(v, ShaderMe::IfBody { .. })) {
            if let ShaderMe::IfBody { has_return, .. } = me {
                *has_return = true;
            }
        }

        // NOTE: For a transpiler (unlike an interpreter), we do NOT set the trap here.
        // The interpreter sets ScriptTrapOn::Return to actually return control flow,
        // but the transpiler just generates code and must continue processing all opcodes
        // to properly close if/else blocks and other control structures.
        // The compile_fn loop uses fn_end_index (derived from FN_BODY_DYN's opargs) to know
        // when to stop, rather than relying on the Return trap.
    }

    fn handle_if_test(&mut self, opargs: OpcodeArgs) {
        let (_ty, val) = self.stack.pop(&self.trap);
        let start_pos = self.out.len();
        self.out.push_str("if(");
        self.out.push_str(&val);
        self.out.push_str("){\n");
        self.shader_scope.enter_scope();
        self.stack.free_string(val);

        self.mes.push(ShaderMe::IfBody {
            target_ip: self.trap.ip.index + opargs.to_u32(),
            start_pos,
            stack_depth: self.stack.types.len(),
            phi: None,
            phi_type: None,
            has_return: false,
            if_branch_returned: false,
        });
    }
    
    /// Handle IF_TEST when in unreachable code - don't generate code or pop stack,
    /// but track the control structure so we can properly close it
    fn handle_if_test_unreachable(&mut self, opargs: OpcodeArgs) {
        // Don't pop from stack or generate code - just track the structure
        // Mark has_return: true since we're already in unreachable code
        self.mes.push(ShaderMe::IfBody {
            target_ip: self.trap.ip.index + opargs.to_u32(),
            start_pos: self.out.len(),
            stack_depth: self.stack.types.len(),
            phi: None,
            phi_type: None,
            has_return: true,  // Already unreachable, so this branch is "returned"
            if_branch_returned: false,
        });
    }

    fn handle_if_else(&mut self, opargs: OpcodeArgs) {
        if let Some(ShaderMe::IfBody {
            target_ip,
            start_pos,
            stack_depth,
            phi,
            phi_type,
            has_return,
            if_branch_returned,
        }) = self.mes.last_mut()
        {
            if self.stack.types.len() > *stack_depth {
                let (ty, val) = self.stack.pop(&self.trap);
                *phi_type = Some(ty);
                let phi_name = if let Some(p) = phi {
                    p.clone()
                } else {
                    let s = format!("_phi_{}", start_pos);
                    *phi = Some(s.clone());
                    s
                };
                self.out.push_str(&format!("{} = {};\n", phi_name, val));
                self.stack.free_string(val);
            }
            self.out.push_str("}\nelse{\n");
            self.shader_scope.exit_scope();
            self.shader_scope.enter_scope();
            *target_ip = self.trap.ip.index + opargs.to_u32();
            // Save whether the if-branch returned, reset has_return for else branch
            *if_branch_returned = *has_return;
            *has_return = false;
        } else {
            self.trap.err_unexpected();
        }
    }
    
    /// Handle IF_ELSE when in unreachable code - just update structure, no code generation
    fn handle_if_else_unreachable(&mut self, opargs: OpcodeArgs) {
        if let Some(ShaderMe::IfBody {
            target_ip,
            has_return,
            if_branch_returned,
            ..
        }) = self.mes.last_mut()
        {
            *target_ip = self.trap.ip.index + opargs.to_u32();
            // Save whether the if-branch "returned", keep has_return true since we're unreachable
            *if_branch_returned = *has_return;
            // Keep has_return true since we're in unreachable code - else branch is also unreachable
            *has_return = true;
        }
    }
    
    /// Handle if/else phi when in unreachable code - just close the structure
    fn handle_if_else_phi_unreachable(&mut self) {
        if let Some(ShaderMe::IfBody { target_ip, has_return, if_branch_returned, .. }) = self.mes.last() {
            if self.trap.ip.index >= *target_ip {
                let both_returned = *if_branch_returned && *has_return;
                self.mes.pop();
                
                // If both branches returned, propagate up
                if both_returned {
                    if let Some(parent) = self.mes.last_mut() {
                        match parent {
                            ShaderMe::IfBody { has_return, .. } => {
                                *has_return = true;
                            }
                            ShaderMe::FnBody { escaped, .. } => {
                                *escaped = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    fn type_from_value(vm: &ScriptVm, value: ScriptValue) -> ShaderType {
        if let Some(pod_ty) = vm.code.builtins.pod.value_to_exact_type(value){
            return ShaderType::Pod(pod_ty);
        }
        if let Some(pod_ty) = vm.heap.pod_type(value){
            return ShaderType::PodType(pod_ty);
        }
        if let Some(pod) = value.as_pod(){
             let pod = &vm.heap.pods[pod.index as usize];
             return ShaderType::Pod(pod.ty);
        }
        if let Some(pod_ty) = value.as_pod_type(){
            return ShaderType::Pod(pod_ty);
        }
        ShaderType::None
    }
    
    /// Find the highest (most ancestral) shader IO definition for a field in the prototype chain.
    /// This ensures that if a parent defines `x: shader.uniform(vec4f)` and a child overrides
    /// with `x: #ffff`, we still use the uniform type from the parent.
    /// Returns (value_object, shader_io_type) if found, or None if no shader IO marker exists.
    fn find_highest_shader_io(
        vm: &ScriptVm, 
        io_self: ScriptObject, 
        field_id: LiveId,
        _trap: &ScriptTrap
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
    fn get_io_self_field_value(
        vm: &ScriptVm,
        io_self: ScriptObject,
        field_id: LiveId,
        trap: &ScriptTrap
    ) -> (ScriptValue, Option<ShaderIoType>) {
        
        // First, try to find the highest shader IO definition
        if let Some((io_obj, io_type)) = Self::find_highest_shader_io(vm, io_self, field_id, trap) {
            return (io_obj.into(), Some(io_type));
        }
        
        // No shader IO marker found - get the normal value (for RustInstance fields)
        let value = vm.heap.value(io_self, field_id.into(), trap);
        (value, None)
    }

    fn handle_field(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (field_ty, field_s) = self.stack.pop(&self.trap);
        let (instance_ty, instance_s) = self.pop_resolved(vm);
        
        if let ShaderType::Id(field_id) = field_ty {
            if let ShaderType::Pod(pod_ty) = instance_ty {
                if let Some(ret_ty) = vm.heap.pod_field_type(pod_ty, field_id, &vm.code.builtins.pod) {
                    let mut s = self.stack.new_string();
                    write!(s, "{}.{}", instance_s, field_id).ok();
                    self.stack.push(&self.trap, ShaderType::Pod(ret_ty), s);
                } else {
                    self.trap.err_not_found();
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                }
                self.stack.free_string(field_s);
                self.stack.free_string(instance_s);
                return
            } else if let ShaderType::PodPtr(pod_ty) = instance_ty {
                // Pointer type (e.g., uniform buffer in Metal) - use -> for field access
                if let Some(ret_ty) = vm.heap.pod_field_type(pod_ty, field_id, &vm.code.builtins.pod) {
                    let mut s = self.stack.new_string();
                    write!(s, "{}->{}", instance_s, field_id).ok();
                    self.stack.push(&self.trap, ShaderType::Pod(ret_ty), s);
                } else {
                    self.trap.err_not_found();
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                }
                self.stack.free_string(field_s);
                self.stack.free_string(instance_s);
                return
            } else if let ShaderType::Texture(tex_type) = instance_ty {
                // Field/method access on a texture - push texture and field name for method call handling
                // The field name (like "size") will be used as the method name
                self.stack.push(&self.trap, ShaderType::Texture(tex_type), instance_s);
                self.stack.push(&self.trap, ShaderType::Id(field_id), field_s);
                return
            } else if let ShaderType::IoSelf(obj) = instance_ty{
                // Look up field value, preferring the highest shader IO marker in the prototype chain
                let (value, maybe_io_type) = Self::get_io_self_field_value(vm, obj, field_id, &self.trap);
                
                if let Some(io_type) = maybe_io_type {
                    // Found a shader IO marker (uniform, varying, texture, etc.)
                    let value_obj = value.as_object().unwrap();
                    let proto = vm.heap.proto(value_obj);
                    let ty = Self::type_from_value(vm, proto);
                    let concrete_ty = match ty {
                        ShaderType::Pod(pt) => Some(pt),
                        ShaderType::PodType(pt) => Some(pt),
                        _ => None
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
                        self.stack.push(&self.trap, ShaderType::Texture(*tex_type), s);
                        self.stack.free_string(field_s);
                        self.stack.free_string(instance_s);
                        return
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
                        self.stack.push(&self.trap, shader_ty, s);
                        self.stack.free_string(field_s);
                        self.stack.free_string(instance_s);
                        return
                    }
                }
                
                // No shader IO marker found - check if this is a Rust struct field
                // Get the actual value (might be different from shader IO lookup)
                let actual_value = vm.heap.value(obj, field_id.into(), &self.trap);
                let ty = Self::type_from_value(vm, actual_value);
                let concrete_ty = match ty {
                    ShaderType::Pod(pt) => Some(pt),
                    ShaderType::PodType(pt) => Some(pt),
                    _ => None
                };
                
                if let Some(pod_ty) = concrete_ty {
                    // This is a Rust struct field - treat it as RustInstance
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
                    self.stack.push(&self.trap, ShaderType::Pod(pod_ty), s);
                    self.stack.free_string(field_s);
                    self.stack.free_string(instance_s);
                    return
                }
            }
        }
        self.trap.err_no_matching_shader_type();
        self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
        self.stack.free_string(field_s);
        self.stack.free_string(instance_s);
    }

    fn handle_let_dyn(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        if opargs.is_nil() {
            self.trap.err_have_to_initialise_variable();
            self.stack.pop(&self.trap);
        } else {
            let (ty_value, value) = self.stack.pop(&self.trap);
            let (ty_id, _id) = self.stack.pop(&self.trap);
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
                    self.trap.err_no_matching_shader_type();
                }
            } else {
                self.trap.err_unexpected();
            }
        }
    }

    fn handle_var_dyn(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        if opargs.is_nil() {
            self.trap.err_have_to_initialise_variable();
            self.stack.pop(&self.trap);
        } else {
            let (ty_value, value) = self.stack.pop(&self.trap);
            let (ty_id, _id) = self.stack.pop(&self.trap);
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
                    self.trap.err_no_matching_shader_type();
                }
            } else {
                self.trap.err_unexpected();
            }
        }
    }

    fn handle_for_1(&mut self) {
        let (source, _) = self.stack.pop(&self.trap);
        let (val_id, _) = self.stack.pop(&self.trap);
        if let ShaderType::Range { start, end, ty } = source {
            if let ShaderType::Id(id) = val_id {
                self.shader_scope.enter_scope();
                self.shader_scope.define_var(id, ty);
                write!(self.out, "for(var {0} = {1}; {0} < {2}; {0}++){{\n", id, start, end).ok();
                self.mes.push(ShaderMe::ForLoop {
                    var_id: id,
                });
            } else {
                self.trap.err_unexpected();
            }
        } else {
            self.trap.err_unexpected();
        }
    }

    fn handle_for_end(&mut self) {
        if let Some(me) = self.mes.pop() {
            if let ShaderMe::ForLoop { .. } = me {
                self.out.push_str("}\n");
                self.shader_scope.exit_scope();
            } else {
                self.trap.err_unexpected();
            }
        } else {
            self.trap.err_unexpected();
        }
    }

    fn handle_range(&mut self, vm: &mut ScriptVm) {
        let (_end_ty, end_s) = self.stack.pop(&self.trap);
        let (start_ty, start_s) = self.stack.pop(&self.trap);
        if let Some(ty) = start_ty.make_concrete(&vm.code.builtins.pod) {
            self.stack.push(
                &self.trap,
                ShaderType::Range {
                    start: start_s,
                    end: end_s,
                    ty,
                },
                String::new(),
            );
        } else {
            self.trap.err_no_matching_shader_type();
        }
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
    
    fn pop_to_me(&mut self, vm:&ScriptVm){
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
    
    fn maybe_pop_to_me(&mut self, vm:&ScriptVm, opargs:OpcodeArgs){
        if opargs.is_pop_to_me(){
            self.pop_to_me(vm);
        }
    }
}
