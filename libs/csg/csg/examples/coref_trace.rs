// Trace corefinement boolean for two overlapping cubes.
// Identify where boundary edges come from.

use makepad_csg_boolean::classify::{classify_triangles, TriLocation};
use makepad_csg_boolean::corefine::corefine;
use makepad_csg_math::{dvec3, Mat4d, Vec3d};
use makepad_csg_mesh::mesh::{make_unit_cube, TriMesh};
use makepad_csg_mesh::validate::validate_mesh;
use makepad_csg_mesh::volume::mesh_volume;
use std::collections::HashMap;

fn report_mesh(label: &str, mesh: &TriMesh) {
    let r = validate_mesh(mesh);
    let vol = mesh_volume(mesh);
    println!(
        "  {:40} tris={:3}  verts={:3}  vol={:8.4}  closed={}  manifold={}  bnd={}  nonmani={}",
        label,
        mesh.triangle_count(),
        mesh.vertex_count(),
        vol,
        r.is_closed,
        r.is_manifold,
        r.boundary_edges,
        r.non_manifold_edges,
    );
}

fn analyze_boundary_edges(mesh: &TriMesh) {
    let r = validate_mesh(mesh);
    if r.boundary_edges == 0 {
        println!("  No boundary edges!");
        return;
    }

    type Edge = (u32, u32);
    let mut edge_count: HashMap<Edge, u32> = HashMap::new();
    for &[a, b, c] in &mesh.triangles {
        *edge_count.entry((a, b)).or_insert(0) += 1;
        *edge_count.entry((b, c)).or_insert(0) += 1;
        *edge_count.entry((c, a)).or_insert(0) += 1;
    }

    let mut undirected: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    for (&(a, b), &count) in &edge_count {
        let key = if a < b { (a, b) } else { (b, a) };
        let entry = undirected.entry(key).or_insert((0, 0));
        if a < b {
            entry.0 += count;
        } else {
            entry.1 += count;
        }
    }

    let mut count = 0;
    for (&(a, b), &(fwd, rev)) in &undirected {
        let total = fwd + rev;
        if total == 1 {
            let va = mesh.vertices[a as usize];
            let vb = mesh.vertices[b as usize];
            if count < 15 {
                println!(
                    "  boundary: ({:.4},{:.4},{:.4})-({:.4},{:.4},{:.4}) fwd={} rev={}",
                    va.x, va.y, va.z, vb.x, vb.y, vb.z, fwd, rev
                );
            }
            count += 1;
        }
        if total > 2 {
            let va = mesh.vertices[a as usize];
            let vb = mesh.vertices[b as usize];
            println!(
                "  NON-MANIFOLD: ({:.4},{:.4},{:.4})-({:.4},{:.4},{:.4}) fwd={} rev={} total={}",
                va.x, va.y, va.z, vb.x, vb.y, vb.z, fwd, rev, total
            );
        }
    }
    if count > 15 {
        println!("  ... and {} more boundary edges", count - 15);
    }
}

