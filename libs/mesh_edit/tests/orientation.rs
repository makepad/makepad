use makepad_mesh_edit::*;

#[test]
fn flipping_shell_changes_inside_outside_and_preserves_every_attribute() {
    let mut mesh = Mesh::cube([2.; 3], &mut Context::default()).unwrap();
    let faces = mesh.faces().iter().map(|f|f.id).collect::<Vec<_>>();
    let corner = mesh.corners()[0].id;
    let vertex = mesh.vertices()[0].id;
    let edge = *mesh.adjacency(&mut Context::default()).unwrap().edges.keys().next().unwrap();
    mesh.set_vertex_weights(vertex, &[JointWeight {joint: 2, weight: 1.}], &mut Context::default()).unwrap();
    mesh.set_face_material(faces[0], 7, &mut Context::default()).unwrap();
    mesh.set_corner_uv(corner, [0.2, 0.7], &mut Context::default()).unwrap();
    mesh.pin_uv(&[corner], true, &mut Context::default()).unwrap();
    mesh.set_edge_attributes(edge, EdgeAttributes {seam: true, crease: 0.8}, &mut Context::default()).unwrap();
    mesh.recalculate_normals(false, 0., &mut Context::default()).unwrap();
    let original = mesh.clone();
    assert!(mesh.validate_global(&mut Context::default()).unwrap().is_valid_solid);
    let changes = mesh.flip_faces(&faces, &mut Context::default()).unwrap();
    assert!(changes.created.is_empty() && changes.deleted.is_empty());
    assert_eq!(mesh.vertices(), original.vertices());
    assert_eq!(mesh.faces(), original.faces());
    assert_eq!(mesh.edge_attributes(), original.edge_attributes());
    assert_eq!(mesh.uv_pins(), original.uv_pins());
    for old in original.corners() {
        let new = mesh.corner(old.id).unwrap();
        assert_eq!((new.vertex,new.uv), (old.vertex,old.uv));
        assert_eq!(new.normal, old.normal.map(|v|v.map(|x|-x)));
    }
    let report = mesh.validate_global(&mut Context::default()).unwrap();
    assert!(report.is_embedded_surface);
    assert_eq!(report.orientation_errors, 1);
    assert!(!report.is_valid_solid);
    let bytes = mesh.to_bytes(&mut Context::default()).unwrap();
    mesh = Mesh::from_bytes(&bytes, &mut Context::default()).unwrap();
    mesh.flip_faces(&faces, &mut Context::default()).unwrap();
    assert_eq!(mesh, original);
}

#[test]
fn invalid_selections_budget_and_cancellation_leave_winding_unchanged() {
    let mut mesh = Mesh::cube([1.; 3], &mut Context::default()).unwrap();
    let original = mesh.clone();
    let face = mesh.faces()[0].id;
    for ids in [vec![face,face],vec![FaceId(u64::MAX)]] {
        assert!(mesh.flip_faces(&ids, &mut Context::default()).is_err());
        assert_eq!(mesh, original);
    }
    let mut budget = Limits::default();
    budget.max_work = 2;
    assert!(mesh.flip_faces(&[face], &mut Context::new(budget,None)).is_err());
    assert_eq!(mesh, original);
    assert_eq!(mesh.flip_faces(&[face], &mut Context::new(Limits::default(),Some(&||true))).unwrap_err(), MeshError::Cancelled);
    assert_eq!(mesh, original);
}

#[test]
fn flipping_an_open_pane_changes_its_visible_side_without_inventing_a_solid() {
    let mut mesh = Mesh::plane([2.,3.], &mut Context::default()).unwrap();
    let faces = mesh.faces().iter().map(|f|f.id).collect::<Vec<_>>();
    let before = mesh.triangulate(&mut Context::default()).unwrap();
    mesh.flip_faces(&faces, &mut Context::default()).unwrap();
    let after = mesh.triangulate(&mut Context::default()).unwrap();
    assert_eq!(before.vertices[0].normal.map(|v|-v),after.vertices[0].normal);
    assert!(!mesh.validate(&mut Context::default()).unwrap().is_closed);
}
