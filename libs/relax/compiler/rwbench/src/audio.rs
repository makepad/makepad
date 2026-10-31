// Audio workloads: the physical + learned piano render (makepad-piano-model, the voice path the
// score app plays through), the in-repo Ogg Vorbis file through makepad-audio-decode (the repo
// carries no MP3: the decoder's MP3 tests build their streams bit by bit inside the test), and the
// game mixer (makepad-audio-synth `SynthBus`: recipes, tones, vehicles, spatializer, room send,
// master dynamics).
use crate::{fold, inputs};
use makepad_piano_model::learned::LearnedPiano;
use makepad_piano_model::{Piano, PianoEvent, TimedEvent};

fn fold_pcm(mut h: u64, l: &[f32], r: &[f32]) -> u64 {
    // every sample's bits, both channels: a bit-exact checksum of the render
    for (a, b) in l.iter().zip(r.iter()) {
        h = fold(h, (a.to_bits() as u64) << 32 | b.to_bits() as u64);
    }
    h
}

/// A fixed phrase: pedalled chords (a rolled C-minor-9, an open fifth, a high triad) over a bass
/// line, then release. Sample offsets are absolute; `render_events` slices them into blocks.
fn phrase(rate: f32) -> Vec<(u64, PianoEvent)> {
    use PianoEvent::*;
    let at = |s: f64| (s * rate as f64) as u64;
    let mut ev = vec![(at(0.0), Sustain { value: 1.0 })];
    let chords: [(f64, &[u8], u8); 4] = [
        (0.00, &[36, 48, 55, 58, 62, 65], 96),
        (0.12, &[43, 55, 62, 67, 74], 78),
        (0.24, &[29, 41, 60, 65, 69, 72, 77], 110),
        (0.36, &[84, 88, 91, 96], 70),
    ];
    for (t, keys, vel) in chords {
        for (i, k) in keys.iter().enumerate() {
            ev.push((at(t + i as f64 * 0.004), NoteOn { key: *k, velocity: vel - (i as u8 % 3) * 6 }));
        }
    }
    for k in [36u8, 48, 55] {
        ev.push((at(0.42), NoteOff { key: k }));
    }
    ev.push((at(0.45), Sustain { value: 0.0 }));
    ev.sort_by_key(|e| e.0);
    ev
}

fn render_blocks(events: &[(u64, PianoEvent)], total: usize, block: usize, mut process: impl FnMut(&[TimedEvent], &mut [f32], &mut [f32]), l: &mut [f32], r: &mut [f32]) {
    let mut te: Vec<TimedEvent> = Vec::new();
    let mut pos = 0usize;
    while pos < total {
        let n = block.min(total - pos);
        te.clear();
        for e in events {
            if e.0 >= pos as u64 && e.0 < (pos + n) as u64 {
                te.push(TimedEvent { offset: (e.0 - pos as u64) as u32, event: e.1 });
            }
        }
        process(&te, &mut l[pos..pos + n], &mut r[pos..pos + n]);
        pos += n;
    }
}

/// libs/piano_model: the modal-resonator Piano (strings, soundboard, sympathetic bank, room) and the
/// LearnedPiano (additive branch from the embedded network) each render the phrase through their
/// `process`; both are built fresh in every iteration (their reset() does not return to the constructed state, so the
/// checksum would drift), so an iteration is a session start plus the render.
pub fn piano_setup() -> Box<dyn FnMut() -> u64> {
    const RATE: f32 = 48_000.0;
    let secs = std::env::var("RW_PIANO_SECS").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.3);
    let total = (secs * RATE as f64) as usize;
    let events = phrase(RATE);
    let mut l = vec![0.0f32; total];
    let mut r = vec![0.0f32; total];
    Box::new(move || {
        let mut modal = Piano::new(RATE);
        l.fill(0.0);
        r.fill(0.0);
        render_blocks(&events, total, 512, |e, a, b| modal.process(e, a, b), &mut l, &mut r);
        let mut h = fold_pcm(0, &l, &r);
        let mut learned = LearnedPiano::new(RATE);
        l.fill(0.0);
        r.fill(0.0);
        render_blocks(&events, total, 512, |e, a, b| learned.process(e, a, b), &mut l, &mut r);
        h = fold_pcm(h, &l, &r);
        h
    })
}

