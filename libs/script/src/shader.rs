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
use crate::shader_tables::*;
use std::fmt::Write;
use crate::makepad_error_log::*;

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
                        let mut compiler = ShaderFnCompiler::new(fnobj);
                        // compiling the entrypoint pixelshader
                        let mut output = ShaderOutput::default();
                        compiler.compile_fn(vm, &mut output, fnip);
                        output.functions.push(ShaderFn{
                            overload: 0,
                            name: id!(pixel),
                            call_sig: "fn pixel()".into(),
                            args:Default::default(),
                            fnobj,
                            out: compiler.out,
                            ret: vm.code.builtins.pod.pod_void
                        });
                        // alright we should have output now
                        for fns in output.functions{
                            println!("COMPILED:{}\n{}\n",fns.call_sig, fns.out);
                        }
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
    IfBody{
        target_ip: u32,
        start_pos: usize,
        stack_depth: usize,
        phi: Option<String>
    },
    BuiltinCall{out:String, fnptr: NativeId, args:Vec<ScriptPodType>},
    ScriptCall{out:String, name:LiveId, fnobj: ScriptObject, this:ShaderThis, args:Vec<ScriptPodType>},
    Pod{out:String, pod_ty:ScriptPodType, offset:ScriptPodOffset},
}

#[derive(Debug)]
pub enum ShaderThis{
    None,
    ShaderThis,
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

#[derive(Debug)]
pub struct ShaderFn{
    call_sig: String,
    overload: usize,
    name: LiveId,
    args: Vec<ScriptPodType>,
    fnobj: ScriptObject,
    out: String,
    ret: ScriptPodType,
}

struct ShaderScopeItem{
    shadow: usize,
    is_var: bool,
    ty: ScriptPodType
}

#[derive(Debug)]
struct ScriptPodTypeNamed{
    name: LiveId,
    ty: ScriptPodType
}

#[derive(Default, Debug)]
struct ShaderOutput{
    pub recur_block: Vec<ScriptObject>,
    pub structs: Vec<ScriptPodTypeNamed>,
    pub functions: Vec<ShaderFn>,
} 

impl ShaderOutput{
    fn find_struct_name(&self, ty:ScriptPodType)->Option<LiveId>{
        if let Some(v) = self.structs.iter().find(|v| v.ty == ty){
            Some(v.name)
        }
        else{
            None
        }
    }
}

#[derive(Default)]
struct ShaderFnCompiler{
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


impl ShaderFnCompiler{
    
    fn new(script_scope:ScriptObject)->Self{
        ShaderFnCompiler{
            script_scope,
            stack: ShaderStack{
                stack_limit: 1000000,
                ..Default::default()
            },
            mes: vec![],
            ..Default::default()
        }
    }
    
