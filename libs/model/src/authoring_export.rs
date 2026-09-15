//! Evaluation and portable metadata assembly stay on the authoring worker.
use crate::{Document,Result,Error,mesh,CompiledModel,PrimitiveSource,Attachment,LightKind,Transform,transform::*};
use makepad_gltf::*;
use std::collections::BTreeMap;

fn f32_value(v:f64)->Result<f32>{let f=v as f32;if f.is_finite(){Ok(f)}else{Err(Error::Invalid("authoring export exceeds render precision"))}}
fn trs(t:Transform)->Result<GlbTransform>{t.validate()?;Ok(GlbTransform{
    translation:[f32_value(t.translation[0])?,f32_value(t.translation[1])?,f32_value(t.translation[2])?],
    rotation:[f32_value(t.rotation[0])?,f32_value(t.rotation[1])?,f32_value(t.rotation[2])?,f32_value(t.rotation[3])?],
    scale:[f32_value(t.scale[0])?,f32_value(t.scale[1])?,f32_value(t.scale[2])?]})}
fn attachment(a:&Attachment,names:&BTreeMap<String,usize>)->Result<GlbAttachment>{match a{
    Attachment::Root=>Ok(GlbAttachment::Root),Attachment::Joint(j)=>Ok(GlbAttachment::Joint(*j as usize)),
    Attachment::Object(n)=>Ok(GlbAttachment::Object(*names.get(n).ok_or_else(||Error::MissingObject(n.clone()))?))}}

impl Document {
    pub fn compile(&self,cancelled:Option<&dyn Fn()->bool>)->Result<CompiledModel>{
        self.compile_view(cancelled,false)
    }
    /// Compile a static draft at the current editable head. Geometry, materials,
    /// shape-key weights and rest-pose lights are visible before binding is done.
    /// The derived GLB deliberately omits skin/animation, wheel connections,
    /// sockets, collision and delivery LODs; it is not a publication product.
    /// Source state, history and completion checks in `compile` are unchanged.
    pub fn compile_preview(&self,cancelled:Option<&dyn Fn()->bool>)->Result<CompiledModel>{
        self.compile_view(cancelled,true)
    }
    fn compile_view(&self,cancelled:Option<&dyn Fn()->bool>,preview:bool)->Result<CompiledModel>{
        if !preview&&self.soft_body().is_none()&&self.scene().is_empty()&&self.rig().morphs.is_empty(){return self.compile_evaluated(cancelled);}
        let mut ctx=mesh::Context::new(self.limits().mesh.clone(),cancelled);
        ctx.checkpoint(1)?;
        self.scene().validate(&self.state,self.limits(),&mut ctx)?;
        crate::soft_body::validate(&self.state,self.limits(),&mut ctx)?;
        if !preview{validate_vehicle_export(self)?;}
        let cached=self.scene().evaluated_meshes(&self.state,&mut ctx)?;
        let size=cached.values().map(|m|m.memory_bytes()).sum::<usize>();
        let mut evaluated=self.render_copy(size.saturating_mul(3))?;
        // Source binding is validated above in authored object space. The
        // evaluation copy bakes those transforms for skin export; attach the
        // immutable original metadata after that derived product is complete.
        evaluated.state.soft_body=None;
        if preview{prepare_preview_metadata(&mut evaluated,&mut ctx)?;}
        let mut objects=BTreeMap::new();let mut bounds=[[f64::INFINITY;3],[f64::NEG_INFINITY;3]];
        for (name,_) in self.objects(){
            ctx.checkpoint(1)?;
            let mut mesh=(**cached.get(name).ok_or_else(||Error::MissingObject(name.into()))?).clone();
            let source=crate::morph_export::source(self,name)?;
            for morph in self.rig().morphs.values().filter(|m|m.object==source){if crate::rig_control::topology(&mesh)!=morph.topology{return Err(Error::Invalid("modifier changed shape key topology; bake modifiers before authoring shape keys"));}}
            for ((object,vertex),color) in &self.surface().vertex_colors{if object==source&&source!=name{evaluated.state.surface.vertex_colors.insert((name.into(),*vertex),*color);}}

            let world=self.scene().world_matrix(name)?;
            for vertex in mesh.vertices(){ctx.checkpoint(1)?;let mut position=vertex.position;for morph in self.rig().morphs.values().filter(|m|m.object==source){if let Some(delta)=morph.deltas.get(&vertex.id){position=add(position,mul(*delta,morph.weight));}}let p=transform_point(world,position);for d in 0..3{bounds[0][d]=bounds[0][d].min(p[d]);bounds[1][d]=bounds[1][d].max(p[d]);}}
            if preview{
                let weights=mesh.vertices().iter().filter(|v|!v.weights.is_empty()).map(|v|(v.id,Vec::new())).collect::<Vec<_>>();
                if !weights.is_empty(){mesh.set_weights_bulk(&weights,&mut ctx)?;}
            }else if self.skeleton().is_some(){
                let ids=mesh.vertices().iter().map(|v|v.id).collect::<Vec<_>>();mesh.transform(&ids,world,&mut ctx)?;
            }
            objects.insert(name.into(),mesh);
        }
        // Interpolate painted attributes onto newly generated modifier vertices.
        evaluated.state.surface.reconcile(&self.state.objects,&objects,self.limits(),&mut ctx)?;
        evaluated.state.objects=objects;
        drop(cached);
        crate::delivery::expand(&mut evaluated,&mut ctx)?;
        let mut product=evaluated.compile_evaluated(cancelled)?;
        if !preview {if let Some(metadata)=self.soft_body(){
            product.glb=augment_glb_soft_body(&product.glb,metadata).map_err(|_|Error::Invalid("soft-body GLB metadata"))?;
        }}
        for i in 0..2{for d in 0..3{product.bounds[i][d]=f32_value(bounds[i][d])?;}}
        Ok(product)
    }
}

