//! Demix a whole wav into four stem wavs — the by-ear proof tool.
//!
//! ```text
//! stems_demix <checkpoint.ckpt> <track.wav> <out_dir>
//! ```
//!
//! Reads 16-bit PCM stereo RIFF (the store's audio blobs), runs the full
//! separator, writes `drums.wav` / `bass.wav` / `other.wav` / `vocals.wav`
//! at the input rate plus per-stem RMS/peak so leakage is visible in
//! numbers before ears.

use makepad_ai_stems::config::Stem;
use makepad_ai_stems::demix::demix_all;
use makepad_ai_stems::model::{StemsModel, StereoBuf};
use std::io::Write;
use std::path::Path;

fn read_wav_pcm16(path: &Path) -> (StereoBuf, u32) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(&bytes[0..4], b"RIFF", "not a RIFF file");
    assert_eq!(&bytes[8..12], b"WAVE", "not a WAVE file");
    let mut pos = 12usize;
    let (mut rate, mut channels, mut bits) = (0u32, 0u16, 0u16);
    let mut data: &[u8] = &[];
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size =
            u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]])
                as usize;
        let body = &bytes[pos + 8..(pos + 8 + size).min(bytes.len())];
        match id {
            b"fmt " => {
                channels = u16::from_le_bytes([body[2], body[3]]);
                rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                bits = u16::from_le_bytes([body[14], body[15]]);
            }
            b"data" => data = body,
            _ => {}
        }
        pos += 8 + size + (size & 1);
    }
    assert_eq!(channels, 2, "track must be stereo");
    assert_eq!(bits, 16, "track must be 16-bit PCM");
    let frames = data.len() / 4;
    let mut out = StereoBuf::silence(frames);
    for frame in 0..frames {
        let at = frame * 4;
        out.left[frame] = i16::from_le_bytes([data[at], data[at + 1]]) as f32 / 32768.0;
        out.right[frame] = i16::from_le_bytes([data[at + 2], data[at + 3]]) as f32 / 32768.0;
    }
    (out, rate)
}

fn write_wav_pcm16(path: &Path, buf: &StereoBuf, rate: u32) {
    let frames = buf.frames();
    let data_bytes = (frames * 4) as u32;
    let mut out = Vec::with_capacity(44 + frames * 4);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes()); // byte rate
    out.extend_from_slice(&4u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for frame in 0..frames {
        for ch in 0..2 {
            let v = (buf.channel(ch)[frame].clamp(-1.0, 1.0) * 32767.0) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    std::fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn stats(buf: &StereoBuf) -> (f32, f32) {
    let mut sum = 0.0f64;
    let mut peak = 0.0f32;
    let n = buf.frames().max(1);
    for ch in 0..2 {
        for &v in buf.channel(ch) {
            sum += (v as f64) * (v as f64);
            peak = peak.max(v.abs());
        }
    }
    (((sum / (2 * n) as f64).sqrt()) as f32, peak)
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: stems_demix <checkpoint.ckpt> <track.wav> <out_dir>");
        std::process::exit(2);
    }
    let out_dir = Path::new(&args[3]);
    std::fs::create_dir_all(out_dir).expect("create out dir");

    let (track, rate) = read_wav_pcm16(Path::new(&args[2]));
    println!("track: {} frames @ {rate} Hz ({:.1}s)", track.frames(), track.frames() as f64 / rate as f64);

    let load = std::time::Instant::now();
    let mut model = StemsModel::load(&args[1]).expect("load separator");
    println!("load+compile {:.2}s", load.elapsed().as_secs_f64());

    let run = std::time::Instant::now();
    let stems = demix_all(&mut model, &track, |done, total| {
        print!("\rchunk {done}/{total}");
        let _ = std::io::stdout().flush();
    })
    .expect("demix");
    let secs = run.elapsed().as_secs_f64();
    println!(
        "\ndemix {:.1}s for {:.1}s of audio = {:.2}x realtime",
        secs,
        track.frames() as f64 / rate as f64,
        (track.frames() as f64 / rate as f64) / secs
    );

    let (track_rms, track_peak) = stats(&track);
    println!("mix    rms {:>6.1} dB  peak {:>6.1} dB", db(track_rms), db(track_peak));
    for stem in Stem::ALL {
        let buf = &stems[stem.index()];
        let (rms, peak) = stats(buf);
        let name = match stem {
            Stem::Drums => "drums",
            Stem::Bass => "bass",
            Stem::Other => "other",
            Stem::Vocals => "vocals",
        };
        write_wav_pcm16(&out_dir.join(format!("{name}.wav")), buf, rate);
        println!("{name:<6} rms {:>6.1} dB  peak {:>6.1} dB", db(rms), db(peak));
    }

    // Residual: mix minus sum of stems — how much audio the model dropped.
    let mut resid_sum = 0.0f64;
    let n = track.frames();
    for ch in 0..2 {
        for i in 0..n {
            let s: f32 = (0..4).map(|k| stems[k].channel(ch)[i]).sum();
            let d = track.channel(ch)[i] - s;
            resid_sum += (d as f64) * (d as f64);
        }
    }
    let resid_rms = ((resid_sum / (2 * n) as f64).sqrt()) as f32;
    println!("residual (mix - sum of stems): {:.1} dB rms", db(resid_rms));
    println!("wrote 4 stems to {}", out_dir.display());
}
