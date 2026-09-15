use crate::{context::invalid,construction::vertex_normals,geometry::*,mesh::check_position,modifiers::invalidate_normals,*};
use std::collections::BTreeMap;
impl Mesh {
    /// Smoothstep surface brush with per-vertex protection and reflected brush
    /// centers. Overlapping reflected strokes choose the strongest influence,
    /// so symmetry-plane vertices are never double displaced.
    pub fn sculpt(&mut self,vertices:&[VertexId],brush:&SculptBrush,ctx:&mut Context<'_>)->Result<ChangeSet>{
        check_position(brush.center)?;check_position(brush.normal)?;
        if brush.symmetry>7||!brush.radius.is_finite()||brush.radius<=0.||!brush.strength.is_finite()||!brush.max_displacement.is_finite()||brush.max_displacement<0.
            ||brush.strength.abs()>brush.max_displacement{return Err(invalid("sculpt requires a finite radius and strength within max displacement"));}
        let normal_len=length(brush.normal);if normal_len<=1e-12{return Err(invalid("sculpt brush normal must be nonzero"));}let normal=mul(brush.normal,1./normal_len);
        let selected=self.selected_vertices(vertices,ctx)?;if brush.masks.len()>vertices.len(){return Err(invalid("mask entries exceed selection"));}
        let mut masks=BTreeMap::new();for &(id,value) in &brush.masks{ctx.checkpoint(1)?;if !selected.contains(&id)||!value.is_finite()||!(0.0..=1.0).contains(&value)||masks.insert(id,value).is_some(){return Err(invalid("invalid or duplicate sculpt mask"));}}
        let normals=if brush.kind==SculptKind::Inflate{vertex_normals(self,ctx)?}else{BTreeMap::new()};
        let centers=(0u8..8).filter(|bits|bits&!brush.symmetry==0).map(|bits|{
            (std::array::from_fn(|d|if bits&(1<<d)!=0{-brush.center[d]}else{brush.center[d]}),std::array::from_fn(|d|if bits&(1<<d)!=0{-normal[d]}else{normal[d]}))
        }).collect::<Vec<([f64;3],[f64;3])>>();
        Ok(self.edit(ctx,vertices.len().saturating_mul(256),|m,ctx|{
            for v in &mut m.vertices{ctx.checkpoint(1)?;if !selected.contains(&v.id){continue;}let mask=1.-masks.get(&v.id).copied().unwrap_or(0.);if mask==0.{continue;}
                let mut best=0.;let mut delta=[0.;3];for &(center,n) in &centers{ctx.checkpoint(1)?;let rel=sub(v.position,center);let distance=length(rel);if distance>=brush.radius{continue;}let t=1.-distance/brush.radius;let falloff=t*t*(3.-2.*t)*mask;if falloff<=best{continue;}best=falloff;
                    let raw=match brush.kind{
                        SculptKind::Inflate=>mul(*normals.get(&v.id).ok_or_else(||invalid("inflate requires surface vertices"))?,brush.strength),
                        SculptKind::Flatten=>mul(n,-dot(rel,n).clamp(-brush.strength.abs(),brush.strength.abs())*brush.strength.signum()),
                        SculptKind::Crease=>{let planar=sub(rel,mul(n,dot(rel,n)));let planar_len=length(planar);let pull=if planar_len>0.{mul(planar,-brush.strength/planar_len)}else{[0.;3]};mul(add(pull,mul(n,-brush.strength)),0.5)},
                    };delta=mul(raw,falloff);
                }
                if length(delta)>brush.max_displacement*(1.+1e-12){return Err(invalid("sculpt displacement exceeds bound"));}v.position=add(v.position,delta);check_position(v.position)?;
            }invalidate_normals(m,&selected);Ok(((),Vec::new()))
        })?.1)
    }
}
