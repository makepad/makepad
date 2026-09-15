//! Windowed inference and Basic Pitch note creation.

use crate::config::*;
use crate::cqt::Cqt;
use crate::graph::NotesGraph;
use crate::weights::{NotesWeights, WeightCensus};
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct NoteEvent {
    pub start_secs: f64,
    pub end_secs: f64,
    pub midi: u8,
    pub amplitude: f32,
    /// One estimate per active model frame, in semitones relative to `midi`.
    pub bends: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoteTranscription {
    pub notes: Vec<NoteEvent>,
    pub frame_rate: f64,
    /// Row-major onset posteriorgram, `[time][88]`.
    pub onsets: Vec<f32>,
}

pub struct NotesModel {
    cqt: Cqt,
    graph: NotesGraph,
    census: WeightCensus,
}

impl NotesModel {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let weights = NotesWeights::load(path)?;
        Ok(Self {
            cqt: Cqt::new(weights.cqt.clone()),
            graph: NotesGraph::new(&weights),
            census: weights.census,
        })
    }

    pub fn census(&self) -> &WeightCensus {
        &self.census
    }

    pub fn transcribe(&mut self, mono_22k: &[f32]) -> Result<NoteTranscription, String> {
        self.transcribe_with_progress(mono_22k, |_, _| true)
    }

    /// As [`Self::transcribe`], with a callback after each completed window.
    /// Returning false cancels before the next window begins.
    pub fn transcribe_with_progress<F>(
        &mut self,
        mono_22k: &[f32],
        mut progress: F,
    ) -> Result<NoteTranscription, String>
    where
        F: FnMut(usize, usize) -> bool,
    {
        if mono_22k.is_empty() {
            return Ok(empty_transcription());
        }
        let total_windows = (mono_22k.len() + OVERLAP_SAMPLES / 2).div_ceil(WINDOW_HOP_SAMPLES);
        // Avoid the non-zero folded biases turning exact digital silence into
        // low posterior noise, and avoid doing hundreds of millions of MACs.
        if mono_22k.iter().all(|sample| sample.abs() <= 1.0e-8) {
            progress(total_windows, total_windows);
            return Ok(empty_transcription());
        }

        let mut padded = vec![0.0f32; OVERLAP_SAMPLES / 2];
        padded.extend_from_slice(mono_22k);
        let mut contour_windows = Vec::with_capacity(total_windows);
        let mut note_windows = Vec::with_capacity(total_windows);
        let mut onset_windows = Vec::with_capacity(total_windows);
        for window_index in 0..total_windows {
            if window_index > 0 && !progress(window_index, total_windows) {
                return Err("Basic Pitch transcription cancelled".to_string());
            }
            let start = window_index * WINDOW_HOP_SAMPLES;
            let mut window = vec![0.0f32; AUDIO_N_SAMPLES];
            if start < padded.len() {
                let count = AUDIO_N_SAMPLES.min(padded.len() - start);
                window[..count].copy_from_slice(&padded[start..start + count]);
            }
            let features = self.cqt.transform(&window)?;
            if features.frames != WINDOW_FRAMES {
                return Err(format!(
                    "CQT returned {} frames for one checkpoint window, expected {WINDOW_FRAMES}",
                    features.frames
                ));
            }
            let output = self.graph.forward(&features)?;
            contour_windows.push(output.contours);
            note_windows.push(output.notes);
            onset_windows.push(output.onsets);
        }
        progress(total_windows, total_windows);

        let expected_frames = ((mono_22k.len() as f64 / WINDOW_HOP_SAMPLES as f64)
            * OUTPUT_FRAMES_PER_WINDOW as f64) as usize;
        let contours = unwrap_windows(&contour_windows, CONTOUR_BINS, expected_frames)?;
        let frames = unwrap_windows(&note_windows, NOTES, expected_frames)?;
        let onsets = unwrap_windows(&onset_windows, NOTES, expected_frames)?;
        let mut notes = create_notes(&frames, &onsets, &contours, true)?;
        align_leading_edge_onsets(&mut notes, mono_22k);
        Ok(NoteTranscription {
            notes,
            frame_rate: FRAME_RATE,
            onsets,
        })
    }
}

