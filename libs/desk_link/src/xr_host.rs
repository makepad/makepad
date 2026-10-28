//! The desktop end of Desk Link XR (`crate::xr`): an XR server in an app's own process that has
//! a remote headset slot (`MAKEPAD_XR_REMOTE=1`, `makepad_hal::xr_remote`). A headset connects
//! (fleet TLS; `Need::XrViewer`), says its eye size and refresh; its poses go to the app's XR
//! window as `XrFrame`s. The app renders each into its own textures and packs the picture on the
//! GPU (NV12 in a shared texture, `makepad_xr_stream`), then hands it here with [`submit`]: the
//! picture's eye columns are encoded at once on the GPU's encode engines (a stream per eye, the
//! texture read where it lies) and sent with the eyes of the pose it was drawn with. One headset
//! at a time.
//!
//! Pipelined and bounded: the app asks for the next pose as soon as it built a frame and has
//! [`room`] (a picture slot free, and fewer than `in_flight` pictures built and not yet
//! decoded by the headset); while there is none it waits, told by the wake ([`set_wake`]) when
//! there is again. So a slow link or decoder makes the server render less, never queue more.
//!
//! Threads: the listener; per session its TLS thread (reads poses and stats, writes pictures),
//! its picture thread (matches a picture to its pose and fans it out) and an encode thread per
//! eye. Nothing here runs on the app's UI thread.

use crate::server::{fleet_identity, lan_address};
use crate::xr::*;
use crate::{write_all, Inbox};
use makepad_hal::xr_remote as hal;
use makepad_network::fleet::id::{self, now_secs, Need};
use makepad_network::tls::TlsServer;
use makepad_video::{GpuCompletion, QpGrid, StreamTile, StreamTuning, StreamVideoCodec, SurfaceHandle, VideoStreamEncoder, VideoStreamEncoderOptions};
use std::collections::VecDeque;
use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// The room field's bricks from a headset: (voxel size m, chunk edge voxels, bricks).
pub type WorldSink = Arc<dyn Fn(f32, i32, Vec<WorldBrick>) + Send + Sync>;

pub struct XrHostConfig {
    /// The address to listen on (port 0: the OS picks one; headsets find it by the pairing
    /// probe, `crate::xr_pair`).
    pub addr: SocketAddr,
    /// The app's name (what a headset is told it links to) and where its pairing is kept (its
    /// data folder; None: not kept).
    pub app: String,
    pub pairing: Option<std::path::PathBuf>,
    /// Where a headset's room field goes (the app mirrors it for its physics).
    pub world: Option<WorldSink>,
    /// The TLS certificate's name.
    pub tls_name: String,
    pub log: Log,
}

pub struct XrHost {
    stop: Arc<AtomicBool>,
    pub addr: SocketAddr,
}

impl Drop for XrHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Whether this process can serve a remote headset (its HAL has the slot).
pub fn available() -> bool {
    hal::hal_xr_remote_enabled()
}

