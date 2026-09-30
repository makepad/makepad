//! B2, the records: one `Glyph` record per element (a glyph, a voxel cell,
//! times the copies), the kernel's input. The rest pose, ordinals and
//! counts are set when a glyph set arrives; the change times carry over
//! from the previous set (a glyph whose char changed at its ordinal gets
//! the arrival time); the karaoke words are refreshed every frame.

use crate::shapes::GlyphSet;
use makepad_script_compute::kernel::{FieldTy, Layout, LayoutField};
use makepad_typography::karaoke::{states, KaraokeStyle, State, SungWord};

/// Words per `Glyph` record.
pub const GLYPH_WORDS: usize = 40;

const F_CHANGED: usize = 24;
const F_SUNG: usize = 25;
const F_AGE: usize = 26;
const F_NEAR: usize = 27;
const F_PROGRESS: usize = 28;
const F_DYING: usize = 39;

/// The `Glyph` layout the animator reads (`let g = glyphs[i]`).
pub fn glyph_layout() -> Layout {
    let f = |name: &str, ty, offset| LayoutField { name: name.into(), ty, offset };
    let mut fields = vec![f("rest", FieldTy::Vec3, 0), f("size", FieldTy::Vec3, 3), f("word_c", FieldTy::Vec3, 6), f("line_c", FieldTy::Vec3, 9)];
    let scalars = [
        "index", "count", "t", "word", "words", "line", "lines", "copy", "copies", "char", "shape", "seed", "changed_at", "sung", "age", "near", "progress", "ink", "layer", "glyph",
    ];
    for (k, name) in scalars.iter().enumerate() {
        fields.push(f(name, FieldTy::F32, 12 + k as u32));
    }
    fields.push(f("from", FieldTy::Vec3, 32));
    // base scale (a cloud word's size), word weight 0..1, the word's ink
    // width and height (at its scale).
    // `dying`: 1 on a record of the previous text's surplus (see
    // `Records::set`), 0 on the text's own.
    for (k, name) in ["k", "weight", "word_w", "word_h", "dying"].iter().enumerate() {
        fields.push(f(name, FieldTy::F32, 35 + k as u32));
    }
    Layout { name: "Glyph".into(), stride: GLYPH_WORDS as u32, fields }
}

/// What is being sung.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Karaoke {
    /// Nothing: every glyph unsung.
    #[default]
    None,
    /// The line's sung fraction (0..1): glyph k of n is sung from
    /// `progress * n > k`, the wipe crossing it.
    Progress(f32),
    /// Sung words with their times (char ranges of the text) at a time.
    Words { words: Vec<SungWord>, time: f32, style: KaraokeStyle },
}

/// The records of a set, kept across frames.
#[derive(Default)]
pub struct Records {
    pub data: Vec<f32>,
    /// Char per element (to carry change times over).
    chars: Vec<char>,
    char_index: Vec<usize>,
    text_chars: usize,
    /// Records in all: the text's own, then the dying ones.
    pub count: usize,
    /// The dying records at the end of `data`.
    pub dying: usize,
}

fn hash01(i: u32, k: u32) -> f32 {
    let mut h = i.wrapping_mul(0x9E37_79B9) ^ k.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    (h >> 8) as f32 / 16_777_216.0
}

