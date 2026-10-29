//! The server's network side: the listener (and the pairing probe's responder), and per
//! headset its session: the TLS link (hello, welcome, the settings the headset is told, its
//! poses, stats and reports in, the pictures out) and its per-window numbers.

use super::pipeline::{Core, Out, Uplink};
use super::{LiveHeadset, ServerShared};
use crate::settings_types::HeadsetSettings;
use makepad_desk_link::server::{fleet_identity, lan_address};
use makepad_desk_link::xr::*;
use makepad_desk_link::xr_pair::{self, ServerPairing};
use makepad_desk_link::{write_all, Inbox};
use makepad_widgets::makepad_platform::makepad_hal::{self, xr_remote as hal};
use makepad_widgets::makepad_platform::makepad_network::fleet::id::{self, now_secs, Need};
use makepad_widgets::makepad_platform::makepad_network::tls::{self as net_tls, TlsServer};
use makepad_widgets::makepad_platform::video_file::{StreamVideoCodec, VideoStreamEncoder};
use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The room field's bricks from a headset: (voxel size m, chunk edge voxels, bricks).
pub type WorldSink = Arc<dyn Fn(f32, i32, Vec<WorldBrick>) + Send + Sync>;

/// What the server keeps while it is switched off and on ([`Host::set_listening`]): its TLS
/// identity, its pairing (the PIN, the paired headset; this instance's claim on the pairing file
/// is held for the process, `xr_pair::claim`), the room field's sink and the one-headset flag.
/// Off, nothing of it runs: no listener, no probe answers, no session, no encoder.
pub(crate) struct Host {
    app: String,
    name: String,
    tls: Arc<TlsServer>,
    verifier: Arc<id::Verifier>,
    pairing: Arc<Mutex<ServerPairing>>,
    world: WorldSink,
    /// A headset is linked (shared by every listener this host starts: a session of the last
    /// one still ending refuses the next listener's headset until it ended).
    busy: Arc<AtomicBool>,
    _claim: Option<std::fs::File>,
    /// The running listener (None: off).
    listener: Mutex<Option<Listener>>,
    /// The port listened on (0: none).
    port: Arc<AtomicU16>,
}

