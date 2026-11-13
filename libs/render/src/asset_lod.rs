//! Worker-side LOD variants. Each variant retains the original node graph,
//! animation targets and binary accessors; only active mesh references change.
use crate::asset_metadata::{parse_asset_lods,AssetLod};
use makepad_draw::makepad_math::{Mat4f,Vec3f};
use std::ops::Range;

pub struct AssetLodPlan<'a>{
    json:&'a[u8],
    mesh_spans:Vec<MeshSpan>,
    tail:&'a[u8],
    groups:Vec<AssetLod>,
    meshes:Vec<Option<usize>>,
    distances:Vec<f32>,
}
impl<'a> AssetLodPlan<'a>{
    pub fn parse(bytes:&'a[u8],byte_limit:usize)->Result<Option<Self>,String>{
        if bytes.len()>byte_limit{return Err("LOD source byte budget exceeded".into());}
        let parsed=makepad_gltf::parse_glb_bytes(bytes).map_err(|e|format!("LOD GLB: {e:?}"))?;
        let groups=parse_asset_lods(&parsed.document)?;if groups.is_empty(){return Ok(None);}
        let (json,_)=crate::asset_morph::chunks(bytes)?;
        if json.get("scenes").is_none(){return Err("LOD variants require an explicit default scene".into());}
        let active=crate::asset_morph::active_nodes(&json)?;let nodes=parsed.document.nodes_slice();

        for group in &groups{
            if !active[group.node]{return Err("LOD owner is outside the default scene".into());}
            let base=&nodes[group.node];
            for &level in &group.levels{let target=&nodes[level];
                if active[level]||target.skin!=base.skin||target.matrix!=base.matrix||target.translation!=base.translation||target.rotation!=base.rotation||target.scale!=base.scale||target.weights!=base.weights||!target.children.as_deref().unwrap_or(&[]).is_empty(){
                    return Err("LOD targets must be hidden leaf geometry with the owner's skin, local transform and weights".into());
                }
            }
        }
        let mut distances=groups.iter().flat_map(|group|group.distances.iter().copied()).collect::<Vec<_>>();
        distances.sort_by(f32::total_cmp);distances.dedup();
        let meshes=nodes.iter().map(|n|n.mesh).collect();
        let json_size=u32::from_le_bytes(bytes[12..16].try_into().unwrap())as usize;
        let json=&bytes[20..20+json_size];let mesh_spans=mesh_spans(json)?;
        if mesh_spans.len()!=nodes.len(){return Err("LOD node span count".into());}
        let tail=bytes.get(20+json_size..).ok_or("LOD JSON chunk length")?;
        Ok(Some(Self{json,mesh_spans,tail,groups,meshes,distances}))
    }
    pub fn distances(&self)->&[f32]{&self.distances}
    /// `level` indexes the lower variants; the original bytes are level zero.
    pub fn variant_glb(&mut self,level:usize,byte_limit:usize)->Result<Vec<u8>,String>{
        let distance=*self.distances.get(level).ok_or("LOD variant index")?;
        let mut changes=Vec::new();
        for group in &self.groups{
            let selected=group.select(distance)?;let span=&self.mesh_spans[group.node];if span.member.is_empty(){return Err("LOD owner mesh span missing".into());}
            changes.push(match self.meshes[selected]{Some(mesh)=>(span.value.clone(),mesh.to_string()),None=>(span.member.clone(),String::new())});
        }
        changes.sort_by_key(|(span,_)|span.start);
        let estimate=self.json.len().saturating_add(changes.len().saturating_mul(20)).saturating_add(self.tail.len()).saturating_add(23);if estimate>byte_limit{return Err("LOD variant byte budget exceeded".into());}
        let mut json=Vec::with_capacity(self.json.len().saturating_add(changes.len().saturating_mul(20)));let mut cursor=0;
        for(span,value)in changes{json.extend_from_slice(&self.json[cursor..span.start]);json.extend_from_slice(value.as_bytes());cursor=span.end;}json.extend_from_slice(&self.json[cursor..]);
        while json.len()%4!=0{json.push(b' ');}
        let size=20usize.checked_add(json.len()).and_then(|n|n.checked_add(self.tail.len())).ok_or("LOD GLB size overflow")?;
        if size>byte_limit||size>u32::MAX as usize{return Err("LOD variant byte budget exceeded".into());}
        let mut out=Vec::with_capacity(size);out.extend_from_slice(b"glTF");out.extend_from_slice(&2u32.to_le_bytes());out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&(json.len()as u32).to_le_bytes());out.extend_from_slice(b"JSON");out.extend_from_slice(&json);out.extend_from_slice(self.tail);Ok(out)
    }
}

