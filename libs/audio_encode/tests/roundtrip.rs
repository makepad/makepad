//! Whole-stream round trips: encode with this crate, decode with the sibling
//! decoder, compare. The decoder is the oracle — it was itself validated
//! against CoreAudio — so agreement here means agreement with the world.

use makepad_audio_decode::vorbis;
use makepad_audio_encode::{encode_vorbis, EncodeOptions};

/// A music-shaped test signal: chords, a moving bass, hats-like noise bursts
/// and a transient every half second — enough spectral and temporal variety
/// to exercise every class book.
fn music(rate: u32, channels: u16, secs: f64) -> Vec<f32> {
    let frames = (rate as f64 * secs) as usize;
    let ch = channels as usize;
    let mut out = vec![0f32; frames * ch];
    let mut noise = 0x853c49e6748fea9bu64;
    let mut rng = move || {
        noise ^= noise >> 12;
        noise ^= noise << 25;
        noise ^= noise >> 27;
        ((noise.wrapping_mul(0x2545F4914F6CDD1D) >> 40) as f32 / 16_777_216.0) * 2.0 - 1.0
    };
    for f in 0..frames {
        let t = f as f64 / rate as f64;
        let bass_hz = 55.0 * (1.0 + ((t * 0.5).floor() % 4.0));
        let bass = (2.0 * std::f64::consts::PI * bass_hz * t).sin() * 0.30;
        let chord = [261.63, 329.63, 392.0, 523.25]
            .iter()
            .map(|&hz| (2.0 * std::f64::consts::PI * hz * t).sin())
            .sum::<f64>()
            * 0.08;
        let shimmer = (2.0 * std::f64::consts::PI * 6000.0 * t).sin()
            * 0.03
            * (2.0 * std::f64::consts::PI * 3.0 * t).sin().max(0.0);
        let hat_gate = if (t * 4.0).fract() < 0.08 { 1.0 } else { 0.0 };
        let kick_phase = (t * 2.0).fract();
        let kick = if kick_phase < 0.05 {
            ((2.0 * std::f64::consts::PI * 60.0 * kick_phase * 20.0).sin()
                * (1.0 - kick_phase / 0.05))
                * 0.5
        } else {
            0.0
        };
        for c in 0..ch {
            let pan = if ch == 1 { 1.0 } else if c == 0 { 0.8 } else { 1.2 };
            let hats = rng() * 0.05 * hat_gate as f32;
            out[f * ch + c] =
                ((bass + chord * pan + shimmer + kick) as f32 + hats).clamp(-1.0, 1.0);
        }
    }
    out
}

fn snr_db(reference: &[f32], got: &[f32]) -> f64 {
    assert_eq!(reference.len(), got.len());
    let mut signal = 0f64;
    let mut noise = 0f64;
    for (&r, &g) in reference.iter().zip(got.iter()) {
        signal += r as f64 * r as f64;
        noise += (r as f64 - g as f64) * (r as f64 - g as f64);
    }
    if noise == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (signal / noise).log10()
}

#[test]
fn stereo_music_round_trips_above_35_db() {
    let rate = 44_100u32;
    let pcm = music(rate, 2, 3.0);
    let ogg = encode_vorbis(rate, 2, &pcm, &EncodeOptions::default()).unwrap();
    let decoded = vorbis::decode_all(&ogg).unwrap();
    assert_eq!(decoded.rate, rate);
    assert_eq!(decoded.channels, 2);
    assert_eq!(
        decoded.pcm_interleaved_f32.len(),
        pcm.len(),
        "frame-exact round trip: {} vs {}",
        decoded.frames(),
        pcm.len() / 2
    );
    let snr = snr_db(&pcm, &decoded.pcm_interleaved_f32);
    let kbps = ogg.len() as f64 * 8.0 / 3.0 / 1000.0;
    println!("stereo 44.1k: snr {snr:.2} dB at {kbps:.0} kbit/s");
    assert!(snr > 35.0, "snr {snr:.2} dB");
}

#[test]
fn mono_48k_round_trips() {
    let rate = 48_000u32;
    let pcm = music(rate, 1, 2.0);
    let ogg = encode_vorbis(rate, 1, &pcm, &EncodeOptions { quality: 0.9, ..Default::default() }).unwrap();
    let decoded = vorbis::decode_all(&ogg).unwrap();
    assert_eq!(decoded.rate, rate);
    assert_eq!(decoded.channels, 1);
    assert_eq!(decoded.pcm_interleaved_f32.len(), pcm.len());
    let snr = snr_db(&pcm, &decoded.pcm_interleaved_f32);
    println!("mono 48k: snr {snr:.2} dB");
    assert!(snr > 35.0, "snr {snr:.2} dB");
}

