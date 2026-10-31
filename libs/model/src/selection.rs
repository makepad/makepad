//! Explicit revision-bound named selections. Queries run against a worker-owned
//! document; no hidden viewport selection participates in an operation.
use crate::{document::State,mesh,Error,Result,Limits,Operation,OperationResult,schema::*,transform::*};
use std::collections::{BTreeMap,BTreeSet,VecDeque};
use crate::{canon::{Reader,Writer},mesh_editing::{Action,codec::*},mesh::{VertexId,FaceId,EdgeKey}};
#[derive(Clone,Debug,PartialEq)] pub struct SelectionGroup{pub object:String,pub name:String,pub vertices:Vec<mesh::VertexId>,pub faces:Vec<mesh::FaceId>,pub stale:bool}
#[derive(Clone,Debug,Default,PartialEq)] pub struct SelectionState{pub groups:BTreeMap<(String,String),SelectionGroup>}
#[derive(Clone,Debug,PartialEq)] pub enum SelectionQuery{All,Material(u32),Bounds{min:[f64;3],max:[f64;3],world:bool},Connected{face:mesh::FaceId,stop_seams:bool},Loop{edge:mesh::EdgeKey,ring:bool},Ray{origin:[f64;3],direction:[f64;3],world:bool}}
#[derive(Clone,Debug,PartialEq)] pub enum SelectionOperation{
    Set{object:String,name:String,vertices:Vec<mesh::VertexId>,faces:Vec<mesh::FaceId>},
    Query{object:String,name:String,query:SelectionQuery,element:String,min:usize,max:usize},
    Delete{object:String,name:String},Use{object:String,name:String,element:String,operation:Box<crate::Operation>},
}
impl SelectionState{
    pub fn memory_bytes(&self)->usize{self.groups.values().map(|g|g.object.len()+g.name.len()+(g.vertices.len()+g.faces.len())*8+256).sum()}
    pub(crate) fn reconcile(&mut self,objects:&BTreeMap<String,mesh::Mesh>){self.groups.retain(|(object,_),_|objects.contains_key(object));for g in self.groups.values_mut(){let m=&objects[&g.object];if g.vertices.iter().any(|id|m.vertex(*id).is_none())||g.faces.iter().any(|id|m.face(*id).is_none()){g.stale=true;}}}
    pub(crate) fn validate(&self,state:&State,limits:&Limits)->Result<()>{if self.groups.len()>128||self.memory_bytes()>limits.mesh.max_bytes/4{return Err(Error::Budget("selection groups"));}
        for ((object,n),g) in &self.groups{name(object,limits)?;name(n,limits)?;if object!=&g.object||n!=&g.name{return Err(Error::Invalid("selection identity"));}let m=state.objects.get(object).ok_or_else(||Error::MissingObject(object.clone()))?;
            if g.vertices.len()>limits.mesh.max_vertices||g.faces.len()>limits.mesh.max_faces{return Err(Error::Budget("named selection IDs"));}
            if g.vertices.windows(2).any(|w|w[0]>=w[1])||g.faces.windows(2).any(|w|w[0]>=w[1]){return Err(Error::Invalid("selection IDs must be unique and ordered"));}
            if !g.stale&&(g.vertices.iter().any(|id|m.vertex(*id).is_none())||g.faces.iter().any(|id|m.face(*id).is_none())){return Err(Error::Invalid("named selection references missing elements"));}
        }Ok(())
    }
    pub(crate) fn write(&self,w:&mut crate::canon::Writer)->Result<()>{w.count(self.groups.len())?;for g in self.groups.values(){w.string(&g.object)?;w.string(&g.name)?;w.u8(g.stale as u8)?;w.count(g.vertices.len())?;for v in &g.vertices{w.u64(v.0)?;}w.count(g.faces.len())?;for f in &g.faces{w.u64(f.0)?;}}Ok(())}
    pub(crate) fn read(r:&mut crate::canon::Reader,limits:&Limits)->Result<Self>{let mut out=Self::default();for _ in 0..r.count(128)?{let object=r.string(limits.max_name_bytes)?;let name=r.string(limits.max_name_bytes)?;let stale=match r.u8()?{0=>false,1=>true,_=>return Err(Error::Corrupt("selection stale flag"))};let mut vertices=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{vertices.push(mesh::VertexId(r.u64()?));}let mut faces=Vec::new();for _ in 0..r.count(limits.mesh.max_faces)?{faces.push(mesh::FaceId(r.u64()?));}let g=SelectionGroup{object:object.clone(),name:name.clone(),vertices,faces,stale};if out.groups.insert((object,name),g).is_some(){return Err(Error::Corrupt("duplicate selection"));}}Ok(out)}
}
impl SelectionOperation{
    pub fn object_name(&self)->&str{match self{Self::Set{object,..}|Self::Query{object,..}|Self::Delete{object,..}|Self::Use{object,..}=>object}}
    pub fn memory_bytes(&self)->usize{let mut w=Writer::new(usize::MAX);if self.write(&mut w).is_err(){return usize::MAX;}w.bytes.len().saturating_mul(3).saturating_add(128)}
    pub(crate) fn apply(&self,state:&mut State,limits:&Limits,ctx:&mut mesh::Context<'_>)->Result<OperationResult>{
        self.check(limits)?;
        let object=self.object_name();let m=state.objects.get(object).ok_or_else(||Error::MissingObject(object.into()))?;
        match self{
            Self::Use{name,element,operation,..}=>{
                let g=state.selections.groups.get(&(object.into(),name.clone())).ok_or(Error::Invalid("unknown named selection"))?;
                if g.stale||g.vertices.iter().any(|v|m.vertex(*v).is_none())||g.faces.iter().any(|f|m.face(*f).is_none()){return Err(Error::Invalid("named selection stale; reselect against current head"));}
                let ids=match element.as_str(){"vertices" if !g.vertices.is_empty()=>g.vertices.iter().map(|v|v.0).collect::<Vec<_>>(),"faces" if !g.faces.is_empty()=>g.faces.iter().map(|f|f.0).collect(),"face" if g.faces.len()==1=>vec![g.faces[0].0],_=>return Err(Error::Invalid("selection required cardinality"))};
                let resolved=retarget(operation,object,element,&ids)?;check_resolved(&resolved,limits)?;
                let resolved=[resolved];
                let mut results=crate::document::execute(state,&resolved,limits,ctx)?;Ok(results.remove(0))
            }
            Self::Delete{name,..}=>{if state.selections.groups.remove(&(object.into(),name.clone())).is_none(){return Err(Error::Invalid("unknown named selection"));}Ok(OperationResult::default())}
            Self::Set{name,vertices,faces,..}=>save(state,object,name,vertices.clone(),faces.clone(),limits),
            Self::Query{name,query,element,min,max,..}=>{if min>max||*max>limits.mesh.max_vertices.max(limits.mesh.max_faces)||!["vertices","faces"].contains(&element.as_str()){return Err(Error::Invalid("selection query cardinality"));}let(vertices,faces)=query.evaluate(state,object,ctx)?;let count=if element=="vertices"{vertices.len()}else{faces.len()};if count<*min||count>*max{return Err(Error::Invalid("selection cardinality mismatch"));}save(state,object,name,vertices,faces,limits)}
        }
    }
}
fn save(state:&mut State,object:&str,name:&str,mut vertices:Vec<mesh::VertexId>,mut faces:Vec<mesh::FaceId>,limits:&Limits)->Result<OperationResult>{vertices.sort();vertices.dedup();faces.sort();faces.dedup();let m=&state.objects[object];if vertices.iter().any(|v|m.vertex(*v).is_none())||faces.iter().any(|f|m.face(*f).is_none()){return Err(Error::Invalid("selection element missing"));}
    // Earlier edits in this atomic batch may have removed elements belonging
    // to other groups. Mark those stale before validating the fresh query.
    state.selections.reconcile(&state.objects);
    if state.selections.groups.len()>=128&&!state.selections.groups.contains_key(&(object.into(),name.into())){return Err(Error::Budget("named selections"));}
    let result=OperationResult{object:object.into(),vertices:vertices.clone(),faces:faces.clone(),..Default::default()};state.selections.groups.insert((object.into(),name.into()),SelectionGroup{object:object.into(),name:name.into(),vertices,faces,stale:false});state.selections.validate(state,limits)?;Ok(result)}
