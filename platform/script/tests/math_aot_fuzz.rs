//! Layer: fuzz (differential).
//!
//! Generates random pure-math splash expressions (scalars + vec3s over
//! the whole taught intrinsic set), then requires, for every random
//! input tuple:
//!
//!   splash interpreter == StitchBackend == VirInterpBackend
//!
//! bit-for-bit (NaN outputs compare as NaN-class: the interpreter boxes
//! NaN results with a source-trace payload). The batch entry is also
//! checked against the single-call entry per expression.
//!
//! The committed run is deterministic (seeds 1..=FUZZ_EXPRS). For a
//! larger sweep set MATH_AOT_FUZZ_EXPRS, e.g.:
//!   MATH_AOT_FUZZ_EXPRS=5000 cargo test --release --test math_aot_fuzz
//! (a 5000-expression run is recorded in the mathaot report).

use makepad_script::math_aot::vir;
use makepad_script::math_aot::{
    MathAot, MathAotParam, MathAotValue, MathBackend, StitchBackend, VirInterpBackend,
};
use makepad_script::makepad_math::Vec3f;
use makepad_script::*;

const FUZZ_EXPRS: u64 = 600;
const INPUTS_PER_EXPR: usize = 6;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host,
        bx: Box::new(ScriptVmBase::new()),
    }
}

/// xorshift64* — deterministic, dependency-free.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn f64(&mut self) -> f64 {
        // Mix of magnitudes and specials.
        match self.below(12) {
            0 => 0.0,
            1 => -0.0,
            2 => 1.0,
            3 => -1.0,
            4 => (self.below(2000) as f64 - 1000.0) / 64.0,
            5 => (self.below(2000) as f64 - 1000.0) * 64.0,
            _ => (self.below(1_000_000) as f64 - 500_000.0) / 32768.0,
        }
    }
}

/// Generates a scalar-valued expression over params `x`, `y` (scalars)
/// and `p`, `q` (vec3s).
fn gen_scalar(rng: &mut Rng, depth: u32) -> String {
    if depth == 0 {
        return match rng.below(7) {
            0 => "x".into(),
            1 => "y".into(),
            6 => "u".into(),
            2 => format!("{:.4}", (rng.below(4000) as f64 - 2000.0) / 256.0),
            3 => format!("{}", rng.below(9)), // integer: parser inline-fusion path
            4 => "p.x".into(),
            5 => ["p.y", "p.z", "q.x", "q.y", "q.z"][rng.below(5) as usize].into(),
            _ => unreachable!(),
        };
    }
    let d = depth - 1;
    match rng.below(22) {
        0 => format!("({} + {})", gen_scalar(rng, d), gen_scalar(rng, d)),
        1 => format!("({} - {})", gen_scalar(rng, d), gen_scalar(rng, d)),
        2 => format!("({} * {})", gen_scalar(rng, d), gen_scalar(rng, d)),
        3 => format!("({} / {})", gen_scalar(rng, d), gen_scalar(rng, d)),
        4 => format!("({} % {})", gen_scalar(rng, d), gen_scalar(rng, d)),
        5 => format!("(-{})", gen_scalar(rng, d)),
        6 => {
            let f = ["sin", "cos", "tan", "asin", "acos", "atan", "exp", "log", "sqrt",
                     "abs", "floor", "ceil", "fract"][rng.below(13) as usize];
            format!("{}({})", f, gen_scalar(rng, d))
        }
        7 => {
            let f = ["min", "max", "pow", "atan2", "step", "modf"][rng.below(6) as usize];
            format!("{}({}, {})", f, gen_scalar(rng, d), gen_scalar(rng, d))
        }
        8 => format!(
            "clamp({}, {}, {})",
            gen_scalar(rng, d),
            gen_scalar(rng, d),
            gen_scalar(rng, d)
        ),
        9 => format!(
            "mix({}, {}, {})",
            gen_scalar(rng, d),
            gen_scalar(rng, d),
            gen_scalar(rng, d)
        ),
        10 => format!(
            "smoothstep({}, {}, {})",
            gen_scalar(rng, d),
            gen_scalar(rng, d),
            gen_scalar(rng, d)
        ),
        11 => format!("dot({}, {})", gen_vec(rng, d), gen_vec(rng, d)),
        12 => format!("length({})", gen_vec(rng, d)),
        13 => format!("distance({}, {})", gen_vec(rng, d), gen_vec(rng, d)),
        14 => {
            let cc = ["<", ">", "<=", ">="][rng.below(4) as usize];
            format!(
                "if {} {} {} {{ {} }} else {{ {} }}",
                gen_scalar(rng, d),
                cc,
                gen_scalar(rng, d),
                gen_scalar(rng, d),
                gen_scalar(rng, d)
            )
        }
        15 => format!("({} && {})", gen_scalar(rng, d), gen_scalar(rng, d)),
        16 => format!("({} || {})", gen_scalar(rng, d), gen_scalar(rng, d)),
        17 => format!("{}.length()", gen_vec(rng, d)),
        18 => format!("{}.dot({})", gen_vec(rng, d), gen_vec(rng, d)),
        19 => {
            let sw = ["x", "y", "z"][rng.below(3) as usize];
            format!("{}.{}", gen_vec(rng, d), sw)
        }
        20 => format!("lerp({}, {}, {})", gen_scalar(rng, d), gen_scalar(rng, d), gen_scalar(rng, d)),
        21 => format!("length(cross({}, {}))", gen_vec(rng, d), gen_vec(rng, d)),
        _ => unreachable!(),
    }
}

