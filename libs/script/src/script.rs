use crate::makepad_value::id::*;
use crate::heap::*;
use crate::makepad_value::value::*;
use crate::makepad_value_derive::*;
use crate::parser::ScriptParser;
use crate::methods::*;
use crate::thread::*;
use crate::native::*;
use crate::modules::*;

// the script! macro
#[derive(Default)]
pub struct ScriptBody{
    pub cargo_manifest_path: String,
    pub module_path: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub values: Vec<Value>,
}

pub struct ScriptCtx{
    pub methods: ScriptMethods,
    pub modules: ScriptModules,
    pub builtins: ScriptBuiltins,
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
        let builtins = ScriptBuiltins::new(&mut heap, &modules);
        
        Self{
            ctx:ScriptCtx{
                builtins,
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
    
    pub fn parse(&mut self, body:&ScriptBody){
        self.ctx.parser.parse(&body.code, &mut self.heap, &body.values);
        self.ctx.parser.tok.dump_tokens(&self.heap);
    }
    
    pub fn run(&mut self, code: &ScriptBody){
        self.parse(code);
        self.threads[0].run(&mut self.heap, &self.ctx)
    }
}