// CSG benchmark harness — deterministic, std-only.
//
// Produces markdown tables of wall-clock timings for the whole CSG stack:
// booleans at 1k/10k/100k triangles, cascaded CAD-style differences,
// coincident-face cases, an internal phase breakdown of one boolean,
// primitive construction and SDF meshing.
//
// Every case is run 3x and the median is reported. A case whose first run
// already exceeds `SLOW_SECS` is reported from that single sample (marked
// `n=1`) so a 100k boolean does not turn the run into an afternoon.
//
//   cargo build --release --manifest-path libs/csg/csg/Cargo.toml --example bench
//   ./target/release/examples/bench              # everything
//   ./target/release/examples/bench bool10k sdf  # selected cases
//   ./target/release/examples/bench --loop u10k  # repeat forever (for `sample`)
//   ./target/release/examples/bench --list

use makepad_csg::{dvec3, Solid, Vec3d};
use makepad_csg_boolean::aabb_tree::AabbTree;
use makepad_csg_boolean::classify::{classify_triangles, MeshAccel, TriLocation};
use makepad_csg_boolean::corefine::corefine;
use makepad_csg_mesh::mesh::TriMesh;
use makepad_csg_sdf::{SdfBlobChain, SdfSphere};
use std::f64::consts::PI;
use std::time::Instant;

/// A case whose first sample takes longer than this is not repeated.
const SLOW_SECS: f64 = 20.0;

const CASES: &[(&str, &str)] = &[
    ("bool1k", "a: sphere/sphere boolean, ~1k tris per operand"),
    ("bool10k", "a: sphere/sphere boolean, ~10k tris per operand"),
    ("bool100k", "a: sphere/sphere boolean, ~100k tris per operand"),
    ("skew", "b: 10k sphere/sphere with a 0.3 rad skew rotation"),
    ("cascade", "c: plate minus 10 cylinders, timed per step"),
    ("coincident", "d: coincident-face cube union / difference"),
    ("phases", "e: internal phase breakdown of the 10k union"),
    ("prims", "f: primitive construction"),
    ("sdf", "g: SDF meshing at depth 5..8"),
    ("phone", "h: phone-case model (cube shell minus 10 cutters)"),
    ("u10k", "just the 10k sphere union (threads on/off compare)"),
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--list") {
        for (name, desc) in CASES {
            println!("{:<12} {}", name, desc);
        }
        return;
    }

    if let Some(pos) = args.iter().position(|a| a == "--loop") {
        let case = args
            .get(pos + 1)
            .cloned()
            .unwrap_or_else(|| "u10k".to_string());
        eprintln!("bench: looping case `{}` forever (pid {})", case, pid());
        let mut i = 0u64;
        loop {
            let t = Instant::now();
            run_loop_body(&case);
            i += 1;
            eprintln!("  iter {} — {:.1} ms", i, t.elapsed().as_secs_f64() * 1000.0);
        }
    }

    println!("# CSG benchmark");
    println!();
    println!("- threads: `{}` worker(s)", thread_count_str());
    println!("- slow-case cutoff: {:.0} s (above this, n=1)", SLOW_SECS);
    println!();

    let selected: Vec<&str> = if args.is_empty() {
        CASES.iter().map(|(n, _)| *n).collect()
    } else {
        args.iter().map(|s| s.as_str()).collect()
    };

    for case in selected {
        match case {
            "bool1k" => case_booleans("1k", 32, 17),
            "bool10k" => case_booleans("10k", 100, 51),
            "bool100k" => case_booleans("100k", 250, 201),
            "skew" => case_skew(),
            "cascade" => case_cascade(),
            "coincident" => case_coincident(),
            "phases" => case_phases(),
            "prims" => case_primitives(),
            "sdf" => case_sdf(),
            "phone" => case_phone(),
            "u10k" => case_u10k(),
            other => eprintln!("bench: unknown case `{}` (try --list)", other),
        }
    }
}

// ---------------------------------------------------------------- helpers

fn pid() -> u32 {
    std::process::id()
}

fn thread_count_str() -> String {
    format!("{}", makepad_csg_boolean::thread_pool::thread_count())
}