/// Camera distance in world units, measured from the asset instance origin.
/// Invalid transforms fall back to the highest detail and cannot index a LOD.
pub fn instance_distance(transform:&Mat4f,eye:Vec3f)->f32{
    let distance=((transform.v[12]as f64-eye.x as f64).powi(2)+(transform.v[13]as f64-eye.y as f64).powi(2)+(transform.v[14]as f64-eye.z as f64).powi(2)).sqrt();
    if distance.is_finite(){distance.min(f32::MAX as f64)as f32}else{0.}
}
pub fn selected_level(distances:&[f32],distance:f32)->usize{
    if distance.is_finite()&&distance>=0.{distances.partition_point(|d|*d<=distance)}else{0}
}
pub fn validate_distances(distances:impl IntoIterator<Item=f32>)->Result<(),String>{
    let mut previous=0.;for distance in distances{if !distance.is_finite()||distance<=previous{return Err("LOD distances must be finite, positive and increasing".into());}previous=distance;}Ok(())
}

// Locate mesh value and whole-member spans (including a separating comma). Copy every other original JSON byte,
// including vendor metadata, large integers, Unicode and animation tracks.
struct MeshSpan{value:Range<usize>,member:Range<usize>}
struct Spans<'a>{bytes:&'a[u8],at:usize}
impl Spans<'_>{
    fn ws(&mut self){while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace){self.at+=1;}}
    fn token(&mut self,b:u8)->Result<(),String>{self.ws();if self.bytes.get(self.at)!=Some(&b){return Err("LOD JSON token".into());}self.at+=1;Ok(())}
    fn string(&mut self)->Result<Range<usize>,String>{self.ws();let start=self.at;self.token(b'"')?;while let Some(&b)=self.bytes.get(self.at){self.at+=1;if b==b'"'{return Ok(start..self.at);}if b==b'\\'{self.at+=1;}}Err("LOD JSON string".into())}
    fn value(&mut self,depth:usize)->Result<Range<usize>,String>{
        if depth>128{return Err("LOD JSON nesting exceeds128".into());}self.ws();let start=self.at;
        match self.bytes.get(self.at).copied(){
            Some(b'"')=>{self.string()?;},Some(b'{')=>{self.fields(depth+1)?;},
            Some(b'[')=>{self.at+=1;self.ws();while self.bytes.get(self.at)!=Some(&b']'){self.value(depth+1)?;self.ws();if self.bytes.get(self.at)==Some(&b','){self.at+=1;}else{break;}}self.token(b']')?;},
            Some(_)=>{while self.bytes.get(self.at).is_some_and(|b|!b.is_ascii_whitespace()&&!b",]}".contains(b)){self.at+=1;}if self.at==start{return Err("LOD JSON value".into());}},None=>return Err("LOD JSON end".into()),
        }Ok(start..self.at)
    }
    fn fields(&mut self,depth:usize)->Result<Vec<(String,Range<usize>,Range<usize>)>,String>{
        self.token(b'{')?;let mut fields=Vec::new();let mut names=std::collections::BTreeSet::new();let mut separator=None;self.ws();while self.bytes.get(self.at)!=Some(&b'}'){
            let key=self.string()?;let start=key.start;let parsed=crate::skin::JsonParser::parse(&self.bytes[key])?;let name=parsed.str().ok_or("LOD field name")?.to_owned();if !names.insert(name.clone()){return Err("duplicate LOD JSON field".into());}
            self.token(b':')?;let span=self.value(depth+1)?;self.ws();let comma=self.bytes.get(self.at)==Some(&b',');
            let member=if comma{start..self.at+1}else{separator.unwrap_or(start)..self.at};fields.push((name,span,member));
            if comma{separator=Some(self.at);self.at+=1;}else{break;}
        }self.token(b'}')?;Ok(fields)
    }
}
fn mesh_spans(json:&[u8])->Result<Vec<MeshSpan>,String>{
    let mut reader=Spans{bytes:json,at:0};let root=reader.fields(0)?;reader.at=root.iter().find(|(k,_,_)|k=="nodes").ok_or("LOD JSON nodes")?.1.start;reader.token(b'[')?;let mut out=Vec::new();reader.ws();
    while reader.bytes.get(reader.at)!=Some(&b']'){let fields=reader.fields(0)?;out.push(fields.into_iter().find(|(k,_,_)|k=="mesh").map(|(_,value,member)|MeshSpan{value,member}).unwrap_or(MeshSpan{value:0..0,member:0..0}));reader.ws();if reader.bytes.get(reader.at)==Some(&b','){reader.at+=1;}else{break;}}
    reader.token(b']')?;Ok(out)
}

