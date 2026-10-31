//! Word timings read off Kokoro's own duration predictor.
//!
//! Kokoro decides how many frames each phoneme token lasts before it renders
//! a sample, and [`crate::g2p::tokens_with_sources`] says which input word
//! each token sounds. A word's span is therefore exact, not estimated: from
//! the first frame of its first phoneme to the last frame of its last one.
//! Pads, spaces and punctuation (the pauses between words) belong to no word.
//!
//! "Words" are the input's whitespace-separated words, the units a subtitle
//! or karaoke display shows: `"42,"` is one word although it is spoken as
//! "forty two", and a word Kokoro has no sound for (`"—"`, `"&"`) still gets
//! an entry, zero-length, where it falls between its neighbours.

use crate::tts::{PhoneTiming, WordTiming};

/// Kokoro's single-letter diphthongs (misaki's), written out in IPA.
fn ipa(symbol: char) -> Option<&'static str> {
    Some(match symbol {
        'A' => "eɪ",
        'I' => "aɪ",
        'O' => "oʊ",
        'W' => "aʊ",
        'Y' => "ɔɪ",
        'Q' => "əʊ",
        _ => return None,
    })
}

/// Frames the audio runs ahead of the predicted durations: the leading pad
/// token's duration overstates the silence before the first phoneme by
/// three frames, so every phoneme sounds that much earlier than the
/// cumulative frame count says. Upstream Kokoro's timestamps drop the same
/// three frames (`2 * (pred_dur[0] - 3)` at 80 steps a second). Measured on
/// our takes (vowels on voiced frames, fricatives on hiss): the raw phoneme
/// times sat 60-70 ms late at speeds 0.8, 1.0 and 1.25 (af_heart, bf_lily);
/// with this correction they sit 5-15 ms early, under half a video frame.
pub const LEADING_PAD_FRAMES: usize = 3;

/// The phonemes of one synthesized chunk, appended to `out`: every token
/// that sounds a word (`sources[i]` set) with its frames, from `offset`
/// seconds at `frame_secs` a frame, less [`LEADING_PAD_FRAMES`] so they sit
/// on the audio. Stress and length marks lengthen the phoneme before them;
/// spaces and punctuation are pauses, not phonemes.
pub fn chunk_phones(tokens: &[u16], sources: &[Option<usize>], frames: &[usize], offset: f64, frame_secs: f64, out: &mut Vec<PhoneTiming>) {
    let mut frame = 0usize;
    let lead = LEADING_PAD_FRAMES as f64 * frame_secs;
    for ((token, source), count) in tokens.iter().zip(sources).zip(frames) {
        let begin = (offset + frame as f64 * frame_secs - lead).max(offset);
        frame += count;
        let end = (offset + frame as f64 * frame_secs - lead).max(offset);
        let Some(symbol) = crate::g2p::symbol(*token) else { continue };
        if source.is_none() || symbol.is_whitespace() || symbol.is_ascii_punctuation() || matches!(symbol, '—' | '…' | '“' | '”') {
            continue;
        }
        if matches!(symbol, 'ˈ' | 'ˌ' | 'ː') {
            if let Some(last) = out.last_mut() {
                last.end = last.end.max(end);
            }
            continue;
        }
        let phone = ipa(symbol).map_or_else(|| symbol.to_string(), str::to_string);
        out.push(PhoneTiming { phone, start: begin, end });
    }
}

/// The whitespace-separated words of `text` with their byte ranges.
pub fn split_words(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (at, ch) in text.char_indices() {
        match (ch.is_whitespace(), start) {
            (true, Some(from)) => {
                out.push((from, at));
                start = None;
            }
            (false, None) => start = Some(at),
            _ => {}
        }
    }
    if let Some(from) = start {
        out.push((from, text.len()));
    }
    out
}

/// The word (index into `words`, from [`split_words`]) holding byte `at`.
fn word_at(words: &[(usize, usize)], at: usize) -> Option<usize> {
    let index = words.partition_point(|(_, end)| *end <= at);
    words.get(index).filter(|(start, _)| *start <= at).map(|_| index)
}

/// Accumulates spans over the chunks one text is synthesized in.
pub struct WordClock {
    words: Vec<(usize, usize)>,
    spans: Vec<Option<(f64, f64)>>,
}

impl WordClock {
    pub fn new(text: &str) -> Self {
        let words = split_words(text);
        let spans = vec![None; words.len()];
        Self { words, spans }
    }

    /// One synthesized chunk: `sources[i]` is the byte offset (relative to
    /// `chunk_start` in the whole text) of the word token `i` sounds,
    /// `frames[i]` its predicted frame count; the chunk starts at `offset`
    /// seconds and each frame lasts `frame_secs`.
    pub fn add_chunk(
        &mut self,
        chunk_start: usize,
        sources: &[Option<usize>],
        frames: &[usize],
        offset: f64,
        frame_secs: f64,
    ) {
        let mut frame = 0usize;
        for (source, count) in sources.iter().zip(frames) {
            let begin = offset + frame as f64 * frame_secs;
            frame += count;
            let end = offset + frame as f64 * frame_secs;
            let Some(word) = source.and_then(|at| word_at(&self.words, chunk_start + at)) else {
                continue;
            };
            let span = self.spans[word].get_or_insert((begin, end));
            span.0 = span.0.min(begin);
            span.1 = span.1.max(end);
        }
    }

