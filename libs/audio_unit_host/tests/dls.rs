//! The host against Apple's General MIDI synth, which ships with macOS, so
//! the test depends on no third-party plug-in.

use makepad_audio_unit_host::{instruments, live, ComponentDescription, Instrument, MidiMessage};

#[cfg(target_os = "macos")]
fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |a, x| a.max(x.abs()))
}

#[cfg(target_os = "macos")]
fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|x| x * x).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

#[cfg(target_os = "macos")]
#[test]
fn the_built_in_synth_is_listed_plays_a_note_and_stops_it() {
    const RATE: f64 = 48_000.0;
    let listed = instruments();
    let dls = listed
        .iter()
        .find(|info| info.description == ComponentDescription::DLS_SYNTH)
        .unwrap_or_else(|| panic!("the built-in synth is not listed among {listed:?}"));
    assert!(!dls.name.is_empty(), "the system names the built-in synth");

    let mut synth = Instrument::new(&ComponentDescription::DLS_SYNTH, RATE, 512).expect("load the built-in synth");
    assert_eq!(synth.info().description, ComponentDescription::DLS_SYNTH);
    let _ = synth.set_offline(true);

    // Its parameters describe themselves, a value set reads back, and the
    // plug-in writes its own text for it.
    let params = synth.parameters();
    assert!(!params.is_empty(), "the built-in synth reports its parameters");
    for p in &params {
        assert!(!p.name.is_empty() && p.min <= p.default && p.default <= p.max, "{p:?}");
    }
    let p = &params[0];
    let mid = 0.5 * (p.min + p.max);
    synth.set_parameter(p.id, mid).unwrap();
    let read = synth.parameter(p.id).unwrap();
    assert!((read - mid).abs() <= 1e-3 * (p.max - p.min).abs().max(1.0), "{} set to {mid} reads {read}", p.name);
    synth.set_parameter(p.id, p.default).unwrap();
    let _ = synth.parameter_text(p.id, p.default);
    let _ = synth.factory_presets();

    // Before any note: silence. Spans longer than one slice are split.
    let (mut left, mut right) = (vec![1.0f32; 4_800], vec![1.0f32; 4_800]);
    synth.render(&mut left, &mut right).unwrap();
    assert!(peak(&left).max(peak(&right)) < 1e-4, "silent before a note");

    // A note sounds, in both channels, finite.
    synth.note_on(0, 60, 110).unwrap();
    let (mut left, mut right) = (vec![0.0f32; 12_000], vec![0.0f32; 12_000]);
    synth.render(&mut left, &mut right).unwrap();
    assert!(left.iter().chain(&right).all(|x| x.is_finite()), "finite output");
    let held = rms(&left[6_000..]).max(rms(&right[6_000..]));
    assert!(peak(&left) > 0.01 && peak(&right) > 0.01, "the note reaches both channels");

    // Its note-off ends it.
    synth.note_off(0, 60).unwrap();
    let (mut left, mut right) = (vec![0.0f32; 48_000], vec![0.0f32; 48_000]);
    synth.render(&mut left, &mut right).unwrap();
    let after = rms(&left[43_200..]).max(rms(&right[43_200..]));
    assert!(after < held * 0.1, "released: {after} vs held {held}");

    // Live: messages through the ring reach the synth on the next block, and
    // a full ring hands the message back instead of waiting.
    let (mut sender, mut renderer) = live(synth, 2);
    assert!(sender.send(MidiMessage::note_on(0, 67, 110)).is_ok());
    assert!(sender.send(MidiMessage::note_on(0, 64, 110)).is_ok());
    assert_eq!(sender.send(MidiMessage::note_on(0, 72, 110)), Err(MidiMessage::note_on(0, 72, 110)), "full ring");
    let (mut left, mut right) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    let mut loud = 0.0f32;
    for _ in 0..12 {
        renderer.render(&mut left, &mut right).unwrap();
        loud = loud.max(peak(&left));
    }
    assert!(loud > 0.01, "the queued notes sound: {loud}");
    assert_eq!(sender.free(), 2, "the callback drained the ring");
    drop(renderer.into_inner());
}

#[cfg(not(target_os = "macos"))]
#[test]
fn nothing_is_installed_off_macos() {
    assert!(instruments().is_empty());
    let error = Instrument::new(&ComponentDescription::DLS_SYNTH, 48_000.0, 512).err().expect("no audio units here");
    assert_eq!(error.status, makepad_audio_unit_host::Error::UNSUPPORTED);
    let _ = (live, MidiMessage::all_notes_off(0));
}
