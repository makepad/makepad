//! The front-end: a sung line (notes + syllables) to the frames the acoustic
//! model reads. Every singer follows the same alignment rule: the vowel owns
//! the note, onset consonants come *before* the note so the vowel lands on the
//! beat, coda consonants take the end of the note, and a melisma carries the
//! vowel across notes without a new attack.

use crate::dsp::{hz_to_midi, midi_to_hz, Rng, F0_REF};
use crate::phonemes::{self as ph, Ph};

/// A syllable: consonants before the vowel, the vowel (two for a diphthong),
/// consonants after it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Syllable {
    pub onset: Vec<Ph>,
    pub nucleus: Vec<Ph>,
    pub coda: Vec<Ph>,
}

impl Syllable {
    /// Split a phoneme run (one syllable's worth) at its vowels.
    pub fn from_phones(p: &[Ph]) -> Syllable {
        let first = p.iter().position(|x| ph::is_vowel(*x));
        match first {
            None => Syllable { onset: p.to_vec(), nucleus: Vec::new(), coda: Vec::new() },
            Some(f) => {
                let mut l = f;
                while l + 1 < p.len() && ph::is_vowel(p[l + 1]) {
                    l += 1;
                }
                Syllable { onset: p[..f].to_vec(), nucleus: p[f..=l].to_vec(), coda: p[l + 1..].to_vec() }
            }
        }
    }
}

/// One sung note. `syllable: None` continues the previous vowel (melisma).
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub start: f32,
    pub dur: f32,
    pub midi: f32,
    pub vel: f32,
    pub syllable: Option<Syllable>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SingScore {
    pub notes: Vec<Note>,
    pub singer: usize,
}

/// Expressive pitch, sung in tune: a timed glide (or a scoop into a fresh
/// note) through the onset consonants that lands by the vowel, delayed
/// vibrato swinging evenly about the note, a little drift.
#[derive(Clone, Debug, PartialEq)]
pub struct PitchStyle {
    /// Glide time between notes (s); wider intervals take a little longer,
    /// and a glide lands by the vowel plus half this at the latest.
    pub portamento: f32,
    /// Semitones below a fresh note (after a rest) the voice starts from.
    pub scoop: f32,
    /// Vibrato peak depth (semitones) and rate (Hz).
    pub vibrato_depth: f32,
    pub vibrato_rate: f32,
    /// Seconds after the vowel starts before vibrato begins, and its fade in.
    pub vibrato_delay: f32,
    pub vibrato_fade: f32,
    /// Slow random drift, semitones (std).
    pub drift: f32,
    pub seed: u64,
}

impl Default for PitchStyle {
    fn default() -> Self {
        PitchStyle { portamento: 0.08, scoop: 0.3, vibrato_depth: 0.3, vibrato_rate: 5.4, vibrato_delay: 0.22, vibrato_fade: 0.35, drift: 0.025, seed: 1 }
    }
}

/// The aligned line, one row per 10 ms frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frames {
    /// The phoneme sequence and each token's length in frames.
    pub tokens: Vec<Ph>,
    pub token_frames: Vec<usize>,
    pub token_of_frame: Vec<usize>,
    /// Note MIDI per frame (0 in rests), the note-attack flag, velocity.
    pub note: Vec<f32>,
    pub onset: Vec<f32>,
    pub vel: Vec<f32>,
    /// Position within the token, 0..1.
    pub pos: Vec<f32>,
    /// f0 in Hz, 0 where unvoiced.
    pub f0: Vec<f32>,
    pub singer: usize,
}

/// Width of the per-frame feature row the acoustic model reads.
pub const FEATS: usize = 8;

impl Frames {
    pub fn len(&self) -> usize {
        self.token_of_frame.len()
    }

    pub fn is_empty(&self) -> bool {
        self.token_of_frame.is_empty()
    }

    /// Note-side features `[T, FEATS]`: note, has-note, attack, velocity,
    /// position, and three f0 columns left zero (see `f0_feats`).
    pub fn note_feats(&self) -> Vec<f32> {
        let mut out = vec![0.0; self.len() * FEATS];
        for t in 0..self.len() {
            let r = &mut out[t * FEATS..(t + 1) * FEATS];
            let has = self.note[t] > 0.0;
            r[0] = if has { (self.note[t] - 60.0) / 12.0 } else { 0.0 };
            r[1] = has as u8 as f32;
            r[2] = self.onset[t];
            r[3] = self.vel[t];
            r[4] = self.pos[t];
        }
        out
    }

