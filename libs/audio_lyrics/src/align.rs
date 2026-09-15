//! Word-level karaoke timing from the model that heard the words.
//!
//! Factored out of `apps/vj/src/lyrics_align.rs` so the asset-ui bake and
//! the VJ decks run the one implementation (and the VJ's audit harness,
//! `apps/vj/src/bin/karaoke_align.rs`, keeps scoring the exact shipped
//! code). The VJ's older path guessed word times from the vocal stem's RMS
//! envelope — honest, but a guess: proportional estimates pulled to energy
//! rises. This module replaces the guessing with measurement, in three
//! stages, each of which the audit harness can score separately:
//!
//! 1. **Cross-attention DTW** (`makepad_ai_speech::whisper::transcribe_aligned`): the
//!    decoder's alignment heads attend to the audio being sung as each token
//!    is written; a DTW path through that attention IS the word timing, on a
//!    20 ms grid. This is what `word_timestamps=True` does inside OpenAI's
//!    whisper, running against our own decoder.
//! 2. **Teacher-forced re-alignment** (`force_align`): each segment's known
//!    tokens re-decoded against a tight window around it. Short windows kill
//!    the within-chunk drift of pass 1, and forcing the KNOWN text removes
//!    transcription variance entirely — the text cannot change, only the
//!    timing can.
//! 3. **Onset snapping**: each word start pulled to the nearest attack the
//!    vocal stem actually shows, within a small window. The DTW is
//!    frame-quantized (20 ms) and softmax attention smears across doubled
//!    vocals; the stem's own onset is where the ear puts the word.
//!
//! A sanity layer scores every line: words in order, no word swallowing the
//! line, attention that was actually looking at the audio, snaps that
//! confirmed rather than fought the DTW. Lines that fail keep their word
//! times out of the cache and the display sweeps smoothly instead of hopping
//! wrongly — confidently wrong is worse than a smooth line.
//!
//! Everything here is track-time SECONDS and self-contained (makepad_ai_speech::whisper +
//! std only), so the audit harness can drive the exact code the apps ship.

use makepad_ai_speech::whisper::{AlignedSegment, WhisperModel, WhisperState};

/// Whisper's input rate.
pub const WHISPER_RATE: f64 = 16_000.0;

/// Constant correction added to every aligned word time before it is used.
/// The DTW grid stamps a token at the START of the 20 ms encoder frame the
/// path enters; the audit's bias measurement across tracks decides whether
/// that needs a shift. Measured 2026-08-20 (two tracks, DTW+forced stage,
/// against independent multi-band onsets): mean signed error was inside
/// ±15 ms with snapping on, so the correction stays zero — a named lever,
/// not a magic number.
pub const ALIGN_BIAS_SECS: f64 = 0.0;

/// How far a word start may be pulled to a stem onset. Wider than the 20 ms
/// DTW grid it corrects, narrower than a syllable.
const SNAP_WINDOW_SECS: f64 = 0.15;

/// No two words share an instant.
const MIN_WORD_SECS: f64 = 0.06;

/// A silence this long inside a segment separates two karaoke lines.
const LINE_GAP_SECS: f64 = 0.6;

/// A karaoke line a singer can read at a glance.
const MAX_LINE_WORDS: usize = 8;
const MIN_CLAUSE_WORDS: usize = 4;

/// Margin either side of a segment for the teacher-forced window: enough
/// context that the model can hear the phrase enter, short enough that the
/// window stays drift-free. The bracket rows of `force_align` absorb
/// whatever the margin lets in.
const FORCE_MARGIN_SECS: f64 = 1.5;

/// Attention-mass floor: below this the token was not really looking at the
/// audio it was stamped to (calibrated on the audit tracks — sung words on
/// clean stems sit well above it, hallucinated fillers below).
const SCORE_FLOOR: f32 = 0.15;

// ---------------------------------------------------------------------------
// resampling (the whisper mixdown)
// ---------------------------------------------------------------------------

/// Centered polyphase windowed-sinc resampler: output index `i` is built
/// symmetrically around input position `i * from/to`, so the group delay is
/// ZERO by construction — the mixdown cannot shift the word clock. Verified
/// by test (`resampling_moves_no_time`).
pub fn resample(input: &[f32], from_rate: f64, to_rate: f64) -> Vec<f32> {
    if input.is_empty() || (from_rate - to_rate).abs() < 0.5 {
        return input.to_vec();
    }
    const PHASES: usize = 64;
    const TAPS: usize = 16;
    let ratio = to_rate / from_rate;
    let cutoff = 0.5 * ratio.min(1.0);
    let width = ((TAPS as f64) / (2.0 * cutoff)).ceil() as usize;
    let span = 2 * width + 1;
    let mut taps = vec![0.0f32; PHASES * span];
    for phase in 0..PHASES {
        let frac = phase as f64 / PHASES as f64;
        let row = phase * span;
        let mut sum = 0.0f64;
        for tap in 0..span {
            let x = frac - (tap as f64 - width as f64);
            let arg = 2.0 * cutoff * x;
            let sinc = if arg.abs() < 1e-9 {
                1.0
            } else {
                (std::f64::consts::PI * arg).sin() / (std::f64::consts::PI * arg)
            };
            let t = (x / width as f64).clamp(-1.0, 1.0);
            let angle = std::f64::consts::PI * (t + 1.0);
            let window = 0.42 - 0.5 * angle.cos() + 0.08 * (2.0 * angle).cos();
            let weight = sinc * window;
            taps[row + tap] = weight as f32;
            sum += weight;
        }
        if sum.abs() > 1e-12 {
            let inverse = (1.0 / sum) as f32;
            for tap in 0..span {
                taps[row + tap] *= inverse;
            }
        }
    }
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    let inverse_ratio = 1.0 / ratio;
    for index in 0..out_len {
        let center = index as f64 * inverse_ratio;
        let base = center.floor() as isize;
        let frac = center - base as f64;
        let phase = ((frac * PHASES as f64) as usize).min(PHASES - 1);
        let row = phase * span;
        let mut sum = 0.0f32;
        for tap in 0..span {
            let at = base + tap as isize - width as isize;
            if at < 0 {
                continue;
            }
            let Some(sample) = input.get(at as usize) else { break };
            sum += sample * taps[row + tap];
        }
        out.push(sum);
    }
    out
}

