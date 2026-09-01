//! User-saved pipeline snapshots. Click to run, × to delete.

use crate::pipeline::{
    format_music_duration, GenParams, EDIT_STRENGTHS, VIDEO_INTERPOLATE, PRESETS, IMAGE_SIZES, IMAGE_STEPS, MESH_FACE_COUNTS,
    MESH_TEXTURE_SIZES, MUSIC_LENGTHS, VIDEO_LENGTHS, VIDEO_SIZES,
};
use makepad_micro_serde::*;
use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_FAST_PRESETS: usize = 8;

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct SavedModelPin {
    pub domain: String,
    pub model: String,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct SavedFastPreset {
    pub id: String,
    pub name: String,
    pub pipeline: String,
    pub models: Vec<SavedModelPin>,
    pub voice: Option<String>,
    pub image_w: u32,
    pub image_h: u32,
    pub image_steps: Option<u32>,
    pub mesh_texture: u32,
    pub mesh_faces: Option<u32>,
    /// Option so presets saved before the flag existed still load (None =
    /// false).
    pub mesh_trellis_texture: Option<bool>,
    /// Motion-prompt override (None/empty = playable set).
    pub motion_prompt: Option<String>,
    pub video_w: u32,
    pub video_h: u32,
    pub video_frames: u32,
    pub video_steps: u32,
    /// Option so presets saved before the flag existed still load (None =
    /// true — the H3 default is an audible clip).
    pub video_audio: Option<bool>,
    pub edit_strength: Option<f32>,
    pub video_interpolate: Option<u32>,
    pub image_lora: Option<String>,
    pub image_lora_strength: Option<f32>,
    pub music_seconds: u32,
    /// Options so presets saved before the enhance stage existed still load.
    pub enhance_upscale: Option<u32>,
    pub enhance_interpolate: Option<u32>,
    pub enhance_flow: Option<bool>,
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
struct SavedFastFile {
    presets: Vec<SavedFastPreset>,
}

pub fn store_path() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../local/ai_content_library/fast_presets.json"
    ))
}

pub fn load(path: &Path) -> Vec<SavedFastPreset> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    SavedFastFile::deserialize_json(&text)
        .map(|file| file.presets)
        .unwrap_or_default()
}

pub fn save(path: &Path, presets: &[SavedFastPreset]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file = SavedFastFile {
        presets: presets.to_vec(),
    };
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, file.serialize_json()).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Short label from the current panel. Keep it chip-sized.
pub fn auto_name(
    pipeline: &str,
    models: &[(String, String)],
    gen: &GenParams,
) -> String {
    let short_pipe = pipeline
        .replace("expand → ", "exp→")
        .replace("image → ", "img→")
        .replace(" (playable)", "")
        .replace(" (small)", "");
    let mut bits = vec![short_pipe];
    for (domain, model) in models {
        bits.push(short_model(domain, model));
    }
    if pipeline.contains("music") {
        bits.push(format_music_duration(gen.music_seconds));
    } else if pipeline.contains("video") {
        bits.push(format!("{}f", gen.video_frames));
    } else if pipeline.contains("image") && !pipeline.contains("mesh") {
        bits.push(format!("{}×{}", gen.image_size.0, gen.image_size.1));
    }
    let name = bits.join(" · ");
    if name.chars().count() > 36 {
        name.chars().take(34).collect::<String>() + "…"
    } else {
        name
    }
}

fn short_model(_domain: &str, model: &str) -> String {
    model
        .trim_start_matches("minimax-")
        .trim_end_matches("-1.5-xl")
        .replace("music3-python", "py")
        .replace("music3", "m3")
        .replace("ace-step", "ace")
        .replace("flux1-", "fx1-")
        .replace("sa3-sfx", "sa3")
        .replace("moss-sfx", "moss")
        .replace("woosh-sfx", "woosh")
        .replace("indextts-2.5", "clone")
        .replace("birefnet-hr", "matte")
        .replace("da3-metric-large", "da3")
        .replace("sam3-1-multiplex", "sam3")
        .replace("hunyuan3d-paint-2.1", "hy")
        .replace("pbr-testpattern", "pbr-test")
        .replace("trellis-2", "trellis")
}

