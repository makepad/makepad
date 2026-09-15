use makepad_model::*;
use makepad_render::{StaticModel,renderer::ModelInstance,asset_lights::parse_asset_lights};

fn apply(doc:&mut Document,id:&str,operations:Vec<Operation>){doc.apply(Transaction{request_id:id.into(),expected:doc.head(),operations},None).unwrap();}
fn parsed(doc:&Document,source:&str)->Vec<Operation>{parse_operations(&json::parse(source.as_bytes()).unwrap(),doc.limits()).unwrap()}
fn vehicle()->Document {
    let mut d=Document::new(Limits::default()).unwrap();
    let ops=parsed(&d,r#"[{"op":"cube","object":"body","size":[1.8,0.8,4]},
        {"op":"object_node","object":"body","node":{"transform":{"translation":[4,1,-2]}}},
        {"op":"texture_solid","material":0,"width":4,"height":4,"color":[100,40,20]},
        {"op":"light","name":"headlight","attachment":{"object":"body"},"transform":{"translation":[0.6,0.1,2.01],"rotation":[0,1,0,0]},"kind":"spot","inner":0.1,"outer":0.4,"color":[1,0.9,0.7],"intensity":500,"range":30}]"#);
    apply(&mut d,"body",ops);
    for (i,connection) in VEHICLE_WHEEL_CONNECTIONS.iter().enumerate(){
        let object=format!("tire_{i}");let x=if i%2==0{0.95}else{-0.95};let z=if i<2{1.3}else{-1.3};
        // Preserve this historical source fixture's faceted cylinders even
        // when the default for newly generated primitives changes.
        let ops=parsed(&d,&format!(r#"[{{"op":"cylinder","object":"{object}","radius":0.35,"height":0.24,"segments":12,"smooth":false}}]"#));
        apply(&mut d,&format!("wheel_geometry_{i}"),ops);
        let vertices=d.object(&object).unwrap().vertices().iter().map(|v|v.id).collect();
        let ops=vec![Operation::Transform{object:object.clone(),vertices,matrix:[[0.,-1.,0.,0.],[1.,0.,0.,0.],[0.,0.,1.,0.],[0.,0.,0.,1.]]},
            Operation::Scene(SceneOperation::Node{object:object.clone(),node:SceneNode{parent:Some("body".into()),transform:Transform{translation:[x,-0.35,z],..Default::default()},..Default::default()}}),
            Operation::Scene(SceneOperation::VehicleWheel{object,wheel:VehicleWheel{connection:(*connection).into(),pivot:[0.;3],radius:0.35,width:0.24,visual:None}})];
        apply(&mut d,&format!("wheel_binding_{i}"),ops);
    }
    d
}

#[test]
fn textured_editable_vehicle_roundtrips_into_four_driven_wheels(){
    let doc=vehicle();let source=doc.to_bytes(None).unwrap();
    // Recorded before visual-wheel metadata existed: omission must preserve
    // canonical source identity for every existing library snapshot.
    assert_eq!(doc.head().content,[247,4,205,15,64,99,16,22,106,30,9,56,249,217,107,238,56,119,176,169,130,193,56,131,201,124,242,203,184,105,138,13]);
    let restored=Document::from_bytes(&source,Limits::default(),None).unwrap();
    assert_eq!(restored.head(),doc.head());assert_eq!(restored.scene().wheels,doc.scene().wheels);
    let product=restored.compile(None).unwrap();assert_eq!(product.glb,doc.compile(None).unwrap().glb);
    let model=StaticModel::parse_glb(&product.glb).unwrap();
    assert_eq!(model.driven_parts.len(),4);assert_eq!(model.indices.len(),36,"wheels must leave the rigid chassis stream");
    assert_eq!(model.indices.len()+model.driven_parts.iter().map(|p|p.indices.len()).sum::<usize>(),product.triangles*3);
    for (i,connection) in VEHICLE_WHEEL_CONNECTIONS.iter().enumerate(){
        let wheel=model.driven_parts.iter().find(|p|&p.connection==connection).unwrap();
        assert_eq!(wheel.indices.len(),132);assert!(!wheel.layers.is_empty());
        let x=4.+if i%2==0{0.95}else{-0.95};let z=-2.+if i<2{1.3}else{-1.3};
        assert!((wheel.anchor.x-x).abs()<1e-5&&(wheel.anchor.y-0.65).abs()<1e-5&&(wheel.anchor.z-z).abs()<1e-5);
        assert!((wheel.radius-0.35).abs()<1e-6&&(wheel.width-0.24).abs()<1e-6);
        assert!(wheel.visual.is_none());
        let rest=wheel.rest_transform();assert!((rest.v[12]-wheel.anchor.x).abs()<1e-5&&(rest.v[14]-wheel.anchor.z).abs()<1e-5);
        assert!((wheel.max.x-wheel.min.x-0.24).abs()<1e-5,"wheel axle geometry is X");
    }
    assert!(makepad_render::model::embedded_base_color_png(&product.glb).is_some());
}

#[test]
fn authored_headlight_tracks_the_car_body_and_faces_its_driving_direction(){
    let product=vehicle().compile(None).unwrap();let glb=makepad_gltf::parse_glb_bytes(&product.glb).unwrap();
    let model=StaticModel::parse_glb(&product.glb).unwrap();let lights=parse_asset_lights(&glb.document).unwrap();assert_eq!(lights.len(),1);
    let (sin,cos)=0.7f32.sin_cos();let mut frame=model.driven_parts[0].parent;
    frame.v=[cos,0.,-sin,0.,0.,1.,0.,0.,sin,0.,cos,0.,8.,2.,5.,1.];
    let instance=ModelInstance::on_body("car".into(),(model.min,model.max),1.5,0.8,&frame);
    let light=lights[0].placed(&instance.transform,None);
    let m=instance.transform.v;let expected=[m[0]*4.6+m[4]*1.1+m[8]*0.01+m[12],m[1]*4.6+m[5]*1.1+m[9]*0.01+m[13],m[2]*4.6+m[6]*1.1+m[10]*0.01+m[14]];
    assert!((light.pos.x-expected[0]).abs()<1e-5&&(light.pos.y-expected[1]).abs()<1e-5&&(light.pos.z-expected[2]).abs()<1e-5);
    assert!((light.dir.x+frame.v[8]).abs()<1e-5&&(light.dir.y+frame.v[9]).abs()<1e-5&&(light.dir.z+frame.v[10]).abs()<1e-5,"headlight must aim along body -Z");
    assert_eq!(light.radius,30.);assert_eq!(light.color.x,500.);
}

#[test]
fn invalid_or_incomplete_vehicle_bindings_refuse_without_mutating_source(){
    let mut doc=vehicle();let original=doc.to_bytes(None).unwrap();
    for source in [
        r#"[{"op":"vehicle_wheel","object":"tire_0","connection":"wheel_rear_right","pivot":[0,0,0],"radius":0.35,"width":0.24}]"#,
        r#"[{"op":"vehicle_wheel","object":"tire_0","connection":"bogus","pivot":[0,0,0],"radius":0.35,"width":0.24}]"#,
        r#"[{"op":"vehicle_wheel","object":"tire_0","connection":"wheel_front_left","pivot":[0,0,0],"radius":0,"width":0.24}]"#,
        r#"[{"op":"vehicle_wheel","object":"tire_0","connection":"wheel_front_left","pivot":[0,0,0],"radius":1e-200,"width":0.24}]"#,
        r#"[{"op":"object_node","object":"tire_0","node":{"parent":"body","transform":{"scale":[1,2,1]}}}]"#,
    ]{let operations=parsed(&doc,source);assert!(doc.apply(Transaction{request_id:"invalid".into(),expected:doc.head(),operations},None).is_err());assert_eq!(doc.to_bytes(None).unwrap(),original);}
    let ops=parsed(&doc,r#"[{"op":"delete_vehicle_wheel","object":"tire_3"}]"#);apply(&mut doc,"incomplete",ops);
    assert!(doc.compile(None).is_err());doc.undo(doc.head(),None).unwrap();assert!(doc.compile(None).is_ok());
    let mut generic=Document::new(Limits::default()).unwrap();apply(&mut generic,"ordinary",vec![Operation::Cube{object:"ordinary".into(),size:[1.;3]}]);assert!(generic.compile(None).is_ok());
}

#[test]
fn changing_a_wheel_pivot_preserves_the_exported_anchor(){
    let mut doc=vehicle();let old=StaticModel::parse_glb(&doc.compile(None).unwrap().glb).unwrap();
    apply(&mut doc,"pivot",vec![Operation::Scene(SceneOperation::Pivot{object:"tire_0".into(),position:[0.1,0.2,0.3]})]);
    let new=StaticModel::parse_glb(&doc.compile(None).unwrap().glb).unwrap();
    let before=old.driven_parts.iter().find(|p|p.connection==VEHICLE_WHEEL_CONNECTIONS[0]).unwrap();
    let after=new.driven_parts.iter().find(|p|p.connection==VEHICLE_WHEEL_CONNECTIONS[0]).unwrap();
    assert!((before.anchor-after.anchor).length()<1e-5);assert!((after.pivot.x+0.1).abs()<1e-6);
}

#[test]
fn visual_wheel_limits_roundtrip_without_changing_physical_connections(){
    let mut doc=vehicle();let legacy_head=doc.head();
    let legacy=StaticModel::parse_glb(&doc.compile(None).unwrap().glb).unwrap();
    let visual=VisualWheelMotion{steer_gain:0.55,steer_max:0.32,compression:0.08,droop:0.10};
    let operations=VEHICLE_WHEEL_CONNECTIONS.iter().enumerate().map(|(i,connection)|{
        let object=format!("tire_{i}");let mut wheel=doc.scene().wheels[&object].clone();
        assert_eq!(&wheel.connection,connection);
        wheel.visual=Some(VisualWheelMotion{steer_gain:if i<2{visual.steer_gain}else{0.},..visual});
        Operation::Scene(SceneOperation::VehicleWheel{object,wheel})
    }).collect();
    apply(&mut doc,"visual_wheel_limits",operations);
    assert_ne!(doc.head().content,legacy_head.content);
    let source=doc.to_bytes(None).unwrap();
    let restored=Document::from_bytes(&source,Limits::default(),None).unwrap();
    assert_eq!(restored.head(),doc.head());assert_eq!(restored.scene().wheels,doc.scene().wheels);
    let product=restored.compile(None).unwrap();
    let model=StaticModel::parse_glb(&product.glb).unwrap();
    for part in &model.driven_parts{
        let before=legacy.driven_parts.iter().find(|p|p.connection==part.connection).unwrap();
        let expected=restored.scene().wheels.values().find(|w|w.connection==part.connection).unwrap().visual.unwrap();
        assert_eq!(part.visual,Some(expected));
        assert_eq!(part.radius,before.radius);assert_eq!(part.width,before.width);
        assert_eq!(part.anchor,before.anchor);assert_eq!(part.pivot,before.pivot);
        assert_eq!(part.rest_transform().v,before.rest_transform().v);
        // Packed color/UV lanes can have NaN bit patterns; compare the stream
        // as stored bytes instead of floating-point numeric equality.
        assert!(part.vertices.iter().map(|v|v.to_bits()).eq(before.vertices.iter().map(|v|v.to_bits())));
        assert_eq!(part.indices,before.indices);
        assert_eq!(expected.pose(1.,1.),(if part.connection.contains("front"){0.32}else{0.},0.08));
        assert_eq!(expected.pose(-1.,-1.).1,-0.10);
    }
    // Rebinding without the optional field removes the display contract and
    // restores the exact legacy content identity, not an emitted null/default.
    let operations=doc.scene().wheels.iter().map(|(object,wheel)|{
        let mut wheel=wheel.clone();wheel.visual=None;
        Operation::Scene(SceneOperation::VehicleWheel{object:object.clone(),wheel})
    }).collect();
    apply(&mut doc,"restore_legacy_motion",operations);
    assert_eq!(doc.head().content,legacy_head.content);
    assert!(!doc.scene().value().to_json().contains("visual"));
}

#[test]
fn invalid_visual_wheel_limits_refuse_atomically(){
    let mut doc=vehicle();let source=doc.to_bytes(None).unwrap();
    for visual in [
        "null", "{}",
        r#"{"steer_gain":0.55,"steer_max":0.32,"compression":0.08}"#,
        r#"{"steer_gain":1.01,"steer_max":0.32,"compression":0.08,"droop":0.10}"#,
        r#"{"steer_gain":-0.01,"steer_max":0.32,"compression":0.08,"droop":0.10}"#,
        r#"{"steer_gain":0.55,"steer_max":1.21,"compression":0.08,"droop":0.10}"#,
        r#"{"steer_gain":0.55,"steer_max":-0.01,"compression":0.08,"droop":0.10}"#,
        r#"{"steer_gain":0.55,"steer_max":0.32,"compression":-0.01,"droop":0.10}"#,
        r#"{"steer_gain":0.55,"steer_max":0.32,"compression":5.01,"droop":0.10}"#,
        r#"{"steer_gain":0.55,"steer_max":0.32,"compression":0.08,"droop":5.01}"#,
        r#"{"steer_gain":0.55,"steer_max":0.32,"compression":0.08,"droop":0.10,"typo":1}"#,
    ]{
        let request=format!(r#"[{{"op":"vehicle_wheel","object":"tire_0","connection":"wheel_front_left","pivot":[0,0,0],"radius":0.35,"width":0.24,"visual":{visual}}}]"#);
        let result=parse_operations(&json::parse(request.as_bytes()).unwrap(),doc.limits())
            .and_then(|operations|doc.apply(Transaction{request_id:"invalid_visual".into(),expected:doc.head(),operations},None).map(|_|()));
        assert!(result.is_err(),"accepted {visual}");
        assert_eq!(doc.to_bytes(None).unwrap(),source);
    }
}
