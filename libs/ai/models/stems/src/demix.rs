//! Chunked overlap-add, expressed as a STREAM.
//!
//! The reference (`utils.model_utils.demix`, generic path) is a batch loop that
//! reflect-pads the whole track, walks fixed windows with 2x overlap, and
//! divides an accumulator by a window-sum counter at the end. Every step of
//! that is local in time, so the same arithmetic runs incrementally: after the
//! chunk starting at `c*STEP` is folded in, the samples in
//! `[c*STEP, (c+1)*STEP)` can no longer be touched by any later chunk and are
//! final. `Demixer::next_span` emits exactly that span.
//!
//! Consequences the caller gets for free:
//! * playback can start after ~2 chunks instead of after the whole track;
//! * `seek` restarts the cursor at the chunk boundary covering the target and
//!   costs the two chunks that overlap it, not a re-demix of the track;
//! * running the stream to the end produces the reference's batch result
//!   sample-for-sample (`demix_all`).

use crate::config::*;
use crate::model::{empty_stem_set, StemSet, StemsModel, StereoBuf};
use makepad_ai_common::{DiffusionError, Result};

/// One finished span of separated audio.
#[derive(Clone, Debug)]
pub struct StemSpan {
    /// First sample of this span in TRACK coordinates.
    pub start: usize,
    /// Separated audio, `Stem::ALL` order, `frames` long.
    pub stems: StemSet,
}

impl StemSpan {
    pub fn frames(&self) -> usize {
        self.stems[0].frames()
    }
}

/// Reflect padding as `torch.nn.functional.pad(mode="reflect")` does it: the
/// edge sample is not repeated, so index -1 mirrors to 1 and `n` mirrors to
/// `n-2`.
fn reflect_index(index: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    if len == 1 {
        return 0;
    }
    let period = 2 * (len as isize - 1);
    let mut i = index % period;
    if i < 0 {
        i += period;
    }
    if i >= len as isize {
        i = period - i;
    }
    i as usize
}

/// The reference only pads the track when it is longer than `2 * BORDER`.
fn track_padding(len: usize) -> usize {
    if len > 2 * BORDER {
        BORDER
    } else {
        0
    }
}

/// Number of chunks the reference loop runs for a track of `len` samples.
pub fn chunk_count(len: usize) -> usize {
    let padded = len + 2 * track_padding(len);
    if padded == 0 {
        return 0;
    }
    padded.div_ceil(CHUNK_STEP)
}

/// The reference's `_getWindowingArray`: linear fade in/out, ones between.
fn base_window() -> Vec<f32> {
    let mut w = vec![1.0f32; CHUNK_SAMPLES];
    for i in 0..FADE_SAMPLES {
        let t = i as f32 / (FADE_SAMPLES - 1) as f32;
        w[i] = t;
        w[CHUNK_SAMPLES - FADE_SAMPLES + i] = 1.0 - t;
    }
    w
}

/// Streaming separator over one track.
pub struct Demixer<'a> {
    model: &'a mut StemsModel,
    track: &'a StereoBuf,
    len: usize,
    pad: usize,
    padded_len: usize,
    window: Vec<f32>,
    /// Index of the next chunk to run.
    next_chunk: usize,
    /// Padded-domain sample index the accumulator starts at.
    acc_start: usize,
    /// Per stem, per channel, `CHUNK_SAMPLES` of pending overlap-add.
    acc: Vec<Vec<f32>>,
    /// Window-sum counter shared by every stem/channel.
    counter: Vec<f32>,
    /// Scratch chunk handed to the model.
    chunk: StereoBuf,
    /// Spans to compute and throw away before returning one — the price of
    /// starting mid-track (see `reset_to_span`).
    discard: usize,
}

