use super::*;
use std::sync::{mpsc::{self, Sender}, Arc};

fn budget(cores: usize) -> Arc<Budget> {
    Arc::new(Budget::new(cores, cores))
}
/// Blocks until the table shows `predicate`, looking again at each of its changes (a state that
/// never comes holds the test: a hung test).
fn until(b: &Budget, predicate: impl Fn(&State) -> bool) {
    let mut s = b.lock();
    s.watchers += 1;
    while !predicate(&s) {
        s = b.changed.wait(s).unwrap();
    }
    s.watchers -= 1;
}
fn worker(b: &Arc<Budget>, class: u8, client: u8, id: u32, entered: &Sender<u32>) -> (Sender<()>, thread::JoinHandle<()>) {
    let b = b.clone();
    let entered = entered.clone();
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let permit = b.admit(class, client, "worker");
        entered.send(id).unwrap();
        rx.recv().unwrap();
        b.release(permit);
    });
    (tx, handle)
}
fn finish(worker: (Sender<()>, thread::JoinHandle<()>)) {
    worker.0.send(()).unwrap();
    worker.1.join().unwrap();
}

#[test]
fn permits_and_admissions_belong_to_the_budget_that_issued_them() {
    let a = BudgetHandle::of_cores(2);
    let b = BudgetHandle::of_cores(2);
    assert_ne!(a.id(), b.id(), "separate live budgets have different IDs");
    assert_eq!(a.clone().id(), a.id(), "a clone identifies the same budget");
    assert_eq!(BudgetHandle::process().id(), 0, "zero identifies the process budget");

    let a_first = a.admit(INTERACTIVE, APP, "first");
    let b_first = b.admit(INTERACTIVE, APP, "first");
    assert_eq!((a_first, b_first), (0, 0), "each budget issues row zero first");
    a.release(a_first);
    assert_eq!((a.tokens().running, b.tokens().running), (0, 1), "releasing a's row leaves b's held");
    b.release(b_first);

    // Each budget has issued one ticket. On two cores one Background job fills the Visible limit.
    let a_held = a.admit(BACKGROUND, APP, "held");
    let b_held = b.admit(BACKGROUND, APP, "held");
    let mut a_wait = a.admission();
    let mut b_wait = b.admission();
    assert_eq!(a_wait.poll(VISIBLE, APP, "waiting"), None, "a's Visible job waits at its limit");
    assert_eq!(b_wait.poll(VISIBLE, APP, "waiting"), None, "b's Visible job waits at its limit");
    drop(a_wait);
    assert!(a.tokens().waiting.is_empty(), "dropping a's admission removes its waiter");
    assert_eq!(b.tokens().waiting.len(), 1, "b keeps its waiter with the same ticket");
    assert!(b.should_yield(b_held), "b's Visible waiter needs its Background holder to yield");
    assert!(!a.should_yield(a_held), "b's waiter leaves a's holder running");
    assert_eq!(a.admission().checkpoint(a_held, 0, ""), Some(a_held), "a's admission checkpoint keeps its row");
    let a_next = a.checkpoint(a_held, 0, "");
    assert_eq!(a_next, a_held, "a keeps its row at a worker checkpoint");

    b.release(b_held);
    let b_next = b_wait.poll(VISIBLE, APP, "waiting").expect("b admits its waiter on the first poll after release");
    b.release(b_next);
    a.release(a_next);
    assert_eq!((a.tokens().running, b.tokens().running), (0, 0), "both budgets finish with every token released");
}

#[test]
fn highest_class_first_across_clients() {
    let b = budget(1);
    let p = b.admit(INTERACTIVE, APP, "blocker");
    let (tx, rx) = mpsc::channel();
    let bg = worker(&b, BACKGROUND, HAL, 2, &tx);
    until(&b, |s| s.waiters.len() == 1);
    let vis = worker(&b, VISIBLE, APP, 1, &tx);
    until(&b, |s| s.waiters.len() == 2);
    let interactive = worker(&b, INTERACTIVE, HAL, 0, &tx);
    until(&b, |s| s.waiters.len() == 3);
    b.release(p);
    assert_eq!(rx.recv().unwrap(), 0);
    finish(interactive);
    assert_eq!(rx.recv().unwrap(), 1);
    finish(vis);
    assert_eq!(rx.recv().unwrap(), 2);
    finish(bg);
}

