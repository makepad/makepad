use crate::*;

#[test]
fn catmull_clark_cube_preserves_weights_uv_seams_and_composes_provenance() {
    let mut ctx = Context::default();
    let mut mesh = Mesh::cube([2.; 3], &mut ctx).unwrap();
    let old_vertex = mesh.vertices()[0].id;
    let old_corner = mesh.corners()[0].id;
    let source = mesh.face_corners(mesh.faces()[0].id).unwrap();
    let edge = EdgeKey::new(source[0].vertex, source[1].vertex);
    mesh.set_edge_attributes(
        edge,
        EdgeAttributes {
            seam: true,
            crease: 0.,
        },
        &mut ctx,
    )
    .unwrap();
    let weights = mesh
        .vertices()
        .iter()
        .enumerate()
        .map(|(i, v)| {
            (
                v.id,
                vec![JointWeight {
                    joint: (i % 2) as u32,
                    weight: 1.,
                }],
            )
        })
        .collect::<Vec<_>>();
    mesh.set_weights_bulk(&weights, &mut ctx).unwrap();
    let change = mesh.subdivide(1, &mut ctx).unwrap();
    assert_eq!(
        (
            mesh.vertices().len(),
            mesh.faces().len(),
            mesh.corners().len()
        ),
        (26, 24, 96)
    );
    for p in mesh.vertex(old_vertex).unwrap().position {
        assert!((p.abs() - 5. / 9.).abs() < 1e-12);
    }
    assert!(mesh.validate(&mut ctx).unwrap().is_closed_manifold);
    assert!(mesh
        .validate(&mut ctx)
        .unwrap()
        .issues
        .iter()
        .any(|i| i.kind == ValidationKind::NonPlanarFace));
    assert_eq!(
        mesh.edge_attributes()
            .values()
            .filter(|d| d.attributes.seam)
            .count(),
        2
    );
    let remap = change
        .remapped
        .iter()
        .find(|(source, _)| *source == ElementId::Corner(old_corner))
        .unwrap();
    let ElementId::Corner(new_corner) = remap.1[0] else {
        panic!("corner provenance")
    };
    assert_eq!(mesh.corner(new_corner).unwrap().uv, [0., 0.]);
    for v in mesh.vertices() {
        assert!((v.weights.iter().map(|w| w.weight).sum::<f64>() - 1.).abs() < 1e-12);
    }
    let mut two = Mesh::cube([2.; 3], &mut ctx).unwrap();
    let source_corner = two.corners()[0].id;
    let result = two.subdivide(2, &mut ctx).unwrap();
    assert_eq!((two.vertices().len(), two.faces().len()), (98, 96));
    let targets = &result
        .remapped
        .iter()
        .find(|(id, _)| *id == ElementId::Corner(source_corner))
        .unwrap()
        .1;
    assert_eq!(targets.len(), 1);
    let ElementId::Corner(target) = targets[0] else {
        panic!("corner")
    };
    assert!(two.corner(target).is_some());
    assert_eq!(two.triangulate(&mut ctx).unwrap().triangles.len(), 192);
    let encoded = two.to_bytes(&mut ctx).unwrap();
    assert_eq!(
        Mesh::from_bytes(&encoded, &mut ctx)
            .unwrap()
            .to_bytes(&mut ctx)
            .unwrap(),
        encoded
    );
}

