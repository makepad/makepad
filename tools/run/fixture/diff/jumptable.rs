// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

#[derive(Clone, Copy, Debug)]
enum Op { Add(i32), Sub(i32), Mul(i32), Shl(u8), Not, Neg, Swap, Dup, Pop, Xor(i32), Rot, Nop, Inc, Dec, Abs, Sq, Half, Min(i32), Max(i32), Sel }
#[inline(never)]
fn exec(ops: &[Op], mut a: i32, mut b: i32) -> (i32, i32) {
    for op in ops {
        match *op {
            Op::Add(n) => a = a.wrapping_add(n),
            Op::Sub(n) => a = a.wrapping_sub(n),
            Op::Mul(n) => a = a.wrapping_mul(n),
            Op::Shl(n) => a = a.wrapping_shl(n as u32),
            Op::Not => a = !a,
            Op::Neg => a = a.wrapping_neg(),
            Op::Swap => std::mem::swap(&mut a, &mut b),
            Op::Dup => b = a,
            Op::Pop => a = b,
            Op::Xor(n) => a ^= n,
            Op::Rot => a = a.rotate_left(3),
            Op::Nop => {}
            Op::Inc => a = a.wrapping_add(1),
            Op::Dec => a = a.wrapping_sub(1),
            Op::Abs => a = a.wrapping_abs(),
            Op::Sq => a = a.wrapping_mul(a),
            Op::Half => a /= 2,
            Op::Min(n) => a = a.min(n),
            Op::Max(n) => a = a.max(n),
            Op::Sel => a = if a > b { a } else { b },
        }
    }
    (a, b)
}
#[inline(never)]
fn name(i: u32) -> &'static str {
    match i % 13 { 0 => "zero", 1 => "one", 2 => "two", 3 => "three", 4 => "four", 5 => "five", 6 => "six", 7 => "seven", 8 => "eight", 9 => "nine", 10 => "ten", 11 => "eleven", _ => "other" }
}
fn work() {
    let mut r = 12345u32;
    let mut ops = Vec::new();
    for _ in 0..400 {
        r = r.wrapping_mul(1664525).wrapping_add(1013904223);
        let n = (r >> 8) as i32 % 100;
        ops.push(match (r >> 24) % 20 {
            0 => Op::Add(n), 1 => Op::Sub(n), 2 => Op::Mul(n | 1), 3 => Op::Shl((n & 7) as u8), 4 => Op::Not, 5 => Op::Neg, 6 => Op::Swap,
            7 => Op::Dup, 8 => Op::Pop, 9 => Op::Xor(n), 10 => Op::Rot, 11 => Op::Nop, 12 => Op::Inc, 13 => Op::Dec, 14 => Op::Abs,
            15 => Op::Sq, 16 => Op::Half, 17 => Op::Min(n), 18 => Op::Max(n), _ => Op::Sel,
        });
    }
    let mut h = 0i32;
    for i in 0..50 { let (a, b) = exec(&ops, i, 7 - i); h = h.wrapping_mul(31).wrapping_add(a ^ b); }
    d!("h={}", h);
    let s: String = (0..26).map(name).collect::<Vec<_>>().join(",");
    d!("{}", s);
    let mut cnt = [0u32; 13];
    for i in 0..5000u32 { cnt[(i.wrapping_mul(2654435761) >> 7) as usize % 13] += name(i).len() as u32; }
    d!("{:?}", cnt);
}