    /// Every input word, in order, with its seconds. Words without a sound
    /// sit zero-length at the end of the word before them (or the start of
    /// the first word that has one).
    pub fn finish(self, text: &str) -> Vec<WordTiming> {
        let mut out: Vec<WordTiming> = Vec::with_capacity(self.words.len());
        let first_sounded = self.spans.iter().flatten().next().map(|(start, _)| *start).unwrap_or(0.0);
        let mut last_end = first_sounded;
        for ((start, end), span) in self.words.iter().zip(&self.spans) {
            // Monotonic by construction (tokens are in text order); the max
            // only guards against a word split across two chunks.
            let (word_start, word_end) = match span {
                Some((a, b)) => (a.max(last_end), b.max(a.max(last_end))),
                None => (last_end, last_end),
            };
            last_end = word_end;
            out.push(WordTiming { word: text[*start..*end].to_string(), start: word_start, end: word_end });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_split_on_whitespace_with_ranges() {
        let text = "  Type,\tand  it — saves.\n";
        let words: Vec<&str> = split_words(text).iter().map(|(a, b)| &text[*a..*b]).collect();
        assert_eq!(words, vec!["Type,", "and", "it", "—", "saves."]);
        assert_eq!(word_at(&split_words(text), 0), None);
        assert_eq!(word_at(&split_words(text), 2), Some(0));
        assert_eq!(word_at(&split_words(text), 7), None);
    }

    #[test]
    fn synthetic_durations_map_to_words() {
        // "Hi there, 42." sounded as: pad, h, i, space, ð, ɛ, r, ',', space,
        // f, o, r, t, i, space, t, u, '.', pad — "42" spelled as two words
        // that both point at its digits.
        let text = "Hi there, 42.";
        let (hi, there, digits) = (Some(0), Some(3), Some(10));
        let sources = [
            None, hi, hi, None, there, there, there, None, None, digits, digits, digits, digits,
            digits, None, digits, digits, None, None,
        ];
        let frames = [3, 2, 2, 1, 2, 3, 2, 4, 1, 2, 2, 2, 2, 2, 1, 2, 3, 5, 3];
        let mut clock = WordClock::new(text);
        clock.add_chunk(0, &sources, &frames, 0.0, 0.025);
        let timings = clock.finish(text);
        let words: Vec<&str> = timings.iter().map(|t| t.word.as_str()).collect();
        assert_eq!(words, vec!["Hi", "there,", "42."]);
        let secs = |f: usize| f as f64 * 0.025;
        assert_eq!((timings[0].start, timings[0].end), (secs(3), secs(7)));
        assert_eq!((timings[1].start, timings[1].end), (secs(8), secs(15)));
        // Both spelled words, and the space between them, are "42.".
        assert_eq!((timings[2].start, timings[2].end), (secs(20), secs(36)));
        let total: usize = frames.iter().sum();
        assert!(timings.last().unwrap().end <= secs(total));
    }

    #[test]
    fn chunks_offset_and_silent_words_stay_in_order() {
        // Two chunks: "Go now." and " & stop." — "&" has no sound.
        let text = "Go now. & stop.";
        let mut clock = WordClock::new(text);
        clock.add_chunk(0, &[None, Some(0), Some(0), None, Some(3), Some(3), None, None], &[2, 2, 2, 1, 2, 2, 4, 2], 0.0, 0.1);
        // The second chunk's text starts at byte 7 (" & stop."), "stop" at 3.
        clock.add_chunk(7, &[None, Some(3), Some(3), Some(3), None, None], &[2, 1, 1, 1, 3, 2], 1.7, 0.1);
        let timings = clock.finish(text);
        assert_eq!(timings.len(), 4);
        assert_eq!(timings[2].word, "&");
        assert_eq!(timings[2].start, timings[2].end);
        assert_eq!(timings[2].start, timings[1].end);
        assert!((timings[3].start - 1.9).abs() < 1e-9);
        assert!((timings[3].end - 2.2).abs() < 1e-9);
        for pair in timings.windows(2) {
            assert!(pair[0].start <= pair[0].end && pair[0].end <= pair[1].start, "{timings:?}");
        }
    }

    #[test]
    fn phonemes_take_their_frames_and_marks_fold_in() {
        let token = |c: char| crate::g2p::vocab::token(c).unwrap();
        // pad h ə l ˈ O space w pad: "hello w…", the pads and space unspoken.
        let tokens = [0, token('h'), token('ə'), token('l'), token('ˈ'), token('O'), token(' '), token('w'), 0];
        let sources = [None, Some(0), Some(0), Some(0), Some(0), Some(0), None, Some(6), None];
        let frames = [2, 1, 1, 2, 1, 3, 2, 1, 2];
        let mut out = Vec::new();
        chunk_phones(&tokens, &sources, &frames, 1.0, 0.025, &mut out);
        let phones: Vec<&str> = out.iter().map(|p| p.phone.as_str()).collect();
        assert_eq!(phones, ["h", "ə", "l", "oʊ", "w"]);
        // Frames counted from the chunk, less the leading pad's 3 surplus
        // frames (never before the chunk's start).
        let at = |frames: usize| 1.0 + (frames as f64 - LEADING_PAD_FRAMES as f64).max(0.0) * 0.025;
        assert!((out[0].start - at(2)).abs() < 1e-9 && (out[0].end - at(3)).abs() < 1e-9, "{:?}", out[0]);
        // `ˈ` lengthened the `l` before it.
        assert!((out[2].end - at(7)).abs() < 1e-9, "{:?}", out[2]);
        assert!((out[4].start - at(12)).abs() < 1e-9, "the space is a pause");
    }
}
