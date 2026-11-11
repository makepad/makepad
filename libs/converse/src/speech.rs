//! Speech output: a hub TTS session plus the player it feeds.
//!
//! The hub hands back PCM rather than owning a device, so playback goes
//! through `cx.audio_output` like any other audio in Makepad. Muting is then
//! just "stop feeding the player", which also makes it instant.
//!
//! Streamed reply text goes in through [`SpeechOutput::feed`]; each finished
//! sentence is queued on the session and spoken while the rest of the reply
//! is still being generated. Which voice speaks — Kokoro in this process, on
//! the machine node, on a LAN box, or the OS voice — is the hub's decision
//! ([`makepad_ai_hub::speech`]); this file only plays what comes back.
//!
//! The audio callback owns a [`SpeechRenderer`] and takes no lock. The pump thread hands it each
//! spoken sentence as one chunk (its samples, their rate and the speech epoch) through a bounded
//! ring; the renderer hands every chunk it is done with back through a second ring, so the
//! samples are freed on the pump, never on the device thread. That ring holds the first one's
//! capacity plus the chunk being played, and the pump empties it before each send, so it never
//! fills. [`SpeechOutput::stop`] moves the epoch on: the renderer hands back every older chunk.

#[cfg(feature = "tts")]
use makepad_ai_hub::speech::{TtsConfig, TtsEngine, TtsEvent, TtsHandle, TtsSession};
use makepad_lock_free::{ring, Consumer, Producer};
use makepad_widgets_core::log;
use makepad_widgets_core::makepad_draw::audio::AudioBuffer;
#[cfg(feature = "tts")]
use makepad_widgets_core::makepad_draw::thread::ThreadOptions;
use makepad_widgets_core::makepad_draw::thread::ThreadSpawner;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(any(feature = "tts", test))]
use std::sync::Mutex;
#[cfg(feature = "tts")]
use std::sync::OnceLock;

/// Sentences on their way to the device at once: more than a reply has. A pump that finds the
/// ring full waits for room ([`Pump::send`]).
const CHUNKS: usize = 64;

/// One spoken sentence: mono samples at their own rate, and the speech epoch it was sent in.
struct Chunk {
    pcm: Arc<[f32]>,
    rate: f64,
    epoch: u64,
}

/// What the UI, the pump and the renderer share: atomics only.
#[derive(Default)]
struct Shared {
    /// Moved on by `stop`: the renderer hands back every chunk of an older epoch unplayed.
    epoch: AtomicU64,
    /// Chunks given to the pump to send, and chunks the renderer finished or handed back
    /// unplayed: speech is queued or playing while they differ.
    sent: AtomicU64,
    done: AtomicU64,
    /// Chunks the renderer leaked because the retire ring was full: a sizing bug, so it stays 0.
    retire_lost: AtomicU64,
}

/// The pump's ends: chunks to the renderer, and the played ones back for their drop.
#[cfg(any(feature = "tts", test))]
struct Pump {
    chunks: Producer<Chunk>,
    played: Consumer<Chunk>,
    shared: Arc<Shared>,
}

