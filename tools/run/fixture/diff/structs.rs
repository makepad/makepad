// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

#[derive(Clone, Copy, Debug, PartialEq)]
struct V3 { x: f32, y: f32, z: f32 }
#[derive(Clone, Copy, Debug)]
struct Big { a: [u64; 9], v: V3, tag: u8, w: (u16, i8, bool) }
#[derive(Clone, Debug)]
struct Owned { name: String, items: Vec<Big>, opt: Option<Box<Owned>> }
#[inline(never)]
fn make(i: u64) -> Big { let mut a = [0u64; 9]; for k in 0..9 { a[k] = i * (k as u64 + 1) ^ 0x5555; } Big { a, v: V3 { x: i as f32, y: 0.5, z: -(i as f32) }, tag: i as u8, w: (i as u16, -(i as i8), i % 2 == 0) } }
#[inline(never)]
fn bump(mut b: Big) -> Big { b.a[3] += 1; b.v.x *= 2.0; b.tag = b.tag.wrapping_add(200); b.w.1 = b.w.1.wrapping_sub(1); b }
#[inline(never)]
fn sum(b: &Big) -> u64 { b.a.iter().sum::<u64>() + b.v.x as u64 + b.tag as u64 + b.w.0 as u64 + b.w.2 as u64 }
fn work() {
    let mut arr = [make(1); 8];
    for i in 0..8 { arr[i] = bump(make(i as u64 * 3)); }
    let copy = arr;
    arr[2].a[0] = 999;
    d!("{} {} {}", sum(&copy[2]), sum(&arr[2]), copy.iter().map(sum).sum::<u64>());
    let mut a = make(5); let mut b = make(6);
    std::mem::swap(&mut a, &mut b);
    let old = std::mem::replace(&mut a, make(7));
    d!("{:?}\n{:?}", old.v, (a.tag, b.tag, old.w));
    let o = Owned { name: "root".into(), items: vec![make(1), make(2)], opt: Some(Box::new(Owned { name: "kid".into(), items: vec![make(3)], opt: None })) };
    let mut c = o.clone();
    c.items.push(make(9)); c.opt.as_mut().unwrap().name.push_str("++");
    d!("{} {} {} {}", o.items.len(), c.items.len(), o.opt.as_ref().unwrap().name, c.opt.as_ref().unwrap().name);
    let tup = ((1u8, 2.5f64), [3u16; 3], (make(4).w, "s"));
    let t2 = tup; d!("{:?}", t2);
    let mut vs = vec![V3 { x: 1.0, y: 2.0, z: 3.0 }; 4];
    for (i, v) in vs.iter_mut().enumerate() { v.x += i as f32; *v = V3 { x: v.z, y: v.x, z: v.y }; }
    d!("{:?}", vs);
}
