//! Layer: integration.
//!
//! A gridded field (`SdfSplashExpr::compile_gridded`): its fn reads the grid through the kernel
//! prelude's `sdf_grid3`, the trilinear value clamped to the grid's edge, so a linear function
//! sampled on the grid comes back exactly (to f32 rounding) inside it and at the nearest point
//! of the grid's box outside. A brick's piece of the grid (`cull` crops it) gives the same bits
//! as the whole grid wherever the region reaches.

use makepad_csg_sdf::{cull, SdfGrid, SdfKernel, SdfNode, SdfSplashExpr, SplashPoint};
use makepad_script::*;

fn make_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) }
}

const ORIGIN: [f32; 3] = [-2.0, -1.0, -3.0];
const H: f32 = 0.5;
const DIMS: [u32; 3] = [16, 12, 20];

fn linear(p: [f64; 3]) -> f64 {
    0.5 * p[0] + 0.25 * p[1] - 0.125 * p[2] + 1.0
}

fn grid() -> SdfGrid {
    let mut data = Vec::new();
    for k in 0..DIMS[2] {
        for j in 0..DIMS[1] {
            for i in 0..DIMS[0] {
                let p = [i, j, k].map(|c| c as f64 * H as f64);
                data.push(linear([p[0] + ORIGIN[0] as f64, p[1] + ORIGIN[1] as f64, p[2] + ORIGIN[2] as f64]) as f32);
            }
        }
    }
    SdfGrid::new(ORIGIN, H, DIMS, 0.0, data.into())
}

fn field(vm: &mut ScriptVm, grid: SdfGrid) -> SdfSplashExpr {
    vm.capture_errors();
    let v = vm.eval(ScriptMod::new("", "", "grid_field", 0, 0, "use mod.math.*\nlet f = |p| sdf_grid3(p) - 0.25\n(f)", &[]));
    assert!(vm.take_errors().is_empty());
    let f = v.as_object().expect("a fn");
    let _keep = vm.new_object_ref(f);
    SdfSplashExpr::compile_gridded(vm, f, SplashPoint::Vec3, &[], grid).unwrap_or_else(|e| panic!("{e:?}"))
}

fn eval(k: &SdfSplashExpr, ps: &[[f64; 3]]) -> Vec<f32> {
    let xyz: Vec<f32> = ps.iter().flat_map(|p| p.map(|x| x as f32)).collect();
    let mut out = vec![0f32; ps.len()];
    k.distance_batch(&xyz, &mut out);
    out
}

/// Deterministic points in a box (a 32-bit LCG).
fn points(n: usize, lo: [f64; 3], hi: [f64; 3]) -> Vec<[f64; 3]> {
    let mut s = 777u32;
    let mut r = || {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        (s >> 8) as f64 / 16777216.0
    };
    (0..n).map(|_| std::array::from_fn(|a| lo[a] + r() * (hi[a] - lo[a]))).collect()
}

#[test]
fn a_grid_reads_back_trilinear_and_its_crop_reads_the_same_bits() {
    let mut vm = make_vm();
    let whole = field(&mut vm, grid());
    let hi: [f64; 3] = std::array::from_fn(|a| ORIGIN[a] as f64 + (DIMS[a] - 1) as f64 * H as f64);
    // inside, and a margin outside where the grid's edge holds
    let ps = points(200, [-3.0, -2.0, -4.0], [hi[0] + 1.0, hi[1] + 1.0, hi[2] + 1.0]);
    let got = eval(&whole, &ps);
    for (p, g) in ps.iter().zip(&got) {
        let q: [f64; 3] = std::array::from_fn(|a| p[a].clamp(ORIGIN[a] as f64, hi[a]));
        let want = linear(q) - 0.25;
        assert!((*g as f64 - want).abs() < 2e-5, "at {p:?}: {g} against {want}");
    }
    // a brick's piece: the culled kernel holds fewer samples and reads the same bits
    let region = ([-0.7, 0.1, -1.2], [0.4, 0.9, 0.3]);
    let node = SdfNode::Kernel(SdfKernel::splash(whole.clone(), None));
    let Some(SdfNode::Kernel(piece)) = cull(&node, &region) else { panic!("the kernel stays") };
    let piece = piece.splash.expect("a Splash kernel");
    let (g, w) = (piece.grid().unwrap(), whole.grid().unwrap());
    assert!(g.data.len() * 8 < w.data.len(), "the piece holds {} of {} samples", g.data.len(), w.data.len());
    assert_ne!(piece.key(), whole.key());
    assert_eq!(piece.structure_key(), whole.structure_key(), "the same program");
    let inside = points(64, region.0, region.1);
    let (a, b) = (eval(&whole, &inside), eval(&piece, &inside));
    for (i, p) in inside.iter().enumerate() {
        assert_eq!(a[i].to_bits(), b[i].to_bits(), "at {p:?}: whole {} piece {}", a[i], b[i]);
    }
    // regions reading the same samples cull alike, others do not
    assert!(makepad_csg_sdf::culls_alike(&node, &region, &([-0.69, 0.11, -1.19], [0.41, 0.89, 0.31])));
    assert!(!makepad_csg_sdf::culls_alike(&node, &region, &([1.0, 0.1, -1.2], [2.0, 0.9, 0.3])));
}
