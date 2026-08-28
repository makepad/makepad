//! Math-AOT microbenchmark: splash interpreter vs StitchBackend vs
//! VirInterpBackend vs a native Rust closure, over 1M points.
//!
//!   cargo run -p makepad-script-test --bin mathaot_bench --release
//!
//! Expressions:
//! - scalar SDF: sin(x)*cos(z) - y + 0.3*sin(5x)*sin(5y)*sin(5z)
//! - vector-heavy: two-sphere min with normalize/dot/packed sin,
//!   in both vec3-parameter (packed) and hand-scalarized forms.

use makepad_script::math_aot::{MathAot, MathAotParam, MathAotValue, MathBackend, VirInterpBackend};
use makepad_script::makepad_math::Vec3f;
use makepad_script::*;
use std::time::Instant;

const N: usize = 1_000_000;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(0i32));
    let std = Box::leak(Box::new(0i32));
    ScriptVm {
        host,
        std,
        bx: Box::new(ScriptVmBase::new()),
    }
}

/// Evaluates the script and returns the fn value ROOTED (a bare eval
/// result is unrooted; a later eval could recycle its object slot).
fn eval_fn(vm: &mut ScriptVm, name: &str, code: &str) -> (ScriptFnRef, ScriptValue) {
    vm.bx.captured_errors = Some(Vec::new());
    let v = vm.eval(ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: name.into(),
        line: 0,
        column: 0,
        code: code.into(),
        values: vec![],
    });
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{errors:?}");
    let obj = v.as_object().expect("script did not yield a fn");
    (vm.bx.heap.new_fn_ref(obj), v)
}

fn points() -> Vec<f32> {
    let mut out = Vec::with_capacity(N * 3);
    let mut state = 0x12345678u32;
    for _ in 0..N * 3 {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        out.push((state >> 8) as f32 / (1 << 24) as f32 * 4.0 - 2.0);
    }
    out
}

fn time<F: FnMut()>(mut f: F) -> f64 {
    // Warmup + best of 3.
    f();
    let mut best = f64::MAX;
    for _ in 0..3 {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64());
    }
    best
}

fn checksum(out: &[f32]) -> f64 {
    out.iter().step_by(997).map(|v| *v as f64).sum()
}

