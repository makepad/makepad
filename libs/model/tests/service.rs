use makepad_model::*;
fn apply(doc: &mut Document, id: &str, operations: Vec<Operation>) {
    doc.apply(Transaction {request_id: id.into(), expected: doc.head(), operations}, None).unwrap();
}
#[test]
fn overview_reports_complete_transformed_dimensions_without_editing_source() {
    let mut doc=Document::new(Limits::default()).unwrap();
    apply(&mut doc,"shape",vec![
        Operation::Cube {object:"body".into(),size:[2.,1.,4.]},
        Operation::Scene(SceneOperation::Node {object:"body".into(),node:SceneNode {
            transform:Transform {translation:[3.,2.,-1.],..Default::default()},..Default::default()
        }}),
    ]);
    let head=doc.head();
    let source=doc.to_bytes(None).unwrap();
    let overview=doc.compile(None).unwrap();
    assert_eq!(overview.bounds,[[2.,1.5,-3.],[4.,2.5,1.]]);
    let dimensions: [f32; 3] = std::array::from_fn(|i| overview.bounds[1][i]-overview.bounds[0][i]);
    assert_eq!(dimensions,[2.,1.,4.]);
    assert_eq!(doc.head(),head);
    assert_eq!(doc.to_bytes(None).unwrap(),source);
}
#[test]
fn primitives_are_closed_outward_and_have_seam_uvs() {
    let mut doc=Document::new(Limits::default()).unwrap();
    apply(&mut doc,"create",vec![
        Operation::sphere("ball", 1.0, 12, 6, true, &Limits::default()).unwrap(),
        Operation::cylinder("barrel", 0.5, 2.0, 12, true, &Limits::default()).unwrap(),
    ]);
    for name in ["ball","barrel"] {
        let mesh=doc.object(name).unwrap();
        assert!(mesh.validate(&mut mesh::Context::default()).unwrap().is_closed_manifold);
        let tri=mesh.triangulate(&mut mesh::Context::default()).unwrap();
        let mut volume=0.0;
        for t in &tri.triangles {
            let [a,b,c]=t.indices.map(|i|tri.vertices[i as usize].position);
            volume+=(a[0]*(b[1]*c[2]-b[2]*c[1])+a[1]*(b[2]*c[0]-b[0]*c[2])+a[2]*(b[0]*c[1]-b[1]*c[0]))/6.0;
        }
        assert!(volume>0.0,"{name} volume {volume}");
        assert!(mesh.corners().iter().any(|c|c.uv[0]==1.0));
    }
}

#[test]
fn paged_corner_lookup_and_exact_id_protocol() {
    let mut doc=Document::new(Limits::default()).unwrap();
    apply(&mut doc,"cube",vec![Operation::Cube{object:"body".into(),size:[1.;3]}]);
    let source=doc.to_bytes(None).unwrap();
    assert!(doc.apply(Transaction{request_id:"cancel".into(),expected:doc.head(),
        operations:vec![Operation::Cube{object:"cancelled".into(),size:[1.;3]}]},Some(&||true)).is_err());
    assert_eq!(doc.to_bytes(None).unwrap(),source);
}
