//! Karaoke state for any text (PDOOM-PARITY F6): per char, syllable or word,
//! whether it is unsung, being sung, sung or cooled, from sung-word
//! timings, with an anticipation ramp before a unit starts.
//!
//! The no-run-ahead rule: a unit is never sung before its own start time.
//! Within a word, chars (or syllables) become sung at even steps of the
//! word's span, or at the syllables' own times when they are known;
//! anticipation only raises `anticipation` before the start, it never
//! changes the state.

/// A sung word: its chars in the text and its times.
#[derive(Clone, Debug, PartialEq)]
pub struct SungWord {
    /// Char range in the text, `start..end`.
    pub chars: std::ops::Range<usize>,
    pub start: f32,
    pub end: f32,
    /// Syllables as (char range, start, end), when the aligner knows them.
    pub syllables: Vec<(std::ops::Range<usize>, f32, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum KaraokeBy {
    #[default]
    Char,
    Syllable,
    Word,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    Ltr,
    Rtl,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KaraokeStyle {
    pub by: KaraokeBy,
    /// Seconds of lead-in during which `anticipation` ramps 0 → 1.
    pub anticipate: f32,
    /// Seconds after a unit is sung until it is cooled.
    pub cool: f32,
    pub dir: Direction,
}

impl Default for KaraokeStyle {
    fn default() -> Self {
        Self { by: KaraokeBy::Char, anticipate: 0.4, cool: 1.5, dir: Direction::Ltr }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Unsung,
    Sung,
    Cooled,
}

/// One char's karaoke state at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharState {
    pub state: State,
    /// 0 → 1 over the lead-in before the unit starts (1 once started).
    pub anticipation: f32,
    /// Seconds since the unit was sung (0 while unsung).
    pub age: f32,
    /// Position of the sweep across the unit, 0 → 1 while it is sung.
    pub sweep: f32,
    /// Word ordinal, or `None` for chars outside every word.
    pub word: Option<usize>,
}

/// Linear RGBA of a state: unsung, sung and cooled colours, the sung one
/// fading to the cooled one over the cool time.
pub fn state_color(s: &CharState, colors: &[[f32; 4]; 3], style: &KaraokeStyle) -> [f32; 4] {
    let mix = |a: [f32; 4], b: [f32; 4], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t];
    match s.state {
        State::Unsung => colors[0],
        State::Sung => mix(colors[1], colors[2], if style.cool > 0.0 { (s.age / style.cool).clamp(0.0, 1.0) } else { 0.0 }),
        State::Cooled => colors[2],
    }
}

/// Each char's state at time `t` for a text of `chars` chars.
pub fn states(words: &[SungWord], chars: usize, t: f32, style: &KaraokeStyle) -> Vec<CharState> {
    let mut out = vec![CharState { state: State::Unsung, anticipation: 0.0, age: 0.0, sweep: 0.0, word: None }; chars];
    for (w, word) in words.iter().enumerate() {
        let range = word.chars.start.min(chars)..word.chars.end.min(chars);
        let n = range.len();
        if n == 0 {
            continue;
        }
        // (char offset within the word, unit start, unit end) per unit.
        let mut units: Vec<(std::ops::Range<usize>, f32, f32)> = match style.by {
            KaraokeBy::Word => vec![(0..n, word.start, word.end)],
            KaraokeBy::Syllable if !word.syllables.is_empty() => {
                word.syllables.iter().map(|(r, s, e)| (r.start.saturating_sub(word.chars.start)..r.end.saturating_sub(word.chars.start).min(n), s.max(word.start), *e)).collect()
            }
            _ => {
                let span = (word.end - word.start).max(0.0);
                (0..n).map(|k| (k..k + 1, word.start + span * k as f32 / n as f32, word.start + span * (k + 1) as f32 / n as f32)).collect()
            }
        };
        if style.dir == Direction::Rtl && style.by != KaraokeBy::Word {
            // The sweep runs from the word's last char to its first.
            let times: Vec<(f32, f32)> = units.iter().map(|u| (u.1, u.2)).collect();
            let len = units.len();
            for (k, u) in units.iter_mut().enumerate() {
                (u.1, u.2) = times[len - 1 - k];
            }
        }
        for (r, s, e) in units {
            for k in r.clone() {
                let c = &mut out[range.start + k];
                c.word = Some(w);
                c.anticipation = if style.anticipate > 0.0 { ((t - (s - style.anticipate)) / style.anticipate).clamp(0.0, 1.0) } else if t >= s { 1.0 } else { 0.0 };
                if t < s {
                    continue;
                }
                let age = t - e.max(s);
                let len = r.len().max(1) as f32;
                let into = if e > s { ((t - s) / (e - s)).clamp(0.0, 1.0) } else { 1.0 };
                // Within a multi-char unit the sweep crosses it char by char.
                let local = (k - r.start) as f32;
                c.sweep = (into * len - local).clamp(0.0, 1.0);
                c.age = age.max(0.0);
                c.state = if age > style.cool && style.cool >= 0.0 { State::Cooled } else { State::Sung };
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words() -> Vec<SungWord> {
        vec![SungWord { chars: 0..3, start: 1.0, end: 1.6, syllables: vec![] }, SungWord { chars: 4..10, start: 2.0, end: 3.0, syllables: vec![(4..6, 2.0, 2.3), (6..10, 2.5, 3.0)] }]
    }

    #[test]
    fn chars_are_sung_in_order_and_never_early() {
        let st = KaraokeStyle::default();
        let s = states(&words(), 10, 1.19, &st);
        assert_eq!((s[0].state, s[1].state, s[2].state), (State::Sung, State::Unsung, State::Unsung));
        assert!(s[1].anticipation > 0.9 && s[4].anticipation == 0.0);
        assert_eq!(s[3].word, None, "the space belongs to no word");
        let all: Vec<State> = states(&words(), 10, 0.99, &st).iter().map(|c| c.state).collect();
        assert!(all.iter().all(|s| *s == State::Unsung), "nothing runs ahead of the first word");
        let late = states(&words(), 10, 4.4, &st);
        assert_eq!(late[0].state, State::Cooled);
        assert_eq!(late[9].state, State::Sung);
    }

    #[test]
    fn syllables_words_and_right_to_left() {
        let by_syl = KaraokeStyle { by: KaraokeBy::Syllable, ..KaraokeStyle::default() };
        let s = states(&words(), 10, 2.4, &by_syl);
        assert_eq!((s[4].state, s[5].state, s[6].state), (State::Sung, State::Sung, State::Unsung), "the second syllable starts at 2.5");
        let by_word = KaraokeStyle { by: KaraokeBy::Word, ..KaraokeStyle::default() };
        let w = states(&words(), 10, 2.1, &by_word);
        assert!(w[4..10].iter().all(|c| c.state == State::Sung));
        assert!(w[4].sweep > 0.0 && w[9].sweep == 0.0, "the sweep crosses a word left to right");
        let rtl = KaraokeStyle { dir: Direction::Rtl, ..KaraokeStyle::default() };
        let r = states(&words(), 10, 1.19, &rtl);
        assert_eq!((r[0].state, r[2].state), (State::Unsung, State::Sung));
        let c = state_color(&r[2], &[[0.0; 4], [1.0; 4], [0.5, 0.5, 0.5, 1.0]], &rtl);
        assert_eq!(c, [1.0; 4]);
    }
}
