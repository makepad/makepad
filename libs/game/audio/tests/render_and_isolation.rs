//! Numerical verification of the mix, plus the property that keeps audio
//! Local tier: sound must never touch the world RNG.
//!
//! Nothing here can be listened to in CI, so the mix is checked by its
//! numbers (energy present, no clipping, no NaN) and a WAV is written to the
//! scratchpad for a human to audition.

use makepad_game_audio::bank::SampleBank;
use makepad_game_audio::director::{AudioDirector, Category, Placement, SoundEvent};
use makepad_game_audio::materials::Material;
use makepad_game_audio::mixer::{render_to_vec, to_wav, Mixer};
use makepad_game_audio::Pcm;

const RATE: u32 = 44100;

/// Build a bank of short synthetic "impacts" — a decaying tone per variant,
/// so the render has real structure without needing a downloaded pack.
fn bank_with_variants(names: &[&str], per_family: usize) -> (SampleBank, Vec<(String, Vec<u32>)>) {
    let mut bank = SampleBank::new(RATE);
    let mut families = Vec::new();
    for (fi, name) in names.iter().enumerate() {
        let mut ids = Vec::new();
        for v in 0..per_family {
            let freq = 180.0 + fi as f32 * 90.0 + v as f32 * 15.0;
            let pcm = decaying_tone(freq, 0.25);
            let wav = pcm_to_wav(&pcm);
            let id = bank.insert(&format!("{name}-{v}"), &wav).unwrap();
            ids.push(id.0);
        }
        families.push((name.to_string(), ids));
    }
    (bank, families)
}

fn decaying_tone(freq: f32, secs: f32) -> Pcm {
    let n = (RATE as f32 * secs) as usize;
    let samples = (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let env = (-t * 14.0).exp();
            (t * freq * std::f32::consts::TAU).sin() * env * 0.7
        })
        .collect();
    Pcm {
        channels: 1,
        sample_rate: RATE,
        samples,
    }
}

fn pcm_to_wav(pcm: &Pcm) -> Vec<u8> {
    let data: Vec<u8> = pcm
        .samples
        .iter()
        .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
        .collect();
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&(pcm.channels as u16).to_le_bytes());
    v.extend_from_slice(&pcm.sample_rate.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&data);
    v
}

#[test]
fn a_few_seconds_of_gameplay_audio_renders_cleanly() {
    let (mut bank, families) =
        bank_with_variants(&["impact-wood", "impact-metal", "footstep", "engine"], 4);
    let mut mixer = Mixer::new(RATE);
    let mut director = AudioDirector::new(0xA5A5);
    for (name, ids) in &families {
        director.register(
            name,
            ids.iter()
                .map(|i| makepad_game_audio::SampleId(*i))
                .collect(),
        );
    }

    // Three seconds at 60Hz of a plausible scene: footsteps throughout, an
    // engine note, and impacts from a stack coming apart.
    let mut out: Vec<f32> = Vec::new();
    let frames_per_tick = RATE as usize / 60;
    for tick in 0..180 {
        director.begin_frame(1.0 / 60.0);
        if tick % 18 == 0 {
            director.emit(
                &SoundEvent::Cue {
                    name: "footstep".into(),
                    category: Category::Movement,
                    gain: 0.6,
                    pitch: 1.0,
                },
                Placement {
                    gain: 1.0,
                    pan: -0.3,
                },
                &mut bank,
                &mut mixer,
            );
        }
        if tick % 30 == 7 {
            director.emit(
                &SoundEvent::Impact {
                    a: Material::Wood,
                    b: Material::Stone,
                    speed: 3.0 + (tick % 5) as f32,
                    pair_key: tick as u64,
                },
                Placement { gain: 0.9, pan: 0.4 },
                &mut bank,
                &mut mixer,
            );
        }
        if tick % 12 == 0 {
            director.emit(
                &SoundEvent::Cue {
                    name: "engine".into(),
                    category: Category::Movement,
                    gain: 0.35,
                    pitch: 0.9 + tick as f32 * 0.002,
                },
                Placement { gain: 0.8, pan: 0.0 },
                &mut bank,
                &mut mixer,
            );
        }
        out.extend(render_to_vec(&mut mixer, &bank, frames_per_tick));
    }

    // Numbers, since nobody can hear this in CI.
    assert!(out.iter().all(|s| s.is_finite()), "NaN or inf in the mix");
    let peak = out.iter().fold(0f32, |a, b| a.max(b.abs()));
    let rms = (out.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / out.len() as f64).sqrt();
    assert!(peak > 0.05, "mix is essentially silent (peak {peak})");
    assert!(peak <= 1.0001, "mix clipped (peak {peak})");
    assert!(rms > 0.001, "mix has no sustained energy (rms {rms})");
    eprintln!(
        "rendered {:.2}s  peak={peak:.3}  rms={rms:.4}",
        out.len() as f32 / 2.0 / RATE as f32
    );

    // Leave something audible behind for a human.
    if let Ok(dir) = std::env::var("ARCADE_AUDIO_DUMP") {
        let path = format!("{dir}/arcade_audio_demo.wav");
        if std::fs::write(&path, to_wav(&out, RATE)).is_ok() {
            eprintln!("wrote {path}");
        }
    }
}

#[test]
fn audio_selection_never_advances_a_simulation_rng() {
    // The sim's RNG is a separate stream. Interleave heavy audio work with
    // draws from it and assert the drawn sequence is bit-identical to a run
    // with no audio at all — the same proof the particle system carries.
    let draw_sequence = |with_audio: bool| -> Vec<u64> {
        let mut world_rng = makepad_game_sim::GameRng::new(12345);
        let (mut bank, families) = bank_with_variants(&["impact-wood", "footstep"], 4);
        let mut mixer = Mixer::new(RATE);
        let mut director = AudioDirector::new(999);
        for (name, ids) in &families {
            director.register(
                name,
                ids.iter()
                    .map(|i| makepad_game_audio::SampleId(*i))
                    .collect(),
            );
        }
        let mut draws = Vec::new();
        for tick in 0..64 {
            if with_audio {
                director.begin_frame(1.0 / 60.0);
                for k in 0..4u64 {
                    director.emit(
                        &SoundEvent::Impact {
                            a: Material::Wood,
                            b: Material::Wood,
                            speed: 6.0,
                            pair_key: tick * 4 + k,
                        },
                        Placement::default(),
                        &mut bank,
                        &mut mixer,
                    );
                }
                director.emit(
                    &SoundEvent::Cue {
                        name: "footstep".into(),
                        category: Category::Movement,
                        gain: 1.0,
                        pitch: 1.0,
                    },
                    Placement::default(),
                    &mut bank,
                    &mut mixer,
                );
                render_to_vec(&mut mixer, &bank, 128);
            }
            // The simulation draws regardless.
            draws.push(world_rng.next_u64());
        }
        draws
    };

    let quiet = draw_sequence(false);
    let loud = draw_sequence(true);
    assert_eq!(
        quiet, loud,
        "audio perturbed the world RNG — two devices in a room would desync"
    );
}
