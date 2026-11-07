// Scaling regressions for N-ary booleans (PERF_REVIEW.md §3 / IMPLEMENTED).
//
// A single far-away operand made the T-junction grid degenerate (12x for the
// same op). This test pins the fixed behavior: a boolean costs what it cuts,
// not the mesh, counted in the triangles that take part (not timed, so a
// loaded box cannot change it). The balanced reduction's own scaling is
// counted in work by `union_all_scaling` (src/solid.rs).

use makepad_csg::{union_all, FinishParams, Solid};
use makepad_csg_boolean::boolean::{mesh_boolean_work, BoolOp};

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
fn far_operand_marginal_op() {
    // Finding 3 in PERF_REVIEW.md: the T-junction repair grid was sized from
    // the GLOBAL bbox, so a compact accumulator plus one far-away component
    // collapsed every vertex into a handful of cells (12x for the same op).
    // Build a wide-extent accumulator (cluster + far sphere, concatenated),
    // then cut into the cluster: the op's work must follow the local cut.
    let cluster = union_all(&spheres(27, 0.5));
    let far = Solid::sphere(0.4, 16, 9).translate(1000.0, 0.0, 0.0);
    let acc = cluster.merge(&far); // wide bbox, like a scattered scene
    // The bit must actually cut the cluster surface (a bit at the grid cell
    // center is swallowed whole - zero segments), so hang it off a corner.
    let bit = Solid::sphere(0.4, 16, 9).translate(-0.3, -0.3, -0.3);

    let r = acc.union(&bit);
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

    // The far sphere takes no part (it passes through), and of the cluster only
    // what the bit reaches does: the same triangles as cutting the cluster alone.
    let work = |of: &Solid| mesh_boolean_work(of.mesh(), bit.mesh(), BoolOp::Union, FinishParams::default()).1;
    let (with_far, alone) = (work(&acc), work(&cluster));
    println!("the cut took {with_far} triangles with the far operand, {alone} without, of {} in the accumulator", acc.triangle_count());
    assert_eq!(with_far, alone, "the far operand took part in the cut");
    assert!(
        with_far * 10 < acc.triangle_count(),
        "the cut took {with_far} of the accumulator's {} triangles: its cost does not follow the cut",
        acc.triangle_count()
    );
}
