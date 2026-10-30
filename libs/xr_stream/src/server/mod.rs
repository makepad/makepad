//! The desktop end of remote rendering: an app's XR server ([`XrServer`]), in the app's own
//! process. The app does nothing stream-specific: it renders its XR pass as on any headset
//! (`DrawPass::set_as_xr_pass`) and the server takes each frame from there (the Cx XR frame
//! sink, `makepad_draw::xr_sink`). The caption's XR control (`crate::panel`) installs it.
//!
//! A headset links (fleet TLS, `Need::XrViewer`; found by its PIN, `makepad_desk_link::xr_pair`)
//! and says its eye size and refresh: the process gets a remote headset slot (an offscreen XR
//! display, `makepad_hal::xr_remote`) whose frames are the headset's poses. Each frame's colour
//! and depth are packed on the GPU into an encode surface ([`sink`]), the picture's eye columns
//! encoded at once on the GPU's encode engines ([`pipeline`], the tile fan-out and rate
//! controller of every Desk Link stream) and sent with the eyes of the pose it was drawn with
//! ([`session`]). One headset at a time.
//!
//! Pipelined and bounded: the app is asked for the next pose as soon as it built a frame and
//! there is room (a picture slot free, fewer than `in_flight` pictures built and not yet
//! decoded by the headset); while there is none it renders nothing more, woken when there is
//! again. A slow link or decoder makes the server render less, never queue more.
//!
//! Threads: the listener; per session its TLS thread (reads poses and stats, writes pictures),
//! its picture thread (matches a picture to its pose and fans it out) and an encode thread per
//! eye. Nothing runs on the app's UI thread but the pack ([`sink`]).

mod debug;
mod foveation;
mod pipeline;
mod session;
mod sink;

pub use pipeline::picture_size;
pub use sink::DrawXrBand;
pub(crate) use sink::script_mod;
use crate::settings_types::{HeadsetSettings, StreamAa, StreamView, MIN_RENDER_SCALE_PCT, STREAM_AA_MODES};

use makepad_desk_link::xr::WorldBrick;
use makepad_desk_link::xr_pair::ServerPairing;
use makepad_widgets::*;
use pipeline::Live;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// The pictures are NV12 textures, one per tile, the pack rendering into their planes (the
/// encoder takes each as it lies): D3D11. Elsewhere one packed picture (BGRA8 texels holding
/// the NV12 bytes of both eye columns) the encoders read their columns of.
pub(crate) fn nv12_planes() -> bool {
    cfg!(target_os = "windows")
}

/// How recent the headset's last pose is while its frames count as flowing (ns).
pub(crate) const FLOWING_NS: u64 = 250_000_000;

/// The display rates a headset may run at (Hz).
pub const REFRESH_RATES: [u32; 4] = [72, 80, 90, 120];

/// The server's numbers of its last stats window (5 s; medians per picture): for the panel and
/// the automatic stream settings. `window` counts the windows (0: none yet).
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

/// The headset linked now.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveHeadset {
    pub instance: u64,
    pub name: String,
    pub session: u64,
}

