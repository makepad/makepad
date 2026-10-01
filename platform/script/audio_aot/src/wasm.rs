//! Audio shaders as generated WebAssembly, for hosts that run in a browser
//! (the AudioWorklet thread) and cannot map native code. The code is the
//! compute core's wasm backend (`makepad_script_compute::wasm`, its module
//! ABI's audio part): one scalar entry per shader, called once per voice
//! (or effect) per block, bit-identical to [`AudioShader::run_interp`] and
//! the native backend.
//!
//! Flow on the web:
//! - Ahead of time (a film or app build on the desktop): [`record_compiled`]
//!   on while the document evaluates, [`take_recorded`] after; [`module`]
//!   compiles the recorded shaders into one module (export `run<k>` is
//!   shader k's entry) shipped with the shaders' [`AudioShader::program_key`]s
//!   in export order.
//! - At run time, the thread that runs audio (the AudioWorklet's instance
//!   of the app module, on the app's shared memory) instantiates that
//!   module against its memory, places each export in its own function
//!   table and calls [`link`] with (key, table slot) on that thread. From
//!   then on [`AudioShader::run`] on that thread calls the generated code
//!   for every shader whose program has a linked key (no JavaScript on the
//!   call); any other runs on the interpreter.
//!
//! Slots are per thread (each thread's instance has its own table), so the
//! registry is thread local.

use crate::AudioShader;
use makepad_script_compute::wasm::{self as backend, Entry, Target};
use std::sync::Arc;

/// One module whose export `run<k>` is `shaders[k]`'s render entry
/// (scalar; see the compute core's `wasm` module ABI for the entry's
/// parameters); None if a program uses what the backend does not compile.
/// `target`: how the module imports `env.memory` (a threaded app's memory
/// is shared with a maximum). Audio code has no fused multiply-add, so
/// `relaxed_fma` changes nothing here.
pub fn module(shaders: &[&AudioShader], target: Target) -> Option<Vec<u8>> {
    let entries: Vec<Entry> = shaders.iter().map(|s| Entry { program: &s.render, simd: false }).collect();
    backend::module(&entries, target)
}

/// Words of the `frame` scratch a call of `shader`'s entry needs (its
/// `scratch` from [`AudioShader::new_scratch`] is always large enough).
pub fn frame_words(shader: &AudioShader) -> usize {
    backend::frame_words(&shader.render, false)
}

thread_local! {
    static SLOTS: std::cell::RefCell<std::collections::HashMap<u64, u32>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Registers, for the calling thread, the function table slot holding the
/// generated entry of the program with this key (0 removes it). Called on
/// the audio thread after it linked a module (off the render path: it may
/// allocate).
pub fn link(key: u64, slot: u32) {
    SLOTS.with(|s| {
        let mut s = s.borrow_mut();
        if slot == 0 {
            s.remove(&key);
        } else {
            s.insert(key, slot);
        }
    });
}

/// The calling thread's slot for `key`, if linked.
pub fn slot(key: u64) -> Option<u32> {
    SLOTS.with(|s| s.try_borrow().ok().and_then(|s| s.get(&key).copied()))
}

/// Runs `n` frames (1..=MAX_FRAMES) on the generated entry at table slot
/// `slot`, with `scratch` as its frame. wasm32 only.
#[cfg(target_arch = "wasm32")]
pub(crate) fn run_slot(shader: &AudioShader, slot: u32, ctx: &mut [u32], state: &mut [u32], scratch: &mut [u32], ins: [&[f32]; 2], outs: [&mut [f32]; 2], n: usize) {
    let [o0, o1] = outs;
    let io = [ins[0].as_ptr() as u32, ins[1].as_ptr() as u32, o0.as_mut_ptr() as u32, o1.as_mut_ptr() as u32];
    type Entry = extern "C" fn(u32, u32, u32, u32, u32, u32);
    // SAFETY: `slot` was linked by this thread's host for this program's
    // entry of type (i32 x 6) -> () (the engine checks the type at the
    // call). The code clamps every access into the regions passed: ctx,
    // state, shared and scratch at their lengths (the entry was compiled
    // for this shader's), and every I/O index into 0..n, which the caller
    // checked each slice holds.
    let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(slot as usize) };
    f(ctx.as_mut_ptr() as u32, state.as_mut_ptr() as u32, shader.shared.as_ptr() as u32, io.as_ptr() as u32, n as u32, scratch.as_mut_ptr() as u32);
}

// -- recording (a build tool learns which audio programs a document makes) --

static RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn recorded() -> &'static (std::sync::mpsc::Sender<Arc<AudioShader>>, std::sync::Mutex<std::sync::mpsc::Receiver<Arc<AudioShader>>>) {
    static Q: std::sync::OnceLock<(std::sync::mpsc::Sender<Arc<AudioShader>>, std::sync::Mutex<std::sync::mpsc::Receiver<Arc<AudioShader>>>)> = std::sync::OnceLock::new();
    Q.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        (tx, std::sync::Mutex::new(rx))
    })
}

/// Keeps every audio shader compiled from now on (any thread) for
/// [`take_recorded`], or stops keeping them.
pub fn record_compiled(on: bool) {
    RECORDING.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The shaders compiled while recording, since the last call. Several
/// compiles of one program each appear; dedupe by
/// [`AudioShader::program_key`].
pub fn take_recorded() -> Vec<Arc<AudioShader>> {
    let Ok(rx) = recorded().1.lock() else { return Vec::new() };
    rx.try_iter().collect()
}

pub(crate) fn note_compiled(shader: &Arc<AudioShader>) {
    if RECORDING.load(std::sync::atomic::Ordering::Relaxed) {
        recorded().0.send(shader.clone()).ok();
    }
}
