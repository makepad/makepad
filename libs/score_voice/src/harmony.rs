//! Harmony parts under a melody, from the Score's chords: each lower part
//! takes a chord tone below the part above it (at least a whole tone
//! apart, within a twelfth of the melody), choosing the one nearest its
//! own previous note so the parts move smoothly (voice leading).

/// Parts for `melody` over `chord` (MIDI pitches, any octave): the melody
/// first, then `parts - 1` lower voices. `prev` is the previous result (or
/// empty) for voice leading.
pub fn harmonize(melody: f32, chord: &[f32], parts: usize, prev: &[f32]) -> Vec<f32> {
    let mut out = vec![melody];
    if chord.is_empty() {
        return out;
    }
    let classes: Vec<f32> = chord.iter().map(|p| p.rem_euclid(12.0)).collect();
    // Every chord tone from a twelfth below the melody up to it.
    let low = melody - 19.0;
    let mut tones = Vec::new();
    let mut octave = (low / 12.0).floor() * 12.0;
    while octave <= melody {
        for c in &classes {
            let p = octave + c;
            if p >= low && p < melody - 0.5 {
                tones.push(p);
            }
        }
        octave += 12.0;
    }
    tones.sort_by(|a, b| b.partial_cmp(a).unwrap());
    tones.dedup();
    for part in 1..parts {
        let above = *out.last().unwrap();
        let candidates: Vec<f32> = tones.iter().cloned().filter(|t| *t <= above - 2.0).collect();
        let pick = match (candidates.first(), prev.get(part)) {
            (None, _) => above - 12.0,
            (Some(highest), None) => *highest,
            (Some(_), Some(p)) => candidates.iter().cloned().min_by(|a, b| (a - p).abs().partial_cmp(&(b - p).abs()).unwrap()).unwrap(),
        };
        out.push(pick);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_harmony_under_the_melody_with_voice_leading() {
        // C major under E5: C5 then G4.
        assert_eq!(harmonize(76.0, &[60.0, 64.0, 67.0], 3, &[]), vec![76.0, 72.0, 67.0]);
        // F major under F5, leading from C5 / G4: stays near (C5, A4).
        let next = harmonize(77.0, &[65.0, 69.0, 72.0], 3, &[76.0, 72.0, 67.0]);
        assert_eq!(next, vec![77.0, 72.0, 69.0]);
        // No chord: the melody alone.
        assert_eq!(harmonize(70.0, &[], 3, &[]), vec![70.0]);
    }
}
