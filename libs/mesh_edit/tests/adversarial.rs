//! Adversarial / property-style checks against the public mesh kernel.
//! These fixtures encode claimed topology, identity, codec and rollback
//! contracts. They are not transcribed from the implementation.
use makepad_mesh_edit::*;
use std::cell::Cell;

fn ctx() -> Context<'static> {
    Context::default()
}

fn ident() -> [[f64; 4]; 4] {
    [
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [0., 0., 1., 0.],
        [0., 0., 0., 1.],
    ]
}

fn bytes_of(mesh: &Mesh) -> Vec<u8> {
    mesh.to_bytes(&mut ctx()).expect("canonical bytes")
}

fn tetrahedron(offset: [f64; 3]) -> (Vec<[f64; 3]>, Vec<Polygon>) {
    let p = vec![
        [offset[0], offset[1], offset[2]],
        [offset[0] + 1., offset[1], offset[2]],
        [offset[0], offset[1] + 1., offset[2]],
        [offset[0], offset[1], offset[2] + 1.],
    ];
    let faces = vec![
        Polygon::new(vec![0, 2, 1]),
        Polygon::new(vec![0, 1, 3]),
        Polygon::new(vec![0, 3, 2]),
        Polygon::new(vec![1, 2, 3]),
    ];
    (p, faces)
}

#[test]
fn plane_is_open_surface_not_solid() {
    let mut c = ctx();
    let mesh = Mesh::plane([2., 3.], &mut c).unwrap();
    let report = mesh.validate(&mut c).unwrap();
    assert!(report.is_valid_surface, "{report:?}");
    assert!(report.is_surface_manifold, "{report:?}");
    assert!(!report.is_closed, "{report:?}");
    assert!(!report.is_valid_solid, "{report:?}");
    assert_eq!(report.boundary_edges, 4);
}

#[test]
fn cube_is_closed_manifold_and_roundtrips() {
    let mut c = ctx();
    let mesh = Mesh::cube([1., 2., 3.], &mut c).unwrap();
    let report = mesh.validate(&mut c).unwrap();
    assert!(report.is_closed_manifold, "{report:?}");
    assert!(report.is_valid_surface, "{report:?}");
    assert_eq!(report.boundary_edges, 0);
    assert_eq!(report.non_manifold_edges, 0);
    // Conservative: NotChecked must not certify an embedded solid.
    assert!(
        report.self_intersections != SelfIntersectionStatus::NotChecked || !report.is_valid_solid,
        "{report:?}"
    );
    let bytes = bytes_of(&mesh);
    let again = Mesh::from_bytes(&bytes, &mut ctx()).unwrap();
    assert_eq!(bytes_of(&again), bytes);
    assert_eq!(again.vertices().len(), 8);
    assert_eq!(again.faces().len(), 6);
}

#[test]
fn regular_ngons_triangulate_deterministically() {
    for n in 3..=8 {
        let mut positions = Vec::new();
        for i in 0..n {
            let a = (i as f64) * std::f64::consts::TAU / (n as f64);
            positions.push([a.cos(), a.sin(), 0.]);
        }
        let mut c = ctx();
        let mesh = Mesh::from_polygons(
            &positions,
            &[Polygon::new((0..n as u32).collect())],
            &mut c,
        )
        .unwrap();
        let tri = mesh.triangulate(&mut c).unwrap();
        assert_eq!(tri.triangles.len(), n - 2, "n={n}");
        assert_eq!(tri, mesh.triangulate(&mut c).unwrap());
        let bytes = bytes_of(&mesh);
        assert_eq!(bytes_of(&Mesh::from_bytes(&bytes, &mut ctx()).unwrap()), bytes);
        let report = mesh.validate(&mut c).unwrap();
        assert!(report.is_valid_surface, "n={n} {report:?}");
        assert!(!report.is_closed, "n={n}");
    }
}

