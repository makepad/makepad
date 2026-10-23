//! Turn Kokoro voices into singer profiles.
//!
//!     cargo run --release -p makepad-score-voice --features kokoro --bin voice_profile -- \
//!         <kokoro-v1_0.mktts> <out dir> <voice.mkvoice>... [--speak "text"]
//!
//! Writes `<out>/kokoro_<voice>.mkvox` per voice (the library asset), and
//! with `--speak` also `<out>/kokoro_<voice>_speech.wav` of that text.

use makepad_ai_speech::kokoro::KokoroSpeaker;
use makepad_score_voice::render::wav_bytes;
use std::path::Path;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let speak = args.iter().position(|a| a == "--speak").map(|i| {
        let text = args.get(i + 1).cloned().unwrap_or_default();
        args.drain(i..(i + 2).min(args.len()));
        text
    });
    if args.len() < 3 {
        eprintln!("usage: voice_profile <model.mktts> <out dir> <voice.mkvoice>... [--speak \"text\"]");
        std::process::exit(2);
    }
    let out = Path::new(&args[1]);
    std::fs::create_dir_all(out).expect("out dir");
    for voice in &args[2..] {
        let name = Path::new(voice).file_stem().unwrap().to_string_lossy().to_string();
        let started = std::time::Instant::now();
        let mut speaker = KokoroSpeaker::load_with_voice(&args[0], voice).expect("load kokoro");
        let profile = makepad_score_voice::kokoro::analyze_voice(&mut speaker, &format!("kokoro/{name}")).expect("analyse");
        let bytes = profile.encode();
        std::fs::write(out.join(format!("kokoro_{name}.mkvox")), &bytes).expect("write profile");
        println!("kokoro/{name}: f0 {:.0} Hz, {} bytes, {:.1} s", profile.f0_ref, bytes.len(), started.elapsed().as_secs_f32());
        if let Some(text) = &speak {
            let audio = speaker.synthesize(text).expect("speak");
            std::fs::write(out.join(format!("kokoro_{name}_speech.wav")), wav_bytes(audio.sample_rate, &[&audio.samples])).expect("write wav");
        }
    }
}
