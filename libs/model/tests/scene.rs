use makepad_model::*;
use makepad_gltf::JsonValue;
fn apply(d:&mut Document,id:&str,ops:Vec<Operation>){d.apply(Transaction{request_id:id.into(),expected:d.head(),operations:ops},None).unwrap();}
fn field<'a>(v:&'a JsonValue,key:&str)->&'a JsonValue{let JsonValue::Object(v)=v else{panic!("object")};&v[key]}
fn array(v:&JsonValue)->&[JsonValue]{let JsonValue::Array(v)=v else{panic!("array")};v}

#[test]
fn old_source_opens_with_exact_identity_then_writes_version_two(){
    let original=include_bytes!("fixtures/v1_robot.mpmodel");
    let doc=Document::from_bytes(original,Limits::default(),None).unwrap();
    assert_eq!(doc.head().generation,u64::from_le_bytes(original[12..20].try_into().unwrap()));
    assert_eq!(doc.head().content.as_slice(),&original[20..52]);
    let upgraded=doc.to_bytes(None).unwrap();assert_eq!(u32::from_le_bytes(upgraded[8..12].try_into().unwrap()),2);
    let round=Document::from_bytes(&upgraded,Limits::default(),None).unwrap();assert_eq!(round.head(),doc.head());
    assert_eq!(round.to_bytes(None).unwrap(),upgraded);assert_eq!(round.compile(None).unwrap().glb,doc.compile(None).unwrap().glb);
}
#[test]
fn car_headlights_and_socket_survive_source_and_glb_scene_graph(){
    let mut doc=Document::new(Limits::default()).unwrap();
    apply(&mut doc,"car",vec![Operation::Cube{object:"car".into(),size:[2.,1.,4.]},Operation::Scene(SceneOperation::Node{
        object:"car".into(),node:SceneNode{transform:Transform{translation:[5.,0.,3.],rotation:transform::quat_axis_angle([0.,1.,0.],0.5).unwrap(),..Default::default()},..Default::default()}})]);
    let ops=[("left",-0.7),("right",0.7)].into_iter().map(|(name,x)|Operation::Scene(SceneOperation::Light(LightEmitter{
        name:name.into(),attachment:Attachment::Object("car".into()),transform:Transform{translation:[x,0.,-2.],..Default::default()},
        kind:LightKind::Spot{inner:0.15,outer:0.4},color:[1.,0.94,0.8],intensity:1000.,range:60.,}))).chain(std::iter::once(Operation::Scene(SceneOperation::Socket(Socket{
            name:"roof".into(),attachment:Attachment::Object("car".into()),transform:Transform{translation:[0.,0.6,0.],..Default::default()}})))).collect();
    apply(&mut doc,"attachments",ops);
    let bytes=doc.to_bytes(None).unwrap();let round=Document::from_bytes(&bytes,Limits::default(),None).unwrap();assert_eq!(round.scene(),doc.scene());
    let product=round.compile(None).unwrap();let glb=makepad_gltf::parse_glb_bytes(&product.glb).unwrap();
    let lights=array(field(field(glb.document.extensions.as_ref().unwrap(),"KHR_lights_punctual"),"lights"));
    assert_eq!(lights.len(),2);assert!(matches!(field(&lights[0],"intensity"),JsonValue::F64(v) if *v==1000.)|matches!(field(&lights[0],"intensity"),JsonValue::U64(1000)));
    let nodes=glb.document.nodes_slice();let car=nodes.iter().position(|n|n.name.as_deref()==Some("car")).unwrap();
    assert_eq!(nodes[car].translation,Some([5.,0.,3.]));assert!(nodes[car].mesh.is_some());
    for name in ["left","right","roof"]{let i=nodes.iter().position(|n|n.name.as_deref()==Some(name)).unwrap();assert!(nodes[car].children.as_ref().unwrap().contains(&i));}
    assert!(product.bounds[0][0]>3. && product.bounds[1][0]<7.);
}
#[test]
fn linked_geometry_tracks_source_and_cycles_or_bad_lights_roll_back(){
    let mut doc=Document::new(Limits::default()).unwrap();apply(&mut doc,"base",vec![Operation::Cube{object:"source".into(),size:[1.;3]},
        Operation::Scene(SceneOperation::Instance{object:"copy".into(),source:"source".into(),transform:Transform{translation:[3.,0.,0.],..Default::default()}})]);
    let p=doc.compile(None).unwrap();assert_eq!(p.triangles,24);assert_eq!(doc.object("copy").unwrap().faces().len(),0);
    let source=doc.object("source").unwrap();let vertices=source.vertices().iter().map(|v|v.id).collect();
    apply(&mut doc,"grow",vec![Operation::Transform{object:"source".into(),vertices,matrix:[[2.,0.,0.,0.],[0.,1.,0.,0.],[0.,0.,1.,0.],[0.,0.,0.,1.]]}]);
    assert_eq!(doc.compile(None).unwrap().bounds[1][0],4.);
    let before=doc.to_bytes(None).unwrap();let bad=Operation::Scene(SceneOperation::Node{object:"source".into(),node:SceneNode{linked_to:Some("copy".into()),..Default::default()}});
    assert!(doc.apply(Transaction{request_id:"cycle".into(),expected:doc.head(),operations:vec![bad]},None).is_err());assert_eq!(doc.to_bytes(None).unwrap(),before);
    let bad=Operation::Scene(SceneOperation::Light(LightEmitter{name:"bad".into(),attachment:Attachment::Root,transform:Transform::default(),kind:LightKind::Spot{inner:0.5,outer:0.2},color:[1.;3],intensity:1.,range:10.}));
    assert!(doc.apply(Transaction{request_id:"cone".into(),expected:doc.head(),operations:vec![bad]},None).is_err());assert_eq!(doc.to_bytes(None).unwrap(),before);
}
