//! The classical piano pieces in `examples/classical`: Bernd Krueger's
//! performances (piano-midi.de, CC BY-SA 3.0 DE; the MIDI files ship with
//! Makepad's Score app) transcribed into score code by `score::midi`.
//!
//! - `classics_round_trip`: each file is what the transcriber writes today,
//!   and plays its performance: every note starts and ends within a
//!   millisecond of the file, the pedal moves with it.
//! - `cargo run --release -p makepad-score-live --example transcribe_classics` writes the files again.

#[path = "classical/pieces.rs"]
mod pieces;
use makepad_score_live::midi;
use pieces::{examples, pieces, read_midi};

#[test]
fn classics_round_trip() {
    for (stem, piece) in pieces() {
        let source = std::fs::read_to_string(examples().join(format!("{stem}.score.splash"))).unwrap_or_else(|e| panic!("{stem}: {e}"));
        makepad_score_live::load(&source).unwrap_or_else(|d| panic!("{stem}: {d:?}"));
        let bytes = read_midi(stem).expect("the performances are in apps/score/resources/performances");
        assert_eq!(midi::transcribe(&bytes, &piece).unwrap(), source, "{stem} is not what the transcriber writes: run transcribe_classics");
        let c = midi::check(&bytes, &piece, &source).unwrap_or_else(|e| panic!("{stem}: {e}"));
        assert!(c.onset < 1e-3 && c.release < 1e-3 && c.pedal < 1e-3, "{stem}: {c:?}");
        assert!(c.velocity <= 1 && c.pedal_moves > 0, "{stem}: {c:?}");
    }
}
