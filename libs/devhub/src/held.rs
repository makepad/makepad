//! Locks that say who holds them. A hub's long-held locks (the lease start, its leases, a sync's
//! apply) record their holder (the call site and its thread) and who waits on them. A box whose
//! lease start waits then shows what it waits on and who holds it (`caps.stalls`, the master's
//! `nodes` and its lease reasons), and the hub log names every wait that took a while with its
//! holder. Before this, a self-update ask held the leases and asked for them again on its own
//! thread: a bench box's lease starts and ends waited 25 min and nothing said why (11-03).

use std::panic::Location;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, TryLockError, Weak};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// A wait at least this long is written to the hub log with its holder.
const SAY_WAIT: Duration = Duration::from_secs(1);

#[derive(Clone)]
struct Who {
    // A gate entry's key. A closer or a mutex holder takes it alone.
    key: Option<Arc<str>>,
    at: &'static Location<'static>,
    thread: ThreadId,
    name: String,
    since: Instant,
}

impl Who {
    #[track_caller]
    fn here() -> Who {
        let t = std::thread::current();
        Who { key: None, at: Location::caller(), thread: t.id(), name: t.name().unwrap_or("unnamed").to_string(), since: Instant::now() }
    }

    /// `file:line [key] (thread)`; a gate entry's key in brackets: a lease start's is its lease. A
    /// Windows box's start read "lease\exec.rs:3169 (makepad-remote-lease-start) waits on lease
    /// start held by lease\exec.rs:3169 (makepad-remote-lease-start)", taken for a start waiting on
    /// itself: it was one lease's start sent again, waiting out its first (11-26).
    fn line(&self) -> String {
        let key = self.key.as_deref().map_or(String::new(), |k| format!(" [{k}]"));
        format!("{}:{}{key} ({})", self.at.file().rsplit_once("src/").or_else(|| self.at.file().rsplit_once("src\\")).map_or(self.at.file(), |(_, f)| f), self.at.line(), self.name)
    }

    fn warn_own(&self, name: &str, holders: &[Who]) {
        if let Some(h) = holders.iter().find(|h| h.thread == self.thread) {
            eprintln!("devhub: {} waits on {} that its own thread holds since {}: it never frees", self.line(), name, h.line());
        }
    }

    fn waited_line(&self, name: &str, holders: &[Who], waited: Duration) -> String {
        let lines = if holders.is_empty() { "one gone by then".into() } else { holders.iter().map(Who::line).collect::<Vec<_>>().join(", ") };
        format!("devhub: {} waited {:.1} s on {} held by {}", self.line(), waited.as_secs_f64(), name, lines)
    }

    fn say_waited(&self, name: &str, holders: &[Who], waited: Duration) {
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), "{}", self.waited_line(name, holders, waited));
    }

    fn waited(&self, name: &str, holders: &[Who]) {
        let waited = self.since.elapsed();
        if waited >= SAY_WAIT {
            self.say_waited(name, holders, waited);
        }
    }
}

#[derive(Clone, Default)]
struct Seen {
    holders: Vec<Who>,
    waiting: Vec<Who>,
}

impl Seen {
    fn closer_waits(&self) -> bool { self.waiting.iter().any(|w| w.key.is_none()) }

    fn blockers<'a>(&'a self, w: &'a Who) -> impl Iterator<Item = &'a Who> {
        let all = w.key.is_none() || self.closer_waits();
        self.holders.iter().filter(move |h| all || h.key.is_none() || h.key == w.key || h.thread == w.thread)
    }

    fn wait(&mut self, me: &Who) -> Vec<Who> {
        self.waiting.push(me.clone());
        self.blockers(me).cloned().collect()
    }

    fn waited(&mut self, me: &Who) {
        if let Some(i) = self.waiting.iter().position(|w| w.thread == me.thread && w.since == me.since) {
            self.waiting.swap_remove(i);
        }
    }
}

/// Every live [`Held`] and [`Gate`] by name, for [`stalls`].
static ALL: Mutex<Vec<(&'static str, Weak<Mutex<Seen>>)>> = Mutex::new(Vec::new());

fn seen_of(seen: &Mutex<Seen>) -> MutexGuard<'_, Seen> {
    seen.lock().unwrap_or_else(PoisonError::into_inner)
}

