//! Seeded surface fibers as ordinary closed geometry. No transparent shell
//! ordering, geometry shader, per-frame allocation or extra renderer is needed.
use super::*;

pub(super) const MAX_STRANDS: u32 = 32_768;
use std::collections::{BTreeSet,HashMap};

#[derive(Clone,Copy)]
struct Capsule {a:[f64;3],b:[f64;3],radius:f64}

/// Each strand lies inside two capsules, including the embedded root cap.
/// Grid iteration and candidate order are deterministic; the hash table is
/// only queried by key, never iterated to choose a strand.
struct Spacing {
    cell_size:f64,
    cells:HashMap<[i64;3],Vec<usize>>,
    strands:Vec<[Capsule;2]>,
}
impl Spacing {
    fn accept(&mut self,capsules:[Capsule;2],ctx:&mut Context<'_>)->Result<bool> {
        let mut low=[f64::INFINITY;3];let mut high=[f64::NEG_INFINITY;3];
        for capsule in capsules {for p in [capsule.a,capsule.b] {for axis in 0..3 {
            low[axis]=low[axis].min(p[axis]-capsule.radius);high[axis]=high[axis].max(p[axis]+capsule.radius);
        }}}
        let cell=|p:[f64;3]|->Result<[i64;3]> {
            let p=p.map(|v|(v/self.cell_size).floor());
            // Refuse ranges where f64 arithmetic cannot preserve useful
            // sub-cell distances. Saturating float casts must not alias bins.
            if p.iter().any(|v|!v.is_finite()||v.abs()>1e9){return Err(Error::Invalid("fiber_shell spacing coordinate range"));}
            Ok(p.map(|v|v as i64))
        };
        let low=cell(low)?;let high=cell(high)?;
        if (0..3).any(|i|high[i]-low[i]>2){return Err(Error::Budget("fiber_shell spacing cells"));}
        let mut keys=Vec::with_capacity(27);let mut neighbors=BTreeSet::new();
        for x in low[0]..=high[0] {for y in low[1]..=high[1] {for z in low[2]..=high[2] {
            ctx.checkpoint(1)?;let key=[x,y,z];keys.push(key);
            if let Some(entries)=self.cells.get(&key) {
                ctx.checkpoint(entries.len() as u64)?;neighbors.extend(entries.iter().copied());
            }
        }}}
        for index in neighbors {
            ctx.checkpoint(4)?;
            for a in capsules {for b in self.strands[index] {
                if segment_distance_squared(a.a,a.b,b.a,b.b)<=(a.radius+b.radius).powi(2){return Ok(false);}
            }}
        }
        let index=self.strands.len();self.strands.push(capsules);
        for key in keys {self.cells.entry(key).or_default().push(index);}
        Ok(true)
    }
}

fn point_segment_distance_squared(p:[f64;3],a:[f64;3],b:[f64;3])->f64 {
    let ab=sub(b,a);let length=dot(ab,ab);
    let t=if length>0.{(dot(sub(p,a),ab)/length).clamp(0.,1.)}else{0.};
    let d=sub(p,add(a,mul(ab,t)));dot(d,d)
}
fn segment_distance_squared(a:[f64;3],b:[f64;3],c:[f64;3],d:[f64;3])->f64 {
    let mut distance=point_segment_distance_squared(a,c,d).min(point_segment_distance_squared(b,c,d))
        .min(point_segment_distance_squared(c,a,b)).min(point_segment_distance_squared(d,a,b));
    let u=sub(b,a);let v=sub(d,c);let w=sub(a,c);let axis=cross(u,v);let denominator=dot(axis,axis);
    // Cross-product form avoids catastrophic cancellation in a*c-b*b for
    // nearly parallel segments. Endpoint projections cover parallel cases.
    if denominator>0. {
        let s=dot(cross(v,w),axis)/denominator;let t=dot(cross(u,w),axis)/denominator;
        if (0.0..=1.).contains(&s)&&(0.0..=1.).contains(&t) {
            let delta=sub(add(a,mul(u,s)),add(c,mul(v,t)));distance=distance.min(dot(delta,delta));
        }
    }
    // Numerical ambiguity rejects the candidate instead of admitting a
    // potentially intersecting strand.
    if distance.is_finite(){distance}else{0.}
}

