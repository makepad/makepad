use makepad_model::*;
use makepad_gltf::JsonValue;
fn apply(d:&mut Document,id:&str,ops:Vec<Operation>){d.apply(Transaction{request_id:id.into(),expected:d.head(),operations:ops},None).unwrap();}
fn field<'a>(v:&'a JsonValue,key:&str)->&'a JsonValue{let JsonValue::Object(v)=v else{panic!("object")}; &v[key]}
fn arr(v:&JsonValue)->&[JsonValue]{let JsonValue::Array(v)=v else{panic!("array")};v}
fn idx(v:&JsonValue)->usize{match v{JsonValue::U64(i)=>*i as usize,JsonValue::I64(i)=>*i as usize,_=>panic!("index")}}
fn make(skinned:bool)->Document{
    let mut d=Document::new(Limits::default()).unwrap();let ops=parse_operations(&json::parse(br#"[{"op":"sphere","object":"ball","radius":1,"segments":12,"rings":8}]"#).unwrap(),d.limits()).unwrap();apply(&mut d,"shape",ops);
    if skinned{apply(&mut d,"rig",vec![Operation::SetSkeleton{skeleton:Skeleton{joints:vec![Joint{name:"root".into(),parent:None,translation:[0.;3]}]}},Operation::AutoWeights{object:"ball".into()}]);}
    apply(&mut d,"delivery",vec![Operation::Scene(SceneOperation::Lods{object:"ball".into(),levels:vec![LodLevel{target_faces:120,max_error:1.,distance:10.},LodLevel{target_faces:80,max_error:2.,distance:30.}]}),Operation::Scene(SceneOperation::Collider{object:"ball".into(),proxy:CollisionProxy::Sphere})]);d
}
#[test] fn lod_geometry_is_real_reduced_hidden_and_collision_is_portable(){verify(false)}
#[test] fn skinned_lods_keep_common_skin_and_rest_space(){verify(true)}
#[test]
fn painted_colors_survive_every_static_and_skinned_lod() {
    for skinned in [false,true] {
        let mut d=make(skinned);
        let vertices=d.object("ball").unwrap().vertices().iter().map(|v|v.id).collect();
        let color=[0.8,0.12,0.05,1.];
        apply(&mut d,"paint",vec![Operation::Surface(SurfaceOperation::VertexPaint{
            object:"ball".into(),vertices,color,opacity:1.,
        })]);
        let before=d.to_bytes(None).unwrap();
        let product=d.compile(None).unwrap();
        assert_eq!(d.to_bytes(None).unwrap(),before,"derived paint must not change the source");
        let g=makepad_gltf::load_gltf_from_bytes(&product.glb,None).unwrap();
        let nodes=g.document.nodes_slice();
        let base=nodes.iter().position(|node|node.extensions.as_ref().is_some_and(|value|
            matches!(value,JsonValue::Object(fields)if fields.contains_key("MSFT_lod")))).unwrap();
        let levels=arr(field(field(nodes[base].extensions.as_ref().unwrap(),"MSFT_lod"),"ids"));
        assert_eq!(levels.len(),2);
        for node in std::iter::once(base).chain(levels.iter().map(idx)) {
            let mesh=nodes[node].mesh.unwrap();
            for primitive in 0..g.document.meshes_slice()[mesh].primitives.len() {
                let decoded=makepad_gltf::decode_mesh_primitive(&g,mesh,primitive).unwrap();
                let colors=decoded.colors0.expect("COLOR_0 at every LOD");
                assert!(!colors.is_empty());
                assert!(colors.iter().all(|actual|actual.iter().zip(color).all(|(a,b)|(*a as f64-b).abs()<1e-6)),
                    "paint changed at node {node}, skinned={skinned}");
            }
        }
    }
}
fn verify(skinned:bool){let d=make(skinned);let before=d.to_bytes(None).unwrap();let product=d.compile(None).unwrap();assert_eq!(d.to_bytes(None).unwrap(),before);
    let g=makepad_gltf::parse_glb_bytes(&product.glb).unwrap();let nodes=g.document.nodes_slice();let base=nodes.iter().position(|n|n.extensions.as_ref().is_some_and(|v|matches!(v,JsonValue::Object(f)if f.contains_key("MSFT_lod")))).unwrap();
    let ids=arr(field(field(nodes[base].extensions.as_ref().unwrap(),"MSFT_lod"),"ids"));assert_eq!(ids.len(),2);
    let count=|i:usize|g.document.meshes_slice()[nodes[i].mesh.unwrap()].primitives.iter().map(|p|g.document.accessors_slice()[p.indices.unwrap()].count/3).sum::<usize>();
    let mut previous=count(base);for id in ids{let id=idx(id);let n=count(id);assert!(n<previous);previous=n;assert!(nodes.iter().all(|n|!n.children.as_ref().is_some_and(|c|c.contains(&id))));if skinned{assert_eq!(nodes[id].skin,Some(0));}}
    let owner=nodes.iter().find(|n|n.name.as_deref()==Some("ball")).unwrap();assert!(matches!(field(field(owner.extras.as_ref().unwrap(),"MAKEPAD_collision"),"kind"),JsonValue::String(s)if s=="sphere"));
}