#[test]
fn fifo_within_class_across_clients() {
    let b = budget(1);
    let p = b.admit(INTERACTIVE, APP, "blocker");
    let (tx, rx) = mpsc::channel();
    let workers: Vec<_> = [HAL, APP].into_iter().enumerate().map(|(i, client)| {
        let w = worker(&b, VISIBLE, client, i as u32, &tx);
        until(&b, |s| s.waiters.len() == i + 1);
        w
    }).collect();
    b.release(p);
    for (i, w) in workers.into_iter().enumerate() {
        assert_eq!(rx.recv().unwrap(), i as u32);
        finish(w);
    }
}

#[test]
fn interactive_stops_at_cores() {
    let b = budget(4);
    let mut held: Vec<_> = (0..4).map(|_| b.admit(INTERACTIVE, APP, "held")).collect();
    let (tx, rx) = mpsc::channel();
    let w = worker(&b, INTERACTIVE, HAL, 1, &tx);
    until(&b, |s| s.waiters.len() == 1);
    assert!(rx.try_recv().is_err());
    assert_eq!(b.state.lock().unwrap().running, 4);
    assert!(!b.should_yield(held[0]));
    b.release(held.pop().unwrap());
    assert_eq!(rx.recv().unwrap(), 1);
    finish(w);
    for p in held { b.release(p); }
}

/// Background's part leaves Visible two of the limit's 3 tokens, and Visible's limit leaves
/// Interactive the kept core. (Polled admissions: a broken rule fails an assertion, it does not hold
/// the test.)
#[test]
fn background_reserves_two_visible_slots_and_visible_reserves_one() {
    let b = budget(4);
    let handle = BudgetHandle(Which::Own(b.clone()));
    let held = b.admit(BACKGROUND, APP, "held");
    let mut background = handle.admission();
    assert_eq!(background.poll(BACKGROUND, HAL, "waits"), None, "Background stops at its part of one");
    let v1 = handle.admission().poll(VISIBLE, HAL, "visible one").expect("Visible has the two Background leaves");
    let v2 = handle.admission().poll(VISIBLE, HAL, "visible two").expect("Visible has the two Background leaves");
    let mut visible = handle.admission();
    assert_eq!(visible.poll(VISIBLE, HAL, "visible three"), None, "Visible stops at its limit of 3");
    let interactive = handle.admission().poll(INTERACTIVE, APP, "interactive reserve").expect("Interactive has the kept core");
    assert_eq!(b.lock().running, 4);
    b.release(interactive);
    assert_eq!(visible.poll(VISIBLE, HAL, "visible three"), None, "the kept core is not Visible's");
    b.release(v2);
    let v3 = visible.poll(VISIBLE, HAL, "visible three").expect("a token within Visible's limit");
    for p in [v1, v3] { b.release(p); }
    assert_eq!(background.poll(BACKGROUND, HAL, "waits"), None, "idle tokens are not Background's beyond its part");
    b.release(held);
    let bg = background.poll(BACKGROUND, HAL, "waits").expect("its part has room");
    b.release(bg);
    assert_eq!(b.lock().running, 0);
}

#[test]
fn yield_only_to_strictly_higher_waiting_class() {
    let b = budget(1);
    let p = b.admit(VISIBLE, APP, "visible");
    let (tx, rx) = mpsc::channel();
    let equal = worker(&b, VISIBLE, HAL, 1, &tx);
    until(&b, |s| s.waiters.len() == 1);
    assert!(!b.should_yield(p));
    let higher = worker(&b, INTERACTIVE, HAL, 0, &tx);
    until(&b, |s| s.waiters.len() == 2);
    assert!(b.should_yield(p));
    b.release(p);
    assert_eq!(rx.recv().unwrap(), 0);
    finish(higher);
    assert_eq!(rx.recv().unwrap(), 1);
    finish(equal);
}