    /// f0 features `[T, FEATS]` in the last three columns: ln(f0/ref), voiced,
    /// and the f0's offset from the note in semitones.
    pub fn f0_feats(&self, f0: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0; self.len() * FEATS];
        for t in 0..self.len() {
            let r = &mut out[t * FEATS..(t + 1) * FEATS];
            if f0[t] > 0.0 {
                r[5] = (f0[t] / F0_REF).ln();
                r[6] = 1.0;
                if self.note[t] > 0.0 {
                    r[7] = ((hz_to_midi(f0[t]) - self.note[t]) / 2.0).clamp(-3.0, 3.0);
                }
            }
        }
        out
    }
}

/// One aligned segment in seconds.
#[derive(Clone, Debug)]
struct Seg {
    ph: Ph,
    start: f32,
    end: f32,
    /// Index of the note this segment is sung on (for pitch), if any.
    note: Option<usize>,
}

/// How long each consonant takes (seconds): the rule table, or the duration
/// model's predictions in the same order as the tokens `align` visits.
pub type ConsonantLen<'a> = &'a dyn Fn(Ph, bool) -> f32;

pub fn rule_consonants(p: Ph, coda: bool) -> f32 {
    ph::rule_len(p, coda)
}

/// Align the line: phoneme segments on the note timeline, then frames. The
/// timeline starts `lead_in` seconds before the first note.
pub fn align(score: &SingScore, clen: ConsonantLen, style: &PitchStyle, lead_in: f32) -> Frames {
    let notes = &score.notes;
    let mut segs: Vec<Seg> = Vec::new();
    let t0 = notes.first().map(|n| n.start).unwrap_or(0.0) - lead_in;
    let mut cursor = t0;
    let mut i = 0;
    while i < notes.len() {
        let n = &notes[i];
        let syl = n.syllable.clone().unwrap_or_default();
        // The notes this syllable's vowel spans (it and the melisma after it).
        let mut j = i;
        while j + 1 < notes.len() && notes[j + 1].syllable.is_none() {
            j += 1;
        }
        let end = notes[j].start + notes[j].dur;
        // Onset before the beat: never more than 60 % of the time since the
        // previous segment boundary, and at most 0.3 s.
        let mut onset: Vec<(Ph, f32)> = syl.onset.iter().map(|p| (*p, clen(*p, false))).collect();
        let total_on: f32 = onset.iter().map(|o| o.1).sum();
        let room = ((n.start - cursor) * 0.6).max(0.0) + if segs.last().map(|s| ph::is_vowel(s.ph)).unwrap_or(false) {
            (segs.last().unwrap().end - segs.last().unwrap().start) * 0.4
        } else {
            0.0
        };
        let on_scale = if total_on > room.min(0.3) && total_on > 0.0 { room.min(0.3) / total_on } else { 1.0 };
        for o in &mut onset {
            o.1 *= on_scale;
        }
        let on_start = n.start - onset.iter().map(|o| o.1).sum::<f32>();
        // Silence (or a breath) before the syllable.
        if on_start > cursor + 1e-4 {
            let gap = on_start - cursor;
            if gap >= 0.3 && !segs.is_empty() || gap >= 0.3 && i == 0 && lead_in >= 0.3 {
                let br = (gap * 0.5).min(0.35);
                segs.push(Seg { ph: ph::SP, start: cursor, end: on_start - br, note: None });
                segs.push(Seg { ph: ph::AP, start: on_start - br, end: on_start, note: None });
            } else {
                segs.push(Seg { ph: ph::SP, start: cursor, end: on_start, note: None });
            }
        } else if on_start < cursor {
            // The onset steals from the previous vowel.
            if let Some(last) = segs.last_mut() {
                last.end = on_start.max(last.start + 0.02);
            }
        }
        let mut t = segs.last().map(|s| s.end).unwrap_or(on_start).max(on_start.min(cursor));
        if segs.is_empty() {
            t = on_start;
        }
        for (p, l) in onset {
            segs.push(Seg { ph: p, start: t, end: t + l, note: Some(i) });
            t += l;
        }
        // Coda at the end of the last melisma note; the vowel keeps at least 40 %.
        let mut coda: Vec<(Ph, f32)> = syl.coda.iter().map(|p| (*p, clen(*p, true))).collect();
        let span = (end - t).max(0.03);
        let total_coda: f32 = coda.iter().map(|c| c.1).sum();
        if total_coda > span * 0.6 {
            let s = span * 0.6 / total_coda;
            for c in &mut coda {
                c.1 *= s;
            }
        }
        let vowel_end = end - coda.iter().map(|c| c.1).sum::<f32>();
        match syl.nucleus.len() {
            0 => {}
            1 => {
                // A single vowel: split per note of the melisma so each note has its pitch.
                push_vowel(&mut segs, syl.nucleus[0], t, vowel_end, notes, i, j);
            }
            _ => {
                let glide = ((vowel_end - t) * 0.3).min(0.12);
                push_vowel(&mut segs, syl.nucleus[0], t, vowel_end - glide, notes, i, j);
                let last_note = j;
                segs.push(Seg { ph: *syl.nucleus.last().unwrap(), start: vowel_end - glide, end: vowel_end, note: Some(last_note) });
            }
        }
        let mut t = vowel_end;
        for (p, l) in coda {
            segs.push(Seg { ph: p, start: t, end: t + l, note: Some(j) });
            t += l;
        }
        cursor = t;
        i = j + 1;
    }
    segs.push(Seg { ph: ph::SP, start: cursor, end: cursor + lead_in.max(0.1), note: None });
    to_frames(&segs, notes, t0, style, score.singer)
}

