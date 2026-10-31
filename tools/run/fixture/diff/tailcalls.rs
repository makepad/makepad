// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
// Calls in tail position become jumps (sibling calls), as LLVM makes them: millions of chained calls run in
// constant stack -- a threaded interpreter of `extern "C"` handlers (makepad_stitch's shape: a Rust dispatch fn
// called from each handler, behind its cannot-unwind guard), mutual recursion, fn-pointer chains. Calls that
// are not in tail position (drops after them, a local's address passed) keep their frames and drop order,
// also when a panic unwinds through the chain.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

// ---- a threaded interpreter (the stitch shape): code is a list of handler pointers and immediates
type Handler = unsafe extern "C" fn(ip: *const usize, sp: *mut u64, n: u32, acc: u64, x: f32, y: f64, cx: *mut u64) -> u64;

#[inline(never)]
unsafe fn next(ip: *const usize, sp: *mut u64, n: u32, acc: u64, x: f32, y: f64, cx: *mut u64) -> u64 {
    let h: Handler = std::mem::transmute(*ip);
    h(ip.add(1), sp, n, acc, x, y, cx)
}

unsafe extern "C" fn op_add(ip: *const usize, sp: *mut u64, n: u32, acc: u64, x: f32, y: f64, cx: *mut u64) -> u64 {
    let k = *ip as u64;
    next(ip.add(1), sp, n, acc.wrapping_add(k), x + 0.5, y, cx)
}
unsafe extern "C" fn op_mul(ip: *const usize, sp: *mut u64, n: u32, acc: u64, x: f32, y: f64, cx: *mut u64) -> u64 {
    next(ip, sp, n, acc.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407), x, y * 1.0000001, cx)
}
unsafe extern "C" fn op_store(ip: *const usize, sp: *mut u64, n: u32, acc: u64, x: f32, y: f64, cx: *mut u64) -> u64 {
    *sp.add((acc & 7) as usize) ^= acc;
    *cx += 1;
    next(ip, sp, n, acc, x, y, cx)
}
// the loop: back to the start until the counter runs out
unsafe extern "C" fn op_loop(ip: *const usize, sp: *mut u64, n: u32, acc: u64, x: f32, y: f64, cx: *mut u64) -> u64 {
    let start = *ip as *const usize;
    if n == 0 {
        return acc ^ (x as u64) ^ (y as u64);
    }
    next(start, sp, n - 1, acc, x, y, cx)
}

fn interp(rounds: u32) -> (u64, u64, u64) {
    let mut code: Vec<usize> = Vec::with_capacity(16);
    code.push(op_add as *const () as usize);
    code.push(3);
    code.push(op_mul as *const () as usize);
    code.push(op_store as *const () as usize);
    code.push(op_add as *const () as usize);
    code.push(11);
    code.push(op_mul as *const () as usize);
    code.push(op_loop as *const () as usize);
    let start = code.as_ptr() as usize;
    code.push(start);
    let mut stack = [0u64; 8];
    let mut count = 0u64;
    let r = unsafe { next(code.as_ptr(), stack.as_mut_ptr(), rounds, 1, 0.0, 1.0, &mut count) };
    (r, stack.iter().fold(0, |a, &b| a ^ b.rotate_left(7)), count)
}

// ---- mutual recursion (Relax ABI direct calls)
#[inline(never)]
fn even(n: u64, acc: u64) -> u64 {
    if n == 0 { acc } else { odd(n - 1, acc.wrapping_mul(31).wrapping_add(n)) }
}
#[inline(never)]
fn odd(n: u64, acc: u64) -> u64 {
    if n == 0 { !acc } else { even(n - 1, acc ^ (n << 3)) }
}

// ---- a chain through fn pointers (Relax ABI indirect calls) with float arguments
type Step2 = fn(u64, f64) -> f64;
#[inline(never)]
fn ping(n: u64, f: f64) -> f64 {
    if n == 0 { return f; }
    let t = PONG[(n & 1) as usize];
    t(n - 1, f * 0.999 + 1.0)
}
#[inline(never)]
fn pong(n: u64, f: f64) -> f64 {
    if n == 0 { return -f; }
    let t = PING[(n & 1) as usize];
    t(n - 1, f + 0.25)
}
static PING: [Step2; 2] = [ping, ping];
static PONG: [Step2; 2] = [pong, pong];

// ---- not in tail position: a guard drops after the call; a local's address is passed down
struct Guard(u32, &'static std::sync::Mutex<Vec<u32>>);
impl Drop for Guard {
    fn drop(&mut self) { self.1.lock().unwrap().push(self.0); }
}
static ORDER: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
#[inline(never)]
fn guarded(n: u32) -> u32 {
    let _g = Guard(n, &ORDER);
    if n == 0 { panic!("bottom"); }
    deeper(n - 1)
}
#[inline(never)]
fn deeper(n: u32) -> u32 {
    if n % 3 == 0 { guarded(n) } else { inner_local(n) }
}
#[inline(never)]
fn inner_local(n: u32) -> u32 {
    let x = n * 2;
    with_ref(&x, n)
}
#[inline(never)]
fn with_ref(x: &u32, n: u32) -> u32 {
    let v = *x;
    if n == 0 { v } else { guarded(n - 1) + v }
}

#[inline(never)]
fn sum_to(n: u64, acc: u64) -> u64 {
    if n == 0 { return acc; }
    sum_to(n - 1, acc + n)
}

fn work() {
    let (r, s, c) = interp(1_000_000);
    d!("interp r={:x} stack={:x} count={}", r, s, c);
    d!("even/odd {:x} {:x}", even(5_000_000, 7), odd(3_000_001, 9));
    d!("ping {:.6}", ping(4_000_000, 1.5));
    d!("sum_to {}", sum_to(6_000_000, 0));
    ORDER.lock().unwrap().clear();
    let caught = std::panic::catch_unwind(|| guarded(40));
    d!("guarded panicked={} order={:?}", caught.is_err(), ORDER.lock().unwrap());
    // a thread with a small stack: the chains still fit
    let h = std::thread::Builder::new().stack_size(256 << 10).spawn(|| (interp(200_000), even(1_000_000, 1))).unwrap();
    let ((r, s, c), e) = h.join().unwrap();
    d!("small-stack thread r={:x} stack={:x} count={} even={:x}", r, s, c, e);
}
