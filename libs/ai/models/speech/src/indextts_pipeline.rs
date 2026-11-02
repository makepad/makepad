//! IndexTTS-2.5 end-to-end pipeline assembly.
//!
//! This file owns everything BETWEEN the ported model stages: text
//! preprocessing + segmentation, reference-audio resampling, the emotion
//! vector plumbing, wav stitching, seeded noise, progress/cancel. The model
//! stages themselves live in the sibling `indextts_*` modules (see
//! `indextts.rs` for the map) and are assembled by [`IndexTtsPipeline`]
//! once loaded.
//!
//! Text normalization scope (v1, documented gap): the reference runs a full
//! WeText/NeMo normalizer (dates, currency, addresses). This port applies
//! the reference's `char_rep_map` punctuation folding, lowercases (EN), and
//! spells out plain numbers — enough for game/NPC dialogue lines; complex
//! written forms (e.g. "$3.5M", "01/02/2026") will be read literally.

use crate::error::{DiffusionError, Result};
use crate::indextts::{
    normalize_emotion_vector, GPT_START_MEL, GPT_STOP_MEL, INDEXTTS_SAMPLE_RATE, S2MEL_LEN_FACTOR,
};
use crate::indextts_bigvgan::IndexTtsBigVgan;
use crate::indextts_campplus::{campplus_fbank, CampPlus};
use crate::indextts_codec::SemanticCodecDecoder;
use crate::indextts_gpt::{gpt_cuda_available, EmotionMatrices, GptSamplingConfig, IndexTtsGpt};
use crate::indextts_mel::mel_spectrogram_22k;
use crate::indextts_s2mel::{S2mel, S2melNoiseSource, S2melSeededNoise};
use crate::indextts_tokenizer::IndexTtsTokenizer;
use crate::indextts_w2v::{extract_w2v_features, W2vBertEncoder};
use crate::{emit_progress, hook_ref, BoxedProgressHook, ProgressHook};
use std::path::{Path, PathBuf};

/// The reference's `char_rep_map`: punctuation folding applied before
/// tokenization (order matters for the multi-char entries — longest first).
const CHAR_REPLACEMENTS: &[(&str, &str)] = &[
    ("，，，", "…"),
    ("……", "…"),
    ("...", "…"),
    (",,,", "…"),
    ("：", ","),
    ("；", ","),
    (";", ","),
    ("，", ","),
    ("。", "."),
    ("！", "!"),
    ("？", "?"),
    ("\n", " "),
    ("·", "-"),
    ("、", ","),
    ("“", "'"),
    ("”", "'"),
    ("\"", "'"),
    ("‘", "'"),
    ("’", "'"),
    ("（", "'"),
    ("）", "'"),
    ("(", "'"),
    (")", "'"),
    ("《", "'"),
    ("》", "'"),
    ("【", "'"),
    ("】", "'"),
    ("[", "'"),
    ("]", "'"),
    ("—", "-"),
    ("～", "-"),
    ("~", "-"),
    ("「", "'"),
    ("」", "'"),
    (":", ","),
];

/// Punctuation the segmenter may break after (reference `split_text_by_tokens`).
const SEGMENT_BREAKS: &[char] = &[
    '，', '。', '！', '？', '、', '；', '：', ',', '.', '!', '?', ';', ':', '\n',
];

/// Folds punctuation, spells numbers, lowercases: the EN preprocessing used
/// before the tokenizer. (The reference's full normalizer is wetext; see the
/// module doc for the v1 scope.)
pub fn preprocess_text_en(text: &str) -> String {
    let mut text = text.to_string();
    for (from, to) in CHAR_REPLACEMENTS {
        text = text.replace(from, to);
    }
    let text = spell_numbers(&text);
    // Collapse runs of spaces introduced by the folding.
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for c in text.trim().chars() {
        let is_space = c == ' ';
        if !(is_space && last_space) {
            out.push(c);
        }
        last_space = is_space;
    }
    out.to_lowercase()
}

