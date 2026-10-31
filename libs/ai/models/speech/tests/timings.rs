//! Model-dependent checks of the word-timing paths (EDITS.md §9 P8, P9).
//! Ignored by default: they need the weights on this machine.
//!
//!     cargo test --release -p makepad-ai-speech --test timings -- --ignored --nocapture
//!
//! Weights, first found wins:
//! - Kokoro: `$MAKEPAD_TTS_MODEL`, else `~/.makepad/weights/tts/kokoro-v1_0.mktts`;
//!   voice `$MAKEPAD_TTS_VOICE`, else the first of `af_heart`, `bm_daniel`,
//!   `bf_lily` `.mkvoice` next to the model.
//! - Whisper: `$MAKEPAD_VOICE_MODEL`, else `local/models/ggml-large-v3-turbo.bin`
//!   in the Makepad checkout, else `~/.makepad/weights/stt/ggml-large-v3-turbo.bin`.
//!
//! Tokenizer fixtures come from two independent sources, neither of them this
//! encoder: the vocab file's own entries (a string that IS one token in the
//! file must encode to exactly that id; the ids below were read out of
//! `ggml-large-v3-turbo.bin`'s vocab section, and agree with OpenAI's
//! `multilingual.tiktoken` ranks: "," 11, "." 13, " the" 264), and the
//! model's own writing (Whisper transcribes a Kokoro take; the token ids it
//! emitted must be what the encoder makes of the text it emitted).

use makepad_ai_speech::kokoro::KokoroSpeaker;
use makepad_ai_speech::whisper::{WhisperModel, WhisperParams, WhisperState};
use makepad_ai_speech::{SpeechAudio, WordTiming};
use std::path::PathBuf;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn env_file(name: &str) -> Option<PathBuf> {
    std::env::var(name).ok().map(PathBuf::from).filter(|p| p.is_file())
}

fn kokoro_paths() -> (PathBuf, PathBuf) {
    let model = env_file("MAKEPAD_TTS_MODEL")
        .unwrap_or_else(|| home().join(".makepad/weights/tts/kokoro-v1_0.mktts"));
    assert!(model.is_file(), "no kokoro weights at {model:?} (set MAKEPAD_TTS_MODEL)");
    let dir = model.parent().unwrap().to_path_buf();
    let voice = env_file("MAKEPAD_TTS_VOICE")
        .or_else(|| {
            ["af_heart", "bm_daniel", "bf_lily"]
                .iter()
                .map(|v| dir.join(format!("{v}.mkvoice")))
                .find(|p| p.is_file())
        })
        .expect("no kokoro voice pack next to the model (set MAKEPAD_TTS_VOICE)");
    (model, voice)
}

fn whisper_path() -> PathBuf {
    let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../local/models/ggml-large-v3-turbo.bin");
    let path = env_file("MAKEPAD_VOICE_MODEL")
        .or_else(|| checkout.is_file().then_some(checkout))
        .unwrap_or_else(|| home().join(".makepad/weights/stt/ggml-large-v3-turbo.bin"));
    assert!(path.is_file(), "no whisper weights at {path:?} (set MAKEPAD_VOICE_MODEL)");
    path
}

fn speaker() -> KokoroSpeaker {
    let (model, voice) = kokoro_paths();
    KokoroSpeaker::load_with_voice(&model.to_string_lossy(), &voice.to_string_lossy()).expect("load kokoro")
}

const SENTENCES: &[&str] = &[
    "Notes starts with one tap.",
    "Type, and it saves as you go.",
    "That's it.",
    "In 2026 we shipped 42 apps — each one faster than the last!",
    "Hello world. This is a second sentence, with a pause; and a third clause?",
];

/// Where the take is audible: the first and last 10 ms RMS frame above 5 %
/// of the loudest one, in seconds.
fn envelope(audio: &SpeechAudio) -> (f64, f64) {
    let hop = (audio.sample_rate / 100) as usize;
    let rms: Vec<f32> = audio
        .samples
        .chunks(hop)
        .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    let peak = rms.iter().cloned().fold(0.0, f32::max);
    let on = rms.iter().position(|r| *r > peak * 0.05).unwrap_or(0) as f64 * 0.01;
    let off = rms.iter().rposition(|r| *r > peak * 0.05).unwrap_or(0) as f64 * 0.01;
    (on, off)
}