fn prepare_preview_metadata(doc:&mut Document,ctx:&mut mesh::Context<'_>)->Result<()> {
    let rests=doc.skeleton().map(|s|doc.rig().global_rest(s)).transpose()?.unwrap_or_default();
    for light in doc.state.scene.emitters.values_mut(){
        ctx.checkpoint(1)?;
        if let Attachment::Joint(joint)=light.attachment{
            let rest=*rests.get(joint as usize).ok_or(Error::Invalid("draft light joint"))?;
            let world=matrix_mul(rest,light.transform.matrix()?);
            // Punctual lights have a point and normalized -Z direction, so this
            // preserves their visible rest placement even with sheared ancestry.
            light.transform=Transform{translation:transform_point(world,[0.;3]),
                rotation:quat_from_to([0.,0.,-1.],transform_vector(world,[0.,0.,-1.]))?,scale:[1.;3]};
            light.attachment=Attachment::Root;
        }
    }
    doc.state.skeleton=None;
    doc.state.clips.clear();
    let morphs=std::mem::take(&mut doc.state.rig.morphs);
    doc.state.rig=crate::RigState{morphs,..Default::default()};
    doc.state.scene.wheels.clear();
    doc.state.scene.sockets.clear();
    doc.state.scene.colliders.clear();
    doc.state.scene.lods.clear();
    Ok(())
}

