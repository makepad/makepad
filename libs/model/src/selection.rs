//! Explicit revision-bound named selections. Queries run against a worker-owned
//! document; no hidden viewport selection participates in an operation.
use crate::{document::State,mesh,Error,Result,Limits,Operation,OperationResult,json::{self,Value},schema::*,service::{fields,need,text,integer,selections,array,stable_id},transform::*};
use std::collections::{BTreeMap,BTreeSet,VecDeque};
#[derive(Clone,Debug,PartialEq)] pub struct SelectionGroup{pub object:String,pub name:String,pub vertices:Vec<mesh::VertexId>,pub faces:Vec<mesh::FaceId>,pub stale:bool}
#[derive(Clone,Debug,Default,PartialEq)] pub struct SelectionState{pub groups:BTreeMap<(String,String),SelectionGroup>}
#[derive(Clone,Debug,PartialEq)] pub enum SelectionQuery{All,Material(u32),Bounds{min:[f64;3],max:[f64;3],world:bool},Connected{face:mesh::FaceId,stop_seams:bool},Loop{edge:mesh::EdgeKey,ring:bool},Ray{origin:[f64;3],direction:[f64;3],world:bool}}
#[derive(Clone,Debug,PartialEq)] pub enum SelectionOperation{
    Set{object:String,name:String,vertices:Vec<mesh::VertexId>,faces:Vec<mesh::FaceId>},
    Query{object:String,name:String,query:SelectionQuery,element:String,min:usize,max:usize},
    Delete{object:String,name:String},Use{object:String,name:String,element:String,operation:Value},
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
    pub fn memory_bytes(&self)->usize{self.value().to_json().len().saturating_mul(3)+128}
    pub fn parse(v:&Value,limits:&Limits)->Result<Option<Self>>{
        let op=text(v,"op")?;if !["selection","select","delete_selection","use_selection"].contains(&op){return Ok(None);}
        let object=text(v,"object")?.to_owned();let n=text(v,"name")?.to_owned();name(&object,limits)?;name(&n,limits)?;
        Ok(Some(match op{
            "selection"=>{fields(v,&["op","object","name","vertices","faces"])?;let vertices=selections(need(v,"vertices")?,limits.mesh.max_vertices)?.into_iter().map(mesh::VertexId).collect();let faces=selections(need(v,"faces")?,limits.mesh.max_faces)?.into_iter().map(mesh::FaceId).collect();Self::Set{object,name:n,vertices,faces}},
            "delete_selection"=>{fields(v,&["op","object","name"])?;Self::Delete{object,name:n}},
            "use_selection"=>{fields(v,&["op","object","name","element","operation"])?;let element=text(v,"element")?.to_owned();if !["vertices","faces","face"].contains(&element.as_str()){return Err(Error::Invalid("selection element"));}let operation=need(v,"operation")?.clone();if !matches!(operation,Value::Obj(_)){return Err(Error::Invalid("selection operation template"));}Self::Use{object,name:n,element,operation}},
            _=>{fields(v,&["op","object","name","query","element","min","max"])?;let element=text(v,"element")?.to_owned();if !["vertices","faces"].contains(&element.as_str()){return Err(Error::Invalid("query element"));}let min=integer(need(v,"min")?)? as usize;let max=integer(need(v,"max")?)? as usize;if min>max||max>limits.mesh.max_vertices.max(limits.mesh.max_faces){return Err(Error::Invalid("selection cardinality"));}
                let q=need(v,"query")?;let query=match text(q,"kind")?{
                    "all"=>{fields(q,&["kind"])?;SelectionQuery::All},
                    "material"=>{fields(q,&["kind","material"])?;SelectionQuery::Material(integer(need(q,"material")?)?)},
                    "bounds"=>{fields(q,&["kind","min","max","space"])?;let min:[f64;3]=array(need(q,"min")?)?;let max:[f64;3]=array(need(q,"max")?)?;if (0..3).any(|i|min[i]>max[i]){return Err(Error::Invalid("selection bounds"));}SelectionQuery::Bounds{min,max,world:space(q)?}},
                    "connected"=>{fields(q,&["kind","face","stop_seams"])?;SelectionQuery::Connected{face:mesh::FaceId(stable_id(need(q,"face")?)?),stop_seams:boolean(need(q,"stop_seams")?)?}},
                    "loop"|"ring"=>{fields(q,&["kind","edge"])?;let a=need(q,"edge")?.as_arr().filter(|a|a.len()==2).ok_or(Error::Invalid("selection edge pair"))?;SelectionQuery::Loop{edge:mesh::EdgeKey::new(mesh::VertexId(stable_id(&a[0])?),mesh::VertexId(stable_id(&a[1])?)),ring:text(q,"kind")?=="ring"}},
                    "ray"=>{fields(q,&["kind","origin","direction","space"])?;let direction=array(need(q,"direction")?)?;normalized(direction)?;SelectionQuery::Ray{origin:array(need(q,"origin")?)?,direction,world:space(q)?}},
                    _=>return Err(Error::Invalid("unknown selection query")),
                };Self::Query{object,name:n,query,element,min,max}}
        }))
    }
    pub fn value(&self)->Value{let(op,n,extra)=match self{
        Self::Set{name,vertices,faces,..}=>("selection",name,vec![("vertices",ids(vertices.iter().map(|v|v.0))),("faces",ids(faces.iter().map(|v|v.0)))]),
        Self::Delete{name,..}=>("delete_selection",name,vec![]),Self::Use{name,element,operation,..}=>("use_selection",name,vec![("element",json::s(element)),("operation",operation.clone())]),
        Self::Query{name,query,element,min,max,..}=>("select",name,vec![("query",query.value()),("element",json::s(element)),("min",Value::Int(*min as i64)),("max",Value::Int(*max as i64))]),
    };let mut out=vec![("op",json::s(op)),("object",json::s(self.object_name())),("name",json::s(n))];out.extend(extra);json::obj(out)}
    pub(crate) fn apply(&self,state:&mut State,limits:&Limits,ctx:&mut mesh::Context<'_>)->Result<OperationResult>{
        let object=self.object_name();let m=state.objects.get(object).ok_or_else(||Error::MissingObject(object.into()))?;
        match self{
            Self::Use{name,element,operation,..}=>{
                let g=state.selections.groups.get(&(object.into(),name.clone())).ok_or(Error::Invalid("unknown named selection"))?;
                if g.stale||g.vertices.iter().any(|v|m.vertex(*v).is_none())||g.faces.iter().any(|f|m.face(*f).is_none()){return Err(Error::Invalid("named selection stale; reselect against current head"));}
                let Value::Obj(mut op)=operation.clone()else{return Err(Error::Invalid("selection template object"));};if op.iter().any(|(k,_)|k=="object"||k==element){return Err(Error::Invalid("selection template overrides target"));}
                let selected=match element.as_str(){"vertices" if !g.vertices.is_empty()=>ids(g.vertices.iter().map(|v|v.0)),"faces" if !g.faces.is_empty()=>ids(g.faces.iter().map(|f|f.0)),"face" if g.faces.len()==1=>json::s(g.faces[0].0.to_string()),_=>return Err(Error::Invalid("selection required cardinality"))};
                op.push(("object".into(),json::s(object)));op.push((element.clone(),selected));let resolved=crate::parse_operations(&Value::Arr(vec![Value::Obj(op)]),limits)?;
                if matches!(resolved[0],Operation::Selection(_)|Operation::Scene(_)|Operation::Rig(_)|Operation::Surface(_)|Operation::Construction(_)){return Err(Error::Invalid("selection template must be direct mesh edit"));}
                let mut results=crate::document::execute(state,&resolved,limits,ctx)?;Ok(results.remove(0))
            }
            Self::Delete{name,..}=>{if state.selections.groups.remove(&(object.into(),name.clone())).is_none(){return Err(Error::Invalid("unknown named selection"));}Ok(OperationResult::default())}
            Self::Set{name,vertices,faces,..}=>save(state,object,name,vertices.clone(),faces.clone(),limits),
            Self::Query{name,query,element,min,max,..}=>{if min>max||*max>limits.mesh.max_vertices.max(limits.mesh.max_faces)||!["vertices","faces"].contains(&element.as_str()){return Err(Error::Invalid("selection query cardinality"));}let(vertices,faces)=query.evaluate(state,object,ctx)?;let count=if element=="vertices"{vertices.len()}else{faces.len()};if count<*min||count>*max{return Err(Error::Invalid("selection cardinality mismatch"));}save(state,object,name,vertices,faces,limits)}
        }
    }
}
fn save(state:&mut State,object:&str,name:&str,mut vertices:Vec<mesh::VertexId>,mut faces:Vec<mesh::FaceId>,limits:&Limits)->Result<OperationResult>{vertices.sort();vertices.dedup();faces.sort();faces.dedup();let m=&state.objects[object];if vertices.iter().any(|v|m.vertex(*v).is_none())||faces.iter().any(|f|m.face(*f).is_none()){return Err(Error::Invalid("selection element missing"));}
    if state.selections.groups.len()>=128&&!state.selections.groups.contains_key(&(object.into(),name.into())){return Err(Error::Budget("named selections"));}
    let result=OperationResult{object:object.into(),vertices:vertices.clone(),faces:faces.clone(),..Default::default()};state.selections.groups.insert((object.into(),name.into()),SelectionGroup{object:object.into(),name:name.into(),vertices,faces,stale:false});state.selections.validate(state,limits)?;Ok(result)}
