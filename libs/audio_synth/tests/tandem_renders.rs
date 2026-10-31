//! Tandem: notes rendered and measured (TESTS.md B2-B5, B8, B9 and
//! sound-producing checks): pitch, the amplitude and pan laws, envelope
//! timing at block resolution, voice management, filters, effects, and
//! determinism across host buffer sizes.

use makepad_audio_synth::kit::core::{megapan, SlotValue};
use makepad_audio_synth::tandem::{Patch, Tandem};

const RATE: f32 = 48_000.0;

fn engine(p: &Patch) -> Tandem {
    let mut t = Tandem::new(RATE);
    t.load(p);
    t
}

/// A plain init patch with osc 1 of type `ty`, retrigger on, no drift.
fn init(ty: f32) -> Patch {
    let mut p = Patch::init();
    p.set("a_osc1_type", ty);
    p.set("a_osc1_retrigger", 1.0);
    p
}

fn render(t: &mut Tandem, secs: f32, chunk: usize) -> (Vec<f32>, Vec<f32>) {
    let n = (secs * RATE) as usize;
    let mut l = vec![0.0; n];
    let mut r = vec![0.0; n];
    let mut at = 0;
    while at < n {
        let end = (at + chunk).min(n);
        t.render(&mut l[at..end], &mut r[at..end]);
        at = end;
    }
    (l, r)
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Period by autocorrelation with parabolic refinement, in Hz.
fn pitch(x: &[f32], lo: f32, hi: f32) -> f32 {
    let min_lag = (RATE / hi) as usize;
    let max_lag = (RATE / lo) as usize + 2;
    let n = x.len() - max_lag - 1;
    let ac = |lag: usize| -> f64 { (0..n).map(|i| x[i] as f64 * x[i + lag] as f64).sum() };
    let vals: Vec<f64> = (min_lag..=max_lag).map(ac).collect();
    let (k, _) = vals.iter().enumerate().skip(1).take(vals.len() - 2).fold((1, f64::MIN), |b, (i, v)| if *v > b.1 { (i, *v) } else { b });
    let (a, b, c) = (vals[k - 1], vals[k], vals[k + 1]);
    let off = 0.5 * (a - c) / (a - 2.0 * b + c);
    RATE / (min_lag as f64 + k as f64 + off) as f32
}

fn cents(f: f32, want: f32) -> f32 {
    1200.0 * (f / want).log2()
}

#[test]
fn every_osc_type_plays_note_69() {
    for ty in [0.0, 1.0, 2.0, 5.0, 6.0, 7.0, 8.0, 9.0, 11.0] {
        let mut t = engine(&init(ty));
        t.note_on(0, 69, 100);
        let (l, _) = render(&mut t, 0.6, 256);
        assert!(l.iter().all(|v| v.is_finite()), "type {ty}: NaN");
        let tail = &l[9600..];
        let level = rms(tail);
        assert!(level > 1e-3 && level < 4.0, "type {ty}: level {level}");
        if ty != 9.0 {
            let f = pitch(tail, 300.0, 600.0);
            assert!(cents(f, 440.0).abs() < 2.0, "type {ty}: {f} Hz");
        }
    }
}

#[test]
fn amplitude_law_cubes_the_level() {
    let level = |a: f32| {
        let mut p = init(1.0);
        p.set("a_level_o1", a);
        let mut t = engine(&p);
        t.note_on(0, 69, 127);
        let (l, _) = render(&mut t, 0.5, 256);
        rms(&l[9600..])
    };
    let full = level(1.0);
    let half = level(0.5);
    let db = 20.0 * (half / full).log10();
    let want = 20.0 * 0.125f32.log10();
    assert!((db - want).abs() < 0.01, "{db} vs {want}");
}

#[test]
fn pan_law() {
    let gains = |pan: f32| {
        let mut p = init(1.0);
        p.set("a_pan", pan);
        let mut t = engine(&p);
        t.note_on(0, 69, 127);
        let (l, r) = render(&mut t, 0.5, 256);
        (rms(&l[9600..]), rms(&r[9600..]))
    };
    let (l0, r0) = gains(0.0);
    for pan in [-1.0f32, -0.5, 0.5, 1.0] {
        let (l, r) = gains(pan);
        let (gl, gr) = megapan(pan);
        assert!((l / l0 - gl).abs() < 1e-3, "pan {pan}: L {}", l / l0);
        assert!((r / r0 - gr).abs() < 1e-3, "pan {pan}: R {}", r / r0);
    }
}

#[test]
fn attack_peaks_on_its_block() {
    let v = -2.0f32; // 0.25 s
    let mut p = init(1.0);
    p.set("a_env1_attack", v);
    p.set("a_env1_attack_shape", 1.0);
    let mut t = engine(&p);
    t.note_on(0, 69, 127);
    let (l, _) = render(&mut t, 0.6, 32);
    // Block envelope: peak of each 32-sample block.
    let peaks: Vec<f32> = l.chunks(32).map(|b| b.iter().fold(0.0f32, |m, x| m.max(x.abs()))).collect();
    let top = peaks[peaks.len() - 20..].iter().cloned().fold(0.0f32, f32::max);
    let first = peaks.iter().position(|p| *p >= top * 0.995).unwrap();
    let want = (v.exp2() * RATE / 32.0).ceil() as usize;
    assert!((first as i64 - want as i64).abs() <= 3, "peak at block {first}, want {want}");
}

#[test]
fn sine_is_clean_after_oversampling() {
    let mut t = engine(&init(1.0));
    t.note_on(0, 69, 127);
    let (l, _) = render(&mut t, 1.0, 256);
    let x = &l[RATE as usize / 2..RATE as usize / 2 + 16384];
    // Goertzel at harmonics 2..10: all far below the fundamental.
    let g = |f: f32| {
        let w = std::f64::consts::TAU * f as f64 / RATE as f64;
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for (i, v) in x.iter().enumerate() {
            let win = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / x.len() as f64).cos();
            let s = *v as f64 * win + 2.0 * w.cos() * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2).sqrt()
    };
    let f0 = g(440.0);
    for h in 2..10 {
        let db = 20.0 * (g(440.0 * h as f32) / f0).log10();
        assert!(db < -90.0, "harmonic {h}: {db} dB");
    }
}