#[test]
fn smooth_brush_and_projection_are_bounded_and_preserve_boundary_attributes() {
    let mut positions = Vec::new();
    for z in -1..=1 {
        for x in -1..=1 {
            positions.push([x as f64, if x == 0 && z == 0 { 1. } else { 0. }, z as f64]);
        }
    }
    let polygons = [
        vec![0, 3, 4, 1],
        vec![1, 4, 5, 2],
        vec![3, 6, 7, 4],
        vec![4, 7, 8, 5],
    ]
    .map(Polygon::new);
    let mut ctx = Context::default();
    let mut mesh = Mesh::from_polygons(&positions, &polygons, &mut ctx).unwrap();
    let ids = mesh.vertices().iter().map(|v| v.id).collect::<Vec<_>>();
    let center = ids[4];
    mesh.set_vertex_weights(
        center,
        &[JointWeight {
            joint: 7,
            weight: 1.,
        }],
        &mut ctx,
    )
    .unwrap();
    let corners = mesh
        .corners()
        .iter()
        .map(|c| (c.id, c.uv))
        .collect::<Vec<_>>();
    mesh.smooth(&ids, 1, 0.5, true, &mut ctx).unwrap();
    assert_eq!(mesh.vertex(center).unwrap().position, [0., 0.5, 0.]);
    for (i, p) in positions.iter().enumerate() {
        if i != 4 {
            assert_eq!(&mesh.vertex(ids[i]).unwrap().position, p);
        }
    }
    assert_eq!(
        mesh.corners()
            .iter()
            .map(|c| (c.id, c.uv))
            .collect::<Vec<_>>(),
        corners
    );
    assert_eq!(mesh.vertex(center).unwrap().weights[0].joint, 7);
    mesh.brush(&ids, [0., 0.5, 0.], 1., [0., 0.25, 0.], 0.25, &mut ctx)
        .unwrap();
    assert_eq!(mesh.vertex(center).unwrap().position, [0., 0.75, 0.]);
    let before = mesh.to_bytes(&mut ctx).unwrap();
    assert!(mesh
        .brush(&ids, [0.; 3], 1., [0., 1., 0.], 0.1, &mut ctx)
        .is_err());
    assert_eq!(mesh.to_bytes(&mut ctx).unwrap(), before);
    let faces = mesh.faces().iter().map(|f| f.id).collect::<Vec<_>>();
    mesh.project_uv(&faces, 1, [0.5; 2], [0.5; 2], &mut ctx)
        .unwrap();
    for c in mesh.corners() {
        assert!(c.uv.iter().all(|v| (0.0..=1.0).contains(v)));
    }
    let before = mesh.to_bytes(&mut ctx).unwrap();
    let calls = std::cell::Cell::new(0);
    let cancel = || {
        calls.set(calls.get() + 1);
        calls.get() > 40
    };
    assert!(mesh
        .smooth(
            &ids,
            64,
            0.5,
            true,
            &mut Context::new(Limits::default(), Some(&cancel))
        )
        .is_err());
    assert_eq!(mesh.to_bytes(&mut ctx).unwrap(), before);
}

#[test]
fn subdivision_preserves_fully_creased_cube_corners() {
    let mut ctx = Context::default();
    let mut mesh = Mesh::cube([1.; 3], &mut ctx).unwrap();
    let original=mesh.vertices().to_vec();
    let edges=mesh.adjacency(&mut ctx).unwrap().edges.keys().copied().collect::<Vec<_>>();
    for edge in edges {
    mesh.set_edge_attributes(
        edge,
        EdgeAttributes {
            seam: false,
            crease: 1.,
        },
        &mut ctx,
    )
    .unwrap();
    }
    mesh.subdivide(2,&mut ctx).unwrap();
    for v in original {assert_eq!(mesh.vertex(v.id).unwrap().position,v.position);}
    assert!(mesh.validate(&mut ctx).unwrap().is_closed_manifold);
    assert!(mesh.corners().iter().all(|c|c.normal.unwrap().iter().filter(|x|x.abs()>1e-8).count()==1));
}

