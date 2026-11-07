//! Nothing on the audio thread allocates: every Morph preset, every effect in Morph's rack and
//! every filter type, and Tandem's oscillators, filters, shapers and effects, each played from a
//! fresh engine; and a Morph patch change and a Tandem program change made mid-play, each made
//! ready off the audio thread. One test in a binary of its own: the tables a process builds once
//! are built by nothing else here first, so a part that builds one on its render path is caught
//! however the tests run.

use makepad_audio_synth::kit::filter::TYPES;
use makepad_audio_synth::kit::fx::KINDS;
use makepad_audio_synth::morph::{Morph, MorphInstall, MorphPatch, PRESETS};
use makepad_audio_synth::tandem::program::Program;
use makepad_audio_synth::tandem::{Patch, Tandem, TandemInstall};
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

/// What this thread allocates while `f` runs.
fn allocations(f: impl FnOnce()) -> usize {
    let before = COUNT.with(|c| c.get());
    f();
    COUNT.with(|c| c.get()) - before
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
            allocated.push(format!("{what}: {n} allocations"));
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
    println!("{played} engines played, {} allocated on the audio thread", allocated.len());
    assert!(allocated.is_empty(), "the audio thread allocated:\n{}", allocated.join("\n"));
}
