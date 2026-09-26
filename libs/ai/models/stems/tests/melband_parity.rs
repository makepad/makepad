//! Parity of the vocals model against a reference forward (the "oracle").
//!
//! The oracle is a float64 NumPy forward written from the MIT reference
//! implementation, which dumps every stage of one 8-second chunk as `.npy`:
//!
//! ```text
//! 00_input        [2, 352800]      channel, sample
//! 01_stft         [2050, 801, 2]   row = bin*2 + channel; re, im
//! 02_band_split   [801, 60, 384]
//! 03_layer_{0..5} [801, 60, 384]   after that block's time then freq transformer
//! 04_masks        [2050, 801, 2]   averaged over the bands covering each row
//! 06_output       [2, 352800]      the vocal estimate
//! ```
//!
//! Neither the 913 MB checkpoint nor the taps are in the repository. They are
//! looked for at `MAKEPAD_MELBAND_CKPT` (default
//! `local/stems_ref/ckpt/MelBandRoformer.ckpt`) and `MAKEPAD_MELBAND_TAPS`
//! (default `local/melband_ref/taps`), and every test here SKIPS when one is
//! absent or the machine has no device runtime, so a machine without them
//! stays green. On a machine that has all three they are the contract: a
//! separator that then fails to build is a FAILURE, not a skip -- a renamed
//! tensor or a wrong extent would otherwise turn every gate here green.

use makepad_ai_common::backend::DeviceRuntime;
use makepad_ai_stems::config::{AUDIO_CHANNELS, DIM, FEATURES, FREQ_BINS};
use makepad_ai_stems::melband::config::{
    band_feature_offset, band_mask_offset, band_width, BAND_FEATURES, DEPTH, NUM_BANDS,
};
use makepad_ai_stems::melband::{instrumental, Stage, StageProbe, CHUNK};
use makepad_ai_stems::model::feature_index;
use makepad_ai_stems::stft::Stft;
use makepad_ai_stems::{demix_all_lanes, ChunkSeparator, StereoBuf, VocalsModel};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One model on the device at a time: the tests of this file run on parallel
/// threads, and two compiled graphs of this size need not fit side by side.
static DEVICE: Mutex<()> = Mutex::new(());

fn located(var: &str, default: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(format!("{}/../../../../{default}", env!("CARGO_MANIFEST_DIR"))),
    }
}

fn checkpoint() -> PathBuf {
    located("MAKEPAD_MELBAND_CKPT", "local/stems_ref/ckpt/MelBandRoformer.ckpt")
}

fn taps() -> PathBuf {
    located("MAKEPAD_MELBAND_TAPS", "local/melband_ref/taps")
}

/// The fixture paths, or `None` (having said why) when the test must skip.
fn fixtures(needs_checkpoint: bool) -> Option<(PathBuf, PathBuf)> {
    let ckpt = checkpoint();
    let taps = taps();
    if needs_checkpoint && !ckpt.is_file() {
        eprintln!("SKIP: checkpoint absent (want {})", ckpt.display());
        return None;
    }
    if !taps.join("00_input.npy").is_file() {
        eprintln!("SKIP: reference taps absent (want {}/00_input.npy)", taps.display());
        return None;
    }
    Some((ckpt, taps))
}

/// Whether this machine can run a graph at all. The one failure of a load
/// that is a reason to skip; every other one is the port's.
fn device_present() -> bool {
    match DeviceRuntime::new() {
        Ok(_) => true,
        Err(e) => {
            eprintln!("SKIP: no device runtime on this machine: {e}");
            false
        }
    }
}

/// Minimal `.npy` reader: little-endian f32, C order — everything the oracle
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
    assert_eq!(
        values.len(),
        shape.iter().product::<usize>(),
        "{} is truncated",
        path.display()
    );
    (shape, values)
}

fn read_input(taps: &Path) -> StereoBuf {
    let (shape, input) = read_npy(&taps.join("00_input.npy"));
    assert_eq!(shape, vec![AUDIO_CHANNELS, CHUNK.samples]);
    StereoBuf {
        left: input[..CHUNK.samples].to_vec(),
        right: input[CHUNK.samples..].to_vec(),
    }
}

