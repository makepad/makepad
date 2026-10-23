//! `midi-text <id | "Artist - Title" | search words | file.mid | song.splash> [--bars 1-16] [--parts bass,drums] [--detail full|chords|drums|melody|bass] [--all]`:
//! a song as the text transcription an AI reads (`midi_library text`).

#[path = "midi_library.rs"]
#[allow(dead_code)]
mod library;

fn main() {
    let mut args = vec!["text".to_string()];
    args.extend(std::env::args().skip(1));
    library::run(args);
}
