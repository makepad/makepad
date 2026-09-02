use makepad_drumkit::DrumVoice;
use makepad_piano_model::PianoEvent;
use makepad_score_view::build::{DrumHit, PitchedNote};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PreviewEvent {
    Piano(PianoEvent),
    Drum { voice: DrumVoice, velocity: f32 },
}

#[derive(Clone, Debug)]
pub struct PreviewSequence {
    pub sample_rate: u32,
    pub events: Vec<(u64, PreviewEvent)>,
    pub len_frames: u64,
    pub looped: bool,
}

fn usable_bpm(bpm: f64) -> f64 {
    if bpm.is_finite() && bpm > 0.0 { bpm } else { 120.0 }
}

fn beat_frame(beats: f64, bpm: f64, sample_rate: u32) -> u64 {
    if !beats.is_finite() || beats <= 0.0 {
        return 0;
    }
    (beats * 60.0 / usable_bpm(bpm) * sample_rate.max(1) as f64).round() as u64
}

fn sequence_len(bpm: f64, bars: u32, sample_rate: u32) -> u64 {
    beat_frame(bars.max(1) as f64 * 4.0, bpm, sample_rate).max(1)
}

pub fn sequence_from_drums(
    hits: &[DrumHit],
    bpm: f64,
    bars: u32,
    sample_rate: u32,
    looped: bool,
) -> PreviewSequence {
    let sample_rate = sample_rate.max(1);
    let mut events = Vec::with_capacity(hits.len());
    for hit in hits {
        let Ok(voice) = DrumVoice::try_from(hit.voice.gm_note()) else { continue };
        events.push((
            beat_frame(hit.time_beats, bpm, sample_rate),
            PreviewEvent::Drum {
                voice,
                velocity: hit.velocity.clamp(0.0, 1.0),
            },
        ));
    }
    events.sort_by_key(|event| event.0);
    PreviewSequence {
        sample_rate,
        events,
        len_frames: sequence_len(bpm, bars, sample_rate),
        looped,
    }
}

pub fn sequence_from_notes(
    notes: &[PitchedNote],
    bpm: f64,
    bars: u32,
    sample_rate: u32,
    looped: bool,
) -> PreviewSequence {
    let sample_rate = sample_rate.max(1);
    let mut events = Vec::with_capacity(notes.len() * 2 + 1);
    events.push((0, PreviewEvent::Piano(PianoEvent::Sustain { value: 0.0 })));
    for note in notes {
        let velocity = (note.velocity.clamp(0.0, 1.0) * 126.0).round() as u8 + 1;
        events.push((
            beat_frame(note.onset_beats, bpm, sample_rate),
            PreviewEvent::Piano(PianoEvent::NoteOn { key: note.midi, velocity }),
        ));
        events.push((
            beat_frame(note.onset_beats + note.duration_beats.max(0.0), bpm, sample_rate),
            PreviewEvent::Piano(PianoEvent::NoteOff { key: note.midi }),
        ));
    }
    events.sort_by_key(|event| event.0);
    PreviewSequence {
        sample_rate,
        events,
        len_frames: sequence_len(bpm, bars, sample_rate),
        looped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_score_view::build::DrumVoice as ScoreDrumVoice;

    #[test]
    fn score_preview_drums_are_timed_in_device_frames() {
        let sequence = sequence_from_drums(
            &[
                DrumHit { time_beats: 0.0, voice: ScoreDrumVoice::Kick, velocity: 0.75 },
                DrumHit { time_beats: 1.5, voice: ScoreDrumVoice::Snare, velocity: 0.5 },
            ],
            120.0,
            2,
            48_000,
            true,
        );
        assert_eq!(sequence.len_frames, 192_000);
        assert_eq!(sequence.events[0].0, 0);
        assert_eq!(sequence.events[1].0, 36_000);
        assert_eq!(
            sequence.events[1].1,
            PreviewEvent::Drum { voice: DrumVoice::Snare, velocity: 0.5 }
        );
        assert!(sequence.looped);
    }

    #[test]
    fn score_preview_notes_have_pedal_on_off_and_exact_velocity() {
        let sequence = sequence_from_notes(
            &[PitchedNote {
                onset_beats: 0.5,
                duration_beats: 1.25,
                midi: 64,
                velocity: 0.5,
            }],
            60.0,
            1,
            48_000,
            false,
        );
        assert_eq!(sequence.len_frames, 192_000);
        assert_eq!(sequence.events[0], (0, PreviewEvent::Piano(PianoEvent::Sustain { value: 0.0 })));
        assert_eq!(
            sequence.events[1],
            (24_000, PreviewEvent::Piano(PianoEvent::NoteOn { key: 64, velocity: 64 }))
        );
        assert_eq!(
            sequence.events[2],
            (84_000, PreviewEvent::Piano(PianoEvent::NoteOff { key: 64 }))
        );
        assert!(!sequence.looped);
    }
}
