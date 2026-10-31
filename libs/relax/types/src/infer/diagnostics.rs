// ported from N:rustc_errors/src/lib.rs @8d1a764
use crate::next_solver::fulfill::ObligationCause;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use relax_session::sync::Mutex;

thread_local! { static THREAD_EMITTED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
#[derive(Clone, Debug)]
pub struct Diagnostic { pub cause: ObligationCause, pub message: String }
struct Emitted { diagnostic: Diagnostic, thread: u32, delivered: bool }
#[derive(Default)]
struct DiagCtxtInner {
    errors: Vec<Emitted>,
    warnings: Vec<Emitted>,
    delayed_bugs: Vec<Diagnostic>,
    reported_error: bool,
}

/// The session's diagnostics, shared by every inference context and query.
/// rustc_errors/src/lib.rs:1427: delayed bugs belong to the session, not a query
/// result. A handle made by `into_taintable` is rustc_errors' DiagCtxtHandle with
/// `tainted_with_errors` (lib.rs:279,566-571): every error emitted through it, also a
/// deduplicated one (lib.rs:1362-1371), taints its owner (the InferCtxt).
#[derive(Clone, Default)]
pub struct DiagCtxt { data: Arc<Mutex<DiagCtxtInner>>, tainted_with_errors: Option<Arc<AtomicBool>> }
#[derive(Clone, Copy)]
pub struct DiagnosticCheckpoint { errors: usize, warnings: usize }
impl DiagCtxt {
    pub fn checkpoint(&self) -> DiagnosticCheckpoint {
        let data = self.data.lock().unwrap();
        DiagnosticCheckpoint { errors: data.errors.len(), warnings: data.warnings.len() }
    }
    pub fn err_count(&self) -> usize { self.data.lock().unwrap().errors.len() }
    /// A handle on the same session diagnostics with its own taint flag.
    pub fn into_taintable(&self) -> Self { Self { data: self.data.clone(), tainted_with_errors: Some(Arc::new(AtomicBool::new(false))) } }
    /// The session diagnostics without this handle's taint flag.
    pub fn untainted(&self) -> Self { Self { data: self.data.clone(), tainted_with_errors: None } }
    pub fn tainted_with_errors(&self) -> bool { self.tainted_with_errors.as_ref().is_some_and(|t| t.load(Ordering::Relaxed)) }
    fn taint(&self) { if let Some(t) = &self.tainted_with_errors { t.store(true, Ordering::Relaxed); } }
    pub fn has_errors(&self) -> bool { self.data.lock().unwrap().reported_error }
    pub fn note_reported_error(&self) { self.data.lock().unwrap().reported_error = true; }
    pub fn report(&self, cause: ObligationCause, message: String) {
        self.taint();
        let mut data = self.data.lock().unwrap();
        data.reported_error = true;
        Self::emit(&mut data.errors, cause, message);
    }
    pub fn report_to_buffer(&self, cause: ObligationCause, message: String) -> Option<Diagnostic> {
        self.taint();
        let mut data = self.data.lock().unwrap();
        data.reported_error = true;
        let index = data.errors.len();
        Self::emit(&mut data.errors, cause, message);
        if data.errors.len() == index { return None; }
        data.errors[index].delivered = true;
        Some(data.errors[index].diagnostic.clone())
    }
    /// rustc_errors/src/lib.rs (emit_fatal) and rustc_errors/src/fatal.rs:
    /// a fatal diagnostic aborts compilation through the unwinding boundary.
    pub fn fatal(&self, cause: ObligationCause, message: String) -> ! {
        self.report(cause, message.clone());
        std::panic::resume_unwind(Box::new(FatalError(message)));
    }
    pub fn warn(&self, cause: ObligationCause, message: String) {
        Self::emit(&mut self.data.lock().unwrap().warnings, cause, message);
    }
    /// Diagnostics (errors and warnings, also deduplicated ones) this thread has emitted so far:
    /// a check that ran on one thread compares it before and after to know it found nothing.
    pub fn thread_emitted() -> u64 { THREAD_EMITTED.with(|c| c.get()) }
    fn emit(log: &mut Vec<Emitted>, cause: ObligationCause, message: String) {
        THREAD_EMITTED.with(|c| c.set(c.get() + 1));
        if log.iter().any(|d| d.diagnostic.cause == cause && d.diagnostic.message == message) { return; }
        log.push(Emitted { diagnostic: Diagnostic { cause, message }, thread: crate::conc::thread_id(), delivered: false });
    }
    pub fn delayed_bug(&self, cause: ObligationCause, message: String) {
        let mut data = self.data.lock().unwrap();
        if !data.delayed_bugs.iter().any(|d| d.cause == cause && d.message == message) {
            data.delayed_bugs.push(Diagnostic { cause, message });
        }
    }
    pub fn drain_reported(&self) -> (Vec<Diagnostic>, Vec<Diagnostic>) {
        let mut data = self.data.lock().unwrap();
        (Self::take(&mut data.errors, 0, false), Self::take(&mut data.warnings, 0, false))
    }
    /// Body error lists feed JIT stubbing. Capture diagnostics emitted on this
    /// checking thread; unrelated concurrent bodies keep their own diagnostics.
    pub fn take_since(&self, checkpoint: DiagnosticCheckpoint) -> (Vec<Diagnostic>, Vec<Diagnostic>) {
        let mut data = self.data.lock().unwrap();
        (Self::take(&mut data.errors, checkpoint.errors, true), Self::take(&mut data.warnings, checkpoint.warnings, true))
    }
    fn take(log: &mut [Emitted], start: usize, current_thread: bool) -> Vec<Diagnostic> {
        let thread = crate::conc::thread_id();
        let mut result = Vec::new();
        for item in &mut log[start..] {
            if !item.delivered && (!current_thread || item.thread == thread) {
                item.delivered = true;
                result.push(item.diagnostic.clone());
            }
        }
        result
    }
    /// The delayed bugs not yet turned into errors, taken (a check reports each as an internal
    /// failure instead of ending the session on them).
    pub fn take_delayed(&self) -> Vec<String> {
        let mut data = self.data.lock().unwrap();
        std::mem::take(&mut data.delayed_bugs).into_iter().map(|bug| bug.message).collect()
    }
    pub fn flush_delayed(&self) {
        let bugs = {
            let mut data = self.data.lock().unwrap();
            let bugs = std::mem::take(&mut data.delayed_bugs);
            if data.reported_error { return; }
            bugs
        };
        if !bugs.is_empty() {
            let mut message = String::new();
            for bug in bugs {
                if !message.is_empty() { message.push('\n'); }
                message.push_str(&bug.message);
            }
            panic!("delayed bug at session end: {message}");
        }
    }
}

#[derive(Debug)]
pub struct FatalError(pub String);
