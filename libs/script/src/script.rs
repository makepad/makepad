use crate::makepad_value::id::*;
use crate::heap::*;
use crate::makepad_value::value::*;
use crate::makepad_value_derive::*;
use crate::parser::*;
use crate::tokenizer::*;
use crate::methods::*;
use crate::thread::*;
use crate::native::*;
use crate::modules::*;

pub struct ScriptRust{
    pub cargo_manifest_path: String,
    pub module_path: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub values: Vec<Value>,
}

// the script! macro
pub enum ScriptSource{
    Rust{
        rust: ScriptRust,
    },
    Streaming{
        code: String,
    }
}

pub struct ScriptBody{
    pub source: ScriptSource,
    pub tokenizer: ScriptTokenizer,
    pub parser: ScriptParser,
    pub scope: ObjectPtr,
    pub me: ObjectPtr,
}

pub struct ScriptCtx{
    pub methods: ScriptMethods,
    pub modules: ScriptModules,
    pub builtins: ScriptBuiltins,
    pub native: ScriptNative,
    pub bodies: Vec<ScriptBody>,
}

pub struct ScriptVm{
    pub ctx: ScriptCtx,
    pub global: ObjectPtr,
    pub heap: ScriptHeap,
    pub threads: Vec<ScriptThread>,
}

impl ScriptVm{
    pub fn new()->Self{
        let mut heap = ScriptHeap::new();
        let mut native = ScriptNative::default();
        let methods = ScriptMethods::new(&mut heap, &mut native);
        let modules = ScriptModules::new(&mut heap, &mut native);
        let global = heap.new_object_with_proto(id!(global).into());
        let builtins = ScriptBuiltins::new(&mut heap, &modules);
        
        Self{
            ctx:ScriptCtx{
                builtins,
                modules,
                methods,
                native,
                bodies: Default::default(),
            },
            threads: vec![ScriptThread::new()],
            global,
            heap: heap,
        }
    }
    
    pub fn add_rust_body(&mut self, new_rust:ScriptRust)->u16{
        let scope = self.heap.new_object_with_proto(id!(scope).into());
        self.heap.set_object_value(scope, id!(mod).into(), self.ctx.modules.obj.into());
        self.heap.set_object_value(scope, id!(global).into(), self.global.into());
        let me = self.heap.new_object_with_proto(id!(root_me).into());
        
        let new_body = ScriptBody{
            source: ScriptSource::Rust{rust:new_rust},
            tokenizer: ScriptTokenizer::default(),
            parser: ScriptParser::default(),
            scope,
            me,
        };
        for i in 0..self.ctx.bodies.len(){
            let body = &mut self.ctx.bodies[i];
            if let ScriptSource::Rust{rust} = &body.source{
                if let ScriptSource::Rust{rust:new_rust} = &new_body.source{
                    if  rust.file == new_rust.file &&
                        rust.line == new_rust.line &&
                        rust.column == new_rust.column{
                        *body = new_body;
                        return i as u16
                    }
                }
            }
        }
        let i = self.ctx.bodies.len();
        self.ctx.bodies.push(new_body);
        i as u16
    }
    
    pub fn eval(&mut self, new_rust: ScriptRust){
        let body_id = self.add_rust_body(new_rust);
        let body = &mut self.ctx.bodies[body_id as usize];
        
        if let ScriptSource::Rust{rust} = &body.source{
            body.tokenizer.tokenize(&rust.code, &mut self.heap);
            body.parser.parse(&body.tokenizer.tokens, &mut self.heap, &rust.values);
            // lets point our thread to it
            self.threads[0].run(&mut self.heap, &self.ctx, body_id)
        }
    }
}