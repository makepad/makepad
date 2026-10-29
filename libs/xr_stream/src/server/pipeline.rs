//! The server's picture pipeline for one session: the headset's poses to the app's XR display
//! (and kept to match their pictures), the app's pictures matched to their poses and fanned out
//! to the eye encoders (the tile fan-out and rate controller every Desk Link stream shares,
//! `makepad_desk_link::{tiles, rate}`), the flow that bounds what is in flight.

use super::debug::{dump_picture, tune, Tune};
use super::foveation::{foveation, foveation_peak};
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
    /// What the view centres on (m), for reprojection without per-pixel depth.
    pub focus_m: f32,
}

/// Pictures built and their eyes decoded by the headset (`Stats`, one per tile): the pictures in
/// flight, which bound how many the app builds ahead.
#[derive(Default)]
pub(crate) struct Flow {
    pub(crate) built_total: AtomicU64,
    pub(crate) decoded_tiles: AtomicU64,
    /// Pictures that never reach the headset (an encode that failed): out of flight.
    pub(crate) lost: AtomicU64,
    /// This window's (the stats line).
    pub(crate) built: AtomicU64,
    pub(crate) skipped: AtomicU64,
    /// The app asked for room and the headset had too many pictures in flight (this window).
    pub(crate) held: AtomicU64,
}

impl Flow {
    pub(crate) fn in_flight(&self, tiles: u32) -> u64 {
        let done = self.decoded_tiles.load(Ordering::Acquire) / tiles.max(1) as u64 + self.lost.load(Ordering::Acquire);
        self.built_total.load(Ordering::Acquire).saturating_sub(done)
    }
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
        let link_queued = r.trip.queued(trip_ms, trip_min_ms);
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
                    let Some((seq, _sample_ns, display_ns, frame)) = (from.ip() == peer).then(|| pose_datagram::open(&key, &buf[..n])).flatten() else {
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
                    take_pose(&shared, &poses, seq, display_ns, frame);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Uplink { port, key, stop, socket: own, from: from_at })
    }

    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// A pose from the headset: to the app's XR window, and kept to match its picture.
