//! Source-preserving material/normal revision for the authored workshop car.
//! No beam/steering/collider or vertex positions are changed.
use makepad_model::*;
fn main()->std::result::Result<(),Box<dyn std::error::Error>>{
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    if args.len()!=2{return Err("usage: polish_headlights <source.mpmodel> <output.mpmodel>".into());}
    let mut doc=Document::from_bytes(&std::fs::read(&args[0])?,Limits{max_joints:128,..Default::default()},None)?;
    let before=doc.head();let scene=doc.scene().clone();
    let before_positions=doc.objects().map(|(name,m)|(name.to_string(),m.vertices().iter().map(|v|(v.id,v.position)).collect::<Vec<_>>())).collect::<Vec<_>>();
    let lens=doc.object("headlamp-lens").ok_or("expected headlamp-lens source object")?;
    let faces=lens.faces().iter().map(|f|f.id).collect();
    let material=doc.materials().keys().copied().max().unwrap_or(0)+1;
    let mut operations=vec![Operation::SetMaterial{material,value:Material{color:[1.,0.88,0.68],base_color_png:Vec::new()}},
        Operation::Surface(SurfaceOperation::Material{material,value:SurfaceMaterial{base_color:[1.,0.91,0.76,1.],metallic:0.,roughness:0.14,
            emissive:[1.,0.80,0.52],emissive_strength:3.5,..Default::default()}}),
        Operation::AssignMaterial{object:"headlamp-lens".into(),faces,material}];
    let curved=doc.objects().filter(|(name,_)|name.starts_with("headlamp")||*name=="hubcap"||name.contains("fender")||name.contains("tire")).map(|(name,_)|name.to_string()).collect::<Vec<_>>();
    for name in &curved {
        let args=json::obj(vec![("op",json::s("normals")),("object",json::s(name)),("smooth",json::Value::Bool(true)),("angle",json::Value::F64(1.2))]);
        operations.extend(parse_operations(&json::Value::Arr(vec![args]),doc.limits())?);
    }
    doc.apply(Transaction{request_id:"warm-luminous-headlamp-lenses".into(),expected:before,operations},None)?;
    assert_eq!(doc.scene(),&scene,"beam lights, wheel contracts and placement must stay identical");
    for(name,vertices)in before_positions{assert_eq!(vertices,doc.object(&name).unwrap().vertices().iter().map(|v|(v.id,v.position)).collect::<Vec<_>>());}
    let checks=doc.publication_checks(None)?;if checks.get("valid")!=Some(&json::Value::Bool(true)){return Err(checks.to_json().into());}
    let compiled=doc.compile(None)?;let source=doc.to_snapshot_bytes(None)?;
    std::fs::write(&args[1],&source)?;
    let output=std::path::Path::new(&args[1]);std::fs::write(output.with_extension("glb"),&compiled.glb)?;
    println!("{}",json::obj(vec![("head",head_json(doc.head())),("lens_material",json::Value::Int(material as i64)),("emissive_strength",json::Value::F64(3.5)),("smoothed",json::Value::Arr(curved.into_iter().map(json::s).collect())),("triangles",json::Value::Int(compiled.triangles as i64)),("vertices_unchanged",json::Value::Bool(true)),("lights_wheels_scene_unchanged",json::Value::Bool(true))]).to_json());Ok(())
}