impl SelectionQuery{
    fn evaluate(&self,state:&State,object:&str,ctx:&mut mesh::Context<'_>)->Result<(Vec<mesh::VertexId>,Vec<mesh::FaceId>)>{
        match self{
            Self::Bounds{min,max,..}if min.iter().chain(max).any(|v|!v.is_finite())||(0..3).any(|i|min[i]>max[i])=>return Err(Error::Invalid("selection bounds")),
            Self::Ray{origin,direction,..}if origin.iter().chain(direction).any(|v|!v.is_finite())=>return Err(Error::Invalid("selection ray")),_=>{}
        }
        let m=&state.objects[object];let mut vertices=BTreeSet::new();let mut faces=BTreeSet::new();
        match self{
            Self::All=>{vertices.extend(m.vertices().iter().map(|v|v.id));faces.extend(m.faces().iter().map(|f|f.id));},
            Self::Material(id)=>{for f in m.faces(){ctx.checkpoint(1)?;if f.material==*id{faces.insert(f.id);}}},
            Self::Bounds{min,max,world}=>{let matrix=if *world{state.scene.world_matrix(object)?}else{IDENTITY_MATRIX};for v in m.vertices(){ctx.checkpoint(1)?;let p=transform_point(matrix,v.position);if (0..3).all(|i|p[i]>=min[i]&&p[i]<=max[i]){vertices.insert(v.id);}}
                for f in m.faces(){ctx.checkpoint(1)?;if m.face_corners(f.id)?.iter().all(|c|vertices.contains(&c.vertex)){faces.insert(f.id);}}},
            Self::Connected{face,stop_seams}=>{m.face_corners(*face)?;let adj=m.adjacency(ctx)?;let mut queue=VecDeque::from([*face]);faces.insert(*face);while let Some(f)=queue.pop_front(){for edge in face_edges(m,f)?{ctx.checkpoint(1)?;if *stop_seams&&m.edge_attributes().get(&edge).is_some_and(|e|e.attributes.seam){continue;}for use_ in adj.radial(edge){if faces.insert(use_.face){queue.push_back(use_.face);}}}}},
            Self::Loop{edge,ring}=>{
                let adj=m.adjacency(ctx)?;if adj.radial(*edge).is_empty(){return Err(Error::Invalid("selection edge missing"));}let mut queue=VecDeque::from([*edge]);let mut visited=BTreeSet::from([*edge]);
                while let Some(edge)=queue.pop_front(){ctx.checkpoint(1)?;vertices.extend([edge.0,edge.1]);for use_ in adj.radial(edge){ctx.checkpoint(1)?;faces.insert(use_.face);}
                    let mut next=BTreeSet::new();if *ring{for use_ in adj.radial(edge){let edges=face_edges(m,use_.face)?;ctx.checkpoint(edges.len()as u64)?;if edges.len()!=4{return Err(Error::Invalid("edge ring requires quads"));}let i=edges.iter().position(|e|e==&edge).unwrap();next.insert(edges[(i+2)%4]);}}
                    else{for vertex in [edge.0,edge.1]{let incident=&adj.vertex_edges[&vertex];ctx.checkpoint(1)?;if incident.len()!=4{continue;}for &candidate in incident{ctx.checkpoint(1)?;if candidate==edge{continue;}let mut shared=false;for a in adj.radial(edge){for b in adj.radial(candidate){ctx.checkpoint(1)?;shared|=a.face==b.face;}}if !shared{next.insert(candidate);}}}}
                    for edge in next{if visited.insert(edge){queue.push_back(edge);}}
                }
            }
            Self::Ray{origin,direction,world}=>{let matrix=if *world{state.scene.world_matrix(object)?}else{IDENTITY_MATRIX};let direction=normalized(*direction)?;let tri=m.triangulate(ctx)?;let mut best=None;
                for t in &tri.triangles{ctx.checkpoint(1)?;let p=t.indices.map(|i|transform_point(matrix,tri.vertices[i as usize].position));if let Some(distance)=ray(*origin,direction,p){if best.is_none_or(|(d,f)|distance<d||(distance==d&&t.source_face<f)){best=Some((distance,t.source_face));}}}if let Some((_,face))=best{faces.insert(face);}
            }
        }
        if !matches!(self,Self::Bounds{..}|Self::Loop{..}){for f in &faces{for c in m.face_corners(*f)?{vertices.insert(c.vertex);}}}
        Ok((vertices.into_iter().collect(),faces.into_iter().collect()))
    }
}
fn face_edges(m:&mesh::Mesh,face:mesh::FaceId)->Result<Vec<mesh::EdgeKey>>{let c=m.face_corners(face)?;Ok((0..c.len()).map(|i|mesh::EdgeKey::new(c[i].vertex,c[(i+1)%c.len()].vertex)).collect())}
fn ray(origin:[f64;3],direction:[f64;3],p:[[f64;3];3])->Option<f64>{let a=sub(p[1],p[0]);let b=sub(p[2],p[0]);let h=cross(direction,b);let det=dot(a,h);if det.abs()<1e-12{return None;}let s=sub(origin,p[0]);let u=dot(s,h)/det;if !(0.0..=1.0).contains(&u){return None;}let q=cross(s,a);let v=dot(direction,q)/det;if v<0.||u+v>1.{return None;}let distance=dot(b,q)/det;(distance>=0.).then_some(distance)}