impl<'a> Demixer<'a> {
    pub fn new(model: &'a mut StemsModel, track: &'a StereoBuf) -> Result<Self> {
        let len = track.frames();
        if track.left.len() != track.right.len() {
            return Err(DiffusionError::model(
                "stems: track channels have different lengths",
            ));
        }
        let pad = track_padding(len);
        let mut demixer = Self {
            model,
            track,
            len,
            pad,
            padded_len: len + 2 * pad,
            window: base_window(),
            next_chunk: 0,
            acc_start: 0,
            acc: vec![vec![0.0; CHUNK_SAMPLES]; NUM_STEMS * AUDIO_CHANNELS],
            counter: vec![0.0; CHUNK_SAMPLES],
            chunk: StereoBuf::silence(CHUNK_SAMPLES),
            discard: 0,
        };
        // The reference only ever pads by exactly one step, which is what lets
        // a span index be a chunk index shifted by one.
        debug_assert!(demixer.pad == 0 || demixer.pad == CHUNK_STEP);
        demixer.reset_to_span(0);
        Ok(demixer)
    }

    /// Model forwards this track needs end to end.
    pub fn chunk_count(&self) -> usize {
        chunk_count(self.len)
    }

    /// Track spans this stream will hand back — `ceil(len / CHUNK_STEP)`, the
    /// same indexing the on-disk cache uses.
    pub fn span_count(&self) -> usize {
        self.len.div_ceil(CHUNK_STEP)
    }

    /// Track-span index containing the given TRACK sample.
    pub fn span_of_track_sample(&self, sample: usize) -> usize {
        sample / CHUNK_STEP
    }

    /// Restart the stream so the next returned span is the one containing
    /// `sample`. Costs two model forwards (the span overlaps two chunks);
    /// spans already computed are the caller's to cache — this only moves the
    /// cursor.
    pub fn seek(&mut self, sample: usize) {
        let span = self.span_of_track_sample(sample.min(self.len.saturating_sub(1)));
        self.reset_to_span(span);
    }

    fn reset_to_span(&mut self, span: usize) {
        for buf in self.acc.iter_mut() {
            buf.fill(0.0);
        }
        self.counter.fill(0.0);
        // Padded-domain start of the requested span.
        let want = span * CHUNK_STEP + self.pad;
        // A span is covered by the chunk that starts on it AND the one before,
        // so starting anywhere but the very beginning means computing the
        // PREVIOUS span first (it comes out wrong, having no predecessor of its
        // own) and discarding it. Its chunk is what seeds the accumulator.
        if want == 0 {
            self.acc_start = 0;
            self.discard = 0;
        } else {
            self.acc_start = want - CHUNK_STEP;
            self.discard = 1;
        }
        self.next_chunk = self.acc_start / CHUNK_STEP;
    }

    /// Folds in every chunk that can still touch the span at `acc_start`. A
    /// chunk starting after `acc_start` begins at or after the span's end, so
    /// the span is final once all chunks with `start <= acc_start` are in.
    fn fold_covering_chunks(&mut self) -> Result<()> {
        while self.next_chunk < self.chunk_count()
            && self.next_chunk * CHUNK_STEP <= self.acc_start
        {
            self.run_chunk(self.next_chunk)?;
            self.next_chunk += 1;
        }
        Ok(())
    }

    pub fn finished(&self) -> bool {
        self.acc_start >= self.padded_len
    }

    /// Runs however many chunks are needed and returns the next finished span,
    /// or `None` once the track is exhausted.
    ///
    /// Spans that fall entirely inside the reference's leading reflect pad
    /// carry no track audio; they are consumed silently rather than handed
    /// back, so every returned span has `frames() > 0` and a `start` on the
    /// `CHUNK_STEP` grid.
    pub fn next_span(&mut self) -> Result<Option<StemSpan>> {
        loop {
            if self.finished() {
                return Ok(None);
            }
            self.fold_covering_chunks()?;
            let span = self.emit_span();
            if self.discard > 0 {
                self.discard -= 1;
                continue;
            }
            if span.frames() > 0 {
                return Ok(Some(span));
            }
        }
    }

