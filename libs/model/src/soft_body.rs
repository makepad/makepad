//! Transactional worker-side cage binding. Geometry stays authoritative;
//! affine palette weights and immutable runtime metadata are derived once.
use crate::{canon::{Reader,Writer},document::State,mesh,schema,transform::*,Error,Limits,Result,Joint,Skeleton,OperationResult};
use makepad_soft_body as physics;
pub use makepad_gltf::{SoftBodyMetadata,SoftBodySettings,SoftBodyBinding,SoftBodyAttachment};
use makepad_gltf::{SOFT_BODY_TETRAHEDRA,SOFT_BODY_PARTICLES};
use crate::mesh_editing::codec::*;
use std::collections::BTreeSet;

#[derive(Clone,Debug,PartialEq)]
pub struct SoftBodyAttachmentRequest {
    pub name:String,
    pub objects:Vec<String>,
    /// Model-space pivot. Omitted uses the group's world-space bounds center.
    pub pivot:Option<[f64;3]>,
}
#[derive(Clone,Debug,PartialEq)]
pub struct SoftBodyBind {
    pub object:String,
    pub deform_objects:Vec<String>,
    pub attachments:Vec<SoftBodyAttachmentRequest>,
    pub settings:SoftBodySettings,
}

pub(crate) fn core_settings(p:SoftBodySettings)->physics::SoftBodySettings{physics::SoftBodySettings {
    mass:p.mass,edge_compliance:p.edge_compliance,volume_compliance:p.volume_compliance,pose_compliance:p.pose_compliance,
    damping:p.damping,gravity:p.gravity,contact_radius:p.contact_radius,friction:p.friction,substeps:p.substeps,
    iterations:p.iterations,max_speed:p.max_speed,max_displacement:p.max_displacement,
}}
fn binding(b:physics::SoftBodyBinding)->SoftBodyBinding{SoftBodyBinding{tetrahedron:b.tetrahedron,weights:b.weights}}
fn core_binding(b:SoftBodyBinding)->physics::SoftBodyBinding{physics::SoftBodyBinding{tetrahedron:b.tetrahedron,weights:b.weights}}
pub(crate) fn definition(m:&SoftBodyMetadata)->physics::SoftBodyDefinition{physics::SoftBodyDefinition{
    rest_positions:m.rest_positions.clone(),tetrahedra:m.tetrahedra.clone(),surface_samples:m.surface_samples.iter().copied().map(core_binding).collect(),anchors:m.anchors.clone(),
}}
fn point32(p:[f64;3])->Result<[f32;3]>{let p=p.map(|v|v as f32);if p.iter().any(|v|!v.is_finite()||v.abs()>1.0e6){return Err(Error::Invalid("soft-body position exceeds render precision"));}Ok(p)}

