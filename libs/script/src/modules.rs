use makepad_script_derive::*;
use crate::id::*;
use crate::heap::*;
use crate::value::*;
use crate::native::*;
use crate::object::*;

pub struct ScriptModules{
    pub obj: ObjectPtr
}

impl ScriptModules{
    pub fn new(heap:&mut ScriptHeap, native:&mut ScriptNative)->Self{
        let mut t = Self{
            obj: heap.new_object_with_proto(id!(mod).into())
        };
        t.add_math(heap, native);
        t
    }
    
    pub fn add<F>(&mut self, heap:&mut ScriptHeap, native:&mut ScriptNative, args:&[(Id, Value)], module:Id, method:Id, f: F) 
    where F: Fn(&mut ScriptHeap, ObjectPtr)->Value + 'static{
        // lets get the 
        let fn_index = native.fn_table.len();
        let fn_obj = heap.new_object_with_proto(id!(fn).into());
        heap.set_object_type(fn_obj, ObjectType::VEC2);        
                        
        for (arg, def) in args{
            heap.set_object_value(fn_obj, (*arg).into(), *def);
        }
        
        heap.set_object_native_fn(fn_obj, fn_index as u32);
        native.fn_table.push(NativeFnEntry::new(f));
                
        if !heap.object_value(self.obj, module.into()).is_object(){
            let obj = heap.new_object_with_proto(id!(module).into());
            heap.set_object_value(self.obj, module.into(), obj.into());
        }
        let obj = heap.object_value(self.obj, module.into()).as_object().unwrap();
        
        heap.set_object_value(obj, method.into(), fn_obj.into());
    }
    
    pub fn add_math(&mut self, heap:&mut ScriptHeap, native:&mut ScriptNative){
        self.add(heap, native, &[(id!(x), 0.0.into())], id!(math), id!(sin), |heap, args|{
            heap.cast_to_f64(heap.object_value(args, id!(x).into())).sin().into()
        });
    }
}    
      
