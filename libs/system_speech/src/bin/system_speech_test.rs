//! Exercise the OS speech engines from the command line.
//!
//! ```text
//! system-speech-test info
//! system-speech-test voices
//! system-speech-test tts "Hello there" [--voice ID] [--lang en-GB] [--rate 1.2] [--pitch 1.0] [-o out.wav]
//! system-speech-test stt input.wav [--lang en]            # PCM-input engines
//! system-speech-test listen [--lang en] [--seconds 8]     # mic-owning engines
//! ```

use makepad_system_speech::{stt, tts, wav, SttEvent, SttOptions, TtsOptions, STT_SAMPLE_RATE};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("info") | None => {
            println!("stt engine : {} (available: {})", stt::engine_name(), stt::available());
            println!("stt caps   : {:?}", stt::capabilities());
            println!("tts engine : {} (available: {})", tts::engine_name(), tts::available());
            println!("tts voices : {}", tts::voices().len());
        }
        Some("voices") => {
            for v in tts::voices() {
                println!("{:<48} {:<8} {:?} {}", v.id, v.language, v.gender, v.name);
            }
        }
        Some("tts") => {
            let text = args.get(1).cloned().unwrap_or_else(|| "Hello from makepad system speech.".into());
            let mut options = TtsOptions::default();
            options.voice = arg_value(&args, "--voice");
            if let Some(lang) = arg_value(&args, "--lang") {
                options.language = lang;
            }
            if let Some(rate) = arg_value(&args, "--rate").and_then(|v| v.parse().ok()) {
                options.rate = rate;
            }
            if let Some(pitch) = arg_value(&args, "--pitch").and_then(|v| v.parse().ok()) {
                options.pitch = pitch;
            }
            let t0 = Instant::now();
            match tts::synthesize(&text, &options) {
                Ok(audio) => {
                    println!(
                        "rendered {:.2}s at {} Hz in {:.2}s",
                        audio.duration_secs(),
                        audio.sample_rate,
                        t0.elapsed().as_secs_f64()
                    );
                    let out = arg_value(&args, "-o").unwrap_or_else(|| "system_speech_tts.wav".into());
                    std::fs::write(&out, wav::encode_pcm16_mono(&audio)).expect("write wav");
                    println!("wrote {out}");
                }
                Err(err) => {
                    eprintln!("tts failed: {err}");
                    std::process::exit(1);
                }
            }
        }
        Some("stt") => {
            let path = args.get(1).expect("stt <input.wav>");
            let bytes = std::fs::read(path).expect("read wav");
            let audio = wav::decode(&bytes).expect("decode wav").resampled(STT_SAMPLE_RATE);
            let mut options = SttOptions::default();
            if let Some(lang) = arg_value(&args, "--lang") {
                options.language = lang;
            }
            if let Err(err) = stt::prepare(&options.language) {
                eprintln!("prepare: {err} (continuing)");
            }
            let t0 = Instant::now();
            match stt::transcribe(&audio.samples, &options) {
                Ok(transcript) => {
                    println!("transcribed {:.2}s of audio in {:.2}s", audio.duration_secs(), t0.elapsed().as_secs_f64());
                    for s in &transcript.segments {
                        println!("[{:>7.2} --> {:>7.2}] {}", s.start_ms as f64 / 1000.0, s.end_ms as f64 / 1000.0, s.text);
                    }
                    println!("text: {}", transcript.text());
                }
                Err(err) => {
                    eprintln!("stt failed: {err}");
                    std::process::exit(1);
                }
            }
        }
        Some("listen") => {
            let mut options = SttOptions::default();
            if let Some(lang) = arg_value(&args, "--lang") {
                options.language = lang;
            }
            let seconds: u64 = arg_value(&args, "--seconds").and_then(|v| v.parse().ok()).unwrap_or(8);
            let (tx, rx) = mpsc::channel();
            let handle = match stt::listen(&options, tx) {
                Ok(handle) => handle,
                Err(err) => {
                    eprintln!("listen failed: {err}");
                    std::process::exit(1);
                }
            };
            println!("listening for {seconds}s ...");
            let deadline = Instant::now() + Duration::from_secs(seconds);
            let mut handle = Some(handle);
            loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(SttEvent::Level(level)) => print!("\rlevel {level:.2}   "),
                    Ok(SttEvent::Partial(text)) => println!("\npartial: {text}"),
                    Ok(SttEvent::Final(t)) => println!("\nfinal  : {}", t.text()),
                    Ok(SttEvent::Error(err)) => println!("\nerror  : {err}"),
                    Ok(SttEvent::Ended) => {
                        println!("\nended");
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                if Instant::now() >= deadline {
                    if let Some(handle) = handle.take() {
                        handle.stop();
                    }
                }
            }
        }
        Some(other) => {
            eprintln!("unknown command {other}");
            std::process::exit(2);
        }
    }
}
