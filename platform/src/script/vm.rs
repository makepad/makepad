
use crate::*;

pub trait ScriptVmCx{
    fn cx(&mut self)->&mut Cx;
}

impl<'a> ScriptVmCx for ScriptVmRef<'a>{
    fn cx(&mut self)->&mut Cx{
        self.host.downcast_mut().unwrap()
    }
}

// plug some scripting apis on Cx
impl Cx{
    
}