/// Relative-max onset decoding cannot select frame zero. For a clip whose
/// first audible event is within the CQT's leading context, Melodia recovers
/// the pitch but can place that first event up to one minimum-note span late.
/// Snap only that boundary chord back to the measured waveform onset; all
/// interior events remain the reference postprocessor's exact frame times.
fn align_leading_edge_onsets(notes: &mut [NoteEvent], audio: &[f32]) {
    let peak = audio.iter().copied().map(f32::abs).fold(0.0f32, f32::max);
    if peak <= 1.0e-8 || notes.is_empty() {
        return;
    }
    let threshold = peak * 0.01;
    let Some(first_sample) = audio.iter().position(|sample| sample.abs() >= threshold) else {
        return;
    };
    let waveform_start = first_sample as f64 / SAMPLE_RATE as f64;
    let decoded_start = notes
        .iter()
        .map(|note| note.start_secs)
        .min_by(f64::total_cmp)
        .unwrap_or(waveform_start);
    let lag = decoded_start - waveform_start;
    if !(0.0..=(MIN_NOTE_LEN as f64 + 1.0) / FRAME_RATE).contains(&lag) {
        return;
    }
    for note in notes
        .iter_mut()
        .filter(|note| note.start_secs <= decoded_start + 2.0 / FRAME_RATE)
    {
        let added = ((note.start_secs - waveform_start).max(0.0) * FRAME_RATE).round() as usize;
        if let Some(&first_bend) = note.bends.first() {
            let mut bends = vec![first_bend; added];
            bends.append(&mut note.bends);
            note.bends = bends;
        }
        note.start_secs = waveform_start;
    }
}

fn empty_transcription() -> NoteTranscription {
    NoteTranscription {
        notes: Vec::new(),
        frame_rate: FRAME_RATE,
        onsets: Vec::new(),
    }
}

fn unwrap_windows(
    windows: &[Vec<f32>],
    width: usize,
    expected_frames: usize,
) -> Result<Vec<f32>, String> {
    let trim = OVERLAP_FRAMES / 2;
    let mut output = Vec::with_capacity(windows.len() * OUTPUT_FRAMES_PER_WINDOW * width);
    for window in windows {
        if window.len() != WINDOW_FRAMES * width {
            return Err(format!(
                "Basic Pitch head returned {} values, expected {}",
                window.len(),
                WINDOW_FRAMES * width
            ));
        }
        output.extend_from_slice(&window[trim * width..(WINDOW_FRAMES - trim) * width]);
    }
    output.truncate(expected_frames.min(output.len() / width) * width);
    Ok(output)
}

#[derive(Clone, Debug)]
struct FrameNote {
    start: usize,
    end: usize,
    pitch: usize,
    amplitude: f32,
}

