pub use makepad_script_derive::*;

pub mod tokenizer; 
pub mod object;
pub mod value;
pub mod id;
pub mod colorhex;
pub mod parser;
pub mod interpreter;
pub mod heap;

// lifetimes
// stack
// locals
// globals

// args
// locals (let)
// objecttree

pub fn test(){
    use crate::parser::*;
    use crate::interpreter::*;
    let code = "
    
    t = 1.0
    x = {
        x: 1.0
    }
    ";
        
    // Todo = Todo{done:1*x[1].y(2+3)};
        
    
    let mut interpreter = ScriptInterpreter::default();
    let mut parser = ScriptParser::default();
    parser.parse(&code, &mut interpreter.heap);
    parser.tok.dump_tokens(&interpreter.heap);
    interpreter.run(&parser);
}
