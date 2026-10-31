//! Mandarin round trip: Kokoro speaks, Whisper listens, and the characters it
//! heard are scored against what was said. Files only (no microphone).
//!
//!     zh-eval <kokoro model.mktts> <voice dir> <whisper ggml.bin> <out dir> \
//!             <front end: v10|v11> <voice,voice,...> [sentences|short]
//!
//! `sentences` (default): per voice, character error rate of Whisper with
//! `language = zh` (with and without a Simplified-Chinese prompt) and with
//! `language = auto`, plus how often the output is Traditional script.
//! `short`: one- to four-syllable utterances in Mandarin and English; reports
//! how often `auto` names the right language.

use makepad_ai_speech::g2p::{zh::number::spell_numbers, Frontend};
use makepad_ai_speech::kokoro::KokoroSpeaker;
use makepad_ai_speech::whisper::{WhisperModel, WhisperParams, WhisperState};
use makepad_ai_speech::SpeechAudio;

const SENTENCES: &[&str] = &[
    "今天天气很好，我们一起去公园散步吧。",
    "请问去火车站怎么走？",
    "这家餐厅的菜非常好吃，价格也很合理。",
    "明天上午九点钟开会，请大家准时参加。",
    "他昨天晚上看了一部很有意思的电影。",
    "我的手机没有电了，可以借你的充电器用一下吗？",
    "中国有五千年的悠久历史和灿烂的文化。",
    "春天来了，树上的花开得特别漂亮。",
    "这本书一共有三百二十页，我已经读完了一半。",
    "对不起，我不太明白你的意思，能再说一遍吗？",
    "科学技术的发展让我们的生活变得越来越方便。",
    "银行的工作人员告诉我，今天下午不办理这项业务。",
];

const SHORT_ZH: &[&str] = &["你好", "谢谢", "再见", "好的", "早上好", "对不起", "没关系", "不知道"];
const SHORT_EN: &[&str] = &["Hello.", "Thank you.", "Goodbye.", "Okay.", "Good morning.", "I'm sorry.", "No problem.", "I don't know."];

/// Characters that exist only in Traditional script among the common ones.
const TRADITIONAL: &str = "們這個說會來時國學還從點對開關見問間長門車東馬風書為電機無業進動過樣種後經麼邊體讓產歲環爲園頭點幾實發現聽記語認識讀寫謝請與於師體氣歡樓錢鐘鐵點課題飛龍鳥魚貓兒鄉買賣";

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn normalize(text: &str) -> Vec<char> {
    spell_numbers(text).chars().filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c)).collect()
}