fn register(name: &'static str) -> Arc<Mutex<Seen>> {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut all = ALL.lock().unwrap_or_else(PoisonError::into_inner);
    all.retain(|(_, w)| w.strong_count() > 0);
    all.push((name, Arc::downgrade(&seen)));
    seen
}

/// A mutex that knows its holder and its waiters ([`stalls`]).
pub struct Held<T> {
    name: &'static str,
    mu: Mutex<T>,
    seen: Arc<Mutex<Seen>>,
}

struct SayWait(Option<(Who, &'static str, Vec<Who>, Duration)>);

impl Drop for SayWait {
    fn drop(&mut self) {
        if let Some((who, name, holders, waited)) = self.0.take() {
            who.say_waited(name, &holders, waited);
        }
    }
}

pub struct HeldGuard<'a, T> {
    seen: &'a Mutex<Seen>,
    // Drop order is load-bearing: `HeldGuard::drop` clears `seen.holders` while `guard` still
    // holds `mu`; then `guard` drops and releases `mu`; only then does `_say` drop to format and
    // write any long wait to stderr outside both `seen` and `mu`.
    guard: MutexGuard<'a, T>,
    _say: SayWait,
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
        seen_of(self.seen).holders.clear();
    }
}

/// [`Held::try_lock`]: another holds it.
#[derive(Debug)]
pub struct WouldBlock;

pub type HeldResult<'a, T> = Result<HeldGuard<'a, T>, PoisonError<HeldGuard<'a, T>>>;

impl<T> Held<T> {
    pub fn new(name: &'static str, value: T) -> Held<T> {
        Held { name, mu: Mutex::new(value), seen: register(name) }
    }

    fn held<'a>(&'a self, guard: MutexGuard<'a, T>, me: Who) -> HeldGuard<'a, T> {
        seen_of(&self.seen).holders.push(me);
        HeldGuard { seen: &self.seen, guard, _say: SayWait(None) }
    }

    /// Like [`Mutex::lock`]; a wait is seen by [`stalls`] while it lasts and said in the hub log
    /// when it took a while (written after the guard releases the mutex, never inside the
    /// critical section).
    #[track_caller]
    pub fn lock(&self) -> HeldResult<'_, T> {
        self.lock_inner(Who::here(), SAY_WAIT)
    }

    fn lock_inner(&self, mut me: Who, say_after: Duration) -> HeldResult<'_, T> {
        match self.mu.try_lock() {
            Ok(g) => Ok(self.held(g, me)),
            Err(TryLockError::Poisoned(p)) => Err(PoisonError::new(self.held(p.into_inner(), me))),
            Err(TryLockError::WouldBlock) => {
                let holders = seen_of(&self.seen).wait(&me);
                me.warn_own(self.name, &holders);
                let got = self.mu.lock();
                let waited = me.since.elapsed();
                let say = if waited >= say_after {
                    let mut held = me.clone();
                    held.since = Instant::now();
                    let mut seen = seen_of(&self.seen);
                    seen.waited(&me);
                    seen.holders.push(held);
                    SayWait(Some((me, self.name, holders, waited)))
                } else {
                    let mut seen = seen_of(&self.seen);
                    seen.waited(&me);
                    me.since = Instant::now();
                    seen.holders.push(me);
                    SayWait(None)
                };
                match got {
                    Ok(guard) => Ok(HeldGuard { seen: &self.seen, guard, _say: say }),
                    Err(p) => Err(PoisonError::new(HeldGuard { seen: &self.seen, guard: p.into_inner(), _say: say })),
                }
            }
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
        seen_of(&self.seen).holders.first().map(|h| format!("{} for {} s", h.line(), h.since.elapsed().as_secs()))
    }
}

/// Entries with different keys share the gate. A closer takes it alone.
pub struct Gate {
    name: &'static str,
    seen: Arc<Mutex<Seen>>,
    changed: Condvar,
}

