//! The server's picture pipeline for one session: the headset's poses to the app's XR display
//! (and kept to match their pictures), the app's pictures matched to their poses and fanned out
//! to the eye encoders (the tile fan-out and rate controller every Desk Link stream shares,
//! `makepad_desk_link::{tiles, rate}`), the flow that bounds what is in flight.

use super::debug::{dump_picture, tune, Tune};
use super::foveation::fovea_setting;
use super::ServerShared;
use makepad_desk_link::rate::{effort, LinkTrip, RateControl, Second};
use makepad_desk_link::tiles::{TileDone, TileEncoders, TileSink, TileSpec, TileWork};
use makepad_desk_link::xr::*;
use makepad_widgets::makepad_platform::makepad_hal::{self, xr_remote as hal};
use makepad_widgets::makepad_platform::makepad_network::tls as net_tls;
use makepad_widgets::makepad_platform::video_file::{GpuCompletion, StreamTile, StreamTuning, StreamVideoCodec, SurfaceHandle, VideoStreamEncoder, VideoStreamEncoderOptions};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the encode side hands the session's TLS thread: an eye's access unit, when encoded.
pub(crate) enum Out {
    Video(VideoHead, Vec<u8>, Instant),
}

/// A picture the app packed (`makepad_xr_stream`) for the encoder: the shared surface (the
/// packed NV12 layout of the whole picture, both eye columns), when the GPU work that drew it
/// completes (`ready`), the display time of the pose it was drawn with. `held` is dropped once
/// every eye's encode read the surface: the app may draw into it again.
pub(crate) struct Picture {
    /// One surface (both eye columns, each tile encodes its column) or one per tile.
    pub surfaces: Vec<SurfaceHandle>,
    pub ready: GpuCompletion,
    pub held: Box<dyn Send + Sync>,
    pub display_ns: u64,
    pub built_at: Instant,
    /// When the app last asked for a pose before it drew this one (`hal_xr_remote_drawn`): a
    /// pose that came before waited for the app until then.
    pub asked: Option<Instant>,
    /// What the view centres on (m), for reprojection without per-pixel depth.
    pub focus_m: f32,
    pub rendered: Option<RenderedPose>,
}

#[derive(Clone, Copy)]
pub(crate) struct RenderedPose {
    pub seq: u64,
    pub eyes: [XrRenderEye; 2],
    pub arrived: Instant,
}

/// Pictures built and their eyes decoded by the headset (`Stats`, one per tile): the pictures in
/// flight, which bound how many the app builds ahead.
#[derive(Default)]
pub(crate) struct Flow {
    pub(crate) built_total: AtomicU64,
    pub(crate) decoded_tiles: AtomicU64,
    /// Pictures that never reach the headset (an encode that failed): out of flight.
    pub(crate) lost: AtomicU64,
    /// This window's (the stats line): the headset's poses that came for the app, the frames
    /// it built.
    pub(crate) poses: AtomicU64,
    pub(crate) built: AtomicU64,
    pub(crate) skipped: AtomicU64,
    /// Pictures with no eyes to be seen from (this window): [`picture_eyes`].
    pub(crate) no_eyes: AtomicU64,
    /// The app asked for room and the headset had too many pictures in flight (this window).
    pub(crate) held: AtomicU64,
    /// Eye pictures the encoders finished (an access unit, a drop or an error; with the
    /// pictures that never reached them): what waits for an encoder (`backlog`).
    pub(crate) encoded_tiles: AtomicU64,
    /// Eye pictures the encoder dropped (it gave no access unit: VideoToolbox's real-time mode
    /// under load), this window: the headset never gets that picture whole.
    pub(crate) dropped: AtomicU64,
    /// Per picture built this window: how long its pose waited for the app to ask for it (µs;
    /// it came while the app still built the frame before).
    pub(crate) app_wait_us: Mutex<Vec<u32>>,
}

impl Flow {
    /// Pictures built and not yet encoded (both eyes): one encoding and at most one more
    /// waiting keeps the encoders busy; more only waits (a slow encoder queued pictures 53 ms
    /// before their encode on an M4: each one that much further from its pose).
    pub(crate) fn backlog(&self, tiles: u32) -> u64 {
        self.built_total.load(Ordering::Acquire).saturating_sub(self.encoded_tiles.load(Ordering::Acquire) / tiles.max(1) as u64)
    }

