//! Audio shader nodes in the player: hot swaps without clicks, no
//! allocation while rendering, slicing-independent output.

use makepad_score_player::shader::{compile, Expression, ProbeRecord, ProbeTap, ShaderFx, ShaderInst, SongTime, FADE, MAX_RENDER};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::Ordering;

struct Counting;
thread_local! {
    static ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static ALLOCS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static FAIL_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.with(|a| a.get()) {
            ALLOCS.with(|n| n.set(n.get() + 1));
        }
        if FAIL_BYTES.with(|n| n.get() == layout.size() && n.replace(0) != 0) { return std::ptr::null_mut(); }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.with(|a| a.get()) {
            ALLOCS.with(|n| n.set(n.get() + 1));
        }
        System.dealloc(ptr, layout)
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;

fn counting(f: impl FnOnce()) -> usize {
    ALLOCS.with(|n| n.set(0));
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    ALLOCS.with(|n| n.get())
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
    a.note_on(69.0, 1.0, 3, 1);
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
        b.note_on(69.0, 1.0, 3, 1);
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
fn hot_swap_keeps_voice_pressure_and_controllers() {
    let source = "fn voice() { vec2(pressure, cc(7)) }";
    let mut old = inst(source);
    old.note_on(60.0, 1.0, 3, 1);
    old.set_expression(3, Expression::Pressure, 0.75);
    old.set_cc(7, 0.5);
    let check = |s: &mut ShaderInst| {
        let (mut l, mut r) = ([0.0; MAX_RENDER], [0.0; MAX_RENDER]);
        s.render(&mut l, &mut r);
        assert_eq!(l, [0.75; MAX_RENDER], "voice pressure across swap");
        assert_eq!(r, [0.5; MAX_RENDER], "controller 7 across swap");
    };
    check(&mut old);
    let mut carried = inst(source);
    carried.swap_from(&mut old);
    check(&mut carried);
    let mut faded = inst(&format!("var extra = 0.0\n{source}"));
    faded.swap_from(&mut carried);
    check(&mut faded);
    check(&mut faded);
}

#[test]
fn ported_nodes_match_explicit_silence_and_carry_the_held_output() {
    const N: usize = 5;
    let silence = [0.0; N];
    let input = [0.25, -1.0, 0.5, 1.0, -0.25];
    for instrument in [false, true] {
        let entry = if instrument {
            "fn voice() { held = velocity + key.x; vec2(held, held + key.y) }"
        } else {
            "fn effect(l,r) { held = l + key.x; vec2(held, held + r + key.y) }"
        };
        let shader = compile(&format!("let key = input(2) let held = output(1) {entry}")).unwrap();
        let (mut left, mut right) = ([0.0; N], [0.0; N]);
        let (mut want_l, mut want_r, mut held) = ([0.0; N], [0.0; N], [0.0; N]);
        let mut ctx = vec![0; shader.ctx_words()];
        shader.init_ctx(&mut ctx, RATE);
        let mut scratch = vec![0; shader.scratch_words()];
        let mut state = shader.state_init().to_vec();
        if instrument {
            let mut synth = ShaderInst::new(shader.clone(), RATE);
            for (v, velocity) in [0.25, 0.5].into_iter().enumerate() {
                synth.note_on(60.0, velocity, v as u8, 1);
                shader.note_on(&mut state, 60.0, velocity, 0, 1, v as u32);
                shader.run_ports(&mut ctx, &mut state, &mut scratch, &[&silence[..]; 4],
                    &mut [&mut want_l, &mut want_r, &mut held], N);
            }
            synth.render(&mut left, &mut right);
            assert_eq!(held, [0.75; N], "output summed across voices");
        } else {
            let mut fx = ShaderFx::new(shader.clone(), RATE);
            fx.render(&input, &silence, &mut left, &mut right);
            shader.run_ports(&mut ctx, &mut state, &mut scratch, &[&input, &silence, &silence, &silence],
                &mut [&mut want_l, &mut want_r, &mut held], N);
            let changed = compile("let held = output(1) fn effect(l,r) { held }").unwrap();
            let mut next = ShaderFx::new(changed, RATE);
            next.swap_from(&mut fx);
            let quiet = [0.0; FADE as usize];
            next.render(&quiet, &quiet, &mut [0.0; FADE as usize], &mut [0.0; FADE as usize]);
            let (mut l, mut r) = ([0.0; N], [0.0; N]);
            next.render(&silence, &silence, &mut l, &mut r);
            assert_eq!((l,r), ([-0.25; N],[-0.25; N]), "held output carried by name");
        }
        assert_eq!((left,right), (want_l,want_r), "instrument={instrument}");
    }

    let mut old = ShaderFx::new(compile("fn effect(l,r) { 1 }").unwrap(), RATE);
    let mut new = ShaderFx::new(compile("let aux = output(2) fn effect(l,r) { aux = vec2(2,3); aux.x }").unwrap(), RATE);
    new.swap_from(&mut old);
    let frames = makepad_score_player::shader::FADE as usize + 1;
    let (z, mut l, mut r) = (vec![0.0; frames], vec![0.0; frames], vec![0.0; frames]);
    new.render(&z,&z,&mut l,&mut r);
    for (i,(&l,&r)) in l.iter().zip(&r).enumerate() {
        let want = 1.0 + i as f32 / (frames - 1) as f32;
        assert_eq!((l,r), (want,want), "stereo to ported fade at frame {i}");
    }
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
fn convolver_response_edits_while_playing() {
    use makepad_score_player::shader::{compile_with, library, AudioBindings, SampleView, FADE};
    // A polarity change at constant input makes even a one-frame jump
    // unambiguous. The delayed tap also needs the pre-edit input history.
    let effect = |len: usize, sign: f32| {
        let mut ir = vec![0.0; len];
        ir[0] = 0.75 * sign;
        ir[80] = 0.25 * sign;
        let samples = [SampleView { name: "ir", channels: 1, frames: len as u32, data: &ir }];
        let shader = compile_with(library("convolver").unwrap(), &AudioBindings { samples: &samples, ..Default::default() }).unwrap();
        ShaderFx::new(shader, RATE)
    };
    let input = [0.25; MAX_RENDER];
    let mut results = Vec::new();
    for (length, same_layout) in [(600, false), (100, true)] {
        let mut playing = effect(100, 1.0);
        let mut reference = effect(100, 1.0);
        let mut next = effect(length, -1.0);
        let render = |fx: &mut ShaderFx| {
            let (mut l, mut r) = ([0.0; MAX_RENDER], [0.0; MAX_RENDER]);
            fx.render(&input, &input, &mut l, &mut r);
            l
        };
        for _ in 0..3 { render(&mut playing); render(&mut reference); }
        let last = render(&mut playing)[MAX_RENDER-1];
        let baseline = max_step(&render(&mut reference));
        let old_box = playing.state().as_ptr();
        let mut output = [0.0; 2*MAX_RENDER+1];
        output[0] = last;
        let allocations = counting(|| {
            next.swap_from(&mut playing);
            for chunk in output[1..].chunks_exact_mut(MAX_RENDER) { chunk.copy_from_slice(&render(&mut next)); }
        });
        assert_eq!(allocations, 0, "swap or render allocated or freed at length {length}");
        assert_eq!(next.state().as_ptr() == old_box, same_layout);
        // The new response at its settled level: the convolver scales by the response's energy.
        let mut fresh = effect(length, -1.0);
        for _ in 0..3 { render(&mut fresh); }
        let settled = render(&mut fresh)[MAX_RENDER-1];
        assert!(settled < 0.0 && last > 0.0, "the edit inverts the polarity: {last} -> {settled}");
        assert!((output[2*MAX_RENDER] - settled).abs() < 1e-6, "new response must sound");
        // The new program's share grows by 1/FADE a frame, so frame t after the edit moves by at
        // most t/FADE of the swing (its own settling included); an unfaded switch jumps at once.
        let swing = (last - settled).abs();
        let unfaded = output.windows(2).enumerate().find_map(|(i, w)| {
            let (step, bound) = ((w[1] - w[0]).abs(), baseline + swing * (i + 1) as f32 / FADE as f32 + 1e-6);
            (step > bound).then_some((i + 1, step, bound))
        });
        results.push((length, unfaded));
    }
    assert!(results.iter().all(|(_, unfaded)| unfaded.is_none()),
        "response length, first unfaded frame after the edit (frame, step, bound): {results:?}");
}

#[test]
fn large_effect_edits_keep_the_playing_state() {
    const SOURCE: &str = "var history = [0.0; 1572864] var ticks = int(0)
        fn effect(l,r) { ticks += 1; history[1572863] += 0.125; vec2(history[1572863], float(ticks)) }";
    let shader = compile(SOURCE).unwrap();
    let mut playing = ShaderFx::new(shader.clone(), RATE);
    let mut reference = ShaderFx::new(shader.clone(), RATE);
    let render = |fx: &mut ShaderFx| {
        let (mut l, mut r) = ([0.0; 3], [0.0; 3]);
        fx.render(&[0.0; 3], &[0.0; 3], &mut l, &mut r);
        (l.map(f32::to_bits), r.map(f32::to_bits))
    };
    assert_eq!(render(&mut playing), render(&mut reference));
    let edit = SOURCE.replace("float(ticks)", "float(ticks) + 0.0");
    let mut same = ShaderFx::new(compile(&edit).unwrap(), RATE);
    let playing_box = playing.state().as_ptr();
    assert_eq!(counting(|| same.swap_from(&mut playing)), 0, "same-layout takeover allocated or freed");
    assert_eq!(same.state().as_ptr(), playing_box, "the edit takes over the playing box");
    assert!(playing.state().is_empty(), "the old program's state copy now belongs to the fade");
    let (left, right) = render(&mut same);
    let (want_l, want_r) = render(&mut reference);
    for (channel, actual, expected) in [("left", left, want_l), ("right", right, want_r)] {
        for (i, (sample, want)) in actual.into_iter().zip(expected).enumerate() {
            assert!(sample.abs_diff(want) <= 1, "{channel} at frame {i} must continue within one ulp");
        }
    }

    let edit = SOURCE.replace("fn effect", "var added = 0.5 fn effect").replace("ticks += 1", "added += 1.0; ticks += 1");
    let structural = compile(&edit).unwrap();
    for used in [false, true] {
        let mut new = ShaderFx::new(structural.clone(), RATE);
        if used { render(&mut new); }
        let buffer = new.state().as_ptr();
        let mut expected = same.state().to_vec();
        expected[makepad_script_audio_aot::header::FRESH as usize] = 1;
        expected.push(0.5f32.to_bits());
        assert_eq!(counting(|| new.swap_from(&mut same)), 0, "structural takeover allocated or freed");
        assert_eq!(new.state().as_ptr(), buffer);
        assert!(new.state() == expected, "some carried or initial words differ, rendered destination={used}");
        // The replaced instance's playing box now belongs to the fade.
        same = ShaderFx::new(shader.clone(), RATE);
        render(&mut same);
    }

    for entry in ["effect(l,r)", "voice()"] {
        let shader = compile(&format!("var buf = [0.0; 1237] fn {entry} {{ buf[0] }}")).unwrap();
        let bytes = shader.state_words() * 4;
        FAIL_BYTES.with(|n| n.set(bytes));
        let error = if entry == "voice()" { ShaderInst::try_new(shader, RATE).err() }
            else { ShaderFx::try_new(shader, RATE).err() };
        assert_eq!(error, Some(format!("could not allocate {bytes} bytes for shader state")));
    }
}

#[test]
fn effect_streams_are_decorrelated_and_repeat_after_reset() {
    let shader = compile("fn effect(l, r) { vec2(rand(), instance) }").unwrap();
    fn render<const N: usize>(fx: &mut ShaderFx) -> ([f32; N], [f32; N]) {
        let zeros = [0.0; N];
        let (mut noise, mut identity) = ([0.0; N], [0.0; N]);
        fx.render(&zeros, &zeros, &mut noise, &mut identity);
        (noise, identity)
    }
    let mut first = ShaderFx::new(shader.clone(), RATE);
    let mut second = ShaderFx::new(shader.clone(), RATE);
    first.set_seed(1);
    second.set_seed(2);
    let (a, id_a) = render::<1024>(&mut first);
    let (b, id_b) = render::<1024>(&mut second);
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    let (ma, mb) = (mean(&a), mean(&b));
    let (mut covariance, mut va, mut vb) = (0.0, 0.0, 0.0);
    for (&a, &b) in a.iter().zip(&b) {
        let (a, b) = (a - ma, b - mb);
        covariance += a * b;
        va += a * a;
        vb += b * b;
    }
    let correlation = covariance / (va * vb).sqrt();
    assert!(correlation.abs() < 0.1, "effect stream correlation {correlation}");
    assert_ne!(id_a[0], id_b[0], "effect identities differ");
    first.reset();
    second.reset();
    assert_eq!(render(&mut first), (a, id_a), "first effect after reset");
    assert_eq!(render(&mut second), (b, id_b), "second effect after reset");

    first.reset();
    render::<512>(&mut first);
    second.swap_from(&mut first);
    let (start, id) = render::<512>(&mut second);
    assert_ne!(start, a[512..], "the edit leaves the old stream");
    let fade = FADE as usize;
    for i in 0..fade {
        let gain = i as f32 / FADE as f32;
        let blend = |new, old| new * gain + old * (1.0 - gain);
        assert_eq!(start[i].to_bits(), blend(b[i], a[512 + i]).to_bits(), "noise during fade at {i}");
        assert_eq!(id[i].to_bits(), blend(id_b[i], id_a[512 + i]).to_bits(), "identity during fade at {i}");
    }
    assert_eq!(start[fade..], b[fade..512], "the new stream is fully in after the fade");
    assert_eq!(id[fade..], id_b[fade..512], "the edit takes the new instance");

    let mut next = ShaderFx::new(shader, RATE);
    next.set_seed(2);
    next.swap_from(&mut second);
    let (end, id) = render::<512>(&mut next);
    for i in 0..fade {
        assert!(end[i].to_bits().abs_diff(b[512 + i].to_bits()) <= 1, "same seed at sample {}", 512 + i);
    }
    assert_eq!(end[fade..], b[512 + fade..], "the same seed continues exactly after the fade");
    assert_eq!(id, id_b[512..], "the instance holds across the swap");
}

#[test]
fn a_stolen_voice_fades_on_its_own_clock() {
    use makepad_score_player::shader::{Steal, FADE, SHADER_VOICES};
    let code = "var phase = 0.0
        fn voice() { probe(slot, float(voice_index))
            phase = fract(phase + 1.0 / 256.0)
            if voice_index == 4 { vec2(velocity * sin(TAU * phase), 0.0) } else { vec2(0.0, velocity) } }";
    let mut pool = inst(code);
    let mut old = inst(code);
    let mut next = inst(code);
    for i in 0..SHADER_VOICES as u8 {
        pool.note_on(60.0, if i == 4 { 0.125 } else { 1.0 }, i, 7);
    }
    old.note_on(60.0, 0.125, 4, 7);
    let n = FADE as usize;
    let (mut warm, mut right) = (vec![0.0; n], vec![0.0; n]);
    pool.render(&mut warm, &mut right);
    old.render(&mut vec![0.0; n], &mut right);
    assert_eq!(pool.pick_voice(), 0, "oldest held voice");
    pool.set_steal(Steal::Quietest);
    let slot = pool.pick_voice();
    assert_eq!(slot, 4, "quietest held voice");
    let (ring, mut records) = makepad_lock_free::ring(32);
    pool.attach_probes(ProbeTap::new(ring, Default::default())).unwrap();
    pool.note_on(72.0, 0.0, slot, 11);
    let (mut faded, mut reference, mut r) = (vec![0.0; n + 8], vec![0.0; n + 8], vec![0.0; n + 8]);
    old.render(&mut reference, &mut r);
    pool.render(&mut faded[..17], &mut r[..17]);
    let record = std::iter::from_fn(|| records.pop()).find(|r| r.voice >= SHADER_VOICES as u16).expect("the spare feeds its probe");
    assert_eq!((record.last, record.frames), (4.0, 17), "spare keeps the stolen voice index");
    next.swap_from(&mut pool);
    next.render(&mut faded[17..], &mut r[17..]);
    for i in 0..n {
        assert_eq!(faded[i], reference[i] * (n - i) as f32 / n as f32, "fade frame {i}");
    }
    assert_eq!(&faded[n..], &[0.0; 8], "the spare stops at the fade boundary");
    assert!(max_step(&faded) <= max_step(&warm) + 1e-6, "stealing introduced a click");
    next.note_off(9, 0.5);
    for mode in [Steal::Oldest, Steal::Quietest] {
        next.set_steal(mode);
        assert_eq!(next.pick_voice(), 9, "a released voice goes before a held one");
    }
}

#[test]
fn shader_nodes_never_allocate_while_rendering() {
    let mut synth = ShaderInst::new(compile(&example("fm4")).unwrap(), RATE);
    let source = format!("let level = sample(1) let key = input(1) let aux = output(1)\n{}",
        SINE_RENAMED.replace("fn voice() {", "fn voice() { aux = key + velocity;").replace("* 0.5", "* level[0]"));
    let sample = makepad_score_player::shader::SampleView { name: "level", channels: 1, frames: 1, data: &[0.5] };
    let mut next = ShaderInst::new(makepad_score_player::shader::compile_with(&source, &makepad_score_player::shader::AudioBindings { samples: &[sample], ..Default::default() }).unwrap(), RATE);
    let mut fx = ShaderFx::new(compile(&example("waveshaper4x")).unwrap(), RATE);
    let mut l = vec![0.0f32; MAX_RENDER];
    let mut r = vec![0.0f32; MAX_RENDER];
    let mut ol = vec![0.0f32; MAX_RENDER];
    let mut or = vec![0.0f32; MAX_RENDER];
    let allocs = counting(|| {
        for note in 0..16 {
            let slot = synth.pick_voice();
            synth.note_on(48.0 + note as f32, 0.8, slot, 1);
        }
        assert_eq!(synth.steal_cuts(), 1);
        for block in 0..300 {
            if block % 20 == 0 {
                synth.note_on(48.0 + (block / 20) as f32, 0.8, ((block / 20) % 12) as u8, 1);
            }
            if block % 20 == 10 {
                synth.note_off(((block / 20) % 12) as u8, 0.5);
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

    let mut synth = inst("fn voice() { if pressure > 0.0 { probe(level, pressure) }\n pressure }");
    let (ring, mut reader) = makepad_lock_free::ring(2);
    let folded = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    synth.attach_probes(ProbeTap::new(ring, folded.clone())).unwrap();
    for voice in [1, 4] {
        synth.note_on(60.0, 1.0, voice, 1);
    }
    assert_eq!(counting(|| {
        for (a, b, n) in [(0.5, 0.5, 2), (0.75, 1.0, 3), (0.0, 0.25, 1), (0.25, 0.5, 2)] {
            synth.set_expression(1, Expression::Pressure, a);
            synth.set_expression(4, Expression::Pressure, b);
            synth.render(&mut l[..n], &mut r[..n]);
        }
    }), 0, "recording and folding allocate or free");
    assert_eq!(folded.load(Ordering::Relaxed), 5, "only reached probes fold");
    for voice in [1, 4] {
        assert_eq!(reader.pop(), Some(ProbeRecord { frame: 0, frames: 2, voice, probe: 0, last: 0.5, min: 0.5, max: 0.5 }));
    }
    synth.kill();
    assert_eq!(counting(|| synth.render(&mut l[..1], &mut r[..1])), 0, "flushing allocates or frees");
    for (voice, last, max) in [(1, 0.25, 0.75), (4, 0.5, 1.0)] {
        assert_eq!(reader.pop(), Some(ProbeRecord { frame: 2, frames: 6, voice, probe: 0, last, min: 0.25, max }));
    }
    assert!(reader.is_empty());

    // A new state variable forces a fade, and a preceding param moves the
    // index. Both programs must hear the same target at each absolute boundary.
    let source = "let amount = param(0,0,1,{smooth: 0}) fn effect(l,r) { amount }";
    let mut before = ShaderFx::new(compile(source).unwrap(), RATE);
    let changed = format!("let other = param(0,0,1) var added = 0.25 {}", source.replace("{ amount }", "{ amount + added }"));
    let mut after = ShaderFx::new(compile(&changed).unwrap(), RATE);
    assert_ne!(before.shader().param_index("amount"), after.shader().param_index("amount"));
    for fx in [&mut before, &mut after] {
        let i = fx.shader().param_index("amount").unwrap();
        fx.prepare_param_wires([i].into_iter());
    }
    let input = [0.0; 192];
    let (mut left, mut right) = ([0.0; 192], [0.0; 192]);
    let value = |_: usize, frame: usize| (frame / 64 + 1) as f32 * 0.25;
    assert_eq!(counting(|| {
        before.render_with_params(&input[..64], &input[..64], &mut left[..64], &mut right[..64], value);
        after.swap_from(&mut before);
        after.render_with_params(&input[64..], &input[64..], &mut left[64..], &mut right[64..], |slot, at| value(slot, at + 64));
    }), 0, "wired render, migration and fade allocate or free");
    for i in 0usize..192 {
        let mix = i.saturating_sub(64) as f32 / makepad_score_player::shader::FADE as f32;
        let want = value(0, i) + 0.25 * mix;
        assert_eq!((left[i], right[i]), (want, want), "wired fade, frame {i}");
    }

    // Every voice borrows the same stereo key. Check the output pair against
    // three isolated runs, then against the individual gains of the new,
    // stolen and fading voices. Reordering the bindings exercises both tables.
    let voice = "let key = input(2) let send = output(2) let level = param(0,0,1,{smooth: 0})
        fn voice() { send = key * (velocity * level); vec2(send.y, send.x) }";
    let edited = format!("let unused = input(1) let aux = output(1) let other = param(0,0,1) var twice = 2.0 {}",
        voice.replace("velocity * level", "twice * velocity * level"));
    let make = |code: &str| {
        let mut host = inst(code);
        let port = |name| (0..host.shader().port_count()).find(|&i| host.shader().port_view(i).name == name).unwrap();
        let (key, send, level) = (port("key"), port("send"), host.shader().param_index("level").unwrap());
        host.prepare_wires([key].into_iter(), [send].into_iter());
        host.prepare_param_wires([level].into_iter());
        host
    };
    let mut pool = make(voice);
    let mut replacement = make(&edited);
    let key_l: [f32; 256] = std::array::from_fn(|i| ((i % 7) + 1) as f32 / 8.0);
    let key_r: [f32; 256] = std::array::from_fn(|i| -(((i % 3) + 1) as f32) / 4.0);
    let side = |_: usize| Some((&key_l[..], &key_r[..]));
    let level = |_: usize, i: usize| (i / 64 + 1) as f32 / 4.0;
    let velocities = [0.125, 0.25, 0.5];
    let (mut sum_l, mut sum_r) = ([0.0; 64], [0.0; 64]);
    for (slot, velocity) in velocities.into_iter().enumerate() {
        pool.note_on(60.0, velocity, slot as u8, 1);
        let mut solo = make(voice);
        solo.note_on(60.0, velocity, slot as u8, 1);
        solo.begin_pass(64, &side);
        solo.render_piece(&mut sum_l, &mut sum_r, 0, &side, &level);
    }
    let (mut l, mut r) = ([0.0; 256], [0.0; 256]);
    assert_eq!(counting(|| {
        pool.begin_pass(64, &side);
        pool.render_piece(&mut l[..64], &mut r[..64], 0, &side, &level);
    }), 0, "wired instrument pass allocated or freed");
    let (aux_l, aux_r) = pool.output_buffer(0);
    assert_eq!((&l[..64], &r[..64]), (&sum_l[..], &sum_r[..]));
    assert_eq!((&aux_l[..64], &aux_r[..64]), (&sum_r[..], &sum_l[..]));
    l.fill(0.0);
    r.fill(0.0);
    assert_eq!(counting(|| {
        pool.note_on(72.0, 0.75, 0, 2);
        replacement.swap_from(&mut pool);
        replacement.begin_pass(256, &side);
        replacement.render_piece(&mut l, &mut r, 0, &side, &level);
    }), 0, "wired steal, swap and fade allocated or freed");
    let (aux_l, aux_r) = replacement.output_buffer(0);
    for i in 0..256 {
        let fade_in = i as f32 / makepad_score_player::shader::FADE as f32;
        let fade_out = 1.0 - fade_in;
        for (key, main, aux) in [(key_l[i], r[i], aux_l[i]), (key_r[i], l[i], aux_r[i])] {
            let sample = |vel: f32| key * (vel * level(0, i));
            let mut want = 0.0;
            for (scale, mix) in [(2.0, fade_in), (1.0, fade_out)] {
                for vel in [0.75, 0.25, 0.5] { want += sample(vel) * scale * mix; }
                want += sample(0.125) * scale * (mix * fade_out);
            }
            assert_eq!(main.to_bits(), want.to_bits(), "main voice sum at {i}");
            assert_eq!(aux.to_bits(), want.to_bits(), "named voice sum at {i}");
        }
    }

    // A released voice can still be audible through its named output alone.
    let mut tail = make(&voice.replace("vec2(send.y, send.x)", "0.0"));
    tail.note_on(60.0, 0.5, 0, 1);
    tail.begin_pass(1, &side);
    tail.render_piece(&mut [0.0], &mut [0.0], 0, &side, &level);
    tail.note_off(0, 0.5);
    tail.begin_pass(256, &side);
    tail.render_piece(&mut l, &mut r, 0, &side, &level);
    assert!(tail.active(0), "release with an audible send was cut");

    // A note starting between block ticks sees the last target on either
    // side of a wire edit or a program swap, even with a new parameter range.
    let mut kept = make(voice);
    kept.begin_pass(1, &side);
    kept.render_piece(&mut [0.0], &mut [0.0], 0, &side, &|_, _| 1.0);
    kept.take_prepared_wires(&mut make(voice));
    kept.note_on(60.0, 1.0, 0, 1);
    kept.begin_pass(1, &side);
    let mut out = [0.0];
    kept.render_piece(&mut [0.0], &mut out, 0, &side, &|_, _| 0.0);
    assert_eq!(out[0], key_l[0], "wire edit reset the held target");
    kept.kill();
    let mut changed = make(&voice.replace("param(0,0,1", "param(0,0,0.5"));
    changed.swap_from(&mut kept);
    changed.note_on(60.0, 1.0, 1, 2);
    changed.begin_pass(1, &side);
    let mut out = [0.0];
    changed.render_piece(&mut [0.0], &mut out, 0, &side, &|_, _| 0.0);
    assert_eq!(out[0], key_l[0] * 0.5, "new voice missed the held target in its edited range");

    let effect = |name: &str, value: usize| {
        let source = format!("var {name} = {value}.0\nfn effect(l, r) {{ probe(level, {name})\n vec2(l, r) }}");
        let source = if value == 2 {
            format!("let key = input(1) let aux = output(2) {}", source.replace("probe(level,", "aux = vec2(key,l); probe(level,"))
        } else { source };
        let mut fx = ShaderFx::new(compile(&source).unwrap(), RATE);
        let (ring, reader) = makepad_lock_free::ring(2);
        fx.attach_probes(ProbeTap::new(ring, Default::default())).unwrap();
        (fx, reader)
    };
    let (mut first, mut a) = effect("a", 1);
    let (mut second, mut b) = effect("b", 2);
    let (mut third, mut c) = effect("c", 3);
    let key = (0..second.shader().port_count()).find(|&i| second.shader().port_view(i).name == "key").unwrap();
    let aux = (0..second.shader().port_count()).find(|&i| second.shader().port_view(i).name == "aux").unwrap();
    second.prepare_wires([key].into_iter(), [aux].into_iter());
    assert_eq!(counting(|| {
        second.swap_from(&mut first);
        second.side_buffer(0).0[0] = 1.0;
        second.render_wired(&l[..1], &r[..1], &mut ol[..1], &mut or[..1]);
        third.swap_from(&mut second);
        third.render(&l[..1], &r[..1], &mut ol[..1], &mut or[..1]);
    }), 0, "replacing a fade allocates or frees");
    for (reader, frame, value) in [(&mut a, 0, 1.0), (&mut b, 0, 2.0), (&mut c, 1, 3.0)] {
        assert_eq!(reader.pop(), Some(ProbeRecord { frame, frames: 1, voice: u16::MAX, probe: 0, last: value, min: value, max: value }));
    }
    assert_eq!(b.pop().map(|r| (r.frame, r.last)), Some((1, 2.0)), "fading program kept its tap");
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
                s.note_on(45.0, 1.0, 0, 1);
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

/// The song's time reaches an effect and an instrument on its exact
/// sample however the pass is cut into calls, across a tempo change inside
/// the pass, and survives a hot swap.
#[test]
fn song_time_follows_the_pass_however_it_is_cut() {
    const PASS: usize = 256;
    const CHANGE: usize = 100;
    const AT: u32 = 1000;
    let per_frame = |bpm: f64| bpm / 60.0 / RATE as f64;
    let beat_change = 2.5 + (CHANGE as f64 + 10.0) * per_frame(120.0);
    let time = |beats_per_bar: f32| {
        let mut t = SongTime { count: 2, beats_per_bar, playing: true, ..SongTime::default() };
        t.anchors[0] = (2.5, -10, per_frame(120.0), 120.0);
        t.anchors[1] = (beat_change, CHANGE as i64, per_frame(150.0), 150.0);
        t
    };
    // The f32 value the program computes from the anchor in effect at pass frame `f`.
    let expect = |f: usize, per_bar: f32| {
        let (at, frame, per) = if f < CHANGE { (2.5, -10, per_frame(120.0)) } else { (beat_change, CHANGE as i32, per_frame(150.0)) };
        let beat = at as f32 + ((f as i32 - frame) as f32) * (per as f32);
        (beat, beat / per_bar)
    };
    let slicings: [&[usize]; 3] = [&[PASS], &[97, 159], &[1, 7, 64, 128, 3, 53]];
    let probes = [0usize, 1, 99, 100, 101, 200, 255];

    let effect = "fn effect(l, r) { vec2(beat, bar) }";
    let render_fx = |fx: &mut ShaderFx, from: usize, to: usize, slices: &[usize], out: &mut (Vec<f32>, Vec<f32>)| {
        let zeros = vec![0.0f32; PASS];
        let (mut at, mut k) = (from, 0);
        while at < to {
            let n = slices[k % slices.len()].min(to - at);
            fx.render(&zeros[..n], &zeros[..n], &mut out.0[at..at + n], &mut out.1[at..at + n]);
            at += n;
            k += 1;
        }
    };
    let new_fx = |src: &str| {
        let mut fx = ShaderFx::new(compile(src).unwrap(), RATE);
        fx.sync_frame(AT);
        fx.set_song_time(&time(4.0));
        fx
    };
    let mut reference: Option<(Vec<u32>, Vec<u32>)> = None;
    for slices in slicings {
        let mut fx = new_fx(effect);
        let mut out = (vec![0.0f32; PASS], vec![0.0f32; PASS]);
        render_fx(&mut fx, 0, PASS, slices, &mut out);
        let bits = (out.0.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), out.1.iter().map(|x| x.to_bits()).collect::<Vec<_>>());
        match &reference {
            None => reference = Some(bits),
            Some(want) => assert!(&bits == want, "effect slicing: calls of {slices:?} differ from one call of {PASS}"),
        }
        for f in probes {
            let (beat, bar) = expect(f, 4.0);
            assert_eq!((out.0[f].to_bits(), out.1[f].to_bits()), (beat.to_bits(), bar.to_bits()), "effect formula at frame {f}: got ({}, {}), want ({beat}, {bar})", out.0[f], out.1[f]);
        }
    }

    // The instrument, a note on, the same pass and slicings.
    let mut reference: Option<Vec<u32>> = None;
    for slices in slicings {
        let mut i = inst("fn voice() { vec2(beat, bar) }");
        i.sync_frame(AT);
        i.set_song_time(&time(3.0));
        i.note_on(69.0, 1.0, 0, 1);
        let (mut l, mut r) = (vec![0.0f32; PASS], vec![0.0f32; PASS]);
        let (mut at, mut k) = (0, 0);
        while at < PASS {
            let n = slices[k % slices.len()].min(PASS - at);
            i.render(&mut l[at..at + n], &mut r[at..at + n]);
            at += n;
            k += 1;
        }
        let bits: Vec<u32> = l.iter().chain(&r).map(|x| x.to_bits()).collect();
        match &reference {
            None => reference = Some(bits),
            Some(want) => assert!(&bits == want, "instrument slicing: calls of {slices:?} differ from one call of {PASS}"),
        }
        for f in probes {
            let (beat, bar) = expect(f, 3.0);
            assert_eq!((l[f].to_bits(), r[f].to_bits()), (beat.to_bits(), bar.to_bits()), "instrument formula at frame {f}: got ({}, {}), want ({beat}, {bar})", l[f], r[f]);
        }
    }

    // A quarter cycle per block, once from Hz and once from the transport.
    // A quarter-cycle phase offset puts the first phase at 1/4.
    let lfo = compile(makepad_score_player::shader::library("lfo").unwrap()).unwrap();
    let mut clock = SongTime { count: 1, beats_per_bar: 4.0, playing: true, ..Default::default() };
    clock.anchors[0] = (0.0, 0, 1.0 / 256.0, 1920.0);
    for beats in [0.0, 1.0] {
        for (shape, wave) in [[1.0, 0.0, -1.0, 0.0], [0.0, 1.0, 0.0, -1.0],
                [-0.5, 0.0, 0.5, -1.0], [1.0, -1.0, -1.0, 1.0], [0.0; 4]].into_iter().enumerate() {
            let mut node = ShaderFx::new(lfo.clone(), 8192.0);
            node.prepare_wires(std::iter::empty(), [0].into_iter());
            for (param, value) in [("rate", if beats == 0.0 { 32.0 } else { 0.0 }), ("beats", beats), ("phase", 0.25),
                ("shape", shape as f32), ("depth", 2.0), ("center", 3.0)] {
                assert!(node.set_param_by_name(param, value));
            }
            node.set_song_time(&clock);
            node.render_wired(&[0.0; PASS], &[0.0; PASS], &mut [0.0; PASS], &mut [0.0; PASS]);
            let (l, r) = node.output_buffer(0);
            assert_eq!(l, r, "mono output duplicates, shape {shape}");
            for (i, block) in l.chunks_exact(64).enumerate() {
                assert!(block.iter().all(|x| *x == block[0]), "held across block {i}");
                if shape < 4 { assert!((block[0] - (3.0 + 2.0 * wave[i])).abs() < 1e-5, "shape {shape}, beats {beats}, block {i}: {}", block[0]); }
            }
            if shape == 4 {
                assert!(l[..192].iter().all(|x| *x == l[0]) && l[192] != l[0], "hold draws once per cycle");
            }
        }
    }

    // Hot swap mid-pass, before the tempo change: a carried swap keeps the
    // time exactly; a swap that cannot carry state crossfades two programs
    // that read the same time, so the beat stays (to the fade's rounding).
    for (code, exact) in [("fn effect(l, r) { vec2(beat * 1.0, bar) }", true), ("var count = 0.0\nfn effect(l, r) { count = count + 1.0\n vec2(beat, bar) }", false)] {
        let mut old = new_fx(effect);
        let mut out = (vec![0.0f32; PASS], vec![0.0f32; PASS]);
        render_fx(&mut old, 0, 64, &[64], &mut out);
        let mut fresh = ShaderFx::new(compile(code).unwrap(), RATE);
        fresh.swap_from(&mut old);
        render_fx(&mut fresh, 64, PASS, &[97, 159], &mut out);
        for f in [64usize, 99, 100, 150, 255] {
            let (beat, bar) = expect(f, 4.0);
            let ok = if exact { out.0[f].to_bits() == beat.to_bits() && out.1[f].to_bits() == bar.to_bits() } else { (out.0[f] - beat).abs() <= beat * 1e-5 && (out.1[f] - bar).abs() <= bar * 1e-5 };
            assert!(ok, "hot swap keeps the time (exact: {exact}) at frame {f}: got ({}, {}), want ({beat}, {bar})", out.0[f], out.1[f]);
        }
    }
}