fn main() {
    println!("=== Corefinement trace: two 2x2x2 cubes offset x=1.0 ===\n");

    let mut mesh_a = make_unit_cube();
    mesh_a.transform(Mat4d::scale_uniform(2.0));

    let mut mesh_b = make_unit_cube();
    mesh_b.transform(Mat4d::scale_uniform(2.0));
    mesh_b.transform(Mat4d::translation(dvec3(1.0, 0.0, 0.0)));

    report_mesh("input A", &mesh_a);
    report_mesh("input B", &mesh_b);

    // Step 1: Corefine
    let coref = corefine(&mesh_a, &mesh_b);
    println!("\nAfter corefinement:");
    report_mesh("corefined A", &coref.mesh_a);
    report_mesh("corefined B", &coref.mesh_b);

    // Check boundary triangles
    let boundary_a: usize = coref.on_boundary_a.iter().filter(|&&b| b).count();
    let boundary_b: usize = coref.on_boundary_b.iter().filter(|&&b| b).count();
    println!(
        "  on_boundary_a: {} of {}",
        boundary_a,
        coref.mesh_a.triangle_count()
    );
    println!(
        "  on_boundary_b: {} of {}",
        boundary_b,
        coref.mesh_b.triangle_count()
    );

    // Step 2: Classify
    let class_a = classify_triangles(&coref.mesh_a, &coref.mesh_b, &coref.on_boundary_a);
    let class_b = classify_triangles(&coref.mesh_b, &coref.mesh_a, &coref.on_boundary_b);

    let mut a_counts = [0usize; 3]; // Inside, Outside, OnBoundary
    for &c in &class_a {
        match c {
            TriLocation::Inside => a_counts[0] += 1,
            TriLocation::Outside => a_counts[1] += 1,
            TriLocation::OnBoundary => a_counts[2] += 1,
        }
    }
    let mut b_counts = [0usize; 3];
    for &c in &class_b {
        match c {
            TriLocation::Inside => b_counts[0] += 1,
            TriLocation::Outside => b_counts[1] += 1,
            TriLocation::OnBoundary => b_counts[2] += 1,
        }
    }
    println!(
        "\nClassification A: inside={} outside={} on_boundary={}",
        a_counts[0], a_counts[1], a_counts[2]
    );
    println!(
        "Classification B: inside={} outside={} on_boundary={}",
        b_counts[0], b_counts[1], b_counts[2]
    );

    // Step 3: Manually do union selection
    println!("\n--- Manual union selection ---");
    let mut result = TriMesh::new();

    // From A: keep Outside
    for ti in 0..coref.mesh_a.triangle_count() {
        if class_a[ti] == TriLocation::Outside {
            let (v0, v1, v2) = coref.mesh_a.triangle_vertices(ti);
            let a = result.add_vertex(v0);
            let b = result.add_vertex(v1);
            let c = result.add_vertex(v2);
            result.add_triangle(a, b, c);
        }
    }
    let a_kept = result.triangle_count();
    println!("  A outside tris kept: {}", a_kept);

    // From B: keep Outside
    for ti in 0..coref.mesh_b.triangle_count() {
        if class_b[ti] == TriLocation::Outside {
            let (v0, v1, v2) = coref.mesh_b.triangle_vertices(ti);
            let a = result.add_vertex(v0);
            let b = result.add_vertex(v1);
            let c = result.add_vertex(v2);
            result.add_triangle(a, b, c);
        }
    }
    let b_kept = result.triangle_count() - a_kept;
    println!("  B outside tris kept: {}", b_kept);

    result.weld_vertices(1e-10);
    report_mesh("union result (simple)", &result);
    analyze_boundary_edges(&result);

    // Print details of each corefined triangle
    println!("\n--- Corefined A triangles ---");
    for ti in 0..coref.mesh_a.triangle_count() {
        let (v0, v1, v2) = coref.mesh_a.triangle_vertices(ti);
        let bnd = coref.on_boundary_a[ti];
        let cls = class_a[ti];
        println!("  A[{:2}] {:?}{} ({:.3},{:.3},{:.3})-({:.3},{:.3},{:.3})-({:.3},{:.3},{:.3}) origin={}",
            ti, cls, if bnd { " BND" } else { "" },
            v0.x, v0.y, v0.z, v1.x, v1.y, v1.z, v2.x, v2.y, v2.z,
            coref.origin_a[ti]);
    }

    println!("\n--- Corefined B triangles ---");
    for ti in 0..coref.mesh_b.triangle_count() {
        let (v0, v1, v2) = coref.mesh_b.triangle_vertices(ti);
        let bnd = coref.on_boundary_b[ti];
        let cls = class_b[ti];
        println!("  B[{:2}] {:?}{} ({:.3},{:.3},{:.3})-({:.3},{:.3},{:.3})-({:.3},{:.3},{:.3}) origin={}",
            ti, cls, if bnd { " BND" } else { "" },
            v0.x, v0.y, v0.z, v1.x, v1.y, v1.z, v2.x, v2.y, v2.z,
            coref.origin_b[ti]);
    }
}
