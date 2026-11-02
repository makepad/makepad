use makepad_drumkit::{DrumKit, DrumVoice, SampleBank};
use std::sync::{Arc, OnceLock};

fn bank() -> Arc<SampleBank> {
    static BANK: OnceLock<Arc<SampleBank>> = OnceLock::new();
    BANK.get_or_init(|| {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("drumkit-sample-kit-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        // Half a second keeps voices overlapping through stealing and fades.
        // The loader still reads every layer and round-robin of the sample kit.
        let frames = 24_000u32;
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&96_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(frames * 2).to_le_bytes());
        for i in 0..frames {
            let envelope = 1.0 - i as f32 / frames as f32;
            let sample = ((std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin()
                * envelope * 16_000.0) as i16;
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        for (voice, layers, takes) in [
            ("kick", &["P", "F", "FF"][..], 2),
            ("snare", &["Ghost", "MP", "F", "FF"][..], 3),
            ("snareStick", &["F"][..], 1),
            ("hihatClosed", &["P", "F"][..], 1),
            ("hihatOpen", &["P", "F", "FF"][..], 1),
            ("hihatFoot", &["MP"][..], 1),
            ("hiTom", &["P", "F", "FF"][..], 1),
            ("loTom", &["PP", "MP", "FF"][..], 1),
            ("crash1", &["P", "FF"][..], 1),
            ("crash2", &["FF"][..], 1),
            ("ride1", &["MP", "FF"][..], 1),
            ("ride1Bell", &["F"][..], 1),
        ] {
            for layer in layers {
                for take in 1..=takes {
                    std::fs::write(dir.join(format!("{voice}_OH_{layer}_{take}.wav")), &wav).unwrap();
                }
            }
        }
        let loaded = SampleBank::load(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        Arc::new(loaded.expect("load generated drum samples"))
    }).clone()
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
    let bank = bank();
    let summary = bank.summary();
    for voice in DrumVoice::ALL {
        assert!(summary.contains(&format!("{voice:?}:")), "{voice:?} absent from {summary}");
    }
    assert!(summary.contains("TomMid:3[1/1/1]@0.891"), "{summary}");
    assert!(summary.contains("TomFloor:3[1/1/1]@0.841"), "{summary}");
}

#[test]
fn every_voice_sounds_and_decays() {
    let bank = bank();
    for voice in DrumVoice::ALL {
        let (energy, active) = render_hit(&bank, voice, 0.9, 48_000 * 12);
        assert!(energy > 1.0e-8, "{voice:?} was silent");
        assert!(!active, "{voice:?} did not decay within 12 seconds");
    }
}

#[test]
fn velocity_gain_is_monotonic() {
    let bank = bank();
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
    let bank = bank();
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
    let bank = bank();
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
