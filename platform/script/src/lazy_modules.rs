//! Lazily expanded system modules. A VM kind declares big system modules (`mod.widgets`,
//! `mod.theme`, ...) as LAZY: their `mod` keys are absent until a script first reads one, then
//! the group's expander (the module's ordinary registration) runs once in that heap and the read
//! is retried. From then on the module is an ordinary object and nothing checks it again.
//!
//! Cost: nothing on a hit. A read that misses on the `mod` root consults the (tiny) key table and
//! marks the group pending instead of failing; the VM-level reads (`a.b`, `a?.b`, `use`,
//! [`crate::vm::ScriptVm::module`]) see the pending mark on their miss path, expand, and read
//! again. Heap-only readers (no VM) cannot run script: they just see the key as absent.
//!
//! Errors: an expander that panics leaves its group lazy (re-registered), so the next touch
//! retries and reports again.
use makepad_live_id::LiveId;
use makepad_relax_api::relax_api;
use crate::value::{ScriptObject, ScriptValue};
use crate::vm::ScriptVm;
use std::cell::Cell;

/// Runs a group's registration in the heap that touched it. A C-ABI fn: code Relax compiles
/// declares lazy modules too, and the VM calls the expander across that boundary (the same
/// shape as `eval_bounded_with`'s callback).
pub type LazyExpander = unsafe extern "C-unwind" fn(*mut ScriptVm);

/// [`ScriptVm::vm_lazy`] modes.
pub const LAZY_EXPAND_PENDING: u32 = 0;
pub const LAZY_EXPAND_ALL: u32 = 1;
pub const LAZY_EXPAND_KEY: u32 = 2;
pub const LAZY_HAS_PENDING: u32 = 3;
/// `name` is a domain's name (`domain::DOMAINS`)
pub const LAZY_SET_DOMAIN: u32 = 4;

#[derive(Default)]
pub struct LazyModules {
    /// `mod` key -> group index
    keys: Vec<(LiveId, u16)>,
    groups: Vec<(String, LazyExpander)>,
    /// group index + 1 of the last lazy miss (0: none)
    pending: Cell<u16>,
    /// the script domain the VM was made for (domain.rs)
    pub(crate) domain: Option<&'static crate::domain::ScriptDomain>,
}

impl LazyModules {
    /// `obj.key` missed: true (and the group pending) when it is a lazy module of `mod`.
    #[inline]
    pub(crate) fn miss(&self, obj: ScriptObject, modules: ScriptObject, key: ScriptValue) -> bool {
        if self.keys.is_empty() || obj != modules {
            return false;
        }
        self.miss_slow(key)
    }

    #[cold]
    fn miss_slow(&self, key: ScriptValue) -> bool {
        let Some(id) = key.as_id() else { return false };
        match self.keys.iter().find(|(k, _)| *k == id) {
            Some((_, group)) => {
                self.pending.set(group + 1);
                true
            }
            None => false,
        }
    }

    pub(crate) fn take_pending(&self) -> Option<u16> {
        let p = self.pending.replace(0);
        (p != 0).then(|| p - 1)
    }

    /// Declares `keys` of `mod` as one lazy group expanded by `expander`.
    pub(crate) fn declare(&mut self, name: &str, keys: &[LiveId], expander: LazyExpander) {
        let group = self.groups.len() as u16;
        self.groups.push((name.to_string(), expander));
        for key in keys {
            self.keys.retain(|(k, _)| k != key);
            self.keys.push((*key, group));
        }
    }

    /// Takes group `group` out of the table (its keys read as ordinary from now on) and hands back
    /// its expander and keys; [`Self::restore`] puts it back after a failed expansion.
    pub(crate) fn take_group(&mut self, group: u16) -> Option<(String, LazyExpander, Vec<LiveId>)> {
        let keys: Vec<LiveId> = self.keys.iter().filter(|(_, g)| *g == group).map(|(k, _)| *k).collect();
        if keys.is_empty() {
            return None;
        }
        self.keys.retain(|(_, g)| *g != group);
        let (name, expander) = self.groups[group as usize].clone();
        Some((name, expander, keys))
    }

    pub(crate) fn restore(&mut self, group: u16, keys: Vec<LiveId>) {
        for key in keys {
            self.keys.push((key, group));
        }
    }

    /// The group a `mod` key belongs to while it is unexpanded.
    pub fn group_of(&self, key: LiveId) -> Option<u16> {
        self.keys.iter().find(|(k, _)| *k == key).map(|(_, g)| *g)
    }

    /// Unexpanded groups.
    pub fn pending_groups(&self) -> Vec<u16> {
        let mut g: Vec<u16> = self.keys.iter().map(|(_, g)| *g).collect();
        g.sort_unstable();
        g.dedup();
        g
    }

    pub fn group_name(&self, group: u16) -> &str {
        self.groups.get(group as usize).map_or("", |g| g.0.as_str())
    }
}