#[test]
fn poly_limit_steals() {
    let mut p = init(0.0);
    p.set("polylimit", 4.0);
    let mut t = engine(&p);
    for k in 0..6 {
        t.note_on(0, 60 + k, 100);
        let _ = render(&mut t, 0.02, 32);
    }
    let gated = t.gated_keys(0);
    assert_eq!(gated.len(), 4, "{gated:?}");
    assert!(gated.contains(&65) && gated.contains(&62) && !gated.contains(&60));
    let _ = render(&mut t, 0.2, 32);
    assert!(t.voices(0) <= 4 + 3);
}

#[test]
fn mono_single_trigger_is_legato() {
    let mut p = init(0.0);
    p.set("a_polymode", 2.0);
    let mut t = engine(&p);
    t.note_on(0, 60, 100);
    let _ = render(&mut t, 0.05, 32);
    t.note_on(0, 64, 100);
    let _ = render(&mut t, 0.05, 32);
    assert_eq!(t.gated_keys(0), vec![64]);
    assert_eq!(t.voices(0), 1);
    t.note_off(0, 64, 64);
    let _ = render(&mut t, 0.05, 32);
    assert_eq!(t.gated_keys(0), vec![60], "back to the held key");
    t.note_off(0, 60, 64);
    let _ = render(&mut t, 0.5, 32);
    assert_eq!(t.voices(0), 0);
}