/// Splits preprocessed text into segments of at most `max_tokens` tokenizer
/// tokens (the reference default is 120 per segment), breaking at
/// punctuation and greedily packing. The `lang_prefix` budget is reserved.
pub fn segment_text(
    tokenizer: &IndexTtsTokenizer,
    text: &str,
    lang_prefix: &str,
    max_tokens: usize,
) -> Vec<String> {
    let budget = max_tokens
        .saturating_sub(tokenizer.encode(lang_prefix).len())
        .max(1);
    let token_len = |s: &str| tokenizer.encode(s).len();
    if token_len(text) <= budget {
        return vec![text.to_string()];
    }
    // Split keeping the delimiter attached to the preceding piece.
    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        current.push(c);
        if SEGMENT_BREAKS.contains(&c) {
            pieces.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    // Oversized single pieces split by character.
    let mut chunks: Vec<String> = Vec::new();
    for piece in pieces {
        if token_len(&piece) <= budget {
            chunks.push(piece);
            continue;
        }
        let mut current = String::new();
        for c in piece.chars() {
            let mut with = current.clone();
            with.push(c);
            if !current.is_empty() && token_len(&with) > budget {
                chunks.push(std::mem::take(&mut current));
                current.push(c);
            } else {
                current = with;
            }
        }
        if !current.is_empty() {
            chunks.push(current);
        }
    }
    // Greedy packing.
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    for chunk in chunks {
        let mut with = current.clone();
        with.push_str(&chunk);
        if !current.is_empty() && token_len(&with) > budget {
            segments.push(std::mem::take(&mut current));
            current = chunk;
        } else {
            current = with;
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    if segments.is_empty() {
        segments.push(text.to_string());
    }
    segments
}

// ---------------------------------------------------------------------------
// Number spelling (v1 English normalization)
// ---------------------------------------------------------------------------

const ONES: [&str; 20] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

fn spell_under_1000(n: u64, out: &mut String) {
    if n >= 100 {
        out.push_str(ONES[(n / 100) as usize]);
        out.push_str(" hundred");
        if n % 100 != 0 {
            out.push(' ');
        }
    }
    let rem = n % 100;
    if rem >= 20 {
        out.push_str(TENS[(rem / 10) as usize]);
        if rem % 10 != 0 {
            out.push(' ');
            out.push_str(ONES[(rem % 10) as usize]);
        }
    } else if rem > 0 || n == 0 {
        out.push_str(ONES[rem as usize]);
    }
}

/// Spells a non-negative integer ("1234" -> "one thousand two hundred
/// thirty four"). Numbers too large to read naturally (> 15 digits) are
/// spelled digit by digit.
pub fn spell_integer(digits: &str) -> String {
    let n: u64 = match digits.parse() {
        Ok(n) => n,
        Err(_) => {
            // Digit-by-digit fallback (overflow / leading zeros beyond u64).
            return digits
                .chars()
                .filter_map(|c| c.to_digit(10))
                .map(|d| ONES[d as usize])
                .collect::<Vec<_>>()
                .join(" ");
        }
    };
    // Leading zeros read digit by digit ("007" -> "zero zero seven").
    if digits.len() > 1 && digits.starts_with('0') {
        return digits
            .chars()
            .filter_map(|c| c.to_digit(10))
            .map(|d| ONES[d as usize])
            .collect::<Vec<_>>()
            .join(" ");
    }
    let mut out = String::new();
    let scales: [(u64, &str); 4] = [
        (1_000_000_000_000, "trillion"),
        (1_000_000_000, "billion"),
        (1_000_000, "million"),
        (1_000, "thousand"),
    ];
    let mut rest = n;
    for (scale, name) in scales {
        if rest >= scale {
            spell_under_1000(rest / scale, &mut out);
            out.push(' ');
            out.push_str(name);
            rest %= scale;
            if rest != 0 {
                out.push(' ');
            }
        }
    }
    if rest != 0 || n == 0 {
        spell_under_1000(rest, &mut out);
    }
    out
}

/// Replaces number runs in text with spelled-out words. Decimals are read as
/// "<int> point <digit> <digit>"; digit groups separated by commas inside a
/// number are joined first ("1,234" -> 1234).
pub fn spell_numbers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() * 2);
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // Scan the whole number: digits, thousands commas, one decimal point.
        let mut int_part = String::new();
        let mut frac_part = String::new();
        let mut in_frac = false;
        while i < chars.len() {
            let c = chars[i];
            if c.is_ascii_digit() {
                if in_frac {
                    frac_part.push(c);
                } else {
                    int_part.push(c);
                }
                i += 1;
            } else if !in_frac
                && c == ','
                && i + 3 < chars.len()
                && chars[i + 1].is_ascii_digit()
                && chars[i + 2].is_ascii_digit()
                && chars[i + 3].is_ascii_digit()
                && (i + 4 >= chars.len() || !chars[i + 4].is_ascii_digit())
            {
                i += 1; // thousands separator
            } else if !in_frac
                && c == '.'
                && i + 1 < chars.len()
                && chars[i + 1].is_ascii_digit()
            {
                in_frac = true;
                i += 1;
            } else {
                break;
            }
        }
        out.push_str(&spell_integer(&int_part));
        if !frac_part.is_empty() {
            out.push_str(" point");
            for d in frac_part.chars().filter_map(|c| c.to_digit(10)) {
                out.push(' ');
                out.push_str(ONES[d as usize]);
            }
        }
        // Keep a space between the spelled number and a following word.
        if i < chars.len() && chars[i].is_alphabetic() {
            out.push(' ');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Audio helpers
// ---------------------------------------------------------------------------

/// Shared with the Music3 reference-clip path; the algorithm lives in
/// `makepad_ai_common::audio`.
pub use makepad_ai_common::audio::resample_mono;

/// Concatenates per-segment waveforms with `silence_ms` of silence between
/// them (reference `insert_interval_silence`, default 200 ms).
pub fn stitch_segments(segments: &[Vec<f32>], sample_rate: u32, silence_ms: u32) -> Vec<f32> {
    let gap = (sample_rate as usize * silence_ms as usize) / 1000;
    let total: usize =
        segments.iter().map(|s| s.len()).sum::<usize>() + gap * segments.len().saturating_sub(1);
    let mut out = Vec::with_capacity(total);
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            out.extend(std::iter::repeat(0f32).take(gap));
        }
        out.extend_from_slice(segment);
    }
    out
}

/// Trims reference audio to the pipeline's 15 s cap (reference
/// `_load_and_cut_audio`).
pub fn cut_reference(samples: &[f32], sample_rate: u32) -> &[f32] {
    let max = 15 * sample_rate as usize;
    &samples[..samples.len().min(max)]
}

// ---------------------------------------------------------------------------
// Weight layout
// ---------------------------------------------------------------------------

/// Absolute paths to every weight file the pipeline loads. Two shipped
/// layouts exist; anything else can fill the struct by hand.
#[derive(Clone, Debug)]
pub struct IndexTtsWeightPaths {
    /// Directory holding `gpt.pth` + `feat1.pt`/`feat2.pt` (the GPT and
    /// emotion-matrix loaders take a directory).
    pub model_dir: PathBuf,
    pub s2mel: PathBuf,
    pub codec: PathBuf,
    pub tiktoken: PathBuf,
    pub w2v_safetensors: PathBuf,
    pub w2v_stats: PathBuf,
    pub campplus: PathBuf,
    pub bigvgan: PathBuf,
}

impl IndexTtsWeightPaths {
    /// The reference checkout layout (`local/indextts_ref/checkpoints`):
    /// flat top level + HF pulls under `hf_cache/`.
    pub fn reference_layout(checkpoints_dir: &Path) -> Self {
        let d = checkpoints_dir;
        Self {
            model_dir: d.to_path_buf(),
            s2mel: d.join("s2mel.pth"),
            codec: d.join("codec.pth"),
            tiktoken: d.join("multilingual_zh_ja_yue_char_del.tiktoken"),
            w2v_safetensors: d.join("hf_cache/w2v-bert-2.0/model.safetensors"),
            w2v_stats: d.join("wav2vec2bert_stats.pt"),
            campplus: d.join("hf_cache/campplus_cn_common.bin"),
            bigvgan: d.join("hf_cache/bigvgan/bigvgan_generator.pt"),
        }
    }

    /// The ai_content service cache layout (registry `cache_as` paths): flat
    /// `indextts/` with only the w2v safetensors nested one level.
    pub fn service_layout(indextts_cache_dir: &Path) -> Self {
        let d = indextts_cache_dir;
        Self {
            model_dir: d.to_path_buf(),
            s2mel: d.join("s2mel.pth"),
            codec: d.join("codec.pth"),
            tiktoken: d.join("multilingual_zh_ja_yue_char_del.tiktoken"),
            w2v_safetensors: d.join("w2v-bert-2.0/model.safetensors"),
            w2v_stats: d.join("wav2vec2bert_stats.pt"),
            campplus: d.join("campplus_cn_common.bin"),
            bigvgan: d.join("bigvgan_generator.pt"),
        }
    }

    /// Every expected file that is absent — loaders would fail one at a time;
    /// callers use this for a single complete error up front.
    pub fn missing_files(&self) -> Vec<PathBuf> {
        let mut files = vec![
            self.model_dir.join("gpt.pth"),
            self.model_dir.join("feat1.pt"),
            self.model_dir.join("feat2.pt"),
        ];
        files.extend(
            [
                &self.s2mel,
                &self.codec,
                &self.tiktoken,
                &self.w2v_safetensors,
                &self.w2v_stats,
                &self.campplus,
                &self.bigvgan,
            ]
            .into_iter()
            .cloned(),
        );
        files.into_iter().filter(|p| !p.is_file()).collect()
    }
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// All eight model stages loaded (~7 GB host f32).
pub struct IndexTtsPipeline {
    pub tokenizer: IndexTtsTokenizer,
    pub w2v: W2vBertEncoder,
    pub campplus: CampPlus,
    pub gpt: IndexTtsGpt,
    pub emotion: EmotionMatrices,
    pub codec: SemanticCodecDecoder,
    pub s2mel: S2mel,
    pub bigvgan: IndexTtsBigVgan,
}

/// Precomputed conditioning for one reference voice — everything `synthesize`
/// needs that depends only on the reference clip. ~1 MB; cacheable per voice.
pub struct IndexTtsVoice {
    /// Normalized w2v-bert hidden\[17\], `spk_frames x 1024` (50 Hz).
    pub spk_cond_emb: Vec<f32>,
    pub spk_frames: usize,
    /// Reference mel, channel-major `80 x mel_frames` (86.13 Hz).
    pub ref_mel: Vec<f32>,
    pub mel_frames: usize,
    /// CAMPPlus speaker embedding `[192]`.
    pub style: Vec<f32>,
    /// Length-regulated speaker condition, `mel_frames x 512`.
    pub prompt_condition: Vec<f32>,
    /// Emotion latent of the reference clip itself `[1280]` (the neutral /
    /// no-emotion-vector operating point).
    pub emovec_ref: Vec<f32>,
}

/// Per-call synthesis knobs. `Default` is the reference production setup.
#[derive(Clone, Debug)]
pub struct IndexTtsSynthesisParams {
    /// Raw 8-dim emotion vector `[happy, angry, sad, afraid, disgusted,
    /// melancholic, surprised, calm]`; bias + 0.8 sum-cap normalization is
    /// applied inside. None = neutral (the reference clip's own emotion).
    pub emotion: Option<[f32; 8]>,
    /// 1.0 = natural pace; internally `duration_factor = (1/speed).clamp(0.5, 2.0)`.
    pub speed: f32,
    /// GPT sampling config; its `seed` also seeds the flow-matching noise.
    /// Per-segment GPT seeds are decorrelated from it deterministically.
    pub sampling: GptSamplingConfig,
    /// Text budget per GPT call (reference `max_text_tokens_per_segment`).
    pub max_segment_tokens: usize,
    /// Silence stitched between segments.
    pub silence_ms: u32,
}

impl Default for IndexTtsSynthesisParams {
    fn default() -> Self {
        Self {
            emotion: None,
            speed: 1.0,
            sampling: GptSamplingConfig::default(),
            max_segment_tokens: 120,
            silence_ms: 200,
        }
    }
}

/// `int(s_frames * 1.72 * duration_factor)` — Python semantics. Uses the f64
/// literal 1.72 (== Python's float) so truncation lands on the same side at
/// exact-multiple boundaries (e.g. 25 codec frames -> 42, not 43).
pub fn cfm_target_len(s_frames: usize, duration_factor: f64) -> usize {
    (s_frames as f64 * S2MEL_LEN_FACTOR * duration_factor) as usize
}

/// Reference `duration_factor`: reciprocal speed, clamped to \[0.5, 2.0\].
pub fn duration_factor(speed: f32) -> f64 {
    if !(speed > 0.0) {
        return 1.0;
    }
    (1.0 / speed as f64).clamp(0.5, 2.0)
}

/// splitmix64 finalizer — decorrelates per-segment GPT seeds derived from the
/// user seed (seed+i alone would give near-identical sampling streams).
fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Wraps the caller hook so a callee's `[0,1]` fraction lands in the
/// `[base, base+span]` slice of segment `seg`'s `1/nseg` band, and labels
/// gain a ` (seg i/k)` suffix. Single-segment calls keep bare labels.
fn seg_band<'a>(
    progress: &'a mut Option<ProgressHook<'_>>,
    seg: usize,
    nseg: usize,
    base: f64,
    span: f64,
) -> Option<BoxedProgressHook<'a>> {
    progress.as_mut().map(|hook| -> BoxedProgressHook<'a> {
        Box::new(move |label: &str, fraction: f64| {
            let local = base + fraction.clamp(0.0, 1.0) * span;
            let overall = (seg as f64 + local) / nseg as f64;
            if nseg > 1 {
                hook(&format!("{label} (seg {}/{nseg})", seg + 1), overall)
            } else {
                hook(label, overall)
            }
        })
    })
}