/// Run `f` up to three times, returning (median seconds, sample count, last result).
fn bench_n<R, F: FnMut() -> R>(mut f: F) -> (f64, usize, R) {
    let t = Instant::now();
    let mut r = f();
    let first = t.elapsed().as_secs_f64();
    let mut samples = vec![first];
    if first <= SLOW_SECS {
        for _ in 0..2 {
            let t = Instant::now();
            r = f();
            samples.push(t.elapsed().as_secs_f64());
        }
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (samples[samples.len() / 2], samples.len(), r)
}

/// Time one call.
fn time1<R, F: FnOnce() -> R>(f: F) -> (f64, R) {
    let t = Instant::now();
    let r = f();
    (t.elapsed().as_secs_f64(), r)
}

fn fmt_ms(secs: f64) -> String {
    if secs >= 1.0 {
        format!("{:.0} ({:.2} s)", secs * 1000.0, secs)
    } else {
        format!("{:.2}", secs * 1000.0)
    }
}

/// `closed / manifold / oriented` as a compact flag string.
fn validity(s: &Solid) -> String {
    let r = s.validate();
    format!(
        "{}{}{} (b={}, nm={}, deg={})",
        if r.is_closed { "C" } else { "-" },
        if r.is_manifold { "M" } else { "-" },
        if r.is_consistently_oriented { "O" } else { "-" },
        r.boundary_edges,
        r.non_manifold_edges,
        r.degenerate_triangles,
    )
}

fn header(title: &str) {
    println!("## {}", title);
    println!();
}

// ---------------------------------------------------------------- (a) booleans

fn case_booleans(label: &str, segments: u32, rings: u32) {
    let a = Solid::sphere(1.0, segments, rings);
    let b = Solid::sphere(1.0, segments, rings).translate(0.5, 0.0, 0.0);
    header(&format!(
        "a. sphere/sphere booleans — {} ({} tris per operand, sphere({}, {}))",
        label,
        a.triangle_count(),
        segments,
        rings
    ));
    bool_table(&a, &b);
}

fn bool_table(a: &Solid, b: &Solid) {
    println!("| op | median ms | n | out tris | validate |");
    println!("|---|---:|---:|---:|---|");
    for (name, f) in [
        ("union", 0u8),
        ("difference", 1u8),
        ("intersection", 2u8),
    ] {
        let (t, n, r) = bench_n(|| match f {
            0 => a.union(b),
            1 => a.difference(b),
            _ => a.intersection(b),
        });
        println!(
            "| {} | {} | {} | {} | {} |",
            name,
            fmt_ms(t),
            n,
            r.triangle_count(),
            validity(&r)
        );
    }
    println!();
}

// ---------------------------------------------------------------- (b) skew

fn case_skew() {
    let a = Solid::sphere(1.0, 100, 51);
    // 0.3 rad about a normalised skew axis — breaks the axis alignment that
    // makes the plain translated case cheap for the AABB tree.
    let axis = dvec3(0.577_350_269_189_625_7, 0.577_350_269_189_625_7, 0.577_350_269_189_625_7);
    let b = Solid::sphere(1.0, 100, 51)
        .rotate(axis, 0.3_f64.to_degrees())
        .translate(0.5, 0.0, 0.0);
    header(&format!(
        "b. sphere/sphere booleans — 10k, second operand rotated 0.3 rad about (1,1,1)/√3 ({} tris per operand)",
        a.triangle_count()
    ));
    bool_table(&a, &b);
}

// ---------------------------------------------------------------- (c) cascade

/// Plate with 10 drilled holes; each `difference` timed individually.
fn cascade_cutters() -> Vec<Solid> {
    let mut out = Vec::new();
    for i in 0..10 {
        let gx = -6.0 + (i % 5) as f64 * 3.0;
        let gy = -3.0 + (i / 5) as f64 * 6.0;
        // Cylinder is Y-axis aligned; rotate onto Z so it passes through the
        // 2-unit-thick plate, then place on the grid.
        out.push(
            Solid::cylinder(0.8, 4.0, 48, true)
                .rotate_x(90.0)
                .translate(gx, gy, 0.0),
        );
    }
    out
}

fn case_cascade() {
    let plate = Solid::cube(20.0, 20.0, 2.0, true);
    let cutters = cascade_cutters();
    header(&format!(
        "c. CAD cascade — cube(20,20,2) ({} tris) minus 10x cylinder(0.8, 4, 48) ({} tris each)",
        plate.triangle_count(),
        cutters[0].triangle_count()
    ));

    // Median over 3 full cascades, per step.
    let mut per_step: Vec<Vec<f64>> = vec![Vec::new(); cutters.len()];
    let mut counts: Vec<usize> = vec![0; cutters.len()];
    let mut valids: Vec<String> = vec![String::new(); cutters.len()];
    let mut totals: Vec<f64> = Vec::new();

    for _ in 0..3 {
        let mut result = plate.clone();
        let mut total = 0.0;
        for (i, c) in cutters.iter().enumerate() {
            let (t, r) = time1(|| result.difference(c));
            total += t;
            per_step[i].push(t);
            counts[i] = r.triangle_count();
            valids[i] = validity(&r);
            result = r;
        }
        totals.push(total);
    }

    println!("| step | cutter | median ms | tris after | validate |");
    println!("|---:|---|---:|---:|---|");
    for i in 0..cutters.len() {
        let mut s = per_step[i].clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "| {} | cylinder {} | {} | {} | {} |",
            i + 1,
            i + 1,
            fmt_ms(s[1]),
            counts[i],
            valids[i]
        );
    }
    totals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("| **total** | 10 differences | **{}** | | |", fmt_ms(totals[1]));
    println!();
}

