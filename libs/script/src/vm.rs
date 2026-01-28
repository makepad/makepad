use crate::makepad_live_id::*;
use crate::heap::*;
use crate::value::*;
use crate::parser::*;
use crate::tokenizer::*;
use crate::thread::*;
use crate::native::*;
use crate::mod_std::*;
use crate::mod_math::*;
use crate::mod_pod::*;
use crate::mod_shader::*;
use crate::object::*;
use crate::function::*;
use std::cell::RefCell;
use std::any::Any;
use std::collections::HashMap;

#[derive(Default, Debug)]
pub struct ScriptMod{
    pub cargo_manifest_path: String,
    pub module_path: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub values: Vec<ScriptValue>,
}

pub enum ScriptSource{
    Mod(ScriptMod),
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

pub struct ScriptBuiltins{
    pub range: ScriptObject,
    pub pod: ScriptPodBuiltins,
}

impl ScriptBuiltins{
    pub fn new(heap:&mut ScriptHeap, pod: ScriptPodBuiltins)->Self{
        Self{
            range: heap.value_path(heap.modules, ids!(std.Range),&mut Default::default()).as_object().unwrap(),
            pod,
        }
    }
}

pub struct ScriptCode{
    pub builtins: ScriptBuiltins,
    pub native: RefCell<ScriptNative>,
    pub bodies: RefCell<Vec<ScriptBody>>,
    pub crate_manifests: RefCell<HashMap<String, String>>,
}

pub struct ScriptLoc{
    pub file: String,
    pub col: u32,
    pub line: u32,
}

impl std::fmt::Debug for ScriptLoc{
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}


impl std::fmt::Display for ScriptLoc{
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.col)
    }
}

impl ScriptCode{
    pub fn ip_to_loc(&self, ip:ScriptIp)->Option<ScriptLoc>{
        if let Some(body) = self.bodies.borrow().get(ip.body as usize){
            if let Some(Some(index)) = body.parser.source_map.get(ip.index as usize){
                if let Some(rc) = body.tokenizer.token_index_to_row_col(*index){
                    if let ScriptSource::Mod(script_mod) = &body.source{
                        return Some(
                            ScriptLoc{
                                file: script_mod.file.clone(),
                                line: rc.0 + script_mod.line as u32,
                                col: rc.1
                            }
                        )
                    }else{
                        return Some(ScriptLoc{
                            file: "generated".into(),
                            line: rc.0,
                            col: rc.1
                        })
                    };
                }
            }
        }
        return Some(ScriptLoc{
            file: "unknown".into(),
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
    pub fn with_vm<R,F:FnOnce(&mut ScriptVm)->R>(&mut self, f:F)->R{
        f(self)
    }
    
    pub fn call(&mut self,fnobj:ScriptValue, args:&[ScriptValue])->ScriptValue{
        self.thread.call(self.heap, self.code, self.host, fnobj, args)
    }
    
    /// Checks if the value has an apply transform and calls it, returning the transformed value.
    /// Returns None if no transform exists, Some(transformed) if a transform was applied.
    pub fn call_apply_transform(&mut self, value: ScriptValue) -> Option<ScriptValue> {
        if let Some(obj) = value.as_object() {
            if let Some(ni) = self.heap.objects[obj.index as usize].tag.as_apply_transform() {
                let native = self.code.native.borrow();
                let result = (*native.functions[ni.index as usize])(
                    &mut ScriptVm {
                        host: self.host,
                        heap: self.heap,
                        thread: self.thread,
                        code: self.code
                    },
                    obj
                );
                drop(native);
                return Some(result);
            }
        }
        else if let Some(arr) = value.as_array() {
            if let Some(ni) = self.heap.arrays[arr.index as usize].tag.as_apply_transform() {
                // For arrays, we need to create a temporary args object
                let args_obj = self.heap.new_object();
                self.heap.set_value_def(args_obj, id!(self).into(), value);
                let native = self.code.native.borrow();
                let result = (*native.functions[ni.index as usize])(
                    &mut ScriptVm {
                        host: self.host,
                        heap: self.heap,
                        thread: self.thread,
                        code: self.code
                    },
                    args_obj
                );
                drop(native);
                return Some(result);
            }
        }
        None
    }
    
    pub fn resume(&mut self)->ScriptValue{
        self.thread.is_paused = false;
        self.thread.run_core(self.heap, self.code, self.host)
    }
          
    pub fn cast_to_f64(&self, v:ScriptValue)->f64{
        self.heap.cast_to_f64(v, self.thread.trap.ip)
    }
    
    pub fn handle_type(&self, id:LiveId)->ScriptHandleType{
        *self.code.native.borrow().handle_type.get(&id).unwrap()
    }
    
    pub fn new_handle_type(&mut self, id:LiveId)->ScriptHandleType{
        self.code.native.borrow_mut().new_handle_type(
            self.heap,
            id
        )
    }
    
    pub fn add_handle_method<F>(&mut self, ht:ScriptHandleType, method:LiveId, args:&[(LiveId, ScriptValue)], f: F) 
    where F: Fn(&mut ScriptVm, ScriptObject)->ScriptValue + 'static{
        self.code.native.borrow_mut().add_type_method(
            self.heap,
            ht.to_redux(),
            method,
            args,
            f
        )
    }
    
    pub fn set_handle_setter<F>(&mut self, ht:ScriptHandleType, f: F) 
    where F: Fn(&mut ScriptVm, ScriptValue, LiveId, ScriptValue)->ScriptValue + 'static{
        self.code.native.borrow_mut().set_type_setter(
            ht.to_redux(),
            f
        )
    }
    
    pub fn set_handle_getter<F>(&mut self, ht:ScriptHandleType, f: F) 
    where F: Fn(&mut ScriptVm, ScriptValue, LiveId)->ScriptValue + 'static{
        self.code.native.borrow_mut().set_type_getter(
            ht.to_redux(),
            f
        )
    }
    
