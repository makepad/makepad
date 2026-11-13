//! Consumers of the lazy checker's structured event stream.
use crate::borrowck::{event::{GandalfEvent, Origin}, lazy};
use std::thread::JoinHandle;

/// (a run detaches the printer; a test waits for it to read what it printed)
pub struct Console { id: u64, #[cfg(test)] join: JoinHandle<usize> }
impl Console {
    pub fn start(json: bool, quiet: bool) -> Self {
        let _k = crate::zone_alloc::kept();
        let (id, context) = lazy::subscribe();
        let join = crate::run_threads::spawn("gandalf-out", move || {
            let mut count = 0;
            for ev in context {
                // The test dependency check prints and reports these when the tests end.
                if ev.origin == Origin::DeferredCheck { continue; }
                count += 1;
                // A live-edit gate refusal is printed and emitted once, by reload's
                // report_gandalf_refusal, before the transaction returns.
                let gate = ev.origin == Origin::Gate;
                // check-before-run (Origin::Run) printed its refusal on the installing thread
                if !quiet && !gate && ev.origin != Origin::Run {
                    if ev.origin == Origin::Background {
                        eprintln!("relax: [t={:.0} ms] borrow check failed: {} at {}:{}: {}\n{}", crate::clock::ms_since_start(), ev.error.fn_path, ev.error.file, ev.error.line, ev.error.message, ev.error.render());
                    } else { eprintln!("{}", ev.error.render()); }
                }
                if json && !gate { crate::diag::emit(&ev.error.to_diagnostic()); }
                if crate::run::reporting() { crate::run::report("borrowck", &ev.error.to_diagnostic().to_json()); }
            }
            count
        }).expect("start gandalf-out");
        #[cfg(not(test))]
        drop(join);
        Self { id, #[cfg(test)] join }
    }
    /// Exit stops accepting events and detaches the printer, including a blocked one.
    pub fn detach(self) {
        lazy::unsubscribe(self.id);
    }
    /// The events printed, once the printer has printed them all (a test reads its stderr).
    #[cfg(test)]
    pub fn finish(self) -> usize {
        lazy::unsubscribe(self.id);
        self.join.join().expect("join gandalf-out")
    }
}

/// A check's borrow errors, kept for its one ordered print (check_order.rs) instead of printed as
/// they come: the check's threads finish in any order. The run's report gets each as it comes.
pub struct Collector { id: u64, join: JoinHandle<Vec<GandalfEvent>> }
impl Collector {
    pub fn start() -> Self {
        let _k = crate::zone_alloc::kept();
        let (id, context) = lazy::subscribe();
        let join = crate::run_threads::spawn("gandalf-collect", move || {
            let mut events = Vec::new();
            for ev in context {
                // The test dependency check prints and reports these when the tests end.
                if ev.origin == Origin::DeferredCheck { continue; }
                if crate::run::reporting() { crate::run::report("borrowck", &ev.error.to_diagnostic().to_json()); }
                events.push(ev);
            }
            events
        }).expect("start gandalf-collect");
        Self { id, join }
    }
    /// Every borrow error the check reported, in the order they came.
    pub fn finish(self) -> Vec<GandalfEvent> {
        lazy::unsubscribe(self.id);
        self.join.join().expect("join gandalf-collect")
    }
}
