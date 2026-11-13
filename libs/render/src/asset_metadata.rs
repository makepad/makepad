//! Collision proxies and MSFT_lod geometry choices in owner-local space.
use makepad_gltf::{GltfDocument,JsonValue,LoadedGltf};
use makepad_draw::first_report;
#[derive(Clone,Debug,PartialEq)]pub enum AssetCollisionShape{Box{min:[f32;3],max:[f32;3]},Sphere{center:[f32;3],radius:f32},Mesh{positions:Vec<[f32;3]>,indices:Vec<u32>}}
#[derive(Clone,Debug,PartialEq)]pub struct AssetCollision{pub node:usize,pub shape:AssetCollisionShape}
/// Prepared once on the asset worker. `shape` stays node-local, while `mesh`
/// already includes `rest_transform` and is in model coordinates. The rest
/// transform is metadata; consumers must not apply it to the mesh again.
#[derive(Clone,Debug)]pub struct PreparedAssetCollision{pub node:usize,pub shape:AssetCollisionShape,pub rest_transform:makepad_draw::makepad_math::Mat4f,pub mesh:std::sync::Arc<makepad_scene::mesh::PreparedMesh>}
pub fn prepare_asset_collisions(loaded:&LoadedGltf)->Result<std::sync::Arc<Vec<PreparedAssetCollision>>,String>{
    let shapes=parse_asset_collisions(loaded)?;
    if shapes.is_empty(){return Ok(Default::default());}
    let worlds=crate::asset_sockets::node_rest_transforms(&loaded.document)?;
    let mut out=Vec::new();
    for collision in shapes {
        let rest_transform=worlds[collision.node];
        let m=&rest_transform.v;let det=m[0]*(m[5]*m[10]-m[6]*m[9])-m[4]*(m[1]*m[10]-m[2]*m[9])+m[8]*(m[1]*m[6]-m[2]*m[5]);
        let (positions,indices)=collision_triangles(&collision.shape);
        // An owner scaled to nothing leaves nothing to collide with.
        if !det.is_finite()||det.abs()<1e-10{
            if first_report(mesh_key(&positions,&indices)^SINGULAR_REPORT){makepad_draw::log!("authored collision of node {} left out: its owner transform is singular",collision.node);}
            continue;
        }
        let triangles=indices.len()/3;
        // A mesh over the cooking bound is cooked as several pieces within it.
        let pieces=if indices.len()>MAX_COOKED_TRIANGLES*3{
            let pieces=split_mesh(&positions,&indices);
            if first_report(mesh_key(&positions,&indices)){makepad_draw::log!("authored collision mesh of {triangles} triangles (node {}) cooked as {} pieces of at most {MAX_COOKED_TRIANGLES}",collision.node,pieces.len());}
            pieces.into_iter().map(|(positions,indices)|(AssetCollisionShape::Mesh{positions:positions.clone(),indices:indices.clone()},positions,indices)).collect()
        }else{vec![(collision.shape,positions,indices)]};
        let (mut skipped,mut skipped_key)=(0,0u64);
        for (shape,mut positions,mut indices) in pieces {
            for p in &mut positions {let [x,y,z]=*p;*p=[m[0]*x+m[4]*y+m[8]*z+m[12],m[1]*x+m[5]*y+m[9]*z+m[13],m[2]*x+m[6]*y+m[10]*z+m[14]];}
            if det<0. {for triangle in indices.chunks_exact_mut(3){triangle.swap(1,2);}}
            // A zero-area triangle has no surface: it is left out, the rest collides.
            let before=indices.len()/3;drop_degenerate(&positions,&mut indices);
            if indices.len()/3<before{skipped+=before-indices.len()/3;skipped_key^=mesh_key(&positions,&indices);}
            if indices.is_empty(){continue;}
            let mesh=prepare_mesh(positions,indices)?;
            out.push(PreparedAssetCollision{node:collision.node,shape,rest_transform,mesh});
        }
        if skipped>0&&first_report(skipped_key^DEGENERATE_REPORT){
            makepad_draw::log!("authored collision mesh of {triangles} triangles (node {}): {skipped} degenerate triangles left out{}",collision.node,if skipped==triangles{", nothing to collide with"}else{""});
        }
    }
    Ok(std::sync::Arc::new(out))
}
/// Triangles one cooked collision piece holds. A larger authored mesh is split
/// into pieces of at most this many, never refused: each piece keeps its own
/// tight bounds for the broadphase.
pub const MAX_COOKED_TRIANGLES:usize=8192;
// A model prepares its collision again on each install, so each report is
// keyed by the mesh it is about and made once per process (first_report).
const DEGENERATE_REPORT:u64=0x9e37_79b9_7f4a_7c15;
const SINGULAR_REPORT:u64=0xc2b2_ae3d_27d4_eb4f;
fn mesh_key(positions:&[[f32;3]],indices:&[u32])->u64{
    use std::hash::{Hash,Hasher};
    let mut hasher=std::collections::hash_map::DefaultHasher::new();indices.hash(&mut hasher);
    for p in positions{for v in p{v.to_bits().hash(&mut hasher);}}
    hasher.finish()
}
/// Keeps the triangles with area: a cross product of (near) zero length has no
/// surface to collide with and no normal to push along.
fn drop_degenerate(positions:&[[f32;3]],indices:&mut Vec<u32>){
    let mut kept=0;
    for t in 0..indices.len()/3{
        let [p,q,r]=[0,1,2].map(|c|positions[indices[t*3+c] as usize]);
        let a=std::array::from_fn::<_,3,_>(|i|q[i] as f64-p[i] as f64);
        let b=std::array::from_fn::<_,3,_>(|i|r[i] as f64-p[i] as f64);
        let cross=[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]];
        if cross.iter().map(|v|v*v).sum::<f64>()>=1e-24{indices.copy_within(t*3..t*3+3,kept*3);kept+=1;}
    }
    indices.truncate(kept*3);
}
/// Spatially compact pieces of at most [`MAX_COOKED_TRIANGLES`]: each split
/// halves a piece at the median triangle centroid along its longest axis.
/// Every triangle lands in exactly one piece, so the surface is unchanged.
fn split_mesh(positions:&[[f32;3]],indices:&[u32])->Vec<(Vec<[f32;3]>,Vec<u32>)>{
    let centroid=|t:usize|->[f32;3]{std::array::from_fn(|a|(0..3).map(|c|positions[indices[t*3+c] as usize][a]).sum::<f32>()/3.)};
    let centroids=(0..indices.len()/3).map(centroid).collect::<Vec<_>>();
    let mut pending=vec![(0..centroids.len()).collect::<Vec<usize>>()];let mut out=Vec::new();
    while let Some(mut triangles)=pending.pop(){
        if triangles.len()<=MAX_COOKED_TRIANGLES{
            let mut remap=std::collections::HashMap::new();let mut piece_positions=Vec::new();
            let piece_indices=triangles.iter().flat_map(|&t|indices[t*3..t*3+3].iter()).map(|&i|*remap.entry(i).or_insert_with(||{piece_positions.push(positions[i as usize]);(piece_positions.len()-1) as u32})).collect();
            out.push((piece_positions,piece_indices));continue;
        }
        let (mut lo,mut hi)=([f32::INFINITY;3],[f32::NEG_INFINITY;3]);
        for &t in &triangles{for a in 0..3{lo[a]=lo[a].min(centroids[t][a]);hi[a]=hi[a].max(centroids[t][a]);}}
        let axis=(0..3).max_by(|&a,&b|(hi[a]-lo[a]).total_cmp(&(hi[b]-lo[b]))).unwrap();
        let mid=triangles.len()/2;
        triangles.select_nth_unstable_by(mid,|&a,&b|centroids[a][axis].total_cmp(&centroids[b][axis]).then(a.cmp(&b)));
        let upper=triangles.split_off(mid);pending.push(upper);pending.push(triangles);
    }
    out
}
/// One cooked piece in model space: within [`MAX_COOKED_TRIANGLES`] (larger
/// meshes are split first) and free of zero-area triangles (left out first).
/// Welding and acceleration validation remain the physics producer's job.
fn prepare_mesh(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> Result<std::sync::Arc<makepad_scene::mesh::PreparedMesh>, String> {
    use makepad_draw::makepad_math::vec3f;
    if positions.len() < 3 || indices.len() < 3
        || indices.len() % 3 != 0 || indices.len() > MAX_COOKED_TRIANGLES * 3 {
        return Err("authored collision piece shape".into());
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
pub const STALE_COLLISION:&str="collision mesh positions must be an accessor: rebuild or republish this model";
pub fn collision_format_stale(document:&GltfDocument)->bool{document.nodes_slice().iter().any(|n|n.extras.as_ref().and_then(|e|e.key("MAKEPAD_collision")).is_some_and(|v|
    matches!(v.key("kind"),Some(JsonValue::String(kind))if kind=="mesh")&&["positions","indices"].iter().any(|k|v.key(k).is_some_and(|v|index(v).is_err()))))}
pub fn parse_asset_collisions(loaded:&LoadedGltf)->Result<Vec<AssetCollision>,String>{let mut out=Vec::new();for(node,n)in loaded.document.nodes_slice().iter().enumerate(){let Some(v)=n.extras.as_ref().and_then(|e|e.key("MAKEPAD_collision"))else{continue};let kind=need(v,"kind")?.string().ok_or("collision kind")?;let shape=match kind.as_str(){
    "box"=>{let min=vector(need(v,"min")?)?;let max=vector(need(v,"max")?)?;if (0..3).any(|i|min[i]>=max[i]){return Err("collision box extents".into());}AssetCollisionShape::Box{min,max}},
    "sphere"=>{let center=vector(need(v,"center")?)?;let radius=number(need(v,"radius")?)?;if radius<=0.||radius>1e6{return Err("collision sphere radius".into());}AssetCollisionShape::Sphere{center,radius}},
    "mesh"=>{let p=index(need(v,"positions")?).map_err(|_|STALE_COLLISION.to_string())?;let i=index(need(v,"indices")?).map_err(|_|STALE_COLLISION.to_string())?;
        let positions=makepad_gltf::read_accessor_f32x3(loaded,p).map_err(|e|format!("collision mesh node {node} positions: {e}"))?;
        let indices=makepad_gltf::read_accessor_indices_u32(loaded,i).map_err(|e|format!("collision mesh node {node} indices: {e}"))?;
        if positions.len()<3||indices.is_empty()||indices.len()%3!=0{return Err("collision mesh shape".into());}
        for p in &positions{if p.iter().any(|v|!v.is_finite()){return Err("non-finite metadata number".into());}if p.iter().any(|v|v.abs()>1e6){return Err("collision coordinate budget".into());}}
        if indices.iter().any(|&i|i as usize>=positions.len()){return Err("collision mesh index outside positions".into());}AssetCollisionShape::Mesh{positions,indices}},
    _=>return Err("unsupported collision proxy kind".into()),
};out.push(AssetCollision{node,shape});}Ok(out)}
pub fn parse_asset_lods(document:&GltfDocument)->Result<Vec<AssetLod>,String>{let nodes=document.nodes_slice();let roots=document.scenes.as_ref().into_iter().flatten().flat_map(|s|s.nodes.as_deref().unwrap_or(&[])).copied().collect::<std::collections::BTreeSet<_>>();let children=nodes.iter().flat_map(|n|n.children.as_deref().unwrap_or(&[])).copied().collect::<std::collections::BTreeSet<_>>();let mut owned=std::collections::BTreeSet::new();let mut out=Vec::new();for(node,n)in nodes.iter().enumerate(){let Some(extension)=n.extensions.as_ref().and_then(|v|v.key("MSFT_lod"))else{continue};let ids=array(need(extension,"ids")?)?;let distances=array(n.extras.as_ref().and_then(|v|v.key("MAKEPAD_lod_distances")).ok_or("LOD distances required")?)?;if ids.is_empty()||ids.len()!=distances.len()||n.mesh.is_none(){return Err("LOD levels/distances/base geometry".into());}let mut levels=Vec::new();let mut thresholds=Vec::new();let mut previous=0.;for(i,d)in ids.iter().zip(distances){let i=index(i)?;let target=nodes.get(i).ok_or("LOD target node")?;if i==node||!owned.insert(i)||target.extensions.as_ref().is_some_and(|e|e.key("MSFT_lod").is_some())||roots.contains(&i)||children.contains(&i){return Err("LOD target must be unique hidden geometry without nested LODs".into());}let d=number(d)?;if d<=previous||d>1e6{return Err("LOD distances must strictly increase".into());}previous=d;levels.push(i);thresholds.push(d);}out.push(AssetLod{node,levels,distances:thresholds});}Ok(out)}
#[cfg(test)]mod tests{use super::*;fn doc(nodes:&str)->LoadedGltf{let document=makepad_gltf::parse_gltf_json(&format!(r#"{{"asset":{{"version":"2.0"}},"nodes":{nodes},"meshes":[{{"primitives":[]}},{{"primitives":[]}},{{"primitives":[]}}]}}"#)).unwrap();LoadedGltf{kind:makepad_gltf::GltfContainerKind::Glb,document,buffers:Vec::new(),source_path:None,base_dir:None}}
fn with_mesh(nodes:&str,node:usize,positions:&[[f32;3]],indices:&[u32])->LoadedGltf{
    let mut bin=Vec::with_capacity(positions.len()*12+indices.len()*4);
    for p in positions{for v in p{bin.extend_from_slice(&v.to_le_bytes());}}let offset=bin.len();
    for i in indices{bin.extend_from_slice(&i.to_le_bytes());}
    let mut document=makepad_gltf::parse_gltf_json(&format!(r#"{{"asset":{{"version":"2.0"}},"nodes":{nodes},"buffers":[{{"byteLength":{}}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{offset}}},{{"buffer":0,"byteOffset":{offset},"byteLength":{}}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":{},"type":"VEC3"}},{{"bufferView":1,"componentType":5125,"count":{},"type":"SCALAR"}}]}}"#,bin.len(),indices.len()*4,positions.len(),indices.len())).unwrap();
    let mesh=JsonValue::Object([("kind".into(),JsonValue::String("mesh".into())),("positions".into(),JsonValue::U64(0)),("indices".into(),JsonValue::U64(1))].into_iter().collect());
    document.nodes.as_mut().unwrap()[node].extras=Some(JsonValue::Object([("MAKEPAD_collision".into(),mesh)].into_iter().collect()));
    LoadedGltf{kind:makepad_gltf::GltfContainerKind::Glb,document,buffers:vec![bin],source_path:None,base_dir:None}
}
#[test]fn old_json_collision_mesh_is_stale(){
    let d=doc(r#"[{"extras":{"MAKEPAD_collision":{"kind":"mesh","positions":[[0,0,0],[1,0,0],[0,0,1]],"indices":[0,1,2]}}}]"#);
    assert!(collision_format_stale(&d.document));assert_eq!(parse_asset_collisions(&d).unwrap_err(),STALE_COLLISION);
    let d=with_mesh("[{}]",0,&[[0.,0.,0.],[1.,0.,0.],[0.,0.,1.]],&[0,1,2]);assert!(!collision_format_stale(&d.document));
}
#[test]fn nine_lod_ids_parse_and_select_the_last_level(){
    let d=doc(&format!(r#"[{{"mesh":0,"extensions":{{"MSFT_lod":{{"ids":[1,2,3,4,5,6,7,8,9]}}}},"extras":{{"MAKEPAD_lod_distances":[1,2,3,4,5,6,7,8,9]}}}}{}]"#,",{\"mesh\":0}".repeat(9)));
    let lods=parse_asset_lods(&d.document).unwrap();
    assert_eq!(lods[0].levels,(1..=9).collect::<Vec<_>>());assert_eq!(lods[0].select(9.).unwrap(),9);
}
#[test]fn collision_shapes_and_lod_thresholds_are_validated(){let d=doc(r#"[{"mesh":0,"extensions":{"MSFT_lod":{"ids":[1,2]}},"extras":{"MAKEPAD_lod_distances":[10,30],"MAKEPAD_collision":{"kind":"box","min":[-1,-1,-1],"max":[1,1,1]}}},{"mesh":1},{"mesh":2}]"#);let lod=parse_asset_lods(&d.document).unwrap();assert_eq!(lod[0].select(9.).unwrap(),0);assert_eq!(lod[0].select(10.).unwrap(),1);assert_eq!(lod[0].select(30.).unwrap(),2);assert_eq!(parse_asset_collisions(&d).unwrap().len(),1);
    let nodes=(0..213).map(|i|format!(r#"{{"mesh":0,"extensions":{{"MSFT_lod":{{"ids":[{}]}}}},"extras":{{"MAKEPAD_lod_distances":[30]}}}},{{}}"#,2*i+1)).collect::<Vec<_>>().join(",");
    let d=doc(&format!("[{nodes}]"));let lods=parse_asset_lods(&d.document).unwrap();assert_eq!(lods.len(),213);assert_eq!(d.document.nodes_slice()[lods[212].select(30.).unwrap()].mesh,None);
}
#[test]fn prepared_collision_keeps_owner_transform_and_bounded_sphere_geometry(){
    let d=doc(r#"[{"translation":[3,4,5],"scale":[-2,1,1],"extras":{"MAKEPAD_collision":{"kind":"box","min":[-1,-1,-1],"max":[1,1,1]}}},{"extras":{"MAKEPAD_collision":{"kind":"sphere","center":[0,0,0],"radius":2}}}]"#);
    let rows=prepare_asset_collisions(&d).unwrap();assert_eq!(rows.len(),2);
    assert_eq!(rows[0].mesh.min,makepad_draw::makepad_math::vec3f(1.,3.,4.));assert_eq!(rows[0].mesh.max,makepad_draw::makepad_math::vec3f(5.,5.,6.));assert_eq!(rows[0].mesh.indices.len()/3,12);
    assert_eq!(rows[1].mesh.indices.len()/3,3968);assert!(rows[1].mesh.positions.iter().all(|v|(v[0]*v[0]+v[1]*v[1]+v[2]*v[2]-4.).abs()<1e-4));
}
#[test]fn invalid_lod_and_collision_inputs_fail_without_partial_results(){for nodes in [r#"[{"mesh":0,"extensions":{"MSFT_lod":{"ids":[0]}},"extras":{"MAKEPAD_lod_distances":[10]}}]"#,r#"[{"extras":{"MAKEPAD_collision":{"kind":"sphere","center":[0,0,0],"radius":-1}}}]"#]{let d=doc(nodes);assert!(parse_asset_lods(&d.document).is_err()||parse_asset_collisions(&d).is_err());}}
/// Former refusals degrade: each input is just over and far over what used to
/// be refused, and every one prepares.
#[test]fn collision_inputs_over_former_limits_prepare(){
    fn grid(n:u32)->(Vec<[f32;3]>,Vec<u32>){
        let positions=(0..n*n).map(|k|[(k%n) as f32,0.,(k/n) as f32]).collect();
        let indices=(0..n-1).flat_map(|j|(0..n-1).flat_map(move|i|{let a=j*n+i;[a,a+n,a+1,a+1,a+n,a+n+1]})).collect();
        (positions,indices)
    }
    let triangles=|rows:&[PreparedAssetCollision]|rows.iter().map(|r|r.mesh.indices.len()/3).sum::<usize>();
    // Positions and indices just over the former 100k / 600k: cooked as pieces within the bound.
    let (positions,indices)=grid(318);assert!(positions.len()>100_000&&indices.len()>600_000);
    let d=with_mesh("[{}]",0,&positions,&indices);let rows=prepare_asset_collisions(&d).unwrap();
    assert_eq!(triangles(&rows),indices.len()/3,"every triangle of the just-over mesh is in one piece");
    assert!(rows.len()>1&&rows.iter().all(|r|r.mesh.indices.len()<=MAX_COOKED_TRIANGLES*3));
    // 4x the positions, 5x the indices, and just over the former 16 MiB: parsed whole.
    let mut positions=vec![[0.,0.,0.],[1.,0.,0.],[0.,0.,1.]];positions.extend((3..400_000u32).map(|k|[(k%1000) as f32,1.,(k/1000) as f32]));
    let indices=[0u32,1,2].repeat(1_000_000);assert!(positions.len()*12+indices.len()*4>16*1024*1024);
    let d=with_mesh("[{}]",0,&positions,&indices);let rows=parse_asset_collisions(&d).unwrap();
    assert!(matches!(&rows[0].shape,AssetCollisionShape::Mesh{positions:p,indices:i}if p.len()==400_000&&i.len()==3_000_000),"the far-over mesh parses whole");
    drop((d,rows,positions,indices));
    // Proxy counts past the former 64, and node counts past the former 4096.
    for proxies in [65usize,640]{
        let d=doc(&format!("[{}]",vec![r#"{"extras":{"MAKEPAD_collision":{"kind":"box","min":[-1,-1,-1],"max":[1,1,1]}}}"#;proxies].join(",")));
        assert_eq!(prepare_asset_collisions(&d).unwrap().len(),proxies,"{proxies} proxies all prepare");
    }
    for nodes in [4097usize,40_960]{
        let (positions,indices)=grid(3);let d=with_mesh(&format!("[{}]",vec!["{}";nodes].join(",")),nodes-1,&positions,&indices);
        let rows=prepare_asset_collisions(&d).unwrap();assert_eq!((rows.len(),rows[0].node),(1,nodes-1),"a collider on node {nodes} prepares");
    }
    // Zero-area triangles are left out and the rest collides: one of 201, 90 of 98, and all of them.
    let (mut positions,grid_indices)=grid(11);let line=positions.len() as u32;positions.extend([[0.,5.,0.],[1.,5.,0.],[2.,5.,0.]]);
    for (kept,degenerate) in [(grid_indices.clone(),1usize),(grid_indices[..24].to_vec(),90),(Vec::new(),90)]{
        let mut indices=kept.clone();for _ in 0..degenerate{indices.extend([line,line+1,line+2]);}
        let d=with_mesh("[{}]",0,&positions,&indices);let rows=prepare_asset_collisions(&d).unwrap();
        assert_eq!(triangles(&rows),kept.len()/3,"{degenerate} degenerate of {} left out",indices.len()/3);
    }
    // An owner scaled to nothing leaves its collider out instead of failing the model.
    let d=doc(r#"[{"scale":[0,0,0],"extras":{"MAKEPAD_collision":{"kind":"box","min":[-1,-1,-1],"max":[1,1,1]}}},{"extras":{"MAKEPAD_collision":{"kind":"box","min":[-1,-1,-1],"max":[1,1,1]}}}]"#);
    let rows=prepare_asset_collisions(&d).unwrap();assert_eq!((rows.len(),rows[0].node),(1,1));
}
}