impl SoftBodyBind {
    pub(crate) fn memory_bytes(&self)->usize{let mut w=Writer::new(usize::MAX);if self.write(&mut w).is_err(){return usize::MAX;}w.bytes.len().saturating_mul(3)}
    pub(crate) fn apply(&self,state:&mut State,limits:&Limits,ctx:&mut mesh::Context<'_>)->Result<OperationResult>{
        self.check(limits)?;
        if state.soft_body.is_some(){return Err(Error::Invalid("soft body already bound; use soft_body_unbind before editing and rebinding"));}
        self.settings.validate().map_err(|_|Error::Invalid("soft-body settings"))?;
        core_settings(self.settings).validate().map_err(|_|Error::Invalid("soft-body settings"))?;
        schema::name(&self.object,limits)?;
        let mut skeleton=state.skeleton.clone().unwrap_or_else(||Skeleton{joints:vec![Joint{name:"soft_body_root".into(),parent:None,translation:[0.;3]}]});
        skeleton.validate(limits,ctx)?;
        let reused_tets=reusable_tets(state,&skeleton)?;
        let reusing=reused_tets.is_some();
        let new_frames=self.attachments.iter().filter(|a|!reusing||!skeleton.joints.iter().any(|j|j.name==a.name)).count();
        let total=skeleton.joints.len()+if reusing{0}else{SOFT_BODY_TETRAHEDRA}+new_frames;
        if total>limits.max_joints.min(128){return Err(Error::Budget("soft-body joints; use an explicit max_joints=128 document"));}
        let body=state.objects.get(&self.object).ok_or_else(||Error::MissingObject(self.object.clone()))?;
        if !body.validate(ctx)?.is_closed_manifold{return Err(Error::Invalid("soft-body body must be a closed manifold"));}
        let mut groups=BTreeSet::from([self.object.as_str()]);let mut names=BTreeSet::new();
        if self.deform_objects.len()>32||self.attachments.len()>32{return Err(Error::Budget("soft-body object groups"));}
        for object in &self.deform_objects {schema::name(object,limits)?;if !groups.insert(object.as_str()){return Err(Error::Invalid("duplicate soft-body deform object"));}}
        for a in &self.attachments {
            schema::name(&a.name,limits)?;
            if a.name.starts_with("__soft_body_tet_"){return Err(Error::Invalid("reserved soft-body cell joint name"));}
            if !names.insert(a.name.as_str())||a.objects.is_empty(){return Err(Error::Invalid("duplicate/empty soft-body attachment"));}
            for object in &a.objects{schema::name(object,limits)?;if !groups.insert(object.as_str()){return Err(Error::Invalid("soft-body object belongs to multiple bindings"));}}
        }
        for name in &groups {check_geometry(state,name)?;}
        let working=groups.iter().map(|name|state.objects[*name].vertices().len().saturating_mul(160)).sum::<usize>();
        if working>ctx.limits.max_bytes {return Err(Error::Budget("soft-body binding working bytes"));}
        // Existing rig root is preserved. A rotated/scaled root would make
        // rigid child animation inherit scale/shear, so refuse that contract.
        let root=state.skeleton.as_ref().map(|s|state.rig.local_rest(s,0)).transpose()?.unwrap_or_default();
        if root.rotation!=[0.,0.,0.,1.]||root.scale!=[1.;3]{return Err(Error::Invalid("soft-body rig root must have identity rest rotation and scale"));}
        let tet_joints=if let Some(joints)=reused_tets{joints}else{
            (0..SOFT_BODY_TETRAHEDRA).map(|index|{let joint=skeleton.joints.len() as u16;
                skeleton.joints.push(Joint{name:format!("__soft_body_tet_{index:02}"),parent:Some(0),translation:root.translation.map(|v|-v)});joint
            }).collect::<Vec<_>>()
        };
        let points=world_points(state,&self.object,ctx)?;
        let deform_points=self.deform_objects.iter().map(|object|Ok((object.clone(),world_points(state,object,ctx)?))).collect::<Result<Vec<_>>>()?;
        let body_bounds=bounds(points.iter().map(|(_,p)|*p))?;
        let center=std::array::from_fn::<_,3,_>(|i|(body_bounds[0][i]+body_bounds[1][i])*0.5);
        let mut radii=std::array::from_fn::<_,3,_>(|i|(body_bounds[1][i]-body_bounds[0][i])*0.5);
        if radii.iter().any(|r|*r<1e-4){return Err(Error::Invalid("soft-body body needs three nonzero dimensions"));}
        let mut pivots=Vec::new();
        for a in &self.attachments {
            let pivot=if let Some(p)=a.pivot{p}else{
                let mut all=Vec::new();for object in &a.objects{all.extend(world_points(state,object,ctx)?.into_iter().map(|(_,p)|p));}
                let b=bounds(all.into_iter())?;std::array::from_fn(|i|(b[0][i]+b[1][i])*0.5)
            };point32(pivot)?;pivots.push(pivot);
        }
        // Enclose every authored body point and rigid-frame pivot, including
        // nonspherical bodies. No authored vertex is displaced or projected.
        let mut enclosure=1.0f64;
        for p in points.iter().map(|(_,p)|p).chain(deform_points.iter().flat_map(|(_,points)|points.iter().map(|(_,p)|p))).chain(&pivots){ctx.checkpoint(1)?;
            enclosure=enclosure.max((0..3).map(|i|((p[i]-center[i])/radii[i]).powi(2)).sum::<f64>().sqrt());}
        for r in &mut radii{*r*=enclosure*(1.+1e-5);}
        let mut cage=physics::SoftBodyDefinition::ellipsoid(point32(center)?,point32(radii)?).map_err(|_|Error::Invalid("soft-body enclosing cage"))?;
        if cage.rest_positions.len()!=SOFT_BODY_PARTICLES||cage.tetrahedra.len()!=SOFT_BODY_TETRAHEDRA{return Err(Error::Invalid("unsupported soft-body cage topology"));}
        let binder=cage.binder().map_err(|_|Error::Invalid("soft-body cage definition"))?;
        let mut weights=Vec::with_capacity(points.len());
        for (vertex,p) in &points{ctx.checkpoint(SOFT_BODY_TETRAHEDRA as u64)?;
            let bind=binder.bind_point(point32(*p)?).map_err(|_|Error::Invalid("body vertex is outside soft-body cage"))?;
            weights.push((*vertex,vec![mesh::JointWeight{joint:tet_joints[bind.tetrahedron as usize] as u32,weight:1.}]));
        }
        let mut extra_weights=Vec::new();
        for(object,points)in &deform_points {let mut weights=Vec::with_capacity(points.len());for(vertex,p)in points{
            ctx.checkpoint(80)?;let bind=binder.bind_point(point32(*p)?).map_err(|_|Error::Invalid("deform vertex outside soft-body cage"))?;
            weights.push((*vertex,vec![mesh::JointWeight{joint:tet_joints[bind.tetrahedron as usize] as u32,weight:1.}]));
        }extra_weights.push((object.clone(),weights));}
        let samples=sample_surface(&binder,&points,ctx)?;
        let mut attachments=Vec::new();
        for (a,pivot) in self.attachments.iter().zip(pivots){
            let rest_pivot=point32(pivot)?;let bind=binder.bind_point(rest_pivot).map_err(|_|Error::Invalid("attachment pivot outside soft-body cage"))?;
            let translation=std::array::from_fn(|i|pivot[i]-root.translation[i]);
            let joint=if let Some(index)=skeleton.joints.iter().position(|j|j.name==a.name).filter(|_|reusing){
                let rest=state.rig.local_rest(&skeleton,index)?;
                if index==0||skeleton.joints[index].parent!=Some(0)||rest.rotation!=[0.,0.,0.,1.]||rest.scale!=[1.;3]{return Err(Error::Invalid("existing soft-body attachment name requires a rigid root-child frame"));}
                skeleton.joints[index].translation=translation;index as u16
            }else{let joint=skeleton.joints.len() as u16;skeleton.joints.push(Joint{name:a.name.clone(),parent:Some(0),translation});joint};
            attachments.push(SoftBodyAttachment{name:a.name.clone(),joint,binding:binding(bind),rest_pivot,objects:a.objects.clone()});
        }
        drop(binder);
        cage.surface_samples=samples;
        cage.validate().map_err(|_|Error::Invalid("soft-body cage definition"))?;
        skeleton.validate(limits,ctx)?;
        // All fallible schema/cage/budget work is complete before mutations.
        // Document.apply still owns the outer transaction candidate for mesh
        // budget/cancellation failures while writing these weights.
        state.skeleton=Some(skeleton);
        for a in &attachments {if let Some(rest)=state.rig.rests.get_mut(&(a.joint as u32)){rest.translation=state.skeleton.as_ref().unwrap().joints[a.joint as usize].translation;}}
        state.objects.get_mut(&self.object).unwrap().set_weights_bulk(&weights,ctx)?;
        for(object,weights)in extra_weights{state.objects.get_mut(&object).unwrap().set_weights_bulk(&weights,ctx)?;}
        for a in &attachments {for object in &a.objects{
            let mesh=state.objects.get_mut(object).ok_or_else(||Error::MissingObject(object.clone()))?;
            let skeleton=state.skeleton.as_ref().unwrap();
            let weights=mesh.vertices().iter().map(|v|{let preserve=reusing&&!v.weights.is_empty()&&v.weights.iter().all(|w|within_frame(skeleton,w.joint,a.joint as u32));
                (v.id,if preserve{v.weights.clone()}else{vec![mesh::JointWeight{joint:a.joint as u32,weight:1.}]})}).collect::<Vec<_>>();
            mesh.set_weights_bulk(&weights,ctx)?;
        }}
        let mut metadata=SoftBodyMetadata{version:1,object:self.object.clone(),deform_objects:self.deform_objects.clone(),root_joint:0,
            rest_positions:cage.rest_positions,tetrahedra:cage.tetrahedra,surface_samples:cage.surface_samples.into_iter().map(binding).collect(),anchors:cage.anchors,
            settings:self.settings,tet_joints,attachments,geometry_hash:String::new()};
        metadata.geometry_hash=geometry_hash(state,&metadata,limits,ctx)?;
        metadata.validate(total).map_err(|_|Error::Invalid("soft-body metadata"))?;
        let mut metrics=std::collections::BTreeMap::from([("particles".into(),43),("tetrahedra".into(),80),("joints".into(),total),("collision_samples".into(),metadata.surface_samples.len())]);
        for a in &metadata.attachments{metrics.insert(format!("attachment_joint:{}",a.name),a.joint as usize);}
        state.soft_body=Some(metadata);
        Ok(OperationResult{object:self.object.clone(),metrics,..Default::default()})
    }
}