/// A `[2050, frames, 2]` tap (row = bin*2 + channel; re, im) in the crate's
/// own `[4100, frames]` frame-major feature order.
fn tap_as_features(tap: &[f32], frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; FEATURES * frames];
    for bin in 0..FREQ_BINS {
        for ch in 0..AUDIO_CHANNELS {
            let row = bin * AUDIO_CHANNELS + ch;
            for frame in 0..frames {
                for c in 0..2 {
                    out[frame * FEATURES + feature_index(bin, ch, c)] =
                        tap[(row * frames + frame) * 2 + c];
                }
            }
        }
    }
    out
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

    /// Signal-to-noise ratio of our values against the oracle's, in dB.
    fn snr_db(&self) -> f64 {
        if self.rms_error <= 0.0 {
            return f64::INFINITY;
        }
        20.0 * (self.rms_signal / self.rms_error).log10()
    }
}

/// The SNR of each band of a `[frame][band][384]` trunk stage against the
/// oracle: a fault in one band's weights or offsets shows here long before
/// it moves the figure for the whole stage.
fn band_snrs(got: &[f32], want: &[f32], frames: usize) -> Vec<f64> {
    let mut snrs = Vec::with_capacity(NUM_BANDS);
    for band in 0..NUM_BANDS {
        let mut err = 0.0f64;
        let mut sig = 0.0f64;
        for frame in 0..frames {
            let at = (frame * NUM_BANDS + band) * DIM;
            for (g, w) in got[at..at + DIM].iter().zip(&want[at..at + DIM]) {
                let d = (g - w) as f64;
                err += d * d;
                sig += (*w as f64) * (*w as f64);
            }
        }
        snrs.push(if err > 0.0 { 10.0 * (sig / err).log10() } else { f64::INFINITY });
    }
    snrs
}

/// The band of `bands` that agrees least, and its SNR.
fn worst_of(snrs: &[f64], bands: std::ops::Range<usize>) -> (usize, f64) {
    bands
        .map(|band| (band, snrs[band]))
        .fold((0, f64::INFINITY), |worst, this| if this.1 < worst.1 { this } else { worst })
}

#[test]
fn one_chunk_matches_the_reference_forward() {
    let Some((ckpt, taps)) = fixtures(true) else {
        return;
    };
    let chunk = read_input(&taps);
    let frames = CHUNK.frames;

    let _device = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
    if !device_present() {
        return;
    }
    let load = std::time::Instant::now();
    let mut model = VocalsModel::load(&ckpt).expect("the separator builds from its checkpoint");
    eprintln!("load+compile: {:.2}s", load.elapsed().as_secs_f64());

    let run = std::time::Instant::now();
    let vocals = model.separate_chunk(&chunk).expect("separate_chunk");
    let chunk_secs = run.elapsed().as_secs_f64();
    eprintln!(
        "one chunk: {chunk_secs:.3}s for {:.2}s of audio ({:.2}x realtime)",
        CHUNK.secs(),
        CHUNK.secs() / chunk_secs
    );

    // -- the averaged mask, the last thing before the transforms take over --
    let (mask_shape, mask_tap) = read_npy(&taps.join("04_masks.npy"));
    assert_eq!(mask_shape, vec![FREQ_BINS * AUDIO_CHANNELS, frames, 2]);
    let mask = Diff::of(model.last_mask(), &tap_as_features(&mask_tap, frames));
    eprintln!(
        "  masks: max_abs {:.3e}  snr {:.1} dB  (rms {:.6})",
        mask.max_abs,
        mask.snr_db(),
        mask.rms_signal
    );

    // -- the vocal --
    let (out_shape, output) = read_npy(&taps.join("06_output.npy"));
    assert_eq!(out_shape, vec![AUDIO_CHANNELS, CHUNK.samples]);
    let mut worst_snr = f64::INFINITY;
    let mut worst_max = 0.0f32;
    for ch in 0..AUDIO_CHANNELS {
        let want = &output[ch * CHUNK.samples..(ch + 1) * CHUNK.samples];
        let diff = Diff::of(vocals.channel(ch), want);
        eprintln!(
            "  vocals ch{ch}: max_abs {:.3e}  snr {:.1} dB  (rms {:.6})",
            diff.max_abs,
            diff.snr_db(),
            diff.rms_signal
        );
        assert!(
            diff.rms_signal > 1e-3,
            "the fixture's vocal is near silent on ch{ch}; an SNR against it says nothing"
        );
        worst_snr = worst_snr.min(diff.snr_db());
        worst_max = worst_max.max(diff.max_abs);
    }

    // Twelve transformers of f32 arithmetic on a different kernel set, held
    // against a float64 forward, will not agree to the bit; what must hold is
    // that the difference is numerical noise and not a wrong graph. A layout
    // or convention bug (a band offset in the wrong unit, the GLU halves
    // swapped, a missing output norm, the wrong RoPE flavour) scores
    // single-digit dB. The subtle ones score far higher -- a DC bin left
    // in on one frame of 801 still reaches some 68 dB -- so the gates sit
    // close under what the port achieves on this excerpt (masks 89.9 dB,
    // vocal 119.6 dB), not close above what a gross fault scores.
    assert!(
        mask.snr_db() >= 75.0,
        "mask SNR vs the oracle is only {:.1} dB",
        mask.snr_db()
    );
    assert!(
        worst_snr >= 100.0,
        "vocal SNR vs the oracle is only {worst_snr:.1} dB"
    );
    // Absolute ceiling relative to full scale, so a loud passage cannot hide
    // a localized blow-up behind a good average.
    assert!(worst_max < 2e-3, "max abs deviation {worst_max:.3e}");
}

