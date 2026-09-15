//! Editable yarn character with real cage physics, rigid facial frames, and
//! ordinary gaze/blink/limb animation. Build with `cargo build --release -p
//! makepad-model --example yarn_character`, then run the resulting executable
//! with an output directory and optional generated albedo PNG. Geometry and
//! surfaces use public authoring APIs; generated color never drives normals.
use makepad_model::*;
use std::{collections::BTreeMap, f64::consts::{PI,TAU}};

fn apply(d:&mut Document,id:&str,operations:Vec<Operation>)->Result<()> {
    d.apply(Transaction{request_id:id.into(),expected:d.head(),operations},None).map_err(|error|{eprintln!("{id}: {error}");error})?;Ok(())
}
fn commands(d:&mut Document,id:&str,source:&str)->Result<()> {
    let operations=parse_operations(&json::parse(source.as_bytes()).map_err(Error::Invalid)?,d.limits()).map_err(|error|{eprintln!("{id}: {error}");error})?;apply(d,id,operations)
}
fn material(d:&mut Document,id:u32,color:[f64;4],roughness:f64)->Result<()> {
    apply(d,&format!("material-{id}"),vec![Operation::SetMaterial{material:id,value:Material::default()},
        Operation::Surface(SurfaceOperation::Material{material:id,value:SurfaceMaterial{base_color:color,roughness,metallic:0.,..Default::default()}})])
}
fn sphere(d:&mut Document,name:&str,center:[f64;3],radii:[f64;3],material:u32)->Result<()> {
    commands(d,&format!("mesh-{name}"),&format!(r#"[{{"op":"sphere","object":"{name}","radius":1,"segments":24,"rings":12,"smooth":true}}]"#))?;
    let mesh=d.object(name).unwrap();let vertices=mesh.vertices().iter().map(|v|v.id).collect();let faces=mesh.faces().iter().map(|f|f.id).collect();
    apply(d,&format!("place-{name}"),vec![Operation::Transform{object:name.into(),vertices,matrix:Transform{translation:center,scale:radii,..Default::default()}.matrix()?},Operation::AssignMaterial{object:name.into(),faces,material}])
}
fn strand(d:&mut Document,name:&str,path:Vec<[f64;3]>,radius:f64,material:u32)->Result<()> {
    let profile=(0..8).map(|i|{let a=TAU*i as f64/8.;[radius*a.cos(),radius*a.sin()]}).collect::<Vec<_>>();
    let vectors=|points:Vec<Vec<f64>>|json::Value::Arr(points.into_iter().map(|p|json::Value::Arr(p.into_iter().map(json::Value::F64).collect())).collect());
    let value=json::obj(vec![("op",json::s("sweep")),("object",json::s(name)),("profile",vectors(profile.into_iter().map(|p|p.to_vec()).collect())),("path",vectors(path.into_iter().map(|p|p.to_vec()).collect())),("caps",json::Value::Bool(true)),("material",json::Value::Int(material as i64))]);
    let ops=parse_operations(&json::Value::Arr(vec![value]),d.limits())?;apply(d,&format!("strand-{name}"),ops)
}
fn normalize(p:[f64;3])->[f64;3]{let n=p.iter().map(|v|v*v).sum::<f64>().sqrt();p.map(|v|v/n)}
fn cross(a:[f64;3],b:[f64;3])->[f64;3]{[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]}

/// A triangulated UV sphere with a smooth recessed mouth, keeping a closed
/// manifold body. Area-weighted normals follow the dent, including its rim.
fn warped_sphere(d:&mut Document,name:&str,radius:f64,segments:u32,rings:u32,material:u32,warp:impl Fn([f64;3])->[f64;3])->Result<()> {
    let temp=json::parse(format!(r#"[{{"op":"sphere","object":"{name}","radius":{radius},"segments":{segments},"rings":{rings}}}]"#).as_bytes()).map_err(Error::Invalid)?;
    let operations=parse_operations(&temp,d.limits())?;
    let Operation::ImportMesh{source,..}=&operations[0] else{unreachable!()};
    let mut ctx=mesh::Context::new(d.limits().mesh.clone(),None);let original=mesh::Mesh::from_bytes(source,&mut ctx)?;
    let mut positions=Vec::new();let mut map=BTreeMap::new();
    for v in original.vertices() {
        map.insert(v.id,positions.len() as u32);positions.push(warp(v.position));
    }
    let mut polygons=Vec::new();
    for face in original.faces() {
        let corners=original.face_corners(face.id)?;
        for i in 1..corners.len()-1 {let triangle=[&corners[0],&corners[i],&corners[i+1]];
            polygons.push(mesh::Polygon{vertices:triangle.iter().map(|c|map[&c.vertex]).collect(),uvs:triangle.iter().map(|c|c.uv).collect(),material});
        }
    }
    let mut normals=vec![[0.;3];positions.len()];
    for face in &polygons {let [a,b,c]=std::array::from_fn(|i|positions[face.vertices[i] as usize]);let n=cross(std::array::from_fn(|i|b[i]-a[i]),std::array::from_fn(|i|c[i]-a[i]));for &v in &face.vertices{for i in 0..3{normals[v as usize][i]+=n[i];}}}
    let mut mesh=mesh::Mesh::from_polygons(&positions,&polygons,&mut ctx)?;
    let indices=mesh.vertices().iter().enumerate().map(|(i,v)|(v.id,i)).collect::<BTreeMap<_,_>>();
    let values=mesh.corners().iter().map(|c|(c.id,Some(normalize(normals[indices[&c.vertex]])))).collect::<Vec<_>>();mesh.set_corner_normals_bulk(&values,&mut ctx)?;
    assert!(mesh.validate(&mut ctx)?.is_closed_manifold);
    apply(d,&format!("shaped-{name}"),vec![Operation::ImportMesh{object:name.into(),source:mesh.to_bytes(&mut ctx)?}])
}
fn body(d:&mut Document)->Result<()> {
    warped_sphere(d,"body",0.45,64,40,0,|mut p|{
        let ellipse=(p[0]/0.185).powi(2)+((p[1]+0.085)/0.09).powi(2);
        if p[2]< -0.2&&ellipse<1. {p[2]+=0.10*(1.-ellipse).powi(2);}
        p[1]+=0.77;p
    })
}
fn child(skeleton:&mut Skeleton,name:&str,parent:u32,translation:[f64;3])->u32 {
    let index=skeleton.joints.len() as u32;skeleton.joints.push(Joint{name:name.into(),parent:Some(parent),translation});index
}
fn bind_objects(d:&Document,objects:&[&str],joint:u32,operations:&mut Vec<Operation>) {
    for object in objects {assert!(d.object(object).is_some());operations.push(Operation::Rig(RigOperation::Weights{object:(*object).into(),vertices:Vec::new(),edit:WeightEdit::Assign{joint}}));}
}
fn rotation(joint:u32,axis:[f64;3],times:&[(f64,f64)])->AnimationChannel {
    AnimationChannel{joint,path:AnimationPath::Rotation,keys:times.iter().map(|&(time,angle)|Keyframe{time,value:transform::quat_axis_angle(axis,angle).unwrap()}).collect()}
}
fn blink(joint:u32,duration:f64)->AnimationChannel {
    AnimationChannel{joint,path:AnimationPath::Scale,keys:vec![Keyframe{time:0.,value:[1.,1.,1.,0.]},Keyframe{time:duration*0.67,value:[1.,1.,1.,0.]},Keyframe{time:duration*0.70,value:[1.,0.04,1.,0.]},Keyframe{time:duration*0.73,value:[1.,1.,1.,0.]},Keyframe{time:duration,value:[1.,1.,1.,0.]}]}
}

fn character()->Result<Document> {
    let mut d=Document::new(Limits{max_joints:128,..Default::default()})?;
    for(id,color,roughness)in[(0,[1.,0.82,0.9,1.],0.95),(1,[1.,0.98,0.96,1.],0.25),(2,[0.10,0.62,1.,1.],0.35),(3,[0.006,0.008,0.018,1.],0.13),(4,[1.,1.,1.,1.],0.1),(5,[0.105,0.008,0.025,1.],0.84),(6,[1.,0.15,0.28,1.],0.75),(7,[0.48,0.025,0.12,1.],0.95),(8,[0.96,0.35,0.57,1.],0.98)] {material(&mut d,id,color,roughness)?;}
    body(&mut d)?;
    commands(&mut d,"yarn-surface",r#"[
      {"op":"surface_layer","material":0,"channel":"base_color","layer":"yarn","width":512,"height":512},
      {"op":"surface_pattern","material":0,"channel":"base_color","layer":"yarn","pattern":{"kind":"yarn","color_a":[0.56,0.22,0.34,1],"color_b":[0.87,0.43,0.56,1],"scale":[72,50],"seed":"42"}},
      {"op":"surface_derive","material":0,"source_channel":"base_color","source_layer":"yarn","channel":"normal","layer":"fiber-normal","strength":0.0006,"wrap":true},
      {"op":"surface_derive","material":0,"source_channel":"base_color","source_layer":"yarn","channel":"metallic_roughness","layer":"fiber-roughness","roughness_min":0.90,"roughness_max":0.99,"metallic":0},
      {"op":"fiber_shell","object":"fuzz","source":"body","count":1400,"length":0.012,"width":0.0006,"seed":42,"material":8}
    ]"#)?;
    for (side,x) in [("left",-0.167),("right",0.167)] {
        sphere(&mut d,&format!("{side}-sclera"),[x,0.97,-0.413],[0.165,0.18,0.108],1)?;
        sphere(&mut d,&format!("{side}-iris"),[x+0.012,0.967,-0.513],[0.092,0.103,0.026],2)?;
        sphere(&mut d,&format!("{side}-pupil"),[x+0.018,0.969,-0.537],[0.054,0.065,0.014],3)?;
        sphere(&mut d,&format!("{side}-highlight"),[x-0.010,1.009,-0.551],[0.025,0.029,0.009],4)?;
        sphere(&mut d,&format!("{side}-glint"),[x+0.039,0.946,-0.550],[0.010,0.012,0.007],4)?;
        let brow=(0..17).map(|i|{let t=i as f64/16.;[x-0.14+0.28*t,1.15+0.05*(PI*t).sin(),-0.393]}).collect();
        strand(&mut d,&format!("{side}-brow"),brow,0.014,7)?;
    }
    sphere(&mut d,"mouth-interior",[0.,0.684,-0.365],[0.157,0.070,0.018],5)?;
    sphere(&mut d,"tongue",[0.,0.645,-0.391],[0.068,0.020,0.010],6)?;
    // A single closed enamel band curves upward and recedes at the smile's
    // corners. Its front stays behind the lip, inside the body's mouth recess.
    warped_sphere(&mut d,"upper-teeth",1.,32,16,4,|p|[0.148*p[0],0.702+0.024*p[1]+0.020*p[0]*p[0],-0.416+0.010*p[2]+0.016*p[0]*p[0]])?;
    let smile=(0..25).map(|i|{let t=i as f64/24.;let x=-0.17+0.34*t;let y=0.737-0.12*(PI*t).sin();[x,y,-(0.45f64.powi(2)-x*x-(y-0.77).powi(2)).sqrt()-0.006]}).collect();
    strand(&mut d,"smile-rim",smile,0.012,7)?;
    let upper=(0..19).map(|i|{let t=i as f64/18.;let x=-0.168+0.336*t;let y=0.737-0.011*(PI*t).sin();[x,y,-(0.45f64.powi(2)-x*x-(y-0.77).powi(2)).sqrt()-0.005]}).collect();
    strand(&mut d,"upper-lip",upper,0.008,7)?;
    for(side,sign)in[("left",-1.),("right",1.)] {
        strand(&mut d,&format!("{side}-arm"),vec![[sign*0.405,0.79,0.],[sign*0.49,0.70,-0.015],[sign*0.57,0.56,-0.07]],0.023,0)?;
        sphere(&mut d,&format!("{side}-hand"),[sign*0.575,0.535,-0.075],[0.043,0.052,0.035],0)?;
        strand(&mut d,&format!("{side}-leg"),vec![[sign*0.17,0.405,0.],[sign*0.18,0.26,-0.005],[sign*0.18,0.12,-0.025]],0.024,0)?;
        sphere(&mut d,&format!("{side}-foot"),[sign*0.18,0.077,-0.07],[0.058,0.050,0.105],0)?;
    }
    let heart_segments=[
        [[0.,0.024],[-0.02,0.056],[-0.04,0.065],[-0.055,0.056]],
        [[-0.055,0.056],[-0.075,0.05],[-0.085,0.02],[-0.07,0.002]],
        [[-0.07,0.002],[-0.060,-0.021],[-0.031,-0.043],[-0.015,-0.055]],
        [[-0.015,-0.055],[-0.005,-0.063],[0.005,-0.063],[0.015,-0.055]],
        [[0.015,-0.055],[0.031,-0.043],[0.060,-0.021],[0.07,0.002]],
        [[0.07,0.002],[0.085,0.02],[0.075,0.05],[0.055,0.056]],
        [[0.055,0.056],[0.04,0.065],[0.02,0.05],[0.006,0.031]],
    ];
    let heart=heart_segments.iter().flat_map(|p|(0..8).map(move|i|{let t=i as f64/8.;let u=1.-t;let q=std::array::from_fn::<_,2,_>(|a|u*u*u*p[0][a]+3.*u*u*t*p[1][a]+3.*u*t*t*p[2][a]+t*t*t*p[3][a]);let x=0.255+q[0];let y=0.69+q[1];let z=-(0.45f64.powi(2)-x*x-(y-0.77).powi(2)).sqrt()-0.01;[x,y,z]})).collect();
    strand(&mut d,"heart-strand",heart,0.005,6)?;
    let top_heart=heart_segments.iter().flat_map(|p|(0..8).map(move|i|{let t=i as f64/8.;let u=1.-t;let q=std::array::from_fn::<_,2,_>(|a|u*u*u*p[0][a]+3.*u*u*t*p[1][a]+3.*u*t*t*p[2][a]+t*t*t*p[3][a]);[0.025+q[0],1.60+q[1],-0.025]})).collect();
    strand(&mut d,"top-heart",top_heart,0.007,8)?;
    let stalk=(0..25).map(|i|{let t=i as f64/24.;[0.025*t+0.023*(PI*t).sin(),1.185+0.36*t,-0.025*t]}).collect();
    strand(&mut d,"top-stalk",stalk,0.009,0)?;
    commands(&mut d,"soft-body-bind",r#"[{"op":"soft_body_bind","object":"body","deform_objects":["fuzz","heart-strand"],"preset":"yarn_ball","attachments":[
      {"name":"face-left","objects":["left-sclera","left-iris","left-pupil","left-highlight","left-glint","left-brow"],"pivot":[-0.167,0.97,-0.413]},
      {"name":"face-right","objects":["right-sclera","right-iris","right-pupil","right-highlight","right-glint","right-brow"],"pivot":[0.167,0.97,-0.413]},
      {"name":"mouth-frame","objects":["mouth-interior","tongue","smile-rim","upper-lip","upper-teeth"],"pivot":[0,0.684,-0.40]},
      {"name":"top-stalk-frame","objects":["top-stalk","top-heart"],"pivot":[0,1.185,0]},
      {"name":"arm-left-frame","objects":["left-arm","left-hand"],"pivot":[-0.405,0.79,0]},
      {"name":"arm-right-frame","objects":["right-arm","right-hand"],"pivot":[0.405,0.79,0]},
      {"name":"leg-left-frame","objects":["left-leg","left-foot"],"pivot":[-0.17,0.405,0]},
      {"name":"leg-right-frame","objects":["right-leg","right-foot"],"pivot":[0.17,0.405,0]}
    ]}]"#)?;
    let frames=d.soft_body().unwrap().attachments.iter().map(|a|(a.name.clone(),a.joint as u32)).collect::<BTreeMap<_,_>>();
    let mut skeleton=d.skeleton().unwrap().clone();let mut operations=Vec::new();let mut blink_joints=Vec::new();let mut gaze_joints=Vec::new();
    for side in ["left","right"] {
        let blink=child(&mut skeleton,&format!("{side}-blink"),frames[&format!("face-{side}")],[0.;3]);
        let gaze=child(&mut skeleton,&format!("{side}-gaze"),blink,[0.;3]);blink_joints.push(blink);gaze_joints.push(gaze);
        bind_objects(&d,&[&format!("{side}-sclera")],blink,&mut operations);
        for part in ["iris","pupil","highlight","glint"] {bind_objects(&d,&[&format!("{side}-{part}")],gaze,&mut operations);}
    }
    let mut limbs=Vec::new();
    for side in ["left","right"] {for limb in ["arm","leg"] {
        let joint=child(&mut skeleton,&format!("{side}-{limb}-swing"),frames[&format!("{limb}-{side}-frame")],[0.;3]);limbs.push((side,limb,joint));
        bind_objects(&d,&[&format!("{side}-{limb}"),&format!("{side}-{}",if limb=="arm"{"hand"}else{"foot"})],joint,&mut operations);
    }}
    let stalk_joint=child(&mut skeleton,"top-stalk-sway",frames["top-stalk-frame"],[0.;3]);
    bind_objects(&d,&["top-stalk","top-heart"],stalk_joint,&mut operations);
    operations.insert(0,Operation::SetSkeleton{skeleton});
    // A single atomic rig update keeps all body and attachment binding checks
    // valid while assigning descendants of the rigid frames.
    apply(&mut d,"independent-face-and-limb-rig",operations)?;
    let mut idle=Vec::new();for &joint in &blink_joints{idle.push(blink(joint,3.));}for &joint in &gaze_joints{idle.push(rotation(joint,[0.,1.,0.],&[(0.,0.),(0.8,-0.09),(1.5,0.1),(2.2,0.02),(3.,0.)]));}
    idle.push(rotation(stalk_joint,[0.,0.,1.],&[(0.,0.),(0.75,0.10),(1.5,0.),(2.25,-0.10),(3.,0.)]));
    for &(side,limb,joint) in &limbs{if limb=="arm"{idle.push(rotation(joint,[0.,0.,1.],&[(0.,0.),(1.5,if side=="left"{0.08}else{-0.08}),(3.,0.)]));}}
    let mut walk=Vec::new();for &joint in &blink_joints{walk.push(blink(joint,1.));}for &(side,limb,joint) in &limbs{let sign=if (side=="left")== (limb=="leg"){1.}else{-1.};let amplitude=if limb=="leg"{0.42}else{0.26};walk.push(rotation(joint,[1.,0.,0.],&[(0.,sign*amplitude),(0.5,-sign*amplitude),(1.,sign*amplitude)]));}
    walk.push(rotation(stalk_joint,[0.,0.,1.],&[(0.,-0.13),(0.5,0.13),(1.,-0.13)]));
    apply(&mut d,"idle-and-walk",vec![Operation::SetClip{clip:AnimationClip{name:"idle".into(),channels:idle}},Operation::SetClip{clip:AnimationClip{name:"walk".into(),channels:walk}}])?;
    Ok(d)
}

fn main()->std::result::Result<(),Box<dyn std::error::Error>> {
    let output=std::path::PathBuf::from(std::env::args_os().nth(1).ok_or("supply output directory")?);
    let mut d=character()?;
    if let Some(path)=std::env::args_os().nth(2) {
        let bytes=std::fs::read(path)?;let image=RgbaImage::from_png(&bytes,d.limits())?;
        let mut layer=SurfaceLayer::new("generated-yarn-color",image);layer.opacity=0.25;
        apply(&mut d,"generated-color-only",vec![Operation::Surface(SurfaceOperation::Layer{material:0,channel:SurfaceChannel::BaseColor,layer})])?;
    }
    // Keep the editable final source while compacting construction receipts
    // and undo history before durable sharing with another client.
    d.checkpoint(d.head())?;
    let source=d.to_bytes(None).map_err(|error|{eprintln!("source export: {error}");error})?;
    let restored=Document::from_bytes(&source,d.limits().clone(),None).map_err(|error|{eprintln!("source reopen: {error}");error})?;
    assert_eq!(restored.head(),d.head());assert_eq!(restored.soft_body(),d.soft_body());
    let product=restored.compile(None).map_err(|error|{eprintln!("GLB export: {error}");error})?;let loaded=makepad_render::skin::SkinnedModel::parse_glb_validated(&product.glb)?;
    assert_eq!(loaded.soft_body(),d.soft_body());assert_eq!(loaded.clips.len(),2);
    std::fs::create_dir_all(&output)?;std::fs::write(output.join("yarn-character.mpmodel"),source)?;std::fs::write(output.join("yarn-character.glb"),&product.glb)?;
    println!("yarn character: {} triangles, {} joints, 43 particles, 80 tetrahedra, {} GLB bytes; {}",product.triangles,loaded.joint_count(),product.glb.len(),output.display());Ok(())
}