#[test]
fn mono_glide_reaches_the_new_key() {
    let mut p = init(1.0);
    p.set("a_polymode", 1.0);
    p.set("a_portamento", -3.0); // 0.125 s
    let mut t = engine(&p);
    t.note_on(0, 57, 127);
    let _ = render(&mut t, 0.2, 32);
    t.note_on(0, 69, 127);
    let (early, _) = render(&mut t, 0.03, 32);
    let (late, _) = render(&mut t, 0.5, 32);
    let fe = pitch(&early[200..], 150.0, 600.0);
    let fl = pitch(&late[12000..], 300.0, 600.0);
    assert!(fe < 400.0, "glide starts low: {fe}");
    assert!(cents(fl, 440.0).abs() < 2.0, "glide ends on 440: {fl}");
}

#[test]
fn lowpass_filter_darkens() {
    let bright = |cut: Option<f32>| {
        let mut p = init(0.0);
        if let Some(c) = cut {
            p.set("a_filter1_type", 1.0);
            p.set("a_filter1_cutoff", c);
        }
        let mut t = engine(&p);
        t.note_on(0, 57, 100);
        let (l, _) = render(&mut t, 0.5, 256);
        // High band energy: first difference RMS over signal RMS.
        let x = &l[9600..];
        let d: Vec<f32> = x.windows(2).map(|w| w[1] - w[0]).collect();
        rms(&d) / rms(x)
    };
    let open = bright(None);
    let closed = bright(Some(-24.0));
    assert!(closed < open * 0.3, "{closed} vs {open}");
}

#[test]
fn insert_delay_echoes() {
    let mut p = init(1.0);
    p.set("fx1_type", 1.0);
    let mut t = engine(&p);
    t.note_on(0, 69, 127);
    let _ = render(&mut t, 0.05, 32);
    t.note_off(0, 69, 64);
    let (l, r) = render(&mut t, 2.0, 32);
    // Past the release (31 ms): only echoes remain.
    let late = rms(&l[4800..RATE as usize]) + rms(&r[4800..RATE as usize]);
    assert!(late > 1e-4, "echo tail {late}");
    assert!(l.iter().chain(r.iter()).all(|v| v.is_finite()));
}

#[test]
fn every_filter_type_and_config_is_finite() {
    for ty in 0..36 {
        for config in [0.0, 3.0, 5.0, 7.0] {
            let mut p = init(0.0);
            p.set("a_filter1_type", ty as f32);
            p.set("a_filter2_type", ((ty + 7) % 36) as f32);
            p.set("a_filter1_resonance", 0.9);
            p.set("a_fb_config", config);
            p.set("a_feedback", 0.3);
            p.set("a_ws_type", (ty % 43) as f32);
            let mut t = engine(&p);
            t.note_on(0, 48, 127);
            t.note_on(0, 60, 127);
            let (l, r) = render(&mut t, 0.3, 64);
            let peak = l.iter().chain(r.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak.is_finite() && peak <= 8.0, "type {ty} config {config}: {peak}");
        }
    }
}

#[test]
fn every_fx_type_is_finite() {
    for ty in 0..32 {
        let mut p = init(0.0);
        p.set("fx1_type", ty as f32);
        p.set("fx7_type", ((ty + 5) % 32) as f32);
        let mut t = engine(&p);
        t.note_on(0, 60, 127);
        let (l, r) = render(&mut t, 0.3, 64);
        t.note_off(0, 60, 64);
        let (l2, r2) = render(&mut t, 0.3, 64);
        assert!(l.iter().chain(r.iter()).chain(l2.iter()).chain(r2.iter()).all(|v| v.is_finite()), "fx {ty}");
    }
}

#[test]
fn deterministic_across_buffer_sizes() {
    let mut p = init(0.0);
    p.set("a_level_noise", 0.5);
    p.set("a_mute_noise", 0.0);
    p.set("a_filter1_type", 2.0);
    p.set("fx1_type", 2.0);
    let run = |chunk: usize| {
        let mut t = engine(&p);
        t.note_on(0, 60, 100);
        render(&mut t, 0.5, chunk)
    };
    let a = run(32);
    for chunk in [7, 100, 512] {
        let b = run(chunk);
        assert!(a == b, "buffer size {chunk} changed the output");
    }
}

