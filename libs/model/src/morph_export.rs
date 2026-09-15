use crate::{Document,PrimitiveSource,Result,Error,mesh,transform::*,Interpolation,json};
use makepad_gltf::*;

fn num(v:f64)->Result<f32>{let out=v as f32;if out.is_finite(){Ok(out)}else{Err(Error::Invalid("morph render precision"))}}
pub(crate) fn source<'a>(doc:&'a Document,name:&'a str)->Result<&'a str>{let mut name=name;for _ in 0..=doc.limits().max_objects{
    match doc.scene().nodes.get(name).and_then(|n|n.linked_to.as_deref()){Some(next)=>name=next,None=>return Ok(name)}
}Err(Error::Invalid("linked morph cycle"))}
fn json_value(value:json::Value)->Result<JsonValue>{Ok(match value{
    json::Value::Null=>JsonValue::Null,json::Value::Bool(v)=>JsonValue::Bool(v),json::Value::Int(v)=>JsonValue::I64(v),json::Value::F64(v)=>JsonValue::F64(v),json::Value::Str(v)=>JsonValue::String(v),
    json::Value::Arr(v)=>JsonValue::Array(v.into_iter().map(json_value).collect::<Result<_>>()?),
    json::Value::Obj(v)=>JsonValue::Object(v.into_iter().map(|(k,v)|Ok((k,json_value(v)?))).collect::<Result<_>>()?),
})}
pub(crate) fn finish(doc:&Document,glb:Vec<u8>,primitives:&[PrimitiveSource],ctx:&mut mesh::Context<'_>)->Result<Vec<u8>>{
    if doc.rig().morphs.is_empty()&&doc.rig().clip_options.is_empty(){return Ok(glb);}
    let vertices=primitives.iter().map(|p|p.positions.len()).sum::<usize>();
    if vertices.saturating_mul(doc.rig().morphs.len()).saturating_mul(128).saturating_add(glb.len()*3)>ctx.limits.max_bytes{return Err(Error::Budget("morph export working bytes"));}
    let mut targets=Vec::new();
    for morph in doc.rig().morphs.values(){
        let mut streams=Vec::new();
        for primitive in primitives{
            ctx.checkpoint(1)?;let count=primitive.positions.len();let active=source(doc,&primitive.object)?==morph.object;
            let world=if doc.skeleton().is_some(){doc.scene().world_matrix(&primitive.object)?}else{IDENTITY_MATRIX};
            let mut positions=Vec::with_capacity(count);let mut normals=vec![[0.;3];count];
            for vertex in &primitive.source_vertices{ctx.checkpoint(1)?;let delta=if active{morph.deltas.get(vertex).copied().unwrap_or([0.;3])}else{[0.;3]};let d=transform_vector(world,delta);positions.push([num(d[0])?,num(d[1])?,num(d[2])?]);}
            if active{for base in (0..count).step_by(3){
                let p=|i:usize|primitive.positions[base+i].map(|v|v as f64);let q=|i:usize|add(p(i),positions[base+i].map(|v|v as f64));
                let before=cross(sub(p(1),p(0)),sub(p(2),p(0)));let after=cross(sub(q(1),q(0)),sub(q(2),q(0)));
                let rotation=quat_from_to(before,after).map_err(|_|Error::Invalid("shape key collapses a render triangle"))?;
                for i in 0..3{let n=primitive.normals[base+i].map(|v|v as f64);let d=sub(quat_rotate(rotation,n),n);normals[base+i]=[num(d[0])?,num(d[1])?,num(d[2])?];}
            }}streams.push(GlbMorphDeltas{positions,normals});
        }targets.push(GlbMorphTarget{name:morph.name.clone(),weight:num(morph.weight)?,primitives:streams});
    }
    let mut clips=Vec::new();
    for (name,options) in &doc.rig().clip_options{
        let clip=doc.clips().get(name).ok_or(Error::Invalid("clip metadata target"))?;
        let mut times=Vec::new();let mut weights=Vec::new();
        if !options.morph_keys.is_empty(){
            let duration=clip.duration();
            if options.interpolation==Interpolation::Cubic{let steps=(duration*60.).ceil()as usize;if steps>doc.limits().max_keyframes{return Err(Error::Budget("baked morph frames"));}
                for i in 0..=steps{times.push((i as f64/60.).min(duration));}}
            else{times.push(0.);times.push(duration);for keys in options.morph_keys.values(){times.extend(keys.iter().map(|k|k.time));}times.sort_by(f64::total_cmp);times.dedup();}
            if times.len().saturating_mul(targets.len())>doc.limits().max_keyframes{return Err(Error::Budget("morph weight tracks"));}
            for &time in &times{for morph in doc.rig().morphs.values(){ctx.checkpoint(1)?;
                let weight=options.morph_keys.get(&morph.name).map(|keys|{
                    let hi=keys.partition_point(|k|k.time<=time).min(keys.len()-1);let lo=hi.saturating_sub(1);let a=&keys[lo];let b=&keys[hi];
                    if time<=a.time{return a.weight;}if time>=b.time{return b.weight;}
                    let t=(time-a.time)/(b.time-a.time);let t=match options.interpolation{Interpolation::Step=>0.,Interpolation::Linear=>t,Interpolation::Cubic=>t*t*(3.-2.*t)};a.weight+(b.weight-a.weight)*t
                }).unwrap_or(morph.weight);weights.push(num(weight)?);
            }}
        }
        let metadata=json_value(json::obj(vec![("MAKEPAD_animation",json::obj(vec![
            ("root_motion",json::Value::Bool(options.root_motion)),
            ("events",json::Value::Arr(options.events.iter().map(|e|json::obj(vec![("time",json::Value::F64(e.time)),("name",json::s(&e.name)),("payload",json::s(&e.payload))])).collect())),
            ("interpolation",json::s(match options.interpolation{Interpolation::Linear=>"linear",Interpolation::Step=>"step",Interpolation::Cubic=>"cubic"}))]))]))?;
        clips.push(GlbAuthoringClip{name:name.clone(),step:options.interpolation==Interpolation::Step,times:times.into_iter().map(num).collect::<Result<_>>()?,weights,metadata:Some(metadata)});
    }
    ctx.checkpoint((vertices*targets.len())as u64)?;
    augment_glb_morphs(&glb,&targets,&clips).map_err(|_|Error::Invalid("morph/animation GLB augmentation"))
}
