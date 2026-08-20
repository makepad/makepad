//! Cross-attention DTW token alignment — whisper's own word timestamps.
//!
//! The decoder computes cross-attention from every text token to every
//! encoder frame anyway; a handful of those heads (the "alignment heads")
//! attend, sharply, to the audio being spoken *while* it is being spoken.
//! Collect their softmaxed rows during decoding, normalize and median-filter
//! them the way OpenAI's `find_alignment` does, and a monotonic DTW path
//! through the token×frame matrix reads off when each token was said, on the
//! encoder's 20 ms grid. That is the mechanism behind whisper.cpp's
//! `token_timestamps` and OpenAI's `word_timestamps=True`, and it is the
//! principled core of karaoke word timing: the model tells us where it heard
//! each word, instead of us guessing from segment stamps.
//!
//! Capture is opt-in and CPU-only: when a capture is handed to the decoder
//! the fused accelerator paths for cross-attention step aside (they never
//! materialize the attention matrix), the plain path runs, and the selected
//! rows are copied out. Everything else — the encoder, the mel front end,
//! decoder self-attention — keeps whatever accelerator it had.

use crate::model::WhisperHparams;

/// One encoder position is two 10 ms mel frames.
pub const AUDIO_FRAME_MS: i64 = 20;

/// Median filter width over the frame axis, as in the reference
/// implementations (`medfilt_width = 7`).
const MEDIAN_WIDTH: usize = 7;

// ---------------------------------------------------------------------------
// which heads align
// ---------------------------------------------------------------------------

/// The (decoder layer, head) pairs whose cross-attention tracks the audio.
#[derive(Debug, Clone)]
pub struct AlignmentHeads {
    pub pairs: Vec<(usize, usize)>,
}

impl AlignmentHeads {
    /// The published alignment heads for the models we ship, by decoder
    /// shape. A ggml checkpoint does not carry the list (it lives in the HF
    /// `generation_config.json` / whisper.cpp's per-model tables), so the
    /// known shapes are pinned here and anything else falls back to every
    /// head of the top half of the decoder — the layers where alignment
    /// heads empirically live.
    pub fn for_model(hp: &WhisperHparams) -> AlignmentHeads {
        // large-v3-turbo: 4 decoder layers, 20 heads, state 1280.
        // whisper.cpp `g_aheads_large_v3_turbo`.
        if hp.n_text_layer == 4 && hp.n_text_state == 1280 && hp.n_text_head == 20 {
            return AlignmentHeads {
                pairs: vec![(2, 4), (2, 11), (3, 3), (3, 6), (3, 11), (3, 14)],
            };
        }
        let from = (hp.n_text_layer as usize) / 2;
        let mut pairs = Vec::new();
        for layer in from..hp.n_text_layer as usize {
            for head in 0..hp.n_text_head as usize {
                pairs.push((layer, head));
            }
        }
        AlignmentHeads { pairs }
    }
}

// ---------------------------------------------------------------------------
// capture
// ---------------------------------------------------------------------------

/// Softmaxed cross-attention rows for the alignment heads, one row per
/// decoded token position, appended in decode order (prompt tokens first,
/// then every generated token as it is fed back in).
pub struct AlignCapture {
    /// Encoder positions per row (1500 for a 30 s window).
    pub n_audio_ctx: usize,
    /// Captured heads.
    pub n_slots: usize,
    /// `[layer][head]` → slot index or -1.
    slots: Vec<Vec<i32>>,
    /// `[row][slot][n_audio_ctx]`, contiguous.
    rows: Vec<f32>,
    pub n_rows: usize,
}

impl AlignCapture {
    pub fn new(
        heads: &AlignmentHeads,
        n_layers: usize,
        n_heads: usize,
        n_audio_ctx: usize,
    ) -> AlignCapture {
        let mut slots = vec![vec![-1i32; n_heads]; n_layers];
        let mut n_slots = 0usize;
        for (layer, head) in &heads.pairs {
            if *layer < n_layers && *head < n_heads && slots[*layer][*head] < 0 {
                slots[*layer][*head] = n_slots as i32;
                n_slots += 1;
            }
        }
        AlignCapture {
            n_audio_ctx,
            n_slots,
            slots,
            rows: Vec::new(),
            n_rows: 0,
        }
    }