fn push_vowel(segs: &mut Vec<Seg>, v: Ph, start: f32, end: f32, notes: &[Note], first: usize, last: usize) {
    let mut s = start;
    for k in first..=last {
        let e = if k == last { end } else { notes[k + 1].start.clamp(s, end) };
        if e > s {
            segs.push(Seg { ph: v, start: s, end: e, note: Some(k) });
        }
        s = e;
    }
}

fn to_frames(segs: &[Seg], notes: &[Note], t0: f32, style: &PitchStyle, singer: usize) -> Frames {
    let fps = 100.0;
    let mut f = Frames { singer, ..Default::default() };
    let mut frame_end_prev = 0usize;
    for s in segs {
        let end = (((s.end - t0) * fps).round() as usize).max(frame_end_prev + 1);
        let n = end - frame_end_prev;
        // Merge consecutive vowel pieces of one phoneme (melisma) into one token.
        let merge = f.tokens.last() == Some(&s.ph) && ph::is_vowel(s.ph);
        if merge {
            *f.token_frames.last_mut().unwrap() += n;
        } else {
            f.tokens.push(s.ph);
            f.token_frames.push(n);
        }
        let tok = f.tokens.len() - 1;
        for k in 0..n {
            f.token_of_frame.push(tok);
            let (midi, vel) = s.note.map(|i| (notes[i].midi, notes[i].vel)).unwrap_or((0.0, 0.0));
            f.note.push(midi);
            f.vel.push(vel);
            f.onset.push(0.0);
            let _ = k;
        }
        frame_end_prev = end;
    }
    // Positions within tokens.
    let mut start = 0;
    for (tok, n) in f.token_frames.iter().enumerate() {
        for k in 0..*n {
            f.pos.push(if *n > 1 { k as f32 / (*n - 1) as f32 } else { 0.5 });
        }
        let _ = tok;
        start += n;
    }
    debug_assert_eq!(start, f.token_of_frame.len());
    // Attack flags at each note start.
    for n in notes {
        let fr = ((n.start - t0) * fps).round() as isize;
        if fr >= 0 && (fr as usize) < f.onset.len() && n.syllable.is_some() {
            f.onset[fr as usize] = 1.0;
        }
    }
    f.f0 = rule_f0(&f, style);
    f
}

/// A word of a recorded sung line: its phonemes and its span in seconds from
/// the segment start. Spans come from a lyric aligner and may run on over the
/// pause after the word; the recorded f0 says where the voice really is.
#[derive(Clone, Debug, PartialEq)]
pub struct SungWord {
    pub start: f32,
    pub end: f32,
    pub phones: Vec<Ph>,
}

