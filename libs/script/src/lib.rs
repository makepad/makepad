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
    /*
    let code = "
        let va = [@prop1, @prop2, @prop3];
        .z = {prop:@prop6},
        .z[.z.prop] = 5,
        $thing.prop();
        let x = z{key:43}
        let t = x + 2;
        $thing.prop = 10.0;
    ";*/
    let code = "
        let x = {'123'};
    ";
    
    let mut interp = ScriptInterpreter::new();
    let mut parser = ScriptParser::default();
    parser.parse(&code, &mut interp.heap);
    parser.tok.dump_tokens(&interp.heap);
    interp.run(&parser);
}
/*
pub const OP_(\w+): Value = Value\(Self::TYPE_OPCODE \| (\d+)\);
pub const ID_$1:u64 = $2;pub const OP_$1: Value = Value(Self::TYPE_OPCODE | Self::ID_$1);*/