    pub fn new_module(&mut self, id:LiveId)->ScriptObject{
        self.heap.new_module(id)
    }
          
    pub fn module(&mut self, id:LiveId)->ScriptObject{
        self.heap.module(id)
    }
    
    pub fn map_mut_with<R,F:FnOnce(&mut Self, &mut ScriptObjectMap)->R>(&mut self, object:ScriptObject, f:F)->R{
        let mut map = ScriptObjectMap::default();
        std::mem::swap(&mut map, &mut self.heap.objects[object.index as usize].map);
        let r = f(self, &mut map);
        std::mem::swap(&mut map, &mut self.heap.objects[object.index as usize].map);
        r
    }
    
    pub fn add_method<F>(&mut self, module:ScriptObject, method:LiveId, args:&[(LiveId, ScriptValue)], f: F) 
    where F: Fn(&mut ScriptVm, ScriptObject)->ScriptValue + 'static{
        self.code.native.borrow_mut().add_method(&mut self.heap, module, method, args, f)
    }
    
    /// Registers a native function to be used as an apply_transform and returns its NativeId.
    /// This is used for creating objects that transform to a computed value when applied.
    pub fn add_apply_transform_fn<F>(&mut self, f: F) -> NativeId
    where F: Fn(&mut ScriptVm, ScriptObject)->ScriptValue + 'static{
        self.code.native.borrow_mut().add_apply_transform_fn(f)
    }
    
    
    pub fn add_script_mod(&mut self, new_mod:ScriptMod)->u16{
        // Register this crate's manifest path for crate path resolution
        let crate_name = new_mod.module_path.split("::").next().unwrap_or("");
        if !crate_name.is_empty() {
            self.code.crate_manifests.borrow_mut().insert(
                crate_name.replace('-', "_"),
                new_mod.cargo_manifest_path.clone()
            );
        }
        
        let scope = self.heap.new_with_proto(id!(scope).into());
        self.heap.set_object_deep(scope);
        self.heap.set_value_def(scope, id!(mod).into(), self.heap.modules.into());
        let me = self.heap.new_with_proto(id!(root_me).into());
                
        let new_body = ScriptBody{
            source: ScriptSource::Mod(new_mod),
            tokenizer: ScriptTokenizer::default(),
            parser: ScriptParser::default(),
            scope,
            me,
        };
        let mut bodies = self.code.bodies.borrow_mut();
        for (i, body) in bodies.iter_mut().enumerate(){
            if let ScriptSource::Mod(script_mod) = &body.source{
                if let ScriptSource::Mod(new_mod)= &new_body.source{
                    if  script_mod.file == new_mod.file &&
                    script_mod.line == new_mod.line &&
                    script_mod.column == new_mod.column{
                        *body = new_body;
                        return i as u16
                    }
                }
            }
        }
        let i = bodies.len();
        bodies.push(new_body);
        i as u16
    }
        
    pub fn eval(&mut self, script_mod: ScriptMod)->ScriptValue{
        let body_id = self.add_script_mod(script_mod);
        let mut bodies = self.code.bodies.borrow_mut();
        let body = &mut bodies[body_id as usize];
                
        if let ScriptSource::Mod(script_mod) = &body.source{
            body.tokenizer.tokenize(&script_mod.code, &mut self.heap);
            body.parser.parse(&body.tokenizer, &script_mod.file, (script_mod.line, script_mod.column), &script_mod.values);
            drop(bodies);
            // lets point our thread to it
            
            self.thread.run_root(&mut self.heap, &self.code, self.host, body_id)
        }
        else{
            NIL
        }
    }
    
}

