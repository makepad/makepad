//! Op-time bench: the worst ns per AIR op over hostile op mixes, and the
//! cost of a dependent load that misses every cache (the yardstick
//! `tests/host_hostile.rs` holds host components' declared costs to;
//! budgets themselves count ops). The cancellation latency at the admitted
//! ceiling is measured by `tests/admission.rs` (`--nocapture` prints it).
//!
//! cargo run --release -p makepad-script-compute --example bench_admission

use makepad_script_compute::kernel::compile_with;
use makepad_script_compute::Backend;
use std::time::Instant;

const MIXES: &[(&str, &str)] = &[
    ("fdiv chain", "let s = float(i) + 1.5\n for a in 0..1000 { s = 1.0 / (s + 1.5) }\n o[i] = s"),
    ("fsqrt chain", "let s = float(i) + 1.5\n for a in 0..1000 { s = sqrt(s + 2.0) }\n o[i] = s"),
    ("idiv chain", "let s = i + 12345\n for a in 0..1000 { s = s / 3 + s % 7 + 1000003 }\n o[i] = float(s)"),
    ("sin chain", "let s = float(i)\n for a in 0..1000 { s = sin(s) + 0.5 }\n o[i] = s"),
    ("random loads", "let s = i\n for a in 0..1000 { s = int(src[(s * 7919 + a) % 65536]) + s }\n o[i] = float(s)"),
    ("fbm", "let s = 0.0\n for a in 0..100 { s = s + fbm2(vec2(float(i), float(a)) * 0.01, 4, 2.0, 0.5) }\n o[i] = s"),
];

/// Dependent loads over a 64 MiB buffer (a random cycle): every load is a
/// cache and TLB miss, the slowest thing a kernel op can be.
fn pointer_chase() {
    let n = 1usize << 24;
    let mut perm: Vec<u32> = (0..n as u32).collect();
    let mut x = 0x2545F491u64;
    for k in (1..n).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        perm.swap(k, (x % k as u64) as usize);
    }
    let mut next = vec![0f32; n];
    for k in 0..n {
        next[perm[k] as usize] = perm[(k + 1) % n] as f32;
    }
    drop(perm);
    for backend in [Backend::Native, Backend::Interp] {
        let code = "let src = input(f32)\nlet o = output(f32)\nfn element(i) {\n let s = i * 977\n for a in 0..1000 { s = int(src[s]) }\n o[i] = float(s)\n }";
        let k = compile_with(code, &[], backend).unwrap();
        let m = 200;
        let mut out = vec![0.0f32; m];
        let mut c = k.call();
        c.input("src", &next).unwrap();
        c.output("o", &mut out).unwrap();
        let t0 = Instant::now();
        c.run(m).unwrap();
        let s = t0.elapsed().as_secs_f64();
        println!("{:?} pointer chase 64 MiB: {:6.1} ns/load  {:6.3} ns/op (cost {})", backend, s * 1e9 / (m as f64 * 1000.0), s * 1e9 / (m as f64 * k.cost as f64), k.cost);
    }
}

fn main() {
    pointer_chase();
    let src: Vec<f32> = (0..65536).map(|k| (k % 997) as f32).collect();
    for backend in [Backend::Native, Backend::Interp] {
        for (name, body) in MIXES {
            let code = format!("let src = input(f32)\nlet o = output(f32)\nfn element(i) {{\n {}\n }}", body);
            let k = compile_with(&code, &[], backend).unwrap_or_else(|e| panic!("{}: {:?}", name, e));
            let n = if backend == Backend::Native { 2000 } else { 100 };
            let mut out = vec![0.0f32; n];
            let mut best = f64::MAX;
            for _ in 0..3 {
                let mut c = k.call();
                c.input("src", &src).unwrap();
                c.output("o", &mut out).unwrap();
                let t0 = Instant::now();
                c.run(n).unwrap();
                best = best.min(t0.elapsed().as_secs_f64());
            }
            let ns_op = best * 1e9 / (k.cost as f64 * n as f64);
            println!("{:?} {:14} cost {:8} ops/elem  {:6.3} ns/op  {:8.1} us/elem", backend, name, k.cost, ns_op, best * 1e6 / n as f64);
        }
    }
}
