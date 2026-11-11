//! Nothing on the audio thread allocates: every Morph preset, every effect in Morph's rack and
//! every filter type, and Tandem's oscillators, filters, shapers and effects, each played from a
//! fresh engine; and a Morph patch change and a Tandem program change made mid-play, each made
//! ready off the audio thread; the synth bus applies commands, renders and reaps with no
//! allocation and no free. One test in a binary of its own: the tables a process builds once
//! are built by nothing else here first, so a part that builds one on its render path is caught
//! however the tests run.

use makepad_audio_synth::bus::{Command, Emitter, Group, Priority, Retired, SynthBus};
use makepad_audio_synth::dsp::Wave;
use makepad_audio_synth::engine::{preset, EngineInput};
use makepad_audio_synth::instrument::Control;
use makepad_audio_synth::kit::filter::TYPES;
use makepad_audio_synth::kit::fx::KINDS;
use makepad_audio_synth::morph::{Morph, MorphInstall, MorphPatch, PRESETS};
use makepad_audio_synth::recipe::{builtin, Recipe};
use makepad_audio_synth::reverb::RoomPreset;
use makepad_audio_synth::spatial::Listener3;
use makepad_audio_synth::tandem::program::Program;
use makepad_audio_synth::tandem::{Patch, Tandem, TandemInstall};
use makepad_audio_synth::vehicle::{loop_set, EngineLoop, TyreInput};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

const RATE: f32 = 48_000.0;

struct Counting;

