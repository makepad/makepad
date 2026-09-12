//! Editable rests, FK poses, constraints, weight tools and shape keys. Controls
//! evaluate on the worker and bake into ordinary glTF animation channels.
use crate::{document::State,mesh,transform::*,schema::*,service::*,json::{self,Value},canon::{Reader,Writer},
    AnimationChannel,AnimationClip,AnimationPath,Keyframe,Skeleton,Error,Limits,Result,OperationResult};
use std::collections::{BTreeMap,BTreeSet};

#[derive(Clone,Debug,PartialEq)] pub struct Pose {pub name:String,pub joints:BTreeMap<u32,Transform>}
#[derive(Clone,Debug,PartialEq)] pub enum ConstraintKind {
    Copy{target:u32,translation:bool,rotation:bool,scale:bool,influence:f64},
    Aim{target:u32,axis:[f64;3],influence:f64},
    Limit{min_translation:[f64;3],max_translation:[f64;3],min_scale:[f64;3],max_scale:[f64;3],max_angle:f64},
    TwoBoneIk{middle:u32,end:u32,target:u32,pole:[f64;3],clamp_reach:bool},
}
#[derive(Clone,Debug,PartialEq)] pub struct RigConstraint{pub name:String,pub joint:u32,pub enabled:bool,pub kind:ConstraintKind}
#[derive(Clone,Debug,PartialEq)] pub struct MorphTarget {
    pub name:String,pub object:String,pub topology:[u8;32],pub deltas:BTreeMap<mesh::VertexId,[f64;3]>,pub weight:f64,
}
#[derive(Clone,Debug,PartialEq)] pub struct ClipEvent{pub time:f64,pub name:String,pub payload:String}
#[derive(Clone,Copy,Debug,PartialEq,Eq)] pub enum Interpolation{Linear,Step,Cubic}
#[derive(Clone,Debug,PartialEq)] pub struct MorphKey{pub time:f64,pub weight:f64}
#[derive(Clone,Debug,PartialEq)] pub struct ClipOptions{
    pub interpolation:Interpolation,pub root_motion:bool,pub events:Vec<ClipEvent>,pub morph_keys:BTreeMap<String,Vec<MorphKey>>,
}
impl Default for ClipOptions{fn default()->Self{Self{interpolation:Interpolation::Linear,root_motion:false,events:Vec::new(),morph_keys:BTreeMap::new()}}}
#[derive(Clone,Debug,Default,PartialEq)] pub struct RigState {
    pub rests:BTreeMap<u32,Transform>,pub poses:BTreeMap<String,Pose>,pub constraints:BTreeMap<String,RigConstraint>,
    pub morphs:BTreeMap<String,MorphTarget>,pub clip_options:BTreeMap<String,ClipOptions>,pub locked_groups:BTreeMap<String,BTreeSet<u32>>,
}
#[derive(Clone,Debug,PartialEq)] pub enum WeightEdit{Assign{joint:u32},Normalize{max_influences:usize},Smooth{iterations:u32,factor:f64},Mirror{axis:usize,joints:BTreeMap<u32,u32>,tolerance:f64},Transfer{source:String,max_distance:f64},Bind{max_influences:usize,power:f64}}
#[derive(Clone,Debug,PartialEq)] pub enum RigOperation {
    MirrorJoints {joints:Vec<u32>,axis:usize,names:BTreeMap<u32,String>},
    Rest{joint:u32,transform:Transform},RenameJoint{joint:u32,name:String},ReparentJoint{joint:u32,parent:Option<u32>},Roll{joint:u32,angle:f64},
    Pose(Pose),DeletePose{name:String},Constraint(RigConstraint),DeleteConstraint{name:String},
    SolvePose{pose:String},Ik{pose:String,root:u32,middle:u32,end:u32,target:[f64;3],pole:[f64;3],clamp_reach:bool},
    BakeClip{name:String,poses:Vec<(f64,String)>,fps:f64,solve_constraints:bool},
    BlendClip{name:String,a:String,b:String,weight:f64,fps:f64},
    RetargetClip{name:String,source:String,joints:BTreeMap<u32,u32>,translation_scale:f64},
    ClipOptions{name:String,options:ClipOptions},
    WeightLocks{object:String,joints:BTreeSet<u32>},Weights{object:String,vertices:Vec<mesh::VertexId>,edit:WeightEdit},
    Morph{name:String,object:String,deltas:BTreeMap<mesh::VertexId,[f64;3]>,weight:f64},
    CaptureMorph{name:String,object:String,target:String,weight:f64},DeleteMorph{name:String},MorphWeight{name:String,weight:f64},
}