#[test]
fn corner_uvs_remain_discontinuous_across_shared_vertex() {
    let positions = [[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]];
    let mut left = Polygon::new(vec![0, 1, 3]);
    left.uvs = vec![[0., 0.], [1., 0.], [0., 1.]];
    let mut right = Polygon::new(vec![1, 2, 3]);
    right.uvs = vec![[0., 0.], [1., 0.], [0., 1.]];
    let mut c = ctx();
    let mesh = Mesh::from_polygons(&positions, &[left, right], &mut c).unwrap();
    let shared = mesh.vertices()[1].id;
    let uvs: Vec<_> = mesh
        .corners()
        .iter()
        .filter(|corner| corner.vertex == shared)
        .map(|corner| corner.uv)
        .collect();
    assert_eq!(uvs.len(), 2);
    assert_ne!(uvs[0], uvs[1]);
    let tri = mesh.triangulate(&mut c).unwrap();
    let tri_uvs: Vec<_> = tri
        .vertices
        .iter()
        .filter(|v| v.source_vertex == shared)
        .map(|v| v.uv)
        .collect();
    assert_eq!(tri_uvs.len(), 2);
    assert_ne!(tri_uvs[0], tri_uvs[1]);
}

#[test]
fn delete_faces_keeps_vertex_ids_and_does_not_reuse_face_ids() {
    let mut c = ctx();
    let mut mesh = Mesh::cube([2., 2., 2.], &mut c).unwrap();
    let before = mesh.faces().iter().map(|f| f.id).collect::<Vec<_>>();
    let removed = before[0];
    let kept: Vec<_> = before.iter().copied().skip(1).collect();
    let max_id_before = mesh
        .corners()
        .iter()
        .map(|c| c.id.0)
        .chain(mesh.faces().iter().map(|f| f.id.0))
        .chain(mesh.vertices().iter().map(|v| v.id.0))
        .max()
        .unwrap();
    let vertex_ids: Vec<_> = mesh.vertices().iter().map(|v| v.id).collect();
    mesh.delete_faces(&[removed], &mut c).unwrap();
    assert!(mesh.face(removed).is_none());
    assert_eq!(
        mesh.vertices().iter().map(|v| v.id).collect::<Vec<_>>(),
        vertex_ids
    );
    for id in &kept {
        assert!(mesh.face(*id).is_some(), "lost face {id:?}");
    }
    assert_eq!(
        mesh.delete_faces(&[removed], &mut c).unwrap_err(),
        MeshError::UnknownElement(ElementId::Face(removed))
    );
    let cap = mesh.faces()[0].id;
    // [1,1,1] leaves every axis-aligned cube face.
    let extrusion = mesh.extrude_face(cap, [1., 1., 1.], &mut c).unwrap();
    for id in extrusion.side_faces {
        assert!(id.0 > max_id_before, "reused face id {id:?}");
    }
    for v in mesh.vertices() {
        if !vertex_ids.contains(&v.id) {
            assert!(v.id.0 > max_id_before, "reused vertex id {:?}", v.id);
        }
    }
}

#[test]
fn extrude_preserves_cap_identity_and_rolls_back_on_in_plane_offset() {
    let mut c = ctx();
    let mut mesh = Mesh::cube([2., 2., 2.], &mut c).unwrap();
    let face = mesh.faces()[0].id;
    let corners_before: Vec<_> = mesh
        .face_corners(face)
        .unwrap()
        .iter()
        .map(|c| (c.id, c.uv, c.vertex))
        .collect();
    let snapshot = bytes_of(&mesh);
    assert!(mesh.extrude_face(face, [0., 0., 0.], &mut c).is_err());
    assert_eq!(bytes_of(&mesh), snapshot);
    let result = mesh.extrude_face(face, [0., 0., 1.], &mut c).unwrap();
    assert_eq!(result.cap, face);
    let corners_after: Vec<_> = mesh
        .face_corners(face)
        .unwrap()
        .iter()
        .map(|c| (c.id, c.uv))
        .collect();
    assert_eq!(
        corners_before
            .iter()
            .map(|(id, uv, _)| (*id, *uv))
            .collect::<Vec<_>>(),
        corners_after
    );
    for (_, _, vertex) in &corners_before {
        assert!(mesh.vertex(*vertex).is_some());
    }
    assert!(result
        .changes
        .deleted
        .iter()
        .all(|id| *id != ElementId::Face(face)));
}

