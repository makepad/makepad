//! Character recipe contracts shared with Asset UI. These are explicit model
//! pins, never a guess about the chat model's authoring ability.
use makepad_ai_hub::protocol::GenerateRequestJson;

pub const CHARACTER_LLM_MODEL: &str = "qwen3.5-9b";
pub const CHARACTER_IMAGE_MODEL: &str = "flux1-dev";
pub const CHARACTER_MATTE_MODEL: &str = "birefnet-hr";
pub const CHARACTER_MESH_MODEL: &str = "trellis-2";
pub const CHARACTER_RIG_MODEL: &str = "skintokens";
pub const CHARACTER_MOTION_MODEL: &str = "hy-motion";
pub const CHARACTER_DOMAINS: &[&str] = &["text", "image", "matte", "mesh", "rig", "motion"];
pub const CHARACTER_PINS: &[(&str, &str)] = &[
    ("text", CHARACTER_LLM_MODEL), ("image", CHARACTER_IMAGE_MODEL),
    ("matte", CHARACTER_MATTE_MODEL), ("mesh", CHARACTER_MESH_MODEL),
    ("rig", CHARACTER_RIG_MODEL), ("motion", CHARACTER_MOTION_MODEL),
];

pub fn configure_expansion(request: &mut GenerateRequestJson, prompt: &str) {
    request.target_domain = Some("rig".into());
    request.identity_anchor = Some(prompt.trim().into());
    request.temperature = Some(0.0);
    request.style = Some("When the intent names an established character, preserve the exact named identity and canonical official design unchanged. Do not redesign, genericize, or guess traits. If a visual trait is uncertain, omit it instead of inventing it; it is better to say 'canonical official design unchanged' and spend the remaining prompt on full-body framing, a relaxed wide A-pose with straight diagonal arms and hands clear above the hips, visible gaps between every limb and the torso, even studio light, a uniform plain background, and a clean separated silhouette. Rigging constraints may change pose and spacing but never delete canonical anatomy or worn pieces.".into());
    request.max_tokens = Some(512);
}

pub fn validate_brief(prompt: &str, text: &str) -> Result<(), String> {
    let words = text.split_whitespace().count();
    if words < 24 {
        return Err(format!("LLM character brief is too short ({words} words, need at least 24); refusing to start image generation"));
    }
    if !text.to_lowercase().contains(&prompt.trim().to_lowercase()) {
        return Err(format!("LLM character brief dropped identity anchor {:?}; refusing to start image generation", prompt.trim()));
    }
    Ok(())
}

/// Facts read from the artifact, not promised because a rig job was submitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharacterMetadata {
    pub skinned: bool,
    pub animated: bool,
    pub embedded_atlas: bool,
    pub clips: Vec<String>,
    pub playable: bool,
}

pub fn inspect_character(bytes: &[u8]) -> Result<CharacterMetadata, String> {
    let loaded = makepad_gltf::load_gltf_from_bytes(bytes, None).map_err(|e| e.to_string())?;
    // Rigid part oscillators are legitimate assets, but they are not a skin.
    if loaded.document.skins.as_deref().unwrap_or(&[]).is_empty() {
        return Ok(CharacterMetadata { skinned: false, animated: false, embedded_atlas: false,
            clips: Vec::new(), playable: false });
    }
    // Do not bless attribute names alone. This is the exact runtime skin
    // parser, with admission checks before its legacy weight normalization.
    let model = makepad_render::skin::SkinnedModel::parse_glb_validated(bytes)?;
    let clips = model.clips.iter().map(|clip| clip.name.clone()).collect::<Vec<_>>();
    let animated = !clips.is_empty();
    // Store-streamed CharacterModel needs its atlas embedded, unlike a local
    // pack which can supply a sidecar texture. Decode it with the UI's CPU
    // decoder as well: an arbitrary buffer named image/png is not a texture.
    let embedded_atlas = makepad_render::embedded_base_color_png(bytes)
        .is_some_and(|png| makepad_draw::image_cache::ImageBuffer::from_png(&png).is_ok());
    let playable = embedded_atlas && model.gait_clips().is_some();
    Ok(CharacterMetadata { skinned: true, animated, embedded_atlas, clips, playable })
}
