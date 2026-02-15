// Trace the CDT retriangulation for the cube-cylinder corefinement case.
// Focus on the corefined meshes to find exactly which triangles produce
// mismatched edges.

use makepad_csg_boolean::boolean::{mesh_boolean, BoolOp};
use makepad_csg_boolean::classify::{classify_triangles, TriLocation};
use makepad_csg_boolean::corefine::corefine;
use makepad_csg_math::{dvec3, Mat4d, Vec3d};
use makepad_csg_mesh::mesh::{make_unit_cube, TriMesh};
use makepad_csg_mesh::validate::validate_mesh;
use makepad_csg_mesh::volume::mesh_volume;
use std::collections::HashMap;

fn report(label: &str, mesh: &TriMesh) {
    let r = validate_mesh(mesh);
    println!(
        "  {:40} tris={:3}  verts={:3}  vol={:8.4}  bnd={}  nonmani={}  closed={}  manifold={}",
        label,
        mesh.triangle_count(),
        mesh.vertex_count(),
        mesh_volume(mesh),
        r.boundary_edges,
        r.non_manifold_edges,
        r.is_closed,
        r.is_manifold,
    );
}

/// Analyze what's happening with the corefined meshes
fn analyze_coref(label: &str, mesh_a: &TriMesh, mesh_b: &TriMesh) {
    println!("\n=== {} ===", label);
    report("input A", mesh_a);
    report("input B", mesh_b);

    let coref = corefine(mesh_a, mesh_b);
    println!("\nAfter corefinement:");
    report("corefined A", &coref.mesh_a);
    report("corefined B", &coref.mesh_b);

    // Weld corefined meshes individually to check their topology
    let mut ca = coref.mesh_a.clone();
    ca.weld_vertices(1e-10);
    report("corefined A (welded)", &ca);

    let mut cb = coref.mesh_b.clone();
    cb.weld_vertices(1e-10);
    report("corefined B (welded)", &cb);

    let class_a = classify_triangles(&coref.mesh_a, &coref.mesh_b, &coref.on_boundary_a);
    let class_b = classify_triangles(&coref.mesh_b, &coref.mesh_a, &coref.on_boundary_b);

    // Build the union result manually and check T-junctions
    let mut result = TriMesh::new();
    // Collect A outside triangles
    let mut a_tris: Vec<(Vec3d, Vec3d, Vec3d)> = Vec::new();
    for ti in 0..coref.mesh_a.triangle_count() {
        if class_a[ti] == TriLocation::Outside {
            let (v0, v1, v2) = coref.mesh_a.triangle_vertices(ti);
            a_tris.push((v0, v1, v2));
            let a = result.add_vertex(v0);
            let b = result.add_vertex(v1);
            let c = result.add_vertex(v2);
            result.add_triangle(a, b, c);
        }
    }
    // Collect B outside triangles
    let mut b_tris: Vec<(Vec3d, Vec3d, Vec3d)> = Vec::new();
    for ti in 0..coref.mesh_b.triangle_count() {
        if class_b[ti] == TriLocation::Outside {
            let (v0, v1, v2) = coref.mesh_b.triangle_vertices(ti);
            b_tris.push((v0, v1, v2));
            let a = result.add_vertex(v0);
            let b = result.add_vertex(v1);
            let c = result.add_vertex(v2);
            result.add_triangle(a, b, c);
        }
    }

    result.weld_vertices(1e-10);
    report("union result (before T-fix)", &result);

    // Find boundary edges and trace which triangles they belong to
    let r = validate_mesh(&result);
    if r.boundary_edges > 0 {
        println!("\n  Boundary edge details:");
        find_boundary_details(&result);
    }

    // Now test with the actual boolean function
    let actual = mesh_boolean(mesh_a, mesh_b, BoolOp::Union);
    report("actual mesh_boolean union", &actual);
}

