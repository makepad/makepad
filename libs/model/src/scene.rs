//! Editable object hierarchy and attachments. Punctual lights are real scene
//! emitters; surface emissiveness is authored separately in the material domain.
use crate::{canon::{Reader,Writer},document::State,json::{self,Value},mesh, schema::*,
    transform::*,Error,Limits,OperationResult,Result,MeshEditingOperation};
use std::collections::{BTreeMap,BTreeSet};
use std::sync::Arc;
pub use makepad_gltf::VisualWheelMotion;

#[derive(Clone,Debug,PartialEq,Default)]
pub struct SceneNode { pub parent:Option<String>, pub linked_to:Option<String>, pub transform:Transform }
#[derive(Clone,Debug,PartialEq)]
pub enum Attachment { Root,Object(String),Joint(u32) }
#[derive(Clone,Debug,PartialEq)]
pub enum LightKind { Point,Spot{inner:f64,outer:f64} }
#[derive(Clone,Debug,PartialEq)]
pub struct LightEmitter {
    pub name:String,pub attachment:Attachment,pub transform:Transform,pub kind:LightKind,
    /// Linear RGB; intensity is candela. Range is metres, independent of scale.
    pub color:[f64;3],pub intensity:f64,pub range:f64,
}
#[derive(Clone,Debug,PartialEq)]
pub struct Socket {pub name:String,pub attachment:Attachment,pub transform:Transform}
/// Rigid wheel geometry: local X is its axle; the complete car faces model +Z.
/// Pivot is object-local, while radius/width are in model metres.
#[derive(Clone,Debug,PartialEq)]
pub struct VehicleWheel {
    pub connection:String,pub pivot:[f64;3],pub radius:f64,pub width:f64,
    /// Optional visual-only steering and suspension limits; absence preserves
    /// legacy motion. Physical steering and contacts are never changed.
    pub visual:Option<VisualWheelMotion>,
}
pub const VEHICLE_WHEEL_CONNECTIONS:[&str;4]=["wheel_front_left","wheel_front_right","wheel_rear_left","wheel_rear_right"];
/// How many generic driven parts (non-wheel connections) one model may bind.
pub const MAX_DRIVEN_PARTS:usize=48;
#[derive(Clone,Debug,PartialEq)]
pub struct LodLevel {pub target_faces:usize,pub max_error:f64,pub distance:f64}
#[derive(Clone,Debug,PartialEq)]
/// Hull and capsule are fitted to the object's own vertices at compile and
/// delivered as compact triangle proxies.
pub enum CollisionProxy {Box,Sphere,Mesh{object:String},Hull,Capsule}
#[derive(Clone,Debug,PartialEq)]
pub struct NamedModifier {pub name:String,pub enabled:bool,pub operation:MeshEditingOperation}
#[derive(Clone,Debug,Default,PartialEq)]
pub struct SceneState {
    pub nodes:BTreeMap<String,SceneNode>,pub emitters:BTreeMap<String,LightEmitter>,
    pub sockets:BTreeMap<String,Socket>,pub lods:BTreeMap<String,Vec<LodLevel>>,
    pub colliders:BTreeMap<String,CollisionProxy>,
    pub modifiers:BTreeMap<String,Vec<NamedModifier>>,
    pub wheels:BTreeMap<String,VehicleWheel>,
}
#[derive(Clone,Debug,PartialEq)]
pub enum SceneOperation {
    Node{object:String,node:SceneNode},
    Instance{object:String,source:String,transform:Transform},
    Duplicate{object:String,source:String},
    MakeUnique{object:String},
    Join{object:String,sources:Vec<String>},
    Separate{object:String,name:String,faces:Vec<mesh::FaceId>},
    Snap{object:String,grid:f64},
    Pivot{object:String,position:[f64;3]},
    Dimensions{object:String,size:[f64;3]},
    Light(LightEmitter),DeleteLight{name:String},Socket(Socket),DeleteSocket{name:String},
    Lods{object:String,levels:Vec<LodLevel>},Collider{object:String,proxy:CollisionProxy},
    Modifier{object:String,modifier:NamedModifier},RemoveModifier{object:String,name:String},
    ModifierOrder{object:String,names:Vec<String>},ApplyModifiers{object:String},
    VehicleWheel{object:String,wheel:VehicleWheel},DeleteVehicleWheel{object:String},
}
impl SceneState {
    pub fn is_empty(&self)->bool {self==&Self::default()}
    pub fn memory_bytes(&self)->usize {let mut w=Writer::new(usize::MAX);if self.write(&mut w).is_err(){usize::MAX}else{w.bytes.len().saturating_mul(3)}}
    pub(crate) fn validate(&self,state:&State,limits:&Limits,ctx:&mut mesh::Context<'_>)->Result<()> {
        if self.nodes.len()>limits.max_objects||self.emitters.len()>32||self.sockets.len()>128||self.lods.len()>limits.max_objects||self.colliders.len()>limits.max_objects||self.modifiers.len()>limits.max_objects||self.modifiers.values().map(Vec::len).sum::<usize>()>128 {
            return Err(Error::Budget("scene attachments"));
        }
        for (object,node) in &self.nodes {
            ctx.checkpoint(1)?;name(object,limits)?;node.transform.validate()?;
            if !state.objects.contains_key(object) {return Err(Error::MissingObject(object.clone()));}
            for target in node.parent.iter().chain(&node.linked_to) {
                if !state.objects.contains_key(target) {return Err(Error::MissingObject(target.clone()));}
            }
        }
        for (object,stack) in &self.modifiers {
            name(object,limits)?;if !state.objects.contains_key(object){return Err(Error::MissingObject(object.clone()));}
            if stack.is_empty()||stack.len()>16{return Err(Error::Budget("object modifier count"));}let mut unique=BTreeSet::new();
            for modifier in stack {ctx.checkpoint(1)?;name(&modifier.name,limits)?;
                if !unique.insert(&modifier.name)||!modifier.operation.is_modifier_safe()||modifier.operation.object_name()!=object{return Err(Error::Invalid("invalid, duplicate, or wrongly targeted modifier"));}
                for dependency in modifier.operation.dependencies(){if !state.objects.contains_key(dependency){return Err(Error::MissingObject(dependency.into()));}}
            }
        }
        let mut done=BTreeSet::new();for object in state.objects.keys(){self.visit(object,&mut BTreeSet::new(),&mut done,ctx)?;}
        for light in self.emitters.values() {
            name(&light.name,limits)?;light.transform.validate()?;self.attachment(&light.attachment,state)?;
            if light.color.iter().any(|v|!v.is_finite()||!(0.0..=1.0).contains(v))
                || !light.intensity.is_finite()||!(0.0..=1e6).contains(&light.intensity)
                || !light.range.is_finite()||light.range<=0.||light.range>10000. {
                return Err(Error::Invalid("light color/intensity/range"));
            }
            if let LightKind::Spot{inner,outer}=light.kind {
                if !inner.is_finite()||!outer.is_finite()||inner<0.||outer<=inner||outer>std::f64::consts::FRAC_PI_2 {
                    return Err(Error::Invalid("spot cone requires 0 <= inner < outer <= pi/2"));
                }
            }
        }
        for socket in self.sockets.values() {name(&socket.name,limits)?;socket.transform.validate()?;self.attachment(&socket.attachment,state)?;}
        // A binding on one of the four wheel connections is a road wheel; any
        // other connection is a generic DRIVEN PART (a control surface, a
        // gear leg, a canopy) posed by the host through the same contract.
        let is_wheel=|c:&str|VEHICLE_WHEEL_CONNECTIONS.contains(&c);
        if self.wheels.values().filter(|w|is_wheel(&w.connection)).count()>4{return Err(Error::Budget("vehicle wheel bindings"));}
        if self.wheels.values().filter(|w|!is_wheel(&w.connection)).count()>MAX_DRIVEN_PARTS{return Err(Error::Budget("driven part bindings"));}
        let mut connections=BTreeSet::new();
        for (object,wheel) in &self.wheels {
            ctx.checkpoint(1)?;name(object,limits)?;
            if !state.objects.contains_key(object){return Err(Error::MissingObject(object.clone()));}
            if wheel.connection.starts_with("wheel_")&&!is_wheel(&wheel.connection){return Err(Error::Invalid("unknown vehicle wheel connection"));}
            name(&wheel.connection,limits)?;
            if !connections.insert(&wheel.connection){return Err(Error::Invalid("duplicate vehicle wheel or driven part connection"));}
            if wheel.pivot.iter().any(|v|!v.is_finite()||v.abs()>1e6)||[wheel.radius,wheel.width].iter().any(|v|!v.is_finite()||(*v as f32)<=0.||*v>10000.){return Err(Error::Invalid("wheel requires finite pivot and positive bounded renderable radius/width"));}
            if wheel.visual.is_some_and(|visual|!visual.is_valid()){return Err(Error::Invalid("wheel visual requires steer_gain 0..1, steer_max 0..1.2 radians, compression/droop 0..5 model metres"));}
            let world=self.world_matrix(object)?;let scale=world[0][0];
            if scale<=0.||(0..3).any(|r|(0..3).any(|c|(world[r][c]-if r==c{scale}else{0.}).abs()>1e-8*scale.max(1.))){return Err(Error::Invalid("wheel nodes require positive uniform scale and no rotation; rotate axle geometry onto local X before binding"));}
            let mut parent=self.nodes.get(object).and_then(|n|n.parent.as_deref());
            while let Some(p)=parent{if self.wheels.contains_key(p){return Err(Error::Invalid("wheel geometry cannot contain another wheel binding"));}parent=self.nodes.get(p).and_then(|n|n.parent.as_deref());}
        }
        for (object,levels) in &self.lods {
            if !state.objects.contains_key(object)||levels.is_empty()||levels.len()>4 {return Err(Error::Invalid("LOD object/levels"));}
            let mut prev=(usize::MAX,0.);
            for level in levels {
                if level.target_faces==0||level.target_faces>=prev.0||!level.distance.is_finite()||level.distance<=prev.1||level.distance>1e6||!level.max_error.is_finite()||level.max_error<0. {return Err(Error::Invalid("LOD order or error"));}
                prev=(level.target_faces,level.distance);
            }
        }
        for (object,proxy) in &self.colliders {
            if !state.objects.contains_key(object) {return Err(Error::MissingObject(object.clone()));}
            if let CollisionProxy::Mesh{object}=proxy {if !state.objects.contains_key(object) {return Err(Error::MissingObject(object.clone()));}}
        }
        if !self.modifiers.is_empty(){self.evaluated_meshes(state,ctx)?;}
        Ok(())
    }
    fn attachment(&self,a:&Attachment,state:&State)->Result<()> {
        match a {Attachment::Root=>Ok(()),Attachment::Object(n) if state.objects.contains_key(n)=>Ok(()),
            Attachment::Joint(j) if state.skeleton.as_ref().is_some_and(|s|(*j as usize)<s.joints.len())=>Ok(()),
            _=>Err(Error::Invalid("unknown attachment target"))}
    }
    fn visit(&self,n:&str,visiting:&mut BTreeSet<String>,done:&mut BTreeSet<String>,ctx:&mut mesh::Context<'_>)->Result<()> {
        ctx.checkpoint(1)?;if done.contains(n){return Ok(());}if !visiting.insert(n.into()){return Err(Error::Invalid("scene dependency cycle"));}
        if let Some(node)=self.nodes.get(n){for source in node.parent.iter().chain(&node.linked_to){self.visit(source,visiting,done,ctx)?;}}
        for modifier in self.modifiers.get(n).into_iter().flatten().filter(|m|m.enabled){for source in modifier.operation.dependencies(){self.visit(source,visiting,done,ctx)?;}}
        visiting.remove(n);done.insert(n.into());Ok(())
    }
    pub fn world_matrix(&self,object:&str)->Result<Matrix4> {
        let mut chain=Vec::new();let mut seen=BTreeSet::new();let mut next=Some(object);
        while let Some(n)=next {if !seen.insert(n){return Err(Error::Invalid("object parent cycle"));}
            if let Some(node)=self.nodes.get(n){chain.push(node.transform.matrix()?);next=node.parent.as_deref();}else{break;}}
        Ok(chain.into_iter().rev().fold(IDENTITY_MATRIX,matrix_mul))
    }
    pub(crate) fn local_mesh(&self,state:&State,object:&str,ctx:&mut mesh::Context<'_>)->Result<mesh::Mesh> {
        let mut cache=BTreeMap::new();let mesh=self.evaluate_one(state,object,&mut cache,&mut BTreeSet::new(),ctx)?;
        if cache_bytes(&cache).saturating_add(mesh.memory_bytes())>ctx.limits.max_bytes{return Err(Error::Budget("evaluated mesh copy"));}Ok((*mesh).clone())
    }
    /// One cache per evaluation; instances and shrinkwrap references share an
    /// immutable result. No cache survives a document revision.
    pub(crate) fn evaluated_meshes(&self,state:&State,ctx:&mut mesh::Context<'_>)->Result<BTreeMap<String,Arc<mesh::Mesh>>>{
        let mut cache=BTreeMap::new();let mut visiting=BTreeSet::new();for object in state.objects.keys(){self.evaluate_one(state,object,&mut cache,&mut visiting,ctx)?;}Ok(cache)
    }
    fn evaluate_one(&self,state:&State,object:&str,cache:&mut BTreeMap<String,Arc<mesh::Mesh>>,visiting:&mut BTreeSet<String>,ctx:&mut mesh::Context<'_>)->Result<Arc<mesh::Mesh>>{
        ctx.checkpoint(1)?;if let Some(mesh)=cache.get(object){return Ok(mesh.clone());}if !visiting.insert(object.into()){return Err(Error::Invalid("modifier/instance dependency cycle"));}
        let original=state.objects.get(object).ok_or_else(||Error::MissingObject(object.into()))?;
        let base=if let Some(source)=self.nodes.get(object).and_then(|n|n.linked_to.as_deref()){self.evaluate_one(state,source,cache,visiting,ctx)?}else{
            if cache_bytes(cache).saturating_add(original.memory_bytes().saturating_mul(3))>ctx.limits.max_bytes{return Err(Error::Budget("modifier source copies"));}Arc::new(original.clone())};
        let mut result=base;for modifier in self.modifiers.get(object).into_iter().flatten().filter(|m|m.enabled){ctx.checkpoint(1)?;
            let mut dependencies=BTreeMap::new();for dependency in modifier.operation.dependencies(){let reference=self.evaluate_one(state,dependency,cache,visiting,ctx)?;dependencies.insert(dependency.to_owned(),reference);}
            let required=cache_bytes(cache).saturating_add(result.memory_bytes().saturating_mul(3)).saturating_add(dependencies.values().map(|m|m.memory_bytes().saturating_mul(3)).sum::<usize>());
            if required>ctx.limits.max_bytes{return Err(Error::Budget("modifier evaluation working set"));}
            let operation=modifier.operation.for_modifier(object,&result)?;let mut objects=BTreeMap::new();
            let owner_inverse=inverse(self.world_matrix(object)?)?;
            for (name,reference) in dependencies {let mut reference=(*reference).clone();let transform=matrix_mul(owner_inverse,self.world_matrix(&name)?);let vertices=reference.vertices().iter().map(|v|v.id).collect::<Vec<_>>();reference.transform(&vertices,transform,ctx)?;objects.insert(name,reference);}
            objects.insert(object.into(),(*result).clone());
            let limits=Limits{mesh:ctx.limits.clone(),..Limits::default()};operation.apply(&mut objects,&limits,ctx)?;result=Arc::new(objects.remove(object).unwrap());
        }
        visiting.remove(object);if cache_bytes(cache).saturating_add(result.memory_bytes())>ctx.limits.max_bytes{return Err(Error::Budget("modifier evaluation cache"));}cache.insert(object.into(),result.clone());Ok(result)
    }
    pub(crate) fn apply(op:&SceneOperation,state:&mut State,limits:&Limits,ctx:&mut mesh::Context<'_>)->Result<OperationResult> {
        let mut result=OperationResult::default();
        match op {
            SceneOperation::Node{object,node}=>{name(object,limits)?;state.scene.nodes.insert(object.clone(),node.clone());}
            SceneOperation::Instance{object,source,transform}=>{
                create_name(state,object,limits)?;if !state.objects.contains_key(source){return Err(Error::MissingObject(source.clone()));}
                state.objects.insert(object.clone(),mesh::Mesh::new());
                state.scene.nodes.insert(object.clone(),SceneNode{parent:None,linked_to:Some(source.clone()),transform:*transform});
            }
            SceneOperation::Duplicate{object,source}=>{
                create_name(state,object,limits)?;let mesh=state.scene.local_mesh(state,source,ctx)?;state.objects.insert(object.clone(),mesh);
                let mut node=state.scene.nodes.get(source).cloned().unwrap_or_default();node.linked_to=None;state.scene.nodes.insert(object.clone(),node);
            }
            SceneOperation::MakeUnique{object}=>{
                let mesh=state.scene.local_mesh(state,object,ctx)?;state.objects.insert(object.clone(),mesh);
                state.scene.nodes.entry(object.clone()).or_default().linked_to=None;
                state.scene.modifiers.remove(object);
            }
            SceneOperation::Join{object,sources}=>{
                create_name(state,object,limits)?;
                if sources.is_empty()||sources.len()>limits.max_objects {return Err(Error::Budget("join objects"));}
                let mut joined=mesh::Mesh::new();let mut unique=BTreeSet::new();let evaluated=state.scene.evaluated_meshes(state,ctx)?;
                for source in sources {
                    if !unique.insert(source){return Err(Error::Invalid("duplicate join source"));}
                    let mut mesh=(**evaluated.get(source).ok_or_else(||Error::MissingObject(source.clone()))?).clone();let ids=mesh.vertices().iter().map(|v|v.id).collect::<Vec<_>>();
                    mesh.transform(&ids,state.scene.world_matrix(source)?,ctx)?;
                    joined.append(&mesh,ctx)?;
                }
                state.objects.insert(object.clone(),joined);
            }
            SceneOperation::Separate{object,name:target,faces}=>{
                create_name(state,target,limits)?;
                if state.scene.nodes.get(object).is_some_and(|n|n.linked_to.is_some())||state.scene.modifiers.get(object).is_some_and(|m|!m.is_empty()){return Err(Error::Invalid("make geometry unique and apply modifiers before separating faces"));}
                let source=state.objects.get_mut(object).ok_or_else(||Error::MissingObject(object.clone()))?;let(separated,_)=source.separate_faces(faces,ctx)?;state.objects.insert(target.clone(),separated);
                let mut node=state.scene.nodes.get(object).cloned().unwrap_or_default();node.linked_to=None;state.scene.nodes.insert(target.clone(),node);
            }
            SceneOperation::Pivot{object,position}=>{
                if position.iter().any(|v|!v.is_finite()){return Err(Error::Invalid("pivot position"));}
                if state.scene.nodes.get(object).is_some_and(|n|n.linked_to.is_some()) || state.scene.nodes.values().any(|n|n.linked_to.as_deref()==Some(object)) || state.scene.modifiers.get(object).is_some_and(|m|!m.is_empty()) {
                    return Err(Error::Invalid("make linked geometry unique and apply modifiers before changing its pivot"));
                }
                let geometry=state.objects.get_mut(object).ok_or_else(||Error::MissingObject(object.clone()))?;
                let vertices=geometry.vertices().iter().map(|v|v.id).collect::<Vec<_>>();
                let mut offset=IDENTITY_MATRIX;for i in 0..3{offset[i][3]=-position[i];}
                geometry.transform(&vertices,offset,ctx)?;
                let node=state.scene.nodes.entry(object.clone()).or_default();
                node.transform.translation=transform_point(node.transform.matrix()?,*position);
                for child in state.scene.nodes.values_mut().filter(|n|n.parent.as_deref()==Some(object)){child.transform.translation=sub(child.transform.translation,*position);}
                for light in state.scene.emitters.values_mut().filter(|l|l.attachment==Attachment::Object(object.clone())){light.transform.translation=sub(light.transform.translation,*position);}
                for socket in state.scene.sockets.values_mut().filter(|s|s.attachment==Attachment::Object(object.clone())){socket.transform.translation=sub(socket.transform.translation,*position);}
                if let Some(wheel)=state.scene.wheels.get_mut(object){wheel.pivot=sub(wheel.pivot,*position);}
                result.vertices=vertices;
            }
            SceneOperation::Snap{object,grid}=>{
                if !grid.is_finite()||*grid<=0. {return Err(Error::Invalid("snap grid"));}
                let node=state.scene.nodes.entry(object.clone()).or_default();node.transform.translation=node.transform.translation.map(|v|(v/grid).round()*grid);
            }
            SceneOperation::Dimensions{object,size}=>{
                if size.iter().any(|v|!v.is_finite()||*v<=0.){return Err(Error::Invalid("requested dimensions"));}
                let mesh=state.scene.local_mesh(state,object,ctx)?;let mut lo=[f64::INFINITY;3];let mut hi=[f64::NEG_INFINITY;3];
                for v in mesh.vertices(){ctx.checkpoint(1)?;for d in 0..3 {lo[d]=lo[d].min(v.position[d]);hi[d]=hi[d].max(v.position[d]);}}
                if (0..3).any(|d|hi[d]-lo[d]<1e-12){return Err(Error::Invalid("dimensions require nonzero local extent"));}
                state.scene.nodes.entry(object.clone()).or_default().transform.scale=std::array::from_fn(|d|size[d]/(hi[d]-lo[d]));
            }
            SceneOperation::Light(light)=>{state.scene.emitters.insert(light.name.clone(),light.clone());}
            SceneOperation::DeleteLight{name}=>{if state.scene.emitters.remove(name).is_none(){return Err(Error::Invalid("unknown light"));}}
            SceneOperation::Socket(socket)=>{state.scene.sockets.insert(socket.name.clone(),socket.clone());}
            SceneOperation::DeleteSocket{name}=>{if state.scene.sockets.remove(name).is_none(){return Err(Error::Invalid("unknown socket"));}}
            SceneOperation::VehicleWheel{object,wheel}=>{state.scene.wheels.insert(object.clone(),wheel.clone());}
            SceneOperation::DeleteVehicleWheel{object}=>{if state.scene.wheels.remove(object).is_none(){return Err(Error::Invalid("unknown vehicle wheel binding"));}}
            SceneOperation::Lods{object,levels}=>{state.scene.lods.insert(object.clone(),levels.clone());}
            SceneOperation::Collider{object,proxy}=>{state.scene.colliders.insert(object.clone(),proxy.clone());}
            SceneOperation::Modifier{object,modifier}=>{let stack=state.scene.modifiers.entry(object.clone()).or_default();if let Some(old)=stack.iter_mut().find(|m|m.name==modifier.name){*old=modifier.clone();}else{if stack.len()>=16{return Err(Error::Budget("object modifiers"));}stack.push(modifier.clone());}}
            SceneOperation::RemoveModifier{object,name}=>{let stack=state.scene.modifiers.get_mut(object).ok_or(Error::Invalid("unknown modifier"))?;let index=stack.iter().position(|m|&m.name==name).ok_or(Error::Invalid("unknown modifier"))?;stack.remove(index);if stack.is_empty(){state.scene.modifiers.remove(object);}}
            SceneOperation::ModifierOrder{object,names}=>{let stack=state.scene.modifiers.get_mut(object).ok_or(Error::Invalid("unknown modifier stack"))?;
                if names.len()!=stack.len()||names.iter().collect::<BTreeSet<_>>().len()!=names.len(){return Err(Error::Invalid("modifier order must be an exact permutation"));}
                let reordered=names.iter().map(|name|stack.iter().find(|m|&m.name==name).cloned().ok_or(Error::Invalid("unknown modifier in order"))).collect::<Result<Vec<_>>>()?;*stack=reordered;}
            SceneOperation::ApplyModifiers{object}=>{let mesh=state.scene.local_mesh(state,object,ctx)?;state.objects.insert(object.clone(),mesh);state.scene.modifiers.remove(object);state.scene.nodes.entry(object.clone()).or_default().linked_to=None;}
        }
        if let Some(object)=op.object_name(){result.object=object.into();if let Some(m)=state.objects.get(object){result.vertices=m.vertices().iter().map(|v|v.id).collect();result.faces=m.faces().iter().map(|f|f.id).collect();}}
        // Cross-object invariants are checked at the transaction boundary, so
        // a batch can remove/reparent attachments alongside their objects.
        Ok(result)
    }
}

