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

/// The active viewer's picture area (device pixels), its display refresh (mHz) and scale
/// (hundredths): what a desktop that follows its viewer (a virtual display) takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewerWindow {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    pub scale: u32,
}

/// Someone viewing this desktop, for the bar's Desk Link item and its panel.
#[derive(Clone, Debug, PartialEq)]
pub struct DeskLinkViewer {
    pub session: u32,
    /// What the viewer calls itself (`desklink <user>`).
    pub name: String,
    /// Its address (IP only).
    pub peer: String,
    /// Not watching right now (the viewer shows another desktop); still connected.
    pub paused: bool,
    /// Seconds since it connected.
    pub secs: u64,
}

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
    use makepad_desk_link::server::{Control, Geometry, Input, Out, Server, ServerConfig, ToUi};
    use super::settings::DeskLinkSettings;
    use makepad_desk_link::{self as proto, ScreenInfo, ServerMsg};
    use super::encode::{EncodeStats, Encoder, Fence, Job, Subscribers, TilePlan};
    use makepad_widgets::makepad_platform::video_file::VideoStreamEncoder;
    use makepad_widgets::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    /// More pictures than this queued for a viewer: no picture is made until it catches up
    /// (none is ever dropped once encoded: the next one references it).
    /// How long the desktop holds still before its picture is sent again near lossless (s).
    const REFINE_AFTER: f64 = 0.12;

    const BACKLOG: usize = 2;

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
        backlog: Arc<AtomicUsize>,
        paused: Arc<AtomicBool>,
        /// From its last `Stats`: pictures it was behind the last one sent, its decode and
        /// present time (µs).
        shown: Option<(u32, u32, u32)>,
        /// Its picture area: device pixels and scale (hundredths), from its `Window`.
        window: Option<(u32, u32, u16)>,
        /// When it last became the active viewer (it sent input, or it opened).
        active_at: std::time::Instant,
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
        /// The picture textures registered for the encoder (Linux: DMA-BUFs of the copy
        /// registry) and the textures they are (the capture's two, drawn in turn).
        pictures: Vec<(u64, Texture)>,
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
        tl_congested_at: Option<f64>,
        tl_congested_s: f64,
        /// The active viewer's window as last handed out (`take_viewer_window`).
        viewer_window: Option<super::ViewerWindow>,
    }

    impl DeskLink {
        /// Who is connected now (the dump viewer, a dev tool, is not listed).
        pub fn viewers(&self) -> Vec<super::DeskLinkViewer> {
            self.viewers
                .iter()
                .filter(|v| v.session != 0)
                .map(|v| super::DeskLinkViewer { session: v.session, name: v.name.clone(), peer: v.peer.clone(), paused: v.paused.load(Ordering::Acquire), secs: v.since.elapsed().as_secs() })
                .collect()
        }

        /// Changes whenever [`Self::viewers`] would.
        pub fn generation(&self) -> u32 {
            self.generation
        }

        /// Ends a viewer's session from this desktop (the bar's panel): it is told why and
        /// closed; it leaves the list when the server reports it gone.
        /// The active viewer's window when it changed since the last call: the watching viewer
        /// that sent input last (else the last to come) and has said its window size.
        pub fn take_viewer_window(&mut self) -> Option<super::ViewerWindow> {
            let v = self.viewers.iter().filter(|v| !v.paused.load(Ordering::Acquire) && v.window.is_some()).max_by_key(|v| v.active_at)?;
            let (width, height, scale) = v.window?;
            let refresh_mhz = if v.refresh_hz == 0 { 0 } else { v.refresh_hz as u32 * 1000 };
            let window = super::ViewerWindow { width, height, refresh_mhz, scale: scale as u32 };
            if self.viewer_window == Some(window) {
                return None;
            }
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
                    allow_peers: settings.allow_peers.clone(),
                    tls_dir: crate::theme::makepad_home().join("wm/desk-link/tls"),
                    tls_name: format!("makepad desk link {host}"),
                    fleet_dir: settings.fleet_dir(),
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
                        super::note(&format!("desk link listens on {} (certificate {})", server.addr, server.fingerprint));
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
            let backlog = Arc::new(AtomicUsize::new(0));
            let (backlog2, signal) = (backlog.clone(), self.signal.clone());
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
                        backlog2.fetch_sub(1, Ordering::AcqRel);
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
                self.viewers.push(Viewer { session: 0, name: "dump".into(), peer: String::new(), full_chroma: true, refresh_hz: 0, since: std::time::Instant::now(), out: tx, backlog, paused: Arc::new(AtomicBool::new(false)), shown: None, window: None, active_at: std::time::Instant::now() });
                self.sync_subscribers();
            }
        }

        /// The screens viewers may pick, in desktop pixels, and the default one.
        fn screens(&self, cx: &mut Cx) -> (Vec<ScreenInfo>, usize) {
            let st = cx.global::<CaptureState>();
            let desktop = st.desktop_px;
            let mut out = Vec::new();
            let mut primary = 0;
            #[cfg(target_os = "linux")]
            {
                let snapshot = cx.linux_display_snapshot();
                for o in snapshot.outputs.iter().filter(|o| o.active && o.width > 0) {
                    let Some((x, y)) = o.desktop_position else { continue };
                    if o.primary {
                        primary = out.len();
                    }
                    out.push(ScreenInfo { name: o.name.clone(), x, y, width: o.width, height: o.height });
                }
            }
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
            for v in self.viewers.iter().filter(|v| v.refresh_hz > 0 && !v.paused.load(Ordering::Acquire)) {
                fps = fps.min(v.refresh_hz as u32);
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
            self.viewers.iter().any(|v| !v.paused.load(Ordering::Acquire))
        }

        /// 4:4:4 when the settings ask for it and every watching viewer's decoder takes it.
        fn full_chroma(&self) -> bool {
            self.settings.as_ref().is_some_and(|s| s.full_chroma) && self.viewers.iter().filter(|v| !v.paused.load(Ordering::Acquire)).all(|v| v.full_chroma)
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
            let e = &self.encode_stats;
            let (encoded, refined, enc_max) = (e.pictures.swap(0, Ordering::Relaxed), e.refined.swap(0, Ordering::Relaxed), e.encode_us_max.swap(0, Ordering::Relaxed));
            let (sent, bytes, write_max) = match &self.server {
                Some(s) => (s.sent.pictures.swap(0, Ordering::Relaxed), s.sent.bytes.swap(0, Ordering::Relaxed), s.sent.write_us_max.swap(0, Ordering::Relaxed)),
                None => (0, 0, 0),
            };
            let backlog = self.viewers.iter().map(|v| v.backlog.load(Ordering::Acquire)).max().unwrap_or(0);
            // the viewer furthest behind (pictures sent that it has not shown yet, from its Stats)
            let viewer = self.viewers.iter_mut().filter_map(|v| v.shown.take()).max_by_key(|s| s.0).map_or(String::new(), |(behind, decode, present)| format!(" · viewer behind {behind} (decode {:.1} ms, present {:.1} ms)", decode as f64 / 1000.0, present as f64 / 1000.0));
            super::note(&format!(
                "desk link 1s: frames {} · captured {} (held {held}, behind {behind}) · congested {:.0} ms · encoded {encoded} (refined {refined}, max {:.1} ms) · sent {sent} ({:.0} Mbit/s, write max {:.1} ms) · backlog {backlog} · rate {}{viewer}",
                std::mem::take(&mut self.tl_frames),
                std::mem::take(&mut self.tl_captured),
                std::mem::take(&mut self.tl_congested_s) * 1000.0,
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
                self.send_all(&ServerMsg::Rate { fps: fps as u16 });
            }
            let refresh = self.screen_refresh(cx).unwrap_or(fps as f64);
            cx.global::<CaptureState>().every = ((refresh / fps as f64).round() as u32).max(1);
            let (screens, _) = self.screens(cx);
            let screen = screens.get(self.screen).or(screens.first()).cloned();
            let want = match (self.watching(), screen.filter(|s| s.width >= 16 && s.height >= 16)) {
                (true, Some(s)) => {
                    let (stream, full_chroma) = (Self::stream_size_of(&s), self.full_chroma());
                    let split = self.settings.as_ref().map_or(super::settings::Split::Tiles, |s| s.split);
                    let auto = TilePlan::new(stream, fps, split, VideoStreamEncoder::engines(), VideoStreamEncoder::max_size(full_chroma));
                    // a `tiles = CxR` setting, never fewer than the encoder's size needs
                    let plan = match self.settings.as_ref().and_then(|s| s.tiles).filter(|_| split == super::settings::Split::Tiles) {
                        Some((c, r)) => TilePlan { cols: c.max(TilePlan::new(stream, fps, super::settings::Split::Off, 1, VideoStreamEncoder::max_size(full_chroma)).cols), rows: r },
                        None => auto,
                    };
                    Some(Want { screen_px: [s.x, s.y, s.width, s.height], stream, full_chroma, plan })
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
            cx.redraw_all();
        }

        /// The picture textures' registrations go after the pictures queued before them: the
        /// encoder thread releases them in order (pictures still queued are encoded first).
        fn release_picture(&mut self) {
            for (_handle, _) in std::mem::take(&mut self.pictures) {
                match &self.encoder {
                    Some(e) => e.send(Job::Release(_handle)),
                    #[cfg(target_os = "linux")]
                    None => makepad_hal::dmabuf::hal_dmabuf_release(_handle),
                    #[cfg(not(target_os = "linux"))]
                    None => {}
                }
            }
        }

        /// The picture texture's handle for the encoder: its DMA-BUF in the copy registry
        /// (Linux), registered once per texture. None until the texture is exported.
        fn picture_handle(&mut self, cx: &mut Cx, texture: &Texture) -> Option<u64> {
            if let Some((handle, _)) = self.pictures.iter().find(|(_, t)| t == texture) {
                return Some(*handle);
            }
            // a texture of the capture made again: the registrations of the old ones go
            if self.pictures.len() >= 2 {
                self.release_picture();
            }
            #[cfg(target_os = "linux")]
            {
                use std::os::fd::IntoRawFd;
                let (fd, layout) = cx.shared_texture_dmabuf(texture).ok()?;
                let handle = makepad_hal::dmabuf::hal_dmabuf_register(fd.into_raw_fd(), &layout);
                if handle == 0 {
                    super::note("desk link stream: the picture's DMA-BUF was refused");
                    return None;
                }
                self.pictures.push((handle, texture.clone()));
                return Some(handle);
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (cx, texture);
                None
            }
        }

        /// A picture was drawn in the frame just submitted: to the encoder with that frame's
        /// fence (the encoder reads the texture once the frame's GPU work is done).
        fn picture_done(&mut self, cx: &mut Cx) {
            // dropped on every way out below: the texture may be drawn again
            let (want, texture, held) = {
                let st = cx.global::<CaptureState>();
                let Some(held) = st.painted.take() else { return };
                (st.want, st.picture(), held)
            };
            let (Some(want), Some(texture)) = (want, texture) else { return };
            let Some(handle) = self.picture_handle(cx, &texture) else {
                // not exported yet: draw again next frame, the picture follows
                self.export_next = cx.new_next_frame();
                return;
            };
            #[cfg(target_os = "linux")]
            let fence = match cx.foreign_dmabuf_fence() {
                Ok(f) => Fence::Linux(f),
                Err(e) => {
                    super::note(&format!("desk link stream: frame fence: {e}"));
                    return;
                }
            };
            #[cfg(not(target_os = "linux"))]
            let fence = Fence::None;
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
            if let Some(e) = &self.encoder {
                e.send(Job::Picture { handle, held, size: want.stream, fence, built_ns: makepad_hal::clock_ns(), fps: self.fps, kbps: settings.kbps, split: settings.split, full_chroma: want.full_chroma, refine: false, plan: want.plan });
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
            let Some(&(handle, _)) = self.pictures.iter().find(|(_, t)| *t == texture) else { return };
            let settings = self.settings.clone().unwrap_or_default();
            if let Some(e) = &self.encoder {
                // the picture's frame finished long ago: nothing to wait for
                e.send(Job::Picture { handle, held, size: want.stream, fence: Fence::None, built_ns: makepad_hal::clock_ns(), fps: self.fps, kbps: settings.kbps, split: settings.split, full_chroma: want.full_chroma, refine: true, plan: want.plan });
            }
        }

        fn send_all(&self, msg: &ServerMsg) {
            let bytes = msg.encode();
            for v in &self.viewers {
                let _ = v.out.send(Out::Msg(bytes.clone()));
            }
        }

        fn sync_subscribers(&self) {
            *self.subscribers.lock().unwrap() = self.viewers.iter().map(|v| (v.out.clone(), v.backlog.clone(), v.paused.clone())).collect();
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
                let congested = self.viewers.iter().any(|v| !v.paused.load(Ordering::Acquire) && v.backlog.load(Ordering::Acquire) > BACKLOG);
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
                    ToUi::Opened { session, peer, client, name, screen, paused, caps, out, backlog } => {
                        let (screens, default) = self.screens(cx);
                        if self.viewers.is_empty() {
                            self.screen = if screen >= 0 && (screen as usize) < screens.len() { screen as usize } else { default };
                        }
                        let stream = screens.get(self.screen).map(Self::stream_size_of).unwrap_or([0, 0]);
                        super::note(&format!("desk link: {client} ({name}) from {peer} watches screen {}{}", self.screen, if paused { " (paused)" } else { "" }));
                        self.session_seq += 1;
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
                        let _ = out.send(Out::Msg(ServerMsg::Rate { fps: self.fps as u16 }.encode()));
                        // the address without its port
                        let ip = peer.rsplit_once(':').map_or(peer.as_str(), |(ip, _)| ip).trim_matches(|c| c == '[' || c == ']').to_string();
                        self.viewers.push(Viewer { session, name, peer: ip, full_chroma: caps & proto::caps::FULL_CHROMA != 0, refresh_hz: 0, since: std::time::Instant::now(), out, backlog, paused: Arc::new(AtomicBool::new(paused)), shown: None, window: None, active_at: std::time::Instant::now() });
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
                                self.viewers[i].paused.store(true, Ordering::Release);
                                self.generation = self.generation.wrapping_add(1);
                            }
                            Control::Resume => {
                                self.viewers[i].paused.store(false, Ordering::Release);
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
                            Control::Split(split) => {
                                let split = match split {
                                    proto::split::OFF => super::settings::Split::Off,
                                    proto::split::INTERLEAVE => super::settings::Split::Interleave,
                                    _ => super::settings::Split::Tiles,
                                };
                                super::note(&format!("desk link: a viewer asks for {split:?}"));
                                if let Some(s) = self.settings.as_mut() {
                                    s.split = split;
                                }
                            }
                            Control::Refresh(hz) => {
                                super::note(&format!("desk link: {} shows {hz} Hz", self.viewers[i].name));
                                self.viewers[i].refresh_hz = hz;
                            }
                            Control::Window { width, height, scale } => {
                                super::note(&format!("desk link: {} window {width}x{height} at {:.2}x", self.viewers[i].name, scale as f64 / 100.0));
                                self.viewers[i].window = Some((width, height, scale));
                            }
                            Control::Active => {
                                self.viewers[i].active_at = std::time::Instant::now();
                            }
                            Control::Clipboard(text) => cx.copy_to_clipboard(&text),
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
                            super::note(&format!("desk link: {} left ({reason})", v.name));
                            self.generation = self.generation.wrapping_add(1);
                        }
                        self.sync_subscribers();
                        self.update_capture(cx);
                    }
                }
            }
            // a viewer behind on pictures stops the capture until it caught up
            let congested = self.viewers.iter().any(|v| !v.paused.load(Ordering::Acquire) && v.backlog.load(Ordering::Acquire) > BACKLOG);
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
                    self.release_picture();
                    if let Some(e) = &self.encoder {
                        e.send(Job::Reset);
                    }
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
