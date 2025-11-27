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
use crate::shader_builtins::*;
use std::fmt::Write;
use crate::makepad_error_log::*;
use std::collections::BTreeSet;

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
                        let mut out = String::new();
                        output.create_struct_defs(vm, &mut out);
                        println!("Structs:\n{}", out);
                        for fns in output.functions{
                            println!("{}\n{}\n",fns.call_sig, fns.out);
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
pub struct ShaderPodArg{
    pub name: Option<LiveId>,
    pub ty: ShaderType,
    pub s: String
}

#[derive(Debug)]
pub enum ShaderMe{
    FnBody{ret:Option<ScriptPodType>},
    LoopBody,
    ForLoop{
        var_id: LiveId,
        shadow: usize,
    },
    IfBody{
        target_ip: u32,
        start_pos: usize,
        stack_depth: usize,
        phi: Option<String>,
        phi_type: Option<ShaderType>
    },
    BuiltinCall{out:String, name:LiveId, fnptr: NativeId, args:Vec<ScriptPodType>},
    ScriptCall{out:String, name:LiveId, fnobj: ScriptObject, this:ShaderThis, args:Vec<ScriptPodType>},
    Pod{pod_ty:ScriptPodType, args: Vec<ShaderPodArg>},
    ArrayConstruct{args:Vec<String>, elem_ty:Option<ScriptPodType>},
}

#[derive(Debug)]
pub enum ShaderThis{
    None,
    ShaderThis,
    PodType(ScriptPodType),
    Pod(ScriptPodType)
}

#[derive(Debug, PartialEq, Clone)]
pub enum ShaderType{
    Pod(ScriptPodType),
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
            Self::Id(_id) => None,
            Self::AbstractInt => Some(builtins.pod_i32),
            Self::AbstractFloat => Some(builtins.pod_f32),
            Self::Range{ty,..} => Some(*ty),
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

#[derive(Default, Debug)]
struct ShaderOutput{
    pub recur_block: Vec<ScriptObject>,
    pub structs: BTreeSet<ScriptPodType>,
    pub functions: Vec<ShaderFn>,
} 

impl ShaderOutput{
    fn create_struct_defs(&self, vm:&ScriptVm, out:&mut String){
        vm.heap.pod_type_wgsl_struct_defs(&self.structs, out);
    }
}


#[derive(Default)]
struct ShaderScope{
    pub shader_scope: Vec<LiveIdMap<LiveId, ShaderScopeItem>>,
}

#[derive(Default)]
struct ShaderFnCompiler{
    pub out: String,
    pub stack: ShaderStack,
    pub script_scope: ScriptObject,
    pub shader_scope: ShaderScope,
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

impl ShaderScope{
        
    fn enter_scope(&mut self) {
        self.shader_scope.push(Default::default());
    }
    
    fn exit_scope(&mut self) {
        self.shader_scope.pop();
    }
    
    fn find_var(&self, id: LiveId) -> Option<(&ShaderScopeItem, String)> {
        for scope in self.shader_scope.iter().rev() {
            if let Some(item) = scope.get(&id) {
                let name = if item.shadow > 0 {
                    format!("_s{}{}", item.shadow, id)
                } else {
                    format!("{}", id)
                };
                return Some((item, name));
            }
        }
        None
    }
    
    fn define_var(&mut self, id: LiveId, ty: ScriptPodType, is_var: bool) -> String {
        let scope = self.shader_scope.last_mut().unwrap();
        if let Some(item) = scope.get_mut(&id) {
            item.shadow += 1;
            item.ty = ty;
            item.is_var = is_var;
            format!("_s{}{}", item.shadow, id)
        } else {
            scope.insert(id, ShaderScopeItem {
                shadow: 0,
                is_var,
                ty
            });
            format!("{}", id)
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
            shader_scope: ShaderScope{shader_scope:vec![Default::default()]},
            ..Default::default()
        }
    }
    

    fn pop_resolved(&mut self, vm:&ScriptVm)->(ShaderType,String){
        let (ty, s) = self.stack.pop(&self.trap);
        // if ty is an id, look it up
        match ty{
            ShaderType::Id(id)=>{
                // look it up on our scope
                if let Some((sc, name)) = self.shader_scope.find_var(id){
                    let mut s2 = self.stack.new_string();
                    write!(s2, "{}", name).ok();
                    self.stack.free_string(s);
                    return (ShaderType::Pod(sc.ty), s2)
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

    fn handle_float_arithmetic(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str){
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
        let ty = type_table_float_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod);
        self.stack.push(&self.trap, ty, s);
    }

    fn handle_int_arithmetic(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str){
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
        let ty = type_table_int_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod);
        self.stack.push(&self.trap, ty, s);
    }
    
    fn handle_float_arithmetic_assign(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str){
        let (t2, s2) = if opargs.is_u32(){
            let mut s = self.stack.new_string();
            write!(s, "{}", opargs.to_u32()).ok();
            (ShaderType::AbstractInt, s)
        }else{
            self.pop_resolved(vm)
        };
        let (id_ty, id_s) = self.stack.pop(&self.trap);
        if let ShaderType::Id(id) = id_ty{
            if let Some((var, name)) = self.shader_scope.find_var(id){
                if !var.is_var{
                    self.trap.err_let_is_immutable();
                }
                let t1 = ShaderType::Pod(var.ty);
                let _ty = type_table_float_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod);
                
                let mut s = self.stack.new_string();
                write!(s, "{} {} {}", name, op, s2).ok();
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

    fn handle_int_arithmetic_assign(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str){
        let (t2, s2) = if opargs.is_u32(){
            let mut s = self.stack.new_string();
            write!(s, "{}", opargs.to_u32()).ok();
            (ShaderType::AbstractInt, s)
        }else{
            self.pop_resolved(vm)
        };
        let (id_ty, id_s) = self.stack.pop(&self.trap);
        if let ShaderType::Id(id) = id_ty{
            if let Some((var, name)) = self.shader_scope.find_var(id){
                if !var.is_var{
                    self.trap.err_let_is_immutable();
                }
                let t1 = ShaderType::Pod(var.ty);
                let _ty = type_table_int_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod);
                
                let mut s = self.stack.new_string();
                write!(s, "{} {} {}", name, op, s2).ok();
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
    
    fn handle_float_arithmetic_field_assign(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str){
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
                    let op_res_ty = type_table_float_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod);
                    
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

    fn handle_int_arithmetic_field_assign(&mut self, vm:&ScriptVm, opargs:OpcodeArgs, op:&str){
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
                    let op_res_ty = type_table_int_arithmetic(&t1, &t2, &self.trap, &vm.code.builtins.pod);
                    
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
    
    fn handle_if_else_phi(&mut self, vm:&ScriptVm){
        if let Some(ShaderMe::IfBody{target_ip, phi, start_pos, stack_depth, phi_type}) = self.mes.last(){
            if self.trap.ip.index >= *target_ip{
                if self.stack.types.len() > *stack_depth{
                    let (ty, val) = self.stack.pop(&self.trap);
                    if let Some(phi) = phi{
                        if let Some(phi_type) = phi_type{
                            self.out.push_str(&format!("{} = {};\n", phi, val));
                            // declare the phi at start
                            let ty = type_table_if_else(phi_type, &ty, &self.trap, &vm.code.builtins.pod);
                            let ty = ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                            let ty_name = if let Some(name) = vm.heap.pod_type_name(ty){
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
                    }
                    self.stack.free_string(val);
                }
                self.out.push_str("}\n");
                self.shader_scope.exit_scope();
                self.mes.pop();
            }
        }
    }
    
    fn ensure_struct_name(&self, vm: &mut ScriptVm, output: &mut ShaderOutput, pod_ty: ScriptPodType, used_name: LiveId) -> LiveId {
        if let Some(name) = vm.heap.pod_type_name(pod_ty) {
            if name != used_name {
                self.trap.err_struct_name_not_consistent();
            }
            return name;
        }
        output.structs.insert(pod_ty);
        vm.heap.pod_type_name_set(pod_ty, used_name);
        used_name
    }

    fn handle_call_args(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        let (ty, _s) = self.stack.pop(&self.trap);
        if let ShaderType::Id(name) = ty {
            // alright lets look it up on our script scope
            let value = vm.heap.scope_value(self.script_scope, name.into(), &self.trap);
            // lets check if our obj is a PodType
            if let Some(pod_ty) = vm.heap.pod_type(value) {
                
                if let ScriptPodTy::ArrayBuilder = &vm.heap.pod_types[pod_ty.index as usize].ty {
                    self.mes.push(ShaderMe::ArrayConstruct {
                        args: Vec::new(),
                        elem_ty: None,
                    });
                    self.maybe_pop_to_me(vm, opargs);
                    return;
                }

                let mut _out = self.stack.new_string();
                // alright lets see what Id we got
                let _name = self.ensure_struct_name(vm, output, pod_ty, name);
                //write!(out, "{}(", name).ok();
                
                self.mes.push(ShaderMe::Pod {
                    pod_ty: pod_ty,
                    args: Vec::new()
                });
                
                self.maybe_pop_to_me(vm, opargs);
                return;
            }
            if let Some(fnobj) = value.as_object() {
                if let Some(fnptr) = vm.heap.as_fn(fnobj) {
                    match fnptr {
                        // another script fn
                        ScriptFnPtr::Script(_fnptr) => {
                            let mut out = self.stack.new_string();
                            write!(out, "{}(", name).ok();
                            self.mes.push(ShaderMe::ScriptCall {
                                name,
                                out,
                                fnobj,
                                this: ShaderThis::None,
                                args: Default::default(),
                            });
                        }
                        // builtin shader fns
                        ScriptFnPtr::Native(fnptr) => {
                            let mut out = self.stack.new_string();
                            write!(out, "{}(", name).ok();
                            self.mes.push(ShaderMe::BuiltinCall {
                                out,
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
             write!(out, "array<{}, {}>", name, count).ok();
             if matches!(vm.heap.pod_types[elem_ty.index as usize].ty, ScriptPodTy::Struct{..}) {
                 output.structs.insert(elem_ty);
             }
        }
        else {
            self.trap.err_no_matching_shader_type();
        }
        
        write!(out, "(").ok();
        for (i, s) in args.iter().enumerate() {
            if i > 0 { out.push_str(", "); }
            out.push_str(s);
        }
        out.push_str(")");
        
        for s in args {
            self.stack.free_string(s);
        }
        
        self.stack.push(&self.trap, ShaderType::Pod(array_ty), out);
    }

    fn handle_pod_construct(&mut self, vm: &mut ScriptVm, pod_ty: ScriptPodType, args: Vec<ShaderPodArg>) {
         let mut offset = ScriptPodOffset::default();
         
         let mut out = self.stack.new_string();
         if let Some(name) = vm.heap.pod_type_name(pod_ty) {
             write!(out, "{}(", name).ok();
         }
         else {
             self.trap.err_no_matching_shader_type();
         }
         
         let pod_ty_data = &vm.heap.pod_types[pod_ty.index as usize];
         
         if let Some(first) = args.first(){
             if first.name.is_some(){ // Named args
                  if let ScriptPodTy::Struct{fields, ..} = &pod_ty_data.ty {
                       for (i, field) in fields.iter().enumerate(){
                           if i > 0 { out.push_str(", "); }
                           
                           // Find the arg with this name
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
                                              if v.ty != field.ty.self_ref {
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
                            ShaderType::Pod(pod_ty_field)=>{
                                vm.heap.pod_check_constructor_arg(pod_ty, *pod_ty_field, &mut offset, &self.trap);
                            }
                            ShaderType::Id(id)=>{
                                if let Some((v, _name)) = self.shader_scope.find_var(*id){
                                    vm.heap.pod_check_constructor_arg(pod_ty, v.ty, &mut offset, &self.trap);
                                }
                                else{
                                    self.trap.err_not_found();
                                }
                            }
                            ShaderType::AbstractInt | ShaderType::AbstractFloat=>{
                                vm.heap.pod_check_abstract_constructor_arg(pod_ty, &mut offset, &self.trap);
                            }
                            ShaderType::Range{..}|ShaderType::Error(_)=>{
                            }
                        }
                        out.push_str(&arg.s);
                  }
                  vm.heap.pod_check_constructor_arg_count(pod_ty, &offset, &self.trap);
             }
         }
         else {
              vm.heap.pod_check_constructor_arg_count(pod_ty, &offset, &self.trap);
         }
         
         out.push_str(")");
         
         for arg in args {
             self.stack.free_string(arg.s);
         }
         
         self.stack.push(&self.trap, ShaderType::Pod(pod_ty), out);
    }

    fn handle_script_call(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, mut out: String, name: LiveId, fnobj: ScriptObject, this: ShaderThis, args: Vec<ScriptPodType>) {
        // we should compare number of arguments (needs to be exact)
        let argc = vm.heap.vec_len(fnobj);
        let check_argc = if let ShaderThis::Pod(_) = this { argc + 1 } else { argc };
        if check_argc != args.len() {
            self.trap.err_invalid_arg_count();
        } else { // lets type trace it
            
            let mut method_name_prefix = self.stack.new_string();
            if let ShaderThis::PodType(ty) = this{
                 if let Some(name) = vm.heap.pod_type_name(ty) {
                     write!(method_name_prefix, "{}_", name).ok();
                 } 
            }
            else if let ShaderThis::Pod(ty) = this {
                 if let Some(name) = vm.heap.pod_type_name(ty) {
                     write!(method_name_prefix, "{}_", name).ok();
                 } 
            }

            // lets see if we already have fnobj with our argstypes
            let ret = if let Some(fun) = output.functions.iter().find(|v| {
                v.fnobj == fnobj && v.args == args
            }) {
                if fun.overload != 0 {
                    let mut n = self.stack.new_string();
                    write!(n, "_f{}", fun.overload).ok();
                    out.insert_str(0, &n);
                    self.stack.free_string(n);
                }
                out.insert_str(0, &method_name_prefix);
                fun.ret
            } else {
                let overload = output.functions.iter().filter(|v| { v.name == name }).count();
                // allow multiple typetraces of the same function:
                // add a counter to the fn name somehow
                // lets run a compile
                let mut compiler = ShaderFnCompiler::new(fnobj);
                // we need to pass in a vec of types to the function
                let mut call_sig = String::new();
                if overload != 0 {
                    let mut n = self.stack.new_string();
                    write!(n, "_f{}", overload).ok();
                    out.insert_str(0, &n);
                    self.stack.free_string(n);
                    write!(call_sig, "fn _f{}{}{}(", overload, method_name_prefix, name).ok();
                } else {
                    write!(call_sig, "fn {}{}(", method_name_prefix, name).ok();
                }
                out.insert_str(0, &method_name_prefix);

                if let ShaderThis::Pod(ty) = this {
                    write!(call_sig, "this:").ok();
                    if let Some(name) = vm.heap.pod_type_name(ty) {
                        write!(call_sig, "{}", name).ok();
                    }
                    compiler.shader_scope.define_var(id!(this), ty, false);
                    if argc > 0 { call_sig.push_str(", "); }
                }
                
                let arg_offset = if let ShaderThis::Pod(_) = this { 1 } else { 0 };

                for i in 0..argc {
                    // put in argument types
                    let kv = vm.heap.vec_key_value(fnobj, i, &self.trap);
                    if let Some(id) = kv.key.as_id() {
                        if i != 0 { call_sig.push_str(", "); }
                        let arg_ty = args[i + arg_offset];
                        write!(call_sig, "{}:", id).ok();
                        if let Some(name) = vm.heap.pod_type_name(arg_ty) {
                            write!(call_sig, "{}", name).ok();
                        } else {
                            todo!()
                        }
                        compiler.shader_scope.define_var(id, arg_ty, false);
                    }
                }
                write!(call_sig, ")").ok();
                if let Some(fnptr) = vm.heap.as_fn(fnobj) {
                    if let ScriptFnPtr::Script(fnip) = fnptr {
                        if output.recur_block.iter().any(|v| *v == fnobj) {
                            self.trap.err_recursion_not_allowed();
                            vm.code.builtins.pod.pod_void
                        } else {
                            output.recur_block.push(fnobj);
                            let ret = compiler.compile_fn(vm, output, fnip);
                            output.recur_block.pop();
                            if let Some(name) = vm.heap.pod_type_name(ret) {
                                write!(call_sig, "->{}", name).ok();
                            } else {
                                todo!()
                            }
                            
                            output.functions.push(ShaderFn {
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
                    } else { panic!() }
                } else { panic!() }
            };
            
            out.push_str(")");
            self.stack.push(&self.trap, ShaderType::Pod(ret), out);
            self.stack.free_string(method_name_prefix);
        }
    }

    fn handle_call_exec(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        if let Some(me) = self.mes.pop() {
            match me {
                ShaderMe::ArrayConstruct { args, elem_ty } => {
                    self.handle_array_construct(vm, output, args, elem_ty);
                }
                ShaderMe::Pod { pod_ty, args } => {
                    self.handle_pod_construct(vm, pod_ty, args);
                }
                ShaderMe::ScriptCall { out, name, fnobj, this, args } => {
                    self.handle_script_call(vm, output, out, name, fnobj, this, args);
                }
                ShaderMe::BuiltinCall { mut out, name, fnptr: _, args } => {
                    let ret = type_table_builtin(name, &args, &vm.code.builtins.pod, &self.trap);
                    out.push_str(")");
                    self.stack.push(&self.trap, ShaderType::Pod(ret), out);
                }
                _ => { self.trap.err_not_impl(); }
            }
        }
    }

    fn handle_method_call_args(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        let (method_ty, method_s) = self.stack.pop(&self.trap);
        let (this_ty, this_s) = self.stack.pop(&self.trap);
        self.stack.free_string(method_s);
        
        if let ShaderType::Id(method_id) = method_ty {
            if let ShaderType::Id(this_id) = this_ty {
                
                // Try to resolve as variable on shader scope
                if let Some((var, _name)) = self.shader_scope.find_var(this_id){
                    let pod_ty = var.ty;
                    // It is a Pod instance. Look up method on the type.
                    let pod_ty_data = &vm.heap.pod_types[pod_ty.index as usize];
                    let fnobj = vm.heap.value(pod_ty_data.object, method_id.into(), &self.trap);
                    
                    if let Some(fnobj) = fnobj.as_object(){
                        if let Some(fnptr) = vm.heap.as_fn(fnobj) {
                            match fnptr {
                                ScriptFnPtr::Script(_fnptr) => {
                                    let mut out = self.stack.new_string();
                                    write!(out, "{}({}", method_id, this_s).ok();
                                    self.mes.push(ShaderMe::ScriptCall {
                                        name: method_id,
                                        out,
                                        fnobj,
                                        this: ShaderThis::Pod(pod_ty),
                                        args: vec![pod_ty],
                                    });
                                }
                                ScriptFnPtr::Native(fnptr) => {
                                    let mut out = self.stack.new_string();
                                    write!(out, "{}({}", method_id, this_s).ok();
                                    self.mes.push(ShaderMe::BuiltinCall {
                                        out,
                                        name: method_id,
                                        fnptr,
                                        args: vec![pod_ty]
                                    });
                                }
                            }
                            self.stack.free_string(this_s);
                            self.maybe_pop_to_me(vm, opargs);
                            return
                        }
                    }
                }
                else{               
                    // Try to resolve as PodType in script scope
                    let value = vm.heap.scope_value(self.script_scope, this_id.into(), &self.trap);
                    if let Some(pod_ty) = vm.heap.pod_type(value) {
                        self.ensure_struct_name(vm, output, pod_ty, this_id);
                        // It is a PodType. Look up static method.
                        let pod_ty_data = &vm.heap.pod_types[pod_ty.index as usize];
                        let fnobj = vm.heap.value(pod_ty_data.object, method_id.into(), &self.trap);
                        
                        if let Some(fnobj) = fnobj.as_object(){
                            if let Some(fnptr) = vm.heap.as_fn(fnobj) {
                                match fnptr {
                                    ScriptFnPtr::Script(_fnptr) => {
                                        let mut out = self.stack.new_string();
                                        write!(out, "{}(", method_id).ok();
                                        self.mes.push(ShaderMe::ScriptCall {
                                            name: method_id,
                                            out,
                                            fnobj,
                                            this: ShaderThis::PodType(pod_ty),
                                            args: Default::default(),
                                        });
                                    }
                                    ScriptFnPtr::Native(fnptr) => {
                                        let mut out = self.stack.new_string();
                                        write!(out, "{}(", method_id).ok();
                                        self.mes.push(ShaderMe::BuiltinCall {
                                            out,
                                            name: method_id,
                                            fnptr,
                                            args: Default::default()
                                        });
                                    }
                                }
                                self.stack.free_string(this_s);
                                self.maybe_pop_to_me(vm, opargs);
                                return
                            }
                        }
                    }
                }
            }
        }
        self.stack.free_string(this_s);
        self.trap.err_not_impl();
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
            let opcode = body.parser.opcodes[self.trap.ip.index as usize];
            if let Some((opcode, args)) = opcode.as_opcode(){
                self.opcode(vm, output, opcode, args);
                self.trap.goto_next();
                self.handle_if_else_phi(vm);
            }
            else{ // id or immediate value
                self.push_immediate(opcode, &vm.code.builtins.pod);
                self.trap.goto_next();
                self.handle_if_else_phi(vm);
            }
            // alright lets see if we have a trap, ifso we can log it
            if let Some(err) = self.trap.err.take(){
                if let Some(ptr) = err.value.as_err(){
                    if let Some(loc2) = vm.code.ip_to_loc(ptr.ip){
                        log_with_level(&loc2.file, loc2.line, loc2.col, loc2.line, loc2.col, format!("{}", err.value), LogLevel::Error);
                    }
                }
            }
            if let Some(trap) = self.trap.on.take(){
                match trap{
                    
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
            Opcode::NOT=>{}
            Opcode::NEG=>self.handle_neg(vm, opargs, "-"),
            Opcode::MUL=>self.handle_float_arithmetic(vm, opargs, "*"),
            Opcode::DIV=>self.handle_float_arithmetic(vm, opargs, "/"),
            Opcode::MOD=>self.handle_float_arithmetic(vm, opargs, "%"),
            Opcode::ADD=>self.handle_float_arithmetic(vm, opargs, "+"),
            Opcode::SUB=>self.handle_float_arithmetic(vm, opargs, "-"),
            Opcode::SHL=>self.handle_int_arithmetic(vm, opargs, ">>"),
            Opcode::SHR=>self.handle_int_arithmetic(vm, opargs, "<<"),
            Opcode::AND=>self.handle_int_arithmetic(vm, opargs, "&"),
            Opcode::OR=>self.handle_int_arithmetic(vm, opargs, "|"),
            Opcode::XOR=>self.handle_int_arithmetic(vm, opargs, "^"),
                        
// ASSIGN
            Opcode::ASSIGN=>{
                let (_value_ty, value) = self.stack.pop(&self.trap);
                let (id_ty, _id) = self.stack.pop(&self.trap);
                if let ShaderType::Id(id) = id_ty{
                    if let Some((var, name)) = self.shader_scope.find_var(id){
                        if !var.is_var{
                            self.trap.err_let_is_immutable();
                        }
                        let mut s = self.stack.new_string();
                        write!(s, "{} = {}", name, value).ok();
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
                self.stack.free_string(value);
            },
            Opcode::ASSIGN_ADD=>{self.handle_float_arithmetic_assign(vm, opargs, "+=");},
            Opcode::ASSIGN_SUB=>{self.handle_float_arithmetic_assign(vm, opargs, "-=");},
            Opcode::ASSIGN_MUL=>{self.handle_float_arithmetic_assign(vm, opargs, "*=");},
            Opcode::ASSIGN_DIV=>{self.handle_float_arithmetic_assign(vm, opargs, "/=");},
            Opcode::ASSIGN_MOD=>{self.handle_float_arithmetic_assign(vm, opargs, "%=");},
            Opcode::ASSIGN_AND=>{self.handle_int_arithmetic_assign(vm, opargs, "&=");},
            Opcode::ASSIGN_OR=>{self.handle_int_arithmetic_assign(vm, opargs, "|=");},
            Opcode::ASSIGN_XOR=>{self.handle_int_arithmetic_assign(vm, opargs, "^=");},
            Opcode::ASSIGN_SHL=>{self.handle_int_arithmetic_assign(vm, opargs, ">>=");},
            Opcode::ASSIGN_SHR=>{self.handle_int_arithmetic_assign(vm, opargs, "<<=");},
            Opcode::ASSIGN_IFNIL=>{self.trap.err_not_impl();},
// ASSIGN FIELD                       
            Opcode::ASSIGN_FIELD=>{
                let (value_ty, value_s) = self.pop_resolved(vm);
                let (field_ty, field_s) = self.stack.pop(&self.trap);
                let (instance_ty, instance_s) = self.pop_resolved(vm);
                
                if let ShaderType::Id(field_id) = field_ty {
                    if let ShaderType::Pod(pod_ty) = instance_ty {
                        if let Some(ret_ty) = vm.heap.pod_field_type(pod_ty, field_id, &vm.code.builtins.pod) {
                            
                            let val_ty = value_ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                            if val_ty != ret_ty{
                                 self.trap.err_pod_type_not_matching();
                            }

                            let mut s = self.stack.new_string();
                            write!(s, "{}.{} = {}", instance_s, field_id, value_s).ok();
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
                self.stack.free_string(value_s);
                self.stack.free_string(field_s);
                self.stack.free_string(instance_s);
            },
            Opcode::ASSIGN_FIELD_ADD=>{self.handle_float_arithmetic_field_assign(vm, opargs, "+=");},
            Opcode::ASSIGN_FIELD_SUB=>{self.handle_float_arithmetic_field_assign(vm, opargs, "-=");},
            Opcode::ASSIGN_FIELD_MUL=>{self.handle_float_arithmetic_field_assign(vm, opargs, "*=");},
            Opcode::ASSIGN_FIELD_DIV=>{self.handle_float_arithmetic_field_assign(vm, opargs, "/=");},
            Opcode::ASSIGN_FIELD_MOD=>{self.handle_float_arithmetic_field_assign(vm, opargs, "%=");},
            Opcode::ASSIGN_FIELD_AND=>{self.handle_int_arithmetic_field_assign(vm, opargs, "&=");},
            Opcode::ASSIGN_FIELD_OR=>{self.handle_int_arithmetic_field_assign(vm, opargs, "|=");},
            Opcode::ASSIGN_FIELD_XOR=>{self.handle_int_arithmetic_field_assign(vm, opargs, "^=");},
            Opcode::ASSIGN_FIELD_SHL=>{self.handle_int_arithmetic_field_assign(vm, opargs, ">>=");},
            Opcode::ASSIGN_FIELD_SHR=>{self.handle_int_arithmetic_field_assign(vm, opargs, "<<=");},
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
            Opcode::ASSIGN_ME=>{
                let (val_ty, val_s) = self.stack.pop(&self.trap);
                let (id_ty, id_s) = self.stack.pop(&self.trap);
                if let ShaderType::Id(id) = id_ty{
                     if let Some(ShaderMe::Pod{args, ..}) = self.mes.last_mut(){
                         if let Some(last) = args.last(){
                             if last.name.is_none() {
                                 self.trap.err_use_only_named_or_ordered_pod_fields();
                             }
                         }
                         args.push(ShaderPodArg{
                             name: Some(id),
                             ty: val_ty,
                             s: val_s
                         });
                     }
                     else{
                         self.trap.err_unexpected();
                         self.stack.free_string(val_s);
                     }
                     self.stack.free_string(id_s);
                }
                else{
                    self.trap.err_unexpected();
                    self.stack.free_string(val_s);
                    self.stack.free_string(id_s);
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_void), String::new());
                }
            },
                                    
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
                        
                        if ty == vm.code.builtins.pod.pod_void{
                            self.out.push_str(&s);
                            self.out.push_str(";\nreturn;\n");
                        }
                        else{
                            self.out.push_str("return ");
                            self.out.push_str(&s);
                            self.out.push_str(";\n");
                        }
                        
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
                self.shader_scope.enter_scope();
                self.stack.free_string(val);
                
                self.mes.push(ShaderMe::IfBody{
                    target_ip: self.trap.ip.index + opargs.to_u32(),
                    start_pos,
                    stack_depth: self.stack.types.len(),
                    phi: None,
                    phi_type: None
                });
            }
                        
            Opcode::IF_ELSE=>{
                if let Some(ShaderMe::IfBody{target_ip, start_pos, stack_depth, phi, phi_type}) = self.mes.last_mut(){
                     if self.stack.types.len() > *stack_depth{
                         let (ty, val) = self.stack.pop(&self.trap);
                         *phi_type = Some(ty);
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
                     self.shader_scope.exit_scope();
                     self.shader_scope.enter_scope();
                     *target_ip = self.trap.ip.index + opargs.to_u32();
                }
                else{
                    self.trap.err_unexpected();
                }
            }
// Use            
            Opcode::USE=>{self.trap.err_opcode_not_supported_in_shader();},
// Field            
            Opcode::FIELD=>{
                let (field_ty, field_s) = self.stack.pop(&self.trap);
                let (instance_ty, instance_s) = self.pop_resolved(vm);
                
                if let ShaderType::Id(field_id) = field_ty {
                    if let ShaderType::Pod(pod_ty) = instance_ty {
                        if let Some(ret_ty) = vm.heap.pod_field_type(pod_ty, field_id, &vm.code.builtins.pod) {
                            let mut s = self.stack.new_string();
                            write!(s, "{}.{}", instance_s, field_id).ok();
                            self.stack.push(&self.trap, ShaderType::Pod(ret_ty), s);
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
                self.stack.free_string(field_s);
                self.stack.free_string(instance_s);
            },
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
                            let name = self.shader_scope.define_var(id, ty, false);
                            write!(self.out, "let {} = {};\n", name, value).ok();
                        }
                        else{
                            self.trap.err_no_matching_shader_type();
                        }
                    }
                    else{
                        self.trap.err_unexpected();
                    }
                }
            },
            Opcode::LET_TYPED=>{self.trap.err_not_impl();},
            Opcode::VAR_DYN=>{
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
                            let name = self.shader_scope.define_var(id, ty, true);
                            write!(self.out, "var {} = {};\n", name, value).ok();
                        }
                        else{
                            self.trap.err_no_matching_shader_type();
                        }
                    }
                    else{
                        self.trap.err_unexpected();
                    }
                }
            },
            Opcode::VAR_TYPED=>{self.trap.err_not_impl();},
// Tree search            
            Opcode::SEARCH_TREE=>{self.trap.err_opcode_not_supported_in_shader();},
// Log            
            Opcode::LOG=>{self.trap.err_opcode_not_supported_in_shader();},
// Me/Scope
            Opcode::ME=>{self.trap.err_opcode_not_supported_in_shader();},
                        
            Opcode::SCOPE=>{self.trap.err_opcode_not_supported_in_shader();},
// For            
            Opcode::FOR_1 =>{
                let (source, _) = self.stack.pop(&self.trap);
                let (val_id, _) = self.stack.pop(&self.trap);
                if let ShaderType::Range{start, end, ty} = source{
                    if let ShaderType::Id(id) = val_id{
                        self.shader_scope.enter_scope();
                        let var_name = self.shader_scope.define_var(id, ty, false);
                        write!(self.out, "for(var {0} = {1}; {0} < {2}; {0}++){{\n", var_name, start, end).ok();
                        self.mes.push(ShaderMe::ForLoop{
                            var_id: id,
                            shadow: 0
                        });
                    }
                    else{
                        self.trap.err_unexpected();
                    }
                }
                else{
                    self.trap.err_unexpected();
                }
            },
            Opcode::FOR_2 =>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::FOR_3=>{self.trap.err_opcode_not_supported_in_shader();},
            Opcode::LOOP=>{self.trap.err_not_impl();},
            Opcode::FOR_END=>{
                if let Some(me) = self.mes.pop(){
                    if let ShaderMe::ForLoop{..} = me{
                        self.out.push_str("}\n");
                        self.shader_scope.exit_scope();
                    }
                    else{
                        self.trap.err_unexpected();
                    }
                }
                else{
                     self.trap.err_unexpected();
                }
            },
            Opcode::BREAK=>{self.trap.err_not_impl();},
            Opcode::BREAKIFNOT=>{self.trap.err_not_impl();},
            Opcode::CONTINUE=>{self.trap.err_not_impl();},
// Range            
            Opcode::RANGE=>{
                let (_end_ty, end_s) = self.stack.pop(&self.trap);
                let (start_ty, start_s) = self.stack.pop(&self.trap);
                if let Some(ty) = start_ty.make_concrete(&vm.code.builtins.pod){
                    self.stack.push(&self.trap, ShaderType::Range{
                        start: start_s,
                        end: end_s,
                        ty
                    }, String::new());
                }
                else{
                     self.trap.err_no_matching_shader_type();
                }
            },
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
                ShaderMe::FnBody{ ret:_} | ShaderMe::ForLoop{..} | ShaderMe::IfBody{..}=>{
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
                             v.ty
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
                ShaderMe::ScriptCall{out, args, ..}=>{
                    let (ty, s) = self.stack.pop(&self.trap);
                    if args.len() > 0 {
                        out.push_str(", ");
                    }
                    if let ShaderType::Id(id) = ty{
                         if let Some((v, _name)) = self.shader_scope.find_var(id){
                             args.push(v.ty);
                         }
                         else{
                             self.trap.err_not_found();
                         }
                    }
                    else if let Some(ty) = ty.make_concrete(&vm.code.builtins.pod){
                        args.push(ty);
                    }
                    else{
                        self.trap.err_no_matching_shader_type();
                    }
                    out.push_str(&s);
                    self.stack.free_string(s);
                }
                ShaderMe::BuiltinCall{out, args, ..}=>{
                    let (ty, s) = self.stack.pop(&self.trap);
                    if args.len() > 0 {
                        out.push_str(", ");
                    }
                    if let ShaderType::Id(id) = ty{
                         if let Some((v, _name)) = self.shader_scope.find_var(id){
                             args.push(v.ty);
                         }
                         else{
                             self.trap.err_not_found();
                         }
                    }
                    else if let Some(ty) = ty.make_concrete(&vm.code.builtins.pod){
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
