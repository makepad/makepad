//! The headset's microphone for the desktop app that asks (`XrServerMsg::Mic`): while the app
//! asks, what the wearer says goes up the link as mono 16 kHz PCM (`XrClientMsg::Mic`), in the
//! link's gaps. Off until asked, off again at the off or when the link ends; no sound is taken
//! or sent in between.
//!
//! The sound is the wearer's: it goes to the app that asked and nowhere else (never to a log,
//! never to disk). The logs carry counts and durations only.

use makepad_desk_link::xr::link::XrLink;
use makepad_desk_link::xr::{mic_state, XrClientMsg, MIC_RATE};
use makepad_widgets::makepad_platform::audio::AudioDeviceId;
use makepad_widgets::makepad_platform::permission::{Permission, PermissionResult, PermissionStatus};
use makepad_widgets::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Samples a message carries: 100 ms.
const CHUNK: usize = (MIC_RATE / 10) as usize;
/// A microphone that has delivered no sound this long after it opened says so.
const SILENT_AFTER: Duration = Duration::from_secs(3);

/// Interleaved float frames at any rate as mono 16 kHz samples: the channels averaged, each output
/// sample the mean of the input frames it spans (a box filter: no sound above 8 kHz folds down).
pub struct Downmix {
    rate: u32,
    phase: u32,
    sum: f32,
    count: u32,
}

impl Downmix {
    pub fn new(rate: u32) -> Self {
        Downmix { rate: rate.max(MIC_RATE), phase: 0, sum: 0.0, count: 0 }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn push(&mut self, frames: &[f32], channels: usize, out: &mut Vec<i16>) {
        let channels = channels.max(1);
        for frame in frames.chunks_exact(channels) {
            self.sum += frame.iter().sum::<f32>() / channels as f32;
            self.count += 1;
            self.phase += MIC_RATE;
            if self.phase >= self.rate {
                self.phase -= self.rate;
                let v = self.sum / self.count as f32;
                out.push((v.clamp(-1.0, 1.0) * 32767.0).round() as i16);
                self.sum = 0.0;
                self.count = 0;
            }
        }
    }
}

enum Msg {
    State(u8, String),
    Pcm(Vec<i16>),
    End,
}

struct Tap {
    down: Downmix,
    pending: Vec<i16>,
}

/// The sound on its way up: the audio callback's feed, and the sender that puts it in messages.
pub struct MicUplink {
    on: AtomicBool,
    tap: Mutex<Tap>,
    tx: Sender<Msg>,
    heard: AtomicU64,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl MicUplink {
    /// `send` takes each message (the link's low-priority queue; a test's wire).
    pub fn start(send: impl Fn(XrClientMsg) + Send + 'static) -> Arc<MicUplink> {
        let (tx, rx) = channel();
        let thread = std::thread::Builder::new().name("xr-mic".into()).spawn(move || send_loop(rx, send)).ok();
        Arc::new(MicUplink { on: AtomicBool::new(true), tap: Mutex::new(Tap { down: Downmix::new(MIC_RATE), pending: Vec::with_capacity(CHUNK + CHUNK / 4) }), tx, heard: AtomicU64::new(0), thread: Mutex::new(thread) })
    }

    /// Whether the sender thread runs (it did not start: nothing can be sent).
    pub fn running(&self) -> bool {
        self.thread.lock().unwrap().as_ref().is_some_and(|t| !t.is_finished())
    }

    /// A buffer from the audio callback. Nothing once off.
    pub fn feed(&self, rate: u32, frames: &[f32], channels: usize) {
        if !self.on.load(Ordering::Acquire) {
            return;
        }
        let Ok(mut tap) = self.tap.try_lock() else { return };
        let Tap { down, pending } = &mut *tap;
        if down.rate() != rate.max(MIC_RATE) {
            // (the device's rate, known with its first buffer)
            *down = Downmix::new(rate);
        }
        down.push(frames, channels, pending);
        self.heard.fetch_add((frames.len() / channels.max(1)) as u64, Ordering::AcqRel);
        while pending.len() >= CHUNK {
            let rest = pending.split_off(CHUNK);
            let chunk = std::mem::replace(pending, rest);
            let _ = self.tx.send(Msg::Pcm(chunk));
        }
    }

    pub fn state(&self, state: u8, text: String) {
        let _ = self.tx.send(Msg::State(state, text));
    }

    /// Frames the microphone has delivered.
    pub fn heard(&self) -> u64 {
        self.heard.load(Ordering::Acquire)
    }

    /// Off: the sound still held goes up, then the off, and no more is taken.
    pub fn stop(&self) {
        if !self.on.swap(false, Ordering::AcqRel) {
            return;
        }
        let tail = std::mem::take(&mut self.tap.lock().unwrap().pending);
        if !tail.is_empty() {
            let _ = self.tx.send(Msg::Pcm(tail));
        }
        let _ = self.tx.send(Msg::State(mic_state::OFF, String::new()));
        let _ = self.tx.send(Msg::End);
    }

    /// Waits for the sender to have sent everything (after `stop`).
    pub fn join(&self) {
        if let Some(t) = self.thread.lock().unwrap().take() {
            let _ = t.join();
        }
    }
}

fn send_loop(rx: Receiver<Msg>, send: impl Fn(XrClientMsg)) {
    let (mut seq, mut chunks, mut samples) = (0u32, 0u64, 0u64);
    let started = Instant::now();
    while let Ok(msg) = rx.recv() {
        let (state, text, pcm) = match msg {
            Msg::State(s, text) => (s, text, Vec::new()),
            Msg::Pcm(p) => {
                chunks += 1;
                samples += p.len() as u64;
                (mic_state::LISTENING, String::new(), p.iter().flat_map(|v| v.to_le_bytes()).collect())
            }
            Msg::End => break,
        };
        send(XrClientMsg::Mic { seq, state, text, pcm });
        seq = seq.wrapping_add(1);
    }
    log!("xr mic: off after {:.1} s, {chunks} chunks, {:.1} s of sound", started.elapsed().as_secs_f32(), samples as f32 / MIC_RATE as f32);
}

/// A microphone the desktop app asked for, running while it asks and the link lasts.
pub(crate) struct HeadsetMic {
    link: Arc<XrLink>,
    uplink: Arc<MicUplink>,
    permission: i32,
    begun: Option<Instant>,
    told_silent: bool,
}

impl HeadsetMic {
    fn start(cx: &mut Cx, link: Arc<XrLink>) -> HeadsetMic {
        log!("xr mic: on");
        let sender = link.clone();
        let uplink = MicUplink::start(move |m| sender.send_bulk(&m));
        uplink.state(mic_state::STARTING, "waiting for the headset's permission".into());
        // (asked once: granted already, it is answered at once)
        let permission = cx.request_permission(Permission::AudioInput);
        HeadsetMic { link, uplink, permission, begun: None, told_silent: false }
    }