/// Decode already-unwrapped head outputs. Public primarily for model-oracle
/// tests and applications that retain posteriorgrams outside `NotesModel`.
pub fn create_notes(
    frames: &[f32],
    onsets: &[f32],
    contours: &[f32],
    melodia_trick: bool,
) -> Result<Vec<NoteEvent>, String> {
    if frames.len() != onsets.len() || frames.len() % NOTES != 0 {
        return Err("note and onset posteriorgrams must both have shape [T,88]".to_string());
    }
    let n_frames = frames.len() / NOTES;
    if contours.len() != n_frames * CONTOUR_BINS {
        return Err("contour posteriorgram must have shape [T,264]".to_string());
    }
    if n_frames == 0 {
        return Ok(Vec::new());
    }
    let inferred_onsets = infer_onsets(onsets, frames, n_frames);
    let mut peaks = Vec::new();
    for time in 1..n_frames.saturating_sub(1) {
        for pitch in 0..NOTES {
            let value = inferred_onsets[time * NOTES + pitch];
            if value >= ONSET_THRESHOLD
                && value > inferred_onsets[(time - 1) * NOTES + pitch]
                && value > inferred_onsets[(time + 1) * NOTES + pitch]
            {
                peaks.push((time, pitch));
            }
        }
    }
    peaks.reverse();
    let mut remaining = frames.to_vec();
    let mut decoded = Vec::new();
    for (start, pitch) in peaks {
        if start >= n_frames - 1 {
            continue;
        }
        let mut end = start + 1;
        let mut below = 0usize;
        while end < n_frames - 1 && below < ENERGY_TOLERANCE {
            if remaining[end * NOTES + pitch] < FRAME_THRESHOLD {
                below += 1;
            } else {
                below = 0;
            }
            end += 1;
        }
        end -= below;
        if end - start <= MIN_NOTE_LEN {
            continue;
        }
        clear_energy(&mut remaining, n_frames, start, end, pitch);
        decoded.push(FrameNote {
            start,
            end,
            pitch,
            amplitude: mean_pitch(frames, start, end, pitch),
        });
    }

    if melodia_trick {
        loop {
            let Some((middle_index, &maximum)) = remaining
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
            else {
                break;
            };
            if maximum <= FRAME_THRESHOLD {
                break;
            }
            let middle = middle_index / NOTES;
            let pitch = middle_index % NOTES;
            remaining[middle_index] = 0.0;

            let mut cursor = middle + 1;
            let mut below = 0usize;
            while cursor < n_frames - 1 && below < ENERGY_TOLERANCE {
                if remaining[cursor * NOTES + pitch] < FRAME_THRESHOLD {
                    below += 1;
                } else {
                    below = 0;
                }
                clear_energy(&mut remaining, n_frames, cursor, cursor + 1, pitch);
                cursor += 1;
            }
            let end = cursor.saturating_sub(1 + below);

            let mut cursor = middle.saturating_sub(1);
            let mut below = 0usize;
            while cursor > 0 && below < ENERGY_TOLERANCE {
                if remaining[cursor * NOTES + pitch] < FRAME_THRESHOLD {
                    below += 1;
                } else {
                    below = 0;
                }
                clear_energy(&mut remaining, n_frames, cursor, cursor + 1, pitch);
                cursor -= 1;
            }
            let start = cursor + 1 + below;
            if end > start && end - start > MIN_NOTE_LEN {
                decoded.push(FrameNote {
                    start,
                    end,
                    pitch,
                    amplitude: mean_pitch(frames, start, end, pitch),
                });
            }
        }
    }

    let mut notes: Vec<_> = decoded
        .into_iter()
        .map(|note| {
            let midi = (note.pitch as i32 + MIDI_OFFSET) as u8;
            let bends = pitch_bends(contours, n_frames, &note);
            NoteEvent {
                start_secs: note.start as f64 / FRAME_RATE,
                end_secs: note.end as f64 / FRAME_RATE,
                midi,
                amplitude: note.amplitude,
                bends,
            }
        })
        .collect();
    notes.sort_by(|a, b| {
        a.start_secs
            .total_cmp(&b.start_secs)
            .then_with(|| a.midi.cmp(&b.midi))
    });
    Ok(notes)
}

fn infer_onsets(onsets: &[f32], frames: &[f32], n_frames: usize) -> Vec<f32> {
    let mut difference = vec![0.0f32; onsets.len()];
    let mut max_difference = 0.0f32;
    for time in 2..n_frames {
        for pitch in 0..NOTES {
            let current = frames[time * NOTES + pitch];
            let d1 = current - frames[(time - 1) * NOTES + pitch];
            let d2 = current - frames[(time - 2) * NOTES + pitch];
            let value = d1.min(d2).max(0.0);
            difference[time * NOTES + pitch] = value;
            max_difference = max_difference.max(value);
        }
    }
    let max_onset = onsets.iter().copied().fold(0.0f32, f32::max);
    let scale = if max_difference > 0.0 {
        max_onset / max_difference
    } else {
        0.0
    };
    onsets
        .iter()
        .zip(difference)
        .map(|(&onset, difference)| onset.max(difference * scale))
        .collect()
}