#[must_use]
pub struct GateEntry<'a> {
    gate: &'a Gate,
    key: Option<Arc<str>>,
    // The recorded holder is the thread that took the entry.
    thread: std::marker::PhantomData<MutexGuard<'a, ()>>,
}

#[must_use]
pub struct GateClosed<'a> {
    _entry: GateEntry<'a>,
}

impl Gate {
    pub fn new(name: &'static str) -> Gate {
        Gate { name, seen: register(name), changed: Condvar::new() }
    }

    /// Waits out closers and another entry of this key.
    #[track_caller]
    pub fn enter(&self, key: &str) -> GateEntry<'_> {
        self.take(Some(Arc::from(key)))
    }

    /// Waits for every holder. New entries wait until all closers are done.
    #[track_caller]
    pub fn close(&self) -> GateClosed<'_> {
        GateClosed { _entry: self.take(None) }
    }

    #[track_caller]
    fn take(&self, key: Option<Arc<str>>) -> GateEntry<'_> {
        let mut me = Who::here();
        me.key.clone_from(&key);
        let blocked = |s: &Seen| match &key {
            None => !s.holders.is_empty(),
            Some(_) => s.closer_waits() || s.holders.iter().any(|h| h.key.is_none() || h.key == key),
        };
        let mut seen = seen_of(&self.seen);
        if blocked(&seen) {
            let holders = seen.wait(&me);
            drop(seen);
            me.warn_own(self.name, &holders);
            seen = self.changed.wait_while(seen_of(&self.seen), |s| blocked(s)).unwrap_or_else(PoisonError::into_inner);
            seen.waited(&me);
            seen.holders.push(me.clone());
            drop(seen);
            me.waited(self.name, &holders);
        } else {
            seen.holders.push(me);
        }
        GateEntry { gate: self, key, thread: std::marker::PhantomData }
    }

    /// No holder and no closer waiting.
    pub fn is_idle(&self) -> bool {
        let seen = seen_of(&self.seen);
        seen.holders.is_empty() && !seen.closer_waits()
    }
}

impl Drop for GateEntry<'_> {
    fn drop(&mut self) {
        let mut seen = seen_of(&self.gate.seen);
        let i = seen.holders.iter().position(|h| h.key == self.key).expect("entry in the gate");
        seen.holders.remove(i);
        drop(seen);
        self.gate.changed.notify_all();
    }
}

/// The waits in this process now, one line each: `<waiter> waits on <lock> held by <holder>`,
/// and what the holder itself waits on (its own thread, or the waiter's: a deadlock). A line's
/// text before ` (for` stays the same while the wait does.
pub fn stalls() -> Vec<String> {
    waits().into_iter().map(|w| w.line).collect()
}

/// The waits of [`stalls`] that never free by themselves, waiting at least `at_least` each: a
/// thread that waits on a lock it holds itself, or whose holder waits on a lock that it, or the
/// waiter, holds. A lease start that ended its lease under the start lock took that lock again
/// on its thread, and two pool boxes' hubs answered no lease start or end, nor their own
/// self-update or TERM, until each was stopped by hand 26 min later (11-18).
pub fn deadlocks(at_least: Duration) -> Vec<String> {
    waits().into_iter().filter(|w| w.deadlock && w.waited >= at_least).map(|w| w.line).collect()
}

/// A deadlock of `now` that `before` (an earlier look's [`deadlocks`]) named too: the same wait
/// on both looks, not a view of locks that moved while it was taken.
pub fn lasting<'a>(before: &[String], now: &'a [String]) -> Option<&'a String> {
    now.iter().find(|l| before.iter().any(|b| stall_key(b) == stall_key(l)))
}

struct Wait {
    line: String,
    deadlock: bool,
    waited: Duration,
}

