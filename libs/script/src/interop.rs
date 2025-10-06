use crate::interpreter::*;
use crate::heap::*;
use crate::value::*;
use crate::id::*;

pub struct ScriptContext<'a>{
    pub thread: &'a mut ScriptThread,
    pub heap: & 'a mut ScriptHeap
}

pub trait ScriptCall{
    fn update_fields(&mut self, obj: ObjectPtr);
    fn call_method(&mut self, ctx:&ScriptContext, method: Id, args: ObjectPtr)->Value;
}