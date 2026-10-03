// Atomics and threads: every atomic operation on every integer width (wrapping, signed
// and unsigned max/min, nand, compare-exchange success and failure), then four threads
// started with pthread_create (C calling HotRust `extern "C"` bodies on new threads, which
// compile lazily under the unit lock) incrementing shared counters with fetch_add and with
// CAS loops. Output matches rustc.

use std::sync::atomic::*;

extern "C" {
    fn pthread_create(t: *mut usize, attr: *const u8, start: extern "C" fn(*mut u8) -> *mut u8, arg: *mut u8) -> i32;
    fn pthread_join(t: usize, ret: *mut *mut u8) -> i32;
}

static COUNTER: AtomicU64 = AtomicU64::new(0);
static CAS_COUNTER: AtomicU32 = AtomicU32::new(0);
static MAXV: AtomicI64 = AtomicI64::new(-1000);
static FLAGS: AtomicU8 = AtomicU8::new(0);

extern "C" fn worker(arg: *mut u8) -> *mut u8 {
    let id = arg as usize as u64;
    let mut i = 0u64;
    while i < 20000 {
        COUNTER.fetch_add(1 + id, Ordering::Relaxed);
        let mut cur = CAS_COUNTER.load(Ordering::Relaxed);
        loop {
            match CAS_COUNTER.compare_exchange(cur, cur + 1, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => break,
                Err(now) => cur = now,
            }
        }
        MAXV.fetch_max((i * 7 + id * 3) as i64 % 1001, Ordering::SeqCst);
        i += 1;
    }
    FLAGS.fetch_or(1 << id, Ordering::Release);
    (id * 10) as usize as *mut u8
}

#[test]
fn single_thread_ops() {
    let a = AtomicU8::new(250);
    let r1 = a.fetch_add(10, Ordering::SeqCst);
    let r2 = a.load(Ordering::Acquire);
    let r3 = a.fetch_sub(5, Ordering::Relaxed);
    let r4 = a.load(Ordering::Relaxed);
    println!("u8 {} {} {} {}", r1, r2, r3, r4);
    let b = AtomicI8::new(-100);
    let s1 = b.fetch_sub(100, Ordering::SeqCst);
    let s2 = b.load(Ordering::SeqCst);
    let s3 = b.fetch_max(-5, Ordering::SeqCst);
    let s4 = b.fetch_min(-120, Ordering::SeqCst);
    let s5 = b.load(Ordering::SeqCst);
    println!("i8 {} {} {} {} {}", s1, s2, s3, s4, s5);
    let c = AtomicU16::new(0xF0F0);
    let t1 = c.fetch_and(0x0FF0, Ordering::SeqCst);
    let t2 = c.fetch_or(0x000F, Ordering::SeqCst);
    let t3 = c.fetch_xor(0xFFFF, Ordering::SeqCst);
    let t4 = c.fetch_nand(0x00FF, Ordering::SeqCst);
    let t5 = c.load(Ordering::SeqCst);
    println!("u16 {} {} {} {} {}", t1, t2, t3, t4, t5);
    let d = AtomicI32::new(-7);
    let u1 = d.swap(i32::MIN, Ordering::SeqCst);
    let u2 = d.fetch_sub(1, Ordering::SeqCst);
    let u3 = d.load(Ordering::SeqCst);
    let u4 = d.fetch_min(5, Ordering::SeqCst);
    let u5 = d.fetch_max(5, Ordering::SeqCst);
    println!("i32 {} {} {} {} {}", u1, u2, u3, u4, u5);
    let e = AtomicU32::new(4000000000);
    let v1 = e.fetch_max(100, Ordering::SeqCst);
    let v2 = e.fetch_min(100, Ordering::SeqCst);
    let v3 = e.fetch_nand(0xFFFF, Ordering::SeqCst);
    let v4 = e.load(Ordering::SeqCst);
    println!("u32 {} {} {} {}", v1, v2, v3, v4);
    let f = AtomicU64::new(5);
    let w1 = match f.compare_exchange(5, 9, Ordering::SeqCst, Ordering::SeqCst) {
        Ok(x) => x as i64,
        Err(x) => -(x as i64),
    };
    let w2 = match f.compare_exchange(5, 11, Ordering::SeqCst, Ordering::Relaxed) {
        Ok(x) => x as i64,
        Err(x) => -(x as i64),
    };
    f.store(u64::MAX, Ordering::Release);
    let w3 = f.fetch_add(2, Ordering::SeqCst);
    let w4 = f.load(Ordering::SeqCst);
    println!("u64 {} {} {} {}", w1, w2, w3, w4);
    let g = AtomicI64::new(i64::MAX);
    let x1 = g.fetch_add(1, Ordering::SeqCst);
    let x2 = g.load(Ordering::SeqCst);
    let i = AtomicIsize::new(-3);
    let x3 = i.fetch_max(-4, Ordering::SeqCst);
    let x4 = i.fetch_min(-4, Ordering::SeqCst);
    let x5 = i.load(Ordering::SeqCst);
    let u = AtomicUsize::new(1);
    let x6 = u.fetch_sub(2, Ordering::SeqCst);
    let x7 = u.load(Ordering::SeqCst);
    fence(Ordering::SeqCst);
    compiler_fence(Ordering::SeqCst);
    let h = AtomicI16::new(-2);
    let x8 = h.fetch_and(0x7fff, Ordering::SeqCst);
    let x9 = h.load(Ordering::SeqCst);
    println!("i64/isize/usize/i16 {} {} {} {} {} {} {} {} {}", x1, x2, x3, x4, x5, x6, x7, x8, x9);
}

#[test]
fn threads_and_counters() {
    let mut ts = [0usize; 4];
    let mut k = 0;
    while k < 4 {
        let r = unsafe { pthread_create(&mut ts[k] as *mut usize, 0 as *const u8, worker, k as *mut u8) };
        if r != 0 {
            println!("pthread_create failed {}", r);
        }
        k += 1;
    }
    let mut sum = 0usize;
    k = 0;
    while k < 4 {
        let mut ret = 0 as *mut u8;
        unsafe { pthread_join(ts[k], &mut ret as *mut *mut u8) };
        sum += ret as usize;
        k += 1;
    }
    println!("joined sum {}", sum);
    println!("counter {} cas {} max {} flags {}", COUNTER.load(Ordering::SeqCst), CAS_COUNTER.load(Ordering::SeqCst), MAXV.load(Ordering::SeqCst), FLAGS.load(Ordering::SeqCst));
}