// ---------------------------------------------------------------------------
// vocal analysis: energy + onsets
// ---------------------------------------------------------------------------

/// One detected vocal attack.
#[derive(Clone, Copy, Debug)]
pub struct Onset {
    pub time: f64,
    /// 0..1, relative to the track's strongest flux peak.
    pub strength: f32,
}

/// Multi-band spectral-flux analysis of the vocals stem.
#[derive(Clone, Debug)]
pub struct VocalAnalysis {
    pub hop_secs: f64,
    /// Per-frame broadband level (linear), for silence gates.
    pub energy: Vec<f32>,
    /// "What loud means on this track": 90th percentile of `energy`.
    pub loud: f32,
    pub onsets: Vec<Onset>,
}

/// Two deliberately different parameterizations, so the snapping stage and
/// the audit's ground truth are not the same measurement wearing two hats:
/// ground truth runs at twice the resolution with a stricter, sparser picker
/// — the onsets a human spot-checker would call unambiguous.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnsetPreset {
    Snapping,
    GroundTruth,
}

struct OnsetParams {
    hop_secs: f64,
    window: usize,
    /// Peak must beat `mean(local) * ratio`.
    threshold_ratio: f32,
    refractory_secs: f64,
}

impl OnsetPreset {
    fn params(self) -> OnsetParams {
        match self {
            OnsetPreset::Snapping => OnsetParams {
                hop_secs: 0.005,
                window: 1024,
                threshold_ratio: 1.4,
                refractory_secs: 0.07,
            },
            // Moderate threshold, LONG refractory: consonant bursts fire
            // too (a 1.9× threshold missed them and anchored every
            // consonant-initial word at its vowel, a consistent −125 ms
            // against both whisper's attention and the snapping preset),
            // and the refractory then keeps the FIRST event of each attack
            // cluster — the start of the word's first phoneme, which is
            // what a human spot-checker calls the word onset.
            OnsetPreset::GroundTruth => OnsetParams {
                hop_secs: 0.0025,
                window: 2048,
                threshold_ratio: 1.55,
                refractory_secs: 0.13,
            },
        }
    }
}

/// In-place iterative radix-2 FFT over interleaved (re, im).
fn fft_inplace(buffer: &mut [f32]) {
    let n = buffer.len() / 2;
    debug_assert!(n.is_power_of_two());
    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 0..n {
        if i < j {
            buffer.swap(2 * i, 2 * j);
            buffer.swap(2 * i + 1, 2 * j + 1);
        }
        let mut mask = n >> 1;
        while mask > 0 && j & mask != 0 {
            j &= !mask;
            mask >>= 1;
        }
        j |= mask;
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * std::f64::consts::PI / len as f64;
        let (w_re, w_im) = (angle.cos() as f32, angle.sin() as f32);
        let mut base = 0;
        while base < n {
            let (mut cur_re, mut cur_im) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let even = 2 * (base + k);
                let odd = 2 * (base + k + len / 2);
                let (o_re, o_im) = (buffer[odd], buffer[odd + 1]);
                let (t_re, t_im) = (
                    o_re * cur_re - o_im * cur_im,
                    o_re * cur_im + o_im * cur_re,
                );
                buffer[odd] = buffer[even] - t_re;
                buffer[odd + 1] = buffer[even + 1] - t_im;
                buffer[even] += t_re;
                buffer[even + 1] += t_im;
                let next_re = cur_re * w_re - cur_im * w_im;
                cur_im = cur_re * w_im + cur_im * w_re;
                cur_re = next_re;
            }
            base += len;
        }
        len <<= 1;
    }
}

/// Where a sung voice announces a new syllable: 150 Hz – 9.6 kHz in six
/// octave bands. The low fundamental smears; the consonant band is crisp.
const BAND_EDGES_HZ: [f64; 7] = [150.0, 300.0, 600.0, 1200.0, 2400.0, 4800.0, 9600.0];

