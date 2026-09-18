//! Chunked overlap-add, expressed as a STREAM.
//!
//! The reference (`utils.model_utils.demix`, generic path) is a batch loop that
//! reflect-pads the whole track, walks fixed windows with 2x overlap, and
//! divides an accumulator by a window-sum counter at the end. Every step of
//! that is local in time, so the same arithmetic runs incrementally: after the
//! chunk starting at `c*step` is folded in, the samples in
//! `[c*step, (c+1)*step)` can no longer be touched by any later chunk and are
//! final. `DemixCursor::next_span` emits exactly that span.
//!
//! Consequences the caller gets for free:
//! * playback can start after ~2 chunks instead of after the whole track;
//! * `seek` restarts the cursor at the chunk boundary covering the target and
//!   costs the two chunks that overlap it, not a re-demix of the track;
//! * running the stream to the end produces the reference's batch result
//!   sample-for-sample (`demix_all`).
//!
//! The arithmetic is the same at every [`ChunkGeometry`]; a cursor takes the
//! geometry of the model that drives it. A [`DemixCursor`] borrows nothing,
//! so one thread can hold a cursor per track against one model and advance
//! whichever it likes; [`Demixer`] ties a cursor to one model and one track
//! for the callers that want the stream and nothing else.
//!
//! None of it depends on how many sources the model returns either. The
//! window, the counter, the padding and the span grid belong to the chunk;
//! a source is only one more pair of accumulators folded with the same
//! weights. So the arithmetic lives once, in [`LaneCursor`], over any
//! [`ChunkSeparator`], and the four-stem names above are that cursor held at
//! four lanes with its spans handed back as a [`StemSet`]. A separator with
//! one target streams through [`LaneDemixer`] on exactly the same code.

use crate::config::*;
use crate::model::{StemSet, StemsModel, StereoBuf};
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

    /// The four-stem view of a span the shared cursor finished. The buffers
    /// are moved, not copied.
    fn from_lanes(span: LaneSpan) -> Result<StemSpan> {
        let start = span.start;
        let stems: StemSet = span.lanes.try_into().map_err(|lanes: Vec<StereoBuf>| {
            DiffusionError::model(format!(
                "stems: a four-stem span came back with {} lanes",
                lanes.len()
            ))
        })?;
        Ok(StemSpan { start, stems })
    }
}

/// One finished span from a separator of any number of targets.
#[derive(Clone, Debug)]
pub struct LaneSpan {
    /// First sample of this span in TRACK coordinates.
    pub start: usize,
    /// Separated audio, one buffer per target in the separator's own order,
    /// each `frames` long.
    pub lanes: Vec<StereoBuf>,
}

impl LaneSpan {
    pub fn frames(&self) -> usize {
        self.lanes.first().map_or(0, |lane| lane.frames())
    }
}

/// What the overlap-add needs of a model: the chunk it consumes, how many
/// sources it returns, and the separation of one chunk. The stream is
/// written against this and nothing else, so a second separator gets the
/// padding, the windowing, the span grid and the seek semantics without
/// repeating any of them.
pub trait ChunkSeparator {
    /// The chunk this separator was compiled for.
    fn geometry(&self) -> ChunkGeometry;

    /// Sources returned per chunk. Constant for the life of the separator.
    fn targets(&self) -> usize;

    /// Separates exactly one chunk of [`geometry`](Self::geometry) samples
    /// into [`targets`](Self::targets) buffers of the same length, always in
    /// the same order.
    fn separate(&mut self, chunk: &StereoBuf) -> Result<Vec<StereoBuf>>;
}

impl ChunkSeparator for StemsModel {
    fn geometry(&self) -> ChunkGeometry {
        StemsModel::geometry(self)
    }

    fn targets(&self) -> usize {
        NUM_STEMS
    }