/// Detach simulation while preserving the ordinary rest skin, joint ordinals,
/// descendants and every authored rig reference. A snapshot needs no separate
/// undo history or transient rebinding state to remain editable.
pub(crate) fn unbind(state:&mut State,ctx:&mut mesh::Context<'_>)->Result<OperationResult>{
    ctx.checkpoint(1)?;
    let metadata=state.soft_body.take().ok_or(Error::Invalid("no soft body is bound"))?;
    Ok(OperationResult{object:metadata.object,metrics:std::collections::BTreeMap::from([
        ("joints_retained".into(),state.skeleton.as_ref().map_or(0,|s|s.joints.len()))
    ]),..Default::default()})
}

fn reusable_tets(state:&State,skeleton:&Skeleton)->Result<Option<Vec<u16>>>{
    let count=skeleton.joints.iter().filter(|j|j.name.starts_with("__soft_body_tet_")).count();
    if count==0{return Ok(None);}
    if count!=SOFT_BODY_TETRAHEDRA{return Err(Error::Invalid("incomplete reserved soft-body cell joint block"));}
    let root=state.rig.local_rest(skeleton,0)?;
    let mut joints=Vec::with_capacity(SOFT_BODY_TETRAHEDRA);
    for index in 0..SOFT_BODY_TETRAHEDRA{
        let name=format!("__soft_body_tet_{index:02}");
        let joint=skeleton.joints.iter().position(|j|j.name==name).ok_or(Error::Invalid("invalid reserved soft-body cell joint name"))?;
        let rest=state.rig.local_rest(skeleton,joint)?;
        if joint==0||skeleton.joints[joint].parent!=Some(0)||rest.rotation!=[0.,0.,0.,1.]||rest.scale!=[1.;3]
            ||(0..3).any(|i|(rest.translation[i]+root.translation[i]).abs()>1e-8){return Err(Error::Invalid("reserved soft-body cell rest frame changed"));}
        joints.push(joint as u16);
    }
    Ok(Some(joints))
}

