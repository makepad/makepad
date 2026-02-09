// Validate all showcase pieces for manifoldness

use makepad_csg::{dvec3, Solid};
use makepad_csg_mesh::validate::validate_mesh;
use std::f64::consts::PI;

fn check(label: &str, solid: &Solid) -> bool {
    let mesh = solid.mesh();
    let report = validate_mesh(mesh);
    let ok = report.boundary_edges == 0 && report.non_manifold_edges == 0;
    eprintln!(
        "{:>25}: tris={:<5} bnd={:<3} nonmani={:<3} {}",
        label,
        mesh.triangle_count(),
        report.boundary_edges,
        report.non_manifold_edges,
        if ok { "OK" } else { "FAIL" }
    );
    ok
}

fn main() {
    let mut pass = 0;
    let mut fail = 0;

    // Rook
    let n = 12;
    let eps = 0.01;
    let base = Solid::tapered_cylinder(6.0, 5.0, 2.0, n, false);
    let body = Solid::cylinder(4.0, 10.0 + 2.0 * eps, n, false).translate(0.0, 2.0 - eps, 0.0);
    let neck = Solid::tapered_cylinder(4.0, 5.0, 2.0 + 2.0 * eps, n, false).translate(
        0.0,
        12.0 - eps,
        0.0,
    );
    let crown = Solid::cylinder(5.0, 2.0 + 2.0 * eps, n, false).translate(0.0, 14.0 - eps, 0.0);
    let hollow = Solid::cylinder(3.0, 14.0, n, false).translate(0.0, 2.5, 0.0);
    let mut rook = base.union(&body).union(&neck).union(&crown);
    rook = rook.difference(&hollow);
    for i in 0..4 {
        let angle = (i as f64) * 90.0;
        let notch = Solid::cube(3.0, 2.5, 6.0, true)
            .translate(0.0, 15.25, 3.0)
            .rotate_y(angle);
        rook = rook.difference(&notch);
    }
    if check("rook", &rook) {
        pass += 1;
    } else {
        fail += 1;
    }

    // Twisted vase
    let nv = 5;
    let mut star = Vec::new();
    for i in 0..(nv * 2) {
        let angle = PI * (i as f64) / (nv as f64);
        let r = if i % 2 == 0 { 4.0 } else { 2.5 };
        star.push([r * angle.cos(), r * angle.sin()]);
    }
    let outer = Solid::linear_extrude(&star, 16.0, 180.0, 0.5, 10);
    let mut inner_star = Vec::new();
    for i in 0..(nv * 2) {
        let angle = PI * (i as f64) / (nv as f64);
        let r = if i % 2 == 0 { 3.2 } else { 1.8 };
        inner_star.push([r * angle.cos(), r * angle.sin()]);
    }
    let inner = Solid::linear_extrude(&inner_star, 15.0, 180.0, 0.5, 10).translate(0.0, 1.0, 0.0);
    let vase_base = Solid::cylinder(5.0, 1.0, 16, false);
    let hollowed = outer.difference(&inner);
    let vase = hollowed.union(&vase_base);
    if check("twisted_vase", &vase) {
        pass += 1;
    } else {
        fail += 1;
    }

    // Gear
    let teeth = 12;
    let inner_r = 6.0;
    let tooth_h = 2.0;
    let thickness = 3.0;
    let disk = Solid::cylinder(inner_r, thickness, 16, true);
    let mut gear = disk;
    for i in 0..teeth {
        let angle = 360.0 * (i as f64) / (teeth as f64);
        let tooth = Solid::cube(tooth_h, thickness, 2.5, true)
            .translate(inner_r + tooth_h / 2.0, 0.0, 0.0)
            .rotate_y(angle);
        gear = gear.merge(&tooth);
    }
    let axle = Solid::cylinder(2.0, thickness + 2.0, 12, true);
    gear = gear.difference(&axle);
    for i in 0..3 {
        let angle_rad = 2.0 * PI * (i as f64) / 3.0;
        let hx = 4.0 * angle_rad.cos();
        let hz = 4.0 * angle_rad.sin();
        let hole = Solid::cylinder(1.2, thickness + 2.0, 10, true).translate(hx, 0.0, hz);
        gear = gear.difference(&hole);
    }
    if check("gear", &gear) {
        pass += 1;
    } else {
        fail += 1;
    }

    // Lathe vase
    let profile = vec![
        [1.5, 0.0],
        [3.0, 0.5],
        [2.2, 2.5],
        [2.8, 4.0],
        [3.5, 6.0],
        [3.0, 8.0],
        [2.5, 9.0],
        [2.0, 9.5],
        [1.5, 9.0],
        [2.0, 8.0],
        [2.8, 6.0],
        [2.2, 4.0],
        [2.5, 2.5],
        [2.0, 0.5],
    ];
    let lathe = Solid::rotate_extrude(&profile, 360.0, 24);
    if check("lathe_vase", &lathe) {
        pass += 1;
    } else {
        fail += 1;
    }

    // Boolean demos
    let cube = Solid::cube(3.0, 3.0, 3.0, true);
    let sph = Solid::sphere(2.0, 16, 8);
    if check("bool_union", &cube.union(&sph)) {
        pass += 1;
    } else {
        fail += 1;
    }
    if check("bool_difference", &cube.difference(&sph)) {
        pass += 1;
    } else {
        fail += 1;
    }
    if check("bool_intersection", &cube.intersection(&sph)) {
        pass += 1;
    } else {
        fail += 1;
    }
    if check("bool_xor", &cube.symmetric_difference(&sph)) {
        pass += 1;
    } else {
        fail += 1;
    }

    // Individual primitives
    if check("cube", &cube) {
        pass += 1;
    } else {
        fail += 1;
    }
    if check("cylinder", &Solid::cylinder(2.0, 5.0, 16, true)) {
        pass += 1;
    } else {
        fail += 1;
    }
    if check("sphere", &sph) {
        pass += 1;
    } else {
        fail += 1;
    }
    if check("cone", &Solid::cone(2.0, 4.0, 16, true)) {
        pass += 1;
    } else {
        fail += 1;
    }
    if check("torus", &Solid::torus(3.0, 1.0, 16, 8)) {
        pass += 1;
    } else {
        fail += 1;
    }

    // Twisted star
    let star10: Vec<[f64; 2]> = (0..10)
        .map(|i| {
            let a = PI * i as f64 / 5.0;
            let r = if i % 2 == 0 { 2.5 } else { 1.2 };
            [r * a.cos(), r * a.sin()]
        })
        .collect();
    let twisted = Solid::linear_extrude(&star10, 6.0, 120.0, 0.5, 16);
    if check("twisted_star", &twisted) {
        pass += 1;
    } else {
        fail += 1;
    }

    eprintln!("\n{} passed, {} failed", pass, fail);
}
