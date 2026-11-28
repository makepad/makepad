//! Incoming video gets one long-lived decode worker per participant. Each opens its decoder
//! on a key frame and sends pictures to the UI, which keeps the newest per tile.

use super::codec::{Codecs, Decoder, Picture};
use super::{spawn_worker, CallEvent, PeerId, Shared, Spawn, UiMsg};
use crate::video::Frame;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// One microsecond in 100 ns units: every frame index gets a distinct microsecond PTS so
/// microsecond-native decoders (Android MediaCodec / HalStreamDecoder) round-trip it losslessly.
const FRAME_PTS_UNIT_100NS: i64 = 10;
const LATE_PICTURE_POLL_MIN: Duration = Duration::from_millis(10);
const LATE_PICTURE_POLL_MAX: Duration = Duration::from_millis(100);

#[derive(Default)]
struct PictureSlot {
    newest: Option<(Arc<Picture>, u64)>,
    // A departing tile hands over its entire retry list if the return channel is still full.
    retired: Vec<Arc<Picture>>,
}

pub(super) struct RemotePictures {
    slot: Arc<Mutex<PictureSlot>>,
    returns: SyncSender<Arc<Picture>>,
}

impl RemotePictures {
    pub(super) fn take(&self) -> Option<(Arc<Picture>, u64)> {
        self.slot.lock().unwrap().newest.take()
    }

    pub(super) fn give_back(&self, picture: Arc<Picture>, waiting: &mut Vec<Arc<Picture>>) {
        if let Err(TrySendError::Full(picture)) = self.returns.try_send(picture) {
            waiting.push(picture);
        }
        // On disconnect the decoder has exited.
    }

    pub(super) fn retry(&self, waiting: &mut Vec<Arc<Picture>>) {
        while let Some(picture) = waiting.pop() {
            if let Err(TrySendError::Full(picture)) = self.returns.try_send(picture) {
                waiting.push(picture);
                break;
            }
        }
    }

    pub(super) fn detach(&self, waiting: &mut Vec<Arc<Picture>>) {
        if !waiting.is_empty() {
            self.slot.lock().unwrap().retired = std::mem::take(waiting);
        }
    }
}

// A growing FIFO keeps every incoming frame and reuses its storage. The standard unbounded
// channel frees a block on the decode thread about every 31 frames. Only net and decode use this.
#[derive(Default)]
struct FrameQueue {
    decoded: AtomicU64,
    last_frame: AtomicU32,
    errors: AtomicU64,
    state: Mutex<FrameQueueState>,
    ready: Condvar,
    #[cfg(test)]
    parked: Condvar,
}

#[derive(Default)]
struct FrameQueueState {
    frames: VecDeque<Frame>,
    closed: bool,
    #[cfg(test)]
    last_wait: Option<(u64, Option<Duration>)>,
}

struct FrameSender(Arc<FrameQueue>);

impl FrameSender {
    fn send(&self, frame: Frame) {
        self.0.state.lock().unwrap().frames.push_back(frame);
        self.0.ready.notify_one();
    }
}

impl Drop for FrameSender {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().closed = true;
        self.0.ready.notify_one();
    }
}

enum Receive {
    Frame(Frame),
    Timeout,
    Closed,
}

impl FrameQueue {
    fn receive_within(&self, stop: &AtomicBool, wait: Option<Duration>) -> Receive {
        let mut state = self.state.lock().unwrap();
        let deadline = wait.map(|d| Instant::now() + d);
        loop {
            if stop.load(Ordering::Relaxed) { return Receive::Closed }
            if let Some(frame) = state.frames.pop_front() { return Receive::Frame(frame) }
            if state.closed { return Receive::Closed }
            #[cfg(test)]
            {
                state.last_wait = Some((self.decoded.load(Ordering::Acquire), wait));
                self.parked.notify_all();
            }
            let mut step = Duration::from_millis(50);
            if let Some(at) = deadline {
                let left = at.saturating_duration_since(Instant::now());
                if left.is_zero() { return Receive::Timeout }
                step = step.min(left);
            }
            state = self.ready.wait_timeout(state, step).unwrap().0;
        }
    }
}

