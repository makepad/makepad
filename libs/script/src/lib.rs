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
pub mod sys_fns;
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

pub fn test(){
    
    let time = std::time::Instant::now();
    
    let _code = "
        let View = {@view}
        let Window = {@window}
        let Button = {@button}
        let MyWindow = Window{
            size: 1.0
            $b1: Button{}
            $body: View{}
            $b2: Button{}
        }
        let x = MyWindow{
            $b1 <: View{}
        }
        
        let x = if true 1 else 0
        for v in 0..10{
            
        }
        let x = x{};
        for v in [1,2,3,4] ~v;
        ~x;
    ";
    
    let code = "
        let x = @range{start:1 end:10000 step:1};
        for i in x{
            Button{
                prop1:i
            }
        }
        ~@finished;
    ";
    
    let code = "
        let fib = |n| if n <= 1 n else fib(n - 1) + fib(n - 2)
        ~fib(38);
    ";
    let dt = std::time::Instant::now();
    let mut interp = Script::new();
    interp.run(&code);
    println!("Duration {}", dt.elapsed().as_secs_f64())
    
}
