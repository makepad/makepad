//! Automatic charts and atlas layout through `makepad_uv_unwrap`. This module
//! only adapts polygons, seams and pins to it: charts keep whole polygons, and
//! a pinned chart keeps its layout while everything else packs around it.
use crate::{context::invalid,modifiers::select_faces,geometry::{dot,face_geometry},uv::{components,install_uv},*};
use makepad_uv_unwrap::{AtlasTarget,ChartOptions,MeshInput,PackOptions,UnwrapError};
use std::collections::{BTreeMap,BTreeSet};

/// The texel grid an atlas in the unit square is laid out on. Padding and the
/// chart masks are measured against it.
const LAYOUT_TEXELS:u32=2048;
/// Free layout texels kept around each chart by `unwrap_uv`.
const LAYOUT_PADDING:u32=4;
/// Dot product of normals below which an edge cuts charts: 60 degrees.
const CREASE_COS:f64=0.5;

/// Runs one of the unwrapper's steps with the context's budget and cancellation.
pub(crate) fn with_budget<T>(ctx:&mut Context<'_>,run:impl FnOnce(&mut makepad_uv_unwrap::Checkpoint<'_>)->std::result::Result<T,UnwrapError>)->Result<T>{
    let mut stopped=None;
    let result=run(&mut |work|match ctx.checkpoint(work){Ok(())=>true,Err(error)=>{stopped=Some(error);false}});
    match result{
        Ok(value)=>Ok(value),
        Err(error)=>Err(stopped.unwrap_or(match error{UnwrapError::Cancelled=>MeshError::Cancelled,other=>invalid(other.to_string())})),
    }
}
pub(crate) fn layout_options(padding:u32,obstacles:&[[[f64;2];3]])->PackOptions<'_>{
    PackOptions{target:AtlasTarget::Fit{width:LAYOUT_TEXELS,height:LAYOUT_TEXELS},padding,obstacles}
}
pub(crate) fn layout_padding(padding:f64)->u32{(padding*LAYOUT_TEXELS as f64).ceil()as u32}
pub(crate) fn to_unit(p:[f64;2])->[f64;2]{[p[0]/LAYOUT_TEXELS as f64,p[1]/LAYOUT_TEXELS as f64]}

impl Mesh {
    /// Cuts at authored seams and dihedral angles above 60 degrees, splits what
    /// is left into charts that flatten within the distortion limit, maps flat
    /// charts by projection and curved ones conformally, and packs them into
    /// the unit square at one texel density. Charts holding a pin keep their
    /// boundary and pins, relax inside, and the other charts pack around them.
    pub fn unwrap_uv(&mut self,faces:&[FaceId],ctx:&mut Context<'_>)->Result<ChangeSet>{
        let selected=select_faces(self,faces,ctx)?;if selected.is_empty(){return Err(invalid("empty unwrap selection"));}
        let adj=crate::topology::surface(self,ctx)?;let normals=selected.iter().map(|&f|Ok((f,face_geometry(self,f,ctx)?.normal))).collect::<Result<BTreeMap<_,_>>>()?;
        let mut neighbors=BTreeMap::<FaceId,Vec<FaceId>>::new();for (&edge,uses) in &adj.edges{ctx.checkpoint(1)?;
            if uses.len()==2&&uses.iter().all(|u|selected.contains(&u.face))&&!self.edge_data.get(&edge).is_some_and(|d|d.attributes.seam)&&dot(normals[&uses[0].face],normals[&uses[1].face])>=CREASE_COS{
                neighbors.entry(uses[0].face).or_default().push(uses[1].face);neighbors.entry(uses[1].face).or_default().push(uses[0].face);
            }
        }
        let charts=components(selected,&neighbors,ctx)?;let mut values=BTreeMap::new();
        let is_pinned=|mesh:&Mesh,chart:&[FaceId]|chart.iter().any(|&f|mesh.face_corners(f).unwrap().iter().any(|c|mesh.uv_pins.contains(&c.id)));
        let pinned_faces=charts.iter().filter(|chart|is_pinned(self,chart)).flatten().copied().collect::<BTreeSet<_>>();
        let free_faces=charts.iter().filter(|chart|!is_pinned(self,chart)).flatten().copied().collect::<BTreeSet<_>>();
        if !pinned_faces.is_empty(){
            // Existing chart placement is the constraint system: a fresh layout
            // would overwrite pinned intent.
            ctx.bytes(self.memory_bytes().saturating_mul(3)+self.corners.len().saturating_mul(512))?;
            let faces=pinned_faces.iter().copied().collect::<Vec<_>>();let mut candidate=self.clone();candidate.relax_uv(&faces,128,1.,ctx)?;
            if candidate.uv_islands(&faces,ctx)?.iter().any(|island|island.area<=1e-14){return Err(invalid("pinned unwrap needs an existing noncollapsed boundary layout"));}
            for &f in &faces{for c in candidate.face_corners(f)?{values.insert(c.id,c.uv);}}
        }
        if !free_faces.is_empty(){
            let triangulated=self.triangulate(ctx)?;
            let index=self.vertices.iter().enumerate().map(|(i,v)|(v.id,i as u32)).collect::<BTreeMap<_,_>>();
            let positions=self.vertices.iter().map(|v|v.position).collect::<Vec<_>>();
            let (mut indices,mut polygons,mut corners,mut obstacles)=(Vec::new(),Vec::new(),Vec::new(),Vec::new());
            let polygon_ids=free_faces.iter().enumerate().map(|(i,&f)|(f,i as u32)).collect::<BTreeMap<_,_>>();
            for t in &triangulated.triangles{ctx.checkpoint(1)?;
                if let Some(&polygon)=polygon_ids.get(&t.source_face){
                    polygons.push(polygon);
                    for i in t.indices{let v=&triangulated.vertices[i as usize];indices.push(index[&v.source_vertex]);corners.push(v.source_corner);}
                }else if pinned_faces.contains(&t.source_face){
                    obstacles.push(t.indices.map(|i|{let uv=values[&triangulated.vertices[i as usize].source_corner];let [u,v]:[f64;2]=uv;[u*LAYOUT_TEXELS as f64,v*LAYOUT_TEXELS as f64]}));
                }
            }
            let seams=self.edge_data.iter().filter(|(_,d)|d.attributes.seam).filter_map(|(e,_)|Some([*index.get(&e.0)?,*index.get(&e.1)?])).collect::<Vec<_>>();
            let input=MeshInput{positions:&positions,indices:&indices,polygons:Some(&polygons),seams:&seams};
            // Vertex ids are the topology here; welding could only join what
            // the mesh keeps apart.
            let options=ChartOptions{crease_cos:CREASE_COS,weld_tolerance:0.,..ChartOptions::default()};
            let obstacles=obstacles.into_iter().filter(|t:&[[f64;2];3]|t.iter().flatten().all(|c|(0.0..=LAYOUT_TEXELS as f64).contains(c))).collect::<Vec<_>>();
            let unwrapped=with_budget(ctx,|cp|makepad_uv_unwrap::unwrap(&input,&options,&layout_options(LAYOUT_PADDING,&obstacles),cp))?;
            for (corner,uv) in corners.iter().zip(&unwrapped.uv){values.insert(*corner,*uv);}
        }
        install_uv(self,values,ctx)
    }
}