#[cfg(test)]
mod modifier_tests {
    use super::*;
    use crate::{Document,Operation,Transaction,Action};
    fn apply(doc:&mut Document,id:&str,operations:Vec<Operation>)->crate::Applied{doc.apply(Transaction{request_id:id.into(),expected:doc.head(),operations},None).unwrap()}
    fn maximum_x(mesh:&mesh::Mesh)->f64{mesh.vertices().iter().map(|v|v.position[0]).fold(f64::NEG_INFINITY,f64::max)}
    #[test]
    fn ordered_modifiers_preserve_base_roundtrip_and_bake_once(){
        let mut doc=Document::new(Limits::default()).unwrap();apply(&mut doc,"cube",vec![
        Operation::Cube { object: "body".into(), size: [2.0, 2.0, 2.0] },
    ]);
        apply(&mut doc,"stack",vec![
        Operation::Scene(SceneOperation::Modifier {
            object: "body".into(),
            modifier: NamedModifier {
                name: "copies".into(),
                enabled: true,
                operation: MeshEditingOperation {
                    object: "body".into(),
                    action: Action::Array { count: 2, offset: [3.0, 0.0, 0.0] }
                }
            }
        }),
        Operation::Scene(SceneOperation::Modifier {
            object: "body".into(),
            modifier: NamedModifier {
                name: "scale".into(),
                enabled: true,
                operation: MeshEditingOperation {
                    object: "body".into(),
                    action: Action::Deform {
                        vertices: vec![],
                        deformation: mesh::Deformation::Taper { axis: 1, range: [-1.0, 1.0], scales: [2.0, 2.0] }
                    }
                }
            }
        }),
    ]);
        assert_eq!(doc.object("body").unwrap().vertices().len(),8);let mut ctx=mesh::Context::default();let evaluated=doc.scene().local_mesh(&doc.state,"body",&mut ctx).unwrap();assert_eq!(evaluated.vertices().len(),16);assert_eq!(maximum_x(&evaluated),8.);
        let bytes=doc.to_bytes(None).unwrap();let restored=Document::from_bytes(&bytes,Limits::default(),None).unwrap();assert_eq!(restored.scene().local_mesh(&restored.state,"body",&mut ctx).unwrap(),evaluated);
        apply(&mut doc,"order",vec![
        Operation::Scene(SceneOperation::ModifierOrder { object: "body".into(), names: vec!["scale".into(), "copies".into()] }),
    ]);let evaluated=doc.scene().local_mesh(&doc.state,"body",&mut ctx).unwrap();assert_eq!(maximum_x(&evaluated),5.);
        apply(&mut doc,"bake",vec![
        Operation::Scene(SceneOperation::ApplyModifiers { object: "body".into() }),
    ]);assert!(doc.scene().modifiers.is_empty());assert_eq!(doc.object("body").unwrap(),&evaluated);assert_eq!(doc.scene().local_mesh(&doc.state,"body",&mut ctx).unwrap(),evaluated);
    }
    #[test]
    fn instance_cache_shares_evaluation_and_dependency_cycles_roll_back(){
        let mut doc=Document::new(Limits::default()).unwrap();apply(&mut doc,"seed",vec![
        Operation::Cube { object: "a".into(), size: [2.0, 2.0, 2.0] },
        Operation::Scene(SceneOperation::Instance { object: "b".into(), source: "a".into(), transform: Transform::default() }),
        Operation::Scene(SceneOperation::Modifier {
            object: "a".into(),
            modifier: NamedModifier {
                name: "copies".into(),
                enabled: true,
                operation: MeshEditingOperation {
                    object: "a".into(),
                    action: Action::Array { count: 2, offset: [3.0, 0.0, 0.0] }
                }
            }
        }),
    ]);
        let cache=doc.scene().evaluated_meshes(&doc.state,&mut mesh::Context::default()).unwrap();assert!(Arc::ptr_eq(&cache["a"],&cache["b"]));assert_eq!(cache["b"].vertices().len(),16);
        let before=doc.to_bytes(None).unwrap();let ops=vec![
        Operation::Scene(SceneOperation::Modifier {
            object: "a".into(),
            modifier: NamedModifier {
                name: "wrap".into(),
                enabled: true,
                operation: MeshEditingOperation {
                    object: "a".into(),
                    action: Action::Shrinkwrap {
                        vertices: vec![],
                        reference: "b".into(),
                        max_distance: 10.0,
                        offset: 0.0
                    }
                }
            }
        }),
    ];
        assert!(doc.apply(Transaction{request_id:"cycle".into(),expected:doc.head(),operations:ops},None).is_err());assert_eq!(doc.to_bytes(None).unwrap(),before);
        apply(&mut doc,"unique",vec![
        Operation::Scene(SceneOperation::MakeUnique { object: "b".into() }),
    ]);assert_eq!(doc.object("b").unwrap().vertices().len(),16);assert!(doc.scene().nodes["b"].linked_to.is_none());
    }
    #[test]
    fn shrinkwrap_modifier_projects_between_object_coordinate_spaces(){
        let mut doc=Document::new(Limits::default()).unwrap();
        apply(&mut doc,"seed",vec![
        Operation::Plane { object: "cage".into(), size: [2.0, 2.0] },
        Operation::Plane { object: "reference".into(), size: [8.0, 8.0] },
        Operation::Scene(SceneOperation::Node {
            object: "cage".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.0, 3.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 2.0, 1.0]
                }
            }
        }),
        Operation::Scene(SceneOperation::Node {
            object: "reference".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.0, 1.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Scene(SceneOperation::Modifier {
            object: "cage".into(),
            modifier: NamedModifier {
                name: "wrap".into(),
                enabled: true,
                operation: MeshEditingOperation {
                    object: "cage".into(),
                    action: Action::Shrinkwrap {
                        vertices: vec![],
                        reference: "reference".into(),
                        max_distance: 2.0,
                        offset: 0.0
                    }
                }
            }
        }),
    ]);
        let mesh=doc.scene().local_mesh(&doc.state,"cage",&mut mesh::Context::default()).unwrap();
        assert!(mesh.vertices().iter().all(|v|(v.position[1]+1.).abs()<1e-12));
        assert!(doc.object("cage").unwrap().vertices().iter().all(|v|v.position[1]==0.));
        let before=doc.to_bytes(None).unwrap();let stop=||true;let mut ctx=mesh::Context::new(mesh::Limits::default(),Some(&stop));assert!(doc.scene().evaluated_meshes(&doc.state,&mut ctx).is_err());assert_eq!(doc.to_bytes(None).unwrap(),before);
    }
    #[test]
    fn join_and_separate_keep_split_normals_edge_markers_pins_and_ids(){
        let mut ctx=mesh::Context::default();let mut source=mesh::Mesh::cube([2.;3],&mut ctx).unwrap();source.recalculate_normals(false,0.,&mut ctx).unwrap();let face=source.faces()[0].id;let cs=source.face_corners(face).unwrap();let pin=cs[0].id;let edge=mesh::EdgeKey::new(cs[0].vertex,cs[1].vertex);
        source.pin_uv(&[pin],true,&mut ctx).unwrap();source.set_edge_attributes(edge,mesh::EdgeAttributes{seam:true,crease:0.5},&mut ctx).unwrap();
        let mut doc=Document::new(Limits::default()).unwrap();doc.apply(Transaction{request_id:"source".into(),expected:doc.head(),operations:vec![Operation::ImportMesh{object:"a".into(),source:source.to_bytes(&mut ctx).unwrap()}]},None).unwrap();
        apply(&mut doc,"join",vec![
        Operation::Scene(SceneOperation::Join { object: "joined".into(), sources: vec!["a".into()] }),
    ]);let joined=doc.object("joined").unwrap();assert_eq!(joined.uv_pins().len(),1);assert_eq!(joined.edge_attributes().values().next().unwrap().attributes,mesh::EdgeAttributes{seam:true,crease:0.5});assert!(joined.corners().iter().all(|c|c.normal.is_some()));
        let op=SceneOperation::Separate{object:"a".into(),name:"cap".into(),faces:vec![face]};doc.apply(Transaction{request_id:"separate".into(),expected:doc.head(),operations:vec![Operation::Scene(op)]},None).unwrap();
        let cap=doc.object("cap").unwrap();assert_eq!(cap.faces()[0].id,face);assert!(cap.uv_pins().contains(&pin));assert_eq!(cap.edge_attributes()[&edge].attributes,mesh::EdgeAttributes{seam:true,crease:0.5});assert!(cap.corners().iter().all(|c|c.normal.is_some()));assert_eq!(cap.vertices().len(),4);
        let bytes=doc.to_bytes(None).unwrap();assert_eq!(Document::from_bytes(&bytes,Limits::default(),None).unwrap().object("cap"),Some(cap));
    }
}
fn create_name(state:&State,object:&str,limits:&Limits)->Result<()> {
    name(object,limits)?;if state.objects.contains_key(object){return Err(Error::DuplicateObject(object.into()));}
    if state.objects.len()>=limits.max_objects{Err(Error::Budget("objects"))}else{Ok(())}
}
fn cache_bytes(cache:&BTreeMap<String,Arc<mesh::Mesh>>)->usize{let mut seen=BTreeSet::new();cache.values().filter(|m|seen.insert(Arc::as_ptr(m)as usize)).fold(0usize,|n,m|n.saturating_add(m.memory_bytes()))}

