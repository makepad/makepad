// CSG Showcase: Chess Rook + Twisted Vase + Gear + Primitives Gallery
//
// Demonstrates every primitive and operation in the CSG library.
// Outputs individual STLs and a combined trophy display.

use makepad_csg::{dvec3, Solid};
use std::f64::consts::PI;
use std::time::Instant;

fn timed(t: &Instant, msg: &str) {
    println!("[{:6.2}s] {}", t.elapsed().as_secs_f64(), msg);
}

fn main() {
    let out_dir = "/Users/admin/makepad/makepad/libs/csg/output";
    std::fs::create_dir_all(out_dir).unwrap();
    let t = Instant::now();

    // ================================================================
    // 1. Chess Rook — cylinders, tapered cylinders, cube differences
    // ================================================================
    timed(&t, "Building chess rook...");
    let rook = build_rook();
    write(&t, &rook, out_dir, "rook");

    // ================================================================
    // 2. Twisted Vase — linear_extrude with twist + scale, hollowed
    // ================================================================
    timed(&t, "Building twisted vase...");
    let vase = build_twisted_vase();
    write(&t, &vase, out_dir, "twisted_vase");

    // ================================================================
    // 3. Gear — cylinder + cube teeth + drilled holes
    // ================================================================
    timed(&t, "Building gear...");
    let gear = build_gear();
    write(&t, &gear, out_dir, "gear");

    // ================================================================
    // 4. Lathe Vase — rotate_extrude
    // ================================================================
    timed(&t, "Building lathe vase...");
    let lathe_vase = build_lathe_vase();
    write(&t, &lathe_vase, out_dir, "lathe_vase");

    // ================================================================
    // 5. Boolean demo pieces
    // ================================================================
    timed(&t, "Building boolean demos...");
    let cube = Solid::cube(3.0, 3.0, 3.0, true);
    let sph = Solid::sphere(2.0, 16, 8);

    let bool_union = cube.union(&sph);
    write(&t, &bool_union, out_dir, "bool_union");

    let bool_diff = cube.difference(&sph);
    write(&t, &bool_diff, out_dir, "bool_difference");

    let bool_isect = cube.intersection(&sph);
    write(&t, &bool_isect, out_dir, "bool_intersection");

    let bool_xor = cube.symmetric_difference(&sph);
    write(&t, &bool_xor, out_dir, "bool_xor");

    // ================================================================
    // 6. Individual primitives
    // ================================================================
    timed(&t, "Building individual primitives...");

    write(&t, &cube, out_dir, "cube");
    write(
        &t,
        &Solid::cylinder(2.0, 5.0, 16, true),
        out_dir,
        "cylinder",
    );
    write(&t, &sph, out_dir, "sphere");
    write(&t, &Solid::cone(2.0, 4.0, 16, true), out_dir, "cone");
    write(&t, &Solid::torus(3.0, 1.0, 16, 8), out_dir, "torus");
    write(
        &t,
        &Solid::tapered_cylinder(2.5, 1.0, 4.0, 16, true),
        out_dir,
        "tapered_cylinder",
    );

    // Hexagonal extrusion
    let hex: Vec<[f64; 2]> = (0..6)
        .map(|i| {
            let a = PI / 3.0 * i as f64;
            [2.0 * a.cos(), 2.0 * a.sin()]
        })
        .collect();
    write(&t, &Solid::extrude(&hex, 3.0), out_dir, "hexagon_extrude");

    // Polyhedron pyramid
    let pyramid = Solid::polyhedron(
        &[
            dvec3(0.0, 3.0, 0.0),
            dvec3(-2.0, 0.0, -2.0),
            dvec3(2.0, 0.0, -2.0),
            dvec3(2.0, 0.0, 2.0),
            dvec3(-2.0, 0.0, 2.0),
        ],
        &[
            [0, 2, 1],
            [0, 3, 2],
            [0, 4, 3],
            [0, 1, 4],
            [1, 2, 3],
            [1, 3, 4],
        ],
    );
    write(&t, &pyramid, out_dir, "pyramid");

    // Twisted star
    let star: Vec<[f64; 2]> = (0..10)
        .map(|i| {
            let a = PI * i as f64 / 5.0;
            let r = if i % 2 == 0 { 2.5 } else { 1.2 };
            [r * a.cos(), r * a.sin()]
        })
        .collect();
    let twisted = Solid::linear_extrude(&star, 6.0, 120.0, 0.5, 16);
    write(&t, &twisted, out_dir, "twisted_star");

    // Mirror demo
    let mirrored = twisted.mirror(0);
    write(&t, &mirrored, out_dir, "twisted_star_mirrored");

    // ================================================================
    // 7. Trophy: everything on a display base
    // ================================================================
    timed(&t, "Building trophy display...");
    let trophy = build_trophy(&rook, &vase, &gear, &lathe_vase);
    write(&t, &trophy, out_dir, "showcase_trophy");

    timed(&t, "All done!");
    println!("\nSTL files written to: {}", out_dir);
}