struct Random(u64);
impl Random {
    fn next(&mut self)->f64 {
        self.0=self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut n=self.0;n=(n^(n>>30)).wrapping_mul(0xbf58476d1ce4e5b9);
        n=(n^(n>>27)).wrapping_mul(0x94d049bb133111eb);n^=n>>31;
        (n>>11)as f64/(1u64<<53)as f64
    }
}
pub(super) fn generate(source:&Mesh,count:u32,length:f64,width:f64,seed:u32,material:u32,ctx:&mut Context<'_>)->Result<Mesh> {
    if !(1..=MAX_STRANDS).contains(&count)||!(0.0001..=1.).contains(&length)||!(0.00001..=0.1).contains(&width)||width>length*0.5 {
        return Err(Error::Invalid("fiber_shell count/length/width"));
    }
    let count=count as usize;admit(count*7,count*7,count*24,ctx)?;
    let packed=source.triangulate(ctx)?;
    // Triangulation already enforces the document triangle and work limits.
    let working=count.saturating_mul(31*1024+27*64+512).saturating_add(source.memory_bytes()).saturating_add(packed.triangles.len().saturating_mul(512));
    if working>ctx.limits.max_bytes{return Err(Error::Budget("fiber_shell spacing working bytes"));}
    let triangles=packed.triangles.iter().map(|t|SourceTriangle{
        points:t.indices.map(|i|packed.vertices[i as usize].position),
        uv:t.indices.map(|i|packed.vertices[i as usize].uv),
        normals:t.indices.map(|i|packed.vertices[i as usize].normal),
        weights:t.indices.map(|i|packed.vertices[i as usize].weights.clone()),material:t.material,
    }).collect::<Vec<_>>();
    let mut cumulative=Vec::with_capacity(triangles.len());let mut total=0.;
    for triangle in &triangles {
        total+=length3(cross(sub(triangle.points[1],triangle.points[0]),sub(triangle.points[2],triangle.points[0])))*0.5;
        cumulative.push(total);
    }
    if !total.is_finite()||total<=1e-12 {return Err(Error::Invalid("fiber_shell needs a nondegenerate surface"));}
    let mut random=Random(seed as u64);let mut positions=Vec::with_capacity(count*7);let mut polygons=Vec::with_capacity(count*7);
    let mut root_weights=Vec::with_capacity(count);
    let mut spacing=Spacing{cell_size:length*2.+width*2.,cells:HashMap::new(),strands:Vec::with_capacity(count)};
    for _ in 0..count {
        let mut accepted=None;
        for _ in 0..64 {
            ctx.checkpoint(24)?;
            let chosen=(random.next()*total).min(total-f64::EPSILON);
            let i=cumulative.partition_point(|&area|area<=chosen).min(triangles.len()-1);
            let triangle=&triangles[i];let u=random.next().sqrt();let v=random.next();let weights=[1.-u,u*(1.-v),u*v];
            let root=std::array::from_fn(|axis|(0..3).map(|k|triangle.points[k][axis]*weights[k]).sum());
            let n=unit(std::array::from_fn(|axis|(0..3).map(|k|triangle.normals[k][axis]*weights[k]).sum()))?;
            let uv=std::array::from_fn(|axis|(0..3).map(|k|triangle.uv[k][axis]*weights[k]).sum());
            let t=frame_normal(n)?;let b=cross(n,t);let angle=random.next()*std::f64::consts::TAU;
            let bend=add(mul(t,angle.cos()),mul(b,angle.sin()));
            let height=length*(0.65+random.next()*0.7);let radius=width*0.5*(0.7+random.next()*0.6);
            let centers=[-0.06,0.52,1.].map(|along|add(root,add(mul(n,height*along),mul(bend,height*0.18*along*along))));
            let padding=1e-9*(1.+centers.iter().flatten().map(|v|v.abs()).fold(0.,f64::max));
            let capsules=[Capsule{a:centers[0],b:centers[1],radius:radius+padding},Capsule{a:centers[1],b:centers[2],radius:radius*0.6+padding}];
            if spacing.accept(capsules,ctx)? {accepted=Some((i,weights,uv,t,b,angle,radius,centers));break;}
        }
        let (i,weights,uv,t,b,angle,radius,centers)=accepted.ok_or(Error::Budget("fiber_shell packing: could not place requested count within 64 attempts per strand; reduce count, width or length"))?;
        let triangle=&triangles[i];
        let first=positions.len() as u32;
        for (center,size) in [(centers[0],1.),(centers[1],0.6)] {
            for side in 0..3 {
                let turn=angle+side as f64*std::f64::consts::TAU/3.;
                positions.push(add(center,add(mul(t,turn.cos()*radius*size),mul(b,turn.sin()*radius*size))));
            }
        }
        positions.push(centers[2]);
        polygons.push(Polygon{vertices:vec![first+2,first+1,first],uvs:vec![uv;3],material});
        for side in 0..3 {
            let next=(side+1)%3;
            polygons.push(Polygon{vertices:vec![first+side,first+next,first+3+next,first+3+side],uvs:vec![uv;4],material});
            polygons.push(Polygon{vertices:vec![first+3+side,first+3+next,first+6],uvs:vec![uv;3],material});
        }
        let mut joint_weights=BTreeMap::<u32,f64>::new();
        for k in 0..3 {for weight in &triangle.weights[k] {*joint_weights.entry(weight.joint).or_default()+=weight.weight*weights[k];}}
        let mut joint_weights=joint_weights.into_iter().map(|(joint,weight)|JointWeight{joint,weight}).collect::<Vec<_>>();
        joint_weights.sort_by(|a,b|b.weight.total_cmp(&a.weight).then(a.joint.cmp(&b.joint)));joint_weights.truncate(4);
        let sum: f64=joint_weights.iter().map(|w|w.weight).sum();if sum>0.{for weight in &mut joint_weights{weight.weight/=sum;}}
        root_weights.push(joint_weights);
    }
    let mut mesh=Mesh::from_polygons(&positions,&polygons,ctx)?;
    mesh.recalculate_normals(true,std::f64::consts::PI,ctx)?;
    let weights=mesh.vertices().iter().enumerate().map(|(i,v)|(v.id,root_weights[i/7].clone())).collect::<Vec<_>>();
    // Dense weights are inherited from each root; subsequent soft_body_bind
    // can replace them with cell embeddings for the complete fiber mesh.
    let weights=weights.into_iter().filter(|(_,weights)|!weights.is_empty()).collect::<Vec<_>>();
    if !weights.is_empty(){mesh.set_weights_bulk(&weights,ctx)?;}
    Ok(mesh)
}
fn length3(v:[f64;3])->f64{super::length(v)}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fibers_are_seeded_closed_and_surface_rooted() {
        let mut context=Context::default();let cube=Mesh::cube([1.;3],&mut context).unwrap();
        let a=generate(&cube,64,0.02,0.001,42,3,&mut context).unwrap();
        let b=generate(&cube,64,0.02,0.001,42,3,&mut context).unwrap();
        assert_eq!(a.to_bytes(&mut context).unwrap(),b.to_bytes(&mut context).unwrap());
        assert_eq!(a.vertices().len(),64*7);assert_eq!(a.faces().len(),64*7);
        let topology=a.adjacency(&mut context).unwrap();
        for edge in topology.vertex_edges.values().flatten(){assert_eq!(topology.radial(*edge).len(),2);}
        for vertex in a.vertices(){assert!(vertex.position.into_iter().all(|v|v.is_finite()&&v.abs()<0.54));}
        assert!(a.corners().iter().all(|corner|corner.normal.is_some()));
    }

    #[test]
    fn capsules_reject_crossing_and_touching_strands_across_grid_boundaries() {
        let mut context=Context::default();
        let mut grid=Spacing{cell_size:4.,cells:HashMap::new(),strands:Vec::new()};
        let a=Capsule{a:[-1.,0.,0.],b:[1.,0.,0.],radius:0.02};
        assert!(grid.accept([a,a],&mut context).unwrap());
        let crossing=Capsule{a:[0.,-1.,0.],b:[0.,1.,0.],radius:0.02};
        assert!(!grid.accept([crossing,crossing],&mut context).unwrap());
        let touching=Capsule{a:[-1.,0.04,0.],b:[1.,0.04,0.],radius:0.02};
        assert!(!grid.accept([touching,touching],&mut context).unwrap());
        let clear=Capsule{a:[-1.,0.1,0.],b:[1.,0.1,0.],radius:0.02};
        assert!(grid.accept([clear,clear],&mut context).unwrap());
        assert_eq!(grid.strands.len(),2);
        assert!((segment_distance_squared([0.;3],[1.,0.,0.],[2.,1.,0.],[2.,2.,0.])-2.).abs()<1e-12);
        assert!((segment_distance_squared([0.;3],[0.;3],[0.,0.3,0.],[1.,0.3,0.])-0.09).abs()<1e-12);
        assert!(segment_distance_squared([-1.,0.,0.],[1.,0.,0.],[0.,-1.,0.],[0.,1.,0.])<1e-24);
    }

    fn dented_sphere()->Mesh {
        let mut ctx=Context::default();
        let bytes=crate::primitives::sphere(0.45,64,40,true,&Limits::default()).unwrap();
        let sphere=Mesh::from_bytes(&bytes,&mut ctx).unwrap();
        let mut positions=Vec::new();let mut indices=BTreeMap::new();
        for vertex in sphere.vertices() {
            let mut p=vertex.position;
            let ellipse=(p[0]/0.185).powi(2)+((p[1]+0.085)/0.09).powi(2);
            if p[2]< -0.2&&ellipse<1.{p[2]+=0.10*(1.-ellipse).powi(2);}
            p[1]+=0.77;indices.insert(vertex.id,positions.len()as u32);positions.push(p);
        }
        let triangles=sphere.triangulate(&mut ctx).unwrap();
        let polygons=triangles.triangles.iter().map(|triangle|Polygon{
            vertices:triangle.indices.map(|i|indices[&triangles.vertices[i as usize].source_vertex]).to_vec(),
            uvs:triangle.indices.map(|i|triangles.vertices[i as usize].uv).to_vec(),material:0,
        }).collect::<Vec<_>>();
        assert_eq!(polygons.len(),4992);
        let mut normals=vec![[0.;3];positions.len()];
        for triangle in &polygons {
            let [a,b,c]=std::array::from_fn(|i|positions[triangle.vertices[i]as usize]);
            let normal=cross(sub(b,a),sub(c,a));
            for &index in &triangle.vertices{normals[index as usize]=add(normals[index as usize],normal);}
        }
        let mut sphere=Mesh::from_polygons(&positions,&polygons,&mut ctx).unwrap();
        let indices=sphere.vertices().iter().enumerate().map(|(i,v)|(v.id,i)).collect::<BTreeMap<_,_>>();
        let values=sphere.corners().iter().map(|corner|(corner.id,Some(unit(normals[indices[&corner.vertex]]).unwrap()))).collect::<Vec<_>>();
        sphere.set_corner_normals_bulk(&values,&mut ctx).unwrap();sphere
    }

    #[test]
    fn dense_bent_fibers_on_dented_sphere_are_closed_and_globally_nonintersecting() {
        let source=dented_sphere();
        let before=source.to_bytes(&mut Context::default()).unwrap();
        let mut context=Context::default();
        let fibers=generate(&source,1400,0.012,0.0006,42,3,&mut context).unwrap();
        assert_eq!(fibers.vertices().len(),1400*7);assert_eq!(fibers.faces().len(),1400*7);
        let report=fibers.validate_global(&mut Context::default()).unwrap();
        assert_eq!(report.components,1400);
        assert_eq!(report.intersection_count,0,"{report:?}");
        assert!(report.is_valid_solid,"{report:?}");
        let repeat=generate(&source,1400,0.012,0.0006,42,3,&mut Context::default()).unwrap();
        assert_eq!(fibers.to_bytes(&mut Context::default()).unwrap(),repeat.to_bytes(&mut Context::default()).unwrap());
        assert_eq!(source.to_bytes(&mut Context::default()).unwrap(),before);
    }

    #[test]
    fn impossible_packing_and_cancellation_refuse_without_a_partial_shell() {
        let source=Mesh::cube([0.001;3],&mut Context::default()).unwrap();
        let before=source.to_bytes(&mut Context::default()).unwrap();
        let error=generate(&source,2,0.1,0.05,42,0,&mut Context::default()).unwrap_err();
        assert!(matches!(error,Error::Budget(message)if message.contains("packing")));
        let error=generate(&source,2,0.01,0.001,42,0,&mut Context::new(Default::default(),Some(&||true))).unwrap_err();
        assert!(matches!(error,Error::Mesh(crate::mesh::MeshError::Cancelled)));
        assert_eq!(source.to_bytes(&mut Context::default()).unwrap(),before);
    }
}
