//! The encode worker: on the capture cadence, the guard decides whether to encode and at which
//! rung, the encoder runs, and its access units go to the network worker. Each picture it encodes
//! is also this device's own tile.

use super::codec::{Codecs, Encoder, Picture};
use super::synth::SynthCamera;
use super::{ring, CallEvent, CameraFrame, Shared, UiMsg, VideoConfig, VideoSource};
use crate::guard::{video_ask, Guard, Tick, LADDER};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Captures a second. The guard lowers the rung's own rate below this.
const CAPTURE_FPS: u64 = 30;
/// Spare camera buffers kept for reuse.
const SPARE: usize = 4;

pub(super) struct Encoded {
    pub au: Vec<u8>,
    pub key: bool,
    pub capture_us: u64,
}

pub(super) struct Encode {
    video: VideoConfig,
    codecs: Arc<dyn Codecs>,
    shared: Arc<Shared>,
    camera: Receiver<CameraFrame>,
    spare: Arc<Mutex<Vec<Vec<u8>>>>,
    out: Sender<Encoded>,
    inlet: SyncSender<(SocketAddr, Vec<u8>)>,
    recycle: Receiver<Arc<Picture>>,
    ui: Sender<UiMsg>,
    signal: Arc<dyn Fn() + Send + Sync>,
}

/// Nearest-sample scaling of an NV12 picture (both sizes even).
fn scale_nv12(src: &CameraFrame, w: u32, h: u32, dst: &mut Vec<u8>) {
    let (sw, sh) = (src.width as usize, src.height as usize);
    let (w, h) = (w as usize, h as usize);
    dst.resize(w * h * 3 / 2, 128);
    if sw < 2 || sh < 2 || src.nv12.len() < sw * sh * 3 / 2 {
        return;
    }
    for y in 0..h {
        let sy = y * sh / h;
        for x in 0..w {
            dst[y * w + x] = src.nv12[sy * sw + x * sw / w];
        }
    }
    let (c, sc) = (w * h, sw * sh);
    for y in 0..h / 2 {
        let sy = y * (sh / 2) / (h / 2);
        for x in 0..w / 2 {
            let sx = x * (sw / 2) / (w / 2);
            dst[c + y * w + 2 * x] = src.nv12[sc + sy * sw + 2 * sx];
            dst[c + y * w + 2 * x + 1] = src.nv12[sc + sy * sw + 2 * sx + 1];
        }
    }
}

