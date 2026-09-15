use crate::{context::invalid,geometry::*,mesh::check_position,modifiers::invalidate_normals,*};
use std::collections::BTreeMap;

impl Mesh {
    /// Installs a bounded batch of absolute positions while retaining element
    /// identities and attributes. Any invalid face rolls back the whole batch.
    pub fn set_positions_bulk(&mut self,values:&[(VertexId,[f64;3])],ctx:&mut Context<'_>)->Result<ChangeSet>{
        let ids=values.iter().map(|(id,_)|*id).collect::<Vec<_>>();let selected=self.selected_vertices(&ids,ctx)?;
        for &(_,p) in values{ctx.checkpoint(1)?;check_position(p)?;}let positions=values.iter().copied().collect::<BTreeMap<_,_>>();
        Ok(self.edit(ctx,values.len().saturating_mul(128),|m,ctx|{for v in &mut m.vertices{ctx.checkpoint(1)?;if let Some(&p)=positions.get(&v.id){v.position=p;}}
            invalidate_normals(m,&selected);Ok(((),Vec::new()))})?.1)
    }
    pub fn deform(&mut self,vertices:&[VertexId],deformation:&Deformation,ctx:&mut Context<'_>)->Result<ChangeSet>{
        let (axis,range)=match deformation{Deformation::Bend{axis,range,angle}|Deformation::Twist{axis,range,angle}=>{
            if !angle.is_finite()||angle.abs()>std::f64::consts::TAU{return Err(invalid("deformation angle must be within +/-2pi radians"));}(*axis,*range)},
            Deformation::Taper{axis,range,scales}=>{if scales.iter().any(|s|!s.is_finite()||*s<=0.){return Err(invalid("taper scales must be positive"));}(*axis,*range)}};
        if axis>2||range.iter().any(|v|!v.is_finite()||v.abs()>1e100)||range[1]<=range[0]{return Err(invalid("deformation needs axis 0..2 and an increasing finite range"));}
        let selected=self.selected_vertices(vertices,ctx)?;let span=range[1]-range[0];let radial=(axis+1)%3;let other=(axis+2)%3;
        Ok(self.edit(ctx,vertices.len().saturating_mul(128),|m,ctx|{
            for v in &mut m.vertices{ctx.checkpoint(1)?;if !selected.contains(&v.id){continue;}let p=v.position;let t=((p[axis]-range[0])/span).clamp(0.,1.);let mut q=p;
                match deformation{
                    Deformation::Twist{angle,..}=>{let(s,c)=(t*angle).sin_cos();q[radial]=c*p[radial]-s*p[other];q[other]=s*p[radial]+c*p[other];}
                    Deformation::Taper{scales,..}=>{let scale=scales[0]*(1.-t)+scales[1]*t;q[radial]*=scale;q[other]*=scale;}
                    Deformation::Bend{angle,..}=>{if *angle!=0.{let theta=t*angle;let(s,c)=theta.sin_cos();let radius=span/angle;let tail=p[axis]-p[axis].clamp(range[0],range[1]);
                        q[radial]=p[radial]*c+radius*2.*(theta*0.5).sin().powi(2)+tail*s;q[axis]=range[0]+(radius-p[radial])*s+tail*c;}}
                }check_position(q)?;v.position=q;
            }invalidate_normals(m,&selected);Ok(((),Vec::new()))
        })?.1)
    }

    /// Trilinear lattice displacement with x-fastest controls. Every selected
    /// point must lie inside bounds; no hidden extrapolation or clamping occurs.
    pub fn lattice(&mut self,vertices:&[VertexId],bounds:[[f64;3];2],divisions:[u32;3],displacements:&[[f64;3]],ctx:&mut Context<'_>)->Result<ChangeSet>{
        check_position(bounds[0])?;check_position(bounds[1])?;for d in 0..3{if bounds[1][d]<=bounds[0][d]||!(2..=8).contains(&divisions[d]){return Err(invalid("lattice requires increasing bounds and 2..8 controls per axis"));}}
        let count=divisions.iter().product::<u32>() as usize;if displacements.len()!=count{return Err(invalid("lattice control count mismatch"));}
        for &p in displacements{ctx.checkpoint(1)?;check_position(p)?;}let selected=self.selected_vertices(vertices,ctx)?;
        Ok(self.edit(ctx,vertices.len().saturating_mul(128),|m,ctx|{
            for v in &mut m.vertices{ctx.checkpoint(1)?;if !selected.contains(&v.id){continue;}let mut cell=[0;3];let mut fraction=[0.;3];
                for d in 0..3{if v.position[d]<bounds[0][d]||v.position[d]>bounds[1][d]{return Err(invalid("selected vertex lies outside lattice"));}
                    let coordinate=(v.position[d]-bounds[0][d])/(bounds[1][d]-bounds[0][d])*(divisions[d]-1)as f64;cell[d]=(coordinate.floor()as usize).min(divisions[d]as usize-2);fraction[d]=coordinate-cell[d]as f64;}
                let mut delta=[0.;3];for z in 0..2{for y in 0..2{for x in 0..2{let corner=[x,y,z];let mut w=1.;for d in 0..3{w*=if corner[d]==0{1.-fraction[d]}else{fraction[d]};}
                    let index=cell[0]+x+divisions[0]as usize*(cell[1]+y+divisions[1]as usize*(cell[2]+z));delta=add(delta,mul(displacements[index],w));
                }}}v.position=add(v.position,delta);check_position(v.position)?;
            }invalidate_normals(m,&selected);Ok(((),Vec::new()))
        })?.1)
    }

