//! Desk Link XR's game audio (`crate::xr::audio_datagram`): the app's own mixer output, live
//! to the headset, lined up with the pictures.
//!
//! Desktop: the app's audio callback hands each block it made to [`tap`] (after its mix, before
//! the device plays it; a few atomics and a copy: realtime-safe). While a headset streams, the
//! session's sender thread ([`AudioOut`]) takes the frames from that ring every millisecond,
//! brings them to 48 kHz stereo (the teamtalk resampler; most devices already are), cuts 5 ms
//! datagrams, each with the desktop-clock time its first frame was due (when the mixer made
//! it), and sends them from the session's pose socket to the address the poses come from. The
//! tap can mute the desktop's own output meanwhile (`MAKEPAD_XR_REMOTE_MUTE_LOCAL`, default on;
//! `mutelocal=` in the live tuning file).
//!
//! Headset: [`XrAudioRx`] takes the datagrams on its own thread into teamtalk's reorder ring;
//! [`XrAudioPlayer`] (moved into the app's audio output callback, AAudio low latency on the
//! Quest) plays them through teamtalk's playout: a small adaptive buffer, a lost or late
//! datagram concealed (the last one repeated, faded out, the next faded in), clock drift taken
//! up by a playback-rate nudge of at most 0.5%, so the buffer neither drains nor grows. The
//! buffer's least size is set so that the sound is heard as late as the pictures are seen
//! (lips and impacts together): the pictures' own latency less the link and the device.
//!
//! The test signal (`MAKEPAD_XR_REMOTE_AUDIO_TEST=1`, or `audiotest=1` in the tuning file)
//! replaces what streams by a 440 Hz tone with a click due at every 500 ms of the desktop's
//! clock: the headset measures each click's latency exactly (its clock offset from the link's
//! ping) and counts the tone's discontinuities.

use crate::xr::audio_datagram as wire;
use makepad_hal::clock_ns;
use makepad_teamtalk::jitter::{JitterRing, Playout, PlayoutConfig};
use makepad_teamtalk::resample::Resampler;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

// ---- desktop: the tap ----------------------------------------------------------------------

/// The stream on (`XrServer::set_streaming`, makepad-xr-stream): off, nothing is sent and the desktop plays its own
/// sound again.
static STREAM_ON: AtomicBool = AtomicBool::new(true);
pub fn set_streaming(on: bool) {
    STREAM_ON.store(on, Ordering::Release);
}

/// Frames the tap's ring holds (341 ms at 48 kHz): the sender drains it every millisecond.
const RING: usize = 1 << 14;
/// Per block the tap took: its first frame's index and when it was due (the clock at the tap).
const MARKS: usize = 64;

/// The tap's state, shared by the app's audio callback (it holds an [`XrAudioTap`]) and the
/// session's sender. On the heap, never in statics: the audio callback may run where the
/// app's statics are not the ones it sees (a JIT's realtime code).
struct Tap {
    /// A frame as it waits: left i16 in the low half, right in the high.
    frames: Box<[AtomicU32]>,
    written: AtomicU64,
    read: AtomicU64,
    /// A headset streams: the tap writes.
    on: AtomicBool,
    /// The sender knows where to send (the poses came): the local output may be muted.
    peer: AtomicBool,
    mute_local: AtomicBool,
    test: AtomicBool,
    test_phase: AtomicU32,
    rate: AtomicU32,
    overflow: AtomicU64,
    mark_at: Box<[AtomicU64]>,
    mark_ns: Box<[AtomicU64]>,
    mark_n: AtomicU64,
    /// The steady timeline's start (f64 ns bits): the tap's alone.
    base_ns: AtomicU64,
    /// Blocks the app's callback handed the tap (headset or not): whether its audio runs at all.
    tapped: AtomicU64,
}

fn shared_tap() -> &'static Arc<Tap> {
    static TAP: std::sync::OnceLock<Arc<Tap>> = std::sync::OnceLock::new();
    TAP.get_or_init(|| {
        Arc::new(Tap {
            frames: (0..RING).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            read: AtomicU64::new(0),
            on: AtomicBool::new(false),
            peer: AtomicBool::new(false),
            mute_local: AtomicBool::new(true),
            test: AtomicBool::new(false),
            test_phase: AtomicU32::new(0),
            rate: AtomicU32::new(48_000),
            overflow: AtomicU64::new(0),
            mark_at: (0..MARKS).map(|_| AtomicU64::new(0)).collect(),
            mark_ns: (0..MARKS).map(|_| AtomicU64::new(0)).collect(),
            mark_n: AtomicU64::new(0),
            tapped: AtomicU64::new(0),
            base_ns: AtomicU64::new(0),
        })
    })
}

