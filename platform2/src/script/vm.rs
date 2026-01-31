
use crate::*;
use crate::cx::*;
use makepad_script::*;
use std::any::Any;

impl Cx{
    pub fn with_vm_and_async<R,F:FnOnce(&mut ScriptVm)->R>(&mut self, f:F)->R{
        let mut bx = self.script_vm.take().expect("Script VM swapped off, make sure to call with_cx if you want this to work'");
        bx.threads.set_current_to_first_unpaused_thread();
        let mut vm = ScriptVm {
            host: self,
            bx,
        };
        let r = f(&mut vm);
        self.script_vm = Some(vm.bx);
        self.handle_script_tasks();
        r
    }
    
    pub fn with_vm<R,F:FnOnce(&mut ScriptVm)->R>(&mut self, f:F)->R{
        let mut bx = self.script_vm.take().expect("Script VM swapped off, make sure to call with_cx if you want this to work'");
        bx.threads.set_current_to_first_unpaused_thread();
        let mut vm = ScriptVm {
            host: self,
            bx,
        };
        let r = f(&mut vm);
        self.script_vm = Some(vm.bx);
        r
    }
    
    pub fn with_vm_thread<R,F:FnOnce(&mut ScriptVm)->R>(&mut self, thread_id:ScriptThreadId, f:F)->R{
        let mut bx = self.script_vm.take().expect("Script VM swapped off, make sure to call with_cx if you want this to work'");
        bx.threads.set_current_thread_id(thread_id);
        let mut vm = ScriptVm {
            host: self,
            bx,
        };
        let r = f(&mut vm);
        self.script_vm = Some(vm.bx);
        r
    }
    
    pub fn eval(&mut self, script_mod: ScriptMod)->ScriptValue{
        self.with_vm_and_async(|vm|{
            vm.eval(script_mod)
        })
    }
}

pub trait ScriptVmCx{
    fn cx_mut(&mut self)->&mut Cx;
    fn cx(&mut self)->&Cx;
    fn with_cx<R, F: FnOnce(&Cx) -> R>(&mut self, f: F) -> R;
    fn with_cx_mut<R, F: FnOnce(&mut Cx) -> R>(&mut self, f: F) -> R;
}

impl<'a> ScriptVmCx for ScriptVm<'a>{
    fn cx_mut(&mut self)->&mut Cx{
        self.host.downcast_mut().unwrap()
    }
    fn cx(&mut self)->&Cx{
        self.host.downcast_ref().unwrap()
    }
    fn with_cx<R, F: FnOnce(&Cx) -> R>(&mut self, f: F) -> R {
        // Store current thread ID to restore after
        let saved_thread_id = self.bx.threads.current();
        
        let cx: &mut Cx = self.host.downcast_mut().unwrap();
        // Swap bx back onto Cx
        let bx = std::mem::replace(&mut self.bx, Box::new(ScriptVmBase::empty()));
        cx.script_vm = Some(bx);
        let r = f(cx);
        // Swap bx back out
        self.bx = cx.script_vm.take().unwrap();
        
        // Restore current thread
        self.bx.threads.set_current(saved_thread_id);
        r
    }
    fn with_cx_mut<R, F: FnOnce(&mut Cx) -> R>(&mut self, f: F) -> R {
        // Store current thread ID to restore after
        let saved_thread_id = self.bx.threads.current();
        
        let cx: &mut Cx = self.host.downcast_mut().unwrap();
        // Swap bx back onto Cx
        let bx = std::mem::replace(&mut self.bx, Box::new(ScriptVmBase::empty()));
        cx.script_vm = Some(bx);
        let r = f(cx);
        // Swap bx back out
        self.bx = cx.script_vm.take().unwrap();
        
        // Restore current thread
        self.bx.threads.set_current(saved_thread_id);
        r
    }
}

impl ScriptVmCx for &mut dyn Any{
    fn cx_mut(&mut self)->&mut Cx{
        self.downcast_mut().unwrap()
    }
    fn cx(&mut self)->&Cx{
        self.downcast_ref().unwrap()
    }
    fn with_cx<R, F: FnOnce(&Cx) -> R>(&mut self, f: F) -> R {
        let cx: &Cx = self.downcast_ref().unwrap();
        f(cx)
    }
    fn with_cx_mut<R, F: FnOnce(&mut Cx) -> R>(&mut self, f: F) -> R {
        let cx: &mut Cx = self.downcast_mut().unwrap();
        f(cx)
    }
}