pub fn analyze_vocals(mono: &[f32], rate: f64, preset: OnsetPreset) -> VocalAnalysis {
    let params = preset.params();
    let hop = ((rate * params.hop_secs).round() as usize).max(1);
    let window = params.window;
    let hop_secs = hop as f64 / rate;
    if mono.len() < window {
        return VocalAnalysis { hop_secs, energy: Vec::new(), loud: 0.0, onsets: Vec::new() };
    }
    let frames = (mono.len() - window) / hop + 1;
    let bands = BAND_EDGES_HZ.len() - 1;
    let mut band_edges = Vec::with_capacity(BAND_EDGES_HZ.len());
    for edge in BAND_EDGES_HZ {
        band_edges.push(((edge / rate * window as f64).round() as usize).min(window / 2));
    }
    let hann: Vec<f32> = (0..window)
        .map(|i| {
            (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / window as f64).cos()) as f32
        })
        .collect();

    let mut band_log = vec![0.0f32; frames * bands];
    let mut energy = vec![0.0f32; frames];
    let mut buffer = vec![0.0f32; window * 2];
    for frame in 0..frames {
        let from = frame * hop;
        for i in 0..window {
            buffer[2 * i] = mono[from + i] * hann[i];
            buffer[2 * i + 1] = 0.0;
        }
        fft_inplace(&mut buffer);
        let mut total = 0.0f64;
        for band in 0..bands {
            let mut sum = 0.0f64;
            for bin in band_edges[band]..band_edges[band + 1] {
                let re = buffer[2 * bin] as f64;
                let im = buffer[2 * bin + 1] as f64;
                sum += (re * re + im * im).sqrt();
            }
            band_log[frame * bands + band] = (1.0 + sum).ln() as f32;
            total += sum;
        }
        energy[frame] = (total / window as f64) as f32;
    }

    let mut sorted = energy.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let loud = sorted
        .get((sorted.len() as f64 * 0.9) as usize)
        .copied()
        .unwrap_or(0.0);

    // Half-wave-rectified multi-band flux.
    let mut flux = vec![0.0f32; frames];
    for frame in 1..frames {
        let mut sum = 0.0f32;
        for band in 0..bands {
            sum += (band_log[frame * bands + band] - band_log[(frame - 1) * bands + band]).max(0.0);
        }
        flux[frame] = sum;
    }
    let max_flux = flux.iter().fold(0.0f32, |a, v| a.max(*v));
    if max_flux <= 0.0 {
        return VocalAnalysis { hop_secs, energy, loud, onsets: Vec::new() };
    }

    // Peak picking against a local adaptive threshold. TIME CONVENTION:
    // frame `t` is stamped at its window CENTER (`t*hop + window/2`). The
    // STFT sees an attack from the moment the window's leading edge touches
    // it, so start-of-window stamps read a full window EARLY — a measured
    // 44 ms bias at the ground-truth window before this convention fixed it.
    let center_secs = window as f64 * 0.5 / rate;
    let local = ((0.03 / hop_secs).round() as usize).max(1);
    let neighbourhood = ((0.25 / hop_secs).round() as usize).max(2);
    let refractory = ((params.refractory_secs / hop_secs).round() as usize).max(1);
    // A SHORT walk from the flux peak toward the start of the steep rise:
    // the ear places the note at the attack's start, but most of what lies
    // before the peak is window smear, not attack, so the walk is capped.
    let backtrack = ((0.02 / hop_secs).round() as usize).max(1);
    let floor = max_flux * 0.03;
    let mut onsets = Vec::new();
    let mut last_foot: Option<usize> = None;
    for frame in 1..frames {
        let value = flux[frame];
        if value < floor {
            continue;
        }
        let a = frame.saturating_sub(local);
        let b = (frame + local + 1).min(frames);
        if flux[a..b].iter().any(|other| *other > value) {
            continue;
        }
        let a = frame.saturating_sub(neighbourhood);
        let b = (frame + neighbourhood + 1).min(frames);
        let mean = flux[a..b].iter().sum::<f32>() / (b - a) as f32;
        if value < mean * params.threshold_ratio + floor {
            continue;
        }
        let mut foot = frame;
        while foot > 1 && frame - foot < backtrack && flux[foot - 1] >= value * 0.35 {
            foot -= 1;
        }
        // Then back to the preceding local minimum of the ENERGY envelope:
        // the foot of the rise the ear calls the note's start. Where the
        // voice never dipped (legato) the minimum sits immediately behind
        // the peak and nothing moves.
        let energy_cap = ((0.25 / hop_secs).round() as usize).max(1);
        let origin = foot;
        while foot > 0
            && origin - foot < energy_cap
            && smoothed(&energy, foot - 1) < smoothed(&energy, foot) * 0.995
        {
            foot -= 1;
        }
        if last_foot.is_some_and(|previous| foot.saturating_sub(previous) < refractory) {
            continue;
        }
        onsets.push(Onset {
            time: foot as f64 * hop_secs + center_secs,
            strength: value / max_flux,
        });
        last_foot = Some(foot);
    }
    // Bridging can pull a foot back past an earlier onset; the list the
    // snapper binary-searches must be sorted and deduplicated.
    onsets.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    let mut deduped: Vec<Onset> = Vec::with_capacity(onsets.len());
    for onset in onsets {
        match deduped.last_mut() {
            Some(last) if onset.time - last.time < 0.03 => {
                last.strength = last.strength.max(onset.strength);
            }
            _ => deduped.push(onset),
        }
    }
    VocalAnalysis { hop_secs, energy, loud, onsets: deduped }
}

/// 3-frame smoothed read of the energy envelope.
fn smoothed(energy: &[f32], at: usize) -> f32 {
    let a = at.saturating_sub(1);
    let b = (at + 1).min(energy.len() - 1);
    (energy[a] + energy[at] + energy[b]) / (b - a + 1) as f32
}

impl VocalAnalysis {
    pub fn peak_between(&self, from: f64, to: f64) -> f32 {
        if self.energy.is_empty() {
            return 0.0;
        }
        let a = ((from.max(0.0) / self.hop_secs) as usize).min(self.energy.len() - 1);
        let b = ((to.max(0.0) / self.hop_secs) as usize).clamp(a, self.energy.len() - 1);
        self.energy[a..=b].iter().fold(0.0f32, |m, v| m.max(*v))
    }

    /// Nearest onset to `time` within ±`window`, by proximity weighted a
    /// little by strength so a strong attack 30 ms away beats a whisper of
    /// one 10 ms away.
    pub fn onset_near(&self, time: f64, window: f64) -> Option<Onset> {
        let from = self.onsets.partition_point(|onset| onset.time < time - window);
        let mut best: Option<(f64, Onset)> = None;
        for onset in &self.onsets[from..] {
            if onset.time > time + window {
                break;
            }
            let cost = (onset.time - time).abs() / (0.3 + onset.strength as f64);
            if best.is_none_or(|(previous, _)| cost < previous) {
                best = Some((cost, *onset));
            }
        }
        best.map(|(_, onset)| onset)
    }
}

