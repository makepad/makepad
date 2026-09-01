/// Image canvas presets; entry 0 is the chain default. Flux wants /16 dims.
/// What a run says when its expander could not deliver. The run itself
/// carries on with the prompt the person typed — an expansion is a
/// courtesy, never a precondition.
pub const EXPAND_FALLBACK_NOTE: &str = "expand failed, used raw prompt";

/// Poll cadence per active job.
pub const IMAGE_SIZES: &[(u32, u32)] = &[
    (512, 512),
    (768, 768),
    (1024, 1024),
    (768, 512),
    (512, 768),
    (1024, 576),
];
/// Image step presets; the dropdown's extra first entry means "model
/// default" (schnell 4, dev-class ~20).
pub const IMAGE_STEPS: &[u32] = &[4, 8, 12, 20, 28, 50];
/// img2img strength choices for edit chains: 1.0 = a full instruction edit
/// (reference tokens only); lower = the sampler starts from the VAE-encoded
/// input at sigma index floor((1-strength)*steps), keeping more of it.
pub const EDIT_STRENGTHS: &[f32] = &[1.0, 0.85, 0.7, 0.55, 0.4, 0.25];
/// LoRA strength choices for the image stage.
pub const LORA_STRENGTHS: &[f32] = &[1.0, 0.8, 0.6, 0.4, 1.2];
/// RIFE interpolation factors offered for video (1 = off).
pub const VIDEO_INTERPOLATE: &[u32] = &[1, 2, 4];
/// Enhance-stage factor choices, shared by the uprez and tween pickers.
pub const ENHANCE_FACTORS: &[u32] = &[1, 2, 4];
/// TRELLIS UV-atlas presets. 1024 preserves the current fast default; the
/// larger atlases trade bake time and device memory for sharper materials.
pub const MESH_TEXTURE_SIZES: &[u32] = &[1024, 2048, 4096];
/// QEM face-count presets. Index 0 is Auto (12k objects / 20k characters).
pub const MESH_FACE_COUNTS: &[u32] = &[0, 12_000, 20_000, 40_000, 80_000, 160_000];
/// Video canvas presets; entry 0 is the small default.
pub const VIDEO_SIZES: &[(u32, u32)] = &[(640, 352), (864, 480), (960, 544)];
/// Video (frames, steps) presets at 16 fps; entry 0 is the default.
pub const VIDEO_LENGTHS: &[(u32, u32)] = &[(39, 30), (65, 30), (97, 40), (129, 50)];
/// Full-song targets offered by the UI. Music3 accepts any duration from
/// five seconds through five minutes; these minute-aligned presets keep the
/// common choice legible and make a three-minute song the honest default.
pub const MUSIC_LENGTHS: &[u32] = &[60, 120, 180, 240, 300];
pub const MUSIC_DEFAULT_SECONDS: u32 = 180;
pub const MUSIC_MIN_SECONDS: u32 = 5;
pub const MUSIC_MAX_SECONDS: u32 = 300;
