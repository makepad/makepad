
use crate::*;

pub trait ScriptVmCx{
    fn cx(&self)->Option<&mut Cx>;
}

impl<'a> ScriptVmCx for ScriptVmRef<'a>{
    fn cx(&self)->Option<&mut Cx>{
        None
    }
}