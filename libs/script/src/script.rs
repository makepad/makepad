use makepad_script_derive::*;
use crate::id::*;
use crate::parser::ScriptParser;
use crate::value::*;
use crate::heap::*;
use crate::methods::*;
use crate::thread::*;
use crate::native::*;
use crate::modules::*;

pub struct ScriptCtx{
    pub methods: ScriptMethods,
    pub modules: ScriptModules,
    pub native: ScriptNative,
    pub parser: ScriptParser,
}

pub struct Script{
    pub ctx: ScriptCtx,
    pub heap: ScriptHeap,
    pub threads: Vec<ScriptThread>,
    pub scope: ObjectPtr,
}

impl Script{
    pub fn new()->Self{
        let mut heap = ScriptHeap::new();
        let mut native = ScriptNative::default();
        let methods = ScriptMethods::new(&mut heap, &mut native);
        let modules = ScriptModules::new(&mut heap, &mut native);
        let scope = heap.new_object_with_proto(id!(scope).into());
        let global = heap.new_object_with_proto(id!(global).into());
        heap.set_object_value(scope, id!(mod).into(), modules.obj.into());
        heap.set_object_value(scope, id!(global).into(), global.into());
                
        Self{
            ctx:ScriptCtx{
                modules,
                methods,
                native,
                parser: Default::default(),
            },
            threads: vec![ScriptThread::new(&mut heap, scope)],
            scope,
            heap: heap,
        }
    }
    
    pub fn parse(&mut self, code:&str){
        self.ctx.parser.parse(code, &mut self.heap);
        self.ctx.parser.tok.dump_tokens(&self.heap);
    }
    
    pub fn run(&mut self, code: &str){
        self.parse(code);
        self.threads[0].run(&mut self.heap, &self.ctx)
    }
}