    /// Reserve rows for the tokens of one decode call. Returns the base row.
    pub(crate) fn begin_rows(&mut self, n_tokens: usize) -> usize {
        let base = self.n_rows;
        self.n_rows += n_tokens;
        self.rows.resize(self.n_rows * self.n_slots * self.n_audio_ctx, 0.0);
        base
    }

    /// Per-head slot table for one layer, or `None` when the layer holds no
    /// alignment head at all (lets the decoder skip the copy entirely).
    pub(crate) fn layer_slots(&self, layer: usize) -> Option<&[i32]> {
        let table = self.slots.get(layer)?;
        if table.iter().any(|slot| *slot >= 0) {
            Some(table)
        } else {
            None
        }
    }

    pub(crate) fn rows_ptr(&mut self) -> *mut f32 {
        self.rows.as_mut_ptr()
    }

    pub fn row(&self, row: usize, slot: usize) -> &[f32] {
        let at = (row * self.n_slots + slot) * self.n_audio_ctx;
        &self.rows[at..at + self.n_audio_ctx]
    }

    /// A raw-pointer writer for one decoder layer's cross-attention, or
    /// `None` when the layer carries no alignment head. Built AFTER
    /// [`Self::begin_rows`] so the buffer can no longer move; the writes go
    /// through a `SendPtr` because they happen inside the decoder's
    /// per-head `parallel_for`, at (row, slot) targets that are disjoint
    /// per head by construction.
    pub(crate) fn writer(
        &mut self,
        layer: usize,
        base_row: usize,
        n_audio_ctx: usize,
    ) -> Option<CaptureWrite> {
        let ptr = crate::tensor::SendPtr::new(self.rows_ptr());
        let slots = self.layer_slots(layer)?.to_vec();
        Some(CaptureWrite {
            ptr,
            slots,
            n_slots: self.n_slots,
            stride: self.n_audio_ctx,
            copy: self.n_audio_ctx.min(n_audio_ctx),
            base_row,
        })
    }
}

/// See [`AlignCapture::writer`].
pub(crate) struct CaptureWrite {
    ptr: crate::tensor::SendPtr,
    slots: Vec<i32>,
    n_slots: usize,
    stride: usize,
    copy: usize,
    base_row: usize,
}