pub fn snapshot(
    pipeline: &str,
    models: Vec<(String, String)>,
    voice: Option<String>,
    gen: &GenParams,
    name: String,
) -> SavedFastPreset {
    SavedFastPreset {
        id: format!(
            "fp-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ),
        name,
        pipeline: pipeline.to_string(),
        models: models
            .into_iter()
            .map(|(domain, model)| SavedModelPin { domain, model })
            .collect(),
        voice,
        image_w: gen.image_size.0,
        image_h: gen.image_size.1,
        image_steps: gen.image_steps,
        mesh_texture: gen.mesh_texture_size,
        mesh_faces: gen.mesh_faces,
        mesh_trellis_texture: Some(gen.mesh_trellis_texture),
        motion_prompt: Some(gen.motion_prompt.clone()),
        video_w: gen.video_size.0,
        video_h: gen.video_size.1,
        video_frames: gen.video_frames,
        video_steps: gen.video_steps,
        video_audio: Some(gen.video_audio),
        edit_strength: Some(gen.edit_strength),
        video_interpolate: Some(gen.video_interpolate),
        image_lora: gen.image_lora.as_ref().map(|(name, _)| name.clone()),
        image_lora_strength: gen.image_lora.as_ref().map(|(_, strength)| *strength),
        music_seconds: gen.music_seconds,
        enhance_upscale: Some(gen.enhance_upscale),
        enhance_interpolate: Some(gen.enhance_interpolate),
        enhance_flow: Some(gen.enhance_flow),
    }
}

pub fn apply_gen(saved: &SavedFastPreset) -> GenParams {
    GenParams {
        image_size: (saved.image_w, saved.image_h),
        image_steps: saved.image_steps,
        mesh_texture_size: saved.mesh_texture,
        mesh_faces: saved.mesh_faces,
        mesh_trellis_texture: saved.mesh_trellis_texture.unwrap_or(false),
        motion_prompt: saved.motion_prompt.clone().unwrap_or_default(),
        video_size: (saved.video_w, saved.video_h),
        video_frames: saved.video_frames,
        video_steps: saved.video_steps,
        video_audio: saved.video_audio.unwrap_or(true),
        edit_strength: saved.edit_strength.unwrap_or(1.0),
        video_interpolate: saved.video_interpolate.unwrap_or(1),
        image_lora: saved
            .image_lora
            .clone()
            .filter(|name| !name.is_empty())
            .map(|name| (name, saved.image_lora_strength.unwrap_or(1.0))),
        music_seconds: saved.music_seconds,
        enhance_upscale: saved.enhance_upscale.unwrap_or(2),
        enhance_interpolate: saved.enhance_interpolate.unwrap_or(2),
        enhance_flow: saved.enhance_flow.unwrap_or(true),
        // Loop-ness derives from the preset row at dispatch, never a spec.
        video_loop: false,
    }
}

/// Dropdown index of `factor` in [`VIDEO_INTERPOLATE`] (off when unknown).
pub fn nearest_video_interpolate(factor: u32) -> usize {
    VIDEO_INTERPOLATE.iter().position(|f| *f == factor).unwrap_or(0)
}

/// Dropdown index of the nearest [`EDIT_STRENGTHS`] entry.
pub fn nearest_edit_strength(strength: f32) -> usize {
    EDIT_STRENGTHS
        .iter()
        .enumerate()
        .min_by(|a, b| {
            (a.1 - strength)
                .abs()
                .partial_cmp(&(b.1 - strength).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

pub fn nearest_image_size(w: u32, h: u32) -> usize {
    IMAGE_SIZES
        .iter()
        .position(|size| *size == (w, h))
        .unwrap_or(0)
}

pub fn nearest_image_steps(steps: Option<u32>) -> usize {
    match steps {
        None => 0,
        Some(s) => IMAGE_STEPS
            .iter()
            .position(|v| *v == s)
            .map(|i| i + 1)
            .unwrap_or(0),
    }
}

pub fn nearest_mesh_texture(size: u32) -> usize {
    MESH_TEXTURE_SIZES
        .iter()
        .position(|v| *v == size)
        .unwrap_or(0)
}

pub fn nearest_mesh_faces(faces: Option<u32>) -> usize {
    match faces {
        None => 0,
        Some(n) => MESH_FACE_COUNTS
            .iter()
            .position(|v| *v == n)
            .unwrap_or(0),
    }
}

pub fn nearest_video_size(w: u32, h: u32) -> usize {
    VIDEO_SIZES
        .iter()
        .position(|size| *size == (w, h))
        .unwrap_or(0)
}

pub fn nearest_video_len(frames: u32, steps: u32) -> usize {
    VIDEO_LENGTHS
        .iter()
        .position(|pair| *pair == (frames, steps))
        .unwrap_or(0)
}

pub fn nearest_music_len(seconds: u32) -> usize {
    MUSIC_LENGTHS
        .iter()
        .position(|v| *v == seconds)
        .unwrap_or(0)
}

pub fn pipeline_index(name: &str) -> Option<usize> {
    PRESETS.iter().position(|p| p.name == name)
}