#[test]
fn every_stage_matches_the_reference_forward() {
    let Some((ckpt, taps)) = fixtures(true) else {
        return;
    };
    let chunk = read_input(&taps);
    let frames = CHUNK.frames;

    // -- the transform, which needs no device --
    let (stft_shape, stft_tap) = read_npy(&taps.join("01_stft.npy"));
    assert_eq!(stft_shape, vec![FREQ_BINS * AUDIO_CHANNELS, frames, 2]);
    let stft = Stft::bs_roformer();
    for ch in 0..AUDIO_CHANNELS {
        let (spec, got_frames) = stft.forward(chunk.channel(ch));
        assert_eq!(got_frames, frames);
        let mut want = vec![0.0f32; FREQ_BINS * frames * 2];
        for bin in 0..FREQ_BINS {
            let row = bin * AUDIO_CHANNELS + ch;
            let from = row * frames * 2;
            let to = bin * frames * 2;
            want[to..to + frames * 2].copy_from_slice(&stft_tap[from..from + frames * 2]);
        }
        let diff = Diff::of(&spec, &want);
        eprintln!("  stft ch{ch}: max_abs {:.3e}  snr {:.1} dB", diff.max_abs, diff.snr_db());
        assert!(diff.snr_db() >= 100.0, "stft ch{ch} is only {:.1} dB", diff.snr_db());
    }

    // -- the graph, one stage at a time; the first that falls short names
    //    the part that is wrong --
    //
    // One floor serves every stage. Each band is normalised on its own, so a
    // band with next to nothing in it (the top two or three, above 17 kHz, on
    // most programme material) is scaled up to unit level together with
    // whatever rounding the transform left there: the production STFT runs
    // in f32 and builds its window in f32 as torch does, where the oracle
    // does both in float64. On such a band the two forwards part at some 55-75 dB
    // while every other band agrees past 100, and the stage as a whole lands
    // between. A wrong layout or convention lands in single digits, so 60 dB
    // still tells the two apart with room on both sides.
    const STAGE_FLOOR_DB: f64 = 60.0;
    // The whole-stage figure is an average over sixty bands, and an average
    // hides one band that is wrong by a little: so each band is held to a
    // floor of its own. On this excerpt bands 0-56 agree to 88-137 dB at
    // every stage and the three above them to 56-86 dB, for the reason
    // given above; both floors sit some 6-8 dB under the worst observed.
    const QUIET_BANDS_FROM: usize = 57;
    const BODY_BAND_FLOOR_DB: f64 = 80.0;
    const QUIET_BAND_FLOOR_DB: f64 = 50.0;
    let _device = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
    if !device_present() {
        return;
    }
    let mut stages = vec![(Stage::BandSplit, "02_band_split".to_string())];
    for block in 0..DEPTH {
        stages.push((Stage::Layer(block), format!("03_layer_{block}")));
    }
    for (stage, name) in stages {
        let mut probe =
            StageProbe::load(&ckpt, CHUNK, stage).expect("the stage builds from its checkpoint");
        let got = probe.run(&chunk).expect("run stage");
        let (shape, want) = read_npy(&taps.join(format!("{name}.npy")));
        assert_eq!(shape, vec![frames, NUM_BANDS, DIM]);
        let diff = Diff::of(&got, &want);
        let snrs = band_snrs(&got, &want, frames);
        let (body_band, body_db) = worst_of(&snrs, 0..QUIET_BANDS_FROM);
        let (top_band, top_db) = worst_of(&snrs, QUIET_BANDS_FROM..NUM_BANDS);
        eprintln!(
            "  {name}: max_abs {:.3e}  snr {:.1} dB  (rms {:.6}); worst band below \
             {QUIET_BANDS_FROM}: {body_band} at {body_db:.1} dB; from it up: {top_band} at \
             {top_db:.1} dB",
            diff.max_abs,
            diff.snr_db(),
            diff.rms_signal
        );
        assert!(
            diff.snr_db() >= STAGE_FLOOR_DB,
            "{name} is only {:.1} dB against the oracle",
            diff.snr_db()
        );
        assert!(
            body_db >= BODY_BAND_FLOOR_DB,
            "{name}: band {body_band} is only {body_db:.1} dB against the oracle"
        );
        assert!(
            top_db >= QUIET_BAND_FLOOR_DB,
            "{name}: band {top_band} is only {top_db:.1} dB against the oracle"
        );
    }

    // The band masks have no tap of their own: the oracle keeps only their
    // average. But a bin that a single band covers has that band's mask for
    // its average, and the lowest four bins and the highest 67 are such
    // bins, of the first band and of the last. So the two ends of the
    // band-mask vector can be held against the averaged tap directly, with
    // none of this crate's averaging in between.
    let mut probe = StageProbe::load(&ckpt, CHUNK, Stage::Masks).expect("build the mask stage");
    let band_masks = probe.run(&chunk).expect("run masks");
    assert_eq!(band_masks.len(), BAND_FEATURES * frames);
    let (_, mask_tap) = read_npy(&taps.join("04_masks.npy"));
    let averaged = tap_as_features(&mask_tap, frames);
    let last = NUM_BANDS - 1;
    let last_alone = band_feature_offset(last - 1) + band_width(last - 1);
    let ends = [
        ("first band", 0..band_feature_offset(1), 0),
        (
            "last band",
            last_alone..FEATURES,
            band_mask_offset(last) + last_alone - band_feature_offset(last),
        ),
    ];
    for (what, features, band_mask_at) in ends {
        let mut got = Vec::new();
        let mut want = Vec::new();
        for frame in 0..frames {
            let from = frame * BAND_FEATURES + band_mask_at;
            got.extend_from_slice(&band_masks[from..from + features.len()]);
            want.extend_from_slice(
                &averaged[frame * FEATURES + features.start..frame * FEATURES + features.end],
            );
        }
        let diff = Diff::of(&got, &want);
        eprintln!(
            "  band masks, {what} alone over {} features: max_abs {:.3e}  snr {:.1} dB",
            features.len(),
            diff.max_abs,
            diff.snr_db()
        );
        assert!(
            diff.snr_db() >= STAGE_FLOOR_DB,
            "the {what}'s mask is only {:.1} dB against the oracle",
            diff.snr_db()
        );
    }
}

