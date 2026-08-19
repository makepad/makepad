//! Whole-track end to end: streaming demix + span cache + seek, checked
//! against the PyTorch reference's own 4-minute output.
//!
//! Skips unless `local/stems_ref/` carries the checkpoint, the fixture wav and
//! the reference stems. Regenerate the reference with:
//! ```text
//! cd local/stems_ref
//! ./venv/bin/python oracle.py demix --in fixtures/music_4min.wav \
//!     --out out_4min_mps --device mps
//! ```

use makepad_ai_stems::cache::{CacheHeader, StemCache};
use makepad_ai_stems::config::{AUDIO_CHANNELS, CHUNK_STEP, NUM_STEMS, SAMPLE_RATE, STEM_NAMES};
use makepad_ai_stems::{Demixer, StemsModel, StereoBuf};
use std::path::{Path, PathBuf};

fn oracle_root() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../local/stems_ref"
    ))
}

/// 16-bit PCM RIFF reader — enough for the fixtures `oracle.py` writes.
fn read_wav_pcm16(path: &Path) -> (StereoBuf, u32) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut pos = 12usize;
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut data: &[u8] = &[];
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]])
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
    assert_eq!(channels, 2, "fixture must be stereo");
    assert_eq!(bits, 16, "fixture must be 16-bit PCM");
    let frames = data.len() / 4;
    let mut out = StereoBuf::silence(frames);
    for frame in 0..frames {
        let at = frame * 4;
        out.left[frame] = i16::from_le_bytes([data[at], data[at + 1]]) as f32 / 32768.0;
        out.right[frame] = i16::from_le_bytes([data[at + 2], data[at + 3]]) as f32 / 32768.0;
    }
    (out, rate)
}

fn read_npy_f32(path: &Path) -> (Vec<usize>, Vec<f32>) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(&bytes[0..6], b"\x93NUMPY");
    let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    let header = std::str::from_utf8(&bytes[10..10 + header_len]).unwrap();
    let open = header.find('(').unwrap();
    let close = header[open..].find(')').unwrap() + open;
    let shape: Vec<usize> = header[open + 1..close]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect();
    let values = bytes[10 + header_len..]
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    (shape, values)
}

fn snr_db(got: &[f32], want: &[f32]) -> f64 {
    let mut err = 0.0f64;
    let mut sig = 0.0f64;
    for (g, w) in got.iter().zip(want) {
        assert!(g.is_finite());
        let d = (g - w) as f64;
        err += d * d;
        sig += (*w as f64) * (*w as f64);
    }
    if err <= 0.0 {
        return f64::INFINITY;
    }
    20.0 * (sig / err).sqrt().log10()
}

