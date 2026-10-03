// std-os lane: core::sync::atomic (differential: os_diff.sh)
use core::sync::atomic::{fence, AtomicBool, AtomicI16, AtomicI32, AtomicI64, AtomicI8, AtomicIsize, AtomicPtr, AtomicU16, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);
static FLAG: AtomicBool = AtomicBool::new(false);

#[test]
fn a01_unsigned() {
    let a = AtomicU8::new(250);
    println!("| {} {} {}", a.fetch_add(10, Ordering::SeqCst), a.load(Ordering::Relaxed), a.fetch_sub(5, Ordering::AcqRel));
    println!("| {} {} {}", a.fetch_and(0x0f, Ordering::SeqCst), a.fetch_or(0xf0, Ordering::SeqCst), a.fetch_xor(0xff, Ordering::SeqCst));
    println!("| {} {}", a.fetch_nand(0x3c, Ordering::SeqCst), a.load(Ordering::Acquire));
    println!("| {} {} {}", a.fetch_max(200, Ordering::SeqCst), a.fetch_min(7, Ordering::SeqCst), a.load(Ordering::SeqCst));
    let b = AtomicU16::new(65535);
    println!("| {} {}", b.fetch_add(2, Ordering::Relaxed), b.swap(9, Ordering::Relaxed));
    let c = AtomicU32::new(1);
    println!("| {:?} {:?}", c.compare_exchange(1, 5, Ordering::AcqRel, Ordering::Acquire), c.compare_exchange(1, 6, Ordering::SeqCst, Ordering::Relaxed));
    let mut w = c.compare_exchange_weak(5, 8, Ordering::SeqCst, Ordering::Relaxed);
    while w.is_err() {
        w = c.compare_exchange_weak(5, 8, Ordering::SeqCst, Ordering::Relaxed);
    }
    println!("| {:?} {}", w, c.load(Ordering::SeqCst));
    let d = AtomicU64::new(u64::MAX - 1);
    println!("| {} {} {:?}", d.fetch_add(3, Ordering::SeqCst), d.load(Ordering::SeqCst), d);
    let r = d.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |x| if x < 10 { Some(x * 7) } else { None });
    let r2 = d.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |x| if x > 100 { Some(x) } else { None });
    println!("| {:?} {:?} {}", r, r2, d.into_inner());
    let mut e = AtomicUsize::default();
    *e.get_mut() += 41;
    e.store(e.load(Ordering::Relaxed) + 1, Ordering::Release);
    println!("| {:?} {}", e, AtomicUsize::from(3).into_inner());
}

#[test]
fn a02_signed() {
    let a = AtomicI8::new(-120);
    println!("| {} {} {}", a.fetch_sub(10, Ordering::SeqCst), a.load(Ordering::SeqCst), a.fetch_max(-5, Ordering::SeqCst));
    println!("| {} {}", a.fetch_min(-100, Ordering::SeqCst), a.load(Ordering::SeqCst));
    let b = AtomicI16::new(i16::MIN);
    println!("| {} {} {}", b.fetch_max(3, Ordering::SeqCst), b.fetch_min(-3, Ordering::SeqCst), b.load(Ordering::SeqCst));
    let c = AtomicI32::new(-1);
    println!("| {} {} {}", c.fetch_add(1, Ordering::SeqCst), c.fetch_max(-7, Ordering::SeqCst), c.load(Ordering::SeqCst));
    let d = AtomicI64::new(i64::MIN);
    println!("| {} {} {}", d.fetch_sub(1, Ordering::SeqCst), d.fetch_min(0, Ordering::SeqCst), d.load(Ordering::SeqCst));
    let e = AtomicIsize::new(5);
    println!("| {} {} {:?}", e.fetch_and(-2, Ordering::SeqCst), e.fetch_nand(3, Ordering::SeqCst), e);
}

#[test]
fn a03_bool_ptr() {
    let b = AtomicBool::new(true);
    println!("| {} {} {}", b.fetch_and(false, Ordering::SeqCst), b.fetch_or(true, Ordering::SeqCst), b.fetch_xor(true, Ordering::SeqCst));
    println!("| {} {} {}", b.fetch_nand(true, Ordering::SeqCst), b.fetch_nand(false, Ordering::SeqCst), b.load(Ordering::SeqCst));
    println!("| {:?} {:?}", b.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst), b.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst));
    println!("| {} {:?} {:?}", b.swap(true, Ordering::SeqCst), b, AtomicBool::default());
    println!("| {:?}", b.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |x| Some(!x)));
    FLAG.store(true, Ordering::Release);
    COUNTER.fetch_add(5, Ordering::Relaxed);
    println!("| {} {}", FLAG.load(Ordering::Acquire), COUNTER.load(Ordering::Relaxed));
    let mut xs = [10i32, 20, 30];
    let p = AtomicPtr::new(xs.as_mut_ptr());
    let q = unsafe { xs.as_mut_ptr().add(2) };
    let old = p.swap(q, Ordering::SeqCst);
    println!("| {} {}", unsafe { *old }, unsafe { *p.load(Ordering::SeqCst) });
    println!("| {:?}", p.compare_exchange(old, old, Ordering::SeqCst, Ordering::SeqCst).is_err());
    println!("| {}", AtomicPtr::<u8>::default().load(Ordering::SeqCst).is_null());
    fence(Ordering::SeqCst);
    fence(Ordering::Acquire);
}