/// A running listener: its thread, its probe answerer and its sessions end when this drops
/// (the listener and the probe socket close within a poll, each session at its next turn).
struct Listener {
    stop: Arc<AtomicBool>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Host {
    /// The host for `app`'s headsets (`pairing`: the base of its pairing file; None: not kept);
    /// `world` takes a headset's room field. Not listening yet.
    pub(crate) fn new(shared: &ServerShared, app: &str, pairing: Option<std::path::PathBuf>, world: WorldSink) -> Result<Host, String> {
        let (identity, verifier, name) = fleet_identity("makepad xr host").map_err(|why| format!("xr host: {why}"))?;
        let tls = Arc::new(TlsServer::new(identity).map_err(|e| format!("xr host: TLS server: {e}"))?);
        let (path, claim) = match pairing {
            Some(base) => {
                let (path, claim) = xr_pair::claim(base);
                (Some(path), claim)
            }
            None => (None, None),
        };
        let pairing = Arc::new(Mutex::new(ServerPairing::load(path)));
        *shared.pairing.lock().unwrap() = Some(pairing.clone());
        Ok(Host {
            app: app.to_string(),
            name,
            tls,
            verifier: Arc::new(verifier),
            pairing,
            world,
            busy: Arc::new(AtomicBool::new(false)),
            _claim: claim,
            listener: Mutex::new(None),
            port: Arc::new(AtomicU16::new(0)),
        })
    }

    pub(crate) fn listening(&self) -> bool {
        self.listener.lock().unwrap().is_some()
    }

    /// The port headsets link to (None: off, or not bound yet).
    pub(crate) fn port(&self) -> Option<u16> {
        Some(self.port.load(Ordering::Acquire)).filter(|p| *p != 0)
    }

    /// On: listens on a port the OS picks and answers the headsets' pairing probe. Off: the
    /// listener, the probe answerer and any session end (its encoders with it). Never waits.
    pub(crate) fn set_listening(&self, shared: &Arc<ServerShared>, on: bool) -> Result<(), String> {
        let mut listener = self.listener.lock().unwrap();
        if listener.is_some() == on {
            return Ok(());
        }
        if !on {
            *listener = None;
            (shared.log)(&format!("xr host: {} stops listening (switched off)", self.app));
            return Ok(());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let shared_for_test = shared.clone();
        let (shared, stop2, port) = (shared.clone(), stop.clone(), self.port.clone());
        let (app, name, tls, verifier, pairing, world, busy) = (self.app.clone(), self.name.clone(), self.tls.clone(), self.verifier.clone(), self.pairing.clone(), self.world.clone(), self.busy.clone());
        std::thread::Builder::new()
            .name("xr-host-listen".into())
            .spawn(move || {
                let log = shared.log.clone();
                // the port may still be held a moment by a host this process just dropped (an app
                // restart): bind again until it is free
                let addr = SocketAddr::from(([0, 0, 0, 0], 0));
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
                let bound = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                port.store(bound, Ordering::Release);
                log(&format!("xr host: {app} listens on port {bound} as {name} (the admin or a paired headset), instance {:016x}", pairing.lock().unwrap().instance));
                // headsets find this server by their probe (answered while paired, or for the
                // PIN typed here)
                let responder = match xr_pair::identity("pairing headsets") {
                    Ok((me, fleet)) => match xr_pair::Responder::start(me, fleet, pairing.clone(), app.clone(), bound, log.clone()) {
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
                            let ctx = Session { peer, tls: tls.clone(), verifier: verifier.clone(), shared: shared.clone(), stop: stop2.clone(), busy: busy.clone(), server: name.clone(), world: world.clone(), pairing: pairing.clone() };
                            let _ = std::thread::Builder::new().name("xr-host-session".into()).spawn(move || ctx.run(tcp));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                        Err(e) => {
                            log(&format!("xr host: accept: {e}"));
                            std::thread::sleep(Duration::from_millis(200));
                        }
                    }
                }
                drop(responder);
                drop(listener);
                let _ = port.compare_exchange(bound, 0, Ordering::AcqRel, Ordering::Acquire);
                log(&format!("xr host: port {bound} closed, pairing probe no longer answered"));
            })
            .map_err(|e| e.to_string())?;
        // the headset on a loop (debug): a session of this listener's, ended by the off switch
        if std::env::var("MAKEPAD_XR_REMOTE_SELFTEST").as_deref() == Ok("1") {
            super::debug::selftest(shared_for_test, [1680, 1760], stop.clone());
        }
        *listener = Some(Listener { stop });
        Ok(())
    }
}

struct Session {
    world: WorldSink,
    peer: SocketAddr,
    tls: Arc<TlsServer>,
    verifier: Arc<id::Verifier>,
    shared: Arc<ServerShared>,
    stop: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    server: String,
    pairing: Arc<Mutex<ServerPairing>>,
}

impl Session {
    fn run(self, tcp: TcpStream) {
        let why = self.serve(tcp).unwrap_or_else(|e| e);
        (self.shared.log)(&format!("xr host: session {}: {why}", self.peer));
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
        let refuse = |stream: &mut makepad_widgets::makepad_platform::makepad_network::tls::TlsStream, reason: String| -> Result<String, String> {
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
        if self.stop.load(Ordering::Acquire) {
            return refuse(&mut stream, "the desktop switched the stream off".into());
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
                (self.shared.log)(&format!("xr host: paired with {name} ({instance:016x})"));
            }
            p.paired.as_ref().is_some_and(|h| h.instance == instance)
        };
        let result = self.session(&mut stream, inbox, buf, &name, &peer.name, instance, paired, eye, refresh_mhz, decodes);
        self.busy.store(false, Ordering::Release);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn session(&self, stream: &mut makepad_widgets::makepad_platform::makepad_network::tls::TlsStream, mut inbox: Inbox, mut buf: Vec<u8>, name: &str, who: &str, instance: u64, paired: bool, eye: [u32; 2], refresh_mhz: u32, decodes: u32) -> Result<String, String> {
        let eye = [eye[0].clamp(64, 4096) & !3, eye[1].clamp(64, 4096) & !3];
        // H.265 when both ends have it
        let codec = if decodes & codecs::HEVC != 0 && VideoStreamEncoder::caps().hevc { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
        let fps = (refresh_mhz / 1000).clamp(30, 120);
        let core = Core::start(self.shared.clone(), eye, fps, codec)?;
        (self.shared.log)(&format!("xr host: {who} ({name}) from {}: eyes {}x{} at {fps} Hz, {codec:?} {}x{} in {} tile(s) on {}", self.peer, eye[0], eye[1], core.size[0], core.size[1], core.tiles, core.description));
        // the app's XR display from now on (it presents on it at its next drawing)
        hal::hal_xr_remote_enable();
        hal::hal_xr_remote_attach(eye[0], eye[1], refresh_mhz);
        let session = {
            let mut b = [0u8; 8];
            let _ = net_tls::os_random(&mut b);
            u64::from_le_bytes(b)
        };
        let server_instance = self.pairing.lock().unwrap().instance;
        *self.shared.live_headset.lock().unwrap() = Some(LiveHeadset { instance, name: name.to_string(), session });
        let welcome = XrServerMsg::Welcome { version: XR_VERSION, server: self.server.clone(), instance: server_instance, session, size: core.size, codec: core.codec, tiles: core.tiles as u8, encoder: core.description.clone() };
        write_all(stream, &welcome.encode()).map_err(|e| format!("write: {e}"))?;
        // the poses come as datagrams: a socket of the session's own, a key for it
        let uplink = Uplink::open(self.peer.ip(), &core)?;
        // the app's game audio goes down from the same socket to where the poses come from
        let audio = makepad_desk_link::xr_audio::AudioOut::start(uplink.socket.try_clone().map_err(|e| format!("audio socket: {e}"))?, uplink.key, uplink.from.clone(), self.shared.log.clone())?;
        write_all(stream, &XrServerMsg::Uplink { port: uplink.port, key: uplink.key.to_vec() }.encode()).map_err(|e| format!("write: {e}"))?;
        stream.set_read_timeout(Some(Duration::from_millis(1))).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
        self.shared.haptics.lock().unwrap().clear();
        self.shared.haptics_ok.store(decodes & codecs::HAPTICS != 0, Ordering::Release);
        let why = self.pump(stream, &mut inbox, &mut buf, &core, (paired, instance));
        self.shared.haptics_ok.store(false, Ordering::Release);
        *self.shared.live_headset.lock().unwrap() = None;
        self.shared.reports.lock().unwrap().clear();
        audio.stop();
        uplink.stop();
        core.stop();
        // the app's side hears that the headset went (the desktop paints as always again)
        self.shared.wake();
        let _ = write_all(stream, &XrServerMsg::Bye { reason: why.clone() }.encode());
        stream.shutdown();
        Ok(why)
    }

    /// `paired`: whether the headset (its instance id) is the one paired here: when the desktop
    /// unpairs it, it is told and the link ends.
    fn pump(&self, stream: &mut makepad_widgets::makepad_platform::makepad_network::tls::TlsStream, inbox: &mut Inbox, buf: &mut [u8], core: &Core, paired: (bool, u64)) -> String {
        let mut last_heard = Instant::now();
        let mut visible = true;
        let mut stats = PumpStats::new();
        // the room field, the stream's on/off and the see-through the headset was told (None:
        // not yet: told at once)
        let mut room_field_told: Option<bool> = None;
        let mut streaming_told: Option<bool> = None;
        let mut see_through_told: Option<bool> = None;
        let mut headset_told: Option<HeadsetSettings> = None;
        let mut flowing = false;
        loop {
            let sh = &self.shared;
            let wanted = sh.room_field_wanted.load(Ordering::Acquire) && sh.room_scan.load(Ordering::Acquire);
            if room_field_told != Some(wanted) {
                if let Err(e) = write_all(stream, &XrServerMsg::RoomField { wanted }.encode()) {
                    return format!("write: {e}");
                }
                (self.shared.log)(&format!("xr host: room field {}", if wanted { "wanted" } else { "not wanted" }));
                room_field_told = Some(wanted);
            }
            let on = sh.streaming();
            if streaming_told != Some(on) {
                if let Err(e) = write_all(stream, &XrServerMsg::Streaming { on }.encode()) {
                    return format!("write: {e}");
                }
                (self.shared.log)(&format!("xr host: stream {}", if on { "on" } else { "off" }));
                // (on again: the decoder starts from a key picture)
                if on && streaming_told.is_some() {
                    core.keyframe();
                }
                streaming_told = Some(on);
            }
            let headset = *sh.headset_settings.lock().unwrap();
            if headset_told != Some(headset) {
                if let Err(e) = write_all(stream, &headset_message(&headset).encode()) {
                    return format!("write: {e}");
                }
                (self.shared.log)(&format!("xr host: headset settings {headset:?}"));
                headset_told = Some(headset);
            }
            // the app's controller buzzes (only to a headset that plays them)
            let haptics = std::mem::take(&mut *sh.haptics.lock().unwrap());
            for h in &haptics {
                let msg = XrServerMsg::Haptic { hand: h.hand, amplitude: h.amplitude, duration_us: (h.seconds * 1e6) as u32 };
                if let Err(e) = write_all(stream, &msg.encode()) {
                    return format!("write: {e}");
                }
            }
            let see = sh.see_through.load(Ordering::Acquire);
            if see_through_told != Some(see) {
                if let Err(e) = write_all(stream, &XrServerMsg::SeeThrough { on: see }.encode()) {
                    return format!("write: {e}");
                }
                see_through_told = Some(see);
            }
            // another display rate asked (`set_refresh_hz`): the headset is told and the session
            // ends; it links again at that rate (new encoders)
            let hz = sh.refresh_hz.load(Ordering::Acquire);
            if hz != 0 && hz != core.fps {
                if let Err(e) = write_all(stream, &XrServerMsg::Refresh { mhz: hz * 1000 }.encode()) {
                    return format!("write: {e}");
                }
                return format!("the display rate goes from {} to {hz} Hz", core.fps);
            }
            if self.stop.load(Ordering::Acquire) {
                return "the desktop switched the stream off".into();
            }
            if paired.0 && self.pairing.lock().unwrap().paired.as_ref().map(|h| h.instance) != Some(paired.1) {
                let _ = write_all(stream, &XrServerMsg::Unpaired.encode());
                return "the desktop unpaired the headset".into();
            }
            // the headset's poses stopped (asleep, taken off, not visible): the app is woken once so
            // its desktop is no longer held for the headset (`ServerShared::frames_flowing`)
            let now_flowing = sh.frames_flowing();
            if flowing && !now_flowing {
                sh.wake();
            }
            flowing = now_flowing;
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
                        (self.shared.log)(&format!("xr host: headset {} {}", if v { "visible" } else { "not visible" }, if focused { "focused" } else { "not focused" }));
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
                        (self.world)(voxel_size, chunk_edge, bricks);
                    }
                    XrClientMsg::Report { values } => self.shared.push_report(values),
                    XrClientMsg::Hello { .. } => return "a second hello".into(),
                }
            }
            stats.window(core);
        }
    }
}

/// The per-5 s stats of a session's link.
pub(crate) struct PumpStats {
    at: Instant,
    pub(crate) poses: u64,
    pub(crate) refused_poses: u64,
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
    /// The rate controller's second (its own window inside this one).
    rate: RateWindow,
}

/// One second for the rate controller: pictures and bytes sent, the tiles' round trips.
struct RateWindow {
    at: Instant,
    pictures: u64,
    bytes: u64,
    trip_us: Vec<u32>,
}

impl RateWindow {
    fn new() -> RateWindow {
        RateWindow { at: Instant::now(), pictures: 0, bytes: 0, trip_us: Vec::new() }
    }
}

fn median_ms(v: &mut [u32]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable();
    v[v.len() / 2] as f64 / 1000.0
}

impl PumpStats {
    pub(crate) fn new() -> Self {
        PumpStats { at: Instant::now(), poses: 0, refused_poses: 0, pictures: 0, bytes: 0, stages_us: Default::default(), write_us: Vec::new(), decode_us: Vec::new(), trip_us: Vec::new(), unreported: Default::default(), bricks: 0, brick_bytes: 0, rate: RateWindow::new() }
    }

