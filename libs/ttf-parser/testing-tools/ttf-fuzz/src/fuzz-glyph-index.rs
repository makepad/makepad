#[macro_use]
extern crate afl;

const CHARS: &[char] = &[
    '\u{0}',
    'A',
    'Ф',
    '0',
    '\u{D7FF}',
    '\u{10FFFF}',
];

fn main() {
    afl::fuzz!(|data: &[u8]| {
        if let Some(face) = ttf_parser::Face::parse(data, 0) {
            for c in CHARS {
                let _ = face.glyph_index(*c);
            }
        }
    });
}