fn main() {
    let mut vm = test_vm();
    let aot = MathAot::new(&mut vm);
    let pts = points();

    // ---- scalar SDF expression ------------------------------------------
    println!("== scalar SDF: sin(x)*cos(z) - y + 0.3*sin(5x)*sin(5y)*sin(5z), {N} points ==");
    let code = "use mod.math.*\nlet f = |x, y, z| sin(x) * cos(z) - y + 0.3 * sin(5 * x) * sin(5 * y) * sin(5 * z)\n(f)";
    let (_root, fn_value) = eval_fn(&mut vm, "bench_scalar", code);
    let t_compile = Instant::now();
    let virf = aot
        .to_vir(&vm, fn_value, &[MathAotParam::Scalar; 3], &[])
        .expect("in subset");
    let mut compiled = aot
        .compile(&vm, fn_value, &[MathAotParam::Scalar; 3], &[])
        .expect("in subset");
    println!("compile: {:.3} ms ({} VIR ops)", t_compile.elapsed().as_secs_f64() * 1e3, virf.ops.len());
    let vir_backend = VirInterpBackend;
    let vir_compiled = vir_backend.compile(&virf).unwrap();

    let mut out = vec![0f32; N];

    // Native closure mirroring the interpreter's semantics (f64 scalar
    // arithmetic, f32 trig roundtrips).
    let native = |x: f64, y: f64, z: f64| -> f64 {
        let s = |v: f64| (v as f32).sin() as f64;
        let c = |v: f64| (v as f32).cos() as f64;
        s(x) * c(z) - y + 0.3 * s(5.0 * x) * s(5.0 * y) * s(5.0 * z)
    };
    let t_native = time(|| {
        for i in 0..N {
            out[i] = native(
                pts[i * 3] as f64,
                pts[i * 3 + 1] as f64,
                pts[i * 3 + 2] as f64,
            ) as f32;
        }
    });
    println!("native closure:   {:8.2} ms  ({:6.1} ns/pt)  checksum {:.4}", t_native * 1e3, t_native / N as f64 * 1e9, checksum(&out));

    let t_stitch = time(|| compiled.eval_batch(&pts, &[], &mut out));
    println!("stitch AOT:       {:8.2} ms  ({:6.1} ns/pt)  checksum {:.4}", t_stitch * 1e3, t_stitch / N as f64 * 1e9, checksum(&out));

    let t_vir = time(|| vir_compiled.eval_batch(&pts, &[], &mut out));
    println!("VIR interp:       {:8.2} ms  ({:6.1} ns/pt)  checksum {:.4}", t_vir * 1e3, t_vir / N as f64 * 1e9, checksum(&out));

    // Splash interpreter over a subsample (per-point vm.call), scaled.
    let interp_n = 20_000;
    let t_interp = time(|| {
        for i in 0..interp_n {
            let r = vm.call(
                fn_value,
                &[
                    (pts[i * 3] as f64).into(),
                    (pts[i * 3 + 1] as f64).into(),
                    (pts[i * 3 + 2] as f64).into(),
                ],
            );
            out[i] = r.as_number().unwrap() as f32;
        }
    });
    let t_interp_scaled = t_interp / interp_n as f64 * N as f64;
    println!(
        "splash interp:    {:8.2} ms  ({:6.1} ns/pt)  [{} points, scaled]",
        t_interp_scaled * 1e3,
        t_interp / interp_n as f64 * 1e9,
        interp_n
    );
    println!(
        "ratios: interp/stitch = {:.1}x   stitch/native = {:.2}x   interp/native = {:.0}x",
        t_interp_scaled / t_stitch,
        t_stitch / t_native,
        t_interp_scaled / t_native
    );

    // ---- vector-heavy expression ----------------------------------------
    println!();
    println!("== vector-heavy: min(len(p-c1), len(p-c2)) - 0.8 + 0.05*dot(normalize(p), sin(p*4)), {N} points ==");
    let vcode = "use mod.math.*\nuse mod.pod.*\nlet f = |p| min(length(p - vec3(0.5, 0.2, 0.1)), length(p - vec3(0.0 - 0.4, 0.1, 0.0 - 0.3))) - 0.8 + 0.05 * dot(normalize(p), sin(p * 4.0))\n(f)";
    let (_vroot, vfn) = eval_fn(&mut vm, "bench_vec", vcode);
    let virf_v = aot.to_vir(&vm, vfn, &[MathAotParam::Vec3], &[]).expect("in subset");
    let mut compiled_v = aot.compile(&vm, vfn, &[MathAotParam::Vec3], &[]).expect("in subset");
    println!("packed VIR ops: {}", virf_v.ops.len());

    // The same expression hand-scalarized (what the compiler would do
    // without packed lanes).
    let scode = "use mod.math.*\nlet f = |x, y, z| {\n\
        let dx1 = x - 0.5\nlet dy1 = y - 0.2\nlet dz1 = z - 0.1\n\
        let dx2 = x + 0.4\nlet dy2 = y - 0.1\nlet dz2 = z + 0.3\n\
        let l1 = sqrt(dx1 * dx1 + dy1 * dy1 + dz1 * dz1)\n\
        let l2 = sqrt(dx2 * dx2 + dy2 * dy2 + dz2 * dz2)\n\
        let ln = sqrt(x * x + y * y + z * z)\n\
        let nx = x / ln\nlet ny = y / ln\nlet nz = z / ln\n\
        min(l1, l2) - 0.8 + 0.05 * (nx * sin(x * 4.0) + ny * sin(y * 4.0) + nz * sin(z * 4.0))\n}\n(f)";
    let (_sroot, sfn) = eval_fn(&mut vm, "bench_scalarized", scode);
    let virf_s = aot
        .to_vir(&vm, sfn, &[MathAotParam::Scalar; 3], &[])
        .expect("in subset");
    let mut compiled_s = aot
        .compile(&vm, sfn, &[MathAotParam::Scalar; 3], &[])
        .expect("in subset");
    println!("scalarized VIR ops: {}", virf_s.ops.len());

    let native_v = |x: f32, y: f32, z: f32| -> f64 {
        let l1 = ((x - 0.5) * (x - 0.5) + (y - 0.2) * (y - 0.2) + (z - 0.1) * (z - 0.1)).sqrt();
        let l2 = ((x + 0.4) * (x + 0.4) + (y - 0.1) * (y - 0.1) + (z + 0.3) * (z + 0.3)).sqrt();
        let ln = (x * x + y * y + z * z).sqrt();
        let d = (x / ln) * (x * 4.0).sin() + (y / ln) * (y * 4.0).sin() + (z / ln) * (z * 4.0).sin();
        (l1.min(l2) as f64) - 0.8 + 0.05 * d as f64
    };
    let t_native_v = time(|| {
        for i in 0..N {
            out[i] = native_v(pts[i * 3], pts[i * 3 + 1], pts[i * 3 + 2]) as f32;
        }
    });
    println!("native closure:   {:8.2} ms  ({:6.1} ns/pt)  checksum {:.4}", t_native_v * 1e3, t_native_v / N as f64 * 1e9, checksum(&out));

    let t_packed = time(|| compiled_v.eval_batch(&pts, &[], &mut out));
    println!("stitch packed:    {:8.2} ms  ({:6.1} ns/pt)  checksum {:.4}", t_packed * 1e3, t_packed / N as f64 * 1e9, checksum(&out));

    let t_scalar = time(|| compiled_s.eval_batch(&pts, &[], &mut out));
    println!("stitch scalarized:{:8.2} ms  ({:6.1} ns/pt)  checksum {:.4}", t_scalar * 1e3, t_scalar / N as f64 * 1e9, checksum(&out));

    let interp_n = 10_000;
    let t_interp_v = time(|| {
        for i in 0..interp_n {
            let p = Vec3f {
                x: pts[i * 3],
                y: pts[i * 3 + 1],
                z: pts[i * 3 + 2],
            }
            .script_to_value(&mut vm);
            let r = vm.call(vfn, &[p]);
            out[i] = r.as_number().unwrap() as f32;
        }
    });
    let t_interp_v_scaled = t_interp_v / interp_n as f64 * N as f64;
    println!(
        "splash interp:    {:8.2} ms  ({:6.1} ns/pt)  [{} points, scaled]",
        t_interp_v_scaled * 1e3,
        t_interp_v / interp_n as f64 * 1e9,
        interp_n
    );
    println!(
        "ratios: interp/packed = {:.1}x   packed/native = {:.2}x   scalarized/packed = {:.2}x",
        t_interp_v_scaled / t_packed,
        t_packed / t_native_v,
        t_scalar / t_packed
    );
    let _ = MathAotValue::Scalar(0.0);
}