#[cfg(any(feature = "tts", test))]
impl Pump {
    /// Sends one sentence to the renderer, after dropping every chunk it handed back. A full ring
    /// (a device that does not run) makes the pump wait for room, a block at a time, and keep
    /// the sentence; a stop meanwhile ends the wait and drops it.
    fn send(&mut self, pcm: Arc<[f32]>, rate: f64) {
        let mut chunk = Chunk { pcm, rate, epoch: self.shared.epoch.load(Ordering::Acquire) };
        self.shared.sent.fetch_add(1, Ordering::AcqRel);
        loop {
            self.drain();
            match self.chunks.push(chunk) {
                Ok(()) => return,
                Err(back) => {
                    if back.epoch < self.shared.epoch.load(Ordering::Acquire) {
                        self.shared.done.fetch_add(1, Ordering::AcqRel);
                        return;
                    }
                    chunk = back;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    }

    /// Drops the chunks the renderer is done with, here on the pump's thread.
    fn drain(&mut self) {
        while self.played.pop().is_some() {}
    }
}

/// The speech half of an audio callback: it plays the sentences the pump sends, in order, at the
/// device's rate, and hands each back when it is done. No lock, no allocation, no free.
pub struct SpeechRenderer {
    chunks: Consumer<Chunk>,
    played: Producer<Chunk>,
    shared: Arc<Shared>,
    current: Option<Chunk>,
    /// Where `current` plays, in its own samples.
    cursor: f64,
}

impl SpeechRenderer {
    /// Mixes the queued speech additively into every channel of `output`, resampled to
    /// `device_rate` (a new device rate keeps the pitch: the step is taken per block).
    pub fn mix_into(&mut self, output: &mut AudioBuffer, device_rate: f64) {
        let epoch = self.shared.epoch.load(Ordering::Acquire);
        let (frames, channels) = (output.frame_count(), output.channel_count());
        let mut frame = 0;
        while frame < frames && device_rate > 0.0 && self.next_chunk(epoch) {
            let Some(chunk) = &self.current else { break };
            let (pcm, step) = (&chunk.pcm, chunk.rate / device_rate);
            let mut ended = false;
            while frame < frames {
                let index = self.cursor as usize;
                if index + 1 >= pcm.len() {
                    ended = true;
                    break;
                }
                let fraction = (self.cursor - index as f64) as f32;
                let sample = pcm[index] + (pcm[index + 1] - pcm[index]) * fraction;
                for channel in 0..channels {
                    output.channel_mut(channel)[frame] += sample;
                }
                self.cursor += step;
                frame += 1;
            }
            if ended {
                self.finish();
            }
        }
    }

    /// A block that plays nothing (muted): chunks of an older epoch still go back.
    pub fn idle(&mut self) {
        let epoch = self.shared.epoch.load(Ordering::Acquire);
        self.next_chunk(epoch);
    }

    /// Makes `current` a chunk of `epoch`, handing back the older and the unplayable ones; false
    /// when none is queued. Chunks arrive in epoch order, so a current chunk of this epoch has
    /// only such chunks behind it.
    fn next_chunk(&mut self, epoch: u64) -> bool {
        if self.current.as_ref().is_some_and(|c| c.epoch < epoch) {
            self.finish();
        }
        while self.current.is_none() {
            let Some(chunk) = self.chunks.pop() else { return false };
            if chunk.epoch < epoch || chunk.pcm.len() < 2 || !chunk.rate.is_finite() || chunk.rate <= 0.0 {
                self.retire(chunk);
            } else {
                self.current = Some(chunk);
                self.cursor = 0.0;
            }
        }
        true
    }

    fn finish(&mut self) {
        if let Some(chunk) = self.current.take() {
            self.retire(chunk);
        }
    }

    /// Hands `chunk` back for its drop on the pump. A full retire ring breaks the sizing rule
    /// (a bug): it is counted, and the chunk leaked rather than freed on the device thread.
    fn retire(&mut self, chunk: Chunk) {
        if let Err(chunk) = self.played.push(chunk) {
            debug_assert!(false, "the speech retire ring was full: it must hold every chunk plus the one playing");
            self.shared.retire_lost.fetch_add(1, Ordering::Relaxed);
            std::mem::forget(chunk);
        }
        self.shared.done.fetch_add(1, Ordering::AcqRel);
    }
}

/// Don't speak a fragment shorter than this — one-word clips sound like hiccups.
const MIN_SPOKEN_CHARS: usize = 16;

/// Speech output: a hub TTS session plus the player it feeds.
pub struct SpeechOutput {
    /// Kokoro voice pack name (or any [`makepad_ai_hub::speech::Voice`] id).
    #[cfg(feature = "tts")]
    voice: String,
    /// Auto keeps the general conversation fallback; a feature that promises
    /// Kokoro can opt out of silently becoming a system voice.
    #[cfg(feature = "tts")]
    engine: TtsEngine,
    /// Started on the FIRST utterance, not at construction: Kokoro is ~327 MB
    /// resident and an app that never speaks (text tier, muted, a session
    /// where nobody triggers a reply) must not pay for it.
    #[cfg(feature = "tts")]
    session: OnceLock<TtsHandle>,
    #[cfg(feature = "tts")]
    spawner: ThreadSpawner,
    /// The pump's ends until the session starts; its thread owns them from then on.
    #[cfg(any(feature = "tts", test))]
    pump: Mutex<Option<Pump>>,
    /// The renderer until an audio callback takes it ([`SpeechOutput::renderer`]).
    renderer: Option<SpeechRenderer>,
    shared: Arc<Shared>,
    muted: Arc<AtomicBool>,
    /// Streamed reply text not yet spoken.
    pending: String,
    /// "Shh" latch: swallow the rest of the current reply (see `hush`).
    hushed: bool,
}

impl SpeechOutput {
    /// Create the output with a named voice (e.g. `"bm_fable"`; a trailing
    /// `.mkvoice` is tolerated). Nothing loads until something is said.
    pub fn new(voice: &str, spawner: ThreadSpawner) -> Self {
        #[cfg(not(feature = "tts"))]
        let _ = (voice, spawner);
        let shared = Arc::new(Shared::default());
        let (chunks_in, chunks_out) = ring(CHUNKS);
        let (played_in, played_out) = ring(CHUNKS + 1);
        #[cfg(not(any(feature = "tts", test)))]
        drop((chunks_in, played_out));
        Self {
            #[cfg(feature = "tts")]
            voice: voice.strip_suffix(".mkvoice").unwrap_or(voice).to_string(),
            #[cfg(feature = "tts")]
            engine: TtsEngine::Auto,
            #[cfg(feature = "tts")]
            session: OnceLock::new(),
            #[cfg(feature = "tts")]
            spawner,
            #[cfg(any(feature = "tts", test))]
            pump: Mutex::new(Some(Pump { chunks: chunks_in, played: played_out, shared: shared.clone() })),
            renderer: Some(SpeechRenderer { chunks: chunks_out, played: played_in, shared: shared.clone(), current: None, cursor: 0.0 }),
            shared,
            muted: Arc::new(AtomicBool::new(false)),
            pending: String::new(),
            hushed: false,
        }
    }

    /// Create a Kokoro-only output. If Kokoro is unavailable, the session
    /// reports failure instead of substituting an unrelated system voice.
    pub fn new_kokoro(voice: &str, spawner: ThreadSpawner) -> Self {
        let output = Self::new(voice, spawner);
        #[cfg(feature = "tts")]
        let output = SpeechOutput { engine: TtsEngine::Kokoro, ..output };
        output
    }

    /// The renderer, for an app that mixes speech into its own audio callback: the callback
    /// owns it ([`SpeechRenderer::mix_into`]). There is one; `None` once it was taken.
    pub fn renderer(&mut self) -> Option<SpeechRenderer> {
        self.renderer.take()
    }

    /// The shared mute flag, for audio callbacks that check it directly.
    pub fn muted_flag(&self) -> Arc<AtomicBool> {
        self.muted.clone()
    }

    /// True while synthesized audio is still queued or playing — apps use it
    /// to drop mic transcripts of the assistant's own voice.
    pub fn is_speaking(&self) -> bool {
        self.shared.sent.load(Ordering::Acquire) != self.shared.done.load(Ordering::Acquire)
    }

    /// Convenience for apps with no other audio: install an audio-output
    /// callback that plays speech and nothing else.
    pub fn install_audio_output(&mut self, cx: &mut makepad_widgets_core::Cx, index: usize) {
        use makepad_widgets_core::CxMediaApi;
        let Some(mut renderer) = self.renderer() else {
            log!("speech: its renderer is in an audio callback already; output {index} not installed");
            return;
        };
        let muted = self.muted.clone();
        cx.audio_output(index, move |info, output| {
            output.zero();
            if muted.load(Ordering::Relaxed) {
                renderer.idle();
            } else {
                renderer.mix_into(output, info.sample_rate);
            }
        });
    }

    /// Feed streamed reply text. Each finished sentence is spoken as soon as
    /// it lands, so the voice keeps pace with generation instead of waiting
    /// for it.
    pub fn feed(&mut self, delta: &str) {
        self.pending.push_str(delta);
        // An odd number of fences means we are inside a code block: wait it
        // out rather than reading code aloud a sentence at a time.
        if self.pending.matches("```").count() % 2 == 1 {
            return;
        }
        while let Some(sentence) = self.take_sentence() {
            self.enqueue(&sentence);
        }
    }

    /// Speak whatever is left over at the end of a turn.
    pub fn flush(&mut self) {
        let rest = std::mem::take(&mut self.pending);
        self.enqueue(&rest);
    }

    /// Split off the first complete sentence, if there is one worth speaking.
    fn take_sentence(&mut self) -> Option<String> {
        let mut split_at = None;
        for (index, ch) in self.pending.char_indices() {
            let boundary = matches!(ch, '.' | '!' | '?' | '\n' | ':');
            if boundary && index + ch.len_utf8() >= MIN_SPOKEN_CHARS {
                split_at = Some(index + ch.len_utf8());
                break;
            }
        }
        let at = split_at?;
        let rest = self.pending.split_off(at);
        Some(std::mem::replace(&mut self.pending, rest))
    }

    /// Speak a piece of text right away (subject to mute/hush), e.g. an
    /// acknowledgement while the real reply is still seconds out.
    pub fn enqueue(&self, raw: &str) {
        if self.muted.load(Ordering::Relaxed) || self.hushed {
            return;
        }
        let text = spoken_text(raw);
        if text.is_empty() {
            return;
        }
        #[cfg(feature = "tts")]
        {
            let session = self.session.get_or_init(|| {
                let pump = self.pump.lock().unwrap_or_else(|e| e.into_inner()).take();
                Self::start_session(&self.voice, self.engine, pump.expect("the pump starts once, with the session"), self.spawner.clone())
            });
            session.say(text);
        }
        // Built without `tts`: the synthesis stack is ~10 MB of binary, so a
        // build that can never speak does not link it. Every other path (text,
        // muting, cancellation) behaves exactly as it does with speech on.
        #[cfg(not(feature = "tts"))]
        let _ = text;
    }

    /// Start the hub session and the pump thread that sends its audio to
    /// the renderer. The pump blocks on the session's events, so it costs
    /// nothing while nobody speaks.
    #[cfg(feature = "tts")]
    fn start_session(
        voice: &str,
        engine: TtsEngine,
        mut pump: Pump,
        spawner: ThreadSpawner,
    ) -> TtsHandle {
        let (handle, events) = TtsSession::start(TtsConfig {
            engine,
            voice: Some(voice.to_string()),
            ..TtsConfig::default()
        })
        .split();
        let spawned = spawner.spawn_worker(
            ThreadOptions {
                name: Some("converse-speech-pump".into()),
                ..Default::default()
            },
            move || {
                while let Some(event) = events.recv() {
                    match event {
                        TtsEvent::Loading { .. } => {}
                        TtsEvent::Ready(info) => {
                            log!("tts: {} via {}{}", info.engine, info.pipe, match &info.remote {
                                Some(node) => format!(" on {node}"),
                                None => String::new(),
                            });
                        }
                        TtsEvent::Failed(why) => {
                            log!("tts: no voice available: {why}");
                            return;
                        }
                        // one chunk per sentence, queued behind the ones before it
                        TtsEvent::Audio { audio, .. } => pump.send(Arc::from(audio.samples), audio.sample_rate as f64),
                        // converse does not ask for streaming (TtsConfig::stream is off)
                        TtsEvent::AudioChunk { .. } => {}
                        TtsEvent::Error { message, .. } => log!("tts: {message}"),
                    }
                }
            },
        );
        match spawned {
            Ok(worker) => worker.detach(),
            Err(error) => log!("tts: speech pump unavailable: {error}"),
        }
        handle
    }

    /// Cancel queued and playing speech immediately.
    pub fn stop(&mut self) {
        #[cfg(feature = "tts")]
        if let Some(session) = self.session.get() {
            session.cancel();
        }
        self.pending.clear();
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
    }

    /// "Shh": stop speaking AND stay quiet for the rest of the current reply.
    /// Without the latch, the still-streaming reply would resume at its next
    /// sentence boundary. Cleared when a new prompt starts (`unhush`).
    pub fn hush(&mut self) {
        self.hushed = true;
        self.stop();
    }

    pub fn unhush(&mut self) {
        self.hushed = false;
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
        if muted {
            self.stop();
        }
    }
}

/// Markdown is for reading, not for speaking. Drop code blocks and the symbols
/// that would otherwise be read aloud as punctuation soup.
pub fn spoken_text(markdown: &str) -> String {
    let mut spoken = String::with_capacity(markdown.len());
    let mut inside_code = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            inside_code = !inside_code;
            continue;
        }
        if inside_code {
            continue;
        }
        let cleaned: String = line
            .chars()
            .filter(|c| !matches!(c, '*' | '_' | '`' | '#' | '>' | '|' | '→' | '⚙' | '·'))
            .collect();
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() {
            spoken.push_str(cleaned);
            spoken.push(' ');
        }
    }
    spoken.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::Weak;

    /// This test binary's allocator counts the allocations and frees of a thread that asks, so a
    /// test proves the renderer's blocks touch the heap not at all while other tests allocate.
    mod counting {
        use std::cell::Cell;
        struct Counting;
        std::thread_local! {
            static ON: Cell<bool> = const { Cell::new(false) };
            static CALLS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
        }
        fn count(alloc: bool) {
            if ON.try_with(Cell::get).unwrap_or(false) {
                let _ = CALLS.try_with(|c| {
                    let (a, f) = c.get();
                    c.set(if alloc { (a + 1, f) } else { (a, f + 1) });
                });
            }
        }
        unsafe impl std::alloc::GlobalAlloc for Counting {
            unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
                count(true);
                unsafe { std::alloc::System.alloc(layout) }
            }
            unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
                count(true);
                unsafe { std::alloc::System.alloc_zeroed(layout) }
            }
            unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
                count(true);
                unsafe { std::alloc::System.realloc(ptr, layout, size) }
            }
            unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
                count(false);
                unsafe { std::alloc::System.dealloc(ptr, layout) }
            }
        }
        #[global_allocator]
        static ALLOC: Counting = Counting;

        /// The allocations and frees `f` makes on this thread.
        pub fn heap_calls<R>(f: impl FnOnce() -> R) -> (R, (usize, usize)) {
            CALLS.with(|c| c.set((0, 0)));
            ON.with(|c| c.set(true));
            let r = f();
            ON.with(|c| c.set(false));
            (r, CALLS.with(Cell::get))
        }
    }

    fn output() -> SpeechOutput {
        let cx = makepad_widgets_core::Cx::new(Box::new(|_, _| {}));
        SpeechOutput::new("bm_fable.mkvoice", cx.thread_spawner())
    }

    fn pump(speech: &SpeechOutput) -> Pump {
        speech.pump.lock().unwrap().take().unwrap()
    }

    /// `secs` of a sine at `hz`, sampled at `rate`.
    fn tone(hz: f64, rate: f64, secs: f64) -> Arc<[f32]> {
        (0..(rate * secs) as usize).map(|i| (std::f64::consts::TAU * hz * i as f64 / rate).sin() as f32 * 0.5).collect()
    }

    /// The pitch of `samples` at `rate`, from its rising zero crossings.
    fn pitch(samples: &[f32], rate: f64) -> f64 {
        let ups: Vec<usize> = (1..samples.len()).filter(|&i| samples[i - 1] < 0.0 && samples[i] >= 0.0).collect();
        (ups.len() - 1) as f64 * rate / (ups[ups.len() - 1] - ups[0]) as f64
    }

    /// A mono block of `frames` from the renderer.
    fn block(renderer: &mut SpeechRenderer, frames: usize, rate: f64) -> Vec<f32> {
        let mut out = AudioBuffer::new_with_size(frames, 1);
        renderer.mix_into(&mut out, rate);
        out.channel(0).to_vec()
    }

    /// Constructing the output must NOT start a session or load a model:
    /// Kokoro is ~327 MB resident, and an app that never speaks should not
    /// pay for it. The session starts on the first enqueued utterance.
    #[test]
    fn constructing_speech_output_does_not_start_a_session() {
        let mut speech = output();
        #[cfg(feature = "tts")]
        assert!(speech.session.get().is_none());
        assert!(!speech.is_speaking());
        assert!(speech.renderer().is_some() && speech.renderer().is_none(), "one renderer, taken once");
    }

    /// Sentences play in order, each at its own source rate, and a device rate change in the
    /// middle of one keeps its pitch.
    #[test]
    fn speech_keeps_its_order_and_pitch_across_rates() {
        let mut speech = output();
        let (mut pump, mut renderer) = (pump(&speech), speech.renderer().unwrap());
        pump.send(tone(440.0, 24_000.0, 0.25), 24_000.0);
        pump.send(tone(660.0, 22_050.0, 0.25), 22_050.0);
        assert!(speech.is_speaking());
        let at_44 = block(&mut renderer, 4410, 44_100.0);
        let at_48 = block(&mut renderer, 4800, 48_000.0);
        assert!((pitch(&at_44, 44_100.0) - 440.0).abs() < 2.0, "{}", pitch(&at_44, 44_100.0));
        assert!((pitch(&at_48, 48_000.0) - 440.0).abs() < 2.0, "after the rate change: {}", pitch(&at_48, 48_000.0));
        let mut rest = Vec::new();
        while speech.is_speaking() {
            rest.extend(block(&mut renderer, 480, 48_000.0));
        }
        // the first sentence has 0.05 s (2400 frames) left, then the second plays for 0.25 s
        let second = &rest[2880..7680];
        assert!((pitch(second, 48_000.0) - 660.0).abs() < 3.0, "the second sentence: {}", pitch(second, 48_000.0));
        assert!((14_000..=14_880).contains(&rest.len()), "speaking ends with the second sentence: {}", rest.len());
        pump.drain();
    }

    /// The renderer takes no lock (the module's one lock held by a parked UI does not stop it),
    /// allocates nothing and frees nothing, across sentence starts and ends, a rate change and a
    /// stop: every chunk it lets go of is freed on the pump's thread.
    #[test]
    fn the_callback_renders_past_a_parked_ui_and_touches_no_heap() {
        let mut speech = output();
        let (mut pump, renderer) = (pump(&speech), speech.renderer().unwrap());
        let (steps, device) = (mpsc::channel::<(usize, f64)>(), mpsc::channel::<(Vec<f32>, (usize, usize))>());
        let (step_rx, done_tx) = (steps.1, device.0);
        let device_thread = std::thread::spawn(move || {
            let mut renderer = renderer;
            while let Ok((frames, rate)) = step_rx.recv() {
                let mut out = AudioBuffer::new_with_size(frames, 2);
                let ((), heap) = counting::heap_calls(|| renderer.mix_into(&mut out, rate));
                done_tx.send((out.channel(1).to_vec(), heap)).unwrap();
            }
        });
        let render = |frames: usize, rate: f64| {
            steps.0.send((frames, rate)).unwrap();
            device.1.recv().unwrap()
        };
        let (long, short) = (tone(440.0, 24_000.0, 2.0), tone(440.0, 24_000.0, 0.01));
        let (long_gone, short_gone): (Weak<[f32]>, Weak<[f32]>) = (Arc::downgrade(&long), Arc::downgrade(&short));
        pump.send(short, 24_000.0);
        pump.send(long, 24_000.0);
        {
            let _parked = speech.pump.lock().unwrap();
            let mut heard = Vec::new();
            for i in 0..200 {
                let (samples, heap) = render(256, if i < 100 { 44_100.0 } else { 48_000.0 });
                assert_eq!(heap, (0, 0), "block {i} touched the heap");
                heard.push(samples);
            }
            assert!(heard[10..].iter().all(|b| b.iter().any(|s| *s != 0.0)), "every block played");
            assert!((pitch(&heard[150..].concat(), 48_000.0) - 440.0).abs() < 2.0);
        }
        assert!(short_gone.upgrade().is_some(), "the finished sentence waits on the pump");
        let stopped = tone(330.0, 24_000.0, 1.0);
        let stopped_gone = Arc::downgrade(&stopped);
        pump.send(stopped, 24_000.0);
        speech.stop();
        let (samples, heap) = render(256, 48_000.0);
        assert_eq!(heap, (0, 0), "a stop hands back without a free");
        assert!(samples.iter().all(|s| *s == 0.0) && !speech.is_speaking(), "and stops at once");
        assert!(long_gone.upgrade().is_some() && stopped_gone.upgrade().is_some());
        pump.drain();
        assert!(short_gone.upgrade().is_none() && long_gone.upgrade().is_none() && stopped_gone.upgrade().is_none(), "freed by the pump");
        assert_eq!(speech.shared.retire_lost.load(Ordering::Relaxed), 0);
        drop(steps.0);
        device_thread.join().unwrap();
    }

    /// A device that does not run loses no sentence: the pump fills the ring and waits for room
    /// with the next one, and once blocks run every sentence plays, in order.
    #[test]
    fn a_stopped_device_keeps_every_sentence() {
        let mut speech = output();
        let (mut pump, mut renderer) = (pump(&speech), speech.renderer().unwrap());
        let total = CHUNKS + 6;
        let sender = std::thread::spawn(move || {
            for i in 0..total {
                pump.send(vec![(i + 1) as f32; 64].into(), 48_000.0);
            }
            pump
        });
        while renderer.chunks.len() < CHUNKS {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(speech.is_speaking() && !sender.is_finished(), "the ring is full and the pump waits");
        let mut heard: Vec<f32> = Vec::new();
        while speech.is_speaking() {
            for s in block(&mut renderer, 256, 48_000.0) {
                if s != 0.0 && heard.last() != Some(&s) {
                    heard.push(s);
                }
            }
        }
        let mut pump = sender.join().unwrap();
        assert_eq!(heard, (1..=total).map(|i| i as f32).collect::<Vec<_>>(), "every sentence, in order");
        pump.drain();
        assert_eq!(speech.shared.retire_lost.load(Ordering::Relaxed), 0);
    }

    #[cfg(feature = "tts")]
    #[test]
    fn kokoro_only_output_cannot_fall_back_to_a_system_voice() {
        let cx = makepad_widgets_core::Cx::new(Box::new(|_, _| {}));
        let speech = SpeechOutput::new_kokoro("af_heart", cx.thread_spawner());
        assert_eq!(speech.engine, TtsEngine::Kokoro);
        assert!(speech.session.get().is_none());
    }

    #[test]
    fn spoken_text_drops_code_and_markup() {
        let text = "Here **you** go:\n```rust\nfn x() {}\n```\nDone → ok.";
        // The arrow is dropped, not replaced, so its two surrounding spaces remain.
        assert_eq!(spoken_text(text), "Here you go: Done  ok.");
    }
}
