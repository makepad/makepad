// CSG Performance Profiler
//
// Measures timing of every operation category to identify bottlenecks.
// Run with: cargo run --example profile --release

use makepad_csg::Solid;
use std::f64::consts::PI;
use std::time::Instant;

fn main() {
    println!("CSG Performance Profile");
    println!("=======================\n");

    profile_primitives();
    profile_transforms();
    profile_booleans();
    profile_stl_io();
    profile_complex_workflows();
}

fn bench<F: FnOnce() -> R, R>(label: &str, f: F) -> R {
    let t = Instant::now();
    let r = f();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("  {:<45} {:>8.2} ms", label, ms);
    r
}

fn profile_primitives() {
    println!("--- Primitive Construction ---");
    bench("cube(4,4,4)", || Solid::cube(4.0, 4.0, 4.0, true));
    bench("cylinder(2, 5, 32)", || Solid::cylinder(2.0, 5.0, 32, true));
    bench("cylinder(2, 5, 128)", || {
        Solid::cylinder(2.0, 5.0, 128, true)
    });
    bench("sphere(2, 32, 16)", || Solid::sphere(2.0, 32, 16));
    bench("sphere(2, 64, 32)", || Solid::sphere(2.0, 64, 32));
    bench("sphere(2, 128, 64)", || Solid::sphere(2.0, 128, 64));
    bench("cone(2, 4, 64)", || Solid::cone(2.0, 4.0, 64, true));
    bench("torus(3, 1, 64, 32)", || Solid::torus(3.0, 1.0, 64, 32));
    bench("torus(3, 1, 128, 64)", || Solid::torus(3.0, 1.0, 128, 64));
    bench("tapered_cylinder(3, 1, 4, 64)", || {
        Solid::tapered_cylinder(3.0, 1.0, 4.0, 64, true)
    });

    let hex: Vec<[f64; 2]> = (0..6)
        .map(|i| {
            let a = PI / 3.0 * i as f64;
            [2.0 * a.cos(), 2.0 * a.sin()]
        })
        .collect();
    bench("extrude(hexagon, 4)", || Solid::extrude(&hex, 4.0));

    let star: Vec<[f64; 2]> = (0..10)
        .map(|i| {
            let a = PI * i as f64 / 5.0;
            let r = if i % 2 == 0 { 2.5 } else { 1.2 };
            [r * a.cos(), r * a.sin()]
        })
        .collect();
    bench("linear_extrude(star, twist=90, slices=8)", || {
        Solid::linear_extrude(&star, 6.0, 90.0, 0.5, 8)
    });
    bench("linear_extrude(star, twist=90, slices=32)", || {
        Solid::linear_extrude(&star, 6.0, 90.0, 0.5, 32)
    });

    let profile = vec![[2.0, 0.0], [3.0, 0.0], [3.0, 2.0], [2.0, 2.0]];
    bench("rotate_extrude(rect, 360, 32)", || {
        Solid::rotate_extrude(&profile, 360.0, 32)
    });
    bench("rotate_extrude(rect, 360, 128)", || {
        Solid::rotate_extrude(&profile, 360.0, 128)
    });
    println!();
}

fn profile_transforms() {
    println!("--- Transforms ---");
    let s = Solid::sphere(2.0, 64, 32);
    let tris = s.triangle_count();
    println!("  (sphere with {} triangles)", tris);
    bench("translate", || s.translate(1.0, 2.0, 3.0));
    bench("rotate_y(45)", || s.rotate_y(45.0));
    bench("scale(2, 1, 1)", || s.scale(2.0, 1.0, 1.0));
    bench("mirror(0)", || s.mirror(0));
    bench("flip", || s.flip());
    bench("weld(1e-10)", || s.weld(1e-10));
    println!();
}