fn attachment_value(a:&Attachment)->Value {match a {Attachment::Root=>Value::Null,
    Attachment::Object(n)=>json::obj(vec![("object",json::s(n))]),Attachment::Joint(j)=>json::obj(vec![("joint",Value::Int(*j as i64))])}}
fn node_value(n:&SceneNode)->Value {json::obj(vec![("parent",n.parent.as_ref().map(json::s).unwrap_or(Value::Null)),
    ("linked_to",n.linked_to.as_ref().map(json::s).unwrap_or(Value::Null)),("transform",transform_value(&n.transform))])}
impl SceneOperation {
    pub fn object_name(&self)->Option<&str>{match self {Self::Node{object,..}|Self::Instance{object,..}|Self::Duplicate{object,..}
        |Self::MakeUnique{object}|Self::Join{object,..}|Self::Separate{object,..}|Self::Pivot{object,..}|Self::Snap{object,..}|Self::Dimensions{object,..}
        |Self::Lods{object,..}|Self::Collider{object,..}|Self::Modifier{object,..}|Self::RemoveModifier{object,..}|Self::ModifierOrder{object,..}|Self::ApplyModifiers{object}
        |Self::VehicleWheel{object,..}|Self::DeleteVehicleWheel{object}=>Some(object),_=>None}}
    pub fn memory_bytes(&self)->usize {let mut w=Writer::new(usize::MAX);if self.write(&mut w).is_err(){usize::MAX}else{w.bytes.len().saturating_mul(3)}}
    pub fn check(&self,limits:&Limits)->Result<()> {
        match self {
            Self::Node{node,..}=>node.transform.validate()?,
            Self::Instance{transform,..}=>transform.validate()?,
            Self::Join{sources,..}=>check_count(sources.len(),limits.max_objects)?,
            Self::Separate{faces,..}=>if faces.len()>limits.mesh.max_faces{return Err(Error::Budget("selection size"));},
            Self::Snap{grid,..}=>finite(&[*grid])?,
            Self::Pivot{position,..}=>finite(position)?,
            Self::Dimensions{size,..}=>finite(size)?,
            Self::Light(light)=>check_light(light)?,
            Self::Socket(socket)=>socket.transform.validate()?,
            Self::Lods{levels,..}=>check_lods(levels)?,
            Self::Modifier{object,modifier}=>check_modifier(object,modifier,limits)?,
            Self::ModifierOrder{names,..}=>check_count(names.len(),16)?,
            Self::VehicleWheel{wheel,..}=>check_wheel(wheel)?,
            _=>{}
        }
        if let Some(object)=self.object_name(){name(object,limits)?;}Ok(())
    }
    pub fn value(&self)->Value {
        let mut f=match self {
            Self::Modifier{object,modifier}=>vec![("op",json::s("modifier")),("object",json::s(object)),("name",json::s(&modifier.name)),("enabled",Value::Bool(modifier.enabled)),("operation",modifier.operation.value())],
            Self::RemoveModifier{object,name}=>vec![("op",json::s("remove_modifier")),("object",json::s(object)),("name",json::s(name))],
            Self::ModifierOrder{object,names}=>vec![("op",json::s("modifier_order")),("object",json::s(object)),("names",Value::Arr(names.iter().map(json::s).collect()))],
            Self::ApplyModifiers{object}=>vec![("op",json::s("apply_modifiers")),("object",json::s(object))],
            Self::Node{object,node}=>vec![("op",json::s("object_node")),("object",json::s(object)),("node",node_value(node))],
            Self::Instance{object,source,transform}=>vec![("op",json::s("instance")),("object",json::s(object)),("source",json::s(source)),("transform",transform_value(transform))],
            Self::Duplicate{object,source}=>vec![("op",json::s("duplicate")),("object",json::s(object)),("source",json::s(source))],
            Self::MakeUnique{object}=>vec![("op",json::s("make_unique")),("object",json::s(object))],
            Self::Join{object,sources}=>vec![("op",json::s("join")),("object",json::s(object)),("sources",Value::Arr(sources.iter().map(json::s).collect()))],
            Self::Separate{object,name,faces}=>vec![("op",json::s("separate")),("object",json::s(object)),("name",json::s(name)),("faces",Value::Arr(faces.iter().map(|id|json::s(id.0.to_string())).collect()))],
            Self::Pivot{object,position}=>vec![("op",json::s("pivot")),("object",json::s(object)),("position",vec_value(position))],
            Self::Snap{object,grid}=>vec![("op",json::s("snap")),("object",json::s(object)),("grid",Value::F64(*grid))],
            Self::Dimensions{object,size}=>vec![("op",json::s("dimensions")),("object",json::s(object)),("size",vec_value(size))],
            Self::Light(l)=>vec![("op",json::s("light")),("name",json::s(&l.name)),("attachment",attachment_value(&l.attachment)),("transform",transform_value(&l.transform)),
                ("kind",json::s(match l.kind{LightKind::Point=>"point",LightKind::Spot{..}=>"spot"})),("color",vec_value(&l.color)),("intensity",Value::F64(l.intensity)),("range",Value::F64(l.range))],
            Self::DeleteLight{name}=>vec![("op",json::s("delete_light")),("name",json::s(name))],
            Self::Socket(s)=>vec![("op",json::s("socket")),("name",json::s(&s.name)),("attachment",attachment_value(&s.attachment)),("transform",transform_value(&s.transform))],
            Self::DeleteSocket{name}=>vec![("op",json::s("delete_socket")),("name",json::s(name))],
            Self::VehicleWheel{object,wheel}=>vec![("op",json::s("vehicle_wheel")),("object",json::s(object)),("connection",json::s(&wheel.connection)),("pivot",vec_value(&wheel.pivot)),("radius",Value::F64(wheel.radius)),("width",Value::F64(wheel.width))],
            Self::DeleteVehicleWheel{object}=>vec![("op",json::s("delete_vehicle_wheel")),("object",json::s(object))],
            Self::Lods{object,levels}=>vec![("op",json::s("lods")),("object",json::s(object)),("levels",Value::Arr(levels.iter().map(|l|json::obj(vec![
                ("target_faces",Value::Int(l.target_faces as i64)),("max_error",Value::F64(l.max_error)),("distance",Value::F64(l.distance))])).collect()))],
            Self::Collider{object,proxy}=>vec![("op",json::s("collider")),("object",json::s(object)),("kind",json::s(match proxy{CollisionProxy::Box=>"box",CollisionProxy::Sphere=>"sphere",CollisionProxy::Mesh{..}=>"mesh",CollisionProxy::Hull=>"hull",CollisionProxy::Capsule=>"capsule"}))],
        };
        if let Self::Light(LightEmitter{kind:LightKind::Spot{inner,outer},..})=self{f.push(("inner",Value::F64(*inner)));f.push(("outer",Value::F64(*outer)));}
        if let Self::Collider{proxy:CollisionProxy::Mesh{object},..}=self{f.push(("source",json::s(object)));}
        if let Self::VehicleWheel{wheel:VehicleWheel{visual:Some(visual),..},..}=self{f.push(("visual",json::obj(vec![
            ("steer_gain",Value::F64(visual.steer_gain)),("steer_max",Value::F64(visual.steer_max)),
            ("compression",Value::F64(visual.compression)),("droop",Value::F64(visual.droop))])));}
        json::obj(f)
    }
}
impl SceneState {
    pub fn check(&self,limits:&Limits)->Result<()> {
        if self.nodes.len()>limits.max_objects||self.emitters.len()>32||self.sockets.len()>128||self.lods.len()>limits.max_objects||self.colliders.len()>limits.max_objects||self.modifiers.len()>limits.max_objects||self.modifiers.values().map(Vec::len).sum::<usize>()>128 {
            return Err(Error::Budget("scene attachments"));
        }
        if self.wheels.len()>4+MAX_DRIVEN_PARTS{return Err(Error::Budget("driven part bindings"));}
        for (object,node) in &self.nodes{name(object,limits)?;node.transform.validate()?;}
        for light in self.emitters.values(){check_light(light)?;}
        for socket in self.sockets.values(){socket.transform.validate()?;}
        for (object,levels) in &self.lods{name(object,limits)?;check_lods(levels)?;}
        for object in self.colliders.keys(){name(object,limits)?;}
        for (object,stack) in &self.modifiers{
            name(object,limits)?;if stack.is_empty()||stack.len()>16{return Err(Error::Budget("object modifier count"));}
            let mut seen=BTreeSet::new();for m in stack{check_modifier(object,m,limits)?;if !seen.insert(&m.name){return Err(Error::Corrupt("duplicate scene entry"));}}
        }
        for (object,wheel) in &self.wheels{name(object,limits)?;check_wheel(wheel)?;}
        Ok(())
    }
    pub(crate) fn write(&self,w:&mut Writer)->Result<()> {
        write_map(w,&self.nodes,write_node)?;
        write_map(w,&self.emitters,write_light)?;
        write_map(w,&self.sockets,write_socket)?;
        write_map(w,&self.lods,|w,levels|write_lods(w,levels))?;
        write_map(w,&self.colliders,write_proxy)?;
        write_map(w,&self.modifiers,|w,stack|{w.count(stack.len())?;for m in stack{write_modifier(w,m)?;}Ok(())})?;
        write_map(w,&self.wheels,write_wheel)
    }
    pub(crate) fn read(r:&mut Reader,limits:&Limits)->Result<Self>{
        let value=Self{
            nodes:read_map(r,limits.max_objects,limits.max_name_bytes,|r|read_node(r,limits))?,
            emitters:read_map(r,32,limits.max_source_bytes,|r|read_light(r,limits))?,
            sockets:read_map(r,128,limits.max_source_bytes,|r|read_socket(r,limits))?,
            lods:read_map(r,limits.max_objects,limits.max_name_bytes,read_lods)?,
            colliders:read_map(r,limits.max_objects,limits.max_name_bytes,|r|read_proxy(r,limits))?,
            modifiers:read_map(r,limits.max_objects,limits.max_name_bytes,|r|{let mut stack=Vec::new();for _ in 0..r.count(16)?{stack.push(read_modifier(r,limits)?);}Ok(stack)})?,
            wheels:read_map(r,4+MAX_DRIVEN_PARTS,limits.max_name_bytes,|r|read_wheel(r,limits))?,
        };value.check(limits)?;Ok(value)
    }
}

