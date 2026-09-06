//! Deterministic shortest-edge reduction with a cumulative displacement bound.
//! A conservative UV/material/pin boundary policy deliberately trades reduction
//! rate for retaining authoring attributes.
use crate::{context::invalid,geometry::*,topology::{surface,blended_vertex,replace_corners},*};
use std::collections::{BTreeMap,BTreeSet};

impl Mesh {
    /// Triangulates the cage, then collapses eligible manifold edges toward the
    /// requested triangle-face count. max_error bounds every original vertex's
    /// cumulative displacement from its surviving representative. UV seams,
    /// material boundaries, pinned vertices and marked edge neighborhoods stay
    /// fixed. The achieved count is explicit when these constraints stop work.
    pub fn decimate(&mut self,target_faces:usize,max_error:f64,ctx:&mut Context<'_>)->Result<DecimateResult>{
        if target_faces==0||!max_error.is_finite()||max_error<0.{return Err(invalid("decimation requires positive face target and nonnegative finite error"));}
        surface(self,ctx)?;let triangles=self.triangulate(ctx)?;ctx.counts(self.vertices.len(),triangles.triangles.len(),triangles.triangles.len()*3,triangles.triangles.len()*3)?;
        let ((achieved_faces,collapses),changes)=self.edit(ctx,triangles.vertices.len().saturating_mul(2048)+triangles.triangles.len().saturating_mul(1024),|m,ctx|{
            let source=m.clone();let mut first=BTreeSet::new();let mut deferred=Vec::new();let mut next=Mesh::new();next.next_id=source.next_id;next.vertices=source.vertices.clone();next.edge_data=source.edge_data.clone();
            let mut mappings=BTreeMap::<ElementId,Vec<ElementId>>::new();let mut reused=BTreeSet::new();
            for tri in &triangles.triangles{ctx.checkpoint(1)?;if first.insert(tri.source_face){emit_triangle(&mut next,&source,&triangles,tri,Some(tri.source_face),&mut mappings,&mut reused)?;}else{deferred.push(tri);}}
            for tri in deferred{emit_triangle(&mut next,&source,&triangles,tri,None,&mut mappings,&mut reused)?;}
            if reused.len()!=source.corners.len(){return Err(invalid("decimation refuses to discard collinear authored corners during triangulation"));}
            next.check_structure(ctx)?;*m=next;
            let mut protected=BTreeSet::new();let mut first_uv=BTreeMap::new();let mut first_material=BTreeMap::new();
            for f in &m.faces{for c in m.face_corners(f.id)?{ctx.checkpoint(1)?;if first_uv.get(&c.vertex).is_some_and(|uv|*uv!=c.uv)||first_material.get(&c.vertex).is_some_and(|mat|*mat!=f.material)||m.uv_pins.contains(&c.id){protected.insert(c.vertex);}
                first_uv.entry(c.vertex).or_insert(c.uv);first_material.entry(c.vertex).or_insert(f.material);
            }}
            for edge in m.edge_data.keys(){protected.extend([edge.0,edge.1]);}
            let mut radii=m.vertices.iter().map(|v|(v.id,0f64)).collect::<BTreeMap<_,_>>();let mut vertex_mapping=source.vertices.iter().map(|v|(v.id,v.id)).collect::<BTreeMap<_,_>>();let mut collapses=0;
            while m.faces.len()>target_faces{ctx.checkpoint(1)?;let adj=m.adjacency(ctx)?;let mut candidates=Vec::new();
                for &edge in adj.edges.keys(){ctx.checkpoint(1)?;if protected.contains(&edge.0)||protected.contains(&edge.1){continue;}let distance=length(sub(m.vertex(edge.0).unwrap().position,m.vertex(edge.1).unwrap().position));let error=radii[&edge.0].max(radii[&edge.1])+distance*0.5;
                    if error<=max_error{candidates.push((error,edge));}}
                candidates.sort_by(|a,b|a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));let mut progressed=false;
                for (error,edge) in candidates{ctx.checkpoint(1)?;
                    // Never remove more incident triangles than the target asks.
                    if m.faces.len().saturating_sub(adj.radial(edge).len())<target_faces{continue;}
                    match collapse_triangle_neighborhood(m,&adj,edge,ctx){
                        Ok(_)=>{radii.insert(edge.0,error);radii.remove(&edge.1);for value in vertex_mapping.values_mut(){ctx.checkpoint(1)?;if *value==edge.1{*value=edge.0;}}collapses+=1;progressed=true;break;}
                        Err(error@MeshError::Cancelled)|Err(error@MeshError::Budget{..})=>return Err(error),
                        Err(_)=>continue,
                    }
                }
                if !progressed{break;}
            }
            // The outer edit owns rollback. Local link/orientation checks guard
            // candidates; a full manifold qualification still gates publication.
            surface(m,ctx)?;
            mappings.extend(vertex_mapping.into_iter().map(|(a,b)|(ElementId::Vertex(a),vec![ElementId::Vertex(b)])));
            Ok(((m.faces.len(),collapses),mappings.into_iter().collect()))
        })?;Ok(DecimateResult{changes,achieved_faces,collapses})
    }
}
fn emit_triangle(next:&mut Mesh,source:&Mesh,triangles:&TriangleMesh,tri:&Triangle,retain:Option<FaceId>,mappings:&mut BTreeMap<ElementId,Vec<ElementId>>,reused:&mut BTreeSet<CornerId>)->Result<()> {
    let face=match retain{Some(id)=>id,None=>FaceId(next.allocate_id()?)};let offset=u32::try_from(next.corners.len()).map_err(|_|invalid("decimation corner offset overflow"))?;
    next.faces.push(Face{id:face,first_corner:offset,corner_count:3,material:tri.material});mappings.entry(ElementId::Face(tri.source_face)).or_default().push(ElementId::Face(face));
    for index in tri.indices{let v=&triangles.vertices[index as usize];let id=if reused.insert(v.source_corner){v.source_corner}else{CornerId(next.allocate_id()?)};next.corners.push(Corner{id,vertex:v.source_vertex,uv:v.uv,normal:Some(v.normal)});
        mappings.entry(ElementId::Corner(v.source_corner)).or_default().push(ElementId::Corner(id));if source.uv_pins.contains(&v.source_corner){next.uv_pins.insert(id);}
    }Ok(())
}

