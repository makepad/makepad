//! Render a sung line with Cantor.
//!
//!   sing_render [--weights model.mksing | --random tiny|base] [--out line.wav]
//!               [--steps N] [--f0 rule|model]
//!   sing_render --weights model.mksing --bench N
//!       Speed: N lines of 8-14 syllables on random C-major melodies (96 BPM),
//!       after one warm-up render; prints seconds of audio per second of
//!       render (x real time). With CANTOR_TIMING set, each phrase's acoustic /
//!       refiner / vocoder times go to stderr.
//!
//! With no weights it renders the demo line on randomly initialised weights,
//! which proves the pipeline end to end (the sound is noise-like until trained).

use makepad_ai_sing::acoustic::AcousticConfig;
use makepad_ai_sing::cantor::f0_error_stats;
use makepad_ai_sing::dsp::{wav_bytes, SR};
use makepad_ai_sing::score::simple_line;
use makepad_ai_sing::vocoder::VocoderConfig;
use makepad_ai_sing::{Cantor, F0Mode, RenderOpts};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let t = Instant::now();
    let cantor = match arg("--weights") {
        Some(p) => Cantor::load(std::path::Path::new(&p)).unwrap_or_else(|e| panic!("{p}: {e}")),
        None => match arg("--random").as_deref() {
            Some("base") => Cantor::random(AcousticConfig::base(), VocoderConfig::base(), 1),
            _ => Cantor::random(AcousticConfig::tiny(), VocoderConfig::tiny(), 1),
        },
    };
    eprintln!(
        "model: {} params (acoustic {}, vocoder {}), loaded in {:.0} ms",
        cantor.params.count(),
        cantor.params.count_prefix("ac."),
        cantor.params.count_prefix("voc."),
        t.elapsed().as_secs_f32() * 1000.0
    );
    let mut opts = RenderOpts::default();
    if let Some(s) = arg("--steps") {
        opts.refine_steps = s.parse().unwrap();
    }
    if arg("--f0").as_deref() == Some("model") {
        opts.f0 = F0Mode::Model;
    }
    if let Some(n) = arg("--bench").and_then(|v| v.parse::<usize>().ok()) {
        let syl = ["ðə", "sʌn", "ɪz", "ʃaɪ", "nɪŋ", "ɑn", "ðə", "ɹɪ", "vəɹ", "aɪ", "wɪl", "ɹɪ", "mɛm", "bəɹ", "ju", "tə", "naɪt", "wi", "wɔk", "ə", "lɔŋ", "ðə", "ɛmp", "ti", "stɹit"];
        let scale = [0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0, 12.0, 14.0, 16.0];
        let mut rng = makepad_ai_sing::dsp::Rng::new(17);
        let warm = simple_line(&[(60.0, 1.0, "la"), (62.0, 1.0, "la")], 96.0);
        cantor.render(&warm, &opts);
        let (mut audio_s, mut took) = (0.0f32, 0.0f32);
        let mut k = 0;
        for _ in 0..n {
            let count = 8 + rng.below(7);
            let mut deg = 2i32;
            let notes: Vec<(f32, f32, &str)> = (0..count)
                .map(|i| {
                    deg = (deg + rng.below(5) as i32 - 2).clamp(0, 9);
                    let beats = if i + 1 == count { 2.0 } else { [1.0, 1.0, 0.5, 0.5, 1.5][rng.below(5)] };
                    k += 1;
                    (60.0 + scale[deg as usize], beats, syl[k % syl.len()])
                })
                .collect();
            let line = simple_line(&notes, 96.0);
            let t = Instant::now();
            let (audio, _) = cantor.render(&line, &opts);
            took += t.elapsed().as_secs_f32();
            audio_s += audio.len() as f32 / SR as f32;
        }
        println!("bench: {n} lines, {audio_s:.1} s of audio in {took:.2} s: {:.1}x real time", audio_s / took);
        return;
    }
    // "Twinkle" opening, with a melisma and a rest.
    let line = simple_line(
        &[
            (60.0, 1.0, "twɪŋ"), (60.0, 1.0, "kəl"), (67.0, 1.0, "twɪŋ"), (67.0, 1.0, "kəl"),
            (69.0, 1.0, "lɪ"), (69.0, 1.0, "təl"), (67.0, 1.5, "stɑɹ"), (65.0, 0.5, "_"),
        ],
        100.0,
    );
    let t = Instant::now();
    let (audio, phrases) = cantor.render(&line, &opts);
    let secs = audio.len() as f32 / SR as f32;
    let took = t.elapsed().as_secs_f32();
    eprintln!("rendered {secs:.2} s of audio in {:.0} ms ({:.1}x real time)", took * 1000.0, secs / took);
    for p in &phrases {
        let (c, gross, n) = f0_error_stats(&p.audio, &p.frames.f0);
        eprintln!("phrase at {:.2} s: {} frames, {} tokens, f0 error {c:.1} cents ({:.1}% gross) over {n} frames", p.start, p.frames.len(), p.frames.tokens.len(), gross * 100.0);
    }
    let peak = audio.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let gain = if peak > 0.0 { 0.9 / peak } else { 1.0 };
    let out: Vec<f32> = audio.iter().map(|v| v * gain).collect();
    let path = arg("--out").unwrap_or_else(|| "cantor_line.wav".into());
    std::fs::write(&path, wav_bytes(SR, &out)).unwrap();
    eprintln!("wrote {path} (peak {peak:.3})");
}
