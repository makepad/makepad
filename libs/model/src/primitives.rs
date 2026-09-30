use crate::{mesh::{self, Polygon}, Error, Limits, Result};

pub(crate) fn sphere(radius:f64,segments:u32,rings:u32,smooth:bool,limits:&Limits)->Result<Vec<u8>> {
    if !radius.is_finite() || radius<=0.0 || !(3..=128).contains(&segments) || !(2..=128).contains(&rings) {
        return Err(Error::Invalid("sphere needs positive radius, 3..128 segments and 2..128 rings"));
    }
    let n=segments as usize; let r=rings as usize;
    preflight((r-1)*n+2,r*n,n*(4*r-2),limits)?;
    let mut positions=Vec::with_capacity((r-1)*n+2); positions.push([0.,radius,0.]);
    for row in 1..r {
        let (theta_cos,theta_sin)=turn(row,2*r);
        for col in 0..n {
            let (phi_cos,phi_sin)=turn(col,n);
            positions.push([radius*theta_sin*phi_cos,radius*theta_cos,radius*theta_sin*phi_sin]);
        }
    }
    let bottom=positions.len() as u32; positions.push([0.,-radius,0.]);
    let index=|row:usize,col:usize| (1+(row-1)*n+col%n) as u32;
    let uv=|row:usize,col:usize| [col as f64/n as f64,row as f64/r as f64];
    let mut polygons=Vec::with_capacity(r*n);
    for col in 0..n {
        polygons.push(Polygon {vertices:vec![0,index(1,col+1),index(1,col)],uvs:vec![[(col as f64+0.5)/n as f64,0.],uv(1,col+1),uv(1,col)],material:0});
        for row in 1..r-1 {
            // Spherical latitude quads are coplanar; winding points outward.
            polygons.push(Polygon {vertices:vec![index(row,col),index(row,col+1),index(row+1,col+1),index(row+1,col)],
                uvs:vec![uv(row,col),uv(row,col+1),uv(row+1,col+1),uv(row+1,col)],material:0});
        }
        polygons.push(Polygon {vertices:vec![index(r-1,col),index(r-1,col+1),bottom],
            uvs:vec![uv(r-1,col),uv(r-1,col+1),[(col as f64+0.5)/n as f64,1.]],material:0});
    }
    let mut ctx=mesh::Context::new(limits.mesh.clone(),None);
    let mut mesh=mesh::Mesh::from_polygons(&positions,&polygons,&mut ctx)?;
    if smooth {
        // Per-corner storage preserves UV seams while all corners at the same
        // sphere vertex share its analytic normal, including the two poles.
        let normals=mesh.corners().iter().map(|corner| {
            let p=mesh.vertex(corner.vertex).unwrap().position;
            (corner.id,Some(p.map(|v|v/radius)))
        }).collect::<Vec<_>>();
        mesh.set_corner_normals_bulk(&normals,&mut ctx)?;
    }
    Ok(mesh.to_bytes(&mut ctx)?)
}