#[test]
fn weld_of_opposite_quad_vertices_rolls_back_pinched_face() {
    let positions = [[0., 0., 0.], [2., 0., 0.], [2., 2., 0.], [0., 2., 0.]];
    let mut c = ctx();
    let mut mesh = Mesh::from_polygons(&positions, &[Polygon::new(vec![0, 1, 2, 3])], &mut c)
        .unwrap();
    let a = mesh.vertices()[0].id;
    let c_id = mesh.vertices()[2].id;
    let snapshot = bytes_of(&mesh);
    assert!(mesh.weld(&[a, c_id], 10., &mut c).is_err());
    assert_eq!(bytes_of(&mesh), snapshot);
}

#[test]
fn transform_empty_selection_is_noop_not_all_vertices() {
    let mut c = ctx();
    let mut mesh = Mesh::from_polygons(
        &[[0., 0., 0.], [2., 0., 0.], [0., 2., 0.]],
        &[Polygon::new(vec![0, 1, 2])],
        &mut c,
    )
    .unwrap();
    let snapshot = bytes_of(&mesh);
    let mut matrix = ident();
    matrix[0][3] = 10.;
    mesh.transform(&[], matrix, &mut c).unwrap();
    assert_eq!(bytes_of(&mesh), snapshot);
    let one = [mesh.vertices()[0].id];
    mesh.transform(&one, matrix, &mut c).unwrap();
    assert_ne!(bytes_of(&mesh), snapshot);
    assert!((mesh.vertex(one[0]).unwrap().position[0] - 10.).abs() < 1e-12);
    assert!((mesh.vertices()[1].position[0] - 2.).abs() < 1e-12);
}

#[test]
fn failed_ops_leave_canonical_bytes_unchanged() {
    let mut c = ctx();
    let mut mesh = Mesh::cube([1., 1., 1.], &mut c).unwrap();
    let face = mesh.faces()[0].id;
    let snapshot = bytes_of(&mesh);
    let mut nan = ident();
    nan[0][0] = f64::NAN;
    assert!(mesh.extrude_face(face, [0., 0., 0.], &mut c).is_err());
    assert!(mesh.inset_face(face, 10., &mut c).is_err());
    assert!(mesh.inset_face(face, 0., &mut c).is_err());
    assert!(mesh.mirror(3, 0., &mut c).is_err());
    let vertex = mesh.vertices()[0].id;
    assert!(mesh.transform(&[vertex], nan, &mut c).is_err());
    assert!(mesh.delete_faces(&[FaceId(0)], &mut c).is_err());
    assert!(mesh.set_corner_uv(CornerId(0), [0., 0.], &mut c).is_err());
    assert_eq!(bytes_of(&mesh), snapshot);
}

#[test]
fn cancellation_during_extrude_is_atomic() {
    let mut mesh = Mesh::cube([2., 2., 2.], &mut ctx()).unwrap();
    let face = mesh.faces()[0].id;
    let snapshot = bytes_of(&mesh);
    let calls = Cell::new(0u32);
    let cancel = || {
        let n = calls.get() + 1;
        calls.set(n);
        n > 6
    };
    let mut cancelled = Context::new(Limits::default(), Some(&cancel));
    assert_eq!(
        mesh.extrude_face(face, [0., 1., 0.], &mut cancelled)
            .unwrap_err(),
        MeshError::Cancelled
    );
    assert_eq!(bytes_of(&mesh), snapshot);
}

#[test]
fn self_intersecting_face_is_refused() {
    // Include cancelling Newell area as well as an unequal-wing bow tie.
    for positions in [
        [[0., 0., 0.], [3., 2., 0.], [0., 1., 0.], [1., 0., 0.]],
        [[0., 0., 0.], [1., 1., 0.], [1., 0., 0.], [0., 1., 0.]],
    ] {
        let err = Mesh::from_polygons(&positions, &[Polygon::new(vec![0, 1, 2, 3])], &mut ctx())
            .unwrap_err();
        match err {
            MeshError::InvalidGeometry {
                kind: ValidationKind::SelfIntersectingFace,
                ..
            } => {}
            other => panic!("expected self-intersecting face, got {other:?}"),
        }
    }
}