fn clear_energy(
    remaining: &mut [f32],
    n_frames: usize,
    start: usize,
    end: usize,
    pitch: usize,
) {
    for time in start..end.min(n_frames) {
        remaining[time * NOTES + pitch] = 0.0;
        if pitch > 0 {
            remaining[time * NOTES + pitch - 1] = 0.0;
        }
        if pitch + 1 < NOTES {
            remaining[time * NOTES + pitch + 1] = 0.0;
        }
    }
}

fn mean_pitch(frames: &[f32], start: usize, end: usize, pitch: usize) -> f32 {
    let sum: f32 = (start..end).map(|time| frames[time * NOTES + pitch]).sum();
    sum / (end - start) as f32
}

fn pitch_bends(contours: &[f32], n_frames: usize, note: &FrameNote) -> Vec<f32> {
    let center = note.pitch as isize * CONTOUR_BINS_PER_SEMITONE as isize;
    let first = (center - PITCH_BEND_TOLERANCE_BINS).max(0);
    let last = (center + PITCH_BEND_TOLERANCE_BINS)
        .min(CONTOUR_BINS as isize - 1);
    let mut bends = Vec::with_capacity(note.end - note.start);
    for time in note.start..note.end.min(n_frames) {
        let mut best_bin = center;
        let mut best_value = f32::NEG_INFINITY;
        for bin in first..=last {
            let offset = bin - center;
            let gaussian = (-0.5 * (offset as f32 / 5.0).powi(2)).exp();
            let value = contours[time * CONTOUR_BINS + bin as usize] * gaussian;
            if value > best_value {
                best_value = value;
                best_bin = bin;
            }
        }
        bends.push((best_bin - center) as f32 / CONTOUR_BINS_PER_SEMITONE as f32);
    }
    bends
}

/// Serialize a transcription as a format-0 Standard MIDI File. Polyphonic
/// notes are assigned independent channels where possible so pitch bends do
/// not leak between simultaneous notes; the General MIDI default bend range
/// is ±2 semitones.
pub fn to_midi_bytes(transcription: &NoteTranscription, bpm: Option<f64>) -> Vec<u8> {
    use makepad_midi_file::{
        ChannelEvent, ChannelMessage, Division, EventKind, Format, Header, MetaEvent,
        MidiFile, Track, TrackEvent,
    };
    const TPQ: u16 = 480;
    const CHANNELS: [u8; 15] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15];
    let bpm = bpm.filter(|value| value.is_finite() && *value > 0.0).unwrap_or(120.0);
    let micros = (60_000_000.0 / bpm).round().clamp(1.0, 16_777_215.0) as u32;
    let ticks_per_second = bpm / 60.0 * f64::from(TPQ);
    let mut channel_free_at = [0.0f64; CHANNELS.len()];
    let mut events: Vec<(u64, u8, EventKind)> = vec![(
        0,
        0,
        EventKind::Meta(MetaEvent::SetTempo(micros)),
    )];
    for (index, &channel) in CHANNELS.iter().enumerate() {
        events.push((
            0,
            1,
            EventKind::Channel(ChannelEvent {
                channel,
                message: ChannelMessage::ProgramChange { program: 4 },
            }),
        ));
        channel_free_at[index] = 0.0;
    }
    let mut notes = transcription.notes.clone();
    notes.sort_by(|a, b| a.start_secs.total_cmp(&b.start_secs));
    for note in &notes {
        let channel_index = channel_free_at
            .iter()
            .position(|&end| end <= note.start_secs)
            .unwrap_or_else(|| {
                channel_free_at
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1.total_cmp(b.1))
                    .map(|(index, _)| index)
                    .unwrap_or(0)
            });
        channel_free_at[channel_index] = note.end_secs;
        let channel = CHANNELS[channel_index];
        let start_tick = seconds_to_ticks(note.start_secs, ticks_per_second);
        let end_tick = seconds_to_ticks(note.end_secs, ticks_per_second).max(start_tick + 1);
        if !note.bends.is_empty() {
            let denominator = note.bends.len().saturating_sub(1).max(1) as f64;
            for (index, &bend) in note.bends.iter().enumerate() {
                let fraction = index as f64 / denominator;
                let tick = start_tick + ((end_tick - start_tick) as f64 * fraction).round() as u64;
                let value = (8192.0 + bend.clamp(-2.0, 2.0) as f64 * 4096.0)
                    .round()
                    .clamp(0.0, 16_383.0) as u16;
                events.push((
                    tick,
                    2,
                    EventKind::Channel(ChannelEvent {
                        channel,
                        message: ChannelMessage::PitchBend { value },
                    }),
                ));
            }
        }
        events.push((
            start_tick,
            3,
            EventKind::Channel(ChannelEvent {
                channel,
                message: ChannelMessage::NoteOn {
                    key: note.midi,
                    velocity: (note.amplitude.clamp(0.0, 1.0) * 127.0).round() as u8,
                },
            }),
        ));
        events.push((
            end_tick,
            0,
            EventKind::Channel(ChannelEvent {
                channel,
                message: ChannelMessage::NoteOff {
                    key: note.midi,
                    velocity: 0,
                },
            }),
        ));
        events.push((
            end_tick,
            1,
            EventKind::Channel(ChannelEvent {
                channel,
                message: ChannelMessage::PitchBend { value: 8192 },
            }),
        ));
    }
    events.sort_by_key(|(tick, priority, _)| (*tick, *priority));
    let final_tick = events.last().map(|event| event.0).unwrap_or(0);
    let mut track = Track::default();
    track.events = events
        .into_iter()
        .map(|(tick, _, kind)| TrackEvent { tick, kind })
        .collect();
    track.events.push(TrackEvent {
        tick: final_tick,
        kind: EventKind::Meta(MetaEvent::EndOfTrack),
    });
    MidiFile {
        header: Header {
            format: Format::SingleTrack,
            track_count: 1,
            division: Division::TicksPerQuarter(TPQ),
            extra_data: Vec::new(),
        },
        tracks: vec![track],
        unknown_chunks: Vec::new(),
    }
    .to_bytes()
    .unwrap_or_default()
}

