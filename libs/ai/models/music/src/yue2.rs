//! YuE2-3B native port: protocol, constants and the weight source.
//!
//! Release contract (protocol `yue2-native-v1`, m-a-p/YuE2-3B @ 8f24312f18,
//! reference package `yue2` 0.1.6 @ multimodal-art-projection/YuE 1647252d68):
//!
//! One 28-layer backbone with two parameter sets per layer ("AR-NAR mixture
//! of transformers"). Each set alone is a Qwen3 dense layer: hidden 2048,
//! 16 q / 8 kv heads, head_dim 128, q/k RMSNorm, rotate-half RoPE theta 1e6,
//! SwiGLU 6144, no biases. The AR set writes an ABC score (ordinary text
//! tokens) and then 25 Hz codec tokens; the NAR set turns the codec sequence
//! into 64-d latents by 32-step midpoint flow matching, attending to the AR
//! prefix keys/values; the Oobleck VAE decodes 48 kHz stereo.
//!
//! The token ids and the prompt text are the checkpoint contract: whitespace
//! in the assembled prompt changes the song.

use crate::{DiffusionError, Result};
use makepad_ai_common::sharded::ShardedSafetensors;
use std::path::{Path, PathBuf};

pub const YUE2_HIDDEN: usize = 2048;
pub const YUE2_LAYERS: usize = 28;
pub const YUE2_HEADS: usize = 16;
pub const YUE2_KV_HEADS: usize = 8;
pub const YUE2_HEAD_DIM: usize = 128;
pub const YUE2_FF: usize = 6144;
pub const YUE2_VOCAB: usize = 184_704;
pub const YUE2_RMS_EPS: f32 = 1e-6;
pub const YUE2_ROPE_THETA: f32 = 1_000_000.0;
pub const YUE2_CONTEXT: usize = 24_576;
pub const YUE2_LATENT_DIM: usize = 64;
pub const YUE2_MAX_LATENT_FRAMES: usize = 24_576;
pub const YUE2_TIME_FREQ_DIM: usize = 256;
pub const YUE2_TIMESTEP_SHIFT: f32 = 1.0;

pub const YUE2_SAMPLE_RATE: usize = 48_000;
pub const YUE2_LATENT_HOP: usize = 1920;
pub const YUE2_FRAME_RATE: f64 = 25.0;

pub const EOD: u32 = 151_643;
pub const ABC_START: u32 = 151_847;
pub const ABC_END: u32 = 151_848;
pub const MUSIC_START: u32 = 151_851;
pub const MUSIC_END: u32 = 151_852;
pub const CODEC_OFFSET: u32 = 151_853;
pub const CODEC_SIZE: u32 = 32_768;
pub const LATENT_START: u32 = 184_621;
pub const LATENT_END: u32 = 184_622;

pub const YUE2_ODE_STEPS: usize = 32;

/// Planning mode: which instruction the checkpoint was trained with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Yue2Cot {
    /// Chord-annotated ABC score, then codec tokens (release default).
    Full,
    /// Melody-only ABC (recommended for covers), then codec tokens.
    Melody,
    /// No symbolic plan.
    Off,
}

impl Yue2Cot {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "full" => Some(Self::Full),
            "melody" => Some(Self::Melody),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Melody => "melody",
            Self::Off => "off",
        }
    }

    pub fn instruction(self) -> &'static str {
        match self {
            Self::Off => "Generate music with codec tokens from the given conditions.",
            Self::Melody => "Generate a melody-only ABC transcription without chord symbols, then generate music with codec tokens from the given conditions.",
            Self::Full => "Generate a chord-annotated ABC transcription, then generate music with codec tokens from the given conditions.",
        }
    }

    /// Release text guidance: 1.0 (single branch) with a plan, 1.01 without.
    pub fn default_guidance(self) -> f32 {
        if self == Self::Off {
            1.01
        } else {
            1.0
        }
    }
}

/// One song request in the release protocol.
#[derive(Clone, Debug)]
pub struct Yue2Request {
    pub style: String,
    pub lyrics: String,
    pub cot: Yue2Cot,
    pub seed: u64,
    /// Supplied (possibly user-edited) ABC score: skips the planning stage.
    pub abc: Option<String>,
    pub cfg_scale: Option<f32>,
}

impl Yue2Request {
    pub fn guidance(&self) -> f32 {
        self.cfg_scale.unwrap_or_else(|| self.cot.default_guidance())
    }

    pub fn text(&self) -> String {
        format!(
            "{}\n[Tags]\n{}\n[Lyrics]\n{}\n",
            self.cot.instruction(),
            self.style,
            self.lyrics
        )
    }
}

/// Sampling preset for one AR phase (`protocol.Sampling`).
#[derive(Clone, Copy, Debug)]
pub struct Yue2Sampling {
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: usize,
    pub repetition_penalty: f32,
    pub penalty_window: usize,
    pub min_tokens: usize,
    pub max_tokens: usize,
}

