//! Audio shader nodes in the player: hot swaps without clicks, no
//! allocation while rendering, slicing-independent output.

use makepad_score_player::shader::{compile, ShaderFx, ShaderInst, MAX_RENDER};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.with(|a| a.get()) {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.with(|a| a.get()) {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        System.dealloc(ptr, layout)
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;

fn counting(f: impl FnOnce()) -> usize {
    let before = ALLOCS.load(Ordering::SeqCst);
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    ALLOCS.load(Ordering::SeqCst) - before
}

const RATE: f32 = 48000.0;

fn example(name: &str) -> String {
    std::fs::read_to_string(format!("{}/../../platform/script/audio_aot/shaders/{}.splash", env!("CARGO_MANIFEST_DIR"), name)).unwrap()
}

const SINE: &str = "var phase = 0.0\nfn voice() { phase = fract(phase + freq / sample_rate)\n sin(TAU * phase) * 0.5 }";
const SINE_LOUDER: &str = "var phase = 0.0\nfn voice() { phase = fract(phase + freq / sample_rate)\n sin(TAU * phase) * 0.6 }";
const SINE_RENAMED: &str = "var ph = 0.0\nfn voice() { ph = fract(ph + freq / sample_rate)\n sin(TAU * ph) * 0.5 }";

fn inst(src: &str) -> ShaderInst {
    ShaderInst::new(compile(src).unwrap(), RATE)
}

/// Largest sample-to-sample step.
fn max_step(x: &[f32]) -> f32 {
    x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
}

/// Plays a note, swaps to `next` after 20 blocks, returns the left channel.
fn swap_render(first: &str, next: &str, fade: bool) -> Vec<f32> {
    let mut a = inst(first);
    a.note_on(69.0, 1.0, 3);
    let mut out = Vec::new();
    let mut l = vec![0.0f32; MAX_RENDER];
    let mut r = vec![0.0f32; MAX_RENDER];
    for _ in 0..20 {
        l.fill(0.0);
        r.fill(0.0);
        a.render(&mut l, &mut r);
        out.extend_from_slice(&l);
    }
    let mut b = inst(next);
    if fade {
        b.swap_from(&mut a);
    } else {
        // A naive swap: the new program starts from scratch.
        b.note_on(69.0, 1.0, 3);
    }
    for _ in 0..20 {
        l.fill(0.0);
        r.fill(0.0);
        b.render(&mut l, &mut r);
        out.extend_from_slice(&l);
    }
    out
}

#[test]
fn hot_swap_does_not_click() {
    let steady = max_step(&swap_render(SINE, SINE, true)[..20 * MAX_RENDER]);
    // Same layout: state carries, the swap is seamless.
    let carried = swap_render(SINE, SINE_LOUDER, true);
    assert!(max_step(&carried) < steady * 1.3, "carried swap stepped {} vs {}", max_step(&carried), steady);
    // Renamed state: nothing carries, so it crossfades.
    let faded = swap_render(SINE, SINE_RENAMED, true);
    assert!(max_step(&faded) < steady * 1.3, "crossfaded swap stepped {} vs {}", max_step(&faded), steady);
    // The check is sensitive: restarting the new program without a fade clicks.
    let naive = swap_render(SINE, SINE_RENAMED, false);
    assert!(max_step(&naive) > steady * 2.0, "naive swap stepped only {}", max_step(&naive));
}

#[test]
fn effect_hot_swap_carries_delay_lines() {
    let src = example("allpass_reverb");
    let tweaked = src.replace("0.5)\n        let dl", "0.55)\n        let dl");
    assert_ne!(src, tweaked);
    let mut a = ShaderFx::new(compile(&src).unwrap(), RATE);
    let impulse: Vec<f32> = (0..MAX_RENDER).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
    let silence = vec![0.0f32; MAX_RENDER];
    let mut l = vec![0.0f32; MAX_RENDER];
    let mut r = vec![0.0f32; MAX_RENDER];
    a.render(&impulse, &impulse, &mut l, &mut r);
    for _ in 0..40 {
        a.render(&silence, &silence, &mut l, &mut r);
    }
    let mut b = ShaderFx::new(compile(&tweaked).unwrap(), RATE);
    b.swap_from(&mut a);
    // The tail keeps ringing through the swap (the delay lines carried).
    let mut tail = 0.0f32;
    for _ in 0..8 {
        l.fill(0.0);
        r.fill(0.0);
        b.render(&silence, &silence, &mut l, &mut r);
        tail = tail.max(l.iter().chain(&r).fold(0.0, |m, x| m.max(x.abs())));
    }
    assert!(tail > 1e-4, "the reverb tail died at the swap: {}", tail);
}

#[test]
fn shader_nodes_never_allocate_while_rendering() {
    let mut synth = ShaderInst::new(compile(&example("fm4")).unwrap(), RATE);
    let mut next = ShaderInst::new(compile(SINE_RENAMED).unwrap(), RATE);
    let mut fx = ShaderFx::new(compile(&example("waveshaper4x")).unwrap(), RATE);
    let mut l = vec![0.0f32; MAX_RENDER];
    let mut r = vec![0.0f32; MAX_RENDER];
    let mut ol = vec![0.0f32; MAX_RENDER];
    let mut or = vec![0.0f32; MAX_RENDER];
    let allocs = counting(|| {
        for block in 0..300 {
            if block % 20 == 0 {
                synth.note_on(48.0 + (block / 20) as f32, 0.8, ((block / 20) % 12) as u8);
            }
            if block % 20 == 10 {
                synth.note_off(((block / 20) % 12) as u8);
            }
            synth.set_param(0, 3.0);
            l.fill(0.0);
            r.fill(0.0);
            synth.render(&mut l, &mut r);
            fx.render(&l, &r, &mut ol, &mut or);
        }
        // A crossfading swap on the audio thread.
        next.swap_from(&mut synth);
        for _ in 0..10 {
            next.render(&mut l, &mut r);
        }
    });
    assert_eq!(allocs, 0);
}

#[test]
fn shader_instrument_output_ignores_host_slicing() {
    let src = example("saw_svf");
    let run = |sizes: &[usize]| {
        let mut s = inst(&src);
        let mut out = vec![0.0f32; 20000];
        let mut r = vec![0.0f32; 20000];
        let mut at = 0;
        let mut k = 0;
        while at < out.len() {
            if at == 0 {
                s.note_on(45.0, 1.0, 0);
            }
            if at == 9000 {
                s.set_param(0, 3000.0);
            }
            let n = sizes[k % sizes.len()].min(out.len() - at).min(if at < 9000 { 9000 - at } else { usize::MAX });
            k += 1;
            s.render(&mut out[at..at + n], &mut r[at..at + n]);
            at += n;
        }
        out.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
    };
    assert_eq!(run(&[128]), run(&[5, 128, 17, 64, 1, 100]));
}