/// `speed` is the synthesis speed: the lead-in and closing pause scale with it.
fn check_timings(text: &str, audio: &SpeechAudio, speed: f64) -> Vec<WordTiming> {
    let timings = audio.timings.clone().expect("kokoro timings");
    let words: Vec<&str> = text.split_whitespace().collect();
    assert_eq!(timings.len(), words.len(), "{text:?}: {timings:?}");
    for (timing, word) in timings.iter().zip(&words) {
        assert_eq!(timing.word, *word);
    }
    let duration = audio.duration_secs() as f64;
    let mut last_end = 0.0;
    for timing in &timings {
        assert!(timing.start >= last_end - 1e-9 && timing.end >= timing.start, "{text:?}: {timings:?}");
        last_end = timing.end;
    }
    // Cover the audio: speech starts right after the lead-in pad and the
    // last word ends at most a closing pause before the end.
    assert!(timings[0].start < 0.5 / speed, "{text:?}: first word at {}", timings[0].start);
    assert!(last_end <= duration + 1e-6, "{text:?}: {last_end} > {duration}");
    assert!(duration - last_end < 1.0 / speed, "{text:?}: last word ends {last_end} of {duration}");
    timings
}

#[test]
#[ignore = "needs kokoro weights"]
fn kokoro_timings_cover_the_audio_word_for_word() {
    let mut speaker = speaker();
    for text in SENTENCES {
        let audio = speaker.synthesize(text).expect("synthesize");
        let timings = check_timings(text, &audio, 1.0);
        let (on, off) = envelope(&audio);
        eprintln!("{text:?} ({:.2}s) envelope {on:.2}..{off:.2}", audio.duration_secs());
        // Kokoro's nominal first-phoneme boundary sits just after the
        // acoustic onset (measured 0.03-0.08 s: the decoder's receptive
        // field starts the sound a little early).
        assert!((timings[0].start - on).abs() < 0.15, "{text:?}: first word {} vs onset {on}", timings[0].start);
        for t in &timings {
            eprintln!("  {:>7.3} {:>7.3}  {}", t.start, t.end, t.word);
        }
    }
    // A text long enough to be synthesized in several chunks.
    let long = SENTENCES.join(" ").repeat(6);
    let audio = speaker.synthesize(&long).expect("synthesize long");
    check_timings(&long, &audio, 1.0);
    // Speed scales the clock.
    let slow = speaker.synthesize_with_speed(SENTENCES[0], 0.5).expect("slow");
    let fast = speaker.synthesize_with_speed(SENTENCES[0], 2.0).expect("fast");
    let (slow, fast) = (check_timings(SENTENCES[0], &slow, 0.5), check_timings(SENTENCES[0], &fast, 2.0));
    assert!(slow.last().unwrap().end > 2.5 * fast.last().unwrap().end);
}

#[test]
#[ignore = "needs whisper weights"]
fn whisper_encoder_matches_the_vocab_file() {
    let model = WhisperModel::load_file(&whisper_path().to_string_lossy()).expect("load whisper");
    let tok = model.tokenizer();
    // Read out of the file's vocab: each string is one token there.
    let fixtures: &[(&str, i32)] = &[(",", 11), (".", 13), (" the", 264)];
    for (text, id) in fixtures {
        assert_eq!(model.vocab.token_bytes[*id as usize], text.as_bytes());
        assert_eq!(tok.encode(text), vec![*id], "{text:?}");
    }
    // Every ordinary vocab entry that is valid UTF-8 encodes back to itself:
    // tiktoken's merges rebuild each of its tokens from the token's bytes,
    // unless the pre-split cuts the string apart first.
    let mut checked = 0;
    let mut mismatched = Vec::new();
    for id in 0..tok.n_text() as i32 {
        let bytes = tok.token_bytes(id);
        let Ok(text) = std::str::from_utf8(bytes) else { continue };
        if text.is_empty() {
            continue;
        }
        checked += 1;
        let ids = tok.encode(text);
        if ids != vec![id] {
            mismatched.push((id, text.to_string(), ids));
        }
    }
    eprintln!("{checked} utf-8 tokens checked, {} encode to several tokens", mismatched.len());
    for (id, text, ids) in &mismatched {
        eprintln!("  {id} {text:?} -> {ids:?}");
    }
    // The only tokens a text cannot reach are upper-case contractions
    // ("'S", "'RE"): the reference pattern's `'s|'t|…` is case-sensitive, so
    // tiktoken splits them off as "'" + "S" too.
    for (_, text, _) in &mismatched {
        assert!(text.starts_with('\'') && text[1..].chars().all(|c| c.is_ascii_uppercase()), "{text:?}");
    }
    // Round trips on varied text.
    for text in [
        " And so my fellow Americans, ask not what your country can do for you.",
        "Ça va? Très bien — merci! Größe, naïve café, 東京タワー, Привет мир, 😀👍",
        "  multiple   spaces\tand\nnewlines\r\n",
        "It's 3.14… \"quoted\" (paren) [bracket] {brace} <|en|>",
    ] {
        assert_eq!(tok.decode(&tok.encode(text)), text);
    }
}