pub(super) struct Decoders {
    spawn: Spawn,
    worker: Decode,
    pub(super) returned_frames: Receiver<(PeerId, u8, Vec<u8>)>,
    // Camera and share have independent prediction chains and latest-picture slots.
    senders: BTreeMap<(PeerId, u8), FrameSender>,
}

impl Decoders {
    pub(super) fn new(
        spawn: Spawn,
        codecs: Arc<dyn Codecs>,
        shared: Arc<Shared>,
        ui: Sender<UiMsg>,
        signal: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        // Room for a burst of returns between network polls. A full channel only costs reuse.
        let (return_frames, returned_frames) = sync_channel(8);
        Self { spawn, worker: Decode { codecs, shared, ui, signal, return_frames }, returned_frames, senders: BTreeMap::new() }
    }

    pub(super) fn frame(&mut self, peer: PeerId, stream: u8, frame: Frame) {
        let sender = self.senders.entry((peer, stream)).or_insert_with(|| {
            let frames = Arc::new(FrameQueue::default());
            let sender = FrameSender(frames.clone());
            let worker = self.worker.clone();
            // Register before spawning, so WorkerDone can never get ahead of this message.
            let slot = Arc::new(Mutex::new(PictureSlot::default()));
            let (returns, returned) = sync_channel(4);
            let pictures = RemotePictures { slot: slot.clone(), returns };
            let _ = worker.ui.send(UiMsg::DecoderStarted { peer, stream, pictures });
            spawn_worker(&self.spawn, "call decode", worker.shared.clone(), worker.ui.clone(), worker.signal.clone(),
                move || worker.run(peer, stream, frames, slot, returned));
            sender
        });
        sender.send(frame);
    }

    pub(super) fn share_progress(&self, peer: PeerId) -> (u64, u32, u64) {
        self.senders.get(&(peer, crate::wire::stream::SHARE)).map_or((0, 0, 0), |s|
            (s.0.decoded.load(Ordering::Acquire), s.0.last_frame.load(Ordering::Relaxed), s.0.errors.load(Ordering::Relaxed)))
    }

    #[cfg(test)]
    fn wait_for_parked(&self, peer: PeerId, stream: u8, decoded: u64) -> Option<Duration> {
        let queue = &self.senders.get(&(peer, stream)).expect("decode worker must exist").0;
        let state = queue.state.lock().unwrap();
        let state = queue.parked.wait_while(state, |s| {
            !s.closed && !matches!(s.last_wait, Some((d, _)) if d >= decoded)
        }).unwrap();
        let (seen, wait) = state.last_wait.expect("decode worker closed before parking");
        assert_eq!(seen, decoded, "decode worker parked at unexpected decoded count");
        wait
    }

    pub(super) fn close(&mut self, peer: PeerId, stream: u8) {
        self.senders.remove(&(peer, stream));
    }

    pub(super) fn leave(&mut self, peer: PeerId) {
        self.senders.retain(|&(id, _), _| id != peer);
    }
}

#[derive(Clone)]
struct Decode {
    codecs: Arc<dyn Codecs>,
    shared: Arc<Shared>,
    ui: Sender<UiMsg>,
    signal: Arc<dyn Fn() + Send + Sync>,
    return_frames: SyncSender<(PeerId, u8, Vec<u8>)>,
}

impl Decode {
    fn tell(&self, msg: UiMsg) {
        let _ = self.ui.send(msg);
        (self.signal)();
    }

