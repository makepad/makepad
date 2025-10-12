use crate::script::*;
use crate::thread::*;
use crate::id::*;
use crate::value::*;

pub struct ScriptCx<'a>{
    pub script: &'a mut Script,
    pub thread: ScriptThreadId
}

pub trait ScriptCall{
    fn update_fields(&mut self, obj: ObjectPtr);
    fn call_method(&mut self, ctx:&ScriptCx, method: Id, args: ObjectPtr)->Value;
}