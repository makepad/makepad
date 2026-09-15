//! Bounded glTF shape-key data. Workers pack immutable target textures; frames
//! sample only small weight tracks, and vertex shaders apply position/normal
//! deltas before the ordinary object or joint transform.
use crate::skin::{Val,JsonParser,Accessors};
use makepad_draw::{makepad_math::{Mat4f,vec4,vec3f},makepad_platform::{Cx,Texture,TextureFormat,TextureUpdated}};
use std::sync::Arc;
const MAX_TARGETS:usize=32;
const MAX_TARGET_BYTES:usize=32*1024*1024;
#[derive(Clone)]
pub struct MorphTrack {pub name:String,pub times:Vec<f32>,pub weights:Vec<f32>,pub step:bool}
#[derive(Clone)]
pub struct AssetMorph {pub targets:usize,pub vertices:usize,pub width:usize,pub height:usize,pub pixels:Vec<f32>,pub defaults:[f32;32],pub clips:Vec<MorphTrack>,pub extent:f32}
#[derive(Clone)]
pub struct UploadedMorph {pub texture:Texture,pub source:Arc<AssetMorph>}
impl AssetMorph {
    pub fn in_static_part_space(mut self,model:&crate::model::StaticModel)->Self{
        let stride=crate::model::MODEL_VERTEX_FLOATS;let mut converted=std::collections::HashSet::new();
        for(part_vertices,transform)in model.anim_parts.iter().map(|p|(&p.vertices,p.rest_transform())).chain(model.driven_parts.iter().map(|p|(&p.vertices,p.rest_transform()))){
            let inverse=transform.invert();
            for vertex in part_vertices.chunks_exact(stride){let source=vertex[6]as usize;if source>=self.vertices||!converted.insert(source){continue}
                for target in 0..self.targets{for lane in 0..2{let at=((target*self.vertices+source)*2+lane)*4;let delta=inverse.transform_vec4(vec4(self.pixels[at],self.pixels[at+1],self.pixels[at+2],0.0));self.pixels[at..at+3].copy_from_slice(&[delta.x,delta.y,delta.z]);}}
            }
        }self
    }
    pub fn upload(mut self,cx:&mut Cx)->UploadedMorph {
        let pixels=std::mem::take(&mut self.pixels);
        let texture=Texture::new_with_format(cx,TextureFormat::VecRGBAf32{width:self.width,height:self.height,data:Some(pixels),updated:TextureUpdated::Full});
        UploadedMorph{texture,source:Arc::new(self)}
    }
    pub fn sample(&self,clip:Option<&str>,time:f32)->[f32;32]{self.sample_playback(clip,time,true)}
    pub fn sample_playback(&self,clip:Option<&str>,time:f32,looping:bool)->[f32;32]{
        let mut out=self.defaults;
        let Some(track)=clip.and_then(|name|self.clips.iter().find(|c|c.name==name)).or_else(||if clip.is_none(){self.clips.first()}else{None})else{return out};
        let duration=track.times.last().copied().unwrap_or(0.0);
        let time=if duration>0.0&&time.is_finite(){if looping{time.rem_euclid(duration)}else{time.clamp(0.0,duration)}}else{0.0};
        let next=track.times.partition_point(|t|*t<=time);let lo=next.saturating_sub(1);let hi=next.min(track.times.len()-1);
        let amount=if track.step||hi==lo{0.0}else{((time-track.times[lo])/(track.times[hi]-track.times[lo])).clamp(0.0,1.0)};
        for i in 0..self.targets{out[i]=track.weights[lo*self.targets+i]*(1.0-amount)+track.weights[hi*self.targets+i]*amount;}
        out
    }
    pub fn parse(bytes:&[u8],skinned:bool)->Result<Option<Self>,String>{
        let(json,bin)=chunks(bytes)?;let acc=Accessors{json:&json,bin};let nodes=json.get("nodes").map(Val::arr).unwrap_or(&[]);let active=active_nodes(&json)?;
        let worlds=node_worlds(&json)?;
        let mut specs=Vec::new();let mut vertices=0usize;let mut target_count=0usize;let mut defaults=None;
        for(node_index,node)in nodes.iter().enumerate(){if !active[node_index]||skinned!=node.get("skin").is_some(){continue}
            let Some(mesh)=node.get("mesh").and_then(Val::usize).and_then(|m|json.get("meshes")?.idx(m))else{continue};
            let weights=node.get("weights").or_else(||mesh.get("weights")).map(Val::arr).unwrap_or(&[]);
            for primitive in mesh.get("primitives").map(Val::arr).unwrap_or(&[]){
                let Some(position)=primitive.get("attributes").and_then(|v|v.get("POSITION")).and_then(Val::usize)else{return Err("morph primitive has no positions".into())};
                let count=json.get("accessors").and_then(|a|a.idx(position)).and_then(|a|a.get("count")).and_then(Val::usize).ok_or("morph position count")?;
                let targets=primitive.get("targets").map(Val::arr).unwrap_or(&[]);target_count=target_count.max(targets.len());
                if targets.len()>MAX_TARGETS{return Err("morph target count exceeds32".into())}
                if !targets.is_empty(){let mut values=[0.0;32];for(i,v)in weights.iter().enumerate(){if i>=targets.len(){return Err("morph default weight count".into())}values[i]=v.f64().ok_or("morph default weight")?as f32;if !values[i].is_finite()||values[i].abs()>8.0{return Err("morph default weight exceeds finite bound8".into())}}
                    if defaults.is_some_and(|old|old!=values){return Err("per-node differing morph weights require separate asset instances".into())}defaults=Some(values);}
                specs.push((node_index,primitive,vertices,count));vertices=vertices.checked_add(count).ok_or("morph vertex overflow")?;
            }
        }
        if target_count==0{return Ok(None)}
        let texels=vertices.checked_mul(target_count).and_then(|n|n.checked_mul(2)).ok_or("morph stream overflow")?;
        if texels.checked_mul(16).is_none_or(|bytes|bytes>MAX_TARGET_BYTES){return Err("morph targets exceed32MiB budget".into())}
        let width=1024usize.min(texels.max(1));let height=texels.div_ceil(width);let mut pixels=vec![0.0;width*height*4];let mut extent=0.0f32;
        for(node,primitive,start,count)in &specs{for(target,streams)in primitive.get("targets").map(Val::arr).unwrap_or(&[]).iter().enumerate(){
            for(lane,name)in ["POSITION","NORMAL"].iter().enumerate(){let Some(accessor)=streams.get(name).and_then(Val::usize)else{continue};let(values,lanes)=acc.read_f32(accessor)?;
                if lanes!=3||values.len()!=count*3||values.iter().any(|v|!v.is_finite()){return Err("invalid morph delta accessor".into())}
                for(vertex,value)in values.chunks_exact(3).enumerate(){let delta=if skinned{vec3f(value[0],value[1],value[2])}else{worlds[*node].transform_vec4(vec4(value[0],value[1],value[2],0.0)).to_vec3f()};
                    if lane==0{extent=extent.max(delta.length())}let at=((target*vertices+start+vertex)*2+lane)*4;pixels[at..at+3].copy_from_slice(&[delta.x,delta.y,delta.z]);
                }
            }
        }}
        let mut clips=Vec::new();
        for(animation_index,animation)in json.get("animations").map(Val::arr).unwrap_or(&[]).iter().enumerate(){
            let mut selected=None;for channel in animation.get("channels").map(Val::arr).unwrap_or(&[]){let Some(target)=channel.get("target")else{continue};if target.get("path").and_then(Val::str)!=Some("weights"){continue}
                let Some(node)=target.get("node").and_then(Val::usize)else{continue};if !specs.iter().any(|s|s.0==node){continue}
                let sampler=channel.get("sampler").and_then(Val::usize).and_then(|s|animation.get("samplers")?.idx(s)).ok_or("morph weight sampler")?;
                let(times,lanes)=acc.read_f32(sampler.get("input").and_then(Val::usize).ok_or("morph weight times")?)?;
                let(weights,weight_lanes)=acc.read_f32(sampler.get("output").and_then(Val::usize).ok_or("morph weight values")?)?;
                if lanes!=1||weight_lanes!=1||times.is_empty()||times.len()>36_001||times.iter().any(|v|!v.is_finite()||*v<0.0)||times.windows(2).any(|w|w[0]>=w[1])||weights.len()!=times.len()*target_count||weights.iter().any(|v|!v.is_finite()||v.abs()>8.0){return Err("invalid/budgeted morph weight track".into())}
                let step=match sampler.get("interpolation").and_then(Val::str).unwrap_or("LINEAR"){"LINEAR"=>false,"STEP"=>true,_=>return Err("morph cubic tracks must be baked before import".into())};
                let candidate=(times,weights,step);if selected.as_ref().is_some_and(|old|old!=&candidate){return Err("differing per-node morph animation tracks require separate assets".into())}selected=Some(candidate);
            }
            if let Some((times,weights,step))=selected{clips.push(MorphTrack{name:animation.get("name").and_then(Val::str).map(str::to_string).unwrap_or_else(||format!("clip_{animation_index}")),times,weights,step});}
            if clips.len()>64{return Err("morph clips exceed64".into())}
        }
        Ok(Some(Self{targets:target_count,vertices,width,height,pixels,defaults:defaults.unwrap_or([0.0;32]),clips,extent}))
    }
}
/// A shadow caster carries the same immutable target texture and sampled
/// weights as its visible instance. No CPU vertex deformation is needed.
#[derive(Clone)]
pub struct DepthMorph {
    pub texture:Texture,
    pub control:makepad_draw::makepad_math::Vec4f,
    pub weights:[makepad_draw::makepad_math::Vec4f;8],
}
impl UploadedMorph {
    pub(crate) fn depth(&self,weights:[f32;32])->DepthMorph {
        DepthMorph{texture:self.texture.clone(),control:vec4(self.source.width as f32,self.source.height as f32,self.source.vertices as f32,self.source.targets as f32),
            weights:std::array::from_fn(|i|vec4(weights[i*4],weights[i*4+1],weights[i*4+2],weights[i*4+3]))}
    }
}
pub(crate) fn chunks(bytes:&[u8])->Result<(Val,&[u8]),String>{if bytes.len()<12||&bytes[..4]!=b"glTF"{return Err("morph GLB magic".into())}let(mut at,mut json,mut bin)=(12usize,None,&[][..]);while at+8<=bytes.len(){let size=u32::from_le_bytes(bytes[at..at+4].try_into().unwrap())as usize;let data=bytes.get(at+8..at+8+size).ok_or("morph GLB chunk")?;match &bytes[at+4..at+8]{b"JSON"=>json=Some(JsonParser::parse(data)?),b"BIN\0"=>bin=data,_=>{}}at+=8+size;}Ok((json.ok_or("morph JSON")?,bin))}
pub(crate) fn active_nodes(json:&Val)->Result<Vec<bool>,String>{let nodes=json.get("nodes").map(Val::arr).unwrap_or(&[]);if nodes.len()>4096{return Err("asset node count exceeds4096".into())}let Some(scenes)=json.get("scenes")else{return Ok(vec![true;nodes.len()])};let scene=json.get("scene").and_then(Val::usize).unwrap_or(0);let roots=scenes.idx(scene).and_then(|s|s.get("nodes")).map(Val::arr).unwrap_or(&[]);let mut active=vec![false;nodes.len()];let mut queue=roots.iter().map(|v|v.usize().ok_or("scene node index")).collect::<Result<Vec<_>,_>>()?;while let Some(i)=queue.pop(){if i>=nodes.len(){return Err("scene node out of bounds".into())}if active[i]{continue}active[i]=true;for child in nodes[i].get("children").map(Val::arr).unwrap_or(&[]){queue.push(child.usize().ok_or("scene child index")?);}}Ok(active)}
pub(crate) fn vertex_offset(json:&Val,node:usize,primitive:usize)->usize{let active=active_nodes(json).unwrap_or_default();let mut offset=0;for(i,n)in json.get("nodes").map(Val::arr).unwrap_or(&[]).iter().enumerate(){if !active.get(i).copied().unwrap_or(false){continue}let Some(mesh)=n.get("mesh").and_then(Val::usize).and_then(|m|json.get("meshes")?.idx(m))else{continue};for(p,prim)in mesh.get("primitives").map(Val::arr).unwrap_or(&[]).iter().enumerate(){if i==node&&p==primitive{return offset}offset+=prim.get("attributes").and_then(|v|v.get("POSITION")).and_then(Val::usize).and_then(|i|json.get("accessors")?.idx(i)?.get("count")?.usize()).unwrap_or(0);}}offset}
pub(crate) fn has_morphs(json:&Val)->bool{json.get("meshes").map(Val::arr).unwrap_or(&[]).iter().any(|m|m.get("primitives").map(Val::arr).unwrap_or(&[]).iter().any(|p|p.get("targets").is_some_and(|t|!t.arr().is_empty())))}
fn node_worlds(json:&Val)->Result<Vec<Mat4f>,String>{let nodes=json.get("nodes").map(Val::arr).unwrap_or(&[]);let mut parents=vec![None;nodes.len()];for(i,n)in nodes.iter().enumerate(){for c in n.get("children").map(Val::arr).unwrap_or(&[]){let c=c.usize().ok_or("node child")?;if c>=nodes.len()||parents[c].replace(i).is_some(){return Err("node parent graph".into())}}}let local=|n:&Val|{if let Some(m)=n.get("matrix"){return Mat4f{v:std::array::from_fn(|i|m.idx(i).and_then(Val::f64).unwrap_or(0.0)as f32)}}let mut trs=crate::skin::NodeTrs::default();let component=|field:&str,i,default|n.get(field).and_then(|v|v.idx(i)).and_then(Val::f64).unwrap_or(default)as f32;trs.t=vec3f(component("translation",0,0.0),component("translation",1,0.0),component("translation",2,0.0));trs.s=vec3f(component("scale",0,1.0),component("scale",1,1.0),component("scale",2,1.0));trs.r=makepad_draw::makepad_math::Quat{x:component("rotation",0,0.0),y:component("rotation",1,0.0),z:component("rotation",2,0.0),w:component("rotation",3,1.0)};crate::skin::trs_to_mat4(&trs)};let mut out=Vec::new();for(i,n)in nodes.iter().enumerate(){let mut world=local(n);let mut parent=parents[i];let mut steps=0;while let Some(p)=parent{steps+=1;if steps>nodes.len(){return Err("cyclic node graph".into())}world=Mat4f::mul(&local(&nodes[p]),&world);parent=parents[p];}out.push(world);}Ok(out)}

