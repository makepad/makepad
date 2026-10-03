// std-os lane: the std-os modules exercised with primitive-only output (no Debug/Display of
// std types), so HotRust can run them before ADT formatting lands. Differential: os_diff.sh --hotrust.
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, Once, OnceLock, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static HITS: AtomicUsize = AtomicUsize::new(0);
static ONCE: Once = Once::new();
static CELL: OnceLock<u64> = OnceLock::new();
static TOTAL: Mutex<u64> = Mutex::new(0);
static READY: Mutex<bool> = Mutex::new(false);
static READY_CV: Condvar = Condvar::new();

#[test]
fn a01_atomics() {
    let a = AtomicU32::new(5);
    let p = a.fetch_add(3, Ordering::SeqCst);
    let s = a.swap(100, Ordering::AcqRel);
    let ok = a.compare_exchange(100, 7, Ordering::SeqCst, Ordering::Relaxed).is_ok();
    let bad = a.compare_exchange(100, 9, Ordering::SeqCst, Ordering::Relaxed).is_err();
    println!("| {} {} {} {} {}", p, s, ok, bad, a.load(Ordering::SeqCst));
    let b = AtomicBool::new(false);
    println!("| {} {}", b.swap(true, Ordering::SeqCst), b.load(Ordering::SeqCst));
    let c = AtomicU64::new(u64::MAX);
    println!("| {} {}", c.fetch_add(2, Ordering::SeqCst), c.load(Ordering::SeqCst));
    HITS.fetch_add(1, Ordering::Relaxed);
    println!("| {}", HITS.load(Ordering::Relaxed));
}

#[test]
fn a02_duration() {
    let a = Duration::new(3, 700_000_000);
    let b = Duration::from_millis(1900);
    let c = a + b;
    let d = a - b;
    let e = a * 3;
    let f = a / 7;
    println!("| {} {} {} {} {} {} {} {}", c.as_secs(), c.subsec_nanos(), d.as_secs(), d.subsec_nanos(), e.as_secs(), e.subsec_nanos(), f.as_secs(), f.subsec_nanos());
    println!("| {} {}", a.checked_sub(b).is_some(), b.checked_sub(a).is_none());
    let g = Duration::from_secs_f64(2.7);
    println!("| {} {} {}", g.as_secs(), g.subsec_nanos(), a.as_secs_f64());
}

#[test]
fn a03_mutex_threads() {
    let mut hs = Vec::new();
    let mut i = 0u64;
    while i < 4 {
        hs.push(thread::spawn(move || {
            let mut k = 0;
            while k < 1000 {
                *TOTAL.lock().unwrap() += i;
                k += 1;
            }
        }));
        i += 1;
    }
    for h in hs {
        h.join().unwrap();
    }
    println!("| {}", *TOTAL.lock().unwrap());
}

#[test]
fn a04_condvar_once() {
    let h = thread::spawn(move || {
        thread::sleep(Duration::from_millis(3));
        *READY.lock().unwrap() = true;
        READY_CV.notify_all();
    });
    let mut g = READY.lock().unwrap();
    while !*g {
        g = READY_CV.wait(g).unwrap();
    }
    drop(g);
    h.join().unwrap();
    println!("| woke");
    ONCE.call_once(|| {
        HITS.fetch_add(10, Ordering::SeqCst);
    });
    ONCE.call_once(|| {
        HITS.fetch_add(10, Ordering::SeqCst);
    });
    println!("| {} {}", ONCE.is_completed(), *CELL.get_or_init(|| 42));
    let l = RwLock::new(5);
    let r = *l.read().unwrap();
    *l.write().unwrap() += 1;
    println!("| {} {}", r, *l.read().unwrap());
}

#[test]
fn a05_time_thread() {
    let t = Instant::now();
    thread::sleep(Duration::from_millis(5));
    let e = t.elapsed();
    println!("| {}", e.as_nanos() >= 5_000_000);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    println!("| {}", now.as_secs() > 1_700_000_000);
    let h = thread::Builder::new().name(String::from("probe")).spawn(|| {
        let t = thread::current();
        t.name() == Some("probe")
    }).unwrap();
    println!("| {}", h.join().unwrap());
}

#[test]
fn a06_io_fs_path() {
    use std::io::{BufRead, BufReader, Read, Write};
    let mut v: Vec<u8> = Vec::new();
    v.write_all(b"one\ntwo\nthree").unwrap();
    let r = BufReader::new(&v[..]);
    let mut n = 0;
    for l in r.lines() {
        n += l.unwrap().len();
    }
    println!("| {} {}", v.len(), n);
    let dir = std::env::temp_dir().join("hr-os-probe");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("x.txt");
    std::fs::write(&f, b"hello file").unwrap();
    let mut s = String::new();
    std::fs::File::open(&f).unwrap().read_to_string(&mut s).unwrap();
    println!("| {} {} {}", s.len(), std::fs::metadata(&f).unwrap().len(), f.extension().unwrap().to_str().unwrap());
    std::fs::remove_dir_all(&dir).unwrap();
    println!("| {} {}", dir.exists(), std::path::Path::new("/a/b/c.txt").parent().unwrap().to_str().unwrap());
    std::env::set_var("HR_PROBE", "1");
    println!("| {}", std::env::var("HR_PROBE").unwrap().len());
}