    pub(crate) fn sent(&mut self, head: &VideoHead, bytes: usize, write: Duration) {
        // a picture counted once, at its first tile; its stages are the slowest tile's
        if head.tile == 0 {
            self.pictures += 1;
            self.rate.pictures += 1;
        }
        self.bytes += bytes as u64;
        self.rate.bytes += bytes as u64;
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
        let trip = (written.elapsed().as_micros() as u32).saturating_sub(decode_us);
        self.trip_us.push(trip);
        self.rate.trip_us.push(trip);
    }

    /// The rate controller's second, when one passed.
    fn rate_second(&mut self, core: &Core) {
        let secs = self.rate.at.elapsed().as_secs_f64();
        if secs < 1.0 {
            return;
        }
        let w = std::mem::replace(&mut self.rate, RateWindow::new());
        let mut trips = w.trip_us;
        let trip_ms = median_ms(&mut trips);
        let trip_min_ms = trips.first().map_or(0.0, |v| *v as f64 / 1000.0);
        core.rate.second(trip_ms, trip_min_ms, (w.pictures as f64 / secs) as u64, (w.bytes as f64 * 8.0 / secs / 1000.0) as u64, core.fps, core.size);
    }

    pub(crate) fn window(&mut self, core: &Core) {
        self.rate_second(core);
        let (sh, log) = (&core.shared, &core.shared.log);
        let secs = self.at.elapsed().as_secs_f64();
        if secs < 5.0 {
            return;
        }
        let built = core.flow.built.swap(0, Ordering::AcqRel);
        let skipped = core.flow.skipped.swap(0, Ordering::AcqRel);
        let kbps = core.rate.current();
        *sh.server_line.lock().unwrap() = format!("server {} Hz · built {:.0}/s · sent {:.1} of {:.0} Mbit/s · render {:.1} gpu {:.1} encode {:.1} ms",
            core.fps, built as f64 / secs, self.bytes as f64 * 8.0 / secs / 1e6, sh.bitrate_kbps.load(Ordering::Acquire) as f64 / 1000.0, median_ms(&mut self.stages_us[0].clone()), median_ms(&mut self.stages_us[1].clone()), median_ms(&mut self.stages_us[2].clone()));
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
            let mut stats = sh.server_stats.lock().unwrap();
            *stats = super::ServerStats {
                window: stats.window + 1,
                fps: core.fps,
                built_per_s: built as f64 / secs,
                sent_mbps: self.bytes as f64 * 8.0 / secs / 1e6,
                rate_mbps: kbps as f64 / 1000.0,
                cap_mbps: sh.bitrate_kbps.load(Ordering::Acquire) as f64 / 1000.0,
                render_ms: median_ms(render),
                gpu_ms: median_ms(gpu),
                encode_ms: median_ms(encode),
                trip_ms,
                trip_base_ms: core.rate.base_ms(),
                waited_per_s: held as f64 / secs,
            };
        }
        let rate = std::mem::replace(&mut self.rate, RateWindow::new());
        *self = PumpStats::new();
        self.rate = rate;
    }
}


