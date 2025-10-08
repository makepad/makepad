use makepad_script_derive::*;
use crate::id::*;
use crate::value::*;
use crate::heap::*;
use crate::interop::*;

pub fn build_sys_fns(sys_fns:&mut SystemFns, h:&mut ScriptHeap){
    sys_fns.inline(h, &[], ValueType::NAN, id!(ty), |_, _|{id!(nan).into()});
    sys_fns.inline(h, &[], ValueType::BOOL, id!(ty), |_, _|{id!(bool).into()});
    sys_fns.inline(h, &[], ValueType::NIL, id!(ty), |_, _|{id!(nil).into()});
    sys_fns.inline(h, &[], ValueType::COLOR, id!(ty), |_, _|{id!(color).into()});
    sys_fns.inline(h, &[], ValueType::STRING, id!(ty), |_, _|{id!(string).into()});
    sys_fns.inline(h, &[], ValueType::OBJECT, id!(ty), |_, _|{id!(object).into()});
    sys_fns.inline(h, &[], ValueType::FACTORY, id!(ty), |_, _|{id!(factory).into()});
    sys_fns.inline(h, &[], ValueType::OPCODE, id!(ty), |_, _|{id!(opcode).into()});
    sys_fns.inline(h, &[], ValueType::ID, id!(ty), |_, _|{id!(id).into()});
        
    sys_fns.inline(h, &[], ValueType::OBJECT, id!(push), |heap, args|{
        let this = heap.fn_this(args).as_object().unwrap();
        heap.push_object_vec_into_object_vec(this, args);
        Value::NIL
    });
    
    sys_fns.inline(h, &[], ValueType::OBJECT, id!(extend), |heap, args|{
        let this = heap.fn_this(args).as_object().unwrap();
        heap.push_object_vec_of_vec_into_object_vec(this, args, false);
        Value::NIL
    });
    
    sys_fns.inline(h, &[], ValueType::OBJECT, id!(merge), |heap, args|{
        let this = heap.fn_this(args).as_object().unwrap();
        heap.push_object_vec_of_vec_into_object_vec(this, args, true);
        Value::NIL
    });
}                
      