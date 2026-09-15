use crate::*;
use makepad_script::*;
use std::any::Any;

pub trait ScriptVmCx {
    fn cx_mut(&mut self) -> &mut Cx;
    fn cx(&mut self) -> &Cx;
    fn with_cx<R, F: FnOnce(&Cx) -> R>(&mut self, f: F) -> R;
    fn with_cx_mut<R, F: FnOnce(&mut Cx) -> R>(&mut self, f: F) -> R;
}

impl ScriptHost for Cx {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn script_std(&mut self) -> &mut dyn Any {
        &mut self.script_data.std
    }

    fn script_vm_slot(&mut self) -> &mut Option<Box<ScriptVmBase>> {
        &mut self.script_vm
    }
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
        self.host.as_any_mut().downcast_mut().unwrap()
    }
    fn cx(&mut self) -> &Cx {
        self.host.as_any().downcast_ref().unwrap()
    }
    fn with_cx<R, F: FnOnce(&Cx) -> R>(&mut self, f: F) -> R {
        with_vm_parked(self, |host| f(host.as_any().downcast_ref().unwrap()))
    }
    fn with_cx_mut<R, F: FnOnce(&mut Cx) -> R>(&mut self, f: F) -> R {
        with_vm_parked(self, |host| f(host.as_any_mut().downcast_mut().unwrap()))
    }
}

/// Park the executing base back onto `Cx` for the duration of `f`, so
/// native code reached through `f` can `cx.with_vm` again, and take it
/// back out afterwards — whether `f` returned or unwound. A panic caught
/// above (a platform's event catcher, a host isolating a guest) must find
/// this `ScriptVm` holding its base again, or the enclosing `with_vm`
/// parks the placeholder instead of the VM.
fn with_vm_parked<R>(vm: &mut ScriptVm, f: impl FnOnce(&mut dyn ScriptHost) -> R) -> R {
    let saved_thread_id = vm.bx.threads.current();
    assert_vm_slot_free(vm.host.as_any().downcast_ref().unwrap());
    let bx = std::mem::replace(&mut vm.bx, Box::new(ScriptVmBase::empty()));
    *vm.host.script_vm_slot() = Some(bx);
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut *vm.host)));
    match (vm.host.script_vm_slot().take(), out) {
        (Some(bx), Ok(out)) => {
            vm.bx = bx;
            vm.bx.threads.set_current(saved_thread_id);
            out
        }
        (None, Ok(_)) => {
            panic!("the closure took the script VM off Cx and never parked it back")
        }
        (bx, Err(payload)) => {
            // An inner `with_vm` parked it before resuming; a frame that
            // lost it leaves the placeholder, and the entry above says so.
            if let Some(bx) = bx {
                vm.bx = bx;
                vm.bx.threads.set_current(saved_thread_id);
            }
            std::panic::resume_unwind(payload)
        }
    }
}

impl ScriptVmCx for &mut dyn ScriptHost {
    fn cx_mut(&mut self) -> &mut Cx {
        self.as_any_mut().downcast_mut().unwrap()
    }
    fn cx(&mut self) -> &Cx {
        self.as_any().downcast_ref().unwrap()
    }
    fn with_cx<R, F: FnOnce(&Cx) -> R>(&mut self, f: F) -> R {
        let cx: &Cx = self.as_any().downcast_ref().unwrap();
        f(cx)
    }
    fn with_cx_mut<R, F: FnOnce(&mut Cx) -> R>(&mut self, f: F) -> R {
        let cx: &mut Cx = self.as_any_mut().downcast_mut().unwrap();
        f(cx)
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
