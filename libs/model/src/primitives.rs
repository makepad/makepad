use crate::{mesh::{self, Polygon}, Error, Limits, Result};

pub(crate) fn sphere(radius:f64,segments:u32,rings:u32,smooth:bool,limits:&Limits)->Result<Vec<u8>> {
    if !radius.is_finite() || radius<=0.0 || !(3..=128).contains(&segments) || !(2..=128).contains(&rings) {
        return Err(Error::Invalid("sphere needs positive radius, 3..128 segments and 2..128 rings"));
    }
    let n=segments as usize; let r=rings as usize;
    preflight((r-1)*n+2,r*n,n*(4*r-2),limits)?;
    let mut positions=Vec::with_capacity((r-1)*n+2); positions.push([0.,radius,0.]);
    for row in 1..r {
        let theta=std::f64::consts::PI*row as f64/r as f64;
        for col in 0..n {
            let phi=std::f64::consts::TAU*col as f64/n as f64;
            positions.push([radius*theta.sin()*phi.cos(),radius*theta.cos(),radius*theta.sin()*phi.sin()]);
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
        let a=std::f64::consts::TAU*col as f64/n as f64;
        positions.push([radius*a.cos(),y,radius*a.sin()]);
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
fn preflight(vertices:usize,faces:usize,corners:usize,limits:&Limits)->Result<()> {
    if vertices>limits.mesh.max_vertices || faces>limits.mesh.max_faces || corners>limits.mesh.max_corners
        || vertices.saturating_mul(128).saturating_add(corners.saturating_mul(160))>limits.mesh.max_bytes {
        return Err(Error::Budget("primitive geometry"));
    }
    Ok(())
}
