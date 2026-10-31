//! Notes, chords and scales: note names (`@c4`, `@fs2`, `@eb3`, `c#4`) to
//! MIDI numbers and back, and the chord and scale vocabularies a score
//! speaks (`chord(@a2, @m7)`, `scale(@d3, @dorian, 2)`).

/// A note name to its MIDI number: `c4` is 60; `s`/`#` sharpen, a `b`
/// after the letter flattens; an omitted octave is 4.
pub fn note_number(name: &str) -> Option<f32> {
    let name = name.trim();
    let bytes = name.as_bytes();
    let first = bytes.first()?.to_ascii_lowercase();
    let base = match first {
        b'c' => 0,
        b'd' => 2,
        b'e' => 4,
        b'f' => 5,
        b'g' => 7,
        b'a' => 9,
        b'b' => 11,
        _ => return None,
    };
    let mut at = 1;
    let mut shift = 0i32;
    while at < bytes.len() {
        match bytes[at] {
            b's' | b'#' => shift += 1,
            // `b` right after the letter (or another flat) is a flat, never
            // an octave: `bb3` is B-flat 3.
            b'b' => shift -= 1,
            _ => break,
        }
        at += 1;
    }
    let rest = &name[at..];
    let octave: i32 = if rest.is_empty() {
        4
    } else if let Some(neg) = rest.strip_prefix('m').or_else(|| rest.strip_prefix('-')) {
        -neg.parse::<i32>().ok()?
    } else {
        rest.parse().ok()?
    };
    let midi = (octave + 1) * 12 + base + shift;
    (0..=127).contains(&midi).then_some(midi as f32)
}

pub const NAMES: [&str; 12] = ["c", "cs", "d", "ds", "e", "f", "fs", "g", "gs", "a", "as", "b"];

/// `60.0` to `c4` (fractions round to the nearest note).
pub fn note_name(midi: f32) -> String {
    let n = midi.round() as i32;
    format!("{}{}", NAMES[n.rem_euclid(12) as usize], n.div_euclid(12) - 1)
}

/// Chord kinds as semitones from the root.
pub fn chord_kind(name: &str) -> Option<&'static [i32]> {
    Some(match name {
        "major" | "maj" | "M" => &[0, 4, 7],
        "minor" | "min" | "m" => &[0, 3, 7],
        "dim" | "diminished" => &[0, 3, 6],
        "aug" | "augmented" => &[0, 4, 8],
        "sus2" => &[0, 2, 7],
        "sus4" | "sus" => &[0, 5, 7],
        "5" | "power" => &[0, 7],
        "7" | "dom7" => &[0, 4, 7, 10],
        "maj7" | "M7" => &[0, 4, 7, 11],
        "m7" | "min7" | "minor7" => &[0, 3, 7, 10],
        "m7b5" | "half_dim" => &[0, 3, 6, 10],
        "dim7" => &[0, 3, 6, 9],
        "mmaj7" => &[0, 3, 7, 11],
        "6" | "maj6" => &[0, 4, 7, 9],
        "m6" => &[0, 3, 7, 9],
        "9" | "dom9" => &[0, 4, 7, 10, 14],
        "maj9" => &[0, 4, 7, 11, 14],
        "m9" | "min9" => &[0, 3, 7, 10, 14],
        "add9" => &[0, 4, 7, 14],
        "madd9" => &[0, 3, 7, 14],
        "11" => &[0, 4, 7, 10, 14, 17],
        "m11" => &[0, 3, 7, 10, 14, 17],
        "13" => &[0, 4, 7, 10, 14, 21],
        _ => return None,
    })
}

pub const CHORD_KINDS: [&str; 22] = [
    "major", "minor", "dim", "aug", "sus2", "sus4", "5", "7", "maj7", "m7", "m7b5", "dim7", "mmaj7", "6", "m6", "9", "maj9", "m9", "add9", "madd9", "11", "m11",
];