    fn begin(&mut self, cx: &mut Cx, input: AudioDeviceId) {
        let uplink = self.uplink.clone();
        cx.audio_input(0, move |info, buffer| uplink.feed(info.sample_rate as u32, &buffer.data, buffer.channel_count));
        cx.use_audio_inputs(&[input]);
        self.begun = Some(Instant::now());
        self.uplink.state(mic_state::STARTING, "opening the microphone".into());
    }

    pub(crate) fn permission(&mut self, result: &PermissionResult, granted: &mut bool) {
        if result.permission != Permission::AudioInput || result.request_id != self.permission {
            return;
        }
        match result.status {
            PermissionStatus::Granted => *granted = true,
            _ => self.uplink.state(mic_state::FAILED, "the headset's microphone permission was denied".into()),
        }
    }

    fn stop(self, cx: &mut Cx) {
        cx.use_audio_inputs(&[]);
        // (a link that is gone drops what this sends)
        self.uplink.stop();
    }

    /// Runs the microphone as `ask` says on `link` (from the client's housekeeping). `granted`:
    /// the wearer allowed it; `input`: the headset's microphone, once listed.
    pub(crate) fn drive(mic: &mut Option<HeadsetMic>, granted: &mut bool, cx: &mut Cx, link: Option<&Arc<XrLink>>, ask: bool, input: Option<AudioDeviceId>) {
        let same = |m: &HeadsetMic| ask && link.is_some_and(|l| Arc::ptr_eq(l, &m.link));
        if mic.as_ref().is_some_and(|m| !same(m)) {
            mic.take().unwrap().stop(cx);
            *granted = false;
        }
        match (mic.as_mut(), link, ask) {
            (None, Some(link), true) => *mic = Some(HeadsetMic::start(cx, link.clone())),
            (Some(m), _, true) => {
                if !m.uplink.running() {
                    return;
                }
                match (m.begun, *granted, input) {
                    (None, true, Some(input)) => m.begin(cx, input),
                    (None, true, None) => m.uplink.state(mic_state::STARTING, "waiting for the headset's microphone".into()),
                    (Some(at), _, _) if m.uplink.heard() == 0 && !m.told_silent && at.elapsed() > SILENT_AFTER => {
                        m.told_silent = true;
                        m.uplink.state(mic_state::FAILED, "the microphone delivers no sound".into());
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_desk_link::xr::XrClientMsg;

    /// A tone of `hz` at `rate`, `secs` long, as one interleaved stereo buffer.
    fn tone(hz: f32, rate: u32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize).flat_map(|i| { let v = 0.5 * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin(); [v, v] }).collect()
    }

    fn pcm_of(msgs: &[XrClientMsg]) -> Vec<i16> {
        msgs.iter().filter_map(|m| if let XrClientMsg::Mic { pcm, .. } = m { Some(pcm) } else { None }).flat_map(|p| p.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]))).collect()
    }

    #[test]
    fn downmix_keeps_a_tone_at_16k_mono() {
        for rate in [48_000u32, 44_100, 16_000] {
            let mut out = Vec::new();
            Downmix::new(rate).push(&tone(440.0, rate, 1.0), 2, &mut out);
            assert!((out.len() as i32 - 16_000).abs() <= 1, "{rate} Hz: {} samples a second", out.len());
            // the tone's period in output samples: rising zero crossings, one per cycle
            let crossings = out.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count();
            assert!((crossings as i32 - 440).abs() <= 1, "{rate} Hz: {crossings} cycles");
            let peak = out.iter().map(|v| v.abs()).max().unwrap();
            assert!((peak as f32 / 32767.0 - 0.5).abs() < 0.06, "{rate} Hz: peak {peak}");
        }
    }

    #[test]
    fn mic_goes_up_the_wire_only_while_on_and_arrives_whole() {
        use crate::server::{MicEvent, ServerShared};
        // the wire: every message through the link's framing (encode, decode) and the server's
        // gate, as the session does, into the app's sink
        let shared = Arc::new(ServerShared::new(Arc::new(|_: &str| {})));
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        *shared.mic_sink.lock().unwrap() = Some(Arc::new(move |_, e: MicEvent| sink.lock().unwrap().push(e)));
        *shared.mic_ask.lock().unwrap() = Some((5, true));
        let server = shared.clone();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let seen = sent.clone();
        let uplink = MicUplink::start(move |m| {
            let mut inbox = makepad_desk_link::Inbox::default();
            inbox.feed(&m.encode());
            let (ty, payload) = inbox.next().expect("a whole frame").expect("a frame");
            let decoded = XrClientMsg::decode(ty, &payload).expect("a mic message decodes");
            seen.lock().unwrap().push(decoded.clone());
            if let XrClientMsg::Mic { state, text, pcm, .. } = decoded {
                if pcm.is_empty() {
                    server.mic_event(5, MicEvent::State { state, text });
                } else {
                    server.mic_event(5, MicEvent::Pcm(pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()));
                }
            }
        });
        let sound = tone(300.0, 48_000, 0.75);
        // some in odd-sized buffers, as the audio device gives them
        for part in sound.chunks(2 * 480 * 3 + 2 * 7) {
            uplink.feed(48_000, part, 2);
        }
        // the app asks it to stop: the tail and the off still arrive
        *shared.mic_ask.lock().unwrap() = Some((5, false));
        uplink.stop();
        // after the off, nothing is taken
        uplink.feed(48_000, &tone(300.0, 48_000, 0.5), 2);
        uplink.join();
        let mut expected = Vec::new();
        Downmix::new(48_000).push(&sound, 2, &mut expected);
        let arrived: Vec<i16> = got.lock().unwrap().iter().filter_map(|e| if let MicEvent::Pcm(p) = e { Some(p.clone()) } else { None }).flatten().collect();
        assert_eq!(arrived, expected, "the app's input is the downmix of what was fed, tail included");
        assert!(matches!(got.lock().unwrap().last(), Some(MicEvent::State { state: mic_state::OFF, .. })), "the last word is the off");
        assert!(shared.mic_ask.lock().unwrap().is_none(), "the off closes the gate");
        let msgs = sent.lock().unwrap().clone();
        assert!(msgs.windows(2).all(|w| match (&w[0], &w[1]) { (XrClientMsg::Mic { seq: a, .. }, XrClientMsg::Mic { seq: b, .. }) => b.wrapping_sub(*a) == 1, _ => false }), "in order, none missing");
        assert_eq!(pcm_of(&msgs), expected);
    }
}
