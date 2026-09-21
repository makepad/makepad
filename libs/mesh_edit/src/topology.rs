//! Local topology operations. Each operation stages one candidate and verifies
//! the complete result before publication; no partial edits survive failure.
use crate::{context::invalid, geometry::*, mesh::check_position, modifiers::{select_faces,invalidate_normals}, ops::normalize_weights, *};
use std::collections::{BTreeMap,BTreeSet};

pub(crate) fn surface(mesh:&Mesh,ctx:&mut Context<'_>)->Result<Adjacency> {
    let report=mesh.validate(ctx)?;
    if !report.is_valid_surface || report.loose_edges!=0 || report.non_manifold_vertices!=0 {
        return Err(invalid("operation requires an oriented manifold surface without loose edges"));
    }
    mesh.adjacency(ctx)
}
pub(crate) fn replace_corners(mesh:&mut Mesh,replacements:&BTreeMap<FaceId,Vec<Corner>>,ctx:&mut Context<'_>)->Result<()> {
    let mut corners=Vec::new();
    for face in &mut mesh.faces {
        ctx.checkpoint(1)?;
        let source=replacements.get(&face.id).map(Vec::as_slice).unwrap_or_else(||
            &mesh.corners[face.first_corner as usize..(face.first_corner+face.corner_count) as usize]);
        let start=u32::try_from(corners.len()).map_err(|_|invalid("corner count overflow"))?;
        corners.extend_from_slice(source);face.first_corner=start;face.corner_count=source.len() as u32;
    }
    mesh.corners=corners;Ok(())
}
pub(crate) fn blended_vertex(mesh:&Mesh,a:VertexId,b:VertexId,t:f64,ctx:&mut Context<'_>)->Result<([f64;3],Vec<JointWeight>)> {
    let a=mesh.vertex(a).ok_or(MeshError::UnknownElement(ElementId::Vertex(a)))?;
    let b=mesh.vertex(b).ok_or(MeshError::UnknownElement(ElementId::Vertex(b)))?;
    let p=add(mul(a.position,1.-t),mul(b.position,t));check_position(p)?;
    let mut merged=BTreeMap::<u32,f64>::new();for (v,f) in [(a,1.-t),(b,t)]{for w in &v.weights{ctx.checkpoint(1)?;*merged.entry(w.joint).or_default()+=w.weight*f;}}
    let weights=merged.into_iter().map(|(joint,weight)|JointWeight{joint,weight}).collect::<Vec<_>>();
    Ok((p,normalize_weights(&weights,ctx)?))
}
fn factor(t:f64)->Result<()> {if t.is_finite()&&t>0.&&t<1. {Ok(())}else{Err(invalid("edge factor must be strictly between zero and one"))}}
fn selected_edges(mesh:&Mesh,edges:&[EdgeKey],ctx:&mut Context<'_>)->Result<(Adjacency,BTreeSet<EdgeKey>)> {
    ctx.limit("selected edges",edges.len() as u64,ctx.limits.max_edges as u64)?;ctx.checkpoint(edges.len() as u64)?;
    let set=edges.iter().copied().collect::<BTreeSet<_>>();
    if set.is_empty()||set.len()!=edges.len(){return Err(invalid("edge selection must be nonempty and unique"));}
    let adj=mesh.adjacency(ctx)?;
    for edge in &set {if edge.0>=edge.1||!adj.edges.contains_key(edge){return Err(MeshError::UnknownElement(ElementId::Edge(*edge)));}}
    Ok((adj,set))
}

