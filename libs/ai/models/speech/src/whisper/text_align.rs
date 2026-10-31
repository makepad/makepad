//! Forced alignment of KNOWN text: a voiceover script, a lyric sheet, a
//! corrected transcript, against the audio that speaks it.
//!
//! The text goes through Whisper's own encoder ([`WhisperTokenizer`]) in the
//! shape the model writes a segment (a leading space, words joined by single
//! spaces), then through [`WhisperState::force_align`]'s teacher-forced DTW.
//! Each token covers a byte range of that text, so every word takes its time
//! from exactly the tokens its bytes are in: the words are the input's own,
//! never re-read from token strings (which split multi-byte characters).
//!
//! [`WhisperTokenizer`]: crate::whisper::WhisperTokenizer

use crate::whisper::align::{TokenAlignment, AUDIO_FRAME_MS};
use crate::whisper::decode_loop::WhisperState;
use crate::whisper::model::WhisperModel;

/// Whisper's input rate: what `align_text` expects its samples at.
pub const ALIGN_SAMPLE_RATE: usize = 16_000;

/// The longest audio one call aligns: Whisper's 30 s window.
pub const MAX_ALIGN_SECS: f64 = 30.0;

/// One word of the given text and when it is spoken, in seconds from the
/// start of the samples.
#[derive(Clone, Debug, PartialEq)]
pub struct TimedWord {
    /// The whitespace-separated word exactly as in the input text.
    pub text: String,
    pub start: f64,
    pub end: f64,
    /// Mean attention mass of the word's tokens, 0..1: low means the model
    /// was not really listening to this stretch when it placed the word.
    pub score: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AlignTextError {
    /// The text has no words.
    EmptyText,
    /// No samples.
    EmptyAudio,
    /// Longer than [`MAX_ALIGN_SECS`]; align it in pieces.
    TooLong { secs: f64 },
    /// More tokens than the decoder's context holds with its prompt.
    TooManyTokens { tokens: usize, max: usize },
    /// The decoder pass or the DTW produced nothing usable.
    NoAlignment,
}

impl std::fmt::Display for AlignTextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyText => f.write_str("no words to align"),
            Self::EmptyAudio => f.write_str("no audio to align against"),
            Self::TooLong { secs } => write!(f, "{secs:.1} s of audio; one alignment takes at most {MAX_ALIGN_SECS} s"),
            Self::TooManyTokens { tokens, max } => write!(f, "{tokens} tokens; the decoder holds {max}"),
            Self::NoAlignment => f.write_str("the alignment pass produced nothing"),
        }
    }
}

/// `text` as Whisper writes a segment: `" w1 w2 …"`, and each word's byte
/// range in it.
fn spoken_form(text: &str) -> (String, Vec<(usize, usize)>) {
    let mut spoken = String::with_capacity(text.len() + 1);
    let mut words = Vec::new();
    for word in text.split_whitespace() {
        spoken.push(' ');
        let start = spoken.len();
        spoken.push_str(word);
        words.push((start, spoken.len()));
    }
    (spoken, words)
}

/// Each word timed from the tokens overlapping its bytes. `tokens[i]` is the
/// byte range of text token `i`, timed by aligned row `lead + i`
/// ([`crate::whisper::align::token_rows`]: the row that predicted it).
fn words_from_rows(
    spoken: &str,
    words: &[(usize, usize)],
    tokens: &[(usize, usize)],
    alignment: &TokenAlignment,
    lead: usize,
) -> Option<Vec<TimedWord>> {
    let frame_secs = AUDIO_FRAME_MS as f64 / 1000.0;
    let mut out = Vec::with_capacity(words.len());
    let mut first = 0;
    let mut last_end = 0.0f64;
    for (start, end) in words {
        while first < tokens.len() && tokens[first].1 <= *start {
            first += 1;
        }
        let mut last = first;
        while last + 1 < tokens.len() && tokens[last + 1].0 < *end {
            last += 1;
        }
        if first >= tokens.len() || tokens[first].0 >= *end {
            return None;
        }
        let (row_first, row_last) = (lead + first, lead + last);
        if row_last >= alignment.starts.len() {
            return None;
        }
        let begin = alignment.starts[row_first] as f64 * frame_secs;
        let finish = alignment.ends[row_last].max(alignment.starts[row_first]) as f64 * frame_secs;
        let score = alignment.scores[row_first..=row_last].iter().sum::<f32>() / (row_last - row_first + 1) as f32;
        // The DTW path is monotonic; the max only keeps a token shared by two
        // words (a split character) from stepping back.
        let begin = begin.max(last_end);
        let finish = finish.max(begin);
        last_end = finish;
        out.push(TimedWord { text: spoken[*start..*end].to_string(), start: begin, end: finish, score });
    }
    Some(out)
}

