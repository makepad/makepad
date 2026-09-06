use crate::{modifiers::select_faces,context::invalid,*};
use std::collections::{BTreeMap,BTreeSet};
impl Mesh {
    /// Copies a mesh with fresh destination IDs and complete attribute transfer.
    /// Imported provenance has its own source scope instead of masquerading as
    /// a mutation mapping from an element of the destination mesh.
    pub fn append(&mut self,source:&Mesh,ctx:&mut Context<'_>)->Result<AppendResult>{
        source.check_structure(ctx)?;source.check_faces(ctx)?;
        ctx.counts(self.vertices.len()+source.vertices.len(),self.faces.len()+source.faces.len(),self.corners.len()+source.corners.len(),self.corners.len()+source.corners.len()+self.edge_data.len()+source.edge_data.len())?;
        let(imported,changes)=self.edit(ctx,source.memory_bytes().saturating_mul(3)+source.corners.len().saturating_mul(256),|m,ctx|{
            let mut vertices=BTreeMap::new();let mut imported=Vec::new();for v in &source.vertices{ctx.checkpoint(1)?;let id=m.push_vertex(v.position,v.weights.clone())?;vertices.insert(v.id,id);imported.push((ElementId::Vertex(v.id),ElementId::Vertex(id)));}
            for f in &source.faces{ctx.checkpoint(1)?;let corners=source.face_corners(f.id)?;let values=corners.iter().map(|c|(vertices[&c.vertex],c.uv,c.normal)).collect::<Vec<_>>();let id=m.push_face(&values,f.material)?;imported.push((ElementId::Face(f.id),ElementId::Face(id)));
                let copied=m.face_corners(id)?.to_vec();for(a,b)in corners.iter().zip(copied){imported.push((ElementId::Corner(a.id),ElementId::Corner(b.id)));if source.uv_pins.contains(&a.id){m.uv_pins.insert(b.id);}}
            }
            for(&edge,data)in &source.edge_data{m.edge_data.insert(EdgeKey::new(vertices[&edge.0],vertices[&edge.1]),*data);}
            for &edge in source.adjacency(ctx)?.edges.keys(){imported.push((ElementId::Edge(edge),ElementId::Edge(EdgeKey::new(vertices[&edge.0],vertices[&edge.1]))));}
            Ok((imported,Vec::new()))
        })?;Ok(AppendResult{changes,imported})
    }

    /// Copies exactly the selected faces and used vertices with the original
    /// IDs/watermark. The returned mesh has a separate object identity scope.
    pub fn extract_faces(&self,faces:&[FaceId],ctx:&mut Context<'_>)->Result<Mesh>{
        let selected=select_faces(self,faces,ctx)?;if selected.is_empty(){return Err(invalid("empty extraction selection"));}
        ctx.bytes(self.memory_bytes().saturating_mul(2)+self.corners.len().saturating_mul(192))?;
        let mut result=Mesh::new();result.next_id=self.next_id;let mut vertices=BTreeSet::new();let mut edges=BTreeSet::new();
        for f in &self.faces{if !selected.contains(&f.id){continue;}ctx.checkpoint(1)?;let cs=self.face_corners(f.id)?;let mut face=f.clone();face.first_corner=result.corners.len()as u32;result.faces.push(face);result.corners.extend_from_slice(cs);
            for i in 0..cs.len(){vertices.insert(cs[i].vertex);edges.insert(EdgeKey::new(cs[i].vertex,cs[(i+1)%cs.len()].vertex));if self.uv_pins.contains(&cs[i].id){result.uv_pins.insert(cs[i].id);}}
        }
        for v in &self.vertices{ctx.checkpoint(1)?;if vertices.contains(&v.id){result.vertices.push(v.clone());}}
        result.edge_data=self.edge_data.iter().filter(|(e,_)|edges.contains(e)).map(|(&e,&d)|(e,d)).collect();result.check_structure(ctx)?;result.check_faces(ctx)?;Ok(result)
    }

    /// Moves a face selection out, dropping only newly unused selection vertices.
    /// Pre-existing isolated vertices and explicit loose edges remain owned here.
    pub fn separate_faces(&mut self,faces:&[FaceId],ctx:&mut Context<'_>)->Result<(Mesh,ChangeSet)>{
        let extracted=self.extract_faces(faces,ctx)?;let selected=faces.iter().copied().collect::<BTreeSet<_>>();let candidates=extracted.vertices.iter().map(|v|v.id).collect::<BTreeSet<_>>();
        let(_,changes)=self.edit(ctx,extracted.memory_bytes().saturating_mul(2),|m,ctx|{
            m.repack_faces(&selected,ctx)?;let mut used=m.corners.iter().map(|c|c.vertex).collect::<BTreeSet<_>>();for(&edge,data)in &m.edge_data{if data.loose{used.insert(edge.0);used.insert(edge.1);}}
            m.vertices.retain(|v|!candidates.contains(&v.id)||used.contains(&v.id));Ok(((),Vec::new()))
        })?;Ok((extracted,changes))
    }
}
