
use crate::*;
use crate::cx::*;
use makepad_script::*;
use std::any::Any;

impl Cx{
    pub fn with_vm_and_async<R,F:FnOnce(&mut ScriptVm)->R>(&mut self, f:F)->R{
        let mut script_vm = None;
        std::mem::swap(&mut self.script_vm, &mut script_vm);
        let r = if let Some(script_vm) = &mut script_vm{
            f(&mut script_vm.as_ref_host(self))
        }
        else{
            panic!()
        };
        std::mem::swap(&mut self.script_vm, &mut script_vm);
        self.handle_script_tasks();
        r
    }
    
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
    
    pub fn with_vm_thread<R,F:FnOnce(&mut ScriptVm)->R>(&mut self, thread_id:ScriptThreadId, f:F)->R{
        let mut script_vm = None;
        std::mem::swap(&mut self.script_vm, &mut script_vm);
        let r = if let Some(script_vm) = &mut script_vm{
            f(&mut script_vm.as_ref_host_thread(thread_id, self))
        }
        else{
            panic!()
        };
        std::mem::swap(&mut self.script_vm, &mut script_vm);
        r
    }
    
    /*
    pub fn with_vm_gc<R,F:FnOnce(&mut ScriptVm)->R>(&mut self, f:F)->R{
        let mut script_vm = None;
        std::mem::swap(&mut self.script_vm, &mut script_vm);
        let r = if let Some(script_vm) = &mut script_vm{
            f(&mut script_vm.as_ref_host(self))
        }
        else{
            panic!()
        };
        script_vm.
        std::mem::swap(&mut self.script_vm, &mut script_vm);
        r
    }*/
    
    pub fn eval(&mut self, block: ScriptBlock)->ScriptValue{
        self.with_vm_and_async(|vm|{
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

impl ScriptVmCx for &mut dyn Any{
    fn cx_mut(&mut self)->&mut Cx{
        self.downcast_mut().unwrap()
    }
    fn cx(&mut self)->&Cx{
        self.downcast_ref().unwrap()
    }
}