fn wav(audio: &SpeechAudio) -> Vec<u8> {
    let n = (audio.samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + n as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + n).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&audio.sample_rate.to_le_bytes());
    out.extend_from_slice(&(audio.sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&n.to_le_bytes());
    for s in &audio.samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

/// 16 kHz mono PCM16 WAV (what `ffmpeg -ar 16000 -ac 1` writes), as samples.
fn read_wav16k(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("read wav");
    let at = bytes.windows(4).position(|w| w == b"data").expect("data chunk") + 8;
    bytes[at..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect()
}

/// `real <whisper.bin> <clip.wav (16 kHz mono)> <text file> [prompt]`: a recording of
/// someone speaking, scored against the text it reads (the best-matching stretch of it).
fn real(args: &[String]) {
    let whisper = WhisperModel::load_file(&args[1]).expect("load whisper");
    let mut state = WhisperState::new(&whisper);
    let samples = read_wav16k(&args[2]);
    let truth: Vec<char> = normalize(&std::fs::read_to_string(&args[3]).expect("read text"));
    let prompt = args.get(4).cloned().unwrap_or_default();
    println!("whisper backend: {}; clip {:.1}s", makepad_ai_speech::whisper::accel_backend_name(), samples.len() as f32 / 16000.0);
    for (label, language, prompt) in [("zh", "zh", ""), ("zh+prompt", "zh", prompt.as_str()), ("auto", "auto", "")] {
        let mut params = WhisperParams::default();
        params.language = language.into();
        params.prompt = prompt.into();
        let text: String = state.transcribe(&whisper, &samples, &params).into_iter().map(|s| s.text).collect();
        let heard = normalize(&text);
        // The clip covers a prefix of the text from some start: try every start
        // within the first 400 characters and a window of the heard length.
        let mut best = (usize::MAX, 0usize);
        for start in (0..truth.len().min(400)).step_by(2) {
            let end = (start + heard.len() + 6).min(truth.len());
            let window = &truth[start..end];
            let e = levenshtein(window, &heard);
            if e < best.0 {
                best = (e, start);
            }
        }
        println!(
            "{label}: CER {:.1}% ({} errors over {} heard chars, text from char {}) lang={:?}\n   {}",
            100.0 * best.0 as f64 / heard.len().max(1) as f64,
            best.0,
            heard.len(),
            best.1,
            state.detected_language().map(|(l, p)| format!("{l} {p:.2}")),
            text.trim()
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("real") {
        return real(&args);
    }
    if args.len() < 6 {
        eprintln!("usage: zh-eval <model.mktts> <voice dir> <whisper.bin> <out dir> <v10|v11> <voices,...> [sentences|short]");
        std::process::exit(2);
    }
    let (model_path, voice_dir, whisper_path, out_dir) = (&args[0], &args[1], &args[2], &args[3]);
    let frontend = match args[4].as_str() {
        "v10" => Frontend::MandarinV10,
        _ => Frontend::MandarinV11,
    };
    let voices: Vec<&str> = args[5].split(',').collect();
    let short = args.get(6).is_some_and(|m| m == "short");
    std::fs::create_dir_all(out_dir).ok();

    let whisper = WhisperModel::load_file(whisper_path).expect("load whisper");
    let mut state = WhisperState::new(&whisper);
    println!("whisper backend: {}", makepad_ai_speech::whisper::accel_backend_name());
    let mut transcribe = |samples: &[f32], language: &str, prompt: &str| -> (String, Option<String>) {
        let mut params = WhisperParams::default();
        params.language = language.into();
        params.prompt = prompt.into();
        let text: String = state.transcribe(&whisper, samples, &params).into_iter().map(|s| s.text).collect();
        (text.trim().to_string(), state.detected_language().map(|(l, _)| l.to_string()))
    };

    let (mut total_detect, mut right_detect) = (0usize, 0usize);
    for voice in &voices {
        let voice_file = format!("{voice_dir}/{voice}.mkvoice");
        let mut speaker = KokoroSpeaker::load_with_voice(model_path, &voice_file).expect("load kokoro");
        speaker.set_frontend(frontend);
        if short {
            for (frontend_i, list) in [(true, SHORT_ZH), (false, SHORT_EN)] {
                for text in list {
                    let mut en_speaker = None;
                    let audio = if frontend_i {
                        speaker.synthesize(text)
                    } else {
                        // English words on the zh model go through the English symbols.
                        let s = en_speaker.get_or_insert_with(|| {
                            let mut s = KokoroSpeaker::load_with_voice(model_path, &voice_file).expect("load");
                            s.set_frontend(if frontend == Frontend::MandarinV10 { Frontend::English } else { Frontend::EnglishV11 });
                            s
                        });
                        s.synthesize(text)
                    };
                    let Ok(audio) = audio else { continue };
                    let pcm = audio.resampled(16_000);
                    let (heard, lang) = transcribe(&pcm, "auto", "");
                    let want = if frontend_i { "zh" } else { "en" };
                    total_detect += 1;
                    let ok = lang.as_deref() == Some(want);
                    right_detect += usize::from(ok);
                    println!("{voice}\t{text}\t-> {:?}\t{}\t{heard}", lang, if ok { "ok" } else { "WRONG" });
                }
            }
            continue;
        }
        let (mut chars, mut e_zh, mut e_prompt, mut e_auto, mut trad) = (0usize, 0usize, 0usize, 0usize, 0usize);
        let (mut audio_secs, mut synth_secs) = (0.0f64, 0.0f64);
        let simple_prompt = "以下是普通话的句子。";
        for (i, text) in SENTENCES.iter().enumerate() {
            let started = std::time::Instant::now();
            let audio = speaker.synthesize(text).expect("synthesize");
            synth_secs += started.elapsed().as_secs_f64();
            audio_secs += audio.duration_secs() as f64;
            std::fs::write(format!("{out_dir}/{voice}_{i:02}.wav"), wav(&audio)).ok();
            let pcm = audio.resampled(16_000);
            let want = normalize(text);
            chars += want.len();
            let (a, _) = transcribe(&pcm, "zh", "");
            let (b, _) = transcribe(&pcm, "zh", simple_prompt);
            let (c, lang) = transcribe(&pcm, "auto", "");
            e_zh += levenshtein(&want, &normalize(&a));
            e_prompt += levenshtein(&want, &normalize(&b));
            e_auto += levenshtein(&want, &normalize(&c));
            trad += [&a, &b, &c].iter().filter(|t| t.chars().any(|ch| TRADITIONAL.contains(ch))).count();
            println!("{voice} #{i:02} auto={lang:?}\n   said : {text}\n   zh   : {a}\n   zh+p : {b}\n   auto : {c}");
        }
        println!(
            "== {voice}: CER zh {:.1}%  zh+simplified-prompt {:.1}%  auto {:.1}%  | traditional-script outputs {trad}/{}  | {audio_secs:.0}s audio in {synth_secs:.0}s",
            100.0 * e_zh as f64 / chars as f64,
            100.0 * e_prompt as f64 / chars as f64,
            100.0 * e_auto as f64 / chars as f64,
            SENTENCES.len() * 3,
        );
    }
    if short {
        println!("== language detection on short utterances: {right_detect}/{total_detect}");
    }
}
