//! Observation of compile phases for a front end that profiles its builds (Relax's per-function
//! cost and warm-start traces). The back end reports phase changes, function sizes and code
//! sizes; with no probe installed (Splash kernels) each report is one atomic load.
use crate::rir::Func;
use std::sync::OnceLock;

pub trait Probe: Sync {
    /// per-function cost tracing is on (per-pass timing then reads the thread CPU clock)
    fn cost_enabled(&self) -> bool;
    /// a compile phase starts on this thread (phase numbers are the front end's)
    fn phase_enter(&self, phase: usize);
    /// the innermost phase this thread entered ends
    fn phase_leave(&self);
    /// the back end starts on function `name` on this thread
    fn backend_enter(&self, name: &str);
    /// the innermost back-end scope this thread entered ends
    fn backend_leave(&self);
    /// the function the back end receives
    fn size(&self, f: &Func);
    /// the machine code size the back end produced
    fn code(&self, bytes: usize);
}

static PROBE: OnceLock<&'static dyn Probe> = OnceLock::new();

/// Installs the process's probe (the first one installed stays).
pub fn install(p: &'static dyn Probe) {
    let _ = PROBE.set(p);
}

#[inline]
fn get() -> Option<&'static dyn Probe> {
    PROBE.get().copied()
}

pub fn cost_enabled() -> bool {
    get().is_some_and(|p| p.cost_enabled())
}

/// A phase, left when dropped.
pub struct Phase(Option<&'static dyn Probe>);

pub fn phase(p: usize) -> Phase {
    let probe = get();
    if let Some(x) = probe {
        x.phase_enter(p);
    }
    Phase(probe)
}

impl Drop for Phase {
    fn drop(&mut self) {
        if let Some(p) = self.0 {
            p.phase_leave();
        }
    }
}

/// The back end's work on one function, left when dropped.
pub struct Backend(Option<&'static dyn Probe>);

pub fn backend(name: &str) -> Backend {
    let probe = get();
    if let Some(x) = probe {
        x.backend_enter(name);
    }
    Backend(probe)
}

impl Drop for Backend {
    fn drop(&mut self) {
        if let Some(p) = self.0 {
            p.backend_leave();
        }
    }
}

pub fn size(f: &Func) {
    if let Some(p) = get() {
        p.size(f);
    }
}

pub fn code(bytes: usize) {
    if let Some(p) = get() {
        p.code(bytes);
    }
}