fn seconds_to_ticks(seconds: f64, ticks_per_second: f64) -> u64 {
    (seconds.max(0.0) * ticks_per_second).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_creation_extends_energy_and_reads_bends() {
        let n = 40;
        let pitch = 48usize;
        let mut frames = vec![0.0; n * NOTES];
        let mut onsets = vec![0.0; n * NOTES];
        let mut contours = vec![0.0; n * CONTOUR_BINS];
        onsets[5 * NOTES + pitch] = 0.9;
        for time in 5..30 {
            frames[time * NOTES + pitch] = 0.8;
            let bend = ((time - 5) / 8).min(2);
            contours[time * CONTOUR_BINS + pitch * 3 + bend] = 1.0;
        }
        let notes = create_notes(&frames, &onsets, &contours, true).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].midi, 69);
        assert!((notes[0].start_secs - 5.0 / FRAME_RATE).abs() < 1e-9);
        assert!(notes[0].bends.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn midi_contains_notes_and_pitch_bends() {
        let transcription = NoteTranscription {
            notes: vec![NoteEvent {
                start_secs: 0.0,
                end_secs: 0.5,
                midi: 69,
                amplitude: 0.8,
                bends: vec![0.0, 0.5, 1.0],
            }],
            frame_rate: FRAME_RATE,
            onsets: Vec::new(),
        };
        let bytes = to_midi_bytes(&transcription, None);
        let midi = makepad_midi_file::parse(&bytes).unwrap();
        let events = &midi.tracks[0].events;
        assert!(events.iter().any(|event| matches!(
            event.kind,
            makepad_midi_file::EventKind::Channel(makepad_midi_file::ChannelEvent {
                message: makepad_midi_file::ChannelMessage::NoteOn { key: 69, .. },
                ..
            })
        )));
        assert!(events.iter().any(|event| matches!(
            event.kind,
            makepad_midi_file::EventKind::Channel(makepad_midi_file::ChannelEvent {
                message: makepad_midi_file::ChannelMessage::PitchBend { .. },
                ..
            })
        )));
    }
}
