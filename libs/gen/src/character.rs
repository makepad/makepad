//! What a generated character is, read from its bytes: the facts the
//! character recipe's checks gate on and the tools report.

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
