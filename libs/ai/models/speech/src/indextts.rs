//! IndexTTS-2.5 (bilibili IndexTeam) — shared constants, weight paths and
//! small helpers for the port. The pipeline stages live in sibling modules:
//!
//! - `indextts_tokenizer` — whisper-style tiktoken BPE (text -> ids)
//! - `indextts_w2v`       — w2v-bert-2.0 conformer feature encoder (+ the
//!                          SeamlessM4T fbank front-end) and CAMPPlus
//! - `indextts_gpt`       — the 0.8B GPT-2 AR (text+conds -> semantic codes),
//!                          incl. the emotion conformer/perceiver path
//! - `indextts_codec`     — semantic codec decoder (codes -> 1024-d features)
//! - `indextts_s2mel`     — length regulator + flow-matching DiT + WaveNet
//!                          post-net (features -> 80-mel @ 22.05 kHz)
//! - `indextts_bigvgan`   — BigVGAN v2 vocoder (mel -> waveform) + the
//!                          HiFiGAN-style mel front-end for the voice prompt
//! - `indextts_pipeline`  — end-to-end assembly, emotion-vector mixing,
//!                          progress/cancel
//!
//! Everything is CPU f32 in the crate's established style (`sa3::linear`,
//! `sa3::par_rows`, plain `Vec<f32>` rows). Oracle dumps for stage validation
//! live in `local/indextts_ref/dumps/` (see `local/agent_state/indextts-port.md`);
//! the `indextts_*_validate` bins compare against them.

use std::path::PathBuf;

/// Sample rate of the generated waveform.
pub const INDEXTTS_SAMPLE_RATE: u32 = 22_050;

/// GPT backbone (config.yaml `gpt`).
pub const GPT_LAYERS: usize = 24;
pub const GPT_DIM: usize = 1280;
pub const GPT_HEADS: usize = 20;
pub const GPT_TEXT_VOCAB: usize = 60_509 + 1; // number_text_tokens + 1
pub const GPT_MEL_VOCAB: usize = 8194;
pub const GPT_START_MEL: u32 = 8192;
pub const GPT_STOP_MEL: u32 = 8193;
pub const GPT_STOP_TEXT: u32 = 1;
pub const GPT_START_TEXT: u32 = 0;
pub const GPT_MAX_MEL_TOKENS: usize = 1815;
pub const GPT_MAX_TEXT_TOKENS: usize = 600;

/// w2v-bert-2.0 encoder (hidden_states[17] is consumed -> only 17 layers run).
pub const W2V_LAYERS_USED: usize = 17;
pub const W2V_DIM: usize = 1024;
pub const W2V_HEADS: usize = 16;

/// s2mel (config.yaml `s2mel`).
pub const S2MEL_DIT_DEPTH: usize = 13;
pub const S2MEL_DIT_DIM: usize = 512;
pub const S2MEL_DIT_HEADS: usize = 8;
pub const S2MEL_MELS: usize = 80;
pub const S2MEL_STYLE_DIM: usize = 192;
/// mel frames per semantic frame (target_lengths = codes*2 * 1.72 * duration).
pub const S2MEL_LEN_FACTOR: f64 = 1.72;
pub const S2MEL_CFM_STEPS: usize = 25;
pub const S2MEL_CFG_RATE: f32 = 0.7;

/// The 8 emotion-vector slots, wire order.
pub const EMOTION_NAMES: [&str; 8] = [
    "happy", "angry", "sad", "afraid", "disgusted", "melancholic", "surprised", "calm",
];
/// UI de-emphasis bias applied before the sum<=0.8 clamp (reference
/// `normalize_emo_vec`).
pub const EMOTION_BIAS: [f32; 8] = [0.9375, 0.875, 1.0, 1.0, 0.9375, 0.9375, 0.6875, 0.5625];
/// Per-category row counts of the feat1/feat2 matrices (config `emo_num`).
pub const EMOTION_CATEGORY_ROWS: [usize; 8] = [3, 17, 2, 8, 4, 5, 10, 24];