/// Generates a vec3-valued expression.
fn gen_vec(rng: &mut Rng, depth: u32) -> String {
    if depth == 0 {
        return if rng.below(2) == 0 { "p".into() } else { "q".into() };
    }
    let d = depth - 1;
    match rng.below(15) {
        0 => format!("({} + {})", gen_vec(rng, d), gen_vec(rng, d)),
        1 => format!("({} - {})", gen_vec(rng, d), gen_vec(rng, d)),
        2 => format!("({} * {})", gen_vec(rng, d), gen_vec(rng, d)),
        3 => format!("({} / {})", gen_vec(rng, d), gen_vec(rng, d)),
        4 => format!("({} * {})", gen_vec(rng, d), gen_scalar(rng, d)),
        5 => format!("({} * {})", gen_scalar(rng, d), gen_vec(rng, d)),
        6 => format!("(-{})", gen_vec(rng, d)),
        7 => {
            let f = ["sin", "cos", "abs", "floor", "ceil", "fract"][rng.below(6) as usize];
            format!("{}({})", f, gen_vec(rng, d))
        }
        8 => format!("normalize({})", gen_vec(rng, d)),
        9 => format!("cross({}, {})", gen_vec(rng, d), gen_vec(rng, d)),
        10 => {
            let f = ["min", "max"][rng.below(2) as usize];
            format!("{}({}, {})", f, gen_vec(rng, d), gen_vec(rng, d))
        }
        11 => format!(
            "mix({}, {}, {})",
            gen_vec(rng, d),
            gen_vec(rng, d),
            gen_scalar(rng, d)
        ),
        12 => {
            let sw = ["zyx", "xxy", "yzx", "zzz", "xyz", "yx"][rng.below(6) as usize];
            if sw.len() == 2 {
                // A vec2 swizzle immediately widened again is not valid
                // vec3 math; use a 3-lane swizzle instead.
                format!("{}.zxy", gen_vec(rng, d))
            } else {
                format!("{}.{}", gen_vec(rng, d), sw)
            }
        }
        13 => format!(
            "vec3({}, {}, {})",
            gen_scalar(rng, d),
            gen_scalar(rng, d),
            gen_scalar(rng, d)
        ),
        14 => format!(
            "clamp({}, {}, {})",
            gen_vec(rng, d),
            gen_scalar(rng, d),
            gen_scalar(rng, d)
        ),
        _ => unreachable!(),
    }
}

