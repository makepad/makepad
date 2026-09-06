use makepad_model::{json::{self,Value},*};
use std::collections::BTreeMap;
fn fixture()->Engine{
    let mut d=Document::new(Limits::default()).unwrap();let mut image=RgbaImage::new(32,32,[0;4],d.limits()).unwrap();for(i,p)in image.pixels.chunks_exact_mut(4).enumerate(){p.copy_from_slice(&[(i%255) as u8,(i/255) as u8,128,255]);}
    d.apply(Transaction{request_id:"seed".into(),expected:d.head(),operations:vec![Operation::Cube{object:"body".into(),size:[1.;3]},Operation::Surface(SurfaceOperation::Layer{material:0,channel:SurfaceChannel::BaseColor,layer:SurfaceLayer::new("paint",image)}),Operation::SetSkeleton{skeleton:Skeleton{joints:vec![Joint{name:"root".into(),parent:None,translation:[0.;3]}]}},Operation::AutoWeights{object:"body".into()},Operation::SetClip{clip:AnimationClip{name:"clip".into(),channels:vec![AnimationChannel{joint:0,path:AnimationPath::Translation,keys:vec![Keyframe{time:0.,value:[0.;4]},Keyframe{time:1.,value:[0.,1.,0.,0.]}]}]}},Operation::Rig(RigOperation::Pose(Pose{name:"pose".into(),joints:BTreeMap::from([(0,Transform::default())])})),Operation::Rig(RigOperation::ClipOptions{name:"clip".into(),options:ClipOptions{events:vec![ClipEvent{time:0.5,name:"event".into(),payload:"\0".repeat(4096)}],..Default::default()}})]},None).unwrap();
    let vertex=d.object("body").unwrap().vertices()[0].id;let face=d.object("body").unwrap().faces()[0].id;let ops=parse_operations(&json::parse(format!(r#"[{{"op":"selection","object":"body","name":"chosen","vertices":["{}"],"faces":["{}"]}}]"#,vertex.0,face.0).as_bytes()).unwrap(),d.limits()).unwrap();d.apply(Transaction{request_id:"selection".into(),expected:d.head(),operations:ops},None).unwrap();let mut e=Engine::new(Limits::default());e.open("doc",Some(&d.to_bytes(None).unwrap()),None).unwrap();e
}
fn inspect(e:&mut Engine,domain:&str,offset:u32)->Value{let mut args=vec![("document",json::s("doc")),("domain",json::s(domain)),("offset",Value::Int(offset as i64)),("limit",Value::Int(128))];if ["uv_islands","uv_island_faces","uv_pins","solid"].contains(&domain){args.push(("object",json::s("body")));}e.execute("model.inspect",&json::obj(args),None).unwrap()}
#[test]fn every_extended_domain_obeys_shared_page_envelope_and_source_is_unchanged(){let mut e=fixture();let before=e.document("doc").unwrap().to_bytes(None).unwrap();for domain in EXTENDED_INSPECTION_DOMAINS{let result=inspect(&mut e,domain,0);assert!(result.to_json().len()<=MAX_REPLY_BYTES,"{domain}");assert!(result.get("total").is_some());assert!(result.get("count").is_some());assert!(result.get("next_offset").is_some());}assert_eq!(e.document("doc").unwrap().to_bytes(None).unwrap(),before);let solid=inspect(&mut e,"solid",0);assert_eq!(solid.get("items").unwrap().as_arr().unwrap()[0].get("is_valid_solid").and_then(Value::as_bool),Some(true),"{}",solid.to_json());}
#[test]fn binary_pixels_and_escaped_event_payload_have_complete_advancing_pages(){let mut e=fixture();let mut offset=0;let mut pixels=Vec::new();loop{let result=inspect(&mut e,"surface_pixels",offset);assert!(result.to_json().len()<=MAX_REPLY_BYTES);for row in result.get("items").unwrap().as_arr().unwrap(){pixels.extend(row.get("rgba").unwrap().as_arr().unwrap().iter().map(|v|v.as_u64().unwrap()as u8));}match result.get("next_offset").and_then(Value::as_u64){Some(next)=>{assert!(next>offset as u64);offset=next as u32;},None=>break}}assert_eq!(pixels,e.document("doc").unwrap().surface().materials[&0].channels[&SurfaceChannel::BaseColor][0].image.pixels);
    let preview=inspect(&mut e,"clip_events",0);let row=&preview.get("items").unwrap().as_arr().unwrap()[0];assert_eq!(row.get("payload_omitted").and_then(Value::as_u64),Some(3584));let mut text=String::new();let mut offset=0;loop{let page=inspect(&mut e,"clip_event_payload",offset);assert!(page.to_json().len()<=MAX_REPLY_BYTES);for row in page.get("items").unwrap().as_arr().unwrap(){assert_eq!(row.get("byte_offset").and_then(Value::as_u64),Some(text.len()as u64));text.push_str(row.get("text").unwrap().as_str().unwrap());}match page.get("next_offset").and_then(Value::as_u64){Some(next)=>{assert!(next>offset as u64);offset=next as u32;},None=>break}}assert_eq!(text,"\0".repeat(4096));
}
#[test]fn selections_have_stable_ids_and_cancelled_global_checks_return_no_certificate(){let mut e=fixture();let groups=inspect(&mut e,"selections",0);assert_eq!(groups.get("total").and_then(Value::as_u64),Some(1));let vertices=inspect(&mut e,"selection_vertices",0);assert!(vertices.get("items").unwrap().as_arr().unwrap()[0].get("vertex").unwrap().as_str().is_some());let before=e.document("doc").unwrap().to_bytes(None).unwrap();assert!(e.execute("model.inspect",&json::obj(vec![("document",json::s("doc")),("object",json::s("body")),("domain",json::s("solid"))]),Some(&||true)).is_err());assert_eq!(e.document("doc").unwrap().to_bytes(None).unwrap(),before);}

#[test]fn joint_inventory_reports_effective_rest_override(){
    let mut e=fixture();let expected=head_json(e.document("doc").unwrap().head());
    let operations=json::parse(br#"[{"op":"rig_rest","joint":0,"transform":{"translation":[3,4,5],"rotation":[0,0,0,1],"scale":[2,2,2]}}]"#).unwrap();
    e.execute("model.apply",&json::obj(vec![("document",json::s("doc")),("request_id",json::s("rest")),("expected",expected),("operations",operations)]),None).unwrap();
    let joints=inspect(&mut e,"joints",0);let row=&joints.get("items").unwrap().as_arr().unwrap()[0];
    assert_eq!(row.get("translation").unwrap().to_json(),"[3.0,4.0,5.0]");
    assert_eq!(row.get("scale").unwrap().to_json(),"[2.0,2.0,2.0]");
}

#[test]
fn vehicle_wheel_inventory_returns_bound_local_geometry_and_advances() {
    let mut e = Engine::new(Limits::default());
    e.open("doc", None, None).unwrap();
    let expected = head_json(e.document("doc").unwrap().head());
    let operations = json::parse(br#"[
        {"op":"cube","object":"tire","size":[0.2,0.7,0.7]},
        {"op":"vehicle_wheel","object":"tire","connection":"wheel_front_left","pivot":[0,0,0],"radius":0.35,"width":0.2}
    ]"#).unwrap();
    e.execute("model.apply", &json::obj(vec![("document", json::s("doc")),
        ("request_id", json::s("wheel")), ("expected", expected), ("operations", operations)]), None).unwrap();
    let page = inspect(&mut e, "vehicle_wheels", 0);
    assert_eq!(page.get("count").and_then(Value::as_u64), Some(1));
    let row = &page.get("items").unwrap().as_arr().unwrap()[0];
    assert_eq!(row.get("connection").and_then(Value::as_str), Some("wheel_front_left"));
    assert_eq!(row.get("object").and_then(Value::as_str), Some("tire"));
    assert_eq!(row.get("radius"), Some(&Value::F64(0.35)));
    assert_eq!(inspect(&mut e, "vehicle_wheels", 1).get("count").and_then(Value::as_u64), Some(0));
    let mut args = json::obj(vec![("document", json::s("doc")), ("domain", json::s("vehicle_wheels")), ("object", json::s("tire"))]);
    assert_eq!(e.execute("model.inspect", &args, None).unwrap().get("total").and_then(Value::as_u64), Some(1));
    if let Value::Obj(fields) = &mut args { fields.iter_mut().find(|(key,_)|key=="object").unwrap().1 = json::s("missing"); }
    assert_eq!(e.execute("model.inspect", &args, None).unwrap().get("total").and_then(Value::as_u64), Some(0));
}
