use crate::*;
use makepad_script::*;
use std::any::Any;

pub trait ScriptVmCx {
    fn cx_mut(&mut self) -> &mut Cx;
    fn cx(&mut self) -> &Cx;
    fn with_cx<R, F: FnOnce(&Cx) -> R>(&mut self, f: F) -> R;
    fn with_cx_mut<R, F: FnOnce(&mut Cx) -> R>(&mut self, f: F) -> R;
}

/// `with_cx`/`with_cx_mut` park the executing `bx` into `cx.script_vm` so the closure
/// can re-enter the VM through `Cx`. That is only sound when the VM being executed is
/// the one that was taken *out of* `cx.script_vm` — i.e. the VM is "installed" on `Cx`.
///
/// If a caller runs a VM that lives somewhere else (e.g. a `Splash` isolate handed in
/// as a separate `&mut`) while another VM still occupies the slot, the assignment below
/// would silently drop that other VM's whole heap and leave the slot `None`. Every VM
/// must therefore be installed on `Cx` for the duration of its execution — see
/// `Cx::with_vm` and the widgets-side `with_script_vm_id`.
#[track_caller]
fn assert_vm_slot_free(cx: &Cx) {
    assert!(
        cx.script_vm.is_none(),
        "with_cx while another script VM is installed on Cx: the executing VM must be \
         the one taken from `cx.script_vm`. A VM run as a side-channel `&mut` (e.g. a \
         Splash isolate) has to be swapped onto `Cx` first."
    );
}

impl<'a> ScriptVmCx for ScriptVm<'a> {
    fn cx_mut(&mut self) -> &mut Cx {
        self.host.downcast_mut().unwrap()
    }
    fn cx(&mut self) -> &Cx {
        self.host.downcast_ref().unwrap()
    }
    fn with_cx<R, F: FnOnce(&Cx) -> R>(&mut self, f: F) -> R {
        // Store current thread ID to restore after
        let saved_thread_id = self.bx.threads.current();

        let cx: &mut Cx = self.host.downcast_mut().unwrap();
        assert_vm_slot_free(cx);
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
        assert_vm_slot_free(cx);
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

impl ScriptVmCx for &mut dyn Any {
    fn cx_mut(&mut self) -> &mut Cx {
        self.downcast_mut().unwrap()
    }
    fn cx(&mut self) -> &Cx {
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
