//! CPU numbers for the example audio shaders: renders a few seconds of
//! each (one voice or effect instance) on the native backend and the
//! interpreter, then the budget benchmark: 64 voices of a 2-op FM into an
//! SVF at 48 kHz.
//!
//! cargo run --release -p makepad-script-audio-aot --example bench

use makepad_script_audio_aot::{compile_with, Backend, Instance, Kind};
use std::time::Instant;

const RATE: f32 = 48000.0;

fn main() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/shaders");
    let names = ["saw_svf", "fm4", "pluck", "wavetable_pad", "fm2_svf", "allpass_reverb", "waveshaper4x"];
    println!("{:16} {:>12} {:>12} {:>10} {:>10}", "shader", "native ns/f", "interp ns/f", "native %", "compile ms");
    for name in names {
        let src = std::fs::read_to_string(format!("{}/{}.splash", dir, name)).unwrap();
        let t0 = Instant::now();
        let native = compile_with(&src, Backend::Native).unwrap();
        let compile_ms = t0.elapsed().as_secs_f64() * 1e3;
        let interp = compile_with(&src, Backend::Interp).unwrap();
        let ns_native = time(&native, 4.0);
        let ns_interp = time(&interp, 0.5);
        // Share of one core at 48 kHz for one instance.
        let pct = ns_native * RATE as f64 / 1e9 * 100.0;
        println!("{:16} {:>12.1} {:>12.1} {:>9.3}% {:>10.1}", name, ns_native, ns_interp, pct, compile_ms);
    }

    // The budget: 64 voices x (2-op FM + SVF) at 48 kHz within ~20% of a core.
    let src = std::fs::read_to_string(format!("{}/fm2_svf.splash", dir)).unwrap();
    for backend in [Backend::Native, Backend::Interp] {
        let shader = compile_with(&src, backend).unwrap();
        let mut voices: Vec<Instance> = (0..64).map(|_| Instance::new(shader.clone(), RATE)).collect();
        for (k, v) in voices.iter_mut().enumerate() {
            v.note_on(36.0 + k as f32, 0.8, k as u32);
        }
        let secs = if backend == Backend::Native { 2.0 } else { 0.25 };
        let blocks = (secs * RATE / 128.0) as usize;
        let mut l = vec![0.0f32; 128];
        let mut r = vec![0.0f32; 128];
        let t0 = Instant::now();
        for _ in 0..blocks {
            for v in voices.iter_mut() {
                v.render(&mut l, &mut r);
            }
        }
        let el = t0.elapsed().as_secs_f64();
        let audio = blocks as f64 * 128.0 / RATE as f64;
        println!("64 voices fm2_svf {:?}: {:.1}% of one core ({:.1} ns per voice-frame)", backend, el / audio * 100.0, el * 1e9 / (audio * RATE as f64 * 64.0));
    }
}

fn time(shader: &std::sync::Arc<makepad_script_audio_aot::AudioShader>, secs: f32) -> f64 {
    let mut inst = Instance::new(shader.clone(), RATE);
    inst.note_on(48.0, 0.8, 3);
    let frames = (secs * RATE) as usize;
    let mut l = vec![0.0f32; 128];
    let mut r = vec![0.0f32; 128];
    let input: Vec<f32> = (0..128).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
    let t0 = Instant::now();
    for _ in 0..frames / 128 {
        if shader.kind == Kind::Effect {
            inst.process(&input, &input, &mut l, &mut r);
        } else {
            inst.render(&mut l, &mut r);
        }
    }
    t0.elapsed().as_secs_f64() * 1e9 / frames as f64
}
