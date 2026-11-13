// Channels with multiple senders, disconnects, ownership, iterators and random operations.
// Differential file: `| ` lines equal under rustc --test and makepad test.
// Timeouts and retry-based rendezvous are left out: their outcome depends on timing.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier};
use std::thread;

struct D(u64, u32);
impl D {
    fn new() -> D {
        D(0xcbf29ce484222325, 0)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    fn add<T: std::fmt::Debug + ?Sized>(&mut self, value: &T) {
        use std::fmt::Write;
        write!(self, "{:?};", value).unwrap();
        self.1 += 1;
    }
    fn line(&self, tag: &str) {
        println!("| {} {} {:016x}", tag, self.1, self.0);
    }
}
impl std::fmt::Write for D {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.bytes(s.as_bytes());
        Ok(())
    }
}

struct Counted {
    id: usize,
    counts: Arc<Vec<AtomicUsize>>,
}
impl Drop for Counted {
    fn drop(&mut self) {
        self.counts[self.id].fetch_add(1, Ordering::SeqCst);
    }
}
fn counts(n: usize) -> Arc<Vec<AtomicUsize>> {
    Arc::new((0..n).map(|_| AtomicUsize::new(0)).collect())
}
fn observed_counts(c: &[AtomicUsize]) -> Vec<usize> {
    c.iter().map(|n| n.load(Ordering::SeqCst)).collect()
}
fn next(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

#[test]
fn a01_many_senders() {
    for flavor in 0..4 {
        let mut handles = Vec::new();
        let rx;
        if flavor == 0 {
            let (tx, receiver) = mpsc::channel();
            rx = receiver;
            for sender in 0..6u64 {
                let tx = tx.clone();
                handles.push(thread::spawn(move || {
                    let mut x = sender + 123;
                    for i in 0..120u64 {
                        tx.send((sender, i, next(&mut x))).unwrap();
                    }
                }));
            }
            drop(tx);
        } else {
            let cap = if flavor == 1 { 0 } else if flavor == 2 { 1 } else { 37 };
            let (tx, receiver) = mpsc::sync_channel(cap);
            rx = receiver;
            for sender in 0..6u64 {
                let tx = tx.clone();
                handles.push(thread::spawn(move || {
                    let mut x = sender + 123;
                    for i in 0..120u64 {
                        tx.send((sender, i, next(&mut x))).unwrap();
                    }
                }));
            }
            drop(tx);
        }
        let mut seq = [0u64; 6];
        let mut state = [123u64, 124, 125, 126, 127, 128];
        let mut received = Vec::new();
        for (sender, i, x) in rx {
            let s = sender as usize;
            assert_eq!(seq[s], i);
            seq[s] += 1;
            assert_eq!(next(&mut state[s]), x);
            received.push((sender, i, x));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(seq, [120; 6]);
        received.sort_unstable();
        let mut d = D::new();
        d.add(&received);
        d.add(&seq);
        d.add(&state);
        d.line(&format!("mpsc many flavor {flavor}"));
    }
}

#[test]
fn a02_disconnect_blocked_senders() {
    for cap in [0usize, 1, 7] {
        let c = counts(cap + 8);
        let (tx, rx) = mpsc::sync_channel(cap);
        for id in 0..cap {
            tx.send(Counted { id, counts: c.clone() }).ok().unwrap();
        }
        let barrier = Arc::new(Barrier::new(9));
        let mut handles = Vec::new();
        for i in 0..8 {
            let tx = tx.clone();
            let c = c.clone();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                match tx.send(Counted { id: cap + i, counts: c }) {
                    Err(error) => {
                        assert_eq!(error.0.id, cap + i);
                        error.0.id
                    }
                    Ok(()) => panic!("send must disconnect"),
                }
            }));
        }
        barrier.wait();
        drop(rx);
        let ids: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        drop(tx);
        let dropped = observed_counts(&c);
        assert!(dropped.iter().all(|n| *n == 1));
        println!("| mpsc disconnect senders {cap} ids {ids:?} drops {dropped:?}");
    }
}