pub struct ScriptVmBase{
    pub void: usize,
    pub code: ScriptCode,
    pub heap: ScriptHeap,
    pub threads: Vec<ScriptThread>,
}

impl ScriptVmBase{
    pub fn as_ref<'a>(&'a mut self)->ScriptVm<'a>{
        ScriptVm{
            host: &mut self.void,
            code: &mut self.code,
            heap: &mut self.heap,
            thread: &mut self.threads[0]
        }
    }
    
    pub fn as_ref_host_thread<'a>(&'a mut self, thread:ScriptThreadId, host:&'a mut dyn Any)->ScriptVm<'a>{
        ScriptVm{
            host,
            code: &self.code,
            heap: &mut self.heap,
            thread: &mut self.threads[thread.to_index()]
        }
    }
    
    pub fn as_ref_host<'a>(&'a mut self, host:&'a mut dyn Any)->ScriptVm<'a>{
        let id = self.get_unpaused_thread();
        // lets get an unpaused thread
        ScriptVm{
            host,
            code: &self.code,
            heap: &mut self.heap,
            thread: &mut self.threads[id.to_index()]
        }
    }
    
    pub fn get_unpaused_thread(&mut self)->ScriptThreadId{
        for (id,thread) in self.threads.iter().enumerate(){
            if !thread.is_paused{
                return ScriptThreadId(id as u32)
            }
        }
        let id = ScriptThreadId(self.threads.len() as u32);
        self.threads.push(ScriptThread::new(id));
        id
    }
    
    pub fn new()->Self{
        let mut heap = ScriptHeap::empty();
        let mut native = ScriptNative::new(&mut heap);
        define_math_module(&mut heap, &mut native);
        define_std_module(&mut heap, &mut native);
        define_shader_module(&mut heap, &mut native);
        let pod_builtins = define_pod_module(&mut heap, &mut native);
            
        let builtins = ScriptBuiltins::new(&mut heap, pod_builtins);
        
        Self{
            void: 0,
            code:ScriptCode{
                builtins,
                native: RefCell::new(native),
                bodies: Default::default(),
                crate_manifests: Default::default(),
            },
            threads: vec![ScriptThread::new(ScriptThreadId(0))],
            heap: heap,
        }
    }
        
}