    fn run(self, peer: PeerId, stream: u8, frames: Arc<FrameQueue>, slot: Arc<Mutex<PictureSlot>>, returned: Receiver<Arc<Picture>>) {
        let mut decoder: Option<Box<dyn Decoder>> = None;
        // Capture times of the pictures pushed and not yet out, by decoder PTS.
        let mut pending = VecDeque::with_capacity(16);
        let mut pictures = Vec::new();
        let mut spares = Vec::new();
        let mut had_return = false;
        let mut logged_allocation = false;
        let mut reported = false;
        let mut retry_wait = LATE_PICTURE_POLL_MIN;
        let mut last_capture_us = 0;
        loop {
            let wait = (decoder.is_some() && !pending.is_empty()).then_some(retry_wait);
            let frame = match frames.receive_within(&self.shared.stop, wait) {
                Receive::Closed => break,
                Receive::Frame(frame) => {
                    last_capture_us = frame.capture_us;
                    retry_wait = LATE_PICTURE_POLL_MIN;
                    Some(frame)
                }
                Receive::Timeout => None,
            };
            if let Some(frame) = frame.as_ref().filter(|f| f.key) {
                if decoder.as_mut().is_some_and(|d| !d.accepts(&frame.data)) {
                    // A new stream (a new size after a step): a new decoder, and the pictures the
                    // old one held never come.
                    decoder = None;
                    pending.clear();
                }
                if decoder.is_none() {
                    match self.codecs.decoder() {
                        Ok(mut d) => {
                            d.accepts(&frame.data);
                            let _ = self.ui.send(UiMsg::Decoder(d.name()));
                            decoder = Some(d);
                        }
                        Err(e) => {
                            frames.errors.fetch_add(1, Ordering::Relaxed);
                            if !std::mem::replace(&mut reported, true) {
                                let what = format!("this person's video couldn't be decoded: {e}");
                                self.tell(UiMsg::Event(CallEvent::Problem { peer: Some(peer), what }));
                            }
                        }
                    }
                }
            }
            let Some(d) = &mut decoder else {
                if let Some(frame) = frame { let _ = self.return_frames.try_send((peer, stream, frame.data)); }
                continue;
            };
            while let Ok(mut picture) = returned.try_recv() {
                had_return = true;
                if Arc::get_mut(&mut picture).is_some() {
                    spares.push(picture);
                }
            }
            let allocated = d.allocated();
            let timed_out = frame.is_none();
            let decoded = if let Some(frame) = frame {
                let pts = i64::from(frame.frame) * FRAME_PTS_UNIT_100NS;
                pending.push_back((pts, frame.capture_us));
                let result = d.push_into(&frame.data, pts, &mut pictures);
                let _ = self.return_frames.try_send((peer, stream, frame.data));
                result
            } else {
                d.ready_into(&mut pictures)
            };
            match decoded {
                Ok(()) => {
                    let count = pictures.len();
                    let last_frame = pictures.last().map(|p| (p.pts_100ns.max(0) / FRAME_PTS_UNIT_100NS) as u32);
                    for p in pictures.drain(..) {
                        while pending.front().is_some_and(|&(k, _)| k < p.pts_100ns) {
                            pending.pop_front();
                        }
                        let capture_us = if pending.front().is_some_and(|&(k, _)| k == p.pts_100ns) {
                            pending.pop_front().unwrap().1
                        } else {
                            last_capture_us
                        };
                        if had_return && !logged_allocation && (allocated != d.allocated() || spares.is_empty()) {
                            eprintln!("call decode: allocating picture storage for peer {peer} after recycling began");
                            logged_allocation = true;
                        }
                        let picture = if let Some(mut spare) = spares.pop() {
                            // Only unique Arcs enter this private pool.
                            let old = std::mem::replace(Arc::get_mut(&mut spare).unwrap(), p);
                            d.recycle(old.nv12);
                            spare
                        } else {
                            Arc::new(p)
                        };
                        let previous = slot.lock().unwrap().newest.replace((picture, capture_us));
                        if let Some((picture, _)) = previous {
                            // The UI never took this picture, so its Arc is already unique.
                            had_return = true;
                            spares.push(picture);
                        }
                    }
                    if count != 0 {
                        retry_wait = LATE_PICTURE_POLL_MIN;
                        if stream == crate::wire::stream::SHARE {
                            frames.last_frame.store(last_frame.unwrap(), Ordering::Relaxed);
                            frames.decoded.fetch_add(count as u64, Ordering::Release);
                        }
                        (self.signal)();
                    } else if timed_out {
                        retry_wait = (retry_wait * 2).min(LATE_PICTURE_POLL_MAX);
                    }
                }
                Err(e) => {
                    if stream == crate::wire::stream::SHARE {
                        frames.errors.fetch_add(1, Ordering::Relaxed);
                        decoder = None;
                        pending.clear();
                    }
                    pictures.clear();
                    if !std::mem::replace(&mut reported, true) {
                        self.tell(UiMsg::Event(CallEvent::Problem { peer: Some(peer), what: format!("a picture from this person couldn't be decoded: {e}") }));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::codec::Encoder;
    use crate::driver::watched_spawn;
    use crate::guard::Rung;
    use crate::video::Frame;
    use crate::wire::stream;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::{channel, RecvTimeoutError};

    type Gate = Arc<(Mutex<bool>, Condvar)>;

    struct GatedCodecs(Mutex<Option<Gate>>);
    struct GatedDecoder {
        gate: Option<Gate>,
        inner: Box<dyn Decoder>,
    }

    impl Codecs for GatedCodecs {
        fn encoder(&self, _: Rung, _: u32) -> Result<Box<dyn Encoder>, String> {
            Err("decode-only test".into())
        }

        fn decoder(&self) -> Result<Box<dyn Decoder>, String> {
            Ok(Box::new(GatedDecoder {
                gate: self.0.lock().unwrap().take(),
                inner: crate::driver::codec::TestCodecs.decoder()?,
            }))
        }
    }

    impl Decoder for GatedDecoder {
        fn push_into(&mut self, au: &[u8], pts_100ns: i64, out: &mut Vec<Picture>) -> Result<(), String> {
            if let Some(gate) = self.gate.take() {
                let mut blocked = gate.0.lock().unwrap();
                *blocked = true;
                gate.1.notify_one();
                drop(gate.1.wait_while(blocked, |b| *b).unwrap());
            }
            self.inner.push_into(au, pts_100ns, out)
        }

        fn ready_into(&mut self, out: &mut Vec<Picture>) -> Result<(), String> { self.inner.ready_into(out) }
        fn recycle(&mut self, nv12: Vec<u8>) { self.inner.recycle(nv12); }
        fn allocated(&self) -> u64 { self.inner.allocated() }

        fn name(&self) -> String {
            "gated decoder".into()
        }
    }

    #[test]
    fn late_decoded_picture_reaches_the_tile_without_another_frame() {
        use crate::driver::codec::TestCodecs;
        #[derive(Default)]
        struct Delay { polls: usize, release: bool }
        struct DelayedCodecs(Arc<(Mutex<Delay>, Condvar)>);
        struct DelayedDecoder {
            delay: Arc<(Mutex<Delay>, Condvar)>,
            inner: Box<dyn Decoder>,
            staged: Vec<Picture>,
        }
        impl Codecs for DelayedCodecs {
            fn encoder(&self, rung: Rung, kbps: u32) -> Result<Box<dyn Encoder>, String> { TestCodecs.encoder(rung, kbps) }
            fn decoder(&self) -> Result<Box<dyn Decoder>, String> {
                Ok(Box::new(DelayedDecoder { delay: self.0.clone(), inner: TestCodecs.decoder()?, staged: Vec::new() }))
            }
        }
        impl Decoder for DelayedDecoder {
            fn push_into(&mut self, au: &[u8], pts: i64, _: &mut Vec<Picture>) -> Result<(), String> {
                self.inner.push_into(au, pts, &mut self.staged)
            }
            fn ready_into(&mut self, out: &mut Vec<Picture>) -> Result<(), String> {
                let mut state = self.delay.0.lock().unwrap();
                state.polls += 1;
                if state.release { out.append(&mut self.staged); }
                self.delay.1.notify_one();
                Ok(())
            }
            fn recycle(&mut self, nv12: Vec<u8>) { self.inner.recycle(nv12); }
            fn allocated(&self) -> u64 { self.inner.allocated() }
            fn name(&self) -> String { self.inner.name() }
        }

        let delay = Arc::new((Mutex::new(Delay::default()), Condvar::new()));
        let (ui, messages) = channel();
        let (signal, wake) = channel();
        let mut decoders = Decoders::new(crate::driver::std_spawn(), Arc::new(DelayedCodecs(delay.clone())),
            Arc::new(Shared::new(Instant::now(), false)), ui, Arc::new(move || { let _ = signal.send(()); }));
        let rung = Rung { width: 4, height: 2, fps: 30, min_ask_bps: 0, full_bps: 1_000 };
        let data = TestCodecs.encoder(rung, 1).unwrap().push(&[128; 12], 0).unwrap().pop().unwrap().data;
        decoders.frame(1, stream::SHARE, Frame { frame: 1, key: true, capture_us: 1, data });
        let UiMsg::DecoderStarted { peer: 1, stream: stream::SHARE, pictures } = messages.recv().unwrap()
            else { panic!("the share tile must register before decoding") };
        assert!(matches!(messages.recv().unwrap(), UiMsg::Decoder(_)), "the key frame opens the decoder");
        let mut state = delay.1.wait_while(delay.0.lock().unwrap(), |s| s.polls == 0).unwrap();
        assert!(pictures.take().is_none(), "the delayed picture cannot appear before release");
        assert_eq!(decoders.share_progress(1), (0, 0, 0), "an empty poll does not count a frame");
        state.release = true;
        drop(state);
        wake.recv().expect("the late picture wakes the UI");
        let (pic, capture_us) = pictures.take().expect("one key frame reaches the tile without another push");
        assert_eq!((pic.width, pic.height, pic.pts_100ns, capture_us), (4, 2, 10, 1), "late delivery preserves size, timestamp and capture time");
        assert_eq!(decoders.share_progress(1), (1, 1, 0), "the share accounts for exactly one decoded frame");
        drop(decoders);
        assert!(matches!(messages.recv().unwrap(), UiMsg::WorkerDone), "dropping the queue ends the worker");
    }

    #[test]
    fn microsecond_rounded_decoder_drains_pending_and_reports_exact_frame_numbers() {
        use crate::driver::codec::TestCodecs;

        #[derive(Default)]
        struct Trace {
            pts_us: Vec<i64>,
            polls: usize,
            release_last: bool,
        }
        struct RoundingCodecs(Arc<Mutex<Trace>>);
        struct MicrosecondRoundingDecoder {
            trace: Arc<Mutex<Trace>>,
            inner: Box<dyn Decoder>,
            staged: Vec<Picture>,
        }
        impl Codecs for RoundingCodecs {
            fn encoder(&self, rung: Rung, kbps: u32) -> Result<Box<dyn Encoder>, String> { TestCodecs.encoder(rung, kbps) }
            fn decoder(&self) -> Result<Box<dyn Decoder>, String> {
                Ok(Box::new(MicrosecondRoundingDecoder { trace: self.0.clone(), inner: TestCodecs.decoder()?, staged: Vec::new() }))
            }
        }
        impl Decoder for MicrosecondRoundingDecoder {
            fn push_into(&mut self, au: &[u8], pts_100ns: i64, out: &mut Vec<Picture>) -> Result<(), String> {
                let pts_us = pts_100ns / 10;
                self.trace.lock().unwrap().pts_us.push(pts_us);
                // Emit the preceding picture so its capture time differs from the latest push.
                out.append(&mut self.staged);
                self.inner.push_into(au, pts_us * 10, &mut self.staged)
            }
            fn ready_into(&mut self, out: &mut Vec<Picture>) -> Result<(), String> {
                let mut trace = self.trace.lock().unwrap();
                trace.polls += 1;
                if trace.release_last { out.append(&mut self.staged); }
                Ok(())
            }
            fn recycle(&mut self, nv12: Vec<u8>) { self.inner.recycle(nv12); }
            fn allocated(&self) -> u64 { self.inner.allocated() }
            fn name(&self) -> String { self.inner.name() }
        }

        let trace = Arc::new(Mutex::new(Trace::default()));
        let (ui, messages) = channel();
        let (signal, wake) = channel();
        let mut decoders = Decoders::new(crate::driver::std_spawn(), Arc::new(RoundingCodecs(trace.clone())),
            Arc::new(Shared::new(Instant::now(), false)), ui, Arc::new(move || { let _ = signal.send(()); }));
        let rung = Rung { width: 4, height: 2, fps: 30, min_ask_bps: 0, full_bps: 1_000 };
        let mut encoder = TestCodecs.encoder(rung, 1).unwrap();
        let mut frame = |number: u32| {
            let packet = encoder.push(&[128; 12], 0).unwrap().pop().unwrap();
            Frame { frame: number, key: packet.key, capture_us: 100 + u64::from(number), data: packet.data }
        };
        decoders.frame(1, stream::SHARE, frame(1));
        let UiMsg::DecoderStarted { peer: 1, stream: stream::SHARE, pictures } = messages.recv().unwrap()
            else { panic!("the share tile must register before decoding") };
        assert!(matches!(messages.recv().unwrap(), UiMsg::Decoder(_)), "the key frame opens the decoder");
        let check_picture = |decoders: &Decoders, number: u32| {
            wake.recv().expect("a decoded picture must wake the UI");
            let (pic, capture_us) = pictures.take().expect("the share tile must have a picture");
            assert_eq!((pic.pts_100ns, capture_us), (10 * i64::from(number), 100 + u64::from(number)),
                "frame {number} must retain its PTS and original capture time");
            assert_eq!(decoders.share_progress(1), (u64::from(number), number, 0), "share progress must report wire frame {number}");
        };
        for number in 2..=11 {
            decoders.frame(1, stream::SHARE, frame(number));
            check_picture(&decoders, number - 1);
        }
        trace.lock().unwrap().release_last = true;
        check_picture(&decoders, 11);
        let polls = {
            let trace = trace.lock().unwrap();
            assert_eq!(trace.pts_us, (1..=11).collect::<Vec<_>>(), "the decoder must see distinct microseconds for all eleven frames");
            trace.polls
        };
        assert_eq!(decoders.wait_for_parked(1, stream::SHARE, 11), None,
            "the worker must park without a poll deadline once pending is empty");
        assert_eq!(trace.lock().unwrap().polls, polls, "polling must stop once the final picture drains pending");
        drop(decoders);
        assert!(matches!(messages.recv().unwrap(), UiMsg::WorkerDone), "dropping the share must stop the worker");
    }

    #[test]
    fn each_peer_decodes_on_its_own_worker() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let codecs = Arc::new(GatedCodecs(Mutex::new(Some(gate.clone()))));
        let shared = Arc::new(Shared::new(Instant::now(), false));
        let (ui, messages) = channel();
        // Every wait below has no deadline, so a loaded box only makes it slower. Each fails at
        // once on what would keep it waiting forever: a reported problem or a panicked worker.
        let panicked = Arc::new(AtomicUsize::new(0));
        let mut decoders = Decoders::new(watched_spawn(panicked.clone()), codecs, shared.clone(), ui, Arc::new(|| {}));
        let rung = Rung { width: 4, height: 2, fps: 30, min_ask_bps: 0, full_bps: 1_000 };
        let mut encoder = crate::driver::codec::TestCodecs.encoder(rung, 1).unwrap();
        let au = encoder.push(&[128; 12], 0).unwrap().pop().unwrap().data;
        let frame = |n, key| Frame { frame: n, key, capture_us: u64::from(n), data: au.clone() };
        let alive = |waiting_for: &str| assert_eq!(panicked.load(Ordering::Relaxed), 0, "{waiting_for}: a decode worker panicked");
        let mut slots = BTreeMap::new();
        decoders.frame(1, stream::CAMERA_HIGH, frame(1, true));
        {
            // Registration and the decoder name precede push. No picture can be published yet.
            let mut blocked = gate.0.lock().unwrap();
            while !*blocked {
                blocked = gate.1.wait_timeout(blocked, Duration::from_millis(10)).unwrap().0;
                alive("waiting for peer 1 to block");
                match messages.try_recv() {
                    Ok(UiMsg::DecoderStarted { peer, pictures, .. }) => { slots.insert(peer, pictures); }
                    Ok(UiMsg::Decoder(name)) if name == "gated decoder" => {}
                    Err(_) => {}
                    Ok(UiMsg::Event(CallEvent::Problem { what, .. })) => panic!("waiting for peer 1 to block: {what}"),
                    Ok(_) => panic!("waiting for peer 1 to block: an unexpected message"),
                }
            }
        }
        assert!(slots.values().all(|pictures| pictures.take().is_none()), "peer 1 must not publish a picture while blocked in push");
        let mut next = |waiting_for: &str| loop {
            for (&peer, pictures) in &slots {
                if let Some((_, capture_us)) = pictures.take() {
                    return Some((peer, capture_us));
                }
            }
            match messages.recv_timeout(Duration::from_millis(10)) {
                Ok(UiMsg::DecoderStarted { peer, pictures, .. }) => { slots.insert(peer, pictures); }
                Ok(UiMsg::WorkerDone) => return None,
                Ok(UiMsg::Event(CallEvent::Problem { what, .. })) => panic!("{waiting_for}: {what}"),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => alive(waiting_for),
                Err(RecvTimeoutError::Disconnected) => panic!("{waiting_for}: every sender is gone"),
            }
        };

        decoders.frame(2, stream::CAMERA_LOW, frame(1, true));
        assert_eq!(next("waiting for peer 2"), Some((2, 1)), "peer 2 should show a picture while peer 1 is blocked");
        *gate.0.lock().unwrap() = false;
        gate.1.notify_one();
        assert_eq!(next("waiting for peer 1"), Some((1, 1)), "releasing peer 1 should deliver its picture");
        let routes: Vec<_> = decoders.returned_frames.try_iter().map(|(peer, stream, _)| (peer, stream)).collect();
        assert_eq!(routes, [(2, stream::CAMERA_LOW), (1, stream::CAMERA_HIGH)], "each buffer must go back to the peer and stream it came from");

        decoders.leave(1);
        assert_eq!(next("waiting for peer 1 to exit"), None, "leaving should end peer 1's worker");
        assert_eq!(shared.workers.load(Ordering::Relaxed), 1, "only peer 2's worker should remain");
        decoders.frame(2, stream::CAMERA_LOW, frame(2, false));
        assert_eq!(next("waiting for peer 2's delta"), Some((2, 2)), "peer 2 should keep its open decoder");
        decoders.frame(1, stream::CAMERA_HIGH, frame(2, false));
        decoders.frame(1, stream::CAMERA_HIGH, frame(3, true));
        assert_eq!(next("waiting for peer 1 to rejoin"), Some((1, 3)), "a rejoined peer should start decoding at its new key");
        drop(decoders);
        for _ in 0..2 {
            assert_eq!(next("waiting for decoder shutdown"), None, "dropping both senders should end both workers");
        }
    }
}
