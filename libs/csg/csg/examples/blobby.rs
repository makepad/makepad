// Blobby — three spheres smooth-unioned with gloopiness
//
// Demonstrates the SDF smooth union: three overlapping spheres
// that melt into each other with a blobby, organic look.

use makepad_csg::{SdfSmoothUnion, SdfSphere, Solid};
use makepad_csg_math::Vec3d;
use std::time::Instant;

fn main() {
    let out_dir = "/Users/admin/makepad/makepad/libs/csg/output";
    std::fs::create_dir_all(out_dir).unwrap();
    let t = Instant::now();

    // Three spheres in a triangle arrangement
    let s1 = SdfSphere::new(Vec3d::new(-1.0, 0.0, 0.0), 1.5);
    let s2 = SdfSphere::new(Vec3d::new(1.0, 0.0, 0.0), 1.5);
    let s3 = SdfSphere::new(Vec3d::new(0.0, 1.5, 0.0), 1.2);

    // Gloopiness controls how blobby the blend is:
    //   0.0 = hard union (sharp creases)
    //   0.5 = subtle blend
    //   1.0 = quite blobby
    //   2.0 = very gloopy
    let gloopiness = 1.0;

    let blob = SdfSmoothUnion::new(SdfSmoothUnion::new(s1, s2, gloopiness), s3, gloopiness);

    let bounds = 4.0;
    let min = Vec3d::new(-bounds, -bounds, -bounds);
    let max = Vec3d::new(bounds, bounds, bounds);

    println!(
        "[{:.2}s] Meshing blobby spheres (gloopiness={})...",
        t.elapsed().as_secs_f64(),
        gloopiness
    );
    let solid = Solid::from_sdf(blob, min, max, 7);

    let path = format!("{}/blobby.stl", out_dir);
    solid.write_stl(&path).unwrap();
    println!(
        "[{:.2}s] Blobby — {} tris, vol={:.1}",
        t.elapsed().as_secs_f64(),
        solid.triangle_count(),
        solid.volume()
    );

    // Cut a tall column through the center — pokes out top and bottom
    // so you can look through the holes and see the blobby interior
    println!(
        "[{:.2}s] Cutting column through blobby...",
        t.elapsed().as_secs_f64()
    );
    let cutter = Solid::cube(1.5, 10.0, 1.5, true);
    let cut = solid.difference(&cutter);

    let cut_path = format!("{}/blobby_cut.stl", out_dir);
    cut.write_stl(&cut_path).unwrap();
    println!(
        "[{:.2}s] Cut — {} tris, vol={:.1}",
        t.elapsed().as_secs_f64(),
        cut.triangle_count(),
        cut.volume()
    );
    println!("\nWritten to:\n  {}\n  {}", path, cut_path);
}
