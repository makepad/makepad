//! Offline renders of every instrument playing a short phrase through the
//! score sequencer, written as WAVs for listening (SYNTH_RENDER_DIR, with a
//! scratch default) and checked for level, NaN and render speed.
#![cfg(feature = "instruments")]

use makepad_audio_synth::instruments::{build, parse_kind, InstrumentKind, SoundFontInstrument};
use makepad_audio_synth::score::{Score, SeqEvent, Sequencer};
use makepad_audio_synth::wav::stereo_wav_16;
use makepad_audio_synth::{Control, Instrument};
use std::time::Instant;

const RATE: f32 = 48_000.0;
const DEFAULT_DIR: &str =
    "/private/tmp/claude-501/-Users-admin-makepad/bbb86735-3473-4fd6-ad57-d987802a0619/scratchpad/synth/instruments";

/// Render `secs` of `score` on `inst`, splitting each block at event offsets
/// so notes land sample-accurately. `pedal_bars` re-takes the sustain pedal at
/// every bar line (a pianist's legato pedalling).
fn render(inst: &mut dyn Instrument, score: &str, secs: f32, pedal_bars: bool) -> (Vec<f32>, Vec<f32>, f64) {
    let mut seq = Sequencer::new(Score::parse(score).unwrap_or_else(|e| panic!("{score}: {e}")));
    let frames = (secs * RATE) as usize;
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let mut events: Vec<(usize, SeqEvent)> = Vec::with_capacity(64);
    let mut last_bar = -1i64;
    let start = Instant::now();
    let mut at = 0;
    while at < frames {
        let n = (frames - at).min(256);
        if pedal_bars {
            let bar = (seq.beat() / 4.0).floor() as i64;
            if bar != last_bar {
                inst.control(Control::Sustain, 0.0);
                inst.control(Control::Sustain, 1.0);
                last_bar = bar;
            }
        }
        events.clear();
        seq.advance(n, RATE, &mut |f, e| events.push((f, e)));
        let mut cursor = 0;
        for &(f, e) in events.iter() {
            if f > cursor {
                inst.render(&mut l[at + cursor..at + f], &mut r[at + cursor..at + f]);
                cursor = f;
            }
            match e {
                SeqEvent::On { note, vel } => inst.note_on(note, vel),
                SeqEvent::Off { note } => inst.note_off(note),
            }
        }
        inst.render(&mut l[at + cursor..at + n], &mut r[at + cursor..at + n]);
        at += n;
    }
    let speed = secs as f64 / start.elapsed().as_secs_f64();
    (l, r, speed)
}

fn check_and_write(name: &str, gain: f32, l: &mut [f32], r: &mut [f32], speed: f64) {
    for x in l.iter_mut().chain(r.iter_mut()) {
        *x *= gain;
    }
    assert!(l.iter().chain(r.iter()).all(|x| x.is_finite()), "{name}: NaN");
    let peak = l.iter().chain(r.iter()).fold(0.0f32, |a, x| a.max(x.abs()));
    let rms = (l.iter().chain(r.iter()).map(|x| x * x).sum::<f32>() / (l.len() * 2) as f32).sqrt();
    println!(
        "{name:>16}: peak {:6.1} dBFS  rms {:6.1} dBFS  render {speed:7.1}x realtime",
        20.0 * peak.max(1e-9).log10(),
        20.0 * rms.max(1e-9).log10()
    );
    assert!(peak < 1.0, "{name}: clips at gain {gain} (peak {peak})");
    assert!(rms > 1.0e-3, "{name}: silent (rms {rms})");
    let dir = std::env::var("SYNTH_RENDER_DIR").unwrap_or_else(|_| DEFAULT_DIR.to_string());
    if std::fs::create_dir_all(&dir).is_ok() {
        let path = format!("{dir}/{name}.wav");
        std::fs::write(&path, stereo_wav_16(l, r, RATE as u32)).unwrap();
        println!("{:>16}  -> {path}", "");
    }
}

fn run(name: &str, kind: InstrumentKind, score: &str, pedal: bool, gain: f32) {
    let t = Instant::now();
    let mut inst = build(&kind, RATE).unwrap_or_else(|e| panic!("{name}: {e}"));
    println!("{name:>16}: built in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let (mut l, mut r, speed) = render(inst.as_mut(), score, 6.0, pedal);
    check_and_write(name, gain, &mut l, &mut r, speed);
}

#[test]
fn render_every_instrument() {
    let kind = |n: &str, p: Option<&str>| parse_kind(n, p).unwrap_or_else(|| panic!("{n}"));
    // I - vi - IV - V with a melody on top, pedal re-taken each bar.
    run(
        "piano",
        kind("piano", None),
        "bpm=80 [C3 G3 E4]:4 | [A2 E3 C4]:4 | [F2 C3 A3]:4 | [G2 D3 B3]:4 ; \
         r:1 G4:1/2 E5 D5:1 C5 | r:1 E5:1/2 C5 A4:2 | r:1 F5:1/2 A5 G5:1 F5 | D5:2 B4:2",
        true,
        1.0,
    );
    run(
        "drums",
        kind("drums", None),
        "bpm=110 loop kick:1 snare kick:1/2 x snare:1 | kick:1 snare kick:1/2 kick snare:1/2 clap:1/2 ; \
         hat:1/2 x x ohat x x x x | hat x x x x x crash:1",
        false,
        1.0,
    );
    run(
        "ironfish",
        kind("ironfish", Some("supersaw")),
        "bpm=128 loop A3:1/2 A3 C4 A3 E4 A3 D4 C4 | F3 F3 A3 F3 C4 F3 E4 D4",
        false,
        1.0,
    );
    let riff = "bpm=100 C4:1/2 E4 G4 C5 B4:1 G4 | A4:1/2 C5 E5 A5 G5:2";
    let chords = "bpm=90 [C4 E4 G4 B4]:2 | [A3 C4 E4 G4]:2 | [F3 A3 C4 E4]:2";
    run("synth_lead", kind("lead", None), riff, false, 1.0);
    run("synth_bass", kind("bass", None), "bpm=100 C2:1/2 C2 G2 C2 Bb1 C2 G1:1 | F1:1/2 F1 C2 F1 G1:2", false, 1.0);
    run("synth_pad", kind("pad", None), chords, false, 1.0);
    run("synth_pluck", kind("pluck", None), riff, false, 1.0);
    run("synth_epiano", kind("epiano", None), chords, false, 1.0);
    // The soundfont crate's procedural piano (no SF2 file needed).
    let t = Instant::now();
    let mut sf = SoundFontInstrument::fallback_piano(RATE);
    println!("{:>16}: built in {:.0} ms", "soundfont_fallback", t.elapsed().as_secs_f64() * 1000.0);
    let (mut l, mut r, speed) = render(&mut sf, riff, 6.0, false);
    check_and_write("soundfont_fallback", 1.0, &mut l, &mut r, speed);
}