/// The model through the crate's streaming overlap-add, against the
/// reference's chunked inference written out here in the plainest way: the
/// whole track reflect-padded by one border, chunks one step apart, a short
/// chunk reflect-padded when more than half of it is audio and zero-padded
/// otherwise, linear fades of a tenth of a chunk that the first chunk's head
/// and the last chunk's tail do without, and the sum divided by the sum of
/// the windows. The cursor is proven against a batch loop with stand-in
/// separators elsewhere; this is the real separator, on its own geometry,
/// over a track whose last two chunks take one tail rule each.
#[test]
fn a_track_of_several_chunks_is_the_reference_overlap_add() {
    let Some((ckpt, taps)) = fixtures(true) else {
        return;
    };
    let _device = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
    if !device_present() {
        return;
    }
    // Programme material longer than a chunk: the excerpt, then the excerpt
    // backwards, cut off the span grid.
    const LEN: usize = 640_000;
    let input = read_input(&taps);
    let mut track = StereoBuf::silence(LEN);
    for ch in 0..AUDIO_CHANNELS {
        let src = input.channel(ch);
        for (i, sample) in track.channel_mut(ch).iter_mut().enumerate() {
            *sample = if i < src.len() { src[i] } else { src[2 * src.len() - 1 - i] };
        }
    }

    let mut model = VocalsModel::load(&ckpt).expect("the separator builds from its checkpoint");
    let (samples, step, fade, border) = (CHUNK.samples, CHUNK.step, CHUNK.fade, CHUNK.border);
    assert!(LEN > 2 * border, "the track is long enough to be padded");
    let padded_len = LEN + 2 * border;
    let padded = |ch: usize, at: usize| -> f32 {
        let src = track.channel(ch);
        let index = at as isize - border as isize;
        let index = if index < 0 {
            -index
        } else if index >= LEN as isize {
            2 * (LEN as isize - 1) - index
        } else {
            index
        };
        src[index as usize]
    };
    let mut sum = vec![vec![0.0f64; padded_len]; AUDIO_CHANNELS];
    let mut weight = vec![0.0f64; padded_len];
    let mut tails = (0usize, 0usize);
    let mut start = 0usize;
    while start < padded_len {
        let chunk_len = (padded_len - start).min(samples);
        let mut part = StereoBuf::silence(samples);
        for ch in 0..AUDIO_CHANNELS {
            let dst = part.channel_mut(ch);
            for i in 0..chunk_len {
                dst[i] = padded(ch, start + i);
            }
            if chunk_len < samples && chunk_len > samples / 2 {
                for i in chunk_len..samples {
                    dst[i] = dst[2 * chunk_len - 2 - i];
                }
            }
        }
        if chunk_len < samples {
            if chunk_len > samples / 2 {
                tails.0 += 1;
            } else {
                tails.1 += 1;
            }
        }
        let estimate = model.separate(&part).expect("separate a chunk").remove(0);
        let first = start == 0;
        let last = !first && start + samples >= padded_len;
        for i in 0..chunk_len {
            let mut w = if i < fade {
                i as f64 / (fade - 1) as f64
            } else if i >= samples - fade {
                (samples - 1 - i) as f64 / (fade - 1) as f64
            } else {
                1.0
            };
            if (first && i < fade) || (last && i >= samples - fade) {
                w = 1.0;
            }
            for ch in 0..AUDIO_CHANNELS {
                sum[ch][start + i] += estimate.channel(ch)[i] as f64 * w;
            }
            weight[start + i] += w;
        }
        start += step;
    }
    assert_eq!(tails, (1, 1), "one short chunk of each kind");

    let got = demix_all_lanes(&mut model, &track, |_, _| {}).expect("demix the track");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].frames(), LEN);
    for ch in 0..AUDIO_CHANNELS {
        let want: Vec<f32> = (0..LEN)
            .map(|i| (sum[ch][border + i] / weight[border + i]) as f32)
            .collect();
        let diff = Diff::of(got[0].channel(ch), &want);
        eprintln!(
            "  several chunks ch{ch}: max_abs {:.3e}  snr {:.1} dB  (rms {:.6})",
            diff.max_abs,
            diff.snr_db(),
            diff.rms_signal
        );
        assert!(diff.rms_signal > 1e-3, "the vocal is near silent on ch{ch}");
        assert!(
            diff.snr_db() >= 110.0,
            "ch{ch} is only {:.1} dB from the reference procedure",
            diff.snr_db()
        );
    }
}