#[cfg(test)]
mod loop_accounting_tests{
    use super::*;
    #[test]
    fn edge_loop_walks_only_vertex_adjacency_and_charges_examined_work(){
        let n=80usize;let positions=(0..n).flat_map(|y|(0..n).map(move |x|[x as f64,y as f64,0.])).collect::<Vec<_>>();
        let mut polygons=Vec::new();for y in 0..n-1{for x in 0..n-1{let a=(y*n+x)as u32;polygons.push(mesh::Polygon::new(vec![a,a+1,a+1+n as u32,a+n as u32]));}}
        let m=mesh::Mesh::from_polygons(&positions,&polygons,&mut mesh::Context::default()).unwrap();let a=m.vertices()[(n/2)*n+n/2].id;let b=m.vertices()[(n/2)*n+n/2+1].id;
        let mut baseline=mesh::Context::default();m.adjacency(&mut baseline).unwrap();let adjacency_work=baseline.work_used();
        let mut state=State::default();state.objects.insert("grid".into(),m);
        let query=SelectionQuery::Loop{edge:mesh::EdgeKey::new(a,b),ring:false};let mut ctx=mesh::Context::default();let (vertices,faces)=query.evaluate(&state,"grid",&mut ctx).unwrap();
        assert_eq!(vertices.len(),n);assert_eq!(faces.len(),2*(n-1));
        let walk_work=ctx.work_used()-adjacency_work;assert!(walk_work>20*n as u64,"candidate comparisons must be charged: {walk_work}");assert!(walk_work<100*n as u64,"walk should depend on adjacent edges: {walk_work}");
        let mut limited=mesh::Context::new(mesh::Limits{max_work:ctx.work_used()-1,..mesh::Limits::default()},None);assert!(matches!(query.evaluate(&state,"grid",&mut limited),Err(Error::Mesh(mesh::MeshError::Budget{resource:"work",..}))));
    }
}