impl WhisperState {
    /// Align known `text` against `samples_16k` (mono, 16 kHz, at most
    /// [`MAX_ALIGN_SECS`]): one [`TimedWord`] per `text.split_whitespace()`
    /// word, in order, times in seconds from the first sample. `language` is
    /// the Whisper code (`"en"`), used by multilingual models.
    ///
    /// The forced pass's bracket rows absorb silence before the first word
    /// and after the last, so a take with lead-in and tail aligns as it is.
    pub fn align_text(
        &mut self,
        model: &WhisperModel,
        samples_16k: &[f32],
        text: &str,
        language: &str,
    ) -> Result<Vec<TimedWord>, AlignTextError> {
        let (spoken, words) = spoken_form(text);
        if words.is_empty() {
            return Err(AlignTextError::EmptyText);
        }
        if samples_16k.is_empty() {
            return Err(AlignTextError::EmptyAudio);
        }
        let secs = samples_16k.len() as f64 / ALIGN_SAMPLE_RATE as f64;
        if secs > MAX_ALIGN_SECS {
            return Err(AlignTextError::TooLong { secs });
        }
        let encoded = model.tokenizer().encode_with_offsets(&spoken);
        // The forced sequence is sot [lang transcribe] not <text> eot.
        let prompt = (if model.vocab.is_multilingual() { 4 } else { 2 }) + 1;
        let max = (model.hparams.n_text_ctx as usize).saturating_sub(prompt);
        if encoded.len() > max {
            return Err(AlignTextError::TooManyTokens { tokens: encoded.len(), max });
        }
        let ids: Vec<i32> = encoded.iter().map(|(id, _, _)| *id).collect();
        let ranges: Vec<(usize, usize)> = encoded.iter().map(|(_, start, end)| (*start, *end)).collect();
        let (alignment, lead) = self
            .force_align_rows(model, samples_16k, &ids, language)
            .ok_or(AlignTextError::NoAlignment)?;
        words_from_rows(&spoken, &words, &ranges, &alignment, lead).ok_or(AlignTextError::NoAlignment)
    }
}

/// [`WhisperState::align_text`] with a state of its own, for a one-off call.
/// A caller aligning many takes keeps one [`WhisperState`] and calls the
/// method.
pub fn align_text(
    model: &WhisperModel,
    samples_16k: &[f32],
    text: &str,
    language: &str,
) -> Result<Vec<TimedWord>, AlignTextError> {
    WhisperState::new(model).align_text(model, samples_16k, text, language)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spoken_form_normalizes_whitespace() {
        let (spoken, words) = spoken_form("  Type,\n and  it saves. ");
        assert_eq!(spoken, " Type, and it saves.");
        let texts: Vec<&str> = words.iter().map(|(a, b)| &spoken[*a..*b]).collect();
        assert_eq!(texts, vec!["Type,", "and", "it", "saves."]);
    }

    #[test]
    fn words_take_the_rows_of_their_bytes() {
        // " Hello wörld!" as tokens " Hello" | " w" | "ö"(2 bytes split 1+1) | "rld" | "!"
        let (spoken, words) = spoken_form("Hello wörld!");
        let tokens = [(0, 6), (6, 8), (8, 9), (9, 10), (10, 13), (13, 14)];
        // Rows: 0 = the lead absorber, 1 = `no_timestamps` (predicts token 0),
        // 1 + i predicts token i, then the rows of the last token's and
        // `eot`'s input positions.
        let alignment = TokenAlignment {
            starts: vec![0, 5, 20, 24, 26, 28, 40, 45, 50],
            ends: vec![5, 19, 23, 25, 27, 39, 44, 49, 55],
            scores: vec![0.1, 0.8, 0.6, 0.4, 0.4, 0.6, 0.9, 0.1, 0.1],
        };
        let timed = words_from_rows(&spoken, &words, &tokens, &alignment, 1).unwrap();
        assert_eq!(timed.len(), 2);
        assert_eq!(timed[0].text, "Hello");
        assert!((timed[0].start - 0.10).abs() < 1e-9 && (timed[0].end - 0.38).abs() < 1e-9);
        assert_eq!(timed[1].text, "wörld!");
        assert!((timed[1].start - 0.40).abs() < 1e-9 && (timed[1].end - 0.88).abs() < 1e-9);
        assert!((timed[1].score - 0.58).abs() < 1e-6);
    }
}
