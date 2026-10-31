// diff_fast corpus program: the lane-by-lane generic SIMD intrinsics (core::intrinsics::simd): integer div/rem,
// bit counts and reversals, fma, round_ties_even, casts (simd_cast / simd_as), splat, extract/insert (constant and
// runtime index), shuffle, reductions, bit masks and select by bit mask; every result line is `D ...`.
// (rustc side: RUSTC_BOOTSTRAP=1, set by diff_fast.sh for programs with #![feature].)
#![feature(repr_simd, core_intrinsics)]
#![allow(non_camel_case_types, internal_features)]
use core::intrinsics::simd::*;

macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}
fn mix(h: &mut u64, x: u64) {
    *h = (*h ^ x).wrapping_mul(0x100000001b3).rotate_left(7);
}
fn canon32(x: f32) -> u64 { if x.is_nan() { 1 } else { x.to_bits() as u64 } }
fn canon64(x: f64) -> u64 { if x.is_nan() { 1 } else { x.to_bits() } }

#[repr(simd)] #[derive(Copy, Clone)] struct i32x4([i32; 4]);
#[repr(simd)] #[derive(Copy, Clone)] struct u8x16([u8; 16]);
#[repr(simd)] #[derive(Copy, Clone)] struct i8x16([i8; 16]);
#[repr(simd)] #[derive(Copy, Clone)] struct u16x8([u16; 8]);
#[repr(simd)] #[derive(Copy, Clone)] struct i64x2([i64; 2]);
#[repr(simd)] #[derive(Copy, Clone)] struct u64x4([u64; 4]);
#[repr(simd)] #[derive(Copy, Clone)] struct f32x4([f32; 4]);
#[repr(simd)] #[derive(Copy, Clone)] struct f64x2([f64; 2]);
#[repr(simd)] #[derive(Copy, Clone)] struct f64x4([f64; 4]);
#[repr(simd)] #[derive(Copy, Clone)] struct i16x4([i16; 4]);
#[repr(simd)] #[derive(Copy, Clone)] struct idx4([u32; 4]);
#[repr(simd)] #[derive(Copy, Clone)] struct idx8([u32; 8]);
#[repr(simd)] #[derive(Copy, Clone)] struct i32x8([i32; 8]);

const I4: idx4 = idx4([7, 0, 5, 2]);
const I8: idx8 = idx8([1, 6, 3, 3, 0, 7, 4, 2]);

