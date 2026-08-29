// Scaling regressions for N-ary booleans (PERF_REVIEW.md §3 / IMPLEMENTED).
//
// The old `union_all` was a left fold: N-1 pairwise ops against an
// ever-growing accumulator, each op finishing with whole-mesh cleanup passes.
// Baseline on Apple Silicon (release): N=128 overlapping 4.6 s, disjoint
// 5.1 s, and a single far-away operand made the T-junction grid degenerate
// (12x for the same op). These tests pin the fixed behavior:
//   - balanced proximity-paired reduction + bbox-disjoint early-outs
//   - boundary-local finishing (cost follows the cut, not the mesh)
//
// Wall-clock bounds are release-only and generous (~6x headroom on the
// measured times) so slower machines stay green; debug builds check the
// same geometry at N=32.

use makepad_csg::{union_all, Solid};
use std::time::Instant;

fn spheres(n: usize, spacing: f64) -> Vec<Solid> {
    let side = (n as f64).cbrt().ceil() as usize;
    (0..n)
        .map(|i| {
            let x = (i % side) as f64;
            let y = ((i / side) % side) as f64;
            let z = (i / (side * side)) as f64;
            Solid::sphere(0.4, 16, 9).translate(x * spacing, y * spacing, z * spacing)
        })
        .collect()
}

#[test]
fn union_all_scaling() {
    let n = if cfg!(debug_assertions) { 32 } else { 128 };

    // Overlapping grid: every neighbor intersects.
    let over = spheres(n, 0.5);
    let t = Instant::now();
    let r_over = union_all(&over);
    let t_over = t.elapsed().as_secs_f64();
    assert!(
        r_over.is_valid(),
        "overlapping {}-sphere union must be a closed manifold",
        n
    );
    // Volume of the union is a property of the solid, not of the reduction
    // order (measured 19.371 at N=128, 5.330 at N=32).
    let expected = if n == 128 { 19.371 } else { 5.330 };
    assert!(
        (r_over.volume() - expected).abs() < 0.05,
        "overlapping {}-sphere union volume {} (expected ~{})",
        n,
        r_over.volume(),
        expected
    );

    // Disjoint grid: no sphere touches any other; the union must be an exact
    // concatenation (per-sphere volume and triangle count preserved).
    let disj = spheres(n, 2.0);
    let t = Instant::now();
    let r_disj = union_all(&disj);
    let t_disj = t.elapsed().as_secs_f64();
    let one = Solid::sphere(0.4, 16, 9);
    assert!(r_disj.is_valid());
    assert_eq!(r_disj.triangle_count(), n * one.triangle_count());
    let vol_expected = one.volume() * n as f64;
    assert!(
        (r_disj.volume() - vol_expected).abs() < 1e-6 * vol_expected,
        "disjoint union volume {} != {} (sum of parts)",
        r_disj.volume(),
        vol_expected
    );

    // Release-only wall-clock bounds. Measured: 0.15 s / 0.04 s on Apple
    // Silicon; the quadratic fold took 4.6 s / 5.1 s.
    #[cfg(not(debug_assertions))]
    {
        assert!(
            t_over < 1.0,
            "overlapping 128-sphere union took {:.3} s (bound 1.0 s; quadratic-fold baseline was 4.6 s)",
            t_over
        );
        assert!(
            t_disj < 0.5,
            "disjoint 128-sphere union took {:.3} s (bound 0.5 s; baseline was 5.1 s)",
            t_disj
        );
    }
    #[cfg(debug_assertions)]
    {
        let _ = (t_over, t_disj);
    }
}

#[test]
fn far_operand_marginal_op() {
    // Finding 3 in PERF_REVIEW.md: the T-junction repair grid was sized from
    // the GLOBAL bbox, so a compact accumulator plus one far-away component
    // collapsed every vertex into a handful of cells (12x for the same op).
    // Build a wide-extent accumulator (cluster + far sphere, concatenated),
    // then cut into the cluster: the op's cost must follow the local cut.
    let cluster = union_all(&spheres(27, 0.5));
    let far = Solid::sphere(0.4, 16, 9).translate(1000.0, 0.0, 0.0);
    let acc = cluster.merge(&far); // wide bbox, like a scattered scene
    // The bit must actually cut the cluster surface (a bit at the grid cell
    // center is swallowed whole - zero segments), so hang it off a corner.
    let bit = Solid::sphere(0.4, 16, 9).translate(-0.3, -0.3, -0.3);

    let t = Instant::now();
    let r = acc.union(&bit);
    let dt = t.elapsed().as_secs_f64();
    assert!(
        r.volume() > cluster.volume(),
        "union with overlapping bit must add volume"
    );
    let report = r.validate();
    assert!(
        report.is_closed && report.is_manifold,
        "far-operand union must stay closed+manifold: {:?}",
        report
    );

    #[cfg(not(debug_assertions))]
    assert!(
        dt < 0.3,
        "marginal op with far operand took {:.3} s (bound 0.3 s; the global-extent grid pathology made this 10x+ the local cost)",
        dt
    );
    #[cfg(debug_assertions)]
    let _ = dt;
}
