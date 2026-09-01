//! Speak sentences through the hub's TTS session, hear them back through its
//! STT session, and score the word error rate.
//!
//! "It made noise" is not a test. This closes the loop through the SAME
//! sessions apps use — whichever engines the hub picks (Kokoro or the OS
//! voice; Whisper here, on the machine node, on a LAN box, or the OS
//! recognizer) — so it is also the scoreboard for comparing them:
//!
//! ```text
//! cd libs/ai/hub && cargo run --release --bin speech-roundtrip
//! cd libs/ai/hub && cargo run --release --bin speech-roundtrip -- --tts system --stt system --reach local
//! cd libs/ai/hub && cargo run --release --bin speech-roundtrip -- score clip.wav "what it should say"
//! ```
//!
//! `score` transcribes a WAV from anywhere (a reference export, a file on
//! disk) through the same STT session and scores it against the words it
//! should contain.

use makepad_ai_hub::hub::AiHub;
use makepad_ai_hub::speech::{
    SpeechReach, SttConfig, SttEngine, SttEvent, TtsConfig, TtsEngine, TtsEvent, STT_SAMPLE_RATE,
};
use std::time::{Duration, Instant};

const SENTENCES: &[&str] = &[
    "Hi! I make games with you.",
    "I made the player jump higher.",
    "Escape the Gummer, a squishy purple blob.",
    "You scored forty two points.",
    "The little guy can run and jump on the platforms.",
    "I gave the ghost bigger eyes and made it chase you faster.",
];

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    // Apple's synthesizer hands its audio back through the process's main run
    // loop: run the tool on a thread and keep main pumping. Everything ends
    // through `std::process::exit`, which is how the pump stops.
    #[cfg(target_os = "macos")]
    {
        std::thread::spawn(|| {
            run();
            std::process::exit(0);
        });
        unsafe { CFRunLoopRun() };
    }
    #[cfg(not(target_os = "macos"))]
    run();
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRunLoopRun();
}

fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let reach = match arg(&args, "--reach").as_deref() {
        Some("local") => SpeechReach::Local,
        Some("machine") => SpeechReach::Machine,
        _ => SpeechReach::Lan,
    };
    let tts_engine = match arg(&args, "--tts").as_deref() {
        Some("system") => TtsEngine::System,
        Some("kokoro") => TtsEngine::Kokoro,
        _ => TtsEngine::Auto,
    };
    let stt_engine = match arg(&args, "--stt").as_deref() {
        Some("system") => SttEngine::System,
        Some("whisper") => SttEngine::Whisper,
        _ => SttEngine::Auto,
    };

    let hub = AiHub::in_process();
    if args.first().map(String::as_str) == Some("score") {
        return score(&hub, &args[1..], stt_engine, reach);
    }
    let tts = hub.start_tts(TtsConfig { engine: tts_engine, reach, voice: arg(&args, "--voice"), ..TtsConfig::default() });
    let stt = hub.start_stt(SttConfig { engine: stt_engine, reach, ..SttConfig::default() });

    // Wait for both engines; print what the hub chose.
    let deadline = Instant::now() + Duration::from_secs(180);
    let (mut tts_ready, mut stt_ready) = (false, false);
    while !(tts_ready && stt_ready) {
        for event in tts.poll() {
            match event {
                TtsEvent::Loading { phase, fraction } => eprintln!("tts loading: {phase} {:.0}%", fraction * 100.0),
                TtsEvent::Ready(info) => {
                    println!("tts : {} via {}{} ({} voices)", info.engine, info.pipe, remote(&info.remote), info.voices.len());
                    tts_ready = true;
                }
                TtsEvent::Failed(why) => {
                    eprintln!("tts failed: {why}");
                    std::process::exit(1);
                }
                other => eprintln!("tts: unexpected {other:?}"),
            }
        }
        for event in stt.poll() {
            match event {
                SttEvent::Loading { phase, fraction } => eprintln!("stt loading: {phase} {:.0}%", fraction * 100.0),
                SttEvent::Ready(info) => {
                    println!("stt : {} via {}{} caps={:?}", info.engine, info.pipe, remote(&info.remote), info.capabilities);
                    if !info.capabilities.pcm_input {
                        eprintln!("this recognizer only listens on the microphone; the roundtrip needs PCM input");
                        std::process::exit(1);
                    }
                    stt_ready = true;
                }
                SttEvent::Failed(why) => {
                    eprintln!("stt failed: {why}");
                    std::process::exit(1);
                }
                other => eprintln!("stt: unexpected {other:?}"),
            }
        }
        if Instant::now() > deadline {
            eprintln!("engines did not come up in time");
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!();

    let (mut total_words, mut total_errors) = (0usize, 0usize);
    let started = Instant::now();
    for sentence in SENTENCES {
        let id = tts.say(*sentence);
        let audio = loop {
            let mut got = None;
            for event in tts.poll() {
                match event {
                    TtsEvent::Audio { utterance, audio, .. } if utterance == id => got = Some(audio),
                    TtsEvent::Error { message, .. } => {
                        eprintln!("tts error: {message}");
                        std::process::exit(1);
                    }
                    _ => {}
                }
            }
            if let Some(audio) = got {
                break audio;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let pcm = audio.resampled(STT_SAMPLE_RATE);
        let synth_secs = audio.duration_secs();
        let id = stt.transcribe(pcm.samples);
        let heard = loop {
            let mut got = None;
            for event in stt.poll() {
                match event {
                    SttEvent::Final { utterance, transcript, .. } if utterance == id => got = Some(transcript.text()),
                    SttEvent::Error { message, .. } => {
                        eprintln!("stt error: {message}");
                        std::process::exit(1);
                    }
                    _ => {}
                }
            }
            if let Some(text) = got {
                break text;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let expected = words(sentence);
        let got = words(&heard);
        let errors = edit_distance(&expected, &got);
        total_words += expected.len();
        total_errors += errors;
        println!("said  : {sentence}");
        println!("heard : {heard}");
        println!("        {synth_secs:.1}s audio, {errors} word error(s)\n");
    }
    println!(
        "word error rate: {:.1}% ({total_errors}/{total_words}) in {:.1}s",
        100.0 * total_errors as f64 / total_words.max(1) as f64,
        started.elapsed().as_secs_f64()
    );
}

fn remote(node: &Option<String>) -> String {
    node.as_ref().map(|n| format!(" on {n}")).unwrap_or_default()
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

fn edit_distance(a: &[String], b: &[String]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, wa) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, wb) in b.iter().enumerate() {
            let cost = if wa == wb { 0 } else { 1 };
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// `score <wav> [expected words...]`: transcribe one file through the hub's
/// STT session and report the word error rate against the expected text.
fn score(hub: &AiHub, args: &[String], engine: SttEngine, reach: SpeechReach) {
    let Some(path) = args.first() else {
        eprintln!("usage: speech-roundtrip score <wav> [expected text]");
        std::process::exit(2);
    };
    let expected = args[1..].join(" ");
    let bytes = std::fs::read(path).expect("read wav");
    let (samples, rate) = makepad_ai_hub::wav::decode_wav_to_mono_f32(&bytes).expect("decode wav");
    let audio = makepad_ai_hub::speech::SpeechAudio { samples, sample_rate: rate };
    println!("wav      : {path} ({:.2}s @ {rate} Hz)", audio.duration_secs());
    let stt = hub.start_stt(SttConfig { engine, reach, ..SttConfig::default() });
    let id = stt.transcribe(audio.resampled(STT_SAMPLE_RATE).samples);
    let deadline = Instant::now() + Duration::from_secs(300);
    let heard = loop {
        match stt.recv_timeout(Duration::from_millis(50)) {
            Some(SttEvent::Ready(info)) => println!("stt      : {} via {}{}", info.engine, info.pipe, remote(&info.remote)),
            Some(SttEvent::Loading { phase, fraction }) => eprintln!("stt loading: {phase} {:.0}%", fraction * 100.0),
            Some(SttEvent::Final { utterance, transcript, .. }) if utterance == id => break transcript.text(),
            Some(SttEvent::Failed(why)) | Some(SttEvent::Error { message: why, .. }) => {
                eprintln!("stt failed: {why}");
                std::process::exit(1);
            }
            _ => {}
        }
        if Instant::now() > deadline {
            eprintln!("no transcript in time");
            std::process::exit(1);
        }
    };
    println!("heard    : {heard}");
    if !expected.is_empty() {
        let (e, g) = (words(&expected), words(&heard));
        let errors = edit_distance(&e, &g);
        println!("expected : {expected}");
        println!("WER      : {:.1}%  ({errors}/{} words)", 100.0 * errors as f64 / e.len().max(1) as f64, e.len());
    }
}