#[test]
fn modwheel_routing_moves_cutoff() {
    // A mod wheel routing on filter 1 cutoff: wheel up brightens.
    let xml = r#"<?xml version="1.0"?><patch revision="31"><parameters>
        <a_osc1_retrigger type="0" value="1"/>
        <a_filter1_type type="0" value="1"/>
        <a_filter1_cutoff type="2" value="-30"><modrouting source="6" depth="60" muted="0" source_index="0"/></a_filter1_cutoff>
        </parameters></patch>"#;
    let p = Patch::from_payload(xml.as_bytes(), makepad_audio_synth::tandem::LoadMode::Patch).unwrap();
    let measure = |wheel: u8| {
        let mut t = engine(&p);
        t.control_change(0, 1, wheel);
        let _ = render(&mut t, 0.3, 32);
        t.note_on(0, 57, 100);
        let (l, _) = render(&mut t, 0.5, 32);
        let x = &l[9600..];
        let d: Vec<f32> = x.windows(2).map(|w| w[1] - w[0]).collect();
        rms(&d) / rms(x)
    };
    assert!(measure(127) > measure(0) * 2.0);
}

#[test]
fn slot_value_defaults_hold() {
    let p = Patch::init();
    let v: SlotValue = *p.global(9);
    assert_eq!(v.v, 16.0);
}

#[test]
fn formula_lfo_moves_pitch() {
    use makepad_audio_synth::tandem::catalog::catalog;
    use makepad_audio_synth::tandem::patch::Routing;
    let mut p = init(1.0);
    p.set("a_lfo0_shape", 9.0);
    p.formulae[0][0] = Some("function process(state)\n    state.output = 1\n    return state\nend\n".to_string());
    let dest = catalog().index("a_osc1_pitch").unwrap() as u16;
    p.routings.push(Routing { dest, source: 17, depth: 7.0, muted: false, source_index: 0, source_scene: 0 });
    let mut t = engine(&p);
    assert!(t.program().problems.is_empty(), "{:?}", t.program().problems);
    t.note_on(0, 69, 127);
    let (l, _) = render(&mut t, 0.6, 32);
    let f = pitch(&l[9600..], 500.0, 800.0);
    assert!(cents(f, 440.0 * (7.0f32 / 12.0).exp2()).abs() < 3.0, "{f} Hz");
}

/// A tuning retunes the keys: in 19 equal steps key 61 sounds one 19th of an octave above
/// middle C (degree 0 on key 60, A4 = 440 Hz); "retune everything" moves the oscillator pitch the
/// same way; an unmapped key stays silent.
#[test]
fn tunings_retune_the_keys() {
    use makepad_audio_synth::kit::tuning::{by_name, Tuning};
    let e19 = by_name("edo19").unwrap();
    let want = e19.key_hz(61).unwrap() as f32;
    for mode in [1.0f32, 0.0] {
        let mut p = init(1.0);
        p.nonparam.tuning_mode = mode as i32;
        let mut t = engine(&p);
        t.set_tuning(Some(e19.clone()));
        t.note_on(0, 61, 127);
        let (l, _) = render(&mut t, 0.5, 32);
        let f = pitch(&l[4800..], want * 0.8, want * 1.25);
        assert!(cents(f, want).abs() < 3.0, "mode {mode}: {f} Hz vs {want}");
    }
    let scl = "gaps\n2\n600.0\n1200.0\n";
    let kbm = "2\n0\n127\n60\n69\n440.0\n2\n0\nx\n";
    let mut t = engine(&init(1.0));
    t.set_tuning(Some(std::sync::Arc::new(Tuning::from_text("gaps", scl, kbm).unwrap())));
    t.note_on(0, 61, 127);
    let (l, _) = render(&mut t, 0.2, 32);
    assert!(rms(&l) < 1e-6, "unmapped key 61 is silent");
}