#[test]
fn cube_extrude_preserves_topology_uv_weights_and_identity() {
    let mut ctx = Context::default();
    let mut mesh = Mesh::cube([2.; 3], &mut ctx).unwrap();
    let face = mesh.faces()[1].id;
    let vertex = mesh.face_corners(face).unwrap()[0].vertex;
    mesh.set_vertex_weights(
        vertex,
        &[
            JointWeight {
                joint: 3,
                weight: 2.,
            },
            JointWeight {
                joint: 7,
                weight: 1.,
            },
        ],
        &mut ctx,
    )
    .unwrap();
    let before = mesh.face_corners(face).unwrap().to_vec();
    let edge = EdgeKey::new(before[0].vertex, before[1].vertex);
    mesh.set_edge_attributes(
        edge,
        EdgeAttributes {
            seam: true,
            crease: 0.7,
        },
        &mut ctx,
    )
    .unwrap();
    let extrusion = mesh.extrude_face(face, [0., 0., 1.], &mut ctx).unwrap();
    assert_eq!(extrusion.cap, face);
    assert_eq!(extrusion.side_faces.len(), 4);
    let after = mesh.face_corners(face).unwrap();
    assert_eq!(
        before.iter().map(|c| (c.id, c.uv)).collect::<Vec<_>>(),
        after.iter().map(|c| (c.id, c.uv)).collect::<Vec<_>>()
    );
    assert_eq!(
        mesh.vertex(after[0].vertex).unwrap().weights,
        mesh.vertex(vertex).unwrap().weights
    );
    assert_eq!(
        mesh.edge_attributes()[&extrusion.rim_edges[0]]
            .attributes
            .crease,
        0.7
    );
    let report = mesh.validate(&mut ctx).unwrap();
    assert!(report.is_closed_manifold, "{report:?}");
    assert_eq!(mesh.triangulate(&mut ctx).unwrap().triangles.len(), 20);
    let bytes = mesh.to_bytes(&mut ctx).unwrap();
    assert_eq!(
        Mesh::from_bytes(&bytes, &mut ctx)
            .unwrap()
            .to_bytes(&mut ctx)
            .unwrap(),
        bytes
    );
}

#[test]
fn planar_inset_has_constant_distance_and_keeps_solid() {
    let mut ctx = Context::default();
    let mut m = Mesh::cube([2.; 3], &mut ctx).unwrap();
    let face = m.faces()[1].id;
    let result = m.inset_face(face, 0.25, &mut ctx).unwrap();
    assert_eq!(result.ring_faces.len(), 4);
    for c in m.face_corners(face).unwrap() {
        let p = m.vertex(c.vertex).unwrap().position;
        assert!((p[0].abs() - 0.75).abs() < 1e-12 && (p[1].abs() - 0.75).abs() < 1e-12);
        assert!(c.uv.iter().all(|v| *v >= 0.125 && *v <= 0.875));
    }
    assert!(m.validate(&mut ctx).unwrap().is_closed_manifold);
    let bytes = m.to_bytes(&mut ctx).unwrap();
    assert!(m.inset_face(face, 10., &mut ctx).is_err());
    assert_eq!(bytes, m.to_bytes(&mut ctx).unwrap());
}

#[test]
fn cancellation_and_bad_import_are_atomic() {
    let mut ctx = Context::default();
    let mut m = Mesh::plane([1., 1.], &mut ctx).unwrap();
    let before = m.to_bytes(&mut ctx).unwrap();
    let calls = std::cell::Cell::new(0);
    let cancel = || {
        let n = calls.get() + 1;
        calls.set(n);
        n > 8
    };
    let mut cancelled = Context::new(Limits::default(), Some(&cancel));
    assert_eq!(
        m.extrude_face(m.faces()[0].id, [0., 1., 0.], &mut cancelled)
            .unwrap_err(),
        MeshError::Cancelled
    );
    assert_eq!(m.to_bytes(&mut ctx).unwrap(), before);
    assert!(Mesh::from_polygons(
        &[[0., 0., 0.], [1., 0., 0.]],
        &[Polygon::new(vec![0, 1, 50])],
        &mut ctx
    )
    .is_err());
    let mut trailing = before.clone();
    trailing.push(0);
    assert!(Mesh::from_bytes(&trailing, &mut ctx).is_err());
    for n in 0..before.len() {
        assert!(Mesh::from_bytes(&before[..n], &mut Context::default()).is_err());
    }
}