// ---------------------------------------------------------------------------
// words
// ---------------------------------------------------------------------------

/// One word with everything the sanity layer wants to know about it.
#[derive(Clone, Debug)]
pub struct TimedWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
    /// DTW attention mass, 0..1.
    pub score: f32,
    /// Seconds the onset snap moved this word (signed), when it found one.
    pub snap: Option<f64>,
    /// |pass-2 − pass-1| start disagreement, when pass 2 ran.
    pub pass_delta: Option<f64>,
}

/// One whisper segment carrying its aligned words and the tokens needed to
/// re-align them.
#[derive(Clone, Debug)]
pub struct SegmentWords {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub words: Vec<TimedWord>,
    pub tokens: Vec<i32>,
    /// Pass 2 succeeded on this segment.
    pub forced: bool,
}

fn is_bracketed(text: &str) -> bool {
    (text.starts_with('[') && text.ends_with(']'))
        || (text.starts_with('(') && text.ends_with(')'))
        || (text.starts_with('♪') && text.ends_with('♪'))
}

/// Stage 0: pass-1 output → clean segments with words in track seconds.
/// Hygiene is the same the envelope path applies: no bracketed markers, no
/// lines over stem silence, no verbatim repetition loops.
pub fn collect_words(
    aligned: Vec<AlignedSegment>,
    analysis: &VocalAnalysis,
    duration_secs: f64,
) -> Vec<SegmentWords> {
    let mut out: Vec<SegmentWords> = Vec::new();
    for segment in aligned {
        let text = segment.text.trim().to_string();
        if text.is_empty() || is_bracketed(&text) {
            continue;
        }
        let start = segment.start_ms as f64 / 1000.0;
        let mut end = segment.end_ms as f64 / 1000.0;
        if duration_secs > 0.0 {
            end = end.min(duration_secs);
        }
        if end <= start {
            continue;
        }
        if analysis.loud > 0.0 && analysis.peak_between(start, end) < analysis.loud * 0.05 {
            continue;
        }
        if let Some(last) = out.last() {
            if last.text == text && start - last.end < 0.05 {
                continue;
            }
        }
        let expected = text.split_whitespace().count();
        let words: Vec<TimedWord> = if segment.words.len() == expected {
            segment
                .words
                .iter()
                .map(|word| TimedWord {
                    text: word.text.clone(),
                    start: word.start_ms as f64 / 1000.0 + ALIGN_BIAS_SECS,
                    end: word.end_ms as f64 / 1000.0 + ALIGN_BIAS_SECS,
                    score: word.score,
                    snap: None,
                    pass_delta: None,
                })
                .collect()
        } else {
            // Alignment did not cover this segment; empty words means "the
            // display sweeps this line", never wrong hops.
            Vec::new()
        };
        out.push(SegmentWords { text, start, end, words, tokens: segment.tokens, forced: false });
    }
    out
}

/// Stage 2 (pass 2): teacher-forced re-alignment of each segment against a
/// tight window around it. The text cannot change — only the timing. A
/// segment whose pass-1 words were unusable (word-count parity failed, or a
/// chunk aligned badly) is not skipped: its stamps bound the window and the
/// forced pass supplies the words pass 1 could not — teacher forcing needs
/// only the tokens, and the tokens survived.
pub fn force_align_segments(
    state: &mut WhisperState,
    model: &WhisperModel,
    samples_16k: &[f32],
    segments: &mut [SegmentWords],
    language: &str,
) {
    let track_secs = samples_16k.len() as f64 / WHISPER_RATE;
    for segment in segments.iter_mut() {
        if segment.tokens.is_empty() {
            continue;
        }
        let (first, last) = match (segment.words.first(), segment.words.last()) {
            (Some(first), Some(last)) => (first.start, last.end),
            _ => (segment.start, segment.end),
        };
        // Symmetric margins even at the very start of the track: a window
        // that begins AT zero pins the leading absorber row to the first
        // word's audio and drags it early — pad with real silence instead.
        let wanted_from = first - FORCE_MARGIN_SECS;
        let lead_pad = (-wanted_from).max(0.0);
        let from = wanted_from.max(0.0);
        let to = (last + FORCE_MARGIN_SECS).min(track_secs);
        if to - from < 0.4 {
            continue;
        }
        let a = (from * WHISPER_RATE) as usize;
        let b = ((to * WHISPER_RATE) as usize).min(samples_16k.len());
        if b <= a {
            continue;
        }
        let mut window: Vec<f32> = Vec::with_capacity(b - a + (lead_pad * WHISPER_RATE) as usize);
        window.resize((lead_pad * WHISPER_RATE) as usize, 0.0);
        window.extend_from_slice(&samples_16k[a..b]);
        let Some(forced) = state.force_align(model, &window, &segment.tokens, language) else {
            continue;
        };
        // The forced words must spell the segment text exactly, word for
        // word, or nothing is adopted — never a partially retimed line.
        let expected: Vec<&str> = segment.text.split_whitespace().collect();
        if forced.len() != expected.len()
            || forced.iter().zip(&expected).any(|(span, text)| span.text != **text)
        {
            continue;
        }
        let offset = from - lead_pad;
        if segment.words.len() == forced.len() {
            for (word, span) in segment.words.iter_mut().zip(&forced) {
                let start = offset + span.start_ms as f64 / 1000.0 + ALIGN_BIAS_SECS;
                let end = offset + span.end_ms as f64 / 1000.0 + ALIGN_BIAS_SECS;
                word.pass_delta = Some((start - word.start).abs());
                word.start = start;
                word.end = end.max(start + MIN_WORD_SECS * 0.5);
                word.score = span.score;
            }
        } else {
            // Pass 1 had nothing usable here; the forced words ARE the words.
            segment.words = forced
                .iter()
                .map(|span| TimedWord {
                    text: span.text.clone(),
                    start: offset + span.start_ms as f64 / 1000.0 + ALIGN_BIAS_SECS,
                    end: (offset + span.end_ms as f64 / 1000.0 + ALIGN_BIAS_SECS)
                        .max(offset + span.start_ms as f64 / 1000.0 + MIN_WORD_SECS * 0.5),
                    score: span.score,
                    snap: None,
                    pass_delta: None,
                })
                .collect();
        }
        segment.forced = true;
    }
}