// ---------------------------------------------------------------- (d) coincident

fn case_coincident() {
    header("d. coincident-face cases (unit cubes, 12 tris each)");
    println!("| case | median ms | n | out tris | validate |");
    println!("|---|---:|---:|---:|---|");

    let a = Solid::cube(1.0, 1.0, 1.0, true);
    let touch = Solid::cube(1.0, 1.0, 1.0, true).translate(1.0, 0.0, 0.0);
    let half = Solid::cube(1.0, 1.0, 1.0, true).translate(0.5, 0.0, 0.0);

    let (t, n, r) = bench_n(|| a.union(&touch));
    println!(
        "| union, faces exactly coincident (dx=1.0) | {} | {} | {} | {} |",
        fmt_ms(t),
        n,
        r.triangle_count(),
        validity(&r)
    );

    let (t, n, r) = bench_n(|| a.difference(&touch));
    println!(
        "| difference, faces exactly coincident (dx=1.0) | {} | {} | {} | {} |",
        fmt_ms(t),
        n,
        r.triangle_count(),
        validity(&r)
    );

    let (t, n, r) = bench_n(|| a.difference(&half));
    println!(
        "| difference, half overlap (dx=0.5) | {} | {} | {} | {} |",
        fmt_ms(t),
        n,
        r.triangle_count(),
        validity(&r)
    );

    let (t, n, r) = bench_n(|| a.union(&half));
    println!(
        "| union, half overlap (dx=0.5) | {} | {} | {} | {} |",
        fmt_ms(t),
        n,
        r.triangle_count(),
        validity(&r)
    );
    println!();
}

// ---------------------------------------------------------------- (e) phases

