use makepad_model::*;
fn apply(doc:&mut Document,id:&str,ops:Vec<Operation>){doc.apply(Transaction{request_id:id.into(),expected:doc.head(),operations:ops},None).unwrap();}
fn node(object:&str,translation:[f64;3],scale:[f64;3])->Operation{Operation::Scene(SceneOperation::Node{object:object.into(),node:SceneNode{transform:Transform{translation,scale,..Default::default()},..Default::default()}})}
fn bake(doc:&mut Document,object:&str,matrix:transform::Matrix4){let vertices=doc.object(object).unwrap().vertices().iter().map(|v|v.id).collect();apply(doc,&format!("bake-{object}"),vec![Operation::Transform{object:object.into(),vertices,matrix}]);}
#[test]
fn boolean_and_remesh_node_frames_match_baked_sources(){
    for command in [vec![
        Operation::Construction(ConstructionOperation {
            object: "result".into(),
            kind: Construction::Boolean { a: "a".into(), b: "b".into(), mode: BooleanMode::Union }
        }),
    ],vec![
        Operation::Construction(ConstructionOperation { object: "result".into(), kind: Construction::Voxel { source: "a".into(), resolution: 8 } }),
    ]]{
        let mut placed=Document::new(Limits::default()).unwrap();let mut baked=Document::new(Limits::default()).unwrap();
        for doc in [&mut placed,&mut baked]{apply(doc,"meshes",vec![Operation::Cube{object:"a".into(),size:[1.;3]},Operation::Cube{object:"b".into(),size:[1.;3]}]);}
        apply(&mut placed,"place",vec![node("a",[4.,2.,-3.],[1.,2.,1.]),node("b",[7.,2.,-3.],[1.;3])]);
        for object in ["a","b"]{bake(&mut baked,object,placed.scene().world_matrix(object).unwrap());}
        let source_a=placed.object("a").unwrap().clone();
        apply(&mut placed,"derive",command.clone());apply(&mut baked,"derive",command);
        assert_eq!(placed.object("result"),baked.object("result"));assert_eq!(placed.object("a"),Some(&source_a));
        assert_eq!(placed.scene().world_matrix("result").unwrap(),transform::IDENTITY_MATRIX);
        assert!(placed.object("result").unwrap().vertices().iter().all(|v|v.position[0]>3.));
    }
}
#[test]
fn direct_shrinkwrap_maps_reference_into_transformed_owner_frame(){
    let mut doc=Document::new(Limits::default()).unwrap();
    apply(&mut doc,"meshes",vec![Operation::Plane{object:"owner".into(),size:[1.;2]},Operation::Plane{object:"reference".into(),size:[4.;2]},node("owner",[3.,4.,0.],[2.,1.,2.]),node("reference",[3.,2.,0.],[1.;3])]);
    let vertices=doc.object("owner").unwrap().vertices().iter().map(|v|v.id).collect();
    apply(&mut doc,"wrap",vec![Operation::MeshEditing(MeshEditingOperation {
        object:"owner".into(), action:Action::Shrinkwrap { vertices,reference:"reference".into(),max_distance:3.,offset:0. },
    })]);
    assert!(doc.object("owner").unwrap().vertices().iter().all(|v|(v.position[1]+2.).abs()<1e-12));
    assert_eq!(doc.scene().world_matrix("owner").unwrap()[1][3],4.);
    let bytes=doc.to_bytes(None).unwrap();assert_eq!(Document::from_bytes(&bytes,Limits::default(),None).unwrap().head(),doc.head());
}
