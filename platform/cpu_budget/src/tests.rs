use super::*;
use std::sync::{mpsc::{self, Receiver, Sender}, Arc};

fn budget(cores: usize) -> (Arc<Budget>, Receiver<()>) {
    let budget = Arc::new(Budget::new(cores, cores));
    let (tx, rx) = mpsc::channel();
    budget.state.lock().unwrap().changed = Some(tx);
    (budget, rx)
}
fn until(b: &Budget, changes: &Receiver<()>, predicate: impl Fn(&State) -> bool) {
    loop {
        if predicate(&b.state.lock().unwrap()) { return; }
        // (nothing changes for this long: the state waited for will not come, a plain failure)
        changes.recv_timeout(std::time::Duration::from_secs(10)).expect("the budget stopped changing before the state this test waits for");
    }
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
fn highest_class_first_across_clients() {
    let (b, changed) = budget(1);
    let p = b.admit(INTERACTIVE, APP, "blocker");
    let (tx, rx) = mpsc::channel();
    let bg = worker(&b, BACKGROUND, HAL, 2, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
    let vis = worker(&b, VISIBLE, APP, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 2);
    let interactive = worker(&b, INTERACTIVE, HAL, 0, &tx);
    until(&b, &changed, |s| s.waiters.len() == 3);
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
    let (b, changed) = budget(1);
    let p = b.admit(INTERACTIVE, APP, "blocker");
    // (with a JIT job running the JIT's next waits for a token as the others do)
    let jit = b.admit(BACKGROUND, JIT, "running");
    let (tx, rx) = mpsc::channel();
    let workers: Vec<_> = (0..3).map(|i| {
        let w = worker(&b, VISIBLE, 2 - i as u8, i, &tx);
        until(&b, &changed, |s| s.waiters.len() == i as usize + 1);
        w
    }).collect();
    b.release(p);
    for (i, w) in workers.into_iter().enumerate() {
        assert_eq!(rx.recv().unwrap(), i as u32);
        finish(w);
    }
    b.release(jit);
}

#[test]
fn interactive_stops_at_cores() {
    let (b, changed) = budget(4);
    let mut held: Vec<_> = (0..4).map(|_| b.admit(INTERACTIVE, APP, "held")).collect();
    let (tx, rx) = mpsc::channel();
    let w = worker(&b, INTERACTIVE, HAL, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
    assert!(rx.try_recv().is_err());
    assert_eq!(b.state.lock().unwrap().running, 4);
    assert!(!b.should_yield(held[0]));
    b.release(held.pop().unwrap());
    assert_eq!(rx.recv().unwrap(), 1);
    finish(w);
    for p in held { b.release(p); }
}

#[test]
fn background_reserves_two_visible_slots_and_visible_reserves_one() {
    let (b, changed) = budget(4);
    let held: Vec<_> = (0..1).map(|_| b.admit(BACKGROUND, APP, "held")).collect();
    let (tx, rx) = mpsc::channel();
    let bg = worker(&b, BACKGROUND, HAL, 2, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
    let v1 = b.admit(VISIBLE, HAL, "visible one");
    let v2 = b.admit(VISIBLE, HAL, "visible two");
    let visible = worker(&b, VISIBLE, HAL, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 2);
    assert!(rx.try_recv().is_err());
    let interactive = b.admit(INTERACTIVE, APP, "interactive reserve");
    assert_eq!(b.state.lock().unwrap().running, 4);
    b.release(interactive);
    assert!(rx.try_recv().is_err());
    b.release(v2);
    assert_eq!(rx.recv().unwrap(), 1);
    finish(visible);
    b.release(v1);
    b.release(held[0]);
    assert_eq!(rx.recv().unwrap(), 2);
    finish(bg);
    for p in &held[1..] { b.release(*p); }
}

#[test]
fn visible_leaves_waiting_jit_background_its_floor() {
    let (b, changed) = budget(16);
    let held: Vec<_> = (0..15).map(|_| b.admit(VISIBLE, APP, "slice")).collect();
    assert!(!b.should_yield(held[0]), "nobody waits");
    let above = b.admit(BACKGROUND, JIT, "tier-up");
    assert!(!b.should_yield(held[0]), "the JIT's first job needs no token");
    let (tx, rx) = mpsc::channel();
    let visible = worker(&b, VISIBLE, APP, 9, &tx);
    let second = worker(&b, BACKGROUND, JIT, 2, &tx);
    until(&b, &changed, |s| s.waiters.len() == 2);
    assert!(b.should_yield(held[0]), "Visible gives way at its next checkpoint");
    b.release(held[0]);
    assert_eq!(rx.recv().unwrap(), 2, "ahead of the Visible job that came first");
    let third = worker(&b, BACKGROUND, JIT, 3, &tx);
    until(&b, &changed, |s| s.waiters.len() == 2);
    assert!(b.should_yield(held[1]), "one permit left to give");
    b.release(held[1]);
    assert_eq!(rx.recv().unwrap(), 3);
    let fourth = worker(&b, BACKGROUND, JIT, 4, &tx);
    until(&b, &changed, |s| s.waiters.len() == 2);
    assert!(!b.should_yield(held[2]), "the floor is met");
    b.release(held[2]);
    assert_eq!(rx.recv().unwrap(), 9, "past the floor Visible comes first again");
    for w in [visible, second, third] { finish(w); }
    for p in &held[3..] { b.release(*p); }
    b.release(above);
    assert_eq!(rx.recv().unwrap(), 4);
    finish(fourth);
}

#[test]
fn visible_is_asked_to_leave_only_when_the_jit_found_no_permit() {
    let (b, _) = budget(16);
    let held: Vec<_> = (0..12).map(|_| b.admit(VISIBLE, APP, "slice")).collect();
    let jit: Vec<_> = (0..3).map(|_| b.admit(BACKGROUND, JIT, "tier-up")).collect();
    for p in held.iter().chain(&jit) { assert!(!b.should_yield(*p), "the JIT found its permits free: nobody leaves"); }
    for p in held.into_iter().chain(jit) { b.release(p); }
}

#[test]
fn the_jit_floor_holds_only_against_visible_work() {
    let (b, changed) = budget(16);
    let held: Vec<_> = (0..13).map(|_| b.admit(BACKGROUND, APP, "estimate")).collect();
    let above = b.admit(BACKGROUND, JIT, "tier-up");
    let (tx, rx) = mpsc::channel();
    let jit = worker(&b, BACKGROUND, JIT, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
    assert!(rx.try_recv().is_err(), "Background's ceiling holds");
    let v = b.admit(VISIBLE, APP, "first of the slice");
    assert_eq!(rx.recv().unwrap(), 1);
    b.release(v);
    finish(jit);
    b.release(above);
    for p in held { b.release(p); }
}

#[test]
fn a_small_machine_keeps_visible_ahead_of_the_jit() {
    let (b, changed) = budget(4);
    let held: Vec<_> = (0..3).map(|_| b.admit(VISIBLE, APP, "slice")).collect();
    let above = b.admit(BACKGROUND, JIT, "tier-up");
    let (tx, rx) = mpsc::channel();
    let jit = worker(&b, BACKGROUND, JIT, 1, &tx);
    let visible = worker(&b, VISIBLE, APP, 2, &tx);
    until(&b, &changed, |s| s.waiters.len() == 2);
    assert!(!b.should_yield(held[0]));
    b.release(held[0]);
    assert_eq!(rx.recv().unwrap(), 2);
    finish(visible);
    for p in &held[1..] { b.release(*p); }
    b.release(above);
    assert_eq!(rx.recv().unwrap(), 1);
    finish(jit);
}

#[test]
fn warmup_limits_app_background_once_and_leaves_room_for_jit() {
    let (b, changed) = budget(6);
    let early = b.admit(BACKGROUND, APP, "before warm-up");
    let p = b.admit(BACKGROUND, APP, "warm seed");
    let jit = b.admit(BACKGROUND, JIT, "warm compiler");
    b.begin_warmup();
    assert!(b.should_yield(early) && b.should_yield(p), "the app's background past one yields while warming");
    assert!(!b.should_yield(jit), "the JIT's background does not");
    b.release(early);
    assert!(!b.should_yield(p));
    let (tx, rx) = mpsc::channel();
    let w = worker(&b, BACKGROUND, APP, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
    assert!(rx.try_recv().is_err());
    assert!(!b.should_yield(p));
    let v = b.admit(VISIBLE, HAL, "visible during warmup");
    b.release(v);
    b.end_warmup();
    assert_eq!(rx.recv().unwrap(), 1);
    b.begin_warmup();
    assert!(!b.warming(), "a later compile burst cannot restart warm-up");
    assert!(!b.should_yield(p) && !b.should_yield(jit));
    b.release(jit);
    let full = b.admit(BACKGROUND, APP, "restored third slot");
    assert_eq!(b.state.lock().unwrap().background, 3);
    b.release(full);
    finish(w);
    assert!(!b.should_yield(p));
    b.release(p);
}

#[test]
fn yield_only_to_strictly_higher_waiting_class() {
    let (b, changed) = budget(1);
    let p = b.admit(VISIBLE, APP, "visible");
    let (tx, rx) = mpsc::channel();
    let equal = worker(&b, VISIBLE, HAL, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
    assert!(!b.should_yield(p));
    let higher = worker(&b, INTERACTIVE, HAL, 0, &tx);
    until(&b, &changed, |s| s.waiters.len() == 2);
    assert!(b.should_yield(p));
    b.release(p);
    assert_eq!(rx.recv().unwrap(), 0);
    finish(higher);
    assert_eq!(rx.recv().unwrap(), 1);
    finish(equal);
}

#[test]
fn checkpoint_parks_and_retains_original_fifo_ticket() {
    let (b, changed) = budget(1);
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
    until(&b, &changed, |s| s.waiters.len() == 1);
    let higher = worker(&b, INTERACTIVE, HAL, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 2);
    checkpoint_tx.send(()).unwrap();
    assert_eq!(rx.recv().unwrap(), 1);
    until(&b, &changed, |s| s.waiters.len() == 2 && s.parked[BACKGROUND as usize] == 1);
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
    let (b, changed) = budget(1);
    let p = b.admit(BACKGROUND, APP, "locked");
    let (tx, rx) = mpsc::channel();
    let higher = worker(&b, INTERACTIVE, HAL, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
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

#[test]
fn reclass_updates_yield_and_background_accounting() {
    let (b, changed) = budget(1);
    let p = b.admit(BACKGROUND, APP, "promoted");
    let (tx, rx) = mpsc::channel();
    let w = worker(&b, VISIBLE, HAL, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
    assert!(b.should_yield(p));
    b.reclass(p, INTERACTIVE);
    assert!(!b.should_yield(p));
    assert_eq!(b.state.lock().unwrap().background, 0);
    assert_eq!(b.checkpoint(p, 0, ""), p);
    b.release(p);
    assert_eq!(rx.recv().unwrap(), 1);
    finish(w);
}

#[test]
fn diagnostics_track_waits_culprit_recent_worst_and_utf8_labels() {
    let (mut b, _) = budget(6);
    Arc::get_mut(&mut b).unwrap().origin = Instant::now() - std::time::Duration::from_secs(61);
    let bg = b.admit(BACKGROUND, HAL, "background culprit");
    let vis = b.admit(VISIBLE, APP, "older visible");
    {
        let mut s = b.state.lock().unwrap();
        s.slots[vis as usize].as_mut().unwrap().checkpoint_at = 0;
    }
    b.waited(INTERACTIVE, JIT, 200_000_000, "requested");
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
    let (b, changed) = budget(2);
    let p = b.admit(BACKGROUND, APP, "sleeper");
    let (tx, rx) = mpsc::channel();
    let visible = worker(&b, VISIBLE, HAL, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
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
    let (b, _changed) = budget(2);
    let p = b.admit(VISIBLE, APP, "released twice");
    b.release(p);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.release(p))).is_err());
    let next = b.admit(VISIBLE, APP, "next");
    b.release(next);
    assert_eq!(b.tokens().running, 0);
}

/// The owner waits for a build for a compile job it serves: that job's token goes to the build, on
/// a one-token machine too, and comes back on the same permit without waiting.
#[test]
fn a_job_served_on_the_owner_lends_its_token_by_permit() {
    let (b, changed) = budget(1);
    let served = b.admit(BACKGROUND, JIT, "served");
    let (tx, rx) = mpsc::channel();
    let build = worker(&b, BACKGROUND, JIT, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
    b.lend_permit(served);
    assert_eq!(rx.recv().unwrap(), 1, "the build runs on the job's token");
    b.take_back_permit(served);
    assert_eq!(b.state.lock().unwrap().running, 2, "the job does not wait for a token");
    finish(build);
    b.release(served);
    assert_eq!(b.state.lock().unwrap().running, 0);
}

/// The app's jobs fill the budget and run on: one JIT job runs beside them, holding no token, and
/// keeps running whoever waits; a second waits for it. Nothing is held back while none runs.
#[test]
fn one_jit_job_runs_above_a_full_budget() {
    let (b, changed) = budget(4);
    let app: Vec<_> = (0..3).map(|_| b.admit(VISIBLE, APP, "busy")).collect();
    let above = b.admit(BACKGROUND, JIT, "tier-up");
    assert_eq!(b.state.lock().unwrap().running, 3, "the job takes no token from the app");
    assert!(b.report().contains("1 JIT job above"), "{}", b.report());
    assert!(b.tokens().holders.iter().any(|h| h.above && h.label == "tier-up"));
    let (tx, rx) = mpsc::channel();
    let second = worker(&b, BACKGROUND, JIT, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
    let more_app = worker(&b, VISIBLE, APP, 2, &tx);
    until(&b, &changed, |s| s.waiters.len() == 2);
    assert!(!b.should_yield(above), "it holds no token anyone could want");
    assert_eq!(b.checkpoint(above, 0, ""), above);
    b.release(above);
    assert_eq!(rx.recv().unwrap(), 1, "the next takes its place, not the app's waiter's");
    assert_eq!(b.state.lock().unwrap().running, 3);
    finish(second);
    b.release(app[0]);
    assert_eq!(rx.recv().unwrap(), 2, "the app's own token goes to the app's waiter");
    finish(more_app);
    for p in &app[1..] { b.release(*p); }
    assert_eq!(b.state.lock().unwrap().running, 0);
}

/// The JIT's only job yields its token to the app and goes on above the budget: its checkpoint
/// passes the token on, it does not stop the JIT.
#[test]
fn the_only_jit_job_gives_up_its_token_and_goes_on_above_the_budget() {
    let (b, changed) = budget(3);
    let job = b.admit(BACKGROUND, JIT, "tier-up");
    let app = b.admit(VISIBLE, APP, "busy");
    let (tx, rx) = mpsc::channel();
    let waiter = worker(&b, VISIBLE, APP, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
    assert!(b.should_yield(job));
    let job = b.checkpoint(job, 0, "");
    assert_eq!(rx.recv().unwrap(), 1, "the app's waiter has its token");
    assert_eq!(b.state.lock().unwrap().running, 2);
    assert!(b.tokens().holders.iter().any(|h| h.above && h.label == "tier-up"));
    assert!(!b.should_yield(job));
    b.release(job);
    assert_eq!(b.state.lock().unwrap().running, 2);
    finish(waiter);
    b.release(app);
    assert_eq!(b.state.lock().unwrap().jit_running, 0);
}

/// One JIT job running keeps the others to the budget's limits.
#[test]
fn a_second_jit_job_waits_for_a_token() {
    let (b, changed) = budget(3);
    let running = b.admit(BACKGROUND, JIT, "running");
    let (tx, rx) = mpsc::channel();
    let second = worker(&b, BACKGROUND, JIT, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
    assert!(rx.try_recv().is_err(), "Background is at its limit and the JIT has a job running");
    b.release(running);
    assert_eq!(rx.recv().unwrap(), 1);
    finish(second);
}