impl Encode {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        video: VideoConfig,
        codecs: Arc<dyn Codecs>,
        shared: Arc<Shared>,
        camera: Receiver<CameraFrame>,
        spare: Arc<Mutex<Vec<Vec<u8>>>>,
        out: Sender<Encoded>,
        inlet: SyncSender<(SocketAddr, Vec<u8>)>,
        recycle: Receiver<Arc<Picture>>,
        ui: Sender<UiMsg>,
        signal: Arc<dyn Fn() + Send + Sync>,
    ) -> Encode {
        Encode { video, codecs, shared, camera, spare, out, inlet, recycle, ui, signal }
    }

    fn tell(&self, msg: UiMsg) {
        let _ = self.ui.send(msg);
        (self.signal)();
    }

    /// A buffer for the next picture: one this device's tile let go of, or a new one.
    fn buffer(&self) -> Vec<u8> {
        while let Ok(p) = self.recycle.try_recv() {
            if let Ok(p) = Arc::try_unwrap(p) {
                return p.nv12;
            }
        }
        Vec::new()
    }

    pub(super) fn run(self) {
        let top = self.video.top.min(LADDER.len() - 1);
        let mut guard = Guard::with_top(Guard::rung_for(video_ask(crate::session::START_BPS)), top);
        let mut encoder: Option<Box<dyn Encoder>> = None;
        let mut kbps = 0u32;
        let mut reported = false;
        let mut synth = match self.video.source {
            VideoSource::Synthetic(kind) => Some(SynthCamera::new(kind)),
            VideoSource::Feed => None,
        };
        let mut latest: Option<CameraFrame> = None;
        // The capture time of the camera frame encoded last: a frame is never encoded twice.
        let mut encoded_frame_us = None;
        let mut captured: BTreeMap<i64, u64> = BTreeMap::new();
        let period = Duration::from_nanos(1_000_000_000 / CAPTURE_FPS);
        let mut due = Instant::now();
        for n in 0u64.. {
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            if self.shared.stop.load(Ordering::Relaxed) {
                break;
            }
            let now = Instant::now();
            due += period;
            if due <= now {
                due = now + period;
            }
            while let Ok(f) = self.camera.try_recv() {
                if let Some(old) = latest.replace(f) {
                    if let Ok(mut spare) = self.spare.try_lock() {
                        if spare.len() < SPARE {
                            spare.push(old.nv12);
                        }
                    }
                }
            }
            if !self.shared.camera_on.load(Ordering::Relaxed) {
                encoder = None;
                captured.clear();
                continue;
            }
            let capture_us = self.shared.now_us();
            let level = f64::from_bits(self.shared.loss_level_bits.load(Ordering::Relaxed));
            let decision = guard.tick(
                capture_us,
                self.shared.estimate_bps.load(Ordering::Relaxed),
                self.shared.pacer_delay_us.load(Ordering::Relaxed),
                (!level.is_nan()).then_some(level),
            );
            let step = guard.take_step();
            let rung = guard.rung();
            let want = (guard.ask_bps() / 1_000).clamp(1, u64::from(self.video.max_kbps.max(1))) as u32;
            if step.is_some() {
                encoder = None;
                captured.clear();
                reported = false;
            }
            if encoder.is_none() && (decision == Tick::Encode || step.is_some()) {
                match self.codecs.encoder(rung, want) {
                    Ok(e) => {
                        self.tell(UiMsg::Encoder(e.name()));
                        encoder = Some(e);
                        kbps = want;
                        self.shared.key_wanted.store(true, Ordering::Relaxed);
                    }
                    Err(e) => {
                        if !std::mem::replace(&mut reported, true) {
                            self.tell(UiMsg::Event(CallEvent::Problem { peer: None, what: format!("the camera's encoder couldn't open for {rung}: {e}") }));
                        }
                    }
                }
            }
            if let Some(e) = &mut encoder {
                if u64::from(want.abs_diff(kbps)) * 20 > u64::from(kbps) {
                    e.set_kbps(want);
                    kbps = want;
                }
            }
            self.shared.rung.store(LADDER.iter().position(|r| *r == rung).unwrap_or(usize::MAX), Ordering::Relaxed);
            self.shared.asked_kbps.store(u64::from(kbps), Ordering::Relaxed);
            if decision == Tick::Skip {
                continue;
            }
            let Some(e) = &mut encoder else { continue };
            // A fed camera's picture keeps its own capture time; a synthetic one is made now.
            let media_us = match (&synth, &latest) {
                (Some(_), _) => capture_us,
                (None, Some(frame)) if encoded_frame_us != Some(frame.capture_us) => frame.capture_us,
                (None, _) => continue,
            };
            let mut nv12 = self.buffer();
            match (&mut synth, &latest) {
                (Some(s), _) => s.picture(rung.width, rung.height, n, &mut nv12),
                (None, Some(frame)) => {
                    scale_nv12(frame, rung.width, rung.height, &mut nv12);
                    encoded_frame_us = Some(frame.capture_us);
                }
                (None, None) => continue,
            }
            if self.shared.key_wanted.swap(false, Ordering::Relaxed) {
                e.request_key();
            }
            let pts = media_us as i64 * 10;
            captured.insert(pts, media_us);
            match e.push(&nv12, pts) {
                Ok(packets) => {
                    for p in packets {
                        guard.encoded(self.shared.now_us(), p.data.len(), p.key);
                        let at = captured.remove(&p.pts_100ns).unwrap_or(media_us);
                        while captured.first_key_value().is_some_and(|(&k, _)| k < p.pts_100ns) {
                            captured.pop_first();
                        }
                        self.shared.pictures_sent.fetch_add(1, Ordering::Relaxed);
                        self.shared.keys_sent.fetch_add(u64::from(p.key), Ordering::Relaxed);
                        let _ = self.out.send(Encoded { au: p.data, key: p.key, capture_us: at });
                        ring(&self.inlet);
                    }
                }
                Err(err) => self.tell(UiMsg::Event(CallEvent::Problem { peer: None, what: format!("the camera's encoder failed: {err}") })),
            }
            let picture = Picture { width: rung.width, height: rung.height, nv12, pts_100ns: pts };
            let _ = self.ui.send(UiMsg::Picture { peer: None, picture: Arc::new(picture), capture_us: media_us });
            (self.signal)();
        }
    }
}
