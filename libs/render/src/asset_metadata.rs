//! Bounded collision proxies and MSFT_lod geometry choices in owner-local space.
use makepad_gltf::{GltfDocument,JsonValue};
#[derive(Clone,Debug,PartialEq)]pub enum AssetCollisionShape{Box{min:[f32;3],max:[f32;3]},Sphere{center:[f32;3],radius:f32},Mesh{positions:Vec<[f32;3]>,indices:Vec<u32>}}
#[derive(Clone,Debug,PartialEq)]pub struct AssetCollision{pub node:usize,pub shape:AssetCollisionShape}
/// Prepared once on the asset worker. `shape` stays node-local, while `mesh`
/// already includes `rest_transform` and is in model coordinates. The rest
/// transform is metadata; consumers must not apply it to the mesh again.
#[derive(Clone,Debug)]pub struct PreparedAssetCollision{pub node:usize,pub shape:AssetCollisionShape,pub rest_transform:makepad_draw::makepad_math::Mat4f,pub mesh:std::sync::Arc<makepad_scene::mesh::PreparedMesh>}
pub fn prepare_asset_collisions(document:&GltfDocument)->Result<std::sync::Arc<Vec<PreparedAssetCollision>>,String>{
    let shapes=parse_asset_collisions(document)?;
    if shapes.is_empty(){return Ok(Default::default());}
    let worlds=crate::asset_sockets::node_rest_transforms(document)?;
    let mut out=Vec::new();
    for collision in shapes {
        let rest_transform=worlds[collision.node];
        let (mut positions,mut indices)=collision_triangles(&collision.shape);
        for p in &mut positions {let m=&rest_transform.v;let [x,y,z]=*p;*p=[m[0]*x+m[4]*y+m[8]*z+m[12],m[1]*x+m[5]*y+m[9]*z+m[13],m[2]*x+m[6]*y+m[10]*z+m[14]];}
        let m=&rest_transform.v;let det=m[0]*(m[5]*m[10]-m[6]*m[9])-m[4]*(m[1]*m[10]-m[2]*m[9])+m[8]*(m[1]*m[6]-m[2]*m[5]);
        if !det.is_finite()||det.abs()<1e-10{return Err("singular authored collision owner transform".into());}
        if det<0. {for triangle in indices.chunks_exact_mut(3){triangle.swap(1,2);}}
        let mesh=prepare_mesh(positions,indices)?;
        out.push(PreparedAssetCollision{node:collision.node,shape:collision.shape,rest_transform,mesh});
    }
    Ok(std::sync::Arc::new(out))
}
/// Keep the former cooking admission limits at the public geometry boundary.
/// Welding and acceleration validation remain the physics producer's job.
fn prepare_mesh(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> Result<std::sync::Arc<makepad_scene::mesh::PreparedMesh>, String> {
    use makepad_draw::makepad_math::vec3f;
    if positions.len() < 3 || positions.len() > 100_000 || indices.len() < 3
        || indices.len() % 3 != 0 || indices.len() > 24_576 {
        return Err("authored collision geometry budget".into());
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in &positions {
        for i in 0..3 {
            if !p[i].is_finite() || p[i].abs() > 1e6 {
                return Err("authored collision coordinate".into());
            }
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    if indices.iter().any(|i| *i as usize >= positions.len()) {
        return Err("authored collision index".into());
    }
    for triangle in indices.chunks_exact(3) {
        let [p, q, r] = [positions[triangle[0] as usize], positions[triangle[1] as usize], positions[triangle[2] as usize]];
        let a = std::array::from_fn::<_, 3, _>(|i| q[i] as f64 - p[i] as f64);
        let b = std::array::from_fn::<_, 3, _>(|i| r[i] as f64 - p[i] as f64);
        let cross = [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]];
        if cross.iter().map(|v| v*v).sum::<f64>() < 1e-24 {
            return Err("degenerate authored collision triangles".into());
        }
    }
    Ok(std::sync::Arc::new(makepad_scene::mesh::PreparedMesh {
        positions, indices,
        min: vec3f(min[0], min[1], min[2]),
        max: vec3f(max[0], max[1], max[2]),
    }))
}
/// Spheres are deterministic 64-longitude / 32-latitude triangle shells
/// (3968 triangles). The authored sphere/ellipsoid remains in source metadata;
/// physics uses this bounded tessellation instead of an enclosing box.
fn collision_triangles(shape:&AssetCollisionShape)->(Vec<[f32;3]>,Vec<u32>){
    match shape{
        AssetCollisionShape::Mesh{positions,indices}=>(positions.clone(),indices.clone()),
        AssetCollisionShape::Box{min,max}=>{
            let p=(0..8).map(|i|std::array::from_fn(|a|if i&(1<<a)==0{min[a]}else{max[a]})).collect();
            (p,vec![0,4,2,2,4,6,1,3,5,3,7,5,0,1,4,1,5,4,2,6,3,3,6,7,0,2,1,1,2,3,4,5,6,5,7,6])
        },
        AssetCollisionShape::Sphere{center,radius}=>{
            const N:usize=64;const R:usize=32;
            let mut p=vec![[center[0],center[1]+radius,center[2]]];
            for ring in 1..R{let angle=std::f32::consts::PI*ring as f32/R as f32;let(y,r)=(angle.cos(),angle.sin());for j in 0..N{let angle=std::f32::consts::TAU*j as f32/N as f32;p.push([center[0]+radius*r*angle.cos(),center[1]+radius*y,center[2]+radius*r*angle.sin()]);}}
            let bottom=p.len()as u32;p.push([center[0],center[1]-radius,center[2]]);let mut indices=Vec::new();
            for j in 0..N{indices.extend([0,(1+(j+1)%N)as u32,(1+j)as u32]);}
            for ring in 0..R-2{let a=1+ring*N;let b=a+N;for j in 0..N{let k=(j+1)%N;indices.extend([(a+j)as u32,(a+k)as u32,(b+j)as u32,(a+k)as u32,(b+k)as u32,(b+j)as u32]);}}
            let last=1+(R-2)*N;for j in 0..N{indices.extend([bottom,(last+j)as u32,(last+(j+1)%N)as u32]);}(p,indices)
        },
    }
}
#[derive(Clone,Debug,PartialEq)]pub struct AssetLod{pub node:usize,pub levels:Vec<usize>,pub distances:Vec<f32>}
impl AssetLod{/// Distance selects exactly one node. Callers suppress lower LOD
    /// nodes from ordinary traversal and draw the selected geometry once.
    pub fn select(&self,distance:f32)->Result<usize,String>{if !distance.is_finite()||distance<0.{return Err("invalid LOD distance".into());}let n=self.distances.partition_point(|d|distance>=*d);Ok(if n==0{self.node}else{self.levels[n-1]})}}
fn array(v:&JsonValue)->Result<&[JsonValue],String>{match v{JsonValue::Array(v)=>Ok(v),_=>Err("metadata array".into())}}
fn number(v:&JsonValue)->Result<f32,String>{let n=match v{JsonValue::F64(v)=>*v,JsonValue::U64(v)=>*v as f64,JsonValue::I64(v)=>*v as f64,_=>return Err("metadata number".into())};let n=n as f32;if !n.is_finite(){Err("non-finite metadata number".into())}else{Ok(n)}}
fn index(v:&JsonValue)->Result<usize,String>{match v{JsonValue::U64(v)=>usize::try_from(*v).map_err(|_|"index overflow".into()),JsonValue::I64(v)if *v>=0=>usize::try_from(*v).map_err(|_|"index overflow".into()),JsonValue::F64(v)if v.is_finite()&&*v>=0.&&v.fract()==0.&&*v<=u32::MAX as f64=>Ok(*v as usize),_=>Err("metadata index".into())}}
fn need<'a>(v:&'a JsonValue,k:&str)->Result<&'a JsonValue,String>{v.key(k).ok_or_else(||format!("missing metadata {k}"))}
fn vector(v:&JsonValue)->Result<[f32;3],String>{let values=array(v)?;if values.len()!=3{return Err("metadata vector length".into());}let out=[number(&values[0])?,number(&values[1])?,number(&values[2])?];if out.iter().any(|v|v.abs()>1e6){return Err("collision coordinate budget".into());}Ok(out)}
pub fn parse_asset_collisions(document:&GltfDocument)->Result<Vec<AssetCollision>,String>{let mut out=Vec::new();let mut bytes=0usize;if document.nodes_slice().len()>4096{return Err("collision node budget".into());}for(node,n)in document.nodes_slice().iter().enumerate(){let Some(v)=n.extras.as_ref().and_then(|e|e.key("MAKEPAD_collision"))else{continue};let kind=need(v,"kind")?.string().ok_or("collision kind")?;let shape=match kind.as_str(){
    "box"=>{let min=vector(need(v,"min")?)?;let max=vector(need(v,"max")?)?;if (0..3).any(|i|min[i]>=max[i]){return Err("collision box extents".into());}AssetCollisionShape::Box{min,max}},
    "sphere"=>{let center=vector(need(v,"center")?)?;let radius=number(need(v,"radius")?)?;if radius<=0.||radius>1e6{return Err("collision sphere radius".into());}AssetCollisionShape::Sphere{center,radius}},
    "mesh"=>{let positions=array(need(v,"positions")?)?;let indices=array(need(v,"indices")?)?;if positions.len()<3||positions.len()>100_000||indices.is_empty()||indices.len()%3!=0||indices.len()>600_000{return Err("collision mesh count budget".into());}bytes=bytes.saturating_add(positions.len()*12+indices.len()*4);if bytes>16*1024*1024{return Err("collision mesh bytes exceed16MiB".into());}let positions=positions.iter().map(vector).collect::<Result<Vec<_>,_>>()?;let mut converted=Vec::with_capacity(indices.len());for i in indices{let i=index(i)?;if i>=positions.len(){return Err("collision mesh index outside positions".into());}converted.push(i as u32);}for triangle in converted.chunks_exact(3){let p=triangle.iter().map(|i|positions[*i as usize]).collect::<Vec<_>>();let a=std::array::from_fn::<_,3,_>(|i|p[1][i] as f64-p[0][i] as f64);let b=std::array::from_fn::<_,3,_>(|i|p[2][i] as f64-p[0][i] as f64);let cross=[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]];if cross.iter().map(|v|v*v).sum::<f64>()<1e-24{return Err("degenerate collision triangle".into());}}AssetCollisionShape::Mesh{positions,indices:converted}},
    _=>return Err("unsupported collision proxy kind".into()),
};out.push(AssetCollision{node,shape});if out.len()>64{return Err("collision proxy count exceeds64".into());}}Ok(out)}
pub fn parse_asset_lods(document:&GltfDocument)->Result<Vec<AssetLod>,String>{let nodes=document.nodes_slice();if nodes.len()>4096{return Err("LOD node budget".into());}let roots=document.scenes.as_ref().into_iter().flatten().flat_map(|s|s.nodes.as_deref().unwrap_or(&[])).copied().collect::<std::collections::BTreeSet<_>>();let children=nodes.iter().flat_map(|n|n.children.as_deref().unwrap_or(&[])).copied().collect::<std::collections::BTreeSet<_>>();let mut owned=std::collections::BTreeSet::new();let mut out=Vec::new();for(node,n)in nodes.iter().enumerate(){let Some(extension)=n.extensions.as_ref().and_then(|v|v.key("MSFT_lod"))else{continue};let ids=array(need(extension,"ids")?)?;let distances=array(n.extras.as_ref().and_then(|v|v.key("MAKEPAD_lod_distances")).ok_or("LOD distances required")?)?;if ids.is_empty()||ids.len()>8||ids.len()!=distances.len()||n.mesh.is_none(){return Err("LOD levels/distances/base geometry".into());}let mut levels=Vec::new();let mut thresholds=Vec::new();let mut previous=0.;for(i,d)in ids.iter().zip(distances){let i=index(i)?;let target=nodes.get(i).ok_or("LOD target node")?;if i==node||!owned.insert(i)||target.mesh.is_none()||target.extensions.as_ref().is_some_and(|e|e.key("MSFT_lod").is_some())||roots.contains(&i)||children.contains(&i){return Err("LOD target must be unique hidden geometry without nested LODs".into());}let d=number(d)?;if d<=previous||d>1e6{return Err("LOD distances must strictly increase".into());}previous=d;levels.push(i);thresholds.push(d);}out.push(AssetLod{node,levels,distances:thresholds});if out.len()>64{return Err("LOD groups exceed64".into());}}Ok(out)}
#[cfg(test)]mod tests{use super::*;fn doc(nodes:&str)->GltfDocument{makepad_gltf::parse_gltf_json(&format!(r#"{{"asset":{{"version":"2.0"}},"nodes":{nodes},"meshes":[{{"primitives":[]}},{{"primitives":[]}},{{"primitives":[]}}]}}"#)).unwrap()}
#[test]fn collision_shapes_and_lod_thresholds_are_validated(){let d=doc(r#"[{"mesh":0,"extensions":{"MSFT_lod":{"ids":[1,2]}},"extras":{"MAKEPAD_lod_distances":[10,30],"MAKEPAD_collision":{"kind":"box","min":[-1,-1,-1],"max":[1,1,1]}}},{"mesh":1},{"mesh":2}]"#);let lod=parse_asset_lods(&d).unwrap();assert_eq!(lod[0].select(9.).unwrap(),0);assert_eq!(lod[0].select(10.).unwrap(),1);assert_eq!(lod[0].select(30.).unwrap(),2);assert_eq!(parse_asset_collisions(&d).unwrap().len(),1);}
#[test]fn prepared_collision_keeps_owner_transform_and_bounded_sphere_geometry(){
    let d=doc(r#"[{"translation":[3,4,5],"scale":[-2,1,1],"extras":{"MAKEPAD_collision":{"kind":"box","min":[-1,-1,-1],"max":[1,1,1]}}},{"extras":{"MAKEPAD_collision":{"kind":"sphere","center":[0,0,0],"radius":2}}}]"#);
    let rows=prepare_asset_collisions(&d).unwrap();assert_eq!(rows.len(),2);
    assert_eq!(rows[0].mesh.min,makepad_draw::makepad_math::vec3f(1.,3.,4.));assert_eq!(rows[0].mesh.max,makepad_draw::makepad_math::vec3f(5.,5.,6.));assert_eq!(rows[0].mesh.indices.len()/3,12);
    assert_eq!(rows[1].mesh.indices.len()/3,3968);assert!(rows[1].mesh.positions.iter().all(|v|(v[0]*v[0]+v[1]*v[1]+v[2]*v[2]-4.).abs()<1e-4));
}
#[test]fn invalid_lod_and_collision_inputs_fail_without_partial_results(){for nodes in [r#"[{"mesh":0,"extensions":{"MSFT_lod":{"ids":[0]}},"extras":{"MAKEPAD_lod_distances":[10]}}]"#,r#"[{"extras":{"MAKEPAD_collision":{"kind":"sphere","center":[0,0,0],"radius":-1}}}]"#,r#"[{"extras":{"MAKEPAD_collision":{"kind":"mesh","positions":[[0,0,0],[1,0,0],[2,0,0]],"indices":[0,1,2]}}}]"#]{let d=doc(nodes);assert!(parse_asset_lods(&d).is_err()||parse_asset_collisions(&d).is_err());}}
}
