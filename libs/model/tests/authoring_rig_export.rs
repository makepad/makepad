use makepad_model::*;
use makepad_gltf::{ParsedGlb,JsonValue};
use std::collections::BTreeMap;
fn apply(d:&mut Document,id:&str,ops:Vec<Operation>){d.apply(Transaction{request_id:id.into(),expected:d.head(),operations:ops},None).unwrap();}
fn values(g:&ParsedGlb,index:usize,lanes:usize)->Vec<f32>{let a=&g.document.accessors_slice()[index];let v=&g.document.buffer_views_slice()[a.buffer_view.unwrap()];let at=v.byte_offset.unwrap_or(0)+a.byte_offset.unwrap_or(0);g.bin_chunk.as_ref().unwrap()[at..at+a.count*lanes*4].chunks_exact(4).map(|b|f32::from_le_bytes(b.try_into().unwrap())).collect()}
fn arr(v:&JsonValue)->&[JsonValue]{let JsonValue::Array(a)=v else{panic!("array")};a}
fn idx(v:&JsonValue)->usize{match v{JsonValue::U64(i)=>*i as usize,JsonValue::I64(i)=>*i as usize,_=>panic!("index")}}
fn field<'a>(v:&'a JsonValue,key:&str)->&'a JsonValue{let JsonValue::Object(v)=v else{panic!("object")}; &v[key]}
#[test]
fn rotated_scaled_rest_inverse_bind_and_joint_lamp_are_portable(){
    let mut d=Document::new(Limits::default()).unwrap();let rest=Transform{translation:[3.,2.,1.],rotation:transform::quat_axis_angle([0.,0.,1.],0.7).unwrap(),scale:[2.,1.,0.5]};
    apply(&mut d,"rig",vec![Operation::Cube{object:"body".into(),size:[1.;3]},Operation::SetSkeleton{skeleton:Skeleton{joints:vec![Joint{name:"root".into(),parent:None,translation:[0.;3]}]}},Operation::AutoWeights{object:"body".into()},Operation::Rig(RigOperation::Rest{joint:0,transform:rest}),Operation::Scene(SceneOperation::Light(LightEmitter{name:"lamp".into(),attachment:Attachment::Joint(0),transform:Transform::default(),kind:LightKind::Point,color:[1.;3],intensity:10.,range:5.}))]);
    let glb=makepad_gltf::parse_glb_bytes(&d.compile(None).unwrap().glb).unwrap();let skin=&glb.document.skins.as_ref().unwrap()[0];let joint=idx(&arr(field(skin,"joints"))[0]);let node=&glb.document.nodes_slice()[joint];
    assert_eq!(node.translation,Some([3.,2.,1.]));assert_eq!(node.scale,Some([2.,1.,0.5]));
    let ibm=values(&glb,idx(field(skin,"inverseBindMatrices")),16);let inverse=transform::inverse(rest.matrix().unwrap()).unwrap();for r in 0..4{for c in 0..4{assert!((ibm[c*4+r]as f64-inverse[r][c]).abs()<1e-6);}}
    let lamp=glb.document.nodes_slice().iter().position(|n|n.name.as_deref()==Some("lamp")).unwrap();assert!(node.children.as_ref().unwrap().contains(&lamp));
}
#[test]
fn static_shape_key_streams_and_defaults_follow_object_partitions(){
    let mut d=Document::new(Limits::default()).unwrap();apply(&mut d,"mesh",vec![Operation::Cube{object:"body".into(),size:[2.;3]},Operation::Scene(SceneOperation::Instance{object:"copy".into(),source:"body".into(),transform:Transform{translation:[4.,0.,0.],..Default::default()}})]);
    let deltas=d.object("body").unwrap().vertices().iter().map(|v|(v.id,[0.,0.25,0.])).collect();
    apply(&mut d,"shape",vec![Operation::Rig(RigOperation::Morph{name:"raise".into(),object:"body".into(),deltas,weight:0.4})]);
    let bytes=d.to_bytes(None).unwrap();let round=Document::from_bytes(&bytes,Limits::default(),None).unwrap();assert_eq!(d.head(),round.head());
    let glb=makepad_gltf::parse_glb_bytes(&round.compile(None).unwrap().glb).unwrap();
    for n in glb.document.nodes_slice().iter().filter(|n|n.mesh.is_some()){
        let m=&glb.document.meshes_slice()[n.mesh.unwrap()];assert_eq!(m.weights,Some(vec![0.4]));for p in &m.primitives{let delta=values(&glb,p.targets.as_ref().unwrap()[0]["POSITION"],3);assert!(delta.chunks_exact(3).all(|p|p==[0.,0.25,0.]));let normal=values(&glb,p.targets.as_ref().unwrap()[0]["NORMAL"],3);assert!(normal.iter().all(|v|v.abs()<1e-6));}
    }
}
#[test]
fn shape_weight_keys_step_and_events_export_as_real_animation(){
    let mut d=Document::new(Limits::default()).unwrap();apply(&mut d,"mesh",vec![Operation::Cube{object:"body".into(),size:[1.;3]},Operation::SetSkeleton{skeleton:Skeleton{joints:vec![Joint{name:"root".into(),parent:None,translation:[0.;3]}]}},Operation::AutoWeights{object:"body".into()},Operation::SetClip{clip:AnimationClip{name:"act".into(),channels:vec![AnimationChannel{joint:0,path:AnimationPath::Translation,keys:vec![Keyframe{time:0.,value:[0.;4]},Keyframe{time:1.,value:[1.,0.,0.,0.]}]}]}}]);
    let deltas=d.object("body").unwrap().vertices().iter().map(|v|(v.id,[0.,0.2,0.])).collect();
    apply(&mut d,"shape",vec![Operation::Rig(RigOperation::Morph{name:"raise".into(),object:"body".into(),deltas,weight:0.}),Operation::Rig(RigOperation::ClipOptions{name:"act".into(),options:ClipOptions{interpolation:Interpolation::Step,root_motion:true,events:vec![ClipEvent{time:0.5,name:"footstep".into(),payload:"left".into()}],morph_keys:BTreeMap::from([("raise".into(),vec![MorphKey{time:0.,weight:0.},MorphKey{time:1.,weight:1.}])])}})]);
    let glb=makepad_gltf::parse_glb_bytes(&d.compile(None).unwrap().glb).unwrap();let a=&glb.document.animations.as_ref().unwrap()[0];let channel=arr(field(a,"channels")).iter().find(|c|matches!(field(field(c,"target"),"path"),JsonValue::String(p)if p=="weights")).unwrap();let sampler=&arr(field(a,"samplers"))[idx(field(channel,"sampler"))];assert!(matches!(field(sampler,"interpolation"),JsonValue::String(s)if s=="STEP"));assert_eq!(values(&glb,idx(field(sampler,"output")),1),vec![0.,1.]);assert!(matches!(field(field(field(a,"extras"),"MAKEPAD_animation"),"root_motion"),JsonValue::Bool(true)));
}