impl Mesh {
    /// Extrudes a connected or disconnected manifold patch. Shared interior
    /// edges stay shared; only its boundary produces walls. Cap identities stay.
    pub fn extrude_region(&mut self,faces:&[FaceId],offset:[f64;3],ctx:&mut Context<'_>)->Result<RegionExtrudeResult> {
        check_position(offset)?;if length(offset)==0.{return Err(invalid("zero extrusion offset"));}
        let selected=select_faces(self,faces,ctx)?;if selected.is_empty(){return Err(invalid("empty extrusion region"));}
        let adjacency=surface(self,ctx)?;let mut boundary=Vec::new();let mut vertices=BTreeSet::new();
        for &face in &selected {
            let normal=face_geometry(self,face,ctx)?.normal;
            if dot(normal,offset).abs()<=length(offset)*1e-10{return Err(invalid("region offset must leave every selected face plane"));}
            let corners=self.face_corners(face)?;
            vertices.extend(corners.iter().map(|c|c.vertex));
            for i in 0..corners.len(){let a=corners[i].vertex;let b=corners[(i+1)%corners.len()].vertex;
                let uses=adjacency.radial(EdgeKey::new(a,b));
                if uses.iter().filter(|u|selected.contains(&u.face)).count()==1 {boundary.push((face,a,b));}
            }
        }
        let rim=boundary.iter().flat_map(|(_,a,b)|[*a,*b]).collect::<BTreeSet<_>>();
        // A patch touching another patch only at a vertex would create a bowtie.
        let mut degree=BTreeMap::<VertexId,usize>::new();for &(_,a,b) in &boundary{ctx.checkpoint(1)?;*degree.entry(a).or_default()+=1;*degree.entry(b).or_default()+=1;}
        if degree.values().any(|&n|n!=2){return Err(invalid("extrusion boundary must be disjoint simple cycles"));}
        ctx.counts(self.vertices.len()+rim.len(),self.faces.len()+boundary.len(),self.corners.len()+4*boundary.len(),self.corners.len()+4*boundary.len())?;
        let (side_faces,changes)=self.edit(ctx,rim.len().saturating_mul(8192)+boundary.len().saturating_mul(1024),|m,ctx|{
            let mut mapping=BTreeMap::new();let mut remaps=Vec::new();
            for &v in &vertices {ctx.checkpoint(1)?;let source=m.vertex(v).unwrap().clone();let p=add(source.position,offset);check_position(p)?;
                if rim.contains(&v){let new=m.push_vertex(p,source.weights)?;mapping.insert(v,new);remaps.push((ElementId::Vertex(v),vec![ElementId::Vertex(v),ElementId::Vertex(new)]));}
                else {let i=m.vertices.binary_search_by_key(&v,|v|v.id).unwrap();m.vertices[i].position=p;mapping.insert(v,v);}
            }
            let mut sides=Vec::new();
            for &(face,a,b) in &boundary {ctx.checkpoint(1)?;let width=length(sub(m.vertex(a).unwrap().position,m.vertex(b).unwrap().position));
                let side=m.push_face(&[(a,[0.,0.],None),(b,[width,0.],None),(mapping[&b],[width,length(offset)],None),(mapping[&a],[0.,length(offset)],None)],m.face(face).unwrap().material)?;sides.push(side);
            }
            for face in &m.faces {if selected.contains(&face.id){for c in &mut m.corners[face.first_corner as usize..(face.first_corner+face.corner_count)as usize]{c.vertex=mapping[&c.vertex];c.normal=None;}}}
            // Transfer every selected cap edge, including interior creases.
            for (&edge,uses) in &adjacency.edges {if uses.iter().any(|u|selected.contains(&u.face)){
                let new=EdgeKey::new(mapping[&edge.0],mapping[&edge.1]);
                if let Some(data)=m.edge_data.get(&edge).copied(){m.edge_data.insert(new,data);}
                if new!=edge{remaps.push((ElementId::Edge(edge),vec![ElementId::Edge(edge),ElementId::Edge(new)]));}
            }}
            m.prune_edge_attributes();surface(m,ctx)?;Ok((sides,remaps))
        })?;
        Ok(RegionExtrudeResult{caps:selected.into_iter().collect(),side_faces,changes})
    }

