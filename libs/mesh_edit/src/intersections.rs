//! Explicit global surface qualification. Broadphase is a deterministic AABB
//! sweep; narrowphase uses robust orientation predicates and identity-aware
//! boundary contact rules. Numerically unsupported pairs fail without a certificate.
use crate::{context::invalid,geometry::*,*};
use makepad_csg_math::{dvec3,orient3d};
use std::collections::{BTreeMap,BTreeSet,VecDeque};

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct FaceIntersection {pub a:FaceId,pub b:FaceId}
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct GlobalValidation {
    pub local:ValidationReport,
    pub candidate_pairs:usize,
    pub tested_pairs:usize,
    /// Distinct intersecting face pairs, including non-topological touching.
    pub intersection_count:usize,
    /// First 128 face pairs in stable-ID order. Count remains exact.
    pub intersections:Vec<FaceIntersection>,
    pub intersections_omitted:usize,
    pub components:usize,
    /// Closed shells must alternate outward/inward orientation with nesting.
    pub orientation_errors:usize,
    pub is_embedded_surface:bool,
    pub is_valid_solid:bool,
}
#[derive(Clone)]
struct Tri {face:FaceId,vertices:[VertexId;3],points:[[f64;3];3],bounds:[[f64;3];2]}
impl Mesh {
    pub fn validate_global(&self,ctx:&mut Context<'_>)->Result<GlobalValidation>{
        let local=self.validate(ctx)?;let source=self.triangulate(ctx)?;
        let working_bytes=self.memory_bytes().saturating_add(source.vertices.len().saturating_mul(1024)).saturating_add(source.triangles.len().saturating_mul(1024));ctx.bytes(working_bytes)?;
        let triangles=source.triangles.iter().map(|t|{let points=t.indices.map(|i|source.vertices[i as usize].position);let bounds=[std::array::from_fn(|d|points.iter().map(|p|p[d]).fold(f64::INFINITY,f64::min)),std::array::from_fn(|d|points.iter().map(|p|p[d]).fold(f64::NEG_INFINITY,f64::max))];
            Tri{face:t.source_face,vertices:t.indices.map(|i|source.vertices[i as usize].source_vertex),points,bounds}}).collect::<Vec<_>>();
        // Count interval overlaps on every axis in O(n log n), without
        // enumerating candidate pairs. A long thin mesh must not become
        // quadratic merely because its long direction happens to be X.
        let (axis,order)=sweep_order(&triangles,ctx)?;
        let mut active=Vec::<usize>::new();let mut hits=BTreeSet::new();let mut report=GlobalValidation{local,candidate_pairs:0,tested_pairs:0,intersection_count:0,intersections:Vec::new(),intersections_omitted:0,components:0,orientation_errors:0,is_embedded_surface:false,is_valid_solid:false};
        for index in order{ctx.checkpoint(1+active.len()as u64)?;let a=&triangles[index];active.retain(|&i|triangles[i].bounds[1][axis]>=a.bounds[0][axis]);
            for &other in &active{ctx.checkpoint(1)?;let b=&triangles[other];if a.face==b.face||(0..3).any(|d|d!=axis&&(a.bounds[1][d]<b.bounds[0][d]||b.bounds[1][d]<a.bounds[0][d])){continue;}
                report.candidate_pairs+=1;report.tested_pairs+=1;ctx.checkpoint(128)?;
                if triangles_intersect(a,b)?{let pair=(a.face.min(b.face),a.face.max(b.face));ctx.bytes(working_bytes.saturating_add((hits.len()+1).saturating_mul(128)))?;hits.insert(pair);}
            }active.push(index);
        }
        let components=face_components(self,ctx)?;report.components=components.len();report.intersection_count=hits.len();report.intersections_omitted=hits.len().saturating_sub(128);report.intersections=hits.iter().take(128).map(|&(a,b)|FaceIntersection{a,b}).collect();
        report.local.self_intersections=if hits.is_empty(){SelfIntersectionStatus::Clear}else{SelfIntersectionStatus::Found};report.is_embedded_surface=report.local.is_valid_surface&&hits.is_empty();
        if report.local.is_closed_manifold&&hits.is_empty(){
            let mut membership=BTreeMap::new();for(i,faces)in components.iter().enumerate(){for &face in faces{ctx.checkpoint(1)?;membership.insert(face,i);}}
            let mut groups=vec![Vec::new();components.len()];for tri in &triangles{ctx.checkpoint(1)?;groups[membership[&tri.face]].push(tri);}
            // Disconnected fibers are closed shells too. Reject impossible
            // containment using exact component bounds before summing winding
            // angles over every triangle of every other strand. Nested shells
            // still receive the complete orientation and containment checks.
            let mut bounds = Vec::with_capacity(groups.len());
            for group in &groups {
                let mut bound = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
                for tri in group {
                    ctx.checkpoint(1)?;
                    for d in 0..3 {
                        bound[0][d] = bound[0][d].min(tri.bounds[0][d]);
                        bound[1][d] = bound[1][d].max(tri.bounds[1][d]);
                    }
                }
                bounds.push(bound);
            }
            for (i, group) in groups.iter().enumerate() {
                ctx.checkpoint(1)?;
                let mut depth = 0;
                let point = group[0].points[0];
                for (j, other) in groups.iter().enumerate() {
                    ctx.checkpoint(1)?;
                    if i != j
                        && (0..3).all(|d| point[d] >= bounds[j][0][d] && point[d] <= bounds[j][1][d])
                        && inside_shell(point, other, ctx)?
                    {
                        depth += 1;
                    }
                }
                let sign=shell_sign(group,ctx)?;if sign!=(if depth%2==0{1}else{-1}){report.orientation_errors+=1;}
            }
        }
        report.is_valid_solid=report.local.is_closed_manifold&&report.is_embedded_surface&&report.orientation_errors==0;
        report.local.is_valid_solid=report.is_valid_solid;ctx.checkpoint(0)?;Ok(report)
    }
}

