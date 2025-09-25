pub use makepad_script_derive::*;

pub mod tokenizer; 
pub mod object;
pub mod value;
pub mod id;
pub mod colorhex;
pub mod parser;
pub mod interpreter;

pub fn test(){
    use crate::parser::*;
    use crate::interpreter::*;
    let code = "
    Todo = Todo{done:1*(2+3)};
    ";
    let mut parser = ScriptParser::default();
    parser.parse(&code);
    parser.tok.dump_tokens();
    let mut interpreter = ScriptInterpreter::default();
    interpreter.run(&parser);
}