impl IndexTtsPipeline {
    /// Loads all stages. Progress fractions are byte-weighted per component;
    /// each label fires before its (blocking) load.
    pub fn load(paths: &IndexTtsWeightPaths, mut progress: Option<ProgressHook>) -> Result<Self> {
        let missing = paths.missing_files();
        if !missing.is_empty() {
            return Err(DiffusionError::model(format!(
                "indextts weights missing: {}",
                missing
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        emit_progress(&mut progress, "load gpt", 0.0)?;
        let gpt = IndexTtsGpt::load(&paths.model_dir)?;
        emit_progress(&mut progress, "load w2v-bert", 0.46)?;
        let w2v = W2vBertEncoder::load_paths(&paths.w2v_safetensors, &paths.w2v_stats)?;
        emit_progress(&mut progress, "load codec", 0.79)?;
        let codec = SemanticCodecDecoder::load(&paths.codec)?;
        emit_progress(&mut progress, "load bigvgan", 0.88)?;
        let bigvgan = IndexTtsBigVgan::load(&paths.bigvgan)?;
        emit_progress(&mut progress, "load s2mel", 0.94)?;
        let s2mel = S2mel::load(&paths.s2mel)?;
        emit_progress(&mut progress, "load campplus", 0.99)?;
        let campplus = CampPlus::load_path(&paths.campplus)?;
        let emotion = EmotionMatrices::load(&paths.model_dir)?;
        let tokenizer = IndexTtsTokenizer::load(&paths.tiktoken)?;
        emit_progress(&mut progress, "weights ready", 1.0)?;
        Ok(Self {
            tokenizer,
            w2v,
            campplus,
            gpt,
            emotion,
            codec,
            s2mel,
            bigvgan,
        })
    }

    /// Reference-voice conditioning from a mono clip at any sample rate:
    /// 15 s cut -> 22.05 k -> 16 k (the reference resample chain), then the
    /// w2v / mel / campplus / length-regulator / emotion stages.
    pub fn prepare_voice(
        &self,
        samples: &[f32],
        sample_rate: u32,
        mut progress: Option<ProgressHook>,
    ) -> Result<IndexTtsVoice> {
        if sample_rate == 0 || samples.is_empty() {
            return Err(DiffusionError::model(
                "indextts reference clip is empty",
            ));
        }
        emit_progress(&mut progress, "voice resample", 0.0)?;
        let cut = cut_reference(samples, sample_rate);
        let audio_22k = resample_mono(cut, sample_rate, INDEXTTS_SAMPLE_RATE);
        let audio_16k = resample_mono(&audio_22k, INDEXTTS_SAMPLE_RATE, 16_000);
        if audio_16k.len() < 1600 {
            return Err(DiffusionError::model(format!(
                "indextts reference clip too short: {} samples at 16 kHz (need >= 0.1 s)",
                audio_16k.len()
            )));
        }
        self.prepare_voice_from_resampled(&audio_22k, &audio_16k, progress)
    }

    /// [`Self::prepare_voice`] after resampling — split out so validation can
    /// inject the oracle-dumped 22 k / 16 k clips and bypass resampler drift.
    pub fn prepare_voice_from_resampled(
        &self,
        audio_22k: &[f32],
        audio_16k: &[f32],
        mut progress: Option<ProgressHook>,
    ) -> Result<IndexTtsVoice> {
        emit_progress(&mut progress, "voice w2v", 0.05)?;
        let feats = extract_w2v_features(audio_16k)?;
        let spk_cond_emb = self.w2v.encode(&feats);
        let spk_frames = spk_cond_emb.len() / 1024;

        emit_progress(&mut progress, "voice mel", 0.50)?;
        let (ref_mel, mel_frames) = mel_spectrogram_22k(audio_22k);

        emit_progress(&mut progress, "voice campplus", 0.55)?;
        let (fbank, fbank_frames) = campplus_fbank(audio_16k)?;
        let style = self.campplus.embed(&fbank, fbank_frames);

        emit_progress(&mut progress, "voice regulator", 0.65)?;
        let prompt_condition =
            self.s2mel
                .length_regulator
                .forward(&spk_cond_emb, spk_frames, mel_frames)?;

        emit_progress(&mut progress, "voice emotion", 0.70)?;
        // Same-clip merge_emovec(spk, spk, 1.0) is bitwise emovec(spk); the
        // separate-emotion-audio path is not exposed (jobs carry vectors).
        let emovec_ref = self.gpt.emovec(&spk_cond_emb, spk_frames)?;
        emit_progress(&mut progress, "voice ready", 1.0)?;

        Ok(IndexTtsVoice {
            spk_cond_emb,
            spk_frames,
            ref_mel,
            mel_frames,
            style,
            prompt_condition,
            emovec_ref,
        })
    }

    /// Full text-to-speech over a prepared voice. Returns mono f32 samples at
    /// [`INDEXTTS_SAMPLE_RATE`]. English text path (v1); progress labels are
    /// per stage and the hook's Err cancels mid-GPT / mid-CFM.
    pub fn synthesize(
        &self,
        voice: &IndexTtsVoice,
        text: &str,
        params: &IndexTtsSynthesisParams,
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        let prepared = preprocess_text_en(text);
        if prepared.trim().is_empty() {
            return Err(DiffusionError::model(
                "indextts: no synthesizable text after normalization",
            ));
        }
        let lang_prefix = "<|en|> ";
        let lang_token = IndexTtsTokenizer::lang_index("EN")
            .ok_or_else(|| DiffusionError::model("indextts: EN language index missing"))?;
        let segments = segment_text(
            &self.tokenizer,
            &prepared,
            lang_prefix,
            params.max_segment_tokens.max(16),
        );
        if segments.is_empty() {
            return Err(DiffusionError::model(
                "indextts: text segmentation produced no segments",
            ));
        }

        // Style + emotion conditioning is per call, not per segment.
        let emovec = match &params.emotion {
            Some(raw) => {
                let weights = normalize_emotion_vector(raw);
                self.emotion
                    .mix(&voice.style, &weights, &voice.emovec_ref)
                    .emovec
            }
            None => voice.emovec_ref.clone(),
        };
        let conds_latent = self.gpt.conds_latent(&voice.style, &emovec);
        let dur = duration_factor(params.speed);
        // One noise stream across segments (sequential draws, like the
        // reference's single global torch RNG).
        let mut noise = S2melSeededNoise::new(params.sampling.seed);

        let nseg = segments.len();
        let mut wavs: Vec<Vec<f32>> = Vec::with_capacity(nseg);
        for (seg_index, segment) in segments.iter().enumerate() {
            let mut text_tokens = self.tokenizer.encode(&format!("{lang_prefix}{segment}"));
            // The reference pipeline appends the text stop token; prefill
            // re-frames either way, kept for oracle-identical inputs.
            text_tokens.push(1);
            if text_tokens.len() <= 2 {
                continue; // lang prefix + stop only: nothing to say
            }
            let mut cfg = params.sampling.clone();
            if nseg > 1 {
                cfg.seed = mix64(params.sampling.seed ^ (seg_index as u64).wrapping_add(1));
            }

            let mut hook = seg_band(&mut progress, seg_index, nseg, 0.0, 0.60);
            // Device transformer when present (parity-gated by
            // indextts_cuda_validate); CPU reference otherwise.
            let mut codes = if gpt_cuda_available() {
                self.gpt.generate_cuda_observed(
                    &conds_latent,
                    &text_tokens,
                    lang_token,
                    &cfg,
                    hook_ref(&mut hook),
                )?
            } else {
                self.gpt.generate_observed(
                    &conds_latent,
                    &text_tokens,
                    lang_token,
                    &cfg,
                    hook_ref(&mut hook),
                )?
            };
            drop(hook);
            if codes.last() == Some(&GPT_STOP_MEL) {
                codes.pop();
            }
            if codes.is_empty() {
                return Err(DiffusionError::model(format!(
                    "indextts: gpt produced no mel codes for segment {}",
                    seg_index + 1
                )));
            }
            if let Some(bad) = codes.iter().find(|&&c| c >= GPT_START_MEL) {
                return Err(DiffusionError::model(format!(
                    "indextts: gpt emitted special code {bad} mid-stream (segment {})",
                    seg_index + 1
                )));
            }

            let wav = self.segment_mel_wav(
                voice,
                &codes,
                dur,
                &mut noise,
                seg_index,
                nseg,
                &mut progress,
            )?;
            wavs.push(wav);
        }
        if wavs.is_empty() {
            return Err(DiffusionError::model(
                "indextts: no segment produced audio",
            ));
        }
        emit_progress(&mut progress, "stitch", 1.0)?;
        Ok(stitch_segments(&wavs, INDEXTTS_SAMPLE_RATE, params.silence_ms))
    }

    /// Codes -> wav back half shared by [`Self::synthesize`] and the oracle
    /// composition test (which injects dumped codes + replayed CFM noise):
    /// codec decode, target-length regulate, CFM mel, BigVGAN.
    #[allow(clippy::too_many_arguments)]
    pub fn segment_mel_wav(
        &self,
        voice: &IndexTtsVoice,
        codes: &[u32],
        duration_factor: f64,
        noise: &mut dyn S2melNoiseSource,
        seg_index: usize,
        nseg: usize,
        progress: &mut Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        let mut hook = seg_band(progress, seg_index, nseg, 0.60, 0.03);
        emit_progress(&mut hook_ref(&mut hook), "codec", 0.0)?;
        drop(hook);
        let s_infer = self.codec.decode(codes)?;
        let s_frames = codes.len() * 2;

        let target_len = cfm_target_len(s_frames, duration_factor).max(1);
        let cond = self
            .s2mel
            .length_regulator
            .forward(&s_infer, s_frames, target_len)?;
        let total_frames = voice.mel_frames + target_len;
        let mut cat_condition = Vec::with_capacity(total_frames * 512);
        cat_condition.extend_from_slice(&voice.prompt_condition);
        cat_condition.extend_from_slice(&cond);

        let mut hook = seg_band(progress, seg_index, nseg, 0.65, 0.27);
        let mel = self.s2mel.generate_mel(
            &cat_condition,
            total_frames,
            &voice.ref_mel,
            voice.mel_frames,
            &voice.style,
            noise,
            hook_ref(&mut hook),
        )?;
        drop(hook);

        let mut hook = seg_band(progress, seg_index, nseg, 0.92, 0.08);
        emit_progress(&mut hook_ref(&mut hook), "vocoder", 0.0)?;
        drop(hook);
        let vc = mel.vc_target();
        let wav = self.bigvgan.synthesize(&vc, target_len)?;
        let mut hook = seg_band(progress, seg_index, nseg, 1.0, 0.0);
        emit_progress(&mut hook_ref(&mut hook), "vocoder", 1.0)?;
        Ok(wav)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_folding_and_case() {
        assert_eq!(
            preprocess_text_en("Hello: \"World\"!\nYes..."),
            "hello, 'world'! yes…"
        );
    }

    #[test]
    fn numbers_are_spelled() {
        assert_eq!(spell_integer("0"), "zero");
        assert_eq!(spell_integer("17"), "seventeen");
        assert_eq!(spell_integer("42"), "forty two");
        assert_eq!(spell_integer("100"), "one hundred");
        assert_eq!(spell_integer("1234"), "one thousand two hundred thirty four");
        assert_eq!(spell_integer("1000000"), "one million");
        assert_eq!(spell_integer("007"), "zero zero seven");
        assert_eq!(spell_numbers("wave 3 begins"), "wave three begins");
        assert_eq!(spell_numbers("2.5 meters"), "two point five meters");
        assert_eq!(spell_numbers("1,234 gold"), "one thousand two hundred thirty four gold");
        assert_eq!(spell_numbers("room 12b"), "room twelve b");
    }

    #[test]
    fn stitching_inserts_silence() {
        let out = stitch_segments(&[vec![1.0; 10], vec![1.0; 10]], 1000, 200);
        assert_eq!(out.len(), 10 + 200 + 10);
        assert_eq!(out[15], 0.0);
    }

    #[test]
    fn resample_identity_and_ratio() {
        let sine: Vec<f32> = (0..2400)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 24000.0).sin())
            .collect();
        assert_eq!(resample_mono(&sine, 24000, 24000).len(), 2400);
        let down = resample_mono(&sine, 24000, 16000);
        assert_eq!(down.len(), 1600);
        // Mid-band sine survives with roughly unit amplitude.
        let peak = down[200..1400].iter().fold(0f32, |a, &b| a.max(b.abs()));
        assert!((peak - 1.0).abs() < 0.05, "peak {peak}");
    }

    #[test]
    fn target_len_python_int_semantics() {
        // Oracle: 92 codes -> 184 codec frames -> int(184 * 1.72) = 316.
        assert_eq!(cfm_target_len(184, 1.0), 316);
        // IEEE multiply rounds 25 * 1.72 to exactly 43.0 (Python agrees).
        assert_eq!(cfm_target_len(25, 1.0), 43);
        assert_eq!(cfm_target_len(184, 0.5), 158);
        assert_eq!(duration_factor(1.0), 1.0);
        assert_eq!(duration_factor(2.0), 0.5);
        assert_eq!(duration_factor(10.0), 0.5); // clamp
        assert_eq!(duration_factor(0.25), 2.0); // clamp
        assert_eq!(duration_factor(0.0), 1.0); // guard
        assert_eq!(duration_factor(-3.0), 1.0); // guard
    }

    #[test]
    fn weight_paths_layouts() {
        let r = IndexTtsWeightPaths::reference_layout(Path::new("/ckpt"));
        assert_eq!(
            r.w2v_safetensors,
            Path::new("/ckpt/hf_cache/w2v-bert-2.0/model.safetensors")
        );
        assert_eq!(r.bigvgan, Path::new("/ckpt/hf_cache/bigvgan/bigvgan_generator.pt"));
        let s = IndexTtsWeightPaths::service_layout(Path::new("/cache/indextts"));
        assert_eq!(
            s.w2v_safetensors,
            Path::new("/cache/indextts/w2v-bert-2.0/model.safetensors")
        );
        assert_eq!(s.campplus, Path::new("/cache/indextts/campplus_cn_common.bin"));
        // A bogus dir reports every expected file.
        assert_eq!(s.missing_files().len(), 10);
    }
}