impl Records {
    /// Fill the records for `set` repeated `copies` times. `now` is the
    /// host time the set arrives; glyphs that differ from the previous
    /// set at their ordinal are stamped with it.
    ///
    /// With `keep_dying`, the previous set's elements past the new set's
    /// count stay as DYING records after the text's own: their old record
    /// (rest, ordinals, `from` = where they were), `changed_at` = `now`,
    /// `dying` = 1, and the new set's shape for the same char (dropped
    /// when the new set has none: a letter shape that is gone).
    pub fn set(&mut self, set: &GlyphSet, copies: usize, now: f32, text_chars: usize, keep_dying: bool) {
        let copies = copies.max(1);
        let n = set.elements.len();
        let mut dying_rows: Vec<f32> = Vec::new();
        if keep_dying && !self.chars.is_empty() {
            let old_n = self.chars.len();
            let alive = self.count - self.dying;
            let old_copies = (alive / old_n.max(1)).max(1);
            for c in 0..old_copies.min(copies) {
                for i in n..old_n {
                    let Some(shape) = set.elements.iter().find(|e| e.ch == self.chars[i]).map(|e| e.shape) else { continue };
                    let row = &self.data[(c * old_n + i) * GLYPH_WORDS..(c * old_n + i + 1) * GLYPH_WORDS];
                    let mut r = row.to_vec();
                    r[32] = r[0];
                    r[33] = r[1];
                    r[34] = r[2];
                    r[12 + 10] = shape as f32;
                    r[F_CHANGED] = now;
                    r[F_DYING] = 1.0;
                    dying_rows.extend_from_slice(&r);
                }
            }
        }
        let old_changed: Vec<f32> = (0..self.chars.len().min(n)).map(|i| self.data[i * GLYPH_WORDS + F_CHANGED]).collect();
        // Where each element was in the previous set (the same ordinal),
        // for morphs from the old text into the new; its own rest if new.
        let old_rest: Vec<[f32; 3]> = (0..self.chars.len()).map(|i| [self.data[i * GLYPH_WORDS], self.data[i * GLYPH_WORDS + 1], self.data[i * GLYPH_WORDS + 2]]).collect();
        let first = self.chars.is_empty();
        let old_chars = std::mem::take(&mut self.chars);
        // Each word's ink extent (x0, y0, x1, y1).
        let mut word_ext = vec![[f32::MAX, f32::MAX, f32::MIN, f32::MIN]; set.words.max(1)];
        for e in &set.elements {
            if let Some(b) = word_ext.get_mut(e.word) {
                let (hx, hy) = (e.size[0] * 0.5 * e.scale, e.size[1] * 0.5 * e.scale);
                b[0] = b[0].min(e.pivot[0] - hx);
                b[1] = b[1].min(e.pivot[1] - hy);
                b[2] = b[2].max(e.pivot[0] + hx);
                b[3] = b[3].max(e.pivot[1] + hy);
            }
        }
        self.count = n * copies;
        self.data.clear();
        self.data.resize(self.count * GLYPH_WORDS, 0.0);
        self.char_index.clear();
        self.text_chars = text_chars;
        let letters = set.letters.max(1);
        for c in 0..copies {
            for (i, e) in set.elements.iter().enumerate() {
                let r = &mut self.data[(c * n + i) * GLYPH_WORDS..(c * n + i + 1) * GLYPH_WORDS];
                let wc = set.word_centers.get(e.word).copied().unwrap_or(e.pivot);
                let lc = set.line_centers.get(e.line).copied().unwrap_or(e.pivot);
                r[0..3].copy_from_slice(&e.pivot);
                r[3..6].copy_from_slice(&e.size);
                r[6..9].copy_from_slice(&wc);
                r[9..12].copy_from_slice(&lc);
                let t = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
                let changed = if first {
                    -1e9
                } else if old_chars.get(i) == Some(&e.ch) {
                    old_changed.get(i).copied().unwrap_or(-1e9)
                } else {
                    now
                };
                let vals = [
                    i as f32,
                    n as f32,
                    t,
                    e.word as f32,
                    set.words as f32,
                    e.line as f32,
                    set.lines as f32,
                    c as f32,
                    copies as f32,
                    e.ch as u32 as f32,
                    e.shape as f32,
                    hash01(i as u32, 17),
                    changed,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    e.ink,
                    e.layer as f32,
                    e.glyph as f32,
                ];
                r[12..32].copy_from_slice(&vals);
                r[32..35].copy_from_slice(old_rest.get(i).unwrap_or(&e.pivot));
                let ext = word_ext.get(e.word).copied().unwrap_or([0.0; 4]);
                r[35] = e.scale;
                r[36] = e.weight;
                r[37] = (ext[2] - ext[0]).max(0.0);
                r[38] = (ext[3] - ext[1]).max(0.0);
            }
        }
        self.chars = set.elements.iter().map(|e| e.ch).collect();
        self.char_index = set.elements.iter().map(|e| e.char_index).collect();
        let _ = letters;
        self.dying = dying_rows.len() / GLYPH_WORDS;
        self.count += self.dying;
        self.data.extend_from_slice(&dying_rows);
    }

    /// Drop the dying records (their time is up).
    pub fn retire_dying(&mut self) {
        if self.dying > 0 {
            self.count -= self.dying;
            self.data.truncate(self.count * GLYPH_WORDS);
            self.dying = 0;
        }
    }

    /// The karaoke fields for this frame.
    pub fn sing(&mut self, karaoke: &Karaoke) {
        let n = self.chars.len();
        if n == 0 {
            return;
        }
        let copies = (self.count - self.dying) / n;
        let mut per: Vec<(f32, f32, f32)> = vec![(0.0, 0.0, 0.0); n];
        let mut progress = 0.0;
        match karaoke {
            Karaoke::None => {}
            Karaoke::Progress(p) => {
                let p = p.clamp(0.0, 1.0);
                progress = p;
                // The letter ordinal of each element (cells share theirs).
                let letters = self.letter_count();
                let at = p * letters as f32;
                for (i, v) in per.iter_mut().enumerate() {
                    let k = self.letter_of(i) as f32;
                    let sung = (at - k).clamp(0.0, 1.0);
                    let d = k + 0.5 - at;
                    v.0 = sung;
                    v.2 = (-d * d * 0.35).exp();
                }
            }
            Karaoke::Words { words, time, style } => {
                let st = states(words, self.text_chars, *time, style);
                let mut sung_total = 0.0;
                for (i, v) in per.iter_mut().enumerate() {
                    let Some(s) = self.char_index.get(i).and_then(|&c| st.get(c)) else { continue };
                    v.0 = match s.state {
                        State::Unsung => 0.0,
                        _ => s.sweep.max(if s.state == State::Cooled { 1.0 } else { 0.0 }),
                    };
                    v.1 = s.age;
                    v.2 = s.anticipation;
                    sung_total += v.0;
                }
                progress = sung_total / n as f32;
            }
        }
        for c in 0..copies {
            for (i, v) in per.iter().enumerate() {
                let r = &mut self.data[(c * n + i) * GLYPH_WORDS..];
                r[F_SUNG] = v.0;
                r[F_AGE] = v.1;
                r[F_NEAR] = v.2;
                r[F_PROGRESS] = progress;
            }
        }
    }