    pub(crate) fn in_flight(&self, tiles: u32) -> u64 {
        let done = self.decoded_tiles.load(Ordering::Acquire) / tiles.max(1) as u64 + self.lost.load(Ordering::Acquire);
        self.built_total.load(Ordering::Acquire).saturating_sub(done)
    }

    /// A picture built that goes no further ([`picture_eyes`]): out of flight, and no encoder
    /// waits for it.
    fn no_eyes(&self, tiles: u32) {
        self.no_eyes.fetch_add(1, Ordering::AcqRel);
        self.lost.fetch_add(1, Ordering::AcqRel);
        self.encoded_tiles.fetch_add(tiles as u64, Ordering::AcqRel);
    }
}

/// A picture that goes no further: out of flight, its slot free, and then the sink woken. No
/// encoder or headset will answer for this one, and the app's next pose waits on such an answer.
fn let_go(flow: &Flow, tiles: u32, held: Box<dyn Send + Sync>, wake: impl FnOnce()) {
    flow.no_eyes(tiles);
    drop(held);
    wake();
}

/// The pose a picture was drawn for, taken from those kept; the older ones were never drawn and
/// go with it.
fn take_pose_of(poses: &mut VecDeque<(u64, Arrived)>, display_ns: u64) -> Option<Arrived> {
    let i = poses.iter().position(|(d, _)| *d == display_ns)?;
    let pose = poses.remove(i).unwrap().1;
    poses.drain(..i);
    Some(pose)
}

/// What a picture is seen from: (pose sequence, eyes, its render time in µs from the pose's
/// arrival). Its own render eyes, else those of the pose it was drawn for. None for a picture
/// drawn again for a pose an earlier picture already took: its head would carry no eyes, and the
/// headset showing one late (with its own eyes, not warped) has every frame refused for the
/// empty pose, its frame loop then stalls and the app's poses and pictures slow to match.
fn picture_eyes(rendered: Option<RenderedPose>, pose: Option<&Arrived>, built_at: Instant) -> Option<(u64, [XrRenderEye; 2], u32)> {
    let (seq, eyes, at) = match (rendered, pose) {
        (Some(r), _) => (r.seq, r.eyes, r.arrived),
        (None, Some(a)) => (a.seq, a.eyes, a.at),
        (None, None) => return None,
    };
    Some((seq, eyes, built_at.saturating_duration_since(at).as_micros() as u32))
}

/// The stream's rate (all tiles, kbit/s): the one rate controller of every Desk Link stream
/// (`crate::rate`), its link-bound signal the link's own delay: a tile's round trip from its
/// last byte written to the headset's report that it took it, past what it is when idle.
///
/// (Not the app waiting for pictures in flight: a picture is in flight from its build until the
/// headset reports it, longer than a display period on a healthy link, so the app waited for a
/// fifth of its frames with the network idle and the rate fell for nothing: 22 -> 15 Mbit/s on
/// 5 ms Wi-Fi.)
pub(crate) struct Rate {
    inner: Mutex<RateInner>,
    shared: Arc<ServerShared>,
}

struct RateInner {
    control: RateControl,
    trip: LinkTrip,
    /// The app's bitrate (`set_bitrate_kbps`) as last seen: a change applies at once.
    cap: u32,
}

impl Rate {
    fn new(shared: Arc<ServerShared>) -> Rate {
        let cap = shared.bitrate_kbps.load(Ordering::Acquire);
        Rate { inner: Mutex::new(RateInner { control: RateControl::with_floor(MIN_KBPS), trip: LinkTrip::default(), cap }), shared }
    }

    /// The rate now: the app's bitrate the moment it changes, else the controller's (never
    /// above it).
    pub(crate) fn current(&self) -> u32 {
        let cap = self.shared.bitrate_kbps.load(Ordering::Acquire);
        let mut r = self.inner.lock().unwrap();
        if r.cap != cap {
            r.cap = cap;
            r.control.restart();
        }
        r.control.kbps(cap)
    }

