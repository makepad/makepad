use makepad_model::*;
fn apply(d:&mut Document,id:&str,ops:Vec<Operation>)->Applied{d.apply(Transaction{request_id:id.into(),expected:d.head(),operations:ops},None).unwrap()}
fn parse(d:&Document,s:&str)->Vec<Operation>{parse_operations(&json::parse(s.as_bytes()).unwrap(),d.limits()).unwrap()}
#[test]
fn published_snapshot_is_current_state_without_replay_and_retains_retry_identity(){
    let mut d=Document::new(Limits::default()).unwrap();
    let ops=parse(&d,r#"[{"op":"lathe","object":"vase","profile":[[0,0],[0.7,0.1],[0.8,0.8],[0.3,1.7]],"axis":1,"segments":17,"caps":true,"material":0}]"#);
    let tx=Transaction{request_id:"lathe".into(),expected:d.head(),operations:ops};let applied=d.apply(tx.clone(),None).unwrap();
    let history=d.to_bytes(None).unwrap();let snapshot=d.to_snapshot_bytes(None).unwrap();
    let mut restored=Document::from_bytes(&snapshot,Limits::default(),None).unwrap();
    assert_eq!(restored.head(),d.head());assert_eq!(restored.history_position(),(0,0));assert_eq!(d.history_position(),(1,1));
    assert_eq!(restored.object("vase"),d.object("vase"));assert_eq!(restored.compile(None).unwrap().glb,d.compile(None).unwrap().glb);
    assert_eq!(restored.apply(tx,None).unwrap().committed,applied.committed);assert_eq!(d.to_bytes(None).unwrap(),history);
    assert!(d.to_snapshot_bytes(Some(&||true)).is_err());
}
#[test]
fn receipt_byte_cap_preserves_editability_and_tombstones(){
    let mut d=Document::new(Limits::default()).unwrap();apply(&mut d,"cube",vec![Operation::Cube{object:"body".into(),size:[1.;3]}]);
    let vertices=d.object("body").unwrap().vertices().iter().map(|v|v.id).collect::<Vec<_>>();
    // Repeated full-scene selection outputs would previously grow without a byte bound.
    // They are legal independently even when the complete receipt is too large to cache.
    let ops=(0..256).map(|_|Operation::Transform{object:"body".into(),vertices:vertices.clone(),matrix:transform::IDENTITY_MATRIX}).collect::<Vec<_>>();
    let first=Transaction{request_id:"many-results-0".into(),expected:d.head(),operations:ops.clone()};d.apply(first.clone(),None).unwrap();
    for i in 1..32{apply(&mut d,&format!("many-results-{i}"),ops.clone());d.checkpoint(d.head()).unwrap();}
    assert_eq!(d.apply(first,None).unwrap_err(),Error::RequestIdReused);
    apply(&mut d,"still-editable",vec![Operation::Scene(SceneOperation::Snap{object:"body".into(),grid:1.})]);
    let bytes=d.to_snapshot_bytes(None).unwrap();assert!(bytes.len()<3*1024*1024);
    assert_eq!(Document::from_bytes(&bytes,Limits::default(),None).unwrap().head(),d.head());
}
#[test]
fn pivot_keeps_vertices_children_and_headlight_world_positions(){
    let mut d=Document::new(Limits::default()).unwrap();
    let ops=parse(&d,r#"[{"op":"cube","object":"car","size":[2,1,4]},{"op":"cube","object":"lamp","size":[0.2,0.2,0.1]},{"op":"object_node","object":"car","node":{"transform":{"translation":[3,2,1],"rotation":[0,0,0,1],"scale":[2,1,3]}}},{"op":"object_node","object":"lamp","node":{"parent":"car","transform":{"translation":[0.7,0,-2]}}},{"op":"light","name":"headlight","attachment":{"object":"car"},"transform":{"translation":[0.7,0,-2]},"kind":"spot","inner":0.2,"outer":0.5,"color":[1,1,1],"intensity":500,"range":40}]"#);apply(&mut d,"car",ops);
    let positions=|d:&Document|d.object("car").unwrap().vertices().iter().map(|v|transform::transform_point(d.scene().world_matrix("car").unwrap(),v.position)).collect::<Vec<_>>();
    let before=positions(&d);let child=d.scene().world_matrix("lamp").unwrap();let emitter=transform::transform_point(d.scene().world_matrix("car").unwrap(),d.scene().emitters["headlight"].transform.translation);
    let ops=parse(&d,r#"[{"op":"pivot","object":"car","position":[0.25,0.5,-1]}]"#);apply(&mut d,"pivot",ops);
    assert_eq!(positions(&d),before);assert_eq!(d.scene().world_matrix("lamp").unwrap(),child);
    assert_eq!(transform::transform_point(d.scene().world_matrix("car").unwrap(),d.scene().emitters["headlight"].transform.translation),emitter);
    let bytes=d.to_bytes(None).unwrap();assert_eq!(Document::from_bytes(&bytes,Limits::default(),None).unwrap().head(),d.head());
}
#[test]
fn rigid_binding_uses_effective_rest_and_owner_world_transform(){
    let mut d=Document::new(Limits::default()).unwrap();
    apply(&mut d,"rig",vec![Operation::Cube{object:"body".into(),size:[0.1;3]},Operation::SetSkeleton{skeleton:Skeleton{joints:vec![
        Joint{name:"root".into(),parent:None,translation:[0.;3]},
        Joint{name:"x".into(),parent:Some(0),translation:[10.,0.,0.]},
        Joint{name:"y".into(),parent:Some(0),translation:[0.,10.,0.]},
    ]}},Operation::Rig(RigOperation::Rest{joint:1,transform:Transform{translation:[0.,10.,0.],..Default::default()}}),Operation::Rig(RigOperation::Rest{joint:2,transform:Transform{translation:[10.,0.,0.],..Default::default()}}),Operation::Scene(SceneOperation::Node{object:"body".into(),node:SceneNode{transform:Transform{translation:[0.,8.,0.],..Default::default()},..Default::default()}}),Operation::AutoWeights{object:"body".into()}]);
    for vertex in d.object("body").unwrap().vertices(){assert_eq!(vertex.weights.len(),1);assert_eq!(vertex.weights[0].joint,1);}
}
