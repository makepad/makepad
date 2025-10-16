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
use std::any::Any;

#[derive(Default)]
pub struct ScriptRust{
    pub cargo_manifest_path: String,
    pub module_path: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub values: Vec<Value>,
}

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

pub struct ScriptCode{
    pub type_methods: ScriptTypeMethods,
    pub builtins: ScriptBuiltins,
    pub native: ScriptNative,
    pub bodies: Vec<ScriptBody>,
}

pub struct ScriptLoc<'a>{
    pub file: &'a str,
    pub col: u32,
    pub line: u32,
}

impl<'a> std::fmt::Debug for ScriptLoc<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}


impl<'a> std::fmt::Display for ScriptLoc<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.col)
    }
}

impl ScriptCode{
    pub fn ip_to_loc(&self, ip:ScriptIp)->Option<ScriptLoc>{
        if let Some(body) = self.bodies.get(ip.body as usize){
            if let Some(Some(index)) = body.parser.source_map.get(ip.index as usize){
                if let Some(rc) = body.tokenizer.token_index_to_row_col(*index){
                    if let ScriptSource::Rust{rust} = &body.source{
                        return Some(
                            ScriptLoc{
                                file: rust.file.as_str(),
                                line: rc.0 + rust.line as u32 + 1,
                                col: rc.1
                            }
                        )
                    }else{
                        return Some(ScriptLoc{
                            file: "generated",
                            line: rc.0,
                            col: rc.1
                        })
                    };
                }
            }
        }
        return Some(ScriptLoc{
            file: "unknown",
            line: ip.body as _,
            col: ip.index as _
        })
    }
}


pub struct ScriptCtx<'a>{
    pub host: &'a mut dyn Any,
    pub thread: &'a mut ScriptThread,
    pub code: &'a ScriptCode,
    pub heap: &'a mut ScriptHeap
}

impl <'a> ScriptCtx<'a>{
      pub fn call(&mut self,fnobj:Value, args:&[Value])->Value{
          self.thread.call(self.heap, self.code, self.host, fnobj, args)
      }
}

pub struct ScriptVm{
    pub code: ScriptCode,
    pub global: ObjectPtr,
    pub heap: ScriptHeap,
    pub threads: Vec<ScriptThread>,
}

impl ScriptVm{
    pub fn ctx<'a>(&'a mut self, host:&'a mut dyn Any)->ScriptCtx<'a>{
        ScriptCtx{
            host,
            code: &self.code,
            heap: &mut self.heap,
            thread: &mut self.threads[0]
        }
    }
    
    pub fn new()->Self{
        let mut heap = ScriptHeap::empty();
        let mut native = ScriptNative::default();
        let type_methods = ScriptTypeMethods::new(&mut heap, &mut native);
        define_math_module(&mut heap, &mut native);
        define_std_module(&mut heap, &mut native);
    
        let global = heap.new_with_proto(id!(global).into());
        let builtins = ScriptBuiltins::new(&mut heap);
        
        Self{
            code:ScriptCode{
                builtins,
                type_methods,
                native,
                bodies: Default::default(),
            },
            threads: vec![ScriptThread::new()],
            global,
            heap: heap,
        }
    }
    
    pub fn add_rust_body(&mut self, new_rust:ScriptRust)->u16{
        let scope = self.heap.new_with_proto(id!(scope).into());
        self.heap.set_object_deep(scope);
        self.heap.set_value(scope, id!(mod).into(), self.heap.modules.into());
        self.heap.set_value(scope, id!(global).into(), self.global.into());
        let me = self.heap.new_with_proto(id!(root_me).into());
        
        let new_body = ScriptBody{
            source: ScriptSource::Rust{rust:new_rust},
            tokenizer: ScriptTokenizer::default(),
            parser: ScriptParser::default(),
            scope,
            me,
        };
        for i in 0..self.code.bodies.len(){
            let body = &mut self.code.bodies[i];
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
        let i = self.code.bodies.len();
        self.code.bodies.push(new_body);
        i as u16
    }
    
    pub fn eval(&mut self, new_rust: ScriptRust, host:&mut dyn Any){
        let body_id = self.add_rust_body(new_rust);
        let body = &mut self.code.bodies[body_id as usize];
        
        if let ScriptSource::Rust{rust} = &body.source{
            body.tokenizer.tokenize(&rust.code, &mut self.heap);
            body.parser.parse(&body.tokenizer.tokens, &mut self.heap, &rust.values);
            // lets point our thread to it
            self.threads[0].run_root(&mut self.heap, &self.code, host, body_id)
        }
    }
}