/// What the server's threads and the app's side share (one per server; was the process's
/// statics).
pub(crate) struct ServerShared {
    pub(crate) log: Log,
    /// The session the app's pictures go to.
    pub(crate) live: Mutex<Option<Live>>,
    /// The app uses the headset's room field (`Cx::xr_set_room_field`): the headset is told,
    /// and integrates and sends its field only then.
    pub(crate) room_field_wanted: AtomicBool,
    /// The user's power switches: the stream runs (pictures and sound to the headset), the room
    /// scan is allowed (the headset scans only when this and the app's want are both on); the
    /// left menu button turns the stream on and off (the app may take the button over).
    pub(crate) streaming: AtomicBool,
    pub(crate) room_scan: AtomicBool,
    pub(crate) menu_toggles: AtomicBool,
    pub(crate) menu_down: AtomicBool,
    /// The headset shows the room through the picture (mixed reality) or not (VR): the app's
    /// ask (`Cx::xr_set_see_through`) under the user's view.
    pub(crate) see_through: AtomicBool,
    pub(crate) app_see_through: AtomicBool,
    pub(crate) view: AtomicU8,
    /// How many times the stream was turned on or off (the panel follows it).
    pub(crate) streaming_changes: AtomicU32,
    /// When the headset's last pose came (`makepad_hal::clock_ns`; 0: none): the desktop is held
    /// for the headset only while its frames flow (`frames_flowing`).
    pub(crate) pose_ns: AtomicU64,
    /// The display rate asked of a streaming headset (Hz; 0: its own), the stream's bitrate
    /// (kbit/s, all tiles: the rate controller never goes above it).
    pub(crate) refresh_hz: AtomicU32,
    pub(crate) bitrate_kbps: AtomicU32,
    /// The stream's AA (an index into `STREAM_AA_MODES`) and the eye render scale (percent).
    pub(crate) aa: AtomicU8,
    pub(crate) render_scale_pct: AtomicU32,
    /// The desktop's mirror repaints from the headset's frames.
    pub(crate) mirror: AtomicBool,
    /// The headset's per-second reports (`XrClientMsg::Report`), the last minute, oldest first.
    pub(crate) reports: Mutex<VecDeque<Vec<f32>>>,
    pub(crate) headset_settings: Mutex<HeadsetSettings>,
    /// The app's controller buzzes (`Cx::xr_haptic`) for a headset that plays them, oldest
    /// first; the session sends them down the link.
    pub(crate) haptics: Mutex<Vec<makepad_widgets::makepad_platform::event::xr::XrHapticPulse>>,
    /// The linked headset plays haptics (`codecs::HAPTICS` in its hello).
    pub(crate) haptics_ok: AtomicBool,
    pub(crate) pairing: Mutex<Option<Arc<Mutex<ServerPairing>>>>,
    pub(crate) live_headset: Mutex<Option<LiveHeadset>>,
    pub(crate) server_line: Mutex<String>,
    pub(crate) server_stats: Mutex<ServerStats>,
    pub(crate) app_line: Mutex<String>,
    /// Told when there is room again (a picture slot freed, or the headset decoded one), the
    /// stream switched, or a session ended: the sink's signal.
    pub(crate) wake: Mutex<Option<SignalToUI>>,
}

impl ServerShared {
    fn new(log: Log) -> ServerShared {
        ServerShared {
            log,
            live: Mutex::new(None),
            room_field_wanted: AtomicBool::new(false),
            streaming: AtomicBool::new(true),
            room_scan: AtomicBool::new(true),
            menu_toggles: AtomicBool::new(true),
            menu_down: AtomicBool::new(false),
            see_through: AtomicBool::new(true),
            app_see_through: AtomicBool::new(true),
            view: AtomicU8::new(0),
            streaming_changes: AtomicU32::new(0),
            pose_ns: AtomicU64::new(0),
            refresh_hz: AtomicU32::new(0),
            bitrate_kbps: AtomicU32::new(80_000),
            aa: AtomicU8::new(3),
            render_scale_pct: AtomicU32::new(100),
            mirror: AtomicBool::new(true),
            reports: Mutex::new(VecDeque::new()),
            headset_settings: Mutex::new(HeadsetSettings::DEFAULT),
            haptics: Mutex::new(Vec::new()),
            haptics_ok: AtomicBool::new(false),
            pairing: Mutex::new(None),
            live_headset: Mutex::new(None),
            server_line: Mutex::new(String::new()),
            server_stats: Mutex::new(ServerStats::default()),
            app_line: Mutex::new(String::new()),
            wake: Mutex::new(None),
        }
    }

    pub(crate) fn wake(&self) {
        if let Some(s) = self.wake.lock().unwrap().as_ref() {
            s.set();
        }
    }

    pub(crate) fn set_streaming(&self, on: bool) {
        if self.streaming.swap(on, Ordering::AcqRel) != on {
            self.streaming_changes.fetch_add(1, Ordering::AcqRel);
            makepad_desk_link::xr_audio::set_streaming(on);
            self.wake();
        }
    }

    /// A headset is linked, the stream runs and its poses come (the last one within
    /// `FLOWING_NS`): only then are the desktop windows held for it. A linked headset that sends
    /// none (asleep, taken off, not visible) never holds the desktop.
    pub(crate) fn frames_flowing(&self) -> bool {
        let at = self.pose_ns.load(Ordering::Acquire);
        at != 0 && self.streaming() && self.session_eye().is_some() && makepad_widgets::makepad_platform::makepad_hal::clock_ns().saturating_sub(at) < FLOWING_NS
    }

    pub(crate) fn streaming(&self) -> bool {
        self.streaming.load(Ordering::Acquire)
    }