fn write_selectionquery(w:&mut Writer,value:&SelectionQuery)->Result<()>{match value{
    SelectionQuery::All=>{w.u8(0)?;},
    SelectionQuery::Material(value)=>{w.u8(1)?;w.u32(*value)?;},
    SelectionQuery::Bounds{min,max,world}=>{w.u8(2)?;for item in min{w.f64(*item)?;}for item in max{w.f64(*item)?;}w.u8(*world as u8)?;},
    SelectionQuery::Connected{face,stop_seams}=>{w.u8(3)?;w.u64(face.0)?;w.u8(*stop_seams as u8)?;},
    SelectionQuery::Loop{edge,ring}=>{w.u8(4)?;w.u64(edge.0.0)?;w.u64(edge.1.0)?;w.u8(*ring as u8)?;},
    SelectionQuery::Ray{origin,direction,world}=>{w.u8(5)?;for item in origin{w.f64(*item)?;}for item in direction{w.f64(*item)?;}w.u8(*world as u8)?;},
}Ok(())}
fn read_selectionquery(r:&mut Reader,limits:&Limits)->Result<SelectionQuery>{let _=limits;Ok(match r.u8()?{
    0=>SelectionQuery::All,
    1=>SelectionQuery::Material(r.u32()?),
    2=>SelectionQuery::Bounds{min:[r.f64()?,r.f64()?,r.f64()?],max:[r.f64()?,r.f64()?,r.f64()?],world:read_bool(r)?},
    3=>SelectionQuery::Connected{face:FaceId(r.u64()?),stop_seams:read_bool(r)?},
    4=>SelectionQuery::Loop{edge:EdgeKey(VertexId(r.u64()?),VertexId(r.u64()?)),ring:read_bool(r)?},
    5=>SelectionQuery::Ray{origin:[r.f64()?,r.f64()?,r.f64()?],direction:[r.f64()?,r.f64()?,r.f64()?],world:read_bool(r)?},
    _=>return Err(Error::Corrupt("SelectionQuery tag")),
})}

