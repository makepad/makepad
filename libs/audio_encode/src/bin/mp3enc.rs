//! Dev CLI: transcode any file the sibling decoder reads (WAV, FLAC, Ogg
//! Vorbis, MP3) to MP3 with this crate; report speed, size and round-trip SNR.
//!
//! ```text
//! cargo run --release -p makepad-audio-encode --bin mp3enc -- in.ogg out.mp3 [kbit/s] [threads]
//! ```

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use makepad_audio_encode::mp3::{encode_mp3, Mp3Options};
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 4 && args[1] == "--compare" {
        // SNR of any decodable file against an original, aligned the same way
        let a = makepad_audio_decode::decode_any(&std::fs::read(&args[2]).expect("read original")).expect("decode original");
        let b = makepad_audio_decode::decode_any(&std::fs::read(&args[3]).expect("read coded")).expect("decode coded");
        for c in 0..a.channels.min(b.channels) as usize {
            let (lag, snr) = snr(&a.channel(c), &b.channel(c));
            eprintln!("{}: channel {c}: lag {lag} snr {snr:.2} dB", args[3]);
        }
        return;
    }
    if args.len() < 3 {
        eprintln!("usage: mp3enc <in> <out.mp3> [kbit/s] [threads]");
        std::process::exit(2);
    }
    let bitrate_kbps = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(192);
    let threads = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
    let bytes = std::fs::read(&args[1]).expect("read input");
    let t0 = std::time::Instant::now();
    let audio = makepad_audio_decode::decode_any(&bytes).expect("decode input");
    let decode_s = t0.elapsed().as_secs_f64();
    let secs = audio.duration_secs();
    eprintln!("in: {} Hz, {} ch, {secs:.2}s (decoded in {decode_s:.3}s)", audio.rate, audio.channels);
    let opts = Mp3Options { bitrate_kbps, threads };
    let t1 = std::time::Instant::now();
    let mp3 = encode_mp3(audio.rate, audio.channels, &audio.pcm_interleaved_f32, &opts).expect("encode");
    let encode_s = t1.elapsed().as_secs_f64();
    eprintln!(
        "encoded {} bytes, {:.1} kbit/s, {encode_s:.3}s = {:.0}x realtime (threads={threads})",
        mp3.len(),
        mp3.len() as f64 * 8.0 / secs / 1000.0,
        secs / encode_s
    );
    std::fs::write(&args[2], &mp3).expect("write output");
    // round trip through the in-tree decoder, aligned by cross-correlation
    let back = makepad_audio_decode::mp3::decode_all(&mp3).expect("decode our mp3");
    for c in 0..audio.channels.min(2) as usize {
        let (lag, snr) = snr(&audio.channel(c), &back.channel(c));
        eprintln!("round trip channel {c}: lag {lag} samples, snr {snr:.2} dB");
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn snr(x: &[f32], y: &[f32]) -> (usize, f64) {
    let (a, b) = (x.len() / 4, x.len() * 3 / 4);
    let mut best = (0usize, f64::MIN);
    for lag in 0..3000 {
        if b + lag >= y.len() {
            break;
        }
        let s: f64 = (a..b).step_by(7).map(|i| x[i] as f64 * y[i + lag] as f64).sum();
        if s > best.1 {
            best = (lag, s);
        }
    }
    let (mut sig, mut err) = (0.0f64, 0.0f64);
    for i in a..b.min(y.len().saturating_sub(best.0)) {
        sig += (x[i] as f64).powi(2);
        err += (x[i] as f64 - y[i + best.0] as f64).powi(2);
    }
    (best.0, 10.0 * (sig / err.max(1e-30)).log10())
}
