//! Portable object hierarchy, arbitrary bone rests and punctual asset lights.
//! Extends the normal self-contained authoring GLB without rebuilding materials.
use crate::{augment::{GlbRewrite,number,object,string,validation},GltfError,JsonValue};
use std::collections::{BTreeSet,HashMap};

#[derive(Clone,Debug)]
pub struct GlbTransform {pub translation:[f32;3],pub rotation:[f32;4],pub scale:[f32;3]}
impl Default for GlbTransform {fn default()->Self{Self{translation:[0.;3],rotation:[0.,0.,0.,1.],scale:[1.;3]}}}
#[derive(Clone,Debug)]
pub struct GlbAuthoringObject {pub name:String,pub parent:Option<usize>,pub transform:GlbTransform,pub primitives:Vec<usize>,pub extras:Option<JsonValue>,pub lods:Vec<usize>,pub lod_distances:Vec<f32>}
#[derive(Clone,Debug)]
pub enum GlbAttachment {Root,Object(usize),Joint(usize)}
#[derive(Clone,Debug)]
pub enum GlbPunctualKind {Point,Spot{inner:f32,outer:f32}}
#[derive(Clone,Debug)]
pub struct GlbPunctualLight {pub name:String,pub attachment:GlbAttachment,pub transform:GlbTransform,
    pub kind:GlbPunctualKind,pub color:[f32;3],pub intensity:f32,pub range:f32}
#[derive(Clone,Debug)]
pub struct GlbSocket {pub name:String,pub attachment:GlbAttachment,pub transform:GlbTransform}
#[derive(Clone,Debug)]
pub struct GlbBoneRest {pub transform:GlbTransform,pub inverse_bind:[f32;16]}
#[derive(Clone,Debug,Default)]
pub struct GlbAuthoring {
    pub objects:Vec<GlbAuthoringObject>,pub lights:Vec<GlbPunctualLight>,pub sockets:Vec<GlbSocket>,
    pub rests:Vec<GlbBoneRest>,pub metadata:Option<JsonValue>,
}
fn fields(v:&mut JsonValue)->Result<&mut HashMap<String,JsonValue>,GltfError>{match v{JsonValue::Object(f)=>Ok(f),_=>Err(validation("authoring expected object"))}}
fn index(v:&JsonValue)->Option<usize>{match v{JsonValue::U64(v)=>usize::try_from(*v).ok(),JsonValue::I64(v)=>usize::try_from(*v).ok(),_=>None}}
fn floats(v:&[f32])->Result<JsonValue,GltfError>{if v.iter().any(|v|!v.is_finite()){return Err(validation("non-finite authoring value"));}
    Ok(JsonValue::Array(v.iter().map(|&v|JsonValue::F64(v as f64)).collect()))}
fn put_transform(f:&mut HashMap<String,JsonValue>,t:&GlbTransform)->Result<(),GltfError>{
    if t.scale.iter().any(|v|v.abs()<1e-8)||(t.rotation.iter().map(|v|v*v).sum::<f32>()-1.).abs()>1e-4{return Err(validation("invalid authoring TRS"));}
    f.remove("matrix");f.insert("translation".into(),floats(&t.translation)?);f.insert("rotation".into(),floats(&t.rotation)?);f.insert("scale".into(),floats(&t.scale)?);Ok(())
}
fn child(nodes:&mut [JsonValue],parent:usize,id:usize)->Result<(),GltfError>{
    let node=nodes.get_mut(parent).ok_or_else(||validation("attachment node out of range"))?;
    let children=fields(node)?.entry("children".into()).or_insert_with(||JsonValue::Array(Vec::new()));
    let JsonValue::Array(children)=children else{return Err(validation("node children are not an array"));};children.push(number(id));Ok(())
}
fn target(a:&GlbAttachment,objects:&[usize],joints:&[usize],root:usize)->Result<usize,GltfError>{match a{
    GlbAttachment::Root=>Ok(root),GlbAttachment::Object(i)=>objects.get(*i).copied().ok_or_else(||validation("light/socket object")),
    GlbAttachment::Joint(i)=>joints.get(*i).copied().ok_or_else(||validation("light/socket joint"))}}

