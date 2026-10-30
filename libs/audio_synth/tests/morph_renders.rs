//! Morph and the synth kit: notes rendered and measured (pitch, partials,
//! stereo width, filtering, modulation over time, the arp, granular and
//! spectral modes, samples), the patch text and the schema the AI reads.

use makepad_audio_synth::kit::lfo::{Lfo, LfoBlock};
use makepad_audio_synth::kit::osc::SampleBuf;
use makepad_audio_synth::kit::table::{self, Table};
use makepad_audio_synth::morph::{layout, Morph, MorphPatch, PRESETS};
use std::sync::Arc;

const RATE: f32 = 48_000.0;

fn engine(text: &str, samples: [Option<SampleBuf>; 3]) -> Morph {
    let patch = MorphPatch::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
    let src = patch.sources(samples);
    let mut m = Morph::new(RATE, Arc::new(patch), src);
    m.set_tempo(2.0);
    m
}

/// Render `secs` with `notes` held from the start (released at `hold`).
fn render(m: &mut Morph, notes: &[u8], hold: f32, secs: f32) -> (Vec<f32>, Vec<f32>) {
    let n = (secs * RATE) as usize;
    let mut l = vec![0.0; n];
    let mut r = vec![0.0; n];
    for note in notes {
        m.note_on(*note, 100);
    }
    let off = (hold * RATE) as usize;
    let mut at = 0;
    while at < n {
        let end = if at < off { off.min(n) } else { n };
        let end = end.min(at + 256);
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

/// Energy at `hz` (Goertzel), normalised by length.
fn tone(x: &[f32], hz: f32) -> f32 {
    let w = std::f32::consts::TAU * hz / RATE;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for v in x {
        let s = v + c * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    ((s1 * s1 + s2 * s2 - c * s1 * s2).max(0.0)).sqrt() / x.len() as f32
}

fn mono(l: &[f32], r: &[f32]) -> Vec<f32> {
    l.iter().zip(r).map(|(a, b)| (a + b) * 0.5).collect()
}

#[test]
fn every_preset_sounds_and_stays_bounded() {
    for (name, _, _) in PRESETS {
        let mut m = engine(&format!("preset={name}"), [None, None, None]);
        let (l, r) = render(&mut m, &[57, 64], 1.0, 1.6);
        assert!(l.iter().chain(r.iter()).all(|v| v.is_finite()), "{name}: not finite");
        let level = rms(&mono(&l, &r));
        let peak = l.iter().chain(r.iter()).fold(0.0f32, |a, v| a.max(v.abs()));
        assert!(level > 0.003, "{name}: silent (rms {level})");
        assert!(peak < 4.0, "{name}: peak {peak}");
    }
}

#[test]
fn sine_frame_plays_its_pitch() {
    let mut m = engine("f1_on=false a_rand_phase=0", [None, None, None]);
    let (l, r) = render(&mut m, &[69], 0.5, 0.5);
    let x = mono(&l, &r);
    let x = &x[4800..];
    assert!(tone(x, 440.0) > 20.0 * tone(x, 880.0), "fundamental {} vs octave {}", tone(x, 440.0), tone(x, 880.0));
}

#[test]
fn square_frame_has_odd_partials() {
    let mut m = engine("f1_on=false a_pos=1", [None, None, None]);
    let (l, r) = render(&mut m, &[57], 0.5, 0.5);
    let x = mono(&l, &r);
    let x = &x[4800..];
    let (h1, h2, h3) = (tone(x, 220.0), tone(x, 440.0), tone(x, 660.0));
    assert!(h3 > 0.2 * h1 && h3 > 5.0 * h2, "h1 {h1} h2 {h2} h3 {h3}");
}

#[test]
fn unison_widens_the_stereo_image() {
    let corr = |text: &str| {
        let mut m = engine(text, [None, None, None]);
        let (l, r) = render(&mut m, &[57], 0.6, 0.6);
        let (l, r) = (&l[4800..], &r[4800..]);
        let dot: f32 = l.iter().zip(r).map(|(a, b)| a * b).sum();
        dot / (l.iter().map(|a| a * a).sum::<f32>().sqrt() * r.iter().map(|b| b * b).sum::<f32>().sqrt()).max(1e-9)
    };
    let one = corr("a_pos=0.66");
    let seven = corr("a_pos=0.66 a_unison=7 a_detune=30 a_width=1");
    assert!(one > 0.99, "mono voice correlation {one}");
    assert!(seven < 0.9, "unison correlation {seven}");
}

#[test]
fn lowpass_takes_the_highs() {
    let high = |text: &str| {
        let mut m = engine(text, [None, None, None]);
        let (l, r) = render(&mut m, &[45], 0.5, 0.5);
        let x = mono(&l, &r);
        tone(&x[4800..], 110.0 * 21.0) / tone(&x[4800..], 110.0)
    };
    let open = high("a_pos=0.66 f1_on=false");
    let closed = high("a_pos=0.66 f1_type=lp24 f1_cutoff=300");
    assert!(closed < open * 0.05, "open {open} closed {closed}");
}

#[test]
fn an_lfo_on_the_table_moves_the_spectrum() {
    // Basic table: position 0 is a sine, 1 a square; an LFO walks it.
    let mut m = engine("f1_on=false lfo1_shape=saw_up lfo1_rate=0.5 lfo1_uni=true mod=lfo1,a_pos,1", [None, None, None]);
    let (l, r) = render(&mut m, &[57], 2.0, 2.0);
    let x = mono(&l, &r);
    let third = |w: &[f32]| tone(w, 660.0) / tone(w, 220.0);
    let early = third(&x[2400..12000]);
    let late = third(&x[72000..86400]);
    assert!(late > early * 4.0 + 0.05, "early {early} late {late}");
}

#[test]
fn the_arp_steps_through_the_chord() {
    // 120 bpm, sixteenths: c4, e4, g4 each an eighth of a second.
    let mut m = engine("arp_on=true arp_rate=n16 arp_gate=0.9 f1_on=false env1_release=0.01", [None, None, None]);
    let (l, r) = render(&mut m, &[60, 64, 67], 1.0, 1.0);
    let x = mono(&l, &r);
    let win = |i: usize| &x[i * 6000 + 600..i * 6000 + 5400];
    let (c, e, g) = (261.63, 329.63, 392.0);
    assert!(tone(win(0), c) > 3.0 * tone(win(0), e), "step 1 is c");
    assert!(tone(win(1), e) > 3.0 * tone(win(1), c), "step 2 is e");
    assert!(tone(win(2), g) > 3.0 * tone(win(2), e), "step 3 is g");
}

#[test]
fn spectral_even_only_drops_the_fundamental() {
    let mut m = engine("a_mode=spectral a_table=harmonic a_pos=1 a_odd=1 f1_on=false", [None, None, None]);
    let (l, r) = render(&mut m, &[57], 0.5, 0.5);
    let x = mono(&l, &r);
    let x = &x[4800..];
    assert!(tone(x, 440.0) > 10.0 * tone(x, 220.0), "h1 {} h2 {}", tone(x, 220.0), tone(x, 440.0));
}

#[test]
fn spectral_stretch_moves_partials_off_the_series() {
    let h2 = |text: &str| {
        let mut m = engine(text, [None, None, None]);
        let (l, r) = render(&mut m, &[57], 0.5, 0.5);
        tone(&mono(&l, &r)[4800..], 440.0)
    };
    let plain = h2("a_mode=spectral a_table=harmonic a_pos=1 f1_on=false");
    let stretched = h2("a_mode=spectral a_table=harmonic a_pos=1 a_stretch=0.2 f1_on=false");
    assert!(stretched < plain * 0.3, "plain {plain} stretched {stretched}");
}

#[test]
fn granular_over_the_table_sounds() {
    let mut m = engine("a_mode=granular a_grain=60 a_density=60 f1_on=false", [None, None, None]);
    let (l, r) = render(&mut m, &[57], 1.0, 1.0);
    assert!(rms(&mono(&l, &r)[9600..]) > 0.01);
}

fn sine_sample(hz: f32, secs: f32) -> SampleBuf {
    let data: Vec<f32> = (0..(secs * RATE) as usize).map(|i| (std::f32::consts::TAU * hz * i as f32 / RATE).sin() * 0.8).collect();
    SampleBuf { data: data.into(), rate: RATE }
}

#[test]
fn a_sample_plays_at_the_note() {
    // A 440 Hz recording, root a4, played an octave up.
    let mut m = engine("a_mode=sample a_root=69 f1_on=false", [Some(sine_sample(440.0, 1.0)), None, None]);
    let (l, r) = render(&mut m, &[81], 0.4, 0.4);
    let x = mono(&l, &r);
    let x = &x[2400..16000];
    assert!(tone(x, 880.0) > 10.0 * tone(x, 440.0));
}

#[test]
fn granular_reads_a_sample() {
    let mut m = engine("a_mode=granular a_root=69 a_grain=50 a_density=80 a_spray=0 f1_on=false", [Some(sine_sample(440.0, 1.0)), None, None]);
    let (l, r) = render(&mut m, &[69], 0.8, 0.8);
    let x = mono(&l, &r);
    let x = &x[9600..];
    assert!(tone(x, 440.0) > 5.0 * tone(x, 660.0));
}

#[test]
fn an_imported_table_keeps_the_sound() {
    // A sample with its 3rd partial strong, cut into frames by pitch.
    let data: Vec<f32> = (0..48_000).map(|i| {
        let t = i as f32 / RATE;
        (std::f32::consts::TAU * 200.0 * t).sin() * 0.5 + (std::f32::consts::TAU * 600.0 * t).sin() * 0.4
    }).collect();
    let t = Table::from_sample(&data, RATE, true);
    assert!(t.frames >= 2);
    let (h1, _) = t.partial(0.5, 0);
    let (h3, _) = t.partial(0.5, 2);
    let (h2, _) = t.partial(0.5, 1);
    assert!(h3 > 0.5 * h1 && h2 < 0.1 * h1, "h1 {h1} h2 {h2} h3 {h3}");
}

#[test]
fn user_frames_build_a_table() {
    let mut m = engine("a_table=user a_frames=1:0:0.5 f1_on=false", [None, None, None]);
    let (l, r) = render(&mut m, &[57], 0.5, 0.5);
    let x = mono(&l, &r);
    let x = &x[4800..];
    assert!(tone(x, 660.0) > 0.3 * tone(x, 220.0) && tone(x, 440.0) < 0.05 * tone(x, 220.0));
}

#[test]
fn warps_change_the_wave() {
    let h = |text: &str| {
        let mut m = engine(text, [None, None, None]);
        let (l, r) = render(&mut m, &[57], 0.5, 0.5);
        let x = mono(&l, &r);
        let x = &x[4800..];
        // The share of the power that is not the fundamental.
        let f = tone(x, 220.0);
        1.0 - 2.0 * f * f / (rms(x) * rms(x)).max(1e-12)
    };
    let plain = h("f1_on=false");
    for warp in ["sync", "fold", "drive", "bend_up", "pwm", "mirror", "quantize", "fm", "crush", "asym_up", "resonant", "rectify"] {
        let warped = h(&format!("f1_on=false b_on=true b_level=0 a_warp1={warp} a_warp1_amt=0.9"));
        assert!(warped > plain + 0.02, "{warp}: plain {plain} warped {warped}");
    }
}

/// The rack runs the shared effects family: every kind plays a note through without blowing up,
/// a delay's echo outlives the dry note, and the rack order is the patch's.
#[test]
fn the_rack_plays_the_shared_effects() {
    use makepad_audio_synth::kit::fx::KINDS;
    for (kind, _) in KINDS {
        let mut m = engine(&format!("a_pos=0.5 env1_release=0.05 fx={kind}"), [None, None, None]);
        let (l, r) = render(&mut m, &[60], 0.3, 1.0);
        assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 8.0), "{kind}");
    }
    let tail = |text: &str| {
        let mut m = engine(text, [None, None, None]);
        let (l, r) = render(&mut m, &[60], 0.2, 1.2);
        rms(&mono(&l, &r)[(0.45 * RATE) as usize..])
    };
    let dry = tail("env1_release=0.02");
    let wet = tail("env1_release=0.02 fx=delay delay_left=-2 delay_right=-2 delay_feedback=0.6 delay_mix=0.5");
    assert!(dry < 1e-4 && wet > 1e-3, "dry {dry} wet {wet}");
    let p = MorphPatch::parse("fx=reverb,delay,chorus").unwrap();
    assert_eq!(p.to_text().split(' ').find(|w| w.starts_with("fx=")), Some("fx=reverb,delay,chorus"));
}

/// The `tuning` choice and a scale of its own retune the keys.
#[test]
fn morph_plays_in_tune() {
    let e19 = makepad_audio_synth::kit::tuning::by_name("edo19").unwrap();
    let want = e19.key_hz(62).unwrap() as f32;
    let mut m = engine("a_pos=0 tuning=edo19 f1_on=false", [None, None, None]);
    let (l, r) = render(&mut m, &[62], 1.0, 0.5);
    let x = mono(&l, &r);
    assert!(tone(&x[4800..], want) > 4.0 * tone(&x[4800..], 293.66), "key 62 sounds two 19ths up");
    let p = MorphPatch::parse("tuning_scale=5/4,3/2,2/1").unwrap();
    assert_eq!(p, MorphPatch::parse(&p.to_text()).unwrap());
    let t = p.tuning().unwrap();
    assert!((t.key_hz(61).unwrap() / t.key_hz(60).unwrap() - 1.25).abs() < 1e-9);
    assert!(MorphPatch::parse("tuning_scale=0/3").is_err());
}

#[test]
fn bad_patches_are_refused_with_the_reason() {
    assert!(MorphPatch::parse("a_tabel=basic").unwrap_err().contains("a_tabel"));
    assert!(MorphPatch::parse("a_table=nope").unwrap_err().contains("@basic"));
    assert!(MorphPatch::parse("mod=lfo9,a_pos,0.5").unwrap_err().contains("lfo9"));
    assert!(MorphPatch::parse("mod=lfo1,a_table,0.5").unwrap_err().contains("cannot be modulated"));
    assert!(MorphPatch::parse("fx=reverb,reverb").unwrap_err().contains("twice"));
    assert!(MorphPatch::parse("preset=nothing").unwrap_err().contains("wide_saw"));
}

#[test]
fn the_text_form_round_trips() {
    for (name, _, _) in PRESETS {
        let p = MorphPatch::parse(&format!("preset={name}")).unwrap();
        let again = MorphPatch::parse(&p.to_text()).unwrap();
        assert_eq!(p, again, "{name}: {}", p.to_text());
    }
    let p = MorphPatch::parse("lfo2_shape=drawn lfo2_points=0:0:0,0.5:1:0.4,1:0:0 mod=lfo2,f1_cutoff,0.3,m1,inv,0.2,bi,0.5 macro_names=bright").unwrap();
    assert_eq!(p, MorphPatch::parse(&p.to_text()).unwrap());
}

#[test]
fn the_schema_describes_itself_for_the_ai() {
    let schema = &layout().schema;
    let text = schema.describe(&[]);
    for key in ["a_pos", "a_warp1", "f1_cutoff", "env4_release", "lfo8_shape", "m8", "arp_pattern", "delay_left", "glide"] {
        assert!(text.contains(&format!("  {key} ")), "{key} missing");
    }
    assert!(text.contains("f1_cutoff 20..20000 Hz log =4000 ~"), "{}", text.lines().find(|l| l.contains("f1_cutoff")).unwrap_or(""));
    let osc = schema.describe(&["osc_a"]);
    assert!(osc.contains("a_pos") && !osc.contains("b_pos"));
    // Token-lean: every line short.
    assert!(text.lines().all(|l| l.len() < 420));
}

#[test]
fn chaos_lfos_stay_bounded_and_move() {
    let schema = &layout().schema;
    let ix = &layout().ix;
    for shape in ["lorenz", "rossler", "star", "rose", "eight"] {
        let mut vals = schema.defaults();
        vals[ix.lfo[0].shape] = schema.params[ix.lfo[0].shape].option(shape).unwrap() as f32;
        let b = LfoBlock::read(&vals[..], &ix.lfo[0]);
        let mut lfo = Lfo::new(7);
        lfo.trigger(&b);
        let mut seen = (f32::MAX, f32::MIN);
        for _ in 0..4000 {
            lfo.advance(&b, &[], 1.0 / 1500.0, 2.0, 0.0);
            let [x, y] = lfo.out();
            assert!(x.abs() <= 1.0 && y.abs() <= 1.0, "{shape}: {x} {y}");
            seen = (seen.0.min(x), seen.1.max(x));
        }
        assert!(seen.1 - seen.0 > 0.3, "{shape} does not move: {seen:?}");
    }
}

#[test]
fn builtin_tables_are_band_limited() {
    // The top level holds one partial: a sine.
    let t = table::builtin(0);
    let mut pic = vec![0.0; 64];
    t.picture(1.0, &mut pic);
    assert!(pic.iter().fold(0.0f32, |a, v| a.max(v.abs())) > 0.5);
    assert_eq!(Table::level_for(0.4), 10);
    assert_eq!(Table::level_for(1.0 / 2048.0), 0);
}