#[test]
fn checkpoint_parks_and_retains_original_fifo_ticket() {
    let b = budget(1);
    let (tx, rx) = mpsc::channel();
    let (checkpoint_tx, checkpoint_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let original = {
        let b = b.clone();
        let tx = tx.clone();
        thread::spawn(move || {
            let p = b.admit(BACKGROUND, APP, "original");
            tx.send(0).unwrap();
            checkpoint_rx.recv().unwrap();
            let p = b.checkpoint(p, 0, "");
            tx.send(2).unwrap();
            release_rx.recv().unwrap();
            b.release(p);
        })
    };
    assert_eq!(rx.recv().unwrap(), 0);
    let newer = worker(&b, BACKGROUND, HAL, 3, &tx);
    until(&b, |s| s.waiters.len() == 1);
    let higher = worker(&b, INTERACTIVE, HAL, 1, &tx);
    until(&b, |s| s.waiters.len() == 2);
    checkpoint_tx.send(()).unwrap();
    assert_eq!(rx.recv().unwrap(), 1);
    until(&b, |s| s.waiters.len() == 2 && s.parked[BACKGROUND as usize] == 1);
    finish(higher);
    assert_eq!(rx.recv().unwrap(), 2);
    release_tx.send(()).unwrap();
    original.join().unwrap();
    assert_eq!(rx.recv().unwrap(), 3);
    finish(newer);
    let s = b.state.lock().unwrap();
    assert_eq!(s.parked, [0, 0, 1]);
    assert_eq!(s.admitted, [1, 0, 3]);
}

#[test]
fn checkpoint_under_lock_keeps_permit_and_reports_frequent_sites() {
    let b = budget(1);
    let p = b.admit(BACKGROUND, APP, "locked");
    let (tx, rx) = mpsc::channel();
    let higher = worker(&b, INTERACTIVE, HAL, 1, &tx);
    until(&b, |s| s.waiters.len() == 1);
    for i in 0..10 {
        for _ in 0..=i { assert_eq!(b.checkpoint(p, 1, &format!("lock:{i}")), p); }
    }
    assert_eq!(b.state.lock().unwrap().parked, [0; 3]);
    let text = b.report();
    assert!(text.contains("checkpoint under lock: lock:9 x10"));
    assert!(!text.contains("checkpoint under lock: lock:0 x1\n"));
    assert!(!text.contains("checkpoint under lock: lock:1 x2\n"));
    assert_eq!(text.matches("checkpoint under lock:").count(), 8);
    assert!(rx.try_recv().is_err());
    b.release(p);
    assert_eq!(rx.recv().unwrap(), 1);
    finish(higher);
}

/// On a machine with two kinds of core the person's work takes the performance cores and
/// Background work is placed on the others (`fast_room`): a live run's sound gapped while hot
/// builds shared a base M-series Mac's 4 performance cores (11-22).
#[test]
fn the_persons_work_takes_the_performance_cores() {
    let b = Budget::new(16, 16).with_fast(4);
    // a span of the person's work from its last change: `tokens` of it, read exactly
    let span = |b: &Budget| {
        let mut s = b.lock();
        let at = s.urgent_at;
        (s.window_at, s.window_area) = (at, s.urgent_area);
        b.roll(&mut s, at + DEMAND_SPAN_NS);
    };
    assert_eq!((b.fast_room(), b.urgent()), (4, false), "nobody's work: every performance core");
    let background: Vec<_> = (0..2).map(|_| b.admit(BACKGROUND, APP, "background")).collect();
    let tiles: Vec<_> = (0..2).map(|_| b.admit(VISIBLE, APP, "tiles")).collect();
    let mut load = vec![b.admit(INTERACTIVE, APP, "a load")];
    assert_eq!(b.state.lock().unwrap().urgent, 3, "the tiles and the load are the person's work, Background is not");
    span(&b);
    assert_eq!((b.fast_room(), b.urgent()), (1, true), "4 performance cores less the 3 the person's work takes");
    b.deadlines.fetch_add(1, Relaxed);
    assert_eq!(b.fast_room(), 0, "a thread that holds a core against a deadline takes the last");
    b.deadlines.fetch_sub(1, Relaxed);
    load.push(b.admit(INTERACTIVE, APP, "a load"));
    span(&b);
    assert_eq!(b.fast_room(), 0, "the person's work takes every performance core: Background runs on the others");
    for p in load.into_iter().chain(tiles).chain(background) { b.release(p); }
    span(&b);
    assert_eq!((b.fast_room(), b.urgent()), (0, true), "work that ended leaves the reading after two spans, not one");
    span(&b);
    assert_eq!((b.fast_room(), b.urgent()), (4, false), "and then every performance core is free again");
    assert_eq!(Budget::new(16, 16).fast_room(), 0, "one kind of core: no performance cores to place on");
    let s = b.state.lock().unwrap();
    assert_eq!((s.running, s.urgent), (0, 0));
}

#[test]
fn reclass_updates_yield_and_background_accounting() {
    let b = budget(1);
    let p = b.admit(BACKGROUND, APP, "promoted");
    let (tx, rx) = mpsc::channel();
    let w = worker(&b, VISIBLE, HAL, 1, &tx);
    until(&b, |s| s.waiters.len() == 1);
    assert!(b.should_yield(p));
    b.reclass(p, INTERACTIVE);
    assert!(!b.should_yield(p));
    assert!(matches!(b.tokens().holders.as_slice(), [Holder { class: INTERACTIVE, client: APP, .. }]),
        "the reclassed job's row carries its new class");
    assert_eq!(b.state.lock().unwrap().urgent, 1, "the promoted job counts as the person's work");
    assert_eq!(b.checkpoint(p, 0, ""), p);
    b.release(p);
    assert_eq!(rx.recv().unwrap(), 1);
    finish(w);
}

#[test]
fn a_wait_behind_an_earlier_job_on_a_strand_is_reported_apart_from_waits_for_a_worker() {
    let b = budget(6);
    b.strand_waited(VISIBLE, 19_700_000_000, "strand job");
    b.waited(VISIBLE, APP, 2_000, "strand job");
    let text = b.report();
    let first = text.lines().next().unwrap();
    assert!(!first.contains("worst:"), "the pool's worst names the strand's wait: {text}");
    assert!(text.contains("behind an earlier job on its strand: wait p50 / p95 / max  "), "{text}");
    assert!(text.contains(", worst: visible \"strand job\" waited 19.7 s\n"), "{text}");
    let hist = std::array::from_fn(|i| b.waits[VISIBLE as usize][i].load(Relaxed));
    assert_eq!(percentile(&hist, 95), 2000);
    assert_eq!(b.max_wait[VISIBLE as usize].load(Relaxed), 2_000);
    assert_eq!(b.max_strand_wait.load(Relaxed), 19_700_000_000);
}

#[test]
fn diagnostics_track_waits_culprit_recent_worst_and_utf8_labels() {
    let mut b = budget(6);
    Arc::get_mut(&mut b).unwrap().origin = monotonic_ns().wrapping_sub(61_000_000_000);
    let bg = b.admit(BACKGROUND, HAL, "background culprit");
    let vis = b.admit(VISIBLE, APP, "older visible");
    {
        let mut s = b.state.lock().unwrap();
        s.slots[vis as usize].as_mut().unwrap().checkpoint_at = 0;
    }
    b.waited(INTERACTIVE, HAL, 200_000_000, "requested");
    b.waited(INTERACTIVE, APP, 2_000, "short");
    let text = b.report();
    assert!(text.contains("worst: interactive \"requested\" waited 200.0 ms -- background \"background culprit\""));
    assert!(text.contains("no checkpoint for: background culprit"));
    let hist = std::array::from_fn(|i| b.waits[0][i].load(Relaxed));
    assert_eq!(percentile(&hist, 50), 2000);
    assert_eq!(b.max_wait[0].load(Relaxed), 200_000_000);
    // Inject timestamps to exercise a rolling minute without sleeping.
    {
        let mut s = b.state.lock().unwrap();
        s.worst.front_mut().unwrap().when = 0;
        s.worst.push_back(Worst { class: VISIBLE, label: Label::new("recent"), ns: 150_000_000, when: RECENT_NS + 1, culprit: None });
    }
    let text = b.report();
    assert!(text.contains("worst: visible \"recent\" waited 150.0 ms"));
    assert!(!text.contains("worst: interactive"));
    b.release(bg);
    b.release(vis);
    let label = Label::new(&"é".repeat(30));
    assert_eq!(label.text().len(), 46);
    assert_eq!(label.text(), "é".repeat(23));
}

/// A thread asleep lends its token and keeps its permit: a waiter runs on it, and the sleeper comes
/// back at once, whatever runs by then (it may hold a lock what runs needs), on the same permit.
#[test]
fn a_lent_token_runs_a_waiter_and_comes_back_at_once_on_the_same_permit() {
    let b = budget(2);
    let p = b.admit(BACKGROUND, APP, "sleeper");
    let (tx, rx) = mpsc::channel();
    let visible = worker(&b, VISIBLE, HAL, 1, &tx);
    until(&b, |s| s.waiters.len() == 1);
    assert!(b.should_yield(p), "the waiter wants the sleeper's token");
    assert!(b.lend());
    assert!(!b.lend(), "nothing more to lend");
    assert_eq!(rx.recv().unwrap(), 1, "the waiter was admitted on the lent token");
    assert!(b.report().contains("1 lent"), "{}", b.report());
    {
        let t = b.tokens();
        assert_eq!((t.running, t.holders.len()), (1, 2), "the permit keeps its row beside the token's new holder");
        assert!(t.holders.iter().any(|h| h.lent && h.label == "sleeper" && h.thread == me()), "who lent it");
        assert!(t.holders.iter().any(|h| !h.lent && h.label == "worker" && h.thread != me()), "who runs on it");
    }
    let interactive = worker(&b, INTERACTIVE, APP, 2, &tx);
    assert_eq!(rx.recv().unwrap(), 2, "rows are no limit: a job is admitted while another's row stays");
    b.take_back();
    assert_eq!(b.state.lock().unwrap().running, 3, "the sleeper does not wait for a token");
    assert!(b.should_yield(p), "over the cores everyone yields at the next checkpoint");
    let permit = b.checkpoint(p, 1, "under a lock");
    assert_eq!(permit, p, "with locks held it keeps running, on the permit it holds");
    finish(interactive);
    finish(visible);
    assert!(!b.should_yield(p));
    b.release(p);
    assert_eq!(b.state.lock().unwrap().running, 0);
}

/// Waits nest on a thread: a program thread in `relax_wait_enter` compiles the futex call it is about
/// to make and that build's wait lends again, and a job back from a wait holds for a pause under the
/// wait's own guard. The token stays lent until the outermost wait ends, and comes back then.
#[test]
fn a_wait_inside_a_wait_ends_with_the_outer_one_and_the_token_comes_back() {
    let permit = admit(VISIBLE, APP, "nested waits");
    let lent = || tokens().holders.iter().find(|h| h.thread == me()).unwrap().lent;
    // as BudgetWait: a take_back only for a lend that said so
    let outer = lend();
    assert!(outer && lent());
    let inner = lend();
    if inner { take_back(); }
    assert!(lent(), "the inner wait's end does not end the outer wait");
    take_back();
    assert!(!lent(), "the outer wait's end brings the token back");
    // as relax_wait_enter and relax_wait_leave: every enter has its leave
    lend();
    lend();
    take_back();
    assert!(lent(), "the inner leave does not end the outer wait");
    take_back();
    assert!(!lent(), "the outer leave brings the token back");
    assert_eq!(tokens().running, 1);
    assert!(tokens().holders.iter().any(|h| h.label == "nested waits"), "a label's spaces come through");
    release(permit);
    assert_eq!(tokens().running, 0);
}

/// A permit that names no row is a bug of whoever passed it, and its thread panics. The other threads
/// go on: their tokens don't depend on it.
#[test]
fn a_panic_under_the_budgets_lock_stays_that_threads_panic() {
    let b = budget(2);
    let p = b.admit(VISIBLE, APP, "released twice");
    b.release(p);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.release(p))).is_err());
    let next = b.admit(VISIBLE, APP, "next");
    b.release(next);
    assert_eq!(b.tokens().running, 0);
}