#[test]
fn nonplanar_cage_quad_is_triangulated_and_warns_but_planar_inset_refuses() {
    let positions = [[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.5]];
    let mut c = ctx();
    let mut mesh = Mesh::from_polygons(&positions, &[Polygon::new(vec![0, 1, 2, 3])], &mut c)
        .expect("non-planar faces are representable");
    let report = mesh.validate(&mut c).unwrap();
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.kind == ValidationKind::NonPlanarFace),
        "{report:?}"
    );
    assert!(
        report.is_valid_surface && !report.is_valid_solid,
        "a simple projected cage face is a surface, not a solid certificate: {report:?}"
    );
    let triangles=mesh.triangulate(&mut c).unwrap();
    assert_eq!(triangles.triangles.len(),2);
    let snapshot=bytes_of(&mesh);
    assert_eq!(Mesh::from_bytes(&snapshot,&mut ctx()).unwrap().triangulate(&mut ctx()).unwrap(),triangles);
    assert!(mesh.inset_face(mesh.faces()[0].id,0.1,&mut c).is_err());
    assert_eq!(bytes_of(&mesh),snapshot);
}

#[test]
fn three_faces_on_one_edge_are_nonmanifold_not_a_solid() {
    let positions = [
        [0., 0., 0.],
        [1., 0., 0.],
        [0.5, 1., 0.],
        [0.5, -1., 0.],
        [0.5, 0., 1.],
    ];
    let mut c = ctx();
    let mesh = Mesh::from_polygons(
        &positions,
        &[
            Polygon::new(vec![0, 1, 2]),
            Polygon::new(vec![0, 1, 3]),
            Polygon::new(vec![0, 1, 4]),
        ],
        &mut c,
    )
    .unwrap();
    let report = mesh.validate(&mut c).unwrap();
    assert!(report.non_manifold_edges >= 1, "{report:?}");
    assert!(!report.is_surface_manifold, "{report:?}");
    assert!(!report.is_valid_solid, "{report:?}");
}

#[test]
fn bowtie_vertex_is_diagnosed_as_nonmanifold_vertex() {
    let positions = [
        [1., 1., 0.],
        [2., 1., 0.],
        [1.5, 2., 0.],
        [0., 0., 0.],
        [-1., 0., 0.],
        [0., -1., 0.],
    ];
    let mut c = ctx();
    let mesh = Mesh::from_polygons(
        &positions,
        &[
            Polygon::new(vec![0, 1, 2]),
            Polygon::new(vec![3, 4, 5]),
        ],
        &mut c,
    )
    .unwrap();
    // Two disjoint triangles are manifold. Join them by welding a pair of
    // distinct vertices onto one identity through a constructed bowtie:
    // share vertex 0 between two fans that only meet at that point.
    let bow = Mesh::from_polygons(
        &[
            [0., 0., 0.],
            [1., 0., 0.],
            [0., 1., 0.],
            [-1., 0., 0.],
            [0., -1., 0.],
        ],
        &[Polygon::new(vec![0, 1, 2]), Polygon::new(vec![0, 3, 4])],
        &mut c,
    )
    .unwrap();
    let report = bow.validate(&mut c).unwrap();
    assert_eq!(report.non_manifold_vertices, 1, "{report:?}");
    assert!(!report.is_valid_surface, "{report:?}");
    let _ = mesh;
}

#[test]
fn interpenetrating_closed_solids_are_not_an_embedded_solid() {
    let (mut positions, mut faces) = tetrahedron([0., 0., 0.]);
    let (other, other_faces) = tetrahedron([0.25, 0.25, 0.25]);
    let base = positions.len() as u32;
    positions.extend(other);
    for mut face in other_faces {
        for v in &mut face.vertices {
            *v += base;
        }
        faces.push(face);
    }
    let mut c = ctx();
    let mesh = Mesh::from_polygons(&positions, &faces, &mut c).unwrap();
    let report = mesh.validate(&mut c).unwrap();
    // Topology can still call this a closed manifold. Embedded-solid
    // qualification must not come back true while intersections are unchecked.
    assert!(report.is_closed_manifold, "{report:?}");
    assert!(
        !report.is_valid_solid
            || report.self_intersections != SelfIntersectionStatus::NotChecked,
        "interpenetrating solids claimed valid without intersection check: {report:?}"
    );
}

