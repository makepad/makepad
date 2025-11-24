#[allow(unused)]
use makepad_live_id::*;
use crate::heap::*;
use crate::native::*;
use crate::value::*;
use crate::trap::*;
use crate::function::*;
use crate::vm::*;
use crate::opcode::*;
use crate::pod::*;
use crate::mod_pod::*;
use std::fmt::Write;
use crate::makepad_error_log::*;

// we collect functions, we do the type inferencing 
// and we just emit topdown.

pub fn define_shader_module(heap:&mut ScriptHeap, native:&mut ScriptNative){
    let math = heap.new_module(id!(shader));
        
    native.add_method(heap, math, id!(compile_draw), script_args!(object=NIL), |vm, args|{
        // lets fetch the code
        let object = script_value!(vm, args.object);
        
        // ok we're going to take a function, and then call it to generate/typetrace it out
        // for every function we make a 'shadercompiler'
        
        if let Some(object) = object.as_object(){
            if let Some(fnobj) = vm.heap.object_method(object, id!(pixel).into(), &vm.thread.trap).as_object(){
                if let Some(fnptr) = vm.heap.as_fn(fnobj){
                    if let ScriptFnPtr::Script(fnip) = fnptr{
                        let mut compiler = ShaderCompiler{
                            script_scope: fnobj,
                            stack: ShaderStack{
                                stack_limit: 1000000,
                                ..Default::default()
                            },
                            mes: vec![ShaderMe::FnBody{
                                ret:None
                            }],
                            ..Default::default()
                        };
                        compiler.compile(vm, fnip);
                        // alright we should have output now
                        println!("COMPILED:\n{}", compiler.out);
                        
                        return NIL
                    }
                }
            }
        }
        // trap error
        NIL
    });
}

#[derive(Debug)]
pub enum ShaderMe{
    FnBody{ret:Option<ScriptPodType>},
    LoopBody,
    IfBody,
    Call{out:String, this:ShaderThis, args:Vec<ShaderArg>},
    Pod{out:String, pod_ty:ScriptPodType, offset:ScriptPodOffset},
}

#[derive(Debug)]
pub enum ShaderThis{
    Free,
    Shader,
    Pod(ScriptPodType)
}

#[derive(Debug, PartialEq)]
pub enum ShaderType{
    Pod(ScriptPodType),
    Id(LiveId),
    AbstractInt,
    AbstractFloat,
    Error(ScriptValue)
}

impl ShaderType{
    fn make_concrete(&self, builtins:&ScriptPodBuiltins)->Option<ScriptPodType>{
        match self{
            Self::Pod(ty) => Some(*ty),
            Self::Id(_id) => None,
            Self::AbstractInt => Some(builtins.pod_i32),
            Self::AbstractFloat => Some(builtins.pod_f32),
            Self::Error(_e) => None,
        }
    }
}

trait ShaderOutput{
}

struct WgslBackend{
}

impl ShaderOutput for WgslBackend{
}

#[derive(Debug)]
pub struct ShaderArg{
    _name: LiveId,
    _ty: ScriptPodType,
}

pub struct ShaderFunction{
    _args: Vec<ShaderArg>,
    _out: String,
    _ret: Option<ScriptPodType>,
}

struct ShaderScopeItem{
    shadow: usize,
    is_var: bool,
    ty: ScriptPodType
}

#[derive(Default)]
struct ShaderCompiler{
    pub out: String,
    pub stack: ShaderStack,
    pub script_scope: ScriptObject,
    pub shader_scope: LiveIdMap<LiveId, ShaderScopeItem>,
    pub mes: Vec<ShaderMe>,
    pub trap: ScriptTrap,
}