fn sweep_order(triangles:&[Tri],ctx:&mut Context<'_>)->Result<(usize,Vec<usize>)>{
    let n=triangles.len();let sort_work=(n as u64).saturating_mul(usize::BITS as u64-n.max(1).leading_zeros()as u64);
    let mut best=None;
    for axis in 0..3{
        ctx.checkpoint(sort_work.saturating_mul(2))?;
        let mut starts=(0..n).collect::<Vec<_>>();let mut ends=starts.clone();
        starts.sort_by(|&a,&b|triangles[a].bounds[0][axis].total_cmp(&triangles[b].bounds[0][axis]).then(a.cmp(&b)));
        ends.sort_by(|&a,&b|triangles[a].bounds[1][axis].total_cmp(&triangles[b].bounds[1][axis]).then(a.cmp(&b)));
        let mut expired=0;let mut pairs=0u64;
        for (seen,&i) in starts.iter().enumerate(){
            ctx.checkpoint(1)?;
            while expired<n&&triangles[ends[expired]].bounds[1][axis]<triangles[i].bounds[0][axis]{ctx.checkpoint(1)?;expired+=1;}
            pairs=pairs.saturating_add(seen.saturating_sub(expired)as u64);
        }
        if best.as_ref().is_none_or(|(count,_,_)|pairs<*count){best=Some((pairs,axis,starts));}
    }
    let(_,axis,order)=best.unwrap();Ok((axis,order))
}

