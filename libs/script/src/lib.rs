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

// object string float vec2 vec3 vec4 bool color nil true false
pub fn test(){
    use crate::parser::*;
    use crate::interpreter::*;
    let code = "
        let x = |x:type| y
    ";
    
    let mut interp = ScriptInterpreter::new();
    let mut parser = ScriptParser::default();
    parser.parse(&code, &mut interp.heap);
    parser.tok.dump_tokens(&interp.heap);
    interp.run(&parser);
}