#[cfg(test)]
pub(crate) mod tests{
    use super::*;
    pub(crate) fn fixture(skin:bool,rigid:bool)->Vec<u8>{
        let mut bin=Vec::new();let mut views=Vec::new();let mut accessors=Vec::new();
        let mut floats=|values:&[f32],lanes:usize|{let offset=bin.len();for v in values{bin.extend_from_slice(&v.to_le_bytes());}let view=views.len();views.push(format!(r#"{{"buffer":0,"byteOffset":{offset},"byteLength":{}}}"#,values.len()*4));let accessor=accessors.len();accessors.push(format!(r#"{{"bufferView":{view},"componentType":5126,"count":{},"type":"{}"}}"#,values.len()/lanes,match lanes{1=>"SCALAR",3=>"VEC3",4=>"VEC4",_=>panic!()}));accessor};
        let positions=floats(&[0.,0.,0.,1.,0.,0.,0.,1.,0.],3);let normals=floats(&[0.,0.,1.,0.,0.,1.,0.,0.,1.],3);
        let delta=floats(&[0.5,0.,0.,0.5,0.,0.,0.5,0.,0.],3);let ndelta=floats(&[0.;9],3);let times=floats(&[0.,1.],1);let values=floats(&[0.,1.],1);
        let weights=floats(&[1.,0.,0.,0.,1.,0.,0.,0.,1.,0.,0.,0.],4);let joints=floats(&[0.;12],4);let translations=floats(&[0.,0.,0.,2.,0.,0.],3);
        let joint_attrs=if skin{format!(r#", "JOINTS_0":{joints},"WEIGHTS_0":{weights}"#)}else{String::new()};
        let skin_member=if skin{r#", "skin":0"#}else{""};let skins=if skin{r#", "skins":[{"joints":[2]}]"#}else{""};
        let rigid_sampler=if rigid{format!(r#",{{"input":{times},"output":{translations},"interpolation":"STEP"}}"#)}else{String::new()};
        let rigid_channel=if rigid{r#",{"sampler":1,"target":{"node":0,"path":"translation"}}"#}else{""};
        let json=format!(r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0,2]}}],"nodes":[{{"children":[1],"scale":[2,2,2]}},{{"name":"body","mesh":0{skin_member}}},{{"name":"root"}},{{"name":"hidden","mesh":0{skin_member}}}],"buffers":[{{"byteLength":{}}}],"bufferViews":[{}],"accessors":[{}],"meshes":[{{"weights":[0.25],"primitives":[{{"attributes":{{"POSITION":{positions},"NORMAL":{normals}{joint_attrs}}},"targets":[{{"POSITION":{delta},"NORMAL":{ndelta}}}]}}]}}],"animations":[{{"name":"pulse","samplers":[{{"input":{times},"output":{values},"interpolation":"LINEAR"}}{rigid_sampler}],"channels":[{{"sampler":0,"target":{{"node":1,"path":"weights"}}}}{rigid_channel}]}}]{skins}}}"#,bin.len(),views.join(","),accessors.join(","));
        let mut json=json.into_bytes();while json.len()%4!=0{json.push(b' ')}let mut glb=Vec::new();glb.extend_from_slice(b"glTF");glb.extend_from_slice(&2u32.to_le_bytes());glb.extend_from_slice(&((20+json.len()+8+bin.len())as u32).to_le_bytes());glb.extend_from_slice(&(json.len()as u32).to_le_bytes());glb.extend_from_slice(b"JSON");glb.extend(json);glb.extend_from_slice(&(bin.len()as u32).to_le_bytes());glb.extend_from_slice(b"BIN\0");glb.extend(bin);glb
    }
    #[test]fn static_target_ids_default_and_animated_weights_are_consumed(){let bytes=fixture(false,false);let model=crate::StaticModel::parse_glb(&bytes).unwrap();assert_eq!(model.vertices.len()/7,3);assert_eq!(model.vertices.chunks_exact(7).map(|v|v[6]).collect::<Vec<_>>(),vec![0.,1.,2.]);let morph=AssetMorph::parse(&bytes,false).unwrap().unwrap();assert_eq!((morph.targets,morph.vertices),(1,3));assert_eq!(morph.defaults[0],0.25);assert_eq!(morph.sample(Some("pulse"),0.5)[0],0.5);assert_eq!(morph.pixels[0],1.0);assert_eq!(morph.pixels[8],1.0);}
    #[test]fn morph_only_skin_clip_is_valid_and_hidden_mesh_is_excluded(){let bytes=fixture(true,false);let model=crate::skin::SkinnedModel::parse_glb_validated(&bytes).unwrap();assert_eq!(model.clips[0].name,"pulse");assert_eq!(model.clips[0].duration,1.0);assert_eq!(model.rest_gpu_flat().vertices.len()/crate::skin::SKIN_GPU_VERTEX_FLOATS,3);let morph=AssetMorph::parse(&bytes,true).unwrap().unwrap();assert_eq!(morph.vertices,3);assert_eq!(morph.pixels[0],0.5);}
    #[test]fn generic_static_parent_animation_keeps_morphs_in_part_space(){let bytes=fixture(false,true);let model=crate::StaticModel::parse_glb(&bytes).unwrap();assert_eq!(model.anim_parts.len(),1);let part=&model.anim_parts[0];assert_eq!(part.kind.as_deref(),Some("asset-animation"));assert_eq!(part.transform_at(0.5).v[12],0.0,"STEP holds prior key");assert_eq!(part.transform_at(1.0).v[12],2.0);let morph=AssetMorph::parse(&bytes,false).unwrap().unwrap().in_static_part_space(&model);assert_eq!(morph.pixels[0],0.5);assert_eq!(part.vertices.chunks_exact(7).map(|v|v[6]).collect::<Vec<_>>(),vec![0.,1.,2.]);}
    #[test]
    fn once_playback_clamps_end_and_blends_node_transforms_from_rest() {
        let bytes=fixture(false,true);
        let morph=AssetMorph::parse(&bytes,false).unwrap().unwrap();
        assert_eq!(morph.sample_playback(Some("pulse"),1.5,false)[0],1.0);
        assert_eq!(morph.sample_playback(Some("pulse"),1.5,true)[0],0.5);
        let model=crate::StaticModel::parse_glb(&bytes).unwrap();
        let hierarchy=model.anim_parts[0].clip.hierarchy.as_ref().unwrap();
        assert_eq!(hierarchy.transform_named(Some("pulse"),Some(1.5),false).v[12],2.0);
        assert_eq!(hierarchy.transform_named(Some("pulse"),Some(1.5),true).v[12],0.0);
        assert_eq!(hierarchy.transform_named_weighted(Some("pulse"),Some(1.5),false,0.25).v[12],0.5);
        assert_eq!(hierarchy.transform_named(None,None,false).v[12],0.0);
    }
}
