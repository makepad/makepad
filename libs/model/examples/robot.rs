//! Headless end-to-end editable, textured, rigged and animated asset example.
//! cargo run --release -p makepad-model --example robot -- <output-directory>
use makepad_model::*;
fn apply(doc:&mut Document,id:&str,operations:Vec<Operation>) {
    doc.apply(Transaction {request_id:id.into(),expected:doc.head(),operations},None).unwrap();
}
fn main()->std::result::Result<(),Box<dyn std::error::Error>> {
    let path=std::env::args_os().nth(1).ok_or("supply output directory")?;
    let path=std::path::PathBuf::from(path);
    let mut doc=Document::new(Limits::default())?;
    apply(&mut doc,"blockout",vec![
        Operation::Cube {object:"body".into(),size:[0.6,0.8,0.3]},
        Operation::Cube {object:"head".into(),size:[0.4,0.4,0.35]},
        Operation::Cube {object:"arm".into(),size:[0.22,0.7,0.22]},
        Operation::SetMaterial {material:1,value:Material {color:[0.18,0.65,0.85],base_color_png:Vec::new()}},
        Operation::SetMaterial {material:2,value:Material {color:[0.9,0.55,0.12],base_color_png:Vec::new()}},
    ]);
    for (i,(name,position,material)) in [
        ("body",[0.0,0.8,0.0],1),("head",[0.0,1.45,0.0],2),("arm",[0.48,0.9,0.0],2)
    ].into_iter().enumerate() {
        let mesh=doc.object(name).unwrap();
        let vertices=mesh.vertices().iter().map(|v|v.id).collect();
        let faces=mesh.faces().iter().map(|f|f.id).collect();
        apply(&mut doc,&format!("place-{i}"),vec![
            Operation::Transform {object:name.into(),vertices,matrix:[
                [1.,0.,0.,position[0]],[0.,1.,0.,position[1]],[0.,0.,1.,position[2]],[0.,0.,0.,1.]]},
            Operation::AssignMaterial {object:name.into(),faces,material},
        ]);
    }
    apply(&mut doc,"surface",vec![
        Operation::TextureSolid {material:1,width:64,height:64,color:[255;3]},
        Operation::PaintTexture {material:1,center:[0.5,0.5],radius:0.2,color:[32,48,72]},
        Operation::SetSkeleton {skeleton:Skeleton {joints:vec![
            Joint {name:"root".into(),parent:None,translation:[0.0,0.8,0.0]},
            Joint {name:"head".into(),parent:Some(0),translation:[0.0,0.65,0.0]},
            Joint {name:"shoulder".into(),parent:Some(0),translation:[0.48,0.4,0.0]},
        ]}},
    ]);
    // Deliberately rigid authored weights make the expected motion unambiguous.
    for (name,joint) in [("body",0),("head",1),("arm",2)] {
        let ops=doc.object(name).unwrap().vertices().iter().map(|v|Operation::SetWeights {
            object:name.into(),vertex:v.id,weights:vec![mesh::JointWeight {joint,weight:1.0}]
        }).collect();
        apply(&mut doc,&format!("bind-{name}"),ops);
    }
    apply(&mut doc,"wave",vec![Operation::SetClip {clip:AnimationClip {
        name:"wave".into(),channels:vec![AnimationChannel {joint:2,path:AnimationPath::Rotation,
            keys:vec![Keyframe {time:0.0,value:[0.,0.,0.,1.]},
                Keyframe {time:0.5,value:[0.,0.,0.5,0.75f64.sqrt()]},
                Keyframe {time:1.0,value:[0.,0.,0.,1.]}]}],
    }}]);
    let source=doc.to_bytes(None)?;
    let reopened=Document::from_bytes(&source,Limits::default(),None)?;
    assert_eq!(reopened.head(),doc.head());
    let compiled=reopened.compile(None)?;
    let parsed=makepad_gltf::parse_glb_bytes(&compiled.glb)?;
    assert_eq!(parsed.document.skins.as_ref().map(Vec::len),Some(1));
    assert_eq!(parsed.document.animations.as_ref().map(Vec::len),Some(1));
    std::fs::create_dir_all(&path)?;
    std::fs::write(path.join("robot.mpmodel"),source)?;
    std::fs::write(path.join("robot.glb"),compiled.glb)?;
    println!("wrote editable source + GLB: {} vertices, {} triangles, {} material primitives, 3 joints, 1 clip",compiled.vertices,compiled.triangles,compiled.primitives.len());
    Ok(())
}