fn profile_booleans() {
    println!("--- Boolean Operations ---");

    // Low poly
    let a = Solid::cube(2.0, 2.0, 2.0, true);
    let b = Solid::cube(2.0, 2.0, 2.0, true).translate(1.0, 0.0, 0.0);
    println!("  cube-cube (12+12 tris):");
    bench("  union", || a.union(&b));
    bench("  difference", || a.difference(&b));
    bench("  intersection", || a.intersection(&b));

    // cube-sphere
    let s = Solid::sphere(1.5, 16, 8);
    println!("  cube-sphere (12+{} tris):", s.triangle_count());
    bench("  union", || a.union(&s));
    bench("  difference", || a.difference(&s));
    bench("  intersection", || a.intersection(&s));

    // Medium poly
    let c1 = Solid::cylinder(1.0, 2.0, 32, true);
    let c2 = Solid::cylinder(0.5, 3.0, 32, true).rotate_x(90.0);
    println!(
        "  cylinder-cylinder ({}+{} tris):",
        c1.triangle_count(),
        c2.triangle_count()
    );
    bench("  union", || c1.union(&c2));
    bench("  difference", || c1.difference(&c2));

    // Higher poly
    let s1 = Solid::sphere(1.5, 32, 16);
    let s2 = Solid::sphere(1.5, 32, 16).translate(1.5, 0.0, 0.0);
    println!(
        "  sphere-sphere ({}+{} tris):",
        s1.triangle_count(),
        s2.triangle_count()
    );
    bench("  union", || s1.union(&s2));
    bench("  difference", || s1.difference(&s2));

    // High poly
    let s3 = Solid::sphere(1.5, 64, 32);
    let s4 = Solid::sphere(1.5, 64, 32).translate(1.5, 0.0, 0.0);
    println!(
        "  sphere-sphere hi ({}+{} tris):",
        s3.triangle_count(),
        s4.triangle_count()
    );
    bench("  union", || s3.union(&s4));
    bench("  difference", || s3.difference(&s4));

    println!();
}

fn profile_stl_io() {
    println!("--- STL I/O ---");
    let s = Solid::sphere(2.0, 64, 32);
    println!("  (sphere with {} triangles)", s.triangle_count());

    let path = "/tmp/csg_profile_test.stl";
    bench("write_stl (binary)", || s.write_stl(path).unwrap());
    bench("read_stl (binary)", || Solid::read_stl(path).unwrap());

    let path_ascii = "/tmp/csg_profile_test_ascii.stl";
    bench("write_stl (ascii)", || {
        s.write_stl_ascii(path_ascii).unwrap()
    });
    bench("read_stl (ascii)", || {
        Solid::read_stl_ascii(path_ascii).unwrap()
    });

    std::fs::remove_file(path).ok();
    std::fs::remove_file(path_ascii).ok();
    println!();
}

fn profile_complex_workflows() {
    println!("--- Complex Workflows ---");

    // Rook-like: 4 sequential boolean differences
    bench("4 cube-notch differences", || {
        let body = Solid::cylinder(4.0, 10.0, 32, false);
        let hollow = Solid::cylinder(3.0, 9.0, 32, false).translate(0.0, 1.0, 0.0);
        let mut result = body.difference(&hollow);
        for i in 0..4 {
            let notch = Solid::cube(3.0, 2.5, 12.0, true)
                .translate(0.0, 9.0, 0.0)
                .rotate_y((i as f64) * 90.0);
            result = result.difference(&notch);
        }
        result
    });

    // Gear-like: merge teeth then drill holes
    bench("gear (12 teeth + 4 holes)", || {
        let disk = Solid::cylinder(6.0, 3.0, 48, true);
        let mut gear = disk;
        for i in 0..12 {
            let angle = 360.0 * (i as f64) / 12.0;
            let tooth = Solid::cube(2.0, 3.0, 2.5, true)
                .translate(7.0, 0.0, 0.0)
                .rotate_y(angle);
            gear = gear.merge(&tooth);
        }
        let axle = Solid::cylinder(2.0, 5.0, 16, true);
        gear = gear.difference(&axle);
        for i in 0..3 {
            let a = 2.0 * PI * (i as f64) / 3.0;
            let hole =
                Solid::cylinder(1.0, 5.0, 12, true).translate(4.0 * a.cos(), 0.0, 4.0 * a.sin());
            gear = gear.difference(&hole);
        }
        gear
    });

    // Vase: linear_extrude then difference for hollow
    bench("twisted vase (extrude + hollow)", || {
        let n = 8;
        let mut star = Vec::new();
        for i in 0..(n * 2) {
            let angle = PI * (i as f64) / (n as f64);
            let r = if i % 2 == 0 { 4.0 } else { 2.5 };
            star.push([r * angle.cos(), r * angle.sin()]);
        }
        let outer = Solid::linear_extrude(&star, 16.0, 180.0, 0.5, 20);

        let mut inner = Vec::new();
        for i in 0..(n * 2) {
            let angle = PI * (i as f64) / (n as f64);
            let r = if i % 2 == 0 { 3.2 } else { 1.8 };
            inner.push([r * angle.cos(), r * angle.sin()]);
        }
        let inner = Solid::linear_extrude(&inner, 15.0, 180.0, 0.5, 20).translate(0.0, 1.0, 0.0);
        outer.difference(&inner)
    });

    println!();
}
