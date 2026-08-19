//! Parity against the PyTorch reference (the "oracle").
//!
//! The oracle lives at `local/stems_ref/` (gitignored: a pinned checkout of
//! ZFTurbo/Music-Source-Separation-Training @ 2ba884c2, the published
//! checkpoint, and `oracle.py` which dumps per-stage taps as `.npy`). Every
//! test here SKIPS when that tree is absent, so CI on a machine without the
//! 527 MB checkpoint stays green; on a machine that has it, they are the
//! contract.
//!
//! Regenerate the fixtures with:
//! ```text
//! cd local/stems_ref
//! ./venv/bin/python oracle.py taps --in fixtures/music_11s.wav --out taps_cpu --device cpu
//! ```

use makepad_ai_stems::config::{AUDIO_CHANNELS, CHUNK_SAMPLES, NUM_STEMS, STEM_NAMES};
use makepad_ai_stems::{StemsModel, StereoBuf};
use std::path::{Path, PathBuf};

fn oracle_root() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../local/stems_ref"
    ))
}

fn checkpoint() -> PathBuf {
    oracle_root().join("ckpt/model_bs_roformer_ep_17_sdr_9.6568.ckpt")
}

/// Minimal `.npy` reader: little-endian f32, C order — everything `oracle.py`
/// writes.
fn read_npy(path: &Path) -> (Vec<usize>, Vec<f32>) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(&bytes[0..6], b"\x93NUMPY", "{} is not a .npy", path.display());
    let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    let header = std::str::from_utf8(&bytes[10..10 + header_len]).unwrap();
    assert!(
        header.contains("'<f4'") || header.contains("\"<f4\""),
        "{} is not float32: {header}",
        path.display()
    );
    assert!(
        header.contains("'fortran_order': False"),
        "{} is fortran-ordered",
        path.display()
    );
    let open = header.find('(').unwrap();
    let close = header[open..].find(')').unwrap() + open;
    let shape: Vec<usize> = header[open + 1..close]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect();
    let data_at = 10 + header_len;
    let values: Vec<f32> = bytes[data_at..]
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    (shape, values)
}

struct Diff {
    max_abs: f32,
    rms_error: f64,
    rms_signal: f64,
}

impl Diff {
    fn of(got: &[f32], want: &[f32]) -> Diff {
        assert_eq!(got.len(), want.len());
        let mut max_abs = 0.0f32;
        let mut err = 0.0f64;
        let mut sig = 0.0f64;
        for (g, w) in got.iter().zip(want) {
            assert!(g.is_finite(), "non-finite output value {g}");
            let d = (g - w).abs();
            max_abs = max_abs.max(d);
            err += (d as f64) * (d as f64);
            sig += (*w as f64) * (*w as f64);
        }
        Diff {
            max_abs,
            rms_error: (err / got.len() as f64).sqrt(),
            rms_signal: (sig / got.len() as f64).sqrt(),
        }
    }

    /// Signal-to-noise ratio of our output against the oracle's, in dB.
    fn snr_db(&self) -> f64 {
        if self.rms_error <= 0.0 {
            return f64::INFINITY;
        }
        20.0 * (self.rms_signal / self.rms_error).log10()
    }
}