pub(crate) fn topology(mesh:&mesh::Mesh)->[u8;32]{
    let mut bytes=Vec::with_capacity((mesh.vertices().len()+mesh.corners().len()*2)*8);
    for v in mesh.vertices(){bytes.extend_from_slice(&v.id.0.to_le_bytes());}
    for f in mesh.faces(){bytes.extend_from_slice(&f.id.0.to_le_bytes());bytes.extend_from_slice(&f.corner_count.to_le_bytes());}
    for c in mesh.corners(){bytes.extend_from_slice(&c.id.0.to_le_bytes());bytes.extend_from_slice(&c.vertex.0.to_le_bytes());}
    makepad_asset_data::sha256(&bytes)
}
impl RigState {
    pub fn is_empty(&self)->bool{self==&Self::default()}
    pub fn memory_bytes(&self)->usize {
        self.rests.len()*256+self.poses.values().map(|p|p.name.len()+p.joints.len()*256).sum::<usize>()+
        self.constraints.len()*1024+self.morphs.values().map(|m|m.name.len()+m.object.len()+m.deltas.len()*96+128).sum::<usize>()+
        self.clip_options.values().map(|o|o.events.iter().map(|e|e.name.len()+e.payload.len()+64).sum::<usize>()+o.morph_keys.values().map(|k|k.len()*32).sum::<usize>()+128).sum::<usize>()+
        self.locked_groups.values().map(|g|g.len()*64).sum::<usize>()
    }
    pub fn local_rest(&self,skeleton:&Skeleton,joint:usize)->Result<Transform>{
        let j=skeleton.joints.get(joint).ok_or(Error::Invalid("unknown rig joint"))?;
        Ok(self.rests.get(&(joint as u32)).copied().unwrap_or(Transform{translation:j.translation,..Default::default()}))
    }
    pub fn local_pose(&self,skeleton:&Skeleton,pose:Option<&Pose>)->Result<Vec<Transform>> {
        if pose.is_some_and(|p|p.joints.keys().any(|j|*j as usize>=skeleton.joints.len())){return Err(Error::Invalid("pose references unknown joint"));}
        (0..skeleton.joints.len()).map(|i|pose.and_then(|p|p.joints.get(&(i as u32))).copied().map(Ok).unwrap_or_else(||self.local_rest(skeleton,i))).collect()
    }
    pub fn global_rest(&self,skeleton:&Skeleton)->Result<Vec<Matrix4>> {globals(skeleton,&self.local_pose(skeleton,None)?).map(|v|v.0)}
    pub(crate) fn validate(&self,state:&State,limits:&Limits,ctx:&mut mesh::Context<'_>)->Result<()> {
        if self.memory_bytes()>limits.max_source_bytes||self.poses.len()>64||self.constraints.len()>128||self.morphs.len()>32||self.clip_options.len()>limits.max_clips||self.locked_groups.len()>limits.max_objects{return Err(Error::Budget("rig authoring state"));}
        let count=state.skeleton.as_ref().map_or(0,|s|s.joints.len());
        for (&joint,rest) in &self.rests {ctx.checkpoint(1)?;if joint as usize>=count{return Err(Error::Invalid("rest joint"));}rest.validate()?;}
        for pose in self.poses.values(){name(&pose.name,limits)?;if pose.joints.len()>count{return Err(Error::Budget("pose joints"));}
            for (&joint,transform) in &pose.joints{ctx.checkpoint(1)?;if joint as usize>=count{return Err(Error::Invalid("pose joint"));}transform.validate()?;}}
        for constraint in self.constraints.values(){name(&constraint.name,limits)?;if constraint.joint as usize>=count{return Err(Error::Invalid("constraint joint"));}
            constraint.validate(count)?;}
        self.constraint_order(state.skeleton.as_ref())?;
        if let Some(skeleton)=&state.skeleton{self.global_rest(skeleton)?;}
        for morph in self.morphs.values(){name(&morph.name,limits)?;let mesh=state.objects.get(&morph.object).ok_or_else(||Error::MissingObject(morph.object.clone()))?;
            if morph.topology!=topology(mesh){return Err(Error::Invalid("morph topology changed; remove or rebuild its shape key explicitly"));}
            valid_weight(morph.weight)?;if morph.deltas.len()>mesh.vertices().len(){return Err(Error::Budget("morph deltas"));}
            for (id,delta) in &morph.deltas{ctx.checkpoint(1)?;if mesh.vertex(*id).is_none()||delta.iter().any(|v|!v.is_finite()||!(*v as f32).is_finite()){return Err(Error::Invalid("morph delta vertex/value"));}}
        }
        for (clip,options) in &self.clip_options {
            let clip=state.clips.get(clip).ok_or(Error::Invalid("clip options target"))?;let duration=clip.duration();
            if options.events.len()>1024||options.morph_keys.len()>32{return Err(Error::Budget("clip metadata"));}
            let mut previous=0.;for e in &options.events{name(&e.name,limits)?;if !e.time.is_finite()||e.time<previous||e.time>duration||e.payload.len()>4096{return Err(Error::Invalid("clip event"));}previous=e.time;}
            let mut total=0;for (name,keys) in &options.morph_keys{if !self.morphs.contains_key(name){return Err(Error::Invalid("morph animation target"));}
                if keys.is_empty(){return Err(Error::Invalid("empty morph keys"));}let mut previous=-1.;for k in keys{ctx.checkpoint(1)?;valid_weight(k.weight)?;
                    if !k.time.is_finite()||k.time<0.||k.time as f32<=previous as f32||k.time>duration{return Err(Error::Invalid("morph key time"));}previous=k.time;total+=1;}}
            if total>limits.max_keyframes{return Err(Error::Budget("morph keyframes"));}
        }
        for (object,joints) in &self.locked_groups{if !state.objects.contains_key(object)||joints.iter().any(|j|*j as usize>=count){return Err(Error::Invalid("locked weight group"));}}
        Ok(())
    }
    fn constraint_order(&self,skeleton:Option<&Skeleton>)->Result<Vec<&RigConstraint>> {
        let active=self.constraints.values().filter(|c|c.enabled).collect::<Vec<_>>();
        if active.is_empty(){return Ok(Vec::new());}
        let skeleton=skeleton.ok_or(Error::Invalid("constraints require skeleton"))?;
        let count=skeleton.joints.len();let mut writers=BTreeMap::new();let mut reads=Vec::new();
        for (index,c) in active.iter().enumerate(){
            c.validate(count)?;let mut written=vec![c.joint];
            if let ConstraintKind::TwoBoneIk{middle,end,..}=c.kind {
                if skeleton.joints[middle as usize].parent!=Some(c.joint)||skeleton.joints[end as usize].parent!=Some(middle){return Err(Error::Invalid("IK constraint requires root-middle-end chain"));}
                written.push(middle);
            }
            for joint in written {if writers.insert(joint,index).is_some(){return Err(Error::Invalid("overlapping constraint joint writers"));}}
        }
        let ancestors=|joint:u32|->Result<BTreeSet<u32>>{let mut result=BTreeSet::new();let mut next=Some(joint);while let Some(j)=next {
            if j as usize>=count||!result.insert(j){return Err(Error::Invalid("constraint parent cycle/index"));}next=skeleton.joints[j as usize].parent;
        }Ok(result)};
        for (index,c) in active.iter().enumerate(){let mut dependencies=BTreeSet::new();match c.kind {
            ConstraintKind::Copy{target,..}=>{dependencies.insert(target);}
            ConstraintKind::Aim{target,..}|ConstraintKind::TwoBoneIk{target,..}=>{
                let target_reads=ancestors(target)?;
                if target_reads.iter().any(|j|writers.get(j)==Some(&index)){return Err(Error::Invalid("constraint target depends on its own output"));}
                dependencies.extend(target_reads);dependencies.extend(ancestors(c.joint)?);
                if let ConstraintKind::TwoBoneIk{middle,end,..}=c.kind{dependencies.insert(middle);dependencies.insert(end);}
            }
            ConstraintKind::Limit{..}=>{}
        }
        reads.push(dependencies.into_iter().filter_map(|j|writers.get(&j).copied()).filter(|i|*i!=index).collect::<BTreeSet<_>>());}
        let mut order=Vec::new();let mut done=BTreeSet::new();while order.len()<active.len(){let before=order.len();for(index,c)in active.iter().enumerate(){if !done.contains(&index)&&reads[index].is_subset(&done){order.push(*c);done.insert(index);}}if order.len()==before{return Err(Error::Invalid("constraint dependency cycle"));}}Ok(order)
    }
    pub fn solve(&self,skeleton:&Skeleton,pose:&Pose)->Result<Pose>{
        validate_solver(skeleton,&self.local_pose(skeleton,Some(pose))?)?;
        let mut local=self.local_pose(skeleton,Some(pose))?;
        for c in self.constraint_order(Some(skeleton))?{
            let j=c.joint as usize;match c.kind {
                ConstraintKind::Copy{target,translation,rotation,scale,influence}=>{
                    let source=local[target as usize];let mut target=local[j];if translation{target.translation=source.translation;}if rotation{target.rotation=source.rotation;}if scale{target.scale=source.scale;}
                    local[j]=local[j].interpolate(target,influence);
                }
                ConstraintKind::Aim{target,axis,influence}=>{
                    let (world,rot)=globals(skeleton,&local)?;let direction=sub(transform_point(world[target as usize],[0.;3]),transform_point(world[j],[0.;3]));
                    let delta=quat_from_to(quat_rotate(rot[j],axis),direction)?;let parent=skeleton.joints[j].parent.map(|p|rot[p as usize]).unwrap_or([0.,0.,0.,1.]);
                    let desired=quat_mul(quat_inverse(parent),quat_mul(delta,rot[j]));local[j].rotation=quat_slerp(local[j].rotation,desired,influence);
                }
                ConstraintKind::Limit{min_translation,max_translation,min_scale,max_scale,max_angle}=>{
                    for d in 0..3{local[j].translation[d]=local[j].translation[d].clamp(min_translation[d],max_translation[d]);local[j].scale[d]=local[j].scale[d].clamp(min_scale[d],max_scale[d]);}
                    let rest=self.local_rest(skeleton,j)?.rotation;let relative=quat_mul(quat_inverse(rest),local[j].rotation);
                    let angle=2.*relative[3].abs().clamp(0.,1.).acos();if angle>max_angle{local[j].rotation=quat_slerp(rest,local[j].rotation,max_angle/angle);}
                }
                ConstraintKind::TwoBoneIk{middle,end,target,pole,clamp_reach}=>{
                    let (world,_)=globals(skeleton,&local)?;let target=transform_point(world[target as usize],[0.;3]);
                    solve_ik(skeleton,&mut local,j,middle as usize,end as usize,target,pole,clamp_reach)?;
                }
            }
        }
        Ok(Pose{name:pose.name.clone(),joints:local.into_iter().enumerate().map(|(i,t)|(i as u32,t)).collect()})
    }
}
impl RigConstraint {
    fn validate(&self,count:usize)->Result<()> {
        if self.joint as usize>=count{return Err(Error::Invalid("constraint joint"));}
        match self.kind {
            ConstraintKind::Copy{target,influence,..}|ConstraintKind::Aim{target,influence,..}=>{if target as usize>=count||target==self.joint||!influence.is_finite()||!(0.0..=1.0).contains(&influence){return Err(Error::Invalid("constraint target/influence"));}}
            ConstraintKind::Limit{min_translation,max_translation,min_scale,max_scale,max_angle}=>{
                if !max_angle.is_finite()||!(0.0..=std::f64::consts::PI).contains(&max_angle)||(0..3).any(|i|!min_translation[i].is_finite()||!max_translation[i].is_finite()||min_translation[i]>max_translation[i]||!min_scale[i].is_finite()||!max_scale[i].is_finite()||min_scale[i]<=0.||min_scale[i]>max_scale[i]){return Err(Error::Invalid("constraint limits"));}}
            ConstraintKind::TwoBoneIk{middle,end,target,pole,..}=>{if [middle,end,target].iter().any(|j|*j as usize>=count)||pole.iter().any(|v|!v.is_finite()){return Err(Error::Invalid("IK constraint target"));}}
        }
        if let ConstraintKind::Aim{axis,..}=self.kind{normalized(axis)?;}
        Ok(())
    }
}
pub(crate) fn globals(skeleton:&Skeleton,local:&[Transform])->Result<(Vec<Matrix4>,Vec<Quaternion>)>{
    if local.len()!=skeleton.joints.len(){return Err(Error::Invalid("pose size"));}
    let mut world=Vec::with_capacity(local.len());let mut rotations=Vec::with_capacity(local.len());
    for (i,t) in local.iter().enumerate(){let m=t.matrix()?;let q=t.rotation;
        if let Some(parent)=skeleton.joints[i].parent {if parent as usize>=i{return Err(Error::Invalid("rig parent order"));}world.push(matrix_mul(world[parent as usize],m));rotations.push(quat_normalize(quat_mul(rotations[parent as usize],q))?);}
        else{world.push(m);rotations.push(q);}}
    Ok((world,rotations))
}
fn validate_solver(skeleton:&Skeleton,local:&[Transform])->Result<()> {
    if skeleton.joints.is_empty()||skeleton.joints.len()>128||local.len()!=skeleton.joints.len(){return Err(Error::Invalid("solver skeleton/pose size"));}
    for(i,joint)in skeleton.joints.iter().enumerate(){if joint.parent.is_some_and(|p|p as usize>=i)||joint.translation.iter().any(|v|!v.is_finite()){return Err(Error::Invalid("solver parent hierarchy/rest"));}local[i].validate()?;}Ok(())
}
/// Atomic public IK: invalid hierarchy/pose, unreachable or degenerate inputs
/// leave the caller's pose unchanged, including failures after root evaluation.
pub fn solve_ik(skeleton:&Skeleton,local:&mut [Transform],root:usize,middle:usize,end:usize,target:[f64;3],pole:[f64;3],clamp_reach:bool)->Result<()> {
    validate_solver(skeleton,local)?;
    if [root,middle,end].iter().any(|i|*i>=local.len())||root==middle||middle==end||root==end{return Err(Error::Invalid("IK chain indices"));}
    let mut next=local.to_vec();solve_ik_inner(skeleton,&mut next,root,middle,end,target,pole,clamp_reach)?;validate_solver(skeleton,&next)?;local.copy_from_slice(&next);Ok(())
}
fn solve_ik_inner(skeleton:&Skeleton,local:&mut [Transform],root:usize,middle:usize,end:usize,target:[f64;3],pole:[f64;3],clamp_reach:bool)->Result<()> {
    if skeleton.joints.get(middle).and_then(|j|j.parent)!=Some(root as u32)||skeleton.joints.get(end).and_then(|j|j.parent)!=Some(middle as u32)||target.iter().chain(&pole).any(|v|!v.is_finite()){return Err(Error::Invalid("IK requires root-middle-end chain"));}
    // Analytic spherical-joint IK assumes uniform positive ancestor scales.
    let mut ancestor=Some(end);while let Some(j)=ancestor{let s=local.get(j).ok_or(Error::Invalid("IK pose"))?.scale;if s[0]<=0.||(s[0]-s[1]).abs()>1e-8||(s[1]-s[2]).abs()>1e-8{return Err(Error::Invalid("analytic IK requires uniform positive chain scales"));}ancestor=skeleton.joints[j].parent.map(|p|p as usize);}
    let (world,rot)=globals(skeleton,local)?;let p=transform_point(world[root],[0.;3]);let m=transform_point(world[middle],[0.;3]);let e=transform_point(world[end],[0.;3]);
    let a=length(sub(m,p));let b=length(sub(e,m));if a<1e-8||b<1e-8{return Err(Error::Invalid("IK zero-length bone"));}
    let direction=normalized(sub(target,p))?;let distance=length(sub(target,p));let low=(a-b).abs()+1e-8;let high=a+b-1e-8;if low>=high{return Err(Error::Invalid("IK chain is too short for stable reach calculation"));}
    if !clamp_reach&&(distance<low||distance>high){return Err(Error::Invalid("IK target outside reachable chain"));}
    let distance=distance.clamp(low,high);let target=add(p,mul(direction,distance));
    let plane=normalized(sub(sub(pole,p),mul(direction,dot(sub(pole,p),direction))))?;
    let along=(a*a+distance*distance-b*b)/(2.*distance);let height=(a*a-along*along).max(0.).sqrt();let desired_mid=add(p,add(mul(direction,along),mul(plane,height)));
    let parent=skeleton.joints[root].parent.map(|p|rot[p as usize]).unwrap_or([0.,0.,0.,1.]);
    let turn=quat_from_to(sub(m,p),sub(desired_mid,p))?;local[root].rotation=quat_normalize(quat_mul(quat_inverse(parent),quat_mul(turn,rot[root])))?;
    let (world,rot)=globals(skeleton,local)?;let m=transform_point(world[middle],[0.;3]);let e=transform_point(world[end],[0.;3]);
    let turn=quat_from_to(sub(e,m),sub(target,m))?;local[middle].rotation=quat_normalize(quat_mul(quat_inverse(rot[root]),quat_mul(turn,rot[middle])))?;Ok(())
}
fn valid_weight(v:f64)->Result<()> {if v.is_finite()&&(-2.0..=2.0).contains(&v){Ok(())}else{Err(Error::Invalid("morph weight range"))}}