#[test]
fn isolated_vertices_after_face_delete_are_diagnosed() {
    let mut c = ctx();
    let mut mesh = Mesh::plane([1., 1.], &mut c).unwrap();
    let face = mesh.faces()[0].id;
    let vertex_ids: Vec<_> = mesh.vertices().iter().map(|v| v.id).collect();
    mesh.delete_faces(&[face], &mut c).unwrap();
    assert_eq!(mesh.faces().len(), 0);
    assert_eq!(
        mesh.vertices().iter().map(|v| v.id).collect::<Vec<_>>(),
        vertex_ids
    );
    let report = mesh.validate(&mut c).unwrap();
    assert_eq!(report.isolated_vertices, 4, "{report:?}");
    assert!(!report.is_closed, "{report:?}");
}

#[test]
fn loose_edge_is_representable_and_not_a_closed_solid() {
    let mut c = ctx();
    let mut mesh = Mesh::plane([1., 1.], &mut c).unwrap();
    let a = mesh.vertices()[0].id;
    let b = mesh.vertices()[2].id;
    mesh.add_loose_edge(a, b, &mut c).unwrap();
    let report = mesh.validate(&mut c).unwrap();
    assert!(report.loose_edges >= 1, "{report:?}");
    assert!(!report.is_valid_solid, "{report:?}");
}