#[relax_api]
impl ScriptVm<'_> {
    /// Declares `keys` of `mod` as one lazily expanded group: absent until a script reads one of
    /// them, then `expander` (the modules' ordinary registration) runs once in this heap.
    #[relax_api]
    pub fn declare_lazy_modules(&mut self, name: &str, keys: &[LiveId], expander: LazyExpander) {
        self.bx.heap.lazy.declare(name, keys, expander);
    }

    /// The lazy-module ops (`LAZY_*`), in the VM: expansion runs the VM's own code on the VM's
    /// own state, never code compiled elsewhere reading the heap's fields.
    #[relax_api]
    pub fn vm_lazy(&mut self, mode: u32, key: LiveId, name: &str) -> bool {
        match mode {
            LAZY_EXPAND_PENDING => match self.bx.heap.lazy.take_pending() {
                Some(group) => self.expand_lazy_group(group),
                None => false,
            },
            LAZY_EXPAND_ALL => {
                let mut any = false;
                for group in self.bx.heap.lazy.pending_groups() {
                    any |= self.expand_lazy_group(group);
                }
                any
            }
            LAZY_EXPAND_KEY => match self.bx.heap.lazy.group_of(key) {
                Some(group) => self.expand_lazy_group(group),
                None => false,
            },
            LAZY_HAS_PENDING => !self.bx.heap.lazy.pending_groups().is_empty(),
            LAZY_SET_DOMAIN => match crate::domain::DOMAINS.iter().find(|d| d.name == name) {
                Some(domain) => {
                    self.bx.heap.lazy.domain = Some(*domain);
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// Expands the group the last read missed on, if any.
    #[inline(always)]
    pub fn expand_lazy_pending(&mut self) -> bool {
        self.vm_lazy(LAZY_EXPAND_PENDING, LiveId(0), "")
    }

    /// Expands every lazy group not expanded yet.
    #[inline(always)]
    pub fn expand_all_lazy(&mut self) {
        self.vm_lazy(LAZY_EXPAND_ALL, LiveId(0), "");
    }

    /// Expands `key`'s group if `key` is a lazy `mod` key not expanded yet.
    #[inline(always)]
    pub fn expand_lazy_key(&mut self, key: LiveId) -> bool {
        self.vm_lazy(LAZY_EXPAND_KEY, key, "")
    }

    /// Whether some lazy group of this heap is not expanded yet.
    #[inline(always)]
    pub fn has_lazy_modules(&mut self) -> bool {
        self.vm_lazy(LAZY_HAS_PENDING, LiveId(0), "")
    }

    /// The script domain this VM runs documents of, by name (`domain::DOMAINS`: "ui", "motion",
    /// "cut", "score", "flow", "scene3d"): named in the errors that suggest an import.
    #[inline(always)]
    pub fn set_script_domain(&mut self, domain: &str) -> bool {
        self.vm_lazy(LAZY_SET_DOMAIN, LiveId(0), domain)
    }

    #[cold]
    #[inline(never)]
    fn expand_lazy_group(&mut self, group: u16) -> bool {
        let Some((name, expander, keys)) = self.heap_mut().lazy.take_group(group) else { return false };
        let start = crate::clock::monotonic_now();
        // The registration evaluates script while this thread is mid-instruction: pause it so the
        // nested evaluations run on another thread (as a native call's do), then come back.
        let thread = self.current_thread();
        let was_paused = self.bx.threads.cur_ref().is_paused;
        self.bx.threads.cur().is_paused = true;
        self.bx.threads.set_current_to_first_unpaused_thread();
        // The registration is the host's own code: the guest's run budget and allocation budget
        // (a Splash isolate's) are not its to spend. Instruction limits are per thread, and this
        // runs on another one.
        let run_budget = self.bx.run_budget.take();
        let allocation_budget = self.bx.heap.allocation_budget.take();
        // The caller's error capture (a document load capturing its diagnostics) is not the
        // registration's: it captures and takes its own, and the caller's is back after it.
        self.capture_errors_nested();
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { expander(self as *mut ScriptVm) }));
        let own_errors = self.take_errors();
        self.capture_errors_restore();
        for error in &own_errors {
            makepad_error_log::error!("lazy module `{name}`: {error}");
        }
        self.bx.heap.allocation_budget = allocation_budget;
        self.bx.run_budget = run_budget;
        self.set_current_thread(thread);
        self.bx.threads.cur().is_paused = was_paused;
        if let Err(payload) = out {
            self.heap_mut().lazy.restore(group, keys);
            std::panic::resume_unwind(payload);
        }
        if crate::makepad_error_log::trace_enabled("script.lazy") {
            let ip = self.bx.threads.cur_ref().trap.ip;
            let at = self.ip_to_loc(ip).map(|l| l.to_string()).unwrap_or_default();
            eprintln!("script.lazy: expanded `{name}` in {:.1} ms (touched at {at})", (crate::clock::monotonic_now() - start) * 1000.0);
        }
        true
    }
}