#[test]
#[ignore = "needs kokoro and whisper weights"]
fn whisper_encodes_what_whisper_writes_and_aligns_kokoro_takes() {
    let mut speaker = speaker();
    let model = WhisperModel::load_file(&whisper_path().to_string_lossy()).expect("load whisper");
    let mut state = WhisperState::new(&model);
    let tok = model.tokenizer();
    // Word-start errors against Kokoro's clock, per path.
    let (mut text_errors, mut forced_errors, mut pass1_errors) = (Vec::new(), Vec::new(), Vec::new());
    for text in SENTENCES {
        let audio = speaker.synthesize(text).expect("synthesize");
        let kokoro = check_timings(text, &audio, 1.0);
        let samples_16k = audio.resampled(16_000);

        // The model's own tokens for what it heard must be the encoder's.
        let params = WhisperParams::default();
        let segments = state.transcribe_aligned(&model, &samples_16k, &params);
        for segment in &segments {
            let text_ids: Vec<i32> = segment.tokens.iter().copied().filter(|id| (*id as usize) < tok.n_text()).collect();
            let written = tok.decode(&text_ids);
            eprintln!("whisper wrote {written:?} = {text_ids:?}");
            assert_eq!(tok.encode(&written), text_ids, "{written:?}");
        }
        // Pass 1 (transcription DTW) and the teacher-forced pass over the
        // tokens Whisper wrote, matched to Kokoro's words by their text.
        for segment in &segments {
            let pass1: Vec<(String, f64)> =
                segment.words.iter().map(|w| (w.text.clone(), w.start_ms as f64 / 1000.0)).collect();
            pass1_errors.extend(start_errors(&pass1, &kokoro));
            let forced = state.force_align(&model, &samples_16k, &segment.tokens, "en").expect("force_align");
            let forced: Vec<(String, f64)> = forced.iter().map(|w| (w.text.clone(), w.start_ms as f64 / 1000.0)).collect();
            forced_errors.extend(start_errors(&forced, &kokoro));
        }

        // Forced alignment of the known text lands on Kokoro's own clock.
        let aligned = state.align_text(&model, &samples_16k, text, "en").expect("align_text");
        assert_eq!(aligned.len(), kokoro.len());
        for (a, k) in aligned.iter().zip(&kokoro) {
            assert_eq!(a.text, k.word);
            eprintln!(
                "  {:<10} kokoro {:.3}-{:.3}  whisper {:.3}-{:.3} ({:.2})",
                a.text, k.start, k.end, a.start, a.end, a.score
            );
            text_errors.push(a.start - k.start);
        }
        let mut last = 0.0;
        for a in &aligned {
            assert!(a.start >= last && a.end >= a.start);
            last = a.end;
        }
        // The first word starts where the take becomes audible.
        let (on, _) = envelope(&audio);
        assert!((aligned[0].start - on).abs() < 0.1, "{text:?}: first word {} vs onset {on}", aligned[0].start);
    }
    // Kokoro's clock is nominal (phoneme frame boundaries, ~0.05 s after the
    // acoustic onset) and Whisper's is attention on a 20 ms grid. Before the
    // row fix every path ran +0.12..+0.15 s late (a whole token); now the
    // starts must be unbiased to within a frame or two. "2026" is the one
    // outlier: Kokoro says "two thousand twenty six", Whisper wrote digits.
    for (name, errors) in [("align_text", &text_errors), ("force_align", &forced_errors), ("pass 1", &pass1_errors)] {
        let n = errors.len().max(1) as f64;
        let bias = errors.iter().sum::<f64>() / n;
        let mean = errors.iter().map(|e| e.abs()).sum::<f64>() / n;
        eprintln!("{name:<11} {} words: start bias {bias:+.3}s, mean |error| {mean:.3}s", errors.len());
        assert!(errors.len() >= 30, "{name}: only {} words matched", errors.len());
        assert!(bias.abs() < 0.06, "{name}: start bias {bias}");
        assert!(mean < 0.08, "{name}: mean start error {mean}");
    }
}

/// Start errors (seconds) of `words` against Kokoro's words, matched in
/// order by their letters and digits (Whisper may write "42" where the
/// script says "forty two"; those simply do not match).
fn start_errors(words: &[(String, f64)], kokoro: &[WordTiming]) -> Vec<f64> {
    let norm = |w: &str| -> String { w.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect() };
    let mut out = Vec::new();
    let mut next = 0;
    for (word, start) in words {
        let word = norm(word);
        if let Some(k) = (next..kokoro.len().min(next + 4)).find(|k| norm(&kokoro[*k].word) == word) {
            out.push(start - kokoro[k].start);
            next = k + 1;
        }
    }
    out
}