fn finite(v:&[f64])->Result<()>{if v.iter().any(|v|!v.is_finite()){Err(Error::Invalid("expected finite number"))}else{Ok(())}}
fn check_count(n:usize,max:usize)->Result<()>{if n>max{Err(Error::Budget("authoring array"))}else{Ok(())}}
fn check_light(light:&LightEmitter)->Result<()>{
    light.transform.validate()?;finite(&light.color)?;finite(&[light.intensity,light.range])?;
    if let LightKind::Spot{inner,outer}=light.kind{finite(&[inner,outer])?;}Ok(())
}
fn check_lods(levels:&[LodLevel])->Result<()>{check_count(levels.len(),4)?;for l in levels{
    if l.target_faces>u32::MAX as usize{return Err(Error::Invalid("expected u32 integer"));}finite(&[l.max_error,l.distance])?;
}Ok(())}
fn check_modifier(object:&str,m:&NamedModifier,limits:&Limits)->Result<()>{
    m.operation.check(limits)?;
    if m.operation.object_name()!=object||!m.operation.is_modifier_safe(){return Err(Error::Invalid("modifier requires its own object and no fixed element selections"));}
    name(&m.name,limits)
}
fn check_wheel(wheel:&VehicleWheel)->Result<()>{finite(&wheel.pivot)?;finite(&[wheel.radius,wheel.width])?;
    if let Some(v)=wheel.visual{finite(&[v.steer_gain,v.steer_max,v.compression,v.droop])?;}Ok(())}
