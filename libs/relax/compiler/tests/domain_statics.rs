// Statics are domain-local (src/domain.rs): code reaches one through the running thread's domain
// block, a static's image points at other statics (cyclic ones too), and a thread std starts runs in
// its parent's domain. With one domain this must read exactly as absolute addresses did.

use std::sync::atomic::*;

struct Node {
    next: &'static Node,
    v: u32,
}

static BASE: u64 = 7;
static PBASE: &u64 = &BASE;
static TABLE: [&u64; 2] = [&BASE, &BASE];
static NA: Node = Node { next: &NB, v: 1 };
static NB: Node = Node { next: &NA, v: 2 };
static mut COUNTER: u64 = 5;
static mut CELL: u64 = 1;
static PCELL: *mut u64 = unsafe { &CELL as *const u64 as *mut u64 };
static HITS: AtomicU64 = AtomicU64::new(0);
static PHITS: &AtomicU64 = &HITS;

#[test]
fn images_point_at_statics() {
    assert_eq!(*PBASE, 7);
    assert!(std::ptr::eq(PBASE, &BASE));
    assert!(std::ptr::eq(TABLE[0], &BASE) && std::ptr::eq(TABLE[1], &BASE));
    assert_eq!(NA.next.v, 2);
    assert_eq!(NB.next.v, 1);
    assert!(std::ptr::eq(NA.next.next, &NA));
    unsafe {
        COUNTER += 1;
        assert_eq!(COUNTER, 6);
        *PCELL += 10;
        assert_eq!(CELL, 11);
        assert!(std::ptr::eq(PCELL as *const u64, &CELL as *const u64));
    }
    PHITS.fetch_add(3, Ordering::SeqCst);
    assert_eq!(HITS.load(Ordering::SeqCst), 3);
}

#[test]
fn threads_run_in_the_domain_of_their_parent() {
    HITS.store(0, Ordering::SeqCst);
    let mut hs = Vec::new();
    for i in 0..4u64 {
        hs.push(std::thread::spawn(move || {
            for _ in 0..1000 {
                PHITS.fetch_add(1, Ordering::SeqCst);
            }
            // the statics this thread names are the process's: the same addresses as in the parent
            (&HITS as *const AtomicU64 as usize, NA.next.next.v as u64 + i, *PBASE)
        }));
    }
    let mut seen = Vec::new();
    for h in hs {
        seen.push(h.join().unwrap());
    }
    assert_eq!(HITS.load(Ordering::SeqCst), 4000);
    for (k, (addr, v, b)) in seen.iter().enumerate() {
        assert_eq!(*addr, &HITS as *const AtomicU64 as usize);
        assert_eq!(*v, 1 + k as u64);
        assert_eq!(*b, 7);
    }
    // a panic caught inside a thread leaves its statics alone
    let r = std::thread::spawn(|| {
        let r = std::panic::catch_unwind(|| {
            PHITS.fetch_add(1, Ordering::SeqCst);
            panic!("domain test");
        });
        r.is_err() && HITS.load(Ordering::SeqCst) >= 4001
    })
    .join()
    .unwrap();
    assert!(r);
}
