//! Locks that say who holds them. A hub's long-held locks (the lease start, its leases, a sync's
//! apply) record their holder (the call site and its thread) and who waits on them. A box whose
//! lease start waits then shows what it waits on and who holds it (`caps.stalls`, the master's
//! `nodes` and its lease reasons), and the hub log names every wait that took a while with its
//! holder. Before this, a self-update ask held the leases and asked for them again on its own
//! thread: a bench box's lease starts and ends waited 25 min and nothing said why (11-03).

use std::panic::Location;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError, Weak};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// A wait at least this long is written to the hub log when it ends, with its holder.
const SAY_WAIT: Duration = Duration::from_secs(1);

#[derive(Clone)]
struct Who {
    at: &'static Location<'static>,
    thread: ThreadId,
    name: String,
    since: Instant,
}

impl Who {
    #[track_caller]
    fn here() -> Who {
        let t = std::thread::current();
        Who { at: Location::caller(), thread: t.id(), name: t.name().unwrap_or("unnamed").to_string(), since: Instant::now() }
    }

    fn line(&self) -> String {
        format!("{}:{} ({})", self.at.file().rsplit_once("src/").or_else(|| self.at.file().rsplit_once("src\\")).map_or(self.at.file(), |(_, f)| f), self.at.line(), self.name)
    }
}

#[derive(Default)]
struct Seen {
    holder: Option<Who>,
    waiting: Vec<Who>,
}

/// Every live [`Held`] by name, for [`stalls`].
static ALL: Mutex<Vec<(&'static str, Weak<Mutex<Seen>>)>> = Mutex::new(Vec::new());

fn seen_of(seen: &Mutex<Seen>) -> MutexGuard<'_, Seen> {
    seen.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A mutex that knows its holder and its waiters ([`stalls`]).
pub struct Held<T> {
    name: &'static str,
    mu: Mutex<T>,
    seen: Arc<Mutex<Seen>>,
}

pub struct HeldGuard<'a, T> {
    guard: MutexGuard<'a, T>,
    seen: &'a Mutex<Seen>,
}

impl<T> std::ops::Deref for HeldGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> std::ops::DerefMut for HeldGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<T> Drop for HeldGuard<'_, T> {
    fn drop(&mut self) {
        seen_of(self.seen).holder = None;
    }
}

/// [`Held::try_lock`]: another holds it.
#[derive(Debug)]
pub struct WouldBlock;

pub type HeldResult<'a, T> = Result<HeldGuard<'a, T>, PoisonError<HeldGuard<'a, T>>>;

impl<T> Held<T> {
    pub fn new(name: &'static str, value: T) -> Held<T> {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut all = ALL.lock().unwrap_or_else(PoisonError::into_inner);
        all.retain(|(_, w)| w.strong_count() > 0);
        all.push((name, Arc::downgrade(&seen)));
        Held { name, mu: Mutex::new(value), seen }
    }

    fn held<'a>(&'a self, guard: MutexGuard<'a, T>, me: Who) -> HeldGuard<'a, T> {
        seen_of(&self.seen).holder = Some(me);
        HeldGuard { guard, seen: &self.seen }
    }

    /// Like [`Mutex::lock`]; a wait is seen by [`stalls`] while it lasts and said in the hub log
    /// when it took a while.
    #[track_caller]
    pub fn lock(&self) -> HeldResult<'_, T> {
        let me = Who::here();
        let got = match self.mu.try_lock() {
            Ok(g) => Ok(g),
            Err(TryLockError::Poisoned(p)) => Err(p),
            Err(TryLockError::WouldBlock) => {
                let holder = {
                    let mut seen = seen_of(&self.seen);
                    seen.waiting.push(me.clone());
                    seen.holder.clone()
                };
                if holder.as_ref().is_some_and(|h| h.thread == me.thread) {
                    eprintln!("devhub: {} waits on {} that its own thread holds since {}: it never frees", me.line(), self.name, holder.as_ref().map_or(String::new(), Who::line));
                }
                let got = self.mu.lock();
                let mut seen = seen_of(&self.seen);
                if let Some(i) = seen.waiting.iter().position(|w| w.thread == me.thread && w.since == me.since) {
                    seen.waiting.swap_remove(i);
                }
                drop(seen);
                let waited = me.since.elapsed();
                if waited >= SAY_WAIT {
                    eprintln!("devhub: {} waited {:.1} s on {} held by {}", me.line(), waited.as_secs_f64(), self.name, holder.as_ref().map_or("one gone by then".to_string(), Who::line));
                }
                got
            }
        };
        match got {
            Ok(g) => Ok(self.held(g, me)),
            Err(p) => Err(PoisonError::new(self.held(p.into_inner(), me))),
        }
    }

    /// Like [`Mutex::try_lock`] (a poisoned lock is taken as it is).
    #[track_caller]
    pub fn try_lock(&self) -> Result<HeldGuard<'_, T>, WouldBlock> {
        let me = Who::here();
        match self.mu.try_lock() {
            Ok(g) => Ok(self.held(g, me)),
            Err(TryLockError::Poisoned(p)) => Ok(self.held(p.into_inner(), me)),
            Err(TryLockError::WouldBlock) => Err(WouldBlock),
        }
    }

    /// Who holds it now (`file:line (thread)` and for how long), if anyone.
    pub fn holder(&self) -> Option<String> {
        seen_of(&self.seen).holder.as_ref().map(|h| format!("{} for {} s", h.line(), h.since.elapsed().as_secs()))
    }
}

