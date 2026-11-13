use makepad_model::*;
fn cube() -> Operation { Operation::Cube { object:"body".into(),size:[1.0;3] } }
fn apply(doc:&mut Document,id:&str,operations:Vec<Operation>)->Applied {
    doc.apply(Transaction { request_id:id.into(),expected:doc.head(),operations },None).unwrap()
}
#[test]
fn transaction_rollback_undo_branch_and_retry() {
    let mut doc=Document::new(Limits::default()).unwrap(); let empty=doc.head();
    let tx=Transaction { request_id:"cube".into(),expected:empty,operations:vec![cube()] };
    let first=doc.apply(tx.clone(),None).unwrap(); let source=doc.to_bytes(None).unwrap();
    assert_eq!(doc.apply(tx.clone(),None).unwrap().committed,first.committed);
    assert!(doc.apply(tx.clone(),None).unwrap().replayed);
    assert_eq!(doc.to_bytes(None).unwrap(),source);
    let failure=Transaction { request_id:"failure".into(),expected:doc.head(),operations:vec![
        Operation::Plane { object:"new".into(),size:[1.0;2] },Operation::DeleteObject { object:"absent".into() }] };
    assert!(doc.apply(failure,None).is_err()); assert_eq!(doc.to_bytes(None).unwrap(),source);
    let undo=doc.undo(doc.head(),None).unwrap(); assert_eq!(undo.content,empty.content); assert!(undo.generation>first.committed.generation);
    let redo=doc.redo(undo,None).unwrap(); assert_eq!(redo.content,first.committed.content);
    doc.undo(redo,None).unwrap(); apply(&mut doc,"branch",vec![Operation::Plane { object:"ground".into(),size:[2.0;2] }]);
    assert!(doc.object("body").is_none()); assert!(doc.redo(doc.head(),None).is_err());
    let old=doc.apply(tx,None).unwrap(); assert!(old.replayed); assert_eq!(old.committed,first.committed); assert_eq!(old.current,doc.head());
    let encoded=doc.to_bytes(None).unwrap(); let reopened=Document::from_bytes(&encoded,Limits::default(),None).unwrap();
    assert_eq!(reopened.to_bytes(None).unwrap(),encoded); assert_eq!(reopened.head(),doc.head());
}
#[test]
fn cancellation_and_conflicting_request_ids_are_atomic() {
    let mut doc=Document::new(Limits::default()).unwrap(); let before=doc.to_bytes(None).unwrap();
    let tx=Transaction { request_id:"same".into(),expected:doc.head(),operations:vec![cube()] };
    assert!(matches!(doc.apply(tx.clone(),Some(&||true)),Err(Error::Mesh(mesh::MeshError::Cancelled))));
    assert_eq!(doc.to_bytes(None).unwrap(),before);
    doc.apply(tx.clone(),None).unwrap();
    let mut changed=tx; changed.operations=vec![Operation::Plane { object:"new".into(),size:[1.0;2] }];
    assert_eq!(doc.apply(changed,None).unwrap_err(),Error::RequestIdReused);
}
#[test]
fn source_roundtrip_preserves_uv_material_and_export() {
    let mut doc=Document::new(Limits::default()).unwrap(); apply(&mut doc,"create",vec![cube()]);
    let corner=doc.object("body").unwrap().corners()[0].id;
    let face=doc.object("body").unwrap().faces()[0].id;
    let png=Texture::solid(2,2,[42,78,123],doc.limits()).unwrap().to_png(doc.limits()).unwrap();
    apply(&mut doc,"paint",vec![Operation::SetMaterial { material:7,value:Material {color:[0.5,1.0,0.25],base_color_png:png.clone()} },
        Operation::AssignMaterial { object:"body".into(),faces:vec![face],material:7 },
        Operation::SetUv { object:"body".into(),corner,uv:[0.25,0.75] }]);
    let compiled=doc.compile(None).unwrap(); assert_eq!(compiled.triangles,12); assert_eq!(compiled.primitives.len(),2);
    let parsed=makepad_gltf::parse_glb_bytes(&compiled.glb).unwrap();
    assert_eq!(parsed.document.meshes.as_ref().unwrap().len(),1);
    let bytes=doc.to_bytes(None).unwrap(); let opened=Document::from_bytes(&bytes,Limits::default(),None).unwrap();
    assert_eq!(opened.materials()[&7].base_color_png,png); assert_eq!(opened.compile(None).unwrap().glb,compiled.glb);
    for n in [0,1,7,12,bytes.len()-1] { assert!(Document::from_bytes(&bytes[..n],Limits::default(),None).is_err()); }
    let mut extra=bytes; extra.push(0); assert!(Document::from_bytes(&extra,Limits::default(),None).is_err());
    // A mirror (x -> -x) negates zero normal components and a literal may be
    // -0.0; the source writes +0.0. The edited document holds the decoded
    // values, so it compiles the bytes its reopened source compiles (after a
    // checkpoint, the source holds the state itself, not edits to replay).
    let mut doc=opened;
    apply(&mut doc,"tyre",vec![Operation::Construction(ConstructionOperation { object:"tyre".into(),
        kind:Construction::Shape { shape:Shape::Cone { bottom:0.3,top:0.3,height:0.2,segments:12,smooth:true },material:0 } })]);
    let vertices=doc.object("tyre").unwrap().vertices().iter().map(|v|v.id).collect();
    let mirror=[[-1.,0.,0.,0.],[0.,1.,0.,0.],[0.,0.,1.,0.],[0.,0.,0.,1.]];
    apply(&mut doc,"mirror",vec![Operation::Transform { object:"tyre".into(),vertices,matrix:mirror },
        Operation::Scene(SceneOperation::Node { object:"tyre".into(),node:SceneNode { transform:Transform { translation:[-0.0,0.5,0.0],..Default::default() },..Default::default() } })]);
    doc.checkpoint(doc.head()).unwrap();
    let reopened=Document::from_bytes(&doc.to_bytes(None).unwrap(),Limits::default(),None).unwrap();
    assert!(doc.compile(None).unwrap().glb==reopened.compile(None).unwrap().glb,"a mirrored edit compiles the bytes of its reopened source");
}
