//! Kernel smoke tests: compile, run natively and interpreted (bit-equal),
//! parallel = serial, reduce, emit, layouts, errors.

use makepad_script_compute::kernel::{compile, compile_with, compact, FieldTy, Layout, LayoutField};
use makepad_script_compute::Backend;

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

const HEIGHTFIELD: &str = r#"
    let W = 256
    let pos = output(vec3)
    let nrm = output(vec3)
    let amp = param(8.0)
    fn height(x, z) { fbm2(vec2(x, z) * 0.02, 5, 2.0, 0.5) * amp + ridged2(vec2(z, x) * 0.01, 3) * amp }
    fn vertex(i) {
        let x = float(i % W)
        let z = float(i / W)
        let h = height(x, z)
        pos[i] = vec3(x, h, z)
        let dx = height(x + 1.0, z) - height(x - 1.0, z)
        let dz = height(x, z + 1.0) - height(x, z - 1.0)
        nrm[i] = normalize(vec3(-dx, 2.0, -dz))
    }
"#;

#[test]
fn heightfield_native_equals_interpreter_and_parallel() {
    let k = compile(HEIGHTFIELD).unwrap_or_else(|e| panic!("{:?}", e));
    let n = 256 * 64;
    let run = |mode: u8| {
        let mut pos = vec![0.0f32; n * 3];
        let mut nrm = vec![0.0f32; n * 3];
        let mut c = k.call();
        c.set_param("amp", 6.0);
        c.output("pos", &mut pos).unwrap();
        c.output("nrm", &mut nrm).unwrap();
        match mode {
            0 => c.run(n).unwrap(),
            1 => c.run_interp(n).unwrap(),
            _ => c.run_parallel(n, 4).unwrap(),
        };
        (pos, nrm)
    };
    let a = run(0);
    let b = run(1);
    let p = run(2);
    assert_eq!(bits(&a.0), bits(&b.0));
    assert_eq!(bits(&a.1), bits(&b.1));
    assert_eq!(bits(&a.0), bits(&p.0));
    assert!(a.0.iter().all(|x| x.is_finite()));
    assert!(k.parallel_safe);
}

#[test]
fn reduce_emit_and_layouts() {
    let sum = compile("let v = input(f32)\nfn reduce_sum(i) { v[i] * 2.0 }").unwrap();
    let data: Vec<f32> = (0..10000).map(|i| i as f32 * 0.001).collect();
    let mut c = sum.call();
    c.input("v", &data).unwrap();
    let serial = c.run(data.len()).unwrap().reduced;
    let mut c = sum.call();
    c.input("v", &data).unwrap();
    let parallel = c.run_parallel(data.len(), 3).unwrap().reduced;
    assert_eq!(bits(&serial), bits(&parallel));
    assert!((serial[0] - 99990.0).abs() < 1.0);

    // Emit: element i emits i % 3 records of 2 words.
    let em = compile("let seg = emit_buffer(2, 4)\nfn primitive(i) { for k in 0..(i % 3) { emit(seg, float(i), float(k)) } }").unwrap();
    let n = 10;
    let mut data = vec![0.0f32; n * 4 * 2];
    let mut counts = vec![0u32; n];
    let mut c = em.call();
    c.output("seg", &mut data).unwrap();
    c.output_u32("seg_count", &mut counts).unwrap();
    c.run(n).unwrap();
    let flat = compact(&data, &counts, 2, 4);
    assert_eq!(flat.len(), 2 * (0 + 1 + 2 + 0 + 1 + 2 + 0 + 1 + 2 + 0));
    assert_eq!(&flat[..6], &[1.0, 0.0, 2.0, 0.0, 2.0, 1.0]);

    // A host layout (a GPU instance struct): fields by name at offsets.
    let layout = Layout {
        name: "Inst".into(),
        stride: 8,
        fields: vec![
            LayoutField { name: "pos".into(), ty: FieldTy::Vec3, offset: 0 },
            LayoutField { name: "phase".into(), ty: FieldTy::F32, offset: 4 },
            LayoutField { name: "id".into(), ty: FieldTy::I32, offset: 5 },
        ],
    };
    let src = "let inst = output(Inst)\nfn instance(i) { inst[i].pos = vec3(float(i), 1.0, 2.0)\n inst[i].phase = 0.5\n inst[i].id = i * 2 }";
    let k = compile_with(src, &[layout.clone()], Backend::Native).unwrap();
    let mut out = vec![0.0f32; 8 * 3];
    let mut c = k.call();
    c.output("inst", &mut out).unwrap();
    c.run(3).unwrap();
    assert_eq!(&out[8..14], &[1.0, 1.0, 2.0, 0.0, 0.5, f32::from_bits(2)]);
    let e = compile_with("let inst = output(Inst)\nfn instance(i) { inst[i].speed = 1.0 }", &[layout], Backend::Native).unwrap_err();
    assert!(e[0].message.contains("has no field `speed`"), "{}", e[0].message);
}

#[test]
fn out_of_range_accesses_clamp_never_escape() {
    // Hostile indices: huge, negative, computed; every access stays inside.
    let k = compile("let src = input(f32)\nlet dst = output(f32)\nfn element(i) { dst[i * 7919 - 100000] = src[-i * 1000003] + src[2147483647] }").unwrap();
    let src = vec![1.0f32; 5];
    let mut dst = vec![0.0f32; 3];
    let mut guard = vec![0.0f32; 16];
    let mut c = k.call();
    c.input("src", &src).unwrap();
    c.output("dst", &mut dst).unwrap();
    c.run(1000).unwrap();
    assert!(dst.iter().all(|x| *x == 0.0 || *x == 2.0));
    guard[0] = 1.0;
    assert!(!k.parallel_safe, "non-local writes must not be split across threads");
}