/// A waiter whose job is raised while it waits (a strand job a person's work queued behind) waits
/// from then on for a token of its new class, beside a Background waiter of another client that
/// waits with no room. The raised job takes the first token its new class may take.
#[test]
fn a_waiter_raised_while_parked_takes_the_first_token_its_new_class_may_take() {
    let b = budget(2);
    let held = [b.admit(INTERACTIVE, APP, "held"), b.admit(INTERACTIVE, APP, "held")];
    let (tx, rx) = mpsc::channel();
    let build = worker(&b, BACKGROUND, HAL, 1, &tx);
    until(&b, |s| s.waiters.len() == 1);
    let desired = Arc::new(AtomicU8::new(BACKGROUND));
    let (go, done) = mpsc::channel::<()>();
    let raised = {
        let (b, desired, tx) = (b.clone(), desired.clone(), tx.clone());
        thread::spawn(move || {
            let permit = b.admit_following(&desired, APP, "raised");
            tx.send(2).unwrap();
            done.recv().unwrap();
            b.release(permit);
        })
    };
    until(&b, |s| s.waiters.len() == 2);
    // as the pool's `Raised::raise`: the class, then the waiting thread woken
    desired.fetch_min(INTERACTIVE, Relaxed);
    raised.thread().unpark();
    until(&b, |s| s.waiters.iter().any(|w| w.client == APP && w.class == INTERACTIVE));
    assert!(rx.try_recv().is_err(), "every token Interactive may take is held");
    b.release(held[0]);
    let first = rx.recv_timeout(std::time::Duration::from_secs(30));
    assert_eq!(first, Ok(2), "the raised waiter takes the token that came free, not the Background waiter");
    finish((go, raised));
    b.release(held[1]);
    assert_eq!(rx.recv().unwrap(), 1, "the Background waiter runs once the last held token is released");
    finish(build);
    assert_eq!(b.state.lock().unwrap().running, 0);
}