/// Complete worker-prepared rest/material data for one lower character LOD.
pub struct PreparedSkinLod{
    pub distance:f32,
    pub rest:crate::skin::SkinRestGpu,
    pub materials:Vec<crate::skin::PreparedSkinPart>,
    pub texture:makepad_draw::ImageBuffer,
    pub morph:Option<crate::asset_morph::AssetMorph>,
}
impl PreparedSkinLod{pub fn upload_bytes(&self)->usize{
    (self.rest.vertices.len()+self.rest.indices.len()+self.texture.data.len())*4+self.rest.ao_pixels.len()+self.materials.iter().map(|p|p.upload_bytes()).sum::<usize>()+self.morph.as_ref().map_or(0,|m|m.pixels.len()*4)
}}

#[cfg(test)]
mod tests{
    use super::*;
    fn fixture(second:bool,bad_transform:bool,empty:bool)->Vec<u8>{
        let source=makepad_gltf::write_glb_mesh(&[[0.,0.,0.],[1.,0.,0.],[1.,1.,0.],[0.,1.,0.]],&[0,1,2,0,2,3]);
        let (_,bin)=crate::asset_morph::chunks(&source).unwrap();
        let mut nodes=format!(r#"{{"name":"owner\"\\雪","mesh":0,"extensions":{{"MSFT_lod":{{"ids":[1]}}}},"extras":{{"MAKEPAD_lod_distances":[10]}}}},{{"mesh":1{}}}"#,if bad_transform{",\"translation\":[1,0,0]"}else{""});
        if second{nodes.push_str(r#",{"mesh":0,"translation":[3,0,0],"extensions":{"MSFT_lod":{"ids":[3]}},"extras":{"MAKEPAD_lod_distances":[20]}},{"mesh":1,"translation":[3,0,0]}"#);}
        if empty{nodes=nodes.replace("\"mesh\":1", "\"extras\":{}");}
        let json=format!(r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[{}]}}],"nodes":[{}],"buffers":[{{"byteLength":{}}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":24}},{{"buffer":0,"byteOffset":24,"byteLength":48}},{{"buffer":0,"byteOffset":72,"byteLength":48}}],"accessors":[{{"bufferView":0,"componentType":5125,"count":6,"type":"SCALAR"}},{{"bufferView":1,"componentType":5126,"count":4,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}},{{"bufferView":2,"componentType":5126,"count":4,"type":"VEC3"}},{{"bufferView":0,"componentType":5125,"count":3,"type":"SCALAR"}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":1,"NORMAL":2}},"indices":0}}]}},{{"primitives":[{{"attributes":{{"POSITION":1,"NORMAL":2}},"indices":3}}]}}]}}"#,if second{"0,2"}else{"0"},nodes,bin.len());
        let mut json=json.into_bytes();while json.len()%4!=0{json.push(b' ');}
        let mut out=b"glTF".to_vec();out.extend_from_slice(&2u32.to_le_bytes());out.extend_from_slice(&((28+json.len()+bin.len())as u32).to_le_bytes());out.extend_from_slice(&(json.len()as u32).to_le_bytes());out.extend_from_slice(b"JSON");out.extend_from_slice(&json);out.extend_from_slice(&(bin.len()as u32).to_le_bytes());out.extend_from_slice(b"BIN\0");out.extend_from_slice(bin);out
    }
    #[test]
    fn distance_union_changes_only_active_geometry_and_preserves_binary_and_ids(){
        for empty in [false,true]{
            let source=fixture(true,false,empty);let original=makepad_gltf::parse_glb_bytes(&source).unwrap();let high=crate::StaticModel::parse_glb(&source).unwrap();assert_eq!(high.indices.len()/3,4);
            let mut plan=AssetLodPlan::parse(&source,1024*1024).unwrap().unwrap();assert_eq!(plan.distances(),&[10.,20.]);
            for(index,triangles)in if empty{[(0,2),(1,0)]}else{[(0,3),(1,2)]}{let bytes=plan.variant_glb(index,1024*1024).unwrap();let parsed=makepad_gltf::parse_glb_bytes(&bytes).unwrap();
                assert_eq!(parsed.bin_chunk,original.bin_chunk);assert_eq!(parsed.document.nodes_slice().len(),4);assert_eq!(parsed.document.nodes_slice()[0].name,original.document.nodes_slice()[0].name);
                let (json,_)=crate::asset_morph::chunks(&bytes).unwrap();assert_eq!(crate::asset_morph::active_nodes(&json).unwrap(),vec![true,false,true,false]);assert_eq!(crate::StaticModel::parse_glb(&bytes).unwrap().indices.len()/3,triangles);
            }
        }
    }
    #[test]
    fn empty_mesh_members_remove_the_key_value_and_correct_comma(){
        for node in [r#"{"mesh":0}"#,r#"{ "mesh" : 0, "name":"a" }"#,r#"{"name":"a", "mesh":0}"#,r#"{"name":"a","mesh":0,"extras":{"id":9007199254740993}}"#]{
            let json=format!(r#"{{"nodes":[{node}]}}"#);let spans=mesh_spans(json.as_bytes()).unwrap();let span=&spans[0].member;
            let edited=format!("{}{}",&json[..span.start],&json[span.end..]);
            let parsed=crate::skin::JsonParser::parse(edited.as_bytes()).unwrap();let owner=parsed.get("nodes").unwrap().idx(0).unwrap();assert!(owner.get("mesh").is_none());
            if node.contains("name"){assert_eq!(owner.get("name").unwrap().str(),Some("a"));}
            if node.contains("extras"){assert!(edited.contains("9007199254740993"),"unrelated bytes remain exact");}
        }
    }
    #[test]
    fn nine_authored_thresholds_and_all_group_levels_survive(){
        let first=r#"{"mesh":0,"extensions":{"MSFT_lod":{"ids":[1,2,3,4,5]}},"extras":{"MAKEPAD_lod_distances":[1,3,5,7,9]}}"#;
        let second=r#"{"mesh":0,"extensions":{"MSFT_lod":{"ids":[7,8,9,10,11]}},"extras":{"MAKEPAD_lod_distances":[2,4,6,8,9]}}"#;
        let mut json=format!(r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0,6]}}],"nodes":[{first}{},{second}{}],"meshes":[{{"primitives":[]}}]}}"#,",{\"mesh\":0}".repeat(5),",{\"mesh\":0}".repeat(5)).into_bytes();
        while json.len()%4!=0{json.push(b' ');}
        let mut glb=b"glTF".to_vec();glb.extend_from_slice(&2u32.to_le_bytes());glb.extend_from_slice(&((20+json.len())as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len()as u32).to_le_bytes());glb.extend_from_slice(b"JSON");glb.extend(json);
        let plan=AssetLodPlan::parse(&glb,usize::MAX).unwrap().unwrap();
        assert_eq!(plan.distances(),&[1.,2.,3.,4.,5.,6.,7.,8.,9.],"every distinct authored threshold survives");
        assert_eq!(plan.groups[0].levels,vec![1,2,3,4,5]);assert_eq!(plan.groups[0].distances,vec![1.,3.,5.,7.,9.]);
        assert_eq!(plan.groups[1].levels,vec![7,8,9,10,11]);assert_eq!(plan.groups[1].distances,vec![2.,4.,6.,8.,9.]);
    }
    #[test]
    fn unsupported_neighborhood_and_budget_are_explicit_errors(){
        assert!(AssetLodPlan::parse(&fixture(false,true,false),1024*1024).is_err());let source=fixture(false,false,false);assert!(AssetLodPlan::parse(&source,source.len()-1).is_err());let mut plan=AssetLodPlan::parse(&source,1024*1024).unwrap().unwrap();assert!(plan.variant_glb(1,1024*1024).is_err());assert!(plan.variant_glb(0,16).is_err());
        assert!(validate_distances([10.,10.]).is_err());assert!(validate_distances((1..=9).map(|n|n as f32)).is_ok());assert_eq!(selected_level(&[10.,30.],10.),1);assert_eq!(selected_level(&[10.,30.],30.),2);assert_eq!(selected_level(&[10.,30.],f32::NAN),0);
    }
}