    pub(crate) fn base_ms(&self) -> f64 {
        self.inner.lock().unwrap().trip.base_ms()
    }

    /// One second: a tile's median and lowest round trip (ms), the pictures and rate sent.
    pub(crate) fn second(&self, trip_ms: f64, trip_min_ms: f64, pictures: u64, sent_kbps: u64, fps: u32, size: [u32; 2]) {
        let log = &self.shared.log;
        let before = self.current();
        let mut r = self.inner.lock().unwrap();
        let link_queued = LinkTrip::ours(r.trip.queued(trip_ms, trip_min_ms), sent_kbps, before);
        let cap = r.cap;
        // one fixed shape (a stream per eye): no encoder effort steps, no decode bands
        let s = Second { fps, pictures, sent_kbps, size, max_effort: effort::SHARP, link_queued, ..Second::default() };
        r.control.second(&s, cap);
        let (kbps, limit) = (r.control.kbps(cap), r.control.limit(cap));
        if kbps != before && r.control.note_due(kbps, limit) {
            let base = r.trip.base_ms();
            log(&format!("xr host: rate {:.0} -> {:.0} Mbit/s ({})", before as f64 / 1000.0, kbps as f64 / 1000.0,
                if link_queued { format!("the link queues: trip {trip_ms:.1} ms, idle {base:.1}") } else { "the link keeps up".to_string() }));
        }
    }
}

/// The lowest the rate controller goes (kbit/s, all tiles).
pub(crate) const MIN_KBPS: u32 = 15_000;
/// Pictures built and not yet decoded by the headset before the app waits (`room`):
/// `MAKEPAD_XR_REMOTE_IN_FLIGHT`, default 4 (a picture's build to its decode takes ~2 display
/// periods at 90 Hz: with 2 in flight the app waited for most frames and built 64 of 90/s; at
/// the 120% eye picture, 3 still held the app back 40-60 times a second and it built 81-86/s:
/// each missing picture a repeat on the headset).
fn in_flight_limit() -> u64 {
    std::env::var("MAKEPAD_XR_REMOTE_IN_FLIGHT").ok().and_then(|v| v.parse().ok()).unwrap_or(4u64).clamp(1, 8)
}

/// A tile of a picture, as its encoder's answer carries it: the picture's head, when it was built.
#[derive(Clone, Copy)]
struct XrTile {
    head: VideoHead,
    built_at: Instant,
}

/// The session's pose datagrams: a thread reads them (newest wins: an older or repeated one is
/// dropped, a gap counted as lost) and hands each to the app as a pose over the link would. The
/// headset sends one per display frame, for a display time on its own grid: the app renders each
/// as it comes, so its pictures keep the display's spacing and phase (no clock of its own).
pub(crate) struct Uplink {
    pub(crate) port: u16,
    pub(crate) key: [u8; 32],
    stop: Arc<AtomicBool>,
    pub(crate) socket: std::net::UdpSocket,
    /// Where the poses come from (the headset's socket): where its audio goes.
    pub(crate) from: Arc<Mutex<Option<SocketAddr>>>,
}

