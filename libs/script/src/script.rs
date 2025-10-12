use makepad_script_derive::*;
use crate::id::*;
use crate::parser::ScriptParser;
use crate::value::*;
use crate::heap::*;
use crate::methods::*;
use crate::thread::*;

pub struct Script{
    pub methods: ScriptMethods,
    pub parser: ScriptParser,
    pub threads: Vec<ScriptThread>,
    pub heap: ScriptHeap,
    pub modules: ObjectPtr,
    pub global: ObjectPtr,
    pub scope: ObjectPtr,
}

impl Script{
    pub fn new()->Self{
        let mut heap = ScriptHeap::new();
        let methods = ScriptMethods::new(&mut heap);
        
        let scope = heap.new_object_with_proto(id!(scope).into());
        let global = heap.new_object_with_proto(id!(global).into());
        let modules = heap.new_object_with_proto(id!(mod).into());
        Self{
            modules,
            methods,
            parser: Default::default(),
            threads: vec![ScriptThread::new(scope, global, modules)],
            scope,
            global: heap.new_object(0),
            heap: heap,
        }
    }
    
    pub fn parse(&mut self, code:&str){
        self.parser.parse(code, &mut self.heap);
        self.parser.tok.dump_tokens(&self.heap);
    }
    
    pub fn run(&mut self, code: &str){
        self.parse(code);
        self.threads[0].run(&self.parser, &mut self.heap, &self.methods)
    }
}