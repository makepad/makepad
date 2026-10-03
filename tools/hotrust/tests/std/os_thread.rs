// std-os lane: std::thread (differential: os_diff.sh)
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

thread_local! {
    static DEPTH: Cell<u32> = Cell::new(0);
    static NAMES: RefCell<Vec<String>> = RefCell::new(Vec::new());
}

fn work(n: u64) -> u64 {
    let mut s = 0u64;
    let mut i = 0;
    while i < n {
        s = s.wrapping_mul(31).wrapping_add(i);
        i += 1;
    }
    s
}

#[test]
fn a01_spawn_join() {
    let hs: Vec<_> = (1..=4).map(|k| thread::spawn(move || work(k * 1000))).collect();
    for h in hs {
        println!("| {}", h.join().unwrap());
    }
    let h = thread::Builder::new().name(String::from("worker-7")).stack_size(256 * 1024).spawn(|| {
        let t = thread::current();
        format!("{:?}", t.name())
    }).unwrap();
    println!("| {}", h.join().unwrap());
    println!("| {:?}", thread::spawn(|| thread::current().name().map(|s| s.to_string())).join().unwrap());
}

#[test]
fn a02_panic_join() {
    let h = thread::spawn(|| {
        if work(3) < 100 {
            panic!("boom in thread");
        }
        5
    });
    let r = h.join();
    println!("| {}", r.is_err());
}

#[test]
fn a03_thread_local() {
    DEPTH.with(|d| d.set(d.get() + 3));
    let h = thread::spawn(|| {
        DEPTH.set(10);
        NAMES.with_borrow_mut(|v| v.push(String::from("t")));
        (DEPTH.get(), NAMES.with_borrow(|v| v.len()))
    });
    println!("| {:?} {} {}", h.join().unwrap(), DEPTH.get(), NAMES.with_borrow(|v| v.len()));
    println!("| {} {}", DEPTH.replace(1), DEPTH.take());
}

#[test]
fn a04_sleep_park() {
    let t0 = Instant::now();
    thread::sleep(Duration::from_millis(20));
    let e = t0.elapsed();
    println!("| {}", e >= Duration::from_millis(20) && e < Duration::from_secs(2));
    let flag = Arc::new(AtomicUsize::new(0));
    let f2 = flag.clone();
    let main = thread::current();
    let h = thread::spawn(move || {
        thread::sleep(Duration::from_millis(5));
        f2.store(1, Ordering::SeqCst);
        main.unpark();
    });
    while flag.load(Ordering::SeqCst) == 0 {
        thread::park();
    }
    h.join().unwrap();
    println!("| {}", flag.load(Ordering::SeqCst));
    let t1 = Instant::now();
    thread::park_timeout(Duration::from_millis(10));
    println!("| {}", t1.elapsed() < Duration::from_secs(2));
    thread::yield_now();
    println!("| {}", thread::available_parallelism().unwrap().get() >= 1);
    println!("| {}", thread::panicking());
}

#[test]
fn a05_scope() {
    let mut data = vec![1, 2, 3, 4];
    let total = AtomicUsize::new(0);
    thread::scope(|s| {
        for x in data.iter() {
            let total = &total;
            s.spawn(move || {
                total.fetch_add(*x, Ordering::SeqCst);
            });
        }
        let h = s.spawn(|| 99);
        println!("| {}", h.join().unwrap());
    });
    data.push(5);
    println!("| {} {:?}", total.load(Ordering::SeqCst), data);
    let ids: Vec<bool> = (0..3).map(|_| thread::spawn(|| thread::current().id()).join().unwrap() != thread::current().id()).collect();
    println!("| {:?}", ids);
    let h = thread::spawn(|| thread::sleep(Duration::from_millis(1)));
    let _ = h.thread().id();
    h.join().unwrap();
}
