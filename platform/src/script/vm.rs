
use crate::*;
use crate::cx::*;
use makepad_script::*;

impl Cx{
    pub fn with_vm<R,F:FnOnce(&mut ScriptVm)->R>(&mut self, f:F)->R{
        let mut script_vm = None;
        std::mem::swap(&mut self.script_vm, &mut script_vm);
        let r = if let Some(script_vm) = &mut script_vm{
            f(&mut script_vm.as_ref_host(self))
        }
        else{
            panic!()
        };
        std::mem::swap(&mut self.script_vm, &mut script_vm);
        r
    }
    
    pub fn eval(&mut self, block: ScriptBlock)->ScriptValue{
        self.with_vm(|vm|{
            vm.eval(block)
        })
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