impl SelectionOperation {
    pub fn check(&self,limits:&Limits)->Result<()>{
        name(self.object_name(),limits)?;
        match self{
            Self::Set{name:n,vertices,faces,..}=>{name(n,limits)?;check_count(vertices.len(),limits.mesh.max_vertices)?;check_count(faces.len(),limits.mesh.max_faces)?;},
            Self::Delete{name:n,..}=>name(n,limits)?,
            Self::Query{name:n,query,element,min,max,..}=>{name(n,limits)?;if !["vertices","faces"].contains(&element.as_str()){return Err(Error::Invalid("query element"));}if min>max||*max>limits.mesh.max_vertices.max(limits.mesh.max_faces)||u32::try_from(*max).is_err(){return Err(Error::Invalid("selection cardinality"));}query.check(limits)?;},
            Self::Use{name:n,element,operation,..}=>{name(n,limits)?;if !["vertices","faces","face"].contains(&element.as_str()){return Err(Error::Invalid("selection element"));}if !has_target(operation,element){return Err(Error::Invalid("selection template must be direct mesh edit"));}},
        }Ok(())
    }
    pub(crate) fn write(&self,w:&mut Writer)->Result<()>{match self{
        Self::Set{object,name,vertices,faces}=>{w.u8(0)?;w.string(object)?;w.string(name)?;w.count(vertices.len())?;for id in vertices{w.u64(id.0)?;}w.count(faces.len())?;for id in faces{w.u64(id.0)?;}},
        Self::Query{object,name,query,element,min,max}=>{w.u8(1)?;w.string(object)?;w.string(name)?;write_selectionquery(w,query)?;w.string(element)?;w.count(*min)?;w.count(*max)?;},
        Self::Delete{object,name}=>{w.u8(2)?;w.string(object)?;w.string(name)?;},
        Self::Use{object,name,element,operation}=>{w.u8(3)?;w.string(object)?;w.string(name)?;w.string(element)?;operation.write(w)?;},
    }Ok(())}
    pub(crate) fn read(r:&mut Reader,limits:&Limits)->Result<Self>{let tag=r.u8()?;let object=r.string(limits.max_name_bytes)?;let name=r.string(limits.max_name_bytes)?;let value=match tag{
        0=>Self::Set{object,name,vertices:read_ids(r,limits.mesh.max_vertices,VertexId)?,faces:read_ids(r,limits.mesh.max_faces,FaceId)?},
        1=>Self::Query{object,name,query:read_selectionquery(r,limits)?,element:r.string(8)?,min:r.u32()? as usize,max:r.u32()? as usize},
        2=>Self::Delete{object,name},3=>Self::Use{object,name,element:r.string(8)?,operation:Box::new(read_template(r,limits)?)},
        _=>return Err(Error::Corrupt("selection operation tag")),
    };value.check(limits)?;Ok(value)}
}
impl SelectionQuery {
    pub fn check(&self,_limits:&Limits)->Result<()>{match self{
        Self::Bounds{min,max,..}=>{finite(*min)?;finite(*max)?;if (0..3).any(|i|min[i]>max[i]){return Err(Error::Invalid("selection bounds"));}},
        Self::Ray{origin,direction,..}=>{finite(*origin)?;finite(*direction)?;normalized(*direction)?;},
        Self::Loop{edge,..}=>if edge.0>edge.1{return Err(Error::Invalid("selection edge pair"));},_=>{}
    }Ok(())}
}