    /// Inserts one shared vertex per edge and interpolates each incident face's
    /// UV independently. Edge markers propagate to both children.
    pub fn split_edges(&mut self,edges:&[EdgeKey],t:f64,ctx:&mut Context<'_>)->Result<ChangeSet> {
        factor(t)?;let (adj,selected)=selected_edges(self,edges,ctx)?;
        let extra=selected.iter().map(|e|adj.radial(*e).len()).sum::<usize>();
        ctx.counts(self.vertices.len()+selected.len(),self.faces.len(),self.corners.len()+extra,adj.edges.len()+selected.len())?;
        Ok(self.edit(ctx,selected.len().saturating_mul(8192)+extra.saturating_mul(256),|m,ctx|{
            let mut mids=BTreeMap::new();let mut remaps=Vec::new();
            for &edge in &selected {ctx.checkpoint(1)?;let (p,w)=blended_vertex(m,edge.0,edge.1,t,ctx)?;let id=m.push_vertex(p,w)?;mids.insert(edge,id);
                let children=[EdgeKey::new(edge.0,id),EdgeKey::new(id,edge.1)];
                if let Some(data)=m.edge_data.remove(&edge){for child in children{m.edge_data.insert(child,data);}}
                remaps.push((ElementId::Edge(edge),children.into_iter().map(ElementId::Edge).collect()));
            }
            let mut replacements=BTreeMap::new();
            for face in m.faces.clone(){let source=m.face_corners(face.id)?.to_vec();let mut next=Vec::new();
                for i in 0..source.len(){ctx.checkpoint(1)?;let a=&source[i];let b=&source[(i+1)%source.len()];next.push(a.clone());
                    let edge=EdgeKey::new(a.vertex,b.vertex);if let Some(&vertex)=mids.get(&edge){let f=if a.vertex==edge.0{t}else{1.-t};
                        next.push(Corner{id:CornerId(m.allocate_id()?),vertex,uv:std::array::from_fn(|d|a.uv[d]*(1.-f)+b.uv[d]*f),normal:None});
                    }
                }replacements.insert(face.id,next);
            }
            replace_corners(m,&replacements,ctx)?;Ok(((),remaps))
        })?.1)
    }

    /// Collapses one unmarked edge. Incident triangles disappear; surviving
    /// faces keep their IDs. UV discontinuities remain per corner. Topology or
    /// local orientation changes outside that collapse are refused.
    pub fn collapse_edge(&mut self,edge:EdgeKey,t:f64,ctx:&mut Context<'_>)->Result<ChangeSet> {
        factor(t)?;let adj=surface(self,ctx)?;
        if adj.radial(edge).is_empty(){return Err(MeshError::UnknownElement(ElementId::Edge(edge)));}
        if self.edge_data.keys().any(|e|[e.0,e.1].iter().any(|v|*v==edge.0||*v==edge.1)){return Err(invalid("collapse requires an unmarked edge neighborhood"));}
        Ok(self.edit(ctx,self.corners.len().saturating_mul(192),|m,ctx|{
            let (p,w)=blended_vertex(m,edge.0,edge.1,t,ctx)?;let mut remove=BTreeSet::new();let mut replacements=BTreeMap::new();let mut normals=BTreeMap::new();
            for face in &m.faces {let source=m.face_corners(face.id)?;if !source.iter().any(|c|c.vertex==edge.0||c.vertex==edge.1){continue;}
                normals.insert(face.id,face_geometry(m,face.id,ctx)?.normal);let mut cs=source.to_vec();for c in &mut cs{if c.vertex==edge.1{c.vertex=edge.0;}c.normal=None;}
                cs.dedup_by_key(|c|c.vertex);if cs.len()>1&&cs[0].vertex==cs.last().unwrap().vertex{cs.pop();}
                if cs.len()<3{remove.insert(face.id);}else if cs.iter().map(|c|c.vertex).collect::<BTreeSet<_>>().len()!=cs.len(){return Err(invalid("collapse would pinch a face"));}else{replacements.insert(face.id,cs);}
            }
            replace_corners(m,&replacements,ctx)?;m.repack_faces(&remove,ctx)?;
            m.vertices.retain(|v|v.id!=edge.1);let i=m.vertices.binary_search_by_key(&edge.0,|v|v.id).unwrap();m.vertices[i].position=p;m.vertices[i].weights=w;
            for (face,normal) in normals{if !remove.contains(&face)&&dot(normal,face_geometry(m,face,ctx)?.normal)<=1e-8{return Err(invalid("collapse would reverse a face"));}}
            surface(m,ctx)?;Ok(((),vec![(ElementId::Vertex(edge.1),vec![ElementId::Vertex(edge.0)])]))
        })?.1)
    }

