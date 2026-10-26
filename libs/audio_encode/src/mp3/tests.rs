use super::*;
use makepad_audio_decode::{mp3 as dec, read_tags};

fn tone(rate: u32, secs: f32, channels: usize, mut f: impl FnMut(f32, usize) -> f32) -> Vec<f32> {
    let n = (rate as f32 * secs) as usize;
    let mut v = Vec::with_capacity(n * channels);
    for i in 0..n {
        let t = i as f32 / rate as f32;
        for c in 0..channels {
            v.push(f(t, c));
        }
    }
    v
}

/// Decoded channel `c` aligned to the input: the lag (0..3000 samples) with
/// the best correlation, and the SNR in dB over the input's middle.
fn compare(input: &[f32], channels: usize, decoded: &makepad_audio_decode::DecodedAudio, c: usize) -> (usize, f64, f64) {
    let x: Vec<f32> = input.iter().skip(c).step_by(channels).copied().collect();
    let y = decoded.channel(c);
    let (a, b) = (x.len() / 4, x.len() * 3 / 4);
    let mut best = (0usize, f64::MIN);
    for lag in 0..3000 {
        if b + lag >= y.len() {
            break;
        }
        let s: f64 = (a..b).step_by(3).map(|i| x[i] as f64 * y[i + lag] as f64).sum();
        if s > best.1 {
            best = (lag, s);
        }
    }
    let lag = best.0;
    let (mut sig, mut err, mut out) = (0.0f64, 0.0f64, 0.0f64);
    for i in a..b {
        let (u, v) = (x[i] as f64, y[i + lag] as f64);
        sig += u * u;
        out += v * v;
        err += (u - v) * (u - v);
    }
    (lag, 10.0 * (sig / err.max(1e-30)).log10(), (out / sig.max(1e-30)).sqrt())
}

#[test]
fn sine_round_trips_through_the_decoder() {
    let rate = 44_100;
    let pcm = tone(rate, 2.0, 2, |t, c| 0.5 * (2.0 * std::f32::consts::PI * if c == 0 { 1000.0 } else { 440.0 } * t).sin());
    let mp3 = encode_mp3(rate, 2, &pcm, &Mp3Options::default()).unwrap();
    let decoded = dec::decode_all(&mp3).unwrap();
    assert_eq!(decoded.rate, rate);
    assert_eq!(decoded.channels, 2);
    assert!(decoded.frames() >= pcm.len() / 2, "decoded {} < input {}", decoded.frames(), pcm.len() / 2);
    for c in 0..2 {
        let (lag, snr, gain) = compare(&pcm, 2, &decoded, c);
        eprintln!("channel {c}: lag {lag} snr {snr:.1} dB gain {gain:.3}");
        assert!(snr > 25.0, "channel {c} snr {snr}");
        assert!((gain - 1.0).abs() < 0.05, "channel {c} gain {gain}");
    }
}

/// Tones, noise and sharp percussive hits (which take short blocks), at a
/// realistic level: the decoder must take every frame and the noise must stay
/// well below the signal.
fn music(rate: u32, secs: f32) -> Vec<f32> {
    let mut seed = 0x1234_5678u32;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    tone(rate, secs, 2, |t, c| {
        let beat = (t * 2.0).fract();
        let hit = if beat < 0.08 { (1.0 - beat / 0.08) * 0.4 * noise() } else { 0.0 };
        let chord = [220.0, 277.18, 329.63, 440.0, 1318.5]
            .iter()
            .enumerate()
            .map(|(k, f)| (2.0 * std::f32::consts::PI * f * t + c as f32 * k as f32).sin() * 0.08)
            .sum::<f32>();
        chord + hit + 0.01 * noise()
    })
}

#[test]
fn music_round_trips_with_short_blocks() {
    let rate = 48_000;
    let pcm = music(rate, 3.0);
    let mp3 = encode_mp3(rate, 2, &pcm, &Mp3Options::default()).unwrap();
    let decoded = dec::decode_all(&mp3).unwrap();
    assert_eq!(decoded.rate, rate);
    let secs = pcm.len() as f64 / 2.0 / rate as f64;
    let kbps = mp3.len() as f64 * 8.0 / secs / 1000.0;
    eprintln!("{} bytes, {kbps:.1} kbit/s", mp3.len());
    assert!((kbps - 192.0).abs() < 8.0, "bitrate {kbps}");
    for c in 0..2 {
        let (lag, snr, gain) = compare(&pcm, 2, &decoded, c);
        eprintln!("channel {c}: lag {lag} snr {snr:.1} dB gain {gain:.3}");
        assert!(snr > 12.0, "channel {c} snr {snr}");
    }
}

