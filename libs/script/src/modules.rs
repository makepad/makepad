use crate::makepad_value::id::*;
use crate::heap::*;
use crate::makepad_value::value::*;
use crate::makepad_value_derive::*;
use crate::native::*;
use crate::object::*;

pub struct ScriptModules{
    pub obj: ObjectPtr,
}

pub struct ScriptBuiltins{
    pub range: ObjectPtr,
}

impl ScriptBuiltins{
    pub fn new(heap:&mut ScriptHeap, modules:&ScriptModules)->Self{
        Self{
            range: heap.object_value_path(modules.obj, ids!(std.Range)).as_object().unwrap()
        }
    }
}

impl ScriptModules{
    pub fn new(heap:&mut ScriptHeap, native:&mut ScriptNative)->Self{
        let mut t = Self{
            obj: heap.new_object_with_proto(id!(mod).into())
        };
        t.add_math(heap, native);
        t.add_std(heap, native);
        t
    }
    
    pub fn add<F>(&mut self, heap:&mut ScriptHeap, native:&mut ScriptNative, module:ObjectPtr, args:&[(Id, Value)],method:Id, f: F) 
    where F: Fn(&mut ScriptHeap, ObjectPtr)->Value + 'static{
        // lets get the 
        let fn_index = native.fn_table.len();
        let fn_obj = heap.new_object_with_proto(id!(native).into());
        heap.set_object_type(fn_obj, ObjectType::VEC2);        
                        
        for (arg, def) in args{
            heap.set_object_value(fn_obj, (*arg).into(), *def);
        }
        
        heap.set_object_native_fn(fn_obj, fn_index as u32);
        native.fn_table.push(NativeFnEntry::new(f));
        
        heap.set_object_value(module, method.into(), fn_obj.into());
    }
    
    pub fn add_math(&mut self, heap:&mut ScriptHeap, native:&mut ScriptNative){
        let module = heap.new_object_with_proto(id!(math_module).into());
        heap.set_object_value(self.obj, id!(math).into(), module.into());
        
        self.add(heap, native, module, &[(id!(x), 0.0.into())], id!(sin), |heap, args|{
            heap.cast_to_f64(heap.object_value(args, id!(x).into())).sin().into()
        });
        
        self.add(heap, native, module, &[(id!(x), 0.0.into()),(id!(y), Value::NIL)], id!(vec2), |heap, args|{
            heap.cast_to_f64(heap.object_value(args, id!(x).into())).sin().into()
        });
    }
    
    pub fn add_std(&mut self, heap:&mut ScriptHeap, native:&mut ScriptNative){
        let std = heap.new_object_with_proto(id!(std_module).into());
        heap.set_object_value(self.obj, id!(std).into(), std.into());
        
        let range = heap.new_object_with_proto(id!(range).into());
        heap.set_object_value(std, id!(Range).into(), range.into());
        
        self.add(heap, native, range, &[(id!(x), 0.0.into())], id!(step), |heap, args|{
            
            if let Some(this) = heap.object_value(args, id!(this).into()).as_object(){
                if let Some(x) = heap.object_value(args, id!(x).into()).as_f64(){
                    heap.set_object_value(this, id!(step).into(), x.into());
                }
                return this.into()
            }
            Value::NIL
        });
    }
}    
      