fn within_frame(skeleton:&Skeleton,joint:u32,frame:u32)->bool{
    let mut next=Some(joint);
    // Parent ordering is validated before binding/validation; keep the walk
    // bounded as this helper is also used while a transaction is being built.
    for _ in 0..skeleton.joints.len(){let Some(j)=next else{return false;};if j==frame{return true;}next=skeleton.joints.get(j as usize).and_then(|j|j.parent);}
    false
}

fn check_geometry(state:&State,object:&str)->Result<()> {
    if !state.objects.contains_key(object){return Err(Error::MissingObject(object.into()));}
    if state.scene.nodes.get(object).is_some_and(|n|n.linked_to.is_some()) || state.scene.modifiers.contains_key(object)
        || state.scene.lods.contains_key(object) || state.rig.morphs.values().any(|m|m.object==object) {
        return Err(Error::Invalid("soft-body binding requires unique baked geometry without body/attachment LODs or shape keys"));
    }Ok(())
}
fn world_points(state:&State,object:&str,ctx:&mut mesh::Context<'_>)->Result<Vec<(mesh::VertexId,[f64;3])>>{
    let mesh=state.objects.get(object).ok_or_else(||Error::MissingObject(object.into()))?;let world=state.scene.world_matrix(object)?;
    mesh.vertices().iter().map(|v|{ctx.checkpoint(1)?;let p=transform_point(world,v.position);point32(p)?;Ok((v.id,p))}).collect()
}
fn bounds(points:impl Iterator<Item=[f64;3]>)->Result<[[f64;3];2]>{let mut b=[[f64::INFINITY;3],[f64::NEG_INFINITY;3]];for p in points{for i in 0..3{b[0][i]=b[0][i].min(p[i]);b[1][i]=b[1][i].max(p[i]);}}if b.iter().flatten().any(|v|!v.is_finite()){return Err(Error::Invalid("empty soft-body geometry"));}Ok(b)}

fn sample_surface(binder:&physics::SoftBodyBinder<'_>,points:&[(mesh::VertexId,[f64;3])],ctx:&mut mesh::Context<'_>)->Result<Vec<physics::SoftBodyBinding>>{
    let count=points.len().min(128);let mut chosen=Vec::with_capacity(count);let mut distances=vec![f64::INFINITY;points.len()];
    // Deterministic farthest-point sampling includes only original visible
    // vertices, spreading contacts without making the cage the collider.
    let mut next=0;
    for _ in 0..count{
        chosen.push(next);let p=points[next].1;let mut farthest=(0.,0);
        for(i,(_,q))in points.iter().enumerate(){ctx.checkpoint(1)?;let d=(0..3).map(|a|(p[a]-q[a]).powi(2)).sum::<f64>();distances[i]=distances[i].min(d);if distances[i]>farthest.0{farthest=(distances[i],i);}}
        if farthest.0<=1e-16{break;}next=farthest.1;
    }
    chosen.into_iter().map(|i|{ctx.checkpoint(80)?;binder.bind_point(point32(points[i].1)?).map_err(|_|Error::Invalid("soft-body surface contact embedding"))}).collect()
}

