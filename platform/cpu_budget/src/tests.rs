use super::*;
use std::sync::{mpsc::{self, Receiver, Sender}, Arc};

fn budget(cores: usize) -> (Arc<Budget>, Receiver<()>) {
    let budget = Arc::new(Budget::new(cores));
    let (tx, rx) = mpsc::channel();
    budget.state.lock().unwrap().changed = Some(tx);
    (budget, rx)
}
fn until(b: &Budget, changes: &Receiver<()>, predicate: impl Fn(&State) -> bool) {
    loop {
        if predicate(&b.state.lock().unwrap()) { return; }
        changes.recv().unwrap();
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
    let interactive = worker(&b, INTERACTIVE, JIT, 0, &tx);
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
}

#[test]
fn interactive_stops_at_cores() {
    let (b, changed) = budget(4);
    let mut held: Vec<_> = (0..4).map(|_| b.admit(INTERACTIVE, APP, "held")).collect();
    let (tx, rx) = mpsc::channel();
    let w = worker(&b, INTERACTIVE, JIT, 1, &tx);
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
    let (b, changed) = budget(6);
    let held: Vec<_> = (0..3).map(|_| b.admit(BACKGROUND, APP, "held")).collect();
    let (tx, rx) = mpsc::channel();
    let bg = worker(&b, BACKGROUND, JIT, 2, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
    let v1 = b.admit(VISIBLE, HAL, "visible one");
    let v2 = b.admit(VISIBLE, HAL, "visible two");
    let visible = worker(&b, VISIBLE, JIT, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 2);
    assert!(rx.try_recv().is_err());
    let interactive = b.admit(INTERACTIVE, APP, "interactive reserve");
    assert_eq!(b.state.lock().unwrap().running, 6);
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
fn warming_allows_one_background_across_clients_and_off_wakes_waiters() {
    let (b, changed) = budget(6);
    b.set_warming(true);
    let p = b.admit(BACKGROUND, APP, "warm seed");
    let (tx, rx) = mpsc::channel();
    let w = worker(&b, BACKGROUND, JIT, 1, &tx);
    until(&b, &changed, |s| s.waiters.len() == 1);
    assert!(rx.try_recv().is_err());
    assert!(!b.should_yield(p));
    let v = b.admit(VISIBLE, HAL, "visible during warmup");
    b.release(v);
    b.set_warming(false);
    assert_eq!(rx.recv().unwrap(), 1);
    b.set_warming(true);
    assert!(b.should_yield(p));
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
    let higher = worker(&b, INTERACTIVE, JIT, 0, &tx);
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
    let higher = worker(&b, INTERACTIVE, JIT, 1, &tx);
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
    let higher = worker(&b, INTERACTIVE, JIT, 1, &tx);
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
    let w = worker(&b, VISIBLE, JIT, 1, &tx);
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
