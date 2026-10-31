//! Desk Link: the WM's remote desktop. A viewer (apps/desklink) on the LAN sees one screen of
//! this desktop as H.264 from the GPU's hardware encoder and drives it with its keyboard, mouse,
//! wheel and gamepads, which arrive here as local devices. Off unless its settings enable it
//! (`settings`); fleet credentials and TLS always (`server`). The picture is the WM's own frame
//! without its cursor (`capture`); the cursor goes separately, so it never waits for video.
//! The wire format: `libs/desk_link`, documented in apps/desklink/PROTOCOL.md.

pub mod capture;
#[cfg(not(target_arch = "wasm32"))]
pub mod encode;
#[cfg(target_os = "linux")]
pub mod inject;
pub mod settings;

/// A Desk Link event: on the WM's log and in `~/.makepad/wm/desk-link.log` (the WM's own
/// output is not always readable where it runs as the desktop).
pub fn note(text: &str) {
    makepad_widgets::log!("wm: {text}");
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Write;
        let path = crate::theme::makepad_home().join("wm/desk-link.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            let _ = writeln!(f, "{secs:.3} {text}");
        }
    }
}

pub fn script_mod(vm: &mut makepad_widgets::ScriptVm) {
    capture::script_mod(vm);
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::DeskLink;

/// `limit::*` in words (the logs).
pub fn limit_name(limit: u8) -> &'static str {
    use makepad_desk_link::limit;
    match limit {
        limit::START => "start",
        limit::CLEAN => "clean",
        limit::LINK => "link-bound",
        limit::ENCODER => "encoder-bound",
        limit::CAP => "capped",
        limit::FIXED => "fixed",
        _ => "?",
    }
}

/// The deciding viewer's (the last connection's) picture area (device pixels), its display refresh (mHz) and scale
/// (hundredths): what a desktop that follows its viewer (a virtual display) takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewerWindow {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    pub scale: u32,
}

