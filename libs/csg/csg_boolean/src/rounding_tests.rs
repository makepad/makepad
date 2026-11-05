use crate::boolean::{mesh_boolean_with, BoolOp, FinishParams};
use makepad_csg_math::{dvec3, Mat4d};
use makepad_csg_mesh::{validate::validate_mesh, volume::mesh_volume, TriMesh};
use makepad_csg_primitives::{cube, cylinder, sphere, tapered_cylinder};
use makepad_csg_sdf::{mesh_field, SdfNode, SdfOp, SdfShape};
use std::f64::consts::PI;

fn closed(mesh: &TriMesh) {
    let r = validate_mesh(mesh);
    eprintln!("open={} nonmanifold={} degenerate={} volume={:.12}", r.boundary_edges, r.non_manifold_edges, r.degenerate_triangles, mesh_volume(mesh));
    assert_eq!((r.boundary_edges, r.non_manifold_edges, r.degenerate_triangles), (0, 0, 0), "{r:?}");
    assert!(r.is_consistently_oriented, "{r:?}");
}

fn boolean(a: &TriMesh, b: &TriMesh, op: BoolOp) -> TriMesh {
    let bb = a.bounding_box().union(b.bounding_box());
    let tolerance = ((bb.max-bb.min).length()*1e-6).clamp(1e-5, 1e-3);
    mesh_boolean_with(a,b,op,FinishParams { tolerance })
}

#[test]
fn rounded_grid_cap() {
    // A cap produced by several constructions can straddle zero at rounding
    // scale. Every patch must reach the same coplanar classification rule.
    let n = 3u32;
    let low = -1.4710455076283324e-15;
    let high = 2e-16;
    let heights = [low,low,low,low, low,low,high,low, low,high,high,0.0, high,low,high,high];
    let mut a = TriMesh::new();
    for z in 0..=n { for x in 0..=n {
        a.add_vertex(dvec3(-2.0+4.0*x as f64/n as f64,heights[(z*(n+1)+x) as usize],-2.0+4.0*z as f64/n as f64));
    }}
    let count=a.vertex_count() as u32;
    for i in 0..count {let v=a.vertices[i as usize];a.add_vertex(dvec3(v.x,2.0,v.z));}
    for z in 0..n {for x in 0..n {
        let i=z*(n+1)+x;let j=i+1;let k=i+n+2;let l=i+n+1;
        a.add_triangle(i,j,k);a.add_triangle(i,k,l);
        a.add_triangle(i+count,k+count,j+count);a.add_triangle(i+count,l+count,k+count);
    }}
    let mut ring=Vec::new();
    ring.extend(0..n);ring.extend((0..n).map(|z|z*(n+1)+n));
    ring.extend((1..=n).rev().map(|x|n*(n+1)+x));ring.extend((1..=n).rev().map(|z|z*(n+1)));
    for i in 0..ring.len(){let u=ring[i];let v=ring[(i+1)%ring.len()];a.add_triangle(u,v+count,v);a.add_triangle(u,u+count,v+count);}
    let b=cylinder(1.4,3.0,16,false);
    closed(&a);closed(&b);
    let area=8.0*1.4f64.powi(2)*(PI/8.0).sin();
    for (left,right,difference) in [(&a,&b,32.0-2.0*area),(&b,&a,area)] {
        for (op,expected) in [(BoolOp::Union,32.0+area),(BoolOp::Difference,difference),(BoolOp::Intersection,2.0*area)] {
            let out=boolean(left,right,op);
            closed(&out);
            assert!((mesh_volume(&out)-expected).abs()<1e-10,"{op:?}: expected {expected}");
        }
    }
}

#[test]
fn meshed_dome_with_flush_columns() {
    let at = |p, of| SdfNode::Transform { at: p, rotate: [0.0; 3], scale: 1.0, of: Box::new(of) };
    let node = SdfNode::Combine { op: SdfOp::Difference, blend: 0.0, of: vec![
        at([-75.0, 0.0, 0.0], SdfNode::Ellipsoid { radii: [6.0, 9.0, 7.5] }),
        at([0.0, -20.0, 0.0], SdfNode::Box { size: [400.0, 40.0, 200.0], radius: 0.0 }),
    ] };
    let shape = SdfShape { root: node, min: [-81.0, -9.0, -7.5], max: [-69.0, 9.0, 7.5], materials: vec![] };
    let mut mesh = mesh_field(shape.field(), dvec3(-81.0, -9.0, -7.5), dvec3(-69.0, 9.0, 7.5), 0.25, shape.lipschitz());
    closed(&mesh);
    let hemisphere = 2.0*PI*6.0*9.0*7.5/3.0;
    assert!((mesh_volume(&mesh)-hemisphere).abs()<hemisphere*0.01);
    for (x,h) in [(-69.0,9.0),(-81.0,8.0)] {
        let mut column=cylinder(3.6,h,48,false);
        column.transform(Mat4d::translation(dvec3(x,0.0,0.0)));
        let before=mesh_volume(&mesh);
        let added=mesh_volume(&column);
        mesh=boolean(&mesh,&column,BoolOp::Union);
        closed(&mesh);
        // Each column is centred at an ellipsoid tip, so at least half its
        // volume lies outside the dome, and some lies inside it.
        assert!(mesh_volume(&mesh)>before+added*0.5);
        assert!(mesh_volume(&mesh)<before+added);
    }
}

