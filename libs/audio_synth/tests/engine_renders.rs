//! Offline renders of every engine preset, with the analysis printed:
//! order spectrum (harmonics of crank rotation), loudness, brightness,
//! aliasing floor and start clicks. WAVs land in `SYNTH_RENDER_DIR` (set it
//! to listen; unset writes nothing).
//!
//! Run: `SYNTH_RENDER_DIR=/tmp/x cargo test --release -p makepad-audio-synth
//! --test engine_renders -- --nocapture`

use makepad_audio_synth::engine::{preset, Engine, EngineInput, EngineKind, PRESET_NAMES};
use makepad_audio_synth::vehicle::{Surface, TyreInput, VehicleSound};
use makepad_audio_synth::wav::stereo_wav_16;

const RATE: f32 = 48_000.0;

fn out_dir() -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(std::env::var_os("SYNTH_RENDER_DIR")?);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn write(name: &str, mono: &[f32]) {
    if let Some(dir) = out_dir() {
        let bytes = stereo_wav_16(mono, mono, RATE as u32);
        std::fs::write(dir.join(format!("{name}.wav")), bytes).unwrap();
    }
}

/// Radix-2 FFT magnitude of a Hann-windowed frame (length power of two).
pub fn spectrum(x: &[f32]) -> Vec<f32> {
    let n = x.len().next_power_of_two() >> if x.len().is_power_of_two() { 0 } else { 1 };
    let mut re: Vec<f32> = (0..n)
        .map(|i| x[i] * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()))
        .collect();
    let mut im = vec![0.0f32; n];
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (ang * k as f32).sin_cos();
                let a = start + k;
                let b = a + len / 2;
                let tr = re[b] * c - im[b] * s;
                let ti = re[b] * s + im[b] * c;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
    (0..n / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

/// Spectral centroid in Hz.
fn centroid(spec: &[f32]) -> f32 {
    let n = spec.len() as f32 * 2.0;
    let (mut num, mut den) = (0.0, 0.0);
    for (k, m) in spec.iter().enumerate() {
        let hz = k as f32 * RATE / n;
        num += hz * m;
        den += m;
    }
    num / den.max(1e-9)
}

/// Octave-band power (dB re total) centred 63 Hz .. 16 kHz.
fn octaves(spec: &[f32]) -> String {
    let bin_hz = RATE / (2.0 * spec.len() as f32);
    let total: f32 = spec.iter().map(|m| m * m).sum::<f32>().max(1e-12);
    let mut out = Vec::new();
    let mut c = 63.0f32;
    while c <= 16_000.0 {
        let (lo, hi) = ((c / 1.414 / bin_hz) as usize, ((c * 1.414 / bin_hz) as usize).min(spec.len()));
        let e: f32 = spec[lo.min(hi)..hi].iter().map(|m| m * m).sum();
        out.push(format!("{:.0}", 10.0 * (e / total).max(1e-9).log10()));
        c *= 2.0;
    }
    out.join(" ")
}

/// Amplitude (dB re the strongest) at orders 0.5..8 of crank rotation.
fn orders(spec: &[f32], rpm: f32) -> Vec<(f32, f32)> {
    let n = spec.len() as f32 * 2.0;
    let bin_hz = RATE / n;
    let rot = rpm / 60.0;
    let mut out = Vec::new();
    let mut o = 0.5;
    while o <= 8.0 {
        let k = (o * rot / bin_hz).round() as usize;
        let m = (k.saturating_sub(2)..=k + 2).map(|i| spec.get(i).copied().unwrap_or(0.0)).fold(0.0f32, f32::max);
        out.push((o, m));
        o += 0.5;
    }
    let top = out.iter().map(|x| x.1).fold(1e-9f32, f32::max);
    out.into_iter().map(|(o, m)| (o, db(m / top))).collect()
}

fn steady(name: &str, rpm: f32, throttle: f32) -> Vec<f32> {
    let spec = preset(name).unwrap();
    let mut e = Engine::new(RATE, spec);
    e.set_input(EngineInput { rpm, throttle, load: throttle, gear: 3, speed: 25.0 });
    let mut out = vec![0.0; (RATE * 1.5) as usize];
    for c in out.chunks_mut(256) {
        e.render(c, 1.0);
    }
    out[(RATE * 0.5) as usize..].to_vec()
}

#[test]
fn engine_order_analysis_and_sweeps() {
    println!("\npreset          rpm   thr  rms_dB  centroid  strongest orders (dB)");
    for name in PRESET_NAMES {
        let spec = preset(name).unwrap();
        let rpms: Vec<f32> = match spec.kind {
            EngineKind::Turbine => vec![0.3, 0.9],
            EngineKind::Rotor => vec![300.0],
            EngineKind::Electric => vec![3000.0, 12000.0],
            EngineKind::Piston => vec![(spec.idle_rpm).max(800.0), 3000.0f32.min(spec.redline_rpm * 0.6), spec.redline_rpm * 0.9],
        };
        for &rpm in &rpms {
            for thr in [0.1f32, 1.0] {
                let x = steady(name, rpm, thr);
                assert!(x.iter().all(|v| v.is_finite()), "{name}");
                let spec_mag = spectrum(&x[..32768]);
                let c = centroid(&spec_mag);
                let mut ord = if spec.kind == EngineKind::Piston { orders(&spec_mag, rpm) } else { vec![] };
                ord.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                let top: Vec<String> = ord.iter().take(5).map(|(o, d)| format!("{o}:{d:.0}")).collect();
                println!("{name:14} {rpm:6.0} {thr:4.1} {:7.1} {c:8.0}  {}  | oct63..16k {}", db(rms(&x)), top.join(" "), octaves(&spec_mag));
            }
        }
    }
}

/// Full-throttle sweep idle → redline, then a lift-off (crackle), per
/// preset. Also the gear-shift and starter sequence for the V8.
#[test]
fn engine_sweep_wavs() {
    for name in PRESET_NAMES {
        let spec = preset(name).unwrap();
        let mut v = VehicleSound::new(RATE, spec);
        let (lo, hi) = match spec.kind {
            EngineKind::Turbine => (0.25, 1.0),
            EngineKind::Rotor => (290.0, 320.0),
            EngineKind::Electric => (0.0, 14_000.0),
            EngineKind::Piston => (1000.0f32.max(spec.idle_rpm), spec.redline_rpm.min(8000.0).max(spec.idle_rpm * 1.5)),
        };
        let hi = if name == &"v10" { 18_000.0 } else if name == &"bike_inline4" { 13_500.0 } else if name == &"kart" { 14_000.0 } else { hi };
        let secs = 8.0;
        let n = (RATE * secs) as usize;
        let mut out = vec![0.0; n];
        let block = 256;
        let mut t = 0.0f32;
        let mut max_step = 0.0f32;
        let mut prev = 0.0f32;
        for (bi, chunk) in out.chunks_mut(block).enumerate() {
            let u = (t / 6.0).min(1.0);
            let lifting = t > 6.0;
            let rpm = if lifting { hi - (hi - lo) * ((t - 6.0) / 2.0).min(1.0) * 0.6 } else { lo + (hi - lo) * u };
            let throttle = if lifting { 0.0 } else { 1.0 };
            let speed = 5.0 + 50.0 * u;
            v.set(
                EngineInput { rpm, throttle, load: if lifting { -0.3 } else { 1.0 }, gear: 3, speed },
                TyreInput { slip: 0.02, load: 1.0, speed, surface: Surface::Asphalt, grounded: 4 },
            );
            v.render(chunk, 1.0);
            for &s in chunk.iter() {
                if bi < 4 {
                    max_step = max_step.max((s - prev).abs());
                }
                prev = s;
            }
            t += block as f32 / RATE;
        }
        let peak = out.iter().fold(0.0f32, |a, x| a.max(x.abs()));
        let hf = {
            // Energy above 16 kHz relative to total, over the high-rpm part:
            // the aliasing/fizz check.
            let s = spectrum(&out[(RATE * 5.0) as usize..(RATE * 5.0) as usize + 32768]);
            let k16 = (16_000.0 / (RATE / (2.0 * s.len() as f32))) as usize;
            let hi_e: f32 = s[k16..].iter().map(|m| m * m).sum();
            let all: f32 = s.iter().map(|m| m * m).sum();
            db((hi_e / all.max(1e-12)).sqrt())
        };
        println!("sweep {name:14} {lo:6.0}->{hi:6.0} peak {peak:.2} rms {:.1} dB  >16k {hf:.1} dB  start-step {max_step:.3}", db(rms(&out)));
        assert!(peak < 1.6, "{name} peak {peak}");
        write(&format!("engine_{name}_sweep"), &out);
    }
}

/// A V8 muscle car: starter, idle lope, blips, 1-2-3 shifts with ignition
/// cut, lift-off crackle.
#[test]
fn v8_drive_cycle_wav() {
    let spec = preset("v8").unwrap();
    let mut v = VehicleSound::new(RATE, spec);
    v.engine.start();
    let n = (RATE * 14.0) as usize;
    let mut out = vec![0.0; n];
    let mut t = 0.0f32;
    for chunk in out.chunks_mut(256) {
        let (rpm, thr, gear) = match t {
            t if t < 1.6 => (750.0, 0.0, 0),
            t if t < 3.0 => (750.0 + 2500.0 * ((t - 1.6) * 3.0).sin().max(0.0), ((t - 1.6) * 3.0).sin().max(0.0), 0),
            t if t < 5.5 => (1500.0 + (t - 3.0) / 2.5 * 4500.0, 1.0, 1),
            t if t < 7.5 => (3800.0 + (t - 5.5) / 2.0 * 2200.0, 1.0, 2),
            t if t < 10.0 => (4200.0 + (t - 7.5) / 2.5 * 1800.0, 1.0, 3),
            t => (6000.0 - (t - 10.0) * 800.0, 0.0, 3),
        };
        let speed = (t - 3.0).max(0.0) * 6.0;
        v.set(
            EngineInput { rpm, throttle: thr, load: if thr > 0.0 { thr } else { -0.2 }, gear, speed },
            TyreInput { slip: if (3.0..3.6).contains(&t) { 0.4 } else { 0.03 }, load: 1.0, speed, surface: Surface::Asphalt, grounded: 4 },
        );
        v.render(chunk, 1.0);
        t += 256.0 / RATE;
    }
    assert!(out.iter().all(|x| x.is_finite()));
    write("engine_v8_drive_cycle", &out);
}

/// Tyres: squeal ramp on asphalt, then gravel, grass, kerb, and a pass-by
/// with doppler.
#[test]
fn tyre_and_surface_wavs() {
    let spec = preset("inline4").unwrap();
    let mut v = VehicleSound::new(RATE, spec);
    v.engine_gain = 0.0;
    let n = (RATE * 12.0) as usize;
    let mut out = vec![0.0; n];
    let mut t = 0.0f32;
    for chunk in out.chunks_mut(256) {
        let (surface, slip) = match t {
            t if t < 4.0 => (Surface::Asphalt, (t / 3.0).min(1.0) * 0.7),
            t if t < 6.5 => (Surface::Gravel, 0.1),
            t if t < 9.0 => (Surface::Grass, 0.05),
            _ => (Surface::Kerb, 0.05),
        };
        v.set(EngineInput::default(), TyreInput { slip, load: 1.2, speed: 22.0, surface, grounded: 4 });
        v.render(chunk, 1.0);
        t += 256.0 / RATE;
    }
    assert!(out.iter().all(|x| x.is_finite()));
    let squeal = spectrum(&out[(RATE * 3.0) as usize..(RATE * 3.0) as usize + 32768]);
    println!("tyre squeal centroid {:.0} Hz, rms {:.1} dB", centroid(&squeal), db(rms(&out[(RATE * 3.0) as usize..(RATE * 4.0) as usize])));
    write("tyres_surfaces", &out);
}
