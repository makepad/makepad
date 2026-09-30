//! Offline renders of every instrument playing a short phrase, written as
//! WAVs for listening (SYNTH_RENDER_DIR, with a scratch default) and checked
//! for level, NaN and render speed.
#![cfg(feature = "instruments")]

use makepad_audio_synth::instruments::{build, parse_kind, InstrumentKind, SoundFontInstrument};
use makepad_audio_synth::wav::stereo_wav_16;
use makepad_audio_synth::{Control, Instrument};
use std::time::Instant;

const RATE: f32 = 48_000.0;
const DEFAULT_DIR: &str =
    "/private/tmp/claude-501/-Users-admin-makepad/bbb86735-3473-4fd6-ad57-d987802a0619/scratchpad/synth/instruments";

/// A phrase: (beat, MIDI note, beats) at a tempo, looped or once.
struct Phrase {
    bpm: f32,
    beats: f32,
    looping: bool,
    notes: &'static [(f32, u8, f32)],
}

const PIANO: Phrase = Phrase { bpm: 80.0, beats: 16.0, looping: false, notes: &[(0.0, 48, 4.0), (0.0, 55, 4.0), (0.0, 64, 4.0), (1.0, 67, 0.5), (1.5, 76, 0.5), (2.0, 74, 1.0), (3.0, 72, 1.0), (4.0, 45, 4.0), (4.0, 52, 4.0), (4.0, 60, 4.0), (5.0, 76, 0.5), (5.5, 72, 0.5), (6.0, 69, 2.0), (8.0, 41, 4.0), (8.0, 48, 4.0), (8.0, 57, 4.0), (9.0, 77, 0.5), (9.5, 81, 0.5), (10.0, 79, 1.0), (11.0, 77, 1.0), (12.0, 43, 4.0), (12.0, 50, 4.0), (12.0, 59, 4.0), (12.0, 74, 2.0), (14.0, 71, 2.0)] };
const DRUMS: Phrase = Phrase { bpm: 110.0, beats: 8.0, looping: true, notes: &[(0.0, 36, 1.0), (0.0, 42, 0.5), (0.5, 42, 0.5), (1.0, 38, 1.0), (1.0, 42, 0.5), (1.5, 46, 0.5), (2.0, 36, 0.5), (2.0, 46, 0.5), (2.5, 36, 0.5), (2.5, 46, 0.5), (3.0, 38, 1.0), (3.0, 46, 0.5), (3.5, 46, 0.5), (4.0, 36, 1.0), (4.0, 42, 0.5), (4.5, 42, 0.5), (5.0, 38, 1.0), (5.0, 42, 0.5), (5.5, 42, 0.5), (6.0, 36, 0.5), (6.0, 42, 0.5), (6.5, 36, 0.5), (6.5, 42, 0.5), (7.0, 38, 0.5), (7.0, 49, 1.0), (7.5, 39, 0.5)] };
const IRONFISH: Phrase = Phrase { bpm: 128.0, beats: 8.0, looping: true, notes: &[(0.0, 57, 0.5), (0.5, 57, 0.5), (1.0, 60, 0.5), (1.5, 57, 0.5), (2.0, 64, 0.5), (2.5, 57, 0.5), (3.0, 62, 0.5), (3.5, 60, 0.5), (4.0, 53, 0.5), (4.5, 53, 0.5), (5.0, 57, 0.5), (5.5, 53, 0.5), (6.0, 60, 0.5), (6.5, 53, 0.5), (7.0, 64, 0.5), (7.5, 62, 0.5)] };
const RIFF: Phrase = Phrase { bpm: 100.0, beats: 8.0, looping: false, notes: &[(0.0, 60, 0.5), (0.5, 64, 0.5), (1.0, 67, 0.5), (1.5, 72, 0.5), (2.0, 71, 1.0), (3.0, 67, 1.0), (4.0, 69, 0.5), (4.5, 72, 0.5), (5.0, 76, 0.5), (5.5, 81, 0.5), (6.0, 79, 2.0)] };
const CHORDS: Phrase = Phrase { bpm: 90.0, beats: 8.0, looping: false, notes: &[(0.0, 60, 2.0), (0.0, 64, 2.0), (0.0, 67, 2.0), (0.0, 71, 2.0), (2.0, 57, 2.0), (2.0, 60, 2.0), (2.0, 64, 2.0), (2.0, 67, 2.0), (4.0, 53, 2.0), (4.0, 57, 2.0), (4.0, 60, 2.0), (4.0, 64, 2.0)] };
const BASS: Phrase = Phrase { bpm: 100.0, beats: 8.0, looping: false, notes: &[(0.0, 36, 0.5), (0.5, 36, 0.5), (1.0, 43, 0.5), (1.5, 36, 0.5), (2.0, 34, 0.5), (2.5, 36, 0.5), (3.0, 31, 1.0), (4.0, 29, 0.5), (4.5, 29, 0.5), (5.0, 36, 0.5), (5.5, 29, 0.5), (6.0, 31, 2.0)] };