/// A word's phonemes as syllables: one per vowel run (two vowels at most, a
/// diphthong); a lone consonant between vowels opens the next syllable, of
/// several the first closes the previous one.
pub fn syllables(p: &[Ph]) -> Vec<Syllable> {
    let mut nuclei: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < p.len() {
        if ph::is_vowel(p[i]) {
            let mut e = i + 1;
            while e < p.len() && e - i < 2 && ph::is_vowel(p[e]) {
                e += 1;
            }
            nuclei.push((i, e));
            i = e;
        } else {
            i += 1;
        }
    }
    if nuclei.is_empty() {
        return vec![Syllable { onset: p.to_vec(), nucleus: Vec::new(), coda: Vec::new() }];
    }
    let mut cuts = vec![0usize];
    for k in 1..nuclei.len() {
        let (gap0, gap1) = (nuclei[k - 1].1, nuclei[k].0);
        cuts.push(if gap1 - gap0 >= 2 { gap0 + 1 } else { gap0 });
    }
    cuts.push(p.len());
    nuclei
        .iter()
        .enumerate()
        .map(|(k, (a, b))| Syllable { onset: p[cuts[k]..*a].to_vec(), nucleus: p[*a..*b].to_vec(), coda: p[*b..cuts[k + 1]].to_vec() })
        .collect()
}

/// The score of a recorded line from its timed words and its f0 (10 ms
/// frames): each word's syllables share the word's sung span (from where the
/// voice starts after the onset consonants to where it stops, plus an
/// unvoiced coda), each on the nearest semitone of its median f0. Rendering
/// this score through `align` gives the frames the line is trained on, so
/// training and rendering share one alignment rule.
pub fn score_from_words(words: &[SungWord], f0: &[f32], singer: usize) -> SingScore {
    let nf = f0.len();
    let fr = |t: f32| ((t * 100.0).round().max(0.0) as usize).min(nf);
    let mut notes: Vec<Note> = Vec::new();
    let mut prev_end = 0.0f32;
    for w in words {
        let syl = syllables(&w.phones);
        let (fs, fe) = (fr(w.start.max(prev_end)), fr(w.end));
        if fe <= fs {
            continue;
        }
        let voiced: Vec<usize> = (fs..fe).filter(|q| f0[*q] > 0.0).collect();
        let (v0, v1) = match (voiced.first(), voiced.last()) {
            (Some(a), Some(b)) => (*a as f32 / 100.0, (*b + 1) as f32 / 100.0),
            _ => (fs as f32 / 100.0, fe as f32 / 100.0),
        };
        let onset: f32 = syl[0].onset.iter().map(|p| ph::rule_len(*p, false)).sum();
        let coda_unvoiced: f32 = syl.last().unwrap().coda.iter().filter(|p| !ph::is_voiced(**p)).map(|p| ph::rule_len(*p, true)).sum();
        let ws = fs as f32 / 100.0;
        let we = fe as f32 / 100.0;
        let start = v0.max(ws + onset).min(we - 0.05);
        let end = (v1 + coda_unvoiced).min(we).max(start + 0.05);
        let n = syl.len();
        let span = (end - start) / n as f32;
        for (k, s) in syl.into_iter().enumerate() {
            let (a, b) = (start + k as f32 * span, start + (k + 1) as f32 * span);
            let mut m: Vec<f32> = (fr(a)..fr(b)).filter(|q| f0[*q] > 0.0).map(|q| hz_to_midi(f0[q])).collect();
            m.sort_by(|x, y| x.partial_cmp(y).unwrap());
            let midi = m.get(m.len() / 2).map(|v| v.round()).or(notes.last().map(|n| n.midi)).unwrap_or(60.0);
            notes.push(Note { start: a, dur: b - a, midi, vel: 0.8, syllable: Some(s) });
        }
        prev_end = end;
    }
    // A gap too short for a rest is legato: the note before runs on to the next.
    for k in 1..notes.len() {
        let gap = notes[k].start - (notes[k - 1].start + notes[k - 1].dur);
        if gap > 0.0 && gap < 0.12 {
            notes[k - 1].dur += gap;
        }
    }
    SingScore { notes, singer }
}