impl CaptureWrite {
    /// Copy one head's softmaxed rows (`scores` is `[n_tokens, n_audio_ctx]`
    /// row-major). No-op for heads that are not alignment heads.
    pub(crate) fn store(&self, head: usize, n_tokens: usize, n_audio_ctx: usize, scores: &[f32]) {
        let Some(slot) = self.slots.get(head).copied().filter(|slot| *slot >= 0) else {
            return;
        };
        let slot = slot as usize;
        for token in 0..n_tokens {
            let source = &scores[token * n_audio_ctx..token * n_audio_ctx + self.copy];
            let at = ((self.base_row + token) * self.n_slots + slot) * self.stride;
            unsafe {
                // Safe: the buffer was sized by begin_rows before this
                // writer took its pointer, and every (token, slot) target is
                // written by exactly one head.
                std::ptr::copy_nonoverlapping(source.as_ptr(), self.ptr.ptr().add(at), self.copy);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// the matrix pipeline: crop → normalize → median filter → head mean
// ---------------------------------------------------------------------------

/// Median filter with reflected edges, matching the reference
/// `median_filter(weights, 7)`.
pub(crate) fn median_filter(row: &mut [f32], width: usize) {
    let len = row.len();
    if len == 0 || width < 3 {
        return;
    }
    let width = if width % 2 == 0 { width - 1 } else { width }.min(if len % 2 == 0 { len - 1 } else { len });
    if width < 3 {
        return;
    }
    let half = width / 2;
    let source = row.to_vec();
    let sample = |index: isize| -> f32 {
        // Reflect: -1 → 1, -2 → 2, len → len-2 …
        let len = len as isize;
        let mut at = index;
        if at < 0 {
            at = -at;
        }
        if at >= len {
            at = 2 * (len - 1) - at;
        }
        source[at.clamp(0, len - 1) as usize]
    };
    let mut window: Vec<f32> = Vec::with_capacity(width);
    for center in 0..len as isize {
        window.clear();
        for offset in -(half as isize)..=(half as isize) {
            window.push(sample(center + offset));
        }
        window.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        row[center as usize] = window[half];
    }
}

/// Monotonic DTW over `cost` (row-major `n × m`), returning for every row
/// the first and one-past-last column the path spends in it.
pub(crate) fn dtw_spans(cost: &[f32], n: usize, m: usize) -> (Vec<usize>, Vec<usize>) {
    debug_assert_eq!(cost.len(), n * m);
    // Accumulated cost, two rolling rows; full traceback matrix in u8.
    let mut trace = vec![0u8; (n + 1) * (m + 1)];
    let mut previous = vec![f32::INFINITY; m + 1];
    let mut current = vec![f32::INFINITY; m + 1];
    previous[0] = 0.0;
    for i in 1..=n {
        current[0] = f32::INFINITY;
        for j in 1..=m {
            let c0 = previous[j - 1]; // diagonal
            let c1 = previous[j]; // token advance, frame holds
            let c2 = current[j - 1]; // frame advance, token holds
            let (c, t) = if c0 < c1 && c0 < c2 {
                (c0, 0u8)
            } else if c1 < c0 && c1 < c2 {
                (c1, 1u8)
            } else {
                (c2, 2u8)
            };
            current[j] = cost[(i - 1) * m + (j - 1)] + c;
            trace[i * (m + 1) + j] = t;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    // Backtrace from (n, m); boundary rows force pure horizontal/vertical.
    for j in 0..=m {
        trace[j] = 2;
    }
    for i in 0..=n {
        trace[i * (m + 1)] = 1;
    }
    let mut starts = vec![0usize; n];
    let mut ends = vec![0usize; n];
    let (mut i, mut j) = (n, m);
    let mut last_row = n + 1; // sentinel: matches no row+1
    while i > 0 && j > 0 {
        let row = i - 1;
        let column = j - 1;
        if row + 1 != last_row {
            // First (walking backwards: latest) visit of this row.
            ends[row] = column + 1;
            last_row = row + 1;
        }
        starts[row] = column; // keeps shrinking to the row's first column
        match trace[i * (m + 1) + j] {
            0 => {
                i -= 1;
                j -= 1;
            }
            1 => i -= 1,
            _ => j -= 1,
        }
    }
    // A path can leave leading rows unvisited only through the forced
    // boundary; give any such row a zero-width span at its successor.
    for row in (0..n).rev() {
        if ends[row] == 0 {
            let next = if row + 1 < n { starts[row + 1] } else { 0 };
            starts[row] = next;
            ends[row] = next;
        }
    }
    (starts, ends)
}

/// `MAKEPAD_VOICE_ALIGN_SHARPEN` — attention temperature β for alignment
/// (see [`align_rows`]). Default 1.0.
fn sharpen_beta() -> f32 {
    static BETA: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *BETA.get_or_init(|| {
        std::env::var("MAKEPAD_VOICE_ALIGN_SHARPEN")
            .ok()
            .and_then(|text| text.trim().parse::<f32>().ok())
            .filter(|beta| beta.is_finite() && *beta > 0.0 && *beta <= 8.0)
            .unwrap_or(1.0)
    })
}

/// Where each captured row was heard: start/end frame on the 20 ms grid and
/// an attention-mass score in 0..1 (how much of the row's raw attention the
/// chosen frames hold — sharp, honest attention scores high; a smeared or
/// hallucinating row scores low).
pub struct TokenAlignment {
    pub starts: Vec<usize>,
    pub ends: Vec<usize>,
    pub scores: Vec<f32>,
}

/// Align capture rows `row_from..row_to` against the first `n_frames`
/// encoder positions (the ones covering real audio).
pub fn align_rows(
    capture: &AlignCapture,
    row_from: usize,
    row_to: usize,
    n_frames: usize,
) -> Option<TokenAlignment> {
    let n = row_to.saturating_sub(row_from);
    let m = n_frames.min(capture.n_audio_ctx);
    if n == 0 || m < 2 || capture.n_slots == 0 || row_to > capture.n_rows {
        return None;
    }
    // Raw head-mean, kept for the scores: the z-scored matrix the DTW runs
    // on is unitless, but "what share of this token's attention sits on the
    // frames the path chose" is a probability and means something.
    let mut raw = vec![0.0f32; n * m];
    let mut matrix = vec![0.0f32; n * m];
    let inverse_slots = 1.0 / capture.n_slots as f32;
    let mut filtered = vec![0.0f32; n * m];
    // Attention temperature. `p^β` renormalized IS softmax at β× the logit
    // scale, so the knob can sharpen (β>1) a path that singing has smeared
    // across doubled vocals without touching the decoder. 1 = off.
    let sharpen = sharpen_beta();
    for slot in 0..capture.n_slots {
        // Crop to the audible frames, then z-score each frame column across
        // the tokens, exactly as the reference does (`std_mean(dim=-2)`).
        for i in 0..n {
            let row = capture.row(row_from + i, slot);
            for j in 0..m {
                filtered[i * m + j] = row[j];
                raw[i * m + j] += row[j] * inverse_slots;
            }
            if (sharpen - 1.0).abs() > 1e-6 {
                let target = &mut filtered[i * m..(i + 1) * m];
                let mut sum = 0.0f64;
                for value in target.iter_mut() {
                    *value = value.max(0.0).powf(sharpen);
                    sum += *value as f64;
                }
                if sum > 0.0 {
                    let inverse = (1.0 / sum) as f32;
                    for value in target.iter_mut() {
                        *value *= inverse;
                    }
                }
            }
        }
        for j in 0..m {
            let mut mean = 0.0f64;
            for i in 0..n {
                mean += filtered[i * m + j] as f64;
            }
            mean /= n as f64;
            let mut variance = 0.0f64;
            for i in 0..n {
                let d = filtered[i * m + j] as f64 - mean;
                variance += d * d;
            }
            let std = (variance / n as f64).sqrt().max(1e-8);
            for i in 0..n {
                filtered[i * m + j] = ((filtered[i * m + j] as f64 - mean) / std) as f32;
            }
        }
        for i in 0..n {
            median_filter(&mut filtered[i * m..(i + 1) * m], MEDIAN_WIDTH);
        }
        for (out, value) in matrix.iter_mut().zip(filtered.iter()) {
            *out += *value * inverse_slots;
        }
    }
    // DTW wants cost, attention is affinity.
    for value in &mut matrix {
        *value = -*value;
    }
    let (starts, ends) = dtw_spans(&matrix, n, m);
    let mut scores = vec![0.0f32; n];
    for i in 0..n {
        let total: f32 = raw[i * m..(i + 1) * m].iter().sum();
        let span = &raw[i * m + starts[i]..i * m + ends[i].max(starts[i])];
        let mass: f32 = span.iter().sum();
        scores[i] = if total > 0.0 { (mass / total).clamp(0.0, 1.0) } else { 0.0 };
    }
    Some(TokenAlignment { starts, ends, scores })
}

// ---------------------------------------------------------------------------
// words
// ---------------------------------------------------------------------------

/// One whitespace-separated word with the time it was sung.
///
/// The split is `str::split_whitespace` over the segment text — exactly the
/// units a karaoke display colours — with each word's time read off the DTW
/// spans of the tokens its characters came from.
#[derive(Debug, Clone, PartialEq)]
pub struct WordSpan {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    /// Mean attention-mass of the word's tokens, 0..1. Low means the model
    /// was not really looking at the audio when it wrote this word.
    pub score: f32,
}

/// Split the concatenation of `texts` (aligned-row index, token text) into
/// whitespace words, timing each from the rows its characters belong to.
pub fn words_from_tokens(
    texts: &[(usize, &str)],
    alignment: &TokenAlignment,
    base_ms: i64,
) -> Vec<WordSpan> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut first_row: Option<usize> = None;
    let mut last_row = 0usize;
    let flush =
        |word: &mut String, first_row: &mut Option<usize>, last_row: usize, out: &mut Vec<WordSpan>| {
            if word.is_empty() {
                return;
            }
            let Some(first) = first_row.take() else {
                word.clear();
                return;
            };
            let last = last_row.max(first).min(alignment.starts.len() - 1);
            let start = alignment.starts[first];
            let end = alignment.ends[last].max(start);
            let mut score = 0.0f32;
            for row in first..=last {
                score += alignment.scores[row];
            }
            score /= (last - first + 1) as f32;
            out.push(WordSpan {
                text: std::mem::take(word),
                start_ms: base_ms + start as i64 * AUDIO_FRAME_MS,
                end_ms: base_ms + end as i64 * AUDIO_FRAME_MS,
                score,
            });
        };
    let mut truncated = false;
    for (row, text) in texts {
        if *row >= alignment.starts.len() {
            // A token past the aligned rows (a generation cut off before its
            // last token was ever fed back). Emitting the in-progress word
            // would emit PARTIAL text; drop it instead — the caller treats a
            // word-count mismatch as "not aligned", which is the truth.
            truncated = true;
            break;
        }
        for character in text.chars() {
            if character.is_whitespace() {
                flush(&mut word, &mut first_row, last_row, &mut out);
            } else {
                if word.is_empty() {
                    first_row = Some(*row);
                }
                last_row = *row;
                word.push(character);
            }
        }
    }
    if !truncated {
        flush(&mut word, &mut first_row, last_row, &mut out);
    }
    out
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_filter_smooths_a_spike_and_keeps_a_step() {
        let mut spike = vec![0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        median_filter(&mut spike, 7);
        assert!(spike.iter().all(|v| *v == 0.0), "{spike:?}");
        let mut step = vec![0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        median_filter(&mut step, 7);
        // A step survives a median; its edge may shift by at most half the
        // window.
        assert_eq!(step[0], 0.0);
        assert_eq!(step[9], 1.0);
        let rises = step.windows(2).filter(|w| w[1] > w[0]).count();
        assert_eq!(rises, 1, "{step:?}");
    }

    #[test]
    fn dtw_follows_a_clean_diagonal() {
        // 4 tokens over 12 frames, each token "attending" to its own 3-frame
        // stretch: the path must give each row its stretch, in order.
        // Off-path cells cost a little (a z-scored attention matrix puts
        // inactive cells below the column mean, so their DTW cost is
        // positive) — without that, equal-cost paths that clip a corner of
        // a neighbouring row are legitimate and the test is degenerate.
        let (n, m) = (4usize, 12usize);
        let mut cost = vec![0.0f32; n * m];
        for i in 0..n {
            for j in 0..m {
                let on = j >= i * 3 && j < (i + 1) * 3;
                cost[i * m + j] = if on { -1.0 } else { 0.1 };
            }
        }
        let (starts, ends) = dtw_spans(&cost, n, m);
        for i in 0..n {
            assert_eq!(starts[i], i * 3, "row {i}: {starts:?}");
            assert_eq!(ends[i], (i + 1) * 3, "row {i}: {ends:?}");
        }
        // Monotonic, gap-free coverage.
        for i in 1..n {
            assert_eq!(ends[i - 1], starts[i]);
        }
    }

    #[test]
    fn dtw_a_held_token_gets_the_whole_hold() {
        // Token 1 is sung across 8 of 12 frames — a held word.
        let (n, m) = (3usize, 12usize);
        let mut cost = vec![0.1f32; n * m];
        for j in 0..2 {
            cost[j] = -1.0;
        }
        for j in 2..10 {
            cost[m + j] = -1.0;
        }
        for j in 10..12 {
            cost[2 * m + j] = -1.0;
        }
        let (starts, ends) = dtw_spans(&cost, n, m);
        assert_eq!((starts[1], ends[1]), (2, 10), "{starts:?} {ends:?}");
    }

    fn synthetic_capture(n_rows: usize, n_frames: usize, stretch: usize) -> AlignCapture {
        let heads = AlignmentHeads { pairs: vec![(0, 0), (0, 1)] };
        let mut capture = AlignCapture::new(&heads, 1, 2, n_frames);
        for row in 0..n_rows {
            let base = capture.begin_rows(1);
            for slot in 0..capture.n_slots {
                let at = (base * capture.n_slots + slot) * capture.n_audio_ctx;
                for j in 0..n_frames {
                    let on = j >= row * stretch && j < (row + 1) * stretch;
                    capture.rows[at + j] = if on { 0.9 } else { 0.001 };
                }
            }
        }
        capture
    }

    #[test]
    fn align_rows_reads_the_diagonal_and_scores_it_sharp() {
        let capture = synthetic_capture(5, 40, 8);
        let alignment = align_rows(&capture, 0, 5, 40).expect("alignment");
        for row in 0..5 {
            assert!(
                (alignment.starts[row] as isize - (row * 8) as isize).abs() <= 3,
                "row {row} starts at {} wanted ~{}",
                alignment.starts[row],
                row * 8
            );
            assert!(alignment.scores[row] > 0.5, "{:?}", alignment.scores);
        }
    }

    #[test]
    fn align_rows_survives_degenerate_input() {
        let heads = AlignmentHeads { pairs: vec![(0, 0)] };
        let mut capture = AlignCapture::new(&heads, 1, 1, 16);
        assert!(align_rows(&capture, 0, 0, 16).is_none());
        let _ = capture.begin_rows(2);
        // All-zero rows: constant columns, std floor engages, no NaN.
        let alignment = align_rows(&capture, 0, 2, 16).expect("alignment");
        assert!(alignment.starts.iter().all(|s| *s < 16));
        assert!(alignment.scores.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn words_split_exactly_like_the_display_and_carry_token_times() {
        // Tokens: " Danc", "ing", " queen,", " young" — two rows share a word.
        let alignment = TokenAlignment {
            starts: vec![10, 14, 20, 30],
            ends: vec![14, 18, 28, 34],
            scores: vec![0.9, 0.8, 0.7, 0.6],
        };
        let texts = vec![(0usize, " Danc"), (1, "ing"), (2, " queen,"), (3, " young")];
        let words = words_from_tokens(&texts, &alignment, 1000);
        let text: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(text, vec!["Dancing", "queen,", "young"]);
        assert_eq!(words[0].start_ms, 1000 + 10 * AUDIO_FRAME_MS);
        assert_eq!(words[0].end_ms, 1000 + 18 * AUDIO_FRAME_MS);
        assert!((words[0].score - 0.85).abs() < 1e-6);
        assert_eq!(words[1].start_ms, 1000 + 20 * AUDIO_FRAME_MS);
        // split_whitespace parity with the karaoke renderer.
        let joined = texts.iter().map(|(_, t)| *t).collect::<String>();
        let display: Vec<&str> = joined.split_whitespace().collect();
        assert_eq!(text, display);
    }

    #[test]
    fn alignment_heads_know_the_turbo_and_fall_back_sanely() {
        let turbo = WhisperHparams {
            n_vocab: 51866,
            n_audio_ctx: 1500,
            n_audio_state: 1280,
            n_audio_head: 20,
            n_audio_layer: 32,
            n_text_ctx: 448,
            n_text_state: 1280,
            n_text_head: 20,
            n_text_layer: 4,
            n_mels: 128,
            ftype: 1,
        };
        let heads = AlignmentHeads::for_model(&turbo);
        assert_eq!(
            heads.pairs,
            vec![(2, 4), (2, 11), (3, 3), (3, 6), (3, 11), (3, 14)]
        );
        let mut tiny = turbo.clone();
        tiny.n_text_layer = 6;
        tiny.n_text_head = 8;
        tiny.n_text_state = 512;
        let fallback = AlignmentHeads::for_model(&tiny);
        assert_eq!(fallback.pairs.len(), 3 * 8);
        assert!(fallback.pairs.iter().all(|(layer, _)| *layer >= 3));
    }
}