#[test]
fn triangulation_handles_concavity_both_windings_and_collinear_corners() {
    let p = [
        [0., 0., 0.],
        [2., 0., 0.],
        [2., 1., 0.],
        [1., 1., 0.],
        [1., 2., 0.],
        [0., 2., 0.],
        [0., 1., 0.],
    ];
    for reversed in [false, true] {
        let mut ids = (0..p.len() as u32).collect::<Vec<_>>();
        if reversed {
            ids.reverse();
        }
        let mut ctx = Context::default();
        let m = Mesh::from_polygons(&p, &[Polygon::new(ids)], &mut ctx).unwrap();
        let t = m.triangulate(&mut ctx).unwrap();
        assert_eq!(t.triangles.len(), p.len() - 2);
        let area = t
            .triangles
            .iter()
            .map(|tri| {
                let [a, b, c] = tri.indices.map(|i| t.vertices[i as usize].position);
                let cross =
                    crate::geometry::cross(crate::geometry::sub(b, a), crate::geometry::sub(c, a));
                assert_eq!(cross[2] > 0., !reversed);
                cross[2].abs() * 0.5
            })
            .sum::<f64>();
        assert!((area - 3.).abs() < 1e-12);
        assert_eq!(t, m.triangulate(&mut ctx).unwrap());
    }
}

#[test]
fn bow_tie_vertex_and_three_face_edge_are_diagnosed() {
    let p = [
        [0., 0., 0.],
        [1., 0., 0.],
        [0., 1., 0.],
        [-1., 0., 0.],
        [0., -1., 0.],
        [0., 0., 1.],
    ];
    let mut ctx = Context::default();
    let m = Mesh::from_polygons(
        &p,
        &[Polygon::new(vec![0, 1, 2]), Polygon::new(vec![0, 3, 4])],
        &mut ctx,
    )
    .unwrap();
    assert_eq!(m.validate(&mut ctx).unwrap().non_manifold_vertices, 1);
    let m = Mesh::from_polygons(
        &p,
        &[
            Polygon::new(vec![0, 1, 2]),
            Polygon::new(vec![1, 0, 4]),
            Polygon::new(vec![0, 1, 5]),
        ],
        &mut ctx,
    )
    .unwrap();
    assert_eq!(
        m.adjacency(&mut ctx)
            .unwrap()
            .radial(EdgeKey::new(m.vertices()[0].id, m.vertices()[1].id))
            .len(),
        3
    );
    assert_eq!(m.validate(&mut ctx).unwrap().non_manifold_edges, 1);
}

#[test]
fn mirror_weld_and_delete_keep_stable_ids() {
    let mut ctx = Context::default();
    let mut m = Mesh::plane([2., 2.], &mut ctx).unwrap();
    let original = m.faces()[0].id;
    m.mirror(0, 0., &mut ctx).unwrap();
    assert_eq!(m.faces().len(), 2);
    let mirrored = m.faces()[1].id;
    let vertices = m.vertices().iter().map(|v| v.id).collect::<Vec<_>>();
    let changes = m.weld(&vertices, 0., &mut ctx).unwrap();
    assert_eq!(m.vertices().len(), 4);
    assert_eq!(
        changes
            .deleted
            .iter()
            .filter(|id| matches!(id, ElementId::Vertex(_)))
            .count(),
        4
    );
    m.delete_faces(&[original], &mut ctx).unwrap();
    assert_eq!(m.faces()[0].id, mirrored);
    assert!(m.validate(&mut ctx).unwrap().is_valid_surface);
    let before = m.to_bytes(&mut ctx).unwrap();
    let mut limits = Limits::default();
    limits.max_corners = 4;
    assert!(m
        .extrude_face(mirrored, [0., 1., 0.], &mut Context::new(limits, None))
        .is_err());
    assert_eq!(before, m.to_bytes(&mut ctx).unwrap());
}
