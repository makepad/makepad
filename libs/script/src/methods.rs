use crate::makepad_live_id::*;
use crate::heap::*;
use crate::value::*;
use crate::native::*;
use crate::vm::*;
use crate::array::*;
use crate::object::*;
use crate::string::*;

#[derive(Default)]
pub struct ScriptTypeMethods{
    pub type_table: Vec<LiveIdMap<LiveId, ScriptObject>>,
}

impl ScriptTypeMethods{
    pub fn new(h:&mut ScriptHeap, native:&mut ScriptNative)->Self{
        let mut tm = Self::default();
        tm.add_shared(h, native);
        ScriptObjectData::add_type_methods(&mut tm, h, native);
        ScriptArrayData::add_type_methods(&mut tm, h, native);
        ScriptStringData::add_type_methods(&mut tm, h, native);
        tm
    }
    
    pub fn add<F>(&mut self, heap:&mut ScriptHeap, native:&mut ScriptNative, args:&[(LiveId,ScriptValue)], ty_redux:usize, method:LiveId, f: F) 
    where F: Fn(&mut ScriptVm, ScriptObject)->ScriptValue + 'static{
        let fn_obj = native.add(heap, args, f);
                
        if ty_redux >= self.type_table.len(){
            self.type_table.resize_with(ty_redux + 1, || Default::default());
        }
        self.type_table[ty_redux].insert(method,fn_obj);
    }
    
    pub fn add_shared(&mut self, h:&mut ScriptHeap, native:&mut ScriptNative){
        self.add(h, native, &[], ScriptValueType::REDUX_NUMBER, id!(ty), |_, _|{id!(number).escape()});
        self.add(h, native, &[], ScriptValueType::REDUX_NAN, id!(ty), |_, _|{id!(nan).escape()});
        self.add(h, native, &[], ScriptValueType::REDUX_BOOL, id!(ty), |_, _|{id!(bool).escape()});
        self.add(h, native, &[], ScriptValueType::REDUX_NIL, id!(ty), |_, _|{id!(nil).escape()});
        self.add(h, native, &[], ScriptValueType::REDUX_COLOR, id!(ty), |_, _|{id!(color).escape()});
        self.add(h, native, &[], ScriptValueType::REDUX_STRING, id!(ty), |_, _|{id!(string).escape()});
        self.add(h, native, &[], ScriptValueType::REDUX_OBJECT, id!(ty), |_, _|{id!(object).escape()});
        self.add(h, native, &[], ScriptValueType::REDUX_ARRAY, id!(ty), |_, _|{id!(rsid).escape()});
        self.add(h, native, &[], ScriptValueType::REDUX_OPCODE, id!(ty), |_, _|{id!(opcode).escape()});
        self.add(h, native, &[], ScriptValueType::REDUX_ERR, id!(ty), |_, _|{id!(err).escape()});
        self.add(h, native, &[], ScriptValueType::REDUX_ID, id!(ty), |_, _|{id!(id).escape()});
        for (ty,id) in [
            (ScriptValueType::REDUX_NUMBER, id!(is_number)),
            (ScriptValueType::REDUX_NAN, id!(is_nan)),
            (ScriptValueType::REDUX_BOOL, id!(is_bool)),
            (ScriptValueType::REDUX_NIL, id!(is_nil)),
            (ScriptValueType::REDUX_COLOR, id!(is_color)),
            (ScriptValueType::REDUX_STRING, id!(is_string)),
            (ScriptValueType::REDUX_OBJECT, id!(is_object)),
            (ScriptValueType::REDUX_ARRAY, id!(is_array)),
            (ScriptValueType::REDUX_OPCODE, id!(is_opcode)),
            (ScriptValueType::REDUX_ERR, id!(is_err)),
            (ScriptValueType::REDUX_ID, id!(is_id))
        ]{
            self.add(h, native, &[], ScriptValueType::REDUX_NUMBER, id, move |_, _|{ (ty == ScriptValueType::REDUX_NUMBER).into()});
            self.add(h, native, &[], ScriptValueType::REDUX_NAN, id, move |_, _|{ (ty == ScriptValueType::REDUX_NAN).into()});
            self.add(h, native, &[], ScriptValueType::REDUX_BOOL, id, move |_, _|{ (ty == ScriptValueType::REDUX_BOOL).into()});
            self.add(h, native, &[], ScriptValueType::REDUX_NIL, id, move |_, _|{ (ty == ScriptValueType::REDUX_NIL).into()});
            self.add(h, native, &[], ScriptValueType::REDUX_COLOR, id, move |_, _|{ (ty == ScriptValueType::REDUX_COLOR).into()});
            self.add(h, native, &[], ScriptValueType::REDUX_STRING, id, move |_, _|{ (ty == ScriptValueType::REDUX_STRING).into()});
            self.add(h, native, &[], ScriptValueType::REDUX_OBJECT, id, move |_, _|{ (ty == ScriptValueType::REDUX_OBJECT).into()});
            self.add(h, native, &[], ScriptValueType::REDUX_ARRAY, id, move |_, _|{ (ty == ScriptValueType::REDUX_ARRAY).into()});
            self.add(h, native, &[], ScriptValueType::REDUX_OPCODE, id, move |_, _|{ (ty == ScriptValueType::REDUX_OPCODE).into()});
            self.add(h, native, &[], ScriptValueType::REDUX_ERR, id, move |_, _|{ (ty == ScriptValueType::REDUX_ERR).into()});
            self.add(h, native, &[], ScriptValueType::REDUX_ID, id, move |_, _|{ (ty == ScriptValueType::REDUX_ID).into()});
        }
    }
}    
      
