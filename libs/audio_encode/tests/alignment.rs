//! Alignment canary: the whole-chain round trip must land at shift ZERO.
//!
//! This is the test that caught the single-page granule bug (the decoder
//! read end-truncation as encoder delay and the whole stream came back 376
//! samples early). It cross-correlates the decoded signal against the
//! input over a wide window and asserts the best alignment is exactly 0.

use makepad_audio_decode::vorbis;
use makepad_audio_encode::{encode_vorbis, EncodeOptions};

fn tone_mix(rate: u32, secs: f64) -> Vec<f32> {
    let frames = (rate as f64 * secs) as usize;
    (0..frames)
        .map(|f| {
            let t = f as f64 / rate as f64;
            let bass = (2.0 * std::f64::consts::PI * 110.0 * t).sin() * 0.30;
            let mid = (2.0 * std::f64::consts::PI * 880.0 * t).sin() * 0.20;
            let high = (2.0 * std::f64::consts::PI * 6100.0 * t).sin() * 0.05;
            (bass + mid + high) as f32
        })
        .collect()
}

#[test]
fn error_profile_by_quality() {
    let rate = 44_100u32;
    let pcm = tone_mix(rate, 2.0);
    for q in [0.1f32, 0.5, 0.9] {
        let ogg =
            encode_vorbis(rate, 1, &pcm, &EncodeOptions { quality: q, ..Default::default() })
                .unwrap();
        let dec = vorbis::decode_all(&ogg).unwrap();
        let got = &dec.pcm_interleaved_f32;
        assert_eq!(got.len(), pcm.len());
        // SNR over windows: find the worst second.
        let win = rate as usize / 10;
        let mut worst = f64::INFINITY;
        let mut worst_at = 0usize;
        for (i, (r, g)) in pcm.chunks(win).zip(got.chunks(win)).enumerate() {
            let s: f64 = r.iter().map(|&v| v as f64 * v as f64).sum();
            let n: f64 =
                r.iter().zip(g).map(|(&a, &b)| (a as f64 - b as f64).powi(2)).sum();
            if n > 0.0 && s > 1e-9 {
                let snr = 10.0 * (s / n).log10();
                if snr < worst {
                    worst = snr;
                    worst_at = i;
                }
            }
        }
        let s: f64 = pcm.iter().map(|&v| v as f64 * v as f64).sum();
        let n: f64 =
            pcm.iter().zip(got).map(|(&a, &b)| (a as f64 - b as f64).powi(2)).sum();
        // Is the error a constant time shift? Search small offsets.
        let mut best = (0isize, f64::NEG_INFINITY);
        for off in -1200isize..=1200 {
            let mut s2 = 0f64;
            let mut n2 = 0f64;
            for i in 2000..pcm.len() - 2000 {
                let g = got[(i as isize + off) as usize] as f64;
                let r = pcm[i] as f64;
                s2 += r * r;
                n2 += (r - g) * (r - g);
            }
            let snr = 10.0 * (s2 / n2.max(1e-12)).log10();
            if snr > best.1 {
                best = (off, snr);
            }
        }
        println!(
            "q={q}: {:.0} kbps, snr {:.1} dB, worst window {:.1} dB at {}ms, best-shift {} -> {:.1} dB",
            ogg.len() as f64 * 8.0 / 2.0 / 1000.0,
            10.0 * (s / n).log10(),
            worst,
            worst_at * 100,
            best.0,
            best.1
        );
        assert_eq!(best.0, 0, "decoded stream is time-shifted by {} samples", best.0);
    }
}