impl Yue2Sampling {
    pub const ABC: Self = Self {
        temperature: 0.7,
        top_p: 0.9,
        top_k: 30,
        repetition_penalty: 1.005,
        penalty_window: 100,
        min_tokens: 32,
        max_tokens: 4096,
    };
    pub const SEMANTIC: Self = Self {
        temperature: 1.0,
        top_p: 0.95,
        top_k: 100,
        repetition_penalty: 1.2,
        penalty_window: 50,
        min_tokens: 200,
        max_tokens: 9000,
    };
    pub const GREEDY: Self = Self {
        temperature: 0.0,
        top_p: 1.0,
        top_k: 1,
        repetition_penalty: 1.0,
        penalty_window: 1,
        min_tokens: 0,
        max_tokens: 128,
    };
}

/// Which AR phase is being sampled: decides the allowed vocabulary and the
/// end token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Yue2Phase {
    Abc,
    Semantic,
}

impl Yue2Phase {
    pub fn end_token(self) -> u32 {
        match self {
            Self::Abc => ABC_END,
            Self::Semantic => MUSIC_END,
        }
    }

    /// Contiguous lm_head row window that covers every allowed token
    /// (`[lo, lo + len)`); rows inside it that are not allowed are masked.
    pub fn head_rows(self) -> (u32, u32) {
        match self {
            Self::Abc => (0, ABC_END + 1),
            Self::Semantic => (MUSIC_END, CODEC_OFFSET + CODEC_SIZE - MUSIC_END),
        }
    }

    /// Whether a full-vocab id may be sampled in this phase.
    pub fn allowed(self, id: u32) -> bool {
        match self {
            Self::Abc => id < EOD || id == ABC_END,
            Self::Semantic => id == MUSIC_END || (CODEC_OFFSET..CODEC_OFFSET + CODEC_SIZE).contains(&id),
        }
    }
}

/// `token_prefixes`: the AR prefix that precedes the generated tokens.
/// `abc_ids` = None with no supplied score -> ABC planning prefix.
pub fn yue2_prefix(text_ids: &[u32], cot: Yue2Cot, abc_ids: Option<&[u32]>) -> Vec<u32> {
    let mut out = Vec::with_capacity(text_ids.len() + abc_ids.map_or(0, |a| a.len()) + 4);
    out.push(EOD);
    out.extend_from_slice(text_ids);
    match (cot, abc_ids) {
        (Yue2Cot::Off, _) => out.extend_from_slice(&[ABC_START, ABC_END, MUSIC_START]),
        (_, None) => out.push(ABC_START),
        (_, Some(abc)) => {
            out.push(ABC_START);
            out.extend_from_slice(abc);
            out.extend_from_slice(&[ABC_END, MUSIC_START]);
        }
    }
    out
}

/// `negative_prefix`: the CFG branch (instruction only, same exact ABC ids).
pub fn yue2_negative_prefix(instruction_ids: &[u32], cot: Yue2Cot, abc_ids: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(instruction_ids.len() + abc_ids.len() + 4);
    out.push(EOD);
    out.extend_from_slice(instruction_ids);
    if cot == Yue2Cot::Off {
        out.push(MUSIC_START);
    } else {
        out.push(ABC_START);
        out.extend_from_slice(abc_ids);
        out.extend_from_slice(&[ABC_END, MUSIC_START]);
    }
    out
}

/// `chunk_ranges`: original acoustic context chunks `[a, b)` over codec frames.
pub fn yue2_chunk_ranges(frames: usize, prefix_tokens: usize) -> Vec<(usize, usize)> {
    let size = (YUE2_CONTEXT.saturating_sub(prefix_tokens + 3) / 2).min(YUE2_CONTEXT);
    if frames == 0 || size == 0 {
        return Vec::new();
    }
    (0..frames)
        .step_by(size)
        .map(|a| (a, (a + size).min(frames)))
        .collect()
}

