//! Speak a sentence, transcribe it back with Whisper, and score the difference.
//!
//! "It made noise" is not a test. This closes the loop: if the synthesizer
//! mumbles, slurs a plural, or reads `ɡˈæmɛs` for "games", Whisper hears it and
//! the word error rate goes up. It is also the scoreboard for comparing
//! backends — run it once on the system voice, once on Kokoro.
//!
//! Run from the makepad root so the Whisper model resolves:
//!
//!     cargo run --release --manifest-path libs/tts/Cargo.toml --example roundtrip

use makepad_tts::{SpeechAudio, Speaker};
use makepad_voice::{VoiceTranscribeParams, VoiceTranscriber};

const WHISPER_SAMPLE_RATE: u32 = 16_000;

const SENTENCES: &[&str] = &[
    "Hi! I make games with you.",
    "I made the player jump higher.",
    "Escape the Gummer, a squishy purple blob.",
    "You scored forty two points.",
    "The little guy can run and jump on the platforms.",
    "I gave the ghost bigger eyes and made it chase you faster.",
];

fn main() {
    let mut speaker = Speaker::from_makepad_env();
    println!("tts backend : {:?}", speaker.kind());

    let mut transcriber = VoiceTranscriber::from_makepad_env();
    println!("asr backend : {:?}", transcriber.kind());
    let params = VoiceTranscribeParams::default();
    if let Err(err) = transcriber.preload(&params) {
        println!("asr preload failed: {err:?}");
        return;
    }
    println!();

    let (mut total_words, mut total_errors) = (0usize, 0usize);
    let started = std::time::Instant::now();

    for sentence in SENTENCES {
        let audio = match speaker.synthesize(sentence) {
            Ok(audio) if !audio.is_empty() => audio,
            other => {
                println!("{sentence:?}\n  synthesis produced nothing ({other:?})\n");
                continue;
            }
        };

        let heard = transcribe(&mut transcriber, &params, &audio);
        let (errors, words) = word_error(sentence, &heard);
        total_errors += errors;
        total_words += words;

        let verdict = if errors == 0 { "exact" } else { "differs" };
        println!("said  : {sentence}");
        println!("heard : {heard}");
        println!(
            "  {verdict}: {errors}/{words} words wrong ({:.0}% WER), {:.1}s audio",
            100.0 * errors as f32 / words.max(1) as f32,
            audio.duration_secs()
        );
        println!();
    }

    println!(
        "overall WER: {:.1}%  ({total_errors}/{total_words} words) in {:.1}s",
        100.0 * total_errors as f32 / total_words.max(1) as f32,
        started.elapsed().as_secs_f32()
    );
}

fn transcribe(
    transcriber: &mut VoiceTranscriber,
    params: &VoiceTranscribeParams,
    audio: &SpeechAudio,
) -> String {
    let samples = audio.resampled(WHISPER_SAMPLE_RATE);
    match transcriber.transcribe(&samples, params) {
        Ok(segments) => segments
            .iter()
            .map(|segment| segment.text.trim())
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string(),
        Err(err) => format!("<transcription failed: {err:?}>"),
    }
}

/// Lowercase, drop punctuation, split on whitespace, and spell out digits.
///
/// Whisper writes "42" where the sentence said "forty two". Without this the
/// scoreboard would blame the synthesizer for the transcriber's formatting.
fn normalize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|word| !word.is_empty())
        .flat_map(|word| match word.parse::<u64>() {
            Ok(number) => makepad_tts::g2p::spell_number(number)
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>(),
            Err(_) => vec![word.to_string()],
        })
        .collect()
}

/// Levenshtein distance over words: substitutions, insertions, deletions.
fn word_error(said: &str, heard: &str) -> (usize, usize) {
    let reference = normalize(said);
    let hypothesis = normalize(heard);

    let mut previous: Vec<usize> = (0..=hypothesis.len()).collect();
    let mut current = vec![0usize; hypothesis.len() + 1];

    for (i, want) in reference.iter().enumerate() {
        current[0] = i + 1;
        for (j, got) in hypothesis.iter().enumerate() {
            let substitution = previous[j] + usize::from(want != got);
            let insertion = current[j] + 1;
            let deletion = previous[j + 1] + 1;
            current[j + 1] = substitution.min(insertion).min(deletion);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    (previous[hypothesis.len()], reference.len())
}
