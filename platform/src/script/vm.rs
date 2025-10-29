
use crate::*;
use crate::cx::*;

impl Cx{
    pub fn with_vm<F:FnOnce(&mut ScriptVm)>(&mut self, f:F){
        let mut script_vm = None;
        std::mem::swap(&mut self.script_vm, &mut script_vm);
        if let Some(script_vm) = &mut script_vm{
            f(&mut script_vm.as_ref_host(self));
        }
        else{
            panic!()
        }
        std::mem::swap(&mut self.script_vm, &mut script_vm);
    }
}

pub trait ScriptVmCx{
    fn cx_mut(&mut self)->&mut Cx;
    fn cx(&mut self)->&Cx;
}

impl<'a> ScriptVmCx for ScriptVm<'a>{
    fn cx_mut(&mut self)->&mut Cx{
        self.host.downcast_mut().unwrap()
    }
    fn cx(&mut self)->&Cx{
        self.host.downcast_ref().unwrap()
    }
}