fn find_boundary_details(mesh: &TriMesh) {
    type Edge = (u32, u32);
    let mut edge_count: HashMap<Edge, Vec<u32>> = HashMap::new(); // edge -> triangle indices

    for (ti, &[a, b, c]) in mesh.triangles.iter().enumerate() {
        let ti = ti as u32;
        edge_count.entry((a, b)).or_default().push(ti);
        edge_count.entry((b, c)).or_default().push(ti);
        edge_count.entry((c, a)).or_default().push(ti);
    }

    let mut undirected: HashMap<(u32, u32), (Vec<u32>, Vec<u32>)> = HashMap::new();
    for (&(a, b), tris) in &edge_count {
        let key = if a < b { (a, b) } else { (b, a) };
        let entry = undirected.entry(key).or_insert((Vec::new(), Vec::new()));
        if a < b {
            entry.0.extend(tris);
        } else {
            entry.1.extend(tris);
        }
    }

    let mut boundary_count = 0;
    for (&(a, b), (fwd, rev)) in &undirected {
        let total = fwd.len() + rev.len();
        if total == 1 {
            let va = mesh.vertices[a as usize];
            let vb = mesh.vertices[b as usize];
            let tris = if !fwd.is_empty() { fwd } else { rev };
            if boundary_count < 20 {
                println!(
                    "    bnd edge v{}({:.4},{:.4},{:.4})-v{}({:.4},{:.4},{:.4}) tri={}",
                    a, va.x, va.y, va.z, b, vb.x, vb.y, vb.z, tris[0]
                );
                // Print the triangle
                let [ta, tb, tc] = mesh.triangles[tris[0] as usize];
                let pta = mesh.vertices[ta as usize];
                let ptb = mesh.vertices[tb as usize];
                let ptc = mesh.vertices[tc as usize];
                println!("      tri[{}]: v{}({:.4},{:.4},{:.4}) v{}({:.4},{:.4},{:.4}) v{}({:.4},{:.4},{:.4})",
                    tris[0], ta, pta.x, pta.y, pta.z, tb, ptb.x, ptb.y, ptb.z, tc, ptc.x, ptc.y, ptc.z);
            }
            boundary_count += 1;
        }
        if total > 2 {
            let va = mesh.vertices[a as usize];
            let vb = mesh.vertices[b as usize];
            println!(
                "    NON-MANIFOLD edge v{}({:.4},{:.4},{:.4})-v{}({:.4},{:.4},{:.4}) fwd={} rev={}",
                a,
                va.x,
                va.y,
                va.z,
                b,
                vb.x,
                vb.y,
                vb.z,
                fwd.len(),
                rev.len()
            );
        }
    }
    if boundary_count > 20 {
        println!("    ... and {} more", boundary_count - 20);
    }
}

fn main() {
    // Test 1: Cube-Cube overlap (should now pass after T-junction fix)
    {
        let mut a = make_unit_cube();
        a.transform(Mat4d::scale_uniform(2.0));
        let mut b = make_unit_cube();
        b.transform(Mat4d::scale_uniform(2.0));
        b.transform(Mat4d::translation(dvec3(1.0, 0.0, 0.0)));
        analyze_coref("Cube-Cube x=1.0", &a, &b);
    }

    // Test 2: Cube-Cylinder overlap - the failing case
    {
        let a_solid = makepad_csg::Solid::cube(2.0, 2.0, 2.0, true);
        let cyl = makepad_csg::Solid::cylinder(1.0, 2.0, 8, true);
        analyze_coref("Cube-Cylinder overlap", a_solid.mesh(), cyl.mesh());
    }

    // Test 3: Cube-Cylinder with cylinder moved to overlap partially
    {
        let a_solid = makepad_csg::Solid::cube(2.0, 2.0, 2.0, true);
        let cyl = makepad_csg::Solid::cylinder(1.0, 2.0, 8, true).translate(1.0, 0.0, 0.0);
        analyze_coref("Cube-Cylinder offset x=1.0", a_solid.mesh(), cyl.mesh());
    }

    // Test 4: Simplest failing case — cube-cylinder centered (bnd=2 after T-fix)
    // Let's look at what the 2 remaining boundary edges are
    {
        let a_solid = makepad_csg::Solid::cube(2.0, 2.0, 2.0, true);
        let cyl = makepad_csg::Solid::cylinder(1.0, 2.0, 8, true);
        let result =
            makepad_csg::Solid::from_mesh(mesh_boolean(a_solid.mesh(), cyl.mesh(), BoolOp::Union));
        let r = result.validate();
        println!("\n=== Cube-Cylinder centered (after mesh_boolean) ===");
        println!(
            "  tris={} verts={} bnd={} nonmani={} closed={} manifold={} degen={}",
            result.triangle_count(),
            result.vertex_count(),
            r.boundary_edges,
            r.non_manifold_edges,
            r.is_closed,
            r.is_manifold,
            r.degenerate_triangles
        );
        if r.boundary_edges > 0 {
            find_boundary_details(result.mesh());
        }
    }
}
