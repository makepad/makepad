// std-os lane: std::sync (differential: os_diff.sh)
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Condvar, LazyLock, Mutex, Once, OnceLock, RwLock, TryLockError};
use std::thread;
use std::time::Duration;

static INIT: Once = Once::new();
static CELL: OnceLock<String> = OnceLock::new();
static LAZY: LazyLock<Vec<u32>> = LazyLock::new(|| vec![1, 2, 3]);

#[test]
fn a01_mutex() {
    let m = Mutex::new(5);
    {
        let mut g = m.lock().unwrap();
        *g += 1;
        println!("| {} {:?}", *g, m.try_lock().is_err());
        match m.try_lock() {
            Err(TryLockError::WouldBlock) => println!("| would block"),
            _ => println!("| ??"),
        }
        println!("| {:?}", m);
    }
    println!("| {:?} {} {:?}", m, m.is_poisoned(), m.lock().unwrap());
    let mut m2 = Mutex::new(String::from("x"));
    m2.get_mut().unwrap().push('y');
    println!("| {:?} {:?}", m2.into_inner(), Mutex::<i32>::default());
}

#[test]
fn a02_mutex_threads() {
    let m = Arc::new(Mutex::new(0u64));
    let mut hs = Vec::new();
    for i in 0..8 {
        let m = m.clone();
        hs.push(thread::spawn(move || {
            for _ in 0..1000 {
                *m.lock().unwrap() += i;
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    println!("| {}", *m.lock().unwrap());
}

#[test]
fn a03_condvar() {
    let pair = Arc::new((Mutex::new(0), Condvar::new()));
    let p2 = pair.clone();
    let h = thread::spawn(move || {
        for i in 1..=5 {
            thread::sleep(Duration::from_millis(2));
            let (m, c) = &*p2;
            *m.lock().unwrap() = i;
            c.notify_all();
        }
    });
    let (m, c) = &*pair;
    let g = c.wait_while(m.lock().unwrap(), |v| *v < 5).unwrap();
    println!("| {}", *g);
    drop(g);
    h.join().unwrap();
    let (g, r) = c.wait_timeout(m.lock().unwrap(), Duration::from_millis(10)).unwrap();
    println!("| {} {}", *g, r.timed_out());
    drop(g);
    let (g, r) = c.wait_timeout_while(m.lock().unwrap(), Duration::from_millis(10), |v| *v == 5).unwrap();
    println!("| {} {:?}", *g, r);
    println!("| {:?}", Condvar::new());
}

#[test]
fn a04_rwlock() {
    let l = Arc::new(RwLock::new(vec![1]));
    {
        let r1 = l.read().unwrap();
        let r2 = l.read().unwrap();
        println!("| {} {} {}", r1.len(), r2.len(), l.try_write().is_err());
        println!("| {:?}", l);
    }
    l.write().unwrap().push(2);
    let mut hs = Vec::new();
    for i in 0..4 {
        let l = l.clone();
        hs.push(thread::spawn(move || {
            for _ in 0..100 {
                l.write().unwrap().push(i);
                let _ = l.read().unwrap().len();
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    let total: u32 = l.read().unwrap().iter().sum();
    println!("| {} {} {:?}", l.read().unwrap().len(), total, l.try_read().is_ok());
}

#[test]
fn a05_once() {
    let n = AtomicUsize::new(0);
    for _ in 0..3 {
        INIT.call_once(|| {
            n.fetch_add(1, Ordering::SeqCst);
        });
    }
    println!("| {} {}", n.load(Ordering::SeqCst), INIT.is_completed());
    println!("| {:?} {:?}", CELL.get(), CELL);
    let v = CELL.get_or_init(|| String::from("first"));
    println!("| {} {:?} {:?}", v, CELL.set(String::from("second")), CELL);
    let c2: OnceLock<u32> = OnceLock::new();
    let hs: Vec<_> = (0..4).map(|i| {
        let c = &c2 as *const OnceLock<u32> as usize;
        thread::spawn(move || unsafe { *(*(c as *const OnceLock<u32>)).get_or_init(|| 100 + i * 0) })
    }).collect();
    for h in hs {
        print!("");
        println!("| {}", h.join().unwrap());
    }
    println!("| {:?} {}", *LAZY, LAZY.len());
    let l2: LazyLock<u32> = LazyLock::new(|| 7);
    println!("| {:?}", l2);
    println!("| {} {:?}", *l2, l2);
    let mut c3 = OnceLock::from(3);
    println!("| {:?} {:?} {:?}", c3.take(), c3.get(), c3);
}

#[test]
fn a06_mpsc() {
    let (tx, rx) = mpsc::channel();
    let mut hs = Vec::new();
    for t in 0..4 {
        let tx = tx.clone();
        hs.push(thread::spawn(move || {
            for i in 0..50 {
                tx.send(t * 1000 + i).unwrap();
            }
        }));
    }
    drop(tx);
    let mut got: Vec<i32> = rx.iter().collect();
    got.sort();
    println!("| {} {} {}", got.len(), got[0], got[199]);
    for h in hs {
        h.join().unwrap();
    }
    println!("| {:?} {:?}", rx.try_recv(), rx.recv());
    let (tx, rx) = mpsc::channel::<u8>();
    println!("| {:?} {:?}", rx.try_recv(), rx.recv_timeout(Duration::from_millis(5)));
    drop(rx);
    println!("| {:?}", tx.send(1));
    match tx.send(2) {
        Err(e) => println!("| {} {:?}", e, e.0),
        Ok(()) => {}
    }
    let (stx, srx) = mpsc::sync_channel(2);
    println!("| {:?} {:?} {:?}", stx.try_send(1), stx.try_send(2), stx.try_send(3));
    println!("| {:?} {:?}", srx.recv(), srx.try_iter().collect::<Vec<_>>());
    let h = thread::spawn(move || {
        for i in 0..10 {
            stx.send(i).unwrap();
        }
    });
    let v: Vec<i32> = srx.iter().collect();
    h.join().unwrap();
    println!("| {:?}", v);
    let (ztx, zrx) = mpsc::sync_channel::<u32>(0);
    let h = thread::spawn(move || {
        ztx.send(42).unwrap();
        ztx.send(43).unwrap();
    });
    println!("| {:?} {:?} {:?}", zrx.recv(), zrx.recv(), {
        h.join().unwrap();
        zrx.recv()
    });
    println!("| {} {} {}", mpsc::RecvError, mpsc::TryRecvError::Empty, mpsc::RecvTimeoutError::Timeout);
    println!("| {:?} {}", mpsc::TrySendError::Full(1), mpsc::TrySendError::Disconnected(1));
}

#[test]
fn a07_barrier() {
    let b = Arc::new(Barrier::new(4));
    let leaders = Arc::new(AtomicUsize::new(0));
    let mut hs = Vec::new();
    for _ in 0..4 {
        let b = b.clone();
        let l = leaders.clone();
        hs.push(thread::spawn(move || {
            for _ in 0..3 {
                if b.wait().is_leader() {
                    l.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    println!("| {} {:?}", leaders.load(Ordering::SeqCst), b);
}