fn has_target(op:&Operation,element:&str)->bool{match (element,op){
    ("vertices",Operation::Transform{..}|Operation::Weld{..}|Operation::Smooth{..}|Operation::Brush{..}|Operation::EdgeAttributes{..})=>true,
    ("faces",Operation::DeleteFaces{..}|Operation::AssignMaterial{..}|Operation::ProjectUv{..})=>true,
    ("face",Operation::Extrude{..}|Operation::Inset{..})=>true,
    (_,Operation::MeshEditing(op))=>match (element,&op.action){
        ("vertices",Action::Fill{..}|Action::Slide{..}|Action::Deform{..}|Action::Lattice{..}|Action::Shrinkwrap{..}|Action::Sculpt{..})=>true,
        ("faces",Action::Region{..}|Action::BoxUv{..}|Action::CylinderUv{..}|Action::FlipFaces{..}|Action::Unwrap{..}|Action::Pack{..}|Action::Relax{..})=>true,
        _=>false,
    },_=>false,
}}
pub fn retarget(op:&Operation,object:&str,element:&str,ids:&[u64])->Result<Operation>{
    if !has_target(op,element){return Err(Error::Invalid("selection template must be direct mesh edit"));}let mut out=op.clone();match &mut out{
    Operation::Transform{object:target,vertices,..}|Operation::Weld{object:target,vertices,..}|Operation::Smooth{object:target,vertices,..}|Operation::Brush{object:target,vertices,..}=>{*target=object.into();*vertices=ids.iter().copied().map(VertexId).collect();},
    Operation::DeleteFaces{object:target,faces,..}|Operation::AssignMaterial{object:target,faces,..}|Operation::ProjectUv{object:target,faces,..}=>{*target=object.into();*faces=ids.iter().copied().map(FaceId).collect();},
    Operation::Extrude{object:target,face,..}|Operation::Inset{object:target,face,..}=>{*target=object.into();if ids.len()!=1{return Err(Error::Invalid("selection required cardinality"));}*face=FaceId(ids[0]);},
    Operation::EdgeAttributes{object:target,edge,..}=>{if ids.len()!=2{return Err(Error::Invalid("edge needs two vertex IDs"));}*target=object.into();*edge=EdgeKey::new(VertexId(ids[0]),VertexId(ids[1]));},
    Operation::MeshEditing(op)=>{op.object=object.into();match &mut op.action{
        Action::Fill{vertices,..}|Action::Slide{vertices,..}|Action::Deform{vertices,..}|Action::Lattice{vertices,..}|Action::Shrinkwrap{vertices,..}|Action::Sculpt{vertices,..}=>{*vertices=ids.iter().copied().map(VertexId).collect();},
        Action::Region{faces,..}|Action::BoxUv{faces,..}|Action::CylinderUv{faces,..}|Action::FlipFaces{faces,..}|Action::Unwrap{faces,..}|Action::Pack{faces,..}|Action::Relax{faces,..}=>{*faces=ids.iter().copied().map(FaceId).collect();},
        _=>unreachable!(),
    }},_=>unreachable!(),
    }Ok(out)
}
// Templates use Operation::write's tags/fields. This restricted decoder skips
// child check: empty selections and FaceId(0) are placeholders until retarget.
// Reject other tags before decoding, so templates cannot recurse into Use.
fn read_template(r:&mut Reader,limits:&Limits)->Result<Operation>{
    let tag=r.u8()?;
    if tag==28{return Ok(Operation::MeshEditing(crate::MeshEditingOperation::read_template(r,limits)?));}
    if ![4,5,6,10,12,13,14,22,24,25].contains(&tag){return Err(Error::Corrupt("selection template tag"));}
    let object=r.string(limits.max_name_bytes)?;Ok(match tag{
        4=>Operation::Transform{object,vertices:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push(VertexId(r.u64()?));}values},matrix:[[r.f64()?,r.f64()?,r.f64()?,r.f64()?],[r.f64()?,r.f64()?,r.f64()?,r.f64()?],[r.f64()?,r.f64()?,r.f64()?,r.f64()?],[r.f64()?,r.f64()?,r.f64()?,r.f64()?]]},
        5=>Operation::Extrude{object,face:FaceId(r.u64()?),offset:[r.f64()?,r.f64()?,r.f64()?]},
        6=>Operation::DeleteFaces{object,faces:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_faces)?{values.push(FaceId(r.u64()?));}values}},
        10=>Operation::AssignMaterial{object,faces:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_faces)?{values.push(FaceId(r.u64()?));}values},material:r.u32()?},
        12=>Operation::Inset{object,face:FaceId(r.u64()?),distance:r.f64()?},
        13=>Operation::Weld{object,vertices:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push(VertexId(r.u64()?));}values},distance:r.f64()?},
        22=>Operation::Smooth{object,vertices:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push(VertexId(r.u64()?));}values},iterations:r.u32()?,factor:r.f64()?,preserve_boundary:read_bool(r)?},
        24=>Operation::ProjectUv{object,faces:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_faces)?{values.push(FaceId(r.u64()?));}values},axis:r.u8()?,scale:[r.f64()?,r.f64()?],offset:[r.f64()?,r.f64()?]},
        25=>Operation::Brush{object,vertices:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push(VertexId(r.u64()?));}values},center:[r.f64()?,r.f64()?,r.f64()?],radius:r.f64()?,delta:[r.f64()?,r.f64()?,r.f64()?],max_displacement:r.f64()?},
        14=>Operation::EdgeAttributes{object,edge:EdgeKey(VertexId(r.u64()?),VertexId(r.u64()?)),attributes:mesh::EdgeAttributes{seam:read_bool(r)?,crease:r.f64()?}},
        _=>unreachable!(),
    })
}

