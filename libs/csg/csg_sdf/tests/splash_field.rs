//! Layer: integration (golden).
//!
//! The sphere expression `length(p) - 1.0`, compiled by the splash math
//! AOT and meshed through dual contouring, must produce:
//!
//! 1. EXACTLY the mesh of a native Rust field mirroring the interpreter's
//!    f32 semantics for that expression (bit-identical vertices) — this
//!    pins the whole compile+batch pipeline into the mesher.
//! 2. The analytic (f64) sphere field's mesh within f32 field precision —
//!    same triangle count, vertices within 1e-4.

use makepad_csg_math::Vec3d;
use makepad_csg_sdf::{sdf_to_mesh, Sdf3, SdfSphere, SdfSplashExpr};
use makepad_script::math_aot::{MathAot, MathAotParam, MathAotValue};
use makepad_script::*;

fn make_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host,
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn compile_sphere_field() -> SdfSplashExpr {
    let mut vm = make_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let fn_value = vm.eval(ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: "sphere_field".into(),
        line: 0,
        column: 0,
        code: "use mod.math.*\nlet f = |p| length(p) - 1.0\n(f)".into(),
        values: vec![],
    });
    assert!(vm.take_errors().is_empty());
    let aot = MathAot::new(&mut vm);
    let compiled = aot
        .compile(&vm, fn_value, &[MathAotParam::Vec3], &[])
        .expect("sphere expression must be in the pure-math subset");
    SdfSplashExpr::new(compiled.into_inner())
}

/// The interpreter's exact semantics for `length(p) - 1.0` on an
/// f32-rounded point: f32 lane products, left-associated f32 adds, f32
/// sqrt, promoted to f64, then an f64 subtract.
struct MirrorSphere;

impl Sdf3 for MirrorSphere {
    fn distance(&self, p: Vec3d) -> f64 {
        let x = p.x as f32;
        let y = p.y as f32;
        let z = p.z as f32;
        let len = (x * x + y * y + z * z).sqrt();
        len as f64 - 1.0
    }
}

#[test]
fn sphere_mesh_matches_mirror_exactly_and_analytic_within_tolerance() {
    let min = Vec3d::new(-1.6, -1.6, -1.6);
    let max = Vec3d::new(1.6, 1.6, 1.6);
    let depth = 5;

    let aot_mesh = sdf_to_mesh(compile_sphere_field(), min, max, depth);
    let mirror_mesh = sdf_to_mesh(MirrorSphere, min, max, depth);
    let analytic_mesh = sdf_to_mesh(SdfSphere::new(Vec3d::new(0.0, 0.0, 0.0), 1.0), min, max, depth);

    assert!(aot_mesh.triangle_count() > 100, "degenerate mesh");

    // 1. Bit-exact against the mirror field.
    assert_eq!(aot_mesh.triangle_count(), mirror_mesh.triangle_count());
    assert_eq!(aot_mesh.vertices.len(), mirror_mesh.vertices.len());
    for (a, b) in aot_mesh.vertices.iter().zip(mirror_mesh.vertices.iter()) {
        assert_eq!(a.x.to_bits(), b.x.to_bits());
        assert_eq!(a.y.to_bits(), b.y.to_bits());
        assert_eq!(a.z.to_bits(), b.z.to_bits());
    }

    // 2. Equal to the analytic sphere's mesh within f32 field precision.
    assert_eq!(aot_mesh.triangle_count(), analytic_mesh.triangle_count());
    let mut max_dev = 0.0f64;
    for (a, b) in aot_mesh.vertices.iter().zip(analytic_mesh.vertices.iter()) {
        let d = (*a - *b).length();
        if d > max_dev {
            max_dev = d;
        }
    }
    assert!(max_dev < 1e-4, "analytic deviation {max_dev}");
    // And every vertex sits on the unit sphere.
    for v in &aot_mesh.vertices {
        assert!((v.length() - 1.0).abs() < 0.05, "vertex off sphere: {v:?}");
    }
}

#[test]
fn batch_matches_pointwise() {
    let field = compile_sphere_field();
    let pts: Vec<Vec3d> = (0..257)
        .map(|i| {
            let t = i as f64 * 0.13;
            Vec3d::new(t.sin() * 1.3, (t * 0.7).cos() * 0.8, t * 0.01 - 1.0)
        })
        .collect();
    let mut xyz = Vec::new();
    for p in &pts {
        xyz.push(p.x as f32);
        xyz.push(p.y as f32);
        xyz.push(p.z as f32);
    }
    let mut out = vec![0f32; pts.len()];
    field.distance_batch(&xyz, &mut out);
    for (i, p) in pts.iter().enumerate() {
        let expected = field.distance(*p) as f32;
        assert_eq!(out[i].to_bits(), expected.to_bits(), "point {i}");
    }
    // A couple of spot values.
    let field = field;
    assert!((field.distance(Vec3d::new(2.0, 0.0, 0.0)) - 1.0).abs() < 1e-6);
    assert!((field.distance(Vec3d::new(0.0, 0.0, 0.0)) + 1.0).abs() < 1e-6);
    let _ = MathAotValue::Scalar(0.0);
}


/// Layer: integration (parametric). A parametric sphere `|p, r|` meshed
/// with two different radii from ONE compiled expression — the
/// parametric-CAD loop: set_uniforms + re-mesh, no recompile.
#[test]
fn parametric_radius_remesh() {
    let mut vm = make_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let fn_value = vm.eval(ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: "param_sphere".into(),
        line: 0,
        column: 0,
        code: "use mod.math.*\nlet f = |p, r| length(p) - r\n(f)".into(),
        values: vec![],
    });
    assert!(vm.take_errors().is_empty());
    let aot = MathAot::new(&mut vm);
    let compiled = aot
        .compile(&vm, fn_value, &[MathAotParam::Vec3], &[MathAotParam::Scalar])
        .expect("in subset");
    // One compiled field, shared with the mesher per radius.
    #[derive(Clone)]
    struct Shared(std::sync::Arc<SdfSplashExpr>);
    impl Sdf3 for Shared {
        fn distance(&self, p: Vec3d) -> f64 {
            self.0.distance(p)
        }
    }
    let mut shared = std::sync::Arc::new(SdfSplashExpr::new(compiled.into_inner()));
    let min = Vec3d::new(-1.6, -1.6, -1.6);
    let max = Vec3d::new(1.6, 1.6, 1.6);
    for r in [0.5f64, 1.0] {
        std::sync::Arc::get_mut(&mut shared)
            .expect("mesher clones dropped")
            .set_uniforms(vec![MathAotValue::Scalar(r)]);
        assert!((shared.distance(Vec3d::new(0.0, 0.0, 0.0)) + r).abs() < 1e-6);
        let mesh = sdf_to_mesh(Shared(shared.clone()), min, max, 4);
        assert!(mesh.triangle_count() > 50, "r={r}: degenerate mesh");
        for v in &mesh.vertices {
            assert!(
                (v.length() - r).abs() < 0.1,
                "r={r}: vertex off sphere: {v:?}"
            );
        }
        // Batch with the stored uniforms.
        let mut out = [0f32; 1];
        shared.distance_batch(&[r as f32, 0.0, 0.0], &mut out);
        assert!(out[0].abs() < 1e-6);
    }
}
