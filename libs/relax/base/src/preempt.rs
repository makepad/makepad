//! Background front-end work that gives way to a writer or shutdown.
//!
//! A background check holds the front end's read lock (jit::FRONT_RW) for one item; a writer
//! (a live patch committing its new file versions) waits for every reader. One background
//! type check can take seconds (a solver search at a generic owner's identity arguments), and
//! the patch waited for it. A thread that opts in (`Preemptible`) stops at the next
//! preemption point (a solver goal, an expression) while a writer waits: the work unwinds
//! with the `Preempted` payload (`std::panic::resume_unwind`: no panic hook, no message), its
//! read guard drops, and the owner of that work queues it again. Only per-check state lives
//! on the unwound frames (the EvalState, the inference tables); shared tables take finished
//! answers only.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static STOPPED: AtomicBool = AtomicBool::new(false);

/// The process is ending: every opted-in thread unwinds at its next preemption point, now and
/// from then on (its owner sees `is_preempted` / `is_stopped` and must not queue the work again).
pub fn stop_all() { STOPPED.store(true, Ordering::SeqCst); }
pub fn is_stopped() -> bool { STOPPED.load(Ordering::Acquire) }

static WRITERS: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static ON: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The unwind payload of preempted work.
pub struct Preempted;

/// Held by a front-end writer while it waits for the lock: preemptible readers stop.
pub struct WriterWaiting(());
impl WriterWaiting {
    pub fn new() -> Self {
        WRITERS.fetch_add(1, Ordering::SeqCst);
        WriterWaiting(())
    }
}
impl Drop for WriterWaiting {
    fn drop(&mut self) {
        WRITERS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The calling thread's work in this scope may be preempted.
pub struct Preemptible(bool);
impl Preemptible {
    pub fn on() -> Self {
        Preemptible(ON.with(|c| c.replace(true)))
    }
}
impl Drop for Preemptible {
    fn drop(&mut self) {
        let prev = self.0;
        ON.with(|c| c.set(prev));
    }
}

/// Unwinds with `Preempted` on opted-in threads when a writer waits or shutdown begins.
#[inline]
pub fn point() {
    if (WRITERS.load(Ordering::Relaxed) != 0 || is_stopped()) && ON.try_with(|c| c.get()).unwrap_or(false) {
        std::panic::resume_unwind(Box::new(Preempted));
    }
}

/// Whether a caught unwind payload is a preemption (not a failure).
pub fn is_preempted(payload: &(dyn std::any::Any + Send)) -> bool {
    payload.is::<Preempted>()
}

/// The unwind payload of work inside a build that needs a module the lazy Rust selection
/// skipped (the compiler's post-boot loader, jit.rs DEFERRED_RETRY): a constant evaluated for
/// the build failed for want of that module. It is no result: every query it crosses is
/// cancelled (not cached, nothing reported), and the build fails with DEFERRED_RETRY so its
/// outer compile loads the module and builds again.
pub struct NeedsLoad;

/// The unwind payload of off-lock lowering ahead of a build (the compiler's lowering helpers)
/// that reached work only the unit lock's owner may do: a constant or static evaluated, an
/// audit. The ordinary build lowers the body again under the lock. A constant evaluation it
/// interrupts has no result, so every query it crosses is cancelled (not cached as an error).
pub struct NeedsUnit;

/// The unwind payload of a request the JIT's owner thread served nested, inside a wait of an
/// earlier request (compiler/src/owner.rs): it reached something only that earlier request can
/// finish -- a query or a build it has open, a load. It is no result: every query it crosses is
/// cancelled, and its requester posts it again once the owner has moved on.
pub struct OwnerBusy;

/// The unwind payload of an operation a publish to the JIT's owner may not run (compiler/src/
/// owner.rs `publish_or_bail`, rule I-PUB): a front, an initializer, JIT code, a wait. Like the
/// others it is no result: a query it crosses records nothing, and the background build whose take
/// it ends fronts `slots` off the owner (empty: no front can help) and takes again, or is dropped.
/// A take collects every inline candidate needing a front before it bails (compiler jit.rs
/// `fronts_or_bail`), so its rounds are the inline depth, not the candidate count.
pub struct PublishBail {
    pub what: &'static str,
    pub slots: Vec<u32>,
}

/// Whether a caught unwind payload is work to be redone rather than a failure: a preemption,
/// a build waiting for the post-boot loader (NeedsLoad), lowering ahead that needs the unit
/// (NeedsUnit), a request the owner thread could not serve yet (OwnerBusy), or a publish that bailed
/// (PublishBail). A query it
/// crosses is cancelled.
pub fn is_cancelled(payload: &(dyn std::any::Any + Send)) -> bool {
    payload.is::<Preempted>() || payload.is::<NeedsLoad>() || payload.is::<NeedsUnit>() || payload.is::<OwnerBusy>() || payload.is::<PublishBail>()
}

#[cfg(test)]
mod tests {
    #[test]
    fn point_unwinds_only_opted_in_threads_while_a_writer_waits() {
        super::point();
        let _on = super::Preemptible::on();
        super::point();
        let w = super::WriterWaiting::new();
        let r = std::panic::catch_unwind(super::point);
        assert!(r.is_err_and(|p| super::is_preempted(&*p)));
        let other = std::thread::spawn(|| std::panic::catch_unwind(super::point).is_ok()).join().unwrap();
        assert!(other, "a thread that did not opt in was preempted");
        drop(w);
        super::point();
    }
}
