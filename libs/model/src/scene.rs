//! Editable object hierarchy and attachments. Punctual lights are real scene
//! emitters; surface emissiveness is authored separately in the material domain.
use crate::{canon::{Reader,Writer},document::State,json::{self,Value},mesh, schema::*,service::*,
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
#[derive(Clone,Debug,PartialEq)]
pub struct LodLevel {pub target_faces:usize,pub max_error:f64,pub distance:f64}
#[derive(Clone,Debug,PartialEq)]
pub enum CollisionProxy {Box,Sphere,Mesh{object:String}}
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
    pub fn memory_bytes(&self)->usize {self.value().to_json().len().saturating_mul(3)}
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
        if self.wheels.len()>4{return Err(Error::Budget("vehicle wheel bindings"));}
        let mut connections=BTreeSet::new();
        for (object,wheel) in &self.wheels {
            ctx.checkpoint(1)?;name(object,limits)?;
            if !state.objects.contains_key(object){return Err(Error::MissingObject(object.clone()));}
            if !VEHICLE_WHEEL_CONNECTIONS.contains(&wheel.connection.as_str())||!connections.insert(&wheel.connection){return Err(Error::Invalid("unknown or duplicate vehicle wheel connection"));}
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
    use crate::{Document,Operation,Transaction};
    fn apply(doc:&mut Document,id:&str,json:&str)->crate::Applied{let operations=parse_operations(&json::parse(json.as_bytes()).unwrap(),doc.limits()).unwrap();doc.apply(Transaction{request_id:id.into(),expected:doc.head(),operations},None).unwrap()}
    fn maximum_x(mesh:&mesh::Mesh)->f64{mesh.vertices().iter().map(|v|v.position[0]).fold(f64::NEG_INFINITY,f64::max)}
    #[test]
    fn ordered_modifiers_preserve_base_roundtrip_and_bake_once(){
        let mut doc=Document::new(Limits::default()).unwrap();apply(&mut doc,"cube",r#"[{"op":"cube","object":"body","size":[2,2,2]}]"#);
        apply(&mut doc,"stack",r#"[{"op":"modifier","object":"body","name":"copies","operation":{"op":"array","count":2,"offset":[3,0,0]}},{"op":"modifier","object":"body","name":"scale","operation":{"op":"taper","vertices":[],"axis":1,"range":[-1,1],"scales":[2,2]}}]"#);
        assert_eq!(doc.object("body").unwrap().vertices().len(),8);let mut ctx=mesh::Context::default();let evaluated=doc.scene().local_mesh(&doc.state,"body",&mut ctx).unwrap();assert_eq!(evaluated.vertices().len(),16);assert_eq!(maximum_x(&evaluated),8.);
        let bytes=doc.to_bytes(None).unwrap();let restored=Document::from_bytes(&bytes,Limits::default(),None).unwrap();assert_eq!(restored.scene().local_mesh(&restored.state,"body",&mut ctx).unwrap(),evaluated);
        apply(&mut doc,"order",r#"[{"op":"modifier_order","object":"body","names":["scale","copies"]}]"#);let evaluated=doc.scene().local_mesh(&doc.state,"body",&mut ctx).unwrap();assert_eq!(maximum_x(&evaluated),5.);
        apply(&mut doc,"bake",r#"[{"op":"apply_modifiers","object":"body"}]"#);assert!(doc.scene().modifiers.is_empty());assert_eq!(doc.object("body").unwrap(),&evaluated);assert_eq!(doc.scene().local_mesh(&doc.state,"body",&mut ctx).unwrap(),evaluated);
    }
    #[test]
    fn instance_cache_shares_evaluation_and_dependency_cycles_roll_back(){
        let mut doc=Document::new(Limits::default()).unwrap();apply(&mut doc,"seed",r#"[{"op":"cube","object":"a","size":[2,2,2]},{"op":"instance","object":"b","source":"a"},{"op":"modifier","object":"a","name":"copies","operation":{"op":"array","count":2,"offset":[3,0,0]}}]"#);
        let cache=doc.scene().evaluated_meshes(&doc.state,&mut mesh::Context::default()).unwrap();assert!(Arc::ptr_eq(&cache["a"],&cache["b"]));assert_eq!(cache["b"].vertices().len(),16);
        let before=doc.to_bytes(None).unwrap();let ops=parse_operations(&json::parse(br#"[{"op":"modifier","object":"a","name":"wrap","operation":{"op":"shrinkwrap","vertices":[],"reference":"b","max_distance":10,"offset":0}}]"#).unwrap(),doc.limits()).unwrap();
        assert!(doc.apply(Transaction{request_id:"cycle".into(),expected:doc.head(),operations:ops},None).is_err());assert_eq!(doc.to_bytes(None).unwrap(),before);
        apply(&mut doc,"unique",r#"[{"op":"make_unique","object":"b"}]"#);assert_eq!(doc.object("b").unwrap().vertices().len(),16);assert!(doc.scene().nodes["b"].linked_to.is_none());
    }
    #[test]
    fn shrinkwrap_modifier_projects_between_object_coordinate_spaces(){
        let mut doc=Document::new(Limits::default()).unwrap();
        apply(&mut doc,"seed",r#"[{"op":"plane","object":"cage","size":[2,2]},{"op":"plane","object":"reference","size":[8,8]},{"op":"object_node","object":"cage","node":{"transform":{"translation":[0,3,0],"rotation":[0,0,0,1],"scale":[1,2,1]}}},{"op":"object_node","object":"reference","node":{"transform":{"translation":[0,1,0],"rotation":[0,0,0,1],"scale":[1,1,1]}}},{"op":"modifier","object":"cage","name":"wrap","operation":{"op":"shrinkwrap","vertices":[],"reference":"reference","max_distance":2,"offset":0}}]"#);
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
        apply(&mut doc,"join",r#"[{"op":"join","object":"joined","sources":["a"]}]"#);let joined=doc.object("joined").unwrap();assert_eq!(joined.uv_pins().len(),1);assert_eq!(joined.edge_attributes().values().next().unwrap().attributes,mesh::EdgeAttributes{seam:true,crease:0.5});assert!(joined.corners().iter().all(|c|c.normal.is_some()));
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
fn parse_attachment(v:&Value)->Result<Attachment>{if v.is_null(){return Ok(Attachment::Root);}fields(v,&["object","joint"])?;
    match (v.get("object"),v.get("joint")){(Some(o),None)=>Ok(Attachment::Object(o.as_str().ok_or(Error::Invalid("attachment object"))?.into())),
        (None,Some(j))=>Ok(Attachment::Joint(integer(j)?)),_=>Err(Error::Invalid("attachment requires exactly one target"))}}
fn optional_name(v:Option<&Value>)->Result<Option<String>> {match v {None|Some(Value::Null)=>Ok(None),Some(Value::Str(s))=>Ok(Some(s.clone())),_=>Err(Error::Invalid("optional name"))}}
fn node_value(n:&SceneNode)->Value {json::obj(vec![("parent",n.parent.as_ref().map(json::s).unwrap_or(Value::Null)),
    ("linked_to",n.linked_to.as_ref().map(json::s).unwrap_or(Value::Null)),("transform",transform_value(&n.transform))])}
fn parse_node(v:&Value)->Result<SceneNode>{fields(v,&["parent","linked_to","transform"])?;Ok(SceneNode{parent:optional_name(v.get("parent"))?,
    linked_to:optional_name(v.get("linked_to"))?,transform:v.get("transform").map(transform).transpose()?.unwrap_or_default()})}
impl SceneOperation {
    pub fn object_name(&self)->Option<&str>{match self {Self::Node{object,..}|Self::Instance{object,..}|Self::Duplicate{object,..}
        |Self::MakeUnique{object}|Self::Join{object,..}|Self::Separate{object,..}|Self::Pivot{object,..}|Self::Snap{object,..}|Self::Dimensions{object,..}
        |Self::Lods{object,..}|Self::Collider{object,..}|Self::Modifier{object,..}|Self::RemoveModifier{object,..}|Self::ModifierOrder{object,..}|Self::ApplyModifiers{object}
        |Self::VehicleWheel{object,..}|Self::DeleteVehicleWheel{object}=>Some(object),_=>None}}
    pub fn memory_bytes(&self)->usize {self.value().to_json().len().saturating_mul(3)}
    pub fn parse(v:&Value,limits:&Limits)->Result<Option<Self>> {
        let op=text(v,"op")?;
        let value=match op {
            "modifier"=>{fields(v,&["op","object","name","enabled","operation"])?;let object=text(v,"object")?.to_owned();let mut inner=need(v,"operation")?.clone();
                let Value::Obj(fields)=&mut inner else{return Err(Error::Invalid("modifier operation must be an object"));};
                if !fields.iter().any(|(key,_)|key=="object"){fields.push(("object".into(),json::s(&object)));}
                let operation=MeshEditingOperation::parse(&inner,limits)?.ok_or(Error::Invalid("unsupported modifier operation"))?;
                if operation.object_name()!=object||!operation.is_modifier_safe(){return Err(Error::Invalid("modifier requires its own object and no fixed element selections"));}
                let enabled=v.get("enabled").map(|v|v.as_bool().ok_or(Error::Invalid("modifier enabled must be boolean"))).transpose()?.unwrap_or(true);
                let modifier=NamedModifier{name:text(v,"name")?.into(),enabled,operation};name(&modifier.name,limits)?;Self::Modifier{object,modifier}}
            "remove_modifier"=>{fields(v,&["op","object","name"])?;Self::RemoveModifier{object:text(v,"object")?.into(),name:text(v,"name")?.into()}}
            "modifier_order"=>{fields(v,&["op","object","names"])?;Self::ModifierOrder{object:text(v,"object")?.into(),names:rows(need(v,"names")?,16)?.iter().map(|v|v.as_str().map(String::from).ok_or(Error::Invalid("modifier name"))).collect::<Result<_>>()?}}
            "apply_modifiers"=>{fields(v,&["op","object"])?;Self::ApplyModifiers{object:text(v,"object")?.into()}}
            "object_node"=>{fields(v,&["op","object","node"])?;Self::Node{object:text(v,"object")?.into(),node:parse_node(need(v,"node")?)?}}
            "instance"=>{fields(v,&["op","object","source","transform"])?;Self::Instance{object:text(v,"object")?.into(),source:text(v,"source")?.into(),transform:v.get("transform").map(transform).transpose()?.unwrap_or_default()}}
            "duplicate"=>{fields(v,&["op","object","source"])?;Self::Duplicate{object:text(v,"object")?.into(),source:text(v,"source")?.into()}}
            "make_unique"=>{fields(v,&["op","object"])?;Self::MakeUnique{object:text(v,"object")?.into()}}
            "join"=>{fields(v,&["op","object","sources"])?;Self::Join{object:text(v,"object")?.into(),sources:rows(need(v,"sources")?,limits.max_objects)?.iter().map(|v|v.as_str().map(String::from).ok_or(Error::Invalid("join source"))).collect::<Result<_>>()?}}
            "separate"=>{fields(v,&["op","object","name","faces"])?;Self::Separate{object:text(v,"object")?.into(),name:text(v,"name")?.into(),faces:selections(need(v,"faces")?,limits.mesh.max_faces)?.into_iter().map(mesh::FaceId).collect()}}
            "pivot"=>{fields(v,&["op","object","position"])?;Self::Pivot{object:text(v,"object")?.into(),position:array(need(v,"position")?)?}}
            "snap"=>{fields(v,&["op","object","grid"])?;Self::Snap{object:text(v,"object")?.into(),grid:float(need(v,"grid")?)?}}
            "dimensions"=>{fields(v,&["op","object","size"])?;Self::Dimensions{object:text(v,"object")?.into(),size:array(need(v,"size")?)?}}
            "light"=>{
                fields(v,&["op","name","attachment","transform","kind","color","intensity","range","inner","outer"])?;
                let kind=match text(v,"kind")? {"point"=>{if v.get("inner").is_some()||v.get("outer").is_some(){return Err(Error::Invalid("point light has no cone"));}LightKind::Point},
                    "spot"=>LightKind::Spot{inner:float(need(v,"inner")?)?,outer:float(need(v,"outer")?)?},_=>return Err(Error::Invalid("light kind must be point or spot"))};
                Self::Light(LightEmitter{name:text(v,"name")?.into(),attachment:parse_attachment(need(v,"attachment")?)?,transform:v.get("transform").map(transform).transpose()?.unwrap_or_default(),
                    kind,color:array(need(v,"color")?)?,intensity:float(need(v,"intensity")?)?,range:float(need(v,"range")?)?})
            }
            "delete_light"=>{fields(v,&["op","name"])?;Self::DeleteLight{name:text(v,"name")?.into()}}
            "socket"=>{fields(v,&["op","name","attachment","transform"])?;Self::Socket(Socket{name:text(v,"name")?.into(),attachment:parse_attachment(need(v,"attachment")?)?,transform:transform(need(v,"transform")?)?})}
            "delete_socket"=>{fields(v,&["op","name"])?;Self::DeleteSocket{name:text(v,"name")?.into()}}
            "vehicle_wheel"=>{fields(v,&["op","object","connection","pivot","radius","width","visual"])?;Self::VehicleWheel{object:text(v,"object")?.into(),wheel:VehicleWheel{connection:text(v,"connection")?.into(),pivot:array(need(v,"pivot")?)?,radius:float(need(v,"radius")?)?,width:float(need(v,"width")?)?,visual:v.get("visual").map(parse_visual_wheel).transpose()?}}}
            "delete_vehicle_wheel"=>{fields(v,&["op","object"])?;Self::DeleteVehicleWheel{object:text(v,"object")?.into()}}
            "lods"=>{fields(v,&["op","object","levels"])?;Self::Lods{object:text(v,"object")?.into(),levels:rows(need(v,"levels")?,4)?.iter().map(|v|{
                fields(v,&["target_faces","max_error","distance"])?;Ok(LodLevel{target_faces:integer(need(v,"target_faces")?)? as usize,max_error:float(need(v,"max_error")?)?,distance:float(need(v,"distance")?)?})}).collect::<Result<_>>()?}}
            "collider"=>{fields(v,&["op","object","kind","source"])?;let proxy=match text(v,"kind")?{"box"=>CollisionProxy::Box,"sphere"=>CollisionProxy::Sphere,
                "mesh"=>CollisionProxy::Mesh{object:text(v,"source")?.into()},_=>return Err(Error::Invalid("collider kind"))};Self::Collider{object:text(v,"object")?.into(),proxy}}
            _=>return Ok(None),
        };if let Some(object)=value.object_name(){name(object,limits)?;}Ok(Some(value))
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
            Self::Collider{object,proxy}=>vec![("op",json::s("collider")),("object",json::s(object)),("kind",json::s(match proxy{CollisionProxy::Box=>"box",CollisionProxy::Sphere=>"sphere",CollisionProxy::Mesh{..}=>"mesh"}))],
        };
        if let Self::Light(LightEmitter{kind:LightKind::Spot{inner,outer},..})=self{f.push(("inner",Value::F64(*inner)));f.push(("outer",Value::F64(*outer)));}
        if let Self::Collider{proxy:CollisionProxy::Mesh{object},..}=self{f.push(("source",json::s(object)));}
        if let Self::VehicleWheel{wheel:VehicleWheel{visual:Some(visual),..},..}=self{f.push(("visual",json::obj(vec![
            ("steer_gain",Value::F64(visual.steer_gain)),("steer_max",Value::F64(visual.steer_max)),
            ("compression",Value::F64(visual.compression)),("droop",Value::F64(visual.droop))])));}
        json::obj(f)
    }
}
fn parse_visual_wheel(v:&Value)->Result<VisualWheelMotion>{
    fields(v,&["steer_gain","steer_max","compression","droop"])?;
    Ok(VisualWheelMotion{steer_gain:float(need(v,"steer_gain")?)?,steer_max:float(need(v,"steer_max")?)?,
        compression:float(need(v,"compression")?)?,droop:float(need(v,"droop")?)?})
}
impl SceneState {
    pub fn value(&self)->Value {Value::Arr(self.nodes.iter().map(|(object,node)|SceneOperation::Node{object:object.clone(),node:node.clone()}.value())
        .chain(self.emitters.values().cloned().map(|l|SceneOperation::Light(l).value()))
        .chain(self.sockets.values().cloned().map(|s|SceneOperation::Socket(s).value()))
        .chain(self.wheels.iter().map(|(object,wheel)|SceneOperation::VehicleWheel{object:object.clone(),wheel:wheel.clone()}.value()))
        .chain(self.lods.iter().map(|(object,levels)|SceneOperation::Lods{object:object.clone(),levels:levels.clone()}.value()))
        .chain(self.colliders.iter().map(|(object,proxy)|SceneOperation::Collider{object:object.clone(),proxy:proxy.clone()}.value()))
        .chain(self.modifiers.iter().flat_map(|(object,stack)|stack.iter().map(move |modifier|SceneOperation::Modifier{object:object.clone(),modifier:modifier.clone()}.value()))).collect())}
    pub(crate) fn write(&self,w:&mut Writer)->Result<()> {write_value(w,self.value())}
    pub(crate) fn read(r:&mut Reader,limits:&Limits)->Result<Self>{
        let value=read_value(r,limits)?;let mut state=Self::default();
        for row in rows(&value,limits.max_objects.saturating_mul(3)+292)? {
            let duplicate=match SceneOperation::parse(row,limits)?.ok_or(Error::Corrupt("scene operation"))? {
                SceneOperation::Node{object,node}=>state.nodes.insert(object,node).is_some(),
                SceneOperation::Light(l)=>state.emitters.insert(l.name.clone(),l).is_some(),
                SceneOperation::Socket(s)=>state.sockets.insert(s.name.clone(),s).is_some(),
                SceneOperation::VehicleWheel{object,wheel}=>state.wheels.insert(object,wheel).is_some(),
                SceneOperation::Lods{object,levels}=>state.lods.insert(object,levels).is_some(),
                SceneOperation::Collider{object,proxy}=>state.colliders.insert(object,proxy).is_some(),
                SceneOperation::Modifier{object,modifier}=>{let stack=state.modifiers.entry(object).or_default();if stack.iter().any(|m|m.name==modifier.name){true}else{stack.push(modifier);false}},
                _=>return Err(Error::Corrupt("non-state scene operation")),
            };if duplicate{return Err(Error::Corrupt("duplicate scene entry"));}
        }Ok(state)
    }
}
