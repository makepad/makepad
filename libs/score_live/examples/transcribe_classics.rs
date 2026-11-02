//! Writes `examples/classical/*.score.splash` again from the performances (see `tests/classical.rs`).

#[path = "../tests/classical/pieces.rs"]
mod pieces;
use makepad_score_live::midi;
use pieces::{examples, pieces, read_midi};

fn main() {
    std::fs::create_dir_all(examples()).unwrap();
    for (stem, piece) in pieces() {
        let bytes = read_midi(stem).expect("the performances are in apps/score/resources/performances");
        let source = midi::transcribe(&bytes, &piece).unwrap_or_else(|e| panic!("{stem}: {e}"));
        std::fs::write(examples().join(format!("{stem}.score.splash")), &source).unwrap();
        let c = midi::check(&bytes, &piece, &source).unwrap_or_else(|e| panic!("{stem}: {e}"));
        println!("{stem}: {} notes, {} pedal moves, {:.1} s, onset {:.3} ms, release {:.3} ms, pedal {:.3} ms, velocity {}", c.notes, c.pedal_moves, c.seconds, c.onset * 1e3, c.release * 1e3, c.pedal * 1e3, c.velocity);
    }
}