pub(crate) fn finish(doc:&Document,glb:Vec<u8>,primitives:&[PrimitiveSource],ctx:&mut mesh::Context<'_>)->Result<Vec<u8>>{
    if doc.scene().is_empty()&&doc.rig().is_empty(){return Ok(glb);}
    let scene=doc.scene();let names=doc.objects().enumerate().map(|(i,(n,_))|(n.to_string(),i)).collect::<BTreeMap<_,_>>();
    let mut export=GlbAuthoring::default();
    for (name,_) in doc.objects(){
        ctx.checkpoint(1)?;let node=scene.nodes.get(name).cloned().unwrap_or_default();
        let mut extras=std::collections::HashMap::new();
        if let Some(collision)=crate::delivery::collision(doc,name,ctx)?{extras.insert("MAKEPAD_collision".into(),collision);}
        if let Some(wheel)=scene.wheels.get(name){
            if !primitives.iter().any(|p|p.object==name){return Err(Error::Invalid("vehicle wheel object has no exported geometry"));}
            let vector=|values:[f64;3]|->Result<JsonValue>{Ok(JsonValue::Array(values.into_iter().map(|v|f32_value(v).map(|v|JsonValue::F64(v as f64))).collect::<Result<_>>()?))};
            extras.insert("kind".into(),JsonValue::String("vehicle_wheel".into()));
            extras.insert("connection".into(),JsonValue::String(wheel.connection.clone()));
            extras.insert("pivot".into(),vector(wheel.pivot)?);
            extras.insert("anchor".into(),vector(transform_point(scene.world_matrix(name)?,wheel.pivot))?);
            extras.insert("radius".into(),JsonValue::F64(f32_value(wheel.radius)? as f64));
            extras.insert("width".into(),JsonValue::F64(f32_value(wheel.width)? as f64));
            if let Some(visual)=wheel.visual{
                extras.insert("visual".into(),JsonValue::Object([
                    ("steer_gain".into(),JsonValue::F64(visual.steer_gain)),
                    ("steer_max".into(),JsonValue::F64(visual.steer_max)),
                    ("compression".into(),JsonValue::F64(visual.compression)),
                    ("droop".into(),JsonValue::F64(visual.droop)),
                ].into_iter().collect()));
            }
        }
        export.objects.push(GlbAuthoringObject{name:name.into(),parent:node.parent.as_ref().map(|n|names.get(n).copied().ok_or_else(||Error::MissingObject(n.clone()))).transpose()?,
            transform:trs(node.transform)?,primitives:primitives.iter().enumerate().filter_map(|(i,p)|(p.object==name).then_some(i)).collect(),extras:(!extras.is_empty()).then_some(JsonValue::Object(extras)),lods:Vec::new(),lod_distances:Vec::new()});
    }
    let originals=doc.objects().filter(|(n,_)|!n.starts_with("__derived_lod_")).map(|(n,_)|n.to_owned()).collect::<Vec<_>>();
    for (i,name) in originals.iter().enumerate(){if let Some(levels)=scene.lods.get(name){let base=*names.get(name).ok_or(Error::Invalid("LOD base object"))?;
        for (level,options) in levels.iter().enumerate(){let derived=crate::delivery::lod_name(i,level);export.objects[base].lods.push(*names.get(&derived).ok_or(Error::Invalid("LOD geometry absent"))?);export.objects[base].lod_distances.push(f32_value(options.distance)?);}
    }}
    for light in scene.emitters.values(){
        ctx.checkpoint(1)?;export.lights.push(GlbPunctualLight{name:light.name.clone(),attachment:attachment(&light.attachment,&names)?,transform:trs(light.transform)?,
            kind:match light.kind{LightKind::Point=>GlbPunctualKind::Point,LightKind::Spot{inner,outer}=>GlbPunctualKind::Spot{inner:f32_value(inner)?,outer:f32_value(outer)?}},
            color:[f32_value(light.color[0])?,f32_value(light.color[1])?,f32_value(light.color[2])?],intensity:f32_value(light.intensity)?,range:f32_value(light.range)?});
    }
    for socket in scene.sockets.values(){export.sockets.push(GlbSocket{name:socket.name.clone(),attachment:attachment(&socket.attachment,&names)?,transform:trs(socket.transform)?});}
    if let Some(skeleton)=doc.skeleton(){
        let globals=doc.rig().global_rest(skeleton)?;
        for (i,world) in globals.iter().enumerate(){let inverse=inverse(*world)?;let mut ibm=[0f32;16];for row in 0..4{for col in 0..4{ibm[col*4+row]=f32_value(inverse[row][col])?;}}
            export.rests.push(GlbBoneRest{transform:trs(doc.rig().local_rest(skeleton,i)?)?,inverse_bind:ibm});
        }
    }
    ctx.checkpoint(glb.len() as u64)?;
    let glb=augment_glb_authoring(&glb,&export).map_err(|_|Error::Invalid("authoring GLB augmentation"))?;
    crate::morph_export::finish(doc,glb,primitives,ctx)
}

fn validate_vehicle_export(doc:&Document)->Result<()> {
    let wheels=&doc.scene().wheels;if wheels.is_empty(){return Ok(());}
    if wheels.len()!=4{return Err(Error::Invalid("vehicle export requires all four wheel connections"));}
    if doc.skeleton().is_some(){return Err(Error::Invalid("vehicle wheels require rigid objects, not a skinned asset"));}
    let mut anchors=[[0.;3];4];
    for (object,wheel) in wheels {
        let index=crate::VEHICLE_WHEEL_CONNECTIONS.iter().position(|v|*v==wheel.connection).ok_or(Error::Invalid("vehicle connection"))?;
        anchors[index]=transform_point(doc.scene().world_matrix(object)?,wheel.pivot);
    }
    if anchors[0][0]<=anchors[1][0]||anchors[2][0]<=anchors[3][0]||anchors[0][2]<=anchors[2][2]||anchors[1][2]<=anchors[3][2]{return Err(Error::Invalid("wheel connections require model +Z front, +X left, and distinct axles"));}
    Ok(())
}
