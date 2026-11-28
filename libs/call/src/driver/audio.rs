//! Device callbacks stay installed while the UI changes calls or rings the speaker.

use super::voice::{VoiceIn, VoiceOut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

// Each block borrows one half. Parking it again at the end lets the UI reclaim it even if
// the device stops at that very block. The single callback takes before it puts back.
struct Mailbox<T> {
    parked: Receiver<Box<T>>,
    next: Option<Option<Box<T>>>, // no command, detach, or attach
    retired: Option<Box<T>>,
    borrowed: bool, // a half is running or parked at the end of its block
}

struct Port<T> {
    mailbox: Arc<Mutex<Mailbox<T>>>,
    park: SyncSender<Box<T>>,
}

struct Control<T> {
    mailbox: Arc<Mutex<Mailbox<T>>>,
    pending: Option<Option<Box<T>>>,
    collecting: bool,
}

fn port<T>() -> (Port<T>, Control<T>) {
    let (park, parked) = sync_channel(1);
    let mailbox = Arc::new(Mutex::new(Mailbox { parked, next: None, retired: None, borrowed: false }));
    // Initialize the platform mutex off the callback thread.
    drop(mailbox.lock());
    (Port { mailbox: mailbox.clone(), park }, Control { mailbox, pending: None, collecting: false })
}

impl<T> Port<T> {
    fn with(&mut self, f: impl FnOnce(&mut T)) {
        let half = {
            let Ok(mut slot) = self.mailbox.try_lock() else { return };
            let old = slot.parked.try_recv().ok();
            let half = if let Some(next) = slot.next.take() {
                // Control collected any earlier retirement before it posted this command.
                slot.retired = old;
                next
            } else {
                old
            };
            slot.borrowed = half.is_some();
            half
        };
        if let Some(mut value) = half {
            f(&mut value);
            // This is the only producer, and the receiver lives in this port's mailbox.
            self.park.try_send(value).unwrap_or_else(|_| unreachable!("a borrowed audio half has a parking place"));
        }
    }
}

impl<T> Control<T> {
    fn set(&mut self, next: Option<T>) {
        self.pending = Some(next.map(Box::new));
        self.tick();
    }

    fn tick(&mut self) {
        if self.pending.is_none() && !self.collecting { return }
        let Ok(mut slot) = self.mailbox.try_lock() else { return };
        let retired = slot.retired.take();
        let superseded = self.pending.take().and_then(|next| slot.next.replace(next));
        // While a change is pending, a parked half belongs to the old call. A running block
        // can park it later, so keep checking until either side has collected it.
        let old = if slot.next.is_some() { slot.parked.try_recv().ok() } else { None };
        if old.is_some() { slot.borrowed = false; }
        self.collecting = slot.next.is_some() && slot.borrowed;
        drop(slot);
        drop((retired, superseded, old));
    }
}

/// The permanent input callback. Device formats can change between blocks.
pub struct InputPort(Port<VoiceIn>);

impl InputPort {
    pub fn push_mono(&mut self, rate: f64, samples: &[f32]) {
        self.0.with(|input| input.push_mono(rate, samples));
    }

    pub fn push_planar(&mut self, rate: f64, frames: usize, channels: usize, data: &[f32]) {
        self.0.with(|input| input.push_planar(rate, frames, channels, data));
    }

    pub fn push_interleaved(&mut self, rate: f64, channels: usize, data: &[f32]) {
        self.0.with(|input| input.push_interleaved(rate, channels, data));
    }
}

/// The permanent speaker callback. Fills each block with the call mix and the incoming ring.
pub struct OutputPort {
    port: Port<VoiceOut>,
    // UI and output use the same monotonic origin. Zero means the ring is off.
    clock: Instant,
    ring_start: Arc<AtomicU64>,
    last_start: u64,
    next_sample: f64,
}

impl OutputPort {
    /// The newest frame received from that source, for diagnostics.
    pub fn received_seq(&mut self, peer: super::PeerId, stream: u8) -> Option<u32> {
        let mut seq = None;
        self.port.with(|out| seq = out.received_seq(peer, stream));
        seq
    }

    fn ring_block(&mut self, rate: f64, frames: usize, mut add: impl FnMut(usize, f32)) {
        let start = self.ring_start.load(Ordering::Relaxed);
        if start == 0 || !rate.is_finite() || rate <= 0.0 { return }
        let now = self.clock.elapsed().as_micros() as u64;
        let elapsed = now.saturating_sub(start - 1) as f64 / 1e6;
        if self.last_start != start {
            self.last_start = start;
            self.next_sample = elapsed;
        }
        // One phase clock for both tones. Follow the device's samples across contiguous
        // blocks, and catch up to the ring cadence if the device stopped or opened late.
        let begin = self.next_sample.max(elapsed);
        let dt = 1.0 / rate;
        self.next_sample = begin + frames as f64 * dt;
        let mut phase = begin % 6.0;
        if phase >= 2.0 && phase + frames as f64 * dt < 6.0 { return }
        for i in 0..frames {
            if phase < 2.0 {
                let angle = std::f64::consts::TAU * phase;
                add(i, (0.12 * ((440.0 * angle).sin() + (480.0 * angle).sin())) as f32);
            }
            phase += dt;
            if phase >= 6.0 { phase -= 6.0; }
        }
    }

    /// Channel-major output, as in the platform's audio buffer.
    pub fn fill_planar(&mut self, rate: f64, frames: usize, channels: usize, data: &mut [f32]) {
        data.fill(0.0);
        if channels == 0 || frames > data.len() / channels { return }
        self.port.with(|out| out.mix_into_planar(rate, frames, channels, data));
        self.ring_block(rate, frames, |i, x| {
            for c in 0..channels { data[c * frames + i] += x; }
        });
    }

    pub fn fill_interleaved(&mut self, rate: f64, channels: usize, data: &mut [f32]) {
        data.fill(0.0);
        if channels == 0 { return }
        self.port.with(|out| out.mix_into_interleaved(rate, channels, data));
        self.ring_block(rate, data.len() / channels, |i, x| {
            for s in &mut data[i * channels..(i + 1) * channels] { *s += x; }
        });
    }
}

/// The UI's controls for the installed input and output callbacks.
pub struct PortControl {
    input: Control<VoiceIn>,
    output: Control<VoiceOut>,
    clock: Instant,
    ring_start: Arc<AtomicU64>,
}

impl PortControl {
    pub fn attach(&mut self, input: Option<VoiceIn>, output: VoiceOut) {
        self.input.set(input);
        self.output.set(Some(output));
    }

    pub fn detach(&mut self) {
        self.input.set(None);
        self.output.set(None);
    }

    /// Start the two-second ring and four-second pause, or silence it.
    pub fn ring(&self, on: bool) {
        if on {
            let start = self.clock.elapsed().as_micros() as u64 + 1;
            let _ = self.ring_start.compare_exchange(0, start, Ordering::Relaxed, Ordering::Relaxed);
        } else {
            self.ring_start.store(0, Ordering::Relaxed);
        }
    }

    /// Retry pending handoffs and free replaced halves on the UI thread.
    pub fn tick(&mut self) {
        self.input.tick();
        self.output.tick();
    }
}

/// Create the device ports once, independently of the calls that will use them.
pub fn audio_ports() -> (InputPort, OutputPort, PortControl) {
    let (input, input_control) = port();
    let (output, output_control) = port();
    let clock = Instant::now();
    let ring_start = Arc::new(AtomicU64::new(0));
    (InputPort(input), OutputPort { port: output, clock, ring_start: ring_start.clone(), last_start: 0, next_sample: 0.0 },
        PortControl { input: input_control, output: output_control, clock, ring_start })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_half_detached_during_its_last_block_returns_on_the_ui_tick() {
        let (mut port, mut ui) = port();
        let owner = Arc::new(());
        ui.set(Some(owner.clone()));
        port.with(|_| {
            ui.set(None);
            assert_eq!(Arc::strong_count(&owner), 2, "the running block must keep its half");
        });
        ui.tick();
        assert_eq!(Arc::strong_count(&owner), 1, "the UI must reclaim the half without another device block");
    }

    #[test]
    fn the_ring_follows_the_ui_start_and_repeats_after_six_seconds() {
        let (_, mut out, ui) = audio_ports();
        ui.ring(true);
        // The speaker opened three seconds after the UI started ringing, during the pause.
        // Move the simulated speaker clock forward to exercise that delay without waiting.
        out.clock -= std::time::Duration::from_secs(3);
        let mut block = vec![1.0; 4 * 8000];
        out.fill_interleaved(8000.0, 1, &mut block);
        assert!(block[800..2 * 8000].iter().all(|&x| x == 0.0), "a late speaker must start inside the four-second pause");
        let rms = (block[3 * 8000 + 800..].iter().map(|x| x * x).sum::<f32>() / 7200.0).sqrt();
        assert!((0.11..0.13).contains(&rms), "ring should resume six seconds after the UI start, measured RMS {rms}");
        ui.ring(false);
        out.fill_interleaved(8000.0, 1, &mut block);
        assert!(block.iter().all(|&x| x == 0.0), "ring off must silence the next block");
    }
}