#[derive(Default)]
struct ShaderStack{
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

macro_rules! impl_float_arithmetic {
    ($self:ident, $vm:ident, $opargs:ident, $op:tt)=>{{
        let (t2, s2) = if $opargs.is_u32(){
            (ShaderType::AbstractInt, free_fmt!($self, "{}", $opargs.to_u32()))
        }else{
            $self.pop_resolved($vm)
        };
        let (t1, s1) = $self.pop_resolved($vm);
        let mut s = $self.stack.new_string();
        write!(s, "({} {} {})", s1, stringify!($op), s2).ok();
        let ty = $self.type_table_float_arithmetic(t1, t2, &$vm.code.builtins.pod);
        $self.stack.push(&$self.trap, ty, s);
    }};
}

macro_rules! impl_int_arithmetic {
    ($self:ident, $vm:ident, $opargs:ident, $op:tt)=>{{
        let (t2, s2) = if $opargs.is_u32(){
            (ShaderType::AbstractInt, free_fmt!($self, "{}", $opargs.to_u32()))
        }else{
            $self.pop_resolved($vm)
        };
        let (t1, s1) = $self.pop_resolved($vm);
        let mut s = $self.stack.new_string();
        write!(s, "({} {} {})", s1, stringify!($op), s2).ok();
        let ty = $self.type_table_int_arithmetic(t1, t2, &$vm.code.builtins.pod);
        $self.stack.push(&$self.trap, ty, s);
    }};
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


impl ShaderCompiler{
        
    fn pop_resolved(&mut self, vm:&ScriptVm)->(ShaderType,String){
        let (ty, s) = self.stack.pop(&self.trap);
        // if ty is an id, look it up
        match ty{
            ShaderType::Id(id)=>{
                // look it up on our scope
                if let Some(sc) = self.shader_scope.get(&id){
                    return (ShaderType::Pod(sc.ty), s)
                }
                // alright lets look it up on our script scope
                let _value = vm.heap.scope_value(self.script_scope, id.into(), &self.trap);
                todo!()
            },
            _=>(ty, s),
        }
    }
    
    fn type_table_float_arithmetic(&mut self, lhs: ShaderType, rhs: ShaderType, builtins:&ScriptPodBuiltins )->ShaderType{
        let r = match lhs{
            ShaderType::AbstractFloat => match rhs{
                ShaderType::AbstractFloat=>ShaderType::AbstractFloat,
                ShaderType::AbstractInt=>ShaderType::AbstractFloat,
                ShaderType::Pod(x) if x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_f32),
                ShaderType::Pod(x) if x == builtins.pod_f16=>ShaderType::Pod(builtins.pod_f16),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::AbstractInt => match rhs{
                ShaderType::AbstractFloat=>ShaderType::AbstractFloat,
                ShaderType::AbstractInt=>ShaderType::AbstractInt,
                ShaderType::Pod(x) if x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_u32),
                ShaderType::Pod(x) if x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_i32),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_f32=> match rhs{
                ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_f32),
                ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_f32),
                ShaderType::Pod(x) if x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_f32),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_f16=> match rhs{
                ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_f16),
                ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_f16),
                ShaderType::Pod(x) if x == builtins.pod_f16=>ShaderType::Pod(builtins.pod_f16),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_u32=> match rhs{
                ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_u32),
                ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_u32),
                ShaderType::Pod(x) if x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_u32),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_i32=> match rhs{
                ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_i32),
                ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_i32),
                ShaderType::Pod(x) if x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_i32),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec2f=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec2f=>ShaderType::Pod(builtins.pod_vec2f),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec3f=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec3f=>ShaderType::Pod(builtins.pod_vec3f),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec4f=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec4f=>ShaderType::Pod(builtins.pod_vec4f),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec2h=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec2h=>ShaderType::Pod(builtins.pod_vec2h),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec3h=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec3h=>ShaderType::Pod(builtins.pod_vec3h),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec4h=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec4h=>ShaderType::Pod(builtins.pod_vec4h),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec2u=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec2u=>ShaderType::Pod(builtins.pod_vec2u),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec3u=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec3u=>ShaderType::Pod(builtins.pod_vec3u),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec4u=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec4u=>ShaderType::Pod(builtins.pod_vec4u),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec2i=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec2i=>ShaderType::Pod(builtins.pod_vec2i),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec3i=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec3i=>ShaderType::Pod(builtins.pod_vec3i),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec4i=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec4i=>ShaderType::Pod(builtins.pod_vec4i),
                _=>ShaderType::Error(NIL),
            }
            _=>ShaderType::Error(NIL),
        };
        if let ShaderType::Error(_) = r{
            self.trap.err_no_wgsl_conversion_available();
        }
        r
    }
    
    fn type_table_int_arithmetic(&mut self, lhs: ShaderType, rhs: ShaderType, builtins:&ScriptPodBuiltins )->ShaderType{
        let r = match lhs{
            ShaderType::AbstractFloat => match rhs{
                _=>ShaderType::Error(NIL),
            }
            ShaderType::AbstractInt => match rhs{
                ShaderType::AbstractInt=>ShaderType::AbstractInt,
                ShaderType::Pod(x) if x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_u32),
                ShaderType::Pod(x) if x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_i32),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_u32=> match rhs{
                ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_u32),
                ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_u32),
                ShaderType::Pod(x) if x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_u32),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_i32=> match rhs{
                ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_i32),
                ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_i32),
                ShaderType::Pod(x) if x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_i32),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec2u=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec2u=>ShaderType::Pod(builtins.pod_vec2u),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec3u=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec3u=>ShaderType::Pod(builtins.pod_vec3u),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec4u=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec4u=>ShaderType::Pod(builtins.pod_vec4u),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec2i=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec2i=>ShaderType::Pod(builtins.pod_vec2i),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec3i=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec3i=>ShaderType::Pod(builtins.pod_vec3i),
                _=>ShaderType::Error(NIL),
            }
            ShaderType::Pod(x) if x == builtins.pod_vec4i=> match rhs{
                ShaderType::Pod(x) if x == builtins.pod_vec4i=>ShaderType::Pod(builtins.pod_vec4i),
                _=>ShaderType::Error(NIL),
            }
            _=>ShaderType::Error(NIL),
        };
        if let ShaderType::Error(_) = r{
            self.trap.err_no_wgsl_conversion_available();
        }
        r
    }
    
    fn push_immediate(&mut self, value:ScriptValue, builtins:&ScriptPodBuiltins){
        if let Some(v) = value.as_f64(){ // abstract int or float
            if v.fract() != 0.0{
                return push_fmt!(self, ShaderType::AbstractFloat, "{}", v);
            }
            else{
                return push_fmt!(self, ShaderType::AbstractInt, "{}", v);
            }
        }
        if let Some(id) = value.as_id(){
            return push_fmt!(self, ShaderType::Id(id), "{}", id);
        }
        if let Some(v) = value.as_f32(){
            return push_fmt!(self, ShaderType::Pod(builtins.pod_f32), "{}f", v);
        }
        if let Some(v) = value.as_f16(){
            return push_fmt!(self, ShaderType::Pod(builtins.pod_f16), "{}h", v);
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
        self.trap.err_no_matching_shader_type();
    }
    
    fn compile(&mut self, vm:&mut ScriptVm, fnip:ScriptIp){
        let mut out = WgslBackend{};
        // alright lets go trace the opcodes
        self.trap.ip = fnip;
        self.trap.in_rust = true;
        let bodies = vm.code.bodies.borrow();
        let mut body = &bodies[self.trap.ip.body as usize];
        while (self.trap.ip.index as usize) < body.parser.opcodes.len(){
            let opcode = body.parser.opcodes[self.trap.ip.index as usize];
            if let Some((opcode, args)) = opcode.as_opcode(){
                self.opcode(vm, &mut out, opcode, args);
                self.trap.goto_next();
            }
            else{ // id or immediate value
                self.push_immediate(opcode, &vm.code.builtins.pod);
                self.trap.goto_next();
            }
            // alright lets see if we have a trap, ifso we can log it
            if let Some(trap) = self.trap.on.take(){
                match trap{
                    ScriptTrapOn::Error{value,..}=>{
                        // check if we have a try clause
                        if let Some(ptr) = value.as_err(){
                            if let Some(loc2) = vm.code.ip_to_loc(ptr.ip){
                                log_with_level(&loc2.file, loc2.line, loc2.col, loc2.line, loc2.col, format!("{}", value), LogLevel::Error);
                            }
                        }
                    },
                    ScriptTrapOn::Return(_value)=>{
                        break
                    }
                    _=>panic!()
                }
            }
                        
            body = &bodies[self.trap.ip.body as usize];
        }
    }
    
    fn opcode(&mut self, vm:&mut ScriptVm, _out: &dyn ShaderOutput, opcode: Opcode, opargs:OpcodeArgs){
        match opcode{
// Arithmetic
            Opcode::NOT=>{
            }
            Opcode::NEG=>{
            }
            Opcode::MUL=>impl_float_arithmetic!(self, vm, opargs, *),
            Opcode::DIV=>impl_float_arithmetic!(self, vm, opargs, /),
            Opcode::MOD=>impl_float_arithmetic!(self, vm, opargs, %),
            Opcode::ADD=>impl_float_arithmetic!(self, vm, opargs, +),
            Opcode::SUB=>impl_float_arithmetic!(self, vm, opargs, -),
            Opcode::SHL=>impl_int_arithmetic!(self, vm, opargs, >>),
            Opcode::SHR=>impl_int_arithmetic!(self, vm, opargs, <<),
            Opcode::AND=>impl_int_arithmetic!(self, vm, opargs, &),
            Opcode::OR=>impl_int_arithmetic!(self, vm, opargs, |),
            Opcode::XOR=>impl_int_arithmetic!(self, vm, opargs, ^),
                        
// ASSIGN
            Opcode::ASSIGN=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_ADD=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_SUB=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_MUL=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_DIV=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_MOD=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_AND=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_OR=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_XOR=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_SHL=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_SHR=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_IFNIL=>{self.trap.err_not_impl();},
// ASSIGN FIELD                       
            Opcode::ASSIGN_FIELD=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_FIELD_ADD=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_FIELD_SUB=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_FIELD_MUL=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_FIELD_DIV=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_FIELD_MOD=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_FIELD_AND=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_FIELD_OR=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_FIELD_XOR=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_FIELD_SHL=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_FIELD_SHR=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_FIELD_IFNIL=>{self.trap.err_not_impl();},
                                    
            Opcode::ASSIGN_INDEX=>{self.trap.err_not_impl();},
// ASSIGN INDEX
            Opcode::ASSIGN_INDEX_ADD=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_INDEX_SUB=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_INDEX_MUL=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_INDEX_DIV=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_INDEX_MOD=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_INDEX_AND=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_INDEX_OR=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_INDEX_XOR=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_INDEX_SHL=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_INDEX_SHR=>{self.trap.err_not_impl();},
            Opcode::ASSIGN_INDEX_IFNIL=>{self.trap.err_not_impl();},
// ASSIGN ME            
            Opcode::ASSIGN_ME=>{self.trap.err_not_impl();},
                                    
            Opcode::ASSIGN_ME_BEFORE | Opcode::ASSIGN_ME_AFTER=>{self.trap.err_opcode_not_supported_in_shader();},
                                    
            Opcode::ASSIGN_ME_BEGIN=>{self.trap.err_opcode_not_supported_in_shader();},
                        
                        
// CONCAT  
            Opcode::CONCAT=>{self.trap.err_opcode_not_supported_in_shader();},
// EQUALITY
            Opcode::EQ=>{self.trap.err_not_impl();},
            Opcode::NEQ=>{self.trap.err_not_impl();},
                        
            Opcode::LT=>{self.trap.err_not_impl();},
            Opcode::GT=>{self.trap.err_not_impl();},
            Opcode::LEQ=>{self.trap.err_not_impl();},
            Opcode::GEQ=>{self.trap.err_not_impl();},
                        
            Opcode::LOGIC_AND =>{self.trap.err_not_impl();},
            Opcode::LOGIC_OR =>{self.trap.err_not_impl();},
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
                let (ty, _s) = self.stack.pop(&self.trap);
                if let ShaderType::Id(id) = ty{
                    // alright lets look it up on our script scope
                    let value = vm.heap.scope_value(self.script_scope, id.into(), &self.trap);
                    // lets check if our obj is a PodType
                    if let Some(pod_ty) = vm.heap.pod_type(value){
                        let mut out = self.stack.new_string();
                        // alright lets see what Id we got
                        
                        vm.code.builtins.pod.wgsl_name(pod_ty, id, |s|{
                            write!(out, "{}(", s).ok();
                        });
                        
                        self.mes.push(ShaderMe::Pod{
                            out,
                            pod_ty: pod_ty,
                            offset: ScriptPodOffset::default(),
                        });
                        
                        self.maybe_pop_to_me(vm, opargs);
                        return
                    }
                    if let Some(obj) = value.as_object(){
                        if let Some(_obj) = vm.heap.as_fn(obj){
                            self.maybe_pop_to_me(vm, opargs);
                            todo!();
                            // its a script function
                            // we need to switch to type-inferring this one w arguments
                            // then hop into it, and serialise it out to our function list
                        }
                    }
                }
                self.trap.err_not_fn();
            },
            Opcode::CALL_EXEC | Opcode::METHOD_CALL_EXEC=>{
                if let Some(me) = self.mes.pop(){
                    match me{
                        ShaderMe::Pod{pod_ty, mut out, offset}=>{
                            // lets check if our field count works out
                            vm.heap.pod_check_constructor_arg_count(pod_ty, &offset, &self.trap);
                            out.push_str(")");
                            self.stack.push(&self.trap, ShaderType::Pod(pod_ty), out);
                        }
                        _=>{self.trap.err_not_impl();}
                    }
                }
            },
            Opcode::METHOD_CALL_ARGS=>{
                // resolve object on the scope
                // it could be a POD on the shader scope
                // or a POD on 'this'
                // or an object from the outside
                
                self.trap.err_not_impl();
            },
// Fn def
            Opcode::FN_ARGS=>{self.trap.err_not_impl();},
            Opcode::FN_LET_ARGS=>{self.trap.err_not_impl();},
            Opcode::FN_ARG_DYN=>{self.trap.err_not_impl();},
            Opcode::FN_ARG_TYPED=>{self.trap.err_not_impl();},
            Opcode::FN_BODY=>{self.trap.err_not_impl();},
            Opcode::RETURN=>{
                // lets find our FnBody
                if let Some(me) = self.mes.iter_mut().rev().find(|v| if let ShaderMe::FnBody{..} = v{true}else{false}){
                    if let ShaderMe::FnBody{ret} = me{
                        let (ty,s) = self.stack.pop(&self.trap);
                        let ty = ty.make_concrete(&vm.code.builtins.pod);
                        if let Some(ty) = ty{
                            if let Some(ret) = ret{
                                if ty != *ret{
                                    self.trap.err_return_type_changed();
                                }
                            }
                            *ret = Some(ty);
                        }
                        self.out.push_str("return ");
                        self.out.push_str(&s);
                        self.out.push_str(";\n");
                        self.stack.free_string(s);
                    }
                }
                
                self.trap.on.set(Some(ScriptTrapOn::Return(NIL)));
            },
            Opcode::RETURN_IF_ERR=>{self.trap.err_opcode_not_supported_in_shader();},
// IF            
            Opcode::IF_TEST=>{self.trap.err_not_impl();},
                        
            Opcode::IF_ELSE=>{self.trap.err_not_impl();},
// Use            
            Opcode::USE=>{self.trap.err_opcode_not_supported_in_shader();},
// Field            
            Opcode::FIELD=>{self.trap.err_not_impl();},
            Opcode::FIELD_NIL=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::ME_FIELD=>{self.trap.err_not_impl();},
            Opcode::PROTO_FIELD=>{self.trap.err_not_impl();},
                        
            Opcode::POP_TO_ME=>{
                self.pop_to_me(vm);    
            },
// Array index            
            Opcode::ARRAY_INDEX=>{self.trap.err_not_impl();},
// Let                   
            Opcode::LET_DYN=>{
                if opargs.is_nil(){
                    self.trap.err_have_to_initialise_variable();
                    self.stack.pop(&self.trap);
                }
                else{
                    let (ty_value, value) = self.stack.pop(&self.trap);
                    let (ty_id, _id) = self.stack.pop(&self.trap);
                    if let ShaderType::Id(id) = ty_id{
                        // lets define our let type
                        if let Some(ty) = ty_value.make_concrete(&vm.code.builtins.pod){
                            if let Some(sc) = self.shader_scope.get_mut(&id){
                                sc.shadow += 1;
                                sc.ty = ty;
                                sc.is_var = false;
                            }
                            else{
                                self.shader_scope.insert(id, ShaderScopeItem{
                                    is_var: false,
                                    shadow: 0,
                                    ty,
                                });
                            }
                        }
                        else{
                            self.trap.err_no_matching_shader_type();
                        }
                        if let Some(sc) = self.shader_scope.get(&id){
                            if sc.shadow>0{
                                write!(self.out, "let {id}{} = {value};\n", sc.shadow).ok();
                            }
                            else{
                                write!(self.out, "let {id} = {value};\n").ok();
                            }
                        }
                    }
                    else{
                        self.trap.err_unexpected();
                    }
                }
            },
            Opcode::LET_TYPED=>{self.trap.err_not_impl();},
// Tree search            
            Opcode::SEARCH_TREE=>{self.trap.err_opcode_not_supported_in_shader();},
// Log            
            Opcode::LOG=>{self.trap.err_opcode_not_supported_in_shader();},
// Me/Scope
            Opcode::ME=>{self.trap.err_opcode_not_supported_in_shader();},
                        
            Opcode::SCOPE=>{self.trap.err_opcode_not_supported_in_shader();},
// For            
            Opcode::FOR_1 =>{self.trap.err_not_impl();},
            Opcode::FOR_2 =>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::FOR_3=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::LOOP=>{self.trap.err_not_impl();},
            Opcode::FOR_END=>{self.trap.err_not_impl();},
            Opcode::BREAK=>{self.trap.err_not_impl();},
            Opcode::BREAKIFNOT=>{self.trap.err_not_impl();},
            Opcode::CONTINUE=>{self.trap.err_not_impl();},
// Range            
            Opcode::RANGE=>{self.trap.err_not_impl();},
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
                ShaderMe::FnBody{ ret:_}=>{
                    let (_ty,s) = self.stack.pop(&self.trap);
                    self.out.push_str(&s);
                    self.out.push_str(";\n");
                    self.stack.free_string(s);
                }
                ShaderMe::Pod{out, offset, pod_ty}=>{
                    let (ty, s) = self.stack.pop(&self.trap);
                    // if ty is an Id, we have to resolve it including ty
                    if offset.field_index > 0 {
                        out.push_str(", ");
                    }
                    match ty{
                        ShaderType::Pod(pod_ty_field)=>{
                            vm.heap.pod_check_constructor_arg(*pod_ty, pod_ty_field, offset, &self.trap);
                        }
                        ShaderType::Id(id)=>{
                            if let Some(v) = self.shader_scope.get(&id){
                                vm.heap.pod_check_constructor_arg(*pod_ty, v.ty, offset, &self.trap);
                            }
                            else{
                                // look value up on script scope
                                // we need to log this value somewhere
                                // if its a buffer or an immediate value
                            }
                        }
                        ShaderType::AbstractInt | ShaderType::AbstractFloat=>{
                            vm.heap.pod_check_abstract_constructor_arg(*pod_ty, offset, &self.trap);
                        }
                        ShaderType::Error(_e)=>{
                        }
                    }
                    out.push_str(&s);
                    self.stack.free_string(s);
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