#[test]
fn awkward_lengths_are_frame_exact() {
    // Not multiples of the hop; the end-trim granule must land exactly.
    for frames in [1usize, 100, 511, 512, 513, 1024, 1025, 5000] {
        let pcm: Vec<f32> =
            (0..frames * 2).map(|i| ((i as f32 * 0.01).sin()) * 0.5).collect();
        let ogg = encode_vorbis(44_100, 2, &pcm, &EncodeOptions::default()).unwrap();
        let decoded = vorbis::decode_all(&ogg).unwrap();
        assert_eq!(decoded.pcm_interleaved_f32.len(), pcm.len(), "{frames} frames");
    }
}

#[test]
fn digital_silence_is_tiny_and_exact() {
    let pcm = vec![0f32; 44_100 * 2 * 2];
    let ogg = encode_vorbis(44_100, 2, &pcm, &EncodeOptions::default()).unwrap();
    // Two seconds of silence should cost well under 4 KB (headers included).
    assert!(ogg.len() < 4096, "{} bytes", ogg.len());
    let decoded = vorbis::decode_all(&ogg).unwrap();
    assert_eq!(decoded.pcm_interleaved_f32.len(), pcm.len());
    assert!(decoded.pcm_interleaved_f32.iter().all(|&v| v == 0.0));
}

#[test]
fn output_is_deterministic_across_thread_counts() {
    let pcm = music(44_100, 2, 1.5);
    let one = encode_vorbis(
        44_100,
        2,
        &pcm,
        &EncodeOptions { threads: 1, ..Default::default() },
    )
    .unwrap();
    let many = encode_vorbis(
        44_100,
        2,
        &pcm,
        &EncodeOptions { threads: 4, ..Default::default() },
    )
    .unwrap();
    assert_eq!(one, many, "thread count changed the bytes");
}

#[test]
fn quality_moves_bitrate_and_snr_together() {
    let pcm = music(44_100, 2, 2.0);
    let lo = encode_vorbis(
        44_100,
        2,
        &pcm,
        &EncodeOptions { quality: 0.1, ..Default::default() },
    )
    .unwrap();
    let hi = encode_vorbis(
        44_100,
        2,
        &pcm,
        &EncodeOptions { quality: 0.9, ..Default::default() },
    )
    .unwrap();
    assert!(hi.len() > lo.len(), "hi {} vs lo {}", hi.len(), lo.len());
    let lo_snr = snr_db(&pcm, &vorbis::decode_all(&lo).unwrap().pcm_interleaved_f32);
    let hi_snr = snr_db(&pcm, &vorbis::decode_all(&hi).unwrap().pcm_interleaved_f32);
    println!(
        "q0.1: {:.0} kbps {lo_snr:.1} dB / q0.9: {:.0} kbps {hi_snr:.1} dB",
        lo.len() as f64 * 4.0 / 1000.0,
        hi.len() as f64 * 4.0 / 1000.0
    );
    assert!(hi_snr > lo_snr + 5.0, "quality knob inert: {lo_snr:.1} vs {hi_snr:.1}");
}

#[test]
fn tags_survive_the_container() {
    let pcm = vec![0.1f32; 44_100];
    let opts = EncodeOptions {
        tags: vec![
            ("TITLE".into(), "Stems Test".into()),
            ("ARTIST".into(), "Makepad".into()),
        ],
        ..Default::default()
    };
    let ogg = encode_vorbis(44_100, 1, &pcm, &opts).unwrap();
    let tags = makepad_audio_decode::read_tags(&ogg).unwrap();
    assert_eq!(tags.title.as_deref(), Some("Stems Test"));
    assert_eq!(tags.artist.as_deref(), Some("Makepad"));
}

#[test]
fn probe_duration_matches_without_decoding() {
    let rate = 44_100u32;
    let pcm = music(rate, 2, 2.5);
    let ogg = encode_vorbis(rate, 2, &pcm, &EncodeOptions::default()).unwrap();
    let secs = makepad_audio_decode::probe_duration(&ogg).unwrap();
    let want = pcm.len() as f64 / 2.0 / rate as f64;
    assert!((secs - want).abs() < 1e-9, "{secs} vs {want}");
}

#[test]
fn the_streaming_decoder_agrees_with_whole_file() {
    let pcm = music(44_100, 2, 1.0);
    let ogg = encode_vorbis(44_100, 2, &pcm, &EncodeOptions::default()).unwrap();
    let whole = vorbis::decode_all(&ogg).unwrap();
    let mut stream = vorbis::VorbisDecoder::new(&ogg).unwrap();
    let mut got = Vec::new();
    while let Some(block) = stream.next_block().unwrap() {
        got.extend_from_slice(block);
    }
    assert_eq!(got, whole.pcm_interleaved_f32);
}