fn check_resolved(op:&Operation,limits:&Limits)->Result<()>{
    match op{
        Operation::MeshEditing(op)=>op.check(limits)?,
        Operation::Transform{vertices,matrix,..}=>{check_count(vertices.len(),limits.mesh.max_vertices)?;finite(matrix.iter().flatten().copied())?;},
        Operation::Extrude{offset,..}=>finite(*offset)?,Operation::Inset{distance,..}=>finite([*distance])?,
        Operation::DeleteFaces{faces,..}|Operation::AssignMaterial{faces,..}=>check_count(faces.len(),limits.mesh.max_faces)?,
        Operation::Weld{vertices,distance,..}=>{check_count(vertices.len(),limits.mesh.max_vertices)?;finite([*distance])?;},
        Operation::EdgeAttributes{attributes,..}=>finite([attributes.crease])?,
        Operation::Smooth{vertices,factor,..}=>{check_count(vertices.len(),limits.mesh.max_vertices)?;finite([*factor])?;},
        Operation::ProjectUv{faces,axis,scale,offset,..}=>{if *axis>2{return Err(Error::Invalid("projection axis must be 0, 1 or 2"));}check_count(faces.len(),limits.mesh.max_faces)?;finite(*scale)?;finite(*offset)?;},
        Operation::Brush{vertices,center,radius,delta,max_displacement,..}=>{check_count(vertices.len(),limits.mesh.max_vertices)?;finite(*center)?;finite([*radius,*max_displacement])?;finite(*delta)?;},
        _=>return Err(Error::Invalid("selection template must be direct mesh edit")),
    }Ok(())
}