pub(crate) fn cylinder(radius:f64,height:f64,segments:u32,smooth:bool,limits:&Limits)->Result<Vec<u8>> {
    if !radius.is_finite() || radius<=0.0 || !height.is_finite() || height<=0.0 || !(3..=128).contains(&segments) {
        return Err(Error::Invalid("cylinder needs positive radius/height and 3..128 segments"));
    }
    let n=segments as usize;
    preflight(n*2,n+2,n*6,limits)?;
    let mut positions=Vec::with_capacity(n*2);
    for y in [-height*0.5,height*0.5] { for col in 0..n {
        let (cos,sin)=turn(col,n);
        positions.push([radius*cos,y,radius*sin]);
    } }
    let mut polygons=Vec::new();
    polygons.push(Polygon {vertices:(0..n as u32).collect(),uvs:positions[..n].iter().map(|p|[p[0]/(2.*radius)+0.5,p[2]/(2.*radius)+0.5]).collect(),material:0});
    polygons.push(Polygon {vertices:(n as u32..(n*2) as u32).rev().collect(),uvs:positions[n..].iter().rev().map(|p|[p[0]/(2.*radius)+0.5,p[2]/(2.*radius)+0.5]).collect(),material:0});
    for col in 0..n {
        let next=(col+1)%n; let u=col as f64/n as f64; let v=(col+1) as f64/n as f64;
        polygons.push(Polygon {vertices:vec![col as u32,(col+n) as u32,(next+n) as u32,next as u32],uvs:vec![[u,1.],[u,0.],[v,0.],[v,1.]],material:0});
    }
    let mut ctx=mesh::Context::new(limits.mesh.clone(),None);
    let mut mesh=mesh::Mesh::from_polygons(&positions,&polygons,&mut ctx)?;
    if smooth {
        let mut normals=Vec::with_capacity(mesh.corners().len());
        for (index,face) in mesh.faces().iter().enumerate() {
            for corner in mesh.face_corners(face.id)? {
                let p=mesh.vertex(corner.vertex).unwrap().position;
                // The first two polygons are the bottom and top caps. Do not
                // average their normals into the radial side wall at the rim.
                let normal=match index {0=>[0.,-1.,0.],1=>[0.,1.,0.],_=>[p[0]/radius,0.,p[2]/radius]};
                normals.push((corner.id,Some(normal)));
            }
        }
        mesh.set_corner_normals_bulk(&normals,&mut ctx)?;
    }
    Ok(mesh.to_bytes(&mut ctx)?)
}
/// cos and sin of TAU·k/n, the same bits on every platform. The platform
/// libm's last bit differs (Apple's and glibc's disagree), which gave one
/// document a different content hash on a Mac and on Linux. Symmetry takes
/// the angle into the first octant, where two short series of IEEE-exact
/// operations finish it; quarter turns come out exactly 0 and ±1.
pub(crate) fn turn(k:usize,n:usize)->(f64,f64) {
    let k=k%n; let octant=8*k/n; let rem=8*k-octant*n;
    let series=|x:f64| {
        let x2=x*x;
        let sin=x*(1.-x2/6.*(1.-x2/20.*(1.-x2/42.*(1.-x2/72.*(1.-x2/110.*(1.-x2/156.*(1.-x2/210.*(1.-x2/272.))))))));
        let cos=1.-x2/2.*(1.-x2/12.*(1.-x2/30.*(1.-x2/56.*(1.-x2/90.*(1.-x2/132.*(1.-x2/182.*(1.-x2/240.)))))));
        (cos,sin)
    };
    // The angle inside its quadrant: from the quadrant's start in an even
    // octant, back from its end in an odd one.
    let (c,s)=if octant%2==0 {series(rem as f64/n as f64*std::f64::consts::FRAC_PI_4)}
        else {let (c,s)=series((n-rem) as f64/n as f64*std::f64::consts::FRAC_PI_4);(s,c)};
    match octant/2 {0=>(c,s),1=>(-s,c),2=>(-c,-s),_=>(s,-c)}
}
fn preflight(vertices:usize,faces:usize,corners:usize,limits:&Limits)->Result<()> {
    if vertices>limits.mesh.max_vertices || faces>limits.mesh.max_faces || corners>limits.mesh.max_corners
        || vertices.saturating_mul(128).saturating_add(corners.saturating_mul(160))>limits.mesh.max_bytes {
        return Err(Error::Budget("primitive geometry"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn turn_is_exact_at_quarters_and_matches_libm_elsewhere(){
        for n in [3usize,4,5,6,7,8,12,16,24,32,33,64,127,128,256] {
            for k in 0..n {
                let (c,s)=super::turn(k,n);
                // libm's own input TAU·k/n is rounded (up to 9e-16 near a
                // full turn), so agreement is to a few ulp of the angle.
                let a=std::f64::consts::TAU*k as f64/n as f64;
                assert!((c-a.cos()).abs()<=2e-15&&(s-a.sin()).abs()<=2e-15,"{k}/{n}: {c} {s}");
                assert!((c*c+s*s-1.).abs()<=4e-16,"{k}/{n} off the unit circle");
                if (4*k)%n==0 {assert!(c.abs().fract()==0.&&s.abs().fract()==0.,"quarter turn {k}/{n} must be exact: {c} {s}");}
            }
        }
    }
}