    pub(crate) fn push_report(&self, values: Vec<f32>) {
        let mut r = self.reports.lock().unwrap();
        r.push_back(values);
        while r.len() > 60 {
            r.pop_front();
        }
    }

    pub(crate) fn apply_view(&self) {
        let see = match self.view.load(Ordering::Acquire) {
            1 => true,
            2 => false,
            _ => self.app_see_through.load(Ordering::Acquire),
        };
        self.see_through.store(see, Ordering::Release);
    }

    /// The tiles (eye columns encoded apart) of the session now (0: none streams).
    pub(crate) fn session_tiles(&self) -> u32 {
        self.live.lock().unwrap().as_ref().map_or(0, |l| l.tiles)
    }

    pub(crate) fn session_eye(&self) -> Option<[u32; 2]> {
        self.live.lock().unwrap().as_ref().map(|l| l.eye)
    }

    /// Whether the app may build the next frame: a headset streams and fewer than its limit of
    /// pictures are in flight to it.
    pub(crate) fn room(&self) -> bool {
        let live = self.live.lock().unwrap();
        let Some(l) = live.as_ref() else { return false };
        let room = l.flow.in_flight(l.tiles) < l.limit;
        if !room {
            l.flow.held.fetch_add(1, Ordering::AcqRel);
        }
        room
    }

    /// A picture for the headset; false when no headset streams (it is dropped, and with it
    /// its hold on the slot).
    pub(crate) fn submit(&self, picture: pipeline::Picture) -> bool {
        let live = self.live.lock().unwrap();
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
    pub(crate) fn note_skipped(&self) {
        if let Some(l) = self.live.lock().unwrap().as_ref() {
            l.flow.skipped.fetch_add(1, Ordering::AcqRel);
        }
    }
}

/// The app's XR server (see the module): a handle (clones share it). One per process, kept in
/// the Cx ([`XrServer::get`]).
#[derive(Clone)]
pub struct XrServer {
    pub(crate) shared: Arc<ServerShared>,
    host: Arc<session::Host>,
}

/// The process's server in the Cx.
#[derive(Default)]
struct XrServerSlot {
    server: Option<XrServer>,
    tried: bool,
}

/// The stream switch this launch starts with, when one is given: `MAKEPAD_XR_STREAM=off|on`
/// (also `0|1`) or `--xr-stream=off|on`; it overrides the app's kept switch for this launch (a
/// change in the panel is kept again).
pub fn launch_enabled() -> Option<bool> {
    let parse = |v: &str| match v.trim().to_ascii_lowercase().as_str() {
        "off" | "0" | "false" | "no" => Some(false),
        "on" | "1" | "true" | "yes" => Some(true),
        _ => None,
    };
    std::env::args().find_map(|a| a.strip_prefix("--xr-stream=").and_then(parse)).or_else(|| std::env::var("MAKEPAD_XR_STREAM").ok().as_deref().and_then(parse))
}

impl XrServer {
    /// Starts this process's XR server for the app `app` (once: later calls return the same
    /// server; None where none can run: the web, or the LAN not allowed for this instance): on
    /// a port the OS picks, found by headsets with the pairing probe; the pairing is kept in
    /// the app's data folder (`xr-pairing`). From then on the app's XR pass frames go to it.
    pub fn install(cx: &mut Cx, app: &str) -> Option<XrServer> {
        if cx.global::<XrServerSlot>().tried {
            return XrServer::get(cx);
        }
        cx.global::<XrServerSlot>().tried = true;
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        let log: Log = Arc::new(|line: &str| log!("{line}"));
        if !makepad_widgets::makepad_platform::makepad_network::lan::service("xr host") {
            log("xr host: local network off for this instance (MAKEPAD_LAN=1 allows it)");
            return None;
        }
        let shared = Arc::new(ServerShared::new(log.clone()));
        let signal = SignalToUI::new();
        *shared.wake.lock().unwrap() = Some(signal.clone());
        // the headset's room field, mirrored into this process's field store: the app's physics
        // (its hit-surface queries) collides with the real room
        let mirror: Mutex<Option<makepad_widgets::makepad_platform::xr_tsdf::BrickMirror>> = Mutex::new(None);
        let world: session::WorldSink = Arc::new(move |voxel_size, chunk_edge, bricks: Vec<WorldBrick>| {
            use makepad_widgets::makepad_platform::xr_tsdf::{Brick, BrickGrid, BrickMirror, ChunkKey};
            let bricks: Vec<Brick> = bricks.into_iter().map(|b| Brick { key: ChunkKey { x: b.key[0], y: b.key[1], z: b.key[2] }, version: b.version, payload: Arc::new(b.payload) }).collect();
            let mut mirror = mirror.lock().unwrap_or_else(|e| e.into_inner());
            mirror.get_or_insert_with(BrickMirror::default).apply(&makepad_widgets::makepad_platform::xr_tsdf::xr_tsdf_store(), BrickGrid { voxel_size, chunk_edge }, &bricks);
        });
        let pairing = makepad_widgets::makepad_platform::home::app_data_dir(app).map(|d| d.join("xr-pairing"));
        let host = match session::Host::new(&shared, app, pairing, world) {
            Ok(h) => Arc::new(h),
            Err(e) => {
                log(&e);
                return None;
            }
        };
        let server = XrServer { shared, host };
        // on unless the app's kept switch or this launch says off
        let on = launch_enabled().unwrap_or_else(|| crate::panel::XrStreamSettings::load(app).streaming);
        server.set_enabled(on);
        // the app's XR pass frames from now on, and its sound
        let pack = sink::XrPack::new(cx, server.clone(), signal);
        makepad_widgets::makepad_draw::xr_sink::set_xr_frame_sink(cx, Box::new(pack));
        sink::tap_audio_output();
        let slot = cx.global::<XrServerSlot>();
        slot.server = Some(server.clone());
        Some(server)
    }