/// The reference-checkout checkpoints dir used by tests and validate bins:
/// `$INDEXTTS_CHECKPOINTS` when set, else `local/indextts_ref/checkpoints`
/// relative to the repo root (resolved from this crate's manifest dir).
pub fn reference_checkpoints_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("INDEXTTS_CHECKPOINTS") {
        return PathBuf::from(dir);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/indextts_ref/checkpoints")
}

/// The oracle dumps dir (`$INDEXTTS_DUMPS` or `local/indextts_ref/dumps`).
pub fn reference_dumps_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("INDEXTTS_DUMPS") {
        return PathBuf::from(dir);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/indextts_ref/dumps")
}

/// Applies the reference's user-facing emotion-vector normalization:
/// per-slot bias, then rescale so the sum stays <= 0.8.
pub fn normalize_emotion_vector(raw: &[f32; 8]) -> [f32; 8] {
    let mut v = [0f32; 8];
    for i in 0..8 {
        v[i] = raw[i] * EMOTION_BIAS[i];
    }
    let sum: f32 = v.iter().sum();
    if sum > 0.8 {
        let scale = 0.8 / sum;
        for value in &mut v {
            *value *= scale;
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emotion_normalization_matches_reference() {
        // Reference: normalize_emo_vec([0,0,0.8,0,...]) == 0.8 (sad bias 1.0,
        // sum 0.8 not above the cap).
        let v = normalize_emotion_vector(&[0.0, 0.0, 0.8, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!((v[2] - 0.8).abs() < 1e-6);
        // All-ones: biased then scaled to sum exactly 0.8.
        let v = normalize_emotion_vector(&[1.0; 8]);
        let sum: f32 = v.iter().sum();
        assert!((sum - 0.8).abs() < 1e-5);
    }

    #[test]
    fn category_rows_sum_to_matrix_height() {
        assert_eq!(EMOTION_CATEGORY_ROWS.iter().sum::<usize>(), 73);
    }

    /// Every IndexTTS checkpoint parses through the nested pth reader
    /// (skipped when the reference checkout is absent).
    #[test]
    fn checkpoints_parse_through_nested_pth_reader() {
        use crate::torch_pth::PthStateDict;
        let dir = reference_checkpoints_dir();
        if !dir.join("gpt.pth").is_file() {
            eprintln!("skipping checkpoints_parse_through_nested_pth_reader: {dir:?} missing");
            return;
        }
        for (file, probe, want_shape) in [
            ("gpt.pth", "spk_emb_proj.weight", vec![1280, 192]),
            (
                "s2mel.pth",
                "net.cfm.estimator.x_embedder.weight_g",
                vec![512, 1],
            ),
            (
                "codec.pth",
                "model.quantizer.quantizers.0.codebook.weight",
                vec![8192, 8],
            ),
            (
                "hf_cache/campplus_cn_common.bin",
                "head.conv1.weight",
                vec![32, 1, 3, 3],
            ),
            (
                "hf_cache/bigvgan/bigvgan_generator.pt",
                "generator.conv_pre.weight_g",
                vec![1536, 1, 1],
            ),
        ] {
            let mut sd = PthStateDict::load_nested(dir.join(file)).unwrap_or_else(|e| {
                panic!("{file}: {e:?}");
            });
            let shape = sd.shape(probe).unwrap_or_else(|e| {
                let mut names: Vec<_> = sd.names().take(12).cloned().collect();
                names.sort();
                panic!("{file}: {e:?} (first names: {names:?})");
            });
            assert_eq!(shape, want_shape, "{file} {probe}");
            let data = sd.f32(probe).unwrap();
            assert_eq!(data.len(), want_shape.iter().product::<usize>());
            assert!(data.iter().all(|v| v.is_finite()), "{file} {probe} non-finite");
        }
        // feat1/feat2: bare-tensor roots.
        let (shape, data) = PthStateDict::load_single_tensor(dir.join("feat1.pt")).unwrap();
        assert_eq!(shape, vec![73, 192]);
        assert_eq!(data.len(), 73 * 192);
        let (shape, _) = PthStateDict::load_single_tensor(dir.join("feat2.pt")).unwrap();
        assert_eq!(shape, vec![73, 1280]);
    }
}
