//! An invariant that degrades instead of taking the show down.
//!
//! A `debug_assert!` is silent in the build that goes to a gig, and an
//! `assert!` takes the whole app with it. Neither is what an operator in
//! front of an audience needs: they need the thing to keep making sound and
//! to say afterwards what went wrong. `verify_or!` does that — it names the
//! invariant, counts how often it broke, and runs the caller's own fallback.

use std::sync::atomic::{AtomicU64, Ordering};

/// Check an invariant; on a break, say so and run the fallback.
///
/// The fallback is the caller's own block, so it can `return`, `continue`,
/// or simply put a safe value in place. Nothing panics and nothing is
/// compiled out, so the build that goes to a gig checks the same things the
/// build on the bench does.
#[macro_export]
macro_rules! verify_or {
    ($cond:expr, $fallback:block) => {
        if !($cond) {
            static SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            $crate::verify::report(&SEEN, file!(), line!(), stringify!($cond));
            $fallback
        }
    };
}

/// Count one break and decide whether to say it out loud. Returns whether
/// it was said.
///
/// Said on the 1st, 10th, 100th and so on: an invariant that breaks once a
/// buffer would otherwise fill the log faster than anyone could read it,
/// and the log is the only place this leaves a trace. The counting itself
/// is one atomic add, which is safe on the audio thread; the logging is
/// not free, which is why it thins out at once.
pub fn report(seen: &AtomicU64, file: &str, line: u32, invariant: &str) -> bool {
    let count = seen.fetch_add(1, Ordering::Relaxed) + 1;
    if !count.is_power_of_ten() {
        return false;
    }
    crate::log!("verify: {file}:{line} broke `{invariant}` ({count} time(s))");
    true
}

trait PowerOfTen {
    fn is_power_of_ten(self) -> bool;
}

impl PowerOfTen for u64 {
    fn is_power_of_ten(mut self) -> bool {
        if self == 0 {
            return false;
        }
        while self % 10 == 0 {
            self /= 10;
        }
        self == 1
    }
}

/// The thread a lane is supposed to run on, claimed by whoever gets there
/// first.
///
/// The audio callback, the decode threads and the UI each own state that
/// the others must not touch. Nothing here enforces that; this only lets a
/// lane ask whether it is still where it started, so a check can say so
/// rather than the app finding out as a data race.
pub struct ThreadMark {
    id: AtomicU64,
    #[allow(dead_code)]
    name: &'static str,
}

impl ThreadMark {
    pub const fn new(name: &'static str) -> ThreadMark {
        ThreadMark { id: AtomicU64::new(0), name }
    }

    /// Whether this is the thread that claimed the mark. The first caller
    /// claims it and is always answered yes.
    pub fn same_thread(&self) -> bool {
        // Thread ids start at 1, so 0 is safe as "nobody yet".
        let me = thread_id();
        match self.id.compare_exchange(0, me, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => true,
            Err(claimed) => claimed == me,
        }
    }
}

fn thread_id() -> u64 {
    // `ThreadId` has no stable numeric form, so hash its debug shape once
    // per thread and keep it.
    use std::cell::Cell;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    thread_local! {
        static ID: Cell<u64> = const { Cell::new(0) };
    }
    ID.with(|cell| {
        let had = cell.get();
        if had != 0 {
            return had;
        }
        let mut hasher = DefaultHasher::new();
        std::thread::current().id().hash(&mut hasher);
        // Never 0: that is the unclaimed mark.
        let id = hasher.finish() | 1;
        cell.set(id);
        id
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn a_holding_invariant_costs_nothing_and_runs_no_fallback() {
        let mut ran = false;
        verify_or!(1 + 1 == 2, { ran = true });
        assert!(!ran, "the fallback is for when it breaks");
    }

    #[test]
    fn a_broken_invariant_runs_the_fallback() {
        let mut ran = false;
        verify_or!(1 + 1 == 3, { ran = true });
        assert!(ran);
    }

    #[test]
    fn the_fallback_may_leave_the_function() {
        fn pick(ok: bool) -> &'static str {
            verify_or!(ok, { return "fell back" });
            "went through"
        }
        assert_eq!(pick(true), "went through");
        assert_eq!(pick(false), "fell back");
    }

    #[test]
    fn every_break_is_counted_and_only_some_are_said_out_loud() {
        static SEEN: AtomicU64 = AtomicU64::new(0);
        let mut spoken = 0;
        for _ in 0..1000 {
            if report(&SEEN, "file.rs", 7, "thing.is_some()") {
                spoken += 1;
            }
        }
        assert_eq!(SEEN.load(std::sync::atomic::Ordering::Relaxed), 1000, "all counted");
        assert_eq!(spoken, 4, "the 1st, 10th, 100th and 1000th: 1, 10, 100, 1000");
    }

    #[test]
    fn the_first_break_is_always_said() {
        static SEEN: AtomicU64 = AtomicU64::new(0);
        assert!(report(&SEEN, "file.rs", 7, "x"), "silence on the first would hide it entirely");
    }

    #[test]
    fn a_thread_mark_knows_the_thread_that_claimed_it() {
        static MARK: ThreadMark = ThreadMark::new("test lane");
        assert!(MARK.same_thread(), "the first caller claims it");
        assert!(MARK.same_thread(), "and is still itself");
        let elsewhere = std::thread::spawn(|| MARK.same_thread()).join().unwrap();
        assert!(!elsewhere, "another thread is not that thread");
        assert!(MARK.same_thread(), "and the claim did not move");
    }
}