#[test]
fn four_minute_track_streams_caches_and_matches_the_reference() {
    let root = oracle_root();
    let ckpt = root.join("ckpt/model_bs_roformer_ep_17_sdr_9.6568.ckpt");
    let fixture = root.join("fixtures/music_4min.wav");
    let reference = root.join("out_4min_mps/stems_f32.npy");
    if !ckpt.is_file() || !fixture.is_file() || !reference.is_file() {
        eprintln!("SKIP: oracle tree absent under {}", root.display());
        return;
    }

    let (track, rate) = read_wav_pcm16(&fixture);
    assert_eq!(rate, SAMPLE_RATE);
    let frames = track.frames();
    let duration = frames as f64 / rate as f64;
    eprintln!("track: {frames} frames, {duration:.1}s");

    let mut model = match StemsModel::load(&ckpt) {
        Ok(model) => model,
        Err(e) => {
            eprintln!("SKIP: no device runtime: {e}");
            return;
        }
    };

    let cache_root = std::env::temp_dir().join(format!("makepad-stems-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache_root);
    let mut cache =
        StemCache::open(&cache_root, "e2efixture", CacheHeader::for_track(frames as u64)).unwrap();

    let mut stems = makepad_ai_stems::model::empty_stem_set(frames);
    let mut first_span_secs = 0.0f64;
    let started = std::time::Instant::now();
    {
        let mut demixer = Demixer::new(&mut model, &track).unwrap();
        let mut spans = 0usize;
        while let Some(span) = demixer.next_span().unwrap() {
            if spans == 0 {
                first_span_secs = started.elapsed().as_secs_f64();
            }
            assert_eq!(
                span.start % CHUNK_STEP,
                0,
                "span {spans} start {} is off the grid",
                span.start
            );
            cache.write_span(span.start, &span.stems).unwrap();
            for stem in 0..NUM_STEMS {
                for ch in 0..AUDIO_CHANNELS {
                    let src = span.stems[stem].channel(ch);
                    let end = (span.start + src.len()).min(frames);
                    let dst = stems[stem].channel_mut(ch);
                    dst[span.start..end].copy_from_slice(&src[..end - span.start]);
                }
            }
            spans += 1;
        }
        assert_eq!(spans, cache.span_count());
    }
    let wall = started.elapsed().as_secs_f64();
    eprintln!(
        "demix: {wall:.1}s wall for {duration:.1}s audio -> {:.2}x realtime \
         (first span after {first_span_secs:.1}s)",
        duration / wall
    );
    assert!(cache.is_complete(), "every span must have been cached");

    // -- parity against the reference's own 4-minute output --
    let (shape, want) = read_npy_f32(&reference);
    assert_eq!(shape, vec![NUM_STEMS, AUDIO_CHANNELS, frames]);
    let mut worst = f64::INFINITY;
    for stem in 0..NUM_STEMS {
        for ch in 0..AUDIO_CHANNELS {
            let at = (stem * AUDIO_CHANNELS + ch) * frames;
            let rms = (want[at..at + frames]
                .iter()
                .map(|v| (*v as f64) * (*v as f64))
                .sum::<f64>()
                / frames as f64)
                .sqrt();
            let snr = snr_db(stems[stem].channel(ch), &want[at..at + frames]);
            eprintln!(
                "  {:>6} ch{ch}: snr {snr:.1} dB (rms {rms:.6})",
                STEM_NAMES[stem]
            );
            if rms > 1e-3 {
                worst = worst.min(snr);
            }
        }
    }
    eprintln!("worst loud-stem SNR over the whole track: {worst:.1} dB");
    assert!(worst > 55.0, "whole-track SNR is only {worst:.1} dB");

    // -- the cache must give back what the stream produced (to i16) --
    let cached = cache.read_all().unwrap();
    for stem in 0..NUM_STEMS {
        for ch in 0..AUDIO_CHANNELS {
            let a = cached[stem].channel(ch);
            let b = stems[stem].channel(ch);
            let max = a
                .iter()
                .zip(b)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max);
            assert!(
                max <= 1.0 / 16384.0,
                "cache round trip for {} ch{ch} deviates by {max:.3e}",
                STEM_NAMES[stem]
            );
        }
    }

    // -- seek: restarting mid-track must reproduce the same span --
    let target_span = cache.span_count() / 2;
    let target = target_span * CHUNK_STEP;
    let seek_started = std::time::Instant::now();
    let mut demixer = Demixer::new(&mut model, &track).unwrap();
    demixer.seek(target);
    let span = demixer.next_span().unwrap().expect("a span after seek");
    let seek_secs = seek_started.elapsed().as_secs_f64();
    eprintln!("seek to {target} -> span at {} in {seek_secs:.1}s", span.start);
    assert_eq!(span.start, target);
    for stem in 0..NUM_STEMS {
        for ch in 0..AUDIO_CHANNELS {
            let got = span.stems[stem].channel(ch);
            let want = &stems[stem].channel(ch)[target..target + got.len()];
            let snr = snr_db(got, want);
            assert!(
                snr > 80.0 || want.iter().all(|v| v.abs() < 1e-4),
                "seeking to {target} changed {} ch{ch} (snr {snr:.1} dB)",
                STEM_NAMES[stem]
            );
        }
    }

    let _ = std::fs::remove_dir_all(&cache_root);
}