/// Someone viewing this desktop, for the bar's Desk Link item and its panel (the row Desk Host's
/// tray dropdown shows too).
pub use makepad_desk_link_panel::ViewerRow as DeskLinkViewer;

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
pub struct DeskLink;
#[cfg(target_arch = "wasm32")]
impl DeskLink {
    pub fn handle_event(&mut self, _cx: &mut makepad_widgets::Cx, _event: &makepad_widgets::Event) {}
    pub fn viewers(&self) -> Vec<DeskLinkViewer> {
        Vec::new()
    }
    pub fn generation(&self) -> u32 {
        0
    }
    pub fn disconnect(&mut self, _session: u32) {}
    pub fn take_viewer_window(&mut self) -> Option<ViewerWindow> {
        None
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::capture::{CaptureState, Want};
    use makepad_desk_link::server::{self, Control, Feed, Geometry, Input, Out, Server, ServerConfig, ToUi};
    use super::settings::DeskLinkSettings;
    use makepad_desk_link::{self as proto, ScreenInfo, ServerMsg};
    use super::encode::{EncodeStats, Encoder, Job, Subscribers, TilePlan};
    use makepad_widgets::makepad_platform::video_file::{GpuCompletion, SurfaceFormat, SurfaceHandle};
    use makepad_widgets::makepad_platform::video_file::{StreamVideoCodec, VideoStreamEncoder};
    use makepad_desk_link::rate::effort;
    use makepad_widgets::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// How long the desktop holds still before its picture is sent again near lossless (s).
    const REFINE_AFTER: f64 = 0.12;


    struct Viewer {
        session: u32,
        name: String,
        /// The viewer's address (IP only) and since when it is here.
        peer: String,
        /// Its decoder takes 4:4:4.
        full_chroma: bool,
        /// Its display's refresh (Hz; 0 = not said yet).
        refresh_hz: u16,
        since: std::time::Instant,
        out: std::sync::mpsc::Sender<Out>,
        /// Its pictures: backlog, pause, whether it follows the stream (`server::Feed`).
        feed: Arc<Feed>,
        /// From its last `Stats`: pictures it was behind the last one sent, its decode and
        /// present time (µs).
        shown: Option<(u32, u32, u32)>,
        /// Its picture area: device pixels and scale (hundredths), from its `Window`.
        window: Option<(u32, u32, u16)>,
        /// `caps::VIEW_ONLY`: its window never decides the desktop's size.
        view_only: bool,
        /// Its decoder takes H.265.
        hevc: bool,
        /// Its `Encoding` choices.
        prefs: Prefs,
    }

    /// A viewer's `Encoding` (`proto::pref::*`; 0 = auto each).
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    struct Prefs {
        chroma: u8,
        codec: u8,
        preset: u8,
        mbps: u16,
        fps: u16,
    }

    /// How the stream is encoded for a size, and why (the viewers' stats line).
    #[derive(Clone, Debug, PartialEq)]
    struct Choice {
        hevc: bool,
        full_chroma: bool,
        sharp: bool,
        /// A band per engine (H.265; H.264 always spreads over the engines).
        spread: bool,
        why: String,
    }


    #[derive(Default)]
    pub struct DeskLink {
        settings: Option<DeskLinkSettings>,
        server: Option<Server>,
        signal: SignalToUI,
        viewers: Vec<Viewer>,
        subscribers: Subscribers,
        /// The encoder thread (started with the first picture).
        encoder: Option<Encoder>,
        /// The picture textures as the encoder's surfaces and the textures they are (the
        /// capture's two, drawn in turn).
        pictures: Vec<(SurfaceHandle, Texture)>,
        screen: usize,
        /// The picture texture is not exported yet: the frame clock that tries again.
        export_next: NextFrame,
        congested: bool,
        cursor: Option<(u16, u16, u8, bool)>,
        pointer: Vec2d,
        session_seq: u32,
        /// The dump viewer (the settings' `dump`): it leaves when its file is written.
        dump_gone: Option<Arc<AtomicBool>>,
        /// A held-back picture is due: the frame clock that draws it.
        owed_next: NextFrame,
        /// Fires when the desktop held still after its last picture: the refinement.
        refine_timer: Timer,
        /// Bumped whenever a viewer comes, goes, pauses or resumes (the bar follows it).
        generation: u32,
        /// The pictures a second streamed now ([`Self::stream_fps`]).
        fps: u32,
        /// The per-second timeline in the log while a viewer watches: its clock, the encoder's
        /// and the sessions' counters, and the UI side's (frames drawn, pictures handed to the
        /// encoder, time spent congested).
        timeline: Timer,
        encode_stats: Arc<EncodeStats>,
        tl_frames: u64,
        tl_captured: u64,
        /// The last second had nothing (no frames, captures, pictures): its line was said once.
        tl_idle: bool,
        tl_congested_at: Option<f64>,
        tl_congested_s: f64,
        /// The deciding viewer's window as last handed out (`take_viewer_window`).
        viewer_window: Option<super::ViewerWindow>,
        /// Stream sizes whose 4:4:4 encode lost the GPU device: they stream 4:2:0 from then on
        /// (the same configuration again would only lose the new device too).
        lost_full_chroma: Vec<[u32; 2]>,
        /// The adaptive stream rate (`rate`), and the `Rate` last told the viewers.
        rate: makepad_desk_link::rate::RateControl,
        told_rate: Option<ServerMsg>,
    }

    impl DeskLink {
        /// Who is connected now (the dump viewer, a dev tool, is not listed).
        pub fn viewers(&self) -> Vec<super::DeskLinkViewer> {
            // every viewer gets the one stream the viewers were told
            let stream = match &self.told_rate {
                Some(ServerMsg::Rate { fps, kbps, full_chroma, codec, streams, .. }) => makepad_desk_link_panel::stream_line(*codec == proto::pref::HEVC, *full_chroma, *fps as u32, *kbps, *streams as u32),
                _ => String::new(),
            };
            self.viewers
                .iter()
                .filter(|v| v.session != 0)
                .map(|v| super::DeskLinkViewer { session: v.session, name: v.name.clone(), peer: v.peer.clone(), paused: v.feed.paused.load(Ordering::Acquire), secs: v.since.elapsed().as_secs(), stream: stream.clone() })
                .collect()
        }

        /// Changes whenever [`Self::viewers`] would.
        pub fn generation(&self) -> u32 {
            self.generation
        }

        /// Ends a viewer's session from this desktop (the bar's panel): it is told why and
        /// closed; it leaves the list when the server reports it gone.
        /// The window the desktop follows when it changed since the last call: the last
        /// connection wins (`server::size_owner`): of the viewers that said their window size
        /// and are not view-only, the one that connected last; the others see the desktop
        /// scaled. When it leaves, the one before it decides again.
        pub fn take_viewer_window(&mut self) -> Option<super::ViewerWindow> {
            let owner = server::size_owner(self.viewers.iter().map(|v| (v.session, !v.view_only && v.window.is_some())))?;
            let v = self.viewers.iter().find(|v| v.session == owner)?;
            let (width, height, scale) = v.window?;
            let refresh_mhz = if v.refresh_hz == 0 { 0 } else { v.refresh_hz as u32 * 1000 };
            let window = super::ViewerWindow { width, height, refresh_mhz, scale: scale as u32 };
            if self.viewer_window == Some(window) {
                return None;
            }
            let was = self.viewer_window.map(|w| w.scale);
            super::note(&format!(
                "desk link: the desktop follows {} ({}): {width}x{height} at {:.2}x{}",
                v.name, v.peer, scale as f64 / 100.0,
                match was { Some(s) if s != window.scale => format!(" (scale was {:.2}x)", s as f64 / 100.0), _ => String::new() }
            ));
            self.viewer_window = Some(window);
            Some(window)
        }

        pub fn disconnect(&mut self, session: u32) {
            if let Some(v) = self.viewers.iter().find(|v| v.session == session) {
                super::note(&format!("desk link: disconnecting {} ({}) at the desktop", v.name, v.peer));
                let _ = v.out.send(Out::Close("disconnected at the desktop".into()));
            }
        }

        fn start(&mut self, cx: &mut Cx) {
            let settings = DeskLinkSettings::load();
            if settings.enabled {
                let signal = self.signal.clone();
                let host = std::fs::read_to_string("/etc/hostname").map(|h| h.trim().to_string()).unwrap_or_default();
                let config = settings.listen_addr().map(|addr| ServerConfig {
                    addr,
                    tls_name: format!("makepad desk link {host}"),
                    log: Arc::new(|t: &str| super::note(t)),
                    clock: makepad_hal::clock_ns,
                    input: Arc::new(|| -> Result<Box<dyn Input>, String> {
                        #[cfg(target_os = "linux")]
                        return super::inject::Injector::new().map(|i| Box::new(i) as Box<dyn Input>).map_err(|e| format!("input unavailable on this host: /dev/uinput: {e} (it needs the uinput permission: group input, mode 0660)"));
                        #[cfg(not(target_os = "linux"))]
                        return Err("input unavailable on this host: no input injection on this desktop".into());
                    }),
                });
                match config.and_then(|c| Server::start(c, Arc::new(move || signal.set()))) {
                    Ok(server) => {
                        super::note(&format!("desk link listens on {} as {}", server.addr, server.name));
                        self.server = Some(server);
                    }
                    Err(e) => super::note(&e),
                }
            } else {
                super::note(&format!("desk link is off ({})", DeskLinkSettings::path().display()));
            }
            self.fps = settings.fps.max(1);
            cx.global::<CaptureState>().interval = 1.0 / self.fps as f64;
            cx.global::<CaptureState>().signal = Some(self.signal.clone());
            let dump = settings.dump.clone();
            self.settings = Some(settings);
            if let Some(dump) = dump {
                self.start_dump(&dump);
            }
            let _ = cx;
        }

        /// The settings' `dump = <file>[,<pictures>]` (a check of the encoder without a viewer or
        /// the network): a local viewer that writes the stream's first pictures (default 600)
        /// to an Annex-B file, then leaves.
        fn start_dump(&mut self, spec: &str) {
            let (path, count) = match spec.split_once(',') {
                Some((p, n)) => (p.to_string(), n.parse::<u32>().unwrap_or(600)),
                None => (spec.to_string(), 600),
            };
            let (tx, rx) = std::sync::mpsc::channel::<Out>();
            let feed = Arc::new(Feed::new(false));
            let (feed2, signal) = (feed.clone(), self.signal.clone());
            let gone = Arc::new(AtomicBool::new(false));
            let gone2 = gone.clone();
            let spawned = std::thread::Builder::new().name("desk-link-dump".into()).spawn(move || {
                use std::io::Write;
                // stream 0 to `path`, stream N (a tile, or an interleaved stream) to `path.N`
                let mut files: Vec<Option<std::fs::File>> = Vec::new();
                let mut written = 0;
                while let Ok(out) = rx.recv() {
                    if let Out::Video(bytes) = out {
                        let mut inbox = proto::Inbox::default();
                        inbox.feed(&bytes);
                        while let Ok(Some((ty, payload))) = inbox.next() {
                            let Ok(ServerMsg::Video { data, frame, key, built_ns, encoded_ns, stream, parts, .. }) = ServerMsg::decode(ty, &payload) else { continue };
                            let s = stream as usize;
                            if files.len() <= s {
                                files.resize_with(s + 1, || None);
                            }
                            if files[s].is_none() {
                                let name = if s == 0 { path.clone() } else { format!("{path}.{s}") };
                                match std::fs::File::create(&name) {
                                    Ok(f) => files[s] = Some(f),
                                    Err(_) => {
                                        super::note(&format!("desk link dump: cannot create {name}"));
                                        continue;
                                    }
                                }
                            }
                            let _ = files[s].as_mut().unwrap().write_all(&data);
                            if parts <= 1 || stream + 1 == parts {
                                written += 1;
                            }
                            super::note(&format!("desk link dump: picture {frame} stream {stream}{} {} bytes, encode {:.2} ms", if key { " (key)" } else { "" }, data.len(), (encoded_ns.saturating_sub(built_ns)) as f64 / 1e6));
                        }
                        feed2.backlog.fetch_sub(1, Ordering::AcqRel);
                        if written >= count {
                            break;
                        }
                    }
                }
                super::note(&format!("desk link dump: {written} pictures in {path}"));
                gone2.store(true, Ordering::Release);
                signal.set();
            });
            if spawned.is_ok() {
                self.dump_gone = Some(gone);
                self.viewers.push(Viewer { session: 0, name: "dump".into(), peer: String::new(), full_chroma: true, refresh_hz: 0, since: std::time::Instant::now(), out: tx, feed, shown: None, window: None, view_only: false, hevc: true, prefs: Prefs::default() });
                self.sync_subscribers();
            }
        }

        /// The screens viewers may pick, in desktop pixels, and the default one.
        fn screens(&self, cx: &mut Cx) -> (Vec<ScreenInfo>, usize) {
            let st = cx.global::<CaptureState>();
            let desktop = st.desktop_px;
            let mut out = Vec::new();
            #[cfg(target_os = "linux")]
            let primary = {
                let mut primary = 0;
                let snapshot = cx.linux_display_snapshot();
                for o in snapshot.outputs.iter().filter(|o| o.active && o.width > 0) {
                    let Some((x, y)) = o.desktop_position else { continue };
                    if o.primary {
                        primary = out.len();
                    }
                    out.push(ScreenInfo { name: o.name.clone(), x, y, width: o.width, height: o.height });
                }
                primary
            };
            #[cfg(not(target_os = "linux"))]
            let primary = 0;
            if out.is_empty() {
                out.push(ScreenInfo { name: "desktop".into(), x: 0, y: 0, width: desktop[0], height: desktop[1] });
            }
            let wanted = self.settings.as_ref().map(|s| s.screen.clone()).unwrap_or_default();
            let default = out.iter().position(|s| s.name == wanted).unwrap_or(primary);
            (out, default)
        }

        /// The pictures a second to stream: the settings' cap, the streamed screen's refresh and
        /// every watching viewer's display refresh, whichever is lowest (an unknown refresh does
        /// not count).
        fn stream_fps(&self, cx: &mut Cx) -> u32 {
            let mut fps = self.settings.as_ref().map_or(120, |s| s.fps).max(1);
            let (screens, _) = self.screens(cx);
            #[cfg(target_os = "linux")]
            if let Some(s) = screens.get(self.screen) {
                let snapshot = cx.linux_display_snapshot();
                if let Some(o) = snapshot.outputs.iter().find(|o| o.name == s.name && o.refresh_hz >= 1.0) {
                    fps = fps.min(o.refresh_hz.round() as u32);
                }
            }
            #[cfg(not(target_os = "linux"))]
            let _ = screens;
            for v in self.viewers.iter().filter(|v| v.refresh_hz > 0 && !v.feed.paused.load(Ordering::Acquire)) {
                fps = fps.min(v.refresh_hz as u32);
            }
            // the deciding viewer's own cap (0: its display)
            let wanted = self.prefs().fps;
            if wanted > 0 {
                fps = fps.min(wanted as u32);
            }
            fps.max(1)
        }

        /// The streamed screen's refresh (Hz), when the displays say it.
        fn screen_refresh(&self, cx: &mut Cx) -> Option<f64> {
            let (screens, _) = self.screens(cx);
            #[cfg(target_os = "linux")]
            if let Some(s) = screens.get(self.screen) {
                return cx.linux_display_snapshot().outputs.iter().find(|o| o.name == s.name && o.refresh_hz >= 1.0).map(|o| o.refresh_hz);
            }
            let _ = screens;
            None
        }

        fn stream_size_of(s: &ScreenInfo) -> [u32; 2] {
            [s.width / 4 * 4, s.height / 2 * 2]
        }

        fn watching(&self) -> bool {
            self.viewers.iter().any(|v| !v.feed.paused.load(Ordering::Acquire))
        }

        /// The choices of the viewer whose `Encoding` decides the stream: of the watching viewers,
        /// the last connection (view-only ones last), as for the size.
        fn prefs(&self) -> Prefs {
            self.viewers.iter().filter(|v| !v.feed.paused.load(Ordering::Acquire)).max_by_key(|v| (!v.view_only, v.session)).map_or(Prefs::default(), |v| v.prefs)
        }

        /// Whether a `size` stream may be 4:4:4 for these choices, and why not.
        fn can_444(&self, size: [u32; 2], p: &Prefs) -> (bool, &'static str) {
            let watching = || self.viewers.iter().filter(|v| !v.feed.paused.load(Ordering::Acquire));
            if p.chroma == proto::pref::HALF {
                (false, "4:2:0: the viewer asks for it")
            } else if !self.settings.as_ref().is_some_and(|s| s.full_chroma) {
                (false, "4:2:0: the desktop's settings say chroma = 420")
            } else if !watching().all(|v| v.full_chroma) {
                (false, "4:2:0: a viewer's decoder takes no 4:4:4")
            } else if self.lost_full_chroma.contains(&size) {
                (false, "4:2:0: 4:4:4 lost the GPU at this size")
            } else {
                (true, "")
            }
        }

        /// The most encoder effort (`rate::effort`) a `size` stream may take: a forced 4:4:4 (or
        /// a 4:2:0 stream) never gives up chroma for the encoder.
        fn max_effort(&self, size: [u32; 2]) -> u8 {
            let p = self.prefs();
            if !self.can_444(size, &p).0 || p.chroma == proto::pref::FULL { effort::FAST } else { effort::HALF }
        }

        /// How a `size` stream is encoded now: the last connection's choices within what every
        /// watching viewer decodes, the GPU encodes and the settings allow, and the step the
        /// encoder's throughput asked for (`rate::effort`).
        fn choice(&self, size: [u32; 2]) -> Choice {
            use proto::pref;
            let p = self.prefs();
            let watching = || self.viewers.iter().filter(|v| !v.feed.paused.load(Ordering::Acquire));
            let (can_444, no_444) = self.can_444(size, &p);
            let mut why: Vec<String> = Vec::new();
            let hevc = if p.codec == pref::H264 {
                false
            } else if !self.settings.as_ref().is_some_and(|s| s.hevc) {
                why.push("H.264: the desktop's settings say codec = h264".into());
                false
            } else if !watching().all(|v| v.hevc) {
                why.push("H.264: a viewer's decoder takes no H.265".into());
                false
            } else if VideoStreamEncoder::max_size(StreamVideoCodec::Hevc, can_444).is_none() {
                why.push("H.264: this GPU's encoder has no H.265".into());
                false
            } else {
                true
            };
            if p.chroma != pref::HALF && !can_444 {
                why.push(no_444.into());
            }
            let max = self.max_effort(size);
            let step = self.rate.effort(size, max);
            let full_chroma = can_444 && step < effort::HALF;
            let sharp = match p.preset {
                pref::SHARP => true,
                pref::FAST => false,
                _ => step < effort::FAST,
            };
            if hevc {
                // bands for the viewers' decoders: the size's own rule, or what a viewer's
                // measured decode asked for
                let rule = TilePlan::decode_bands(size, self.fps);
                let (asked, ms) = self.rate.decode_bands(size);
                if asked > rule {
                    why.push(format!("{asked} bands: a viewer took {ms:.1} ms to decode a picture"));
                } else if rule > 1 {
                    why.push(format!("{rule} bands: {}x{} at {} is past one decoder", size[0], size[1], self.fps));
                }
            }
            match step {
                effort::SPREAD if hevc => why.push("a stream per engine: one was behind".into()),
                effort::FAST if p.preset == pref::AUTO => why.push("fast: the encoder was behind at sharp".into()),
                effort::HALF => why.push("4:2:0: the encoder was behind at 4:4:4".into()),
                _ => {}
            }
            Choice { hevc, full_chroma, sharp, spread: step >= effort::SPREAD, why: why.join("; ") }
        }

        /// The most the stream may take (kbit/s): the settings' `max_mbps` and the watching
        /// viewers' own caps (none by default: the adaptive rate alone decides).
        fn rate_ceiling(&self, _size: [u32; 2], _full_chroma: bool) -> u32 {
            let base = self.settings.as_ref().map_or(u32::MAX, |s| s.ceiling_kbps());
            self.viewers.iter().filter(|v| !v.feed.paused.load(Ordering::Acquire) && v.prefs.mbps > 0).map(|v| v.prefs.mbps as u32 * 1000).fold(base, u32::min).max(1)
        }

        /// The whole stream's rate now (kbit/s, the encoder splits it over the tiles): the
        /// settings' fixed `kbps`, else the adaptive rate under the ceiling.
        fn stream_kbps(&self, size: [u32; 2], full_chroma: bool) -> u32 {
            match self.settings.as_ref().map_or(0, |s| s.kbps) {
                0 => self.rate.kbps(self.rate_ceiling(size, full_chroma)),
                fixed => fixed,
            }
        }

        /// The `Rate` the viewers are told: pictures a second, the rate, what holds it, chroma.
        fn rate_msg(&self, cx: &mut Cx) -> ServerMsg {
            // before the first capture: the screen the viewers watch, at its stream size
            let want = cx.global::<CaptureState>().want;
            let (screens, _) = self.screens(cx);
            let screen = screens.get(self.screen).map(Self::stream_size_of).unwrap_or([0, 0]);
            let size = want.map_or(screen, |w| w.stream);
            let choice = self.choice(size);
            let (full_chroma, hevc, sharp, streams) = want.map_or((choice.full_chroma, choice.hevc, choice.sharp, 1), |w| (w.full_chroma, w.hevc, w.sharp, w.plan.tiles()));
            let fixed = self.settings.as_ref().is_some_and(|s| s.kbps != 0);
            let limit = if fixed { proto::limit::FIXED } else { self.rate.limit(self.rate_ceiling(size, full_chroma)) };
            ServerMsg::Rate {
                fps: self.fps as u16,
                kbps: self.stream_kbps(size, full_chroma),
                limit,
                full_chroma,
                codec: if hevc { proto::pref::HEVC } else { proto::pref::H264 },
                preset: if sharp { proto::pref::SHARP } else { proto::pref::FAST },
                streams: streams.min(255) as u8,
                why: choice.why,
            }
        }

        /// The viewers hear the `Rate` when it changed since they last did (`all`: every one).
        fn tell_rate(&mut self, cx: &mut Cx) {
            let msg = self.rate_msg(cx);
            if self.told_rate.as_ref() != Some(&msg) {
                self.send_all(&msg);
                self.told_rate = Some(msg);
            }
        }

        /// A viewer behind on pictures holds the pictures back (the capture, its texture and the
        /// stream stay: no key frame when it caught up, the owed picture follows).
        fn set_congested(&mut self, cx: &mut Cx, on: bool) {
            self.congested = on;
            cx.global::<CaptureState>().hold = on;
            let now = cx.seconds_since_app_start();
            match (on, self.tl_congested_at.take()) {
                (true, _) => self.tl_congested_at = Some(now),
                (false, Some(at)) => self.tl_congested_s += now - at,
                (false, None) => {}
            }
        }

        /// One line a second while a viewer watches: WM frames drawn, pictures captured (and held
        /// back: too soon, or `behind`: a viewer or the encoder was behind), the time a viewer was
        /// behind, pictures encoded
        /// (refinements, the slowest submit -> bitstream), pictures written to sockets (rate,
        /// the slowest single write), the stream rate.
        fn timeline_line(&mut self, cx: &mut Cx) {
            let now = cx.seconds_since_app_start();
            if let Some(at) = self.tl_congested_at.as_mut() {
                self.tl_congested_s += now - *at;
                *at = now;
            }
            let held = std::mem::take(&mut cx.global::<CaptureState>().held);
            let behind = std::mem::take(&mut cx.global::<CaptureState>().held_behind);
            let held_encoder = std::mem::take(&mut cx.global::<CaptureState>().held_encoder);
            let e = &self.encode_stats;
            let (encoded, refined, enc_max) = (e.pictures.swap(0, Ordering::Relaxed), e.refined.swap(0, Ordering::Relaxed), e.encode_us_max.swap(0, Ordering::Relaxed));
            let (tile_us, tile_pictures) = (e.tile_us_sum.swap(0, Ordering::Relaxed), e.tile_pictures.swap(0, Ordering::Relaxed));
            let tile_ms = if tile_pictures == 0 { 0.0 } else { tile_us as f64 / tile_pictures as f64 / 1000.0 };
            let (sent, bytes, write_max) = match &self.server {
                Some(s) => (s.sent.pictures.swap(0, Ordering::Relaxed), s.sent.bytes.swap(0, Ordering::Relaxed), s.sent.write_us_max.swap(0, Ordering::Relaxed)),
                None => (0, 0, 0),
            };
            let backlog = self.viewers.iter().map(|v| v.feed.backlog.load(Ordering::Acquire)).max().unwrap_or(0);
            // the viewer furthest behind (pictures sent that it has not shown yet, from its Stats)
            let shown: Vec<(u32, u32, u32)> = self.viewers.iter_mut().filter(|v| !v.feed.paused.load(Ordering::Acquire)).filter_map(|v| v.shown.take()).collect();
            let viewer = shown.iter().max_by_key(|s| s.0).map_or(String::new(), |(behind, decode, present)| format!(" · viewer behind {behind} (decode {:.1} ms, present {:.1} ms)", *decode as f64 / 1000.0, *present as f64 / 1000.0));
            // the slowest decoder of the watching viewers (its slowest stream's, per picture)
            let decode_ms = shown.iter().map(|s| s.1).max().unwrap_or(0) as f64 / 1000.0;
            let congested_ms = std::mem::take(&mut self.tl_congested_s) * 1000.0;
            // the adaptive rate decides on this second (a fixed `kbps` setting: nothing to decide)
            if let (Some(want), true) = (cx.global::<CaptureState>().want, self.settings.as_ref().is_some_and(|s| s.kbps == 0)) {
                let max_effort = self.max_effort(want.stream);
                // H.265 bands answer to the viewers' decode (H.264 tiles are sized by the level)
                let banded = want.hevc && self.settings.as_ref().is_none_or(|s| s.split == super::settings::Split::Tiles && s.tiles.is_none());
                let streams = if banded { want.plan.tiles() } else { 0 };
                let second = makepad_desk_link::rate::Second { fps: self.fps, pictures: encoded, congested_ms, held_encoder, tile_ms, sent_kbps: bytes * 8 / 1000, size: want.stream, max_effort, decode_ms, streams, link_queued: false };
                let bands_were = self.rate.decode_bands(want.stream).0;
                let ceiling = self.rate_ceiling(want.stream, want.full_chroma);
                let was = self.rate.kbps(ceiling);
                if self.rate.second(&second, ceiling) {
                    let step = self.rate.effort(want.stream, max_effort);
                    let bands = self.rate.decode_bands(want.stream).0;
                    if bands != bands_were {
                        super::note(&format!(
                            "desk link: {}x{} in at least {bands} bands (a viewer took {decode_ms:.1} ms of {:.1} to decode a picture of {streams} streams)",
                            want.stream[0], want.stream[1], 1000.0 / self.fps.max(1) as f64
                        ));
                    } else {
                        super::note(&format!(
                            "desk link: {}x{} at effort {} ({}; slowest tile {tile_ms:.1} ms of {:.1}, {held_encoder} frames waited for the encoder)",
                            want.stream[0], want.stream[1], step,
                            ["sharp, fewest streams", "sharp, a stream per engine", "fast, a stream per engine", "fast, 4:2:0"][step.min(3) as usize],
                            1000.0 / self.fps.max(1) as f64
                        ));
                    }
                    self.update_capture(cx);
                }
                let now = self.rate.kbps(ceiling);
                if now != was && self.rate.note_due(now, self.rate.limit(ceiling)) {
                    super::note(&format!("desk link: rate {} -> {} Mbit/s ({})", was / 1000, now / 1000, super::limit_name(self.rate.limit(ceiling))));
                }
                self.tell_rate(cx);
            }
            let rate = match &self.told_rate {
                Some(ServerMsg::Rate { kbps, limit, full_chroma, codec, preset, streams, why, .. }) => format!(
                    "{} Mbit/s {} {} {} x{streams} {}{}",
                    kbps / 1000,
                    if *codec == proto::pref::HEVC { "H.265" } else { "H.264" },
                    if *full_chroma { "4:4:4" } else { "4:2:0" },
                    if *preset == proto::pref::SHARP { "sharp" } else { "fast" },
                    super::limit_name(*limit),
                    if why.is_empty() { String::new() } else { format!(" ({why})") }
                ),
                _ => String::new(),
            };
            // a still desktop: one line, not one a second
            let idle = self.tl_frames == 0 && self.tl_captured == 0 && encoded == 0 && sent == 0;
            let said_idle = std::mem::replace(&mut self.tl_idle, idle);
            if idle && said_idle {
                return;
            }
            super::note(&format!(
                "desk link 1s: frames {} · captured {} (held {held}, behind {behind}, encoder {held_encoder}) · congested {congested_ms:.0} ms · encoded {encoded} (refined {refined}, max {:.1} ms, tile {tile_ms:.1} ms) · sent {sent} ({:.0} Mbit/s, write max {:.1} ms) · backlog {backlog} · rate {} · {rate}{viewer}",
                std::mem::take(&mut self.tl_frames),
                std::mem::take(&mut self.tl_captured),
                enc_max as f64 / 1000.0,
                bytes as f64 * 8.0 / 1e6,
                write_max as f64 / 1000.0,
                self.fps
            ));
        }

        fn update_capture(&mut self, cx: &mut Cx) {
            let watching = self.watching();
            if watching && self.timeline.is_empty() {
                self.timeline = cx.start_interval(1.0);
            } else if !watching && !self.timeline.is_empty() {
                cx.stop_timer(self.timeline);
                self.timeline = Timer::default();
            }
            let fps = self.stream_fps(cx);
            if fps != self.fps {
                super::note(&format!("desk link: {fps} pictures a second (was {})", self.fps));
                self.fps = fps;
                cx.global::<CaptureState>().interval = 1.0 / fps as f64;
                self.tell_rate(cx);
            }
            let refresh = self.screen_refresh(cx).unwrap_or(fps as f64);
            cx.global::<CaptureState>().every = ((refresh / fps as f64).round() as u32).max(1);
            let (screens, _) = self.screens(cx);
            let screen = screens.get(self.screen).or(screens.first()).cloned();
            let want = match (self.watching(), screen.filter(|s| s.width >= 16 && s.height >= 16)) {
                (true, Some(s)) => {
                    let stream = Self::stream_size_of(&s);
                    let c = self.choice(stream);
                    let codec = if c.hevc { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
                    let max = VideoStreamEncoder::max_size(codec, c.full_chroma);
                    let split = self.settings.as_ref().map_or(super::settings::Split::Tiles, |s| s.split);
                    let auto = TilePlan::new(stream, fps, split == super::settings::Split::Tiles, VideoStreamEncoder::caps().engines, max, c.hevc, c.spread, self.rate.decode_bands(stream).0);
                    // a `tiles = CxR` setting, never fewer than the encoder's size needs
                    let plan = match self.settings.as_ref().and_then(|s| s.tiles).filter(|_| split == super::settings::Split::Tiles) {
                        Some((cols, r)) => TilePlan { cols: cols.max(TilePlan::new(stream, fps, false, 1, max, c.hevc, false, 1).cols), rows: r },
                        None => auto,
                    };
                    Some(Want { screen_px: [s.x, s.y, s.width, s.height], stream, full_chroma: c.full_chroma, plan, hevc: c.hevc, sharp: c.sharp })
                }
                _ => None,
            };
            if let (Some(server), Some(s)) = (&self.server, screens.get(self.screen)) {
                let desktop = cx.global::<CaptureState>().desktop_px;
                *server.geometry.lock().unwrap() = Geometry { desktop, screen: [s.x, s.y, s.width, s.height] };
            }
            let st = cx.global::<CaptureState>();
            if st.want == want {
                return;
            }
            // the same screen at another size (a desktop that follows its viewer): the viewers
            // map their input and place the picture by it
            if let (Some(old), Some(new)) = (st.want, want) {
                if old.stream != new.stream {
                    let screen = self.screen as u32;
                    self.send_all(&ServerMsg::Stream { screen, stream: new.stream });
                }
            }
            let st = cx.global::<CaptureState>();
            super::note(&format!("desk link capture: {want:?}"));
            // another size or chroma: another picture texture (the registered one goes)
            let changed_size = st.want.map(|w| (w.stream, w.full_chroma, w.plan)) != want.map(|w| (w.stream, w.full_chroma, w.plan));
            st.want = want;
            if want.is_none() && !self.watching() || changed_size {
                // nobody watches (or another size): the picture texture and its memory go
                self.release_picture();
                let mut st = std::mem::take(cx.global::<CaptureState>());
                st.release(cx);
                *cx.global::<CaptureState>() = st;
            }
            if want.is_some() {
                self.keyframe();
            }
            self.tell_rate(cx);
            cx.redraw_all();
        }

        /// The picture textures' surfaces go (the pictures queued before keep theirs until they
        /// are encoded).
        fn release_picture(&mut self) {
            self.pictures.clear();
        }

        /// The picture texture as the encoder's surface (a `size` picture), exported once per
        /// texture. None until the texture is exported.
        fn picture_handle(&mut self, cx: &mut Cx, texture: &Texture, size: [u32; 2]) -> Option<SurfaceHandle> {
            if let Some((handle, _)) = self.pictures.iter().find(|(h, t)| t == texture && h.size == size) {
                return Some(handle.clone());
            }
            // a texture of the capture made again: the old ones go
            if self.pictures.len() >= 2 {
                self.release_picture();
            }
            // packed planes in BGRA texels (NV12, or NV24 for a 4:4:4 stream)
            let handle = cx.encode_surface(texture, size, SurfaceFormat::PackedNv12)?;
            self.pictures.push((handle.clone(), texture.clone()));
            Some(handle)
        }

        /// A picture was drawn in the frame just submitted: to the encoder with that frame's
        /// completion (the encoder reads the texture once the frame's GPU work is done).
        fn picture_done(&mut self, cx: &mut Cx) {
            // dropped on every way out below: the texture may be drawn again
            let (want, texture, held) = {
                let st = cx.global::<CaptureState>();
                let Some(held) = st.painted.take() else { return };
                (st.want, st.picture(), held)
            };
            let (Some(want), Some(texture)) = (want, texture) else { return };
            let Some(surface) = self.picture_handle(cx, &texture, want.stream) else {
                // not exported yet: draw again next frame, the picture follows
                self.export_next = cx.new_next_frame();
                return;
            };
            let ready = match cx.frame_completion() {
                Ok(ready) => ready,
                Err(e) => {
                    super::note(&format!("desk link stream: frame completion: {e}"));
                    return;
                }
            };
            if self.encoder.is_none() {
                let signal = self.signal.clone();
                match Encoder::start(self.subscribers.clone(), Arc::new(move || signal.set()), self.encode_stats.clone()) {
                    Ok(e) => self.encoder = Some(e),
                    Err(e) => {
                        super::note(&format!("desk link stream: encoder thread: {e}"));
                        return;
                    }
                }
            }
            let settings = self.settings.clone().unwrap_or_default();
            let kbps = self.stream_kbps(want.stream, want.full_chroma);
            if let Some(e) = &self.encoder {
                e.send(Job::Picture { surface, held, size: want.stream, ready, built_ns: makepad_hal::clock_ns(), fps: self.fps, kbps, split: settings.split, full_chroma: want.full_chroma, refine: false, plan: want.plan, hevc: want.hevc, sharp: want.sharp });
                self.tl_captured += 1;
            }
            // still for REFINE_AFTER s: the same picture again, near lossless
            self.refine_timer = cx.start_timeout(REFINE_AFTER);
        }

        /// The desktop held still since the last picture: the picture again at a near-lossless
        /// QP (text and edges sharp once the motion stops).
        fn refine(&mut self, cx: &mut Cx) {
            let (want, texture, held) = {
                let st = cx.global::<CaptureState>();
                (st.want, st.picture(), st.hold_last())
            };
            let (Some(want), Some(texture)) = (want, texture) else { return };
            let Some(surface) = self.pictures.iter().find(|(_, t)| *t == texture).map(|(h, _)| h.clone()) else { return };
            let settings = self.settings.clone().unwrap_or_default();
            let kbps = self.stream_kbps(want.stream, want.full_chroma);
            if let Some(e) = &self.encoder {
                // the picture's frame finished long ago: nothing to wait for
                e.send(Job::Picture { surface, held, size: want.stream, ready: GpuCompletion::done(), built_ns: makepad_hal::clock_ns(), fps: self.fps, kbps, split: settings.split, full_chroma: want.full_chroma, refine: true, plan: want.plan, hevc: want.hevc, sharp: want.sharp });
            }
        }

        fn send_all(&self, msg: &ServerMsg) {
            let bytes = msg.encode();
            for v in &self.viewers {
                let _ = v.out.send(Out::Msg(bytes.clone()));
            }
        }

        fn sync_subscribers(&self) {
            *self.subscribers.lock().unwrap() = self.viewers.iter().map(|v| (v.out.clone(), v.feed.clone())).collect();
        }

        fn keyframe(&self) {
            if let Some(e) = &self.encoder {
                e.send(Job::Keyframe);
            }
        }

        fn drain(&mut self, cx: &mut Cx) {
            if self.dump_gone.as_ref().is_some_and(|g| g.load(Ordering::Acquire)) {
                self.dump_gone = None;
                self.viewers.retain(|v| v.session != 0);
                self.sync_subscribers();
                self.update_capture(cx);
            }
            let Some(server) = &self.server else {
                let congested = server::hold(self.viewers.iter().map(|v| &*v.feed));
                if congested != self.congested {
                    self.set_congested(cx, congested);
                    self.update_capture(cx);
                }
                return;
            };
            let messages: Vec<ToUi> = server.rx.try_iter().collect();
            let note = server.input_note.lock().unwrap().clone();
            for m in messages {
                match m {
                    ToUi::Opened { session, peer, client, name, screen, paused, caps, out, feed } => {
                        let (screens, default) = self.screens(cx);
                        if self.viewers.is_empty() {
                            self.screen = if screen >= 0 && (screen as usize) < screens.len() { screen as usize } else { default };
                        }
                        let stream = screens.get(self.screen).map(Self::stream_size_of).unwrap_or([0, 0]);
                        super::note(&format!("desk link: {client} ({name}) from {peer} watches screen {}{}", self.screen, if paused { " (paused)" } else { "" }));
                        self.session_seq += 1;
                        // the first watching viewer starts a session: from the rate it last left
                        // with, else the start rate (not what the last session crept to)
                        let ip = peer.rsplit_once(':').map_or(peer.as_str(), |(ip, _)| ip).trim_matches(|c| c == '[' || c == ']').to_string();
                        if !paused && !self.watching() {
                            self.rate.begin(&format!("{name}@{ip}"));
                        }
                        let welcome = ServerMsg::Welcome {
                            version: proto::VERSION,
                            session,
                            screens,
                            screen: self.screen as u32,
                            stream,
                            input: note.is_empty(),
                            input_note: note.clone(),
                            server: std::fs::read_to_string("/etc/hostname").map(|h| h.trim().to_string()).unwrap_or_else(|_| "makepad wm".into()),
                        };
                        let _ = out.send(Out::Msg(welcome.encode()));
                        let _ = out.send(Out::Msg(self.rate_msg(cx).encode()));
                        self.viewers.push(Viewer { session, name, peer: ip, full_chroma: caps & proto::caps::FULL_CHROMA != 0, refresh_hz: 0, since: std::time::Instant::now(), out, feed, shown: None, window: None, view_only: caps & proto::caps::VIEW_ONLY != 0, hevc: caps & proto::caps::HEVC != 0, prefs: Prefs::default() });
                        self.generation = self.generation.wrapping_add(1);
                        self.sync_subscribers();
                        self.cursor = None;
                        self.keyframe();
                        self.update_capture(cx);
                    }
                    ToUi::Control { session, control } => {
                        let Some(i) = self.viewers.iter().position(|v| v.session == session) else { continue };
                        match control {
                            Control::Pause => {
                                self.viewers[i].feed.paused.store(true, Ordering::Release);
                                self.generation = self.generation.wrapping_add(1);
                            }
                            Control::Resume => {
                                if !self.watching() {
                                    self.rate.begin(&format!("{}@{}", self.viewers[i].name, self.viewers[i].peer));
                                }
                                self.viewers[i].feed.paused.store(false, Ordering::Release);
                                self.generation = self.generation.wrapping_add(1);
                                self.cursor = None;
                                self.keyframe();
                            }
                            Control::SelectScreen(index) => {
                                let (screens, _) = self.screens(cx);
                                if (index as usize) < screens.len() && index as usize != self.screen {
                                    self.screen = index as usize;
                                    let stream = Self::stream_size_of(&screens[self.screen]);
                                    self.send_all(&ServerMsg::Stream { screen: index, stream });
                                    self.keyframe();
                                }
                            }
                            Control::Keyframe => self.keyframe(),
                            Control::Refresh(hz) => {
                                super::note(&format!("desk link: {} shows {hz} Hz", self.viewers[i].name));
                                self.viewers[i].refresh_hz = hz;
                            }
                            Control::Window { width, height, scale } => {
                                super::note(&format!("desk link: {} window {width}x{height} at {:.2}x", self.viewers[i].name, scale as f64 / 100.0));
                                self.viewers[i].window = Some((width, height, scale));
                            }
                            Control::Clipboard(text) => cx.copy_to_clipboard(&text),
                            Control::Encoding { chroma, codec, preset, mbps, fps } => {
                                let prefs = Prefs { chroma, codec, preset, mbps, fps };
                                super::note(&format!("desk link: {} asks for {prefs:?}", self.viewers[i].name));
                                if self.viewers[i].prefs != prefs {
                                    self.viewers[i].prefs = prefs;
                                    // applied live: another chroma / codec / preset is a new
                                    // stream (a key frame); the size's encoder history goes,
                                    // so a forced choice starts from the top again
                                    self.rate.forget(cx.global::<CaptureState>().want.map_or([0, 0], |w| w.stream));
                                    self.keyframe();
                                }
                            }
                            Control::Stats { frame, decode_us, present_us } => {
                                let behind = self.encode_stats.last_frame.load(Ordering::Relaxed).wrapping_sub(frame);
                                self.viewers[i].shown = Some((behind, decode_us, present_us));
                                continue;
                            }
                        }
                        self.update_capture(cx);
                    }
                    ToUi::Closed { session, reason } => {
                        if let Some(i) = self.viewers.iter().position(|v| v.session == session) {
                            let v = self.viewers.remove(i);
                            if !v.feed.paused.load(Ordering::Acquire) {
                                self.rate.remember(&format!("{}@{}", v.name, v.peer));
                            }
                            super::note(&format!("desk link: {} left ({reason})", v.name));
                            // the bands its decoder may have asked for go with it
                            self.rate.forget_decode();
                            self.generation = self.generation.wrapping_add(1);
                        }
                        self.sync_subscribers();
                        self.update_capture(cx);
                    }
                }
            }
            // a viewer let go of the stream that caught up starts again at a key frame
            if server::resync_due(self.viewers.iter().map(|v| &*v.feed)) {
                self.keyframe();
            }
            // a viewer behind on pictures stops the capture until it caught up (one behind while
            // another keeps up is let go of the stream instead: `server::hold`)
            let congested = server::hold(self.viewers.iter().map(|v| &*v.feed));
            if congested != self.congested {
                self.set_congested(cx, congested);
                self.update_capture(cx);
            }
        }

        /// The cursor, apart from the video: position over the stream picture and shape.
        fn cursor_update(&mut self, cx: &mut Cx) {
            if self.viewers.is_empty() {
                return;
            }
            let (screens, _) = self.screens(cx);
            let Some(s) = screens.get(self.screen) else { return };
            let dpi = cx.global::<CaptureState>().dpi.max(0.1);
            let px = self.pointer * dpi;
            let nx = (px.x - s.x as f64) / s.width.max(1) as f64;
            let ny = (px.y - s.y as f64) / s.height.max(1) as f64;
            let inside = (0.0..=1.0).contains(&nx) && (0.0..=1.0).contains(&ny);
            let shape = proto::cursor_index(&format!("{:?}", cx.mouse_cursor()));
            let now = ((nx.clamp(0.0, 1.0) * 65535.0) as u16, (ny.clamp(0.0, 1.0) * 65535.0) as u16, shape, inside);
            if self.cursor != Some(now) {
                self.cursor = Some(now);
                self.send_all(&ServerMsg::Cursor { x: now.0, y: now.1, shape: now.2, visible: now.3 });
            }
        }

        pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
            match event {
                Event::Startup => self.start(cx),
                Event::Signal => {
                    if self.signal.check_and_clear() {
                        self.drain(cx);
                        self.picture_done(cx);
                    }
                }
                Event::MouseMove(e) => {
                    self.pointer = e.abs;
                    self.cursor_update(cx);
                }
                Event::MouseDown(e) => {
                    self.pointer = e.abs;
                    self.cursor_update(cx);
                }
                Event::GpuReset => {
                    // a new GPU device: the picture texture is exported again from the new one,
                    // and the encoder opened again there (its next picture a key frame)
                    super::note("desk link: the GPU device was replaced; the stream starts again on the new one");
                    // a 4:4:4 stream that lost the device: that size goes 4:2:0 (never the same
                    // configuration again: it would lose the new device as well)
                    if let Some(want) = cx.global::<CaptureState>().want.filter(|w| w.full_chroma) {
                        if !self.lost_full_chroma.contains(&want.stream) {
                            super::note(&format!("desk link: 4:4:4 at {}x{} ({}x{} tiles) lost the GPU device: this size streams 4:2:0 from now on", want.stream[0], want.stream[1], want.plan.cols, want.plan.rows));
                            self.lost_full_chroma.push(want.stream);
                        }
                    }
                    self.release_picture();
                    if let Some(e) = &self.encoder {
                        e.send(Job::Reset);
                    }
                    self.update_capture(cx);
                    cx.redraw_all();
                }
                Event::Shutdown => {
                    for v in &self.viewers {
                        let _ = v.out.send(Out::Close("the WM quits".into()));
                    }
                    self.server = None;
                }
                _ => {}
            }
            // the desktop's geometry is known after its first draw and may change (a screen
            // added, a mode set): the capture follows it
            if !self.viewers.is_empty() && matches!(event, Event::Draw(_)) {
                self.update_capture(cx);
            }
            if self.refine_timer.is_event(event).is_some() {
                self.refine(cx);
            }
            if !self.viewers.is_empty() && matches!(event, Event::Draw(_)) {
                self.tl_frames += 1;
            }
            if self.timeline.is_event(event).is_some() {
                self.timeline_line(cx);
            }
            if self.export_next.is_event(event).is_some() {
                cx.redraw_all();
            }
            // a desktop change that came too soon for a picture is drawn again when one is due
            if self.owed_next.is_event(event).is_some() || (matches!(event, Event::Draw(_)) && cx.global::<CaptureState>().owed) {
                let st = cx.global::<CaptureState>();
                if st.owed && st.want.is_some() {
                    let due = cx.seconds_since_app_start() - cx.global::<CaptureState>().last_picture >= cx.global::<CaptureState>().interval * 0.9;
                    if due && !matches!(event, Event::Draw(_)) {
                        cx.redraw_all();
                    } else {
                        self.owed_next = cx.new_next_frame();
                    }
                }
            }
            if !self.viewers.is_empty() && !matches!(event, Event::MouseMove(_)) {
                // a shape change without motion (a hover changes the cursor)
                if matches!(event, Event::NextFrame(_) | Event::Draw(_)) {
                    self.cursor_update(cx);
                }
            }
            let _ = self.session_seq;
        }
    }
}
