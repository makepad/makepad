use makepad_drumkit::{DrumKit, DrumVoice, SampleBank};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/score-corpus/drums/OH")
}

fn bank() -> Option<Arc<SampleBank>> {
    static BANK: OnceLock<Result<Arc<SampleBank>, String>> = OnceLock::new();
    let dir = corpus_dir();
    if !dir.is_dir() {
        eprintln!("skipping Salamander corpus test: {} is absent", dir.display());
        return None;
    }
    match BANK.get_or_init(|| SampleBank::load(&dir).map(Arc::new)) {
        Ok(bank) => Some(bank.clone()),
        Err(error) => panic!("load local Salamander corpus: {error}"),
    }
}

fn render_hit(bank: &Arc<SampleBank>, voice: DrumVoice, velocity: f32, frames: usize) -> (f64, bool) {
    let mut kit = DrumKit::new(48_000.0);
    kit.set_bank(bank.clone());
    kit.trigger(voice, velocity);
    let mut block = [[0.0f32; 2]; 257];
    let mut energy = 0.0f64;
    let mut done = 0usize;
    while done < frames && kit.active() {
        let count = block.len().min(frames - done);
        block[..count].fill([0.0; 2]);
        kit.process(&mut block[..count]);
        energy += block[..count]
            .iter()
            .map(|frame| f64::from(frame[0]) * f64::from(frame[0]) + f64::from(frame[1]) * f64::from(frame[1]))
            .sum::<f64>();
        done += count;
    }
    (energy, kit.active())
}

#[test]
fn bank_loads_and_reports_every_voice() {
    let Some(bank) = bank() else { return };
    let summary = bank.summary();
    for voice in DrumVoice::ALL {
        assert!(summary.contains(&format!("{voice:?}:")), "{voice:?} absent from {summary}");
    }
    assert!(summary.contains("TomMid:3[1/1/1]@0.891"), "{summary}");
    assert!(summary.contains("TomFloor:3[1/1/1]@0.841"), "{summary}");
}

#[test]
fn every_voice_sounds_and_decays() {
    let Some(bank) = bank() else { return };
    for voice in DrumVoice::ALL {
        let (energy, active) = render_hit(&bank, voice, 0.9, 48_000 * 12);
        assert!(energy > 1.0e-8, "{voice:?} was silent");
        assert!(!active, "{voice:?} did not decay within 12 seconds");
    }
}

#[test]
fn velocity_gain_is_monotonic() {
    let Some(bank) = bank() else { return };
    // Stay within the same kick layer so this isolates the v^1.6 gain curve
    // from the deliberately preserved dynamics between recorded layers.
    let quiet = render_hit(&bank, DrumVoice::Kick, 0.10, 48_000).0;
    let medium = render_hit(&bank, DrumVoice::Kick, 0.20, 48_000).0;
    let loud = render_hit(&bank, DrumVoice::Kick, 0.29, 48_000).0;
    assert!(quiet < medium && medium < loud, "{quiet} < {medium} < {loud}");
}

fn render_pattern(bank: &Arc<SampleBank>, block_size: usize) -> Vec<[f32; 2]> {
    let hits = [
        (0usize, DrumVoice::Kick, 1.0),
        (4_800, DrumVoice::HiHatClosed, 0.4),
        (12_000, DrumVoice::Snare, 0.8),
        (24_000, DrumVoice::TomMid, 0.7),
        (36_000, DrumVoice::HiHatOpen, 0.6),
    ];
    let total = 48_000usize;
    let mut output = vec![[0.0f32; 2]; total];
    let mut kit = DrumKit::new(48_000.0);
    kit.set_bank(bank.clone());
    let mut pos = 0;
    let mut hit = 0;
    while pos < total {
        let next_hit = hits.get(hit).map_or(total, |event| event.0);
        if next_hit == pos {
            kit.trigger(hits[hit].1, hits[hit].2);
            hit += 1;
            continue;
        }
        let count = block_size.min(total - pos).min(next_hit - pos);
        kit.process(&mut output[pos..pos + count]);
        pos += count;
    }
    output
}

#[test]
fn output_is_block_size_independent() {
    let Some(bank) = bank() else { return };
    let reference = render_pattern(&bank, 1);
    for block in [7, 64, 257, 1024] {
        let candidate = render_pattern(&bank, block);
        assert!(reference.iter().zip(candidate).all(|(a, b)| {
            a[0].to_bits() == b[0].to_bits() && a[1].to_bits() == b[1].to_bits()
        }), "block size {block} changed the render");
    }
}

#[test]
fn all_off_is_a_twenty_ms_fade() {
    let Some(bank) = bank() else { return };
    let mut kit = DrumKit::new(48_000.0);
    kit.set_bank(bank);
    kit.trigger(DrumVoice::Crash, 1.0);
    let mut warmup = [[0.0; 2]; 64];
    kit.process(&mut warmup);
    kit.all_off();
    assert!(kit.active());
    let mut fade = [[0.0; 2]; 960];
    kit.process(&mut fade);
    assert!(!kit.active());
}
