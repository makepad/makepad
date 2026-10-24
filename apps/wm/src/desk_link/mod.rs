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
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::capture::{CaptureState, Want};
    use makepad_desk_link::server::{Control, Geometry, Input, Out, Server, ServerConfig, ToUi};
    use super::settings::DeskLinkSettings;
    use makepad_desk_link::{self as proto, ScreenInfo, ServerMsg};
    use super::encode::{Encoder, Fence, Job, Subscribers};
    use makepad_widgets::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    /// More pictures than this queued for a viewer: no picture is made until it catches up
    /// (none is ever dropped once encoded: the next one references it).
    const BACKLOG: usize = 2;

    struct Viewer {
        session: u32,
        name: String,
        /// The viewer's address (IP only) and since when it is here.
        peer: String,
        since: std::time::Instant,
        out: std::sync::mpsc::Sender<Out>,
        backlog: Arc<AtomicUsize>,
        paused: Arc<AtomicBool>,
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
        /// The picture texture registered for the encoder (Linux: a DMA-BUF of the copy
        /// registry) and the texture it is.
        picture: Option<(u64, Texture)>,
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
        /// Bumped whenever a viewer comes, goes, pauses or resumes (the bar follows it).
        generation: u32,
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
            cx.global::<CaptureState>().interval = 1.0 / settings.fps.max(1) as f64;
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
                self.viewers.push(Viewer { session: 0, name: "dump".into(), peer: String::new(), since: std::time::Instant::now(), out: tx, backlog, paused: Arc::new(AtomicBool::new(false)) });
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

        fn stream_size_of(s: &ScreenInfo) -> [u32; 2] {
            [s.width / 4 * 4, s.height / 2 * 2]
        }

        fn watching(&self) -> bool {
            self.viewers.iter().any(|v| !v.paused.load(Ordering::Acquire))
        }

        /// The capture follows the viewers: wanted while one watches and none is behind.
        fn update_capture(&mut self, cx: &mut Cx) {
            let (screens, _) = self.screens(cx);
            let screen = screens.get(self.screen).or(screens.first()).cloned();
            let want = match (self.watching() && !self.congested, screen.filter(|s| s.width >= 16 && s.height >= 16)) {
                (true, Some(s)) => Some(Want { screen_px: [s.x, s.y, s.width, s.height], stream: Self::stream_size_of(&s) }),
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
            super::note(&format!("desk link capture: {want:?}"));
            let changed_size = st.want.map(|w| w.stream) != want.map(|w| w.stream);
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

        /// The picture texture's registration goes after the pictures queued before it: the
        /// encoder thread releases it in order (pictures still queued are encoded first).
        fn release_picture(&mut self) {
            if let Some((_handle, _)) = self.picture.take() {
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
            if let Some((handle, t)) = &self.picture {
                if t == texture {
                    return Some(*handle);
                }
            }
            self.release_picture();
            #[cfg(target_os = "linux")]
            {
                use std::os::fd::IntoRawFd;
                let (fd, layout) = cx.shared_texture_dmabuf(texture).ok()?;
                let handle = makepad_hal::dmabuf::hal_dmabuf_register(fd.into_raw_fd(), &layout);
                if handle == 0 {
                    super::note("desk link stream: the picture's DMA-BUF was refused");
                    return None;
                }
                self.picture = Some((handle, texture.clone()));
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
            let (want, texture) = {
                let st = cx.global::<CaptureState>();
                if !std::mem::take(&mut st.painted) {
                    return;
                }
                (st.want, st.picture())
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
                match Encoder::start(self.subscribers.clone(), Arc::new(move || signal.set())) {
                    Ok(e) => self.encoder = Some(e),
                    Err(e) => {
                        super::note(&format!("desk link stream: encoder thread: {e}"));
                        return;
                    }
                }
            }
            let settings = self.settings.clone().unwrap_or_default();
            if let Some(e) = &self.encoder {
                e.send(Job::Picture { handle, size: want.stream, fence, built_ns: makepad_hal::clock_ns(), fps: settings.fps, kbps: settings.kbps, split: settings.split });
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
                    self.congested = congested;
                    self.update_capture(cx);
                }
                return;
            };
            let messages: Vec<ToUi> = server.rx.try_iter().collect();
            let note = server.input_note.lock().unwrap().clone();
            for m in messages {
                match m {
                    ToUi::Opened { session, peer, client, name, screen, paused, out, backlog } => {
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
                        // the address without its port
                        let ip = peer.rsplit_once(':').map_or(peer.as_str(), |(ip, _)| ip).trim_matches(|c| c == '[' || c == ']').to_string();
                        self.viewers.push(Viewer { session, name, peer: ip, since: std::time::Instant::now(), out, backlog, paused: Arc::new(AtomicBool::new(paused)) });
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
                            Control::Clipboard(text) => cx.copy_to_clipboard(&text),
                            Control::Stats { .. } => {}
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
                self.congested = congested;
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
