use crate::interpreter::*;
use crate::heap::*;
use crate::value::*;
use crate::id::*;

// cx needs to be available for heap allocations 

pub enum SystemFnEntry{
    Inline{
        fn_ptr: Box<dyn Fn(&mut ScriptHeap, Value, ObjectPtr)->Value>
    }
}

pub struct SystemFnIndex{
    pub arg_obj: Value,
    pub fn_index: usize
}

impl SystemFnEntry{
    pub fn inline<F>(f: F)->Self 
        where F: Fn(&mut ScriptHeap, Value, ObjectPtr)->Value + 'static{
        Self::Inline{fn_ptr:Box::new(f)}
    }
}

#[derive(Default)]
pub struct SystemFns{
    pub type_table: Vec<IdMap<Id, SystemFnIndex>>,
    pub fn_table: Vec<SystemFnEntry>,
}

impl SystemFns{
    pub fn inline<F>(&mut self, heap:&mut ScriptHeap, args:&[Id], index:ValueType, id:Id, f: F) 
    where F: Fn(&mut ScriptHeap, Value, ObjectPtr)->Value + 'static{
        let index = index.to_index();
        if index >= self.type_table.len(){
            self.type_table.resize_with(index + 1, || Default::default());
        }
        let fn_index = self.fn_table.len();
        let arg_obj = heap.new_object(0);
         heap.set_object_is_system_fn(arg_obj,fn_index as u32);
        for arg in args{
            heap.set_object_value(arg_obj, (*arg).into(), Value::NIL);
        }
        self.type_table[index].insert(id, SystemFnIndex{
            fn_index,
            arg_obj: arg_obj.into()
        });
        self.fn_table.push(SystemFnEntry::inline(f));
    }
}

pub struct ScriptCx<'a>{
    pub script: &'a mut Script,
    pub thread: ScriptThreadId
}

pub trait ScriptCall{
    fn update_fields(&mut self, obj: ObjectPtr);
    fn call_method(&mut self, ctx:&ScriptCx, method: Id, args: ObjectPtr)->Value;
}