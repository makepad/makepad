use crate::{context::invalid,geometry::*,*};
use std::collections::{BTreeMap,BTreeSet,VecDeque};
impl Mesh {
    /// Reverse selected polygon winding and explicit corner normals. Corners
    /// retain their vertex/UV/pin identity; edge attributes and weights survive.
    /// This is an explicit side choice, including for open surfaces. It does
    /// not guess an outside or silently reorient neighboring unselected faces.
    pub fn flip_faces(&mut self,faces:&[FaceId],ctx:&mut Context<'_>)->Result<ChangeSet>{
        ctx.checkpoint(faces.len() as u64)?;
        ctx.limit("selected faces",faces.len() as u64,ctx.limits.max_faces as u64)?;
        ctx.bytes(self.memory_bytes().saturating_add(faces.len().saturating_mul(96)))?;
        let selected=faces.iter().copied().collect::<BTreeSet<_>>();
        if selected.len()!=faces.len(){return Err(invalid("face selection contains duplicates"));}
        for &id in &selected{if self.face(id).is_none(){return Err(MeshError::UnknownElement(ElementId::Face(id)));}}
        Ok(self.edit(ctx,faces.len().saturating_mul(96),|mesh,ctx|{
            for face in &mesh.faces{ctx.checkpoint(1)?;if selected.contains(&face.id){
                let corners=&mut mesh.corners[face.first_corner as usize..(face.first_corner+face.corner_count)as usize];
                ctx.checkpoint(corners.len() as u64)?;
                // Keep the initial corner stable as well as every persistent ID.
                corners[1..].reverse();
                for corner in corners{corner.normal=corner.normal.map(|normal|normal.map(|v|-v));}
            }}Ok(((),Vec::new()))
        })?.1)
    }
    pub fn set_corner_normals_bulk(&mut self,values:&[(CornerId,Option<[f64;3]>)],ctx:&mut Context<'_>)->Result<ChangeSet>{
        ctx.limit("corner normals",values.len()as u64,ctx.limits.max_corners as u64)?;let valid=self.corners.iter().map(|c|c.id).collect::<BTreeSet<_>>();let mut normals=BTreeMap::new();
        for &(id,normal) in values{ctx.checkpoint(1)?;if !valid.contains(&id){return Err(MeshError::UnknownElement(ElementId::Corner(id)));}let normal=normal.map(|n|{crate::mesh::check_position(n)?;let len=length(n);if len<=1e-12{return Err(invalid("zero corner normal"));}Ok(mul(n,1./len))}).transpose()?;
            if normals.insert(id,normal).is_some(){return Err(invalid("duplicate corner normal"));}}
        Ok(self.edit(ctx,values.len().saturating_mul(128),|m,ctx|{for c in &mut m.corners{ctx.checkpoint(1)?;if let Some(&n)=normals.get(&c.id){c.normal=n;}}Ok(((),Vec::new()))})?.1)
    }
    /// Derives split normals across hard crease edges and a dihedral threshold.
    /// UV seams do not force a lighting seam. Geometry/UV/weights stay unchanged.
    pub fn recalculate_normals(&mut self,smooth:bool,angle:f64,ctx:&mut Context<'_>)->Result<ChangeSet>{
        if !angle.is_finite()||!(0.0..=std::f64::consts::PI).contains(&angle){return Err(invalid("normal angle must be 0..pi radians"));}
        Ok(self.edit(ctx,self.corners.len().saturating_mul(256),|m,ctx|{derive_normals(m,smooth,angle,ctx)?;Ok(((),Vec::new()))})?.1)
    }
}
pub(crate) fn derive_normals(mesh:&mut Mesh,smooth:bool,angle:f64,ctx:&mut Context<'_>)->Result<()> {
    let adj=mesh.adjacency(ctx)?;let normals=mesh.faces.iter().map(|f|Ok((f.id,face_geometry(mesh,f.id,ctx)?.normal))).collect::<Result<BTreeMap<_,_>>>()?;
    let mut results=BTreeMap::new();let cosine=angle.cos();
    for (&vertex,faces) in &adj.vertex_faces{let mut remaining=faces.iter().copied().collect::<BTreeSet<_>>();
        while let Some(&seed)=remaining.first(){let mut queue=VecDeque::from([seed]);remaining.remove(&seed);let mut fan=Vec::new();let mut n=[0.;3];
            while let Some(face)=queue.pop_front(){ctx.checkpoint(1)?;fan.push(face);n=add(n,normals[&face]);if !smooth{continue;}
                for &edge in &adj.vertex_edges[&vertex]{ctx.checkpoint(1)?;let uses=adj.radial(edge);
                    if uses.len()!=2||mesh.edge_data.get(&edge).is_some_and(|d|d.attributes.crease>0.)||!uses.iter().any(|u|u.face==face){continue;}
                    let other=uses.iter().find(|u|u.face!=face).unwrap().face;
                    if dot(normals[&face],normals[&other])+1e-12>=cosine&&remaining.remove(&other){queue.push_back(other);}
                }
            }
            let len=length(n);for face in fan{results.insert((vertex,face),if len>1e-12{mul(n,1./len)}else{normals[&face]});}
        }
    }
    for face in &mesh.faces{for c in &mut mesh.corners[face.first_corner as usize..(face.first_corner+face.corner_count)as usize]{ctx.checkpoint(1)?;c.normal=Some(results[&(c.vertex,face.id)]);}}
    Ok(())
}
