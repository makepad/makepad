pub use makepad_script_derive::*;

pub mod tokenizer; 
pub mod object;
pub mod value;
pub mod id;
pub mod colorhex;
pub mod parser;
pub mod interpreter;
pub mod heap;

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


pub fn test(){
    use crate::parser::*;
    use crate::interpreter::*;
    let code = "
        let t = 1.0
        Todo = {
            done: t
        };
    ";
        
    // Todo = Todo{done:1*x[1].y(2+3)};
    
    let mut interpreter = ScriptInterpreter::new();
    let mut parser = ScriptParser::default();
    parser.parse(&code, &mut interpreter.heap);
    parser.tok.dump_tokens(&interpreter.heap);
    interpreter.run(&parser);
}
