//! Offline renders through the whole game bus (spatializer, reverb, master
//! dynamics): the recipe bank by family, a car pass-by with doppler, a
//! multi-car race mix, room presets, and the render cost per voice.
//!
//! `SYNTH_RENDER_DIR=/tmp/x cargo test --release -p makepad-audio-synth
//! --test bus_renders -- --nocapture --test-threads 1`

use makepad_audio_synth::bus::{Emitter, Priority, SynthBus, RECIPE_VOICES};
use makepad_audio_synth::engine::{preset, EngineInput};
use makepad_audio_synth::recipe::{builtin, Recipe, BUILTIN_NAMES};
use makepad_audio_synth::reverb::RoomPreset;
use makepad_audio_synth::spatial::{Distance, Listener3};
use makepad_audio_synth::vehicle::{Surface, TyreInput};
use makepad_audio_synth::wav::stereo_wav_16;
use std::time::Instant;

const RATE: f32 = 48_000.0;

fn write(name: &str, l: &[f32], r: &[f32]) {
    if let Some(dir) = std::env::var_os("SYNTH_RENDER_DIR") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.wav")), stereo_wav_16(l, r, RATE as u32)).unwrap();
    }
}

fn render(bus: &mut SynthBus, secs: f32) -> (Vec<f32>, Vec<f32>) {
    let n = (RATE * secs) as usize;
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    for (cl, cr) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
        bus.render(cl, cr);
    }
    (l, r)
}

fn stats(x: &[f32]) -> (f32, f32, f32) {
    let peak = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt();
    // Largest sample-to-sample step relative to the peak: a click detector
    // (a hard edge is a step near 2× peak; a smooth sound stays far below).
    let step = x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    (20.0 * peak.max(1e-9).log10(), 20.0 * rms.max(1e-9).log10(), step / peak.max(1e-9))
}

/// Every built-in recipe, spaced 0.1 s apart after its own length, in
/// families; one WAV per family.
#[test]
fn recipe_bank_by_family() {
    let families: &[(&str, &[&str])] = &[
        ("ui", &["click", "hover", "confirm", "cancel", "error", "notify", "beep", "coin", "pickup"]),
        ("weapons", &["gun_pistol", "gun_rifle", "gun_smg", "gun_shotgun", "gun_sniper", "gun_rocket", "gun_laser", "reload", "empty", "sword", "bow"]),
        ("explosions", &["explosion_small", "explosion", "explosion_big", "fireball", "thunder"]),
        ("impacts", &["thud", "impact_metal", "impact_stone", "impact_glass", "impact_flesh", "impact_plastic", "impact_car", "thump"]),
        ("footsteps", &["footstep", "footstep_grass", "footstep_gravel", "footstep_wood", "footstep_metal", "footstep_snow", "footstep_water", "land"]),
        ("magic_water", &["magic", "heal", "teleport", "shield", "water", "bubble", "splash", "wind_gust", "bell"]),
        ("classic", &["jump", "shoot", "zap", "grab", "angry", "calm", "rescue", "shove", "board", "hurt", "win", "lose", "squeak", "roar", "bark", "moo", "whip", "alarm", "horn"]),
    ];
    let mut covered = 0;
    for (family, names) in families {
        let mut bus = SynthBus::new(RATE);
        let (mut l, mut r) = (Vec::new(), Vec::new());
        println!("-- {family}");
        for name in names.iter() {
            let recipe = Recipe::parse(builtin(name).unwrap()).unwrap();
            let len = recipe.length().min(4.0) + 0.25;
            bus.play_recipe(&recipe, 1.0, 1.0, Emitter::FLAT, Priority::Normal, 0);
            let (a, b) = render(&mut bus, len);
            let (peak, rms, step) = stats(&a);
            println!("   {name:16} len {len:4.2}s peak {peak:6.1} dBFS rms {rms:6.1} step/peak {step:.2}");
            assert!(peak < -0.5, "{name} peaks at {peak}");
            l.extend_from_slice(&a);
            r.extend_from_slice(&b);
            covered += 1;
        }
        write(&format!("sfx_{family}"), &l, &r);
    }
    assert!(covered >= BUILTIN_NAMES.len() - 6, "families cover the bank");
}

/// A V8 passing the listener at 35 m/s, 6 m to the side: pan sweeps left to
/// right, doppler drops the pitch by ~20 %, air absorption darkens it far.
#[test]
fn car_pass_by_with_doppler() {
    let mut bus = SynthBus::new(RATE);
    let spec = preset("v8").unwrap();
    bus.set_listener(Listener3::default());
    let secs = 8.0;
    let n = (RATE * secs) as usize;
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    let block = 512;
    let mut t = 0.0f32;
    for (cl, cr) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
        // Travels along x from -140 m to +140 m, 6 m in front (z = -6).
        let x = -140.0 + 35.0 * t;
        let e = Emitter {
            vel: [35.0, 0.0, 0.0],
            dist: Distance { near: 3.0, range: 200.0, rolloff: 0.7 },
            ..Emitter::at([x, 0.0, -6.0], 200.0)
        };
        bus.vehicle(1, &spec, EngineInput { rpm: 4800.0, throttle: 1.0, load: 1.0, gear: 4, speed: 35.0 }, TyreInput { slip: 0.02, load: 1.0, speed: 35.0, surface: Surface::Asphalt, grounded: 4 }, e, 0.0, 1.0);
        bus.end_frame();
        bus.render(cl, cr);
        t += block as f32 / RATE;
    }
    let (pl, _, _) = stats(&l);
    let (pr, _, _) = stats(&r);
    // Before the pass (t=2..3 s) the left ear is louder; after (t=5..6) the right.
    let seg = |x: &[f32], a: f32, b: f32| stats(&x[(a * RATE) as usize..(b * RATE) as usize]).1;
    println!("pass-by: peak L {pl:.1} R {pr:.1}; before L {:.1} R {:.1}; after L {:.1} R {:.1}", seg(&l, 2.5, 3.5), seg(&r, 2.5, 3.5), seg(&l, 4.8, 5.8), seg(&r, 4.8, 5.8));
    assert!(seg(&l, 2.5, 3.5) > seg(&r, 2.5, 3.5) + 3.0);
    assert!(seg(&r, 4.8, 5.8) > seg(&l, 4.8, 5.8) + 3.0);
    write("car_pass_by_doppler", &l, &r);
}