/// Re-implementation of `mesh_boolean(.., Union)` with a timer around each
/// public phase. The private tail (T-junction repair + sliver removal) is
/// obtained by subtracting the measured phases from a real `Solid::union`.
fn case_phases() {
    let a = Solid::sphere(1.0, 100, 51);
    let b = Solid::sphere(1.0, 100, 51).translate(0.5, 0.0, 0.0);
    header(&format!(
        "e. phase breakdown — union of two {}-triangle spheres",
        a.triangle_count()
    ));

    let (t_total, whole) = time1(|| a.union(&b));

    let ma = a.mesh();
    let mb = b.mesh();

    let (t_coref, coref) = time1(|| corefine(ma, mb));
    let (t_cls_a, class_a) = time1(|| {
        classify_triangles(&coref.mesh_a, &coref.mesh_b, &coref.on_boundary_a)
    });
    let (t_cls_b, class_b) = time1(|| {
        classify_triangles(&coref.mesh_b, &coref.mesh_a, &coref.on_boundary_b)
    });
    let (t_accel_b, accel_b) = time1(|| MeshAccel::build(&coref.mesh_b));
    let (t_accel_a, accel_a) = time1(|| MeshAccel::build(&coref.mesh_a));

    let (t_select, mut result) = time1(|| {
        select_union(&coref.mesh_a, &coref.mesh_b, &class_a, &class_b,
                     &coref.on_boundary_a, &coref.on_boundary_b, &accel_a, &accel_b)
    });
    let tris_pre_weld = result.triangle_count();
    let (t_weld, _) = time1(|| result.weld_vertices(1e-4));

    // Standalone AABB tree build cost on the corefined meshes, for reference.
    let tris_a: Vec<_> = (0..coref.mesh_a.triangle_count())
        .map(|i| coref.mesh_a.triangle_vertices(i))
        .collect();
    let tris_b: Vec<_> = (0..coref.mesh_b.triangle_count())
        .map(|i| coref.mesh_b.triangle_vertices(i))
        .collect();
    let (t_tree_a, _) = time1(|| AabbTree::build(&tris_a));
    let (t_tree_b, _) = time1(|| AabbTree::build(&tris_b));

    let measured = t_coref + t_cls_a + t_cls_b + t_accel_a + t_accel_b + t_select + t_weld;
    let t_tail = (t_total - measured).max(0.0);

    println!("| phase | ms | % of union |");
    println!("|---|---:|---:|");
    let row = |name: &str, t: f64| {
        println!("| {} | {} | {:.1}% |", name, fmt_ms(t), t / t_total * 100.0);
    };
    row("corefine (2x AabbTree build + broad phase + tri_tri + CDT)", t_coref);
    row("classify A vs B (1x AabbTree build + ray casts)", t_cls_a);
    row("classify B vs A (1x AabbTree build + ray casts)", t_cls_b);
    row("MeshAccel::build(B) (1x AabbTree build)", t_accel_b);
    row("MeshAccel::build(A) (1x AabbTree build)", t_accel_a);
    row("face selection loops", t_select);
    row("weld_vertices(1e-4)", t_weld);
    row("t-junction repair + sliver removal (by subtraction)", t_tail);
    println!("| **total union** | **{}** | 100% |", fmt_ms(t_total));
    println!();

    println!("| detail | value |");
    println!("|---|---:|");
    println!("| corefined mesh A triangles | {} |", coref.mesh_a.triangle_count());
    println!("| corefined mesh B triangles | {} |", coref.mesh_b.triangle_count());
    println!("| boundary triangles in A | {} |", coref.on_boundary_a.iter().filter(|b| **b).count());
    println!("| boundary triangles in B | {} |", coref.on_boundary_b.iter().filter(|b| **b).count());
    println!("| selected triangles (pre weld) | {} |", tris_pre_weld);
    println!("| final triangles | {} |", whole.triangle_count());
    println!("| AabbTree::build over corefined A | {} ms |", fmt_ms(t_tree_a));
    println!("| AabbTree::build over corefined B | {} ms |", fmt_ms(t_tree_b));
    println!("| AabbTree::build calls per boolean | 6 (corefine 2, classify 2, MeshAccel 2) |");
    println!(
        "| ray_cast_count calls per boolean | 3 dirs x 1..3 probes x {} corefined tris |",
        coref.mesh_a.triangle_count() + coref.mesh_b.triangle_count()
    );
    println!();
}

#[allow(clippy::too_many_arguments)]
fn select_union(
    mesh_a: &TriMesh,
    mesh_b: &TriMesh,
    class_a: &[TriLocation],
    class_b: &[TriLocation],
    on_boundary_a: &[bool],
    on_boundary_b: &[bool],
    accel_a: &MeshAccel,
    accel_b: &MeshAccel,
) -> TriMesh {
    let mut result = TriMesh::new();
    for ti in 0..mesh_a.triangle_count() {
        let c = class_a[ti];
        let mut keep = c == TriLocation::Outside;
        if !keep
            && on_boundary_a[ti]
            && (c == TriLocation::Inside || c == TriLocation::OnBoundary)
        {
            let (v0, v1, v2) = mesh_a.triangle_vertices(ti);
            let centroid = (v0 + v1 + v2) / 3.0;
            let normal = mesh_a.triangle_normal(ti);
            if !accel_b.point_inside(centroid + normal * 1e-6) {
                keep = true;
            }
        }
        if keep {
            let (v0, v1, v2) = mesh_a.triangle_vertices(ti);
            let a = result.add_vertex(v0);
            let b = result.add_vertex(v1);
            let c = result.add_vertex(v2);
            result.add_triangle(a, b, c);
        }
    }
    for ti in 0..mesh_b.triangle_count() {
        let c = class_b[ti];
        if c != TriLocation::Outside {
            continue;
        }
        if on_boundary_b[ti] && c == TriLocation::Inside {
            let (v0, v1, v2) = mesh_b.triangle_vertices(ti);
            let centroid = (v0 + v1 + v2) / 3.0;
            let normal = mesh_b.triangle_normal(ti);
            if !accel_a.point_inside(centroid + normal * 1e-6) {
                continue;
            }
        }
        let (v0, v1, v2) = mesh_b.triangle_vertices(ti);
        let a = result.add_vertex(v0);
        let b = result.add_vertex(v1);
        let c = result.add_vertex(v2);
        result.add_triangle(a, b, c);
    }
    result
}

