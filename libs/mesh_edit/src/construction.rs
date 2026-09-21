use crate::{context::invalid,geometry::*,mesh::check_position,topology::{surface,replace_corners,blended_vertex},*};
use std::collections::{BTreeMap,VecDeque};

impl Mesh {
    /// Cuts a maximal opposite-edge quad strip. Open strips stop at boundaries;
    /// a non-quad, inconsistent loop parameter, or crossed strip is refused.
    pub fn loop_cut(&mut self,seed:EdgeKey,factor:f64,ctx:&mut Context<'_>)->Result<ChangeSet> {
        if !factor.is_finite()||factor<=0.||factor>=1.{return Err(invalid("loop cut factor must be in (0,1)"));}
        let adj=surface(self,ctx)?;if adj.radial(seed).is_empty(){return Err(MeshError::UnknownElement(ElementId::Edge(seed)));}
        let mut factors=BTreeMap::from([(seed,factor)]);let mut faces=BTreeMap::<FaceId,usize>::new();let mut queue=VecDeque::from([seed]);
        while let Some(edge)=queue.pop_front(){ctx.checkpoint(1)?;for usage in adj.radial(edge){
            let cs=self.face_corners(usage.face)?;if cs.len()!=4{return Err(invalid("loop cut must stay in quads"));}
            let i=(0..4).find(|&i|EdgeKey::new(cs[i].vertex,cs[(i+1)%4].vertex)==edge).unwrap();
            if let Some(&old)=faces.get(&usage.face){if old%2!=i%2{return Err(invalid("loop strip crosses itself"));}}else{faces.insert(usage.face,i);}
            let opposite=EdgeKey::new(cs[(i+2)%4].vertex,cs[(i+3)%4].vertex);
            let forward=if cs[i].vertex==edge.0{factors[&edge]}else{1.-factors[&edge]};
            let t=if opposite.0==cs[(i+2)%4].vertex{1.-forward}else{forward};
            if let Some(&old)=factors.get(&opposite){if (old-t).abs()>1e-10{return Err(invalid("loop strip reverses its parameter"));}}
            else{factors.insert(opposite,t);queue.push_back(opposite);}
        }}
        ctx.counts(self.vertices.len()+factors.len(),self.faces.len()+faces.len(),self.corners.len()+4*faces.len(),self.corners.len()+4*faces.len())?;
        Ok(self.edit(ctx,factors.len().saturating_mul(8192)+faces.len().saturating_mul(2048),|m,ctx|{
            let mut mids=BTreeMap::new();let mut remaps=Vec::new();
            for (&edge,&t) in &factors{let(p,w)=blended_vertex(m,edge.0,edge.1,t,ctx)?;let id=m.push_vertex(p,w)?;mids.insert(edge,id);
                let children=[EdgeKey::new(edge.0,id),EdgeKey::new(id,edge.1)];if let Some(data)=m.edge_data.remove(&edge){for e in children{m.edge_data.insert(e,data);}}
                remaps.push((ElementId::Edge(edge),children.into_iter().map(ElementId::Edge).collect()));
            }
            let mut replacements=BTreeMap::new();
            for (&face,&i) in &faces{ctx.checkpoint(1)?;let source=m.face_corners(face)?.to_vec();let cs=(0..4).map(|j|source[(i+j)%4].clone()).collect::<Vec<_>>();
                let mut midpoint=|a:&Corner,b:&Corner|->Result<Corner>{let e=EdgeKey::new(a.vertex,b.vertex);let t=if a.vertex==e.0{factors[&e]}else{1.-factors[&e]};
                    Ok(Corner{id:CornerId(m.allocate_id()?),vertex:mids[&e],uv:std::array::from_fn(|d|a.uv[d]*(1.-t)+b.uv[d]*t),normal:None})};
                let ab=midpoint(&cs[0],&cs[1])?;let cd=midpoint(&cs[2],&cs[3])?;
                let ab2=Corner{id:CornerId(m.allocate_id()?),..ab.clone()};let cd2=Corner{id:CornerId(m.allocate_id()?),..cd.clone()};
                let mut left=vec![cs[0].clone(),ab,cd,cs[3].clone()];let mut right=vec![ab2,cs[1].clone(),cs[2].clone(),cd2];
                for c in left.iter_mut().chain(&mut right){c.normal=None;}
                let new=append_corners(m,right,m.face(face).unwrap().material)?;replacements.insert(face,left);
                remaps.push((ElementId::Face(face),vec![ElementId::Face(face),ElementId::Face(new)]));
            }
            replace_corners(m,&replacements,ctx)?;surface(m,ctx)?;Ok(((),remaps))
        })?.1)
    }