#[test]
fn rounded_box_with_clipped_socket_shell() {
    let mut body=rounded_box([22.0,10.0,26.0],1.5,6);
    body.transform(Mat4d::translation(dvec3(0.0,5.0,0.0)));
    let rad=6.3;
    let h1=rad/std::f64::consts::SQRT_2-0.1*rad;
    let r1=(rad*rad-h1*h1).sqrt()-0.004*rad;
    let mut ball=sphere(rad,96,48);
    ball.transform(Mat4d::translation(dvec3(0.0,5.0,13.0)));
    let mut roof=tapered_cylinder(r1,0.0,rad*std::f64::consts::SQRT_2-h1,96,false);
    roof.transform(Mat4d::translation(dvec3(0.0,5.0+h1,13.0)));
    let mut shell=boolean(&ball,&roof,BoolOp::Union);
    let mut slab=cube(dvec3(50.0,9.9,50.0),true);
    slab.transform(Mat4d::translation(dvec3(0.0,5.0,13.0)));
    shell=boolean(&shell,&slab,BoolOp::Intersection);
    closed(&body);closed(&shell);
    let out=boolean(&body,&shell,BoolOp::Union);
    closed(&out);
    assert!(mesh_volume(&out)>mesh_volume(&body));
    assert!(mesh_volume(&out)<mesh_volume(&body)+mesh_volume(&shell));
}

fn rounded_box(size: [f64; 3], r: f64, arc: usize) -> TriMesh {
    let half = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
    let inner = [half[0] - r, half[1] - r, half[2] - r];
    // Grid coordinates along each axis: the arc at each end and the flat between.
    let coords = |a: usize| -> Vec<f64> {
        let mut c: Vec<f64> = (0..=arc).map(|j| -inner[a] - r * (std::f64::consts::FRAC_PI_2 * (1.0 - j as f64 / arc as f64)).sin()).collect();
        let mirrored: Vec<f64> = c.iter().rev().map(|x| -x).collect();
        if inner[a] > 1e-12 {
            c.extend(mirrored);
        } else {
            c.extend(mirrored.into_iter().skip(1));
        }
        c
    };
    let axes = [coords(0), coords(1), coords(2)];
    let mut m = TriMesh::new();
    let mut ids: std::collections::HashMap<[u64; 3], u32> = std::collections::HashMap::new();
    let mut vertex = |m: &mut TriMesh, p: [f64; 3]| -> u32 {
        let q = [p[0].clamp(-inner[0], inner[0]), p[1].clamp(-inner[1], inner[1]), p[2].clamp(-inner[2], inner[2])];
        let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-300);
        let s = [q[0] + d[0] / l * r, q[1] + d[1] / l * r, q[2] + d[2] / l * r];
        // Weld by the grid point (faces share their border points exactly).
        *ids.entry([p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]).or_insert_with(|| m.add_vertex(dvec3(s[0], s[1], s[2])))
    };
    // Each face: the normal axis n at side +-1, spanned by axes u, v (u x v = +n).
    for (n, u, v) in [(0usize, 1usize, 2usize), (1, 2, 0), (2, 0, 1)] {
        for side in [-1.0f64, 1.0] {
            let (cu, cv) = (&axes[u], &axes[v]);
            let mut grid = vec![vec![0u32; cv.len()]; cu.len()];
            for (i, a) in cu.iter().enumerate() {
                for (j, b) in cv.iter().enumerate() {
                    let mut p = [0.0; 3];
                    p[n] = side * half[n];
                    p[u] = *a;
                    p[v] = *b;
                    grid[i][j] = vertex(&mut m, p);
                }
            }
            for i in 0..cu.len() - 1 {
                for j in 0..cv.len() - 1 {
                    let (a, b, c, d) = (grid[i][j], grid[i + 1][j], grid[i + 1][j + 1], grid[i][j + 1]);
                    if side > 0.0 {
                        m.add_triangle(a, b, c);
                        m.add_triangle(a, c, d);
                    } else {
                        m.add_triangle(a, c, b);
                        m.add_triangle(a, d, c);
                    }
                }
            }
        }
    }
    // Arc points that coincide (a rounding as large as a half side) leave slivers.
    m.triangles.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
    m
}