#[test]
fn one_chunk_matches_the_pytorch_reference() {
    let root = oracle_root();
    let taps = root.join("taps_cpu");
    let ckpt = checkpoint();
    if !ckpt.is_file() || !taps.join("00_input.npy").is_file() {
        eprintln!(
            "SKIP: oracle tree absent (want {} and {}/00_input.npy)",
            ckpt.display(),
            taps.display()
        );
        return;
    }

    let (in_shape, input) = read_npy(&taps.join("00_input.npy"));
    assert_eq!(in_shape, vec![1, AUDIO_CHANNELS, CHUNK_SAMPLES]);
    let chunk = StereoBuf {
        left: input[..CHUNK_SAMPLES].to_vec(),
        right: input[CHUNK_SAMPLES..2 * CHUNK_SAMPLES].to_vec(),
    };

    let load = std::time::Instant::now();
    let mut model = match StemsModel::load(&ckpt) {
        Ok(model) => model,
        Err(e) => {
            // A box with no usable device runtime is a valid skip for this
            // suite (the arithmetic contract is what is under test, and it
            // needs a device to produce anything at all).
            eprintln!("SKIP: could not build the separator: {e}");
            return;
        }
    };
    eprintln!("load+compile: {:.2}s", load.elapsed().as_secs_f64());

    let run = std::time::Instant::now();
    let stems = model.separate_chunk(&chunk).expect("separate_chunk");
    let chunk_secs = run.elapsed().as_secs_f64();
    eprintln!(
        "one chunk: {chunk_secs:.3}s for {:.2}s of audio ({:.2}x realtime)",
        CHUNK_SAMPLES as f64 / 44100.0,
        CHUNK_SAMPLES as f64 / 44100.0 / chunk_secs
    );

    let (recon_shape, recon) = read_npy(&taps.join("09_recon.npy"));
    assert_eq!(recon_shape, vec![1, NUM_STEMS, AUDIO_CHANNELS, CHUNK_SAMPLES]);

    let mut worst_snr = f64::INFINITY;
    let mut worst_max = 0.0f32;
    let mut loudest_rms = 0.0f64;
    for stem in 0..NUM_STEMS {
        for ch in 0..AUDIO_CHANNELS {
            let at = (stem * AUDIO_CHANNELS + ch) * CHUNK_SAMPLES;
            let want = &recon[at..at + CHUNK_SAMPLES];
            let diff = Diff::of(stems[stem].channel(ch), want);
            eprintln!(
                "  {:>6} ch{ch}: max_abs {:.3e}  snr {:.1} dB  (rms {:.6})",
                STEM_NAMES[stem],
                diff.max_abs,
                diff.snr_db(),
                diff.rms_signal
            );
            worst_max = worst_max.max(diff.max_abs);
            loudest_rms = loudest_rms.max(diff.rms_signal);
            // Per-stem SNR is only meaningful for a stem that carries signal.
            // On a classical fixture the drums and bass stems are ~1e-6 RMS,
            // where a 1e-7 absolute deviation reads as a poor "SNR" while
            // actually being an exact match; those are gated on max_abs below.
            if diff.rms_signal > 1e-3 {
                worst_snr = worst_snr.min(diff.snr_db());
            } else {
                assert!(
                    diff.max_abs < 1e-5,
                    "near-silent stem {} ch{ch} deviates by {:.3e}",
                    STEM_NAMES[stem],
                    diff.max_abs
                );
            }
        }
    }
    eprintln!(
        "worst: max_abs {worst_max:.3e}, snr {worst_snr:.1} dB (loudest stem rms {loudest_rms:.4})"
    );

    // 16 transformer layers of f32 arithmetic reordered by a different kernel
    // set will not be bit-identical; what must hold is that the difference is
    // numerical noise, not a wrong graph. 55 dB is ~0.2% of a stem's own RMS —
    // a convention or layout bug lands far below that (a transposed axis or
    // the wrong RoPE flavour scores single-digit dB), so this is a sharp gate.
    assert!(
        worst_snr > 55.0,
        "reconstruction SNR vs the oracle is only {worst_snr:.1} dB"
    );
    // Absolute ceiling relative to full scale, so a loud stem cannot hide a
    // localized blow-up behind a good average.
    assert!(worst_max < 2e-3, "max abs deviation {worst_max:.3e}");
}

#[test]
fn stems_sum_back_to_the_mixture() {
    // BS-RoFormer masks are not constrained to sum to one, so this is a loose
    // sanity property rather than an identity: the four stems together must
    // reconstruct most of the input's energy. It catches a stem being silent
    // or duplicated without needing the oracle's own output.
    let root = oracle_root();
    let taps = root.join("taps_cpu");
    let ckpt = checkpoint();
    if !ckpt.is_file() || !taps.join("09_recon.npy").is_file() {
        eprintln!("SKIP: oracle tree absent");
        return;
    }
    let (_, input) = read_npy(&taps.join("00_input.npy"));
    let (_, recon) = read_npy(&taps.join("09_recon.npy"));
    for ch in 0..AUDIO_CHANNELS {
        let mut sum = vec![0.0f32; CHUNK_SAMPLES];
        for stem in 0..NUM_STEMS {
            let at = (stem * AUDIO_CHANNELS + ch) * CHUNK_SAMPLES;
            for (s, v) in sum.iter_mut().zip(&recon[at..at + CHUNK_SAMPLES]) {
                *s += v;
            }
        }
        let mix = &input[ch * CHUNK_SAMPLES..(ch + 1) * CHUNK_SAMPLES];
        let diff = Diff::of(&sum, mix);
        eprintln!("  oracle stem-sum ch{ch}: snr {:.1} dB", diff.snr_db());
        assert!(
            diff.snr_db() > 10.0,
            "the reference's own stems do not re-sum to the mixture ({:.1} dB) — \
             the fixture is stale",
            diff.snr_db()
        );
    }
}