    /// Removes a shared edge between two coplanar same-material faces. A UV
    /// discontinuity or authored edge marker is an explicit refusal.
    pub fn dissolve_edge(&mut self,edge:EdgeKey,ctx:&mut Context<'_>)->Result<ChangeSet> {
        let adj=surface(self,ctx)?;let uses=adj.radial(edge);
        if uses.len()!=2{return Err(invalid("dissolve needs exactly two incident faces"));}
        if self.edge_data.contains_key(&edge){return Err(invalid("cannot dissolve a marked edge"));}
        let a=uses[0].face.min(uses[1].face);let b=uses[0].face.max(uses[1].face);
        if self.face(a).unwrap().material!=self.face(b).unwrap().material{return Err(invalid("cannot dissolve across material boundaries"));}
        let ga=face_geometry(self,a,ctx)?;let gb=face_geometry(self,b,ctx)?;
        if dot(ga.normal,gb.normal)<1.-1e-10{return Err(invalid("dissolve requires coplanar faces"));}
        for v in [edge.0,edge.1]{if self.face_corners(a)?.iter().find(|c|c.vertex==v).unwrap().uv!=self.face_corners(b)?.iter().find(|c|c.vertex==v).unwrap().uv{return Err(invalid("cannot dissolve a UV seam"));}}
        Ok(self.edit(ctx,(self.face_corners(a)?.len()+self.face_corners(b)?.len()).saturating_mul(256),|m,ctx|{
            let mut outgoing=BTreeMap::new();for f in [a,b]{let cs=m.face_corners(f)?;for i in 0..cs.len(){let next=cs[(i+1)%cs.len()].vertex;
                if EdgeKey::new(cs[i].vertex,next)!=edge&&outgoing.insert(cs[i].vertex,(cs[i].clone(),next)).is_some(){return Err(invalid("dissolve boundary is not a simple cycle"));}
            }}
            let start=*outgoing.keys().next().unwrap();let mut at=start;let mut merged=Vec::new();
            while let Some((mut c,next))=outgoing.remove(&at){ctx.checkpoint(1)?;c.normal=None;merged.push(c);at=next;if at==start{break;}}
            if at!=start||!outgoing.is_empty(){return Err(invalid("dissolve boundary is disconnected"));}
            replace_corners(m,&BTreeMap::from([(a,merged)]),ctx)?;m.repack_faces(&BTreeSet::from([b]),ctx)?;surface(m,ctx)?;
            Ok(((),vec![(ElementId::Face(b),vec![ElementId::Face(a)])]))
        })?.1)
    }

    pub fn fill_boundary(&mut self,vertices:&[VertexId],material:u32,ctx:&mut Context<'_>)->Result<ChangeSet> {
        let adj=surface(self,ctx)?;let cycle=boundary_cycle(&adj,vertices)?;
        Ok(self.edit(ctx,cycle.len().saturating_mul(512),|m,ctx|{
            let values=project_cycle(m,&cycle);m.push_face(&values,material)?;surface(m,ctx)?;Ok(((),Vec::new()))
        })?.1)
    }