    fn letter_of(&self, i: usize) -> usize {
        let g = self.data[i * GLYPH_WORDS + 31];
        if g >= 0.0 { g as usize } else { 0 }
    }

    fn letter_count(&self) -> usize {
        (0..self.chars.len()).map(|i| self.letter_of(i) + 1).max().unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::{build, ShapeSpec};

    #[test]
    fn records_carry_ordinals_changes_and_karaoke() {
        let a = build(&ShapeSpec { text: "10:58".into(), depth: 0.0, ..ShapeSpec::default() }).unwrap();
        let b = build(&ShapeSpec { text: "10:59".into(), depth: 0.0, ..ShapeSpec::default() }).unwrap();
        let mut r = Records::default();
        r.set(&a, 2, 1.0, 5, false);
        assert_eq!(r.count, 10);
        let at = |r: &Records, i: usize, f: usize| r.data[i * GLYPH_WORDS + f];
        assert_eq!((at(&r, 6, 12), at(&r, 6, 19), at(&r, 6, 20)), (1.0, 1.0, 2.0), "copy 1 of glyph 1");
        assert_eq!(at(&r, 4, 14), 1.0, "t of the last glyph");
        r.set(&b, 2, 5.0, 5, false);
        assert_eq!((at(&r, 3, F_CHANGED), at(&r, 4, F_CHANGED)), (-1e9, 5.0), "only the last digit changed");
        r.sing(&Karaoke::Progress(0.5));
        assert_eq!((at(&r, 0, F_SUNG), at(&r, 2, F_SUNG), at(&r, 4, F_SUNG)), (1.0, 0.5, 0.0), "the wipe is in the middle glyph");
        r.sing(&Karaoke::Words { words: vec![SungWord { chars: 0..5, start: 0.0, end: 1.0, syllables: vec![] }], time: 0.5, style: KaraokeStyle::default() });
        assert!(at(&r, 0, F_SUNG) == 1.0 && at(&r, 4, F_SUNG) == 0.0);
        let l = glyph_layout();
        assert_eq!(l.fields.iter().find(|f| f.name == "word_h").unwrap().offset, 38);
        assert!(at(&r, 0, 37) > 0.0 && at(&r, 0, 35) == 1.0, "word width and base scale");
        assert_eq!(at(&r, 4, 32), a.elements[4].pivot[0], "from: where glyph 4 was before");
    }

    /// A shorter text keeps the old surplus as dying records after its own
    /// (from where they were, stamped with the change) until retired; a
    /// letter shape the new text lacks is not kept.
    #[test]
    fn a_shorter_text_keeps_its_surplus_as_dying_records() {
        let a = build(&ShapeSpec { text: "1111".into(), depth: 0.0, ..ShapeSpec::default() }).unwrap();
        let b = build(&ShapeSpec { text: "11".into(), depth: 0.0, ..ShapeSpec::default() }).unwrap();
        let c = build(&ShapeSpec { text: "2".into(), depth: 0.0, ..ShapeSpec::default() }).unwrap();
        let mut r = Records::default();
        r.set(&a, 1, 0.0, 4, true);
        assert_eq!((r.count, r.dying), (4, 0));
        r.set(&b, 1, 3.0, 2, true);
        assert_eq!((r.count, r.dying), (4, 2), "two ones die");
        let at = |r: &Records, i: usize, f: usize| r.data[i * GLYPH_WORDS + f];
        assert_eq!((at(&r, 2, F_DYING), at(&r, 2, F_CHANGED)), (1.0, 3.0));
        assert_eq!(at(&r, 1, F_DYING), 0.0);
        assert_eq!(at(&r, 3, 32), a.elements[3].pivot[0], "from where it was");
        r.sing(&Karaoke::Progress(1.0));
        r.retire_dying();
        assert_eq!((r.count, r.dying, r.data.len()), (2, 0, 2 * GLYPH_WORDS));
        r.set(&c, 1, 5.0, 1, true);
        assert_eq!(r.dying, 0, "a `1` has no shape in `2`: not kept");
        let l = glyph_layout();
        assert_eq!(l.fields.iter().find(|f| f.name == "dying").unwrap().offset, 39);
    }
}