fn geometry_hash(state:&State,metadata:&SoftBodyMetadata,limits:&Limits,ctx:&mut mesh::Context<'_>)->Result<String>{
    let objects=std::iter::once(&metadata.object).chain(&metadata.deform_objects).chain(metadata.attachments.iter().flat_map(|a|a.objects.iter())).collect::<BTreeSet<_>>();
    let mut w=Writer::new(limits.max_source_bytes);w.raw(b"soft-body-geometry-v1\0")?;
    for object in objects{check_geometry(state,object)?;w.string(object)?;let mesh=&state.objects[object];w.raw(&crate::rig_control::topology(mesh))?;
        for (id,p) in world_points(state,object,ctx)?{w.u64(id.0)?;for v in p{w.f64(v)?;}}
    }
    Ok(makepad_core_util::sha256::sha256(&w.bytes).iter().map(|b|format!("{b:02x}")).collect())
}

pub(crate) fn validate(state:&State,limits:&Limits,ctx:&mut mesh::Context<'_>)->Result<()> {
    let Some(m)=&state.soft_body else{return Ok(());};let skeleton=state.skeleton.as_ref().ok_or(Error::Invalid("soft-body metadata requires skeleton"))?;
    m.validate(skeleton.joints.len()).map_err(|_|Error::Invalid("soft-body metadata"))?;
    let cage=definition(m);let binder=cage.binder().map_err(|_|Error::Invalid("soft-body cage"))?;
    core_settings(m.settings).validate().map_err(|_|Error::Invalid("soft-body settings"))?;
    if geometry_hash(state,m,limits,ctx)?!=m.geometry_hash{return Err(Error::Invalid("bound soft-body geometry changed; use soft_body_unbind before editing and rebinding"));}
    let root=state.rig.local_rest(skeleton,m.root_joint as usize)?;
    if root.rotation!=[0.,0.,0.,1.]||root.scale!=[1.;3]{return Err(Error::Invalid("soft-body root rest rotation/scale changed"));}
    let physical_joints=m.tet_joints.iter().copied().chain(m.attachments.iter().map(|a|a.joint)).collect::<BTreeSet<_>>();
    if state.clips.values().flat_map(|clip|&clip.channels).any(|channel|physical_joints.contains(&(channel.joint as u16))) {
        return Err(Error::Invalid("animate children of soft-body rigid frames; physical frame joints are solver-owned"));
    }
    for &joint in &m.tet_joints {
        let j=&skeleton.joints[joint as usize];let rest=state.rig.local_rest(skeleton,joint as usize)?;
        if j.parent!=Some(m.root_joint as u32)||rest.rotation!=[0.,0.,0.,1.]||rest.scale!=[1.;3]
            || (0..3).any(|i|(rest.translation[i]+root.translation[i]).abs()>1e-8) {
            return Err(Error::Invalid("soft-body affine joint rest changed"));
        }
    }
    let mut tet_for_joint=[None;128];for(index,joint)in m.tet_joints.iter().enumerate(){tet_for_joint[*joint as usize]=Some(index);}
    for (object,mesh) in &state.objects{
        let required=object==&m.object||m.deform_objects.contains(object);
        let world=state.scene.world_matrix(object)?;
        for vertex in mesh.vertices(){
            ctx.checkpoint(1+vertex.weights.len() as u64)?;
            let affine=vertex.weights.iter().filter(|w|w.weight>0.).find_map(|w|tet_for_joint.get(w.joint as usize).copied().flatten());
            if !required&&affine.is_none(){continue;}
            let mut weights=vertex.weights.iter().filter(|w|w.weight>0.);
            if !weights.next().is_some_and(|w|(w.weight-1.).abs()<1e-6)||weights.next().is_some(){return Err(Error::Invalid("soft-body affine vertex has blended skin weights"));}
            ctx.checkpoint(80)?;
            let p=point32(transform_point(world,vertex.position))?;
            let b=binder.bind_point(p).map_err(|_|Error::Invalid("soft-body affine vertex outside cage"))?;
            if affine!=Some(b.tetrahedron as usize){return Err(Error::Invalid("soft-body affine vertex weights changed"));}
        }
    }
    for a in &m.attachments {
        let j=&skeleton.joints[a.joint as usize];let rest=state.rig.local_rest(skeleton,a.joint as usize)?;
        if j.name!=a.name||j.parent!=Some(m.root_joint as u32)||rest.rotation!=[0.,0.,0.,1.]||rest.scale!=[1.;3]
            || (0..3).any(|i|((rest.translation[i]+root.translation[i]) as f32-a.rest_pivot[i]).abs()>1e-5){return Err(Error::Invalid("soft-body rigid frame rest changed"));}
        for object in &a.objects {for vertex in state.objects[object].vertices(){ctx.checkpoint(1)?;
            if vertex.weights.is_empty(){return Err(Error::Invalid("soft-body attachment is unbound"));}
            for w in &vertex.weights {if !within_frame(skeleton,w.joint,a.joint as u32){return Err(Error::Invalid("rigid attachment weights must remain inside its frame hierarchy"));}}
        }}
    }
    Ok(())
}

