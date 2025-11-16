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

// we collect functions, we do the type inferencing 
// and we just emit topdown.

pub fn define_shader_module(heap:&mut ScriptHeap, native:&mut ScriptNative){
    let math = heap.new_module(id!(shader));
        
    native.add_method(heap, math, id!(compile), script_args!(code=NIL), |vm, args|{
        // lets fetch the code
        let fnobj = script_value!(vm, args.code);
        let mut compiler = ShaderCompiler{
            stack_limit: 1000000,
            ..ShaderCompiler::default()
        };
        if let Some(fnobj) = fnobj.as_object(){
            if let Some(fnptr) = vm.heap.as_fn(fnobj){
                if let ScriptFnPtr::Script(fnip) = fnptr{
                    compiler.compile(vm, fnip);
                    return NIL
                }
            }
        }
        // trap error
        NIL
    });
}

#[derive(Debug)]
pub enum ShaderMe{
    Call{this:Option<ShaderValueType>, args:ScriptObject},
    Pod{pod:ScriptPodType, offset:ScriptPodOffset},
}

#[derive(Debug)]
pub enum ShaderValueType{
    _Function,
    Pod(ScriptPodType),
    Id(LiveId),
    AbstractInt,
    AbstractFloat,
    Error(ScriptValue)
}

impl ShaderValueType{
    fn from_value(value: ScriptValue, trap:&ScriptTrap, builtins:&ScriptPodBuiltins)->Self{
        if let Some(ty) = builtins.value_to_exact_type(value){
            return Self::Pod(ty)
        }
        if let Some(v) = value.as_f64(){ // abstract int or float
            if v.fract() != 0.0{
                return Self::AbstractFloat
            }
            else{
                return Self::AbstractInt
            }
        }
        if let Some(id) = value.as_id(){
            return Self::Id(id)
        }
        Self::Error(trap.err_no_matching_shader_type())
    }
}

#[derive(Default)]
struct ShaderCompiler{
    pub stack_limit: usize,
    pub stack: Vec<ShaderValueType>,
    pub trap: ScriptTrap,
}

impl ShaderCompiler{
        
    pub fn _pop_stack_value(&mut self)->ShaderValueType{
        if let Some(value) = self.stack.pop(){
            return value
        }
        else{
            ShaderValueType::Error(self.trap.err_stack_underflow())
        }
    }
        
    pub fn push_stack_value(&mut self, value:ShaderValueType){
        if self.stack.len() > self.stack_limit{
            self.trap.err_stack_overflow();
        }
        else{
            self.stack.push(value);
        }
    }
    
    fn compile(&mut self, vm:&mut ScriptVm, fnip:ScriptIp){
        // alright lets go trace the opcodes
        self.trap.ip = fnip;
        self.trap.in_rust = true;
        let bodies = vm.code.bodies.borrow();
        let mut body = &bodies[self.trap.ip.body as usize];
        while (self.trap.ip.index as usize) < body.parser.opcodes.len(){
            let opcode = body.parser.opcodes[self.trap.ip.index as usize];
            if let Some((opcode, args)) = opcode.as_opcode(){
                self.opcode(vm, opcode, args);
                self.trap.goto_next();
            }
            else{ // id or immediate value
                self.push_stack_value(ShaderValueType::from_value(opcode, &self.trap, &vm.code.builtins.pod));
                self.trap.goto_next();
            }
            // alright lets see if we have a trap, ifso we can log it
            if let Some(trap) = self.trap.on.take(){
                match trap{
                    ScriptTrapOn::Error{value,..}=>{
                        // check if we have a try clause
                        if let Some(ptr) = value.as_err(){
                            if let Some(loc2) = vm.code.ip_to_loc(ptr.ip){
                                println!("{} {} - {}", value, loc2, opcode);
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
    
    fn opcode(&mut self, _vm:&mut ScriptVm, opcode: Opcode, opargs:OpcodeArgs){
        match opcode{
// Arithmetic
            Opcode::NOT=>{self.trap.err_not_impl();},
            Opcode::NEG=>{self.trap.err_not_impl();},
            Opcode::MUL=>{self.trap.err_not_impl();},
            Opcode::DIV=>{self.trap.err_not_impl();},
            Opcode::MOD=>{self.trap.err_not_impl();},
            Opcode::ADD=>{
                // we pop 2 operands, and write 2 operands
                // here we just process the scope lookup and type handling
                
                
            }
            Opcode::SUB=>{self.trap.err_not_impl();},
            Opcode::SHL=>{self.trap.err_not_impl();},
            Opcode::SHR=>{self.trap.err_not_impl();},
            Opcode::AND=>{self.trap.err_not_impl();},
            Opcode::OR=>{self.trap.err_not_impl();},
            Opcode::XOR=>{self.trap.err_not_impl();},
                        
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
            Opcode::CALL_ARGS=>{self.trap.err_not_impl();},
            Opcode::CALL_EXEC | Opcode::METHOD_CALL_EXEC=>{self.trap.err_not_impl();},
            Opcode::METHOD_CALL_ARGS=>{self.trap.err_not_impl();},
// Fn def
            Opcode::FN_ARGS=>{self.trap.err_not_impl();},
            Opcode::FN_LET_ARGS=>{self.trap.err_not_impl();},
            Opcode::FN_ARG_DYN=>{self.trap.err_not_impl();},
            Opcode::FN_ARG_TYPED=>{self.trap.err_not_impl();},
            Opcode::FN_BODY=>{self.trap.err_not_impl();},
            Opcode::RETURN=>{
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
                        
            Opcode::POP_TO_ME=>{self.trap.err_not_impl();},
// Array index            
            Opcode::ARRAY_INDEX=>{self.trap.err_not_impl();},
// Let                   
            Opcode::LET_DYN=>{self.trap.err_not_impl();},
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
        if opargs.is_pop_to_me(){
        }
        
    }
}