    /// The four stems in `Stem::ALL` order. Only the four buffer handles
    /// move into the list; no sample is copied.
    fn separate(&mut self, chunk: &StereoBuf) -> Result<Vec<StereoBuf>> {
        Ok(Vec::from(self.separate_chunk(chunk)?))
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

/// Number of chunks the reference loop runs for a track of `len` samples, at
/// the full geometry.
pub fn chunk_count(len: usize) -> usize {
    ChunkGeometry::FULL.chunk_count(len)
}

/// The reference's `_getWindowingArray`: linear fade in/out, ones between.
fn base_window(geometry: ChunkGeometry) -> Vec<f32> {
    let mut w = vec![1.0f32; geometry.samples];
    let fade = geometry.fade;
    for i in 0..fade {
        let t = i as f32 / (fade - 1) as f32;
        w[i] = t;
        w[geometry.samples - fade + i] = 1.0 - t;
    }
    w
}

/// The stream's position and pending overlap-add over one track, for a
/// separator of any number of targets, holding no model. Advance it with
/// [`next_span`](Self::next_span) against a separator of the same geometry
/// and target count; the track is handed in each time as well, so the cursor
/// owns neither and a worker can keep several against one model.
///
/// This is the one implementation of the arithmetic. Every lane is folded
/// with the same window and divided by the same counter, each in its own
/// accumulator, so what a lane comes out as does not depend on how many
/// others ran beside it.
pub struct LaneCursor {
    geometry: ChunkGeometry,
    targets: usize,
    len: usize,
    pad: usize,
    padded_len: usize,
    window: Vec<f32>,
    /// Index of the next chunk to run.
    next_chunk: usize,
    /// Padded-domain sample index the accumulator starts at.
    acc_start: usize,
    /// Per lane, per channel, one chunk of pending overlap-add.
    acc: Vec<Vec<f32>>,
    /// Window-sum counter shared by every lane/channel.
    counter: Vec<f32>,
    /// Scratch chunk handed to the model.
    chunk: StereoBuf,
    /// Spans to compute and throw away before returning one — the price of
    /// starting mid-track (see `reset_to_span`).
    discard: usize,
}

impl LaneCursor {
    pub fn new(geometry: ChunkGeometry, targets: usize, track: &StereoBuf) -> Result<Self> {
        // A stream with no lanes would run every forward and hand back
        // nothing; that is a caller's mistake, not an empty result.
        if targets == 0 {
            return Err(DiffusionError::model(
                "stems: a separator must return at least one target",
            ));
        }
        if track.left.len() != track.right.len() {
            return Err(DiffusionError::model(
                "stems: track channels have different lengths",
            ));
        }
        let len = track.frames();
        let pad = geometry.track_padding(len);
        // The reference only ever pads by exactly one step, which is what lets
        // a span index be a chunk index shifted by one. A geometry whose
        // border is not its step would put every span off its grid.
        if pad != 0 && pad != geometry.step {
            return Err(DiffusionError::model(format!(
                "stems: chunk geometry pads by {pad} samples, not its step of {}",
                geometry.step
            )));
        }
        let mut cursor = Self {
            geometry,
            targets,
            len,
            pad,
            padded_len: len + 2 * pad,
            window: base_window(geometry),
            next_chunk: 0,
            acc_start: 0,
            acc: vec![vec![0.0; geometry.samples]; targets * AUDIO_CHANNELS],
            counter: vec![0.0; geometry.samples],
            chunk: StereoBuf::silence(geometry.samples),
            discard: 0,
        };
        cursor.reset_to_span(0);
        Ok(cursor)
    }

    pub fn geometry(&self) -> ChunkGeometry {
        self.geometry
    }

    /// Lanes in every span this cursor hands back.
    pub fn targets(&self) -> usize {
        self.targets
    }

    /// Track length in samples, as handed to `new`.
    pub fn track_len(&self) -> usize {
        self.len
    }

    /// Model forwards this track needs end to end.
    pub fn chunk_count(&self) -> usize {
        self.geometry.chunk_count(self.len)
    }

    /// Track spans this stream will hand back — `ceil(len / step)`. A span
    /// cache on this geometry's step uses the same indexing.
    pub fn span_count(&self) -> usize {
        self.len.div_ceil(self.geometry.step)
    }

    /// Track-span index containing the given TRACK sample.
    pub fn span_of_track_sample(&self, sample: usize) -> usize {
        sample / self.geometry.step
    }

    /// Track sample the next returned span starts at; the track length once
    /// the stream is exhausted.
    pub fn next_span_start(&self) -> usize {
        (self.acc_start + self.discard * self.geometry.step)
            .saturating_sub(self.pad)
            .min(self.len)
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
        let step = self.geometry.step;
        // Padded-domain start of the requested span.
        let want = span * step + self.pad;
        // A span is covered by the chunk that starts on it AND the one before,
        // so starting anywhere but the very beginning means computing the
        // PREVIOUS span first (it comes out wrong, having no predecessor of its
        // own) and discarding it. Its chunk is what seeds the accumulator.
        if want == 0 {
            self.acc_start = 0;
            self.discard = 0;
        } else {
            self.acc_start = want - step;
            self.discard = 1;
        }
        self.next_chunk = self.acc_start / step;
    }

    /// Folds in every chunk that can still touch the span at `acc_start`. A
    /// chunk starting after `acc_start` begins at or after the span's end, so
    /// the span is final once all chunks with `start <= acc_start` are in.
    fn fold_covering_chunks<S: ChunkSeparator + ?Sized>(
        &mut self,
        separator: &mut S,
        track: &StereoBuf,
    ) -> Result<()> {
        while self.next_chunk < self.chunk_count()
            && self.next_chunk * self.geometry.step <= self.acc_start
        {
            self.run_chunk(self.next_chunk, separator, track)?;
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
    /// step grid.
    ///
    /// The separator must be compiled for this cursor's geometry and return
    /// this cursor's number of targets, and the track must be the one the
    /// cursor was made over; any mismatch is an error rather than a silent
    /// misalignment.
    pub fn next_span<S: ChunkSeparator + ?Sized>(
        &mut self,
        separator: &mut S,
        track: &StereoBuf,
    ) -> Result<Option<LaneSpan>> {
        if separator.geometry() != self.geometry {
            return Err(DiffusionError::model(format!(
                "stems: cursor is at a {}-sample chunk, model at {}",
                self.geometry.samples,
                separator.geometry().samples
            )));
        }
        if separator.targets() != self.targets {
            return Err(DiffusionError::model(format!(
                "stems: cursor holds {} lanes, model returns {}",
                self.targets,
                separator.targets()
            )));
        }
        if track.frames() != self.len {
            return Err(DiffusionError::model(format!(
                "stems: cursor was made over {} samples, track has {}",
                self.len,
                track.frames()
            )));
        }
        loop {
            if self.finished() {
                return Ok(None);
            }
            self.fold_covering_chunks(separator, track)?;
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

    fn run_chunk<S: ChunkSeparator + ?Sized>(
        &mut self,
        chunk: usize,
        separator: &mut S,
        track: &StereoBuf,
    ) -> Result<()> {
        let samples = self.geometry.samples;
        let step = self.geometry.step;
        let start = chunk * step;
        let available = self.padded_len.saturating_sub(start);
        let chunk_len = available.min(samples);
        for ch in 0..AUDIO_CHANNELS {
            let src = track.channel(ch);
            let dst = self.chunk.channel_mut(ch);
            for i in 0..chunk_len {
                let padded = (start + i) as isize - self.pad as isize;
                dst[i] = src[reflect_index(padded, self.len)];
            }
            // The reference reflect-pads the tail only when more than half the
            // window is real audio; otherwise it zero-pads.
            if chunk_len < samples {
                if chunk_len > samples / 2 {
                    for i in chunk_len..samples {
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

        let lanes = separator.separate(&self.chunk)?;
        // A separator that breaks its own contract must not index past a
        // buffer or leave an accumulator unfed.
        if lanes.len() != self.targets {
            return Err(DiffusionError::model(format!(
                "stems: model returned {} lanes for a chunk, the cursor holds {}",
                lanes.len(),
                self.targets
            )));
        }
        for (index, lane) in lanes.iter().enumerate() {
            if lane.left.len() != samples || lane.right.len() != samples {
                return Err(DiffusionError::model(format!(
                    "stems: lane {index} came back {}/{} samples long, the chunk is {samples}",
                    lane.left.len(),
                    lane.right.len()
                )));
            }
        }

        let is_first = chunk == 0;
        let is_last = start + step >= self.padded_len;
        let offset = start - self.acc_start;
        for i in 0..samples {
            let at = offset + i;
            if at >= samples {
                break;
            }
            if i >= chunk_len {
                break;
            }
            let w = window_at(&self.window, self.geometry, i, is_first, is_last);
            self.counter[at] += w;
            for (index, lane) in lanes.iter().enumerate() {
                for ch in 0..AUDIO_CHANNELS {
                    self.acc[index * AUDIO_CHANNELS + ch][at] += lane.channel(ch)[i] * w;
                }
            }
        }
        Ok(())
    }

    fn emit_span(&mut self) -> LaneSpan {
        let samples = self.geometry.samples;
        let step = self.geometry.step;
        let padded_end = (self.acc_start + step).min(self.padded_len);
        let span_len = padded_end - self.acc_start;

        // Padded-domain [acc_start, padded_end) -> track domain, dropping the
        // reflect padding at both ends.
        let track_start = self.acc_start.saturating_sub(self.pad);
        let lead = self.pad.saturating_sub(self.acc_start);
        let track_end = (padded_end.saturating_sub(self.pad)).min(self.len);
        let out_len = track_end.saturating_sub(track_start);

        let mut lanes: Vec<StereoBuf> = (0..self.targets)
            .map(|_| StereoBuf::silence(out_len))
            .collect();
        for (index, lane) in lanes.iter_mut().enumerate() {
            for ch in 0..AUDIO_CHANNELS {
                let acc = &self.acc[index * AUDIO_CHANNELS + ch];
                let out = lane.channel_mut(ch);
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
            let keep = samples - span_len;
            buf[keep..].fill(0.0);
        }
        self.counter.copy_within(span_len.., 0);
        let keep = samples - span_len;
        self.counter[keep..].fill(0.0);
        self.acc_start = padded_end;

        LaneSpan {
            start: track_start,
            lanes,
        }
    }
}

fn window_at(
    window: &[f32],
    geometry: ChunkGeometry,
    i: usize,
    is_first: bool,
    is_last: bool,
) -> f32 {
    if is_first && i < geometry.fade {
        return 1.0;
    }
    if is_last && i >= geometry.samples - geometry.fade {
        return 1.0;
    }
    window[i]
}

/// The stream's position and pending overlap-add over one track, holding no
/// model. Advance it with [`next_span`](Self::next_span) against a model of
/// the same geometry; the track is handed in each time as well, so the
/// cursor owns neither and a worker can keep several against one model.
///
/// It is a [`LaneCursor`] at four lanes: the position, the seek and every
/// sample come from there, and only the span's shape is its own.
pub struct DemixCursor {
    lanes: LaneCursor,
}

impl DemixCursor {
    pub fn new(geometry: ChunkGeometry, track: &StereoBuf) -> Result<Self> {
        Ok(Self {
            lanes: LaneCursor::new(geometry, NUM_STEMS, track)?,
        })
    }

    pub fn geometry(&self) -> ChunkGeometry {
        self.lanes.geometry()
    }

    /// Track length in samples, as handed to `new`.
    pub fn track_len(&self) -> usize {
        self.lanes.track_len()
    }

    /// Model forwards this track needs end to end.
    pub fn chunk_count(&self) -> usize {
        self.lanes.chunk_count()
    }

    /// Track spans this stream will hand back — `ceil(len / step)`. At the
    /// full geometry this is the same indexing the on-disk cache uses.
    pub fn span_count(&self) -> usize {
        self.lanes.span_count()
    }

    /// Track-span index containing the given TRACK sample.
    pub fn span_of_track_sample(&self, sample: usize) -> usize {
        self.lanes.span_of_track_sample(sample)
    }

    /// Track sample the next returned span starts at; the track length once
    /// the stream is exhausted.
    pub fn next_span_start(&self) -> usize {
        self.lanes.next_span_start()
    }

    /// Restart the stream so the next returned span is the one containing
    /// `sample`. Costs two model forwards (the span overlaps two chunks);
    /// spans already computed are the caller's to cache — this only moves the
    /// cursor.
    pub fn seek(&mut self, sample: usize) {
        self.lanes.seek(sample);
    }

    pub fn finished(&self) -> bool {
        self.lanes.finished()
    }

    /// Runs however many chunks are needed and returns the next finished span,
    /// or `None` once the track is exhausted.
    ///
    /// Spans that fall entirely inside the reference's leading reflect pad
    /// carry no track audio; they are consumed silently rather than handed
    /// back, so every returned span has `frames() > 0` and a `start` on the
    /// step grid.
    ///
    /// The model must be compiled for this cursor's geometry and the track
    /// must be the one the cursor was made over; either mismatch is an error
    /// rather than a silent misalignment.
    pub fn next_span(
        &mut self,
        model: &mut StemsModel,
        track: &StereoBuf,
    ) -> Result<Option<StemSpan>> {
        match self.lanes.next_span(model, track)? {
            Some(span) => Ok(Some(StemSpan::from_lanes(span)?)),
            None => Ok(None),
        }
    }
}

/// Streaming separator over one track: a [`DemixCursor`] with the model and
/// the track it runs over.
pub struct Demixer<'a> {
    cursor: DemixCursor,
    model: &'a mut StemsModel,
    track: &'a StereoBuf,
}

impl<'a> Demixer<'a> {
    /// A stream at the model's own geometry.
    pub fn new(model: &'a mut StemsModel, track: &'a StereoBuf) -> Result<Self> {
        let cursor = DemixCursor::new(model.geometry(), track)?;
        Ok(Self { cursor, model, track })
    }

    pub fn geometry(&self) -> ChunkGeometry {
        self.cursor.geometry()
    }

    /// Model forwards this track needs end to end.
    pub fn chunk_count(&self) -> usize {
        self.cursor.chunk_count()
    }

    /// Track spans this stream will hand back — `ceil(len / step)`, at the
    /// full geometry the same indexing the on-disk cache uses.
    pub fn span_count(&self) -> usize {
        self.cursor.span_count()
    }

    /// Track-span index containing the given TRACK sample.
    pub fn span_of_track_sample(&self, sample: usize) -> usize {
        self.cursor.span_of_track_sample(sample)
    }

    /// Track sample the next returned span starts at.
    pub fn next_span_start(&self) -> usize {
        self.cursor.next_span_start()
    }

    /// Restart the stream so the next returned span is the one containing
    /// `sample`. Costs two model forwards; see [`DemixCursor::seek`].
    pub fn seek(&mut self, sample: usize) {
        self.cursor.seek(sample);
    }

    pub fn finished(&self) -> bool {
        self.cursor.finished()
    }

    /// Runs however many chunks are needed and returns the next finished span,
    /// or `None` once the track is exhausted. See [`DemixCursor::next_span`].
    pub fn next_span(&mut self) -> Result<Option<StemSpan>> {
        self.cursor.next_span(self.model, self.track)
    }
}

/// Streaming separator over one track for any [`ChunkSeparator`]: a
/// [`LaneCursor`] with the separator and the track it runs over. The same
/// stream as [`Demixer`], with spans of however many lanes the separator
/// returns.
pub struct LaneDemixer<'a, S: ChunkSeparator + ?Sized> {
    cursor: LaneCursor,
    separator: &'a mut S,
    track: &'a StereoBuf,
}

impl<'a, S: ChunkSeparator + ?Sized> LaneDemixer<'a, S> {
    /// A stream at the separator's own geometry and target count.
    pub fn new(separator: &'a mut S, track: &'a StereoBuf) -> Result<Self> {
        let cursor = LaneCursor::new(separator.geometry(), separator.targets(), track)?;
        Ok(Self { cursor, separator, track })
    }

    pub fn geometry(&self) -> ChunkGeometry {
        self.cursor.geometry()
    }

    /// Lanes in every span this stream hands back.
    pub fn targets(&self) -> usize {
        self.cursor.targets()
    }

    /// Model forwards this track needs end to end.
    pub fn chunk_count(&self) -> usize {
        self.cursor.chunk_count()
    }

    /// Track spans this stream will hand back — `ceil(len / step)`, the
    /// indexing of a span cache on this geometry's step.
    pub fn span_count(&self) -> usize {
        self.cursor.span_count()
    }

    /// Track-span index containing the given TRACK sample.
    pub fn span_of_track_sample(&self, sample: usize) -> usize {
        self.cursor.span_of_track_sample(sample)
    }

    /// Track sample the next returned span starts at.
    pub fn next_span_start(&self) -> usize {
        self.cursor.next_span_start()
    }

    /// Restart the stream so the next returned span is the one containing
    /// `sample`. Costs two model forwards; see [`LaneCursor::seek`].
    pub fn seek(&mut self, sample: usize) {
        self.cursor.seek(sample);
    }

    pub fn finished(&self) -> bool {
        self.cursor.finished()
    }

    /// Runs however many chunks are needed and returns the next finished span,
    /// or `None` once the track is exhausted. See [`LaneCursor::next_span`].
    pub fn next_span(&mut self) -> Result<Option<LaneSpan>> {
        self.cursor.next_span(self.separator, self.track)
    }
}

/// Runs the stream to completion — bit-for-bit the reference's batch result.
pub fn demix_all(
    model: &mut StemsModel,
    track: &StereoBuf,
    progress: impl FnMut(usize, usize),
) -> Result<StemSet> {
    let lanes = demix_all_lanes(model, track, progress)?;
    lanes.try_into().map_err(|lanes: Vec<StereoBuf>| {
        DiffusionError::model(format!(
            "stems: a four-stem demix came back with {} lanes",
            lanes.len()
        ))
    })
}

/// Runs any separator's stream to completion: one whole-track buffer per
/// target, in the separator's order. [`demix_all`] is this at four stems.
pub fn demix_all_lanes<S: ChunkSeparator + ?Sized>(
    separator: &mut S,
    track: &StereoBuf,
    mut progress: impl FnMut(usize, usize),
) -> Result<Vec<StereoBuf>> {
    let len = track.frames();
    let mut demixer = LaneDemixer::new(separator, track)?;
    let mut out: Vec<StereoBuf> = (0..demixer.targets())
        .map(|_| StereoBuf::silence(len))
        .collect();
    let total = demixer.chunk_count();
    let mut done = 0usize;
    while let Some(span) = demixer.next_span()? {
        for (lane, part) in out.iter_mut().zip(&span.lanes) {
            for ch in 0..AUDIO_CHANNELS {
                let src = part.channel(ch);
                let dst = lane.channel_mut(ch);
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

    const GEOMETRIES: [ChunkGeometry; 2] = [ChunkGeometry::FULL, ChunkGeometry::BRIDGE];

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
        for geometry in GEOMETRIES {
            assert_eq!(geometry.track_padding(2 * geometry.border), 0);
            assert_eq!(geometry.track_padding(2 * geometry.border + 1), geometry.border);
        }
        assert_eq!(ChunkGeometry::FULL.track_padding(2 * BORDER + 1), BORDER);
    }

    #[test]
    fn window_has_ramps_and_a_flat_middle() {
        for geometry in GEOMETRIES {
            let w = base_window(geometry);
            let (samples, fade) = (geometry.samples, geometry.fade);
            assert_eq!(w.len(), samples);
            assert!(w[0].abs() < 1e-6);
            assert!((w[fade - 1] - 1.0).abs() < 1e-6);
            assert!((w[samples / 2] - 1.0).abs() < 1e-6);
            assert!(w[samples - 1].abs() < 1e-6);
            // Monotone up then down.
            assert!(w[..fade].windows(2).all(|p| p[0] <= p[1]));
            assert!(w[samples - fade..].windows(2).all(|p| p[0] >= p[1]));
        }
    }

    #[test]
    fn window_edges_are_unfaded_on_the_first_and_last_chunk() {
        for geometry in GEOMETRIES {
            let w = base_window(geometry);
            assert_eq!(window_at(&w, geometry, 0, true, false), 1.0);
            assert_eq!(window_at(&w, geometry, 0, false, false), w[0]);
            let last = geometry.samples - 1;
            assert_eq!(window_at(&w, geometry, last, false, true), 1.0);
            assert_eq!(window_at(&w, geometry, last, false, false), w[last]);
        }
    }

    #[test]
    fn overlap_add_of_the_two_covering_windows_is_everywhere_positive() {
        // The divide in `emit_span` must never see a zero counter in the
        // interior, otherwise the reference's `nan_to_num` would be masking a
        // hole rather than an edge.
        for geometry in GEOMETRIES {
            let w = base_window(geometry);
            for i in 0..geometry.step {
                let sum = w[i + geometry.step] + w[i];
                assert!(sum > 0.5, "{:?}: counter dip at {i}: {sum}", geometry.samples);
            }
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
        // The bridge chunk is a quarter the length, so about four times the
        // forwards for the same track.
        let bridge = ChunkGeometry::BRIDGE.chunk_count(len);
        assert!(bridge > 3 * 46 && bridge < 5 * 46, "{bridge}");
    }

    /// The cursor's own bookkeeping, with no model: where it says the next
    /// span starts is where a seek put it, at either geometry.
    #[test]
    fn a_cursor_names_the_span_a_seek_lands_on() {
        for geometry in GEOMETRIES {
            let step = geometry.step;
            let len = 3 * step + 100;
            let track = StereoBuf::silence(len);
            let mut cursor = DemixCursor::new(geometry, &track).unwrap();
            assert_eq!(cursor.geometry(), geometry);
            assert_eq!(cursor.track_len(), len);
            assert_eq!(cursor.span_count(), 4);
            assert_eq!(cursor.next_span_start(), 0, "a fresh cursor starts at the top");
            assert!(!cursor.finished());
            cursor.seek(step + 5);
            assert_eq!(cursor.next_span_start(), step);
            assert_eq!(cursor.span_of_track_sample(step + 5), 1);
            cursor.seek(2 * step);
            assert_eq!(cursor.next_span_start(), 2 * step);
            // Past the end lands on the last span, never beyond it.
            cursor.seek(len + 10_000);
            assert_eq!(cursor.next_span_start(), 3 * step);
            cursor.seek(0);
            assert_eq!(cursor.next_span_start(), 0);
        }
    }

    /// A track short enough to go unpadded seeks on the same grid.
    #[test]
    fn a_short_track_seeks_on_the_step_grid() {
        for geometry in GEOMETRIES {
            let step = geometry.step;
            let track = StereoBuf::silence(2 * geometry.border);
            let mut cursor = DemixCursor::new(geometry, &track).unwrap();
            assert_eq!(cursor.chunk_count(), 2);
            cursor.seek(step);
            assert_eq!(cursor.next_span_start(), step);
        }
    }

    #[test]
    fn channels_of_different_lengths_are_refused() {
        let track = StereoBuf { left: vec![0.0; 10], right: vec![0.0; 9] };
        assert!(DemixCursor::new(ChunkGeometry::FULL, &track).is_err());
        assert!(DemixCursor::new(ChunkGeometry::BRIDGE, &track).is_err());
    }

    // ---- any number of targets ---------------------------------------------

    /// The two geometries the four-stem model runs at, and the chunk the
    /// single-target vocal model is trained at.
    fn lane_geometries() -> [ChunkGeometry; 3] {
        [
            ChunkGeometry::FULL,
            ChunkGeometry::BRIDGE,
            ChunkGeometry::new(352_800).unwrap(),
        ]
    }

    /// A separator with no device behind it: every lane is a pure function of
    /// the chunk. `first_lane` picks which of those functions target 0 is, so
    /// a one-target separator can stand for any single lane of a wider one.
    struct FakeSeparator {
        geometry: ChunkGeometry,
        targets: usize,
        first_lane: usize,
        forwards: usize,
    }

    impl FakeSeparator {
        fn new(geometry: ChunkGeometry, targets: usize, first_lane: usize) -> Self {
            Self { geometry, targets, first_lane, forwards: 0 }
        }
    }

    impl ChunkSeparator for FakeSeparator {
        fn geometry(&self) -> ChunkGeometry {
            self.geometry
        }

        fn targets(&self) -> usize {
            self.targets
        }

        fn separate(&mut self, chunk: &StereoBuf) -> Result<Vec<StereoBuf>> {
            self.forwards += 1;
            Ok((0..self.targets)
                .map(|target| fake_lane(chunk, self.first_lane + target))
                .collect())
        }
    }

    /// Lane `lane` of the fake model. It leans on the position INSIDE the
    /// chunk, so the two chunks covering a sample disagree about it and the
    /// cross-fade between them is part of what a comparison sees.
    fn fake_lane(chunk: &StereoBuf, lane: usize) -> StereoBuf {
        let n = chunk.frames();
        let gain = 0.2 + 0.15 * lane as f32;
        let mut out = StereoBuf::silence(n);
        for i in 0..n {
            let tilt = i as f32 / n as f32;
            out.left[i] = gain * chunk.left[i] + 0.1 * tilt * chunk.right[i];
            out.right[i] = gain * chunk.right[i] - 0.1 * tilt * chunk.left[i] + 0.01 * lane as f32;
        }
        out
    }

    /// Deterministic full-band noise; the two channels differ.
    fn noise_track(len: usize) -> StereoBuf {
        let mut state = 0x2545_f491u32;
        let mut next = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        };
        let mut track = StereoBuf::silence(len);
        for i in 0..len {
            track.left[i] = next();
            track.right[i] = next();
        }
        track
    }

    /// A padded track with a ragged tail, and one short enough to go
    /// unpadded whose second chunk is mostly zero fill.
    fn lane_track_lengths(geometry: ChunkGeometry) -> [usize; 2] {
        [3 * geometry.step + 1234, geometry.step + geometry.step / 3]
    }

    fn same_bits(a: &StereoBuf, b: &StereoBuf) -> bool {
        let bits = |v: &[f32]| v.iter().map(|s| s.to_bits()).collect::<Vec<u32>>();
        bits(&a.left) == bits(&b.left) && bits(&a.right) == bits(&b.right)
    }

    /// The reference's batch loop, written the way the reference writes it:
    /// pad the whole track, walk every window, accumulate, divide once at the
    /// end. It shares the window and the mirror rule with the stream and
    /// nothing else — no sliding accumulator, no spans, no cursor.
    fn batch_overlap_add(separator: &mut FakeSeparator, track: &StereoBuf) -> Vec<StereoBuf> {
        let geometry = separator.geometry;
        let (samples, step) = (geometry.samples, geometry.step);
        let len = track.frames();
        let pad = geometry.track_padding(len);
        let padded_len = len + 2 * pad;
        let window = base_window(geometry);
        let mut sums = vec![vec![0.0f32; padded_len]; separator.targets * AUDIO_CHANNELS];
        let mut counter = vec![0.0f32; padded_len];
        let mut start = 0usize;
        while start < padded_len {
            let chunk_len = (padded_len - start).min(samples);
            let mut chunk = StereoBuf::silence(samples);
            for ch in 0..AUDIO_CHANNELS {
                let dst = chunk.channel_mut(ch);
                for i in 0..chunk_len {
                    let at = (start + i) as isize - pad as isize;
                    dst[i] = track.channel(ch)[reflect_index(at, len)];
                }
                if chunk_len < samples && chunk_len > samples / 2 {
                    for i in chunk_len..samples {
                        let mirror = chunk_len as isize - 2 - (i - chunk_len) as isize;
                        dst[i] = dst[mirror.max(0) as usize];
                    }
                }
            }
            let lanes = separator.separate(&chunk).unwrap();
            let (is_first, is_last) = (start == 0, start + step >= padded_len);
            for i in 0..chunk_len {
                let w = window_at(&window, geometry, i, is_first, is_last);
                counter[start + i] += w;
                for (index, lane) in lanes.iter().enumerate() {
                    for ch in 0..AUDIO_CHANNELS {
                        sums[index * AUDIO_CHANNELS + ch][start + i] += lane.channel(ch)[i] * w;
                    }
                }
            }
            start += step;
        }
        (0..separator.targets)
            .map(|index| {
                let mut lane = StereoBuf::silence(len);
                for ch in 0..AUDIO_CHANNELS {
                    let sum = &sums[index * AUDIO_CHANNELS + ch];
                    for (i, out) in lane.channel_mut(ch).iter_mut().enumerate() {
                        let c = counter[pad + i];
                        *out = if c > 0.0 { sum[pad + i] / c } else { 0.0 };
                    }
                }
                lane
            })
            .collect()
    }

    /// One span, then a seek, then the rest of the track: every span the
    /// stream handed back, in the order it did.
    fn spans_across_a_seek(
        separator: &mut FakeSeparator,
        track: &StereoBuf,
        seek_to: usize,
    ) -> Vec<LaneSpan> {
        let mut demixer = LaneDemixer::new(separator, track).unwrap();
        let mut spans = vec![demixer.next_span().unwrap().expect("a first span")];
        demixer.seek(seek_to);
        while let Some(span) = demixer.next_span().unwrap() {
            spans.push(span);
        }
        spans
    }

    /// What the refactor rests on: the one cursor, held at four lanes or at
    /// one, is still the reference's batch arithmetic to the last bit, at
    /// every geometry, padded or not.
    #[test]
    fn the_stream_is_the_batch_overlap_add_bit_for_bit_at_any_lane_count() {
        for geometry in lane_geometries() {
            for len in lane_track_lengths(geometry) {
                let track = noise_track(len);
                for targets in [NUM_STEMS, 1] {
                    let mut streamed = FakeSeparator::new(geometry, targets, 0);
                    let mut batched = FakeSeparator::new(geometry, targets, 0);
                    let mut calls = 0usize;
                    let stream = demix_all_lanes(&mut streamed, &track, |_, _| calls += 1).unwrap();
                    let batch = batch_overlap_add(&mut batched, &track);
                    assert_eq!(stream.len(), targets);
                    assert_eq!(calls, track.frames().div_ceil(geometry.step));
                    assert_eq!(streamed.forwards, geometry.chunk_count(len));
                    assert_eq!(streamed.forwards, batched.forwards);
                    for (lane, (got, want)) in stream.iter().zip(&batch).enumerate() {
                        assert!(
                            same_bits(got, want),
                            "chunk {} len {len} targets {targets} lane {lane}",
                            geometry.samples
                        );
                    }
                }
            }
        }
    }

    /// A lane does not know how many others ran beside it. A one-target
    /// separator returning what a four-target one returns as its vocals
    /// streams the same bits as that lane — straight through, and again
    /// after a seek has thrown the accumulator away and rebuilt it.
    #[test]
    fn one_target_streams_the_same_bits_as_its_lane_among_four() {
        let vocals = Stem::Vocals.index();
        for geometry in lane_geometries() {
            let step = geometry.step;
            for len in lane_track_lengths(geometry) {
                let track = noise_track(len);
                let mut four = FakeSeparator::new(geometry, NUM_STEMS, 0);
                let mut one = FakeSeparator::new(geometry, 1, vocals);

                let all_four = demix_all_lanes(&mut four, &track, |_, _| {}).unwrap();
                let all_one = demix_all_lanes(&mut one, &track, |_, _| {}).unwrap();
                assert_eq!((all_four.len(), all_one.len()), (NUM_STEMS, 1));
                assert!(same_bits(&all_one[0], &all_four[vocals]), "chunk {}", geometry.samples);
                assert!(
                    !same_bits(&all_one[0], &all_four[0]),
                    "the fake's lanes must differ for this to mean anything"
                );

                // Seek to the last span, a few samples in.
                let last = (len - 1) / step;
                let seek_to = last * step + 5;
                let four_spans = spans_across_a_seek(&mut four, &track, seek_to);
                let one_spans = spans_across_a_seek(&mut one, &track, seek_to);
                assert_eq!(four_spans.len(), one_spans.len());
                assert_eq!(
                    one_spans.iter().map(|s| s.start).collect::<Vec<_>>(),
                    vec![0, last * step],
                    "chunk {} len {len}",
                    geometry.samples
                );
                for (a, b) in one_spans.iter().zip(&four_spans) {
                    assert_eq!(a.start, b.start);
                    assert_eq!((a.lanes.len(), b.lanes.len()), (1, NUM_STEMS));
                    assert!(same_bits(&a.lanes[0], &b.lanes[vocals]), "span at {}", a.start);
                    // And a span reached by a seek is the span the straight
                    // run produced there.
                    let end = a.start + a.frames();
                    let straight = StereoBuf {
                        left: all_one[0].left[a.start..end].to_vec(),
                        right: all_one[0].right[a.start..end].to_vec(),
                    };
                    assert!(same_bits(&a.lanes[0], &straight), "span at {}", a.start);
                }
            }
        }
    }

    /// The lane cursor keeps the four-stem cursor's books: same grid, same
    /// seek, at the vocal model's chunk as at the others.
    #[test]
    fn a_lane_cursor_seeks_on_the_same_grid_as_the_four_stem_cursor() {
        for geometry in lane_geometries() {
            let step = geometry.step;
            let len = 3 * step + 100;
            let track = StereoBuf::silence(len);
            let mut lanes = LaneCursor::new(geometry, 1, &track).unwrap();
            let mut stems = DemixCursor::new(geometry, &track).unwrap();
            assert_eq!(lanes.targets(), 1);
            assert_eq!(lanes.geometry(), stems.geometry());
            assert_eq!(lanes.track_len(), stems.track_len());
            assert_eq!(lanes.chunk_count(), stems.chunk_count());
            assert_eq!(lanes.span_count(), stems.span_count());
            for sample in [0, step + 5, 2 * step, len + 10_000, 0] {
                lanes.seek(sample);
                stems.seek(sample);
                assert_eq!(lanes.next_span_start(), stems.next_span_start());
                assert_eq!(lanes.finished(), stems.finished());
            }
        }
        let vocal_chunk = ChunkGeometry::new(352_800).unwrap();
        assert_eq!(vocal_chunk.step, 176_400);
        assert_eq!(vocal_chunk.border, vocal_chunk.step);
    }

    /// A separator the cursor was not made for is an error before or at the
    /// first forward, never a span of misaligned or missing audio.
    #[test]
    fn a_separator_that_does_not_fit_the_cursor_is_refused() {
        struct ShortChanged(FakeSeparator);
        impl ChunkSeparator for ShortChanged {
            fn geometry(&self) -> ChunkGeometry {
                self.0.geometry
            }
            fn targets(&self) -> usize {
                self.0.targets
            }
            fn separate(&mut self, chunk: &StereoBuf) -> Result<Vec<StereoBuf>> {
                let mut lanes = self.0.separate(chunk)?;
                lanes.pop();
                Ok(lanes)
            }
        }

        let geometry = ChunkGeometry::BRIDGE;
        let track = noise_track(2 * geometry.step);
        assert!(LaneCursor::new(geometry, 0, &track).is_err(), "no lanes is no stream");

        let mut cursor = LaneCursor::new(geometry, 2, &track).unwrap();
        let mut other_chunk = FakeSeparator::new(ChunkGeometry::FULL, 2, 0);
        assert!(cursor.next_span(&mut other_chunk, &track).is_err());
        let mut other_count = FakeSeparator::new(geometry, 1, 0);
        assert!(cursor.next_span(&mut other_count, &track).is_err());
        let mut short_changed = ShortChanged(FakeSeparator::new(geometry, 2, 0));
        assert!(cursor.next_span(&mut short_changed, &track).is_err());
        let other_track = noise_track(2 * geometry.step + 1);
        let mut fits = FakeSeparator::new(geometry, 2, 0);
        assert!(cursor.next_span(&mut fits, &other_track).is_err());

        // The one that fits still streams, through a trait object as well.
        cursor.seek(0);
        let boxed: &mut dyn ChunkSeparator = &mut fits;
        let span = cursor.next_span(boxed, &track).unwrap().expect("a span");
        assert_eq!((span.start, span.frames(), span.lanes.len()), (0, geometry.step, 2));
    }
}
