use crate::value::*;
use crate::*;
use crate::trap::*;

#[derive(Debug,Clone,Copy)]
pub struct NativeId{
    pub index: u32
}

#[derive(Clone)]
pub struct ScriptFnRef(pub(crate) ScriptObjectRef);

impl From<ScriptFnRef> for ScriptValue{
    fn from(v:ScriptFnRef) -> Self{
        ScriptValue::from_object(v.as_object())
    }
}

impl ScriptFnRef{
    pub fn as_object(&self)->ScriptObject{self.0.as_object()}
}

#[derive(Debug,Clone,Copy)]
pub enum ScriptFnPtr{
    Script(ScriptIp),
    Native(NativeId)
}

impl ScriptRefOptionExt for Option<ScriptFnRef>{
    fn as_object(&self)->Option<ScriptObject>{if let Some(x)=self{Some(x.as_object())}else{None}}
}

impl ScriptHeap{
        
        
            
    // Functions
            
            
            
    pub fn set_fn(&mut self, ptr: ScriptObject, fnptr: ScriptFnPtr){
        let object = &mut self.objects[ptr.index as usize];
        object.tag.set_fn(fnptr);
    }
                
    pub fn as_fn(&self, ptr: ScriptObject,)->Option<ScriptFnPtr>{
        let object = &self.objects[ptr.index as usize];
        object.tag.as_fn()
    }
                
    pub fn is_fn(&self, ptr: ScriptObject,)->bool{
        let object = &self.objects[ptr.index as usize];
        object.tag.is_fn()
    }
                
    pub fn set_reffed(&mut self, ptr: ScriptObject,){
        let object = &mut self.objects[ptr.index as usize];
        object.tag.set_reffed();
    }
                
    pub fn parent_as_fn(&self, ptr: ScriptObject,)->Option<ScriptFnPtr>{
        let object = &self.objects[ptr.index as usize];
        if let Some(ptr) = object.proto.as_object(){
            let fn_object = &self.objects[ptr.index as usize];
            fn_object.tag.as_fn()
        }
        else{
            None
        }
    }   
    
    pub fn unnamed_fn_arg(&mut self, top_ptr:ScriptObject, value:ScriptValue, trap:ScriptTrap)->ScriptValue{
        let object = &self.objects[top_ptr.index as usize];
                
        // which arg number?
        let index = object.map_len();
                
        if let Some(ptr) = object.proto.as_object(){
            let proto_object = &self.objects[ptr.index as usize];
            if let Some(kv) = proto_object.vec.get(index){
                let key = kv.key;
                if let Some(def) = object.vec.get(index){
                    if !def.value.is_nil() && def.value.value_type().to_redux() != value.value_type().to_redux(){
                        return script_err_invalid_arg_type!(trap, "arg {} type mismatch: expected {:?}, got {:?}", index, def.value.value_type().to_redux(), value.value_type().to_redux())
                    }
                }
                self.objects[top_ptr.index as usize].map_insert(key, value);
                if let Some(obj) = value.as_object(){
                    let object = &mut self.objects[obj.index as usize];
                    object.tag.set_reffed();
                }
                return NIL
            }
        }
        // only allow if we are varargs
        self.objects[top_ptr.index as usize].vec.push(ScriptVecValue{key:NIL, value});
        return NIL
    }
        
    pub fn named_fn_arg(&mut self, top_ptr:ScriptObject, key:ScriptValue, value:ScriptValue, trap:ScriptTrap)->ScriptValue{
        let object = &self.objects[top_ptr.index as usize];
                    
        if let Some(ptr) = object.proto.as_object(){
            let object = &self.objects[ptr.index as usize];
            for kv in object.vec.iter(){
                if kv.key == key{
                    if !kv.value.is_nil() && kv.value.value_type().to_redux() != value.value_type().to_redux(){
                        return script_err_invalid_arg_type!(trap, "named arg {:?} type mismatch: expected {:?}, got {:?}", key, kv.value.value_type().to_redux(), value.value_type().to_redux())
                    }
                    self.objects[top_ptr.index as usize].map_insert(key, value);
                    return NIL    
                }
            }
            return script_err_invalid_arg_name!(trap, "unknown named arg {:?}", key) 
        }
        script_err_unexpected!(trap, "named_fn_arg called without prototype object")
    }
        
    pub fn push_all_fn_args(&mut self, top_ptr:ScriptObject, args:&[ScriptValue], trap:ScriptTrap)->ScriptValue{
        let object = &self.objects[top_ptr.index as usize];
        if let Some(ptr) = object.proto.as_object(){
            for (index, value) in args.iter().enumerate(){
                let object = &self.objects[ptr.index as usize];
                if let Some(v1) = object.vec.get(index){
                    let key = v1.key;
                    // typecheck against default arg
                    if let Some(def) = object.vec.get(index){
                        if !def.value.is_nil() && def.value.value_type().to_redux() != value.value_type().to_redux(){
                            return script_err_invalid_arg_type!(trap, "arg {} ({:?}) type mismatch: expected {:?}, got {:?}", index, key, def.value.value_type().to_redux(), value.value_type().to_redux())
                        }
                    }
                    self.objects[top_ptr.index as usize].map_insert(key, *value);
                    if let Some(obj) = value.as_object(){
                        let object = &mut self.objects[obj.index as usize];
                        object.tag.set_reffed();
                    }
                }
                else{
                    self.objects[top_ptr.index as usize].vec.push(ScriptVecValue{key:NIL, value:*value});
                }
            }
            return NIL
        }
        script_err_unexpected!(trap, "push_all_fn_args called without prototype object")
    }
}