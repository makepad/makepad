use makepad_ai_notes::config::{AUDIO_N_SAMPLES, SAMPLE_RATE};
use makepad_ai_notes::{NotesModel, MODEL_FILE};
use std::path::{Path, PathBuf};

fn checkpoint() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../local/models/weights/basic_pitch/nmp.onnx")
}

fn frequency(midi: f64) -> f64 {
    440.0 * 2.0f64.powf((midi - 69.0) / 12.0)
}

fn attacked_sine(midi: f64, seconds: f64) -> Vec<f32> {
    let samples = (seconds * SAMPLE_RATE as f64).round() as usize;
    (0..samples)
        .map(|index| {
            let time = index as f64 / SAMPLE_RATE as f64;
            let attack = (time / 0.008).min(1.0);
            let release = ((seconds - time) / 0.015).clamp(0.0, 1.0);
            (0.75 * attack * release
                * (std::f64::consts::TAU * frequency(midi) * time).sin()) as f32
        })
        .collect()
}

fn has_note_near(notes: &[makepad_ai_notes::NoteEvent], midi: u8, start: f64) -> bool {
    notes
        .iter()
        .any(|note| note.midi == midi && (note.start_secs - start).abs() <= 0.040)
}

#[test]
fn silence_produces_no_notes() {
    let mut model = NotesModel::load(checkpoint()).unwrap();
    let result = model.transcribe(&vec![0.0; AUDIO_N_SAMPLES]).unwrap();
    assert!(result.notes.is_empty());
}

#[test]
fn four_note_bass_line_has_expected_pitches_and_onsets() {
    let mut audio = Vec::new();
    for midi in [28.0, 33.0, 38.0, 43.0] {
        audio.extend(attacked_sine(midi, 0.5));
    }
    let mut model = NotesModel::load(checkpoint()).unwrap();
    let result = model.transcribe(&audio).unwrap();
    assert_eq!(result.notes.len(), 4, "unexpected bass notes: {:?}", result.notes);
    for (midi, start) in [(28, 0.0), (33, 0.5), (38, 1.0), (43, 1.5)] {
        assert!(
            has_note_near(&result.notes, midi, start),
            "missing MIDI {midi} near {start:.2}s; got {:?}",
            result.notes
        );
    }
}

#[test]
fn c_major_triad_is_simultaneous() {
    let voices = [attacked_sine(60.0, 0.7), attacked_sine(64.0, 0.7), attacked_sine(67.0, 0.7)];
    let mut audio = vec![0.0f32; voices[0].len()];
    for voice in voices {
        for (sample, value) in audio.iter_mut().zip(voice) {
            *sample += value / 3.0;
        }
    }
    let mut model = NotesModel::load(checkpoint()).unwrap();
    let result = model.transcribe(&audio).unwrap();
    assert_eq!(result.notes.len(), 3, "unexpected triad notes: {:?}", result.notes);
    for midi in [60, 64, 67] {
        assert!(
            has_note_near(&result.notes, midi, 0.0),
            "missing triad MIDI {midi}; got {:?}",
            result.notes
        );
    }
}

#[test]
fn one_semitone_glide_has_rising_bends() {
    let seconds = 0.8;
    let samples = (seconds * SAMPLE_RATE as f64) as usize;
    let mut phase = 0.0f64;
    let mut audio = Vec::with_capacity(samples);
    for index in 0..samples {
        let time = index as f64 / SAMPLE_RATE as f64;
        let midi = 45.0 + time / seconds;
        phase += std::f64::consts::TAU * frequency(midi) / SAMPLE_RATE as f64;
        let envelope = (time / 0.008).min(1.0) * ((seconds - time) / 0.015).clamp(0.0, 1.0);
        audio.push((0.75 * envelope * phase.sin()) as f32);
    }
    let mut model = NotesModel::load(checkpoint()).unwrap();
    let result = model.transcribe(&audio).unwrap();
    let note = result
        .notes
        .iter()
        .filter(|note| note.midi == 45 || note.midi == 46)
        .max_by(|a, b| a.end_secs.total_cmp(&b.end_secs))
        .unwrap_or_else(|| panic!("missing gliding A; got {:?}", result.notes));
    let reversals = note.bends.windows(2).filter(|pair| pair[1] < pair[0]).count();
    assert!(reversals <= 1, "non-rising bend trend: {:?}", note.bends);
    assert!(note.bends.last().unwrap_or(&0.0) > note.bends.first().unwrap_or(&0.0));
}

#[test]
fn overlap_seam_does_not_duplicate_a_sustained_note() {
    let audio = attacked_sine(45.0, 2.4);
    let mut model = NotesModel::load(checkpoint()).unwrap();
    let result = model.transcribe(&audio).unwrap();
    let long_a_notes = result
        .notes
        .iter()
        .filter(|note| note.midi == 45 && note.end_secs - note.start_secs > 0.4)
        .count();
    assert_eq!(long_a_notes, 1, "seam duplicated A2: {:?}", result.notes);
}

#[test]
fn model_file_constant_matches_registry_cache_name() {
    assert_eq!(MODEL_FILE, "basic_pitch_nmp.onnx");
}
