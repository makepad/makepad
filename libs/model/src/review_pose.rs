//! Worker-only, disposable pose baking for visual clearance review. Never
//! commits a pose, changes the document head, or mutates the source mesh.
use crate::{*,transform::*};
use std::collections::BTreeMap;

#[derive(Clone,Debug)]
pub enum ReviewPose {
    SoftBody{scenario:String},
    /// Physical steering angle and requested model-space wheel displacement.
    /// Authored visual mappings are applied exactly as in the game renderer.
    Vehicle{steer:f64,suspension:f64},
    Pose{name:String},
    Clip{name:String,time:f64},
}
impl ReviewPose {
    pub fn from_json(value:&json::Value)->Result<Self>{
        use json::Value;
        let Value::Obj(fields)=value else{return Err(Error::Invalid("motion sample must be an object"));};
        let kind=value.get("kind").and_then(Value::as_str).ok_or(Error::Invalid("motion sample kind"))?;
        let allowed:&[&str]=match kind{"vehicle"=>&["kind","steer","suspension"],"pose"=>&["kind","name"],"clip"=>&["kind","name","time"],"soft_body"=>&["kind","scenario"],_=>return Err(Error::Invalid("motion kind must be vehicle, pose, clip or soft_body"))};
        if fields.iter().any(|(key,_)|!allowed.contains(&key.as_str())){return Err(Error::Invalid("unknown motion sample field"));}
        let number=|key:&str|->Result<f64>{match value.get(key){Some(Value::Int(v))=>Ok(*v as f64),Some(Value::F64(v)) if v.is_finite()=>Ok(*v),_=>Err(Error::Invalid("motion sample needs a finite number"))}};
        let name=||->Result<String>{value.get("name").and_then(Value::as_str).filter(|s|!s.is_empty()&&s.len()<=96).map(str::to_string).ok_or(Error::Invalid("motion sample name"))};
        match kind{
            "soft_body"=>{let scenario=value.get("scenario").and_then(Value::as_str).filter(|s|["acceleration","landing","wall"].contains(s)).ok_or(Error::Invalid("soft-body scenario"))?;Ok(Self::SoftBody{scenario:scenario.into()})},
            "vehicle"=>{let steer=number("steer")?;let suspension=number("suspension")?;if steer.abs()>1.2||suspension.abs()>5.{return Err(Error::Invalid("motion steering/travel budget"));}Ok(Self::Vehicle{steer,suspension})},
            "pose"=>Ok(Self::Pose{name:name()?}),
            _=>{let time=number("time")?;if time<0.||time>3600.{return Err(Error::Invalid("motion clip time"));}Ok(Self::Clip{name:name()?,time})}
        }
    }
    pub fn label(&self)->String{match self{
        Self::SoftBody{scenario}=>format!("simulated soft body: {scenario}"),
        Self::Vehicle{steer,suspension}=>format!("steering input {:.1} degrees, travel input {:+.3} m (authored visual limits applied)",steer.to_degrees(),suspension),
        Self::Pose{name}=>format!("pose {name}"),Self::Clip{name,time}=>format!("clip {name} at {time:.3} s"),
    }}
}
impl Document {
    /// Four bounded diagnostic samples. These are declared review envelopes,
    /// not a claim about every possible gameplay controller configuration.
    pub fn default_review_poses(&self)->Vec<ReviewPose>{
        if self.soft_body().is_some(){return ["acceleration","landing","wall"].into_iter().map(|scenario|ReviewPose::SoftBody{scenario:scenario.into()}).collect();}

        if !self.scene().wheels.is_empty(){
            let steering=self.scene().wheels.values().map(|w|w.visual.map_or(0.55,|v|if v.steer_gain>0.{(v.steer_max/v.steer_gain).min(1.2)}else{0.})).fold(0.,f64::max);
            let compression=self.scene().wheels.values().map(|w|w.visual.map_or((w.radius*0.8).clamp(0.01,5.),|v|v.compression)).fold(0.,f64::max);
            let droop=self.scene().wheels.values().map(|w|w.visual.map_or((w.radius*0.8).clamp(0.01,5.),|v|v.droop)).fold(0.,f64::max);
            return [(-steering,compression),(steering,compression),(-steering,-droop),(steering,-droop)].into_iter().map(|(steer,suspension)|ReviewPose::Vehicle{steer,suspension}).collect();
        }
        if self.skeleton().is_some(){
            // Named authored stress poses first, followed by representative
            // clip quarter cycles. Report the labels so omitted clips are clear.
            let mut out=self.rig().poses.keys().take(4).map(|name|ReviewPose::Pose{name:name.clone()}).collect::<Vec<_>>();
            for clip in self.clips().values(){for phase in [0.25,0.75]{if out.len()<4{out.push(ReviewPose::Clip{name:clip.name.clone(),time:clip.duration()*phase});}}}
            return out;
        }
        Vec::new()
    }
    pub fn compile_review_pose(&self,pose:&ReviewPose,cancelled:Option<&dyn Fn()->bool>)->Result<CompiledModel>{
        let mut ctx=mesh::Context::new(self.limits().mesh.clone(),cancelled);
        let meshes=self.scene().evaluated_meshes(&self.state,&mut ctx)?;
        let size=meshes.values().map(|m|m.memory_bytes()).sum::<usize>();
        let mut draft=self.render_copy(size.saturating_mul(4))?;
        let mut palette=Vec::new();let mut joints=Vec::new();
        let mut movements=BTreeMap::<String,Matrix4>::new();
        match pose{
            ReviewPose::SoftBody{scenario}=>{let result=crate::soft_review::sample(self,scenario,cancelled)?;palette=result.0;joints=result.1;},
            ReviewPose::Vehicle{steer,suspension}=>{
                if !steer.is_finite()||steer.abs()>1.2||!suspension.is_finite()||suspension.abs()>5.{return Err(Error::Invalid("motion steering/travel budget"));}
                if self.scene().wheels.is_empty()||self.skeleton().is_some(){return Err(Error::Invalid("wheel review requires rigid wheel connections"));}
                for (object,wheel) in &self.scene().wheels{
                    let (visual_steer,visual_travel)=wheel.visual.map_or((*steer,*suspension),|v|v.pose(*steer,*suspension));
                    let anchor=transform_point(self.scene().world_matrix(object)?,wheel.pivot);
                    let rotation=Transform{rotation:quat_axis_angle([0.,1.,0.],if wheel.connection.starts_with("wheel_front_"){-visual_steer}else{0.})?,..Default::default()}.matrix()?;
                    let translation=|p|Transform{translation:p,..Default::default()}.matrix();
                    movements.insert(object.clone(),matrix_mul(translation([0.,visual_travel,0.])?,matrix_mul(translation(anchor)?,matrix_mul(rotation,translation(mul(anchor,-1.))?))));
                }
            },
            ReviewPose::Pose{name}=>{
                let skeleton=self.skeleton().ok_or(Error::Invalid("motion review requires skeleton"))?;
                let pose=self.rig().poses.get(name).ok_or(Error::Invalid("unknown review pose"))?;
                joints=crate::rig_control::globals(skeleton,&self.rig().local_pose(skeleton,Some(pose))?)?.0;
            },
            ReviewPose::Clip{name,time}=>{
                let skeleton=self.skeleton().ok_or(Error::Invalid("motion review requires skeleton"))?;
                let clip=self.clips().get(name).ok_or(Error::Invalid("unknown review clip"))?;
                if !time.is_finite()||*time<0.||*time>clip.duration(){return Err(Error::Invalid("review time outside clip"));}
                joints=crate::rig_control::globals(skeleton,&self.rig().sample_clip(skeleton,clip,*time)?)?.0;
                // Match exported animated shape-key weights before skinning.
                if let Some(options)=self.rig().clip_options.get(name){for (name,keys) in &options.morph_keys{
                    let mut weight=keys[0].weight;
                    for pair in keys.windows(2){if *time>=pair[0].time{let t=((*time-pair[0].time)/(pair[1].time-pair[0].time)).clamp(0.,1.);let t=match options.interpolation{Interpolation::Step if t<1.=>0.,Interpolation::Cubic=>t*t*(3.-2.*t),_=>t};weight=pair[0].weight+(pair[1].weight-pair[0].weight)*t;}}
                    if let Some(morph)=draft.state.rig.morphs.get_mut(name){morph.weight=weight;}
                }}
            },
        }
        if let Some(skeleton)=self.skeleton().filter(|_|palette.is_empty()){
            for (posed,rest) in joints.iter().zip(self.rig().global_rest(skeleton)?){palette.push(matrix_mul(*posed,inverse(rest)?));}
        }
        let movement=|object:&str|->Matrix4{
            let mut next=Some(object);
            while let Some(name)=next{if let Some(m)=movements.get(name){return *m;}next=self.scene().nodes.get(name).and_then(|n|n.parent.as_deref());}
            IDENTITY_MATRIX
        };
        let object_frame=|name:&str|->Result<Matrix4>{Ok(matrix_mul(movement(name),self.scene().world_matrix(name)?))};
        let mut evaluated=BTreeMap::new();
        for (name,original) in meshes{
            ctx.checkpoint(1)?;
            let source=crate::morph_export::source(self,&name)?;
            let mut mesh=(*original).clone();let frame=object_frame(&name)?;
            let mut positions=Vec::with_capacity(mesh.vertices().len());
            for vertex in mesh.vertices(){
                ctx.checkpoint(1)?;let mut p=vertex.position;
                for morph in draft.rig().morphs.values().filter(|m|m.object==source){if let Some(delta)=morph.deltas.get(&vertex.id){p=add(p,mul(*delta,morph.weight));}}
                p=transform_point(frame,p);
                if !palette.is_empty(){
                    let mut skinned=[0.;3];let mut mass=0.;
                    for w in &vertex.weights{let matrix=palette.get(w.joint as usize).ok_or(Error::Invalid("review weight joint"))?;skinned=add(skinned,mul(transform_point(*matrix,p),w.weight));mass+=w.weight;}
                    if mass<0.999||mass>1.001{return Err(Error::Invalid("motion review requires normalized vertex weights"));}
                    p=skinned;
                }
                positions.push((vertex.id,p));
            }
            // Freeze actual render triangles before physical deformation.
            // A planar source quad generally becomes nonplanar when its
            // vertices occupy different soft cells; its GPU triangles remain
            // valid. Preserve corner normals instead of flattening the review.
            let triangles=mesh.triangulate(&mut ctx)?;
            // Baking a reflected object frame changes geometric orientation.
            // Keep winding, UVs and corner normals in one corresponding order,
            // as Mesh::transform does for a reflected editable mesh.
            let reflected=dot(cross(transform_vector(frame,[1.,0.,0.]),transform_vector(frame,[0.,1.,0.])),transform_vector(frame,[0.,0.,1.]))<0.;
            let ordered=|mut indices:[u32;3]|{if reflected{indices.swap(1,2);}indices};
            let ids=positions.iter().enumerate().map(|(i,(id,_))|(*id,i as u32)).collect::<BTreeMap<_,_>>();
            let points=positions.iter().map(|(_,p)|*p).collect::<Vec<_>>();
            let polygons=triangles.triangles.iter().map(|triangle|mesh::Polygon{
                vertices:ordered(triangle.indices).map(|i|ids[&triangles.vertices[i as usize].source_vertex]).to_vec(),
                uvs:ordered(triangle.indices).map(|i|triangles.vertices[i as usize].uv).to_vec(),material:triangle.material,
            }).collect::<Vec<_>>();
            let normal_matrix=|m:Matrix4|->Result<Matrix4>{let inv=inverse(m)?;Ok(std::array::from_fn(|i|std::array::from_fn(|j|inv[j][i])))};
            let object_normal=normal_matrix(frame)?;
            let palette_normals=palette.iter().map(|m|normal_matrix(*m)).collect::<Result<Vec<_>>>()?;
            let normals=triangles.triangles.iter().flat_map(|t|ordered(t.indices)).map(|index|{
                let vertex=&triangles.vertices[index as usize];let normal=transform_vector(object_normal,vertex.normal);
                if palette_normals.is_empty(){return normalized(normal);}
                let mut deformed=[0.;3];
                for w in &vertex.weights{deformed=add(deformed,mul(transform_vector(palette_normals[w.joint as usize],normal),w.weight));}
                normalized(deformed)
            }).collect::<Result<Vec<_>>>()?;
            mesh=mesh::Mesh::from_polygons(&points,&polygons,&mut ctx)?;
            let normal_updates=mesh.corners().iter().zip(normals).map(|(c,n)|(c.id,Some(n))).collect::<Vec<_>>();
            mesh.set_corner_normals_bulk(&normal_updates,&mut ctx)?;
            draft.state.surface.vertex_colors.retain(|(object,_),_|object!=&name);
            for (old,new) in positions.iter().zip(mesh.vertices()){
                if let Some(color)=self.surface().vertex_colors.get(&(source.to_string(),old.0)){
                    draft.state.surface.vertex_colors.insert((name.clone(),new.id),*color);
                }
            }
            evaluated.insert(name,mesh);
        }
        for light in draft.state.scene.emitters.values_mut(){
            let owner=match &light.attachment{Attachment::Root=>IDENTITY_MATRIX,Attachment::Object(name)=>object_frame(name)?,Attachment::Joint(j)=>*joints.get(*j as usize).ok_or(Error::Invalid("review light joint"))?};
            let frame=matrix_mul(owner,light.transform.matrix()?);
            light.transform=Transform{translation:transform_point(frame,[0.;3]),rotation:quat_from_to([0.,0.,-1.],transform_vector(frame,[0.,0.,-1.]))?,scale:[1.;3]};
            light.attachment=Attachment::Root;
        }
        draft.state.surface.reconcile(&self.state.objects,&evaluated,self.limits(),&mut ctx)?;
        draft.state.objects=evaluated;
        let emitters=std::mem::take(&mut draft.state.scene.emitters);
        draft.state.scene=SceneState{emitters,..Default::default()};
        draft.state.soft_body=None;draft.state.skeleton=None;draft.state.clips.clear();draft.state.rig=RigState::default();
        draft.compile_preview(cancelled)
    }
}