/// The app's end of the tap: made on the app's thread ([`XrAudioTap::new`]) and moved into its
/// audio output callback, where [`XrAudioTap::tap`] takes each block.
#[derive(Clone)]
pub struct XrAudioTap(Arc<Tap>);

impl Default for XrAudioTap {
    fn default() -> Self {
        Self::new()
    }
}

/// The clicks of the test signal: one due at every multiple of this on the desktop's clock.
pub const CLICK_PERIOD_NS: u64 = 500_000_000;
const CLICK_NS: u64 = 2_000_000;

impl XrAudioTap {
    pub fn new() -> XrAudioTap {
        XrAudioTap(shared_tap().clone())
    }

    /// The app's audio callback, after its mix: one block of `frames` at `rate` (planar; a
    /// mono app passes the same channel twice). True: the app silences its own output (a
    /// headset hears it). Realtime-safe: atomics and a copy, no lock, no allocation.
    pub fn tap(&self, rate: f64, frames: usize, left: &[f32], right: &[f32]) -> bool {
    let t = &*self.0;
    t.tapped.fetch_add(1, Ordering::Relaxed);
    if !t.on.load(Ordering::Acquire) {
        return false;
    }
    if !STREAM_ON.load(Ordering::Acquire) {
        return false;
    }
    let mute = t.peer.load(Ordering::Acquire) && t.mute_local.load(Ordering::Relaxed);
    let frames = frames.min(left.len()).min(right.len());
    let now = clock_ns();
    let w = t.written.load(Ordering::Relaxed);
    let r = t.read.load(Ordering::Acquire);
    if (w - r) as usize + frames > RING {
        // the sender is behind (it never is for long): this block is not sent
        t.overflow.fetch_add(1, Ordering::Relaxed);
        return mute;
    }
    t.rate.store(rate.max(1.0) as u32, Ordering::Relaxed);
    // the block's due time on a steady clock: the device takes blocks in bursts (a 1024-frame
    // quantum: callbacks up to ~20 ms off their place), so the timeline is the frame count at
    // the rate from the earliest a block was ever made (creeping 1 µs a block after the
    // clocks' drift; a jump of 50 ms either way starts it again)
    let ns_per = 1e9 / rate.max(1.0);
    let made = now as f64 - w as f64 * ns_per;
    let base = f64::from_bits(t.base_ns.load(Ordering::Relaxed));
    let base = if base == 0.0 || (made - base).abs() > 50e6 { made } else { (base + 1e3).min(made) };
    t.base_ns.store(base.to_bits(), Ordering::Relaxed);
    let due = (base + w as f64 * ns_per) as u64;
    let n = t.mark_n.load(Ordering::Relaxed);
    t.mark_at[n as usize % MARKS].store(w, Ordering::Relaxed);
    t.mark_ns[n as usize % MARKS].store(due, Ordering::Relaxed);
    t.mark_n.store(n + 1, Ordering::Release);
    let q = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0) as i16 as u16 as u32;
    if t.test.load(Ordering::Relaxed) {
        let step = 440.0 / rate.max(1.0) as f32;
        let mut phase = f32::from_bits(t.test_phase.load(Ordering::Relaxed));
        for i in 0..frames {
            let at = due + (i as f64 * ns_per) as u64;
            let v = if at % CLICK_PERIOD_NS < CLICK_NS {
                if (at / 166_667) % 2 == 0 { 0.8 } else { -0.8 }
            } else {
                (phase * std::f32::consts::TAU).sin() * 0.2
            };
            phase = (phase + step).fract();
            t.frames[(w as usize + i) % RING].store(q(v) | q(v) << 16, Ordering::Relaxed);
        }
        t.test_phase.store(phase.to_bits(), Ordering::Relaxed);
    } else {
        for i in 0..frames {
            t.frames[(w as usize + i) % RING].store(q(left[i]) | q(right[i]) << 16, Ordering::Relaxed);
        }
    }
    t.written.store(w + frames as u64, Ordering::Release);
    mute
    }
}

