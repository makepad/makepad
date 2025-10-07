use crate::interpreter::*;
use crate::value::*;
use crate::id::*;

// cx needs to be available for heap allocations 

pub struct SystemFnEntry(pub Box<dyn Fn(&mut ScriptCx, ObjectPtr)->Value>);
impl SystemFnEntry{
    pub fn new<F>(f: F)->Self 
        where F: Fn(&mut ScriptCx, ObjectPtr)->Value + 'static{
        Self(Box::new(f))
    }
}

#[derive(Default)]
pub struct SystemFns(Vec<IdMap<Id, SystemFnEntry>>);

impl SystemFns{
    pub fn add<F>(&mut self, index:ValueType, id:Id, f: F) 
    where F: Fn(&mut ScriptCx, ObjectPtr)->Value + 'static{
        let index = index.to_index();
        if index >= self.0.len(){
            self.0.resize_with(index + 1, || Default::default());
        }
        self.0[index].insert(id, SystemFnEntry::new(f));
    }
}

pub struct ScriptCx<'a>{
    pub script: &'a mut Script,
    pub thread: ScriptThreadId
}

pub trait ScriptCall{
    fn update_fields(&mut self, obj: ObjectPtr);
    fn call_method(&mut self, ctx:&ScriptCx, method: Id, args: ObjectPtr)->Value;
}