fn write_array(w:&mut Writer,a:&[f64])->Result<()>{for v in a{w.f64(*v)?;}Ok(())}
fn read_array<const N:usize>(r:&mut Reader)->Result<[f64;N]>{let mut a=[0.;N];for v in &mut a{*v=r.f64()?;}Ok(a)}
fn write_transform(w:&mut Writer,t:&Transform)->Result<()>{write_array(w,&t.translation)?;write_array(w,&t.rotation)?;write_array(w,&t.scale)}
fn read_transform(r:&mut Reader)->Result<Transform>{Ok(Transform{translation:read_array(r)?,rotation:read_array(r)?,scale:read_array(r)?})}
fn flag(r:&mut Reader)->Result<bool>{match r.u8()?{0=>Ok(false),1=>Ok(true),_=>Err(Error::Corrupt("scene boolean"))}}
fn write_optional(w:&mut Writer,s:&Option<String>)->Result<()>{w.u8(s.is_some() as u8)?;if let Some(s)=s{w.string(s)?;}Ok(())}
fn read_optional(r:&mut Reader,l:&Limits)->Result<Option<String>>{Ok(if flag(r)?{Some(r.string(l.max_source_bytes)?)}else{None})}
fn write_node(w:&mut Writer,n:&SceneNode)->Result<()>{write_optional(w,&n.parent)?;write_optional(w,&n.linked_to)?;write_transform(w,&n.transform)}
fn read_node(r:&mut Reader,l:&Limits)->Result<SceneNode>{Ok(SceneNode{parent:read_optional(r,l)?,linked_to:read_optional(r,l)?,transform:read_transform(r)?})}
fn write_attachment(w:&mut Writer,a:&Attachment)->Result<()>{match a{
    Attachment::Root=>w.u8(0),Attachment::Object(n)=>{w.u8(1)?;w.string(n)},Attachment::Joint(j)=>{w.u8(2)?;w.u32(*j)}
}}
fn read_attachment(r:&mut Reader,l:&Limits)->Result<Attachment>{Ok(match r.u8()?{
    0=>Attachment::Root,1=>Attachment::Object(r.string(l.max_source_bytes)?),2=>Attachment::Joint(r.u32()?),_=>return Err(Error::Corrupt("scene attachment tag")),
})}
fn write_light(w:&mut Writer,l:&LightEmitter)->Result<()>{
    w.string(&l.name)?;write_attachment(w,&l.attachment)?;write_transform(w,&l.transform)?;
    match l.kind{LightKind::Point=>w.u8(0)?,LightKind::Spot{inner,outer}=>{w.u8(1)?;w.f64(inner)?;w.f64(outer)?;}}
    write_array(w,&l.color)?;w.f64(l.intensity)?;w.f64(l.range)
}
fn read_light(r:&mut Reader,l:&Limits)->Result<LightEmitter>{Ok(LightEmitter{
    name:r.string(l.max_source_bytes)?,attachment:read_attachment(r,l)?,transform:read_transform(r)?,
    kind:match r.u8()?{0=>LightKind::Point,1=>LightKind::Spot{inner:r.f64()?,outer:r.f64()?},_=>return Err(Error::Corrupt("light kind tag"))},
    color:read_array(r)?,intensity:r.f64()?,range:r.f64()?,
})}
fn write_socket(w:&mut Writer,s:&Socket)->Result<()>{w.string(&s.name)?;write_attachment(w,&s.attachment)?;write_transform(w,&s.transform)}
fn read_socket(r:&mut Reader,l:&Limits)->Result<Socket>{Ok(Socket{name:r.string(l.max_source_bytes)?,attachment:read_attachment(r,l)?,transform:read_transform(r)?})}
fn write_lods(w:&mut Writer,levels:&[LodLevel])->Result<()>{w.count(levels.len())?;for l in levels{w.count(l.target_faces)?;w.f64(l.max_error)?;w.f64(l.distance)?;}Ok(())}
fn read_lods(r:&mut Reader)->Result<Vec<LodLevel>>{let mut levels=Vec::new();for _ in 0..r.count(4)?{levels.push(LodLevel{target_faces:r.u32()? as usize,max_error:r.f64()?,distance:r.f64()?});}Ok(levels)}
fn write_proxy(w:&mut Writer,p:&CollisionProxy)->Result<()>{match p{
    CollisionProxy::Box=>w.u8(0),CollisionProxy::Sphere=>w.u8(1),CollisionProxy::Mesh{object}=>{w.u8(2)?;w.string(object)},CollisionProxy::Hull=>w.u8(3),CollisionProxy::Capsule=>w.u8(4),
}}
fn read_proxy(r:&mut Reader,l:&Limits)->Result<CollisionProxy>{Ok(match r.u8()?{
    0=>CollisionProxy::Box,1=>CollisionProxy::Sphere,2=>CollisionProxy::Mesh{object:r.string(l.max_source_bytes)?},3=>CollisionProxy::Hull,4=>CollisionProxy::Capsule,_=>return Err(Error::Corrupt("collision proxy tag")),
})}
fn write_modifier(w:&mut Writer,m:&NamedModifier)->Result<()>{w.string(&m.name)?;w.u8(m.enabled as u8)?;m.operation.write(w)}
fn read_modifier(r:&mut Reader,l:&Limits)->Result<NamedModifier>{Ok(NamedModifier{name:r.string(l.max_name_bytes)?,enabled:flag(r)?,operation:MeshEditingOperation::read(r,l)?})}
fn write_wheel(w:&mut Writer,v:&VehicleWheel)->Result<()>{
    w.string(&v.connection)?;write_array(w,&v.pivot)?;w.f64(v.radius)?;w.f64(v.width)?;w.u8(v.visual.is_some() as u8)?;
    if let Some(v)=v.visual{write_array(w,&[v.steer_gain,v.steer_max,v.compression,v.droop])?;}Ok(())
}
fn read_wheel(r:&mut Reader,l:&Limits)->Result<VehicleWheel>{Ok(VehicleWheel{
    connection:r.string(l.max_source_bytes)?,pivot:read_array(r)?,radius:r.f64()?,width:r.f64()?,
    visual:if flag(r)?{Some(VisualWheelMotion{steer_gain:r.f64()?,steer_max:r.f64()?,compression:r.f64()?,droop:r.f64()?})}else{None},
})}
fn write_map<T>(w:&mut Writer,map:&BTreeMap<String,T>,mut item:impl FnMut(&mut Writer,&T)->Result<()>)->Result<()>{
    w.count(map.len())?;for (key,value) in map{w.string(key)?;item(w,value)?;}Ok(())
}
fn read_map<T>(r:&mut Reader,max:usize,max_name:usize,mut item:impl FnMut(&mut Reader)->Result<T>)->Result<BTreeMap<String,T>>{
    let mut map=BTreeMap::new();for _ in 0..r.count(max)?{let key=r.string(max_name)?;
        if map.last_key_value().is_some_and(|(last,_)|last>=&key){return Err(Error::Corrupt("scene map order"));}map.insert(key,item(r)?);
    }Ok(map)
}
impl SceneOperation {
    pub(crate) fn write(&self,w:&mut Writer)->Result<()> {
        match self{
            Self::Node{object,node}=>{w.u8(0)?;w.string(object)?;write_node(w,node)?;}
            Self::Instance{object,source,transform}=>{w.u8(1)?;w.string(object)?;w.string(source)?;write_transform(w,transform)?;}
            Self::Duplicate{object,source}=>{w.u8(2)?;w.string(object)?;w.string(source)?;}
            Self::MakeUnique{object}=>{w.u8(3)?;w.string(object)?;}
            Self::Join{object,sources}=>{w.u8(4)?;w.string(object)?;w.count(sources.len())?;for s in sources{w.string(s)?;}}
            Self::Separate{object,name,faces}=>{w.u8(5)?;w.string(object)?;w.string(name)?;w.count(faces.len())?;for f in faces{w.u64(f.0)?;}}
            Self::Snap{object,grid}=>{w.u8(6)?;w.string(object)?;w.f64(*grid)?;}
            Self::Pivot{object,position}=>{w.u8(7)?;w.string(object)?;write_array(w,position)?;}
            Self::Dimensions{object,size}=>{w.u8(8)?;w.string(object)?;write_array(w,size)?;}
            Self::Light(light)=>{w.u8(9)?;write_light(w,light)?;}
            Self::DeleteLight{name}=>{w.u8(10)?;w.string(name)?;}
            Self::Socket(socket)=>{w.u8(11)?;write_socket(w,socket)?;}
            Self::DeleteSocket{name}=>{w.u8(12)?;w.string(name)?;}
            Self::Lods{object,levels}=>{w.u8(13)?;w.string(object)?;write_lods(w,levels)?;}
            Self::Collider{object,proxy}=>{w.u8(14)?;w.string(object)?;write_proxy(w,proxy)?;}
            Self::Modifier{object,modifier}=>{w.u8(15)?;w.string(object)?;write_modifier(w,modifier)?;}
            Self::RemoveModifier{object,name}=>{w.u8(16)?;w.string(object)?;w.string(name)?;}
            Self::ModifierOrder{object,names}=>{w.u8(17)?;w.string(object)?;w.count(names.len())?;for n in names{w.string(n)?;}}
            Self::ApplyModifiers{object}=>{w.u8(18)?;w.string(object)?;}
            Self::VehicleWheel{object,wheel}=>{w.u8(19)?;w.string(object)?;write_wheel(w,wheel)?;}
            Self::DeleteVehicleWheel{object}=>{w.u8(20)?;w.string(object)?;}
        }Ok(())
    }
    pub(crate) fn read(r:&mut Reader,l:&Limits)->Result<Self>{
        let value=match r.u8()?{
            0=>Self::Node{object:r.string(l.max_name_bytes)?,node:read_node(r,l)?},
            1=>Self::Instance{object:r.string(l.max_name_bytes)?,source:r.string(l.max_source_bytes)?,transform:read_transform(r)?},
            2=>Self::Duplicate{object:r.string(l.max_name_bytes)?,source:r.string(l.max_source_bytes)?},
            3=>Self::MakeUnique{object:r.string(l.max_name_bytes)?},
            4=>{let object=r.string(l.max_name_bytes)?;let mut sources=Vec::new();for _ in 0..r.count(l.max_objects)?{sources.push(r.string(l.max_source_bytes)?);}Self::Join{object,sources}}
            5=>{let object=r.string(l.max_name_bytes)?;let name=r.string(l.max_source_bytes)?;let mut faces=Vec::new();for _ in 0..r.count(l.mesh.max_faces)?{faces.push(mesh::FaceId(r.u64()?));}Self::Separate{object,name,faces}}
            6=>Self::Snap{object:r.string(l.max_name_bytes)?,grid:r.f64()?},
            7=>Self::Pivot{object:r.string(l.max_name_bytes)?,position:read_array(r)?},
            8=>Self::Dimensions{object:r.string(l.max_name_bytes)?,size:read_array(r)?},
            9=>Self::Light(read_light(r,l)?),
            10=>Self::DeleteLight{name:r.string(l.max_source_bytes)?},
            11=>Self::Socket(read_socket(r,l)?),
            12=>Self::DeleteSocket{name:r.string(l.max_source_bytes)?},
            13=>Self::Lods{object:r.string(l.max_name_bytes)?,levels:read_lods(r)?},
            14=>Self::Collider{object:r.string(l.max_name_bytes)?,proxy:read_proxy(r,l)?},
            15=>Self::Modifier{object:r.string(l.max_name_bytes)?,modifier:read_modifier(r,l)?},
            16=>Self::RemoveModifier{object:r.string(l.max_name_bytes)?,name:r.string(l.max_source_bytes)?},
            17=>{let object=r.string(l.max_name_bytes)?;let mut names=Vec::new();for _ in 0..r.count(16)?{names.push(r.string(l.max_source_bytes)?);}Self::ModifierOrder{object,names}}
            18=>Self::ApplyModifiers{object:r.string(l.max_name_bytes)?},
            19=>Self::VehicleWheel{object:r.string(l.max_name_bytes)?,wheel:read_wheel(r,l)?},
            20=>Self::DeleteVehicleWheel{object:r.string(l.max_name_bytes)?},
            _=>return Err(Error::Corrupt("scene operation tag")),
        };value.check(l)?;Ok(value)
    }
}