/// Scale kinds as semitones within one octave.
pub fn scale_kind(name: &str) -> Option<&'static [i32]> {
    Some(match name {
        "major" | "ionian" => &[0, 2, 4, 5, 7, 9, 11],
        "minor" | "aeolian" | "natural_minor" => &[0, 2, 3, 5, 7, 8, 10],
        "harmonic_minor" => &[0, 2, 3, 5, 7, 8, 11],
        "melodic_minor" => &[0, 2, 3, 5, 7, 9, 11],
        "dorian" => &[0, 2, 3, 5, 7, 9, 10],
        "phrygian" => &[0, 1, 3, 5, 7, 8, 10],
        "lydian" => &[0, 2, 4, 6, 7, 9, 11],
        "mixolydian" => &[0, 2, 4, 5, 7, 9, 10],
        "locrian" => &[0, 1, 3, 5, 6, 8, 10],
        "major_pentatonic" | "pentatonic" => &[0, 2, 4, 7, 9],
        "minor_pentatonic" => &[0, 3, 5, 7, 10],
        "blues" => &[0, 3, 5, 6, 7, 10],
        "chromatic" => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        "whole_tone" => &[0, 2, 4, 6, 8, 10],
        "hirajoshi" => &[0, 2, 3, 7, 8],
        "phrygian_dominant" => &[0, 1, 4, 5, 7, 8, 10],
        _ => return None,
    })
}

pub const SCALE_KINDS: [&str; 16] = [
    "major", "minor", "harmonic_minor", "melodic_minor", "dorian", "phrygian", "lydian", "mixolydian", "locrian", "major_pentatonic",
    "minor_pentatonic", "blues", "chromatic", "whole_tone", "hirajoshi", "phrygian_dominant",
];

/// The notes of a chord on `root`, inverted `inversion` times (each
/// inversion lifts the lowest note an octave).
pub fn chord(root: f32, kind: &[i32], inversion: i32) -> Vec<f32> {
    let mut notes: Vec<f32> = kind.iter().map(|s| root + *s as f32).collect();
    for _ in 0..inversion.max(0).min(8) {
        notes.sort_by(f32::total_cmp);
        if let Some(low) = notes.first_mut() {
            *low += 12.0;
        }
    }
    notes.sort_by(f32::total_cmp);
    notes
}

/// `octaves` octaves of a scale from `root`, plus the top root.
pub fn scale(root: f32, kind: &[i32], octaves: i32) -> Vec<f32> {
    let octaves = octaves.clamp(1, 8);
    let mut notes = Vec::new();
    for octave in 0..octaves {
        for step in kind {
            notes.push(root + (octave * 12 + step) as f32);
        }
    }
    notes.push(root + (octaves * 12) as f32);
    notes
}

/// A Euclidean rhythm: `hits` onsets spread over `steps`, rotated.
pub fn euclid(hits: i32, steps: i32, rotate: i32) -> Vec<bool> {
    let steps = steps.clamp(1, 256);
    let hits = hits.clamp(0, steps);
    let mut out: Vec<bool> = (0..steps).map(|i| (i * hits) % steps < hits && hits > 0).collect();
    // The Bresenham form starts on a hit; rotate to the requested phase.
    let r = rotate.rem_euclid(steps) as usize;
    out.rotate_left(r);
    out
}

/// A step pattern: `x` hit, `X` accent, `o` ghost, digits 1..9 a velocity
/// in tenths, `.`/`-`/`_` rest; spaces and `|` are ignored.
pub fn pattern(text: &str) -> Vec<Option<f32>> {
    text.chars().filter(|c| !c.is_whitespace() && *c != '|').map(|c| match c {
        'x' => Some(0.8),
        'X' => Some(1.0),
        'o' => Some(0.45),
        '1'..='9' => Some((c as u8 - b'0') as f32 / 10.0),
        _ => None,
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_numbers() {
        assert_eq!(note_number("c4"), Some(60.0));
        assert_eq!(note_number("a4"), Some(69.0));
        assert_eq!(note_number("fs2"), Some(42.0));
        assert_eq!(note_number("c#4"), Some(61.0));
        assert_eq!(note_number("eb3"), Some(51.0));
        assert_eq!(note_number("bb3"), Some(58.0));
        assert_eq!(note_number("b3"), Some(59.0));
        assert_eq!(note_number("e"), Some(64.0));
        assert_eq!(note_number("kick"), None);
        assert_eq!(note_name(42.0), "fs2");
        assert_eq!(chord(57.0, chord_kind("m7").unwrap(), 1), vec![60.0, 64.0, 67.0, 69.0]);
        assert_eq!(scale(60.0, scale_kind("minor_pentatonic").unwrap(), 1), vec![60.0, 63.0, 65.0, 67.0, 70.0, 72.0]);
        assert_eq!(euclid(3, 8, 0).iter().filter(|x| **x).count(), 3);
        assert_eq!(pattern("x.o. X..5"), vec![Some(0.8), None, Some(0.45), None, Some(1.0), None, None, Some(0.5)]);
    }
}