    /// The process's server, once installed.
    pub fn get(cx: &mut Cx) -> Option<XrServer> {
        cx.global::<XrServerSlot>().server.clone()
    }

    /// The desktop's stream switch: off, nothing of the server runs: it listens on no port,
    /// answers no pairing probe, a linked headset is told and its session ends (its encoders
    /// with it), the desktop paints as always. On: it listens again (on a new port; headsets
    /// find it by their probe) and a paired headset links again. The pairing and the PIN are
    /// kept either way. Also un-pauses ([`Self::set_streaming`]).
    pub fn set_enabled(&self, on: bool) {
        if on {
            self.shared.set_streaming(true);
        }
        if let Err(e) = self.host.set_listening(&self.shared, on) {
            (self.shared.log)(&format!("xr host: cannot listen: {e}"));
        }
        self.shared.wake();
    }

    pub fn enabled(&self) -> bool {
        self.host.listening()
    }

    /// The port headsets link to while on (None: off, or not bound yet).
    pub fn port(&self) -> Option<u16> {
        self.host.port()
    }

    /// The pause (the headset's left menu button, the panel's resume): paused, the headset
    /// stays linked but the app draws no XR frames for it, nothing is encoded or sent (sound
    /// neither), and the headset stops all of its stream work (it shows the room). On again: a
    /// key picture first. The real off is [`Self::set_enabled`].
    pub fn set_streaming(&self, on: bool) {
        self.shared.set_streaming(on);
    }

    /// A headset is linked and its frames flow (its poses come): the desktop's heavy views pause
    /// for it. A linked headset that sends none (taken off, asleep) is idle.
    pub fn frames_flowing(&self) -> bool {
        self.shared.frames_flowing()
    }

    /// Not paused (see [`Self::set_streaming`]); true while switched off too.
    pub fn streaming(&self) -> bool {
        self.shared.streaming()
    }

    /// Counts every change of [`Self::streaming`] (the headset's menu button's too).
    pub fn streaming_changes(&self) -> u32 {
        self.shared.streaming_changes.load(Ordering::Acquire)
    }

    /// Whether the left controller's menu button turns the stream on and off (default on;
    /// off: the app gets the button).
    pub fn set_menu_toggles_streaming(&self, on: bool) {
        self.shared.menu_toggles.store(on, Ordering::Release);
    }

    /// The room scan allowed (the user's switch; the headset scans while the app wants the room
    /// field and this is on).
    pub fn set_room_scan(&self, on: bool) {
        self.shared.room_scan.store(on, Ordering::Release);
    }

    /// The display rate a streaming headset runs at (Hz, one of [`REFRESH_RATES`]; 0: its
    /// own): a change re-links it at that rate (a second without pictures).
    pub fn set_refresh_hz(&self, hz: u32) {
        self.shared.refresh_hz.store(hz, Ordering::Release);
    }

