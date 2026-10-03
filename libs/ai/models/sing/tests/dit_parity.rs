//! The Cantor DiT device path against the reference implementation's own
//! tensors for one short clip (feature `dit`; needs a CUDA device).
//!
//!   MAKEPAD_CANTOR_DIT_MODEL=cantor-dit.mksing MAKEPAD_CANTOR_DIT_REF=DIR \
//!     cargo test --release -p makepad-ai-sing --features dit --test dit_parity -- --ignored
//!
//! `DIR` is a reference dump (raw little-endian tensors `<name>.bin`: the
//! prompt's and target's tokens, the prompt clip and its mel, the encoder's
//! conditions, the initial noise, the mel after 32 steps at CFG 3, the
//! vocoder's waveforms) and the target's note list `meta.json` (space
//! separated `duration`, `phoneme`, `note_pitch`, `note_type`). The device
//! path runs f16 GEMM operands with f32 accumulation; its error against the
//! fp32 reference is held under the reference's own fp16 mode's (mel
//! rel L2 1.5e-2, waveform 0.20 on a 4.4 s held-out line).

use makepad_ai_sing::dit::front::{DitNote, Tokens};
use makepad_ai_sing::dit::{CantorDit, DitOpts, Voice};
use std::path::{Path, PathBuf};

fn env_path(k: &str) -> PathBuf {
    PathBuf::from(std::env::var(k).unwrap_or_else(|_| panic!("set {k} (see the file header)")))
}

fn f32s(dir: &Path, name: &str) -> Vec<f32> {
    std::fs::read(dir.join(format!("{name}.bin"))).unwrap_or_else(|e| panic!("{name}: {e}")).chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
}

fn u32s(dir: &Path, name: &str) -> Vec<u32> {
    f32s(dir, name).iter().map(|v| v.to_bits()).collect()
}

fn tokens(dir: &Path, side: &str) -> Tokens {
    Tokens {
        phoneme: u32s(dir, &format!("{side}_phoneme")),
        pitch: u32s(dir, &format!("{side}_pitch")),
        kind: u32s(dir, &format!("{side}_type")),
        mel2note: u32s(dir, &format!("{side}_mel2note")),
    }
}

/// Relative L2 error of `a` against the reference `b`.
fn rel(name: &str, a: &[f32], b: &[f32]) -> f64 {
    assert_eq!(a.len(), b.len(), "{name}: lengths");
    let (mut d2, mut b2) = (0f64, 0f64);
    for (x, y) in a.iter().zip(b) {
        d2 += ((x - y) as f64).powi(2);
        b2 += (*y as f64).powi(2);
    }
    let r = (d2 / b2.max(1e-30)).sqrt();
    println!("{name:<26} rel L2 {r:.6}");
    r
}

fn meta_notes(json: &str) -> Vec<DitNote> {
    let field = |k: &str| -> Vec<String> {
        let key = format!("\"{k}\": \"");
        let a = json.find(&key).unwrap_or_else(|| panic!("meta.json has no {k}")) + key.len();
        let b = a + json[a..].find('"').unwrap();
        json[a..b].split(' ').map(String::from).collect()
    };
    let (d, p, q, k) = (field("duration"), field("phoneme"), field("note_pitch"), field("note_type"));
    (0..p.len()).map(|i| DitNote { dur: d[i].parse().unwrap(), phoneme: p[i].clone(), pitch: q[i].parse().unwrap(), kind: k[i].parse().unwrap() }).collect()
}

#[test]
#[ignore = "needs the converted model, a reference dump and a CUDA device"]
fn device_path_matches_the_reference() {
    let dir = env_path("MAKEPAD_CANTOR_DIT_REF");
    let m = CantorDit::load(&env_path("MAKEPAD_CANTOR_DIT_MODEL")).expect("model");
    let (pt, gt) = (tokens(&dir, "pt"), tokens(&dir, "gt"));

    // The front end's tokens are the reference data processor's, exactly.
    let json = std::fs::read_to_string(dir.join("meta.json")).expect("meta.json");
    assert_eq!(m.tokens(&meta_notes(&json)).expect("tokens"), gt);

    let pt_mel = f32s(&dir, "pt_mel");
    assert!(rel("prompt mel", &m.mel_of(&f32s(&dir, "pt_wav")).unwrap(), &pt_mel) < 1e-3);
    let cond = makepad_ai_common::gpu::gpu_download(&m.encode(&pt, &gt).unwrap()).unwrap();
    assert!(rel("encoder conditions", &cond, &f32s(&dir, "cond")) < 5e-3);

    // Same noise, 32 steps at CFG 3.
    let voice = Voice { name: "ref".into(), mel: pt_mel.clone(), tokens: pt.clone() };
    let opts = DitOpts { steps: 32, cfg: 3.0, ..DitOpts::default() };
    let ref_mel = f32s(&dir, "mel");
    let mel = m.sample(&voice, &gt, &opts, Some(&f32s(&dir, "z"))).unwrap();
    assert!(rel("mel, same noise", &mel, &ref_mel) < 5e-3);

    let ref_wav = f32s(&dir, "wav");
    assert!(rel("vocoder on reference mel", &m.vocode(&ref_mel, gt.frames()).unwrap(), &ref_wav) < 2e-2);
    assert!(rel("vocoder on prompt mel", &m.vocode(&pt_mel, pt.frames()).unwrap(), &f32s(&dir, "pt_voc")) < 2e-2);
    assert!(rel("waveform end to end", &m.vocode(&mel, gt.frames()).unwrap(), &ref_wav) < 0.15);
}