/// Render `secs` of `phrase` on `inst`, splitting each block at note on and
/// off frames so notes land sample-accurately. `pedal_bars` re-takes the
/// sustain pedal at every bar line (a pianist's legato pedalling).
fn render(inst: &mut dyn Instrument, phrase: &Phrase, secs: f32, pedal_bars: bool) -> (Vec<f32>, Vec<f32>, f64) {
    let frames = (secs * RATE) as usize;
    let per_beat = RATE * 60.0 / phrase.bpm;
    // Every on (vel > 0) and off (vel 0) as (frame, note, vel), in order.
    let mut events: Vec<(usize, u8, u8)> = Vec::new();
    let mut offset = 0.0;
    while (offset * per_beat) < frames as f32 {
        for &(beat, note, dur) in phrase.notes {
            events.push((((offset + beat) * per_beat) as usize, note, 100));
            events.push((((offset + beat + dur) * per_beat) as usize, note, 0));
        }
        if !phrase.looping {
            break;
        }
        offset += phrase.beats;
    }
    events.sort_by_key(|e| (e.0, e.2));
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let bar = (per_beat * 4.0) as usize;
    let start = Instant::now();
    let mut at = 0;
    let mut next = 0;
    while at < frames {
        let n = (frames - at).min(256);
        if pedal_bars && at % bar < n {
            inst.control(Control::Sustain, 0.0);
            inst.control(Control::Sustain, 1.0);
        }
        let mut cursor = at;
        while next < events.len() && events[next].0 < at + n {
            let (f, note, vel) = events[next];
            if f > cursor {
                inst.render(&mut l[cursor..f], &mut r[cursor..f]);
                cursor = f;
            }
            if vel > 0 { inst.note_on(note, vel) } else { inst.note_off(note) }
            next += 1;
        }
        inst.render(&mut l[cursor..at + n], &mut r[cursor..at + n]);
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

fn run(name: &str, kind: InstrumentKind, phrase: &Phrase, pedal: bool, gain: f32) {
    let t = Instant::now();
    let mut inst = build(&kind, RATE).unwrap_or_else(|e| panic!("{name}: {e}"));
    println!("{name:>16}: built in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let (mut l, mut r, speed) = render(inst.as_mut(), phrase, 6.0, pedal);
    check_and_write(name, gain, &mut l, &mut r, speed);
}

#[test]
fn render_every_instrument() {
    let kind = |n: &str, p: Option<&str>| parse_kind(n, p).unwrap_or_else(|| panic!("{n}"));
    // I - vi - IV - V with a melody on top, pedal re-taken each bar.
    run(
        "piano",
        kind("piano", None),
        &PIANO,
        true,
        1.0,
    );
    run(
        "drums",
        kind("drums", None),
        &DRUMS,
        false,
        1.0,
    );
    run(
        "ironfish",
        kind("ironfish", Some("supersaw")),
        &IRONFISH,
        false,
        1.0,
    );
    let (riff, chords) = (&RIFF, &CHORDS);
    run("synth_lead", kind("lead", None), riff, false, 1.0);
    run("synth_bass", kind("bass", None), &BASS, false, 1.0);
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
