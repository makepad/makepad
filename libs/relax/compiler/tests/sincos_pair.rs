// A sin and a cos of the same value in one block: rustc's LLVM (Darwin) combines them into one
// call of libm's `__sincosf_stret` / `__sincos_stret`, whose results differ from separate
// `sinf` / `cosf` calls in the last bit for about 3% of the arguments. Relax pairs them the same
// way (opt::fuse_sincos), so the printed checksums match rustc on every target. A lone call stays a call.
use std::cell::Cell;
use std::hint::black_box;

fn fold(h: u64, v: u64) -> u64 {
    (h.rotate_left(5) ^ v).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

fn pair32(x: f32) -> (f32, f32) {
    (x.cos(), x.sin())
}

fn pair64(x: f64) -> (f64, f64) {
    (x.sin(), x.cos())
}

// a store with a bounds check between the two: the same block once the check is proven away
fn table(out: &mut [f32; 8], x: f32, i: usize) {
    out[i & 7] = x.cos();
    out[(i + 1) & 7] = x.sin();
}

// a closure that reads its captured angle for a cos and again for a sin (a patchmatch's random
// normal): rustc's LLVM reads it once, as `llvm.cos` writes no memory, and pairs the calls, in
// the closure's own build and inlined alike. Relax took the libm call for a write: two reads, two
// calls in the closure's build, one once inlined, and the bits changed when the code tiered up.
fn rotate(axis: [f64; 3], tangent: [f64; 3], bitangent: [f64; 3], angle: f64) -> [f64; 3] {
    std::array::from_fn(|i| axis[i] * 0.5 + 0.25 * (tangent[i] * angle.cos() + bitangent[i] * angle.sin()))
}

// what may write the angle between the two reads keeps them two values, and the calls apart (rustc
// pairs no more than that): an unknown call ...
fn apart_call(a: &Cell<f64>, hook: fn(&Cell<f64>)) -> (f64, f64) {
    let c = a.get().cos();
    hook(a);
    (c, a.get().sin())
}

fn keep(_: &Cell<f64>) {}

// ... or a store (of the same value, which only the run knows)
fn apart_store(a: &Cell<f64>, v: f64) -> (f64, f64) {
    let c = a.get().cos();
    a.set(v);
    (c, a.get().sin())
}

// the program's own C sin and cos: LLVM keeps them calls (they may write errno) and pairs them
// nowhere, though the float methods' sin and cos of the same value pair
mod c_math {
    extern "C" {
        pub fn sin(x: f64) -> f64;
        pub fn cos(x: f64) -> f64;
    }
}

fn own_pair(x: f64) -> (f64, f64) {
    unsafe { (c_math::cos(x), c_math::sin(x)) }
}

#[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
mod native {
    #[repr(C)]
    pub struct Sc32 {
        pub sin: f32,
        pub cos: f32,
    }
    #[repr(C)]
    pub struct Sc64 {
        pub sin: f64,
        pub cos: f64,
    }
    extern "C" {
        pub fn __sincosf_stret(x: f32) -> Sc32;
        pub fn __sincos_stret(x: f64) -> Sc64;
    }
}

fn run() {
    let (mut a, mut b, mut c, mut d) = (0u64, 0u64, 0u64, 0u64);
    let mut t = [0.0f32; 8];
    let hook: fn(&Cell<f64>) = black_box(keep);
    // enough calls for the pair to be compiled by the optimiser in every tier
    for round in 0..3 {
        for i in 0..4000 {
            let x = black_box(i as f32 * 0.00123 - 2.0);
            let (co, si) = pair32(x);
            let (s64, c64) = pair64(x as f64 * 3.1);
            table(&mut t, x, i);
            // every round: the first runs the first builds, the later ones the tiered-up code
            let angle = x as f64 * 1.7 + 0.3;
            let (axis, tangent, bitangent) = black_box(([0.3, -0.5, 0.8], [0.6, 0.2, -0.4], [-0.1, 0.9, 0.35]));
            let n = rotate(axis, tangent, bitangent, angle);
            let cell = Cell::new(angle);
            let (c1, s1) = apart_call(&cell, hook);
            let (c2, s2) = apart_store(&cell, black_box(angle));
            let (c3, s3) = own_pair(angle);
            d = fold(d, n.iter().fold(c1.to_bits() ^ s1.to_bits().rotate_left(3) ^ c2.to_bits().rotate_left(11) ^ s2.to_bits().rotate_left(17), |h, v| fold(h, v.to_bits())));
            d = fold(d, c3.to_bits() ^ s3.to_bits().rotate_left(23));
            #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
            {
                let r = unsafe { native::__sincos_stret(angle) };
                let want: [f64; 3] = std::array::from_fn(|k| axis[k] * 0.5 + 0.25 * (tangent[k] * r.cos + bitangent[k] * r.sin));
                assert!(n.map(f64::to_bits) == want.map(f64::to_bits), "round {round}: the captured angle's sin and cos at {angle} are not __sincos_stret's");
                // separate calls (opaque operands: neither compiler pairs these)
                let (sn, cs) = unsafe { (c_math::sin(black_box(angle)), c_math::cos(black_box(angle))) };
                assert!(c1.to_bits() == cs.to_bits() && s1.to_bits() == sn.to_bits(), "round {round}: a call between the reads, yet sin and cos at {angle} were paired");
                assert!(c2.to_bits() == cs.to_bits() && s2.to_bits() == sn.to_bits(), "round {round}: a store between the reads, yet sin and cos at {angle} were paired");
                assert!(c3.to_bits() == cs.to_bits() && s3.to_bits() == sn.to_bits(), "round {round}: the program's own extern \"C\" sin and cos at {angle} were paired");
            }
            if round == 2 {
                a = fold(a, co.to_bits() as u64 ^ (si.to_bits() as u64) << 32);
                b = fold(b, s64.to_bits() ^ c64.to_bits().rotate_left(7));
                c = fold(c, t[i & 7].to_bits() as u64 ^ (t[(i + 1) & 7].to_bits() as u64) << 32);
                #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
                {
                    let r = unsafe { native::__sincosf_stret(x) };
                    assert!(r.sin.to_bits() == si.to_bits() && r.cos.to_bits() == co.to_bits(), "pair at {} is not __sincosf_stret", x);
                }
            }
        }
    }
    println!("{a:016x} {b:016x} {c:016x} {d:016x}");
}

#[test]
fn sin_cos_pairs_are_combined_like_rustc() {
    run();
}

fn main() {
    run();
}