    fn run_chunk(&mut self, chunk: usize) -> Result<()> {
        let start = chunk * CHUNK_STEP;
        let available = self.padded_len.saturating_sub(start);
        let chunk_len = available.min(CHUNK_SAMPLES);
        for ch in 0..AUDIO_CHANNELS {
            let src = self.track.channel(ch);
            let dst = self.chunk.channel_mut(ch);
            for i in 0..chunk_len {
                let padded = (start + i) as isize - self.pad as isize;
                dst[i] = src[reflect_index(padded, self.len)];
            }
            // The reference reflect-pads the tail only when more than half the
            // window is real audio; otherwise it zero-pads.
            if chunk_len < CHUNK_SAMPLES {
                if chunk_len > CHUNK_SAMPLES / 2 {
                    for i in chunk_len..CHUNK_SAMPLES {
                        let mirror = chunk_len as isize - 2 - (i - chunk_len) as isize;
                        dst[i] = dst[mirror.max(0) as usize];
                    }
                } else {
                    for value in dst[chunk_len..].iter_mut() {
                        *value = 0.0;
                    }
                }
            }
        }

        let stems = self.model.separate_chunk(&self.chunk)?;

        let is_first = chunk == 0;
        let is_last = start + CHUNK_STEP >= self.padded_len;
        let offset = start - self.acc_start;
        for i in 0..CHUNK_SAMPLES {
            let at = offset + i;
            if at >= CHUNK_SAMPLES {
                break;
            }
            if i >= chunk_len {
                break;
            }
            let w = window_at(&self.window, i, is_first, is_last);
            self.counter[at] += w;
            for stem in 0..NUM_STEMS {
                for ch in 0..AUDIO_CHANNELS {
                    self.acc[stem * AUDIO_CHANNELS + ch][at] += stems[stem].channel(ch)[i] * w;
                }
            }
        }
        Ok(())
    }

    fn emit_span(&mut self) -> StemSpan {
        let padded_end = (self.acc_start + CHUNK_STEP).min(self.padded_len);
        let span_len = padded_end - self.acc_start;

        // Padded-domain [acc_start, padded_end) -> track domain, dropping the
        // reflect padding at both ends.
        let track_start = self.acc_start.saturating_sub(self.pad);
        let lead = self.pad.saturating_sub(self.acc_start);
        let track_end = (padded_end.saturating_sub(self.pad)).min(self.len);
        let out_len = track_end.saturating_sub(track_start);

        let mut stems = empty_stem_set(out_len);
        for stem in 0..NUM_STEMS {
            for ch in 0..AUDIO_CHANNELS {
                let acc = &self.acc[stem * AUDIO_CHANNELS + ch];
                let out = stems[stem].channel_mut(ch);
                for i in 0..out_len {
                    let at = lead + i;
                    let c = self.counter[at];
                    out[i] = if c > 0.0 { acc[at] / c } else { 0.0 };
                }
            }
        }

        // Slide the accumulator forward by one step.
        for buf in self.acc.iter_mut() {
            buf.copy_within(span_len.., 0);
            let keep = CHUNK_SAMPLES - span_len;
            buf[keep..].fill(0.0);
        }
        self.counter.copy_within(span_len.., 0);
        let keep = CHUNK_SAMPLES - span_len;
        self.counter[keep..].fill(0.0);
        self.acc_start = padded_end;

        StemSpan {
            start: track_start,
            stems,
        }
    }
}

fn window_at(window: &[f32], i: usize, is_first: bool, is_last: bool) -> f32 {
    if is_first && i < FADE_SAMPLES {
        return 1.0;
    }
    if is_last && i >= CHUNK_SAMPLES - FADE_SAMPLES {
        return 1.0;
    }
    window[i]
}

