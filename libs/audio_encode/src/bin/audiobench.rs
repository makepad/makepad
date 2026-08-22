//! Decode-speed bench: our MP3 decoder vs our Vorbis decoder, same track.
//!
//! For each input file: decode it as-is (mp3 or ogg), then — for an mp3 —
//! transcode the decoded PCM to our own Ogg Vorbis and decode that too, so
//! both decoders run over the *same content*. This is the number that says
//! what a DJ deck pays to read a track: stems are stored as our ogg, source
//! tracks are usually mp3.
//!
//! ```text
//! cargo run --release -p makepad-audio-encode --bin audiobench -- track1.mp3 track2.ogg ...
//! ```

use makepad_audio_decode::{decode_any, probe_duration, sniff, AudioFormat};
use makepad_audio_encode::{encode_vorbis, EncodeOptions};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: audiobench <files...>");
        std::process::exit(2);
    }
    println!(
        "{:<40} {:>8} {:>7} {:>9} {:>12} {:>12}",
        "file", "format", "secs", "kbit/s", "decode secs", "x realtime"
    );
    for path in &args {
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("missing {path}");
            continue;
        };
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(format) = sniff(&bytes) else {
            eprintln!("{name}: unknown format");
            continue;
        };
        let Some((secs, best)) = bench_decode(&bytes) else {
            eprintln!("{name}: decode failed");
            continue;
        };
        let kbps = bytes.len() as f64 * 8.0 / secs / 1000.0;
        let label = match format {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::OggVorbis => "ogg",
        };
        println!(
            "{:<40} {:>8} {:>7.1} {:>9.0} {:>12.3} {:>12.0}",
            trim(&name),
            label,
            secs,
            kbps,
            best,
            secs / best
        );
        if format == AudioFormat::Mp3 {
            // Same content through our encoder, then our vorbis decoder.
            let decoded = decode_any(&bytes).expect("mp3 decode");
            let ogg = encode_vorbis(
                decoded.rate,
                decoded.channels,
                &decoded.pcm_interleaved_f32,
                &EncodeOptions::default(),
            )
            .expect("encode");
            let (osecs, obest) = bench_decode(&ogg).expect("own ogg decode");
            let okbps = ogg.len() as f64 * 8.0 / osecs / 1000.0;
            println!(
                "{:<40} {:>8} {:>7.1} {:>9.0} {:>12.3} {:>12.0}",
                "  -> transcoded to our ogg",
                "ogg",
                osecs,
                okbps,
                obest,
                osecs / obest
            );
        }
    }
}

/// Best-of-three whole-file decode; returns (audio seconds, best wall secs).
fn bench_decode(bytes: &[u8]) -> Option<(f64, f64)> {
    let secs = probe_duration(bytes)
        .ok()
        .or_else(|| decode_any(bytes).ok().map(|a| a.duration_secs()))?;
    let mut best = f64::INFINITY;
    for _ in 0..3 {
        let started = std::time::Instant::now();
        let audio = decode_any(bytes).ok()?;
        let elapsed = started.elapsed().as_secs_f64();
        std::hint::black_box(&audio);
        best = best.min(elapsed);
    }
    Some((secs, best))
}

fn trim(name: &str) -> String {
    if name.len() <= 40 {
        name.to_string()
    } else {
        format!("{}...", &name[..37])
    }
}