fn waits() -> Vec<Wait> {
    let all: Vec<(&'static str, Arc<Mutex<Seen>>)> = ALL.lock().unwrap_or_else(PoisonError::into_inner).iter().filter_map(|(n, w)| Some((*n, w.upgrade()?))).collect();
    let view: Vec<_> = all.iter().map(|(n, s)| (*n, seen_of(s).clone())).collect();
    let mut out = Vec::new();
    for (name, seen) in &view {
        for w in &seen.waiting {
            let mut line = format!("{} waits on {name}", w.line());
            let mut deadlock = false;
            if let Some(h) = seen.holders.iter().find(|h| h.thread == w.thread) {
                line.push_str(&format!(" that its own thread holds ({}): a deadlock", h.line()));
                deadlock = true;
            } else {
                let mut blockers = seen.blockers(w).peekable();
                if blockers.peek().is_none() { line.push_str(" (freed just now)"); }
                for (i, h) in blockers.enumerate() {
                    line.push_str(if i == 0 { " held by " } else { ", " });
                    line.push_str(&h.line());
                    // (a gate's entry of the same key: asked again, it waits that one out)
                    if h.key.is_some() && h.key == w.key { line.push_str(": the same key entered again, waiting that entry out"); }
                    // what this holder waits on, if anything
                    if let Some((other, s, waiter)) = view.iter().find_map(|(n, s)| s.waiting.iter().find(|x| x.thread == h.thread).map(|x| (n, s, x))) {
                        line.push_str(&format!(", which waits on {other}"));
                        for (j, oh) in s.blockers(waiter).enumerate() {
                            line.push_str(if j == 0 { " held by " } else { ", " });
                            line.push_str(&oh.line());
                            if oh.thread == h.thread || oh.thread == w.thread {
                                line.push_str(": a deadlock");
                                deadlock = true;
                            }
                        }
                    }
                }
            }
            let waited = w.since.elapsed();
            line.push_str(&format!(" (for {} s)", waited.as_secs()));
            out.push(Wait { line, deadlock, waited });
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
    fn a_contended_lock_registers_holder_before_returning_and_defers_wait_log() {
        let a = Arc::new(Held::new("held-test-defer", 0u32));
        let g = a.lock().unwrap();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let b = a.clone();
        let waiter = std::thread::Builder::new().name("held-defer-waiter".into()).spawn(move || {
            let mut g2 = b.lock_inner(Who::here(), Duration::ZERO).unwrap();
            let deferred = g2._say.0.take();
            let (who, name, holders, waited) = deferred.expect("long wait is deferred to drop, never printed inside lock");
            assert_eq!(name, "held-test-defer");
            let line = who.waited_line(name, &holders, waited);
            assert!(line.contains("(held-defer-waiter) waited ") && line.contains("on held-test-defer held by held.rs:"), "{line}");
            assert!(b.holder().is_some_and(|h| h.contains("(held-defer-waiter)")), "holder registered atomically before return: {:?}", b.holder());
            assert!(!stalls().iter().any(|l| l.contains("held-test-defer")), "waiter cleared atomically on acquire");
            *g2 += 1;
            locked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(g2);
            assert!(b.holder().is_none(), "holder cleared on drop");
        }).unwrap();
        let limit = Duration::from_secs(10);
        let end = Instant::now() + limit;
        while !stalls().iter().any(|l| l.contains("(held-defer-waiter) waits on held-test-defer")) {
            assert!(Instant::now() < end, "waiter appears in stalls");
            std::thread::yield_now();
        }
        drop(g);
        locked_rx.recv_timeout(limit).expect("waiter acquires after holder drops");
        assert!(a.holder().is_some_and(|h| h.contains("(held-defer-waiter)")), "outside caller sees new holder: {:?}", a.holder());
        release_tx.send(()).unwrap();
        waiter.join().unwrap();
        assert_eq!(*a.lock().unwrap(), 1);
        assert!(a.holder().is_none() && !stalls().iter().any(|l| l.contains("held-test-defer")));
    }

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
        let found = deadlocks(Duration::ZERO).into_iter().find(|l| l.contains("held-test-c"));
        assert_eq!(found.as_deref().map(stall_key), Some(stall_key(&line)), "a deadlock of its own");
        assert!(!deadlocks(Duration::from_secs(60)).iter().any(|l| l.contains("held-test-c")), "a deadlock is one that lasts: not one this new");
        seen_of(&c.seen).waiting.clear();

        // two threads, each waiting on the lock the other holds
        let other = std::thread::Builder::new().name("held-other".into()).spawn(Who::here).unwrap().join().unwrap();
        let (d, e) = (Held::new("held-test-d", ()), Held::new("held-test-e", ()));
        seen_of(&d.seen).holders.push(Who::here());
        seen_of(&d.seen).waiting.push(other.clone());
        seen_of(&e.seen).holders.push(other);
        seen_of(&e.seen).waiting.push(Who::here());
        let both = deadlocks(Duration::ZERO);
        let line = both.iter().find(|l| l.contains("(held-other) waits on held-test-d")).unwrap();
        assert!(line.contains("which waits on held-test-e held by") && line.contains(": a deadlock"), "{line}");
        assert!(lasting(&[], &both).is_none(), "one look is not lasting");
        let later: Vec<String> = both.iter().map(|l| l.replace("(for 0 s)", "(for 20 s)")).collect();
        assert!(lasting(&both, &later).is_some_and(|l| l.contains("held-test-")), "the same waits on the next look last");
        assert!(lasting(&[format!("{} (for 0 s)", "x waits on y that its own thread holds (z): a deadlock")], &later).is_none(), "another wait does not");
        for h in [&d, &e] {
            *seen_of(&h.seen) = Seen::default();
        }
        assert!(!deadlocks(Duration::ZERO).iter().any(|l| l.contains("held-test-")), "and none after");

        let gate = Arc::new(Gate::new("held-test-gate"));
        let a = gate.enter("a");
        seen_of(&gate.seen).waiting.push(Who::here());
        let own = deadlocks(Duration::ZERO).into_iter().find(|l| l.contains("held-test-gate")).expect("a closer on its entry's thread deadlocks");
        assert!(own.contains("that its own thread holds (held.rs:") && own.contains(": a deadlock"), "{own}");
        seen_of(&gate.seen).waiting.clear();

        let limit = Duration::from_secs(10);
        let wait_line = |name: &str| {
            let end = Instant::now() + limit;
            let what = format!("({name}) waits on held-test-gate held by held.rs:");
            while !stalls().iter().any(|l| l.contains(&what)) {
                assert!(Instant::now() < end, "the gate reports {what}");
                std::thread::yield_now();
            }
        };
        let g = gate.clone();
        let same = std::thread::Builder::new().name("same-key".into()).spawn(move || drop(g.enter("a"))).unwrap();
        wait_line("same-key");
        let again = stalls().into_iter().find(|l| l.contains("(same-key) waits on held-test-gate")).expect("its wait");
        assert!(again.contains(" [a] (same-key) waits on held-test-gate held by held.rs:") && again.contains(": the same key entered again, waiting that entry out"), "a wait on an entry of its own key says so, with the key: {again}");
        let (tx, rx) = std::sync::mpsc::channel();
        let g = gate.clone();
        let other = std::thread::spawn(move || {
            let _b = g.enter("b");
            tx.send(()).unwrap();
        });
        rx.recv_timeout(limit).expect("a different key enters while a is held");
        other.join().unwrap();

        let (tx, order) = std::sync::mpsc::channel();
        let (release, go) = std::sync::mpsc::channel();
        let (g, closed) = (gate.clone(), tx.clone());
        let closer = std::thread::Builder::new().name("closer".into()).spawn(move || {
            let _closed = g.close();
            closed.send("closed").unwrap();
            go.recv_timeout(limit).expect("the test releases the closed gate");
        }).unwrap();
        wait_line("closer");
        let g = gate.clone();
        let late = std::thread::Builder::new().name("late-key".into()).spawn(move || {
            let _c = g.enter("c");
            tx.send("c").unwrap();
        }).unwrap();
        wait_line("late-key");
        drop(a);
        assert_eq!(order.recv_timeout(limit).expect("the closer enters"), "closed", "a waiting closer goes before a new key");
        release.send(()).unwrap();
        assert_eq!(order.recv_timeout(limit).expect("the new key enters once the closer drops"), "c");
        for t in [same, closer, late] { t.join().unwrap(); }
        assert!(gate.is_idle() && !stalls().iter().any(|l| l.contains("held-test-gate")), "the gate has no holder or wait left");
    }
}