fn take_pose(shared: &ServerShared, poses: &Mutex<VecDeque<(u64, Arrived)>>, seq: u64, display_ns: u64, frame: &[u8]) -> bool {
    // the left controller's menu button turns the stream on and off (unless the app took that
    // over, `set_menu_toggles_streaming`): never seen by the app then
    let mut masked = None;
    if shared.menu_toggles.load(Ordering::Acquire) {
        use makepad_hal::{buffers::{read_items, Wire}, xr_controller, XrFrame};
        if let Some(mut f) = read_items::<XrFrame>(frame, 1).next() {
            let left = &mut f.controllers[0];
            let down = left.buttons & xr_controller::ACTIVE != 0 && left.buttons & xr_controller::CLICK_MENU != 0;
            if down && !shared.menu_down.swap(true, Ordering::AcqRel) {
                shared.set_streaming(!shared.streaming());
            } else if !down {
                shared.menu_down.store(false, Ordering::Release);
            }
            if left.buttons & xr_controller::CLICK_MENU != 0 {
                left.buttons &= !xr_controller::CLICK_MENU;
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
    let frame = masked.as_deref().unwrap_or(frame);
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
}

/// The session the app's pictures go to.
pub(crate) struct Live {
    pub(crate) eye: [u32; 2],
    pub(crate) tx: SyncSender<Picture>,
    pub(crate) flow: Arc<Flow>,
    pub(crate) tiles: u32,
    pub(crate) limit: u64,
}

impl Core {
    pub(crate) fn start(shared: Arc<ServerShared>, eye: [u32; 2], fps: u32, codec: StreamVideoCodec) -> Result<Core, String> {
        let log = shared.log.clone();
        let size = picture_size(eye);
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
                let packets = match d.packets {
                    Ok(p) => p,
                    Err(e) => {
                        log(&format!("xr host: eye {t}: encode: {e}"));
                        flow.lost.fetch_add(1, Ordering::AcqRel);
                        keyframe[t].store(true, Ordering::Release);
                        return;
                    }
                };
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
                shared.wake();
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
            let (poses, rate, ending, keyframe, log) = (poses.clone(), rate.clone(), ending.clone(), keyframe.clone(), log.clone());
            let dump = std::env::var_os("MAKEPAD_XR_REMOTE_DUMP");
            std::thread::Builder::new()
                .name("xr-host-pictures".into())
                .spawn(move || {
                    let (mut pts, mut foveated) = (0i64, false);
                    while !ending.load(Ordering::Acquire) {
                        let Ok(p) = rx.recv_timeout(Duration::from_millis(100)) else { continue };
                        let pose = {
                            let mut poses = poses.lock().unwrap();
                            let found = poses.iter().position(|(d, _)| *d == p.display_ns);
                            found.map(|i| {
                                let pose = poses.remove(i).unwrap().1;
                                // older poses were never drawn: gone
                                poses.drain(..i);
                                pose
                            })
                        };
                        let (seq, eyes, render_us) = match &pose {
                            Some(a) => (a.seq, a.eyes, p.built_at.saturating_duration_since(a.at).as_micros() as u32),
                            None => (0, [XrRenderEye::default(); 2], 0),
                        };
                        // each eye's foveation, once its field of view is known (its first picture)
                        if !foveated && eyes.iter().all(|e| e.fov != [0.0; 4]) {
                            foveated = true;
                            if let Some(peak) = foveation_peak() {
                                for t in 0..encoders.len() {
                                    let Some(grid) = encoders.qp_grid(t) else { continue };
                                    let (_, tile_size) = encoders.tile(t);
                                    encoders.set_qp_map(t, foveation(grid, tile_size, eyes[t].fov, peak));
                                    log(&format!("xr host: eye {t}: foveated encode ({}x{} QP regions of {}x{} px, +{peak} QP at the edge)", grid.cols, grid.rows, grid.texel[0], grid.texel[1]));
                                }
                            }
                        }
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
        *shared.live.lock().unwrap() = Some(Live { eye, tx, flow: flow.clone(), tiles, limit: in_flight_limit() });
        shared.wake();
        Ok(Core { shared, size, tiles, codec: codec_byte, description, flow, rate, out, keyframe, poses, ending, fps })
    }

    /// A pose from the headset: to the app's XR window, and kept to match its picture.
    pub(crate) fn pose(&self, seq: u64, display_ns: u64, frame: &[u8]) -> bool {
        take_pose(&self.shared, &self.poses, seq, display_ns, frame)
    }

    pub(crate) fn keyframe(&self) {
        for k in self.keyframe.iter() {
            k.store(true, Ordering::Release);
        }
    }

    /// The headset decoded one eye of a picture.
    pub(crate) fn tile_decoded(&self) {
        self.flow.decoded_tiles.fetch_add(1, Ordering::AcqRel);
        self.shared.wake();
    }

    pub(crate) fn stop(&self) {
        self.ending.store(true, Ordering::Release);
        let mut live = self.shared.live.lock().unwrap();
        if live.as_ref().is_some_and(|l| Arc::ptr_eq(&l.flow, &self.flow)) {
            *live = None;
        }
    }
}

/// The eyes of an `XrFrame` (wire form).
pub(crate) fn render_eyes(frame: &[u8]) -> [XrRenderEye; 2] {
    let Some(f) = makepad_hal::buffers::read_items::<makepad_hal::XrFrame>(frame, 1).next() else { return [XrRenderEye::default(); 2] };
    f.eyes.map(|e| XrRenderEye { orientation: e.pose.orientation, position: e.pose.position, fov: e.fov })
}

/// The picture for eyes of `eye` pixels: both eye columns side by side, each its colour over a
/// band half its height (coverage and inverse depth at half resolution; `crate::xr`).
pub fn picture_size(eye: [u32; 2]) -> [u32; 2] {
    [eye[0] * 2, eye[1] * 3 / 2]
}