impl RigState {
    pub(crate) fn apply(op:&RigOperation,state:&mut State,limits:&Limits,ctx:&mut mesh::Context<'_>)->Result<OperationResult>{
        ctx.checkpoint(1)?;let mut result=OperationResult::default();
        match op {
            RigOperation::MirrorJoints{joints,axis,names}=>mirror::apply(state,joints,*axis,names,limits,ctx)?,
            RigOperation::Rest{joint,transform}=>{transform.validate()?;state.rig.rests.insert(*joint,*transform);}
            RigOperation::RenameJoint{joint,name:new_name}=>{name(new_name,limits)?;state.skeleton.as_mut().and_then(|s|s.joints.get_mut(*joint as usize)).ok_or(Error::Invalid("rename joint"))?.name=new_name.clone();}
            RigOperation::ReparentJoint{joint,parent}=>{state.skeleton.as_mut().and_then(|s|s.joints.get_mut(*joint as usize)).ok_or(Error::Invalid("reparent joint"))?.parent=*parent;}
            RigOperation::Roll{joint,angle}=>{
                let s=state.skeleton.as_ref().ok_or(Error::Invalid("roll requires skeleton"))?;let mut t=state.rig.local_rest(s,*joint as usize)?;
                t.rotation=quat_normalize(quat_mul(t.rotation,quat_axis_angle([0.,1.,0.],*angle)?))?;state.rig.rests.insert(*joint,t);
            }
            RigOperation::Pose(pose)=>{name(&pose.name,limits)?;state.rig.poses.insert(pose.name.clone(),pose.clone());}
            RigOperation::DeletePose{name}=>{if state.rig.poses.remove(name).is_none(){return Err(Error::Invalid("unknown pose"));}}
            RigOperation::Constraint(c)=>{name(&c.name,limits)?;state.rig.constraints.insert(c.name.clone(),c.clone());}
            RigOperation::DeleteConstraint{name}=>{if state.rig.constraints.remove(name).is_none(){return Err(Error::Invalid("unknown constraint"));}}
            RigOperation::SolvePose{pose}=>{
                let s=state.skeleton.as_ref().ok_or(Error::Invalid("pose requires skeleton"))?;
                let solved=state.rig.solve(s,state.rig.poses.get(pose).ok_or(Error::Invalid("unknown pose"))?)?;state.rig.poses.insert(pose.clone(),solved);
            }
            RigOperation::Ik{pose,root,middle,end,target,pole,clamp_reach}=>{
                let s=state.skeleton.as_ref().ok_or(Error::Invalid("IK requires skeleton"))?;let original=state.rig.poses.get(pose).ok_or(Error::Invalid("unknown pose"))?;
                let mut local=state.rig.local_pose(s,Some(original))?;solve_ik(s,&mut local,*root as usize,*middle as usize,*end as usize,*target,*pole,*clamp_reach)?;
                state.rig.poses.insert(pose.clone(),Pose{name:pose.clone(),joints:local.into_iter().enumerate().map(|(i,t)|(i as u32,t)).collect()});
            }
            RigOperation::BakeClip{name,poses,fps,solve_constraints}=>{
                let s=state.skeleton.as_ref().ok_or(Error::Invalid("bake requires skeleton"))?;name_check_keys(name,poses,*fps,limits)?;
                let mut inputs=Vec::new();for (time,key) in poses{ctx.checkpoint(1)?;let pose=state.rig.poses.get(key).ok_or(Error::Invalid("bake pose missing"))?;
                    inputs.push((*time,state.rig.local_pose(s,Some(pose))?));}
                let count=frame_count(poses.last().unwrap().0,*fps,s.joints.len(),limits)?;let mut samples=Vec::new();
                for i in 0..count{ctx.checkpoint(s.joints.len() as u64)?;let time=i as f64/(count-1) as f64*poses.last().unwrap().0;
                    let hi=inputs.partition_point(|(t,_)|*t<=time).min(inputs.len()-1);let lo=hi.saturating_sub(1);
                    let t=if inputs[hi].0>inputs[lo].0{((time-inputs[lo].0)/(inputs[hi].0-inputs[lo].0)).clamp(0.,1.)}else{0.};
                    let mut local=inputs[lo].1.iter().zip(&inputs[hi].1).map(|(a,b)|a.interpolate(*b,t)).collect::<Vec<_>>();
                    if *solve_constraints{let pose=Pose{name:"bake".into(),joints:local.iter().copied().enumerate().map(|(i,t)|(i as u32,t)).collect()};local=state.rig.local_pose(s,Some(&state.rig.solve(s,&pose)?))?;}
                    samples.push((time,local));
                }
                let clip=clip_from_samples(name,&samples);clip.validate(s,limits,ctx)?;state.clips.insert(name.clone(),clip);
            }
            RigOperation::BlendClip{name,a,b,weight,fps}=>{
                if !weight.is_finite()||!(0.0..=1.0).contains(weight){return Err(Error::Invalid("clip blend weight"));}
                let skeleton=state.skeleton.as_ref().ok_or(Error::Invalid("blend requires skeleton"))?;let a=state.clips.get(a).ok_or(Error::Invalid("blend source clip"))?;let b=state.clips.get(b).ok_or(Error::Invalid("blend source clip"))?;
                let duration=a.duration().max(b.duration());let count=frame_count(duration,*fps,skeleton.joints.len(),limits)?;let mut samples=Vec::new();
                for i in 0..count{ctx.checkpoint(skeleton.joints.len() as u64)?;let time=i as f64/(count-1) as f64*duration;
                    let left=state.rig.sample_clip(skeleton,a,time*a.duration()/duration)?;let right=state.rig.sample_clip(skeleton,b,time*b.duration()/duration)?;
                    samples.push((time,left.iter().zip(right).map(|(a,b)|a.interpolate(b,*weight)).collect()));}
                let clip=clip_from_samples(name,&samples);clip.validate(skeleton,limits,ctx)?;state.clips.insert(name.clone(),clip);
            }
            RigOperation::RetargetClip{name,source,joints,translation_scale}=>{
                if !translation_scale.is_finite()||*translation_scale<=0.{return Err(Error::Invalid("retarget translation scale"));}
                let s=state.skeleton.as_ref().ok_or(Error::Invalid("retarget requires skeleton"))?;let source=state.clips.get(source).ok_or(Error::Invalid("retarget source clip"))?;
                let mut clip=source.clone();clip.name=name.clone();for channel in &mut clip.channels {
                    channel.joint=*joints.get(&channel.joint).ok_or(Error::Invalid("retarget requires mapping every animated joint"))?;
                    if channel.path==AnimationPath::Translation{for key in &mut channel.keys{for v in &mut key.value[..3]{*v*=translation_scale;}}}
                }clip.validate(s,limits,ctx)?;state.clips.insert(name.clone(),clip);
            }
            RigOperation::ClipOptions{name,options}=>{state.rig.clip_options.insert(name.clone(),options.clone());}
            RigOperation::WeightLocks{object,joints}=>{state.rig.locked_groups.insert(object.clone(),joints.clone());}
            RigOperation::Weights{object,vertices,edit}=>{
                let changes=edit_weights(state,object,vertices,edit,limits,ctx)?;result.object=object.clone();result.vertices=changes;
            }
            RigOperation::Morph{name:target_name,object,deltas,weight}=>{
                name(target_name,limits)?;valid_weight(*weight)?;let m=state.objects.get(object).ok_or_else(||Error::MissingObject(object.clone()))?;
                state.rig.morphs.insert(target_name.clone(),MorphTarget{name:target_name.clone(),object:object.clone(),topology:topology(m),deltas:deltas.clone(),weight:*weight});
            }
            RigOperation::CaptureMorph{name:target_name,object,target,weight}=>{
                let a=state.objects.get(object).ok_or_else(||Error::MissingObject(object.clone()))?;let b=state.objects.get(target).ok_or_else(||Error::MissingObject(target.clone()))?;
                if topology(a)!=topology(b){return Err(Error::Invalid("capture shape key requires matching topology and element IDs"));}
                let deltas=a.vertices().iter().zip(b.vertices()).map(|(a,b)|(a.id,sub(b.position,a.position))).collect();
                state.rig.morphs.insert(target_name.clone(),MorphTarget{name:target_name.clone(),object:object.clone(),topology:topology(a),deltas,weight:*weight});
            }
            RigOperation::DeleteMorph{name}=>{if state.rig.morphs.remove(name).is_none(){return Err(Error::Invalid("unknown shape key"));}for options in state.rig.clip_options.values_mut(){options.morph_keys.remove(name);}}
            RigOperation::MorphWeight{name,weight}=>{valid_weight(*weight)?;state.rig.morphs.get_mut(name).ok_or(Error::Invalid("unknown shape key"))?.weight=*weight;}
        }
        Ok(result)
    }
    pub fn sample_clip(&self,skeleton:&Skeleton,clip:&AnimationClip,time:f64)->Result<Vec<Transform>>{
        let mut local=self.local_pose(skeleton,None)?;let mode=self.clip_options.get(&clip.name).map_or(Interpolation::Linear,|o|o.interpolation);
        for channel in &clip.channels{let keys=&channel.keys;if keys.is_empty(){return Err(Error::Invalid("empty animation channel"));}
            let hi=keys.partition_point(|k|k.time<=time).min(keys.len()-1);let lo=hi.saturating_sub(1);
            let mut t=if keys[hi].time>keys[lo].time{((time-keys[lo].time)/(keys[hi].time-keys[lo].time)).clamp(0.,1.)}else{0.};
            if time>=keys.last().unwrap().time{t=1.;}if mode==Interpolation::Step&&t<1.{t=0.;}if mode==Interpolation::Cubic{t=t*t*(3.-2.*t);}
            let value=if channel.path==AnimationPath::Rotation{quat_slerp(keys[lo].value,keys[hi].value,t)}else{std::array::from_fn(|i|keys[lo].value[i]+(keys[hi].value[i]-keys[lo].value[i])*t)};
            let target=local.get_mut(channel.joint as usize).ok_or(Error::Invalid("clip joint"))?;match channel.path{AnimationPath::Translation=>target.translation=value[..3].try_into().unwrap(),AnimationPath::Rotation=>target.rotation=value,AnimationPath::Scale=>target.scale=value[..3].try_into().unwrap()}
        }Ok(local)
    }
}
fn frame_count(duration:f64,fps:f64,joints:usize,limits:&Limits)->Result<usize>{
    if !duration.is_finite()||duration<=0.||duration>limits.max_clip_duration||!fps.is_finite()||!(1.0..=120.0).contains(&fps){return Err(Error::Invalid("animation bake duration/fps"));}
    let count=(duration*fps).ceil() as usize+1;if count.saturating_mul(joints).saturating_mul(3)>limits.max_keyframes{return Err(Error::Budget("baked animation keys"));}Ok(count)
}
fn name_check_keys(name_:&str,poses:&[(f64,String)],fps:f64,limits:&Limits)->Result<()>{
    name(name_,limits)?;if poses.len()<2||poses.len()>limits.max_keyframes||poses[0].0!=0.{return Err(Error::Invalid("pose keys start at zero and require two keys"));}
    for pair in poses.windows(2){if !pair[1].0.is_finite()||pair[1].0<=pair[0].0{return Err(Error::Invalid("pose key order"));}}
    frame_count(poses.last().unwrap().0,fps,1,limits)?;Ok(())
}
fn clip_from_samples(name:&str,samples:&[(f64,Vec<Transform>)])->AnimationClip{
    let mut channels=Vec::new();for joint in 0..samples[0].1.len(){for path in [AnimationPath::Translation,AnimationPath::Rotation,AnimationPath::Scale]{
        channels.push(AnimationChannel{joint:joint as u32,path,keys:samples.iter().map(|(time,pose)|{let t=pose[joint];let value=match path{
            AnimationPath::Translation=>[t.translation[0],t.translation[1],t.translation[2],0.],AnimationPath::Rotation=>t.rotation,
            AnimationPath::Scale=>[t.scale[0],t.scale[1],t.scale[2],0.]};Keyframe{time:*time,value}}).collect()});}}
    AnimationClip{name:name.into(),channels}
}
fn normalized_weights(mut proposed:BTreeMap<u32,f64>,old:&[mesh::JointWeight],locks:&BTreeSet<u32>,max:usize)->Result<Vec<mesh::JointWeight>>{
    if max==0||max>64{return Err(Error::Invalid("weight influence limit"));}
    let fixed=old.iter().filter(|w|locks.contains(&w.joint)).cloned().collect::<Vec<_>>();let fixed_sum=fixed.iter().map(|w|w.weight).sum::<f64>();
    if fixed.len()>max||fixed_sum>1.+1e-8{return Err(Error::Invalid("locked weights exceed influence budget"));}
    proposed.retain(|joint,value|!locks.contains(joint)&&value.is_finite()&&*value>0.);
    let mut free=proposed.into_iter().collect::<Vec<_>>();free.sort_by(|a,b|b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));free.truncate(max-fixed.len());
    let sum=free.iter().map(|(_,w)|w).sum::<f64>();let remainder=(1.-fixed_sum).max(0.);if remainder>1e-8&&sum<=0.{return Err(Error::Invalid("no unlocked weight mass"));}
    let mut out=fixed;for (joint,weight) in free{if remainder>1e-12{out.push(mesh::JointWeight{joint,weight:weight/sum*remainder});}}
    out.sort_by_key(|w|w.joint);Ok(out)
}
fn edit_weights(state:&mut State,object:&str,vertices:&[mesh::VertexId],edit:&WeightEdit,limits:&Limits,ctx:&mut mesh::Context<'_>)->Result<Vec<mesh::VertexId>>{
    let skeleton=state.skeleton.as_ref().ok_or(Error::Invalid("weight edit requires skeleton"))?;
    if let WeightEdit::Assign{joint}=edit{if *joint as usize>=skeleton.joints.len(){return Err(Error::Invalid("weight assignment target joint"));}}
    let mesh=state.objects.get(object).ok_or_else(||Error::MissingObject(object.into()))?;let ids=if vertices.is_empty(){mesh.vertices().iter().map(|v|v.id).collect::<Vec<_>>()}else{vertices.to_vec()};
    let selected=ids.iter().copied().collect::<BTreeSet<_>>();if selected.len()!=ids.len()||ids.iter().any(|id|mesh.vertex(*id).is_none()){return Err(Error::Invalid("weight vertices"));}
    let locks=state.rig.locked_groups.get(object).cloned().unwrap_or_default();let mut rows=mesh.vertices().iter().map(|v|(v.id,v.weights.clone())).collect::<BTreeMap<_,_>>();
    let count=match edit{WeightEdit::Smooth{iterations,factor}=>{if *iterations==0||*iterations>64||!factor.is_finite()||!(0.0..=1.0).contains(factor){return Err(Error::Invalid("weight smoothing"));}*iterations},_=>1};
    let adjacency=if matches!(edit,WeightEdit::Smooth{..}){Some(mesh.adjacency(ctx)?)}else{None};
    let global=state.rig.global_rest(skeleton)?;
    for _ in 0..count{let mut next=rows.clone();for id in &ids{ctx.checkpoint(1)?;let vertex=mesh.vertex(*id).unwrap();let mut proposed=BTreeMap::new();let mut max=limits.mesh.max_weights_per_vertex.min(64);
        match edit{
            WeightEdit::Assign{joint}=>{if rows[id].iter().any(|w|w.joint!=*joint&&w.weight>0.&&locks.contains(&w.joint)){return Err(Error::Invalid("rigid weight assignment conflicts with locked influence"));}max=1;proposed.insert(*joint,1.);}
            WeightEdit::Normalize{max_influences}=>{max=*max_influences;for w in &rows[id]{*proposed.entry(w.joint).or_insert(0.)+=w.weight;}}
            WeightEdit::Smooth{factor,..}=>{
                for w in &rows[id]{*proposed.entry(w.joint).or_insert(0.)+=w.weight*(1.-factor);}
                let neighbors=adjacency.as_ref().unwrap().vertex_edges.get(id).map(Vec::as_slice).unwrap_or(&[]);
                if neighbors.is_empty(){proposed=rows[id].iter().map(|w|(w.joint,w.weight)).collect();}
                else{for edge in neighbors{let other=if edge.0==*id{edge.1}else{edge.0};for w in &rows[&other]{ctx.checkpoint(1)?;*proposed.entry(w.joint).or_insert(0.)+=w.weight*factor/neighbors.len() as f64;}}}
            }
            WeightEdit::Mirror{axis,joints,tolerance}=>{
                if *axis>2||!tolerance.is_finite()||*tolerance<0.{return Err(Error::Invalid("weight mirror axis/tolerance"));}let mut p=vertex.position;p[*axis]= -p[*axis];
                let mut best=None;for other in mesh.vertices(){ctx.checkpoint(1)?;let d=length(sub(other.position,p));if best.is_none_or(|(dist,_)|d<dist){best=Some((d,other));}}
                let (distance,source)=best.ok_or(Error::Invalid("weight mirror source"))?;if distance>*tolerance{return Err(Error::Invalid("mirror has no vertex within tolerance"));}
                for w in &rows[&source.id]{let joint=*joints.get(&w.joint).unwrap_or(&w.joint);*proposed.entry(joint).or_insert(0.)+=w.weight;}
            }
            WeightEdit::Transfer{source,max_distance}=>{
                if !max_distance.is_finite()||*max_distance<0.{return Err(Error::Invalid("weight transfer distance"));}
                let source=state.objects.get(source).ok_or_else(||Error::MissingObject(source.clone()))?;let mut best=None;
                for other in source.vertices(){ctx.checkpoint(1)?;let d=length(sub(other.position,vertex.position));if best.is_none_or(|(dist,_)|d<dist){best=Some((d,other));}}
                let (distance,source)=best.ok_or(Error::Invalid("empty weight transfer source"))?;if distance>*max_distance{return Err(Error::Invalid("weight transfer unmatched vertex"));}
                for w in &source.weights{proposed.insert(w.joint,w.weight);}
            }
            WeightEdit::Bind{max_influences,power}=>{
                if !power.is_finite()||*power<=0.||*power>8.{return Err(Error::Invalid("binding power"));}max=*max_influences;
                for (j,bone) in skeleton.joints.iter().enumerate(){ctx.checkpoint(1)?;let b=transform_point(global[j],[0.;3]);let a=bone.parent.map(|p|transform_point(global[p as usize],[0.;3])).unwrap_or(b);
                    let ab=sub(b,a);let t=if dot(ab,ab)>1e-20{(dot(sub(vertex.position,a),ab)/dot(ab,ab)).clamp(0.,1.)}else{0.};let distance=length(sub(vertex.position,add(a,mul(ab,t))));
                    proposed.insert(j as u32,1./distance.max(1e-6).powf(*power));}
            }
        }
        if proposed.keys().any(|j|*j as usize>=skeleton.joints.len()){return Err(Error::Invalid("weight edit target joint"));}
        next.insert(*id,normalized_weights(proposed,&rows[id],&locks,max)?);
    }rows=next;}
    let updates=ids.iter().map(|id|(*id,rows.remove(id).unwrap())).collect::<Vec<_>>();state.objects.get_mut(object).unwrap().set_weights_bulk(&updates,ctx)?;Ok(ids)
}

#[path="rig_control_parse.rs"] mod parse;
#[path="rig_control_codec.rs"] mod codec;

#[path="rig_control_mirror.rs"] mod mirror;