/// libs/audio_decode on the one in-repo compressed audio file (Ogg Vorbis, 6 KB); the format sniff and
/// the whole-file decode, eight times per iteration to reach a measurable iteration.
pub fn ogg_setup() -> Box<dyn FnMut() -> u64> {
    let bytes = inputs::read("libs/audio_decode/testdata/button-press.ogg");
    Box::new(move || {
        let mut h = 0u64;
        for _ in 0..8 {
            let a = makepad_audio_decode::decode_any(&bytes).expect("ogg decodes");
            h = fold(h, (a.rate as u64) << 16 | a.channels as u64);
            for s in &a.pcm_interleaved_f32 {
                h = fold(h, s.to_bits() as u64);
            }
        }
        h
    })
}

/// libs/audio_synth `SynthBus`, the game mixer: recipe voices placed around the listener, two
/// sustained tones, two engines with tyres moving past, the room send and master dynamics.
pub fn mix_setup() -> Box<dyn FnMut() -> u64> {
    use makepad_audio_synth::bus::{Emitter, Priority, SynthBus};
    use makepad_audio_synth::engine::{preset, EngineInput};
    use makepad_audio_synth::recipe::{builtin, Recipe};
    use makepad_audio_synth::spatial::Distance;
    use makepad_audio_synth::vehicle::{Surface, TyreInput};
    const RATE: f32 = 48_000.0;
    let blocks: usize = std::env::var("RW_MIX_BLOCKS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    let names = ["gun_rifle", "impact_metal", "footstep_gravel", "explosion", "magic", "bell"];
    let recipes: Vec<Recipe> = names.iter().map(|n| Recipe::parse(builtin(n).unwrap()).unwrap()).collect();
    let cars = [preset("v8").unwrap(), preset("flat6").unwrap()];
    let mut l = vec![0.0f32; 512];
    let mut r = vec![0.0f32; 512];
    Box::new(move || {
        let mut bus = SynthBus::new(RATE);
        let mut h = 0u64;
        for b in 0..blocks {
            let t = b as f32 * 512.0 / RATE;
            if b % 6 == 0 {
                let i = (b / 6) % recipes.len();
                let e = if i % 2 == 0 {
                    Emitter::at([8.0 * (i as f32 - 2.5), 0.0, -6.0 - i as f32], 60.0)
                } else {
                    Emitter::panned(i as f32 * 0.3 - 0.7)
                };
                bus.play_recipe(&recipes[i], 1.0 + 0.05 * i as f32, 0.8, e, Priority::Normal, 0);
            }
            bus.tone(1, 220.0 + 40.0 * (t * 3.0).sin(), None, 0.1);
            for (i, spec) in cars.iter().enumerate() {
                let phase = t * 2.0 + i as f32 * 2.0;
                let pos = [phase.cos() * 30.0, 0.0, phase.sin() * 30.0];
                let speed = 22.0;
                let vel = [-phase.sin() * speed, 0.0, phase.cos() * speed];
                let rpm = spec.idle_rpm + (spec.redline_rpm - spec.idle_rpm) * (0.4 + 0.3 * (t * 4.0 + i as f32).sin().abs());
                let e = Emitter { vel, dist: Distance { near: 3.0, range: 160.0, rolloff: 0.7 }, ..Emitter::at(pos, 160.0) };
                bus.vehicle(
                    i as u64 + 1,
                    spec,
                    EngineInput { rpm, throttle: 1.0, load: 1.0, gear: 3, speed },
                    TyreInput { slip: 0.04, load: 1.0, speed, surface: Surface::Asphalt, grounded: 4 },
                    e,
                    0.0,
                    1.0,
                );
            }
            bus.end_frame();
            l.fill(0.0);
            r.fill(0.0);
            bus.render(&mut l, &mut r);
            h = fold_pcm(h, &l, &r);
        }
        if std::env::var("RW_DEBUG").is_ok() {
            let st = bus.stats();
            eprintln!("audio_mix: shed {} steals {} cpu_load {:.3} peak {:.3}", st.shed, st.steals, st.cpu_load, st.peak_cpu_load);
        }
        h
    })
}
