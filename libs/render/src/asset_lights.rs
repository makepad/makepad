//! Bounded KHR_lights_punctual import. Node transforms place the emitter;
//! they never scale its photometric intensity, cone angles, or range.
use makepad_draw::makepad_math::{Mat4f, Quat, Vec3f, vec3f, vec4};
use makepad_gltf::{GltfDocument, JsonValue};

pub const MAX_ASSET_LIGHTS: usize = 32;
pub const MAX_ASSET_FRAME_LIGHTS: usize = 256;

#[derive(Clone, Debug)]
pub enum AssetLightKind { Point, Spot { inner: f32, outer: f32 } }

#[derive(Clone, Debug)]
pub struct AssetLightEmitter {
    pub node: usize,
    pub rest_transform: Mat4f,
    pub animation:Option<std::sync::Arc<crate::asset_rigid::RigidHierarchy>>,
    pub color: Vec3f,
    pub intensity: f32,
    pub range: f32,
    pub kind: AssetLightKind,
}

impl AssetLightEmitter {
    /// `node_transform` is mesh-local sampled animation for a rig, or the
    /// imported global node transform for a static object.
    pub fn placed(&self, instance: &Mat4f, node_transform: Option<Mat4f>) -> crate::lightmap::LmLight {
        self.placed_at(instance,node_transform,0.0)
    }
    pub fn placed_at(&self,instance:&Mat4f,node_transform:Option<Mat4f>,time:f32)->crate::lightmap::LmLight{
        let animated=self.animation.as_ref().map(|animation|{let duration=animation.times.last().copied().unwrap_or(0.0);animation.transform(Some(if duration>0.0{time.rem_euclid(duration)}else{0.0}))});
        let matrix = Mat4f::mul(instance, &node_transform.or(animated).unwrap_or(self.rest_transform));
        let pos = matrix.transform_vec4(vec4(0.0, 0.0, 0.0, 1.0)).to_vec3f();
        let direction = matrix.transform_vec4(vec4(0.0, 0.0, -1.0, 0.0)).to_vec3f();
        let mut light = crate::lightmap::LmLight::omni(pos, self.color * self.intensity, self.range);
        // Authored fixtures replace entity headlights, so retain occlusion.
        // The existing atlas selects a bounded set and omits excess requests.
        light.shadows = true;
        // Negative spot is reserved for glTF punctual photometry. Existing
        // callers retain their 0..1 legacy falloff and downlight behavior.
        light.spot = -1.0;
        light.dir = if direction.length() > 1.0e-8 { direction.normalize() } else { vec3f(0.0, 0.0, -1.0) };
        light.cone = match self.kind { AssetLightKind::Point => None,
            AssetLightKind::Spot { inner, outer } => Some((inner.to_degrees(), outer.to_degrees())) };
        light
    }
}

fn number(value: &JsonValue) -> Option<f32> {
    match value { JsonValue::F64(v) => Some(*v as f32), JsonValue::U64(v) => Some(*v as f32),
        JsonValue::I64(v) => Some(*v as f32), _ => None }
}
fn array(value: &JsonValue) -> Option<&[JsonValue]> { match value { JsonValue::Array(v) => Some(v), _ => None } }

