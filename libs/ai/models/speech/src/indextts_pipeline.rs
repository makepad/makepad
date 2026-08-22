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
use std::time::Instant;

/// `INDEXTTS_STAGE_TIMING=1` prints per-stage wall times to stdout
/// (`STAGE <name> <secs>`), used by the native bench to attribute
/// end-to-end time across gpt / codec / regulator / cfm / vocoder.
fn stage_timing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("INDEXTTS_STAGE_TIMING").as_deref() == Ok("1"))
}

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

/// Rational polyphase windowed-sinc resampler (Blackman window), mono. Used
/// on the reference-voice clip (any rate -> 22.05k and 16k). Matches the
/// reference chain (librosa/soxr + torchaudio sinc) to feature-level
/// tolerance, not sample-exactly — stage validation injects dumped audio.
pub fn resample_mono(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    if in_rate == out_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = in_rate as f64 / out_rate as f64;
    // Anti-aliasing cutoff in cycles per INPUT sample: half the lower of the
    // two Nyquists.
    let cutoff = 0.5f64 * (out_rate as f64 / in_rate as f64).min(1.0);
    let half_taps = 32usize; // input samples per side
    let out_len = (input.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len {
        let center = n as f64 * ratio;
        let start = (center.floor() as isize - half_taps as isize).max(0) as usize;
        let end = ((center.floor() as usize) + half_taps + 1).min(input.len());
        let mut acc = 0f64;
        let mut norm = 0f64;
        for (m, &sample) in input.iter().enumerate().take(end).skip(start) {
            let t = m as f64 - center;
            let x = 2.0 * cutoff * t;
            let sinc = if x == 0.0 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            // Blackman window over [-half_taps, half_taps].
            let w = 0.42
                + 0.5 * (std::f64::consts::PI * t / half_taps as f64).cos()
                + 0.08 * (2.0 * std::f64::consts::PI * t / half_taps as f64).cos();
            let tap = sinc * w;
            acc += sample as f64 * tap;
            norm += tap;
        }
        // Per-sample unity-DC normalization corrects both the 2*cutoff gain
        // factor and window truncation at the edges.
        out.push(if norm.abs() > 1e-12 { (acc / norm) as f32 } else { 0.0 });
    }
    out
}

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
        let t_stage = Instant::now();
        let feats = extract_w2v_features(audio_16k)?;
        let spk_cond_emb = self.w2v.encode(&feats);
        let spk_frames = spk_cond_emb.len() / 1024;
        if stage_timing() {
            println!("STAGE voice_w2v {:.3}s", t_stage.elapsed().as_secs_f64());
        }

        emit_progress(&mut progress, "voice mel", 0.50)?;
        let (ref_mel, mel_frames) = mel_spectrogram_22k(audio_22k);

        emit_progress(&mut progress, "voice campplus", 0.55)?;
        let t_stage = Instant::now();
        let (fbank, fbank_frames) = campplus_fbank(audio_16k)?;
        let style = self.campplus.embed(&fbank, fbank_frames);
        if stage_timing() {
            println!("STAGE voice_campplus {:.3}s", t_stage.elapsed().as_secs_f64());
        }

        emit_progress(&mut progress, "voice regulator", 0.65)?;
        let t_stage = Instant::now();
        let prompt_condition =
            self.s2mel
                .length_regulator
                .forward(&spk_cond_emb, spk_frames, mel_frames)?;

        emit_progress(&mut progress, "voice emotion", 0.70)?;
        // Same-clip merge_emovec(spk, spk, 1.0) is bitwise emovec(spk); the
        // separate-emotion-audio path is not exposed (jobs carry vectors).
        let emovec_ref = self.gpt.emovec(&spk_cond_emb, spk_frames)?;
        if stage_timing() {
            println!(
                "STAGE voice_regulator_emovec {:.3}s",
                t_stage.elapsed().as_secs_f64()
            );
        }
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
            let t_stage = Instant::now();
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
            if stage_timing() {
                println!(
                    "STAGE gpt {:.3}s ({} codes)",
                    t_stage.elapsed().as_secs_f64(),
                    codes.len()
                );
            }
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
        let t_stage = Instant::now();
        let s_infer = self.codec.decode(codes)?;
        let s_frames = codes.len() * 2;
        if stage_timing() {
            println!("STAGE codec {:.3}s", t_stage.elapsed().as_secs_f64());
        }

        let t_stage = Instant::now();
        let target_len = cfm_target_len(s_frames, duration_factor).max(1);
        let cond = self
            .s2mel
            .length_regulator
            .forward(&s_infer, s_frames, target_len)?;
        let total_frames = voice.mel_frames + target_len;
        let mut cat_condition = Vec::with_capacity(total_frames * 512);
        cat_condition.extend_from_slice(&voice.prompt_condition);
        cat_condition.extend_from_slice(&cond);
        if stage_timing() {
            println!(
                "STAGE regulator {:.3}s ({total_frames} frames)",
                t_stage.elapsed().as_secs_f64()
            );
        }

        let mut hook = seg_band(progress, seg_index, nseg, 0.65, 0.27);
        let t_stage = Instant::now();
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
        if stage_timing() {
            println!("STAGE cfm {:.3}s", t_stage.elapsed().as_secs_f64());
        }

        let mut hook = seg_band(progress, seg_index, nseg, 0.92, 0.08);
        emit_progress(&mut hook_ref(&mut hook), "vocoder", 0.0)?;
        drop(hook);
        let t_stage = Instant::now();
        let vc = mel.vc_target();
        let wav = self.bigvgan.synthesize(&vc, target_len)?;
        if stage_timing() {
            println!("STAGE bigvgan {:.3}s", t_stage.elapsed().as_secs_f64());
        }
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

    /// Loose validation against the reference resampler chain (soxr +
    /// torchaudio): cosine similarity on the 24k->16k conversion of the
    /// actual reference clip. Skipped without the reference checkout.
    #[test]
    fn resample_close_to_reference_chain() {
        let dumps = crate::indextts::reference_dumps_dir();
        let wav_path = crate::indextts::reference_checkpoints_dir()
            .join("../spk_ref_kokoro.wav");
        let target_path = dumps.join("audio_16k.npy");
        if !wav_path.is_file() || !target_path.is_file() {
            eprintln!("skipping resample_close_to_reference_chain (reference files missing)");
            return;
        }
        let bytes = std::fs::read(&wav_path).unwrap();
        // 16-bit mono PCM wav, 24 kHz (the kokoro reference clip).
        let data = &bytes[44..];
        let samples: Vec<f32> = data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect();
        let ours = resample_mono(&samples, 24000, 16000);
        let theirs = load_npy_f32(&target_path);
        let n = ours.len().min(theirs.len());
        let (mut dot, mut na, mut nb) = (0f64, 0f64, 0f64);
        for i in 0..n {
            dot += ours[i] as f64 * theirs[i] as f64;
            na += (ours[i] as f64).powi(2);
            nb += (theirs[i] as f64).powi(2);
        }
        let cosine = dot / (na.sqrt() * nb.sqrt()).max(1e-12);
        assert!(cosine > 0.995, "resampler cosine vs reference chain: {cosine}");
    }

    /// Minimal .npy f32 reader for the tests below (all IndexTTS dumps used
    /// here are C-order `<f4`; asserted).
    fn load_npy_f32(path: &std::path::Path) -> Vec<f32> {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[0..6], b"\x93NUMPY");
        let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        let data = &bytes[10 + header_len..];
        let header = std::str::from_utf8(&bytes[10..10 + header_len]).unwrap();
        assert!(header.contains("<f4"), "expected f32 npy: {header}");
        assert!(
            header.contains("'fortran_order': False"),
            "expected C-order npy: {header}"
        );
        data.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    fn cosine_and_max_abs(a: &[f32], b: &[f32]) -> (f64, f64) {
        assert_eq!(a.len(), b.len(), "length mismatch {} vs {}", a.len(), b.len());
        let (mut dot, mut na, mut nb, mut max_abs) = (0f64, 0f64, 0f64, 0f64);
        for (&x, &y) in a.iter().zip(b) {
            dot += x as f64 * y as f64;
            na += (x as f64).powi(2);
            nb += (y as f64).powi(2);
            max_abs = max_abs.max((x as f64 - y as f64).abs());
        }
        (dot / (na.sqrt() * nb.sqrt()).max(1e-12), max_abs)
    }

    fn write_wav_mono16(path: &std::path::Path, samples: &[f32], rate: u32) {
        let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
        let data_len = (samples.len() * 2) as u32;
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for &s in samples {
            bytes.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    /// 16-bit mono PCM reader for the reference clip (44-byte canonical
    /// header, as written by the dump scripts).
    fn read_wav_mono16(path: &std::path::Path) -> (Vec<f32>, u32) {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        let rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
        let samples = bytes[44..]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect();
        (samples, rate)
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

    /// Oracle noise replay: `draw` hands back the dumped cfm_noise verbatim.
    struct ReplayNoise(Vec<f32>);
    impl S2melNoiseSource for ReplayNoise {
        fn draw(&mut self, _index: usize, len: usize) -> Vec<f32> {
            assert_eq!(len, self.0.len(), "unexpected noise draw size");
            self.0.clone()
        }
    }

    /// End-to-end composition against the frozen oracle: dumped reference
    /// audio in, dumped greedy codes + dumped CFM noise injected (sampling /
    /// torch-RNG cannot be replayed), everything else computed by the
    /// assembled pipeline. Gates the FINAL waveform against the reference
    /// `bigvgan_wav.npy` — this validates the glue the stage validators
    /// cannot see (layouts, orderings, target-length arithmetic,
    /// prompt/condition concatenation, vc slicing).
    ///
    /// Skipped when the reference weights/dumps are absent. ~10 GB RAM,
    /// several minutes.
    #[test]
    fn e2e_composed_oracle_parity() {
        let checkpoints = crate::indextts::reference_checkpoints_dir();
        let dumps = crate::indextts::reference_dumps_dir();
        let paths = IndexTtsWeightPaths::reference_layout(&checkpoints);
        if !paths.missing_files().is_empty() || !dumps.join("bigvgan_wav.npy").is_file() {
            eprintln!("skipping e2e_composed_oracle_parity (reference env missing)");
            return;
        }
        let mut log = |label: &str, fraction: f64| {
            eprintln!("[e2e {fraction:5.2}] {label}");
            Ok(())
        };
        let pipeline =
            IndexTtsPipeline::load(&paths, Some(&mut log as ProgressHook)).expect("load");

        let audio_22k = load_npy_f32(&dumps.join("audio_22k.npy"));
        let audio_16k = load_npy_f32(&dumps.join("audio_16k.npy"));
        let voice = pipeline
            .prepare_voice_from_resampled(&audio_22k, &audio_16k, Some(&mut log as ProgressHook))
            .expect("prepare_voice");

        // Conditioning parity vs the dumps (computed here from raw audio).
        let oracle_style = load_npy_f32(&dumps.join("campplus_style.npy"));
        let (cos_style, _) = cosine_and_max_abs(&voice.style, &oracle_style);
        let oracle_mel = load_npy_f32(&dumps.join("ref_mel.npy"));
        assert_eq!(voice.ref_mel.len(), oracle_mel.len(), "ref_mel frame count");
        let (cos_mel, _) = cosine_and_max_abs(&voice.ref_mel, &oracle_mel);
        let oracle_prompt = load_npy_f32(&dumps.join("prompt_condition.npy"));
        let (cos_prompt, _) = cosine_and_max_abs(&voice.prompt_condition, &oracle_prompt);
        let oracle_emovec = load_npy_f32(&dumps.join("emovec_ref.npy"));
        let (cos_emovec, _) = cosine_and_max_abs(&voice.emovec_ref, &oracle_emovec);
        eprintln!(
            "conditioning cos: style {cos_style:.8} mel {cos_mel:.8} prompt {cos_prompt:.8} emovec {cos_emovec:.8}"
        );
        assert!(cos_style > 0.9999, "campplus style cos {cos_style}");
        assert!(cos_mel > 0.9999, "ref_mel cos {cos_mel}");
        assert!(cos_prompt > 0.999, "prompt_condition cos {cos_prompt}");
        // The reference loads the emotion copy of the clip through a
        // DIFFERENT audio path (emo_cond_emb != spk_cond_emb in the dumps),
        // so its emovec_ref is the latent of a slightly different input; the
        // production pipeline single-loads. Function parity is asserted below
        // on the dumped emo input; here only input-path drift remains.
        assert!(cos_emovec > 0.99, "emovec_ref cos {cos_emovec}");
        let emo_cond = load_npy_f32(&dumps.join("emo_cond_emb.npy"));
        let emovec_fn = pipeline
            .gpt
            .emovec(&emo_cond, emo_cond.len() / 1024)
            .expect("emovec");
        let (cos_emovec_fn, _) = cosine_and_max_abs(&emovec_fn, &oracle_emovec);
        eprintln!("emovec function parity (dumped emo input): cos {cos_emovec_fn:.8}");
        assert!(cos_emovec_fn > 0.9999, "emovec fn cos {cos_emovec_fn}");

        // Injected greedy codes (92, stop already stripped in the dump).
        let codes: Vec<u32> = load_npy_f32(&dumps.join("semantic_codes.npy"))
            .iter()
            .map(|&c| c as u32)
            .collect();
        assert_eq!(cfm_target_len(codes.len() * 2, 1.0), 316, "oracle target_len");

        let mut noise = ReplayNoise(load_npy_f32(&dumps.join("cfm_noise.npy")));
        let mut progress = Some(&mut log as ProgressHook);
        let wav = pipeline
            .segment_mel_wav(&voice, &codes, 1.0, &mut noise, 0, 1, &mut progress)
            .expect("segment_mel_wav");

        let oracle_wav = load_npy_f32(&dumps.join("bigvgan_wav.npy"));
        let (cos_wav, max_abs) = cosine_and_max_abs(&wav, &oracle_wav);
        eprintln!("final wav cos {cos_wav:.8} max abs {max_abs:.3e} ({} samples)", wav.len());
        write_wav_mono16(
            &checkpoints.join("../out_rust_e2e_composed.wav"),
            &wav,
            INDEXTTS_SAMPLE_RATE,
        );
        assert!(cos_wav > 0.999, "final wav cos vs oracle: {cos_wav}");
    }

    /// Full production path (real resampler, sampled GPT decode, seeded
    /// noise) with timing — the mac CPU baseline for the CUDA phase. Opt-in:
    /// `cargo test --release e2e_free_run_production -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn e2e_free_run_production() {
        let checkpoints = crate::indextts::reference_checkpoints_dir();
        let paths = IndexTtsWeightPaths::reference_layout(&checkpoints);
        let wav_path = checkpoints.join("../spk_ref_kokoro.wav");
        if !paths.missing_files().is_empty() || !wav_path.is_file() {
            eprintln!("skipping e2e_free_run_production (reference env missing)");
            return;
        }
        let mut log = |label: &str, fraction: f64| {
            eprintln!("[freerun {fraction:5.2}] {label}");
            Ok(())
        };
        let t0 = std::time::Instant::now();
        let pipeline =
            IndexTtsPipeline::load(&paths, Some(&mut log as ProgressHook)).expect("load");
        let load_s = t0.elapsed().as_secs_f64();

        let (samples, rate) = read_wav_mono16(&wav_path);
        let t1 = std::time::Instant::now();
        let voice = pipeline
            .prepare_voice(&samples, rate, Some(&mut log as ProgressHook))
            .expect("prepare_voice");
        let voice_s = t1.elapsed().as_secs_f64();

        let text = "The old lighthouse keeper smiled as the storm finally passed.";
        let mut params = IndexTtsSynthesisParams::default();
        params.sampling.seed = 42;
        let t2 = std::time::Instant::now();
        let wav = pipeline
            .synthesize(&voice, text, &params, Some(&mut log as ProgressHook))
            .expect("synthesize");
        let synth_s = t2.elapsed().as_secs_f64();
        let audio_s = wav.len() as f64 / INDEXTTS_SAMPLE_RATE as f64;
        write_wav_mono16(
            &checkpoints.join("../out_rust_e2e_freerun.wav"),
            &wav,
            INDEXTTS_SAMPLE_RATE,
        );

        // Emotion-vector path (sad), same voice/pipeline.
        params.emotion = Some([0.0, 0.0, 0.8, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let t3 = std::time::Instant::now();
        let wav_sad = pipeline
            .synthesize(&voice, text, &params, Some(&mut log as ProgressHook))
            .expect("synthesize sad");
        let sad_s = t3.elapsed().as_secs_f64();
        write_wav_mono16(
            &checkpoints.join("../out_rust_e2e_freerun_sad.wav"),
            &wav_sad,
            INDEXTTS_SAMPLE_RATE,
        );

        eprintln!(
            "load {load_s:.1}s | voice {voice_s:.2}s | synth {synth_s:.2}s for {audio_s:.2}s audio \
             (RTF {:.2}) | sad synth {sad_s:.2}s",
            synth_s / audio_s
        );
    }
}
