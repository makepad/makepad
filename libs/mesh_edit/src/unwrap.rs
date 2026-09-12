//! Automatic dihedral/seam charts with convex-boundary harmonic disk maps.
//! Non-disk charts split to checked per-face maps instead of losing triangles.
use crate::{context::invalid,geometry::*,modifiers::select_faces,uv::{components,install_uv},*};
use std::collections::{BTreeMap,BTreeSet};

impl Mesh {
    /// Cuts at authored seams and dihedral angles above 60 degrees, then maps
    /// disk charts with an arc-length circular boundary and 128 harmonic steps.
    /// Planar charts retain their projected shape. Non-disk charts use one chart
    /// per polygon. Pinned charts keep their boundary and pins and relax inside.
    pub fn unwrap_uv(&mut self,faces:&[FaceId],ctx:&mut Context<'_>)->Result<ChangeSet>{
        let selected=select_faces(self,faces,ctx)?;if selected.is_empty(){return Err(invalid("empty unwrap selection"));}
        let adj=crate::topology::surface(self,ctx)?;let normals=selected.iter().map(|&f|Ok((f,face_geometry(self,f,ctx)?.normal))).collect::<Result<BTreeMap<_,_>>>()?;
        let mut neighbors=BTreeMap::<FaceId,Vec<FaceId>>::new();for (&edge,uses) in &adj.edges{ctx.checkpoint(1)?;
            if uses.len()==2&&uses.iter().all(|u|selected.contains(&u.face))&&!self.edge_data.get(&edge).is_some_and(|d|d.attributes.seam)&&dot(normals[&uses[0].face],normals[&uses[1].face])>=0.5{
                neighbors.entry(uses[0].face).or_default().push(uses[1].face);neighbors.entry(uses[1].face).or_default().push(uses[0].face);
            }
        }
        let charts=components(selected,&neighbors,ctx)?;let mut values=BTreeMap::new();
        let pinned_charts=charts.iter().filter(|chart|chart.iter().any(|&f|self.face_corners(f).unwrap().iter().any(|c|self.uv_pins.contains(&c.id)))).collect::<Vec<_>>();
        let constrained=if pinned_charts.is_empty(){None}else{
            ctx.bytes(self.memory_bytes().saturating_mul(3)+self.corners.len().saturating_mul(512))?;
            let faces=pinned_charts.iter().flat_map(|chart|chart.iter().copied()).collect::<Vec<_>>();let mut candidate=self.clone();candidate.relax_uv(&faces,128,1.,ctx)?;
            if candidate.uv_islands(&faces,ctx)?.iter().any(|island|island.area<=1e-14){return Err(invalid("pinned unwrap needs an existing noncollapsed boundary layout"));}Some(candidate)
        };
        for chart in charts{ctx.checkpoint(1)?;let planar=chart.iter().all(|f|dot(normals[f],normals[&chart[0]])>1.-1e-10)&&chart.iter().map(|&f|face_geometry(self,f,ctx)).collect::<Result<Vec<_>>>()?.iter().all(|g|g.planar);
            let pinned=chart.iter().any(|&f|self.face_corners(f).unwrap().iter().any(|c|self.uv_pins.contains(&c.id)));
            if pinned {
                // Existing chart placement is the constraint system. A fresh
                // circular boundary would overwrite pinned layout intent.
                for &f in &chart{for c in constrained.as_ref().unwrap().face_corners(f)?{values.insert(c.id,c.uv);}}
            }else if planar{for &f in &chart{project(self,f,normals[&chart[0]],&mut values,ctx)?;}}
            else if let Some(mapped)=disk_map(self,&chart,ctx)?{values.extend(mapped);}
            else {for &f in &chart{project(self,f,normals[&f],&mut values,ctx)?;}}
        }
        install_uv(self,values,ctx)
    }
}
fn project(mesh:&Mesh,face:FaceId,n:[f64;3],values:&mut BTreeMap<CornerId,[f64;2]>,ctx:&mut Context<'_>)->Result<()> {
    let axis=(0..3).max_by(|&a,&b|n[a].abs().total_cmp(&n[b].abs())).unwrap();for c in mesh.face_corners(face)?{ctx.checkpoint(1)?;let p=mesh.vertex(c.vertex).unwrap().position;
        values.insert(c.id,[p[(axis+1)%3]*n[axis].signum(),p[(axis+2)%3]]);
    }Ok(())
}
fn disk_map(mesh:&Mesh,faces:&[FaceId],ctx:&mut Context<'_>)->Result<Option<BTreeMap<CornerId,[f64;2]>>>{
    let mut edges=BTreeMap::<EdgeKey,Vec<(VertexId,VertexId)>>::new();let mut vertices=BTreeSet::new();let mut neighbors=BTreeMap::<VertexId,BTreeSet<VertexId>>::new();
    for &f in faces{let cs=mesh.face_corners(f)?;for i in 0..cs.len(){ctx.checkpoint(1)?;let a=cs[i].vertex;let b=cs[(i+1)%cs.len()].vertex;vertices.insert(a);edges.entry(EdgeKey::new(a,b)).or_default().push((a,b));neighbors.entry(a).or_default().insert(b);neighbors.entry(b).or_default().insert(a);}}
    if vertices.len()+faces.len()!=edges.len()+1{return Ok(None);}
    let mut outgoing=BTreeMap::new();for uses in edges.values(){if uses.len()==1{let(a,b)=uses[0];if outgoing.insert(a,b).is_some(){return Ok(None);}}else if uses.len()!=2{return Ok(None);}}
    let Some(&start)=outgoing.keys().next()else{return Ok(None)};let mut at=start;let mut boundary=Vec::new();
    while let Some(next)=outgoing.remove(&at){ctx.checkpoint(1)?;boundary.push(at);at=next;if at==start{break;}}
    if at!=start||!outgoing.is_empty()||boundary.len()<3{return Ok(None);}
    let lengths=(0..boundary.len()).map(|i|length(sub(mesh.vertex(boundary[i]).unwrap().position,mesh.vertex(boundary[(i+1)%boundary.len()]).unwrap().position))).collect::<Vec<_>>();
    let total=lengths.iter().sum::<f64>();if total<=0.||!total.is_finite(){return Err(invalid("unwrap boundary length is invalid"));}
    let mut uv=vertices.iter().map(|&v|(v,[0.5,0.5])).collect::<BTreeMap<_,_>>();let mut arc=0.;for (i,&v) in boundary.iter().enumerate(){let angle=std::f64::consts::TAU*arc/total;uv.insert(v,[0.5+0.5*angle.cos(),0.5+0.5*angle.sin()]);arc+=lengths[i];}
    let fixed=boundary.iter().copied().collect::<BTreeSet<_>>();let mut next=uv.clone();for _ in 0..128{for &v in &vertices{ctx.checkpoint(1)?;if fixed.contains(&v){continue;}let mut p=[0.;2];for other in &neighbors[&v]{ctx.checkpoint(1)?;for d in 0..2{p[d]+=uv[other][d]/neighbors[&v].len()as f64;}}next.insert(v,p);}std::mem::swap(&mut uv,&mut next);}
    let mut result=BTreeMap::new();for &f in faces{let cs=mesh.face_corners(f)?;let mut area=0.;for i in 0..cs.len(){let a=uv[&cs[i].vertex];let b=uv[&cs[(i+1)%cs.len()].vertex];area+=a[0]*b[1]-a[1]*b[0];result.insert(cs[i].id,a);}
        if !area.is_finite()||area<=1e-14{return Ok(None);}
    }Ok(Some(result))
}
