//! Shape-key streams and baked weight tracks for the portable authoring product.
use crate::{augment::{GlbRewrite,number,object,string,validation},GltfError,JsonValue};
use std::collections::HashMap;
#[derive(Clone,Debug,Default)] pub struct GlbMorphDeltas{pub positions:Vec<[f32;3]>,pub normals:Vec<[f32;3]>}
#[derive(Clone,Debug)] pub struct GlbMorphTarget{pub name:String,pub weight:f32,pub primitives:Vec<GlbMorphDeltas>}
#[derive(Clone,Debug)] pub struct GlbAuthoringClip{pub name:String,pub step:bool,pub times:Vec<f32>,pub weights:Vec<f32>,pub metadata:Option<JsonValue>}
fn fields(v:&mut JsonValue)->Result<&mut HashMap<String,JsonValue>,GltfError>{match v{JsonValue::Object(f)=>Ok(f),_=>Err(validation("morph expected object"))}}
fn idx(v:&JsonValue)->Option<usize>{match v{JsonValue::U64(i)=>usize::try_from(*i).ok(),JsonValue::I64(i)=>usize::try_from(*i).ok(),_=>None}}
fn append(r:&mut GlbRewrite,views:&mut Vec<JsonValue>,accessors:&mut Vec<JsonValue>,values:&[f32],lanes:usize,kind:&'static str,bounds:bool)->Result<usize,GltfError>{
    if values.is_empty()||values.len()%lanes!=0||values.iter().any(|v|!v.is_finite()){return Err(validation("invalid morph accessor"));}
    let mut data=Vec::with_capacity(values.len()*4);for v in values{data.extend_from_slice(&v.to_le_bytes());}
    let range=|minimum:bool|JsonValue::Array((0..lanes).map(|lane|JsonValue::F64(values.chunks_exact(lanes).map(|v|v[lane]).fold(if minimum{f32::INFINITY}else{f32::NEG_INFINITY},|a,b|if minimum{a.min(b)}else{a.max(b)})as f64)).collect());
    Ok(r.append_accessor(views,accessors,&data,5126,values.len()/lanes,kind,false,None,bounds.then(||range(true)),bounds.then(||range(false))))
}
/// Adds one common target ordering to every material primitive, including zero
/// deltas for objects unaffected by a target. This keeps split meshes and skins
/// compatible with the same named weight animation.
pub fn augment_glb_morphs(input:&[u8],targets:&[GlbMorphTarget],clips:&[GlbAuthoringClip])->Result<Vec<u8>,GltfError>{
    if targets.len()>32||clips.len()>64{return Err(validation("morph/clip budget"));}
    let mut r=GlbRewrite::begin(input)?;let mut views=r.take_array("bufferViews")?;let mut accessors=r.take_array("accessors")?;
    let mut meshes=r.take_array("meshes")?;let mut nodes=r.take_array("nodes")?;let mut animations=r.take_array("animations")?;
    if !targets.is_empty(){
        let original=r.document.meshes_slice().first().ok_or_else(||validation("missing original morph mesh"))?;
        let originals=original.primitives.clone();let mut by_position=HashMap::new();
        for (p,primitive) in originals.iter().enumerate(){
            let position=*primitive.attributes.get("POSITION").ok_or_else(||validation("morph POSITION"))?;
            let count=r.document.accessors_slice().get(position).ok_or_else(||validation("morph accessor"))?.count;
            let mut streams=Vec::new();
            for target in targets{
                if target.primitives.len()!=originals.len()||!target.weight.is_finite(){return Err(validation("morph primitive count/weight"));}
                let d=&target.primitives[p];if d.positions.len()!=count||d.normals.len()!=count{return Err(validation("morph vertex count"));}
                let pos=append(&mut r,&mut views,&mut accessors,&d.positions.iter().flatten().copied().collect::<Vec<_>>(),3,"VEC3",true)?;
                let normal=append(&mut r,&mut views,&mut accessors,&d.normals.iter().flatten().copied().collect::<Vec<_>>(),3,"VEC3",false)?;
                streams.push(object([("POSITION",number(pos)),("NORMAL",number(normal))]));
            }by_position.insert(position,JsonValue::Array(streams));
        }
        for mesh in &mut meshes{
            let f=fields(mesh)?;let Some(JsonValue::Array(primitives))=f.get_mut("primitives")else{return Err(validation("morph mesh primitives"));};
            for p in primitives{let p=fields(p)?;let position=match p.get("attributes"){Some(JsonValue::Object(a))=>a.get("POSITION").and_then(idx),_=>None}.ok_or_else(||validation("morph primitive position"))?;
                p.insert("targets".into(),by_position.get(&position).ok_or_else(||validation("unmapped morph primitive"))?.clone());}
            f.insert("weights".into(),JsonValue::Array(targets.iter().map(|t|JsonValue::F64(t.weight as f64)).collect()));
            let extras=f.entry("extras".into()).or_insert_with(||object([]));fields(extras)?.insert("targetNames".into(),JsonValue::Array(targets.iter().map(|t|string(&t.name)).collect()));
        }
    }
    let animated_nodes=nodes.iter_mut().enumerate().filter_map(|(i,n)|fields(n).ok().and_then(|f|f.get("mesh").and_then(idx)).map(|_|i)).collect::<Vec<_>>();
    for clip in clips{
        let found=animations.iter_mut().position(|a|fields(a).ok().and_then(|f|f.get("name")).is_some_and(|v|matches!(v,JsonValue::String(n)if n==&clip.name)));
        let animation=if let Some(i)=found{i}else{animations.push(object([("name",string(&clip.name)),("samplers",JsonValue::Array(vec![])),("channels",JsonValue::Array(vec![]))]));animations.len()-1};
        let f=fields(&mut animations[animation])?;
        if let Some(meta)=&clip.metadata{f.insert("extras".into(),meta.clone());}
        let mut samplers=match f.remove("samplers"){Some(JsonValue::Array(a))=>a,_=>return Err(validation("animation samplers"))};
        let mut channels=match f.remove("channels"){Some(JsonValue::Array(a))=>a,_=>return Err(validation("animation channels"))};
        if clip.step{for sampler in &mut samplers{fields(sampler)?.insert("interpolation".into(),string("STEP"));}}
        if !clip.times.is_empty(){
            if targets.is_empty()||clip.weights.len()!=clip.times.len()*targets.len()||clip.times[0]<0.||clip.times.windows(2).any(|w|w[0]>=w[1]){return Err(validation("morph weight track"));}
            let input=append(&mut r,&mut views,&mut accessors,&clip.times,1,"SCALAR",true)?;
            let output=append(&mut r,&mut views,&mut accessors,&clip.weights,1,"SCALAR",false)?;
            let sampler=samplers.len();samplers.push(object([("input",number(input)),("output",number(output)),("interpolation",string(if clip.step{"STEP"}else{"LINEAR"}))]));
            for &node in &animated_nodes{channels.push(object([("sampler",number(sampler)),("target",object([("node",number(node)),("path",string("weights"))]))]));}
        }
        f.insert("samplers".into(),JsonValue::Array(samplers));f.insert("channels".into(),JsonValue::Array(channels));
    }
    r.put_array("meshes",meshes);r.put_array("nodes",nodes);if !animations.is_empty(){r.put_array("animations",animations);}
    r.put_array("bufferViews",views);r.put_array("accessors",accessors);r.finish()
}