impl XrHost {
    pub fn start(config: XrHostConfig) -> Result<XrHost, String> {
        if !available() {
            return Err("xr host: this process has no remote headset slot (MAKEPAD_XR_REMOTE=1)".into());
        }
        let (identity, verifier, name) = fleet_identity(&config.tls_name).map_err(|why| format!("xr host: {why}"))?;
        let tls = Arc::new(TlsServer::new(identity).map_err(|e| format!("xr host: TLS server: {e}"))?);
        let stop = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicBool::new(false));
        let verifier = Arc::new(verifier);
        let log = config.log.clone();
        let (stop2, addr, world) = (stop.clone(), config.addr, config.world.clone());
        let pairing = Arc::new(Mutex::new(crate::xr_pair::ServerPairing::load(config.pairing.clone())));
        *PAIRING.lock().unwrap() = Some(pairing.clone());
        let (app, pairing2) = (config.app.clone(), pairing.clone());
        std::thread::Builder::new()
            .name("xr-host-listen".into())
            .spawn(move || {
                // the port may still be held a moment by a host this process just dropped (an app
                // restart): bind again until it is free
                let listener = loop {
                    if stop2.load(Ordering::Acquire) {
                        return;
                    }
                    match TcpListener::bind(addr).and_then(|l| l.set_nonblocking(true).map(|_| l)) {
                        Ok(l) => break l,
                        Err(e) => {
                            log(&format!("xr host: bind {addr}: {e}; trying again"));
                            std::thread::sleep(Duration::from_secs(1));
                        }
                    }
                };
                let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                log(&format!("xr host: {app} listens on port {port} as {name} (the admin or a paired headset), instance {:016x}", pairing2.lock().unwrap().instance));
                // headsets find this server by their probe (answered while paired, or for the
                // PIN typed here)
                let _responder = match crate::xr_pair::identity("pairing headsets") {
                    Ok((me, fleet)) => match crate::xr_pair::Responder::start(me, fleet, pairing2.clone(), app.clone(), port, log.clone()) {
                        Ok(r) => Some(r),
                        Err(e) => {
                            log(&format!("xr host: {e}"));
                            None
                        }
                    },
                    Err(e) => {
                        log(&format!("xr host: no pairing: {e}"));
                        None
                    }
                };
                while !stop2.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((tcp, peer)) => {
                            if !lan_address(peer.ip()) {
                                continue;
                            }
                            let ctx = Session { peer, tls: tls.clone(), verifier: verifier.clone(), log: log.clone(), stop: stop2.clone(), busy: busy.clone(), server: name.clone(), world: world.clone(), pairing: pairing2.clone() };
                            let _ = std::thread::Builder::new().name("xr-host-session".into()).spawn(move || ctx.run(tcp));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                        Err(e) => {
                            log(&format!("xr host: accept: {e}"));
                            std::thread::sleep(Duration::from_millis(200));
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(XrHost { stop, addr: config.addr })
    }
}

struct Session {
    world: Option<WorldSink>,
    peer: SocketAddr,
    tls: Arc<TlsServer>,
    verifier: Arc<id::Verifier>,
    log: Log,
    stop: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    server: String,
    pairing: Arc<Mutex<crate::xr_pair::ServerPairing>>,
}

impl Session {
    fn run(self, tcp: TcpStream) {
        let why = self.serve(tcp).unwrap_or_else(|e| e);
        (self.log)(&format!("xr host: session {}: {why}", self.peer));
    }

    fn serve(&self, tcp: TcpStream) -> Result<String, String> {
        tcp.set_nonblocking(false).map_err(|e| e.to_string())?;
        let _ = tcp.set_nodelay(true);
        tcp.set_read_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
        tcp.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
        let mut stream = self.tls.accept(tcp).map_err(|e| format!("TLS handshake: {e}"))?;
        let mut inbox = Inbox::default();
        let mut buf = vec![0u8; 256 << 10];
        let started = Instant::now();
        let (ty, payload) = loop {
            if let Some(m) = inbox.next().map_err(|e| e.to_string())? {
                break m;
            }
            match stream.read(&mut buf) {
                Ok(0) => return Err("closed before its hello".into()),
                Ok(n) => inbox.feed(&buf[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => return Err(format!("read: {e}")),
            }
            if started.elapsed() > Duration::from_secs(5) {
                return Err("no hello".into());
            }
        };
        let refuse = |stream: &mut makepad_network::tls::TlsStream, reason: String| -> Result<String, String> {
            let _ = write_all(stream, &XrServerMsg::Refused { reason: reason.clone() }.encode());
            stream.shutdown();
            Err(format!("refused: {reason}"))
        };
        let Ok(XrClientMsg::Hello { version, name, authorization, instance, eye, refresh_mhz, codecs: decodes }) = XrClientMsg::decode(ty, &payload) else {
            return refuse(&mut stream, "the first message must be a hello".into());
        };
        if version != XR_VERSION {
            return refuse(&mut stream, format!("XR protocol version {version}; this server speaks {XR_VERSION}"));
        }
        let peer = match self.verifier.check(&authorization, self.peer.ip(), now_secs()) {
            Ok(peer) => peer,
            Err(e) => return refuse(&mut stream, format!("fleet credential: {e}")),
        };
        if let Err(why) = id::require(&peer, Need::XrViewer) {
            return refuse(&mut stream, format!("may not view XR here: {why}"));
        }
        if self.busy.swap(true, Ordering::AcqRel) {
            return refuse(&mut stream, "another headset is connected".into());
        }
        // the headset the PIN typed here matched: paired from now on (kept)
        let paired = {
            let mut p = self.pairing.lock().unwrap();
            if p.matched.as_ref().is_some_and(|h| h.instance == instance) {
                p.paired = p.matched.take();
                p.save();
                (self.log)(&format!("xr host: paired with {name} ({instance:016x})"));
            }
            p.paired.as_ref().is_some_and(|h| h.instance == instance)
        };
        let result = self.session(&mut stream, inbox, buf, &name, &peer.name, instance, paired, eye, refresh_mhz, decodes);
        self.busy.store(false, Ordering::Release);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn session(&self, stream: &mut makepad_network::tls::TlsStream, mut inbox: Inbox, mut buf: Vec<u8>, name: &str, who: &str, instance: u64, paired: bool, eye: [u32; 2], refresh_mhz: u32, decodes: u32) -> Result<String, String> {
        let eye = [eye[0].clamp(64, 4096) & !3, eye[1].clamp(64, 4096) & !3];
        // H.265 when both ends have it
        let codec = if decodes & codecs::HEVC != 0 && VideoStreamEncoder::caps().hevc { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
        let fps = (refresh_mhz / 1000).clamp(30, 120);
        let core = Core::start(eye, fps, codec, self.log.clone())?;
        (self.log)(&format!("xr host: {who} ({name}) from {}: eyes {}x{} at {fps} Hz, {codec:?} {}x{} in {} tile(s) on {}", self.peer, eye[0], eye[1], core.size[0], core.size[1], core.tiles, core.description));
        hal::hal_xr_remote_attach(eye[0], eye[1], refresh_mhz);
        let session = {
            let mut b = [0u8; 8];
            let _ = makepad_network::tls::os_random(&mut b);
            u64::from_le_bytes(b)
        };
        let server_instance = self.pairing.lock().unwrap().instance;
        *LIVE_HEADSET.lock().unwrap() = Some((instance, name.to_string(), session));
        let welcome = XrServerMsg::Welcome { version: XR_VERSION, server: self.server.clone(), instance: server_instance, session, size: core.size, codec: core.codec, tiles: core.tiles as u8, encoder: core.description.clone() };
        write_all(stream, &welcome.encode()).map_err(|e| format!("write: {e}"))?;
        // the poses come as datagrams: a socket of the session's own, a key for it
        let uplink = Uplink::open(self.peer.ip(), &core, self.log.clone())?;
        // the app's game audio goes down from the same socket to where the poses come from
        let audio = crate::xr_audio::AudioOut::start(uplink.socket.try_clone().map_err(|e| format!("audio socket: {e}"))?, uplink.key, uplink.from.clone(), self.log.clone())?;
        write_all(stream, &XrServerMsg::Uplink { port: uplink.port, key: uplink.key.to_vec() }.encode()).map_err(|e| format!("write: {e}"))?;
        stream.set_read_timeout(Some(Duration::from_millis(1))).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
        let why = self.pump(stream, &mut inbox, &mut buf, &core, (paired, instance));
        *LIVE_HEADSET.lock().unwrap() = None;
        REPORTS.lock().unwrap().clear();
        audio.stop();
        uplink.stop();
        core.stop();
        let _ = write_all(stream, &XrServerMsg::Bye { reason: why.clone() }.encode());
        stream.shutdown();
        Ok(why)
    }

    /// `paired`: whether the headset (its instance id) is the one paired here: when the desktop
    /// unpairs it, it is told and the link ends.
    fn pump(&self, stream: &mut makepad_network::tls::TlsStream, inbox: &mut Inbox, buf: &mut [u8], core: &Core, paired: (bool, u64)) -> String {
        let mut last_heard = Instant::now();
        let mut visible = true;
        let mut stats = PumpStats::new();
        // the room field, the stream's on/off and the see-through the headset was told (None:
        // not yet: told at once)
        let mut room_field_told: Option<bool> = None;
        let mut streaming_told: Option<bool> = None;
        let mut see_through_told: Option<bool> = None;
        let mut headset_told: Option<HeadsetSettings> = None;
        loop {
            let wanted = ROOM_FIELD_WANTED.load(Ordering::Acquire) && ROOM_SCAN.load(Ordering::Acquire);
            if room_field_told != Some(wanted) {
                if let Err(e) = write_all(stream, &XrServerMsg::RoomField { wanted }.encode()) {
                    return format!("write: {e}");
                }
                (self.log)(&format!("xr host: room field {}", if wanted { "wanted" } else { "not wanted" }));
                room_field_told = Some(wanted);
            }
            let on = STREAMING.load(Ordering::Acquire);
            if streaming_told != Some(on) {
                if let Err(e) = write_all(stream, &XrServerMsg::Streaming { on }.encode()) {
                    return format!("write: {e}");
                }
                (self.log)(&format!("xr host: stream {}", if on { "on" } else { "off" }));
                // (on again: the decoder starts from a key picture)
                if on && streaming_told.is_some() {
                    core.keyframe();
                }
                streaming_told = Some(on);
            }
            let headset = *HEADSET_SETTINGS.lock().unwrap();
            if headset_told != Some(headset) {
                if let Err(e) = write_all(stream, &headset.message().encode()) {
                    return format!("write: {e}");
                }
                (self.log)(&format!("xr host: headset settings {headset:?}"));
                headset_told = Some(headset);
            }
            let see = SEE_THROUGH.load(Ordering::Acquire);
            if see_through_told != Some(see) {
                if let Err(e) = write_all(stream, &XrServerMsg::SeeThrough { on: see }.encode()) {
                    return format!("write: {e}");
                }
                see_through_told = Some(see);
            }
            // another display rate asked (`set_refresh_hz`): the headset is told and the session
            // ends; it links again at that rate (new encoders)
            let hz = REFRESH_HZ.load(Ordering::Acquire);
            if hz != 0 && hz != core.fps {
                if let Err(e) = write_all(stream, &XrServerMsg::Refresh { mhz: hz * 1000 }.encode()) {
                    return format!("write: {e}");
                }
                return format!("the display rate goes from {} to {hz} Hz", core.fps);
            }
            if self.stop.load(Ordering::Acquire) {
                return "the server stopped".into();
            }
            if paired.0 && self.pairing.lock().unwrap().paired.as_ref().map(|h| h.instance) != Some(paired.1) {
                let _ = write_all(stream, &XrServerMsg::Unpaired.encode());
                return "the desktop unpaired the headset".into();
            }
            if last_heard.elapsed() > Duration::from_secs(6) {
                return "the headset was silent for 6 s".into();
            }
            loop {
                match core.out.try_recv() {
                    Ok(Out::Video(head, data, encoded_at)) => {
                        let mut head = head;
                        head.stages[stage::SEND] = encoded_at.elapsed().as_micros() as u32;
                        let msg = encode_xr_video(&head, &data);
                        let write_at = Instant::now();
                        if let Err(e) = write_all(stream, &msg) {
                            return format!("write: {e}");
                        }
                        stats.sent(&head, msg.len(), write_at.elapsed());
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return "the encoder stopped".into(),
                }
            }
            let n = match stream.read(buf) {
                Ok(0) => return "the headset closed the connection".into(),
                Ok(n) => n,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => 0,
                Err(e) => return format!("read: {e}"),
            };
            if n > 0 {
                inbox.feed(&buf[..n]);
                last_heard = Instant::now();
            }
            loop {
                let (ty, payload) = match inbox.next() {
                    Ok(Some(m)) => m,
                    Ok(None) => break,
                    Err(e) => return e.to_string(),
                };
                let msg = match XrClientMsg::decode(ty, &payload) {
                    Ok(m) => m,
                    Err(e) => return e.to_string(),
                };
                match msg {
                    XrClientMsg::Pose { seq, display_ns, frame, .. } if visible => {
                        stats.poses += 1;
                        if !core.pose(seq, display_ns, &frame) {
                            stats.refused_poses += 1;
                        }
                    }
                    XrClientMsg::Pose { .. } => {}
                    XrClientMsg::State { visible: v, focused } => {
                        if v && !visible {
                            core.keyframe();
                        }
                        visible = v;
                        (self.log)(&format!("xr host: headset {} {}", if v { "visible" } else { "not visible" }, if focused { "focused" } else { "not focused" }));
                    }
                    XrClientMsg::Stats { seq, decode_us, .. } => {
                        stats.decode_us.push(decode_us);
                        stats.reported(seq, decode_us);
                        core.tile_decoded();
                    }
                    XrClientMsg::Ping { client_ns } => {
                        if let Err(e) = write_all(stream, &XrServerMsg::Pong { client_ns, server_ns: makepad_hal::clock_ns() }.encode()) {
                            return format!("write: {e}");
                        }
                    }
                    XrClientMsg::Keyframe => core.keyframe(),
                    XrClientMsg::World { voxel_size, chunk_edge, bricks } => {
                        stats.bricks += bricks.len() as u64;
                        stats.brick_bytes += payload.len() as u64;
                        if let Some(world) = &self.world {
                            world(voxel_size, chunk_edge, bricks);
                        }
                    }
                    XrClientMsg::Report { values } => {
                        let mut r = REPORTS.lock().unwrap();
                        r.push_back(values);
                        while r.len() > 60 {
                            r.pop_front();
                        }
                    }
                    XrClientMsg::Hello { .. } => return "a second hello".into(),
                }
            }
            stats.window(core, &self.log);
        }
    }
}

/// The per-5 s stats of a session's link.
struct PumpStats {
    at: Instant,
    poses: u64,
    refused_poses: u64,
    pictures: u64,
    bytes: u64,
    stages_us: [Vec<u32>; 4],
    write_us: Vec<u32>,
    decode_us: Vec<u32>,
    /// Per tile: from its last byte written to the headset's report that it took it, less the
    /// headset's own time on it (the link's round trip with whatever queue it holds).
    trip_us: Vec<u32>,
    /// The tiles written and not yet reported: (seq, when written).
    unreported: std::collections::VecDeque<(u64, Instant)>,
    /// Room-field bricks received, and their messages' bytes.
    bricks: u64,
    brick_bytes: u64,
}

fn median_ms(v: &mut [u32]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable();
    v[v.len() / 2] as f64 / 1000.0
}

impl PumpStats {
    fn new() -> Self {
        PumpStats { at: Instant::now(), poses: 0, refused_poses: 0, pictures: 0, bytes: 0, stages_us: Default::default(), write_us: Vec::new(), decode_us: Vec::new(), trip_us: Vec::new(), unreported: Default::default(), bricks: 0, brick_bytes: 0 }
    }

    fn sent(&mut self, head: &VideoHead, bytes: usize, write: Duration) {
        // a picture counted once, at its first tile; its stages are the slowest tile's
        if head.tile == 0 {
            self.pictures += 1;
        }
        self.bytes += bytes as u64;
        for (i, v) in head.stages.iter().enumerate() {
            self.stages_us[i].push(*v);
        }
        self.write_us.push(write.as_micros() as u32);
        if self.unreported.len() >= 64 {
            self.unreported.pop_front();
        }
        self.unreported.push_back((head.seq, Instant::now()));
    }

    /// The headset took a tile of picture `seq` (`decode_us` after it arrived).
    fn reported(&mut self, seq: u64, decode_us: u32) {
        let Some(at) = self.unreported.iter().position(|(s, _)| *s == seq) else { return };
        let (_, written) = self.unreported.remove(at).unwrap();
        // (older ones never reported: lost)
        self.unreported.drain(..at);
        self.trip_us.push((written.elapsed().as_micros() as u32).saturating_sub(decode_us));
    }

    fn window(&mut self, core: &Core, log: &Log) {
        let secs = self.at.elapsed().as_secs_f64();
        if secs < 5.0 {
            return;
        }
        let built = core.flow.built.swap(0, Ordering::AcqRel);
        let skipped = core.flow.skipped.swap(0, Ordering::AcqRel);
        let kbps = core.rate.kbps.load(Ordering::Acquire);
        *SERVER_LINE.lock().unwrap() = format!("server {} Hz · built {:.0}/s · sent {:.1} of {:.0} Mbit/s · render {:.1} gpu {:.1} encode {:.1} ms",
            core.fps, built as f64 / secs, self.bytes as f64 * 8.0 / secs / 1e6, BITRATE_KBPS.load(Ordering::Acquire) as f64 / 1000.0, median_ms(&mut self.stages_us[0].clone()), median_ms(&mut self.stages_us[1].clone()), median_ms(&mut self.stages_us[2].clone()));
        let [ref mut render, ref mut gpu, ref mut encode, ref mut send] = self.stages_us;
        log(&format!(
            "xr host: poses {:.0}/s ({} no XR window) · built {:.0}/s ({} without a slot) · sent {:.0}/s {:.1} Mbit/s (rate {:.0}) · render {:.1} gpu {:.1} encode {:.1} send {:.1} (write {:.1}) ms · headset decode {:.1} ms · in flight {}",
            self.poses as f64 / secs,
            self.refused_poses,
            built as f64 / secs,
            skipped,
            self.pictures as f64 / secs,
            self.bytes as f64 * 8.0 / secs / 1e6,
            kbps as f64 / 1000.0,
            median_ms(render),
            median_ms(gpu),
            median_ms(encode),
            median_ms(send),
            median_ms(&mut self.write_us),
            median_ms(&mut self.decode_us),
            core.flow.in_flight(core.tiles),
        ));
        if self.bricks > 0 {
            log(&format!("xr host: room field {} bricks ({:.1} KB/s) from the headset", self.bricks, self.brick_bytes as f64 / secs / 1000.0));
        }
        let held = core.flow.held.swap(0, Ordering::AcqRel);
        let trip_ms = median_ms(&mut self.trip_us);
        let trip_min_ms = self.trip_us.first().map_or(0.0, |v| *v as f64 / 1000.0);
        log(&format!("xr host: link trip {trip_ms:.1} ms (min {trip_min_ms:.1}, base {:.1}); the app waited for the headset {:.0}/s", core.rate.base_ms(), held as f64 / secs));
        {
            let mut stats = SERVER_STATS.lock().unwrap();
            *stats = ServerStats {
                window: stats.window + 1,
                fps: core.fps,
                built_per_s: built as f64 / secs,
                sent_mbps: self.bytes as f64 * 8.0 / secs / 1e6,
                rate_mbps: kbps as f64 / 1000.0,
                cap_mbps: BITRATE_KBPS.load(Ordering::Acquire) as f64 / 1000.0,
                render_ms: median_ms(render),
                gpu_ms: median_ms(gpu),
                encode_ms: median_ms(encode),
                trip_ms,
                trip_base_ms: core.rate.base_ms(),
                waited_per_s: held as f64 / secs,
            };
        }
        core.rate.second(trip_ms, trip_min_ms, self.bytes as f64 * 8.0 / secs / 1000.0, log);
        *self = PumpStats::new();
    }
}

/// What the encode side hands the session's TLS thread: an eye's access unit, when encoded.
enum Out {
    Video(VideoHead, Vec<u8>, Instant),
}

/// A picture the app packed (`makepad_xr_stream`) for the encoder: the shared surface (the
/// packed NV12 layout of the whole picture, both eye columns), when the GPU work that drew it
/// completes (`ready`), the display time of the pose it was drawn with. `held` is dropped once
/// every eye's encode read the surface: the app may draw into it again.
pub struct Picture {
    pub surface: SurfaceHandle,
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
struct Flow {
    built_total: AtomicU64,
    decoded_tiles: AtomicU64,
    /// Pictures that never reach the headset (an encode that failed): out of flight.
    lost: AtomicU64,
    /// This window's (the stats line).
    built: AtomicU64,
    skipped: AtomicU64,
    /// The app asked for room and the headset had too many pictures in flight (this window).
    held: AtomicU64,
}

impl Flow {
    fn in_flight(&self, tiles: u32) -> u64 {
        let done = self.decoded_tiles.load(Ordering::Acquire) / tiles.max(1) as u64 + self.lost.load(Ordering::Acquire);
        self.built_total.load(Ordering::Acquire).saturating_sub(done)
    }
}

/// The stream's rate (all tiles): down when the link queues pictures (its round trip grows past
/// what it is when idle), back up while it does not. kbit/s.
///
/// The signal is the link's own delay, not the app waiting for pictures in flight: a picture is
/// in flight from its build until the headset reports it (encode, the link both ways, the
/// headset taking it), which is longer than a display period on a healthy link, so the app
/// waited for a fifth of its frames with the network idle and the rate fell for nothing
/// (22 -> 15 Mbit/s on 5 ms Wi-Fi).
struct Rate {
    kbps: AtomicU32,
    /// The app's bitrate (`set_bitrate_kbps`) as last seen: a change applies at once.
    cap: AtomicU32,
    clean: AtomicU32,
    /// The link's idle round trip (µs): the lowest seen, creeping up 2% a window so a changed
    /// route is learned. 0 until the first window.
    base_us: AtomicU32,
}

impl Rate {
    /// The rate now (kbit/s, all tiles): the app's bitrate the moment it changes, else the
    /// controller's (never above it).
    fn current(&self) -> u32 {
        let cap = BITRATE_KBPS.load(Ordering::Acquire);
        if self.cap.swap(cap, Ordering::AcqRel) != cap {
            self.kbps.store(cap, Ordering::Release);
            self.clean.store(0, Ordering::Release);
        }
        self.kbps.load(Ordering::Acquire).min(cap)
    }

    fn base_ms(&self) -> f64 {
        self.base_us.load(Ordering::Acquire) as f64 / 1000.0
    }

    /// One stats window: the median and lowest round trip of a tile (ms), the rate sent (kbit/s).
    fn second(&self, trip_ms: f64, trip_min_ms: f64, sent_kbps: f64, log: &Log) {
        let kbps = self.current();
        let cap = self.cap.load(Ordering::Acquire);
        if trip_ms <= 0.0 {
            return;
        }
        let min_us = (trip_min_ms * 1000.0) as u32;
        let base = self.base_us.load(Ordering::Acquire);
        let base = if base == 0 { min_us } else { min_us.min(base + base / 50 + 1) };
        self.base_us.store(base, Ordering::Release);
        let sent_kbps = sent_kbps as u32;
        // queued: half the tiles wait QUEUED_MS longer than the idle link takes
        let link_bound = trip_ms > base as f64 / 1000.0 + QUEUED_MS && sent_kbps > 0;
        let next = if link_bound {
            self.clean.store(0, Ordering::Release);
            ((sent_kbps as f64 * 0.85) as u32).max(MIN_KBPS)
        } else if self.clean.fetch_add(1, Ordering::AcqRel) >= 2 && kbps < cap {
            (kbps as f64 * 1.1) as u32
        } else {
            kbps
        }
        .min(cap);
        if next != kbps {
            self.kbps.store(next, Ordering::Release);
            log(&format!("xr host: rate {:.0} -> {:.0} Mbit/s ({})", kbps as f64 / 1000.0, next as f64 / 1000.0,
                if link_bound { format!("the link queues: trip {trip_ms:.1} ms, idle {:.1}", base as f64 / 1000.0) } else { "the link keeps up".to_string() }));
        }
    }
}

/// How much longer than the idle link a tile's median round trip may take before the link
/// counts as queueing (ms).
const QUEUED_MS: f64 = 10.0;
/// The lowest the rate controller goes (kbit/s, all tiles).
const MIN_KBPS: u32 = 15_000;
/// Pictures built and not yet decoded by the headset before the app waits (`room`):
/// `MAKEPAD_XR_REMOTE_IN_FLIGHT`, default 4 (a picture's build to its decode takes ~2 display
/// periods at 90 Hz: with 2 in flight the app waited for most frames and built 64 of 90/s; at
/// the 120% eye picture, 3 still held the app back 40-60 times a second and it built 81-86/s:
/// each missing picture a repeat on the headset).
fn in_flight_limit() -> u64 {
    std::env::var("MAKEPAD_XR_REMOTE_IN_FLIGHT").ok().and_then(|v| v.parse().ok()).unwrap_or(4u64).clamp(1, 8)
}

/// One eye's encode: what its thread is given.
struct Work {
    surface: SurfaceHandle,
    ready: GpuCompletion,
    _held: Arc<Box<dyn Send + Sync>>,
    head: VideoHead,
    built_at: Instant,
    kbps: u32,
}

/// The session's pose datagrams: a thread reads them (newest wins: an older or repeated one is
/// dropped, a gap counted as lost) and hands each to the app as a pose over the link would. The
/// headset sends one per display frame, for a display time on its own grid: the app renders each
/// as it comes, so its pictures keep the display's spacing and phase (no clock of its own).
struct Uplink {
    port: u16,
    key: [u8; 32],
    stop: Arc<AtomicBool>,
    socket: std::net::UdpSocket,
    /// Where the poses come from (the headset's socket): where its audio goes.
    from: Arc<Mutex<Option<SocketAddr>>>,
}

impl Uplink {
    fn open(peer: std::net::IpAddr, core: &Core, log: Log) -> Result<Uplink, String> {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("uplink socket: {e}"))?;
        socket.set_read_timeout(Some(Duration::from_millis(100))).map_err(|e| e.to_string())?;
        let port = socket.local_addr().map_err(|e| e.to_string())?.port();
        let mut key = [0u8; 32];
        makepad_network::tls::os_random(&mut key).map_err(|e| format!("uplink key: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let (poses, ending, stop2) = (core.poses.clone(), core.ending.clone(), stop.clone());
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
                    take_pose(&poses, seq, display_ns, frame);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Uplink { port, key, stop, socket: own, from: from_at })
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// A pose from the headset: to the app's XR window, and kept to match its picture.
fn take_pose(poses: &Mutex<VecDeque<(u64, Arrived)>>, seq: u64, display_ns: u64, frame: &[u8]) -> bool {
    // the left controller's menu button turns the stream on and off (unless the app took that
    // over, `set_menu_toggles_streaming`): never seen by the app then
    let mut masked = None;
    if MENU_TOGGLES.load(Ordering::Acquire) {
        use makepad_hal::{buffers::{read_items, Wire}, xr_controller, XrFrame};
        if let Some(mut f) = read_items::<XrFrame>(frame, 1).next() {
            let left = &mut f.controllers[0];
            let down = left.buttons & xr_controller::ACTIVE != 0 && left.buttons & xr_controller::CLICK_MENU != 0;
            if down && !MENU_DOWN.swap(true, Ordering::AcqRel) {
                let on = !STREAMING.load(Ordering::Acquire);
                set_streaming(on);
            } else if !down {
                MENU_DOWN.store(false, Ordering::Release);
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
    if !STREAMING.load(Ordering::Acquire) {
        return true;
    }
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
struct Core {
    size: [u32; 2],
    tiles: u32,
    codec: u8,
    description: String,
    flow: Arc<Flow>,
    rate: Arc<Rate>,
    out: Receiver<Out>,
    keyframe: Arc<[AtomicBool; 2]>,
    /// The poses asked for, to match their pictures.
    poses: Arc<Mutex<VecDeque<(u64, Arrived)>>>,
    ending: Arc<AtomicBool>,
    /// The display rate the session's encoders are made for (Hz).
    fps: u32,
}

/// The session the app's pictures go to.
struct Live {
    eye: [u32; 2],
    tx: SyncSender<Picture>,
    flow: Arc<Flow>,
    tiles: u32,
    limit: u64,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);
/// The headset's per-second reports (`XrClientMsg::Report`), the last minute, oldest first.
/// The app uses the headset's room field (`set_room_field_wanted`): the headset is told, and
/// integrates and sends its field only then.
static ROOM_FIELD_WANTED: AtomicBool = AtomicBool::new(false);

/// The user's power switches: the stream runs (pictures and sound to the headset), the room
/// scan is allowed (the headset scans only when this and the app's want are both on); the
/// left menu button turns the stream on and off (the app may take the button over); the
/// headset shows the room through the picture (mixed reality) or not (VR).
static STREAMING: AtomicBool = AtomicBool::new(true);
static ROOM_SCAN: AtomicBool = AtomicBool::new(true);
static MENU_TOGGLES: AtomicBool = AtomicBool::new(true);
static MENU_DOWN: AtomicBool = AtomicBool::new(false);
static SEE_THROUGH: AtomicBool = AtomicBool::new(true);
/// How many times the stream was turned on or off (an app's settings panel follows it).
static STREAMING_CHANGES: AtomicU32 = AtomicU32::new(0);

/// The stream on or off: off, the app draws no XR frames for the headset, nothing is encoded
/// or sent (sound neither), and the headset stops all of its stream work (it shows the room).
/// On again: a key picture first.
pub fn set_streaming(on: bool) {
    if STREAMING.swap(on, Ordering::AcqRel) != on {
        STREAMING_CHANGES.fetch_add(1, Ordering::AcqRel);
        crate::xr_audio::set_streaming(on);
        wake_app();
    }
}
pub fn streaming() -> bool {
    STREAMING.load(Ordering::Acquire)
}
/// Counts every change of [`streaming`] (the menu button's too).
pub fn streaming_changes() -> u32 {
    STREAMING_CHANGES.load(Ordering::Acquire)
}
/// Whether the left controller's menu button turns the stream on and off (default on; off: the
/// app gets the button).
pub fn set_menu_toggles_streaming(on: bool) {
    MENU_TOGGLES.store(on, Ordering::Release);
}
/// The room scan allowed (the user's switch; the headset scans while the app wants the room
/// field and this is on).
pub fn set_room_scan(on: bool) {
    ROOM_SCAN.store(on, Ordering::Release);
}
pub fn room_scan() -> bool {
    ROOM_SCAN.load(Ordering::Acquire)
}
/// Mixed reality (true: the room shows through the picture's empty parts) or VR (the picture
/// opaque, no passthrough) on the headset.
pub fn set_see_through(on: bool) {
    SEE_THROUGH.store(on, Ordering::Release);
}

/// The display rate the app asks of a streaming headset (Hz; 0: the headset's own), and the
/// stream's bitrate (kbit/s, all tiles: the rate controller never goes above it).
static REFRESH_HZ: AtomicU32 = AtomicU32::new(0);
static BITRATE_KBPS: AtomicU32 = AtomicU32::new(80_000);

/// The display rate a streaming headset runs at (Hz; 0: its own): a change re-links it at that
/// rate (a second without pictures).
pub fn set_refresh_hz(hz: u32) {
    REFRESH_HZ.store(hz, Ordering::Release);
}

/// The stream's bitrate (kbit/s, all tiles): applied at once to a running stream; the rate
/// controller goes below it when the link queues and back up to it.
pub fn set_bitrate_kbps(kbps: u32) {
    BITRATE_KBPS.store(kbps.max(MIN_KBPS), Ordering::Release);
}

/// The display rates a headset may run at (Hz).
pub const REFRESH_RATES: [u32; 4] = [72, 80, 90, 120];
/// Whether the app uses the headset's room field (its physics, particles, field view); a
/// streaming headset is told at once. Off at start.
pub fn set_room_field_wanted(wanted: bool) {
    ROOM_FIELD_WANTED.store(wanted, Ordering::Release);
}

static REPORTS: Mutex<VecDeque<Vec<f32>>> = Mutex::new(VecDeque::new());

/// The server's settings for the headset's own mechanisms (`XrServerMsg::Headset`): the policy
/// lives here, the headset executes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadsetSettings {
    /// Fixed foveation, 0 none .. 3 high (2: the user's pick, the edges look the same, the
    /// headset's pass ~20% cheaper).
    pub foveation: u8,
    /// The eye buffers' range on the headset's GPU time (percent of the start size).
    pub eye_min_pct: u8,
    pub eye_max_pct: u8,
    /// Hands and controllers tracked at once.
    pub multimodal: bool,
    /// The room's depth hides the picture.
    pub occlusion: bool,
    /// The poses at least this many display frames past the headset's newest.
    pub ahead: u8,
}

impl HeadsetSettings {
    pub const DEFAULT: HeadsetSettings = HeadsetSettings { foveation: 2, eye_min_pct: 70, eye_max_pct: 100, multimodal: true, occlusion: true, ahead: 4 };

    fn message(&self) -> XrServerMsg {
        XrServerMsg::Headset { foveation: self.foveation, eye_min_pct: self.eye_min_pct, eye_max_pct: self.eye_max_pct, multimodal: self.multimodal, occlusion: self.occlusion, ahead: self.ahead }
    }
}

static HEADSET_SETTINGS: Mutex<HeadsetSettings> = Mutex::new(HeadsetSettings::DEFAULT);

/// The headset's settings (a streaming headset is told at once).
pub fn set_headset_settings(settings: HeadsetSettings) {
    *HEADSET_SETTINGS.lock().unwrap() = settings;
}

pub fn headset_settings() -> HeadsetSettings {
    *HEADSET_SETTINGS.lock().unwrap()
}

/// The server's pairing state (`crate::xr_pair`), once it started.
static PAIRING: Mutex<Option<Arc<Mutex<crate::xr_pair::ServerPairing>>>> = Mutex::new(None);
/// The headset linked now: its instance id, name and the link's session id.
static LIVE_HEADSET: Mutex<Option<(u64, String, u64)>> = Mutex::new(None);

/// The server's pairing (None before it started): its instance id, the PIN typed, the paired
/// headset, the headset that PIN matched, a clash.
pub fn pairing() -> Option<crate::xr_pair::ServerPairing> {
    PAIRING.lock().unwrap().as_ref().map(|p| p.lock().unwrap().clone())
}

/// The headset linked now (instance id, name, session id).
pub fn live_headset() -> Option<(u64, String, u64)> {
    LIVE_HEADSET.lock().unwrap().clone()
}

/// The PIN as the user types it (a headset's PIN once it has `crate::xr_pair::PIN_DIGITS`
/// digits; empty: none). Any change unpairs the paired headset (told, its link ends; both
/// forget); a whole PIN then looks for the headset showing it.
pub fn set_pin(pin: &str) {
    let Some(p) = PAIRING.lock().unwrap().clone() else { return };
    let mut p = p.lock().unwrap();
    if p.pin == pin {
        return;
    }
    p.pin = pin.to_string();
    p.paired = None;
    p.matched = None;
    p.clash = false;
    p.save();
}

/// The server's own numbers of its last stats window, one line (for a stats panel).
static SERVER_LINE: Mutex<String> = Mutex::new(String::new());

pub fn server_line() -> String {
    SERVER_LINE.lock().unwrap().clone()
}

/// The server's numbers of its last stats window (5 s; medians per picture): for an app's
/// automatic stream settings. `window` counts the windows (0: none yet).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ServerStats {
    pub window: u64,
    pub fps: u32,
    pub built_per_s: f64,
    pub sent_mbps: f64,
    /// The rate controller's rate and the app's cap (Mbit/s).
    pub rate_mbps: f64,
    pub cap_mbps: f64,
    /// Pose to picture built, the GPU's part (render + pack), the encode (ms).
    pub render_ms: f64,
    pub gpu_ms: f64,
    pub encode_ms: f64,
    /// A tile's round trip to the headset (ms) and the idle link's.
    pub trip_ms: f64,
    pub trip_base_ms: f64,
    /// The app waited for the headset (too many pictures in flight), per second.
    pub waited_per_s: f64,
}

static SERVER_STATS: Mutex<ServerStats> = Mutex::new(ServerStats { window: 0, fps: 0, built_per_s: 0.0, sent_mbps: 0.0, rate_mbps: 0.0, cap_mbps: 0.0, render_ms: 0.0, gpu_ms: 0.0, encode_ms: 0.0, trip_ms: 0.0, trip_base_ms: 0.0, waited_per_s: 0.0 });

pub fn server_stats() -> ServerStats {
    *SERVER_STATS.lock().unwrap()
}

/// The headset's last minute of per-second reports (`crate::xr::report` names the values),
/// oldest first: for a stats panel.
pub fn headset_reports() -> Vec<Vec<f32>> {
    REPORTS.lock().unwrap().iter().cloned().collect()
}
type Wake = Arc<dyn Fn() + Send + Sync>;
static WAKE: Mutex<Option<Wake>> = Mutex::new(None);

fn wake_app() {
    let wake = WAKE.lock().unwrap().clone();
    if let Some(wake) = wake {
        wake();
    }
}

/// What the app is told when there is room again ([`room`]): a picture slot freed, or the
/// headset decoded one.
pub fn set_wake(wake: Wake) {
    *WAKE.lock().unwrap() = Some(wake);
}

/// The eye size (pixels) of the headset now streaming: the picture the app packs. None while
/// none streams.
pub fn session_eye() -> Option<[u32; 2]> {
    LIVE.lock().unwrap().as_ref().map(|l| l.eye)
}

/// Whether the app may build the next frame: a headset streams and fewer than its limit of
/// pictures are in flight to it.
pub fn room() -> bool {
    let live = LIVE.lock().unwrap();
    let Some(l) = live.as_ref() else { return false };
    let room = l.flow.in_flight(l.tiles) < l.limit;
    if !room {
        l.flow.held.fetch_add(1, Ordering::AcqRel);
    }
    room
}

/// A picture for the headset (see [`Picture`]); false when no headset streams (it is dropped,
/// and with it `held`).
pub fn submit(picture: Picture) -> bool {
    let live = LIVE.lock().unwrap();
    let Some(l) = live.as_ref() else { return false };
    l.flow.built_total.fetch_add(1, Ordering::AcqRel);
    l.flow.built.fetch_add(1, Ordering::AcqRel);
    match l.tx.try_send(picture) {
        Ok(()) => true,
        Err(_) => {
            // the picture thread is behind (cannot be with the app's slots): out of flight
            l.flow.lost.fetch_add(1, Ordering::AcqRel);
            false
        }
    }
}

/// The app found no free slot for a frame it built (counted on the stats line).
pub fn note_skipped() {
    if let Some(l) = LIVE.lock().unwrap().as_ref() {
        l.flow.skipped.fetch_add(1, Ordering::AcqRel);
    }
}

impl Core {
    fn start(eye: [u32; 2], fps: u32, codec: StreamVideoCodec, log: Log) -> Result<Core, String> {
        let kbps = BITRATE_KBPS.load(Ordering::Acquire);
        let size = picture_size(eye);
        // a stream per eye on its own engine when the GPU has two
        let tiles = if VideoStreamEncoder::caps().engines >= 2 { 2 } else { 1 };
        let flow = Arc::new(Flow::default());
        let rate = Arc::new(Rate { kbps: AtomicU32::new(kbps), cap: AtomicU32::new(kbps), clean: AtomicU32::new(0), base_us: AtomicU32::new(0) });
        let keyframe: Arc<[AtomicBool; 2]> = Arc::new([AtomicBool::new(true), AtomicBool::new(true)]);
        let ending = Arc::new(AtomicBool::new(false));
        let poses: Arc<Mutex<VecDeque<(u64, Arrived)>>> = Arc::default();

        let (out_tx, out) = channel::<Out>();
        let codec_byte = if codec == StreamVideoCodec::Hevc { CODEC_HEVC } else { CODEC_H264 };
        let mut workers = Vec::new();
        let mut description = String::new();
        for t in 0..tiles {
            let w = size[0] / tiles;
            let options = VideoStreamEncoderOptions { codec, width: w, height: size[1], fps, bitrate_kbps: kbps / tiles, keyint: 0, low_latency: true, full_chroma: false };
            let tile = StreamTile { engine: t, x0: t * w, y0: 0, source_height: size[1] };
            // the fastest encode (latency first), a constant rate whose pictures fit the link
            // (`MAKEPAD_XR_REMOTE_SHARP=1`: the encoder's best pictures per bit, more encode time)
            let sharp = std::env::var("MAKEPAD_XR_REMOTE_SHARP").as_deref() == Ok("1");
            let tuning = StreamTuning { quality: 0, constant_rate: true, buffer_pictures: 1, sharp };
            let mut encoder = VideoStreamEncoder::new_tuned(options, tile, tuning).map_err(|e| format!("encoder: {e:?}"))?;
            if t == 0 {
                description = encoder.description();
            }
            let (tx, rx) = channel::<Work>();
            let (out_tx, keyframe, flow, log) = (out_tx.clone(), keyframe.clone(), flow.clone(), log.clone());
            let w_px = w;
            std::thread::Builder::new()
                .name(format!("xr-host-encode-{t}"))
                .spawn(move || {
                    let mut applied = kbps / tiles;
                    let mut pts = 0i64;
                    let mut foveated = false;
                    // the live tuning file (`tune`), read once a second
                    let (mut tuned, mut tune_at) = (Tune::default(), Instant::now() - Duration::from_secs(2));
                    while let Ok(w) = rx.recv() {
                        // the eye's foveation, once its field of view is known (its first picture)
                        if !foveated && w.head.eyes[t as usize].fov != [0.0; 4] {
                            foveated = true;
                            if let (Some(grid), Some(peak)) = (encoder.qp_grid(), foveation_peak()) {
                                let map = foveation(grid, [w_px, size[1]], w.head.eyes[t as usize].fov, peak);
                                if encoder.set_qp_map(&map) {
                                    log(&format!("xr host: eye {t}: foveated encode ({}x{} QP regions of {}x{} px, +{peak} QP at the edge)", grid.cols, grid.rows, grid.texel[0], grid.texel[1]));
                                }
                            }
                        }
                        if tune_at.elapsed() >= Duration::from_secs(1) {
                            tune_at = Instant::now();
                            let now = tune();
                            if now != tuned {
                                if t == 0 {
                                    log(&format!("xr host: tuning: max QP {}", now.max_qp));
                                }
                                encoder.set_max_qp(now.max_qp);
                                tuned = now;
                            }
                        }
                        let tile_kbps = (w.kbps / tiles).max(1);
                        if tile_kbps != applied {
                            encoder.set_bitrate_kbps(tile_kbps);
                            applied = tile_kbps;
                        }
                        if keyframe[t as usize].swap(false, Ordering::AcqRel) {
                            encoder.request_keyframe();
                        }
                        pts += 1;
                        let push_at = Instant::now();
                        let queued = push_at.duration_since(w.built_at);
                        let packets = match encoder.push_surface(&w.surface, &w.ready, pts * 138_888) {
                            Ok(p) => p,
                            Err(e) => {
                                log(&format!("xr host: eye {t}: encode: {e:?}"));
                                flow.lost.fetch_add(1, Ordering::AcqRel);
                                keyframe[t as usize].store(true, Ordering::Release);
                                continue;
                            }
                        };
                        // the push waited for the frame's GPU work, copied the column, encoded it
                        let push = push_at.elapsed();
                        let encode = Duration::from_micros(encoder.timings().encode_us as u64).min(push);
                        if t == 0 {
                            dump_picture(&w.surface, size, &log);
                        }
                        let mut head = w.head;
                        head.tile = t as u8;
                        head.stages[stage::GPU] = (queued + push - encode).as_micros() as u32;
                        head.stages[stage::ENCODE] = encode.as_micros() as u32;
                        drop(w._held);
                        for p in packets {
                            head.key = p.is_key;
                            if out_tx.send(Out::Video(head, p.data, Instant::now())).is_err() {
                                return;
                            }
                        }
                        wake_app();
                    }
                })
                .map_err(|e| e.to_string())?;
            workers.push(tx);
        }
        // the app's pictures: matched to their poses, fanned out to the eyes
        let (tx, rx) = sync_channel::<Picture>(4);
        {
            let (poses, rate, ending, out_tx) = (poses.clone(), rate.clone(), ending.clone(), out_tx.clone());
            std::thread::Builder::new()
                .name("xr-host-pictures".into())
                .spawn(move || {
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
                        let head = VideoHead { seq, tile: 0, key: false, display_ns: p.display_ns, eyes, focus_m: p.focus_m, stages: [render_us, 0, 0, 0], size, codec: codec_byte };
                        let held: Arc<Box<dyn Send + Sync>> = Arc::new(p.held);
                        let kbps = rate.current();
                        // each eye's encode waits for the same frame's completion
                        for worker in &workers {
                            let _ = worker.send(Work { surface: p.surface.clone(), ready: p.ready.clone(), _held: held.clone(), head, built_at: p.built_at, kbps });
                        }
                    }
                    drop(out_tx);
                })
                .map_err(|e| e.to_string())?;
        }
        *LIVE.lock().unwrap() = Some(Live { eye, tx, flow: flow.clone(), tiles, limit: in_flight_limit() });
        wake_app();
        Ok(Core { size, tiles, codec: codec_byte, description, flow, rate, out, keyframe, poses, ending, fps })
    }

    /// A pose from the headset: to the app's XR window, and kept to match its picture.
    fn pose(&self, seq: u64, display_ns: u64, frame: &[u8]) -> bool {
        take_pose(&self.poses, seq, display_ns, frame)
    }

    fn keyframe(&self) {
        for k in self.keyframe.iter() {
            k.store(true, Ordering::Release);
        }
    }

    /// The headset decoded one eye of a picture.
    fn tile_decoded(&self) {
        self.flow.decoded_tiles.fetch_add(1, Ordering::AcqRel);
        wake_app();
    }

    fn stop(&self) {
        self.ending.store(true, Ordering::Release);
        let mut live = LIVE.lock().unwrap();
        if live.as_ref().is_some_and(|l| Arc::ptr_eq(&l.flow, &self.flow)) {
            *live = None;
        }
    }
}

/// A headset on a loop (`MAKEPAD_XR_REMOTE_SELFTEST=1`): synthetic poses at 72 Hz (the head turning
/// slowly, eyes 64 mm apart, ±45° fields) drive the app's XR window exactly as a headset's would, and
/// every picture is encoded and measured, then dropped (and counted decoded at once). The server
/// side's numbers without a headset.
pub fn selftest(eye: [u32; 2], log: Log) {
    use makepad_hal::{buffers::Wire, XrEye, XrFrame, XrPose};
    let codec = if VideoStreamEncoder::caps().hevc { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
    let core = match Core::start(eye, 72, codec, log.clone()) {
        Ok(c) => c,
        Err(e) => return log(&format!("xr selftest: {e}")),
    };
    log(&format!("xr selftest: {}x{} {codec:?} in {} tile(s) on {}", core.size[0], core.size[1], core.tiles, core.description));
    hal::hal_xr_remote_attach(eye[0], eye[1], 72_000);
    let _ = std::thread::Builder::new().name("xr-selftest".into()).spawn(move || {
        let start = Instant::now();
        let mut seq = 0u64;
        let mut stats = PumpStats::new();
        loop {
            seq += 1;
            let t = start.elapsed().as_secs_f32();
            let yaw = (t * 0.5).sin() * 0.6;
            let q = [0.0, (yaw * 0.5).sin(), 0.0, (yaw * 0.5).cos()];
            let right = [yaw.cos(), 0.0, -yaw.sin()];
            let at = |s: f32| XrPose { orientation: q, position: [right[0] * s * 0.032, 1.6, right[2] * s * 0.032] };
            let fov = [-0.785, 0.785, 0.785, -0.785];
            let frame = XrFrame { head: at(0.0), eyes: [XrEye { pose: at(-1.0), fov }, XrEye { pose: at(1.0), fov }], ..Default::default() };
            let mut wire = Vec::with_capacity(<XrFrame as Wire>::SIZE);
            frame.write(&mut wire);
            stats.poses += 1;
            if !core.pose(seq, makepad_hal::clock_ns() + 30_000_000, &wire) {
                stats.refused_poses += 1;
            }
            // drained as they come (a headset decodes on arrival), the next pose at 72 Hz
            let next = Instant::now() + Duration::from_micros(13_889);
            loop {
                while let Ok(m) = core.out.try_recv() {
                    let Out::Video(mut head, data, encoded_at) = m;
                    head.stages[stage::SEND] = encoded_at.elapsed().as_micros() as u32;
                    stats.sent(&head, data.len(), Duration::ZERO);
                    core.tile_decoded();
                }
                let now = Instant::now();
                if now >= next {
                    break;
                }
                std::thread::sleep((next - now).min(Duration::from_millis(1)));
            }
            stats.window(&core, &log);
            // a headset's report once a second (synthetic: a stats panel shows the selftest)
            if seq % 72 == 0 {
                let mut v = vec![0.0f32; report::COUNT];
                let k = (seq / 72) as f32;
                v[report::DISPLAY] = 72.0;
                v[report::SHOWN] = 72.0 - (k % 7.0 == 0.0) as u8 as f32 * 3.0;
                v[report::REPEATS] = (k % 7.0 == 0.0) as u8 as f32 * 3.0;
                v[report::BLANK] = (k % 13.0 == 0.0) as u8 as f32;
                v[report::M2P_MS] = 70.0 + (k * 0.7).sin() * 20.0;
                v[report::BATTERY_PCT] = 80.0;
                let codes: Vec<u8> = (0..72u32).map(|i| match (i + seq as u32 / 72) % 37 { 0 => report::FRAME_REPEAT, 9 => report::FRAME_LATE, 21 => report::FRAME_SKIP, _ => report::FRAME_NEW }).collect();
                report::pack_frames(&mut v, &codes);
                let mut r = REPORTS.lock().unwrap();
                r.push_back(v);
                while r.len() > 60 {
                    r.pop_front();
                }
            }
        }
    });
}

/// The foveation's QP delta at the edge of the view (`MAKEPAD_XR_REMOTE_FOVEATE`: a number, or 0
/// for none; default 10: about a third of the bits per pixel out there).
fn foveation_peak() -> Option<i8> {
    let peak = std::env::var("MAKEPAD_XR_REMOTE_FOVEATE").ok().and_then(|v| v.parse::<i8>().ok()).unwrap_or(10);
    (peak > 0).then_some(peak)
}

/// An eye column's QP deltas on the encoder's grid: the colour (the top two thirds) at full
/// quality within 15 degrees of the eye's axis, then fewer bits outwards to `peak` at 50 degrees
/// (the lens blurs the edge; the eye looks through the centre); the band below: coverage as the
/// centre (its edges are the composite's edges), depth with fewer bits.
fn foveation(grid: QpGrid, size: [u32; 2], fov: [f32; 4], peak: i8) -> Vec<i8> {
    let [l, r, u, d] = fov.map(f32::tan);
    let colour_h = size[1] * 2 / 3;
    let mut map = Vec::with_capacity((grid.cols * grid.rows) as usize);
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let x = (col * grid.texel[0] + grid.texel[0] / 2).min(size[0] - 1) as f32;
            let y = row * grid.texel[1] + grid.texel[1] / 2;
            let delta = if y < colour_h {
                let tx = l + (x + 0.5) / size[0] as f32 * (r - l);
                let ty = u - (y as f32 + 0.5) / colour_h as f32 * (u - d);
                let angle = (tx * tx + ty * ty).sqrt().atan().to_degrees();
                let k = ((angle - 15.0) / 35.0).clamp(0.0, 1.0);
                (k * peak as f32).round() as i32
            } else if x < size[0] as f32 * 0.5 {
                0
            } else {
                (peak as i32 / 2).max(1)
            };
            map.push(delta.clamp(grid.min.max(-128), grid.max.min(127)) as i8);
        }
    }
    map
}

/// `MAKEPAD_XR_REMOTE_DUMP=<dir>`: the 300th picture's planes as `<dir>/picture.pgm` (its luma:
/// both eye columns, each its colour over its band of coverage and inverse depth) once its encode
/// read it, and a line of the band's numbers. A check of what the GPU packed.
#[cfg(not(target_os = "linux"))]
fn dump_picture(_surface: &SurfaceHandle, _size: [u32; 2], _log: &Log) {}

/// (shared pictures are DMA-BUFs: Linux)
#[cfg(target_os = "linux")]
fn dump_picture(surface: &SurfaceHandle, size: [u32; 2], log: &Log) {
    static N: AtomicU32 = AtomicU32::new(0);
    let Some(dir) = std::env::var_os("MAKEPAD_XR_REMOTE_DUMP") else { return };
    if N.fetch_add(1, Ordering::AcqRel) != 300 {
        return;
    }
    let [w, h] = size;
    let mut px = vec![0u8; (w / 4 * h * 4) as usize];
    if makepad_hal::dmabuf::hal_dmabuf_read(surface.id, &[0, 0, w / 4, h], &mut px) < 0 {
        return log("xr host: dump: the picture could not be read");
    }
    // a BGRA texel holds luma samples 0..3 in its bytes as they lie: the row is the plane's row
    let mut pgm = format!("P5\n{w} {h}\n255\n").into_bytes();
    pgm.extend_from_slice(&px);
    let path = std::path::Path::new(&dir).join("picture.pgm");
    let _ = std::fs::write(&path, &pgm);
    // the band of the left eye: coverage (left half) and inverse depth (right half)
    let (ew, eh) = (w / 2, h * 2 / 3);
    let band = |x0: u32, x1: u32| {
        let (mut n, mut sum, mut max, mut nonzero) = (0u64, 0u64, 0u8, 0u64);
        for y in eh..h {
            for x in x0..x1 {
                let v = px[(y * w + x) as usize];
                n += 1;
                sum += v as u64;
                max = max.max(v);
                nonzero += (v > 0) as u64;
            }
        }
        (sum as f64 / n.max(1) as f64, max, nonzero as f64 / n.max(1) as f64)
    };
    let (a_mean, a_max, a_cov) = band(0, ew / 2);
    let (d_mean, d_max, d_cov) = band(ew / 2, ew);
    let near = if d_max > 0 { crate::xr::DEPTH_NEAR_M * 255.0 / d_max as f32 } else { 0.0 };
    log(&format!("xr host: dumped {} ({w}x{h}); left band coverage mean {a_mean:.1} max {a_max} drawn {:.0}%, inverse depth mean {d_mean:.1} max {d_max} (nearest {near:.2} m) drawn {:.0}%", path.display(), a_cov * 100.0, d_cov * 100.0));
}

/// The eyes of an `XrFrame` (wire form).
fn render_eyes(frame: &[u8]) -> [XrRenderEye; 2] {
    let Some(f) = makepad_hal::buffers::read_items::<makepad_hal::XrFrame>(frame, 1).next() else { return [XrRenderEye::default(); 2] };
    f.eyes.map(|e| XrRenderEye { orientation: e.pose.orientation, position: e.pose.position, fov: e.fov })
}

/// The picture for eyes of `eye` pixels: both eye columns side by side, each its colour over a
/// band half its height (coverage and inverse depth at half resolution; `crate::xr`).
pub fn picture_size(eye: [u32; 2]) -> [u32; 2] {
    [eye[0] * 2, eye[1] * 3 / 2]
}

/// The stream's live tuning (`$TMPDIR/xr-remote-tune`, line `maxqp=N`; read once a second by the
/// encoders): a QP ceiling (every encoder of the process). For sweeps on a running server;
/// absent: none. (The bitrate is the app's setting, `set_bitrate_kbps`.)
#[derive(Clone, Copy, Default, PartialEq)]
struct Tune {
    max_qp: u32,
}

fn tune() -> Tune {
    let Ok(text) = std::fs::read_to_string(std::env::temp_dir().join("xr-remote-tune")) else { return Tune::default() };
    let mut t = Tune::default();
    for line in text.lines() {
        match line.trim().split_once('=') {
            Some(("maxqp", v)) => t.max_qp = v.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    t
}
