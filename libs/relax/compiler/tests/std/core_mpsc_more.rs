use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

struct Counted { id: usize, counts: Arc<Vec<AtomicUsize>> }
impl Drop for Counted {
    fn drop(&mut self) { self.counts[self.id].fetch_add(1, Ordering::SeqCst); }
}
fn counts(n: usize) -> Arc<Vec<AtomicUsize>> {
    let mut v = Vec::new();
    for _ in 0..n { v.push(AtomicUsize::new(0)); }
    Arc::new(v)
}
fn exactly_once(c: &Arc<Vec<AtomicUsize>>) -> bool {
    c.iter().all(|n| n.load(Ordering::SeqCst) == 1)
}

#[test]
fn a01_many_senders() {
    // Each producer has a deterministic xorshift stream; arrival order is free.
    for flavor in 0..4 {
        let mut hs = Vec::new();
        let rx;
        if flavor == 0 {
            let (tx, r) = mpsc::channel();
            rx = r;
            for sender in 0..6u64 {
                let tx = tx.clone();
                hs.push(thread::spawn(move || {
                    let mut x = sender + 123;
                    for i in 0..1200u64 {
                        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
                        tx.send((sender, i, x)).unwrap();
                    }
                }));
            }
            drop(tx);
        } else {
            let cap = if flavor == 1 { 0 } else if flavor == 2 { 1 } else { 37 };
            let (tx, r) = mpsc::sync_channel(cap);
            rx = r;
            for sender in 0..6u64 {
                let tx = tx.clone();
                hs.push(thread::spawn(move || {
                    let mut x = sender + 123;
                    for i in 0..1200u64 {
                        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
                        tx.send((sender, i, x)).unwrap();
                    }
                }));
            }
            drop(tx);
        }
        let mut seq = [0u64; 6];
        let mut state = [123u64, 124, 125, 126, 127, 128];
        for (s, i, x) in rx {
            let s = s as usize;
            assert_eq!(seq[s], i); seq[s] += 1;
            state[s] ^= state[s] << 13; state[s] ^= state[s] >> 7; state[s] ^= state[s] << 17;
            assert_eq!(state[s], x);
        }
        for h in hs { h.join().unwrap(); }
        assert_eq!(seq, [1200; 6]);
        println!("| many flavor {} exact order and values", flavor);
    }
}

#[test]
fn a02_disconnect_blocked_senders() {
    for cap in [0usize, 1, 7] {
        let c = counts(cap + 8);
        let (tx, rx) = mpsc::sync_channel(cap);
        for id in 0..cap { tx.send(Counted { id, counts: c.clone() }).ok().unwrap(); }
        let barrier = Arc::new(Barrier::new(9));
        let mut hs = Vec::new();
        for i in 0..8 {
            let tx = tx.clone(); let c = c.clone(); let b = barrier.clone();
            hs.push(thread::spawn(move || {
                b.wait();
                match tx.send(Counted { id: cap + i, counts: c }) {
                    Err(e) => { assert_eq!(e.0.id, cap + i); drop(e); }
                    Ok(()) => panic!("send must disconnect"),
                }
            }));
        }
        barrier.wait(); thread::sleep(Duration::from_millis(10));
        drop(rx);
        for h in hs { h.join().unwrap(); }
        drop(tx);
        assert!(exactly_once(&c));
        println!("| blocked send cap {} returned values dropped once", cap);
    }
}

#[test]
fn a03_disconnect_receiver_and_timeouts() {
    for flavor in 0..3 {
        let (start, ready) = mpsc::channel();
        let h;
        if flavor == 0 {
            let (tx, rx) = mpsc::channel::<usize>();
            assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));
            assert_eq!(rx.recv_timeout(Duration::from_millis(1)), Err(mpsc::RecvTimeoutError::Timeout));
            h = thread::spawn(move || { start.send(()).unwrap(); assert!(rx.recv().is_err()); });
            ready.recv().unwrap(); thread::sleep(Duration::from_millis(5)); drop(tx);
        } else {
            let (tx, rx) = mpsc::sync_channel::<usize>(flavor - 1);
            assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));
            assert_eq!(rx.recv_timeout(Duration::from_millis(1)), Err(mpsc::RecvTimeoutError::Timeout));
            h = thread::spawn(move || { start.send(()).unwrap(); assert!(rx.recv().is_err()); });
            ready.recv().unwrap(); thread::sleep(Duration::from_millis(5)); drop(tx);
        }
        h.join().unwrap();
        println!("| empty timeout then disconnected flavor {}", flavor);
    }
    for cap in [0usize, 1, 4] {
        let (tx, rx) = mpsc::sync_channel(cap);
        for n in 0..cap { assert_eq!(tx.try_send(n), Ok(())); }
        assert_eq!(tx.try_send(99), Err(mpsc::TrySendError::Full(99)));
        for n in 0..cap { assert_eq!(rx.try_recv(), Ok(n)); }
        drop(rx);
        assert_eq!(tx.try_send(100), Err(mpsc::TrySendError::Disconnected(100)));
        let (tx, rx) = mpsc::sync_channel::<usize>(cap); drop(tx);
        assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected));
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Err(mpsc::RecvTimeoutError::Disconnected));
        println!("| try full empty disconnected cap {}", cap);
    }
    // Successful nonblocking rendezvous with a receiver parked or about to park.
    let (tx, rx) = mpsc::sync_channel(0);
    let h = thread::spawn(move || rx.recv_timeout(Duration::from_secs(5)).unwrap());
    loop {
        match tx.try_send(73) {
            Ok(()) => break,
            Err(mpsc::TrySendError::Full(_)) => thread::yield_now(),
            Err(_) => panic!("receiver disconnected"),
        }
    }
    assert_eq!(h.join().unwrap(), 73);
    println!("| rendezvous try_send delivered");
}

#[test]
fn a04_drop_and_iterators() {
    for flavor in 0..3 {
        let c = counts(200);
        if flavor == 0 {
            let (tx, rx) = mpsc::channel();
            for id in 0..200 { tx.send(Counted { id, counts: c.clone() }).ok().unwrap(); }
            for _ in 0..31 { drop(rx.recv().unwrap()); }
            drop(rx); drop(tx);
        } else {
            let (tx, rx) = mpsc::sync_channel(if flavor == 1 { 0 } else { 37 });
            let cc = c.clone();
            let h = thread::spawn(move || {
                for id in 0..200 { let _ = tx.send(Counted { id, counts: cc.clone() }); }
            });
            for _ in 0..31 { drop(rx.recv().unwrap()); }
            drop(rx); h.join().unwrap();
        }
        assert!(exactly_once(&c));
        println!("| drop flavor {} exactly once", flavor);
    }
    let (tx, rx) = mpsc::channel();
    for n in 0..10 { tx.send(n).unwrap(); }
    assert_eq!(rx.try_iter().sum::<i32>(), 45);
    assert_eq!(rx.try_iter().next(), None);
    for n in 0..10 { tx.send(n).unwrap(); }
    drop(tx); assert_eq!(rx.iter().sum::<i32>(), 45);
    let (tx, rx) = mpsc::channel();
    for n in 0..10 { tx.send(n).unwrap(); }
    drop(tx); assert_eq!(rx.into_iter().sum::<i32>(), 45);
    println!("| iter try_iter into_iter sums 45");
}