#[test]
fn a03_disconnect_receiver() {
    for flavor in 0..3 {
        let (start, ready) = mpsc::channel();
        let handle;
        if flavor == 0 {
            let (tx, rx) = mpsc::channel::<usize>();
            assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));
            handle = thread::spawn(move || {
                start.send(()).unwrap();
                rx.recv().is_err()
            });
            ready.recv().unwrap();
            drop(tx);
        } else {
            let (tx, rx) = mpsc::sync_channel::<usize>(flavor - 1);
            assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));
            handle = thread::spawn(move || {
                start.send(()).unwrap();
                rx.recv().is_err()
            });
            ready.recv().unwrap();
            drop(tx);
        }
        let disconnected = handle.join().unwrap();
        assert!(disconnected);
        println!("| mpsc receiver flavor {flavor} disconnected {disconnected}");
    }
    for cap in [0usize, 1, 4] {
        let (tx, rx) = mpsc::sync_channel(cap);
        for n in 0..cap {
            assert_eq!(tx.try_send(n), Ok(()));
        }
        assert_eq!(tx.try_send(99), Err(mpsc::TrySendError::Full(99)));
        for n in 0..cap {
            assert_eq!(rx.try_recv(), Ok(n));
        }
        drop(rx);
        assert_eq!(tx.try_send(100), Err(mpsc::TrySendError::Disconnected(100)));
        let (tx, rx) = mpsc::sync_channel::<usize>(cap);
        drop(tx);
        assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected));
        assert!(rx.recv().is_err());
        println!("| mpsc full empty disconnected cap {cap}");
    }
    let (tx, rx) = mpsc::sync_channel(0);
    let handle = thread::spawn(move || rx.recv().unwrap());
    tx.send(73).unwrap();
    let value = handle.join().unwrap();
    assert_eq!(value, 73);
    println!("| mpsc rendezvous send {value}");
}

#[test]
fn a04_drop_and_iterators() {
    for flavor in 0..3 {
        let c = counts(200);
        if flavor == 0 {
            let (tx, rx) = mpsc::channel();
            for id in 0..200 {
                tx.send(Counted { id, counts: c.clone() }).ok().unwrap();
            }
            for _ in 0..31 {
                drop(rx.recv().unwrap());
            }
            drop(rx);
            drop(tx);
        } else {
            let (tx, rx) = mpsc::sync_channel(if flavor == 1 { 0 } else { 37 });
            let cc = c.clone();
            let handle = thread::spawn(move || {
                for id in 0..200 {
                    let _ = tx.send(Counted { id, counts: cc.clone() });
                }
            });
            for _ in 0..31 {
                drop(rx.recv().unwrap());
            }
            drop(rx);
            handle.join().unwrap();
        }
        let dropped = observed_counts(&c);
        assert!(dropped.iter().all(|n| *n == 1));
        println!("| mpsc drops flavor {flavor} {dropped:?}");
    }
    let (tx, rx) = mpsc::channel();
    for n in 0..10 {
        tx.send(n).unwrap();
    }
    assert_eq!(rx.try_iter().sum::<i32>(), 45);
    assert_eq!(rx.try_iter().next(), None);
    for n in 0..10 {
        tx.send(n).unwrap();
    }
    drop(tx);
    assert_eq!(rx.iter().sum::<i32>(), 45);
    let (tx, rx) = mpsc::channel();
    for n in 0..10 {
        tx.send(n).unwrap();
    }
    drop(tx);
    assert_eq!(rx.into_iter().sum::<i32>(), 45);
    println!("| mpsc iter try_iter into_iter sums 45");
}

#[test]
fn a05_randomized_against_real_std() {
    for cap in [1usize, 2, 7, 31, 32, 65] {
        let (tx, rx) = mpsc::sync_channel(cap);
        let mut x = 0x123456789abcdefu64;
        let mut d = D::new();
        for _ in 0..2_000 {
            next(&mut x);
            if x & 1 == 0 {
                let result = tx.try_send(x);
                d.add(&format!("{:?}", result));
                match result {
                    Err(mpsc::TrySendError::Full(value)) => d.add(&value),
                    Ok(()) => {}
                    Err(mpsc::TrySendError::Disconnected(_)) => panic!("receiver disconnected"),
                }
            } else {
                d.add(&format!("{:?}", rx.try_recv()));
            }
        }
        drop(tx);
        d.add(&rx.into_iter().collect::<Vec<_>>());
        d.line(&format!("mpsc bounded cap {cap}"));
    }
    let (tx, rx) = mpsc::channel();
    let mut x = 0x3456789abcdefu64;
    let mut d = D::new();
    for _ in 0..2_000 {
        next(&mut x);
        if x & 3 != 0 {
            tx.send(x).unwrap();
        } else {
            d.add(&format!("{:?}", rx.try_recv()));
        }
    }
    drop(tx);
    d.add(&rx.into_iter().collect::<Vec<_>>());
    d.line("mpsc unbounded");
}