/// Six race cars of different presets circling the listener: the voice
/// pool, stealing and the master limiter under load, plus measured cost.
#[test]
fn race_mix_and_cost() {
    let mut bus = SynthBus::new(RATE);
    let cars = ["flat6", "v8", "v10", "inline4_turbo", "v8_flat", "v12"];
    let secs = 10.0;
    let n = (RATE * secs) as usize;
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    let block = 512;
    let mut t = 0.0f32;
    let started = Instant::now();
    for (cl, cr) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
        for (i, name) in cars.iter().enumerate() {
            let spec = preset(name).unwrap();
            let phase = t * 0.35 + i as f32 * 1.047;
            let radius = 25.0 + 8.0 * i as f32;
            let pos = [phase.cos() * radius, 0.0, phase.sin() * radius];
            let speed = 0.35 * radius;
            let vel = [-phase.sin() * speed, 0.0, phase.cos() * speed];
            let u = ((t * 0.5 + i as f32 * 0.3).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
            let rpm = spec.idle_rpm + (spec.redline_rpm - spec.idle_rpm) * (0.3 + 0.65 * u);
            let throttle = if (t + i as f32).sin() > -0.6 { 1.0 } else { 0.0 };
            let e = Emitter { vel, dist: Distance { near: 3.0, range: 160.0, rolloff: 0.7 }, ..Emitter::at(pos, 160.0) };
            bus.vehicle(
                i as u64 + 1,
                &spec,
                EngineInput { rpm, throttle, load: if throttle > 0.0 { 1.0 } else { -0.3 }, gear: (1 + (u * 5.0) as i8), speed },
                TyreInput { slip: if (t * 0.7 + i as f32).sin() > 0.9 { 0.4 } else { 0.03 }, load: 1.1, speed, surface: Surface::Asphalt, grounded: 4 },
                e,
                0.0,
                1.0,
            );
        }
        bus.end_frame();
        // Sprinkle impacts and gunfire on top.
        if (t * 10.0) as u32 % 13 == 0 {
            let recipe = Recipe::parse(builtin("impact_metal").unwrap()).unwrap();
            bus.play_recipe(&recipe, 1.0, 0.6, Emitter::at([10.0, 0.0, -5.0], 60.0), Priority::Normal, 0);
        }
        bus.render(cl, cr);
        t += block as f32 / RATE;
    }
    let spent = started.elapsed().as_secs_f32();
    let s = bus.stats();
    println!(
        "race mix: {} cars, {secs} s rendered in {:.3} s = {:.1}x realtime ({:.1} % of one core); bus cpu_load {:.3} peak {:.3}; peak {:.2}; steals {}",
        cars.len(),
        spent,
        secs / spent,
        100.0 * spent / secs,
        s.cpu_load,
        s.peak_cpu_load,
        s.peak,
        s.steals
    );
    let (pl, _, _) = stats(&l);
    assert!(pl < -0.9, "limiter ceiling -1 dBFS, got {pl}");
    write("race_mix_6_cars", &l, &r);
}

/// The same gunshot in each room preset.
#[test]
fn rooms() {
    let recipe = Recipe::parse(builtin("gun_pistol").unwrap()).unwrap();
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for room in RoomPreset::ALL {
        let mut bus = SynthBus::new(RATE);
        bus.set_room(room.params());
        // Let the crossfade settle from the default room.
        render(&mut bus, 3.0);
        bus.play_recipe(&recipe, 1.0, 1.0, Emitter::at([4.0, 0.0, -8.0], 80.0), Priority::High, 0);
        let (a, b) = render(&mut bus, 2.5);
        let tail = stats(&a[(RATE * 1.0) as usize..]).1;
        println!("room {:8}: tail rms after 1 s {tail:.1} dB", room.name());
        l.extend_from_slice(&a);
        r.extend_from_slice(&b);
    }
    write("rooms_pistol", &l, &r);
}

/// Cost of one voice of each kind, and the pool under a 200-shot burst.
#[test]
fn voice_cost_and_pool() {
    let mut bus = SynthBus::new(RATE);
    let recipe = Recipe::parse(builtin("explosion").unwrap()).unwrap();
    for i in 0..RECIPE_VOICES {
        bus.play_recipe(&recipe, 1.0, 0.1, Emitter::at([i as f32, 0.0, -5.0], 100.0), Priority::Normal, 0);
    }
    let started = Instant::now();
    render(&mut bus, 1.0);
    let full = started.elapsed().as_secs_f32();
    println!("{} explosion voices: {:.2} % of one core ({:.1} µs per voice per 10 ms block)", RECIPE_VOICES, full * 100.0, full * 1e6 / RECIPE_VOICES as f32 / 100.0);
    for _ in 0..200 {
        bus.play_recipe(&recipe, 1.0, 0.1, Emitter::FLAT, Priority::Normal, 0);
    }
    assert!(bus.live_counts().0 <= RECIPE_VOICES);
}