impl Uplink {
    pub(crate) fn open(peer: std::net::IpAddr, core: &Core) -> Result<Uplink, String> {
        let log = core.shared.log.clone();
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("uplink socket: {e}"))?;
        socket.set_read_timeout(Some(Duration::from_millis(100))).map_err(|e| e.to_string())?;
        let port = socket.local_addr().map_err(|e| e.to_string())?.port();
        let mut key = [0u8; 32];
        net_tls::os_random(&mut key).map_err(|e| format!("uplink key: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let (poses, ending, stop2, shared) = (core.poses.clone(), core.ending.clone(), stop.clone(), core.shared.clone());
        let from_at: Arc<Mutex<Option<SocketAddr>>> = Arc::default();
        let (from2, own) = (from_at.clone(), socket.try_clone().map_err(|e| format!("uplink socket: {e}"))?);
        std::thread::Builder::new()
            .name("xr-host-uplink".into())
            .spawn(move || {
                let mut buf = vec![0u8; 4096];
                let (mut last_seq, mut got, mut lost, mut stale, mut bad) = (0u64, 0u64, 0u64, 0u64, 0u64);
                let (mut last_at, mut gaps, mut window) = (None::<Instant>, Vec::<u32>::new(), Instant::now());
                while !stop2.load(Ordering::Acquire) && !ending.load(Ordering::Acquire) {
                    if window.elapsed() >= Duration::from_secs(5) {
                        let n = gaps.len().max(1) as f64;
                        let mean = gaps.iter().map(|v| *v as f64).sum::<f64>() / n;
                        let dev = (gaps.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / n).sqrt();
                        log(&format!("xr host: uplink {:.0} poses/s, interval {:.2} ms ±{:.2} (max {:.1}), {lost} lost, {stale} out of order, {bad} refused",
                            got as f64 / window.elapsed().as_secs_f64(), mean / 1000.0, dev / 1000.0, gaps.iter().copied().max().unwrap_or(0) as f64 / 1000.0));
                        (got, lost, stale, bad) = (0, 0, 0, 0);
                        gaps.clear();
                        window = Instant::now();
                    }
                    let (n, from) = match socket.recv_from(&mut buf) {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    let Some((seq, sample_ns, display_ns, frame)) = (from.ip() == peer).then(|| pose_datagram::open(&key, &buf[..n])).flatten() else {
                        bad += 1;
                        continue;
                    };
                    if seq <= last_seq {
                        stale += 1;
                        continue;
                    }
                    {
                        let mut f = from2.lock().unwrap();
                        if *f != Some(from) {
                            log(&format!("xr host: the headset's poses come from {from}: its audio goes there"));
                            *f = Some(from);
                        }
                    }
                    if last_seq != 0 {
                        lost += seq - last_seq - 1;
                    }
                    last_seq = seq;
                    got += 1;
                    let now = Instant::now();
                    if let Some(l) = last_at {
                        gaps.push(now.duration_since(l).as_micros() as u32);
                    }
                    last_at = Some(now);
                    take_pose(&shared, &poses, seq, sample_ns, display_ns, frame);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Uplink { port, key, stop, socket: own, from: from_at })
    }

    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// How long the headset's menu stays down to be a hold, not a press (ns).
const MENU_HOLD_NS: u64 = 600_000_000;

/// The headset's menu: the left controller's menu button, or a tracked left hand's menu gesture.
#[derive(Default)]
pub(crate) struct MenuPress {
    /// When it went down (its pose's sample time), and whether the button was part of it.
    down_at: Option<u64>,
    button: bool,
    /// Down [`MENU_HOLD_NS`]: a hold, until its release.
    held: bool,
}

impl MenuPress {
    /// One pose (`at`: its sample time): whether the menu is held now. The button's short press
    /// turns the stream on and off at its release; a hold turns it on.
    fn step(&mut self, shared: &ServerShared, at: u64, button: bool, gesture: bool) -> bool {
        match (button || gesture, self.down_at) {
            (true, None) => *self = MenuPress { down_at: Some(at), button, held: false },
            (true, Some(from)) => {
                self.button |= button;
                if !self.held && at.saturating_sub(from) >= MENU_HOLD_NS {
                    self.held = true;
                    shared.set_streaming(true);
                    (shared.log)("xr host: the headset's menu held");
                }
            }
            (false, Some(from)) => {
                // (a release seen only after the hold's time, a gap in the poses, is no press)
                if self.button && !self.held && at.saturating_sub(from) < MENU_HOLD_NS {
                    shared.set_streaming(!shared.streaming());
                    (shared.log)(if shared.streaming() { "xr host: the headset's menu button resumed the stream" } else { "xr host: the headset's menu button paused the stream" });
                }
                *self = MenuPress::default();
            }
            (false, None) => {}
        }
        self.held
    }
}

/// A pose from the headset: to the app's XR window, and kept to match its picture.
fn take_pose(shared: &ServerShared, poses: &Mutex<VecDeque<(u64, Arrived)>>, seq: u64, sample_ns: u64, display_ns: u64, frame: &[u8]) -> bool {
    // the left controller's menu (unless the app took the button over,
    // `set_menu_toggles_streaming`): its button never reaches the app; a short press turns the
    // stream on and off, a hold reaches the app as HOLD_MENU on the left controller
    let mut masked = None;
    if shared.menu_toggles.load(Ordering::Acquire) {
        use makepad_hal::{buffers::{read_items, Wire}, xr_controller, xr_hand, XrFrame};
        if let Some(mut f) = read_items::<XrFrame>(frame, 1).next() {
            let gesture = f.hands[0].flags & xr_hand::MENU_PRESSED != 0;
            let left = &mut f.controllers[0];
            let button = left.buttons & xr_controller::ACTIVE != 0 && left.buttons & xr_controller::CLICK_MENU != 0;
            let held = shared.menu.lock().unwrap().step(shared, sample_ns, button, gesture);
            if left.buttons & xr_controller::CLICK_MENU != 0 || held {
                left.buttons &= !xr_controller::CLICK_MENU;
                if held {
                    left.buttons |= xr_controller::HOLD_MENU;
                }
                let mut bytes = Vec::with_capacity(frame.len());
                f.write(&mut bytes);
                masked = Some(bytes);
            }
        }
    }
    // streaming off: no frame drawn for it
    if !shared.streaming() {
        return true;
    }
    shared.pose_ns.store(makepad_hal::clock_ns(), Ordering::Release);
    shared.note_pose();
    let frame = masked.as_deref().unwrap_or(frame);
    if shared.external.load(Ordering::Acquire) {
        let Some(frame) = makepad_hal::buffers::read_items::<makepad_hal::XrFrame>(frame, 1).next() else { return false };
        let snapshot = Arc::new(super::external::TrackingSnapshot { seq, sample_ns, display_ns,
            received_ns: makepad_hal::clock_ns(), frame, arrived: Instant::now() });
        let mut latest = shared.tracking.lock().unwrap();
        if latest.as_ref().is_some_and(|p| p.seq >= seq) { return true; }
        *latest = Some(snapshot);
        drop(latest);
        shared.wake();
        return true;
    }
    {
        let mut poses = poses.lock().unwrap();
        poses.push_back((display_ns, Arrived { at: Instant::now(), seq, eyes: render_eyes(frame) }));
        // poses never drawn (the newest one wins in the HAL): the oldest go
        while poses.len() > 1024 {
            poses.pop_front();
        }
    }
    hal::hal_xr_remote_pose(seq, display_ns, frame)
}

/// A pose that came: when, the eyes it gives, its sequence number.
struct Arrived {
    at: Instant,
    seq: u64,
    eyes: [XrRenderEye; 2],
}

/// The live session's encode side (also the selftest's): the eye encoders and their threads.
pub(crate) struct Core {
    pub(crate) shared: Arc<ServerShared>,
    pub(crate) size: [u32; 2],
    pub(crate) tiles: u32,
    pub(crate) codec: u8,
    pub(crate) description: String,
    pub(crate) flow: Arc<Flow>,
    pub(crate) rate: Arc<Rate>,
    pub(crate) out: Receiver<Out>,
    pub(crate) keyframe: Arc<[AtomicBool; 2]>,
    /// The poses asked for, to match their pictures.
    poses: Arc<Mutex<VecDeque<(u64, Arrived)>>>,
    pub(crate) ending: Arc<AtomicBool>,
    /// The display rate the session's encoders are made for (Hz).
    pub(crate) fps: u32,
    /// The eye size the app renders at, and how its columns are foveated into the picture.
    pub(crate) eye: [u32; 2],
    pub(crate) fovea: Fovea,
}

/// The session the app's pictures go to.
pub(crate) struct Live {
    pub(crate) eye: [u32; 2],
    pub(crate) refresh_mhz: u32,
    /// The picture's foveation and the eye column's encoded size (`Fovea::encoded`).
    pub(crate) fovea: Fovea,
    pub(crate) encoded: [u32; 2],
    pub(crate) tx: SyncSender<Picture>,
    pub(crate) flow: Arc<Flow>,
    pub(crate) tiles: u32,
    pub(crate) limit: u64,
}

impl Core {
    pub(crate) fn start(shared: Arc<ServerShared>, eye: [u32; 2], fps: u32, codec: StreamVideoCodec) -> Result<Core, String> {
        let log = shared.log.clone();
        let fovea = fovea_setting();
        let encoded = fovea.encoded(eye);
        let size = picture_size(encoded);
        let rate = Arc::new(Rate::new(shared.clone()));
        let kbps = rate.current();
        // a stream per eye on its own engine when the GPU has two
        let tiles = if VideoStreamEncoder::caps().engines >= 2 { 2 } else { 1 };
        let flow = Arc::new(Flow::default());
        let keyframe: Arc<[AtomicBool; 2]> = Arc::new([AtomicBool::new(true), AtomicBool::new(true)]);
        let ending = Arc::new(AtomicBool::new(false));
        let poses: Arc<Mutex<VecDeque<(u64, Arrived)>>> = Arc::default();

        let (out_tx, out) = channel::<Out>();
        let codec_byte = if codec == StreamVideoCodec::Hevc { CODEC_HEVC } else { CODEC_H264 };
        // the fastest encode (latency first), a constant rate whose pictures fit the link
        // (`MAKEPAD_XR_REMOTE_SHARP=1`: the encoder's best pictures per bit, more encode time)
        let sharp = std::env::var("MAKEPAD_XR_REMOTE_SHARP").as_deref() == Ok("1");
        let w = size[0] / tiles;
        let specs = (0..tiles)
            .map(|t| TileSpec {
                options: VideoStreamEncoderOptions { codec, width: w, height: size[1], fps, bitrate_kbps: kbps / tiles, keyint: 0, low_latency: true, full_chroma: false },
                // a picture per tile: each tile's whole surface
                tile: StreamTile { engine: t, x0: if super::nv12_planes() { 0 } else { t * w }, y0: 0, source_height: size[1] },
                tuning: StreamTuning { quality: 0, constant_rate: true, buffer_pictures: 1, sharp },
            })
            .collect();
        // an eye's access units to the session's TLS thread as they come
        let sink: TileSink<XrTile> = {
            let (out_tx, keyframe, flow, log, shared) = (Mutex::new(out_tx.clone()), keyframe.clone(), flow.clone(), log.clone(), shared.clone());
            Arc::new(move |d: TileDone<XrTile>| {
                let t = d.tile;
                flow.encoded_tiles.fetch_add(1, Ordering::AcqRel);
                let packets = match d.packets {
                    Ok(p) => p,
                    Err(e) => {
                        log(&format!("xr host: eye {t}: encode: {e}"));
                        flow.lost.fetch_add(1, Ordering::AcqRel);
                        keyframe[t].store(true, Ordering::Release);
                        shared.wake_sink();
                        return;
                    }
                };
                // the encoder dropped the picture (no access unit): nothing goes out, and the
                // flow counts the eye as taken (the headset never reports it: without this each
                // dropped eye held half a picture in flight for good and the app built ever less)
                if packets.is_empty() {
                    flow.dropped.fetch_add(1, Ordering::AcqRel);
                    flow.decoded_tiles.fetch_add(1, Ordering::AcqRel);
                    shared.wake_sink();
                    return;
                }
                // from the build: the wait for the frame's GPU work, the column's copy, its encode
                let encode = Duration::from_micros(d.encode_us as u64);
                let mut head = d.tag.head;
                head.tile = t as u8;
                head.stages[stage::GPU] = d.tag.built_at.elapsed().saturating_sub(encode).as_micros() as u32;
                head.stages[stage::ENCODE] = d.encode_us;
                let out_tx = out_tx.lock().unwrap();
                for p in packets {
                    head.key = p.is_key;
                    if out_tx.send(Out::Video(head, p.data, Instant::now())).is_err() {
                        return;
                    }
                }
                shared.wake_sink();
            })
        };
        let encoders = Arc::new(TileEncoders::open("xr-host-encode", specs, sink)?);
        let description = encoders.description(0).to_string();
        // the live tuning file, read once a second off the encode path
        {
            let (encoders, ending, log) = (encoders.clone(), ending.clone(), log.clone());
            std::thread::Builder::new()
                .name("xr-host-tune".into())
                .spawn(move || {
                    let mut tuned = Tune::default();
                    while !ending.load(Ordering::Acquire) {
                        let now = tune();
                        if now != tuned {
                            log(&format!("xr host: tuning: max QP {}", now.max_qp));
                            encoders.set_max_qp(now.max_qp);
                            tuned = now;
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        // the app's pictures: matched to their poses, fanned out to the eyes
        let (tx, rx) = sync_channel::<Picture>(4);
        {
            let (poses, rate, ending, keyframe, log, flow) = (poses.clone(), rate.clone(), ending.clone(), keyframe.clone(), log.clone(), flow.clone());
            let dump = std::env::var_os("MAKEPAD_XR_REMOTE_DUMP");
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("xr-host-pictures".into())
                .spawn(move || {
                    let mut pts = 0i64;
                    while !ending.load(Ordering::Acquire) {
                        let Ok(p) = rx.recv_timeout(Duration::from_millis(100)) else { continue };
                        let pose = take_pose_of(&mut poses.lock().unwrap(), p.display_ns);
                        // of the render: the pose waiting for the app to ask for it
                        if let (Some(a), Some(asked)) = (&pose, p.asked) {
                            let mut waits = flow.app_wait_us.lock().unwrap();
                            if waits.len() < 4096 {
                                waits.push(asked.saturating_duration_since(a.at).as_micros() as u32);
                            }
                        }
                        let Some((seq, eyes, render_us)) = picture_eyes(p.rendered, pose.as_ref(), p.built_at) else {
                            // it and its hold on the slot go; the next pose draws what it showed
                            let_go(&flow, tiles, p.held, || shared.wake_sink());
                            continue;
                        };
                        if let Some(dir) = &dump {
                            dump_picture(dir, &p, size, &log);
                        }
                        let head = VideoHead { seq, tile: 0, key: false, display_ns: p.display_ns, eyes, focus_m: p.focus_m, stages: [render_us, 0, 0, 0], size, codec: codec_byte };
                        let held: Arc<dyn Send + Sync> = Arc::new(p.held);
                        let kbps = rate.current();
                        pts += 1;
                        // each eye's encode waits for the same frame's completion
                        for t in 0..encoders.len() {
                            let key = keyframe[t].swap(false, Ordering::AcqRel);
                            let surface = p.surfaces.get(t).unwrap_or(&p.surfaces[0]).clone();
                            let work = TileWork { surface, ready: p.ready.clone(), held: held.clone(), kbps: (kbps / tiles).max(1), key, refine: false, pts_100ns: pts * 138_888, tag: XrTile { head, built_at: p.built_at } };
                            encoders.push(t, work);
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        if !fovea.is_off() {
            log(&format!("xr host: foveated encode: eyes of {}x{} as {}x{} ({:.0}% of the pixels)", eye[0], eye[1], encoded[0], encoded[1], (encoded[0] * encoded[1]) as f64 * 100.0 / (eye[0] * eye[1]).max(1) as f64));
        }
        *shared.live.lock().unwrap() = Some(Live { eye, refresh_mhz: fps * 1000, fovea, encoded, tx, flow: flow.clone(), tiles, limit: in_flight_limit() });
        shared.wake();
        Ok(Core { shared, size, tiles, codec: codec_byte, description, flow, rate, out, keyframe, poses, ending, fps, eye, fovea })
    }

    /// A pose from the headset: to the app's XR window, and kept to match its picture.
    pub(crate) fn pose(&self, seq: u64, sample_ns: u64, display_ns: u64, frame: &[u8]) -> bool {
        take_pose(&self.shared, &self.poses, seq, sample_ns, display_ns, frame)
    }

    pub(crate) fn keyframe(&self) {
        for k in self.keyframe.iter() {
            k.store(true, Ordering::Release);
        }
    }

    /// Eye pictures out of flight (decoded by the headset, or dropped on the way): their slots
    /// free, the sink woken for them.
    pub(crate) fn tiles_out(&self, n: u64) {
        if n > 0 {
            self.flow.decoded_tiles.fetch_add(n, Ordering::AcqRel);
            self.shared.wake_sink();
        }
    }

    pub(crate) fn stop(&self) {
        self.ending.store(true, Ordering::Release);
        let mut live = self.shared.live.lock().unwrap();
        if live.as_ref().is_some_and(|l| Arc::ptr_eq(&l.flow, &self.flow)) {
            *live = None;
            *self.shared.tracking.lock().unwrap() = None;
        }
    }
}

/// The eyes of an `XrFrame` (wire form).
pub(crate) fn render_eyes(frame: &[u8]) -> [XrRenderEye; 2] {
    let Some(f) = makepad_hal::buffers::read_items::<makepad_hal::XrFrame>(frame, 1).next() else { return [XrRenderEye::default(); 2] };
    f.eyes.map(|e| XrRenderEye { orientation: e.pose.orientation, position: e.pose.position, fov: e.fov })
}

/// The picture for eye columns of `encoded` pixels (`Fovea::encoded`): both side by side, each its
/// colour over a band half its height (coverage and inverse depth at half resolution; `crate::xr`).
pub fn picture_size(encoded: [u32; 2]) -> [u32; 2] {
    [encoded[0] * 2, encoded[1] * 3 / 2]
}


#[cfg(test)]
mod tests {
    use super::*;

    fn eyes(w: f32) -> [XrRenderEye; 2] {
        [XrRenderEye { orientation: [0.0, 0.0, 0.0, w], ..Default::default() }; 2]
    }

    fn poses(display_ns: &[u64]) -> VecDeque<(u64, Arrived)> {
        display_ns.iter().map(|d| (*d, Arrived { at: Instant::now(), seq: *d + 1, eyes: eyes(1.0) })).collect()
    }

    #[test]
    fn a_picture_takes_its_poses_eyes_and_the_older_poses_go() {
        let mut kept = poses(&[10, 20, 30]);
        let pose = take_pose_of(&mut kept, 20).unwrap();
        assert_eq!(pose.seq, 21);
        assert_eq!(kept.iter().map(|(d, _)| *d).collect::<Vec<_>>(), [30]);
        let (seq, shown, _) = picture_eyes(None, Some(&pose), Instant::now()).unwrap();
        assert_eq!(seq, 21);
        assert_eq!(shown, eyes(1.0));
    }

    #[test]
    fn a_picture_drawn_again_for_a_pose_already_taken_has_no_eyes_to_go_out_with() {
        let mut kept = poses(&[10]);
        assert!(take_pose_of(&mut kept, 10).is_some());
        let again = take_pose_of(&mut kept, 10);
        assert!(again.is_none());
        assert!(picture_eyes(None, again.as_ref(), Instant::now()).is_none());
    }

    #[test]
    fn a_picture_with_its_own_render_eyes_keeps_them() {
        let rendered = RenderedPose { seq: 7, eyes: eyes(0.5), arrived: Instant::now() };
        let (seq, shown, _) = picture_eyes(Some(rendered), None, Instant::now()).unwrap();
        assert_eq!((seq, shown), (7, eyes(0.5)));
    }

    #[test]
    fn a_picture_with_no_eyes_leaves_nothing_in_flight_or_waiting_for_an_encoder() {
        let flow = Flow::default();
        flow.built_total.fetch_add(1, Ordering::AcqRel);
        assert_eq!((flow.in_flight(2), flow.backlog(2)), (1, 1));
        flow.no_eyes(2);
        assert_eq!((flow.in_flight(2), flow.backlog(2)), (0, 0));
        assert_eq!(flow.no_eyes.load(Ordering::Acquire), 1);
    }

    #[test]
    fn a_picture_with_no_eyes_frees_its_slot_and_then_wakes_the_app() {
        struct Slot(Arc<AtomicBool>);
        impl Drop for Slot {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let busy = Arc::new(AtomicBool::new(true));
        let flow = Flow::default();
        flow.built_total.fetch_add(1, Ordering::AcqRel);
        let mut woken_with_a_free_slot = None;
        let_go(&flow, 2, Box::new(Slot(busy.clone())), || woken_with_a_free_slot = Some(!busy.load(Ordering::Acquire)));
        assert_eq!(woken_with_a_free_slot, Some(true), "nothing else wakes the app for this picture");
        assert_eq!((flow.in_flight(2), flow.backlog(2)), (0, 0));
    }
}