fn write(t: &Instant, solid: &Solid, dir: &str, name: &str) {
    let path = format!("{}/{}.stl", dir, name);
    solid.write_stl(&path).unwrap();
    timed(
        t,
        &format!(
            "  {} — {} tris, vol={:.1}",
            name,
            solid.triangle_count(),
            solid.volume()
        ),
    );
}

/// Chess rook: cylinders + tapered cylinder + cube notch differences
fn build_rook() -> Solid {
    let n = 12; // polygon resolution
    let eps = 0.01; // overlap for watertight union seams
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

    // 4 crenellation notches — each cuts a gap in the crown wall.
    // The notch is offset in z so each 90° rotation cuts a distinct region.
    for i in 0..4 {
        let angle = (i as f64) * 90.0;
        let notch = Solid::cube(3.0, 2.5, 6.0, true)
            .translate(0.0, 15.25, 3.0)
            .rotate_y(angle);
        rook = rook.difference(&notch);
    }
    rook
}

/// Twisted vase: star cross-section, linear_extrude with twist+scale, hollowed
fn build_twisted_vase() -> Solid {
    let n = 5;
    let mut star = Vec::new();
    for i in 0..(n * 2) {
        let angle = PI * (i as f64) / (n as f64);
        let r = if i % 2 == 0 { 4.0 } else { 2.5 };
        star.push([r * angle.cos(), r * angle.sin()]);
    }
    let outer = Solid::linear_extrude(&star, 16.0, 180.0, 0.5, 10);

    let mut inner_star = Vec::new();
    for i in 0..(n * 2) {
        let angle = PI * (i as f64) / (n as f64);
        let r = if i % 2 == 0 { 3.2 } else { 1.8 };
        inner_star.push([r * angle.cos(), r * angle.sin()]);
    }
    let inner = Solid::linear_extrude(&inner_star, 15.0, 180.0, 0.5, 10).translate(0.0, 1.0, 0.0);

    // Hollow first, then add the base — doing the difference on the
    // merged shell fails because the overlapping base/outer geometry
    // confuses the corefinement near the seam.
    let base = Solid::cylinder(5.0, 1.0, 16, false);
    let hollowed = outer.difference(&inner);
    hollowed.union(&base)
}

/// Gear with teeth, axle hole, and lightening holes
fn build_gear() -> Solid {
    let teeth = 12;
    let inner_r = 6.0;
    let tooth_h = 2.0;
    let thickness = 3.0;

    let disk = Solid::cylinder(inner_r, thickness, 16, true);
    let mut gear = disk;

    // Teeth
    for i in 0..teeth {
        let angle = 360.0 * (i as f64) / (teeth as f64);
        let tooth = Solid::cube(tooth_h, thickness, 2.5, true)
            .translate(inner_r + tooth_h / 2.0, 0.0, 0.0)
            .rotate_y(angle);
        gear = gear.merge(&tooth);
    }

    // Center axle hole
    let axle = Solid::cylinder(2.0, thickness + 2.0, 12, true);
    gear = gear.difference(&axle);

    // 3 lightening holes
    for i in 0..3 {
        let angle_rad = 2.0 * PI * (i as f64) / 3.0;
        let hx = 4.0 * angle_rad.cos();
        let hz = 4.0 * angle_rad.sin();
        let hole = Solid::cylinder(1.2, thickness + 2.0, 10, true).translate(hx, 0.0, hz);
        gear = gear.difference(&hole);
    }
    gear
}

/// Lathe vase: rotate_extrude of a curvy profile
fn build_lathe_vase() -> Solid {
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
    Solid::rotate_extrude(&profile, 360.0, 24)
}

/// Display base with all pieces arranged
fn build_trophy(rook: &Solid, vase: &Solid, gear: &Solid, lathe: &Solid) -> Solid {
    // Large base platform
    let base = Solid::tapered_cylinder(25.0, 23.0, 2.0, 24, false);
    let ring = Solid::torus(24.0, 0.6, 24, 8).translate(0.0, 2.0, 0.0);
    let platform = base.merge(&ring);

    // Arrange pieces
    let r = rook.translate(-14.0, 2.0, 0.0);
    let v = vase.translate(-5.0, 2.0, 0.0);
    let l = lathe.translate(5.0, 2.0, 0.0);
    let g = gear.rotate_x(90.0).translate(14.0, 6.0, 0.0);

    // Nameplate wedge using polyhedron
    let nameplate = Solid::polyhedron(
        &[
            dvec3(-6.0, 0.0, -1.2),
            dvec3(6.0, 0.0, -1.2),
            dvec3(6.0, 0.0, 1.2),
            dvec3(-6.0, 0.0, 1.2),
            dvec3(-5.5, 1.2, -1.0),
            dvec3(5.5, 1.2, -1.0),
            dvec3(5.5, 1.2, 1.0),
            dvec3(-5.5, 1.2, 1.0),
        ],
        &[
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
            [1, 2, 6],
            [1, 6, 5],
        ],
    );
    let front_plate = nameplate.translate(0.0, 0.0, 20.0);
    let back_plate = nameplate.mirror(2);

    platform
        .merge(&r)
        .merge(&v)
        .merge(&l)
        .merge(&g)
        .merge(&front_plate)
        .merge(&back_plate)
}
