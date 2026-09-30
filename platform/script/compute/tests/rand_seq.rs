//! The kernel prelude's `rand_seq` is the documents' draw (src/rand.rs) to 24 bits.

use makepad_script_compute::kernel::compile;
use makepad_script_compute::rand::rand_seq;

#[test]
fn kernel_draws_equal_the_document_draws_to_24_bits() {
    let k = compile("let o = output(vec4)\nfn element(i) { o[i] = vec4(rand_seq(0, i), rand_seq(42, i), rand_seq(1337, i * 97), rand_seq(4000000, i)) }").unwrap();
    let n = 4096;
    for interp in [false, true] {
        let mut out = vec![0.0f32; n * 4];
        let mut c = k.call();
        c.output("o", &mut out).unwrap();
        if interp { c.run_interp(n).unwrap() } else { c.run(n).unwrap() };
        for i in 0..n {
            let want = [rand_seq(0.0, i as f64), rand_seq(42.0, i as f64), rand_seq(1337.0, (i * 97) as f64), rand_seq(4000000.0, i as f64)];
            for (k, w) in want.iter().enumerate() {
                let w24 = (w * 16777216.0).floor() / 16777216.0;
                assert_eq!(out[i * 4 + k], w24 as f32, "element {i} lane {k} interp {interp}");
            }
        }
    }
}

/// The shared stdlib's noise, hash and colour helpers compile into kernels.
#[test]
fn shared_noise_and_colour_compile_in_kernels() {
    let k = compile("let o = output(vec4)\nfn element(i) { let p = vec2(float(i) * 0.37, float(i) * 0.11)\n o[i] = vec4(snoise2(p), hash12(p), hash22(p).y, srgb_to_linear(vec3(0.5, 0.5, 0.5)).x + linear_to_srgb(vec3(0.2, 0.2, 0.2)).y) }").unwrap();
    let n = 256;
    let mut out = vec![0.0f32; n * 4];
    let mut c = k.call();
    c.output("o", &mut out).unwrap();
    c.run(n).unwrap();
    for i in 0..n {
        let v = &out[i * 4..i * 4 + 4];
        assert!(v[0].abs() <= 1.0 && (0.0..1.0).contains(&v[1]) && (0.0..1.0).contains(&v[2]), "element {i}: {v:?}");
        assert!((v[3] - (0.21404 + 0.48453)).abs() < 1e-3, "element {i}: {}", v[3]);
    }
}