/// The score's frames from the segment start (`align` with the first note's
/// start as the lead in), cut or padded with silence to `frames`.
pub fn align_segment(score: &SingScore, frames: usize) -> Frames {
    let lead_in = score.notes.first().map(|n| n.start).unwrap_or(0.0);
    let mut f = align(score, &rule_consonants, &PitchStyle::default(), lead_in);
    let n = f.len();
    if n > frames {
        let mut over = n - frames;
        while over > 0 {
            let last = f.token_frames.len() - 1;
            let take = over.min(f.token_frames[last]);
            f.token_frames[last] -= take;
            over -= take;
            if f.token_frames[last] == 0 {
                f.token_frames.pop();
                f.tokens.pop();
            }
        }
        for v in [&mut f.note, &mut f.onset, &mut f.vel, &mut f.pos, &mut f.f0] {
            v.truncate(frames);
        }
        f.token_of_frame.truncate(frames);
    } else if n < frames {
        let add = frames - n;
        if f.tokens.last() != Some(&ph::SP) {
            f.tokens.push(ph::SP);
            f.token_frames.push(0);
        }
        let tok = f.tokens.len() - 1;
        *f.token_frames.last_mut().unwrap() += add;
        f.token_of_frame.extend(std::iter::repeat(tok).take(add));
        f.note.extend(std::iter::repeat(0.0).take(add));
        f.vel.extend(std::iter::repeat(0.0).take(add));
        f.onset.extend(std::iter::repeat(0.0).take(add));
        f.f0.extend(std::iter::repeat(0.0).take(add));
        f.pos.extend(std::iter::repeat(0.5).take(add));
    }
    // Positions within the (possibly cut) last token.
    let last = f.token_frames.len() - 1;
    let (d, s0) = (f.token_frames[last], frames - f.token_frames[last]);
    for k in 0..d {
        f.pos[s0 + k] = if d > 1 { k as f32 / (d - 1) as f32 } else { 0.5 };
    }
    f
}

/// The rule pitch model, frame by frame. A note starts where the frames'
/// note changes (its onset consonants carry it) or at a vowel attack; the
/// voice glides there from where it is (legato) or from `scoop` below (after
/// a rest), timed to land by the vowel plus half the portamento, so the vowel
/// is sung on pitch. Vibrato starts `vibrato_delay` into the vowel and swings
/// evenly about the note; drift is a slow random walk of a few cents.
/// Unvoiced frames are 0.
pub fn rule_f0(f: &Frames, s: &PitchStyle) -> Vec<f32> {
    let dt = 0.01f32;
    let n = f.len();
    let mut rng = Rng::new(s.seed);
    let mut out = vec![0.0; n];
    let (mut x, mut from, mut target) = (0.0f32, 0.0f32, 0.0f32);
    let (mut glide_t, mut glide_len) = (0.0f32, 0.0f32);
    let (mut vowel_at, mut vib_phase, mut drift) = (None::<usize>, 0.0f32, 0.0f32);
    for t in 0..n {
        let note = f.note[t];
        if note <= 0.0 {
            target = 0.0;
            continue;
        }
        let fresh = t == 0 || f.note[t - 1] <= 0.0;
        if fresh || note != f.note[t - 1] {
            // A new note: where its vowel starts (the next attack on this note;
            // a melisma note has none, its vowel is already sounding).
            let mut v = t;
            while v + 1 < n && f.onset[v] <= 0.0 && f.note[v + 1] == note {
                v += 1;
            }
            if f.onset[v] <= 0.0 {
                v = t;
            }
            let lead = (v - t) as f32 * dt;
            from = if fresh || target <= 0.0 { note - s.scoop } else { x };
            target = note;
            let jump = (target - from).abs();
            glide_len = (s.portamento + 0.004 * jump).min(lead + 0.5 * s.portamento).max(0.015);
            glide_t = 0.0;
            vowel_at = Some(v);
            vib_phase = 0.0;
        }
        if f.onset[t] > 0.0 && vowel_at.map(|v| v < t).unwrap_or(true) {
            vowel_at = Some(t);
            vib_phase = 0.0;
        }
        // The glide: a smooth step from `from` to the note.
        let u = (glide_t / glide_len).clamp(0.0, 1.0);
        x = from + (target - from) * u * u * (3.0 - 2.0 * u);
        glide_t += dt;
        // Vibrato about the note, from `vibrato_delay` into the vowel.
        let age = vowel_at.map(|v| (t as f32 - v as f32) * dt).unwrap_or(0.0);
        let fade = ((age - s.vibrato_delay) / s.vibrato_fade.max(1e-3)).clamp(0.0, 1.0);
        if fade > 0.0 {
            vib_phase += 2.0 * std::f32::consts::PI * s.vibrato_rate * dt;
        }
        drift = drift * 0.98 + 0.199 * s.drift * rng.normal();
        let midi = x + fade * s.vibrato_depth * vib_phase.sin() + drift;
        let tok = f.tokens[f.token_of_frame[t]];
        out[t] = if ph::is_voiced(tok) { midi_to_hz(midi) } else { 0.0 };
    }
    out
}