#[test]
fn canonical_codec_rejects_truncation_trailing_negative_zero_and_bad_flags() {
    let mesh = Mesh::plane([1., 1.], &mut ctx()).unwrap();
    let bytes = bytes_of(&mesh);
    for n in [0usize, 1, 8, 31, 32, bytes.len() / 2, bytes.len() - 1] {
        assert!(
            Mesh::from_bytes(&bytes[..n], &mut ctx()).is_err(),
            "accepted truncated length {n}"
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(Mesh::from_bytes(&trailing, &mut ctx()).is_err());

    let mut bad_magic = bytes.clone();
    bad_magic[0] ^= 1;
    assert!(Mesh::from_bytes(&bad_magic, &mut ctx()).is_err());

    // Header is 32 bytes; first vertex id is 8 bytes; first coordinate follows.
    let mut neg_zero = bytes.clone();
    let coord = 40;
    neg_zero[coord..coord + 8].copy_from_slice(&(-0.0f64).to_bits().to_le_bytes());
    match Mesh::from_bytes(&neg_zero, &mut ctx()) {
        Err(MeshError::CorruptData(_)) => {}
        other => panic!("negative zero must be noncanonical, got {other:?}"),
    }

    let mut next_id = bytes.clone();
    next_id[8..16].copy_from_slice(&0u64.to_le_bytes());
    assert!(Mesh::from_bytes(&next_id, &mut ctx()).is_err());
}

#[test]
fn codec_rejects_noncanonical_vertex_order_and_duplicate_identities() {
    let mesh = Mesh::plane([1., 1.], &mut ctx()).unwrap();
    let bytes = bytes_of(&mesh);
    // Two 36-byte unweighted vertex records start at offset 32.
    let mut swapped = bytes.clone();
    let a = 32usize;
    let b = 32 + 36;
    let tmp = bytes[a..a + 36].to_vec();
    swapped[a..a + 36].copy_from_slice(&bytes[b..b + 36]);
    swapped[b..b + 36].copy_from_slice(&tmp);
    assert!(Mesh::from_bytes(&swapped, &mut ctx()).is_err());

    let mut dup = bytes.clone();
    dup[32..40].copy_from_slice(&bytes[32 + 36..32 + 44]);
    assert!(Mesh::from_bytes(&dup, &mut ctx()).is_err());
}

#[test]
fn hostile_counts_are_bounded_before_install() {
    let mut header = Vec::from(*b"MPMESH01");
    header.extend_from_slice(&1u64.to_le_bytes());
    header.extend_from_slice(&u32::MAX.to_le_bytes());
    header.extend_from_slice(&u32::MAX.to_le_bytes());
    header.extend_from_slice(&u32::MAX.to_le_bytes());
    header.extend_from_slice(&u32::MAX.to_le_bytes());
    let mut limits = Limits::default();
    limits.max_vertices = 8;
    limits.max_faces = 8;
    limits.max_corners = 32;
    limits.max_edges = 32;
    limits.max_bytes = 4096;
    match Mesh::from_bytes(&header, &mut Context::new(limits, None)) {
        Err(MeshError::Budget { .. } | MeshError::CorruptData(_)) => {}
        other => panic!("hostile counts must not install a mesh: {other:?}"),
    }
}

#[test]
fn preflight_limits_reject_growth_without_mutating() {
    let mut mesh = Mesh::plane([1., 1.], &mut ctx()).unwrap();
    let snapshot = bytes_of(&mesh);
    let mut limits = Limits::default();
    limits.max_vertices = mesh.vertices().len();
    limits.max_faces = mesh.faces().len();
    limits.max_corners = mesh.corners().len();
    let face = mesh.faces()[0].id;
    assert!(matches!(
        mesh.extrude_face(face, [0., 1., 0.], &mut Context::new(limits, None)),
        Err(MeshError::Budget { .. })
    ));
    assert_eq!(bytes_of(&mesh), snapshot);

    let mut tiny = Limits::default();
    tiny.max_work = 1;
    assert!(Mesh::cube([1., 1., 1.], &mut Context::new(tiny, None)).is_err());
}

#[test]
fn weights_normalize_and_cardinality_mismatch_is_refused() {
    let mut c = ctx();
    let mut mesh = Mesh::plane([1., 1.], &mut c).unwrap();
    let v = mesh.vertices()[0].id;
    mesh.set_vertex_weights(
        v,
        &[
            JointWeight {
                joint: 2,
                weight: 1.,
            },
            JointWeight {
                joint: 5,
                weight: 3.,
            },
        ],
        &mut c,
    )
    .unwrap();
    let weights = &mesh.vertex(v).unwrap().weights;
    assert_eq!(weights.len(), 2);
    assert!((weights.iter().map(|w| w.weight).sum::<f64>() - 1.).abs() < 1e-12);
    assert!(Mesh::from_weighted_polygons(
        &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        &[vec![]],
        &[Polygon::new(vec![0, 1, 2])],
        &mut ctx()
    )
    .is_err());
}

#[test]
fn nan_and_nonfinite_inputs_never_install() {
    assert!(Mesh::cube([f64::NAN, 1., 1.], &mut ctx()).is_err());
    assert!(Mesh::cube([f64::INFINITY, 1., 1.], &mut ctx()).is_err());
    assert!(Mesh::plane([0., 1.], &mut ctx()).is_err());
    assert!(Mesh::from_polygons(
        &[[f64::NAN, 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        &[Polygon::new(vec![0, 1, 2])],
        &mut ctx()
    )
    .is_err());
}

#[test]
fn deterministic_edit_sequence_preserves_surviving_ids_and_bytes() {
    let mut c = ctx();
    let mut mesh = Mesh::cube([2., 2., 2.], &mut c).unwrap();
    let face = mesh.faces()[1].id;
    let corner = mesh.face_corners(face).unwrap()[0].id;
    let vertex = mesh.face_corners(face).unwrap()[0].vertex;
    mesh.set_corner_uv(corner, [0.25, 0.75], &mut c).unwrap();
    mesh.set_vertex_weights(
        vertex,
        &[JointWeight {
            joint: 1,
            weight: 1.,
        }],
        &mut c,
    )
    .unwrap();
    let edge = EdgeKey::new(
        mesh.face_corners(face).unwrap()[0].vertex,
        mesh.face_corners(face).unwrap()[1].vertex,
    );
    mesh.set_edge_attributes(
        edge,
        EdgeAttributes {
            seam: true,
            crease: 0.4,
        },
        &mut c,
    )
    .unwrap();
    let extruded = mesh.extrude_face(face, [0., 0., 0.5], &mut c).unwrap();
    assert_eq!(extruded.cap, face);
    assert_eq!(mesh.face_corners(face).unwrap()[0].id, corner);
    mesh.mirror(0, 0., &mut c).unwrap();
    let bytes = bytes_of(&mesh);
    let restored = Mesh::from_bytes(&bytes, &mut ctx()).unwrap();
    assert_eq!(bytes_of(&restored), bytes);
    assert!(restored.face(face).is_some());
    assert_eq!(restored.corner(corner).unwrap().uv, [0.25, 0.75]);
    assert_eq!(restored.to_bytes(&mut ctx()).unwrap(), bytes);
}