#[test]
fn the_instrumental_and_the_vocal_sum_back_to_the_mix() {
    // The accompaniment an app plays is `mix - vocals`. Played beside the
    // vocal it must BE the mix again, to within the rounding of the two
    // operations: one unit in the last place of an f32 at programme level.
    // The reference's own vocal stands in for ours so that this needs no
    // device; parity above is what ties the two together.
    let Some((_, taps)) = fixtures(false) else {
        return;
    };
    let mix = read_input(&taps);
    let (_, output) = read_npy(&taps.join("06_output.npy"));
    let vocals = StereoBuf {
        left: output[..CHUNK.samples].to_vec(),
        right: output[CHUNK.samples..].to_vec(),
    };
    let rest = instrumental(&mix, &vocals);
    assert_eq!(rest.frames(), mix.frames());
    let mut worst = 0.0f32;
    for ch in 0..AUDIO_CHANNELS {
        for i in 0..mix.frames() {
            let back = rest.channel(ch)[i] + vocals.channel(ch)[i];
            worst = worst.max((back - mix.channel(ch)[i]).abs());
        }
    }
    eprintln!("  mix - vocals + vocals: worst error {worst:.3e}");
    assert!(worst <= f32::EPSILON, "the lanes re-sum to the mix only within {worst:e}");
}
