//! Basic Pitch input resampling and note-to-score mapping for loop scores.

use makepad_ai_notes::NoteEvent;
use makepad_score_view::build::PitchedNote;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PitchLane {
    Bass,
    Melody,
    Other,
}

/// Resample one mono channel to Basic Pitch's fixed 22.05 kHz input rate.
///
/// This is the hub resampler's small rational polyphase/windowed-sinc kernel,
/// kept local so the VJ app does not need another crate feature or dependency.
pub(crate) fn resample_to_basic_pitch(input: &[f32], input_rate: u32) -> Vec<f32> {
    const OUTPUT_RATE: u32 = makepad_ai_notes::SAMPLE_RATE as u32;
    assert!(input_rate > 0);
    if input_rate == OUTPUT_RATE || input.is_empty() {
        return input.to_vec();
    }

    let divisor = gcd(u64::from(input_rate), u64::from(OUTPUT_RATE));
    let up = (u64::from(OUTPUT_RATE) / divisor) as usize;
    let down = (u64::from(input_rate) / divisor) as usize;

    const HALF: i64 = 16;
    let cutoff = 0.5 * 0.92 * (OUTPUT_RATE.min(input_rate) as f64 / input_rate as f64);
    let mut kernels = Vec::with_capacity(up);
    for phase in 0..up {
        let fraction = phase as f64 / up as f64;
        let mut taps = Vec::with_capacity((2 * HALF) as usize);
        let mut sum = 0.0;
        for offset in -HALF + 1..=HALF {
            let time = offset as f64 - fraction;
            let sinc = if time == 0.0 {
                1.0
            } else {
                let angle = std::f64::consts::PI * 2.0 * cutoff * time;
                angle.sin() / angle
            };
            let window_x = (time + HALF as f64) / (2.0 * HALF as f64);
            let window = if (0.0..=1.0).contains(&window_x) {
                0.42 - 0.5 * (2.0 * std::f64::consts::PI * window_x).cos()
                    + 0.08 * (4.0 * std::f64::consts::PI * window_x).cos()
            } else {
                0.0
            };
            let tap = 2.0 * cutoff * sinc * window;
            sum += tap;
            taps.push(tap);
        }
        for tap in &mut taps {
            *tap /= sum;
        }
        kernels.push(taps);
    }

    let output_len = input.len() * up / down;
    let mut output = Vec::with_capacity(output_len);
    for output_index in 0..output_len {
        let numerator = output_index * down;
        let base = (numerator / up) as i64;
        let taps = &kernels[numerator % up];
        let mut sample = 0.0;
        for (tap_index, offset) in (-HALF + 1..=HALF).enumerate() {
            let input_index = base + offset;
            if input_index >= 0 && (input_index as usize) < input.len() {
                sample += input[input_index as usize] as f64 * taps[tap_index];
            }
        }
        output.push(sample as f32);
    }
    output
}

/// Convert Basic Pitch seconds to score beats and reduce its polyphonic
/// output to the staff density appropriate for each loop-score row.
pub(crate) fn map_notes(events: &[NoteEvent], bpm: f64, lane: PitchLane) -> Vec<PitchedNote> {
    if !bpm.is_finite() || bpm <= 0.0 {
        return Vec::new();
    }
    let mut events: Vec<&NoteEvent> = events
        .iter()
        .filter(|event| {
            event.start_secs.is_finite()
                && event.end_secs.is_finite()
                && event.end_secs - event.start_secs >= 0.040
        })
        .collect();

    match lane {
        PitchLane::Bass => {
            // Basic Pitch commonly reports the 2nd and 3rd harmonic beside
            // the fundamental. Keep a weak fundamental from suppressing a
            // real upper note: the lower note must carry at least 60% of the
            // candidate's amplitude before it wins.
            let bass_candidates = events.clone();
            events.retain(|candidate| {
                !bass_candidates.iter().any(|lower| {
                    let interval = i16::from(candidate.midi) - i16::from(lower.midi);
                    matches!(interval, 12 | 19)
                        && overlaps(candidate, lower)
                        && finite_amplitude(lower) >= finite_amplitude(candidate) * 0.60
                })
            });
        }
        PitchLane::Melody => events = loudest_non_overlapping(events, 1),
        PitchLane::Other => events = loudest_non_overlapping(events, 4),
    }

    events.sort_by(|left, right| {
        left.start_secs
            .total_cmp(&right.start_secs)
            .then_with(|| left.midi.cmp(&right.midi))
    });
    let beats_per_second = bpm / 60.0;
    events
        .into_iter()
        .map(|event| PitchedNote {
            onset_beats: event.start_secs.max(0.0) * beats_per_second,
            duration_beats: ((event.end_secs - event.start_secs) * beats_per_second).max(1.0 / 16.0),
            midi: event.midi,
            velocity: finite_amplitude(event).clamp(0.05, 1.0),
        })
        .collect()
}

