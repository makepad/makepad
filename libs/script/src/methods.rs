use makepad_script_derive::*;
use crate::id::*;
use crate::heap::*;
use crate::value::*;
use crate::native::*;
use crate::object::*;

#[derive(Default)]
pub struct ScriptMethods{
    pub type_table: Vec<IdMap<Id, NativeFnIndex>>,
}

impl ScriptMethods{
    pub fn new(h:&mut ScriptHeap, native:&mut ScriptNative)->Self{
        let mut t = Self::default();
        t.add_shared(h, native);
        t.add_object(h, native);
        t
    }
    
    pub fn add<F>(&mut self, heap:&mut ScriptHeap, native:&mut ScriptNative, args:&[Id], value_type:ValueType, method:Id, f: F) 
    where F: Fn(&mut ScriptHeap, ObjectPtr)->Value + 'static{
        let ty_index = value_type.to_index();
        if ty_index >= self.type_table.len(){
            self.type_table.resize_with(ty_index + 1, || Default::default());
        }
        let fn_index = native.fn_table.len();
        
        let fn_obj = heap.new_object_with_proto(id!(fn).into());
        heap.set_object_type(fn_obj, ObjectType::VEC2);        
        
        heap.set_object_native_fn(fn_obj, fn_index as u32);
        
        for arg in args{
            heap.set_object_value(fn_obj, (*arg).into(), Value::NIL);
        }
        
        self.type_table[ty_index].insert(method, NativeFnIndex{
            fn_index,
            fn_obj: fn_obj.into()
        });
        native.fn_table.push(NativeFnEntry::new(f));
    }
    
    pub fn add_shared(&mut self, h:&mut ScriptHeap, native:&mut ScriptNative){
        self.add(h, native, &[], ValueType::NAN, id!(ty), |_, _|{id!(nan).into()});
        self.add(h, native, &[], ValueType::BOOL, id!(ty), |_, _|{id!(bool).into()});
        self.add(h, native, &[], ValueType::NIL, id!(ty), |_, _|{id!(nil).into()});
        self.add(h, native, &[], ValueType::COLOR, id!(ty), |_, _|{id!(color).into()});
        self.add(h, native, &[], ValueType::STRING, id!(ty), |_, _|{id!(string).into()});
        self.add(h, native, &[], ValueType::OBJECT, id!(ty), |_, _|{id!(object).into()});
        self.add(h, native, &[], ValueType::FACTORY, id!(ty), |_, _|{id!(factory).into()});
        self.add(h, native, &[], ValueType::OPCODE, id!(ty), |_, _|{id!(opcode).into()});
        self.add(h, native, &[], ValueType::ID, id!(ty), |_, _|{id!(id).into()});
    }
    
    pub fn add_object(&mut self, h: &mut ScriptHeap, native:&mut ScriptNative){
        self.add(h, native, &[], ValueType::OBJECT, id!(push), |heap, args|{
            if let Some(this) = heap.object_value(args, id!(this).into()).as_object(){
                heap.push_object_vec_into_object_vec(this, args);
            }
            Value::NIL
        });
            
        self.add(h, native, &[], ValueType::OBJECT, id!(extend), |heap, args|{
            if let Some(this) = heap.object_value(args, id!(this).into()).as_object(){
                heap.push_object_vec_of_vec_into_object_vec(this, args, false);
            }
            Value::NIL
        });
            
        self.add(h, native, &[], ValueType::OBJECT, id!(import), |heap, args|{
            if let Some(this) = heap.object_value(args, id!(this).into()).as_object(){
                heap.push_object_vec_of_vec_into_object_vec(this, args, true);
            }
            Value::NIL
        });
    }     
}    
      
