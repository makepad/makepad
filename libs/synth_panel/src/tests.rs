//! The fields' tapers and how values are written, and the display maths.

use crate::spec::{self, Taper};

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