/// Input is the engine's one-mesh product. Static primitives are partitioned
/// into authored object nodes. Skinned geometry is already in bind space and
/// retains its common skin mesh; object nodes still carry attached emitters.
pub fn augment_glb_authoring(input:&[u8],authoring:&GlbAuthoring)->Result<Vec<u8>,GltfError>{
    if authoring.objects.len()>256||authoring.lights.len()>32||authoring.sockets.len()>128{return Err(validation("authoring attachment budget"));}
    let mut rewrite=GlbRewrite::begin(input)?;
    let mut root_extensions=rewrite.document.extensions.clone().unwrap_or_else(||object([]));
    let mut nodes=rewrite.take_array("nodes")?;let mut scenes=rewrite.take_array("scenes")?;
    let mut meshes=rewrite.take_array("meshes")?;let mut skins=rewrite.take_array("skins")?;
    let mut views=rewrite.take_array("bufferViews")?;let mut accessors=rewrite.take_array("accessors")?;
    let skinned=!skins.is_empty();
    let root=nodes.len();nodes.push(object([("name",string("__asset_root"))]));
    let scene_index=rewrite.document.scene.unwrap_or(0);
    let scene=fields(scenes.get_mut(scene_index).ok_or_else(||validation("authoring scene missing"))?)?;
    let existing=match scene.insert("nodes".into(),JsonValue::Array(vec![number(root)])){Some(JsonValue::Array(v))=>v,None=>Vec::new(),_=>return Err(validation("scene roots"))};
    for old in existing {child(&mut nodes,root,index(&old).ok_or_else(||validation("scene root index"))?)?;}
    let joint_indices=if let Some(skin)=skins.first_mut(){match fields(skin)?.get("joints"){
        Some(JsonValue::Array(ids))=>ids.iter().map(|v|index(v).ok_or_else(||validation("skin joint index"))).collect::<Result<Vec<_>,_>>()?,
        _=>return Err(validation("skin has no joints")),}}else{Vec::new()};
    if !authoring.rests.is_empty(){
        if skins.len()!=1||authoring.rests.len()!=joint_indices.len(){return Err(validation("rest hierarchy differs from skin"));}
        let mut data=Vec::with_capacity(authoring.rests.len()*64);
        for (rest,&node) in authoring.rests.iter().zip(&joint_indices){
            put_transform(fields(nodes.get_mut(node).ok_or_else(||validation("rest node"))?)?,&rest.transform)?;
            for value in rest.inverse_bind {if !value.is_finite(){return Err(validation("non-finite inverse bind"));}data.extend_from_slice(&value.to_le_bytes());}
        }
        let accessor=rewrite.append_accessor(&mut views,&mut accessors,&data,5126,authoring.rests.len(),"MAT4",false,None,None,None);
        fields(&mut skins[0])?.insert("inverseBindMatrices".into(),number(accessor));
    }
    let original_primitives=if !authoring.objects.is_empty() {
        let mesh=fields(meshes.get_mut(0).ok_or_else(||validation("authoring mesh missing"))?)?;
        match mesh.get("primitives"){Some(JsonValue::Array(v))=>v.clone(),_=>return Err(validation("authoring mesh primitives"))}
    }else{Vec::new()};
    let object_nodes=(0..authoring.objects.len()).map(|i|nodes.len()+i).collect::<Vec<_>>();
    let mut assigned=BTreeSet::new();
    for (i,object_def) in authoring.objects.iter().enumerate(){
        let mut node=object([("name",string(&object_def.name))]);put_transform(fields(&mut node)?,&object_def.transform)?;
        if let Some(extras)=&object_def.extras {fields(&mut node)?.insert("extras".into(),extras.clone());}
        if !object_def.primitives.is_empty(){
            let mut primitives=Vec::new();for &p in &object_def.primitives{
                if !assigned.insert(p){return Err(validation("primitive assigned to more than one authoring object"));}
                primitives.push(original_primitives.get(p).ok_or_else(||validation("authoring primitive out of range"))?.clone());
            }
            let mesh_id=meshes.len();meshes.push(object([("name",string(&object_def.name)),("primitives",JsonValue::Array(primitives))]));
            if !skinned{fields(&mut node)?.insert("mesh".into(),number(mesh_id));}
        }
        if let Some(parent)=object_def.parent {if parent>=authoring.objects.len()||parent==i{return Err(validation("object parent index"));}}
        nodes.push(node);
    }
    let mut geometry_nodes=object_nodes.clone();
    if skinned{
        for (i,definition) in authoring.objects.iter().enumerate(){
            if definition.primitives.is_empty(){continue;}
            let mesh=meshes.iter_mut().enumerate().skip(1).find_map(|(m,v)|fields(v).ok().and_then(|f|f.get("name")).is_some_and(|v|matches!(v,JsonValue::String(n)if n==&definition.name)).then_some(m)).ok_or_else(||validation("skin object mesh"))?;
            let n=nodes.len();nodes.push(object([("name",string(&format!("{}__skin",definition.name))),("mesh",number(mesh)),("skin",number(0))]));geometry_nodes[i]=n;
        }
    }
    let mut lod_children=BTreeSet::new();
    for (i,object_def) in authoring.objects.iter().enumerate(){
        if object_def.lods.len()!=object_def.lod_distances.len()||object_def.lod_distances.iter().any(|v|!v.is_finite()||*v<=0.)||object_def.lod_distances.windows(2).any(|w|w[0]>=w[1]){return Err(validation("LOD distances"));}
        if !object_def.lods.is_empty(){
            for &lod in &object_def.lods{if lod>=object_nodes.len()||lod==i||!lod_children.insert(lod){return Err(validation("LOD node reference"));}}
            let f=fields(&mut nodes[geometry_nodes[i]])?;let extensions=f.entry("extensions".into()).or_insert_with(||object([]));fields(extensions)?.insert("MSFT_lod".into(),object([("ids",JsonValue::Array(object_def.lods.iter().map(|&j|number(geometry_nodes[j])).collect()))]));
            let extras=f.entry("extras".into()).or_insert_with(||object([]));fields(extras)?.insert("MAKEPAD_lod_distances".into(),floats(&object_def.lod_distances)?);
        }
    }
    if !lod_children.is_empty(){let used=rewrite.array_mut("extensionsUsed")?;if !used.iter().any(|v|matches!(v,JsonValue::String(s)if s=="MSFT_lod")){used.push(string("MSFT_lod"));}}
    for (i,object_def) in authoring.objects.iter().enumerate(){
        let mut ancestors=BTreeSet::new();let mut current=Some(i);
        while let Some(j)=current {if !ancestors.insert(j){return Err(validation("authoring object cycle"));}current=authoring.objects[j].parent;}
        if !lod_children.contains(&i){
            child(&mut nodes,object_def.parent.map(|p|object_nodes[p]).unwrap_or(root),object_nodes[i])?;
            if skinned&&geometry_nodes[i]!=object_nodes[i]{child(&mut nodes,root,geometry_nodes[i])?;}
        }
    }
    if !original_primitives.is_empty(){
        if assigned.len()!=original_primitives.len(){return Err(validation("unassigned authoring primitive"));}
        // Keep the original BIN/accessors and mesh definitions for preservation,
        // but detach the old combined geometry from the scene to avoid doubling.
        for node in &mut nodes[..root] {if fields(node)?.get("mesh").and_then(index)==Some(0){fields(node)?.remove("mesh");fields(node)?.remove("skin");}}
    }
    if !authoring.lights.is_empty(){
        let extensions=fields(&mut root_extensions)?;
        let punctual=extensions.entry("KHR_lights_punctual".into()).or_insert_with(||object([]));
        let lights=fields(punctual)?.entry("lights".into()).or_insert_with(||JsonValue::Array(Vec::new()));
        let JsonValue::Array(lights)=lights else{return Err(validation("punctual lights array"));};
        for light in &authoring.lights {
            if !light.intensity.is_finite()||light.intensity<0.||!light.range.is_finite()||light.range<=0.||light.color.iter().any(|v|!(0.0..=1.0).contains(v)){return Err(validation("invalid punctual light"));}
            let mut value=object([("name",string(&light.name)),("color",floats(&light.color)?),("intensity",JsonValue::F64(light.intensity as f64)),("range",JsonValue::F64(light.range as f64))]);
            match light.kind{
                GlbPunctualKind::Point=>{fields(&mut value)?.insert("type".into(),string("point"));}
                GlbPunctualKind::Spot{inner,outer}=>{
                    if !inner.is_finite()||!outer.is_finite()||inner<0.||outer<=inner||outer>std::f32::consts::FRAC_PI_2{return Err(validation("invalid punctual cone"));}
                    fields(&mut value)?.insert("type".into(),string("spot"));fields(&mut value)?.insert("spot".into(),object([
                        ("innerConeAngle",JsonValue::F64(inner as f64)),("outerConeAngle",JsonValue::F64(outer as f64))]));
                }
            }
            let id=lights.len();lights.push(value);
            let mut node=object([("name",string(&light.name)),("extensions",object([("KHR_lights_punctual",object([("light",number(id))]))]))]);
            put_transform(fields(&mut node)?,&light.transform)?;
            let parent=target(&light.attachment,&object_nodes,&joint_indices,root)?;let id=nodes.len();nodes.push(node);child(&mut nodes,parent,id)?;
        }
        rewrite.insert("extensions",root_extensions);
        let used=rewrite.array_mut("extensionsUsed")?;
        if !used.iter().any(|v|matches!(v,JsonValue::String(s) if s=="KHR_lights_punctual")){used.push(string("KHR_lights_punctual"));}
    }
    for socket in &authoring.sockets{
        let mut node=object([("name",string(&socket.name)),("extras",object([("MAKEPAD_socket",JsonValue::Bool(true))]))]);
        put_transform(fields(&mut node)?,&socket.transform)?;let parent=target(&socket.attachment,&object_nodes,&joint_indices,root)?;
        let id=nodes.len();nodes.push(node);child(&mut nodes,parent,id)?;
    }
    if let Some(metadata)=&authoring.metadata{fields(&mut nodes[root])?.insert("extras".into(),metadata.clone());}
    rewrite.put_array("nodes",nodes);rewrite.put_array("scenes",scenes);rewrite.put_array("meshes",meshes);
    if !skins.is_empty(){rewrite.put_array("skins",skins);}
    rewrite.put_array("bufferViews",views);rewrite.put_array("accessors",accessors);rewrite.finish()
}
