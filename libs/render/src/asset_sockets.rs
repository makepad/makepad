//! Named, validated attachments on the ordinary glTF node graph.
use makepad_draw::makepad_math::{Mat4f,Quat,vec3f};
use makepad_gltf::{GltfDocument,JsonValue};
pub const MAX_ASSET_SOCKETS:usize=64;
#[derive(Clone,Debug)]pub struct AssetSocket{pub name:String,pub node:usize,pub rest_transform:Mat4f}
impl AssetSocket{
    /// Sampled node transform must use the same mesh-local frame as instance.
    /// No sampled value uses the imported rest hierarchy.
    pub fn placed(&self,instance:&Mat4f,sampled_node_world:Option<Mat4f>)->Result<Mat4f,String>{let result=Mat4f::mul(instance,&sampled_node_world.unwrap_or(self.rest_transform));if result.v.iter().any(|x|!x.is_finite()){Err("non-finite socket placement".into())}else{Ok(result)}}
}
pub fn node_rest_transforms(document:&GltfDocument)->Result<Vec<Mat4f>,String>{
    let nodes=document.nodes_slice();if nodes.len()>4096{return Err("socket node graph exceeds4096".into());}let mut parents=vec![None;nodes.len()];
    for(parent,node)in nodes.iter().enumerate(){for child in node.children.as_deref().unwrap_or(&[]){if *child>=nodes.len()||parents[*child].replace(parent).is_some(){return Err("socket invalid/multiple parent".into());}}}
    // Validate every chain, including cycles disconnected from scene roots.
    let mut done=vec![false;nodes.len()];for start in 0..nodes.len(){let mut path=Vec::new();let mut index=Some(start);while let Some(i)=index{if done[i]{break}if path.contains(&i){return Err("socket node graph cycle".into());}path.push(i);index=parents[i];}for i in path{done[i]=true;}}
    let locals=nodes.iter().map(|n|{let m=if let Some(v)=n.matrix{if n.translation.is_some()||n.rotation.is_some()||n.scale.is_some(){return Err("node matrix conflicts with TRS".to_string());}Mat4f{v}}else{let t=n.translation.unwrap_or([0.;3]);let r=n.rotation.unwrap_or([0.,0.,0.,1.]);let s=n.scale.unwrap_or([1.;3]);if r.iter().any(|v|!v.is_finite())||(r.iter().map(|v|v*v).sum::<f32>()-1.).abs()>1e-4||s.iter().any(|v|v.abs()<1e-8){return Err("invalid socket node TRS".into());}crate::skin::trs_to_mat4(&crate::skin::NodeTrs{t:vec3f(t[0],t[1],t[2]),r:Quat{x:r[0],y:r[1],z:r[2],w:r[3]},s:vec3f(s[0],s[1],s[2])})};if m.v.iter().any(|v|!v.is_finite())||[3,7,11].iter().any(|i|m.v[*i].abs()>1e-5)||(m.v[15]-1.).abs()>1e-5{return Err("invalid socket affine matrix".into());}Ok(m)}).collect::<Result<Vec<_>,String>>()?;
    let mut worlds=vec![None;nodes.len()];
    for start in 0..nodes.len(){let mut path=Vec::new();let mut at=Some(start);while let Some(i)=at{if worlds[i].is_some(){break}path.push(i);at=parents[i];}while let Some(i)=path.pop(){let world=parents[i].map(|p|Mat4f::mul(&worlds[p].unwrap(),&locals[i])).unwrap_or(locals[i]);if world.v.iter().any(|v|!v.is_finite()){return Err("node hierarchy transform overflow".into());}worlds[i]=Some(world);}}
    Ok(worlds.into_iter().map(Option::unwrap).collect())
}
pub fn parse_asset_sockets(document:&GltfDocument)->Result<Vec<AssetSocket>,String>{
    let nodes=document.nodes_slice();let worlds=node_rest_transforms(document)?;
    let mut out=Vec::new();let mut names=std::collections::BTreeSet::new();for(index,node)in nodes.iter().enumerate(){let marker=node.extras.as_ref().and_then(|v|v.key("MAKEPAD_socket"));match marker{None|Some(JsonValue::Bool(false))=>continue,Some(JsonValue::Bool(true))=>{},_=>return Err("socket marker must be boolean".into())};let name=node.name.as_ref().ok_or("socket needs a name")?;if name.is_empty()||name.len()>96||name.chars().any(char::is_control)||!names.insert(name){return Err("socket invalid/duplicate name".into());}let transform=worlds[index];out.push(AssetSocket{name:name.clone(),node:index,rest_transform:transform});if out.len()>MAX_ASSET_SOCKETS{return Err("socket count exceeds64".into());}}Ok(out)
}
#[cfg(test)]mod tests{use super::*;use makepad_draw::makepad_math::vec4;
    fn doc(nodes:&str)->GltfDocument{makepad_gltf::parse_gltf_json(&format!(r#"{{"asset":{{"version":"2.0"}},"nodes":{nodes}}}"#)).unwrap()}
    #[test]fn socket_rest_and_sampled_joint_attachment_follow_instance(){let d=doc(r#"[{"translation":[3,0,0],"scale":[2,2,2],"children":[1]},{"name":"grip","translation":[0,1,0],"extras":{"MAKEPAD_socket":true}}]"#);let sockets=parse_asset_sockets(&d).unwrap();assert_eq!(sockets[0].node,1);let rest=sockets[0].placed(&Mat4f::identity(),None).unwrap().transform_vec4(vec4(0.,0.,0.,1.)).to_vec3f();assert_eq!(rest,vec3f(3.,2.,0.));let sampled=crate::skin::trs_to_mat4(&crate::skin::NodeTrs{t:vec3f(0.,5.,0.),..Default::default()});assert_eq!(sockets[0].placed(&Mat4f::identity(),Some(sampled)).unwrap().transform_vec4(vec4(0.,0.,0.,1.)).to_vec3f(),vec3f(0.,5.,0.));}
    #[test]fn cycles_duplicate_names_and_bad_markers_refuse(){for nodes in [r#"[{"children":[0]}]"#,r#"[{"name":"a","extras":{"MAKEPAD_socket":true}},{"name":"a","extras":{"MAKEPAD_socket":true}}]"#,r#"[{"name":"a","extras":{"MAKEPAD_socket":1}}]"#]{assert!(parse_asset_sockets(&doc(nodes)).is_err());}}
}