#[inline(never)]
fn ints(rng: &mut Rng) {
    let mut hs = [0u64; 24];
    for _ in 0..64 {
        let a: [i32; 4] = core::array::from_fn(|_| rng.next() as i32 >> (rng.next() % 24));
        let b: [i32; 4] = core::array::from_fn(|_| { let x = (rng.next() as i32 >> (rng.next() % 28)) | 1; if x == -1 { 3 } else { x } });
        let (va, vb) = (i32x4(a), i32x4(b));
        let r: [i32x4; 7] = unsafe { [simd_div(va, vb), simd_rem(va, vb), simd_ctpop(va), simd_ctlz(va), simd_cttz(va), simd_bswap(va), simd_bitreverse(va)] };
        for (i, v) in r.iter().enumerate() {
            let l: [i32; 4] = unsafe { core::mem::transmute(*v) };
            for x in l { mix(&mut hs[i], x as u64); }
        }
        let ub: [u8; 16] = core::array::from_fn(|_| rng.next() as u8);
        let u = u8x16(ub);
        let r: [u8x16; 5] = unsafe { [simd_ctpop(u), simd_ctlz(u), simd_cttz(u), simd_bitreverse(u), simd_bswap(u)] };
        for (i, v) in r.iter().enumerate() {
            let l: [u8; 16] = unsafe { core::mem::transmute(*v) };
            for x in l { mix(&mut hs[7 + i], x as u64); }
        }
        let w: u16x8 = u16x8(core::array::from_fn(|_| rng.next() as u16));
        let r: [u16x8; 3] = unsafe { [simd_ctlz(w), simd_cttz(w), simd_bswap(w)] };
        for (i, v) in r.iter().enumerate() {
            let l: [u16; 8] = unsafe { core::mem::transmute(*v) };
            for x in l { mix(&mut hs[12 + i], x as u64); }
        }
        // reductions
        let q = u64x4(core::array::from_fn(|_| rng.next()));
        let s = i8x16(core::array::from_fn(|_| rng.next() as i8));
        unsafe {
            mix(&mut hs[15], simd_reduce_add_unordered::<u64x4, u64>(q));
            mix(&mut hs[15], simd_reduce_mul_unordered::<u64x4, u64>(q));
            mix(&mut hs[15], simd_reduce_xor::<u64x4, u64>(q));
            mix(&mut hs[15], simd_reduce_and::<u64x4, u64>(q) ^ simd_reduce_or::<u64x4, u64>(q));
            mix(&mut hs[16], simd_reduce_min::<u64x4, u64>(q) ^ simd_reduce_max::<u64x4, u64>(q).rotate_left(1));
            mix(&mut hs[16], simd_reduce_min::<i8x16, i8>(s) as u64 ^ (simd_reduce_max::<i8x16, i8>(s) as u64) << 8);
            mix(&mut hs[16], simd_reduce_add_ordered::<i8x16, i8>(s, 3) as u64);
            mix(&mut hs[16], simd_reduce_mul_ordered::<i32x4, i32>(va, -7) as u64);
            // masks
            let m: i32x4 = simd_lt(va, vb);
            mix(&mut hs[17], simd_reduce_any(m) as u64 | (simd_reduce_all(m) as u64) << 1);
            mix(&mut hs[17], simd_bitmask::<i32x4, u8>(m) as u64);
            let m16: i8x16 = simd_gt(s, i8x16([0; 16]));
            mix(&mut hs[17], simd_bitmask::<i8x16, u16>(m16) as u64);
            let bm: [u8; 2] = simd_bitmask::<i8x16, [u8; 2]>(m16);
            mix(&mut hs[17], bm[0] as u64 | (bm[1] as u64) << 8);
            let sel: i32x4 = simd_select_bitmask((rng.next() & 15) as u8, va, vb);
            let l: [i32; 4] = core::mem::transmute(sel);
            for x in l { mix(&mut hs[18], x as u64); }
            // lanes
            let k = (rng.next() % 4) as u32;
            mix(&mut hs[19], simd_extract::<i32x4, i32>(va, 2) as u64 ^ (simd_extract_dyn::<i32x4, i32>(va, k) as u64) << 1);
            let ins: i32x4 = simd_insert_dyn(simd_insert(va, 0, 77i32), k, -5i32);
            let l: [i32; 4] = core::mem::transmute(ins);
            for x in l { mix(&mut hs[19], x as u64); }
            let sp: i64x2 = simd_splat(rng.next() as i64);
            let l: [i64; 2] = core::mem::transmute(sp);
            for x in l { mix(&mut hs[20], x as u64); }
            let sh: i32x4 = simd_shuffle(va, vb, I4);
            let sh8: i32x8 = simd_shuffle(va, vb, I8);
            let l: [i32; 4] = core::mem::transmute(sh);
            for x in l { mix(&mut hs[21], x as u64); }
            let l: [i32; 8] = core::mem::transmute(sh8);
            for x in l { mix(&mut hs[21], x as u64); }
            // casts
            let c16: i16x4 = simd_cast(va);
            let cf: f32x4 = simd_cast(va);
            let cd: f64x4 = simd_as(va);
            let l: [i16; 4] = core::mem::transmute(c16);
            for x in l { mix(&mut hs[22], x as u64); }
            let l: [f32; 4] = core::mem::transmute(cf);
            for x in l { mix(&mut hs[22], canon32(x)); }
            let l: [f64; 4] = core::mem::transmute(cd);
            for x in l { mix(&mut hs[22], canon64(x)); }
            let qf: f64x4 = simd_cast(q);
            let l: [f64; 4] = core::mem::transmute(qf);
            for x in l { mix(&mut hs[23], canon64(x)); }
        }
    }
    d!("ints {:016x?}", hs);
}

