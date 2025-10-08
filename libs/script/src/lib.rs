pub use makepad_script_derive::*;

pub mod tokenizer; 
pub mod object;
pub mod value;
pub mod id;
pub mod colorhex;
pub mod parser;
pub mod interpreter;
pub mod heap;
pub mod opcode;
pub mod interop;
pub mod string;
// 'locals'
//
// locals
// globals
// symbols

// args
// locals (let)
// objecttree
/*
t = 1.0
x = {
    t: 1.0
    let x = this.t 
    t = 2.0
    t += 1.0 -> nil
    x: t
}
on_click: ||{
    constructor => closure
    call => instructions
}*/

pub struct RustTest{
    _prop: f64    
}

use crate::interop::*;
use crate::interpreter::*;
use crate::value::*;
use crate::id::*;

impl ScriptCall for RustTest{
    // deserialize self from obj?
    fn update_fields(&mut self, _obj: ObjectPtr){
    }
    
    fn call_method(&mut self,_scx:&ScriptCx, method: Id, _args: ObjectPtr)->Value{
        match method{
            id!(on_click)=>{
                return Value::NIL
            }
            _=>{// unknown call
                return Value::NIL
            }
        }
    }
}

// object string float vec2 vec3 vec4 bool color nil true false
pub fn test(){
    
    let time = std::time::Instant::now();
    
    let code = "
        let a = 1;
        let b = 2;
        let c = 3;
        let x={a,b,c};
        ~x.b;
    ";
        
    let _code = "
    let View = {@view}
    let Window = {@window}
    let Button = {@button}
    let x = Window{
        body: View{
            View{@myview}
        }
    }
    ~x.z.ty()
            
    ;
          
    ";
    let _code = "
        let fib = |n|{
            return if(n <= 1){
                n
            }
            else {
                fib(n - 1) + fib(n - 2)
            }
        }
        ~fib(38);
    ";
    
    let mut interp = Script::new();
    interp.run(&code);
    
    println!("{:?}", time.elapsed().as_secs_f64());
}
/*
pub const OP_(\w+): Value = Value\(Self::TYPE_OPCODE \| (\d+)\);
pub const ID_$1:u64 = $2;pub const OP_$1: Value = Value(Self::TYPE_OPCODE | Self::ID_$1);*/