#[test]
fn a_thread_floor_preserves_priority_and_a_refused_raise_is_not_retried() {
    thread::spawn(|| {
        set_thread_floor(true);
        assert!(apply_thread_class_with(INTERACTIVE, |_| Some(true)));
        assert!(set_thread_class(BACKGROUND));
        assert_eq!((thread_class(), applied_thread_class()), (Some(BACKGROUND), Some(INTERACTIVE)));
        assert!(!set_thread_idle(), "a floored thread refuses idle work");
        set_thread_floor(false);
        assert!(apply_thread_class_with(BACKGROUND, |_| Some(true)));
        assert_eq!(applied_thread_class(), Some(BACKGROUND), "a thread whose floor is cleared goes to Background again");
    }).join().unwrap();
    thread::spawn(|| {
        let request = |class, nice| apply_thread_class_with(class, |_| try_nice(nice, || (false, true)));
        assert!(!request(VISIBLE, 1));
        assert_eq!((thread_class(), applied_thread_class()), (Some(VISIBLE), None));
        for nice in [1, 0] {
            assert_eq!(try_nice(nice, || panic!("retry at nice {nice}")), None);
        }
        assert_eq!(try_nice(8, || (true, false)), Some(true), "a reachable nice still applies");
    }).join().unwrap();
}

