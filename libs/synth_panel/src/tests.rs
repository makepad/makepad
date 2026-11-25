//! The fields' tapers and how values are written, and the display maths.

use crate::spec;
use makepad_edit_values::Taper;

#[test]
fn tapers_and_source_text() {
    let cutoff = spec::spec("Synth", "cutoff").unwrap();
    assert_eq!(cutoff.taper, Taper::Log);
    let mid = cutoff.denorm(0.5);
    assert!((cutoff.norm(mid) - 0.5).abs() < 1e-9);
    assert!((mid - (30.0f64 * 16000.0).sqrt()).abs() < 1e-6);
    assert_eq!(cutoff.source_text(812.4), "812");
    assert_eq!(spec::spec("Synth", "sustain").unwrap().source_text(0.62), "0.62");
    assert_eq!(spec::spec("Synth", "attack").unwrap().source_text(0.00437), "0.0044");
    assert_eq!(spec::spec("Synth", "detune").unwrap().source_text(-0.0), "0");
    assert_eq!(spec::spec("Synth", "unison").unwrap().source_text(2.6), "3");
    let delay = spec::spec("Delay", "feedback").unwrap();
    assert_eq!((delay.min, delay.max), (0.0, 0.949999988079071));
    assert_eq!(cutoff.display(1250.0), "1.25 kHz");
}

#[test]
fn shader_fields_control_travel_and_display() {
    let shader = makepad_score_player::shader::compile(r#"
        let cutoff = param(1200, 20, 16000, {label: "Cutoff" group: filter unit: hz curve: log})
        let rise_time = param(0, 0, 1000, {unit: ms curve: exp})
        let wave = param(tri, {choices: [saw "soft pulse" tri]})
        let mix_level = param(0, -1, 1, {unit: percent step: 0.25})
        fn voice() { vec2(wave, cutoff) }
    "#).unwrap();
    let spec = |name| spec::ParamSpec::from_shader(&shader, shader.param_index(name).unwrap());
    let (cut, rise, wave, mix) = (spec("cutoff"), spec("rise_time"), spec("wave"), spec("mix_level"));
    assert_eq!((&*cut.label, &*cut.group, cut.min, cut.max, cut.default), ("Cutoff", "filter", 20.0, 16000.0, 1200.0));
    assert_eq!(&*cut.about, "Cutoff (filter): 20 Hz to 16000 Hz, log, default 1200 Hz");
    assert_eq!(cut.display(cut.min), "20 Hz");
    assert_eq!((&*rise.label, rise.display(30.6)), ("RISE TIME", "31 ms".into()));
    assert_eq!((cut.taper, rise.taper, wave.taper), (Taper::Log, Taper::Exp, Taper::Int));
    assert_eq!(wave.options, ["saw", "soft pulse", "tri"]);
    assert_eq!((wave.default, wave.source_text(1.0)), (2.0, "\"soft pulse\"".into()));
    assert_eq!(wave.parse("\"soft pulse\"", 0.0), Ok(1.0));
    assert!((cut.denorm(0.5) - (20.0f64 * 16000.0).sqrt()).abs() < 1e-9, "logarithmic midpoint");
    assert!((rise.denorm(0.5) - 30.6).abs() < 0.1, "exponential midpoint");
    for spec in [&cut, &rise, &wave] {
        assert_eq!((spec.denorm(0.0), spec.denorm(1.0)), (spec.min, spec.max), "{} endpoints", spec.key);
        for t in [0.0, 0.5, 1.0] {
            assert!((spec.norm(spec.denorm(t)) - t).abs() < 1e-9, "{} travel {t}", spec.key);
        }
    }
    assert!(mix.bipolar);
    assert_eq!(mix.parse("37%", 0.0), Ok(0.25));
    assert_eq!(mix.display(0.25), "25%");
    assert_eq!(mix.source_text(f64::INFINITY), "0", "a nonfinite edit uses the shader default");
}

#[test]
fn display_maths() {
    use crate::dsp;
    // A resonant ladder peaks near its cutoff; far above, it is steep.
    let peak = dsp::filter_db(0, 1000.0, 12.0, 1000.0);
    assert!(peak > 3.0, "{peak}");
    assert!(dsp::filter_db(0, 1000.0, 0.7, 8000.0) < -30.0);
    assert!(dsp::filter_db(3, 1000.0, 0.7, 100.0) < -30.0, "high pass");
    let mut row = [0.0; dsp::ROW];
    let corners = dsp::adsr_row(&mut row, 0.01, 0.3, 0.5, 0.5);
    assert!(corners[0] < corners[1] && corners[1] < corners[2]);
    assert!((row[(corners[1] * 255.0) as usize + 4] - 0.5).abs() < 0.05, "the sustain level");
    assert!(row[255] < 0.02);
    let mut s = dsp::Spectrum::new(1024);
    let tone: Vec<f32> = (0..2048).map(|i| (i as f32 * std::f32::consts::TAU * 1000.0 / 48000.0).sin()).collect();
    let mut bands = [0.0; 48];
    s.analyse(&tone, 48000.0, &mut bands, 1.0);
    let top = bands.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
    let f = 30.0 * 600f32.powf(top as f32 / 48.0);
    assert!((700.0..1300.0).contains(&f), "{f}");
}