#[cfg(test)]
mod typed_tests {
    use super::*;
    fn modifier()->NamedModifier{NamedModifier{name:"copies".into(),enabled:false,operation:MeshEditingOperation{
        object:"body".into(),action:crate::mesh_editing::Action::Array{count:2,offset:[1.,0.,0.]},
    }}}
    fn light(kind:LightKind,attachment:Attachment)->LightEmitter{LightEmitter{
        name:"lamp".into(),attachment,transform:Transform::default(),kind,color:[1.,0.5,0.25],intensity:2.,range:10.,
    }}
    fn wheel(visual:Option<VisualWheelMotion>)->VehicleWheel{VehicleWheel{connection:"wheel_front_left".into(),pivot:[1.,0.,0.],radius:0.5,width:0.2,visual}}
    fn roundtrip(op:&SceneOperation,l:&Limits){
        op.check(l).unwrap();let mut w=Writer::new(usize::MAX);op.write(&mut w).unwrap();
        assert_eq!(op.memory_bytes(),w.bytes.len()*3);
        let mut r=Reader::new(&w.bytes);let decoded=SceneOperation::read(&mut r,l).unwrap();r.end().unwrap();assert_eq!(&decoded,op);
        let mut out=Writer::new(usize::MAX);decoded.write(&mut out).unwrap();assert_eq!(out.bytes,w.bytes);
        for cut in [0,1,w.bytes.len()-1]{assert!(SceneOperation::read(&mut Reader::new(&w.bytes[..cut]),l).is_err());}
    }
    #[test]
    fn literal_operations_cover_every_tag(){
        let l=Limits::default();let m=modifier();let levels=vec![LodLevel{target_faces:20,max_error:0.1,distance:5.}];
        let ops=vec![
            SceneOperation::Node{object:"body".into(),node:SceneNode{parent:Some("root".into()),linked_to:Some("source".into()),transform:Transform::default()}},
            SceneOperation::Instance{object:"body".into(),source:"source".into(),transform:Transform{translation:[1.,2.,3.],..Default::default()}},
            SceneOperation::Duplicate{object:"body".into(),source:"source".into()},
            SceneOperation::MakeUnique{object:"body".into()},
            SceneOperation::Join{object:"body".into(),sources:vec!["b".into(),"a".into()]},
            SceneOperation::Separate{object:"body".into(),name:"cap".into(),faces:vec![mesh::FaceId(u64::MAX),mesh::FaceId(1)]},
            SceneOperation::Snap{object:"body".into(),grid:0.25},
            SceneOperation::Pivot{object:"body".into(),position:[1.,2.,3.]},
            SceneOperation::Dimensions{object:"body".into(),size:[2.,3.,4.]},
            SceneOperation::Light(light(LightKind::Point,Attachment::Root)),
            SceneOperation::DeleteLight{name:"lamp".into()},
            SceneOperation::Socket(Socket{name:"mount".into(),attachment:Attachment::Joint(2),transform:Transform::default()}),
            SceneOperation::DeleteSocket{name:"mount".into()},
            SceneOperation::Lods{object:"body".into(),levels},
            SceneOperation::Collider{object:"body".into(),proxy:CollisionProxy::Box},
            SceneOperation::Modifier{object:"body".into(),modifier:m},
            SceneOperation::RemoveModifier{object:"body".into(),name:"copies".into()},
            SceneOperation::ModifierOrder{object:"body".into(),names:vec!["copies".into()]},
            SceneOperation::ApplyModifiers{object:"body".into()},
            SceneOperation::VehicleWheel{object:"body".into(),wheel:wheel(None)},
            SceneOperation::DeleteVehicleWheel{object:"body".into()},
        ];
        for (tag,op) in ops.iter().enumerate(){roundtrip(op,&l);let mut w=Writer::new(usize::MAX);op.write(&mut w).unwrap();assert_eq!(w.bytes[0],tag as u8);}
        for proxy in [CollisionProxy::Sphere,CollisionProxy::Mesh{object:"proxy".into()},CollisionProxy::Hull,CollisionProxy::Capsule]{roundtrip(&SceneOperation::Collider{object:"body".into(),proxy},&l);}
        roundtrip(&SceneOperation::Node{object:"body".into(),node:SceneNode::default()},&l);
        roundtrip(&SceneOperation::Light(light(LightKind::Spot{inner:0.1,outer:0.5},Attachment::Object("body".into()))),&l);
        roundtrip(&SceneOperation::VehicleWheel{object:"body".into(),wheel:wheel(Some(VisualWheelMotion{steer_gain:1.,steer_max:0.5,compression:0.2,droop:0.1}))},&l);
    }
    #[test]
    fn typed_checks_reject_bad_data(){
        let l=Limits::default();
        for op in [
            SceneOperation::Instance{object:"body".into(),source:"source".into(),transform:Transform::default()},
            SceneOperation::Node{object:"body".into(),node:SceneNode::default()},
            SceneOperation::Snap{object:"body".into(),grid:-1.},
        ] { roundtrip(&op, &l); }
        for op in [
            SceneOperation::Snap{object:String::new(),grid:1.},
            SceneOperation::Pivot{object:"body".into(),position:[f64::NAN,0.,0.]},
            SceneOperation::Node{object:"body".into(),node:SceneNode{transform:Transform{rotation:[0.;4],..Default::default()},..Default::default()}},
            SceneOperation::Lods{object:"body".into(),levels:vec![LodLevel{target_faces:1,max_error:0.,distance:1.};5]},
            SceneOperation::ModifierOrder{object:"body".into(),names:vec!["m".into();17]},
            SceneOperation::Modifier{object:"other".into(),modifier:modifier()},
        ]{assert!(op.check(&l).is_err());}
        let tiny=Limits{max_objects:0,mesh:mesh::Limits{max_faces:0,..l.mesh.clone()},..l.clone()};
        assert!(SceneOperation::Join{object:"body".into(),sources:vec!["source".into()]}.check(&tiny).is_err());
        assert!(SceneOperation::Separate{object:"body".into(),name:"cap".into(),faces:vec![mesh::FaceId(1)]}.check(&tiny).is_err());
        assert!(matches!(SceneOperation::read(&mut Reader::new(&[255]),&l),Err(Error::Corrupt(_))));
        let mut w=Writer::new(64);w.u8(0).unwrap();w.string("body").unwrap();w.u8(2).unwrap();
        assert!(matches!(SceneOperation::read(&mut Reader::new(&w.bytes),&l),Err(Error::Corrupt(_))));
    }
    #[test]
    fn typed_state_preserves_maps_and_modifier_order(){
        let l=Limits::default();let state=SceneState{
            nodes:BTreeMap::from([("body".into(),SceneNode::default())]),
            emitters:BTreeMap::from([("lamp".into(),light(LightKind::Spot{inner:0.1,outer:0.5},Attachment::Root))]),
            sockets:BTreeMap::from([("mount".into(),Socket{name:"mount".into(),attachment:Attachment::Object("body".into()),transform:Transform::default()})]),
            lods:BTreeMap::from([("body".into(),vec![LodLevel{target_faces:12,max_error:0.,distance:5.}])]),
            colliders:BTreeMap::from([("body".into(),CollisionProxy::Hull)]),
            modifiers:BTreeMap::from([("body".into(),vec![modifier(),NamedModifier{name:"second".into(),..modifier()}])]),
            wheels:BTreeMap::from([("body".into(),wheel(None))]),
        };
        for state in [SceneState::default(),state]{
            state.check(&l).unwrap();let mut w=Writer::new(usize::MAX);state.write(&mut w).unwrap();assert_eq!(state.memory_bytes(),w.bytes.len()*3);
            let mut r=Reader::new(&w.bytes);assert_eq!(SceneState::read(&mut r,&l).unwrap(),state);r.end().unwrap();
            assert!(SceneState::read(&mut Reader::new(&w.bytes[..w.bytes.len()-1]),&l).is_err());
        }
        let mut w=Writer::new(1024);w.count(2).unwrap();for _ in 0..2{w.string("body").unwrap();write_node(&mut w,&SceneNode::default()).unwrap();}
        assert!(matches!(SceneState::read(&mut Reader::new(&w.bytes),&l),Err(Error::Corrupt(_))));
    }
}