/// set_thread_idle: the thread runs at SCHED_IDLE, Background work leaves it there (it reads as
/// Background), and UI protection refuses a later request to lower its policy.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn idle_thread_stays_below_background() {
    unsafe extern "C" { fn sched_getscheduler(pid: i32) -> i32; }
    let (policy, idle, class) = thread::spawn(|| {
        assert!(!thread_idle(), "a new thread is not idle");
        set_thread_class(BACKGROUND);
        assert!(set_thread_idle(), "an unprivileged thread may lower itself to SCHED_IDLE");
        set_thread_class(BACKGROUND);
        (unsafe { sched_getscheduler(0) }, thread_idle(), thread_class())
    }).join().unwrap();
    assert_eq!((policy, idle, class), (5, true, Some(BACKGROUND)), "SCHED_IDLE, kept by Background work");
    thread::spawn(|| {
        let applied = set_ui_thread();
        let policy = unsafe { sched_getscheduler(0) };
        assert!(policy >= 0, "the UI thread's scheduler policy is readable");
        // A test launched under SCHED_IDLE may not be allowed to return to SCHED_OTHER.
        // The API reports that failure; UI protection still prevents later policy changes.
        if applied { assert_eq!(policy, 0, "a successful UI promotion selects SCHED_OTHER"); }
        assert!(!set_thread_idle(), "the UI thread refuses idle work");
        assert!(!thread_idle(), "the refused request does not mark the UI thread idle");
        assert_eq!(unsafe { sched_getscheduler(0) }, policy, "the refused request preserves the UI policy");
    }).join().unwrap();
}

/// A run given fewer cores than its machine's (`MAKEPAD_CPU_CORES`) sizes the budget from them;
/// never more than the machine's, and an unset or unreadable count is the machine's.
#[test]
fn a_granted_run_sizes_the_budget_from_its_grant() {
    assert_eq!(granted_cores(16, Some("4")), 4);
    assert_eq!(granted_cores(16, Some("64")), 16);
    assert_eq!(granted_cores(16, None), 16);
    assert_eq!(granted_cores(16, Some("0")), 16);
    assert_eq!(granted_cores(16, Some("four")), 16);
    // the machine a run was told (`MAKEPAD_MACHINE_CORES`) stands for the one it sees, which a CPU
    // quota at its start narrows; an unset or unreadable count is the one it sees
    assert_eq!(machine_from(Some("16"), || 10), 16, "a run under a 10-core quota sizes from its 16-core machine");
    assert_eq!(machine_from(None, || 10), 10, "unset: the cores it sees");
    assert_eq!((machine_from(Some("0"), || 10), machine_from(Some("all"), || 10)), (10, 10), "unreadable: the cores it sees");
}