#[inline(never)]
fn floats(rng: &mut Rng) {
    let edge = [0.0f32, -0.0, 0.5, 1.5, 2.5, -2.5, 3.5, f32::INFINITY, f32::NAN, 3.0e9, -3.0e9, 1.0e20, 2147483520.0, -2147483904.0];
    let mut hs = [0u64; 8];
    for _ in 0..64 {
        let a: [f32; 4] = core::array::from_fn(|_| { let r = rng.next(); if r % 3 == 0 { (rng.next() as i32 as f32) * 1e-6 } else { edge[(r >> 8) as usize % edge.len()] } });
        let b: [f32; 4] = core::array::from_fn(|_| (rng.next() as i32 as f32) * 1e-9);
        let (va, vb) = (f32x4(a), f32x4(b));
        unsafe {
            let r: [f32x4; 3] = [simd_fma(va, vb, va), simd_round_ties_even(va), simd_relaxed_fma(vb, vb, vb)];
            for (i, v) in r.iter().enumerate() {
                let l: [f32; 4] = core::mem::transmute(*v);
                for x in l { mix(&mut hs[i], canon32(x)); }
            }
            let ia: i32x4 = simd_as(va);
            let ub: u8x16 = simd_as(f32x4([a[0], a[1], a[2], a[3]]).clone_to16());
            let l: [i32; 4] = core::mem::transmute(ia);
            for x in l { mix(&mut hs[3], x as u64); }
            let l: [u8; 16] = core::mem::transmute(ub);
            for x in l { mix(&mut hs[3], x as u64); }
            let wide: f64x4 = simd_cast(va);
            let back: f32x4 = simd_cast(simd_add(wide, wide));
            let l: [f32; 4] = core::mem::transmute(back);
            for x in l { mix(&mut hs[4], canon32(x)); }
            mix(&mut hs[5], canon32(simd_reduce_add_ordered(va, 0.25f32)));
            mix(&mut hs[5], canon32(simd_reduce_mul_ordered(vb, -1.5f32)));
            mix(&mut hs[6], simd_reduce_any(simd_lt::<f32x4, i32x4>(va, vb)) as u64);
            let d2 = f64x2([a[0] as f64, b[1] as f64]);
            mix(&mut hs[7], canon64(simd_reduce_add_ordered(d2, -0.0f64)));
            let e: f64 = simd_extract_dyn(d2, (rng.next() % 2) as u32);
            mix(&mut hs[7], canon64(e));
        }
    }
    d!("floats {:016x?}", hs);
}

#[repr(simd)] #[derive(Copy, Clone)] struct f32x16([f32; 16]);
trait To16 { fn clone_to16(self) -> f32x16; }
impl To16 for f32x4 {
    fn clone_to16(self) -> f32x16 {
        let l: [f32; 4] = unsafe { core::mem::transmute(self) };
        f32x16(core::array::from_fn(|i| l[i % 4] * (i as f32 + 1.0)))
    }
}

#[repr(simd)] #[derive(Copy, Clone)] struct f32x3([f32; 3]);
#[repr(simd)] #[derive(Copy, Clone)] struct u8x3([u8; 3]);
#[repr(simd)] #[derive(Copy, Clone)] struct u8x1([u8; 1]);
#[repr(simd)] #[derive(Copy, Clone)] struct f64x8([f64; 8]);
struct Holder(u8, f32x3, u8x3);

fn work() {
    macro_rules! lay { ($($t:ty),*) => { d!("layout {:?}", [$((core::mem::size_of::<$t>(), core::mem::align_of::<$t>())),*]) } }
    lay!(f32x3, u8x3, u8x1, f64x8, i16x4, f32x16, Holder, [f32x3; 2]);
    unsafe {
        let a = f32x3([1.5, -2.0, 3.25]);
        let s: f32x3 = simd_add(a, a);
        let l: [f32; 3] = core::ptr::read(&s as *const f32x3 as *const [f32; 3]);
        d!("f32x3 {:?} {}", l, simd_reduce_add_ordered(s, 0.5f32));
    }
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    ints(&mut rng);
    floats(&mut rng);
}
