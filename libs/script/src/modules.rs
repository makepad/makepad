use crate::makepad_value::id::*;
use crate::heap::*;
use crate::makepad_value::value::*;
use crate::makepad_value_derive::*;
use crate::native::*;
use crate::object::*;
use crate::script::*;

pub struct ScriptModules{
    pub obj: ObjectPtr,
}

pub struct ScriptBuiltins{
    pub range: ObjectPtr,
}

impl ScriptBuiltins{
    pub fn new(heap:&mut ScriptHeap, modules:&ScriptModules)->Self{
        Self{
            range: heap.object_value_path(modules.obj, ids!(std.Range),Value::NIL).as_object().unwrap()
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
    
    pub fn add<F>(&mut self, heap:&mut ScriptHeap, native:&mut ScriptNative, module:ObjectPtr, method:Id, args:&[(Id, Value)], f: F) 
    where F: Fn(&mut ScriptCtx, ObjectPtr)->Value + 'static{
        // lets get the 
        let fn_index = native.fn_table.len();
        let fn_obj = heap.new_object_with_proto(id!(native).into());
        heap.set_object_type(fn_obj, ObjectType::VEC2);        
                        
        for (arg, def) in args{
            heap.set_object_value(fn_obj, (*arg).into(), *def);
        }
        
        heap.set_object_fn(fn_obj, ScriptFnPtr::Native(NativeId{index: fn_index as u32}));
        native.fn_table.push(NativeFnEntry::new(f));
        
        heap.set_object_value(module, method.into(), fn_obj.into());
    }
    
    pub fn add_math(&mut self, heap:&mut ScriptHeap, native:&mut ScriptNative){
        let module = heap.new_object_with_proto(id!(math_module).into());
        heap.set_object_value(self.obj, id!(math).into(), module.into());
        
        self.add(heap, native, module, id!(sin), &[(id!(x), 0.0.into())], |ctx, args|{
            ctx.heap.cast_to_f64(ctx.heap.object_value(args, id!(x).into(),Value::NIL)).sin().into()
        });
        
        self.add(heap, native, module, id!(vec2), &[(id!(x), 0.0.into()),(id!(y), Value::NIL)], |ctx, args|{
            ctx.heap.cast_to_f64(ctx.heap.object_value(args, id!(x).into(),Value::NIL)).sin().into()
        });
    }
    
    pub fn add_std(&mut self, heap:&mut ScriptHeap, native:&mut ScriptNative){
        let std = heap.new_object_with_proto(id!(std_module).into());
        heap.set_object_value(self.obj, id!(std).into(), std.into());
        
        
        self.add(heap, native, std, id!(assert), &[(id!(v), Value::NIL)], |ctx, args|{
            if let Some(x) = ctx.heap.object_value(args, id!(v).into(),Value::NIL).as_bool(){
                if x == true{
                    return Value::NIL
                }
            }
            return Value::from_err_assert(ctx.thread.ip)
        });
                
        
        let range = heap.new_object_with_proto(id!(range).into());
        heap.set_object_value(std, id!(Range).into(), range.into());
        
        self.add(heap, native, range, id!(step), &[(id!(x), 0.0.into())], |ctx, args|{
            if let Some(this) = ctx.heap.object_value(args, id!(this).into(),Value::NIL).as_object(){
                if let Some(x) = ctx.heap.object_value(args, id!(x).into(),Value::NIL).as_f64(){
                    ctx.heap.set_object_value(this, id!(step).into(), x.into());
                }
                return this.into()
            }
            Value::NIL
        });
    }
}    
      
