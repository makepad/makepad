//! Helpers lowered as functions (big, called from several places): the
//! element's own state (its index, the output record's address, its
//! locals) survives the calls on every backend.

use makepad_script_compute::kernel::{compile_with, FieldTy, Layout, LayoutField};
use makepad_script_compute::Backend;

fn segment() -> Layout {
    Layout {
        name: "Segment".into(),
        stride: 11,
        fields: vec![
            LayoutField { name: "a".into(), ty: FieldTy::Vec3, offset: 0 },
            LayoutField { name: "b".into(), ty: FieldTy::Vec3, offset: 3 },
            LayoutField { name: "color".into(), ty: FieldTy::Vec4, offset: 6 },
            LayoutField { name: "width".into(), ty: FieldTy::F32, offset: 10 },
        ],
    }
}

const SRC: &str = r#"
let out = output(Segment)
fn step(h, x) {
    let k = (h ^ (int(x) * 1000003)) * 16777619
    let m = (k ^ (k >>> 13)) * 0x5bd1e995
    m ^ (m >>> 15)
}
fn big(a, b, c) { float(step(step(step(step(step(step(0x811C9DC5, a), b), c), a + b), b + c), a + c) >>> 8) * (1.0 / 16777216.0) }
fn element(i) {
    let x = float(i) * 0.37 - 9.1
    let y = float(i) * 0.53 + 2.2
    let w = big(floor(x), floor(y), 5.0) + big(floor(x) + 1.0, floor(y), 5.0) + big(floor(x), floor(y) + 1.0, 5.0)
    out[i].a = vec3(100.0, 100.0 + float(i), 0.0)
    out[i].b = vec3(101.0, 100.0, 0.0)
    out[i].color = vec4(1.0, 0.5, 0.25, 1.0)
    out[i].width = 5.0 + w
}
"#;

#[test]
fn records_survive_function_calls() {
    let n = 9;
    for backend in [Backend::Native, Backend::Interp] {
        let k = compile_with(SRC, &[segment()], backend).unwrap_or_else(|e| panic!("{:?}", e));
        assert!(!k.program().funcs.is_empty(), "the helper is a function");
        for mode in 0..3 {
            let mut out = vec![-1.0f32; 11 * n];
            let mut c = k.call();
            c.output("out", &mut out).unwrap();
            match mode {
                0 => c.run(n).unwrap(),
                1 => c.run_interp(n).unwrap(),
                _ => c.run_parallel(n, 2).unwrap(),
            };
            for (i, r) in out.chunks_exact(11).enumerate() {
                assert_eq!(&r[..10], &[100.0, 100.0 + i as f32, 0.0, 101.0, 100.0, 0.0, 1.0, 0.5, 0.25, 1.0], "record {i} mode {mode}: {r:?}");
                assert!(r[10] >= 5.0 && r[10] < 8.0, "record {i} mode {mode}: {r:?}");
            }
        }
    }
}
