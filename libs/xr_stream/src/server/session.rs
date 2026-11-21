//! The listener answers pairing probes. Each viewer has a TLS link that can wait on standby
//! or serve a stream. Only the streaming link owns the render, encode and audio session.

use super::links::{Link, Point, XrDeviceKind};
use super::pipeline::{Core, Out, Uplink};
use super::{LiveHeadset, MicEvent, ServerShared};
use crate::settings_types::HeadsetSettings;
use makepad_desk_link::server::lan_address;
use makepad_desk_link::xr::*;
use makepad_desk_link::xr_capture::CaptureAssembler;
use makepad_desk_link::xr_pair::{self, ServerPairing, ViewerProof};
use makepad_desk_link::{write_all, Inbox};
use makepad_widgets::makepad_platform::makepad_hal::{self, xr_remote as hal};
use makepad_widgets::makepad_platform::makepad_network::fleet::id;
use makepad_widgets::makepad_platform::makepad_network::tls::{self as net_tls, TlsServer, TlsStream};
use makepad_widgets::makepad_platform::video_file::{StreamVideoCodec, VideoStreamEncoder};
use makepad_widgets::makepad_platform::thread::{Class, Strand, TaskPool, Ticket};
use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The room field's bricks from a headset: (voxel size m, chunk edge voxels, bricks).
pub type WorldSink = Arc<dyn Fn(f32, i32, Vec<WorldBrick>) + Send + Sync>;

/// The host keeps its identity and pairing claim across listener restarts.
pub(crate) struct Host {
    app: String,
    me: Arc<id::Me>,
    tls: Arc<TlsServer>,
    pub(super) pairing: Arc<Mutex<ServerPairing>>,
    world: WorldSink,
    shared: Arc<ServerShared>,
    control: Strand<Control>,
    _claim: Option<std::fs::File>,
    listener: Mutex<Option<Listener>>,
}

pub(super) enum Change {
    Listen,
    /// The launch override (`MAKEPAD_XR_STREAM`, `--xr-stream`): on listens with the kept
    /// switches as they are; off holds the listener off for this run, the kept switches
    /// untouched, until the user changes a switch or types a PIN.
    Launch(bool),
    Enabled(bool),
    Device(Point, bool),
    Forget(Point),
    Pin(String),
}

/// The strand owns the durable pairing and the listening overrides in submission order.
struct Control {
    pairing: ServerPairing,
    forced: bool,
    /// The launch override held the listener off (see [`Change::Launch`]).
    held_off: bool,
    remembered: Option<Point>,
    pin_suspended: bool,
}

impl Control {
    fn apply(&mut self, change: Change) -> std::io::Result<Option<Point>> {
        let pairing = &mut self.pairing;
        if !matches!(change, Change::Listen | Change::Launch(_)) {
            self.held_off = false;
        }
        match change {
            Change::Listen => {}
            Change::Launch(on) => {
                self.forced = on;
                self.held_off = !on;
            }
            Change::Enabled(true) => {
                if pairing.on.is_none() {
                    pairing.set_on(self.remembered.as_ref())?;
                }
                self.remembered = None;
                self.forced = true;
                self.pin_suspended = false;
            }
            Change::Enabled(false) => {
                let previous = pairing.on;
                pairing.set_on(None)?;
                self.remembered = previous.or(self.remembered);
                self.forced = false;
                self.pin_suspended = true;
            }
            Change::Device(point, true) => {
                pairing.set_on(Some(&point))?;
                self.remembered = None;
            }
            Change::Device(point, false) => {
                if pairing.on == Some(point) {
                    pairing.set_on(None)?;
                    self.remembered = None;
                }
            }
            Change::Forget(point) => {
                pairing.forget_viewer(&point)?;
                if self.remembered == Some(point) {
                    self.remembered = None;
                }
                return Ok(Some(point));
            }
            Change::Pin(pin) => {
                if pairing.pin != pin {
                    pairing.pin = pin;
                    pairing.save();
                }
                self.pin_suspended = false;
            }
        }
        Ok(None)
    }

    fn listens(&self) -> bool {
        !self.held_off && (self.forced || self.pairing.on.is_some() || (!self.pin_suspended && xr_pair::valid_pin(&self.pairing.pin)))
    }
}