fn headset_message(h: &HeadsetSettings) -> XrServerMsg {
    XrServerMsg::Headset { foveation: h.foveation, eye_min_pct: h.eye_min_pct, eye_max_pct: h.eye_max_pct, multimodal: h.multimodal, occlusion: h.occlusion, ahead: h.ahead }
}

#[cfg(test)]
mod tests {
    use super::super::ServerShared;
    use super::*;

    /// This process's XR server threads (Linux: by name; elsewhere none are seen).
    fn xr_threads() -> Vec<String> {
        let Ok(tasks) = std::fs::read_dir("/proc/self/task") else { return Vec::new() };
        tasks
            .filter_map(|t| std::fs::read_to_string(t.ok()?.path().join("comm")).ok())
            .map(|n| n.trim().to_string())
            .filter(|n| n.starts_with("xr-host") || n.starts_with("xr-pair"))
            .collect()
    }

    fn until(what: &str, f: impl Fn() -> bool) {
        let at = Instant::now();
        while !f() {
            assert!(at.elapsed() < Duration::from_secs(10), "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Off, nothing of the server runs: its port is closed and its listener and probe answerer
    /// threads are gone; on again it listens anew with the same pairing and PIN.
    #[test]
    fn switched_off_nothing_listens() {
        let shared = Arc::new(ServerShared::new(Arc::new(|line: &str| println!("{line}"))));
        let world: WorldSink = Arc::new(|_, _, _| {});
        let dir = std::env::temp_dir().join(format!("xr-off-test-{}", std::process::id()));
        let host = match Host::new(&shared, "xr-off-test", Some(dir.join("xr-pairing")), world) {
            Ok(h) => h,
            Err(e) => {
                println!("skipped: {e} (no fleet identity on this machine)");
                return;
            }
        };
        let (instance, linux) = (host.pairing.lock().unwrap().instance, cfg!(target_os = "linux"));
        host.pairing.lock().unwrap().pin = "12345".to_string();
        for _ in 0..2 {
            host.set_listening(&shared, true).unwrap();
            until("binds a port", || host.port().is_some());
            let port = host.port().unwrap();
            assert!(TcpStream::connect(("127.0.0.1", port)).is_ok(), "on: port {port} accepts");
            if linux {
                until("the listener and probe answerer run", || xr_threads().iter().any(|t| t == "xr-host-listen") && xr_threads().iter().any(|t| t == "xr-pair-answer"));
            }
            host.set_listening(&shared, false).unwrap();
            assert!(!host.listening());
            until("the port closes", || host.port().is_none() && TcpStream::connect(("127.0.0.1", port)).is_err());
            if linux {
                until("every server thread ends", || xr_threads().is_empty());
            }
        }
        let p = host.pairing.lock().unwrap();
        assert_eq!((p.instance, p.pin.as_str()), (instance, "12345"), "the pairing and the PIN survive");
        drop(p);
        drop(host);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
