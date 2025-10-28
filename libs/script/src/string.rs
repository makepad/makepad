use crate::value::*;
use crate::heap::*;
use crate::native::*;
use crate::makepad_live_id::*;
use crate::methods::*;
use crate::*;

#[derive(Default)]
pub struct StringTag(u64);

impl StringTag{
    const MARK:u64 = 0x1;
    const ALLOCED:u64 = 0x2;
        
    pub fn is_alloced(&self)->bool{
        return self.0 & Self::ALLOCED != 0
    }
            
    pub fn set_alloced(&mut self){
        self.0 |= Self::ALLOCED
    }
            
    pub fn clear(&mut self){
        self.0 = 0;
    }
            
    pub fn is_marked(&self)->bool{
        self.0 & Self::MARK != 0
    }
            
    pub fn set_mark(&mut self){
        self.0 |= Self::MARK
    }
            
    pub fn clear_mark(&mut self){
        self.0 &= !Self::MARK
    }
}

#[derive(Default)]
pub struct ScriptStringData{
    pub tag: StringTag,
    pub string: String
}

impl ScriptStringData{
    pub fn add_type_methods(tm: &mut ScriptTypeMethods, h: &mut ScriptHeap, native:&mut ScriptNative){
        tm.add(h, native, &[], ScriptValueType::REDUX_STRING, id!(bytes), |vm, args|{
            let this = script_value!(vm, args.this);
            vm.heap.string_to_bytes_array(this).into()
        });
        tm.add(h, native, &[], ScriptValueType::REDUX_STRING, id!(chars), |vm, args|{
            let this = script_value!(vm, args.this);
            vm.heap.string_to_chars_array(this).into()
        });
        tm.add(h, native, &[], ScriptValueType::REDUX_STRING, id!(json), |vm, args|{
            let this = script_value!(vm, args.this);
            vm.heap.string_to_bytes_array(this).into()
        });
    }
    
    pub fn clear(&mut self){
        self.tag.clear();
        self.string.clear()
    }
}