    /// Connects equal-size disjoint boundary loops. The first loop determines
    /// winding; the second is reversed as necessary while retaining its start.
    pub fn bridge_boundaries(&mut self,a:&[VertexId],b:&[VertexId],material:u32,ctx:&mut Context<'_>)->Result<ChangeSet> {
        let adj=surface(self,ctx)?;let a=boundary_cycle(&adj,a)?;let mut b=boundary_cycle(&adj,b)?;
        if a.len()!=b.len()||a.iter().any(|v|b.contains(v)){return Err(invalid("bridge needs disjoint equal-size boundary loops"));}
        b[1..].reverse();let n=a.len();ctx.counts(self.vertices.len(),self.faces.len()+n,self.corners.len()+4*n,self.corners.len()+4*n)?;
        Ok(self.edit(ctx,n.saturating_mul(1024),|m,ctx|{
            for i in 0..n {ctx.checkpoint(1)?;let j=(i+1)%n;let u=i as f64/n as f64;let v=(i+1)as f64/n as f64;
                m.push_face(&[(a[i],[u,0.],None),(a[j],[v,0.],None),(b[j],[v,1.],None),(b[i],[u,1.],None)],material)?;
            }surface(m,ctx)?;Ok(((),Vec::new()))
        })?.1)
    }

    /// Slides selected vertices toward explicit adjacent vertices in one batch.
    /// Negative factors move away; [-1,1] bounds extrapolation. UVs stay fixed.
    pub fn slide_vertices(&mut self,vertices:&[VertexId],towards:&[VertexId],t:f64,ctx:&mut Context<'_>)->Result<ChangeSet> {
        if vertices.len()!=towards.len()||!t.is_finite()||!(-1.0..=1.0).contains(&t){return Err(invalid("slide needs paired vertices and factor in [-1,1]"));}
        let selected=self.selected_vertices(vertices,ctx)?;let adj=self.adjacency(ctx)?;let mut positions=BTreeMap::new();
        for (&a,&b) in vertices.iter().zip(towards){ctx.checkpoint(1)?;if adj.radial(EdgeKey::new(a,b)).is_empty(){return Err(invalid("slide targets must share an edge"));}
            let p=add(self.vertex(a).unwrap().position,mul(sub(self.vertex(b).unwrap().position,self.vertex(a).unwrap().position),t));check_position(p)?;positions.insert(a,p);
        }
        Ok(self.edit(ctx,vertices.len().saturating_mul(128),|m,ctx|{for v in &mut m.vertices{ctx.checkpoint(1)?;if let Some(&p)=positions.get(&v.id){v.position=p;}}
            invalidate_normals(m,&selected);Ok(((),Vec::new()))})?.1)
    }
}

fn boundary_cycle(adj:&Adjacency,vertices:&[VertexId])->Result<Vec<VertexId>> {
    if vertices.len()<3||vertices.iter().copied().collect::<BTreeSet<_>>().len()!=vertices.len(){return Err(invalid("boundary must be a unique cycle with at least three vertices"));}
    let mut same=None;
    for i in 0..vertices.len(){let a=vertices[i];let b=vertices[(i+1)%vertices.len()];let uses=adj.radial(EdgeKey::new(a,b));
        if uses.len()!=1{return Err(invalid("cycle contains a non-boundary edge"));}
        let direction=uses[0].from==a;if same.is_some_and(|s|s!=direction){return Err(invalid("inconsistent boundary winding"));}same=Some(direction);
    }
    let mut out=vertices.to_vec();if same==Some(true){out[1..].reverse();}Ok(out)
}
fn project_cycle(mesh:&Mesh,cycle:&[VertexId])->Vec<(VertexId,[f64;2],Option<[f64;3]>)> {
    let mut normal=[0.;3];for i in 0..cycle.len(){normal=add(normal,cross(mesh.vertex(cycle[i]).unwrap().position,mesh.vertex(cycle[(i+1)%cycle.len()]).unwrap().position));}
    let axis=(0..3).max_by(|&a,&b|normal[a].abs().total_cmp(&normal[b].abs())).unwrap();
    cycle.iter().map(|&v|{let p=mesh.vertex(v).unwrap().position;(v,[p[(axis+1)%3],p[(axis+2)%3]],None)}).collect()
}
