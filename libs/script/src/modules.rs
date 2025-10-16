use crate::makepad_value::id::*;
use crate::heap::*;
use crate::makepad_value::value::*;
use crate::makepad_value_derive::*;
use crate::native::*;
use crate::*;

pub fn define_math_module(heap:&mut ScriptHeap, native:&mut ScriptNative){
    let math = heap.new_module(id!(math));
    
    native.add_method(heap, math, id!(sin), args!(x:0.0), |ctx, args|{
        value_f64!(ctx, args.x).sin().into()
    });
}

pub fn define_std_module(heap:&mut ScriptHeap, native:&mut ScriptNative){
    let std = heap.new_module(id!(std));
            
    native.add_method(heap, std, id!(assert), args!(v: NIL), |ctx, args|{
        if let Some(x) = value!(ctx, args.v).as_bool(){
            if x == true{
                return NIL
            }
        }
        return Value::err_assertfail(ctx.thread.ip)
    });
            
    let range = heap.new_with_proto(id!(range).into());
    heap.set_value(std, id!(Range).into(), range.into());
            
    native.add_method(heap, range, id!(step), args!(x: 0.0), |ctx, args|{
        if let Some(this) = value!(ctx, args.this).as_object(){
            if let Some(x) = value!(ctx, args.x).as_f64(){
                ctx.heap.set_value(this, id!(step).into(), x.into());
            }
            return this.into()
        }
        NIL
    });
}


pub struct ScriptBuiltins{
    pub range: ObjectPtr,
}

impl ScriptBuiltins{
    pub fn new(heap:&mut ScriptHeap)->Self{
        Self{
            range: heap.value_path(heap.modules, ids!(std.Range),NIL).as_object().unwrap()
        }
    }
}