/// How long a word may hold before a weak alignment is called absorbed.
const ABSORBED_HOLD_SECS: f64 = 2.5;
/// Attention mass below which a long hold is absorption, not melisma. A real
/// held note keeps the model's attention; a token parked over an
/// instrumental intro does not.
const ABSORBED_SCORE: f32 = 0.30;
/// Pass-1 and the teacher-forced pass disagreeing by this much on one word
/// is the other absorption signature: two independent decodes could not
/// agree where it was sung. A genuinely held note has BOTH passes agreeing
/// (measured: "the lights are low…" holds agree within 160 ms; the intro-
/// absorbed "Oh," disagreed by 8.3 s).
const ABSORBED_DISAGREE_SECS: f64 = 0.5;

/// A word whose interval is seconds long with the attention looking
/// elsewhere did not take that long to sing — it absorbed non-vocal audio
/// (the classic case: the first word of the first line stretched back over
/// the instrumental intro). Move its start FORWARD to the strongest attack
/// the stem shows inside its interval; never backwards, and never past the
/// word that follows.
pub fn rescue_absorbed_words(segments: &mut [SegmentWords], analysis: &VocalAnalysis) {
    for segment in segments.iter_mut() {
        let count = segment.words.len();
        let segment_end = segment.end;
        for index in 0..count {
            // The hold is measured WITHIN the segment: to the next word, or
            // for the segment's last word to the segment's own end. The
            // instrumental gap AFTER a line must not count — a line-final
            // held word ("…hear my prayer" before the chorus) is a hold the
            // singer meant, not absorption, and rescuing it onto the next
            // phrase's attack was a measured misfire.
            let next = segment
                .words
                .get(index + 1)
                .map(|word| word.start)
                .unwrap_or_else(|| segment_end.max(segment.words[index].start));
            let word = &segment.words[index];
            let hold = next - word.start;
            if hold < ABSORBED_HOLD_SECS {
                continue;
            }
            let disagreed = word
                .pass_delta
                .is_some_and(|delta| delta > ABSORBED_DISAGREE_SECS);
            if word.score >= ABSORBED_SCORE && !disagreed {
                continue;
            }
            let to = next - MIN_WORD_SECS;
            let mut best: Option<Onset> = None;
            let from = analysis.onsets.partition_point(|onset| onset.time <= word.start);
            for onset in &analysis.onsets[from..] {
                if onset.time > to {
                    break;
                }
                if best.is_none_or(|previous| onset.strength > previous.strength) {
                    best = Some(*onset);
                }
            }
            if let Some(onset) = best {
                if onset.time > segment.words[index].start + 0.3 {
                    let word = &mut segment.words[index];
                    word.snap = Some(onset.time - word.start);
                    word.start = onset.time;
                }
            }
        }
    }
}

/// A word may stay unsnapped at this equivalent cost (seconds). Words with
/// no attack of their own — melisma, soft entries — keep their forced time
/// rather than stealing a neighbour's onset.
const SNAP_UNMATCHED_COST: f64 = 0.09;
/// How much a weak onset is handicapped against a strong one (seconds of
/// equivalent distance across the 0..1 strength range).
const SNAP_STRENGTH_WEIGHT: f64 = 0.015;

/// Stage 3: pull word starts onto the attacks the stem shows — as a global
/// MONOTONIC assignment per segment, not a greedy nearest. Words and onsets
/// are two ordered sequences; a classic alignment DP (match / word stays
/// unsnapped / onset unused) decides which attack each word OWNS, so in a
/// crowded run two words can never fight over one onset and a word between
/// attacks takes the unmatched cost instead of a wrong neighbour. This is
/// the "word identity" problem the greedy version got wrong.
pub fn snap_words(segments: &mut [SegmentWords], analysis: &VocalAnalysis) {
    for segment in segments.iter_mut() {
        if segment.words.is_empty() {
            continue;
        }
        let from = segment.words.first().unwrap().start - SNAP_WINDOW_SECS;
        let to = segment.words.last().unwrap().start + SNAP_WINDOW_SECS;
        let first = analysis.onsets.partition_point(|onset| onset.time < from);
        let last = analysis.onsets.partition_point(|onset| onset.time <= to);
        let onsets = &analysis.onsets[first..last];
        let n = segment.words.len();
        let m = onsets.len();
        if m == 0 {
            continue;
        }
        let cost = |word: &TimedWord, onset: &Onset| -> f64 {
            let delta = (onset.time - word.start).abs();
            if delta > SNAP_WINDOW_SECS {
                return f64::INFINITY;
            }
            delta + (1.0 - onset.strength as f64) * SNAP_STRENGTH_WEIGHT
        };
        // dp[i][j]: best cost for the first i words against the first j
        // onsets. 0 = word i matched onset j, 1 = word unsnapped, 2 = onset
        // unused.
        let width = m + 1;
        let mut dp = vec![f64::INFINITY; (n + 1) * width];
        let mut step = vec![2u8; (n + 1) * width];
        for j in 0..width {
            dp[j] = 0.0;
        }
        for i in 1..=n {
            for j in 0..width {
                let unsnapped = dp[(i - 1) * width + j] + SNAP_UNMATCHED_COST;
                let mut best = unsnapped;
                let mut how = 1u8;
                if j > 0 {
                    let unused = dp[i * width + j - 1];
                    if unused < best {
                        best = unused;
                        how = 2;
                    }
                    let matched = dp[(i - 1) * width + j - 1]
                        + cost(&segment.words[i - 1], &onsets[j - 1]);
                    if matched < best {
                        best = matched;
                        how = 0;
                    }
                }
                dp[i * width + j] = best;
                step[i * width + j] = how;
            }
        }
        let (mut i, mut j) = (n, m);
        let mut chosen: Vec<Option<usize>> = vec![None; n];
        while i > 0 {
            match step[i * width + j] {
                0 => {
                    chosen[i - 1] = Some(j - 1);
                    i -= 1;
                    j -= 1;
                }
                1 => i -= 1,
                _ => j -= 1,
            }
        }
        for (word, pick) in segment.words.iter_mut().zip(&chosen) {
            if let Some(index) = pick {
                let onset = onsets[*index];
                word.snap = Some(onset.time - word.start);
                word.end = word.end.max(onset.time + MIN_WORD_SECS * 0.5);
                word.start = onset.time;
            }
        }
    }
    enforce_monotonic(segments);
}