fn loudest_non_overlapping(mut candidates: Vec<&NoteEvent>, limit: usize) -> Vec<&NoteEvent> {
    candidates.sort_by(|left, right| {
        finite_amplitude(right)
            .total_cmp(&finite_amplitude(left))
            .then_with(|| left.start_secs.total_cmp(&right.start_secs))
            .then_with(|| left.midi.cmp(&right.midi))
    });
    let mut selected: Vec<&NoteEvent> = Vec::new();
    for candidate in candidates {
        let mut boundaries = vec![candidate.start_secs, candidate.end_secs];
        for note in &selected {
            if overlaps(candidate, note) {
                boundaries.push(note.start_secs.max(candidate.start_secs));
                boundaries.push(note.end_secs.min(candidate.end_secs));
            }
        }
        boundaries.sort_by(f64::total_cmp);
        boundaries.dedup_by(|left, right| left.total_cmp(right).is_eq());
        let crowded = boundaries.windows(2).any(|window| {
            let midpoint = (window[0] + window[1]) * 0.5;
            midpoint >= candidate.start_secs
                && midpoint < candidate.end_secs
                && selected
                    .iter()
                    .filter(|note| midpoint >= note.start_secs && midpoint < note.end_secs)
                    .count()
                    >= limit
        });
        if !crowded {
            selected.push(candidate);
        }
    }
    selected
}

fn finite_amplitude(event: &NoteEvent) -> f32 {
    if event.amplitude.is_finite() { event.amplitude.max(0.0) } else { 0.0 }
}

fn overlaps(left: &NoteEvent, right: &NoteEvent) -> bool {
    left.start_secs < right.end_secs && right.start_secs < left.end_secs
}

fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

#[cfg(test)]
mod notes_map_tests {
    use super::*;

    fn event(start: f64, end: f64, midi: u8, amplitude: f32) -> NoteEvent {
        NoteEvent { start_secs: start, end_secs: end, midi, amplitude, bends: Vec::new() }
    }

    #[test]
    fn notes_map_converts_seconds_clamps_velocity_and_drops_tiny_notes() {
        let notes = map_notes(
            &[event(0.5, 0.53, 60, 0.9), event(1.0, 1.04, 64, 0.01)],
            60.0,
            PitchLane::Other,
        );
        assert_eq!(notes.len(), 1);
        assert!((notes[0].onset_beats - 1.0).abs() < 1.0e-9);
        assert!((notes[0].duration_beats - 1.0 / 16.0).abs() < 1.0e-9);
        assert_eq!(notes[0].midi, 64);
        assert!((notes[0].velocity - 0.05).abs() < f32::EPSILON);
    }

    #[test]
    fn notes_map_bass_drops_supported_octave_and_nineteenth_overtones() {
        let notes = map_notes(
            &[
                event(0.0, 1.0, 40, 0.6),
                event(0.1, 0.9, 52, 1.0),
                event(0.2, 0.8, 59, 0.8),
                event(0.2, 0.8, 47, 0.7),
            ],
            120.0,
            PitchLane::Bass,
        );
        assert_eq!(notes.iter().map(|note| note.midi).collect::<Vec<_>>(), [40, 47]);
    }

    #[test]
    fn notes_map_bass_keeps_an_upper_note_without_a_strong_fundamental() {
        let notes = map_notes(
            &[event(0.0, 1.0, 40, 0.59), event(0.0, 1.0, 52, 1.0)],
            120.0,
            PitchLane::Bass,
        );
        assert_eq!(notes.iter().map(|note| note.midi).collect::<Vec<_>>(), [40, 52]);
    }

    #[test]
    fn notes_map_melody_keeps_the_loudest_simultaneous_note() {
        let notes = map_notes(
            &[
                event(0.0, 1.0, 60, 0.4),
                event(0.0, 1.0, 67, 0.8),
                event(1.0, 2.0, 69, 0.5),
            ],
            120.0,
            PitchLane::Melody,
        );
        assert_eq!(notes.iter().map(|note| note.midi).collect::<Vec<_>>(), [67, 69]);
    }

    #[test]
    fn notes_map_other_keeps_at_most_four_loudest_simultaneous_notes() {
        let notes = map_notes(
            &[
                event(0.0, 1.0, 60, 0.1),
                event(0.0, 1.0, 61, 0.2),
                event(0.0, 1.0, 62, 0.3),
                event(0.0, 1.0, 63, 0.4),
                event(0.0, 1.0, 64, 0.5),
            ],
            120.0,
            PitchLane::Other,
        );
        assert_eq!(notes.iter().map(|note| note.midi).collect::<Vec<_>>(), [61, 62, 63, 64]);
    }

    #[test]
    fn notes_resampler_preserves_one_kilohertz_frequency_and_amplitude() {
        let input_rate = 48_000u32;
        let frequency = 1_000.0;
        let amplitude = 0.75;
        let input: Vec<f32> = (0..input_rate / 2)
            .map(|index| {
                (amplitude
                    * (2.0 * std::f64::consts::PI * frequency * index as f64
                        / input_rate as f64)
                        .sin()) as f32
            })
            .collect();
        let output = resample_to_basic_pitch(&input, input_rate);
        let edge = 100;
        let interior = &output[edge..output.len() - edge];
        let crossings: Vec<usize> = interior
            .windows(2)
            .enumerate()
            .filter_map(|(index, pair)| (pair[0] <= 0.0 && pair[1] > 0.0).then_some(index))
            .collect();
        let cycles = crossings.len() - 1;
        let seconds = (crossings[crossings.len() - 1] - crossings[0]) as f64
            / makepad_ai_notes::SAMPLE_RATE as f64;
        let measured_frequency = cycles as f64 / seconds;
        let measured_amplitude = interior.iter().copied().map(f32::abs).fold(0.0, f32::max);
        assert!((measured_frequency / frequency - 1.0).abs() < 0.005);
        assert!((f64::from(measured_amplitude) / amplitude - 1.0).abs() < 0.03);
    }
}