#[cfg(test)]
mod typed_tests {
    use super::*;
    fn roundtrip(op:SelectionOperation){let limits=Limits::default();op.check(&limits).unwrap();let mut w=Writer::new(usize::MAX);op.write(&mut w).unwrap();let mut r=Reader::new(&w.bytes);assert_eq!(SelectionOperation::read(&mut r,&limits).unwrap(),op);r.end().unwrap();assert_eq!(op.memory_bytes(),w.bytes.len()*3+128);}
    fn using(op:Operation,element:&str)->SelectionOperation{SelectionOperation::Use{object:"body".into(),name:"group".into(),element:element.into(),operation:Box::new(op)}}
    #[test]
    fn selection_literals_and_query_variants(){
        roundtrip(SelectionOperation::Set{object:"body".into(),name:"group".into(),vertices:vec![VertexId(u64::MAX),VertexId(1)],faces:vec![FaceId(3)]});
        roundtrip(SelectionOperation::Delete{object:"body".into(),name:"group".into()});
        for query in [SelectionQuery::All,SelectionQuery::Material(u32::MAX),SelectionQuery::Bounds{min:[-1.;3],max:[1.;3],world:true},SelectionQuery::Connected{face:FaceId(u64::MAX),stop_seams:true},SelectionQuery::Loop{edge:EdgeKey(VertexId(1),VertexId(u64::MAX)),ring:true},SelectionQuery::Ray{origin:[0.,0.,5.],direction:[0.,0.,-2.],world:false}]{roundtrip(SelectionOperation::Query{object:"body".into(),name:"group".into(),query,element:"vertices".into(),min:0,max:100});}
        roundtrip(SelectionOperation::Set{object:"body".into(),name:"group".into(),vertices:vec![VertexId(2),VertexId(1)],faces:vec![]});
        roundtrip(using(Operation::Extrude{object:"body".into(),face:FaceId(0),offset:[0.,1.,0.]},"face"));
        roundtrip(SelectionOperation::Query{object:"body".into(),name:"group".into(),query:SelectionQuery::Loop{edge:EdgeKey(VertexId(1),VertexId(2)),ring:true},element:"faces".into(),min:0,max:3});
    }
    #[test]
    fn template_tags_retarget_and_preserve_placeholders(){
        let mut templates=vec![
            ("vertices",Operation::Transform{object:"placeholder".into(),vertices:vec![],matrix:IDENTITY_MATRIX}),
            ("face",Operation::Extrude{object:"placeholder".into(),face:FaceId(0),offset:[0.,1.,0.]}),
            ("faces",Operation::DeleteFaces{object:"placeholder".into(),faces:vec![]}),
            ("faces",Operation::AssignMaterial{object:"placeholder".into(),faces:vec![],material:1}),
            ("face",Operation::Inset{object:"placeholder".into(),face:FaceId(0),distance:0.1}),
            ("vertices",Operation::Weld{object:"placeholder".into(),vertices:vec![],distance:0.01}),
            ("vertices",Operation::EdgeAttributes{object:"placeholder".into(),edge:EdgeKey(VertexId(0),VertexId(0)),attributes:mesh::EdgeAttributes{seam:true,crease:0.5}}),
            ("vertices",Operation::Smooth{object:"placeholder".into(),vertices:vec![],iterations:1,factor:0.5,preserve_boundary:true}),
            ("faces",Operation::ProjectUv{object:"placeholder".into(),faces:vec![],axis:2,scale:[1.;2],offset:[0.;2]}),
            ("vertices",Operation::Brush{object:"placeholder".into(),vertices:vec![],center:[0.;3],radius:1.,delta:[0.,1.,0.],max_displacement:0.5}),
        ];
        for action in crate::mesh_editing::typed_tests::actions(){let op=Operation::MeshEditing(crate::MeshEditingOperation{object:"placeholder".into(),action});for element in ["vertices","faces"]{if has_target(&op,element){templates.push((element,op.clone()));}}}
        for (element,op) in templates{let ids=if element=="face"{vec![7]}else{vec![7,8]};let resolved=retarget(&op,"body",element,&ids).unwrap();assert_eq!(resolved.object_name(),Some("body"));check_resolved(&resolved,&Limits::default()).unwrap();let mut w=Writer::new(usize::MAX);resolved.write(&mut w).unwrap();let mut r=Reader::new(&w.bytes);assert_eq!(Operation::read(&mut r,&Limits::default()).unwrap(),resolved);r.end().unwrap();roundtrip(using(op,element));roundtrip(using(resolved,element));}
    }
    #[test]
    fn template_validation_is_deferred_until_apply(){
        let limits=Limits::default();
        let op=using(Operation::MeshEditing(crate::MeshEditingOperation {object:"body".into(),action:Action::Lattice {
            vertices:vec![],bounds:[[0.;3],[1.;3]],divisions:[1,2,2],displacements:vec![],
        }}),"vertices");
        roundtrip(op.clone());let SelectionOperation::Use{operation,..}=op else{unreachable!()};let resolved=retarget(&operation,"body","vertices",&[1]).unwrap();assert!(check_resolved(&resolved,&limits).is_err());
        assert!(using(Operation::Cube{object:"body".into(),size:[1.;3]},"vertices").check(&limits).is_err());
        assert!(using(Operation::Selection(SelectionOperation::Delete{object:"body".into(),name:"group".into()}),"faces").check(&limits).is_err());
        assert!(using(Operation::DeleteFaces{object:"body".into(),faces:vec![]},"vertices").check(&limits).is_err());
        assert!(SelectionQuery::Ray{origin:[0.;3],direction:[0.;3],world:false}.check(&limits).is_err());
        assert!(SelectionQuery::Bounds{min:[1.;3],max:[0.;3],world:false}.check(&limits).is_err());
        let mut w=Writer::new(1024);w.u8(3).unwrap();w.string("body").unwrap();w.string("group").unwrap();w.string("faces").unwrap();w.u8(31).unwrap();assert!(matches!(SelectionOperation::read(&mut Reader::new(&w.bytes),&limits),Err(Error::Corrupt("selection template tag"))));
    }
}