thread_local! {
    static COUNT: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let _ = FREES.try_with(|c| c.set(c.get() + 1));
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        let _ = FREES.try_with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// What this thread allocates while `f` runs.
fn allocations(f: impl FnOnce()) -> usize {
    heap_calls(f).0
}

/// Allocations and frees this thread makes while `f` runs.
fn heap_calls(f: impl FnOnce()) -> (usize, usize) {
    let before = COUNT.with(|c| c.get());
    let freed = FREES.with(|c| c.get());
    f();
    (COUNT.with(|c| c.get()) - before, FREES.with(|c| c.get()) - freed)
}

/// Blocks of 256 frames each engine plays: a note every 50 blocks, released 40 blocks in.
const BLOCKS: u32 = 200;

fn note(i: u32) -> u8 {
    40 + (i / 50 % 40) as u8
}

/// The block a change comes in, a note sounding: the note of block 100 is held until 140.
const CHANGE: u32 = 110;

fn morph_install(text: &str) -> MorphInstall {
    let patch = MorphPatch::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
    let src = patch.sources([None, None, None]);
    MorphInstall::new(RATE, Arc::new(patch), src)
}

fn morph(text: &str) -> Morph {
    let patch = MorphPatch::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
    let src = patch.sources([None, None, None]);
    Morph::new(RATE, Arc::new(patch), src)
}

/// Plays `m`, and `next` from [`CHANGE`] on.
fn play_morph(m: &mut Morph, mut next: Option<&mut MorphInstall>) {
    let (mut l, mut r) = ([0.0; 256], [0.0; 256]);
    for i in 0..BLOCKS {
        if i % 50 == 0 {
            m.note_on(note(i), 100);
        }
        if i % 50 == 40 {
            m.note_off(note(i));
        }
        if i == CHANGE {
            if let Some(next) = next.as_deref_mut() {
                m.install(next);
            }
        }
        m.render(&mut l, &mut r);
    }
}

fn program(set: &[(&str, f32)]) -> Arc<Program> {
    let mut p = Patch::init();
    for (key, v) in set {
        assert!(p.set(key, *v), "{key}");
    }
    Arc::new(Program::compile(&p))
}

fn tandem(set: &[(&str, f32)]) -> Tandem {
    let mut t = Tandem::new(RATE);
    t.set_program(&mut TandemInstall::new(RATE, program(set)));
    t
}

/// Plays `t`, and queues `next` at [`CHANGE`]: it fades in over the held note, which retriggers.
/// What the queue hands back goes to `spent`.
fn play_tandem(t: &mut Tandem, mut next: Option<Box<TandemInstall>>, spent: &mut Vec<Box<TandemInstall>>) {
    let (mut l, mut r) = ([0.0; 256], [0.0; 256]);
    for i in 0..BLOCKS {
        if i % 50 == 0 {
            t.note_on(0, note(i), 100);
        }
        if i % 50 == 40 {
            t.note_off(0, note(i), 64);
        }
        if i == CHANGE {
            if let Some(next) = next.take() {
                for back in t.queue_program(next).into_iter().flatten() {
                    spent.push(back);
                }
            }
        }
        t.render(&mut l, &mut r);
    }
}

#[test]
fn the_audio_thread_never_allocates() {
    let mut allocated = Vec::new();
    let mut played = 0;
    let mut check = |what: String, n: usize| {
        played += 1;
        if n > 0 {
            allocated.push(format!("{what}: {n} heap calls"));
        }
    };
    for (name, _, text) in PRESETS {
        let mut m = morph(text);
        check(format!("Morph preset {name}"), allocations(|| play_morph(&mut m, None)));
    }
    for (kind, _) in KINDS {
        let mut m = morph(&format!("fx={kind}"));
        check(format!("Morph effect {kind}"), allocations(|| play_morph(&mut m, None)));
    }
    for ty in TYPES {
        let mut m = morph(&format!("f1_type={ty} f1_res=0.5 f2_on=true f2_type={ty}"));
        check(format!("Morph filter {ty}"), allocations(|| play_morph(&mut m, None)));
    }
    let mut spent = Vec::new();
    for ty in 0..12 {
        let mut t = tandem(&[("a_osc1_type", ty as f32)]);
        check(format!("Tandem oscillator {ty}"), allocations(|| play_tandem(&mut t, None, &mut spent)));
    }
    for ty in 0..36 {
        let mut t = tandem(&[("a_filter1_type", ty as f32), ("a_filter2_type", ((ty + 7) % 36) as f32), ("a_filter1_resonance", 0.5)]);
        check(format!("Tandem filter {ty}"), allocations(|| play_tandem(&mut t, None, &mut spent)));
    }
    for ty in 0..43 {
        let mut t = tandem(&[("a_ws_type", ty as f32)]);
        check(format!("Tandem shaper {ty}"), allocations(|| play_tandem(&mut t, None, &mut spent)));
    }
    for ty in 0..32 {
        let mut t = tandem(&[("fx1_type", ty as f32)]);
        check(format!("Tandem effect {ty}"), allocations(|| play_tandem(&mut t, None, &mut spent)));
    }
    // a patch change mid-play: from each preset to the next, to each effect from the one before it
    // and to the same effect with other filters, and to each filter type in both filters
    for (k, (name, _, text)) in PRESETS.iter().enumerate() {
        let mut m = morph(PRESETS[(k + PRESETS.len() - 1) % PRESETS.len()].2);
        let mut next = morph_install(text);
        check(format!("Morph change to preset {name}"), allocations(|| play_morph(&mut m, Some(&mut next))));
    }
    for (k, (kind, _)) in KINDS.iter().enumerate() {
        let mut m = morph(&format!("fx={}", KINDS[(k + KINDS.len() - 1) % KINDS.len()].0));
        let mut next = morph_install(&format!("fx={kind}"));
        check(format!("Morph change to effect {kind}"), allocations(|| play_morph(&mut m, Some(&mut next))));
        let mut next = morph_install(&format!("fx={kind} f1_type=comb f2_on=true f2_type=comb_neg"));
        check(format!("Morph change in effect {kind}"), allocations(|| play_morph(&mut m, Some(&mut next))));
    }
    for ty in TYPES {
        let mut m = morph("");
        let mut next = morph_install(&format!("f1_type={ty} f1_res=0.5 f2_on=true f2_type={ty}"));
        check(format!("Morph change to filter {ty}"), allocations(|| play_morph(&mut m, Some(&mut next))));
    }
    // a program change mid-play: to each effect from the one before it, to each oscillator and to
    // each filter type
    for ty in 0..32 {
        let mut t = tandem(&[("fx1_type", ((ty + 31) % 32) as f32), ("fx2_type", ty as f32)]);
        let next = Box::new(TandemInstall::new(RATE, program(&[("fx1_type", ty as f32), ("fx2_type", ty as f32)])));
        check(format!("Tandem change to effect {ty}"), allocations(|| play_tandem(&mut t, Some(next), &mut spent)));
    }
    for ty in 0..12 {
        let mut t = tandem(&[("a_osc1_type", ((ty + 11) % 12) as f32)]);
        let next = Box::new(TandemInstall::new(RATE, program(&[("a_osc1_type", ty as f32), ("a_osc2_type", ty as f32)])));
        check(format!("Tandem change to oscillator {ty}"), allocations(|| play_tandem(&mut t, Some(next), &mut spent)));
    }
    for ty in 0..36 {
        let mut t = tandem(&[]);
        let next = Box::new(TandemInstall::new(RATE, program(&[("a_filter1_type", ty as f32), ("a_filter2_type", ty as f32), ("b_filter1_type", ty as f32)])));
        check(format!("Tandem change to filter {ty}"), allocations(|| play_tandem(&mut t, Some(next), &mut spent)));
    }
    let mut bus = SynthBus::new(RATE);
    let mut retired = Vec::with_capacity(64);
    let loops_a = loop_set(vec![EngineLoop::new(Arc::from(vec![0.0f32; 4800]), RATE, 3000.0)]);
    let loops_b = loop_set(vec![EngineLoop::new(Arc::from(vec![0.0f32; 4800]), RATE, 3000.0)]);
    let inst_a = Box::new(morph(""));
    let inst_b = Box::new(morph(""));
    let recipe = Recipe::parse(builtin("coin").unwrap()).unwrap();
    let batch_a = [
        Command::Listener(Listener3 { pos: [1.0, 0.0, 2.0], ..Listener3::default() }),
        Command::Room(RoomPreset::Room.params()),
        Command::Master(0.8),
        Command::GroupGain { group: Group::Music, gain: 0.5 },
        Command::Duck(0.2),
        Command::PlayRecipe { id: 1, recipe, pitch: 1.0, gain: 0.6, emitter: Emitter::FLAT, priority: Priority::Normal, key: 0 },
        Command::MoveVoice { id: 1, pos: [2.0, 0.0, -3.0], vel: [0.0; 3] },
        Command::Tone { id: 2, freq: 220.0, wave: Some(Wave::Sine), gain: 0.2 },
        Command::ToneSet { id: 2, freq: Some(330.0), gain: Some(0.3) },
        Command::Vehicle {
            key: 7, spec: preset("v8").unwrap(),
            engine: EngineInput { rpm: 3000.0, throttle: 0.7, load: 0.5, gear: 2, speed: 12.0 },
            tyres: TyreInput { speed: 12.0, load: 1.0, grounded: 4, ..TyreInput::default() },
            emitter: Emitter::FLAT, interior: 0.2, gain: 0.6,
        },
        Command::VehicleStart { key: 7 },
        Command::VehicleLoops { key: 7, loops: loops_a, mix: 0.7 },
        Command::VehicleThump { key: 7, strength: 0.3 },
        Command::AddInstrument { key: 3, inst: inst_a, emitter: Emitter::FLAT, gain: 0.8 },
        Command::InstrumentGroup { key: 3, group: Group::Music },
        Command::InstrumentEmitter { key: 3, emitter: Emitter::panned(0.2), gain: Some(0.7) },
        Command::NoteOn { key: 3, note: 60, velocity: 100 },
        Command::Control { key: 3, control: Control::Modulation, value: 0.3 },
        Command::EndFrame,
    ];
    let batch_b = [
        Command::VehicleLoops { key: 7, loops: loops_b, mix: 0.7 },
        Command::AddInstrument { key: 3, inst: inst_b, emitter: Emitter::FLAT, gain: 0.8 },
        Command::NoteOff { key: 3, note: 60 },
        Command::StopVoice { id: 1 },
        Command::ToneStop { id: 2 },
        Command::FadeTones { prefix: 2, shift: 0, secs: 0.05 },
        Command::VehicleStopEngine { key: 7 },
        Command::VehicleRelease { key: 7 },
        Command::FadeInstrument { key: 3, secs: 0.01 },
        Command::RemoveInstrument { key: 99 },
        Command::StopAllTones,
        Command::Silence,
        Command::Clear,
    ];
    let (mut l, mut r) = ([0.0; 256], [0.0; 256]);
    let (allocs, frees) = heap_calls(|| {
        for cmd in batch_a {
            bus.apply(cmd, &mut |r| retired.push(r));
        }
        for _ in 0..50 {
            l.fill(0.0);
            r.fill(0.0);
            bus.render(&mut l, &mut r);
        }
        for cmd in batch_b {
            bus.apply(cmd, &mut |r| retired.push(r));
        }
        for _ in 0..50 {
            l.fill(0.0);
            r.fill(0.0);
            bus.render(&mut l, &mut r);
        }
        bus.reap(&mut |r| retired.push(r));
    });
    check("SynthBus allocations".to_string(), allocs);
    check("SynthBus frees".to_string(), frees);
    assert_eq!(retired.len(), 4, "two replaced loop sets and two instruments");
    assert_eq!(retired.iter().filter(|r| match r {
        Retired::Loops(loops) => loops.iter().any(Option::is_some),
        Retired::Instrument { key: 3, .. } => true,
        _ => false,
    }).count(), 3, "the first loop set, replaced instrument and faded instrument own payloads");
    drop(retired);
    println!("{played} checks, {} made heap calls on the audio thread", allocated.len());
    assert!(allocated.is_empty(), "the audio thread made heap calls:\n{}", allocated.join("\n"));
}