// ---------------------------------------------------------------- (f) primitives

fn case_primitives() {
    header("f. primitive construction");
    println!("| primitive | median ms | n | tris | validate |");
    println!("|---|---:|---:|---:|---|");

    let (t, n, r) = bench_n(|| Solid::sphere(1.0, 316, 158));
    println!("| sphere(1, 316, 158) | {} | {} | {} | {} |", fmt_ms(t), n, r.triangle_count(), validity(&r));

    let (t, n, r) = bench_n(|| Solid::torus(2.0, 0.5, 200, 100));
    println!("| torus(2, 0.5, 200, 100) | {} | {} | {} | {} |", fmt_ms(t), n, r.triangle_count(), validity(&r));

    let (t, n, r) = bench_n(|| Solid::cylinder(1.0, 2.0, 1000, true));
    println!("| cylinder(1, 2, 1000) | {} | {} | {} | {} |", fmt_ms(t), n, r.triangle_count(), validity(&r));

    let ngon: Vec<[f64; 2]> = (0..1000)
        .map(|i| {
            let a = 2.0 * PI * i as f64 / 1000.0;
            [a.cos(), a.sin()]
        })
        .collect();
    let (t, n, r) = bench_n(|| Solid::extrude(&ngon, 1.0));
    println!("| extrude(1000-gon, 1) | {} | {} | {} | {} |", fmt_ms(t), n, r.triangle_count(), validity(&r));
    println!();
}

// ---------------------------------------------------------------- (g) SDF

fn case_sdf() {
    header("g. SDF meshing (dual contouring)");
    println!("| sdf | depth | median ms | n | tris | validate |");
    println!("|---|---:|---:|---:|---:|---|");

    let lo = dvec3(-1.5, -1.5, -1.5);
    let hi = dvec3(1.5, 1.5, 1.5);
    for depth in 5..=8 {
        let (t, n, r) = bench_n(|| {
            Solid::from_sdf(SdfSphere::new(dvec3(0.0, 0.0, 0.0), 1.0), lo, hi, depth)
        });
        println!(
            "| sphere | {} | {} | {} | {} | {} |",
            depth,
            fmt_ms(t),
            n,
            r.triangle_count(),
            validity(&r)
        );
    }

    // 20 spheres on a deterministic lattice, smooth-unioned.
    let lo = dvec3(-3.5, -3.5, -3.5);
    let hi = dvec3(3.5, 3.5, 3.5);
    for depth in 5..=8 {
        let (t, n, r) = bench_n(|| Solid::from_sdf(blob20(), lo, hi, depth));
        println!(
            "| smooth-union of 20 spheres | {} | {} | {} | {} | {} |",
            depth,
            fmt_ms(t),
            n,
            r.triangle_count(),
            validity(&r)
        );
    }
    println!();
}

/// Deterministic 20-sphere blob (no RNG — a fixed golden-angle spiral).
fn blob20() -> SdfBlobChain {
    let mut chain = SdfBlobChain::new(0.6);
    for i in 0..20 {
        let t = i as f64 / 20.0;
        let a = t * 20.0 * 2.399_963_229_728_653; // golden angle
        let r = 1.6;
        let y = -1.6 + 3.2 * t;
        let c = Vec3d::new(r * a.cos(), y, r * a.sin());
        chain = chain.add(SdfSphere::new(c, 0.7));
    }
    chain
}

// ---------------------------------------------------------------- (h) phone case

/// Cutters for the phone-case model from examples/cad/src/main.rs.
fn phone_cutters() -> Vec<(&'static str, Solid)> {
    vec![
        ("camera a", Solid::cylinder(0.24, 1.4, 56, true).rotate_x(90.0).translate(-0.7, 2.3, 0.0)),
        ("camera b", Solid::cylinder(0.24, 1.4, 56, true).rotate_x(90.0).translate(-0.15, 2.3, 0.0)),
        ("camera c", Solid::cylinder(0.24, 1.4, 56, true).rotate_x(90.0).translate(-0.7, 1.75, 0.0)),
        ("charge port", Solid::cube(0.85, 0.26, 1.2, true).translate(0.0, -3.15, 0.05)),
        ("speaker l", Solid::cube(0.55, 0.16, 1.2, true).translate(-0.85, -3.15, 0.05)),
        ("speaker r", Solid::cube(0.55, 0.16, 1.2, true).translate(0.85, -3.15, 0.05)),
        ("mute", Solid::cube(0.5, 0.22, 1.2, true).rotate_y(90.0).translate(-1.65, 2.6, 0.05)),
        ("vol up", Solid::cube(0.5, 0.55, 1.2, true).rotate_y(90.0).translate(-1.65, 1.7, 0.05)),
        ("vol down", Solid::cube(0.5, 0.55, 1.2, true).rotate_y(90.0).translate(-1.65, 0.95, 0.05)),
        ("power", Solid::cube(0.5, 0.7, 1.2, true).rotate_y(90.0).translate(1.65, 1.6, 0.05)),
    ]
}