/// The session's audio: a thread that sends the tap's frames to the headset (see the module).
pub struct AudioOut {
    stop: Arc<AtomicBool>,
}

static LINE: Mutex<String> = Mutex::new(String::new());

/// The sender's last stats window, one line (for a stats panel).
pub fn server_line() -> String {
    LINE.lock().unwrap().clone()
}

/// What the live tuning file says of the audio (`mutelocal=0|1`, `audiotest=0|1`).
fn tuned(mute_default: bool, test_default: bool) -> (bool, bool) {
    let (mut mute, mut test) = (mute_default, test_default);
    if let Ok(text) = std::fs::read_to_string(std::env::temp_dir().join("xr-remote-tune")) {
        for line in text.lines() {
            match line.trim().split_once('=') {
                Some(("mutelocal", v)) => mute = v.trim() != "0",
                Some(("audiotest", v)) => test = v.trim() != "0",
                _ => {}
            }
        }
    }
    (mute, test)
}

impl AudioOut {
    /// Starts sending from `socket` (the session's pose socket) to `peer` (the poses' source,
    /// once known), each datagram sealed with `key`.
    pub fn start(socket: UdpSocket, key: [u8; 32], peer: Arc<Mutex<Option<SocketAddr>>>, log: Log) -> Result<AudioOut, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let mute_default = std::env::var("MAKEPAD_XR_REMOTE_MUTE_LOCAL").map_or(true, |v| v != "0");
        let test_default = std::env::var("MAKEPAD_XR_REMOTE_AUDIO_TEST").is_ok_and(|v| v == "1");
        let t = shared_tap().clone();
        t.read.store(t.written.load(Ordering::Acquire), Ordering::Release);
        t.on.store(true, Ordering::Release);
        std::thread::Builder::new()
            .name("xr-host-audio".into())
            .spawn(move || {
                let mut seq = 0u32;
                let mut rs = [Resampler::new(), Resampler::new()];
                let mut pending: Vec<i16> = Vec::with_capacity(wire::FRAMES * 4);
                let mut pending_due = 0u64;
                let mut datagram = Vec::with_capacity(1100);
                // the tap's block marks not passed yet: (first frame, due ns), oldest first
                let mut mark_n = t.mark_n.load(Ordering::Acquire);
                let mut marks: std::collections::VecDeque<(u64, u64)> = Default::default();
                let (mut window, mut sent, mut frames_in, mut overflow0, mut tune_at) = (Instant::now(), 0u32, 0u64, t.overflow.load(Ordering::Relaxed), Instant::now() - Duration::from_secs(2));
                let (mut muted_line, mut test_line) = (mute_default, test_default);
                let mut last_rate = 0u32;
                while !stop2.load(Ordering::Acquire) {
                    if tune_at.elapsed() >= Duration::from_secs(1) {
                        tune_at = Instant::now();
                        let (mute, test) = tuned(mute_default, test_default);
                        if mute != muted_line || test != test_line {
                            log(&format!("xr host: audio: local output {}, test signal {}", if mute { "muted while a headset hears it" } else { "kept" }, if test { "on" } else { "off" }));
                        }
                        (muted_line, test_line) = (mute, test);
                        t.mute_local.store(mute, Ordering::Relaxed);
                        t.test.store(test, Ordering::Relaxed);
                    }
                    let to = *peer.lock().unwrap();
                    t.peer.store(to.is_some(), Ordering::Release);
                    let w = t.written.load(Ordering::Acquire);
                    let mut r = t.read.load(Ordering::Acquire);
                    let rate = t.rate.load(Ordering::Relaxed).max(1);
                    if rate != last_rate {
                        last_rate = rate;
                        for x in rs.iter_mut() {
                            x.set_ratio(rate as f64, wire::RATE as f64, 0.0);
                        }
                    }
                    let flags = if test_line { wire::TEST } else { 0 };
                    // (a block's mark is published before its frames)
                    let n_now = t.mark_n.load(Ordering::Acquire);
                    while mark_n < n_now {
                        let i = mark_n as usize % MARKS;
                        marks.push_back((t.mark_at[i].load(Ordering::Relaxed), t.mark_ns[i].load(Ordering::Relaxed)));
                        mark_n += 1;
                    }
                    while r < w {
                        // the block this frame came in
                        while marks.len() >= 2 && marks[1].0 <= r {
                            marks.pop_front();
                        }
                        let mark = marks.front().copied().unwrap_or((r, clock_ns()));
                        let v = t.frames[r as usize % RING].load(Ordering::Relaxed);
                        let due = mark.1 + (r.saturating_sub(mark.0) as f64 * 1e9 / rate as f64) as u64;
                        let (l, rr) = ((v & 0xffff) as u16 as i16 as f32, (v >> 16) as u16 as i16 as f32);
                        r += 1;
                        frames_in += 1;
                        if pending.is_empty() {
                            pending_due = due;
                        }
                        if rate == wire::RATE {
                            pending.push(l as i16);
                            pending.push(rr as i16);
                        } else {
                            let mut right = [0.0f32; 8];
                            let mut k = 0;
                            rs[1].push(rr, |y| {
                                if k < 8 {
                                    right[k] = y;
                                }
                                k += 1;
                            });
                            let mut j = 0;
                            rs[0].push(l, |y| {
                                pending.push(y.clamp(-32768.0, 32767.0) as i16);
                                pending.push(right[j.min(7)].clamp(-32768.0, 32767.0) as i16);
                                j += 1;
                            });
                        }
                        if pending.len() >= wire::FRAMES * wire::CHANNELS {
                            if let (Some(to), true) = (to, STREAM_ON.load(Ordering::Acquire)) {
                                wire::seal(&key, seq, wire::CHANNELS, flags, pending_due, &pending[..wire::FRAMES * wire::CHANNELS], &mut datagram);
                                let _ = socket.send_to(&datagram, to);
                                sent += 1;
                            }
                            seq = seq.wrapping_add(1);
                            pending.drain(..wire::FRAMES * wire::CHANNELS);
                            pending_due = due;
                        }
                    }
                    t.read.store(r, Ordering::Release);
                    if window.elapsed() >= Duration::from_secs(5) {
                        let secs = window.elapsed().as_secs_f64();
                        let overflow = t.overflow.load(Ordering::Relaxed);
                        let tapped = t.tapped.swap(0, Ordering::Relaxed);
                        let line = if frames_in == 0 {
                            format!("audio: none from the app ({} blocks at its tap: {})", tapped, if tapped == 0 { "its audio device does not run, or it does not tap its output" } else { "?" })
                        } else {
                            format!("audio {} Hz -> 48 kHz stereo · {:.0} datagrams/s · local {}{} · {} lost at the tap",
                                rate, sent as f64 / secs, if muted_line { "muted" } else { "playing" }, if test_line { " · TEST SIGNAL" } else { "" }, overflow - overflow0)
                        };
                        log(&format!("xr host: {line}"));
                        *LINE.lock().unwrap() = line;
                        (window, sent, frames_in, overflow0) = (Instant::now(), 0, 0, overflow);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(AudioOut { stop })
    }

    pub fn stop(&self) {
        let t = shared_tap();
        self.stop.store(true, Ordering::Release);
        t.on.store(false, Ordering::Release);
        t.peer.store(false, Ordering::Release);
        LINE.lock().unwrap().clear();
    }
}

// ---- headset: receive and play ---------------------------------------------------------------

/// Datagram frames as the ring holds them: 5 ms, interleaved stereo.
const FRAME_MS: f32 = wire::FRAMES as f32 * 1000.0 / wire::RATE as f32;
/// The buffer's least size without a picture latency to aim at (frames of 5 ms).
const DEFAULT_MIN_FRAMES: u32 = 3;
/// The most it is made to add for the pictures' sake (frames: 120 ms).
const MAX_FRAMES: u32 = 24;

/// The headset's receive side, shared by its receive thread, the link's worker (clock offset),
/// the audio callback ([`XrAudioPlayer`]) and the stats.
pub struct XrAudioRx {
    ring: JitterRing,
    /// Bumped when a new session starts: the player starts over.
    generation: AtomicU32,
    /// The desktop's clock less this one's (ns), from the ping with the shortest round trip
    /// of the last few; `offset_ok` once there is one.
    offset_ns: AtomicI64,
    offset_ok: AtomicBool,
    pings: Mutex<std::collections::VecDeque<(u64, i64)>>,
    test: AtomicBool,
    // the receive thread's
    received: AtomicU32,
    lost: AtomicU32,
    /// Due on the desktop to arrived here (µs, smoothed; negative: ahead of its time).
    net_us: AtomicI32,
    // the player's
    buffered_us: AtomicU32,
    target_frames: AtomicU32,
    device_us: AtomicU32,
    underruns: AtomicU32,
    concealed: AtomicU32,
    nudge_ppm: AtomicI32,
    click_us: AtomicU32,
    glitches: AtomicU32,
    /// The pictures' own latency (µs: a frame's content made on the desktop to shown), the
    /// app's ([`XrAudioRx::set_video_latency`]).
    video_us: AtomicU32,
    stop: AtomicBool,
    /// The last report's totals (lost, late, underruns, concealed).
    reported: Mutex<[u32; 4]>,
}

impl XrAudioRx {
    pub fn new() -> Arc<XrAudioRx> {
        Arc::new(XrAudioRx {
            ring: JitterRing::new(),
            generation: AtomicU32::new(0),
            offset_ns: AtomicI64::new(0),
            offset_ok: AtomicBool::new(false),
            pings: Mutex::new(Default::default()),
            test: AtomicBool::new(false),
            received: AtomicU32::new(0),
            lost: AtomicU32::new(0),
            net_us: AtomicI32::new(0),
            buffered_us: AtomicU32::new(0),
            target_frames: AtomicU32::new(0),
            device_us: AtomicU32::new(0),
            underruns: AtomicU32::new(0),
            concealed: AtomicU32::new(0),
            nudge_ppm: AtomicI32::new(0),
            click_us: AtomicU32::new(0),
            glitches: AtomicU32::new(0),
            video_us: AtomicU32::new(0),
            stop: AtomicBool::new(false),
            reported: Mutex::new([0; 4]),
        })
    }

    /// A session began: datagrams arrive on `socket` (the pose socket, connected to the
    /// server), sealed with `key`. Ends with the next `start` or [`XrAudioRx::stop`].
    pub fn start(self: &Arc<Self>, socket: &UdpSocket, key: Vec<u8>) -> Result<(), String> {
        let socket = socket.try_clone().map_err(|e| format!("audio socket: {e}"))?;
        socket.set_read_timeout(Some(Duration::from_millis(100))).map_err(|e| e.to_string())?;
        self.ring.reset();
        self.offset_ok.store(false, Ordering::Release);
        self.pings.lock().unwrap().clear();
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let rx = self.clone();
        std::thread::Builder::new()
            .name("xr-audio-rx".into())
            .spawn(move || {
                let mut buf = vec![0u8; 4096];
                let mut samples = [0i16; makepad_teamtalk::MAX_FRAME];
                let mut expected: Option<u32> = None;
                while !rx.stop.load(Ordering::Acquire) && rx.generation.load(Ordering::Acquire) == generation {
                    let Ok(n) = socket.recv(&mut buf) else { continue };
                    let arrived = clock_ns();
                    let Some(a) = wire::open(&key, &buf[..n]) else { continue };
                    let count = (a.frames * a.channels).min(samples.len());
                    if a.channels != wire::CHANNELS {
                        continue;
                    }
                    for (i, s) in samples[..count].iter_mut().enumerate() {
                        *s = i16::from_le_bytes([a.pcm[2 * i], a.pcm[2 * i + 1]]);
                    }
                    rx.ring.insert(a.seq, &samples[..count], false);
                    rx.received.fetch_add(1, Ordering::Relaxed);
                    rx.test.store(a.flags & wire::TEST != 0, Ordering::Relaxed);
                    match expected {
                        Some(e) if (a.seq.wrapping_sub(e) as i32) > 0 => {
                            rx.lost.fetch_add(a.seq.wrapping_sub(e), Ordering::Relaxed);
                        }
                        _ => {}
                    }
                    if expected.map_or(true, |e| (a.seq.wrapping_sub(e) as i32) >= 0) {
                        expected = Some(a.seq.wrapping_add(1));
                    }
                    if rx.offset_ok.load(Ordering::Acquire) {
                        let due_here = a.due_ns as i64 - rx.offset_ns.load(Ordering::Acquire);
                        let one_way_us = ((arrived as i64 - due_here) / 1000).clamp(-1_000_000, 1_000_000) as i32;
                        let old = rx.net_us.load(Ordering::Relaxed);
                        rx.net_us.store(old + (one_way_us - old) / 16, Ordering::Relaxed);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// A ping's answer: sent at `client_ns` (this clock), answered at `server_ns` (the
    /// desktop's), back now.
    pub fn pong(&self, client_ns: u64, server_ns: u64) {
        let now = clock_ns();
        if client_ns == 0 || now < client_ns {
            return;
        }
        let rtt = now - client_ns;
        let offset = server_ns as i64 - (client_ns + rtt / 2) as i64;
        let mut pings = self.pings.lock().unwrap();
        pings.push_back((rtt, offset));
        while pings.len() > 8 {
            pings.pop_front();
        }
        if let Some(&(_, best)) = pings.iter().min_by_key(|(rtt, _)| *rtt) {
            self.offset_ns.store(best, Ordering::Release);
            self.offset_ok.store(true, Ordering::Release);
        }
    }

    /// Half the shortest recent ping round trip (ms): the way up.
    pub fn uplink_ms(&self) -> f32 {
        self.pings.lock().unwrap().iter().map(|(rtt, _)| *rtt).min().map_or(0.0, |rtt| rtt as f32 / 2e6)
    }

    /// The pictures' latency to aim at (µs: content made on the desktop to shown here; 0:
    /// none known).
    pub fn set_video_latency(&self, us: u32) {
        self.video_us.store(us, Ordering::Relaxed);
    }

    /// This second's numbers into a headset report (`crate::xr::report::AUDIO_*`).
    pub fn report(&self, v: &mut [f32]) {
        use crate::xr::report as r;
        if v.len() < r::COUNT {
            return;
        }
        let ms = |us: u32| us as f32 / 1000.0;
        let net = self.net_us.load(Ordering::Relaxed) as f32 / 1000.0;
        let (buffered, device) = (ms(self.buffered_us.load(Ordering::Relaxed)), ms(self.device_us.load(Ordering::Relaxed)));
        let received = self.received.swap(0, Ordering::Relaxed);
        let totals = [self.lost.load(Ordering::Relaxed), self.ring.late.load(Ordering::Relaxed), self.underruns.load(Ordering::Relaxed), self.concealed.load(Ordering::Relaxed)];
        let mut last = self.reported.lock().unwrap();
        // (a player that started over counts from 0 again)
        let delta = |i: usize| if totals[i] >= last[i] { totals[i] - last[i] } else { totals[i] } as f32;
        v[r::AUDIO_MS] = if received > 0 && self.offset_ok.load(Ordering::Acquire) { net + buffered + device } else { 0.0 };
        v[r::AUDIO_NET_MS] = net;
        v[r::AUDIO_BUFFER_MS] = buffered;
        v[r::AUDIO_TARGET_MS] = self.target_frames.load(Ordering::Relaxed) as f32 * FRAME_MS;
        v[r::AUDIO_DEVICE_MS] = device;
        v[r::AUDIO_VIDEO_MS] = ms(self.video_us.load(Ordering::Relaxed));
        v[r::AUDIO_RECEIVED] = received as f32;
        v[r::AUDIO_LOST] = delta(0);
        v[r::AUDIO_LATE] = delta(1);
        v[r::AUDIO_UNDERRUNS] = delta(2);
        v[r::AUDIO_CONCEALED] = delta(3);
        v[r::AUDIO_DRIFT_PPM] = self.nudge_ppm.load(Ordering::Relaxed) as f32;
        v[r::AUDIO_CLICK_MS] = ms(self.click_us.swap(0, Ordering::Relaxed));
        *last = totals;
    }

    /// The test signal's discontinuities heard so far (the tone's jumps: concealment).
    pub fn glitches(&self) -> u32 {
        self.glitches.load(Ordering::Relaxed)
    }
}

/// The audio callback's end (see the module): move it into the app's audio output callback.
pub struct XrAudioPlayer {
    rx: Arc<XrAudioRx>,
    playout: Playout,
    generation: u32,
    scratch: Vec<f32>,
    /// Frames rendered since the least buffer size was last set.
    since_floor: usize,
    /// The test signal: frames since the last loud sample, the last sample (continuity).
    quiet: usize,
    last: f32,
    jump: u32,
}

fn playout() -> Playout {
    Playout::new(PlayoutConfig { start_target: DEFAULT_MIN_FRAMES + 1, min_target: DEFAULT_MIN_FRAMES, max_target: MAX_FRAMES, default_frame: wire::FRAMES * wire::CHANNELS, channels: wire::CHANNELS })
}

impl XrAudioPlayer {
    pub fn new(rx: Arc<XrAudioRx>) -> XrAudioPlayer {
        XrAudioPlayer { rx, playout: playout(), generation: 0, scratch: vec![0.0; 2 * 4096], since_floor: usize::MAX / 2, quiet: 0, last: 0.0, jump: 0 }
    }

    /// Renders `frames` at `rate` into `data` (planar, `channels` planes of `frames`,
    /// overwritten). The audio thread's: no lock, no allocation.
    pub fn render_planar(&mut self, rate: f64, frames: usize, channels: usize, data: &mut [f32]) {
        let rx = &*self.rx;
        let now = clock_ns();
        let generation = rx.generation.load(Ordering::Acquire);
        if generation != self.generation {
            self.generation = generation;
            self.playout = playout();
        }
        let device_us = (2.0 * frames as f64 / rate.max(1.0) * 1e6) as u32;
        rx.device_us.store(device_us, Ordering::Relaxed);
        // the least buffer: the sound as late as the pictures (set every ~50 ms)
        self.since_floor += frames;
        if self.since_floor as f64 >= rate * 0.05 {
            self.since_floor = 0;
            let video = rx.video_us.load(Ordering::Relaxed) as f32 / 1000.0;
            let floor = if video > 0.0 && rx.offset_ok.load(Ordering::Acquire) {
                let net = rx.net_us.load(Ordering::Relaxed) as f32 / 1000.0;
                (((video - net - device_us as f32 / 1000.0) / FRAME_MS).round() as i64).clamp(DEFAULT_MIN_FRAMES as i64, MAX_FRAMES as i64) as u32
            } else {
                DEFAULT_MIN_FRAMES
            };
            if floor != self.playout.min_target() {
                self.playout.set_min_target(floor);
            }
        }
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(self.scratch.len() / 2);
            let out = &mut self.scratch[..2 * n];
            if self.playout.wants_render(&rx.ring) {
                self.playout.render_interleaved(&rx.ring, rate, out);
            } else {
                out.fill(0.0);
            }
            if rx.test.load(Ordering::Relaxed) {
                for i in 0..n {
                    let l = out[2 * i];
                    if l.abs() > 0.5 {
                        if self.quiet as f64 > rate * 0.05 && rx.offset_ok.load(Ordering::Acquire) {
                            // a click: due at a multiple of the period on the desktop's clock
                            let here = now as f64 + (done + i) as f64 * 1e9 / rate;
                            let there = here as i64 + rx.offset_ns.load(Ordering::Acquire);
                            let late = there.rem_euclid(CLICK_PERIOD_NS as i64) as u64;
                            rx.click_us.store((late / 1000) as u32, Ordering::Relaxed);
                        }
                        self.quiet = 0;
                        // (the resampled edge into a click is no jump of the tone)
                        self.jump = 0;
                    } else {
                        // the tone moves at most ~0.012 a sample; a jump not followed by a
                        // click within 8 samples is a discontinuity (a concealment)
                        if self.jump > 0 {
                            self.jump -= 1;
                            if self.jump == 0 {
                                rx.glitches.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        if self.quiet > 64 && (l - self.last).abs() > 0.05 && self.jump == 0 {
                            self.jump = 8;
                        }
                        self.quiet += 1;
                    }
                    self.last = l;
                }
            }
            for c in 0..channels {
                let plane = &mut data[c * frames + done..c * frames + done + n];
                match c {
                    0 | 1 if channels >= 2 => {
                        for (i, v) in plane.iter_mut().enumerate() {
                            *v = out[2 * i + c];
                        }
                    }
                    0 => {
                        for (i, v) in plane.iter_mut().enumerate() {
                            *v = (out[2 * i] + out[2 * i + 1]) * 0.5;
                        }
                    }
                    _ => plane.fill(0.0),
                }
            }
            done += n;
        }
        rx.buffered_us.store((self.playout.buffered_ms(&rx.ring) * 1000.0) as u32, Ordering::Relaxed);
        rx.target_frames.store(self.playout.target_frames(), Ordering::Relaxed);
        rx.underruns.store(self.playout.stats.underruns, Ordering::Relaxed);
        rx.concealed.store(self.playout.stats.concealed, Ordering::Relaxed);
        rx.nudge_ppm.store((self.playout.nudge() * 1e6) as i32, Ordering::Relaxed);
    }
}
