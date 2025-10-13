use crate::heap::*;
use crate::makepad_value::value::*;

pub type NativeFnType = Box<dyn Fn(&mut ScriptHeap, ObjectPtr)->Value + 'static>;

pub struct NativeFnEntry{
    pub fn_ptr: NativeFnType
}

pub struct NativeFnIndex{
    pub fn_obj: Value,
    pub fn_index: usize
}

impl NativeFnEntry{
    pub fn new<F>(f: F)->Self 
    where F: Fn(&mut ScriptHeap, ObjectPtr)->Value + 'static{
        Self{fn_ptr:Box::new(f)}
    }
}

#[derive(Default)]
pub struct ScriptNative{
    pub fn_table: Vec<NativeFnEntry>,
}

