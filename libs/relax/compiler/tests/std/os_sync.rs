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

#[cfg(relax)]
mod held_locks {
    use super::*;
    use std::relax::held_lock;

    #[test]
    fn held_mutex_sites() {
        assert_eq!(held_lock(), (0, None));
        let a = Mutex::new(());
        let b = Mutex::new(());
        let first_line = line!() + 1;
        let first = a.lock().unwrap();
        let first_site = held_lock().1.unwrap();
        assert_eq!((first_site.file(), first_site.line()), (file!(), first_line));
        let second_line = line!() + 1;
        let second = b.try_lock().unwrap();
        assert_eq!(held_lock().0, 2);
        assert_eq!(held_lock().1.unwrap().line(), second_line);
        assert!(matches!(a.try_lock(), Err(TryLockError::WouldBlock)));
        assert_eq!(held_lock().0, 2);
        drop(second);
        assert_eq!(held_lock(), (1, Some(first_site)));
        let second = b.lock().unwrap();
        let second_site = held_lock().1;
        drop(first);
        assert_eq!(held_lock(), (1, second_site));
        drop(second);
        assert_eq!(held_lock(), (0, None));
    }

    #[test]
    fn held_mutex_overflow_and_threads() {
        let locks: Vec<_> = (0..12).map(|_| Mutex::new(())).collect();
        let mut guards = Vec::new();
        for lock in &locks { guards.push(lock.lock().unwrap()); }
        assert_eq!(held_lock().0, 12);
        while guards.len() > 4 {
            drop(guards.pop());
            assert_eq!(held_lock().0, guards.len() as u32);
        }
        assert_eq!(held_lock(), (4, None));
        drop(guards);
        assert_eq!(held_lock(), (0, None));
        thread::spawn(|| {
            assert_eq!(held_lock(), (0, None));
            let lock = Mutex::new(());
            let guard = lock.lock().unwrap();
            assert_eq!(held_lock().0, 1);
            drop(guard);
            assert_eq!(held_lock(), (0, None));
        }).join().unwrap();
        assert_eq!(held_lock(), (0, None));
    }

    #[test]
    fn held_condvar_round_trip() {
        let pair = std::sync::Arc::new((Mutex::new(false), Condvar::new()));
        let guard = pair.0.lock().unwrap();
        let before = held_lock();
        let other = pair.clone();
        let worker = thread::spawn(move || {
            assert_eq!(held_lock(), (0, None));
            let mut ready = other.0.lock().unwrap();
            assert_eq!(held_lock().0, 1);
            *ready = true;
            other.1.notify_one();
        });
        let guard = pair.1.wait_while(guard, |ready| {
            assert_eq!(held_lock(), before);
            !*ready
        }).unwrap();
        assert_eq!(held_lock(), before);
        let (guard, _) = pair.1.wait_timeout(guard, Duration::ZERO).unwrap();
        assert_eq!(held_lock(), before);
        let (guard, result) = pair.1.wait_timeout_while(guard, Duration::ZERO, |_| {
            assert_eq!(held_lock(), before);
            false
        }).unwrap();
        assert!(!result.timed_out());
        assert_eq!(held_lock(), before);
        let (guard, result) = pair.1.wait_timeout_while(guard, Duration::ZERO, |_| true).unwrap();
        assert!(result.timed_out());
        assert_eq!(held_lock(), before);
        drop(guard);
        worker.join().unwrap();
        assert_eq!(held_lock(), (0, None));
    }

    #[test]
    fn held_rwlock_sites() {
        let lock = RwLock::new(0);
        let first_line = line!() + 1;
        let first = lock.read().unwrap();
        let first_site = held_lock().1.unwrap();
        assert_eq!((first_site.file(), first_site.line()), (file!(), first_line));
        let second_line = line!() + 1;
        let second = lock.try_read().unwrap();
        assert_eq!(held_lock().0, 2);
        assert_eq!(held_lock().1.unwrap().line(), second_line);
        assert!(matches!(lock.try_write(), Err(TryLockError::WouldBlock)));
        drop(second);
        assert_eq!(held_lock(), (1, Some(first_site)));
        drop(first);
        let write_line = line!() + 1;
        let write = lock.write().unwrap();
        assert_eq!(held_lock().0, 1);
        assert_eq!(held_lock().1.unwrap().line(), write_line);
        assert!(matches!(lock.try_read(), Err(TryLockError::WouldBlock)));
        assert!(matches!(lock.try_write(), Err(TryLockError::WouldBlock)));
        let site = held_lock();
        let read = std::sync::RwLockWriteGuard::downgrade(write);
        assert_eq!(held_lock(), site);
        drop(read);
        assert_eq!(held_lock(), (0, None));
        let try_line = line!() + 1;
        let write = lock.try_write().unwrap();
        assert_eq!(held_lock().1.unwrap().line(), try_line);
        drop(write);
        assert_eq!(held_lock(), (0, None));
    }

    #[test]
    fn held_reentrant_sites() {
        let lock = std::sync::ReentrantLock::new(());
        let first_line = line!() + 1;
        let first = lock.lock();
        assert_eq!(held_lock().1.unwrap().line(), first_line);
        let second_line = line!() + 1;
        let second = lock.try_lock().unwrap();
        assert_eq!(held_lock().0, 2);
        assert_eq!(held_lock().1.unwrap().line(), second_line);
        drop(first);
        assert_eq!(held_lock().0, 1);
        assert_eq!(held_lock().1.unwrap().line(), second_line);
        drop(second);
        assert_eq!(held_lock(), (0, None));
    }
}