/// The waits in this process now, one line each: `<waiter> waits on <lock> held by <holder>`,
/// and what the holder itself waits on (its own thread: a deadlock). A line's text before ` (for`
/// stays the same while the wait does.
pub fn stalls() -> Vec<String> {
    let all: Vec<(&'static str, Arc<Mutex<Seen>>)> = ALL.lock().unwrap_or_else(PoisonError::into_inner).iter().filter_map(|(n, w)| Some((*n, w.upgrade()?))).collect();
    let view: Vec<(&'static str, Option<Who>, Vec<Who>)> = all.iter().map(|(n, s)| {
        let s = seen_of(s);
        (*n, s.holder.clone(), s.waiting.clone())
    }).collect();
    let mut out = Vec::new();
    for (name, holder, waiting) in &view {
        for w in waiting {
            let mut line = format!("{} waits on {name}", w.line());
            match holder {
                Some(h) if h.thread == w.thread => line.push_str(&format!(" that its own thread holds ({}): a deadlock", h.line())),
                Some(h) => {
                    line.push_str(&format!(" held by {}", h.line()));
                    // what the holder waits on, if anything: a chain ends at a holder that runs
                    if let Some((other, oh)) = view.iter().find_map(|(n, oh, ws)| ws.iter().any(|x| x.thread == h.thread).then_some((n, oh))) {
                        line.push_str(&format!(", which waits on {other}"));
                        if let Some(oh) = oh {
                            line.push_str(&format!(" held by {}{}", oh.line(), if oh.thread == h.thread { ": a deadlock" } else { "" }));
                        }
                    }
                }
                None => line.push_str(" (freed just now)"),
            }
            line.push_str(&format!(" (for {} s)", w.since.elapsed().as_secs()));
            out.push(line);
        }
    }
    out
}

/// A stall line's lasting part (without how long so far): the same wait on two looks.
pub fn stall_key(line: &str) -> &str {
    line.rsplit_once(" (for ").map_or(line, |(k, _)| k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_names_its_holder_and_a_lock_asked_again_on_its_thread_is_a_deadlock() {
        let a = Arc::new(Held::new("held-test-a", 0u32));
        let g = a.lock().unwrap();
        assert!(a.holder().is_some_and(|h| h.starts_with("held.rs:")), "{:?}", a.holder());
        let (tx, rx) = std::sync::mpsc::channel();
        let b = a.clone();
        let t = std::thread::Builder::new().name("held-waiter".into()).spawn(move || {
            tx.send(()).unwrap();
            *b.lock().unwrap() += 1;
        }).unwrap();
        rx.recv().unwrap();
        let mut line = None;
        while line.is_none() {
            line = stalls().into_iter().find(|l| l.contains("held-test-a"));
            std::thread::yield_now();
        }
        let line = line.unwrap();
        assert!(line.contains("(held-waiter) waits on held-test-a held by held.rs:"), "{line}");
        assert_eq!(stall_key(&line), line.rsplit_once(" (for ").unwrap().0);
        drop(g);
        t.join().unwrap();
        assert_eq!(*a.lock().unwrap(), 1);
        assert!(a.holder().is_none() && !stalls().iter().any(|l| l.contains("held-test-a")), "nothing held, nothing waits");

        // the same thread asking again: said as a deadlock from the state (the second ask
        // runs on a helper that poses as the holder's thread only by sharing its holder record)
        let c = Held::new("held-test-c", ());
        let _held = c.lock().unwrap();
        let me = Who::here();
        seen_of(&c.seen).waiting.push(me);
        let line = stalls().into_iter().find(|l| l.contains("held-test-c")).unwrap();
        assert!(line.contains("that its own thread holds") && line.ends_with("s)") && line.contains("a deadlock"), "{line}");
        seen_of(&c.seen).waiting.clear();
    }
}
