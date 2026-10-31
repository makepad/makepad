use crate::*;
use makepad_script::*;

pub trait ScriptVmCx {
    fn cx_mut(&mut self) -> &mut Cx;
    fn cx(&mut self) -> &Cx;
    fn with_cx<R, F: FnOnce(&Cx) -> R>(&mut self, f: F) -> R;
    fn with_cx_mut<R, F: FnOnce(&mut Cx) -> R>(&mut self, f: F) -> R;
}

unsafe impl ScriptHostCapability for Cx {
    const ID: LiveId = LiveId::from_str("makepad.platform.cx.v1");
}
unsafe impl ScriptHostOwner for Cx {
    unsafe fn query(owner: *mut Self, id: LiveId, standard: bool) -> *mut u8 {
        if !standard && id == <Cx as ScriptHostCapability>::ID {
            owner.cast()
        } else if standard && id == <makepad_script_std::ScriptStd as ScriptHostCapability>::ID {
            std::ptr::addr_of_mut!((*owner).script_data.std).cast()
        } else if !standard && id == SCRIPT_HOST_VM_BASE {
            (*owner)
                .script_vm
                .as_deref()
                .map_or(std::ptr::null_mut(), |base| {
                    base as *const ScriptVmBase as *mut u8
                })
        } else {
            std::ptr::null_mut()
        }
    }
    fn vm_slot(&mut self) -> &mut Option<Box<ScriptVmBase>> {
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
        self.host.get_mut().unwrap()
    }
    fn cx(&mut self) -> &Cx {
        self.host.get().unwrap()
    }
    fn with_cx<R, F: FnOnce(&Cx) -> R>(&mut self, f: F) -> R {
        let mut f = Some(f);
        let mut out = None;
        with_vm_parked(self, &mut |cx| out = Some((f.take().unwrap())(&*cx)));
        out.unwrap()
    }
    fn with_cx_mut<R, F: FnOnce(&mut Cx) -> R>(&mut self, f: F) -> R {
        let mut f = Some(f);
        let mut out = None;
        with_vm_parked(self, &mut |cx| out = Some((f.take().unwrap())(cx)));
        out.unwrap()
    }
}

thread_local! {
    // The placeholder `with_vm_parked` leaves in the caller's `ScriptVm` while the real VM is
    // back on `Cx`: one per thread, reused, because it is never read (it only fills the slot).
    // Making a fresh one per park built a whole empty heap (root table, heap tag, module root)
    // on every script->Cx callback: thousands per second in Stage.
    static PARKED_PLACEHOLDER: std::cell::Cell<Option<Box<ScriptVmBase>>> = const { std::cell::Cell::new(None) };
}

/// Park the executing base back onto `Cx` for the duration of `f`, so
/// native code reached through `f` can `cx.with_vm` again, and take it
/// back out afterwards — whether `f` returned or unwound. A panic caught
/// above (a platform's event catcher, a host isolating a guest) must find
/// this `ScriptVm` holding its base again, or the enclosing `with_vm`
/// parks the placeholder instead of the VM.
///
/// Not generic: every `with_cx`/`with_cx_mut` call site shares this one body
/// (the generic shells above only move the closure and its result through
/// `Option` slots).
#[inline(never)]
fn with_vm_parked(vm: &mut ScriptVm, f: &mut dyn FnMut(&mut Cx)) {
    let saved_thread_id = vm.current_thread();
    assert_vm_slot_free(vm.host.get().unwrap());
    // a nested park finds the spare taken and makes its own
    let placeholder = PARKED_PLACEHOLDER.with(|p| p.take()).unwrap_or_else(|| Box::new(ScriptVmBase::empty()));
    let bx = std::mem::replace(&mut vm.bx, placeholder);
    vm.host.restore_vm(bx);
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        f(vm.host.get_mut().unwrap())
    }));
    match (vm.host.take_vm(), out) {
        (Some(bx), Ok(())) => {
            let placeholder = std::mem::replace(&mut vm.bx, bx);
            PARKED_PLACEHOLDER.with(|p| p.set(Some(placeholder)));
            vm.set_current_thread(saved_thread_id);
        }
        (None, Ok(())) => {
            panic!("the closure took the script VM off Cx and never parked it back")
        }
        (bx, Err(payload)) => {
            // An inner `with_vm` parked it before resuming; a frame that
            // lost it leaves the placeholder, and the entry above says so.
            if let Some(bx) = bx {
                vm.bx = bx;
                vm.set_current_thread(saved_thread_id);
            }
            std::panic::resume_unwind(payload)
        }
    }
}

impl ScriptVmCx for ScriptHostContext<'_> {
    fn cx_mut(&mut self) -> &mut Cx {
        self.get_mut().unwrap()
    }
    fn cx(&mut self) -> &Cx {
        self.get().unwrap()
    }
    fn with_cx<R, F: FnOnce(&Cx) -> R>(&mut self, f: F) -> R {
        let cx: &Cx = self.get().unwrap();
        f(cx)
    }
    fn with_cx_mut<R, F: FnOnce(&mut Cx) -> R>(&mut self, f: F) -> R {
        let cx: &mut Cx = self.get_mut().unwrap();
        f(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    /// A panic that a platform's event catcher contains must leave the VM on
    /// `Cx`: otherwise every later script entry is refused as re-entrant.
    #[test]
    fn a_panic_in_with_vm_leaves_the_vm_usable() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let out = catch_unwind(AssertUnwindSafe(|| {
            cx.with_vm(|_vm| panic!("contained panic in a script entry"))
        }));
        assert!(out.is_err());
        assert!(cx.script_vm.is_some(), "the VM came back on unwind");
        assert_eq!(cx.with_vm(|_vm| 7), 7, "a later entry is not re-entrant");
    }

    /// The same through `with_cx_mut`, which parks the VM on `Cx` while its
    /// closure runs.
    #[test]
    fn a_panic_in_with_cx_mut_leaves_the_vm_usable() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let out = catch_unwind(AssertUnwindSafe(|| {
            cx.with_vm(|vm| vm.with_cx_mut(|_cx| panic!("contained panic in a cx borrow")))
        }));
        assert!(out.is_err());
        assert!(cx.script_vm.is_some());
        assert_eq!(cx.with_vm(|_vm| 7), 7);
    }
}