    /// The stream's bitrate (kbit/s, all tiles): applied at once to a running stream; the rate
    /// controller goes below it when the link queues and back up to it.
    pub fn set_bitrate_kbps(&self, kbps: u32) {
        self.shared.bitrate_kbps.store(kbps.max(pipeline::MIN_KBPS), Ordering::Release);
    }

    /// The stream's anti-aliasing (live: the next frame renders with it).
    pub fn set_aa(&self, mode: StreamAa) {
        let i = STREAM_AA_MODES.iter().position(|m| *m == mode).unwrap_or(3);
        self.shared.aa.store(i as u8, Ordering::Release);
    }

    pub fn aa(&self) -> StreamAa {
        STREAM_AA_MODES[(self.shared.aa.load(Ordering::Acquire) as usize).min(STREAM_AA_MODES.len() - 1)]
    }

    /// The eye render scale (percent of the encoded eye size per axis, [`MIN_RENDER_SCALE_PCT`]
    /// ..100; live from the next frame): below 100 the eyes render smaller and the pack scales
    /// them up into the same encoded size, so the headset's picture keeps its size and nothing
    /// re-links. Under the AA's supersampling.
    pub fn set_render_scale(&self, pct: u32) {
        self.shared.render_scale_pct.store(pct.clamp(MIN_RENDER_SCALE_PCT, 100), Ordering::Release);
    }

    pub fn render_scale(&self) -> u32 {
        self.shared.render_scale_pct.load(Ordering::Acquire)
    }

    /// The user's view (mixed reality, VR, or as the app asks).
    pub fn set_view(&self, view: StreamView) {
        self.shared.view.store(match view { StreamView::App => 0, StreamView::Mixed => 1, StreamView::Vr => 2 }, Ordering::Release);
        self.shared.apply_view();
    }

    /// The desktop's mirror of the headset's view repaints from its frames (a few a second),
    /// else only with the desktop's own UI changes.
    pub fn set_mirror(&self, on: bool) {
        self.shared.mirror.store(on, Ordering::Release);
    }

    /// The desktop's mirror switch (`set_mirror`).
    pub fn mirror(&self) -> bool {
        self.shared.mirror.load(Ordering::Acquire)
    }

    /// The headset's settings (a streaming headset is told at once).
    pub fn set_headset_settings(&self, settings: HeadsetSettings) {
        *self.shared.headset_settings.lock().unwrap() = settings;
    }

    pub fn headset_settings(&self) -> HeadsetSettings {
        *self.shared.headset_settings.lock().unwrap()
    }

    /// A line of the app's own numbers in the stream's stats (the panel), under the stream's
    /// (empty: none). Kept until replaced.
    pub fn set_app_line(&self, line: &str) {
        let mut app = self.shared.app_line.lock().unwrap();
        if *app != line {
            app.clear();
            app.push_str(line);
        }
    }

    pub fn app_line(&self) -> String {
        self.shared.app_line.lock().unwrap().clone()
    }

    /// The eye size (pixels) of the headset now streaming. None while none streams.
    pub fn session_eye(&self) -> Option<[u32; 2]> {
        self.shared.session_eye()
    }

    /// The headset linked now.
    pub fn live_headset(&self) -> Option<LiveHeadset> {
        self.shared.live_headset.lock().unwrap().clone()
    }

    /// The server's pairing: its instance id, the PIN typed, the paired headset, the headset
    /// that PIN matched, a clash.
    pub fn pairing(&self) -> Option<ServerPairing> {
        self.shared.pairing.lock().unwrap().as_ref().map(|p| p.lock().unwrap().clone())
    }

    /// The PIN as the user types it (a headset's PIN once it has
    /// `makepad_desk_link::xr_pair::PIN_DIGITS` digits; empty: none). Any change unpairs the
    /// paired headset (told, its link ends; both forget); a whole PIN then looks for the
    /// headset showing it.
    pub fn set_pin(&self, pin: &str) {
        let Some(p) = self.shared.pairing.lock().unwrap().clone() else { return };
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

    /// The server's numbers of its last stats window, one line.
    pub fn server_line(&self) -> String {
        self.shared.server_line.lock().unwrap().clone()
    }

    pub fn stats(&self) -> ServerStats {
        *self.shared.server_stats.lock().unwrap()
    }

    /// The headset's last minute of per-second reports (`makepad_desk_link::xr::report` names
    /// the values), oldest first.
    pub fn headset_reports(&self) -> Vec<Vec<f32>> {
        self.shared.reports.lock().unwrap().iter().cloned().collect()
    }
}
