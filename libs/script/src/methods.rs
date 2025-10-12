use makepad_script_derive::*;
use crate::id::*;
use crate::heap::*;
use crate::value::*;

// cx needs to be available for heap allocations 

pub struct MethodFnEntry{
    pub fn_ptr: Box<dyn Fn(&mut ScriptHeap, ObjectPtr)->Value>
}

pub struct MethodFnIndex{
    pub arg_obj: Value,
    pub fn_index: usize
}

impl MethodFnEntry{
    pub fn new<F>(f: F)->Self 
    where F: Fn(&mut ScriptHeap, ObjectPtr)->Value + 'static{
        Self{fn_ptr:Box::new(f)}
    }
}

#[derive(Default)]
pub struct ScriptMethods{
    pub type_table: Vec<IdMap<Id, MethodFnIndex>>,
    pub fn_table: Vec<MethodFnEntry>,
}

impl ScriptMethods{
    pub fn new(h:&mut ScriptHeap)->Self{
        let mut t = Self::default();
        t.add_shared(h);
        t.add_object(h);
        t
    }
    
    pub fn add<F>(&mut self, heap:&mut ScriptHeap, args:&[Id], value_type:ValueType, method:Id, f: F) 
    where F: Fn(&mut ScriptHeap, ObjectPtr)->Value + 'static{
        let ty_index = value_type.to_index();
        if ty_index >= self.type_table.len(){
            self.type_table.resize_with(ty_index + 1, || Default::default());
        }
        let fn_index = self.fn_table.len();
        let arg_obj = heap.new_object(0);
        heap.set_object_is_system_fn(arg_obj,fn_index as u32);
        for arg in args{
            heap.set_object_value(arg_obj, (*arg).into(), Value::NIL);
        }
        self.type_table[ty_index].insert(method, MethodFnIndex{
            fn_index,
            arg_obj: arg_obj.into()
        });
        self.fn_table.push(MethodFnEntry::new(f));
    }
    
    pub fn add_shared(&mut self, h:&mut ScriptHeap){
        self.add(h, &[], ValueType::NAN, id!(ty), |_, _|{id!(nan).into()});
        self.add(h, &[], ValueType::BOOL, id!(ty), |_, _|{id!(bool).into()});
        self.add(h, &[], ValueType::NIL, id!(ty), |_, _|{id!(nil).into()});
        self.add(h, &[], ValueType::COLOR, id!(ty), |_, _|{id!(color).into()});
        self.add(h, &[], ValueType::STRING, id!(ty), |_, _|{id!(string).into()});
        self.add(h, &[], ValueType::OBJECT, id!(ty), |_, _|{id!(object).into()});
        self.add(h, &[], ValueType::FACTORY, id!(ty), |_, _|{id!(factory).into()});
        self.add(h, &[], ValueType::OPCODE, id!(ty), |_, _|{id!(opcode).into()});
        self.add(h, &[], ValueType::ID, id!(ty), |_, _|{id!(id).into()});
    }
    
    pub fn add_object(&mut self, h: &mut ScriptHeap){
        self.add(h, &[], ValueType::OBJECT, id!(push), |heap, args|{
            let this = heap.fn_this(args).as_object().unwrap();
            heap.push_object_vec_into_object_vec(this, args);
            Value::NIL
        });
            
        self.add(h, &[], ValueType::OBJECT, id!(extend), |heap, args|{
            let this = heap.fn_this(args).as_object().unwrap();
            heap.push_object_vec_of_vec_into_object_vec(this, args, false);
            Value::NIL
        });
            
        self.add(h, &[], ValueType::OBJECT, id!(import), |heap, args|{
            let this = heap.fn_this(args).as_object().unwrap();
            heap.push_object_vec_of_vec_into_object_vec(this, args, true);
            Value::NIL
        });
    }     
}    
      