#[test]
fn output_is_the_same_for_any_thread_count() {
    let pcm = music(44_100, 2.0);
    let one = encode_mp3(44_100, 2, &pcm, &Mp3Options { threads: 1, ..Default::default() }).unwrap();
    let many = encode_mp3(44_100, 2, &pcm, &Mp3Options { threads: 5, ..Default::default() }).unwrap();
    assert!(one == many);
}

#[test]
fn mono_silence_and_odd_rates_encode() {
    let silent = encode_mp3(44_100, 1, &vec![0.0; 44_100], &Mp3Options::default()).unwrap();
    let d = dec::decode_all(&silent).unwrap();
    assert_eq!(d.channels, 1);
    assert!(d.pcm_interleaved_f32.iter().all(|v| v.abs() < 1e-6));
    let empty = encode_mp3(44_100, 2, &[], &Mp3Options::default()).unwrap();
    assert!(dec::decode_all(&empty).is_ok());
    // 22.05 kHz is resampled to 44.1
    let pcm = tone(22_050, 1.0, 1, |t, _| 0.5 * (2.0 * std::f32::consts::PI * 500.0 * t).sin());
    let mp3 = encode_mp3(22_050, 1, &pcm, &Mp3Options::default()).unwrap();
    let d = dec::decode_all(&mp3).unwrap();
    assert_eq!(d.rate, 44_100);
    let peak = d.pcm_interleaved_f32.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!((peak - 0.5).abs() < 0.08, "peak {peak}");
    // six channels keep the front pair
    let pcm = tone(48_000, 0.5, 6, |_, c| if c < 2 { 0.1 } else { 0.9 });
    let d = dec::decode_all(&encode_mp3(48_000, 6, &pcm, &Mp3Options::default()).unwrap()).unwrap();
    assert_eq!(d.channels, 2);
    assert_eq!(encode_mp3(44_100, 2, &pcm, &Mp3Options { bitrate_kbps: 100, threads: 0 }), Err(Mp3Error::UnsupportedBitrate(100)));
}

#[test]
fn every_bitrate_and_rate_decodes() {
    for rate in [32_000, 44_100, 48_000] {
        let pcm = music(rate, 1.0);
        for kbps in [128, 160, 192, 256, 320] {
            let mp3 = encode_mp3(rate, 2, &pcm, &Mp3Options { bitrate_kbps: kbps, threads: 0 }).unwrap();
            let d = dec::decode_all(&mp3).unwrap();
            let (_, snr, _) = compare(&pcm, 2, &d, 0);
            eprintln!("{rate} Hz {kbps} kbit/s: snr {snr:.1}");
            assert!(snr > 8.0, "{rate} {kbps}: {snr}");
        }
    }
}

#[test]
fn id3_tags_are_read_back() {
    let tags = id3::Id3Tags {
        title: Some("Night Drive · Ünïcode".into()),
        artist: Some("AI · model".into()),
        album: Some("AI music".into()),
        lyrics: Some("line one\nline two".into()),
        lyrics_language: Some("eng".into()),
        ..Default::default()
    };
    let mp3 = encode_mp3_tagged(44_100, 2, &music(44_100, 0.5), &tags, &Mp3Options::default()).unwrap();
    let read = read_tags(&mp3).unwrap();
    assert_eq!(read.title.as_deref(), Some("Night Drive · Ünïcode"));
    assert_eq!(read.artist.as_deref(), Some("AI · model"));
    assert_eq!(read.album.as_deref(), Some("AI music"));
    // USLT: encoding, "eng", empty descriptor, UTF-16 text
    let at = mp3.windows(4).position(|w| w == b"USLT").unwrap();
    assert_eq!(&mp3[at + 11..at + 14], b"eng");
    assert!(dec::decode_all(&mp3).is_ok());
}
