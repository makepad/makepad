//! Dev CLI: encode a WAV to Ogg Vorbis with this crate, report speed and
//! round-trip SNR through the sibling decoder.
//!
//! ```text
//! cargo run --release -p makepad-audio-encode --bin oggenc -- in.wav out.ogg [quality] [threads]
//! ```

#[cfg(target_arch = "wasm32")]
fn main() {}

// This filesystem benchmark is a native developer tool, not part of the wasm library graph.
#[cfg(not(target_arch = "wasm32"))]
mod native {

use makepad_audio_encode::{encode_vorbis, EncodeOptions};

pub(super) fn run() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: oggenc <in.wav> <out.ogg> [quality 0..1] [threads]");
        std::process::exit(2);
    }
    let quality: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.5);
    let threads: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
    let bytes = std::fs::read(&args[1]).expect("read wav");
    let (rate, channels, pcm) = read_wav(&bytes);
    let secs = pcm.len() as f64 / channels as f64 / rate as f64;
    eprintln!("in: {rate} Hz, {channels} ch, {secs:.2}s");

    let opts = EncodeOptions { quality, threads, ..Default::default() };
    // Warm once (page cache, allocator), then measure.
    let started = std::time::Instant::now();
    let ogg = encode_vorbis(rate, channels, &pcm, &opts).expect("encode");
    let elapsed = started.elapsed().as_secs_f64();
    let kbps = ogg.len() as f64 * 8.0 / secs / 1000.0;
    eprintln!(
        "encoded {} bytes, {kbps:.0} kbit/s, {:.3}s = {:.0}x realtime (threads={})",
        ogg.len(),
        elapsed,
        secs / elapsed,
        if threads == 0 { "auto".into() } else { threads.to_string() }
    );
    std::fs::write(&args[2], &ogg).expect("write ogg");

    // Round trip through our decoder.
    let started = std::time::Instant::now();
    let decoded = makepad_audio_decode::vorbis::decode_all(&ogg).expect("decode");
    let dec_elapsed = started.elapsed().as_secs_f64();
    assert_eq!(decoded.rate, rate);
    assert_eq!(decoded.channels, channels);
    let n = decoded.pcm_interleaved_f32.len().min(pcm.len());
    let mut signal = 0f64;
    let mut noise = 0f64;
    for i in 0..n {
        let r = pcm[i] as f64;
        let g = decoded.pcm_interleaved_f32[i] as f64;
        signal += r * r;
        noise += (r - g) * (r - g);
    }
    eprintln!(
        "decode {:.3}s = {:.0}x realtime, snr {:.2} dB, {} vs {} samples",
        dec_elapsed,
        secs / dec_elapsed,
        10.0 * (signal / noise.max(1e-30)).log10(),
        decoded.pcm_interleaved_f32.len(),
        pcm.len()
    );
}

/// Minimal RIFF/WAVE reader: PCM16 or float32, interleaved.
fn read_wav(bytes: &[u8]) -> (u32, u16, Vec<f32>) {
    assert!(&bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE", "not a wav");
    let mut at = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // (format, channels, rate, bits)
    let mut data: Option<&[u8]> = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + len).min(bytes.len())];
        match id {
            b"fmt " => {
                let format = u16::from_le_bytes(body[0..2].try_into().unwrap());
                let channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
                fmt = Some((format, channels, rate, bits));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        at += 8 + len + (len & 1);
    }
    let (format, channels, rate, bits) = fmt.expect("fmt chunk");
    let data = data.expect("data chunk");
    let pcm: Vec<f32> = match (format, bits) {
        (1, 16) => data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect(),
        (3, 32) => data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        other => panic!("unsupported wav format {other:?}"),
    };
    (rate, channels, pcm)
}

}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    native::run();
}