    fn pop_resolved(&mut self, vm:&ScriptVm)->(ShaderType,String){
        let (ty, s) = self.stack.pop(&self.trap);
        // if ty is an id, look it up
        match ty{
            ShaderType::Id(id)=>{
                // look it up on our scope
                if let Some(sc) = self.shader_scope.get(&id){
                    if sc.shadow>0{
                        let mut s2 = self.stack.new_string();
                        write!(s2, "_s{}{s}", sc.shadow).ok();
                        self.stack.free_string(s);
                        return (ShaderType::Pod(sc.ty), s2)
                    }
                    return (ShaderType::Pod(sc.ty), s)
                }
                // alright lets look it up on our script scope
                let _value = vm.heap.scope_value(self.script_scope, id.into(), &self.trap);
                todo!()
            },
            _=>(ty, s),
        }
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

    fn impl_eq(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str){
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
        let ty = type_table_eq(t1, t2, &self.trap, &vm.code.builtins.pod);
        self.stack.push(&self.trap, ty, s);
    }

    fn impl_logic(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str){
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
        let ty = type_table_logic(t1, t2, &self.trap, &vm.code.builtins.pod);
        self.stack.push(&self.trap, ty, s);
    }

    fn impl_float_arithmetic(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str){
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
        let ty = type_table_float_arithmetic(t1, t2, &self.trap, &vm.code.builtins.pod);
        self.stack.push(&self.trap, ty, s);
    }

    fn impl_int_arithmetic(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str){
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
        let ty = type_table_int_arithmetic(t1, t2, &self.trap, &vm.code.builtins.pod);
        self.stack.push(&self.trap, ty, s);
    }
    
    fn handle_if_else_phi(&mut self, vm:&ScriptVm, output:&mut ShaderOutput){
        if let Some(ShaderMe::IfBody{target_ip, phi, start_pos, stack_depth}) = self.mes.last(){
            if self.trap.ip.index >= *target_ip{
                if self.stack.types.len() > *stack_depth{
                    let (ty, val) = self.stack.pop(&self.trap);
                    if let Some(phi) = phi{
                        self.out.push_str(&format!("{} = {};\n", phi, val));
                        self.stack.free_string(val);
                        // declare the phi at start
                        let ty = ty.make_concrete(&vm.code.builtins.pod).unwrap();
                        let ty_name = if let Some(name) = vm.heap.pod_type_name(ty){
                            name
                        }
                        else if let Some(name) = output.find_struct_name(ty){
                            name
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
                    else{
                        self.stack.free_string(val);
                    }
                }
                self.out.push_str("}\n");
                self.mes.pop();
            }
        }
    }
    
    fn compile_fn(&mut self, vm:&mut ScriptVm, output:&mut ShaderOutput, fnip:ScriptIp)->ScriptPodType{
        self.mes.push(ShaderMe::FnBody{
            ret:None
        });
        // alright lets go trace the opcodes
        self.trap.ip = fnip;
        self.trap.in_rust = true;
        let bodies = vm.code.bodies.borrow();
        let mut body = &bodies[self.trap.ip.body as usize];
        while (self.trap.ip.index as usize) < body.parser.opcodes.len(){
            self.handle_if_else_phi(vm, output);
            let opcode = body.parser.opcodes[self.trap.ip.index as usize];
            if let Some((opcode, args)) = opcode.as_opcode(){
                self.opcode(vm, output, opcode, args);
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
        if let Some(ShaderMe::FnBody{ret}) = self.mes.pop(){
            return ret.unwrap_or(vm.code.builtins.pod.pod_void)
        }
        panic!()
    }
    
    fn opcode(&mut self, vm:&mut ScriptVm, output: &mut ShaderOutput, opcode: Opcode, opargs:OpcodeArgs){
        match opcode{
// Arithmetic
            Opcode::NOT=>{
            }
            Opcode::NEG=>{
            }
            Opcode::MUL=>self.impl_float_arithmetic(vm, opargs, "*"),
            Opcode::DIV=>self.impl_float_arithmetic(vm, opargs, "/"),
            Opcode::MOD=>self.impl_float_arithmetic(vm, opargs, "%"),
            Opcode::ADD=>self.impl_float_arithmetic(vm, opargs, "+"),
            Opcode::SUB=>self.impl_float_arithmetic(vm, opargs, "-"),
            Opcode::SHL=>self.impl_int_arithmetic(vm, opargs, ">>"),
            Opcode::SHR=>self.impl_int_arithmetic(vm, opargs, "<<"),
            Opcode::AND=>self.impl_int_arithmetic(vm, opargs, "&"),
            Opcode::OR=>self.impl_int_arithmetic(vm, opargs, "|"),
            Opcode::XOR=>self.impl_int_arithmetic(vm, opargs, "^"),
                        
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
            Opcode::EQ=>{self.impl_eq(vm, opargs, "==");},
            Opcode::NEQ=>{self.impl_eq(vm, opargs, "!=");},
                        
            Opcode::LT=>{self.impl_eq(vm, opargs, "<");},
            Opcode::GT=>{self.impl_eq(vm, opargs, ">");},
            Opcode::LEQ=>{self.impl_eq(vm, opargs, "<=");},
            Opcode::GEQ=>{self.impl_eq(vm, opargs, ">=");},
                        
            Opcode::LOGIC_AND =>{self.impl_logic(vm, opargs, "&&");},
            Opcode::LOGIC_OR =>{self.impl_logic(vm, opargs, "||");},
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
                if let ShaderType::Id(name) = ty{
                    // alright lets look it up on our script scope
                    let value = vm.heap.scope_value(self.script_scope, name.into(), &self.trap);
                    // lets check if our obj is a PodType
                    if let Some(pod_ty) = vm.heap.pod_type(value){
                        
                        let mut out = self.stack.new_string();
                        // alright lets see what Id we got
                        if let Some(name) = vm.heap.pod_type_name(pod_ty){
                            write!(out, "{}(", name).ok();
                        }
                        else{
                            // we should name this pod type somewhere
                            if let Some(sn) = output.structs.iter().find(|v| v.ty == pod_ty){
                                if sn.name != name{
                                    self.trap.err_struct_name_not_consistent();
                                    write!(out, "{}(", sn.name).ok();
                                }
                            }
                            else{
                                output.structs.push(ScriptPodTypeNamed{name, ty:pod_ty});
                                write!(out, "{}(", name).ok();
                            }
                        }
                        
                        self.mes.push(ShaderMe::Pod{
                            out,
                            pod_ty: pod_ty,
                            offset: ScriptPodOffset::default(),
                        });
                        
                        self.maybe_pop_to_me(vm, opargs);
                        return
                    }
                    if let Some(fnobj) = value.as_object(){
                        if let Some(fnptr) = vm.heap.as_fn(fnobj){
                            match fnptr{
                                // another script fn
                                ScriptFnPtr::Script(_fnptr)=>{
                                    let mut out = self.stack.new_string();
                                    write!(out, "{}(", name).ok();
                                    self.mes.push(ShaderMe::ScriptCall{
                                        name,
                                        out,
                                        fnobj,
                                        this: ShaderThis::None,
                                        args: Default::default(),
                                    });
                                }
                                // builtin shader fns
                                ScriptFnPtr::Native(_native_id)=>{
                                    todo!()
                                }
                            }
                            
                            self.maybe_pop_to_me(vm, opargs);
                            return
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
                        ShaderMe::ScriptCall{mut out, name, fnobj, this:_, args}=>{
                            // we should compare number of arguments (needs to be exact)
                            let argc = vm.heap.vec_len(fnobj);
                            if argc != args.len(){
                                self.trap.err_invalid_arg_count();
                            }
                            else{ // lets type trace it
                                // lets see if we already have fnobj with our argstypes
                                let ret = if let Some(fun) = output.functions.iter().find(|v|{
                                    v.fnobj == fnobj && v.args == args
                                }){
                                    if fun.overload != 0{
                                        let mut n = self.stack.new_string();
                                        write!(n, "_f{}",  fun.overload ).ok();
                                        out.insert_str(0, &n);
                                        self.stack.free_string(n);
                                    }
                                    fun.ret
                                }
                                else{
                                    let overload = output.functions.iter().filter(|v|{v.name == name}).count();
                                    // allow multiple typetraces of the same function:
                                    // add a counter to the fn name somehow
                                    // lets run a compile
                                    let mut compiler = ShaderFnCompiler::new(fnobj);
                                    // we need to pass in a vec of types to the function
                                    let mut call_sig = String::new();
                                    if overload != 0{
                                        let mut n = self.stack.new_string();
                                        write!(n, "_f{}",  overload).ok();
                                        out.insert_str(0, &n);
                                        self.stack.free_string(n);
                                        write!(call_sig, "fn _f{}{}(", overload, name).ok();
                                    }
                                    else{
                                        write!(call_sig, "fn {}(", name).ok();
                                    }
                                    for i in 0..argc{
                                        // put in argument types
                                        let kv = vm.heap.vec_key_value(fnobj, i, &self.trap);
                                        if let Some(id) = kv.key.as_id(){
                                            if i!=0{call_sig.push_str(", ");}
                                            let arg_ty = args[i];
                                            write!(call_sig, "{}:", id).ok();
                                            if let Some(name) = vm.heap.pod_type_name(arg_ty){
                                                write!(call_sig, "{}", name).ok();
                                            }
                                            else if let Some(name) = output.find_struct_name(arg_ty){
                                                write!(call_sig, "{}", name).ok();
                                            }
                                            else{
                                                todo!()
                                            }
                                            compiler.shader_scope.insert(id, ShaderScopeItem{
                                                shadow: 0,
                                                is_var: false,
                                                ty: arg_ty
                                            });
                                        }
                                    }
                                    write!(call_sig, ")").ok();
                                    if let Some(fnptr) = vm.heap.as_fn(fnobj){
                                        if let ScriptFnPtr::Script(fnip) = fnptr{
                                            if output.recur_block.iter().any(|v| *v == fnobj){
                                                self.trap.err_recursion_not_allowed();
                                                vm.code.builtins.pod.pod_void
                                            }
                                            else{
                                                output.recur_block.push(fnobj);
                                                let ret = compiler.compile_fn(vm, output, fnip);
                                                output.recur_block.pop();
                                                if let Some(name) = vm.heap.pod_type_name(ret){
                                                    write!(call_sig, "->{}", name).ok();
                                                }
                                                else if let Some(name) = output.find_struct_name(ret){
                                                    write!(call_sig, "->{}", name).ok();
                                                }
                                                else{
                                                    todo!()
                                                }
                                                
                                                output.functions.push(ShaderFn{
                                                    overload,
                                                    call_sig,
                                                    name,
                                                    args,
                                                    fnobj,
                                                    out: compiler.out,
                                                    ret
                                                });
                                                ret
                                            }
                                        }
                                        else{panic!()}
                                    }
                                    else{panic!()}
                                };
                                
                                out.push_str(")");
                                self.stack.push(&self.trap, ShaderType::Pod(ret), out);
                                
                            }
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
                        
                        // we can also return a void
                        let (ty,s) = if opargs.is_nil(){
                            (vm.code.builtins.pod.pod_void, self.stack.new_string())
                        }
                        else{
                            let (ty, s) = self.stack.pop(&self.trap);
                            let ty = ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                            (ty, s)
                        };
                        if let Some(ret) = ret{
                            if ty != *ret{
                                self.trap.err_return_type_changed();
                            }
                        }
                        *ret = Some(ty);
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
            Opcode::IF_TEST=>{
                let (_ty, val) = self.stack.pop(&self.trap);
                let start_pos = self.out.len();
                self.out.push_str("if(");
                self.out.push_str(&val);
                self.out.push_str("){\n");
                self.stack.free_string(val);
                
                self.mes.push(ShaderMe::IfBody{
                    target_ip: self.trap.ip.index + opargs.to_u32(),
                    start_pos,
                    stack_depth: self.stack.types.len(),
                    phi: None
                });
            }
                        
            Opcode::IF_ELSE=>{
                if let Some(ShaderMe::IfBody{target_ip, start_pos, stack_depth, phi}) = self.mes.last_mut(){
                     if self.stack.types.len() > *stack_depth{
                         let (_ty, val) = self.stack.pop(&self.trap);
                         let phi_name = if let Some(p) = phi{
                             p.clone()
                         }
                         else{
                             let s = format!("_phi_{}", start_pos); 
                             *phi = Some(s.clone());
                             s
                         };
                         self.out.push_str(&format!("{} = {};\n", phi_name, val));
                         self.stack.free_string(val);
                     }
                     self.out.push_str("}\nelse{\n");
                     *target_ip = self.trap.ip.index + opargs.to_u32();
                }
                else{
                    self.trap.err_unexpected();
                }
            }
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
                                write!(self.out, "let _s{}{id} = {value};\n", sc.shadow).ok();
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
                                todo!();
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
                ShaderMe::ScriptCall{out, args, ..}=>{
                    let (ty, s) = self.stack.pop(&self.trap);
                    if args.len() > 0 {
                        out.push_str(", ");
                    }
                    if let Some(ty) = ty.make_concrete(&vm.code.builtins.pod){
                        args.push(ty);
                    }
                    else{
                        self.trap.err_no_matching_shader_type();
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