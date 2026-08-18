//! Dual-stream reference branch layout (`unet_dual`).
//!
//! Official `UNet2p5DConditionModel` deep-copies the 4-ch UNet *before*
//! widening `conv_in` to 12, then runs that copy **once per job** (not per
//! DDIM step) at timestep 0, `mode="w"`. Each Basic2p5D block stashes its
//! **norm1** tokens under a layer name. The main 12-ch UNet later reads those
//! tokens in `mode="r"` via `attn_refview`.
//!
//! `unet_dual` is a plain wrapped block (`extras=false`): no MDA / RA / MA /
//! DINO on the write path. CFG `ref_scale` is `[0, 1, 1]`.

use crate::cond_assembly::{cfg_branch_table, CFG_BRANCHES};
use crate::unet_keys::{DUAL_IN_CHANNELS, MAIN_IN_CHANNELS, TEXT_TOKENS};

/// Dual-stream write runs at train timestep 0.
pub const REF_TIMESTEP: f32 = 0.0;

/// Write-mode layer names, official `init_attention` format
/// `{down|mid|up}_{block}_{attn}_{transformer}`.
pub fn write_layer_names() -> Vec<&'static str> {
    vec![
        "down_0_0_0",
        "down_0_1_0",
        "down_1_0_0",
        "down_1_1_0",
        "down_2_0_0",
        "down_2_1_0",
        "mid_0_0",
        "up_1_0_0",
        "up_1_1_0",
        "up_1_2_0",
        "up_2_0_0",
        "up_2_1_0",
        "up_2_2_0",
        "up_3_0_0",
        "up_3_1_0",
        "up_3_2_0",
    ]
}

/// Spatial tokens at each write-mode layer for a 64×64 latent (512 view).
/// Matches SD2.1 down/up spatial schedule.
pub fn write_layer_spatial_64() -> Vec<(&'static str, usize, usize, usize)> {
    // (name, channels, width, height)
    vec![
        ("down_0_0_0", 320, 64, 64),
        ("down_0_1_0", 320, 64, 64),
        ("down_1_0_0", 640, 32, 32),
        ("down_1_1_0", 640, 32, 32),
        ("down_2_0_0", 1280, 16, 16),
        ("down_2_1_0", 1280, 16, 16),
        ("mid_0_0", 1280, 8, 8),
        ("up_1_0_0", 1280, 16, 16),
        ("up_1_1_0", 1280, 16, 16),
        ("up_1_2_0", 1280, 16, 16),
        ("up_2_0_0", 640, 32, 32),
        ("up_2_1_0", 640, 32, 32),
        ("up_2_2_0", 640, 32, 32),
        ("up_3_0_0", 320, 64, 64),
        ("up_3_1_0", 320, 64, 64),
        ("up_3_2_0", 320, 64, 64),
    ]
}

/// Per-CFG-branch reference-attention scale. Identical to [`cfg_branch_table`].
pub fn ref_scale_for_cfg() -> [f32; CFG_BRANCHES] {
    cfg_branch_table().ref_scale
}

/// Dual-stream input is the 4-ch VAE reference latent, **not** the 12-ch
/// `[noise|normal|position]` pack used by the main UNet.
pub fn dual_input_channels() -> usize {
    DUAL_IN_CHANNELS
}

pub fn main_input_channels() -> usize {
    MAIN_IN_CHANNELS
}

/// Learned prompt for the write path is `learned_text_clip_ref` (77×1024),
/// repeated once per reference view. Not the albedo/mr tokens.
pub fn ref_prompt_tokens() -> usize {
    TEXT_TOKENS
}

/// Host-side cache the write path must fill before the first DDIM step.
#[derive(Clone, Debug, Default)]
pub struct RefTokenCache {
    /// Layer name → packed tokens `[n_ref * hw, channels]` (token-major).
    pub layers: Vec<(String, Vec<f32>, usize, usize)>,
}

impl RefTokenCache {
    pub fn insert(&mut self, name: &str, tokens: Vec<f32>, seq: usize, channels: usize) {
        debug_assert_eq!(tokens.len(), seq * channels);
        self.layers.push((name.to_string(), tokens, seq, channels));
    }

    pub fn get(&self, name: &str) -> Option<&[f32]> {
        self.layers
            .iter()
            .find(|(n, _, _, _)| n == name)
            .map(|(_, t, _, _)| t.as_slice())
    }

    pub fn is_complete(&self) -> bool {
        let expect: Vec<&str> = write_layer_names();
        expect.iter().all(|n| self.get(n).is_some()) && self.layers.len() == expect.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_names_match_official_init_attention() {
        let names = write_layer_names();
        assert_eq!(names.len(), 16);
        assert_eq!(names[0], "down_0_0_0");
        assert_eq!(names[6], "mid_0_0");
        assert_eq!(*names.last().unwrap(), "up_3_2_0");
        assert!(!names.iter().any(|n| n.starts_with("up_0_")));
        assert_eq!(write_layer_spatial_64().len(), names.len());
        for ((n, c, w, h), expect) in write_layer_spatial_64().iter().zip(names.iter()) {
            assert_eq!(n, expect);
            assert!(*c > 0 && *w > 0 && *h > 0);
        }
    }

    #[test]
    fn dual_is_4ch_main_is_12ch() {
        assert_eq!(dual_input_channels(), 4);
        assert_eq!(main_input_channels(), 12);
        assert_eq!(ref_scale_for_cfg(), [0.0, 1.0, 1.0]);
        assert_eq!(ref_prompt_tokens(), 77);
        assert_eq!(REF_TIMESTEP, 0.0);
    }

    #[test]
    fn cache_complete_only_with_every_layer() {
        let mut cache = RefTokenCache::default();
        assert!(!cache.is_complete());
        for (name, ch, w, h) in write_layer_spatial_64() {
            cache.insert(name, vec![0.0; w * h * ch], w * h, ch);
        }
        assert!(cache.is_complete());
        assert_eq!(cache.get("mid_0_0").unwrap().len(), 8 * 8 * 1280);
    }
}
