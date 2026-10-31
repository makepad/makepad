//! The analog components as a user meets them: picked one at a time in a
//! Morph voice next to the kit's own, played through the engine. No
//! allocation on the audio thread, CPU per voice, stability, and the
//! presets' renders.

use makepad_audio_synth::morph::{layout, Morph, MorphPatch, PRESETS};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

const RATE: f32 = 48_000.0;

struct Counting;

thread_local! {
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn engine(text: &str) -> Morph {
    let patch = MorphPatch::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
    let src = patch.sources([None, None, None]);
    Morph::new(RATE, Arc::new(patch), src)
}

fn render(m: &mut Morph, notes: &[u8], hold: f32, secs: f32) -> (Vec<f32>, Vec<f32>) {
    let n = (secs * RATE) as usize;
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    for note in notes {
        m.note_on(*note, 100);
    }
    let off = (hold * RATE) as usize;
    let mut at = 0;
    while at < n {
        let end = (if at < off { off.min(n) } else { n }).min(at + 256);
        m.render(&mut l[at..end], &mut r[at..end]);
        at = end;
        if at == off {
            for note in notes {
                m.note_off(*note);
            }
        }
    }
    (l, r)
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn clean(name: &str, l: &[f32], r: &[f32]) {
    for v in l.iter().chain(r) {
        assert!(v.is_finite(), "{name}: not finite");
        assert!(*v == 0.0 || v.abs() > f32::MIN_POSITIVE, "{name}: denormal {v}");
        assert!(v.abs() < 8.0, "{name}: {v}");
    }
}

/// Every component on its own beside the kit's others: each sounds, none
/// needs the rest.
#[test]
fn each_component_is_selectable_alone_next_to_the_kits_own() {
    let cases = [
        ("oscillator alone", "a_mode=analog"),
        ("oscillator next to a table", "a_mode=analog b_on=true b_table=harmonic b_pos=0.5"),
        ("oscillator synced to a table", "b_on=true b_mode=analog b_sync=reset b_warp_src=a b_semi=7"),
        ("filter alone", "a_pos=0.66 f1_type=analog_lp f1_cutoff=900 f1_res=0.5"),
        ("each filter wiring hp", "a_pos=0.66 f1_type=analog_hp f1_cutoff=600"),
        ("each filter wiring bp", "a_pos=0.66 f1_type=analog_bp f1_cutoff=600"),
        ("each filter wiring ap", "a_pos=0.66 f1_type=analog_ap f1_cutoff=600"),
        ("each filter wiring sv", "a_pos=0.66 f1_type=analog_sv_lp f1_cutoff=600 f2_on=true f2_type=analog_notch f2_cutoff=2000"),
        ("second filter only", "a_pos=0.66 f1_type=ladder f2_on=true f2_type=analog_lp f2_cutoff=700"),
        ("envelope alone", "env1_model=analog env1_attack=0.01 env1_decay=0.1 env1_sustain=0.5 env1_release=0.1"),
        ("mod envelope alone", "env2_model=analog env2_decay=0.1 env2_sustain=0 mod=env2,f1_cutoff,0.4"),
        ("amplifier alone", "amp_type=analog"),
        ("amplifier exponential", "amp_type=analog amp_law=exponential env1_decay=0.3 env1_sustain=0.7"),
    ];
    for (name, text) in cases {
        let mut m = engine(text);
        let (l, r) = render(&mut m, &[60], 0.5, 1.0);
        clean(name, &l, &r);
        let level = rms(&l[4800..24000]);
        eprintln!("{name}: rms {level:.3}");
        assert!(level > 0.005, "{name}: silent, {level}");
    }
}

#[test]
fn schema_offers_them_where_a_user_picks_the_kits_own() {
    let s = &layout().schema;
    assert!(s.param("a_mode").unwrap().options.contains(&"analog"));
    assert!(s.param("f1_type").unwrap().options.contains(&"analog_lp"));
    assert!(s.param("env3_model").unwrap().options.contains(&"analog"));
    assert!(s.param("amp_type").unwrap().options.contains(&"analog"));
    for key in ["a_pw", "b_saw", "c_drift", "f2_res", "env2_decay", "amp_gain", "amp_second"] {
        assert!(s.param(key).unwrap().modulatable, "{key} takes mod rows");
    }
}

/// The filter envelope and PWM reach the components through mod rows.
#[test]
fn mod_rows_move_them() {
    let brightness = |text: &str| {
        let x = render(&mut engine(text), &[48], 0.5, 0.5).0;
        let d: Vec<f32> = x[4800..].windows(2).map(|w| w[1] - w[0]).collect();
        rms(&d) / rms(&x[4800..])
    };
    let shut = brightness("a_mode=analog f1_type=analog_lp f1_cutoff=200");
    let moved = brightness("a_mode=analog f1_type=analog_lp f1_cutoff=200 m1=1 mod=m1,f1_cutoff,0.6");
    eprintln!("mod row on the cutoff: brightness {shut:.4} -> {moved:.4}");
    assert!(moved > shut * 2.0);
    let hard = rms(&render(&mut engine("a_mode=analog a_saw=0 a_pulse=1 a_pw=0.05"), &[60], 0.5, 0.5).0[4800..]);
    let wide = rms(&render(&mut engine("a_mode=analog a_saw=0 a_pulse=1 a_pw=0.05 m1=1 mod=m1,a_pw,0.45"), &[60], 0.5, 0.5).0[4800..]);
    assert!((hard - wide).abs() > 1e-4, "{hard} {wide}");
}

#[test]
fn presets_all_sound_and_stay_in_range() {
    for (name, _, text) in PRESETS.iter().filter(|p| ["wide_saw", "reese", "pluck", "wobble", "vowel_pad"].contains(&p.0)) {
        let (l, _) = render(&mut engine(text), &[45, 52, 60], 1.0, 2.5);
        eprintln!("(kit preset) {name}: rms {:.3} peak {:.3}", rms(&l[..48000]), l.iter().fold(0f32, |a, v| a.max(v.abs())));
    }
    for (name, about, text) in PRESETS.iter().filter(|p| p.0.starts_with("analog_")) {
        assert!(!about.is_empty());
        let mut m = engine(text);
        m.set_mod_wheel(0.5);
        let (l, r) = render(&mut m, &[45, 52, 60], 1.0, 2.5);
        clean(name, &l, &r);
        let peak = l.iter().fold(0f32, |a, v| a.max(v.abs()));
        let level = rms(&l[..48000]);
        eprintln!("{name}: rms {level:.3} peak {peak:.3}");
        assert!(level > 0.003 && peak > 0.05 && peak < 4.0, "{name}: rms {level} peak {peak}");
    }
    assert!(PRESETS.iter().filter(|p| p.0.starts_with("analog_")).count() >= 8);
}

#[test]
fn extreme_mod_rows_on_a_full_analog_voice_stay_finite() {
    let text = "a_mode=analog a_pulse=1 b_on=true b_mode=analog b_sync=hard b_warp_src=a b_fm=1 b_fm_exp=4 c_on=true c_mode=analog c_sync=soft c_warp_src=b f1_type=analog_lp f1_res=1 f1_drive=1 f2_on=true f2_type=analog_sv_bp f2_res=1 amp_type=analog amp_idle=1 env1_model=analog env1_attack=0.001 env2_model=analog lfo1_rate=20 lfo1_shape=step lfo2_rate=20 lfo2_shape=square mod=lfo1,f1_cutoff,1 mod=lfo2,f1_res,1 mod=lfo1,a_semi,1 mod=lfo2,b_semi,1 mod=lfo1,a_pw,1 mod=lfo2,f2_cutoff,1 mod=lfo1,amp_gain,1";
    let mut m = engine(text);
    let (l, r) = render(&mut m, &[24, 60, 96, 108], 3.0, 4.0);
    for v in l.iter().chain(&r) {
        assert!(v.is_finite());
        assert!(*v == 0.0 || v.abs() > f32::MIN_POSITIVE, "{v}");
    }
}

#[test]
fn the_audio_thread_never_allocates() {
    for (name, _, text) in PRESETS.iter().filter(|p| p.0.starts_with("analog_")) {
        let mut m = engine(text);
        let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
        let before = COUNT.with(|c| c.get());
        for i in 0..1000u32 {
            if i % 50 == 0 {
                m.note_on(40 + (i / 50 % 40) as u8, 100);
            }
            if i % 50 == 40 {
                m.note_off(40 + (i / 50 % 40) as u8);
            }
            m.render(&mut l, &mut r);
        }
        let n = COUNT.with(|c| c.get()) - before;
        assert_eq!(n, 0, "{name}: {n} allocations");
    }
}

/// CPU per voice against the audio thread's budget. A generous bound (the
/// numbers are printed for the report; boxes differ).
#[test]
fn cpu_per_voice() {
    let cases = [
        ("kit reference: table + lp24", "f1_type=lp24 f1_cutoff=2000", 1),
        ("analog osc (saw, ideal)", "a_mode=analog a_drift=0 f1_on=false", 1),
        ("analog osc (saw, pulse, tri)", "a_mode=analog a_tri=0.5 a_pulse=0.5 f1_on=false", 1),
        ("analog filter alone", "f1_type=analog_lp f1_cutoff=2000 f1_res=0.5", 1),
        ("analog sv filter alone", "f1_type=analog_sv_lp f1_cutoff=2000 f1_res=0.5", 1),
        ("analog amp alone", "amp_type=analog f1_on=false", 1),
        ("analog env alone", "env1_model=analog f1_on=false", 1),
        ("full analog voice", "a_mode=analog a_pulse=0.5 b_on=true b_mode=analog b_fine=5 f1_type=analog_lp f1_cutoff=2000 f1_res=0.5 env1_model=analog env2_model=analog amp_type=analog", 1),
        ("full analog voice, 16 voices", "a_mode=analog a_pulse=0.5 b_on=true b_mode=analog b_fine=5 f1_type=analog_lp f1_cutoff=2000 f1_res=0.5 env1_model=analog env2_model=analog amp_type=analog", 16),
    ];
    let mut full = 0.0;
    for (name, text, voices) in cases {
        let mut m = engine(text);
        for v in 0..voices {
            m.note_on(40 + v * 3, 100);
        }
        let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
        for _ in 0..200 {
            m.render(&mut l, &mut r);
        }
        let blocks = 4000;
        let t = std::time::Instant::now();
        for _ in 0..blocks {
            m.render(&mut l, &mut r);
        }
        let ns = t.elapsed().as_nanos() as f64 / (blocks * 256) as f64;
        // One second of audio is 48000 frames: percent of one core.
        let per_voice = ns / voices as f64;
        let core = per_voice * 48_000.0 / 1e7;
        eprintln!("{name}: {per_voice:.0} ns/frame per voice, {core:.2}% of a core per voice");
        if voices == 1 && name.starts_with("full") {
            full = core;
        }
        std::hint::black_box((&l, &r));
    }
    assert!(full < 5.0, "{full}% of a core per voice");
}

/// Renders for the ear: `ANALOG_RENDERS=<dir>` writes the example files.
#[test]
fn renders_for_the_ear() {
    let Ok(dir) = std::env::var("ANALOG_RENDERS") else { return };
    std::fs::create_dir_all(&dir).unwrap();
    let phrase: &[(&str, &[u8], f32, f32)] = &[
        ("analog_bass", &[33], 0.45, 1.0),
        ("analog_brass", &[48, 55, 60, 64], 1.6, 2.6),
        ("analog_strings", &[48, 60, 64, 67], 3.0, 5.0),
        ("analog_sync_lead", &[60], 1.2, 2.0),
        ("analog_reversal", &[48], 2.0, 2.6),
        ("analog_pluck", &[60, 67, 72], 0.2, 1.4),
        ("analog_whistle", &[72], 1.4, 2.2),
        ("analog_phase_keys", &[48, 60, 64, 67], 3.0, 4.0),
        ("analog_fm_bell", &[72], 0.5, 3.0),
        ("analog_state_bass", &[36], 0.5, 1.2),
    ];
    for (name, notes, hold, secs) in phrase {
        let text = PRESETS.iter().find(|p| p.0 == *name).unwrap().2;
        let mut m = engine(text);
        let (l, r) = render(&mut m, notes, *hold, *secs);
        let path = format!("{dir}/{name}.wav");
        std::fs::write(&path, makepad_audio_synth::wav::stereo_wav_16(&l, &r, RATE as u32)).unwrap();
        eprintln!("wrote {path}");
    }
}