fn bits_match(a: f64, b: f64) -> bool {
    (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits()
}

#[test]
fn differential_fuzz() {
    let exprs: u64 = std::env::var("MATH_AOT_FUZZ_EXPRS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(FUZZ_EXPRS);
    let mut vm = test_vm();
    let aot = MathAot::new(&mut vm);
    let stitch = StitchBackend::new();
    let vir_interp = VirInterpBackend;
    let params = [
        MathAotParam::Scalar,
        MathAotParam::Scalar,
        MathAotParam::Vec3,
        MathAotParam::Vec3,
    ];

    let mut accepted = 0u64;
    let mut rejected = 0u64;
    for seed in 1..=exprs {
        let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
        let depth = 2 + (seed % 3) as u32;
        let expr = gen_scalar(&mut rng, depth);
        let code = format!("use mod.math.*\nuse mod.pod.*\nlet f = |x, y, p, q, u| {expr}\n(f)");

        vm.bx.captured_errors = Some(Vec::new());
        let fn_value = vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: format!("fuzz_{seed}"),
            line: 0,
            column: 0,
            code: code.clone(),
            values: vec![],
        });
        let errors = vm.take_errors();
        assert!(errors.is_empty(), "seed {seed} script errors: {errors:?}\n{code}");
        assert!(fn_value.as_object().is_some(), "seed {seed}: no fn\n{code}");

        let Some(virf) = aot.to_vir(&vm, fn_value, &params, &[MathAotParam::Scalar]) else {
            rejected += 1;
            continue;
        };
        accepted += 1;
        let compiled_stitch = stitch.compile(&virf).expect("stitch backend");
        let compiled_ref = vir_interp.compile(&virf).expect("vir interp backend");

        for _ in 0..INPUTS_PER_EXPR {
            let x = rng.f64();
            let y = rng.f64();
            let p = [rng.f64() as f32, rng.f64() as f32, rng.f64() as f32];
            let q = [rng.f64() as f32, rng.f64() as f32, rng.f64() as f32];
            let u = rng.f64() as f32;
            let args = [
                MathAotValue::Scalar(x),
                MathAotValue::Scalar(y),
                MathAotValue::Vec3(p),
                MathAotValue::Vec3(q),
                MathAotValue::Scalar(u as f64),
            ];
            let script_args: Vec<ScriptValue> = vec![
                x.into(),
                y.into(),
                Vec3f {
                    x: p[0],
                    y: p[1],
                    z: p[2],
                }
                .script_to_value(&mut vm),
                Vec3f {
                    x: q[0],
                    y: q[1],
                    z: q[2],
                }
                .script_to_value(&mut vm),
                (u as f64).into(),
            ];
            let interp = vm.call(fn_value, &script_args);
            let interp = interp.as_number().unwrap_or_else(|| {
                panic!("seed {seed}: interpreter returned non-number\n{code}")
            });
            let aot_stitch = compiled_stitch.call(&args).expect("stitch call");
            let aot_ref = compiled_ref.call(&args).expect("ref call");
            assert!(
                bits_match(aot_stitch, interp),
                "seed {seed} STITCH mismatch\n{code}\nargs x={x:?} y={y:?} p={p:?} q={q:?}\ninterp {interp:?} ({:#x})  stitch {aot_stitch:?} ({:#x})",
                interp.to_bits(),
                aot_stitch.to_bits()
            );
            assert!(
                bits_match(aot_ref, interp),
                "seed {seed} VIR-INTERP mismatch\n{code}\nargs x={x:?} y={y:?} p={p:?} q={q:?}\ninterp {interp:?} ({:#x})  vir {aot_ref:?} ({:#x})",
                interp.to_bits(),
                aot_ref.to_bits()
            );
        }

        // Batch-vs-call parity on a few points (both backends).
        let n = 5;
        let mut input = Vec::new();
        let mut rng2 = Rng(seed.wrapping_mul(0xD1342543DE82EF95) | 1);
        for _ in 0..n {
            for _ in 0..virf.stride() {
                input.push(rng2.f64() as f32);
            }
        }
        let ub = [rng2.f64() as f32];
        let mut out_stitch = vec![0f32; n];
        let mut out_ref = vec![0f32; n];
        compiled_stitch.eval_batch(&input, &ub, &mut out_stitch);
        compiled_ref.eval_batch(&input, &ub, &mut out_ref);
        for i in 0..n {
            let s = i * virf.stride();
            let args = [
                MathAotValue::Scalar(input[s] as f64),
                MathAotValue::Scalar(input[s + 1] as f64),
                MathAotValue::Vec3([input[s + 2], input[s + 3], input[s + 4]]),
                MathAotValue::Vec3([input[s + 5], input[s + 6], input[s + 7]]),
                MathAotValue::Scalar(ub[0] as f64),
            ];
            let expected = compiled_stitch.call(&args).unwrap() as f32;
            let sb = out_stitch[i];
            let rb = out_ref[i];
            let ok_s = (sb.is_nan() && expected.is_nan()) || sb.to_bits() == expected.to_bits();
            let ok_r = (rb.is_nan() && expected.is_nan()) || rb.to_bits() == expected.to_bits();
            assert!(ok_s, "seed {seed} batch/stitch point {i}\n{code}");
            assert!(ok_r, "seed {seed} batch/vir point {i}\n{code}");
        }
    }

    println!("differential fuzz: {accepted} accepted, {rejected} rejected");
    // The generator emits only subset constructs; a high rejection rate
    // would mean the fuzz has stopped covering the compiler.
    assert!(
        accepted * 5 >= (accepted + rejected) * 4,
        "acceptance too low: {accepted} accepted, {rejected} rejected"
    );
    // Silence unused-warning when the vir module is only used via to_vir.
    let _ = vir::VirTy::F64;
}