/// Word starts strictly increase across the whole track; ends stay inside
/// their neighbours. Run after any stage that moves times.
pub fn enforce_monotonic(segments: &mut [SegmentWords]) {
    let mut previous = f64::NEG_INFINITY;
    for segment in segments.iter_mut() {
        for word in segment.words.iter_mut() {
            if word.start < previous + MIN_WORD_SECS {
                word.start = previous + MIN_WORD_SECS;
            }
            if word.end < word.start + MIN_WORD_SECS * 0.5 {
                word.end = word.start + MIN_WORD_SECS * 0.5;
            }
            previous = word.start;
        }
    }
    // Ends never cross the next word's start.
    let mut flat: Vec<(usize, usize)> = Vec::new();
    for (s, segment) in segments.iter().enumerate() {
        for w in 0..segment.words.len() {
            flat.push((s, w));
        }
    }
    for pair in flat.windows(2) {
        let next_start = segments[pair[1].0].words[pair[1].1].start;
        let word = &mut segments[pair[0].0].words[pair[0].1];
        if word.end > next_start {
            word.end = next_start;
        }
    }
}

// ---------------------------------------------------------------------------
// lines
// ---------------------------------------------------------------------------

/// A karaoke line ready for the cache: `words` are start seconds, one per
/// whitespace word of `text`, and `confident` is the sanity layer's honest
/// verdict — the display hops only where it is true.
#[derive(Clone, Debug)]
pub struct TimedLine {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub words: Vec<f64>,
    pub confident: bool,
}

/// Stage 4: words → lines a singer can read. Line starts come from the
/// FIRST WORD's aligned time, never from segment stamps: a lyric line that
/// flows straight out of the previous one ("…give me a / man after
/// midnight") has no silence to cut on, and the stamp lands early — the
/// word alignment is the only truth there.
pub fn assemble_lines(segments: &[SegmentWords], duration_secs: f64) -> Vec<TimedLine> {
    let mut out: Vec<TimedLine> = Vec::new();
    for segment in segments {
        if segment.words.is_empty() {
            // Unaligned segment: one line on the stamps, sweeping fill.
            out.push(TimedLine {
                start: segment.start,
                end: segment.end,
                text: segment.text.clone(),
                words: Vec::new(),
                confident: false,
            });
            continue;
        }
        let words = &segment.words;
        let mut line: Vec<&TimedWord> = Vec::new();
        let flush = |line: &mut Vec<&TimedWord>, out: &mut Vec<TimedLine>| {
            if line.is_empty() {
                return;
            }
            let text = line
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let start = line[0].start;
            let end = line.last().unwrap().end.max(start + 0.2);
            let starts: Vec<f64> = line.iter().map(|word| word.start).collect();
            let confident = line_is_trustworthy(line);
            out.push(TimedLine { start, end, text, words: starts, confident });
            line.clear();
        };
        for word in words.iter() {
            if !line.is_empty() {
                let gap = word.start - line.last().unwrap().end;
                let clause = line.len() >= MIN_CLAUSE_WORDS
                    && ends_a_clause(&line.last().unwrap().text)
                    && gap >= 0.15;
                // Whisper capitalizes sentence starts even mid-segment
                // ("…after midnight Won't somebody…"): a capitalized word is
                // the next lyric line's first word, not this one's last.
                let sentence = line.len() >= MIN_CLAUSE_WORDS && starts_a_sentence(&word.text);
                if line.len() >= MAX_LINE_WORDS || gap >= LINE_GAP_SECS || clause || sentence {
                    flush(&mut line, &mut out);
                }
            }
            line.push(word);
        }
        flush(&mut line, &mut out);
    }
    // Lines never overlap and never outlive the track. The readability
    // floor is applied BEFORE the next-line clamp, so crowded lines end
    // early rather than overlap — overlapping lines break the schedule.
    for index in 0..out.len() {
        let start = out[index].start;
        let mut end = out[index].end.max(start + 0.15);
        if duration_secs > 0.0 {
            end = end.min(duration_secs.max(start));
        }
        if index + 1 < out.len() {
            end = end.min(out[index + 1].start);
        }
        out[index].end = end;
    }
    out
}

