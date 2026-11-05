//! A live edit's reload pass for what a VM registered outside its `script_mod`.
//!
//! The pass runs the app's `script_mod` again in reload mode (app_main.rs), and a Splash
//! isolate's UI registration again (the widgets' `widget_async`). A library that registers
//! itself lazily in a VM (the render crate's material shaders, on a VM's first material) and a
//! host that registers its own modules into an isolate after allocating it (a game's level
//! mods, a hosted module's widgets) run outside both: [`ScriptVmReload::keep_for_reload`]
//! records such a registration, and the VM's reload pass runs it again
//! ([`ScriptVmReload::run_kept_for_reload`]), so its code takes the edit as the app's does.
use crate::*;
use makepad_script::*;
use std::collections::HashMap;
use std::rc::Rc;

/// A registration a VM runs again on each of its reload passes.
pub type ReloadMod = Rc<dyn Fn(&mut ScriptVm)>;

/// What each heap keeps for its reload passes, by heap key, in the order kept.
#[derive(Default)]
struct CxReloadMods(HashMap<usize, Vec<(LiveId, ReloadMod)>>);

/// Runs `f` in the VM that owns heap `heap_key` (a Splash isolate's, routed by the widgets
/// crate), false when that VM is gone or busy. Without one, only the installed VM's heap is
/// reachable.
pub type ScriptHeapRoute = fn(cx: &mut Cx, heap_key: usize, f: &mut dyn FnMut(&mut ScriptVm)) -> bool;

#[derive(Default)]
pub(crate) struct CxScriptHeapRoute(pub(crate) Option<ScriptHeapRoute>);

/// Whether `f` holds for any VM other than the installed one (the widgets crate's isolates
/// that are not running), read-only: a live edit asks whether an isolate registered a patched
/// file's modules.
pub type ScriptVmsAny = fn(cx: &mut Cx, f: &mut dyn FnMut(&ScriptVmBase) -> bool) -> bool;

#[derive(Default)]
struct CxScriptVmsAny(Option<ScriptVmsAny>);

/// [`ScriptVmsAny`] through the hook the widgets crate set (`Cx::set_script_vms_any`); false
/// without one.
#[cfg(relax)]
pub(crate) fn any_other_vm(cx: &mut Cx, f: &mut dyn FnMut(&ScriptVmBase) -> bool) -> bool {
    match cx.global::<CxScriptVmsAny>().0 {
        Some(any) => any(cx, f),
        None => false,
    }
}

pub trait ScriptVmReload {
    /// Records `f`, a registration this VM ran outside its `script_mod`, to run again in
    /// reload mode on each of its reload passes. Once per `key`: keeping a key again keeps
    /// the first.
    fn keep_for_reload(&mut self, key: LiveId, f: impl Fn(&mut ScriptVm) + 'static);
    /// The registrations kept for this VM's reload passes, again, in reload mode and in the
    /// order kept: the last step of its reload pass.
    fn run_kept_for_reload(&mut self);
}

impl<'a> ScriptVmReload for ScriptVm<'a> {
    fn keep_for_reload(&mut self, key: LiveId, f: impl Fn(&mut ScriptVm) + 'static) {
        let heap = self.heap_key();
        let kept = self.cx_mut().global::<CxReloadMods>().0.entry(heap).or_default();
        if !kept.iter().any(|(k, _)| *k == key) {
            kept.push((key, Rc::new(f)));
        }
    }

    fn run_kept_for_reload(&mut self) {
        let heap = self.heap_key();
        let kept: Vec<ReloadMod> = self.cx_mut().global::<CxReloadMods>().0.get(&heap)
            .map_or_else(Vec::new, |kept| kept.iter().map(|(_, f)| f.clone()).collect());
        if kept.is_empty() {
            return;
        }
        self.with_reload(|vm| {
            for f in &kept {
                f(vm);
            }
        });
    }
}

impl Cx {
    /// Forget what heap `heap_key` kept for its reload passes: its VM is gone (a reclaimed
    /// isolate), and a later heap at the same address starts with nothing kept.
    pub fn forget_kept_for_reload(&mut self, heap_key: usize) {
        self.global::<CxReloadMods>().0.remove(&heap_key);
    }

    /// Where a VM other than the installed one is entered by its heap (the widgets crate's
    /// isolates): what a draw made in an isolate takes a live edit through
    /// (`DrawVars::refresh_after_reload`).
    pub fn set_script_heap_route(&mut self, route: ScriptHeapRoute) {
        self.global::<CxScriptHeapRoute>().0 = Some(route);
    }

    /// How a live edit asks the VMs other than the installed one (the widgets crate's
    /// isolates) about a patched file (`live_code_patched`).
    pub fn set_script_vms_any(&mut self, any: ScriptVmsAny) {
        self.global::<CxScriptVmsAny>().0 = Some(any);
    }

    /// Runs `f` in the VM that owns heap `heap_key`: the installed VM when it is that heap's,
    /// otherwise through the heap route. False when neither reaches it.
    pub fn with_vm_of_heap(&mut self, heap_key: usize, f: &mut dyn FnMut(&mut ScriptVm)) -> bool {
        if self.script_vm.as_ref().is_some_and(|bx| bx.heap_ref().heap_key() == heap_key) {
            self.with_vm(|vm| f(vm));
            return true;
        }
        match self.global::<CxScriptHeapRoute>().0 {
            Some(route) => route(self, heap_key, f),
            None => false,
        }
    }
}
