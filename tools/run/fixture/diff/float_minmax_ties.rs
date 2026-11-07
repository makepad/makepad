// diff_fast corpus program: f32/f64 min and max of equal operands (`0.0` and `-0.0`) and of NaNs, the bits
// compared against rustc's: two runtime values, a constant second operand, and running minima and maxima from the
// infinities over a runtime-length slice (a bounding box's `lo = lo.min(p)`), whose accumulator the optimizer knows
// is never NaN (an O3 rewrite once made equal zeros give the accumulator there while the baseline code gave the
// new value: a mesh's bounds were 0.0 or -0.0 depending on the tier; rustc gives the new value). The values go
// through black_box or a volatile read, so rustc keeps the loops scalar (a vectorised reduction gives either
// zero, depending on the target CPU's vector width). Not here, as no tier can match them all: a constant first
// operand (rustc gives the constant; the O1 tier gives the other), two constants (rustc folds
// them with -0.0 below 0.0), a first operand rustc proves never NaN in the same block (an integer's conversion,
// an unrolled fixed-length fold's accumulator: rustc gives it).
use std::hint::black_box as bb;
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

#[inline(never)]
fn two64(a: f64, b: f64) -> [u64; 2] { [a.min(b).to_bits(), a.max(b).to_bits()] }
#[inline(never)]
fn two32(a: f32, b: f32) -> [u32; 2] { [a.min(b).to_bits(), a.max(b).to_bits()] }
#[inline(never)]
fn const_second(a: f64) -> [u64; 4] { [a.min(0.0).to_bits(), a.max(0.0).to_bits(), a.min(-0.0).to_bits(), a.max(-0.0).to_bits()] }
// a runtime-length fold over plain loads
#[inline(never)]
fn fold_plain(xs: &[f64]) -> [u64; 2] {
    let (mut m, mut n) = (f64::INFINITY, f64::NEG_INFINITY);
    for &x in xs {
        m = m.min(x);
        n = n.max(x);
    }
    [m.to_bits(), n.to_bits()]
}
#[derive(Clone, Copy)]
struct V { x: f64, y: f64, z: f64 }
impl V {
    #[inline]
    fn min(self, o: V) -> V { V { x: self.x.min(o.x), y: self.y.min(o.y), z: self.z.min(o.z) } }
    #[inline]
    fn max(self, o: V) -> V { V { x: self.x.max(o.x), y: self.y.max(o.y), z: self.z.max(o.z) } }
}
// a mesh's bounding box: `lo = lo.min(p)`, the running corner first
#[inline(never)]
fn bounds_v(ps: &[[f64; 3]]) -> [u64; 6] {
    let (mut lo, mut hi) = (V { x: f64::INFINITY, y: f64::INFINITY, z: f64::INFINITY }, V { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY, z: f64::NEG_INFINITY });
    for p in ps {
        let p = bb(*p);
        let p = V { x: p[0], y: p[1], z: p[2] };
        lo = lo.min(p);
        hi = hi.max(p);
    }
    [lo.x.to_bits(), lo.y.to_bits(), lo.z.to_bits(), hi.x.to_bits(), hi.y.to_bits(), hi.z.to_bits()]
}
// a mesh's box as an app builds it: `expand(&mut self)` over a slice, no call in the loop (each
// point read volatile, so rustc keeps the loop scalar)
#[derive(Clone, Copy)]
struct BBox { min: V, max: V }
impl BBox {
    fn empty() -> BBox {
        BBox { min: V { x: f64::INFINITY, y: f64::INFINITY, z: f64::INFINITY }, max: V { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY, z: f64::NEG_INFINITY } }
    }
    #[inline]
    fn expand(&mut self, p: V) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
}
#[inline(never)]
fn bbox_volatile(ps: &[V]) -> [u64; 6] {
    let mut b = BBox::empty();
    for p in ps {
        b.expand(unsafe { std::ptr::read_volatile(p) });
    }
    [b.min.x.to_bits(), b.min.y.to_bits(), b.min.z.to_bits(), b.max.x.to_bits(), b.max.y.to_bits(), b.max.z.to_bits()]
}
#[inline(never)]
fn fold_volatile(xs: &[f64]) -> [u64; 2] {
    let (mut m, mut n) = (f64::INFINITY, f64::NEG_INFINITY);
    for x in xs {
        let x = unsafe { std::ptr::read_volatile(x) };
        m = m.min(x);
        n = n.max(x);
    }
    [m.to_bits(), n.to_bits()]
}
#[inline(never)]
fn fold_min(xs: &[f64]) -> [u64; 2] {
    let (mut m, mut n) = (f64::INFINITY, f64::NEG_INFINITY);
    for &x in xs {
        let x = bb(x);
        m = m.min(x);
        n = n.max(x);
    }
    [m.to_bits(), n.to_bits()]
}
#[inline(never)]
fn fold_min32(xs: &[f32]) -> [u32; 2] {
    let (mut m, mut n) = (f32::INFINITY, f32::NEG_INFINITY);
    for &x in xs {
        let x = bb(x);
        m = m.min(x);
        n = n.max(x);
    }
    [m.to_bits(), n.to_bits()]
}
#[inline(never)]
fn bounds64(ps: &[[f64; 3]]) -> [u64; 6] {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in ps {
        let p = bb(*p);
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    [lo[0].to_bits(), lo[1].to_bits(), lo[2].to_bits(), hi[0].to_bits(), hi[1].to_bits(), hi[2].to_bits()]
}
#[inline(never)]
fn bounds32(ps: &[[f32; 2]]) -> [u32; 4] {
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for p in ps {
        let p = bb(*p);
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    [lo[0].to_bits(), lo[1].to_bits(), hi[0].to_bits(), hi[1].to_bits()]
}
fn work() {
    let vals = [0.0f64, -0.0, 1.0, -1.0, f64::NAN, f64::INFINITY];
    for &a in &vals {
        for &b in &vals {
            let [mn, mx] = two64(bb(a), bb(b));
            let [mn32, mx32] = two32(bb(a as f32), bb(b as f32));
            d!("two {:?} {:?}: f64 {:016x} {:016x} f32 {:08x} {:08x}", a, b, mn, mx, mn32, mx32);
        }
        d!("const second {:?}: {:016x?}", a, const_second(bb(a)));
    }
    for xs in [[1.0, 0.0, -0.0, 2.0], [1.0, -0.0, 0.0, 2.0], [-0.0, 0.0, -1.0, 1.0], [0.0, -0.0, 3.0, -3.0]] {
        d!("fold plain {:?}: {:016x?}", xs, fold_plain(bb(&xs[..])));
    }
    // zeros of both signs in each order, the box's corner at zero on every axis
    let ps = [[1.0, 0.0, -0.0], [0.0, -0.0, 2.0], [-0.0, 0.0, 0.0], [-1.0, 3.0, -0.0], [0.0, 0.0, 0.0]];
    for n in 1..=ps.len() {
        d!("bounds64 first {n}: {:016x?}", bounds64(&ps[..n]));
        d!("bounds_v first {n}: {:016x?}", bounds_v(&ps[..n]));
        let xs: Vec<f64> = ps[..n].iter().flatten().copied().collect();
        d!("fold first {n}: {:016x?}", fold_min(&xs));
        d!("fold volatile first {n}: {:016x?}", fold_volatile(bb(&xs)));
        let vs: Vec<V> = ps[..n].iter().map(|p| V { x: p[0], y: p[1], z: p[2] }).collect();
        d!("bbox volatile first {n}: {:016x?}", bbox_volatile(bb(&vs)));
        let ys: Vec<f32> = xs.iter().map(|&x| x as f32).collect();
        d!("fold32 first {n}: {:08x?}", fold_min32(&ys));
        let qs: Vec<[f32; 2]> = ps[..n].iter().map(|p| [p[1] as f32, -(p[2] as f32)]).collect();
        d!("bounds32 first {n}: {:08x?}", bounds32(&qs));
    }
}
