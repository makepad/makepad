//! The audio callbacks' halves of a call never allocate or free: a counting allocator watches
//! the calling thread while it pushes a microphone, renders the mix (mono and planar), and takes a
//! new set of peers in the middle. A test target of its own, so its allocator touches no other test.

use makepad_call::driver::audio::audio_ports;
use makepad_call::aec::RenderRefRing;
use makepad_call::driver::voice::{voice_in, voice_in_with_aec, voice_out, voice_out_with_ref, VoiceDecoder, VoiceMarks};
use makepad_call::wire::stream;
use makepad_teamtalk::jitter::JitterRing;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static FREES: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

fn note(counter: &AtomicU64) {
    if WATCHING.with(Cell::get) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: every call goes straight to the system allocator; counting touches only an atomic and a
// const-initialised thread local, which allocate nothing.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(&ALLOCS);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(&FREES);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        note(&FREES);
        note(&ALLOCS);
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn watched<R>(f: impl FnOnce() -> R) -> R {
    WATCHING.with(|w| w.set(true));
    let r = f();
    WATCHING.with(|w| w.set(false));
    r
}

#[test]
fn the_audio_callbacks_never_allocate_or_free() {
    let (mut input, mut frames) = voice_in(Instant::now(), Arc::new(AtomicBool::new(false)), Arc::new(VoiceMarks::default()));
    let tap = frames.transcript_tap().unwrap();
    tap.set_on(true);
    let mut samples = Vec::new();
    let (mut output, mut control) = voice_out();
    let ring = Arc::new(JitterRing::new());
    let marks = Arc::new(VoiceMarks::default());
    let level = Arc::default();
    control.set([(1, stream::VOICE, ring.clone(), marks.clone(), Arc::clone(&level))]);
    let mut decoder = VoiceDecoder::default();
    let mut payload = Vec::new();
    let (mut phase, mut block, mut mono, mut planar) = (0f32, [0f32; 480], [0f32; 480], [0f32; 960]);
    let mut loud = 0;
    for n in 0..1000 {
        if n == 500 {
            // a second peer joins: the callback swaps the new set in and gives the old one back
            control.set([(1, stream::VOICE, ring.clone(), marks.clone(), level.clone()), (2, stream::VOICE, Arc::new(JitterRing::new()), Arc::new(VoiceMarks::default()), Arc::default())]);
        }
        let talking = n % 200 < 150;
        for s in block.iter_mut() {
            *s = if talking { 0.3 * (phase * std::f32::consts::TAU).sin() } else { 0.0 };
            phase = (phase + 440.0 / 48_000.0).fract();
        }
        watched(|| input.push_mono(48_000.0, &block));
        while let Some(f) = frames.pop(&mut payload) {
            assert!(tap.pop(&mut samples).is_some(), "the watched callback must write the tap");
            decoder.insert(&ring, f.seq, f.flags, &payload);
        }
        control.tick();
        // the device's block, mono and planar stereo in turn (each 10 ms, as the input)
        let heard = if n % 2 == 0 {
            mono.fill(0.0);
            watched(|| output.mix_into_mono(48_000.0, &mut mono));
            &mono[..]
        } else {
            planar.fill(0.0);
            watched(|| output.mix_into_planar(48_000.0, 480, 2, &mut planar));
            &planar[..480]
        };
        loud += usize::from(heard.iter().map(|s| s * s).sum::<f32>() / 480.0 > 0.01);
    }
    assert!(loud > 500, "the tone must play through the mix (loud in {loud} of 1000 blocks), or the test proves nothing");
    // The controller runs outside the allocator watch. Only what the installed device
    // callbacks do in response to these changes belongs to their allocation budget.
    let (mut mic, mut speakers, mut ports) = audio_ports();
    let mut planar = [0.0; 960];
    for call in 0..2 {
        let (input, frames) = voice_in(Instant::now(), Arc::default(), Arc::default());
        let (output, mut mix) = voice_out();
        let ring = Arc::new(JitterRing::new());
        let level = Arc::default();
        mix.set([(1, stream::VOICE, ring.clone(), Arc::default(), Arc::clone(&level))]);
        ports.attach(Some(input), output);
        for n in 0..24 {
            if n == 12 { level.set_gain(0.25); }
            ports.tick();
            mix.tick();
            // Switch devices halfway through a 20 ms wire frame.
            let (rate, samples) = if n < 11 { (48_000.0, 480) } else { (44_100.0, 441) };
            for x in &mut planar[..samples] {
                *x = 0.3 * (phase * std::f32::consts::TAU).sin();
                phase = (phase + 440.0 / rate as f32).fract();
            }
            if n < 11 {
                planar.copy_within(..480, 480);
                watched(|| mic.push_planar(rate, 480, 2, &planar));
            } else {
                watched(|| mic.push_mono(rate, &planar[..samples]));
            }
            while let Some(f) = frames.pop(&mut payload) {
                decoder.insert(&ring, f.seq, f.flags, &payload);
            }
            watched(|| speakers.fill_interleaved(rate, 2, &mut planar[..2 * samples]));
        }
        ports.detach();
        ports.ring(true);
        watched(|| mic.push_interleaved(48_000.0, 2, &planar));
        watched(|| speakers.fill_planar(48_000.0, 480, 2, &mut planar));
        assert!(planar.iter().any(|x| x.abs() > 0.1), "the ring should play between calls after call {call}");
        ports.ring(false);
        watched(|| speakers.fill_interleaved(48_000.0, 2, &mut planar));
        assert!(planar.iter().all(|&x| x == 0.0), "a detached and quiet port must clear the block after call {call}");
        ports.tick();
    }
    let counts = (ALLOCS.load(Ordering::Relaxed), FREES.load(Ordering::Relaxed));
    eprintln!("audio callbacks: {} allocations, {} frees", counts.0, counts.1);
    assert_eq!(counts, (0, 0), "the input and output callbacks must neither allocate nor free");

    let reference = RenderRefRing::new();
    let muted = Arc::new(AtomicBool::new(false));
    let (mut input, mut frames) = voice_in_with_aec(Instant::now(), muted.clone(), Arc::default(), Some(reference.clone()));
    let tap = frames.transcript_tap().unwrap();
    tap.set_on(true);
    let (mut output, mut control) = voice_out_with_ref(Some(reference));
    let far = Arc::new(JitterRing::new());
    control.set([(1, stream::VOICE, far.clone(), Arc::default(), Arc::default())]);
    let mut count = 0;
    let mut pcm = [0i16; 960];
    for (i, x) in pcm.iter_mut().enumerate() {
        *x = (8000.0 * (i as f32 * std::f32::consts::TAU / 96.0).sin()) as i16;
    }
    for i in 0..500 {
        if i == 200 { muted.store(true, Ordering::Relaxed); }
        if i == 250 { muted.store(false, Ordering::Relaxed); }
        if i % 2 == 0 { far.insert(i / 2, &pcm, false); }
        mono.fill(0.0);
        watched(|| output.mix_into_mono(48_000.0, &mut mono));
        match i % 3 {
            0 => watched(|| input.push_mono(48_000.0, &mono)),
            1 => {
                planar[..480].copy_from_slice(&mono);
                planar[480..].copy_from_slice(&mono);
                watched(|| input.push_planar(48_000.0, 480, 2, &planar));
            }
            _ => {
                for (pair, &x) in planar.chunks_exact_mut(2).zip(&mono) { pair.fill(x); }
                watched(|| input.push_interleaved(48_000.0, 2, &planar));
            }
        }
        while frames.pop(&mut payload).is_some() {
            tap.pop(&mut samples).expect("AEC frames reach the transcript tap synchronously");
            count += 1;
        }
    }
    assert_eq!(count, 250, "AEC delivers one frame for every two device blocks");
    let counts = (ALLOCS.load(Ordering::Relaxed), FREES.load(Ordering::Relaxed));
    assert_eq!(counts, (0, 0), "software echo cancellation must neither allocate nor free on either callback");

    let reference = RenderRefRing::new();
    let muted = Arc::new(AtomicBool::new(false));
    let (mut input, mut frames) = voice_in_with_aec(Instant::now(), muted.clone(), Arc::default(), Some(reference.clone()));
    let tap = frames.transcript_tap().unwrap();
    tap.set_on(true);
    let (mut output, mut control) = voice_out_with_ref(Some(reference.clone()));
    let far = Arc::new(JitterRing::new());
    control.set([(1, stream::VOICE, far.clone(), Arc::default(), Arc::default())]);
    let mut render_block = 0;
    let mut render = |out: &mut [f32]| {
        if render_block % 2 == 0 { far.insert(render_block / 2, &pcm, false); }
        render_block += 1;
        out.fill(0.0);
        watched(|| output.mix_into_mono(48_000.0, out));
    };
    let mut capture = |rate, mic: &[f32]| {
        watched(|| input.push_mono(rate, mic));
        while frames.pop(&mut payload).is_some() {
            tap.pop(&mut samples).expect("recovery frames reach the transcript tap");
        }
    };
    for _ in 0..10 { render(&mut mono); }
    for _ in 0..30 {
        render(&mut mono);
        capture(48_000.0, &mono);
    }
    reference.set_capture_muted(true);
    muted.store(true, Ordering::Release);
    for _ in 0..60 { render(&mut mono); }
    reference.set_capture_muted(false);
    muted.store(false, Ordering::Release);
    for _ in 0..5 { render(&mut mono); }
    assert_eq!(reference.overflows(), 0, "closed-mic overflow must stay quiet");
    assert!(reference.paused_overflows() > 0, "resumed callback must exercise paused-overflow recovery");
    capture(48_000.0, &mono);
    for _ in 0..20 { render(&mut mono); }
    capture(48_000.0, &mono);
    reference.request_reset();
    capture(48_000.0, &mono);
    capture(44_100.0, &mono[..441]);
    let counts = (ALLOCS.load(Ordering::Relaxed), FREES.load(Ordering::Relaxed));
    assert_eq!(counts, (0, 0), "backlog trim, capture-gap and device resets must neither allocate nor free");
}