// f32 values use exact f64 widening; readers reject lossy representations.
fn write_softbodysettings(w:&mut Writer,value:&SoftBodySettings)->Result<()>{w.f64(value.mass as f64)?;w.f64(value.edge_compliance as f64)?;w.f64(value.volume_compliance as f64)?;w.f64(value.pose_compliance as f64)?;w.f64(value.damping as f64)?;for item in &value.gravity{w.f64(*item as f64)?;}w.f64(value.contact_radius as f64)?;w.f64(value.friction as f64)?;w.u8(value.substeps)?;w.u8(value.iterations)?;w.f64(value.max_speed as f64)?;w.f64(value.max_displacement as f64)?;Ok(())}
fn read_softbodysettings(r:&mut Reader,limits:&Limits)->Result<SoftBodySettings>{let _=limits;Ok(SoftBodySettings{mass:read_f32(r)?,edge_compliance:read_f32(r)?,volume_compliance:read_f32(r)?,pose_compliance:read_f32(r)?,damping:read_f32(r)?,gravity:[read_f32(r)?,read_f32(r)?,read_f32(r)?],contact_radius:read_f32(r)?,friction:read_f32(r)?,substeps:r.u8()?,iterations:r.u8()?,max_speed:read_f32(r)?,max_displacement:read_f32(r)?})}
fn write_softbodyattachmentrequest(w:&mut Writer,value:&SoftBodyAttachmentRequest)->Result<()>{w.string(&value.name)?;w.count(value.objects.len())?;for item in &value.objects{w.string(item)?;}if let Some(item)=&value.pivot{w.u8(1)?;for item in item{w.f64(*item)?;}}else{w.u8(0)?;}Ok(())}
fn read_softbodyattachmentrequest(r:&mut Reader,limits:&Limits)->Result<SoftBodyAttachmentRequest>{let _=limits;Ok(SoftBodyAttachmentRequest{name:r.string(limits.max_name_bytes)?,objects:{let mut values=Vec::new();for _ in 0..r.count(64)?{values.push(r.string(limits.max_name_bytes)?);}values},pivot:if read_bool(r)?{Some([r.f64()?,r.f64()?,r.f64()?])}else{None}})}
fn write_softbodybind(w:&mut Writer,value:&SoftBodyBind)->Result<()>{w.string(&value.object)?;w.count(value.deform_objects.len())?;for item in &value.deform_objects{w.string(item)?;}w.count(value.attachments.len())?;for item in &value.attachments{write_softbodyattachmentrequest(w,item)?;}write_softbodysettings(w,&value.settings)?;Ok(())}
fn read_softbodybind(r:&mut Reader,limits:&Limits)->Result<SoftBodyBind>{let _=limits;Ok(SoftBodyBind{object:r.string(limits.max_name_bytes)?,deform_objects:{let mut values=Vec::new();for _ in 0..r.count(32)?{values.push(r.string(limits.max_name_bytes)?);}values},attachments:{let mut values=Vec::new();for _ in 0..r.count(32)?{values.push(read_softbodyattachmentrequest(r,limits)?);}values},settings:read_softbodysettings(r,limits)?})}
fn write_softbodybinding(w:&mut Writer,value:&SoftBodyBinding)->Result<()>{w.u32(value.tetrahedron as u32)?;for item in &value.weights{w.f64(*item as f64)?;}Ok(())}
fn read_softbodybinding(r:&mut Reader,limits:&Limits)->Result<SoftBodyBinding>{let _=limits;Ok(SoftBodyBinding{tetrahedron:r.u32()?.try_into().map_err(|_|Error::Corrupt("u16 overflow"))?,weights:[read_f32(r)?,read_f32(r)?,read_f32(r)?,read_f32(r)?]})}
fn write_softbodyattachment(w:&mut Writer,value:&SoftBodyAttachment)->Result<()>{w.string(&value.name)?;w.u32(value.joint as u32)?;write_softbodybinding(w,&value.binding)?;for item in &value.rest_pivot{w.f64(*item as f64)?;}w.count(value.objects.len())?;for item in &value.objects{w.string(item)?;}Ok(())}
fn read_softbodyattachment(r:&mut Reader,limits:&Limits)->Result<SoftBodyAttachment>{let _=limits;Ok(SoftBodyAttachment{name:r.string(128)?,joint:r.u32()?.try_into().map_err(|_|Error::Corrupt("u16 overflow"))?,binding:read_softbodybinding(r,limits)?,rest_pivot:[read_f32(r)?,read_f32(r)?,read_f32(r)?],objects:{let mut values=Vec::new();for _ in 0..r.count(64)?{values.push(r.string(128)?);}values}})}
fn write_softbodymetadata(w:&mut Writer,value:&SoftBodyMetadata)->Result<()>{w.u32(value.version)?;w.string(&value.object)?;w.count(value.deform_objects.len())?;for item in &value.deform_objects{w.string(item)?;}w.u32(value.root_joint as u32)?;w.count(value.rest_positions.len())?;for item in &value.rest_positions{for item in item{w.f64(*item as f64)?;}}w.count(value.tetrahedra.len())?;for item in &value.tetrahedra{for item in item{w.u32(*item as u32)?;}}w.count(value.surface_samples.len())?;for item in &value.surface_samples{write_softbodybinding(w,item)?;}w.count(value.anchors.len())?;for item in &value.anchors{w.u32(*item as u32)?;}write_softbodysettings(w,&value.settings)?;w.count(value.tet_joints.len())?;for item in &value.tet_joints{w.u32(*item as u32)?;}w.count(value.attachments.len())?;for item in &value.attachments{write_softbodyattachment(w,item)?;}w.string(&value.geometry_hash)?;Ok(())}
fn read_softbodymetadata(r:&mut Reader,limits:&Limits)->Result<SoftBodyMetadata>{let _=limits;Ok(SoftBodyMetadata{version:r.u32()?,object:r.string(128)?,deform_objects:{let mut values=Vec::new();for _ in 0..r.count(32)?{values.push(r.string(128)?);}values},root_joint:r.u32()?.try_into().map_err(|_|Error::Corrupt("u16 overflow"))?,rest_positions:{let mut values=Vec::new();for _ in 0..r.count(SOFT_BODY_PARTICLES)?{values.push([read_f32(r)?,read_f32(r)?,read_f32(r)?]);}values},tetrahedra:{let mut values=Vec::new();for _ in 0..r.count(SOFT_BODY_TETRAHEDRA)?{values.push([r.u32()?.try_into().map_err(|_|Error::Corrupt("u16 overflow"))?,r.u32()?.try_into().map_err(|_|Error::Corrupt("u16 overflow"))?,r.u32()?.try_into().map_err(|_|Error::Corrupt("u16 overflow"))?,r.u32()?.try_into().map_err(|_|Error::Corrupt("u16 overflow"))?]);}values},surface_samples:{let mut values=Vec::new();for _ in 0..r.count(128)?{values.push(read_softbodybinding(r,limits)?);}values},anchors:{let mut values=Vec::new();for _ in 0..r.count(SOFT_BODY_PARTICLES)?{values.push(r.u32()?.try_into().map_err(|_|Error::Corrupt("u16 overflow"))?);}values},settings:read_softbodysettings(r,limits)?,tet_joints:{let mut values=Vec::new();for _ in 0..r.count(SOFT_BODY_TETRAHEDRA)?{values.push(r.u32()?.try_into().map_err(|_|Error::Corrupt("u16 overflow"))?);}values},attachments:{let mut values=Vec::new();for _ in 0..r.count(32)?{values.push(read_softbodyattachment(r,limits)?);}values},geometry_hash:r.string(64)?})}
impl SoftBodyBind {
    pub fn check(&self,limits:&Limits)->Result<()>{
        schema::name(&self.object,limits)?;
        if self.deform_objects.len()>32||self.attachments.len()>32{return Err(Error::Budget("authoring array"));}
        for object in &self.deform_objects{schema::name(object,limits)?;}
        for a in &self.attachments{schema::name(&a.name,limits)?;if a.objects.len()>64{return Err(Error::Budget("authoring array"));}if a.objects.is_empty(){return Err(Error::Invalid("empty soft-body attachment objects"));}for object in &a.objects{schema::name(object,limits)?;}if let Some(p)=a.pivot{finite(p)?;}}
        if 1+SOFT_BODY_TETRAHEDRA+self.attachments.len()>limits.max_joints.min(128){return Err(Error::Budget("soft-body joints; use an explicit max_joints=128 document"));}
        self.settings.validate().map_err(|_|Error::Invalid("soft-body settings"))?;
        core_settings(self.settings).validate().map_err(|_|Error::Invalid("soft-body settings"))
    }
    pub(crate) fn write(&self,w:&mut Writer)->Result<()>{write_softbodybind(w,self)}
    pub(crate) fn read(r:&mut Reader,limits:&Limits)->Result<Self>{let value=read_softbodybind(r,limits)?;value.check(limits)?;Ok(value)}
}
pub(crate) fn write(w:&mut Writer,m:&SoftBodyMetadata)->Result<()>{write_softbodymetadata(w,m)}
pub(crate) fn read(r:&mut Reader,limits:&Limits)->Result<SoftBodyMetadata>{let value=read_softbodymetadata(r,limits)?;value.validate(limits.max_joints.min(128)).map_err(|_|Error::Corrupt("soft-body metadata"))?;Ok(value)}

