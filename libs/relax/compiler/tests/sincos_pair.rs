// A sin and a cos of the same value in one block: rustc's LLVM (Darwin) combines them into one
// call of libm's `__sincosf_stret` / `__sincos_stret`, whose results differ from separate
// `sinf` / `cosf` calls in the last bit for about 3% of the arguments. Relax pairs them the same
// way (opt::fuse_sincos), so the printed checksums match rustc on every target. A lone call stays a call.
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

#[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
mod native {
    #[repr(C)]
    pub struct Sc32 {
        pub sin: f32,
        pub cos: f32,
    }
    extern "C" {
        pub fn __sincosf_stret(x: f32) -> Sc32;
    }
}

fn run() {
    let (mut a, mut b, mut c) = (0u64, 0u64, 0u64);
    let mut t = [0.0f32; 8];
    // enough calls for the pair to be compiled by the optimiser in every tier
    for round in 0..3 {
        for i in 0..4000 {
            let x = black_box(i as f32 * 0.00123 - 2.0);
            let (co, si) = pair32(x);
            let (s64, c64) = pair64(x as f64 * 3.1);
            table(&mut t, x, i);
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
    println!("{a:016x} {b:016x} {c:016x}");
}

#[test]
fn sin_cos_pairs_are_combined_like_rustc() {
    run();
}

fn main() {
    run();
}