fn space(v:&Value)->Result<bool>{match text(v,"space")?{"local"=>Ok(false),"world"=>Ok(true),_=>Err(Error::Invalid("selection coordinate space"))}}
fn ids(v:impl Iterator<Item=u64>)->Value{Value::Arr(v.map(|v|json::s(v.to_string())).collect())}
impl SelectionQuery{
    fn value(&self)->Value{json::obj(match self{
        Self::All=>vec![("kind",json::s("all"))],Self::Material(id)=>vec![("kind",json::s("material")),("material",Value::Int(*id as i64))],
        Self::Bounds{min,max,world}=>vec![("kind",json::s("bounds")),("min",vec_value(min)),("max",vec_value(max)),("space",json::s(if *world{"world"}else{"local"}))],
        Self::Connected{face,stop_seams}=>vec![("kind",json::s("connected")),("face",json::s(face.0.to_string())),("stop_seams",Value::Bool(*stop_seams))],
        Self::Loop{edge,ring}=>vec![("kind",json::s(if *ring{"ring"}else{"loop"})),("edge",ids([edge.0.0,edge.1.0].into_iter()))],
        Self::Ray{origin,direction,world}=>vec![("kind",json::s("ray")),("origin",vec_value(origin)),("direction",vec_value(direction)),("space",json::s(if *world{"world"}else{"local"}))],
    })}
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