fn scale_points(points:&[[f64;3]])->Result<Vec<[f64;3]>>{
    let maximum=points.iter().flatten().map(|v|v.abs()).fold(0f64,f64::max);if maximum==0.{return Err(invalid("global validation encountered zero geometry"));}
    let exponent=(maximum.log2().floor()as i32).clamp(-1022,1022);let scale=2f64.powi(-exponent);let out=points.iter().map(|p|p.map(|v|v*scale)).collect::<Vec<_>>();
    // Power-of-two scaling is exact. This exponent window ensures all degree-3
    // determinant products/tails stay representable by the robust predicates.
    if out.iter().flatten().any(|v|!v.is_finite()||(*v!=0.&&v.abs()<2f64.powi(-128))){return Err(invalid("global validation coordinate dynamic range is unsupported"));}Ok(out)
}
fn orientation(a:[f64;3],b:[f64;3],c:[f64;3],d:[f64;3])->f64{orient3d(dvec3(a[0],a[1],a[2]),dvec3(b[0],b[1],b[2]),dvec3(c[0],c[1],c[2]),dvec3(d[0],d[1],d[2]))}
fn triangles_intersect(a:&Tri,b:&Tri)->Result<bool>{
    let scaled=scale_points(&a.points.into_iter().chain(b.points).collect::<Vec<_>>())?;let ap=[scaled[0],scaled[1],scaled[2]];let bp=[scaled[3],scaled[4],scaled[5]];
    let da=ap.map(|p|orientation(bp[0],bp[1],bp[2],p));let db=bp.map(|p|orientation(ap[0],ap[1],ap[2],p));
    if separated(da)||separated(db){return Ok(false);}let na=cross(sub(ap[1],ap[0]),sub(ap[2],ap[0]));let nb=cross(sub(bp[1],bp[0]),sub(bp[2],bp[0]));
    if length(na)==0.||length(nb)==0.{return Err(invalid("global triangle orientation is unresolved"));}
    for i in 0..3 {let j=(i+1)%3;if segment_hits(ap[i],ap[j],a.vertices[i],a.vertices[j],bp,b.vertices,da[i],da[j],nb){return Ok(true);}
        if segment_hits(bp[i],bp[j],b.vertices[i],b.vertices[j],ap,a.vertices,db[i],db[j],na){return Ok(true);}}
    Ok(false)
}
fn separated(d:[f64;3])->bool{d.iter().all(|v|*v>0.)||d.iter().all(|v|*v<0.)}
fn same_sign(d:[f64;3])->bool{d.iter().all(|v|*v>=0.)||d.iter().all(|v|*v<=0.)}
fn segment_hits(p:[f64;3],q:[f64;3],pid:VertexId,qid:VertexId,tri:[[f64;3];3],ids:[VertexId;3],dp:f64,dq:f64,normal:[f64;3])->bool{
    if (dp>0.&&dq>0.)||(dp<0.&&dq<0.){return false;}
    let axis=(0..3).max_by(|&a,&b|normal[a].abs().total_cmp(&normal[b].abs())).unwrap();let project=|p:[f64;3]|[p[(axis+1)%3],p[(axis+2)%3]];
    let t=tri.map(project);let pp=project(p);let qq=project(q);
    if dp==0.&&point_in_triangle(pp,t)&&!ids.contains(&pid){return true;}
    if dq==0.&&point_in_triangle(qq,t)&&!ids.contains(&qid){return true;}
    if dp==0.&&dq==0.{
        for i in 0..3{let j=(i+1)%3;if EdgeKey::new(pid,qid)==EdgeKey::new(ids[i],ids[j]){continue;}
            let a=orient(pp,qq,t[i]);let b=orient(pp,qq,t[j]);let c=orient(t[i],t[j],pp);let d=orient(t[i],t[j],qq);
            if opposite(a,b)&&opposite(c,d){return true;}
            if a==0.&&b==0.{let k=if (pp[0]-qq[0]).abs()>=(pp[1]-qq[1]).abs(){0}else{1};let low=pp[k].min(qq[k]).max(t[i][k].min(t[j][k]));let high=pp[k].max(qq[k]).min(t[i][k].max(t[j][k]));if low<high{return true;}}
        }return false;
    }
    if opposite(dp,dq){return same_sign([orientation(p,q,tri[0],tri[1]),orientation(p,q,tri[1],tri[2]),orientation(p,q,tri[2],tri[0])]);}
    false
}
fn opposite(a:f64,b:f64)->bool{(a>0.&&b<0.)||(a<0.&&b>0.)}
fn point_in_triangle(p:[f64;2],t:[[f64;2];3])->bool{same_sign([orient(t[0],t[1],p),orient(t[1],t[2],p),orient(t[2],t[0],p)])}
fn face_components(mesh:&Mesh,ctx:&mut Context<'_>)->Result<Vec<BTreeSet<FaceId>>>{
    let adj=mesh.adjacency(ctx)?;let mut neighbors=BTreeMap::<FaceId,Vec<FaceId>>::new();for uses in adj.edges.values(){for u in uses{for v in uses{ctx.checkpoint(1)?;if u.face!=v.face{neighbors.entry(u.face).or_default().push(v.face);}}}}
    let mut remaining=mesh.faces.iter().map(|f|f.id).collect::<BTreeSet<_>>();let mut components=Vec::new();while let Some(&seed)=remaining.first(){remaining.remove(&seed);let mut queue=VecDeque::from([seed]);let mut component=BTreeSet::new();
        while let Some(face)=queue.pop_front(){ctx.checkpoint(1)?;component.insert(face);for &other in neighbors.get(&face).into_iter().flatten(){if remaining.remove(&other){queue.push_back(other);}}}components.push(component);
    }Ok(components)
}
fn shell_sign(tris:&[&Tri],ctx:&mut Context<'_>)->Result<i32>{
    let all=tris.iter().flat_map(|t|t.points).collect::<Vec<_>>();let scaled=scale_points(&all)?;let origin=scaled[0];let mut signed=0.;let mut magnitude=0.;
    // orient3d has the opposite sign to the usual outward signed-volume sum.
    for p in scaled.chunks_exact(3){ctx.checkpoint(1)?;let volume=-orientation(p[0],p[1],p[2],origin);signed+=volume;magnitude+=volume.abs();}
    if !signed.is_finite()||signed.abs()<=magnitude*(tris.len()as f64*f64::EPSILON*64.).max(1e-12){return Err(invalid("global shell orientation is numerically unresolved"));}Ok(if signed>0.{1}else{-1})
}
fn inside_shell(point:[f64;3],tris:&[&Tri],ctx:&mut Context<'_>)->Result<bool>{
    let mut angle=0.;for tri in tris{ctx.checkpoint(1)?;let vectors=tri.points.map(|p|sub(p,point));let maximum=vectors.iter().map(|v|length(*v)).fold(0f64,f64::max);if maximum==0.{return Err(invalid("unresolved shell contact"));}
        let p=vectors.map(|v|mul(v,1./maximum));let lengths=p.map(length);if lengths.iter().any(|v|*v==0.){return Err(invalid("unresolved shell contact"));}
        let determinant=dot(p[0],cross(p[1],p[2]));let denominator=lengths.iter().product::<f64>()+dot(p[0],p[1])*lengths[2]+dot(p[1],p[2])*lengths[0]+dot(p[2],p[0])*lengths[1];angle+=2.*determinant.atan2(denominator);
    }
    let winding=(angle/(4.*std::f64::consts::PI)).abs();if !winding.is_finite()||winding.min((winding-1.).abs())>1e-6{return Err(invalid("shell nesting is numerically unresolved"));}Ok(winding>0.5)
}
