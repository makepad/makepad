use crate::vm::*;
use crate::value::*;
use crate::makepad_id::id::*;
use crate::heap::*;
use crate::makepad_id_derive::*;
use crate::object::*;

#[macro_export]
macro_rules! value_f64{
    ($ctx:ident, $args:ident.$id: ident)=>{
        $ctx.heap.cast_to_f64($ctx.heap.value($args, id!($id).into(),&mut $ctx.thread.trap), $ctx.thread.trap.ip)
    };
    ($ctx:ident, $obj:ident[$index: expr])=>{
        $ctx.heap.cast_to_f64($ctx.heap.vec_value($obj, ($index) as usize), $ctx.thread.ip())
    }
}

#[macro_export]
macro_rules! value_bool{
    ($ctx:ident, $args:ident.$id: ident)=>{
        $ctx.heap.cast_to_bool($ctx.heap.value($args, id!($id).into(),NIL), $ctx.thread.ip())
    };
    ($ctx:ident, $obj:ident[$index: expr])=>{
        $ctx.heap.cast_to_bool($ctx.heap.vec_value($obj, ($index) as usize), $ctx.thread.ip())
    }
}
        
#[macro_export]
macro_rules! value{
    ($ctx:ident, $obj:ident.$id: ident)=>{
        $ctx.heap.value($obj, id!($id).into(),&mut $ctx.thread.trap)
    };
    ($ctx:ident, $obj:ident[$index: expr])=>{
        $ctx.heap.vec_value($obj, ($index) as usize,&mut $ctx.thread.trap)
    }
}

#[macro_export]
macro_rules! args{
    ($($id:ident=$val:expr),*)=>{
        &[$((id!($id), ($val).into()),)*]
    }
}

pub type NativeFnType = Box<dyn Fn(&mut ScriptVmRef, ObjectPtr)->Value + 'static>;

pub struct NativeFnEntry{
    pub fn_ptr: NativeFnType
}

impl NativeFnEntry{
    pub fn new<F>(f: F)->Self 
    where F: Fn(&mut ScriptVmRef, ObjectPtr)->Value + 'static{
        Self{fn_ptr:Box::new(f)}
    }
}

#[derive(Default)]
pub struct ScriptNative{
    pub fn_table: Vec<NativeFnEntry>,
}

impl ScriptNative{
    pub fn add<F>(&mut self, heap:&mut ScriptHeap, args:&[(Id,Value)], f: F)-> ObjectPtr
    where F: Fn(&mut ScriptVmRef, ObjectPtr)->Value + 'static{
        let fn_index = self.fn_table.len();
        let fn_obj = heap.new_with_proto(id!(native).into());
        heap.set_object_type(fn_obj, ObjectType::VEC2);
        heap.set_fn(fn_obj, ScriptFnPtr::Native(NativeId{index: fn_index as u32}));
                
        for (arg, def) in args{
            heap.set_value(fn_obj, (*arg).into(), *def);
        }
        
        self.fn_table.push(NativeFnEntry::new(f));
        
        fn_obj
    }
    
    pub fn add_fn<F>(&mut self, heap:&mut ScriptHeap, module:ObjectPtr, method:Id, args:&[(Id, Value)], f: F) 
    where F: Fn(&mut ScriptVmRef, ObjectPtr)->Value + 'static{
        // lets get the 
        let fn_obj = self.add(heap, args, f);
        heap.set_value(module, method.into(), fn_obj.into());
    }
}