/// Internal to the already-staged decimation edit. Refused candidates never
/// mutate the mesh. Only the one-ring is copied/validated, rather than cloning
/// and qualifying the entire mesh for each attempted collapse.
fn collapse_triangle_neighborhood(mesh:&mut Mesh,adj:&Adjacency,edge:EdgeKey,ctx:&mut Context<'_>)->Result<()> {
    let uses=adj.radial(edge);
    if uses.is_empty()||uses.len()>2{return Err(invalid("collapse needs a manifold edge"));}
    let neighbors=|v:VertexId|adj.vertex_edges[&v].iter().map(|e|if e.0==v{e.1}else{e.0}).collect::<BTreeSet<_>>();
    ctx.checkpoint((adj.vertex_edges[&edge.0].len()+adj.vertex_edges[&edge.1].len())as u64)?;
    let a=neighbors(edge.0);let b=neighbors(edge.1);
    let mut opposite=BTreeSet::new();
    for use_ in uses{for c in mesh.face_corners(use_.face)?{ctx.checkpoint(1)?;if c.vertex!=edge.0&&c.vertex!=edge.1{opposite.insert(c.vertex);}}}
    if a.intersection(&b).copied().collect::<BTreeSet<_>>()!=opposite{return Err(invalid("collapse violates triangle link condition"));}
    let boundary=|v|adj.vertex_edges[&v].iter().any(|e|adj.radial(*e).len()==1);
    // An interior edge joining two boundary vertices would pinch the boundary.
    if uses.len()==2&&boundary(edge.0)&&boundary(edge.1){return Err(invalid("collapse would pinch the boundary"));}
    let (position,weights)=blended_vertex(mesh,edge.0,edge.1,0.5,ctx)?;
    let affected=adj.vertex_faces[&edge.0].iter().chain(&adj.vertex_faces[&edge.1]).copied().collect::<BTreeSet<_>>();
    let mut remove=BTreeSet::new();let mut replacements=BTreeMap::new();
    let mut patch_positions=Vec::new();let mut patch_vertices=BTreeMap::new();let mut polygons=Vec::new();let mut old_normals=Vec::new();
    for face in affected{
        ctx.checkpoint(1)?;let source=mesh.face_corners(face)?;
        if source.len()!=3{return Err(invalid("decimation collapse requires triangles"));}
        if source.iter().any(|c|c.vertex==edge.0)&&source.iter().any(|c|c.vertex==edge.1){remove.insert(face);continue;}
        old_normals.push(face_geometry(mesh,face,ctx)?.normal);
        let mut corners=source.to_vec();let mut indices=Vec::new();
        for c in &mut corners{
            ctx.checkpoint(1)?;if c.vertex==edge.1{c.vertex=edge.0;}c.normal=None;
            let next=patch_vertices.len()as u32;
            let index=*patch_vertices.entry(c.vertex).or_insert_with(||{patch_positions.push(if c.vertex==edge.0{position}else{mesh.vertex(c.vertex).unwrap().position});next});
            indices.push(index);
        }
        polygons.push(Polygon{vertices:indices,uvs:corners.iter().map(|c|c.uv).collect(),material:mesh.face(face).unwrap().material});
        replacements.insert(face,corners);
    }
    ctx.bytes(mesh.memory_bytes().saturating_mul(2).saturating_add(patch_positions.len().saturating_mul(2048)).saturating_add(polygons.len().saturating_mul(2048)))?;
    let patch=Mesh::from_polygons(&patch_positions,&polygons,ctx)?;
    for (face,normal) in patch.faces.iter().zip(old_normals){
        if dot(normal,face_geometry(&patch,face.id,ctx)?.normal)<=1e-8{return Err(invalid("collapse would reverse a face"));}
    }
    surface(&patch,ctx)?;
    // Every fallible geometric/topological check precedes mutation. Cancellation
    // during this commit propagates to the outer edit and rolls the whole job back.
    replace_corners(mesh,&replacements,ctx)?;mesh.repack_faces(&remove,ctx)?;
    mesh.vertices.retain(|v|v.id!=edge.1);
    let i=mesh.vertices.binary_search_by_key(&edge.0,|v|v.id).unwrap();mesh.vertices[i].position=position;mesh.vertices[i].weights=weights;
    Ok(())
}
