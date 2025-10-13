pub use makepad_value;
pub use makepad_value::makepad_value_derive;
pub mod tokenizer; 
pub mod object;
pub mod colorhex;
pub mod parser;
pub mod heap;
pub mod string;
pub mod methods;
pub mod modules;
pub mod native;
pub mod script;
pub mod thread;
pub mod thread_opcode;

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


impl RustTest{
    fn ty()->u32{1}
}

use crate::script::*;
use makepad_script_derive::*;

pub fn test(){
    
    let code = script!{
        let x = [@view,@bla]
        for sym in x t[sym]
        
        let View = {@view}
        let Window = {@window}
        let Button = {@button}
        let MyWindow = #(RustTest::ty()){
            size: 1.0
            $b1: Button{}
            $body: View{}
            $b2: Button{}
        }
        let x = MyWindow{
            $b1 : Checkbox{}
        }
        let x = if true 1 else 0
        let x = x{};
        for v in [1 2 3 4] ~v
        ~x;
    };
    
    let _code = script!{
        if true let x = 5;
        ~x;
    };
    
    let _code = script!{
        let fib = |n| if n <= 1 n else fib(n - 1) + fib(n - 2)
        ~fib(38);
    };
    
    let dt = std::time::Instant::now();
    let mut vm = ScriptVm::new();
    vm.eval(code);
    println!("Duration {}", dt.elapsed().as_secs_f64())
    
}