/// Frames for a flat test tone line (used by tests and the CLI demo):
/// `(midi, beats)` pairs with a syllable string each, at `bpm`.
pub fn simple_line(notes: &[(f32, f32, &str)], bpm: f32) -> SingScore {
    let spb = 60.0 / bpm;
    let mut t = 0.3;
    let mut out = Vec::new();
    for (midi, beats, syl) in notes {
        let dur = beats * spb;
        let syllable = if *syl == "_" { None } else { Some(Syllable::from_phones(&ph::parse(syl))) };
        out.push(Note { start: t, dur, midi: *midi, vel: 0.8, syllable });
        t += dur;
    }
    SingScore { notes: out, singer: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_to_score_frames() {
        let p = |s: &str| ph::parse(s);
        let syl = syllables(&p("ænθəm"));
        assert_eq!(syl.len(), 2);
        assert_eq!(syl[0].coda, p("n"));
        assert_eq!(syl[1].onset, p("θ"));
        assert_eq!(syllables(&p("ɛvəɹi")).len(), 3);
        // Two words over 2 s; the second ends voiced at 1.5 s and its span runs on to 2.0 s.
        let mut f0 = vec![0.0f32; 200];
        for q in 22..90 {
            f0[q] = 261.63;
        }
        for q in 110..150 {
            f0[q] = 293.66;
        }
        let words = vec![SungWord { start: 0.15, end: 1.0, phones: p("mi") }, SungWord { start: 1.0, end: 2.0, phones: p("tu") }];
        let s = score_from_words(&words, &f0, 1600);
        assert_eq!(s.notes.len(), 2);
        assert_eq!(s.notes[0].midi, 60.0);
        assert_eq!(s.notes[1].midi, 62.0);
        assert!(s.notes[1].start + s.notes[1].dur < 1.6, "the pause after the word is not sung");
        let f = align_segment(&s, 200);
        assert_eq!(f.len(), 200);
        assert_eq!(f.token_frames.iter().sum::<usize>(), 200);
        assert_eq!(f.tokens.first(), Some(&ph::SP));
        assert_eq!(f.tokens.last(), Some(&ph::SP));
        let u = ph::id("u").unwrap();
        let first_u = f.token_of_frame.iter().position(|t| f.tokens[*t] == u).unwrap();
        assert!((first_u as i32 - 110).abs() <= 8, "u at frame {first_u}");
        let g = align_segment(&s, 120);
        assert_eq!(g.len(), 120);
        assert_eq!(g.token_frames.iter().sum::<usize>(), 120);
    }

    #[test]
    fn vowel_lands_on_the_beat_and_melisma_merges() {
        let s = simple_line(&[(60.0, 1.0, "sta"), (62.0, 1.0, "_"), (64.0, 1.0, "lat")], 120.0);
        let f = align(&s, &rule_consonants, &PitchStyle::default(), 0.3);
        // The first vowel starts at the first note's frame (0.3 s lead in -> frame 30).
        let a = ph::id("a").unwrap();
        let first_a = f.token_of_frame.iter().position(|t| f.tokens[*t] == a).unwrap();
        assert!((first_a as i32 - 30).abs() <= 1, "vowel at frame {first_a}");
        // "a" of "sta" spans both melisma notes as one token with two pitches.
        let tok = f.token_of_frame[first_a];
        assert!(f.token_frames[tok] >= 90, "melisma token {} frames", f.token_frames[tok]);
        assert_eq!(f.note[first_a + 10], 60.0);
        assert_eq!(f.note[first_a + 60], 62.0);
        // f0 is voiced on the vowel and on the note once the glide has landed
        // (before the vibrato starts).
        for q in first_a + 5..first_a + 20 {
            let m = hz_to_midi(f.f0[q]);
            assert!((m - 60.0).abs() < 0.12, "midi {m} at frame {q}");
        }
        // The second "a" lands on 64 by its vowel plus half the portamento.
        let second_a = f.token_of_frame.iter().rposition(|t| f.tokens[*t] == a).unwrap();
        let first_of_second = (0..second_a).rev().take_while(|q| f.tokens[f.token_of_frame[*q]] == a).last().unwrap();
        let m = hz_to_midi(f.f0[first_of_second + 5]);
        assert!((m - 64.0).abs() < 0.12, "midi {m}");
        // s is unvoiced.
        let s_id = ph::id("s").unwrap();
        let s_frame = f.token_of_frame.iter().position(|t| f.tokens[*t] == s_id).unwrap();
        assert_eq!(f.f0[s_frame], 0.0);
        assert_eq!(f.len(), f.token_frames.iter().sum::<usize>());
    }
}
