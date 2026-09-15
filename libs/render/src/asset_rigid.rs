//! Ordinary glTF node animation without a game-specific states convention.
//! Each rigid mesh keeps its ancestor chain and shares immutable channel data.
use crate::skin::{Accessors,NodeTrs,Val,trs_to_mat4};
use makepad_draw::makepad_math::{Mat4f,Quat,vec3f};
use std::sync::Arc;
#[derive(Clone,Debug)]struct RigidChannel{path:u8,times:Vec<f32>,values:Vec<f32>,step:bool}
#[derive(Clone,Debug)]struct RigidNode{rest:NodeTrs,matrix:Option<Mat4f>,channels:Arc<Vec<RigidChannel>>}
#[derive(Clone,Debug)]pub struct RigidHierarchy{pub times:Vec<f32>,pub clip_name:String,nodes:Vec<RigidNode>,clips:std::collections::BTreeMap<String,(f32,Vec<RigidNode>)>}
impl RigidHierarchy{
    pub fn has_clip(&self,name:&str)->bool{self.clips.contains_key(name)}
    pub fn transform_named(&self,name:Option<&str>,time:Option<f32>,looping:bool)->Mat4f{
        self.transform_named_weighted(name,time,looping,1.0)
    }
    pub fn transform_named_weighted(&self,name:Option<&str>,time:Option<f32>,looping:bool,weight:f32)->Mat4f{
        let (duration,nodes)=name.and_then(|name|self.clips.get(name)).map(|(d,n)|(*d,n)).unwrap_or((self.times.last().copied().unwrap_or(0.0),&self.nodes));
        let time=time.map(|t|if looping&&duration>0.0{t.rem_euclid(duration)}else{t.clamp(0.0,duration)});
        Self::sample_nodes_weighted(nodes,time,weight.clamp(0.0,1.0))
    }
    pub fn transform(&self,time:Option<f32>)->Mat4f{Self::sample_nodes(&self.nodes,time)}
    fn sample_nodes(nodes:&[RigidNode],time:Option<f32>)->Mat4f{
        Self::sample_nodes_weighted(nodes,time,1.0)
    }
    fn sample_nodes_weighted(nodes:&[RigidNode],time:Option<f32>,weight:f32)->Mat4f{
        let mut world=Mat4f::identity();
        for node in nodes{
            let mut trs=node.rest;
            if let Some(t)=time{for channel in node.channels.iter(){channel.apply(t,&mut trs)}}
            if weight < 1.0 {
                trs.t = node.rest.t + (trs.t-node.rest.t)*weight;
                trs.s = node.rest.s + (trs.s-node.rest.s)*weight;
                let mut q=trs.r;
                if node.rest.r.dot(q)<0.0 {q=q.neg();}
                let q=Quat{x:node.rest.r.x+(q.x-node.rest.r.x)*weight,y:node.rest.r.y+(q.y-node.rest.r.y)*weight,z:node.rest.r.z+(q.z-node.rest.r.z)*weight,w:node.rest.r.w+(q.w-node.rest.r.w)*weight};
                let length=q.dot(q).sqrt().max(1e-12);
                trs.r=Quat{x:q.x/length,y:q.y/length,z:q.z/length,w:q.w/length};
            }
            world=Mat4f::mul(&world,&node.matrix.unwrap_or_else(||trs_to_mat4(&trs)));
        }world
    }
}
impl RigidChannel{
    fn apply(&self,time:f32,trs:&mut NodeTrs){
        let hi=self.times.partition_point(|t|*t<=time);let lo=hi.saturating_sub(1);let hi=hi.min(self.times.len()-1);
        let f=if self.step||hi==lo{0.0}else{((time-self.times[lo])/(self.times[hi]-self.times[lo])).clamp(0.0,1.0)};
        let lanes=if self.path==1{4}else{3};let a=&self.values[lo*lanes..(lo+1)*lanes];let b=&self.values[hi*lanes..(hi+1)*lanes];
        if self.path==1{let mut b=[b[0],b[1],b[2],b[3]];if a.iter().zip(b).map(|(a,b)|a*b).sum::<f32>()<0.0{for v in &mut b{*v=-*v}}
            let mut q=[0.0;4];for i in 0..4{q[i]=a[i]+(b[i]-a[i])*f}let length=q.iter().map(|v|v*v).sum::<f32>().sqrt().max(1e-12);trs.r=Quat{x:q[0]/length,y:q[1]/length,z:q[2]/length,w:q[3]/length};
        }else{let v=vec3f(a[0]+(b[0]-a[0])*f,a[1]+(b[1]-a[1])*f,a[2]+(b[2]-a[2])*f);if self.path==0{trs.t=v}else{trs.s=v}}
    }
}
pub(crate) fn automatic_clips(json:&Val,acc:&Accessors,rests:&[NodeTrs],existing:&[usize])->Result<Vec<(usize,Arc<RigidHierarchy>)>,String>{node_clips(json,acc,rests,existing,&[])}
pub(crate) fn node_clips(json:&Val,acc:&Accessors,rests:&[NodeTrs],existing:&[usize],forced:&[usize])->Result<Vec<(usize,Arc<RigidHierarchy>)>,String>{
    let nodes=json.get("nodes").map(Val::arr).unwrap_or(&[]);let active=crate::asset_morph::active_nodes(json)?;let mut parents=vec![None;nodes.len()];
    for(i,n)in nodes.iter().enumerate(){for c in n.get("children").map(Val::arr).unwrap_or(&[]){let c=c.usize().ok_or("rigid child index")?;if c>=nodes.len()||parents[c].replace(i).is_some(){return Err("rigid parent graph".into())}}}
    let animations=json.get("animations").map(Val::arr).unwrap_or(&[]);if animations.len()>128{return Err("rigid clip count exceeds128".into())}
    let mut prepared=Vec::new();let mut bytes=0usize;
    for(index,animation)in animations.iter().enumerate(){
        let name=animation.get("name").and_then(Val::str).map(str::to_string).unwrap_or_else(||format!("clip_{index}"));
        let mut channels:Vec<Vec<RigidChannel>>=vec![Vec::new();nodes.len()];let mut times=Vec::new();
        for channel in animation.get("channels").map(Val::arr).unwrap_or(&[]){let Some(target)=channel.get("target")else{continue};let path=match target.get("path").and_then(Val::str){Some("translation")=>0,Some("rotation")=>1,Some("scale")=>2,_=>continue};let node=target.get("node").and_then(Val::usize).ok_or("rigid animation target")?;if node>=nodes.len(){return Err("rigid animation target out of range".into())}
            let sampler=channel.get("sampler").and_then(Val::usize).and_then(|s|animation.get("samplers")?.idx(s)).ok_or("rigid animation sampler")?;
            let (keys,lanes)=acc.read_f32(sampler.get("input").and_then(Val::usize).ok_or("rigid times")?)?;
            let (values,value_lanes)=acc.read_f32(sampler.get("output").and_then(Val::usize).ok_or("rigid values")?)?;
            if lanes!=1||keys.is_empty()||keys.len()>36_001||keys.iter().any(|v|!v.is_finite()||*v<0.0)||keys.windows(2).any(|w|w[0]>=w[1])||value_lanes!=if path==1{4}else{3}||values.len()!=keys.len()*value_lanes||values.iter().any(|v|!v.is_finite()){return Err("invalid rigid animation stream".into())}
            bytes+=(keys.len()+values.len())*4;if bytes>16*1024*1024{return Err("rigid animation exceeds16MiB".into())}
            let step=match sampler.get("interpolation").and_then(Val::str).unwrap_or("LINEAR"){"LINEAR"=>false,"STEP"=>true,_=>return Err("rigid cubic tracks require baked export".into())};
            times.extend(keys.iter().copied());channels[node].push(RigidChannel{path,times:keys,values,step});
        }
        times.sort_by(f32::total_cmp);times.dedup();if times.is_empty(){continue}
        prepared.push((name,times,channels.into_iter().map(Arc::new).collect::<Vec<_>>()));
    }
    let mut out=Vec::new();
    for (i,node) in nodes.iter().enumerate(){if !active[i]||(!forced.contains(&i)&&(node.get("mesh").is_none()||node.get("skin").is_some())){continue}
        let mut chain=vec![i];let mut parent=parents[i];while let Some(p)=parent{if chain.len()>nodes.len(){return Err("cyclic rigid hierarchy".into())}chain.push(p);parent=parents[p]}
        if chain.iter().any(|i|existing.contains(i))||prepared.iter().all(|(_,_,channels)|chain.iter().all(|i|channels[*i].is_empty())){continue}
        let mut variants=std::collections::BTreeMap::new();let mut first=None;
        for(name,times,channels)in &prepared{
            let mut hierarchy=Vec::new();for &j in chain.iter().rev(){let matrix=nodes[j].get("matrix").map(|m|Mat4f{v:std::array::from_fn(|k|m.idx(k).and_then(Val::f64).unwrap_or(0.0)as f32)});if matrix.is_some()&&!channels[j].is_empty(){return Err("animated node must use TRS".into())}hierarchy.push(RigidNode{rest:rests[j],matrix,channels:channels[j].clone()});}
            if first.is_none(){first=Some((name.clone(),times.clone(),hierarchy.clone()));}
            variants.insert(name.clone(),(*times.last().unwrap(),hierarchy));
        }
        let(name,times,nodes)=first.unwrap();out.push((i,Arc::new(RigidHierarchy{times,clip_name:name,nodes,clips:variants})));if out.len()>128{return Err("animated rigid mesh count exceeds128".into())}
    }Ok(out)
}

pub(crate) fn rest_nodes(json:&Val)->Vec<NodeTrs>{json.get("nodes").map(Val::arr).unwrap_or(&[]).iter().map(|node|{
    let component=|field:&str,i,default|node.get(field).and_then(|v|v.idx(i)).and_then(Val::f64).unwrap_or(default)as f32;
    NodeTrs{t:vec3f(component("translation",0,0.0),component("translation",1,0.0),component("translation",2,0.0)),s:vec3f(component("scale",0,1.0),component("scale",1,1.0),component("scale",2,1.0)),r:Quat{x:component("rotation",0,0.0),y:component("rotation",1,0.0),z:component("rotation",2,0.0),w:component("rotation",3,1.0)}}
}).collect()}
