use crate::makepad_live_id::*;
use crate::heap::*;
use crate::value::*;
use crate::parser::*;
use crate::tokenizer::*;
use crate::methods::*;
use crate::thread::*;
use crate::native::*;
use crate::modules::*;
use std::cell::RefCell;
use std::any::Any;

#[derive(Default)]
pub struct ScriptBlock{
    pub cargo_manifest_path: String,
    pub module_path: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub values: Vec<ScriptValue>,
}

pub enum ScriptSource{
    Block{
        block: ScriptBlock,
    },
    Streaming{
        code: String,
    }
}

pub struct ScriptBody{
    pub source: ScriptSource,
    pub tokenizer: ScriptTokenizer,
    pub parser: ScriptParser,
    pub scope: ScriptObject,
    pub me: ScriptObject,
}

pub struct ScriptCode{
    pub type_methods: ScriptTypeMethods,
    pub builtins: ScriptBuiltins,
    pub native: RefCell<ScriptNative>,
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
                    if let ScriptSource::Block{block} = &body.source{
                        return Some(
                            ScriptLoc{
                                file: block.file.as_str(),
                                line: rc.0 + block.line as u32 + 1,
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


pub struct ScriptVm<'a>{
    pub host: &'a mut dyn Any,
    pub thread: &'a mut ScriptThread,
    pub code: &'a ScriptCode,
    pub heap: &'a mut ScriptHeap
}

impl <'a> ScriptVm<'a>{
    pub fn call(&mut self,fnobj:ScriptValue, args:&[ScriptValue])->ScriptValue{
        self.thread.call(self.heap, self.code, self.host, fnobj, args)
    }
          
    pub fn cast_to_f64(&self, v:ScriptValue)->f64{
        self.heap.cast_to_f64(v, self.thread.trap.ip)
    }
    
        
    pub fn new_module(&mut self, id:LiveId)->ScriptObject{
        self.heap.new_module(id)
    }
    
    pub fn add_fn<F>(&mut self, module:ScriptObject, method:LiveId, args:&[(LiveId, ScriptValue)], f: F) 
    where F: Fn(&mut ScriptVm, ScriptObject)->ScriptValue + 'static{
        self.code.native.borrow_mut().add_fn(&mut self.heap, module, method, args, f)
    }
}

pub struct ScriptVmBase{
    pub void: usize,
    pub code: ScriptCode,
    pub global: ScriptObject,
    pub heap: ScriptHeap,
    pub threads: Vec<ScriptThread>,
}

impl ScriptVmBase{
    pub fn as_ref<'a>(&'a mut self)->ScriptVm<'a>{
        ScriptVm{
            host: &mut self.void,
            code: &self.code,
            heap: &mut self.heap,
            thread: &mut self.threads[0]
        }
    }
    
    pub fn as_ref_host<'a>(&'a mut self, host:&'a mut dyn Any)->ScriptVm<'a>{
        ScriptVm{
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
            void: 0,
            code:ScriptCode{
                builtins,
                type_methods,
                native: RefCell::new(native),
                bodies: Default::default(),
            },
            threads: vec![ScriptThread::new()],
            global,
            heap: heap,
        }
    }
        
        
    pub fn add_script_block(&mut self, new_block:ScriptBlock)->u16{
        let scope = self.heap.new_with_proto(id!(scope).into());
        self.heap.set_object_deep(scope);
        self.heap.set_value_def(scope, id!(mod).into(), self.heap.modules.into());
        self.heap.set_value_def(scope, id!(global).into(), self.global.into());
        let me = self.heap.new_with_proto(id!(root_me).into());
        
        let new_body = ScriptBody{
            source: ScriptSource::Block{block:new_block},
            tokenizer: ScriptTokenizer::default(),
            parser: ScriptParser::default(),
            scope,
            me,
        };
        for i in 0..self.code.bodies.len(){
            let body = &mut self.code.bodies[i];
            if let ScriptSource::Block{block} = &body.source{
                if let ScriptSource::Block{block:new_block} = &new_body.source{
                    if  block.file == new_block.file &&
                        block.line == new_block.line &&
                        block.column == new_block.column{
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
    
    pub fn eval(&mut self, block: ScriptBlock, host:&mut dyn Any){
        let body_id = self.add_script_block(block);
        let body = &mut self.code.bodies[body_id as usize];
        
        if let ScriptSource::Block{block} = &body.source{
            body.tokenizer.tokenize(&block.code, &mut self.heap);
            body.parser.parse(&body.tokenizer.tokens, &block.values);
            // lets point our thread to it
            self.threads[0].run_root(&mut self.heap, &self.code, host, body_id)
        }
    }
}