/// Shell + camera plate, before any cutter is applied.
fn phone_shell() -> Solid {
    let outer = Solid::cube(3.2, 6.4, 0.55, true);
    let phone_void = Solid::cube(2.8, 5.9, 0.50, true).translate(0.0, -0.08, 0.18);
    let camera_plate = Solid::cube(1.6, 1.6, 0.18, true).translate(-0.7, 2.3, -0.36);
    outer.difference(&phone_void).merge(&camera_plate)
}

fn phone_case() -> Solid {
    let mut r = phone_shell();
    for (_, c) in &phone_cutters() {
        r = r.difference(c);
    }
    r
}

fn case_phone() {
    header("h. phone case (examples/cad) — shell + plate, then 10 cutter differences");
    let (t, n, r) = bench_n(phone_case);
    println!("| model | median ms | n | tris | validate |");
    println!("|---|---:|---:|---:|---|");
    println!(
        "| phone case (whole model) | {} | {} | {} | {} |",
        fmt_ms(t),
        n,
        r.triangle_count(),
        validity(&r)
    );
    println!();

    // Per-step breakdown so a single pathological cutter is visible.
    println!("| step | cutter | ms | tris after | validate |");
    println!("|---:|---|---:|---:|---|");
    let (t_shell, shell) = time1(phone_shell);
    println!(
        "| 0 | shell + plate | {} | {} | {} |",
        fmt_ms(t_shell),
        shell.triangle_count(),
        validity(&shell)
    );
    let mut r = shell;
    for (i, (name, c)) in phone_cutters().iter().enumerate() {
        let (t, next) = time1(|| r.difference(c));
        println!(
            "| {} | {} | {} | {} | {} |",
            i + 1,
            name,
            fmt_ms(t),
            next.triangle_count(),
            validity(&next)
        );
        r = next;
    }
    println!();
}

// ---------------------------------------------------------------- u10k

fn case_u10k() {
    let a = Solid::sphere(1.0, 100, 51);
    let b = Solid::sphere(1.0, 100, 51).translate(0.5, 0.0, 0.0);
    header(&format!(
        "10k sphere union only — {} tris per operand, {} worker thread(s)",
        a.triangle_count(),
        thread_count_str()
    ));
    let (t, n, r) = bench_n(|| a.union(&b));
    println!("| case | median ms | n | out tris | validate |");
    println!("|---|---:|---:|---:|---|");
    println!(
        "| union 10k | {} | {} | {} | {} |",
        fmt_ms(t),
        n,
        r.triangle_count(),
        validity(&r)
    );
    println!();
}

// ---------------------------------------------------------------- loop mode

fn run_loop_body(case: &str) {
    match case {
        "u10k" | "bool10k" => {
            let a = Solid::sphere(1.0, 100, 51);
            let b = Solid::sphere(1.0, 100, 51).translate(0.5, 0.0, 0.0);
            std::hint::black_box(a.union(&b));
        }
        "bool100k" => {
            let a = Solid::sphere(1.0, 250, 201);
            let b = Solid::sphere(1.0, 250, 201).translate(0.5, 0.0, 0.0);
            std::hint::black_box(a.union(&b));
        }
        "bool1k" => {
            let a = Solid::sphere(1.0, 32, 17);
            let b = Solid::sphere(1.0, 32, 17).translate(0.5, 0.0, 0.0);
            std::hint::black_box(a.union(&b));
        }
        "cascade" => {
            let plate = Solid::cube(20.0, 20.0, 2.0, true);
            let mut r = plate;
            for c in &cascade_cutters() {
                r = r.difference(c);
            }
            std::hint::black_box(r);
        }
        "phone" => {
            std::hint::black_box(phone_case());
        }
        other => {
            eprintln!("bench: no loop body for `{}`", other);
            std::process::exit(2);
        }
    }
}
