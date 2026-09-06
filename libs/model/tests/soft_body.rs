use makepad_model::*;
use makepad_render::skin::SkinnedModel;

fn apply(d: &mut Document, id: &str, operations: Vec<Operation>) -> Result<Applied> {
    d.apply(Transaction { request_id: id.into(), expected: d.head(), operations }, None)
}
fn commands(d: &mut Document, id: &str, text: &str) -> Result<Applied> {
    let operations = parse_operations(&json::parse(text.as_bytes()).unwrap(), d.limits())?;
    apply(d, id, operations)
}
fn fixture() -> Document {
    let mut d = Document::new(Limits { max_joints: 128, ..Default::default() }).unwrap();
    commands(&mut d, "geometry", r#"[
      {"op":"sphere","object":"body","radius":0.45,"segments":16,"rings":8},
      {"op":"sphere","object":"eye","radius":0.14,"segments":8,"rings":4},
      {"op":"sphere","object":"pupil","radius":0.05,"segments":8,"rings":4},
      {"op":"sphere","object":"fuzz","radius":0.47,"segments":8,"rings":4}
    ]"#).unwrap();
    for (object, position) in [("body",[0.,0.8,0.]),("fuzz",[0.,0.8,0.]),("eye",[0.18,0.95,-0.4]),("pupil",[0.18,0.95,-0.53])] {
        apply(&mut d, &format!("place-{object}"), vec![Operation::Scene(SceneOperation::Node { object: object.into(), node: SceneNode { transform: Transform { translation: position, ..Default::default() }, ..Default::default() } })]).unwrap();
    }
    d
}
fn bind(d: &mut Document) {
    commands(d,"soft",r#"[{"op":"soft_body_bind","object":"body","deform_objects":["fuzz"],"attachments":[{"name":"eyes","objects":["eye","pupil"],"pivot":[0.18,0.95,-0.4]}]}]"#).unwrap();
}

#[test]
fn cage_binding_preserves_mesh_contacts_source_and_portable_skin_metadata() {
    let mut d=fixture();
    let original=d.object("body").unwrap().vertices().iter().map(|v|(v.id,v.position)).collect::<Vec<_>>();
    bind(&mut d);
    let m=d.soft_body().unwrap();
    assert_eq!((m.rest_positions.len(),m.tetrahedra.len(),d.skeleton().unwrap().joints.len()),(43,80,82));
    assert_eq!(d.object("body").unwrap().vertices().iter().map(|v|(v.id,v.position)).collect::<Vec<_>>(),original);
    for sample in &m.surface_samples {
        let p=m.rest_point(*sample).unwrap();
        assert!(original.iter().any(|(_,q)| (0..3).map(|i|(p[i] as f64-q[i]-if i==1{0.8}else{0.}).powi(2)).sum::<f64>() < 1e-10),"contacts must lie on visible body, not cage or fuzz");
    }
    let source=d.to_bytes(None).unwrap();
    let round=Document::from_bytes(&source,d.limits().clone(),None).unwrap();
    assert_eq!(round.soft_body(),d.soft_body());
    assert_eq!(round.head(),d.head());
    assert_eq!(round.to_bytes(None).unwrap(),source);
    let product=round.compile(None).unwrap();
    let loaded=SkinnedModel::parse_glb_validated(&product.glb).unwrap();
    assert_eq!(loaded.soft_body(),round.soft_body());
    assert!(std::sync::Arc::ptr_eq(&loaded.soft_body_shared().unwrap(),&loaded.soft_body_shared().unwrap()));
    assert_eq!(loaded.joint_count(),82);
}

#[test]
fn budget_bad_config_and_post_bind_geometry_changes_are_atomic() {
    let mut small=Document::new(Limits::default()).unwrap();
    let before=small.to_bytes(None).unwrap();
    assert!(commands(&mut small,"over-budget",r#"[{"op":"soft_body_bind","object":"body"}]"#).is_err());
    assert_eq!(small.to_bytes(None).unwrap(),before);
    let mut d=fixture(); let before=d.to_bytes(None).unwrap();
    for text in [r#"[{"op":"soft_body_bind","object":"body","config":{"substeps":0}}]"#,r#"[{"op":"soft_body_bind","object":"body","config":{"guess":2}}]"#,r#"[{"op":"soft_body_bind","object":"body","attachments":[{"name":"eye","objects":["body"]}]}]"#] {
        assert!(commands(&mut d,"bad",text).is_err()); assert_eq!(d.to_bytes(None).unwrap(),before);
    }
    bind(&mut d);
    let before=d.to_bytes(None).unwrap(); let vertices=d.object("body").unwrap().vertices().iter().map(|v|v.id).collect();
    assert!(apply(&mut d,"move",vec![Operation::Transform{object:"body".into(),vertices,matrix:Transform{translation:[0.1,0.,0.],..Default::default()}.matrix().unwrap()}]).is_err());
    assert_eq!(d.to_bytes(None).unwrap(),before);
    let joint=d.soft_body().unwrap().attachments[0].joint as u32;
    assert!(apply(&mut d,"animate-physical-frame",vec![Operation::SetClip{clip:AnimationClip{name:"bad".into(),channels:vec![AnimationChannel{joint,path:AnimationPath::Rotation,keys:vec![Keyframe{time:0.,value:[0.,0.,0.,1.]},Keyframe{time:1.,value:transform::quat_axis_angle([1.,0.,0.],0.2).unwrap()}]}]}}]).is_err());
    assert_eq!(d.to_bytes(None).unwrap(),before);
    let metadata=d.soft_body().cloned();
    apply(&mut d,"texture",vec![Operation::TextureSolid{material:0,width:8,height:8,color:[240,80,140]}]).unwrap();
    assert_eq!(d.soft_body(),metadata.as_ref());
}

#[test]
fn rigid_frame_accepts_animated_child_and_refuses_malformed_metadata() {
    let mut d=fixture();bind(&mut d);
    let mut skeleton=d.skeleton().unwrap().clone();let frame=d.soft_body().unwrap().attachments[0].joint as u32;
    let gaze=skeleton.joints.len() as u32;
    skeleton.joints.push(Joint{name:"gaze".into(),parent:Some(frame),translation:[0.;3]});
    let mut operations=vec![Operation::SetSkeleton{skeleton}];
    operations.extend(parse_operations(&json::parse(format!(r#"[{{"op":"weight_edit","object":"pupil","vertices":[],"edit":{{"type":"assign","joint":{gaze}}}}}]"#).as_bytes()).unwrap(),d.limits()).unwrap());
    operations.push(Operation::SetClip{clip:AnimationClip{name:"idle".into(),channels:vec![AnimationChannel{joint:gaze,path:AnimationPath::Rotation,keys:vec![Keyframe{time:0.,value:[0.,0.,0.,1.]},Keyframe{time:1.,value:transform::quat_axis_angle([0.,1.,0.],0.1).unwrap()}]}]}});
    apply(&mut d,"gaze",operations).unwrap();
    let source=d.to_bytes(None).unwrap();let reopened=Document::from_bytes(&source,d.limits().clone(),None).unwrap();assert_eq!(reopened.head(),d.head());
    let glb=d.compile(None).unwrap().glb;let model=SkinnedModel::parse_glb_validated(&glb).unwrap();assert_eq!(model.clips[0].name,"idle");
    let good=d.soft_body().unwrap();
    for bad in [ {let mut m=good.clone();m.version=9;m}, {let mut m=good.clone();m.surface_samples[0].weights=[1.;4];m}, {let mut m=good.clone();m.tet_joints[0]=m.attachments[0].joint;m}, {let mut m=good.clone();m.attachments[0].rest_pivot[0]+=0.1;m}, {let mut m=good.clone();m.tetrahedra[0].swap(0,1);m} ] {
        assert!(makepad_gltf::SoftBodyMetadata::from_value(&bad.to_value(),83).is_err());
    }
}

#[test]
fn compact_rigid_assignment_respects_locked_weights_and_is_atomic() {
    let mut d=fixture();bind(&mut d);let joint=d.soft_body().unwrap().attachments[0].joint as u32;
    apply(&mut d,"lock",vec![Operation::Rig(RigOperation::WeightLocks{object:"pupil".into(),joints:std::collections::BTreeSet::from([joint])})]).unwrap();
    let before=d.to_bytes(None).unwrap();
    for target in [0,127] {assert!(commands(&mut d,"bad-assign",&format!(r#"[{{"op":"weight_edit","object":"pupil","vertices":[],"edit":{{"type":"assign","joint":{target}}}}}]"#)).is_err());assert_eq!(d.to_bytes(None).unwrap(),before);}
    commands(&mut d,"same-assign",&format!(r#"[{{"op":"weight_edit","object":"pupil","vertices":[],"edit":{{"type":"assign","joint":{joint}}}}}]"#)).unwrap();
}

#[test]
fn imported_glb_rejects_metadata_skin_drift_before_model_publication() {
    let mut d=fixture();bind(&mut d);let original=d.compile(None).unwrap().glb;
    let json_len=u32::from_le_bytes(original[12..16].try_into().unwrap()) as usize;
    let mut value=json::parse_depth(&original[20..20+json_len],32).unwrap();
    let json::Value::Obj(fields)=&mut value else{unreachable!()};
    let json::Value::Arr(nodes)=&mut fields.iter_mut().find(|(name,_)|name=="nodes").unwrap().1 else{unreachable!()};
    let node=nodes.iter_mut().find(|n|n.get("name").and_then(json::Value::as_str)==Some("__soft_body_tet_00")).unwrap();
    let json::Value::Obj(fields)=node else{unreachable!()};
    fields.retain(|(key,_)|key!="translation");fields.push(("translation".into(),json::Value::Arr(vec![json::Value::F64(0.02),json::Value::Int(0),json::Value::Int(0)])));
    let mut encoded=value.to_json().into_bytes();while encoded.len()%4!=0{encoded.push(b' ');}
    let mut changed=original[..12].to_vec();changed.extend_from_slice(&(encoded.len() as u32).to_le_bytes());changed.extend_from_slice(&original[16..20]);changed.extend(encoded);changed.extend_from_slice(&original[20+json_len..]);
    let len=changed.len() as u32;changed[8..12].copy_from_slice(&len.to_le_bytes());
    assert!(SkinnedModel::parse_glb_validated(&changed).err().unwrap().contains("affine joint"));
    // Even permissive legacy entry points enforce the contract once marked.
    assert!(SkinnedModel::parse_glb(&changed).is_err());
}

#[test]
fn engine_joint_budget_is_explicit_per_document_and_required_for_source_reopen() {
    let mut engine=Engine::new(Limits::default());
    for budget in [0,129,usize::MAX]{assert!(engine.open_with_joint_budget("bad",None,Some(budget),None).is_err());assert!(engine.document("bad").is_err());}
    let opened=engine.open_with_joint_budget("large",None,Some(128),None).unwrap();assert_eq!(opened.get("max_joints").and_then(json::Value::as_u64),Some(128));
    let ops=json::parse(br#"[{"op":"sphere","object":"body","radius":0.4,"segments":8,"rings":4},{"op":"soft_body_bind","object":"body"}]"#).unwrap();
    engine.execute("model.apply",&json::obj(vec![("document",json::s("large")),("request_id",json::s("bind")),("expected",head_json(engine.document("large").unwrap().head())),("operations",ops)]),None).unwrap();
    let before=engine.document("large").unwrap().to_snapshot_bytes(None).unwrap();
    assert!(engine.open_with_joint_budget("large",None,Some(64),None).is_err());
    assert_eq!(engine.document("large").unwrap().to_snapshot_bytes(None).unwrap(),before);
    engine.open("large",None,None).unwrap();assert_eq!(engine.document("large").unwrap().limits().max_joints,128);
    engine.open("ordinary",None,None).unwrap();assert_eq!(engine.document("ordinary").unwrap().limits().max_joints,64);
    let mut fresh=Engine::new(Limits::default());
    assert!(fresh.open("published",Some(&before),None).is_err());assert!(fresh.document("published").is_err());
    fresh.open_with_joint_budget("published",Some(&before),Some(128),None).unwrap();
    assert_eq!(fresh.document("published").unwrap().head(),engine.document("large").unwrap().head());
    assert_eq!(fresh.document("published").unwrap().soft_body(),engine.document("large").unwrap().soft_body());
}

#[test]
fn published_soft_source_can_unbind_reshape_and_rebind_without_losing_animated_rig() {
    use std::collections::{BTreeMap, BTreeSet};
    let mut d=fixture();
    apply(&mut d,"existing-rig",vec![Operation::SetSkeleton{skeleton:Skeleton{joints:vec![
        Joint{name:"root".into(),parent:None,translation:[0.1,0.2,0.]},
        Joint{name:"arm".into(),parent:Some(0),translation:[0.3,0.,0.]},
    ]}},Operation::Rig(RigOperation::Rest{joint:1,transform:Transform{translation:[0.3,0.,0.],rotation:transform::quat_axis_angle([0.,0.,1.],0.2).unwrap(),..Default::default()}})]).unwrap();
    bind(&mut d);
    let frame=d.soft_body().unwrap().attachments[0].joint as u32;
    let cells=d.soft_body().unwrap().tet_joints.clone();
    let mut skeleton=d.skeleton().unwrap().clone();let gaze=skeleton.joints.len() as u32;
    skeleton.joints.push(Joint{name:"gaze".into(),parent:Some(frame),translation:[0.,0.,-0.03]});
    let gaze_rest=Transform{translation:[0.,0.,-0.03],rotation:transform::quat_axis_angle([0.,1.,0.],0.05).unwrap(),..Default::default()};
    apply(&mut d,"face-controls",vec![
        Operation::SetSkeleton{skeleton},
        Operation::Rig(RigOperation::Rest{joint:gaze,transform:gaze_rest}),
        Operation::Rig(RigOperation::Weights{object:"pupil".into(),vertices:vec![],edit:WeightEdit::Assign{joint:gaze}}),
        Operation::Rig(RigOperation::WeightLocks{object:"pupil".into(),joints:BTreeSet::from([gaze])}),
        Operation::Rig(RigOperation::Pose(Pose{name:"look".into(),joints:BTreeMap::from([(gaze,gaze_rest)])})),
        Operation::Rig(RigOperation::Constraint(RigConstraint{name:"gaze-limit".into(),joint:gaze,enabled:true,
            kind:ConstraintKind::Limit{min_translation:[-1.;3],max_translation:[1.;3],min_scale:[0.5;3],max_scale:[2.;3],max_angle:1.}})),
        Operation::SetClip{clip:AnimationClip{name:"idle".into(),channels:vec![AnimationChannel{joint:gaze,path:AnimationPath::Rotation,
            keys:vec![Keyframe{time:0.,value:gaze_rest.rotation},Keyframe{time:1.,value:transform::quat_axis_angle([0.,1.,0.],0.15).unwrap()}]}]}},
        Operation::Scene(SceneOperation::Socket(Socket{name:"gaze-socket".into(),attachment:Attachment::Joint(gaze),transform:Transform::default()})),
    ]).unwrap();
    let rig=d.rig().clone();let clips=d.clips().clone();let sockets=d.scene().sockets.clone();let count=d.skeleton().unwrap().joints.len();
    let weights=d.object("pupil").unwrap().vertices().iter().map(|v|(v.id,v.weights.clone())).collect::<Vec<_>>();
    let source=d.to_snapshot_bytes(None).unwrap();
    d=Document::from_bytes(&source,d.limits().clone(),None).unwrap();
    assert_eq!(d.history_position(),(0,0));
    for cycle in 0..3 {
        commands(&mut d,&format!("unbind-{cycle}"),r#"[{"op":"soft_body_unbind"}]"#).unwrap();
        assert!(d.soft_body().is_none());assert_eq!(d.skeleton().unwrap().joints.len(),count);
        assert_eq!(d.rig(),&rig);assert_eq!(d.clips(),&clips);
        // Reopening the detached snapshot must not depend on an undo record.
        d=Document::from_bytes(&d.to_snapshot_bytes(None).unwrap(),d.limits().clone(),None).unwrap();
        if cycle==0 {
            let vertices=d.object("body").unwrap().vertices().iter().map(|v|v.id).collect();
            apply(&mut d,"reshape",vec![Operation::Transform{object:"body".into(),vertices,matrix:Transform{scale:[1.1,1.,1.],..Default::default()}.matrix().unwrap()},
                Operation::Scene(SceneOperation::Node{object:"eye".into(),node:SceneNode{transform:Transform{translation:[0.23,0.95,-0.4],..Default::default()},..Default::default()}}),
                Operation::Scene(SceneOperation::Node{object:"pupil".into(),node:SceneNode{transform:Transform{translation:[0.23,0.95,-0.53],..Default::default()},..Default::default()}}),
            ]).unwrap();
        }
        commands(&mut d,&format!("rebind-{cycle}"),r#"[{"op":"soft_body_bind","object":"body","deform_objects":["fuzz"],"attachments":[{"name":"eyes","objects":["eye","pupil"],"pivot":[0.23,0.95,-0.4]}]}]"#).unwrap();
        assert_eq!(d.skeleton().unwrap().joints.len(),count,"rebind must not leak generated joints");
        assert_eq!(d.soft_body().unwrap().tet_joints,cells);assert_eq!(d.soft_body().unwrap().attachments[0].joint as u32,frame);
        assert_eq!(d.soft_body().unwrap().attachments[0].rest_pivot,[0.23,0.95,-0.4]);
        assert_eq!(d.rig(),&rig);assert_eq!(d.clips(),&clips);assert_eq!(d.scene().sockets,sockets);
        assert_eq!(d.object("pupil").unwrap().vertices().iter().map(|v|(v.id,v.weights.clone())).collect::<Vec<_>>(),weights);
        assert_eq!(d.skeleton().unwrap().joints[gaze as usize].parent,Some(frame));
        let model=SkinnedModel::parse_glb_validated(&d.compile(None).unwrap().glb).unwrap();
        assert_eq!(model.clips[0].name,"idle");assert_eq!(model.soft_body(),d.soft_body());
    }
    // History codecs include the new unit operation as well as snapshot state.
    let full=d.to_bytes(None).unwrap();let round=Document::from_bytes(&full,d.limits().clone(),None).unwrap();
    assert_eq!(round.to_bytes(None).unwrap(),full);
}

#[test]
fn unbind_is_atomic_and_reserved_cell_collisions_refuse_without_rig_loss() {
    let mut d=fixture();let before=d.to_bytes(None).unwrap();
    assert!(commands(&mut d,"missing",r#"[{"op":"soft_body_unbind"}]"#).is_err());assert_eq!(d.to_bytes(None).unwrap(),before);
    assert!(commands(&mut d,"extra",r#"[{"op":"soft_body_unbind","object":"body"}]"#).is_err());
    bind(&mut d);let before=d.to_bytes(None).unwrap();
    assert!(commands(&mut d,"rollback",r#"[{"op":"soft_body_unbind"},{"op":"soft_body_bind","object":"missing"}]"#).is_err());
    assert_eq!(d.to_bytes(None).unwrap(),before);
    commands(&mut d,"unbind",r#"[{"op":"soft_body_unbind"}]"#).unwrap();
    let history=d.to_bytes(None).unwrap();
    assert_eq!(Document::from_bytes(&history,d.limits().clone(),None).unwrap().to_bytes(None).unwrap(),history);
    let joint=d.skeleton().unwrap().joints.iter().position(|j|j.name=="__soft_body_tet_00").unwrap() as u32;
    apply(&mut d,"rename",vec![Operation::Rig(RigOperation::RenameJoint{joint,name:"user-renamed-cell".into()})]).unwrap();
    let before=d.to_bytes(None).unwrap();
    assert!(commands(&mut d,"collision",r#"[{"op":"soft_body_bind","object":"body"}]"#).is_err());
    assert_eq!(d.to_bytes(None).unwrap(),before);
}

#[test]
fn removing_and_restoring_attachment_group_reuses_its_retained_frame() {
    let mut d=fixture();bind(&mut d);let frame=d.soft_body().unwrap().attachments[0].joint;
    let skeleton=d.skeleton().unwrap().clone();
    commands(&mut d,"detach-group",r#"[{"op":"soft_body_unbind"},{"op":"soft_body_bind","object":"body","deform_objects":["fuzz"]}]"#).unwrap();
    assert!(d.soft_body().unwrap().attachments.is_empty());assert_eq!(d.skeleton(),Some(&skeleton));
    SkinnedModel::parse_glb_validated(&d.compile(None).unwrap().glb).unwrap();
    commands(&mut d,"restore-group",r#"[{"op":"soft_body_unbind"},{"op":"soft_body_bind","object":"body","deform_objects":["fuzz"],"attachments":[{"name":"eyes","objects":["eye","pupil"],"pivot":[0.18,0.95,-0.4]}]}]"#).unwrap();
    assert_eq!(d.soft_body().unwrap().attachments[0].joint,frame);assert_eq!(d.skeleton(),Some(&skeleton));
}

#[test]
fn accessory_affine_weights_fail_during_edit_and_valid_root_binding_remains_publishable() {
    let mut d=fixture();bind(&mut d);
    commands(&mut d,"hat",r#"[{"op":"cube","object":"hat","size":[0.1,0.1,0.1]}]"#).unwrap();
    let before=d.to_bytes(None).unwrap();
    let error=commands(&mut d,"auto-hat",r#"[{"op":"weight_edit","object":"hat","vertices":[],"edit":{"type":"bind","max_influences":4,"power":2}}]"#).unwrap_err();
    assert!(format!("{error}").contains("soft-body affine"),"{error}");assert_eq!(d.to_bytes(None).unwrap(),before);
    let cell=d.soft_body().unwrap().tet_joints[0];
    assert!(commands(&mut d,"wrong-cell",&format!(r#"[{{"op":"weight_edit","object":"hat","vertices":[],"edit":{{"type":"assign","joint":{cell}}}}}]"#)).is_err());
    assert_eq!(d.to_bytes(None).unwrap(),before);
    commands(&mut d,"rigid-hat",r#"[{"op":"weight_edit","object":"hat","vertices":[],"edit":{"type":"assign","joint":0}}]"#).unwrap();
    SkinnedModel::parse_glb_validated(&d.compile(None).unwrap().glb).unwrap();
    // An accessory can deliberately use affine cells too, provided every
    // vertex satisfies the same embedding contract as the runtime consumer.
    let metadata=d.soft_body().unwrap();
    let cage=makepad_game_sim::soft_body::SoftBodyDefinition{
        rest_positions:metadata.rest_positions.clone(),tetrahedra:metadata.tetrahedra.clone(),surface_samples:vec![],anchors:metadata.anchors.clone(),
    };
    let binder=cage.binder().unwrap();
    let mut operations=vec![Operation::Scene(SceneOperation::Node{object:"hat".into(),node:SceneNode{transform:Transform{translation:[0.,0.8,0.],..Default::default()},..Default::default()}})];
    for v in d.object("hat").unwrap().vertices(){
        let cell=binder.bind_point([v.position[0] as f32,(v.position[1]+0.8) as f32,v.position[2] as f32]).unwrap().tetrahedron as usize;
        operations.push(Operation::SetWeights{object:"hat".into(),vertex:v.id,weights:vec![mesh::JointWeight{joint:metadata.tet_joints[cell] as u32,weight:1.}]});
    }
    apply(&mut d,"valid-affine-hat",operations).unwrap();
    SkinnedModel::parse_glb_validated(&d.compile(None).unwrap().glb).unwrap();
}
