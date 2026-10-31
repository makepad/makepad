// thread_local! reads take an inline path through the thread's key table; the table moves when more
// keys are set (new keys after reads), threads keep their own values, a thread that sets none reads
// the initialiser, and a destructor reads another key of the dying thread
use std::cell::{Cell, RefCell};

macro_rules! keys {
    ($($name:ident = $init:expr;)*) => {
        thread_local! { $(static $name: Cell<u64> = Cell::new($init);)* }
        fn bump_all(by: u64) -> u64 { let mut s = 0; $( $name.with(|c| { c.set(c.get() + by); s += c.get(); }); )* s }
    };
}
keys! {
    K0 = 0; K1 = 1; K2 = 2; K3 = 3; K4 = 4; K5 = 5; K6 = 6; K7 = 7; K8 = 8; K9 = 9;
    K10 = 10; K11 = 11; K12 = 12; K13 = 13; K14 = 14; K15 = 15; K16 = 16; K17 = 17; K18 = 18; K19 = 19;
    K20 = 20; K21 = 21; K22 = 22; K23 = 23; K24 = 24; K25 = 25; K26 = 26; K27 = 27; K28 = 28; K29 = 29;
}

thread_local! { static EARLY: Cell<u32> = Cell::new(7); }
thread_local! { static LATE: RefCell<Vec<u32>> = RefCell::new(vec![1, 2, 3]); }

struct Reader;
impl Drop for Reader {
    fn drop(&mut self) {
        // a destructor of one key reads another key of the same dying thread
        let n = EARLY.try_with(|e| e.get()).unwrap_or(0);
        SEEN.with(|s| s.fetch_add(n as usize, std::sync::atomic::Ordering::SeqCst));
    }
}
static SEEN_: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct Seen;
impl Seen { fn with<R>(&self, f: impl FnOnce(&std::sync::atomic::AtomicUsize) -> R) -> R { f(&SEEN_) } }
static SEEN: Seen = Seen;
thread_local! { static HOLD: Reader = Reader; }

#[test]
fn reads_follow_table_growth() {
    // first read of EARLY, then thirty more keys set (the table grows), EARLY again, then LATE
    assert_eq!(EARLY.with(|e| e.get()), 7);
    let first = bump_all(1);
    assert_eq!(first, (0..30).sum::<u64>() + 30);
    assert_eq!(EARLY.with(|e| e.get()), 7);
    EARLY.with(|e| e.set(9));
    LATE.with(|v| v.borrow_mut().push(4));
    for round in 0..1000u64 {
        let s = bump_all(1);
        assert_eq!(s, (0..30).sum::<u64>() + 30 * (round + 2));
    }
    assert_eq!(EARLY.with(|e| e.get()), 9);
    assert_eq!(LATE.with(|v| v.borrow().len()), 4);
}

#[test]
fn threads_keep_their_own_values() {
    let hs: Vec<_> = (0..8u64).map(|t| std::thread::spawn(move || {
        // a thread that starts by reading a late key sees the initialiser
        assert_eq!(K29.with(|c| c.get()), 29);
        for _ in 0..200 { bump_all(t); }
        (K0.with(|c| c.get()), K29.with(|c| c.get()), EARLY.with(|e| e.get()))
    })).collect();
    for (t, h) in hs.into_iter().enumerate() {
        let (k0, k29, e) = h.join().unwrap();
        assert_eq!((k0, k29, e), (200 * t as u64, 29 + 200 * t as u64, 7));
    }
}

#[test]
fn destructor_reads_another_key() {
    let before = SEEN_.load(std::sync::atomic::Ordering::SeqCst);
    std::thread::spawn(|| {
        EARLY.with(|e| e.set(11));
        HOLD.with(|_| {});
    }).join().unwrap();
    // thread-local destructors run in no promised order: the dying thread's EARLY is 11 or already gone (0)
    let seen = SEEN_.load(std::sync::atomic::Ordering::SeqCst) - before;
    assert!(seen == 11 || seen == 0, "{seen}");
}
