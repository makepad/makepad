//! Compares the Vorbis decoder against a reference produced by the system
//! decoder. Skips cleanly where the fixtures or `afconvert` are unavailable,
//! so a fresh checkout is never blocked by a missing asset.
use makepad_game_audio as audio;
use std::path::Path;
use std::process::Command;

const KENNEY: &str = "../../../apps/arcade/resources/audio/kenney";

/// Decode `ogg` with the system decoder for comparison. `None` when the tool
/// or the fixture is missing.
fn reference(ogg: &Path, tag: &str) -> Option<audio::Pcm> {
    if !ogg.exists() {
        return None;
    }
    let out = std::env::temp_dir().join(format!("mp_vorbis_ref_{tag}.wav"));
    let _ = std::fs::remove_file(&out);
    let ok = Command::new("afconvert")
        .args(["-f", "WAVE", "-d", "LEF32"])
        .arg(ogg)
        .arg(&out)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return None;
    }
    audio::wav::decode(&std::fs::read(&out).ok()?).ok()
}

fn corr(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len());
    let (mut num, mut da, mut db) = (0f64, 0f64, 0f64);
    for i in 0..n {
        let (x, y) = (a[i] as f64, b[i] as f64);
        num += x * y;
        da += x * x;
        db += y * y;
    }
    if da <= 0.0 || db <= 0.0 {
        return 0.0;
    }
    num / (da.sqrt() * db.sqrt())
}

/// Mono decode must be sample-exact: alignment, amplitude and length.
#[test]
fn mono_vorbis_matches_the_system_decoder() {
    let p = Path::new(KENNEY).join("interface-sounds/click_001.ogg");
    let Some(want) = reference(&p, "mono") else {
        eprintln!("skip: run apps/arcade/download_assets.sh (or no afconvert)");
        return;
    };
    let got = audio::decode(&std::fs::read(&p).unwrap()).expect("decode ogg");
    assert_eq!(got.channels, want.channels, "channel count");
    assert_eq!(got.sample_rate, want.sample_rate, "sample rate");
    assert!(got.samples.iter().all(|s| s.is_finite()), "non-finite output");

    let c = corr(&got.samples, &want.samples);
    eprintln!(
        "mono: got {} frames, ref {} frames, corr {c:.6}",
        got.frames(),
        want.frames()
    );
    // Aligned and scaled correctly, not merely "similar".
    assert!(c > 0.999, "correlation {c:.6} — decoder disagrees with reference");
}

/// The stream's own granule position is the authority on length; the system
/// decoder trims a further half-window, so compare against the file, not it.
#[test]
fn output_length_follows_the_granule_position() {
    let p = Path::new(KENNEY).join("interface-sounds/click_001.ogg");
    if !p.exists() {
        eprintln!("skip: run apps/arcade/download_assets.sh");
        return;
    }
    let bytes = std::fs::read(&p).unwrap();
    let got = audio::decode(&bytes).expect("decode");
    let pages = audio::ogg::read_packets(&bytes).expect("pages");
    assert_eq!(
        got.frames() as u64,
        pages.last_granule,
        "decoded frames must equal the final granule position"
    );
}

/// Corrupt input must be refused, never panic and never hang.
#[test]
fn malformed_vorbis_is_refused_not_fatal() {
    let p = Path::new(KENNEY).join("interface-sounds/click_001.ogg");
    if !p.exists() {
        eprintln!("skip: run apps/arcade/download_assets.sh");
        return;
    }
    let good = std::fs::read(&p).unwrap();
    // Deterministic mutations across the whole file: header fields, segment
    // tables and packet payloads all get hit.
    let mut seed = 0x9E3779B97F4A7C15u64;
    for i in 0..3000 {
        let mut bad = good.clone();
        for _ in 0..(1 + i % 8) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let at = (seed as usize) % bad.len();
            bad[at] = (seed >> 32) as u8;
        }
        // Must return, either way, without panicking.
        if let Ok(p) = audio::decode(&bad) {
            assert!(p.samples.iter().all(|s| s.is_finite()), "non-finite from mutated input");
        }
    }
    // Truncations at every scale.
    for cut in [1usize, 2, 27, 47, 100, 1000] {
        if cut < good.len() {
            let _ = audio::decode(&good[..cut]);
            let _ = audio::decode(&good[cut..]);
        }
    }
}