/// Runs the stream to completion — bit-for-bit the reference's batch result.
pub fn demix_all(
    model: &mut StemsModel,
    track: &StereoBuf,
    mut progress: impl FnMut(usize, usize),
) -> Result<StemSet> {
    let len = track.frames();
    let mut out = empty_stem_set(len);
    let mut demixer = Demixer::new(model, track)?;
    let total = demixer.chunk_count();
    let mut done = 0usize;
    while let Some(span) = demixer.next_span()? {
        for stem in 0..NUM_STEMS {
            for ch in 0..AUDIO_CHANNELS {
                let src = span.stems[stem].channel(ch);
                let dst = out[stem].channel_mut(ch);
                let end = (span.start + src.len()).min(len);
                if span.start < end {
                    dst[span.start..end].copy_from_slice(&src[..end - span.start]);
                }
            }
        }
        done += 1;
        progress(done, total);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflect_matches_torch_semantics() {
        // torch: pad([0,1,2,3,4], (2,2), mode='reflect') == [2,1,0,1,2,3,4,3,2]
        let len = 5;
        let got: Vec<usize> = (-2isize..(len as isize + 2))
            .map(|i| reflect_index(i, len))
            .collect();
        assert_eq!(got, vec![2, 1, 0, 1, 2, 3, 4, 3, 2]);
    }

    #[test]
    fn reflect_is_periodic_and_in_range() {
        for len in [2usize, 3, 7, 100] {
            for i in -300isize..300 {
                let r = reflect_index(i, len);
                assert!(r < len, "len {len} index {i} -> {r}");
            }
        }
        assert_eq!(reflect_index(-5, 1), 0);
        assert_eq!(reflect_index(9, 0), 0);
    }

    #[test]
    fn short_tracks_are_not_padded() {
        assert_eq!(track_padding(2 * BORDER), 0);
        assert_eq!(track_padding(2 * BORDER + 1), BORDER);
    }

    #[test]
    fn window_has_ramps_and_a_flat_middle() {
        let w = base_window();
        assert_eq!(w.len(), CHUNK_SAMPLES);
        assert!(w[0].abs() < 1e-6);
        assert!((w[FADE_SAMPLES - 1] - 1.0).abs() < 1e-6);
        assert!((w[CHUNK_SAMPLES / 2] - 1.0).abs() < 1e-6);
        assert!(w[CHUNK_SAMPLES - 1].abs() < 1e-6);
        // Monotone up then down.
        assert!(w[..FADE_SAMPLES].windows(2).all(|p| p[0] <= p[1]));
        assert!(w[CHUNK_SAMPLES - FADE_SAMPLES..]
            .windows(2)
            .all(|p| p[0] >= p[1]));
    }

    #[test]
    fn window_edges_are_unfaded_on_the_first_and_last_chunk() {
        let w = base_window();
        assert_eq!(window_at(&w, 0, true, false), 1.0);
        assert_eq!(window_at(&w, 0, false, false), w[0]);
        let last = CHUNK_SAMPLES - 1;
        assert_eq!(window_at(&w, last, false, true), 1.0);
        assert_eq!(window_at(&w, last, false, false), w[last]);
    }

    #[test]
    fn overlap_add_of_the_two_covering_windows_is_everywhere_positive() {
        // The divide in `emit_span` must never see a zero counter in the
        // interior, otherwise the reference's `nan_to_num` would be masking a
        // hole rather than an edge.
        let w = base_window();
        for i in 0..CHUNK_STEP {
            let sum = w[i + CHUNK_STEP] + w[i];
            assert!(sum > 0.5, "counter dip at {i}: {sum}");
        }
    }

    #[test]
    fn chunk_count_covers_the_padded_track() {
        // 4 minutes at 44.1k, the gate fixture.
        let len = 240 * SAMPLE_RATE as usize;
        assert_eq!(chunk_count(len), 46);
        // A track shorter than the pad threshold still runs at least one chunk.
        assert_eq!(chunk_count(1000), 1);
        assert_eq!(chunk_count(0), 0);
    }
}