    /// Creates `count` total copies (including the original), spaced by offset.
    /// UVs, split normals, materials, sparse weights and edge records are copied.
    pub fn array(&mut self,count:u32,offset:[f64;3],ctx:&mut Context<'_>)->Result<ChangeSet> {
        if !(1..=128).contains(&count){return Err(invalid("array count must be 1..128"));}check_position(offset)?;
        let n=count as usize;ctx.counts(self.vertices.len().saturating_mul(n),self.faces.len().saturating_mul(n),self.corners.len().saturating_mul(n),self.corners.len().saturating_mul(n)+self.edge_data.len().saturating_mul(n))?;
        Ok(self.edit(ctx,self.memory_bytes().saturating_mul(n+1),|m,ctx|{
            let source=m.clone();let mut mappings=BTreeMap::<ElementId,Vec<ElementId>>::new();
            for copy in 1..count{ctx.checkpoint(1)?;let mut vertices=BTreeMap::new();
                for v in &source.vertices{ctx.checkpoint(1)?;let p=add(v.position,mul(offset,copy as f64));check_position(p)?;let new=m.push_vertex(p,v.weights.clone())?;vertices.insert(v.id,new);
                    mappings.entry(ElementId::Vertex(v.id)).or_insert_with(||vec![ElementId::Vertex(v.id)]).push(ElementId::Vertex(new));}
                for face in &source.faces{let cs=source.face_corners(face.id)?;let values=cs.iter().map(|c|(vertices[&c.vertex],c.uv,c.normal)).collect::<Vec<_>>();let new=m.push_face(&values,face.material)?;
                    mappings.entry(ElementId::Face(face.id)).or_insert_with(||vec![ElementId::Face(face.id)]).push(ElementId::Face(new));
                    for (a,b) in cs.iter().zip(m.face_corners(new)?){mappings.entry(ElementId::Corner(a.id)).or_insert_with(||vec![ElementId::Corner(a.id)]).push(ElementId::Corner(b.id));}
                }
                for (&edge,data) in &source.edge_data{let new=EdgeKey::new(vertices[&edge.0],vertices[&edge.1]);m.edge_data.insert(new,*data);}
                for edge in source.adjacency(ctx)?.edges.keys(){let new=EdgeKey::new(vertices[&edge.0],vertices[&edge.1]);mappings.entry(ElementId::Edge(*edge)).or_insert_with(||vec![ElementId::Edge(*edge)]).push(ElementId::Edge(new));}
            }Ok(((),mappings.into_iter().collect()))
        })?.1)
    }

    /// Makes a shell by moving a copied surface outward along its averaged
    /// vertex normals and closing boundary loops. The original becomes the
    /// reversed inner surface. Global shell intersections remain NotChecked.
    pub fn solidify(&mut self,thickness:f64,ctx:&mut Context<'_>)->Result<ChangeSet> {
        if !thickness.is_finite()||thickness<=0.{return Err(invalid("solidify thickness must be positive"));}
        let adj=surface(self,ctx)?;let boundary=adj.edges.values().filter(|u|u.len()==1).map(|u|u[0]).collect::<Vec<_>>();
        ctx.counts(self.vertices.len()*2,self.faces.len()*2+boundary.len(),self.corners.len()*2+boundary.len()*4,self.corners.len()*2+boundary.len()*4)?;
        Ok(self.edit(ctx,self.memory_bytes().saturating_mul(3)+boundary.len().saturating_mul(1024),|m,ctx|{
            let source=m.clone();let normals=vertex_normals(&source,ctx)?;let mut vertices=BTreeMap::new();let mut remaps=Vec::new();
            for v in &source.vertices{ctx.checkpoint(1)?;let n=normals.get(&v.id).ok_or_else(||invalid("solidify refuses isolated vertices"))?;
                let p=add(v.position,mul(*n,thickness));check_position(p)?;let id=m.push_vertex(p,v.weights.clone())?;vertices.insert(v.id,id);
                remaps.push((ElementId::Vertex(v.id),vec![ElementId::Vertex(v.id),ElementId::Vertex(id)]));
            }
            let mut replacements=BTreeMap::new();for f in &source.faces{ctx.checkpoint(1)?;let cs=source.face_corners(f.id)?;
                let values=cs.iter().map(|c|(vertices[&c.vertex],c.uv,None)).collect::<Vec<_>>();let new=m.push_face(&values,f.material)?;
                remaps.push((ElementId::Face(f.id),vec![ElementId::Face(f.id),ElementId::Face(new)]));
                for (a,b) in cs.iter().zip(m.face_corners(new)?){remaps.push((ElementId::Corner(a.id),vec![ElementId::Corner(a.id),ElementId::Corner(b.id)]));}
                let mut reversed=cs.to_vec();reversed.reverse();for c in &mut reversed{c.normal=c.normal.map(|n|mul(n,-1.));}replacements.insert(f.id,reversed);
            }
            for u in boundary{ctx.checkpoint(1)?;let a=u.from;let b=u.to;let width=length(sub(source.vertex(a).unwrap().position,source.vertex(b).unwrap().position));
                m.push_face(&[(a,[0.,0.],None),(b,[width,0.],None),(vertices[&b],[width,thickness],None),(vertices[&a],[0.,thickness],None)],source.face(u.face).unwrap().material)?;
            }
            replace_corners(m,&replacements,ctx)?;
            for (&edge,data) in &source.edge_data{m.edge_data.insert(EdgeKey::new(vertices[&edge.0],vertices[&edge.1]),*data);}
            let report=m.validate(ctx)?;if !report.is_closed_manifold{return Err(invalid("solidify produced an invalid shell"));}Ok(((),remaps))
        })?.1)
    }
}
pub(crate) fn append_corners(mesh:&mut Mesh,corners:Vec<Corner>,material:u32)->Result<FaceId>{
    let id=FaceId(mesh.allocate_id()?);let offset=u32::try_from(mesh.corners.len()).map_err(|_|invalid("corner offset overflow"))?;
    mesh.faces.push(Face{id,first_corner:offset,corner_count:corners.len() as u32,material});mesh.corners.extend(corners);Ok(id)
}
pub(crate) fn vertex_normals(mesh:&Mesh,ctx:&mut Context<'_>)->Result<BTreeMap<VertexId,[f64;3]>>{
    let mut normals=BTreeMap::new();for f in &mesh.faces{let g=face_geometry(mesh,f.id,ctx)?;for c in mesh.face_corners(f.id)?{ctx.checkpoint(1)?;let n=normals.entry(c.vertex).or_insert([0.;3]);*n=add(*n,g.normal);}}
    for n in normals.values_mut(){let len=length(*n);if len<=1e-12{return Err(invalid("undefined averaged surface normal"));}*n=mul(*n,1./len);}Ok(normals)
}