#[cfg(test)]
mod typed_tests {
    fn settings(p:physics::SoftBodySettings)->SoftBodySettings{SoftBodySettings {
        mass:p.mass,edge_compliance:p.edge_compliance,volume_compliance:p.volume_compliance,pose_compliance:p.pose_compliance,
        damping:p.damping,gravity:p.gravity,contact_radius:p.contact_radius,friction:p.friction,substeps:p.substeps,
        iterations:p.iterations,max_speed:p.max_speed,max_displacement:p.max_displacement,
    }}
    use super::*;
    #[test]
    fn bind_literals_and_settings_defaults(){
        let limits=Limits{max_joints:128,..Limits::default()};let mut preset=physics::SoftBodySettings::default();preset.edge_compliance=0.001;preset.pose_compliance=0.001;preset.damping=22.;
        let plain=SoftBodyBind{object:"body".into(),deform_objects:vec![],attachments:vec![],settings:settings(physics::SoftBodySettings::default())};
        let grouped=SoftBodyBind{object:"body".into(),deform_objects:vec!["fibers".into()],attachments:vec![SoftBodyAttachmentRequest{name:"frame".into(),objects:vec!["button".into()],pivot:Some([0.25,0.5,1.])},SoftBodyAttachmentRequest{name:"other".into(),objects:vec!["other".into()],pivot:None}],settings:settings(preset)};
        for op in [&plain,&grouped]{op.check(&limits).unwrap();let mut w=Writer::new(usize::MAX);op.write(&mut w).unwrap();let mut r=Reader::new(&w.bytes);assert_eq!(SoftBodyBind::read(&mut r,&limits).unwrap(),*op);r.end().unwrap();assert_eq!(op.memory_bytes(),w.bytes.len()*3);}
        let mut bad=plain.clone();bad.settings.mass=0.;assert!(bad.check(&limits).is_err());bad=plain.clone();bad.object.clear();assert!(bad.check(&limits).is_err());let mut bad=grouped;bad.attachments[0].objects.clear();assert!(bad.check(&limits).is_err());assert!(plain.check(&Limits::default()).is_err());
        let mut w=Writer::new(usize::MAX);bad.write(&mut w).unwrap();assert!(SoftBodyBind::read(&mut Reader::new(&w.bytes),&limits).is_err());
    }
    #[test]
    fn metadata_binary_roundtrip_and_exact_f32(){
        let limits=Limits{max_joints:128,..Limits::default()};let cage=physics::SoftBodyDefinition::ellipsoid([0.;3],[1.;3]).unwrap();let binder=cage.binder().unwrap();let bind=binding(binder.bind_point([0.;3]).unwrap());drop(binder);
        let metadata=SoftBodyMetadata{version:1,object:"body".into(),deform_objects:vec!["fibers".into()],root_joint:0,rest_positions:cage.rest_positions,tetrahedra:cage.tetrahedra,surface_samples:vec![bind],anchors:cage.anchors,settings:settings(physics::SoftBodySettings::default()),tet_joints:(1..=80).collect(),attachments:vec![SoftBodyAttachment{name:"frame".into(),joint:81,binding:bind,rest_pivot:[0.;3],objects:vec!["button".into()]}],geometry_hash:"0".repeat(64)};
        metadata.validate(128).unwrap();let mut w=Writer::new(usize::MAX);write(&mut w,&metadata).unwrap();let mut r=Reader::new(&w.bytes);assert_eq!(read(&mut r,&limits).unwrap(),metadata);r.end().unwrap();
        let mut bad=metadata;bad.version=2;let mut w=Writer::new(usize::MAX);write(&mut w,&bad).unwrap();assert!(matches!(read(&mut Reader::new(&w.bytes),&limits),Err(Error::Corrupt("soft-body metadata"))));
        let mut w=Writer::new(64);w.f64(0.1).unwrap();assert!(matches!(read_f32(&mut Reader::new(&w.bytes)),Err(Error::Corrupt("noncanonical f32"))));
        assert!(matches!(read_bool(&mut Reader::new(&[2])),Err(Error::Corrupt("boolean tag"))));
    }
}
