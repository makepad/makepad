//! `init_once`: a `OnceLock` whose initialiser runs queries (a declaration's signature, a
//! constant's type, a declaration cell) without any thread ever waiting in the cell's `Once`.
//!
//! `OnceLock::get_or_init` blocks every other reader in `Once::call` -- a wait the query engine
//! cannot see: no cycle check, no pump on the JIT's owner thread, no preemption point. The
//! initialiser itself waits for queries other threads run, so a reader blocked in the `Once`
//! while the initialising thread waits for a query that reader runs is a cycle nobody can break
//! (the owner initialised a signature, pumped inside it, and a lowering-ahead helper slept in that
//! signature's `Once`: Sandbox's race game, 2026-10-21). Here the value is computed with the `Once`
//! free and published with `set` (the first value stands). A thread that meets a cell another
//! thread is computing computes it too: every wait inside goes through the query engine (its
//! latches, its cycle check, the owner's pump), so there is no wait the engine does not see. The
//! initialisers are pure up to diagnostics, which the publishing caller records (`init_once`'s
//! `publish` argument), so a value computed twice is reported once.

std::thread_local! {
    /// cells this thread is computing, innermost last (a fixed array: no heap in a thread-local
    /// that outlives compiler generations); beyond DEPTH the deepest are not tracked
    static COMPUTING: std::cell::Cell<([usize; DEPTH], usize)> = const { std::cell::Cell::new(([0; DEPTH], 0)) };
}
const DEPTH: usize = 64;

struct Pop;

impl Drop for Pop {
    fn drop(&mut self) {
        let _ = COMPUTING.try_with(|c| {
            let (a, n) = c.get();
            c.set((a, n - 1));
        });
    }
}

/// Is this thread computing the cell at `key` lower on its stack? Else pushes it (Some: pop on drop).
fn enter(key: usize) -> Result<Option<Pop>, ()> {
    COMPUTING
        .try_with(|c| {
            let (mut a, n) = c.get();
            if a[..n.min(DEPTH)].contains(&key) {
                return Err(());
            }
            if n < DEPTH {
                a[n] = key;
            }
            c.set((a, n + 1));
            Ok(Some(Pop))
        })
        .unwrap_or(Ok(None))
}

/// `cell.get_or_init(compute)`, with `compute` run outside the cell's `Once` (module doc); `publish`
/// runs only for the value that is stored (its diagnostics; leaf work). A cell asked for again on the thread
/// computing it is a cycle (a `OnceLock` deadlocked there): on the JIT's owner inside a nested
/// request, the request is cancelled (the wait hook's rule for a query the owner runs lower on its
/// stack); anywhere else it panics.
pub fn init_once<'a, T>(cell: &'a std::sync::OnceLock<T>, compute: impl FnOnce() -> T, publish: impl FnOnce(&mut T)) -> &'a T {
    if let Some(v) = cell.get() {
        return v;
    }
    let key = cell as *const std::sync::OnceLock<T> as usize;
    let _pop = match enter(key) {
        Ok(pop) => pop,
        Err(()) => {
            if let Some(hook) = super::job::WAIT_HOOK.get().filter(|h| (h.on_owner)()) {
                // the pump's self-wait rule: a nested request is cancelled (OwnerBusy)
                (hook.pump)((hook.me)(), &|| cell.get().is_some());
                if let Some(v) = cell.get() {
                    return v;
                }
            }
            panic!("makepad: a declaration asks for itself while this thread computes it (a cycle)");
        }
    };
    let mut value = compute();
    // the store, and the stored value's diagnostics (leaf work: no query, no wait), are all the
    // cell's `Once` ever covers
    cell.get_or_init(|| {
        publish(&mut value);
        value
    })
}

#[cfg(test)]
mod tests {
    use super::init_once;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::OnceLock;

    /// The shape of the Sandbox hang: the thread initialising a cell waits (inside its
    /// initialiser) for work of a second thread, and that second thread needs the same cell. With
    /// `OnceLock::get_or_init` the second thread sleeps in the `Once` and the first never finishes;
    /// here the second computes the value itself and both go on. The value is stored once and its
    /// diagnostics are published once.
    #[test]
    fn a_reader_of_a_cell_being_initialised_never_waits_in_its_once() {
        static CELL: OnceLock<u32> = OnceLock::new();
        static SECOND_DONE: AtomicBool = AtomicBool::new(false);
        static PUBLISHED: AtomicU32 = AtomicU32::new(0);
        let first = std::thread::spawn(|| {
            *init_once(&CELL, || {
                let second = std::thread::spawn(|| *init_once(&CELL, || 2, |_| { PUBLISHED.fetch_add(1, Ordering::SeqCst); }));
                // the first initialiser waits for the second reader (a query it runs)
                let v = second.join().unwrap();
                SECOND_DONE.store(true, Ordering::SeqCst);
                assert_eq!(v, 2);
                1
            }, |_| { PUBLISHED.fetch_add(1, Ordering::SeqCst); })
        });
        assert_eq!(first.join().unwrap(), 2, "the first stored value stands");
        assert!(SECOND_DONE.load(Ordering::SeqCst));
        assert_eq!(*CELL.get().unwrap(), 2);
        assert_eq!(PUBLISHED.load(Ordering::SeqCst), 1, "only the stored value's diagnostics are published");
    }

    #[test]
    fn a_cell_asked_for_by_its_own_initialiser_is_a_cycle_panic() {
        static CELL: OnceLock<u32> = OnceLock::new();
        let r = std::thread::spawn(|| *init_once(&CELL, || *init_once(&CELL, || 1, |_| {}), |_| {})).join();
        let p = r.expect_err("a self-asking initialiser panics (a OnceLock deadlocked)");
        let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
        assert!(msg.contains("asks for itself"), "{msg}");
        // the unwound computation left nothing behind: the cell computes normally afterwards
        assert_eq!(*init_once(&CELL, || 7, |_| {}), 7);
    }
}
