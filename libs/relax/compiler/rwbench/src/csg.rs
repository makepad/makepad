// Solid modelling workload: libs/csg boolean ops on meshed primitives at fixed resolution.
use crate::fold;
use makepad_csg::{union_all, Solid};

fn solid_code(h: u64, s: &Solid) -> u64 {
    let m = s.mesh();
    let mut h = fold(h, s.triangle_count() as u64);
    h = fold(h, s.vertex_count() as u64);
    h = fold(h, s.volume().to_bits());
    h = fold(h, s.surface_area().to_bits());
    for v in m.vertices.iter().step_by(7) {
        h = fold(h, v.x.to_bits() ^ v.y.to_bits().rotate_left(21) ^ v.z.to_bits().rotate_left(42));
    }
    h
}

/// primitives (sphere, cube, cylinder, torus) meshed at fixed resolution, then difference / union /
/// intersection / union_all over them: a drilled block, a bolted flange, a sphere cluster.
pub fn setup() -> Box<dyn FnMut() -> u64> {
    Box::new(move || {
        let mut h = 0u64;
        let block = Solid::cube(2.0, 2.0, 2.0, true);
        let ball = Solid::sphere(1.3, 10, 6);
        let bore = Solid::cylinder(0.4, 3.0, 8, true);
        let rounded = block.intersection(&ball);
        h = solid_code(h, &rounded);
        let drilled = rounded.difference(&bore).difference(&bore.rotate_x(90.0)).difference(&bore.rotate_y(90.0));
        h = solid_code(h, &drilled);
        let ring = Solid::torus(1.0, 0.25, 14, 6);
        let flange = drilled.union(&ring.translate(0.0, 0.0, 0.9));
        h = solid_code(h, &flange);
        let cluster: Vec<Solid> = (0..4).map(|i| Solid::sphere(0.5, 8, 5).translate((i % 2) as f64 * 0.6, (i / 2) as f64 * 0.6, 0.0)).collect();
        h = solid_code(h, &union_all(&cluster));
        h
    })
}