/// A running listener: its thread, its probe answerer and its sessions end when this drops
/// (the listener and the probe socket close within a poll, each session at its next turn).
struct Listener {
    stop: Arc<AtomicBool>,
    port: Arc<AtomicU16>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Host {
    /// The host for `app`'s viewers (`pairing`: the base of its pairing file; None: not kept);
    /// `world` takes a headset's room field. Not listening yet.
    pub(crate) fn new(shared: &Arc<ServerShared>, pool: &TaskPool, app: &str, pairing: Option<std::path::PathBuf>, world: WorldSink) -> Result<Arc<Host>, String> {
        let me = id::Me::load(&id::dir()).map_err(|e| format!("xr host: has no fleet identity ({e})"))?;
        let identity = me.tls_identity("makepad xr host").map_err(|e| format!("xr host: cannot make its TLS identity ({e})"))?;
        let tls = Arc::new(TlsServer::new(identity).map_err(|e| format!("xr host: TLS server: {e}"))?);
        let (path, claim) = match pairing {
            Some(base) => {
                let (path, claim) = xr_pair::claim(base);
                (Some(path), claim)
            }
            None => (None, None),
        };
        let mut pairing = ServerPairing::load(path);
        // A PIN pairs within the run it was typed in: one left from an earlier run would open
        // the LAN at every start without the user asking.
        if !pairing.pin.is_empty() {
            pairing.pin.clear();
            pairing.save();
        }
        let viewers = viewer_rows(&pairing);
        shared.links.lock().unwrap().paired(pairing.on, viewers);
        let published = Arc::new(Mutex::new(pairing.clone()));
        let control = Control { pairing, forced: false, held_off: false, remembered: None, pin_suspended: false };
        Ok(Arc::new(Host {
            app: app.to_string(),
            me: Arc::new(me),
            tls,
            pairing: published,
            world,
            shared: shared.clone(),
            control: Strand::with_state(pool, "XR viewer settings", control),
            _claim: claim,
            listener: Mutex::new(None),
        }))
    }

    pub(super) fn change(self: &Arc<Self>, change: Change) {
        let host = self.clone();
        self.control.submit(Class::Interactive, Ticket::none(), move |control| {
            match control.apply(change) {
                Ok(forgotten) => {
                    host.publish(&control.pairing, forgotten.as_ref());
                    host.listen_for(control);
                }
                Err(e) => (host.shared.log)(&format!("xr host: could not change viewer settings: {e}")),
            }
        }).detach();
    }

    fn listen_for(self: &Arc<Self>, control: &Control) {
        if let Err(e) = self.set_listening(control.listens()) {
            (self.shared.log)(&format!("xr host: cannot listen: {e}"));
        }
    }

    fn publish(&self, pairing: &ServerPairing, forgotten: Option<&Point>) {
        let shared = &self.shared;
        // Only the control strand writes the published pairing. Prepare its replacement
        // and the panel's names before taking the link table lock.
        if *self.pairing.lock().unwrap() == *pairing {
            return;
        }
        let next = pairing.clone();
        let previous = std::mem::replace(&mut *self.pairing.lock().unwrap(), next);
        let changed = previous.on != pairing.on;
        if changed || previous.viewers != pairing.viewers {
            let viewers = viewer_rows(pairing);
            let mut links = shared.links.lock().unwrap();
            if let Some(point) = forgotten {
                links.forget(point);
            }
            links.paired(pairing.on, viewers);
            if changed || forgotten.is_some() {
                shared.selection_generation.fetch_add(1, Ordering::Release);
            }
        }
        if changed {
            let name = |p: &ServerPairing| p.viewers.iter().find(|v| Some(v.point) == p.on)
                .map_or_else(|| "nothing".into(), |v| v.name.clone());
            (shared.log)(&format!("xr host: the view goes from {} to {}", name(&previous), name(pairing)));
        }
        shared.wake();
    }

    fn admit(self: &Arc<Self>, proof: ViewerProof, instance: u64, link: Link, stop: Arc<AtomicBool>) -> Result<Vec<u8>, String> {
        let host = self.clone();
        // Only the TLS thread waits for admission. A PIN confirmation follows a durable save.
        self.control.submit(Class::Interactive, Ticket::none(), move |control| {
            if stop.load(Ordering::Acquire) {
                return Err("the desktop switched the stream off".into());
            }
            let pairing = &mut control.pairing;
            let (testing, occupied) = {
                let links = host.shared.links.lock().unwrap();
                (links.testing(), links.occupied())
            };
            if testing {
                return Err("another headset is connected".into());
            }
            if !link.standby {
                // A viewer without standby must get its PIN confirmation in Welcome: it is
                // admitted only when it can stream now.
                let eligible = match pairing.on {
                    Some(point) => point == link.point,
                    None => !pairing.viewers.iter().any(|v| v.point == link.point),
                };
                if pairing.on.is_none() && !eligible {
                    return Err("this device is switched off in the app's headset panel".into());
                }
                if occupied || !eligible {
                    return Err("another headset is connected".into());
                }
            }
            let previous = pairing.on;
            let pin_ok = pairing.admit_viewer(&proof, instance, &link.name)?;
            if previous != pairing.on {
                control.remembered = None;
            }
            host.publish(pairing, None);
            host.shared.links.lock().unwrap().join(link);
            host.listen_for(control);
            host.shared.wake();
            Ok(pin_ok)
        }).join().map_err(|e| format!("viewer admission: {e}"))?
    }

    pub(crate) fn listening(&self) -> bool {
        self.listener.lock().unwrap().is_some()
    }

    /// The port headsets link to (None: off, or not bound yet).
    pub(crate) fn port(&self) -> Option<u16> {
        let port = self.listener.lock().unwrap().as_ref()?.port.load(Ordering::Acquire);
        (port != 0).then_some(port)
    }

    /// On: listens on a port the OS picks and answers viewer probes. Off: the
    /// listener, the probe answerer and any session end (its encoders with it). Never waits.
    fn set_listening(self: &Arc<Self>, on: bool) -> Result<(), String> {
        let shared = &self.shared;
        if self.listening() == on {
            return Ok(());
        }
        if !on {
            *self.listener.lock().unwrap() = None;
            shared.wake();
            (shared.log)(&format!("xr host: {} stops listening (switched off)", self.app));
            return Ok(());
        }
        let stop = Arc::new(AtomicBool::new(false));
        // the headset on a loop (debug) is this server's one headset: no probe is answered and a
        // real headset is refused while it runs (a paired headset on the LAN never joins a test)
        let selftest = std::env::var("MAKEPAD_XR_REMOTE_SELFTEST").as_deref() == Ok("1");
        if !makepad_widgets::makepad_platform::makepad_network::lan::service("xr host") {
            return Ok(());
        }
        let running = Listener { stop: stop.clone(), port: Arc::new(AtomicU16::new(0)) };
        let (thread_shared, stop2, port) = (shared.clone(), stop.clone(), running.port.clone());
        let (app, me, tls, pairing, world) = (self.app.clone(), self.me.clone(), self.tls.clone(), self.pairing.clone(), self.world.clone());
        let host = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("xr-host-listen".into())
            .spawn(move || {
                let shared = thread_shared;
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
                let name = me.cred.name.clone();
                let instance = pairing.lock().unwrap().instance;
                log(&format!("xr host: {app} listens on port {bound} as {name}, instance {instance:016x}"));
                // The probe answer and the TLS certificate use the same server key.
                let responder = if selftest {
                    None
                } else {
                    let ad = xr_pair::HostAd { app: super::uppercase_first(&app), test: shared.test_run.clone() };
                    let links = shared.links.clone();
                    match xr_pair::Responder::start(
                        me, pairing.clone(), ad, bound,
                        Arc::new(move |p| links.lock().unwrap().linked(p)),
                        shared.whereabouts.clone(), log.clone(),
                    ) {
                        Ok(r) => Some(r),
                        Err(e) => {
                            log(&format!("xr host: {e}"));
                            None
                        }
                    }
                };
                let mut test_pending = selftest;
                while !stop2.load(Ordering::Acquire) {
                    if test_pending {
                        // A previous listener can still be tearing down its Core.
                        let slot = shared.links.lock().unwrap().take_selftest();
                        if let Some(slot) = slot {
                            let eye = std::env::var("MAKEPAD_XR_REMOTE_SELFTEST_EYE").ok().and_then(|v| {
                                let (w, h) = v.split_once('x')?;
                                Some([w.trim().parse().ok()?, h.trim().parse().ok()?])
                            });
                            super::debug::selftest(shared.clone(), eye.unwrap_or([1680, 1760]), stop2.clone(), slot);
                            test_pending = false;
                        }
                    }
                    match listener.accept() {
                        Ok((tcp, peer)) => {
                            if !lan_address(peer.ip()) {
                                continue;
                            }
                            let ctx = Session { peer, tls: tls.clone(), shared: shared.clone(), stop: stop2.clone(), host: host.clone(), server: name.clone(), app: super::uppercase_first(&app), world: world.clone(), server_instance: instance, pictures: AtomicU64::new(0), decoded: AtomicU64::new(0) };
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
                log(&format!("xr host: port {bound} closed, pairing probe no longer answered"));
            })
            .map_err(|e| e.to_string())?;
        *self.listener.lock().unwrap() = Some(running);
        shared.wake();
        Ok(())
    }
}

fn viewer_rows(pairing: &ServerPairing) -> Vec<(Point, String, String)> {
    pairing.viewers.iter().map(|v| (v.point, net_tls::to_hex(&v.point), v.name.clone())).collect()
}

struct Session {
    world: WorldSink,
    peer: SocketAddr,
    tls: Arc<TlsServer>,
    shared: Arc<ServerShared>,
    stop: Arc<AtomicBool>,
    host: std::sync::Weak<Host>,
    server: String,
    /// The app's name as its pairing answer gives it (`Welcome`, `Standby`).
    app: String,
    server_instance: u64,
    /// The pictures written to the headset and those it reported decoded, for the line that says
    /// how the session ended.
    pictures: AtomicU64,
    decoded: AtomicU64,
}

struct Viewer {
    id: u64,
    name: String,
    who: String,
    instance: u64,
    eye: [u32; 2],
    refresh_mhz: u32,
    decodes: u32,
    views: u32,
}

struct Connection {
    stream: TlsStream,
    inbox: Inbox,
    buf: Vec<u8>,
    unknown_logged: [bool; 256],
    visible: bool,
    last_heard: Instant,
    pin_ok: Vec<u8>,
}

impl Connection {
    fn send(&mut self, msg: XrServerMsg) -> Result<(), String> {
        write_all(&mut self.stream, &msg.encode()).map_err(|e| format!("write: {e}"))
    }

    fn read(&mut self) -> Result<(), String> {
        match self.stream.read(&mut self.buf) {
            Ok(0) => return Err("the headset closed the connection".into()),
            Ok(n) => {
                self.inbox.feed(&self.buf[..n]);
                self.last_heard = Instant::now();
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(e) => return Err(format!("read: {e}")),
        }
        Ok(())
    }

    fn next(&mut self, log: &super::Log, standby: bool) -> Result<Option<(XrClientMsg, usize)>, String> {
        while let Some((ty, payload)) = self.inbox.next().map_err(|e| e.to_string())? {
            let decoded = if standby {
                XrClientMsg::decode_standby(ty, &payload)
            } else {
                XrClientMsg::decode(ty, &payload).map(Some)
            };
            match decoded {
                Ok(Some(msg)) => return Ok(Some((msg, payload.len()))),
                Ok(None) => continue,
                Err(e) if is_unknown_message(&e) => {
                    if !self.unknown_logged[ty as usize] {
                        self.unknown_logged[ty as usize] = true;
                        log(&format!("xr host: skipping unknown client message {ty} (a newer client)"));
                    }
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(None)
    }

    /// Tells the viewer it is on standby (`server`, `instance`, `app`, `test`: this server's, as in `Welcome`).
    fn standby(&mut self, (server, instance, app, test): (&str, u64, &str, &str), viewer: &Viewer, showing: &str, log: &super::Log) -> Result<(), String> {
        let pin_ok = std::mem::take(&mut self.pin_ok);
        self.send(XrServerMsg::Standby { server: server.to_string(), instance, showing: showing.to_string(), pin_ok, app: app.to_string(), test: test.to_string() })?;
        log(&format!("xr host: {} ({}) on standby, showing {}", viewer.who, viewer.name, if showing.is_empty() { "nothing" } else { showing }));
        self.stream.set_read_timeout(Some(Duration::from_millis(100))).map_err(|e| e.to_string())
    }

    fn state(&mut self, visible: bool, focused: bool, log: &super::Log) {
        self.visible = visible;
        log(&format!("xr host: headset {} {}", if visible { "visible" } else { "not visible" }, if focused { "focused" } else { "not focused" }));
    }

    fn pong(&mut self, client_ns: u64) -> Result<(), String> {
        self.send(XrServerMsg::Pong { client_ns, server_ns: makepad_hal::clock_ns() })
    }

    fn refuse_busy(&mut self) -> Result<String, String> {
        let reason = "another headset is connected".to_owned();
        self.send(XrServerMsg::Refused { reason: reason.clone() })?;
        Ok(reason)
    }

    fn unpaired(&mut self) -> String {
        let _ = self.send(XrServerMsg::Unpaired);
        "the desktop unpaired the headset".into()
    }
}

impl Session {
    fn run(self, tcp: TcpStream) {
        let began = Instant::now();
        let why = self.serve(tcp).unwrap_or_else(|e| e);
        (self.shared.log)(&format!("xr host: session {}: {why} (lasted {:.1} s, {} pictures written, {} decoded by the headset)", self.peer, began.elapsed().as_secs_f64(), self.pictures.load(Ordering::Acquire), self.decoded.load(Ordering::Acquire)));
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
        let refuse = |stream: &mut TlsStream, reason: String| -> Result<String, String> {
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
        if !authorization.starts_with(xr_pair::VIEWER_PREFIX) {
            return refuse(&mut stream, "pair this device with the PIN it shows".into());
        }
        let proof = match ViewerProof::verify(&authorization, &self.tls.identity().fingerprint, id::now_secs()) {
            Ok(proof) => proof,
            Err(why) => return refuse(&mut stream, why),
        };
        if self.stop.load(Ordering::Acquire) {
            return refuse(&mut stream, "the desktop switched the stream off".into());
        }
        let views: u32 = if decodes & codecs::MONO != 0 { 1 } else { 2 };
        if views == 1 && self.shared.external.load(Ordering::Acquire) {
            return refuse(&mut stream, "this app renders its own stereo XR frames: a one-view display is not served".into());
        }
        let id = session_id(net_tls::os_random)?;
        let name = name.replace(['\r', '\n'], " ");
        let who = format!("viewer {}", net_tls::to_hex(&proof.point()[..4]));
        let kind = if decodes & codecs::HANDHELD != 0 { XrDeviceKind::Handheld } else { XrDeviceKind::Headset };
        let link = Link { id, point: *proof.point(), name: name.clone(), kind, standby: decodes & codecs::STANDBY != 0 };
        let host = self.host.upgrade().ok_or("the XR host ended")?;
        let pin_ok = match host.admit(proof, instance, link, self.stop.clone()) {
            Ok(pin_ok) => pin_ok,
            Err(why) => return refuse(&mut stream, why),
        };
        drop(host);
        let viewer = Viewer { id, name, who, instance, eye: aligned_eye(eye), refresh_mhz, decodes, views };
        let mut conn = Connection { stream, inbox, buf, visible: true, unknown_logged: [false; 256], last_heard: Instant::now(), pin_ok };
        let mut standby = false;
        let result = self.linked(&mut conn, &viewer, &mut standby);
        self.shared.links.lock().unwrap().leave(id);
        self.shared.wake();
        let why = result.unwrap_or_else(|e| e);
        if standby {
            (self.shared.log)(&format!("xr host: {} ({}) standby link ended: {why}", viewer.who, viewer.name));
        }
        let _ = conn.send(XrServerMsg::Bye { reason: why.clone() });
        conn.stream.shutdown();
        Ok(why)
    }

    fn linked(&self, conn: &mut Connection, viewer: &Viewer, standby: &mut bool) -> Result<String, String> {
        conn.stream.set_write_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
        let mut first_session = Some(viewer.id);
        loop {
            let mut links = self.shared.links.lock().unwrap();
            let stopped = self.stop.load(Ordering::Acquire);
            if links.get(viewer.id).is_none() {
                drop(links);
                return Ok(conn.unpaired());
            }
            if !stopped && links.take(viewer.id) {
                let generation = self.shared.selection_generation.load(Ordering::Acquire);
                drop(links);
                *standby = false;
                self.shared.wake();
                let session = match first_session.take() {
                    Some(id) => id,
                    None => session_id(net_tls::os_random)?,
                };
                self.session(conn, viewer, session, generation)?;
                if viewer.decodes & codecs::STANDBY == 0 {
                    return conn.refuse_busy();
                }
                let showing = self.shared.links.lock().unwrap().handoff_name(viewer.id);
                *standby = true;
                conn.standby((&self.server, self.server_instance, &self.app, &self.shared.test_run), viewer, &showing, &self.shared.log)?;
                self.shared.links.lock().unwrap().release(viewer.id);
                self.shared.wake();
                conn.last_heard = Instant::now();
                continue;
            }
            // PIN recovery can consume the last reason to listen. Send its confirmation
            // before telling that viewer that the listener closed.
            if stopped && conn.pin_ok.is_empty() {
                return Ok("the desktop switched the stream off".into());
            }
            if links.testing() || viewer.decodes & codecs::STANDBY == 0 {
                drop(links);
                return conn.refuse_busy();
            }
            let showing = (!*standby).then(|| links.showing());
            drop(links);
            if let Some(showing) = showing {
                *standby = true;
                conn.standby((&self.server, self.server_instance, &self.app, &self.shared.test_run), viewer, &showing, &self.shared.log)?;
            }
            if stopped {
                return Ok("the desktop switched the stream off".into());
            }
            if conn.last_heard.elapsed() >= Duration::from_secs(15) {
                return Ok("the viewer was silent for 15 s".into());
            }
            conn.read()?;
            while let Some((msg, _)) = conn.next(&self.shared.log, true)? {
                match msg {
                    XrClientMsg::Ping { client_ns } => conn.pong(client_ns)?,
                    XrClientMsg::State { visible, focused } => conn.state(visible, focused, &self.shared.log),
                    _ => {}
                }
            }
        }
    }

    fn session(&self, conn: &mut Connection, viewer: &Viewer, session: u64, generation: u64) -> Result<(), String> {
        let Viewer { eye, refresh_mhz, decodes, views, instance, .. } = *viewer;
        let (name, who) = (&viewer.name, &viewer.who);
        // H.265 when both ends have it
        let codec = if decodes & codecs::HEVC != 0 && VideoStreamEncoder::caps().hevc { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
        let fps = (refresh_mhz / 1000).clamp(30, 120);
        let handheld = decodes & codecs::HANDHELD != 0;
        // The old viewer's last pose and menu press belong to its own tracking space.
        self.shared.pose_ns.store(0, Ordering::Release);
        *self.shared.menu.lock().unwrap() = Default::default();
        let core = Core::start(self.shared.clone(), eye, fps, codec, views, handheld)?;
        let view_label = if views == 1 { "one view" } else { "eyes" };
        (self.shared.log)(&format!("xr host: {who} ({name}) from {}: {view_label} {}x{} at {fps} Hz, {codec:?} {}x{} in {} tile(s) on {}", self.peer, eye[0], eye[1], core.size[0], core.size[1], core.tiles, core.description));
        // the app's XR display from now on (it presents on it at its next drawing)
        hal::hal_xr_remote_enable();
        hal::hal_xr_remote_attach(eye[0], eye[1], refresh_mhz, core.views, if handheld { makepad_hal::xr_display_kind::HANDHELD } else { makepad_hal::xr_display_kind::WORN });
        // a session of its own: the headset's origin need not be the last session's
        self.shared.new_space("the viewer started streaming");
        *self.shared.live_headset.lock().unwrap() = Some(LiveHeadset { instance, name: name.to_string(), session });
        // a person uses the app through the headset while it is linked, however the session ends:
        // a node's devhub keeps other work off the machine meanwhile (its frames share the GPU)
        struct InUse;
        impl Drop for InUse {
            fn drop(&mut self) {
                makepad_widgets::makepad_platform::restart::set_in_use("xr headset", None);
            }
        }
        makepad_widgets::makepad_platform::restart::set_in_use("xr headset", Some(&format!("an XR headset ({name})")));
        let _in_use = InUse;
        let mut uplink = None;
        let mut audio = None;
        let result = (|| {
            let pin_ok = std::mem::take(&mut conn.pin_ok);
            conn.send(XrServerMsg::Welcome { version: XR_VERSION, server: self.server.clone(), instance: self.server_instance, session, size: core.size, codec: core.codec, tiles: core.tiles as u8, encoder: core.description.clone(), eye: core.eye, fovea: core.fovea, caps: server_caps::MONO | server_caps::HANDHELD | server_caps::STANDBY, pin_ok, app: self.app.clone(), test: self.shared.test_run.clone() })?;
            let up = uplink.insert(Uplink::open(self.peer.ip(), &core)?);
            audio = Some(makepad_desk_link::xr_audio::AudioOut::start(up.socket.try_clone().map_err(|e| format!("audio socket: {e}"))?, up.key, up.from.clone(), self.shared.log.clone())?);
            conn.send(XrServerMsg::Uplink { port: up.port, key: up.key.to_vec() })?;
            conn.stream.set_read_timeout(Some(Duration::from_millis(1))).map_err(|e| e.to_string())?;
            self.shared.haptics.lock().unwrap().clear();
            self.shared.haptics_ok.store(decodes & codecs::HAPTICS != 0, Ordering::Release);
            self.shared.capture_ok.store(decodes & codecs::CAPTURE != 0, Ordering::Release);
            self.shared.mic_ok.store(decodes & codecs::MIC != 0, Ordering::Release);
            // a handheld that shows a narrowed view gets its view narrowed to the app's content
            // (`MAKEPAD_XR_REFRAME=0`: never, for a side by side without it)
            *self.shared.reframe.lock().unwrap() = Default::default();
            let reframe = handheld && decodes & codecs::REFRAME != 0;
            self.shared.reframe_ok.store(reframe, Ordering::Release);
            if reframe {
                let on = self.shared.reframe_on.load(Ordering::Acquire);
                (self.shared.log)(if on { "xr host: reframing: the handheld's view narrowed to the app's content (where it says it is)" } else { "xr host: reframing off (the diagnostics' switch, MAKEPAD_XR_REFRAME=0): the handheld gets its whole view" });
            }
            conn.last_heard = Instant::now();
            self.pump(conn, &core, viewer.id, session, generation)
        })();
        self.decoded.fetch_add(core.flow.decoded_tiles.load(Ordering::Acquire) / core.tiles.max(1) as u64, Ordering::Release);
        self.shared.haptics_ok.store(false, Ordering::Release);
        self.shared.capture_ok.store(false, Ordering::Release);
        self.shared.mic_ok.store(false, Ordering::Release);
        self.shared.reframe_ok.store(false, Ordering::Release);
        self.shared.mic_link_ended(session);
        *self.shared.live_headset.lock().unwrap() = None;
        self.shared.reports.lock().unwrap().clear();
        if let Some(audio) = audio { audio.stop(); }
        if let Some(up) = uplink { up.stop(); }
        core.stop();
        // the app's side hears that the headset went (the desktop paints as always again)
        self.shared.wake();
        result
    }

    /// Returns to standby when the selected point changes. Other exits end the link.
    fn pump(&self, conn: &mut Connection, core: &Core, link: u64, session: u64, mut generation: u64) -> Result<(), String> {
        let mut stats = PumpStats::new();
        // the room field, the stream's on/off and the see-through the headset was told (None:
        // not yet: told at once)
        let mut room_field_told: Option<bool> = None;
        let mut streaming_told: Option<bool> = None;
        let mut see_through_told: Option<bool> = None;
        let mut headset_told: Option<HeadsetSettings> = None;
        let mut capture_told = None;
        let mut mic_told = false;
        let mut handheld_rate_logged = false;
        let mut capture = CaptureAssembler::default();
        let mut flowing = false;
        loop {
            let sh = &self.shared;
            let current = sh.selection_generation.load(Ordering::Acquire);
            if generation != current || self.stop.load(Ordering::Acquire) {
                let links = sh.links.lock().unwrap();
                let stopped = self.stop.load(Ordering::Acquire);
                let paired = links.get(link).is_some();
                let selected = links.wants_stream(link, links.on.as_ref());
                drop(links);
                if !paired {
                    return Err(conn.unpaired());
                }
                if stopped {
                    return Err("the desktop switched the stream off".into());
                }
                if !selected {
                    return Ok(());
                }
                generation = current;
            }
            let wanted = sh.room_field_wanted.load(Ordering::Acquire) && sh.room_scan.load(Ordering::Acquire);
            if room_field_told != Some(wanted) {
                conn.send(XrServerMsg::RoomField { wanted })?;
                (self.shared.log)(&format!("xr host: room field {}", if wanted { "wanted" } else { "not wanted" }));
                room_field_told = Some(wanted);
            }
            let on = sh.streaming();
            if streaming_told != Some(on) {
                conn.send(XrServerMsg::Streaming { on })?;
                (self.shared.log)(&format!("xr host: stream {}", if on { "on" } else { "off" }));
                if streaming_told.is_some() {
                    // a paused headset drops the pictures that reach it, and one that resumes has
                    // none of the old ones: those written and not reported are out of flight, or
                    // each would hold its slot for good and the app build ever less
                    core.tiles_out(stats.forget_unreported());
                    // (on again: the decoder starts from a key picture)
                    if on {
                        core.keyframe();
                    }
                }
                streaming_told = Some(on);
            }
            let headset = *sh.headset_settings.lock().unwrap();
            if headset_told != Some(headset) {
                conn.send(headset_message(&headset))?;
                (self.shared.log)(&format!("xr host: headset settings {headset:?}"));
                headset_told = Some(headset);
            }
            // the app's ask of the cameras (only of a headset that captures)
            let ask = sh.capture_ask_for(session);
            if sh.capture_ok.load(Ordering::Acquire) && capture_told != Some(ask) {
                let msg = match ask {
                    Some(a) => XrServerMsg::Capture { on: true, step_deg: a.step_deg, step_m: a.step_m, preview_hz: a.preview_hz },
                    None => XrServerMsg::Capture { on: false, step_deg: 0.0, step_m: 0.0, preview_hz: 0 },
                };
                conn.send(msg)?;
                (self.shared.log)(&format!("xr host: capture {}", if ask.is_some() { "on" } else { "off" }));
                capture_told = Some(ask);
            }
            // the app's ask of the microphone (only of a headset that has one)
            let mic = sh.mic_asked(session);
            if sh.mic_ok.load(Ordering::Acquire) && mic_told != mic {
                conn.send(XrServerMsg::Mic { on: mic })?;
                (self.shared.log)(&format!("xr host: microphone {}", if mic { "on" } else { "off" }));
                mic_told = mic;
            }
            // the app's controller buzzes (only to a headset that plays them)
            let haptics = std::mem::take(&mut *sh.haptics.lock().unwrap());
            for h in &haptics {
                let msg = XrServerMsg::Haptic { hand: h.hand, amplitude: h.amplitude, duration_us: (h.seconds * 1e6) as u32 };
                conn.send(msg)?;
            }
            let see = sh.see_through.load(Ordering::Acquire);
            if see_through_told != Some(see) {
                conn.send(XrServerMsg::SeeThrough { on: see })?;
                see_through_told = Some(see);
            }
            // another display rate asked (`set_refresh_hz`): the headset is told and the session
            // ends; it links again at that rate (new encoders)
            let hz = sh.refresh_hz.load(Ordering::Acquire);
            if hz != 0 && hz != core.fps {
                if core.handheld {
                    if !handheld_rate_logged {
                        (sh.log)("xr host: a handheld keeps its camera's rate; no refresh asked");
                        handheld_rate_logged = true;
                    }
                } else {
                    conn.send(XrServerMsg::Refresh { mhz: hz * 1000 })?;
                    return Err(format!("the display rate goes from {} to {hz} Hz", core.fps));
                }
            }
            // the headset's poses stopped (asleep, taken off, not visible): the app is woken once so
            // its desktop is no longer held for the headset (`ServerShared::frames_flowing`)
            let now_flowing = sh.frames_flowing();
            if flowing && !now_flowing {
                sh.wake();
            }
            flowing = now_flowing;
            if conn.last_heard.elapsed() > Duration::from_secs(6) {
                return Err("the headset was silent for 6 s".into());
            }
            loop {
                match core.out.try_recv() {
                    Ok(Out::Video(head, data, encoded_at)) => {
                        let mut head = head;
                        head.stages[stage::SEND] = encoded_at.elapsed().as_micros() as u32;
                        let msg = encode_xr_video(&head, &data);
                        let write_at = Instant::now();
                        if let Err(e) = write_all(&mut conn.stream, &msg) {
                            return Err(format!("write: {e}"));
                        }
                        core.tiles_out(stats.sent(&head, msg.len(), write_at.elapsed()));
                        self.pictures.fetch_add(1, Ordering::AcqRel);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Err("the encoder stopped".into()),
                }
            }
            conn.read()?;
            while let Some((msg, bytes)) = conn.next(&self.shared.log, false)? {
                match msg {
                    XrClientMsg::Pose { seq, sample_ns, display_ns, frame } if conn.visible => {
                        stats.poses += 1;
                        if !core.pose(seq, sample_ns, display_ns, &frame) {
                            stats.refused_poses += 1;
                        }
                    }
                    XrClientMsg::Pose { .. } => {}
                    XrClientMsg::State { visible: v, focused } => {
                        if v && !conn.visible {
                            core.keyframe();
                        }
                        conn.state(v, focused, &self.shared.log);
                    }
                    XrClientMsg::Stats { seq, decode_us, .. } => {
                        stats.decode_us.push(decode_us);
                        core.tiles_out(stats.reported(seq, decode_us));
                    }
                    XrClientMsg::Ping { client_ns } => conn.pong(client_ns)?,
                    XrClientMsg::Keyframe => core.keyframe(),
                    XrClientMsg::World { voxel_size, chunk_edge, bricks } => {
                        stats.bricks += bricks.len() as u64;
                        stats.brick_bytes += bytes as u64;
                        (self.world)(voxel_size, chunk_edge, bricks);
                    }
                    XrClientMsg::Report { values } => self.shared.push_report(values),
                    XrClientMsg::Capture { seq, part, parts, data } => {
                        stats.capture_bytes += bytes as u64;
                        match capture.push(seq, part, parts, &data) {
                            Some(Ok(record)) => self.shared.capture_record(session, record),
                            // (the error names the record's fault, never its contents)
                            Some(Err(e)) => (self.shared.log)(&format!("xr host: capture: {e}")),
                            None => {}
                        }
                    }
                    XrClientMsg::Mic { state, text, pcm, .. } => {
                        stats.mic_bytes += pcm.len() as u64;
                        if !pcm.is_empty() {
                            self.shared.mic_event(session, MicEvent::Pcm(pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()));
                        } else {
                            self.shared.mic_event(session, MicEvent::State { state, text });
                        }
                    }
                    XrClientMsg::Recenter => self.shared.new_space("the wearer recentered"),
                    XrClientMsg::Hello { .. } => return Err("a second hello".into()),
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
    /// Per picture: its pose's arrival to written to the socket (the stages' sum, µs).
    desk_us: Vec<u32>,
    write_us: Vec<u32>,
    decode_us: Vec<u32>,
    /// Per tile: from its last byte written to the headset's report that it took it, less the
    /// headset's own time on it (the link's round trip with whatever queue it holds).
    trip_us: Vec<u32>,
    /// The tiles written and not yet reported: (seq, when written). Kept across the windows: each
    /// goes out of flight once, by its report, a later picture's, a pause or resume, or the
    /// list's end.
    unreported: std::collections::VecDeque<(u64, Instant)>,
    /// Room-field bricks received, and their messages' bytes.
    bricks: u64,
    brick_bytes: u64,
    /// Capture chunks' bytes received (the headset's cameras).
    capture_bytes: u64,
    /// Microphone sound bytes received (the wearer's voice: counted, never logged).
    mic_bytes: u64,
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

/// The view size accepted by the pack (four luma bytes per shared texel).
pub(super) fn aligned_eye(eye: [u32; 2]) -> [u32; 2] {
    eye.map(|n| n.clamp(64, 4096) & !3)
}

/// A link's own session id, from the OS's random source. The capture ask is tied to it, so a
/// link whose id cannot be drawn fails: ids that all came out the same would carry an ask
/// across links.
pub(super) fn session_id(random: impl FnOnce(&mut [u8]) -> std::io::Result<()>) -> Result<u64, String> {
    let mut b = [0u8; 8];
    random(&mut b).map_err(|e| format!("no session id (random source: {e})"))?;
    Ok(u64::from_le_bytes(b))
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
        PumpStats { at: Instant::now(), poses: 0, refused_poses: 0, pictures: 0, bytes: 0, stages_us: Default::default(), desk_us: Vec::new(), write_us: Vec::new(), decode_us: Vec::new(), trip_us: Vec::new(), unreported: Default::default(), bricks: 0, brick_bytes: 0, capture_bytes: 0, mic_bytes: 0, rate: RateWindow::new() }
    }

    /// A tile written; the tiles out of flight with it: the oldest one unreported when it no
    /// longer fits the list (its report, if one comes, finds nothing).
    pub(crate) fn sent(&mut self, head: &VideoHead, bytes: usize, write: Duration) -> u64 {
        // a picture counted once, at its first tile; its stages are the slowest tile's
        if head.tile == 0 {
            self.pictures += 1;
            self.rate.pictures += 1;
            self.desk_us.push(head.stages.iter().sum());
        }
        self.bytes += bytes as u64;
        self.rate.bytes += bytes as u64;
        for (i, v) in head.stages.iter().enumerate() {
            self.stages_us[i].push(*v);
        }
        self.write_us.push(write.as_micros() as u32);
        let out = if self.unreported.len() >= 64 { self.unreported.pop_front().is_some() as u64 } else { 0 };
        self.unreported.push_back((head.seq, Instant::now()));
        out
    }

    /// The headset took a tile of picture `seq` (`decode_us` after it arrived): the tiles out of
    /// flight with it, this one and those written before it and never reported (the headset
    /// dropped them); none when it was out already (a pause or a later report took it).
    pub(crate) fn reported(&mut self, seq: u64, decode_us: u32) -> u64 {
        let Some(at) = self.unreported.iter().position(|(s, _)| *s == seq) else { return 0 };
        let (_, written) = self.unreported.remove(at).unwrap();
        // (older ones never reported: lost)
        self.unreported.drain(..at);
        let trip = (written.elapsed().as_micros() as u32).saturating_sub(decode_us);
        self.trip_us.push(trip);
        self.rate.trip_us.push(trip);
        at as u64 + 1
    }

    /// The tiles written and never reported, forgotten (the stream paused or resumed): how many.
    pub(crate) fn forget_unreported(&mut self) -> u64 {
        self.unreported.drain(..).count() as u64
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
        let no_eyes = core.flow.no_eyes.swap(0, Ordering::AcqRel);
        let kbps = core.rate.current();
        *sh.server_line.lock().unwrap() = format!("server {} Hz · built {:.0}/s · sent {:.1} of {:.0} Mbit/s · render {:.1} gpu {:.1} encode {:.1} ms",
            core.fps, built as f64 / secs, self.bytes as f64 * 8.0 / secs / 1e6, sh.bitrate_kbps.load(Ordering::Acquire) as f64 / 1000.0, median_ms(&mut self.stages_us[0].clone()), median_ms(&mut self.stages_us[1].clone()), median_ms(&mut self.stages_us[2].clone()));
        let [ref mut render, ref mut gpu, ref mut encode, ref mut send] = self.stages_us;
        log(&format!(
            "xr host: poses {:.0}/s ({} no XR window) · built {:.0}/s ({} without a slot, {} with no eyes) · sent {:.0}/s {:.1} Mbit/s (rate {:.0}) · render {:.1} gpu {:.1} encode {:.1} send {:.1} (write {:.1}) ms · headset decode {:.1} ms · in flight {}",
            self.poses as f64 / secs,
            self.refused_poses,
            built as f64 / secs,
            skipped,
            no_eyes,
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
        if self.mic_bytes > 0 {
            log(&format!("xr host: microphone {:.1} s of sound from the headset", self.mic_bytes as f64 / (2.0 * MIC_RATE as f64)));
        }
        if self.capture_bytes > 0 {
            log(&format!("xr host: capture {:.1} MB/s from the headset", self.capture_bytes as f64 / secs / 1e6));
        }
        // pictures a frame or more later than the median (the ones a headset's lead misses): the
        // host's (pose to written) and the link's (its round trip over the idle link's)
        let period_us = 1e6 / core.fps.max(1) as f64;
        let desk_median = median_ms(&mut self.desk_us) * 1000.0;
        let host_late = self.desk_us.iter().filter(|d| **d as f64 > desk_median + period_us).count();
        let trip_base_us = core.rate.base_ms() * 1000.0;
        let link_late = self.trip_us.iter().filter(|t| **t as f64 > trip_base_us + period_us).count() / core.tiles.max(1) as usize;
        let held = core.flow.held.swap(0, Ordering::AcqRel);
        let app_wait_ms = median_ms(&mut std::mem::take(&mut *core.flow.app_wait_us.lock().unwrap()));
        let dropped = core.flow.dropped.swap(0, Ordering::AcqRel);
        if dropped > 0 {
            log(&format!("xr host: the encoder dropped {dropped} eye pictures ({:.1}/s): those pictures never reach the headset whole", dropped as f64 / secs));
        }
        let trip_ms = median_ms(&mut self.trip_us);
        let trip_min_ms = self.trip_us.first().map_or(0.0, |v| *v as f64 / 1000.0);
        log(&format!("xr host: link trip {trip_ms:.1} ms (min {trip_min_ms:.1}, base {:.1}); the app waited for the headset {:.0}/s", core.rate.base_ms(), held as f64 / secs));
        {
            let mut stats = sh.server_stats.lock().unwrap();
            *stats = super::ServerStats {
                window: stats.window + 1,
                fps: core.fps,
                poses_per_s: core.flow.poses.swap(0, Ordering::AcqRel) as f64 / secs,
                built_per_s: built as f64 / secs,
                sent_mbps: self.bytes as f64 * 8.0 / secs / 1e6,
                rate_mbps: kbps as f64 / 1000.0,
                cap_mbps: sh.bitrate_kbps.load(Ordering::Acquire) as f64 / 1000.0,
                render_ms: median_ms(render),
                app_wait_ms,
                gpu_ms: median_ms(gpu),
                encode_ms: median_ms(encode),
                send_ms: median_ms(send),
                trip_ms,
                trip_base_ms: core.rate.base_ms(),
                waited_per_s: held as f64 / secs,
                host_late_per_s: host_late as f64 / secs,
                link_late_per_s: link_late as f64 / secs,
            };
        }
        // the rate's window and the tiles in flight go on into the next window
        let (rate, unreported) = (std::mem::replace(&mut self.rate, RateWindow::new()), std::mem::take(&mut self.unreported));
        *self = PumpStats::new();
        (self.rate, self.unreported) = (rate, unreported);
    }
}


fn headset_message(h: &HeadsetSettings) -> XrServerMsg {
    XrServerMsg::Headset { foveation: h.foveation, eye_min_pct: h.eye_min_pct, eye_max_pct: h.eye_max_pct, multimodal: h.multimodal, occlusion: h.occlusion, ahead: h.ahead }
}

#[cfg(test)]
mod tests {
    use super::{Change, Control};
    use makepad_desk_link::xr_pair::{ServerPairing, ViewerPair};

    /// The LAN rule: the server listens only while a device is on, a PIN typed in this run waits
    /// for its device, or the launch asked; neither the launch's off nor the caption's off loses
    /// which device the user turned on.
    #[test]
    fn listens_only_when_asked_and_keeps_the_choice() {
        let (a, b) = ([1u8; 65], [2u8; 65]);
        let viewers = vec![ViewerPair { point: a, name: "quest".into() }, ViewerPair { point: b, name: "ipad".into() }];
        // (no file: kept in memory only)
        let mut pairing = ServerPairing::load(None);
        pairing.viewers = viewers;
        let mut c = Control { pairing, forced: false, held_off: false, remembered: None, pin_suspended: false };
        assert!(!c.listens(), "all off and no PIN: nothing listens");
        c.apply(Change::Pin("1234".into())).unwrap();
        assert!(!c.listens(), "a PIN still being typed opens nothing");
        c.apply(Change::Pin("12345".into())).unwrap();
        assert!(c.listens(), "a whole PIN listens for its device");
        c.apply(Change::Pin(String::new())).unwrap();
        c.apply(Change::Device(b, true)).unwrap();
        assert!(c.listens() && c.pairing.on == Some(b), "a device switched on listens");
        c.apply(Change::Device(a, true)).unwrap();
        assert_eq!(c.pairing.on, Some(a), "one device on at a time");
        c.apply(Change::Device(b, false)).unwrap();
        assert_eq!(c.pairing.on, Some(a), "switching off a device that is off changes nothing");
        c.apply(Change::Launch(false)).unwrap();
        assert!(!c.listens() && c.pairing.on == Some(a), "the launch's off holds the listener off and keeps the choice");
        c.apply(Change::Device(a, true)).unwrap();
        assert!(c.listens(), "a switch the user changes listens again");
        c.apply(Change::Enabled(false)).unwrap();
        assert!(!c.listens() && c.pairing.on.is_none(), "the caption's off is the real off");
        c.apply(Change::Enabled(true)).unwrap();
        assert!(c.listens() && c.pairing.on == Some(a), "the caption's on brings the same device back");
        c.apply(Change::Forget(a)).unwrap();
        c.apply(Change::Enabled(false)).unwrap();
        c.apply(Change::Enabled(true)).unwrap();
        assert!(c.listens() && c.pairing.on.is_none(), "with none to bring back the caption's on listens with all off");
    }
}