fn ends_a_clause(word: &str) -> bool {
    word.ends_with([',', '.', '!', '?', ';', ':', '—'])
}

/// A capitalized word that is not the pronoun "I" (or "I'm", "I'll", …).
fn starts_a_sentence(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else { return false };
    if !first.is_uppercase() {
        return false;
    }
    !(first == 'I' && matches!(chars.next(), None | Some('\'')))
}

/// The sanity layer's per-line verdict. Every check earns its place in the
/// audit: a line that fails ANY of them has, on the evidence, word times not
/// worth hopping on.
fn line_is_trustworthy(words: &[&TimedWord]) -> bool {
    if words.len() < 2 {
        // A one-word line has nothing to hop between; sweeping is identical.
        return words.len() == 1;
    }
    let start = words[0].start;
    let end = words.last().unwrap().end;
    let span = end - start;
    if span <= 0.0 {
        return false;
    }
    let mut anchored = 0usize;
    for (index, word) in words.iter().enumerate() {
        if !word.start.is_finite() || word.score < SCORE_FLOOR {
            return false;
        }
        if index > 0 && word.start - words[index - 1].start < MIN_WORD_SECS - 1e-9 {
            return false;
        }
        // A word is anchored when the stem's own onset confirmed it, or the
        // attention was sharp enough to trust on its own.
        let snapped = word.snap.is_some_and(|delta| delta.abs() <= 0.09);
        if snapped || word.score >= 0.5 {
            anchored += 1;
        }
        // One word eating the line is an assignment gone wrong.
        if words.len() >= 4 {
            let ends = words
                .get(index + 1)
                .map(|next| next.start)
                .unwrap_or(end)
                .max(word.start);
            if ends - word.start > span * 0.6 {
                return false;
            }
        }
    }
    anchored * 10 >= words.len() * 6
}

// ---------------------------------------------------------------------------
// the whole pipeline
// ---------------------------------------------------------------------------

pub struct PipelineConfig {
    pub language: String,
    /// Run pass 2 (teacher-forced windows).
    pub force: bool,
    /// Run stage 3 (onset snapping).
    pub snap: bool,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        PipelineConfig { language: "en".into(), force: true, snap: true }
    }
}

