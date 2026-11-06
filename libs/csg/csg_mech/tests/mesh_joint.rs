use makepad_csg_math::Vec3d;
use makepad_csg_mech::check::{closer_than, min_gap, risk, sweep_limit, Risk, Sweep};
use makepad_csg_sdf::mesh_field::{MeshData, MeshField};
use makepad_csg_sdf::{Hints, Sdf3};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

struct Counted {
    field: MeshField,
    calls: AtomicU64,
}

impl Sdf3 for Counted {
    fn distance(&self, p: Vec3d) -> f64 {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.field.distance(p)
    }
    fn distance_bounded(&self, p: Vec3d, lower: f64, upper: f64, hints: &mut Hints) -> f64 {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.field.distance_bounded(p, lower, upper, hints)
    }
}

// Revolve a counterclockwise radius/height profile. Axis vertices are welded,
// so both the pin and the annular knuckles are closed, consistently wound meshes.
fn revolve(data: &mut MeshData, profile: &[[f32; 2]]) {
    const N: usize = 128;
    let mut rings = Vec::new();
    for &[r, z] in profile {
        let base = data.positions.len() as u32;
        let ring: Vec<u32> = (0..N)
            .map(|i| {
                if r == 0.0 {
                    if i == 0 {
                        data.positions.push([0.0, 0.0, z]);
                    }
                    base
                } else {
                    let a = i as f32 * std::f32::consts::TAU / N as f32;
                    data.positions.push([r * a.cos(), r * a.sin(), z]);
                    base + i as u32
                }
            })
            .collect();
        rings.push(ring);
    }
    for i in 0..rings.len() {
        let next = (i + 1) % rings.len();
        for k in 0..N {
            let j = (k + 1) % N;
            let q = [rings[i][k], rings[i][j], rings[next][j], rings[next][k]];
            for t in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                    data.triangles.push(t);
                }
            }
        }
    }
}

#[test]
fn mesh_pin_and_knuckles() {
    let mut a = MeshData::default();
    let mut b = MeshData::default();
    // One half carries the pin and two outer knuckles; the other rotates
    // between them with 0.2 mm radial and axial clearance.
    revolve(&mut a, &[[0.0, -1.0], [0.2, -1.0], [0.2, 1.0], [0.0, 1.0]]);
    for (lo, hi) in [(-1.0, -0.45), (0.45, 1.0)] {
        revolve(&mut a, &[[0.2, lo], [0.8, lo], [0.8, hi], [0.2, hi]]);
    }
    // Extra axial rings give both halves thousands of triangles without
    // changing the geometry or its known clearance.
    revolve(&mut b, &[[0.4, -0.25], [0.8, -0.25], [0.8, 0.0], [0.8, 0.25], [0.4, 0.25], [0.4, 0.0]]);
    let a = Counted { field: MeshField::new(a), calls: AtomicU64::new(0) };
    let b = Counted { field: MeshField::new(b), calls: AtomicU64::new(0) };
    assert!(a.field.watertight && b.field.watertight);
    println!("triangles: {} / {}", a.field.data.triangles.len(), b.field.data.triangles.len());
    let counts = || a.calls.swap(0, Ordering::Relaxed) + b.calls.swap(0, Ordering::Relaxed);
    let (lo, hi) = ([-1.0; 3], [1.0; 3]);
    let start = Instant::now();
    let gap = min_gap(&a, &b, lo, hi, 0.02);
    let found = if gap.gap < 0.0 { format!("overlap at least {:.9} mm", -gap.gap) } else { format!("{:.9} mm", gap.gap) };
    println!("min_gap: {found}, {} cells, {} field calls, {:.3} ms", gap.evals, counts(), start.elapsed().as_secs_f64() * 1000.0);
    assert!((gap.gap - 0.2).abs() < 0.036, "{gap:?}");
    for contact in [0.05, 0.25] {
        let start = Instant::now();
        let hit = closer_than(&a, &b, lo, hi, contact);
        println!("closer_than({contact}): {hit}, {} field calls, {:.3} ms", counts(), start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(hit, contact > 0.2);
    }
    let start = Instant::now();
    let limit = sweep_limit(&a, &b, Sweep { pivot: [0.0; 3], axis: [0.0, 0.0, 1.0], dir: 1.0, lo, hi, contact: 0.05, max: std::f64::consts::PI, turn: (0, 1), slide: false });
    println!("sweep_limit: {:.9} rad, {} field calls, {:.3} ms", limit, counts(), start.elapsed().as_secs_f64() * 1000.0);
    assert_eq!(limit, std::f64::consts::PI, "a clear hinge rotates through 180 degrees");
    assert_eq!(risk(gap.gap, 0.2, &[0.0; 4], 0.2).0, Risk::Low);
}
