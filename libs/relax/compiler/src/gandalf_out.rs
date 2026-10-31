//! Consumers of the lazy checker's structured event stream.
use crate::borrowck::{event::Origin, lazy};
use std::thread::JoinHandle;

pub struct Console { id: u64, join: JoinHandle<usize> }
impl Console {
    pub fn start(json: bool, quiet: bool) -> Self {
        let _k = crate::zone_alloc::kept();
        let (id, context) = lazy::subscribe();
        let join = std::thread::Builder::new().name("gandalf-out".into()).spawn(move || {
            let mut count = 0;
            for ev in context {
                count += 1;
                // A live-edit gate refusal is printed and emitted once, by reload's
                // report_gandalf_refusal, before the transaction returns.
                let gate = ev.origin == Origin::Gate;
                // check-before-run (Origin::Run) printed its refusal on the installing thread
                if !quiet && !gate && ev.origin != Origin::Run {
                    if ev.origin == Origin::Background {
                        eprintln!("makepad-run: [t={:.0} ms] borrow check failed: {} at {}:{}: {}\n{}", crate::clock::ms_since_start(), ev.error.fn_path, ev.error.file, ev.error.line, ev.error.message, ev.error.render());
                    } else { eprintln!("{}", ev.error.render()); }
                }
                if json && !gate { crate::diag::emit(&ev.error.to_diagnostic()); }
                if crate::run::reporting() { crate::run::report("borrowck", &ev.error.to_diagnostic().to_json()); }
            }
            count
        }).expect("start gandalf-out");
        Self { id, join }
    }
    pub fn finish(self) -> usize {
        self.finish_within(u64::MAX).unwrap_or(0)
    }
    /// Exit stops accepting events and detaches the printer, including a blocked one.
    pub fn detach(self) {
        lazy::unsubscribe(self.id);
    }
    /// As `finish`, waiting at most `ms` for the printer (it may be blocked writing to a slow
    /// stderr or report pipe): the process exit never waits on it. None: still printing.
    pub fn finish_within(self, ms: u64) -> Option<usize> {
        lazy::unsubscribe(self.id);
        let t0 = std::time::Instant::now();
        while !self.join.is_finished() {
            if t0.elapsed().as_millis() as u64 >= ms { return None; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Some(self.join.join().expect("join gandalf-out"))
    }
}