pub fn parse_asset_lights(document: &GltfDocument) -> Result<Vec<AssetLightEmitter>, String> {
    let Some(extension) = document.extensions.as_ref().and_then(|e| e.key("KHR_lights_punctual")) else { return Ok(Vec::new()) };
    let definitions = extension.key("lights").and_then(array).ok_or("punctual extension has no lights array")?;
    if definitions.len() > MAX_ASSET_LIGHTS { return Err("asset exceeds 32 punctual lights".into()); }
    let nodes = document.nodes_slice();
    if nodes.len() > 4096 { return Err("light attachment graph exceeds 4096 nodes".into()); }
    let mut parents = vec![None; nodes.len()];
    for (parent, node) in nodes.iter().enumerate() {
        for &child in node.children.as_deref().unwrap_or(&[]) {
            if child >= nodes.len() || parents[child].replace(parent).is_some() { return Err("light attachment graph has invalid or repeated child".into()); }
        }
    }
    let local: Vec<_> = nodes.iter().map(|node| {
        if let Some(v) = node.matrix { return Mat4f { v } }
        let [tx,ty,tz] = node.translation.unwrap_or([0.0;3]);
        let [x,y,z,w] = node.rotation.unwrap_or([0.0,0.0,0.0,1.0]);
        let [sx,sy,sz] = node.scale.unwrap_or([1.0;3]);
        crate::skin::trs_to_mat4(&crate::skin::NodeTrs { t: vec3f(tx,ty,tz), r: Quat{x,y,z,w}, s: vec3f(sx,sy,sz) })
    }).collect();
    let mut out = Vec::new();
    for (node_index, node) in nodes.iter().enumerate() {
        let Some(reference) = node.extensions.as_ref().and_then(|e| e.key("KHR_lights_punctual")).and_then(|e| e.key("light")) else { continue };
        let index = number(reference).filter(|v| v.is_finite() && *v >= 0.0 && v.fract() == 0.0).ok_or("invalid punctual light index")? as usize;
        let value = definitions.get(index).ok_or("punctual light index outside definitions")?;
        let read = |key, default| -> Result<f32,String> { match value.key(key) { None => Ok(default), Some(v) => number(v).filter(|v| v.is_finite()).ok_or_else(||format!("invalid light {key}")) } };
        let intensity = read("intensity", 1.0)?;
        let range = read("range", 100.0)?;
        if intensity < 0.0 || intensity > 1.0e9 || !(range > 0.0 && range <= 10_000.0) { return Err("punctual intensity/range exceeds runtime bounds".into()); }
        let color = if let Some(value) = value.key("color") {
            let values = array(value).ok_or("invalid light color")?;
            if values.len() != 3 { return Err("light color needs three channels".into()); }
            let mut channels = [0.0;3];
            for (i,v) in values.iter().enumerate() { channels[i] = number(v).filter(|v|v.is_finite() && (0.0..=1.0).contains(v)).ok_or("invalid linear light color")?; }
            vec3f(channels[0],channels[1],channels[2])
        } else { vec3f(1.0,1.0,1.0) };
        let kind = match value.key("type").and_then(JsonValue::string).map(String::as_str) {
            Some("point") => AssetLightKind::Point,
            Some("spot") => {
                let spot = value.key("spot");
                let angle = |key, default| -> Result<f32,String> { match spot.and_then(|s|s.key(key)) { None => Ok(default), Some(v) => number(v).filter(|v|v.is_finite()).ok_or_else(||format!("invalid {key}")) } };
                let inner = angle("innerConeAngle",0.0)?; let outer = angle("outerConeAngle",std::f32::consts::FRAC_PI_4)?;
                if !(inner >= 0.0 && inner < outer && outer <= std::f32::consts::FRAC_PI_2) { return Err("invalid punctual spot cone bounds".into()); }
                AssetLightKind::Spot { inner, outer }
            }
            _ => return Err("asset light must be point or spot".into()),
        };
        let mut rest_transform = local[node_index];
        let mut parent = parents[node_index]; let mut depth = 0;
        while let Some(index) = parent {
            depth += 1; if depth > nodes.len() { return Err("cyclic light attachment graph".into()); }
            rest_transform = Mat4f::mul(&local[index], &rest_transform); parent = parents[index];
        }
        if rest_transform.v.iter().any(|v|!v.is_finite()) { return Err("non-finite light attachment transform".into()); }
        out.push(AssetLightEmitter { animation:None,node:node_index, rest_transform, color,intensity,range,kind });
        if out.len() > MAX_ASSET_LIGHTS { return Err("asset exceeds 32 light attachments".into()); }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document(light:&str,nodes:&str)->GltfDocument {
        makepad_gltf::parse_gltf_json(&format!(r#"{{"asset":{{"version":"2.0"}},"nodes":{nodes},"extensions":{{"KHR_lights_punctual":{{"lights":[{light}]}}}}}}"#)).unwrap()
    }
    #[test]
    fn attached_spot_moves_without_scaling_photometry() {
        let document=document(r#"{"type":"spot","intensity":40,"range":12,"color":[1,0.5,0.25],"spot":{"innerConeAngle":0.1,"outerConeAngle":0.4}}"#,
            r#"[{"translation":[3,0,0],"scale":[2,2,2],"children":[1]},{"translation":[0,1,0],"extensions":{"KHR_lights_punctual":{"light":0}}}]"#);
        let lights=parse_asset_lights(&document).unwrap();
        let instance=crate::skin::trs_to_mat4(&crate::skin::NodeTrs{t:vec3f(10.0,0.0,0.0),s:vec3f(3.0,3.0,3.0),..Default::default()});
        let light=lights[0].placed(&instance,None);
        assert_eq!(light.pos,vec3f(19.0,6.0,0.0));
        assert_eq!(light.dir,vec3f(0.0,0.0,-1.0));
        assert_eq!(light.radius,12.0); assert_eq!(light.color,vec3f(40.0,20.0,10.0));
        assert!(light.spot<0.0);
        let sampled=crate::skin::trs_to_mat4(&crate::skin::NodeTrs{t:vec3f(0.0,4.0,0.0),..Default::default()});
        assert_eq!(lights[0].placed(&instance,Some(sampled)).pos,vec3f(10.0,12.0,0.0));
    }
    #[test]
    fn malformed_cones_cycles_and_attachment_overflow_are_rejected() {
        let nodes=r#"[{"extensions":{"KHR_lights_punctual":{"light":0}}}]"#;
        assert!(parse_asset_lights(&document(r#"{"type":"spot","spot":{"innerConeAngle":0.8,"outerConeAngle":0.4}}"#,nodes)).is_err());
        assert!(parse_asset_lights(&document(r#"{"type":"point"}"#,r#"[{"children":[0],"extensions":{"KHR_lights_punctual":{"light":0}}}]"#)).is_err());
        let repeated=format!("[{}]",std::iter::repeat(r#"{"extensions":{"KHR_lights_punctual":{"light":0}}}"#).take(33).collect::<Vec<_>>().join(","));
        assert!(parse_asset_lights(&document(r#"{"type":"point"}"#,&repeated)).is_err());
    }
}

/// Complete worker preparation, including generic animated parent attachments.
pub fn prepare_asset_lights(glb:&[u8])->Result<Vec<AssetLightEmitter>,String>{
    let loaded=makepad_gltf::load_gltf_from_bytes(glb,None).map_err(|e|e.to_string())?;
    let mut emitters=parse_asset_lights(&loaded.document)?;
    if emitters.is_empty(){return Ok(emitters)}
    let(json,bin)=crate::asset_morph::chunks(glb)?;let acc=crate::skin::Accessors{json:&json,bin};
    let rests=crate::asset_rigid::rest_nodes(&json);let forced=emitters.iter().map(|e|e.node).collect::<Vec<_>>();
    let clips=crate::asset_rigid::node_clips(&json,&acc,&rests,&[],&forced)?;
    for emitter in &mut emitters{emitter.animation=clips.iter().find(|(node,_)|*node==emitter.node).map(|(_,clip)|clip.clone());}Ok(emitters)
}
