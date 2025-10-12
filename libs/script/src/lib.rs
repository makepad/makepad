pub use makepad_script_derive::*;

pub mod tokenizer; 
pub mod object;
pub mod value;
pub mod id;
pub mod colorhex;
pub mod parser;
pub mod heap;
pub mod opcode;
pub mod string;
pub mod methods;
pub mod modules;
pub mod native;
pub mod script;
pub mod thread;

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

use crate::script::*;

pub fn test(){
    
    let _code = "
        let x = [@view,@bla]
        for sym in x t[sym]
        
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
            $b1 <: Checkbox{}
        }
        let x = if true 1 else 0
        for v in 0..10{
        }
        let x = x{};
        for v in [1 2 3 4] ~v
        ~x;
    ";
    
    let code = "
        scope.import(mod.math);
        mod.math.ty();
        let View = {@view}
        let Button = {@button}
        let t = Button{}
        ~(t is Button);
        //for i in 100 ~sin(i);
    ";
    
    let _code = "
        let x = {1,nil,3}
        let fib = |n| if n <= 1 n else fib(n - 1) + fib(n - 2)
        ~fib(38);
    ";
    let dt = std::time::Instant::now();
    let mut interp = Script::new();
    interp.run(&code);
    println!("Duration {}", dt.elapsed().as_secs_f64())
    
}