    /// Nearest point on a checked reference triangle surface, plus signed
    /// triangle-normal offset. Every selected point must match within distance.
    /// The exhaustive search charges every vertex/triangle pair to work budget.
    pub fn shrinkwrap(&mut self,vertices:&[VertexId],reference:&Mesh,max_distance:f64,offset:f64,ctx:&mut Context<'_>)->Result<ChangeSet>{
        if !max_distance.is_finite()||max_distance<0.||!offset.is_finite(){return Err(invalid("shrinkwrap needs nonnegative finite distance and finite offset"));}
        let selected=self.selected_vertices(vertices,ctx)?;let triangles=reference.triangulate(ctx)?;if triangles.triangles.is_empty(){return Err(invalid("reference has no triangles"));}
        ctx.bytes(self.memory_bytes().saturating_mul(2)+reference.memory_bytes()+triangles.vertices.len().saturating_mul(1024))?;
        let mut positions=BTreeMap::new();for &id in &selected{let p=self.vertex(id).unwrap().position;let mut best=None;
            for triangle in &triangles.triangles{ctx.checkpoint(1)?;let points=triangle.indices.map(|i|triangles.vertices[i as usize].position);let q=closest_triangle(p,points);let distance=length(sub(p,q));
                if best.as_ref().is_none_or(|(d,_,_)|distance<*d){let n=cross(sub(points[1],points[0]),sub(points[2],points[0]));let len=length(n);if len>0.{best=Some((distance,q,mul(n,1./len)));}}
            }
            let(distance,q,n)=best.ok_or_else(||invalid("reference has no nondegenerate triangles"))?;if distance>max_distance{return Err(invalid("shrinkwrap vertex exceeds projection distance"));}let q=add(q,mul(n,offset));check_position(q)?;positions.insert(id,q);
        }
        Ok(self.edit(ctx,positions.len().saturating_mul(128),|m,ctx|{for v in &mut m.vertices{ctx.checkpoint(1)?;if let Some(&q)=positions.get(&v.id){v.position=q;}}
            invalidate_normals(m,&selected);Ok(((),Vec::new()))})?.1)
    }
}

/// Closest point uses a normalized triangle frame to avoid quartic products of
/// large authoring coordinates. Degenerate triangles fall back to their edges.
pub(crate) fn closest_triangle(p:[f64;3],tri:[[f64;3];3])->[f64;3]{
    let a=tri[0];let ab=sub(tri[1],a);let ac=sub(tri[2],a);let scale=length(ab).max(length(ac));if scale==0.{return a;}
    let ab=mul(ab,1./scale);let ac=mul(ac,1./scale);let ap=mul(sub(p,a),1./scale);let d1=dot(ab,ap);let d2=dot(ac,ap);
    if d1<=0.&&d2<=0.{return a;}let bp=sub(ap,ab);let d3=dot(ab,bp);let d4=dot(ac,bp);if d3>=0.&&d4<=d3{return tri[1];}
    let vc=d1*d4-d3*d2;if vc<=0.&&d1>=0.&&d3<=0.{return add(a,mul(sub(tri[1],a),d1/(d1-d3)));}
    let cp=sub(ap,ac);let d5=dot(ab,cp);let d6=dot(ac,cp);if d6>=0.&&d5<=d6{return tri[2];}
    let vb=d5*d2-d1*d6;if vb<=0.&&d2>=0.&&d6<=0.{return add(a,mul(sub(tri[2],a),d2/(d2-d6)));}
    let va=d3*d6-d5*d4;if va<=0.&&(d4-d3)>=0.&&(d5-d6)>=0.{return add(tri[1],mul(sub(tri[2],tri[1]),(d4-d3)/((d4-d3)+(d5-d6))));}
    let denom=va+vb+vc;if denom<=0.||!denom.is_finite(){return a;}let v=vb/denom;let w=vc/denom;add(a,add(mul(sub(tri[1],a),v),mul(sub(tri[2],a),w)))
}