/// Song length implied by an ABC score (first `Q:` tempo, first `M:`
/// meter, bars of the first voice, `Z<n>` multi-bar rests counted as n).
/// Used for honest progress/ETA of the codec stage (25 tokens per second);
/// `None` when the score carries no countable bars.
pub fn yue2_abc_seconds(abc: &str) -> Option<f64> {
    let mut meter = (4.0f64, 4.0f64);
    let mut tempo: Option<(f64, f64)> = None; // (unit as a fraction of a whole, bpm)
    let mut first_voice: Option<String> = None;
    let mut in_first = false;
    let mut header_done = false;
    let mut bars = 0.0f64;
    let frac = |t: &str| -> Option<f64> {
        let (a, b) = t.trim().split_once('/')?;
        Some(a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok()?)
    };
    for raw in abc.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('%') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("M:") {
            if !header_done {
                if let Some((a, b)) = rest.trim().split_once('/') {
                    if let (Ok(a), Ok(b)) = (a.trim().parse(), b.trim().parse()) {
                        meter = (a, b);
                    }
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("Q:") {
            if tempo.is_none() {
                if let Some((unit, bpm)) = rest.split_once('=') {
                    if let (Some(u), Ok(b)) = (frac(unit), bpm.trim().parse::<f64>()) {
                        tempo = Some((u, b));
                    }
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("V:") {
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            if rest.contains('=') {
                // A voice declaration in the header.
                if first_voice.is_none() {
                    first_voice = Some(name);
                }
                continue;
            }
            header_done = true;
            in_first = first_voice.as_deref().map_or(true, |f| f == name);
            continue;
        }
        if line.len() >= 2 && line.as_bytes()[1] == b':' && line.as_bytes()[0].is_ascii_alphabetic() {
            continue; // other header fields (X:, T:, L:, K:)
        }
        if !in_first && first_voice.is_some() {
            continue;
        }
        header_done = true;
        let bytes = line.as_bytes();
        let mut i = 0;
        let mut in_chord = false;
        while i < bytes.len() {
            match bytes[i] {
                b'"' => in_chord = !in_chord,
                b'|' if !in_chord => {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                        i += 1;
                    }
                    bars += 1.0;
                }
                b'Z' if !in_chord => {
                    let mut j = i + 1;
                    while j < bytes.len() && bytes[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j > i + 1 {
                        if let Ok(n) = std::str::from_utf8(&bytes[i + 1..j]).unwrap_or("1").parse::<f64>() {
                            bars += n - 1.0;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
    let (unit, bpm) = tempo.unwrap_or((0.25, 120.0));
    if bars <= 0.0 || bpm <= 0.0 || unit <= 0.0 {
        return None;
    }
    Some(bars * (meter.0 / meter.1) / unit * 60.0 / bpm)
}

pub const YUE2_NAMESPACE: &str = "yue2";
const LABEL: &str = "yue2";

/// The MoT checkpoint (one bf16 safetensors file).
pub struct Yue2Weights {
    pub dir: PathBuf,
    shards: ShardedSafetensors,
}

impl Yue2Weights {
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let file = dir.join("model.safetensors");
        let shards = ShardedSafetensors::load(&file, LABEL)?;
        for name in ["model.embed_tokens.weight", "lm_head.weight", "llm2vae.weight", "vae2llm.weight"] {
            if !shards.has_tensor(name) {
                return Err(DiffusionError::model(format!(
                    "yue2 {}: missing tensor {name} (not a YuE2-3B checkpoint)",
                    file.display()
                )));
            }
        }
        Ok(Self { dir, shards })
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        self.shards.has_tensor(name)
    }

    pub fn tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        self.shards.tensor_bytes(name, LABEL)
    }

    pub fn tensor_row_range_bytes(&self, name: &str, row0: u64, nrows: u64) -> Result<Vec<u8>> {
        self.shards.tensor_row_range_bytes(name, row0, nrows, LABEL)
    }

    pub fn tensor_row_f32(&self, name: &str, row: u64) -> Result<Vec<f32>> {
        self.shards.tensor_row_f32(name, row, LABEL)
    }

    pub fn tensor_f32(&self, name: &str) -> Result<Vec<f32>> {
        self.shards.tensor_f32(name, LABEL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_follow_protocol() {
        let text = [1u32, 2, 3];
        assert_eq!(yue2_prefix(&text, Yue2Cot::Full, None), vec![EOD, 1, 2, 3, ABC_START]);
        assert_eq!(
            yue2_prefix(&text, Yue2Cot::Melody, Some(&[9, 8])),
            vec![EOD, 1, 2, 3, ABC_START, 9, 8, ABC_END, MUSIC_START]
        );
        assert_eq!(
            yue2_prefix(&text, Yue2Cot::Off, None),
            vec![EOD, 1, 2, 3, ABC_START, ABC_END, MUSIC_START]
        );
        assert_eq!(yue2_negative_prefix(&[7], Yue2Cot::Off, &[]), vec![EOD, 7, MUSIC_START]);
    }

    #[test]
    fn abc_duration_counts_first_voice_bars() {
        let abc = "X:1\nM:4/4\nL:1/32\nQ:1/4=120\nV: Vocal clef=treble name=\"V\"\nV: Ins clef=treble\nK:C\nV: Vocal\nZ|\"Am\"z32|c32|\nV: Ins\nZ4|\nV: Vocal\nZ2|\n";
        // Vocal: Z (1) + 2 bars + Z2 (2) = 5 bars of 4/4 at 120 bpm = 10 s.
        assert_eq!(yue2_abc_seconds(abc), Some(10.0));
        assert_eq!(yue2_abc_seconds("no bars here"), None);
    }

    #[test]
    fn chunks_match_reference() {
        // size = (24576 - 1000 - 3) / 2 = 11786
        assert_eq!(yue2_chunk_ranges(5400, 1000), vec![(0, 5400)]);
        assert_eq!(yue2_chunk_ranges(12000, 1000), vec![(0, 11786), (11786, 12000)]);
    }

    #[test]
    fn phase_windows_cover_allowed_ids() {
        let (lo, n) = Yue2Phase::Semantic.head_rows();
        assert_eq!(lo, MUSIC_END);
        assert_eq!(lo + n, CODEC_OFFSET + CODEC_SIZE);
        assert!(Yue2Phase::Semantic.allowed(MUSIC_END));
        assert!(!Yue2Phase::Semantic.allowed(MUSIC_START));
        assert!(Yue2Phase::Abc.allowed(ABC_END) && !Yue2Phase::Abc.allowed(EOD));
    }
}