/// Pass 1 through the sanity layer in one call — what the lyrics bake runs.
/// `aligned` is `transcribe_aligned`'s output for the whole track,
/// `samples_16k` the same mixdown it transcribed, `mono` the stem at its own
/// rate for onsets.
pub fn refine(
    state: &mut WhisperState,
    model: &WhisperModel,
    samples_16k: &[f32],
    aligned: Vec<AlignedSegment>,
    analysis: &VocalAnalysis,
    duration_secs: f64,
    config: &PipelineConfig,
) -> (Vec<SegmentWords>, Vec<TimedLine>) {
    let mut segments = collect_words(aligned, analysis, duration_secs);
    enforce_monotonic(&mut segments);
    if config.force {
        force_align_segments(state, model, samples_16k, &mut segments, &config.language);
        enforce_monotonic(&mut segments);
    }
    rescue_absorbed_words(&mut segments, analysis);
    enforce_monotonic(&mut segments);
    if config.snap {
        snap_words(&mut segments, analysis);
    }
    let lines = assemble_lines(&segments, duration_secs);
    (segments, lines)
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_moves_no_time() {
        // A burst whose center must stay put through 44.1k → 16k: group
        // delay is the one constant offset this module cannot tolerate.
        let rate = 44_100.0;
        let mut input = vec![0.0f32; 44_100];
        let center = 22_050usize;
        for i in 0..441 {
            let t = i as f64 / 441.0 * std::f64::consts::PI;
            input[center - 220 + i] = (t.sin() * (i as f64 * 0.9).sin()) as f32;
        }
        let out = resample(&input, rate, 16_000.0);
        let expected = (center as f64 / rate * 16_000.0) as usize;
        let mut best = 0usize;
        let mut peak = 0.0f32;
        // Energy centroid over a window around the expected position.
        let mut num = 0.0f64;
        let mut den = 0.0f64;
        for (i, v) in out.iter().enumerate() {
            let e = (*v as f64) * (*v as f64);
            num += e * i as f64;
            den += e;
            if v.abs() > peak {
                peak = v.abs();
                best = i;
            }
        }
        let centroid = num / den.max(1e-12);
        assert!(
            (centroid - expected as f64).abs() < 16.0, // < 1 ms at 16 kHz
            "centroid {centroid:.1} vs expected {expected} (peak at {best})"
        );
    }

    fn tone_bursts(rate: f64, length: f64, onsets: &[f64]) -> Vec<f32> {
        let mut out = vec![0.0f32; (rate * length) as usize];
        for onset in onsets {
            let from = (rate * onset) as usize;
            let to = ((rate * (onset + 0.35)) as usize).min(out.len());
            for (k, sample) in out[from..to].iter_mut().enumerate() {
                // A plucky attack with harmonics, so the flux has an edge.
                let t = k as f64 / rate;
                let env = (-t * 12.0).exp();
                *sample += (env
                    * ((2.0 * std::f64::consts::PI * 660.0 * t).sin()
                        + 0.5 * (2.0 * std::f64::consts::PI * 1320.0 * t).sin()
                        + 0.3 * (2.0 * std::f64::consts::PI * 2640.0 * t).sin()))
                    as f32
                    * 0.5;
            }
        }
        out
    }

    #[test]
    fn onsets_land_on_the_attacks() {
        let rate = 44_100.0;
        let truth = [0.5, 1.2, 2.0, 3.1, 3.9];
        let audio = tone_bursts(rate, 5.0, &truth);
        for preset in [OnsetPreset::Snapping, OnsetPreset::GroundTruth] {
            let analysis = analyze_vocals(&audio, rate, preset);
            assert!(
                analysis.onsets.len() >= truth.len(),
                "{preset:?}: {:?}",
                analysis.onsets
            );
            for expected in truth {
                let found = analysis
                    .onsets
                    .iter()
                    .map(|onset| (onset.time - expected).abs())
                    .fold(f64::INFINITY, f64::min);
                assert!(
                    found < 0.03,
                    "{preset:?}: onset at {expected}s missed by {found:.3}s"
                );
            }
        }
    }

    #[test]
    fn silence_has_no_onsets_and_no_loudness() {
        let analysis = analyze_vocals(&vec![0.0f32; 44_100], 44_100.0, OnsetPreset::Snapping);
        assert!(analysis.onsets.is_empty());
        assert_eq!(analysis.loud, 0.0);
    }

    fn word(text: &str, start: f64, end: f64, score: f32) -> TimedWord {
        TimedWord { text: text.into(), start, end, score, snap: None, pass_delta: None }
    }

    #[test]
    fn lines_break_on_gaps_and_start_on_their_first_word() {
        // "give me a" flows straight into "man after midnight": tiny gap, so
        // the mid-phrase line boundary must sit exactly on "man"'s time.
        let segments = vec![
            SegmentWords {
                text: "Give me give me give me a".into(),
                start: 9.6, // whisper's stamp, deliberately early
                end: 12.4,
                words: vec![
                    word("Give", 10.0, 10.2, 0.9),
                    word("me", 10.2, 10.4, 0.9),
                    word("give", 10.4, 10.6, 0.9),
                    word("me", 10.6, 10.8, 0.9),
                    word("give", 10.8, 11.0, 0.9),
                    word("me", 11.0, 11.2, 0.9),
                    word("a", 11.2, 11.35, 0.9),
                ],
                tokens: Vec::new(),
                forced: false,
            },
            SegmentWords {
                text: "man after midnight".into(),
                start: 11.0, // stamp reaches back into the previous phrase
                end: 13.5,
                words: vec![
                    word("man", 11.42, 11.9, 0.9),
                    word("after", 11.9, 12.3, 0.9),
                    word("midnight", 12.3, 13.1, 0.9),
                ],
                tokens: Vec::new(),
                forced: false,
            },
        ];
        let lines = assemble_lines(&segments, 200.0);
        assert_eq!(lines.len(), 2, "{lines:?}");
        // The line the display lights up must begin when "man" is SUNG.
        assert!((lines[1].start - 11.42).abs() < 1e-9, "{lines:?}");
        assert!(lines[0].end <= lines[1].start + 1e-9);
        assert!(lines[1].confident);
        assert_eq!(lines[1].words.len(), 3);
    }

    #[test]
    fn a_long_segment_is_cut_into_readable_lines() {
        let mut words = Vec::new();
        for index in 0..14 {
            let at = 10.0 + index as f64 * 0.4;
            words.push(word(&format!("w{index}"), at, at + 0.3, 0.8));
        }
        // A real breath after word 6.
        for w in words.iter_mut().skip(7) {
            w.start += 1.0;
            w.end += 1.0;
        }
        let segments = vec![SegmentWords {
            text: words.iter().map(|w| w.text.clone()).collect::<Vec<_>>().join(" "),
            start: 10.0,
            end: 20.0,
            words,
            tokens: Vec::new(),
            forced: false,
        }];
        let lines = assemble_lines(&segments, 100.0);
        assert!(lines.len() >= 2, "{lines:?}");
        for line in &lines {
            assert!(line.words.len() <= MAX_LINE_WORDS, "{line:?}");
        }
        // The breath is one of the boundaries.
        assert!(
            lines.iter().any(|line| (line.start - (10.0 + 7.0 * 0.4 + 1.0)).abs() < 1e-6),
            "{lines:?}"
        );
    }

    #[test]
    fn a_smeared_line_is_not_trusted() {
        // Diffuse attention scores: sweeping is the honest rendering.
        let words = vec![
            word("all", 5.0, 5.3, 0.08),
            word("over", 5.3, 5.6, 0.09),
            word("the", 5.6, 5.9, 0.1),
            word("place", 5.9, 6.4, 0.07),
        ];
        let refs: Vec<&TimedWord> = words.iter().collect();
        assert!(!line_is_trustworthy(&refs));
        // …and one word swallowing the line fails even with sharp scores.
        let words = vec![
            word("a", 5.0, 5.05, 0.9),
            word("b", 5.06, 5.1, 0.9),
            word("c", 5.12, 5.2, 0.9),
            word("d", 5.25, 9.8, 0.9),
        ];
        let refs: Vec<&TimedWord> = words.iter().collect();
        assert!(!line_is_trustworthy(&refs));
    }

    #[test]
    fn monotonicity_is_enforced_across_segments() {
        let mut segments = vec![
            SegmentWords {
                text: "a b".into(),
                start: 1.0,
                end: 3.0,
                words: vec![word("a", 1.0, 1.4, 0.9), word("b", 2.8, 3.0, 0.9)],
                tokens: Vec::new(),
                forced: false,
            },
            SegmentWords {
                text: "c".into(),
                start: 2.0,
                end: 4.0,
                // Starts BEFORE the previous word — a force-align window
                // disagreement. Must be pushed after it.
                words: vec![word("c", 2.5, 2.9, 0.9)],
                tokens: Vec::new(),
                forced: false,
            },
        ];
        enforce_monotonic(&mut segments);
        assert!(segments[1].words[0].start >= segments[0].words[1].start + MIN_WORD_SECS - 1e-9);
        assert!(segments[0].words[1].end <= segments[1].words[0].start + 1e-9);
    }
}
