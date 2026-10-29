//! Registered Wayland clients owned by a native layer, never by the tiling layout.
use super::*;
use makepad_studio_protocol::{NativeAppBuffer, NativeAppEvent, NativeAppId, NativeAppPlane, NativeAppRequest, PresentableImageId, StudioToApp};
use makepad_widgets::makepad_platform::shared_framebuf::NativeDescriptors;
use makepad_widgets::makepad_platform::makepad_hal::{self as hal, native_layer::{mode, op, state}};
use std::collections::{HashSet, VecDeque};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::{net::UnixStream, process::CommandExt};

type EmbedKey = (ClientId, NativeAppId);
struct ShmSlot {
    texture: Texture, id: u64, width: u32, height: u32,
    busy: bool, sent: bool, completion: Option<GpuCompletion>, commit: u64,
    release: Option<OwnedFd>,
}
pub(super) struct Embedded {
    owner: ClientId, id: NativeAppId, child: Child, connection: Option<u32>, main: Option<Surf>,
    size: (u32, u32), focused: bool, pointer: bool,
    keys: HashSet<u16>,
    buffers: HashMap<u32, u64>, held: HashMap<u64, u32>,
    current: Option<u64>, serial: u64, mapped: bool,
    pixels: Vec<u8>, pixels_size: (u32, u32), dirty: bool, slots: Vec<ShmSlot>, upload_waiting: bool,
    outbox: VecDeque<NativeDescriptors>, releases: Vec<(u64, bool)>,
    closing: bool,
    next_frame: Option<NextFrame>,
}
impl Embedded {
    fn event(&mut self, event: NativeAppEvent) { self.outbox.push_back(NativeDescriptors { event, fds: Vec::new() }); }
    fn state(&mut self, what: u32, a: u64, data: &[u8]) { self.event(NativeAppEvent::State { id: self.id, what, a, data: data.to_vec() }); }
    fn configure(&self, s: &Shared) {
        if let Some(surface) = self.main {
            let mut states = vec![STATE_TILED_LEFT, STATE_TILED_RIGHT, STATE_TILED_TOP, STATE_TILED_BOTTOM];
            if self.focused { states.push(STATE_ACTIVATED); }
            host_send(s, Command::ConfigureToplevel { surface, width: self.size.0 as i32, height: self.size.1 as i32, states });
        }
    }
    fn commit(&mut self, cx: &mut Cx, serial: u64, buffer: BufferUpdate) {
        let stamp = hal::hosted_native::trace_enabled().then(std::time::Instant::now);
        let source = match &buffer { BufferUpdate::Dmabuf(_) => "dmabuf", BufferUpdate::Attach(_) => "shm", BufferUpdate::Keep => "keep", BufferUpdate::Detach => "detach" };
        self.serial = serial;
        match buffer {
            BufferUpdate::Dmabuf(frame) => {
                self.mapped = true;
                let id = *self.buffers.entry(frame.buffer).or_insert_with(|| PresentableImageId::alloc().as_u64());
                if !self.held.contains_key(&id) {
                    let desc = NativeAppBuffer { width: frame.data.width, height: frame.data.height,
                        fourcc: frame.data.fourcc, modifier: frame.data.modifier,
                        planes: frame.data.planes.iter().map(|p| NativeAppPlane { offset: p.offset, stride: p.stride }).collect() };
                    let fds = frame.data.planes.iter().map(|p| p.fd.try_clone().map(|fd| (PresentableImageId::from_u64(id), fd))).collect();
                    match fds {
                        Ok(fds) => self.outbox.push_back(NativeDescriptors { event: NativeAppEvent::Buffer { id: self.id, buffer: id, desc }, fds }),
                        Err(error) => { self.state(state::CRASHED, 0, error.to_string().as_bytes()); return; }
                    }
                    self.held.insert(id, frame.buffer);
                }
                self.current = Some(id);
                self.event(NativeAppEvent::Commit { id: self.id, buffer: id, commit: serial });
            }
            BufferUpdate::Attach(frame) => {
                self.mapped = true;
                let size = (frame.width, frame.height);
                if self.pixels_size != size {
                    if !frame.is_full() { self.state(state::CRASHED, 0, b"partial first shm frame"); return; }
                    self.pixels_size = size;
                    self.pixels.resize(size.0 as usize * size.1 as usize * 4, 0);
                }
                let rows = to_u32_rows(&frame);
                let start = frame.y0 as usize * frame.width as usize * 4;
                for (dst, pixel) in self.pixels[start..].chunks_exact_mut(4).zip(rows) { dst.copy_from_slice(&pixel.to_le_bytes()); }
                // X formats have undefined alpha; normalize while doing the existing row copy.
                if !wayland_server::shm::has_alpha(frame.format) { for p in self.pixels.chunks_exact_mut(4) { p[3] = 255; } }
                self.dirty = true;
                self.upload(cx);
            }
            BufferUpdate::Keep => {
                if !self.dirty {
                    if let Some(slot) = self.slots.iter_mut().rev().find(|s| s.completion.is_some()) { slot.commit = serial; }
                    else if let Some(buffer) = self.current { self.event(NativeAppEvent::Commit { id: self.id, buffer, commit: serial }); }
                }
            }
            BufferUpdate::Detach => { self.mapped = false; self.current = None; self.dirty = false; self.event(NativeAppEvent::Unmapped { id: self.id }); }
        }
        if let Some(stamp) = stamp { log!("native-app commit source={source} serial={serial} us={}", stamp.elapsed().as_micros()); }
    }
    fn upload(&mut self, cx: &mut Cx) {
        if !self.dirty { return; }
        let (w, h) = self.pixels_size;
        let mut forgotten = Vec::new();
        self.slots.retain(|s| {
            let keep = s.busy || s.completion.is_some() || s.release.is_some() || (s.width, s.height) == (w, h);
            if !keep && s.sent { forgotten.push(s.id); }
            keep
        });
        for buffer in forgotten { self.event(NativeAppEvent::Forget { id: self.id, buffer }); }
        let at = self.slots.iter().position(|s| !s.busy && s.completion.is_none() && s.release.is_none() && (s.width, s.height) == (w, h));
        let at = match at {
            Some(at) => at,
            None if self.slots.iter().filter(|s| (s.width, s.height) == (w, h)).count() < makepad_studio_protocol::SWAPCHAIN_IMAGE_COUNT => {
                self.slots.push(ShmSlot { texture: cx.create_shared_texture(w as usize, h as usize), id: PresentableImageId::alloc().as_u64(),
                    width: w, height: h, busy: false, sent: false, completion: None, commit: 0, release: None });
                self.slots.len() - 1
            }
            None => {
                if !std::mem::replace(&mut self.upload_waiting, true) { log!("wm: embedded shm ring busy; retaining latest pixels and callbacks until release"); }
                return;
            }
        };
        self.upload_waiting = false;
        let slot = &mut self.slots[at];
        match cx.upload_shared_bgra(&slot.texture, &self.pixels, w, h).and_then(|_| cx.frame_completion()) {
            Ok(completion) => { slot.completion = Some(completion); slot.commit = self.serial; slot.busy = true; self.dirty = false; }
            Err(error) => log!("wm: embedded shm upload deferred: {error}"),
        }
    }
    fn poll(&mut self, cx: &mut Cx, s: &Shared) {
        let mut waiting = Vec::new();
        for (buffer, fenced) in std::mem::take(&mut self.releases) {
            let fence = if fenced {
                let Some(fd) = makepad_app_host::hosted::native_release_fence(self.owner, buffer) else { waiting.push((buffer, fenced)); continue };
                Some(fd)
            } else { None };
            if let Some(slot) = self.slots.iter_mut().find(|slot| slot.id == buffer) { slot.busy = false; slot.release = fence; }
            else if let (Some(client), Some(wl_buffer)) = (self.connection, self.held.get(&buffer).copied()) {
                host_send(s, Command::BufferDone { client, buffer: wl_buffer, fence });
            }
        }
        self.releases = waiting;
        for slot in &mut self.slots {
            if slot.release.as_ref().is_some_and(wayland_server::compositor::fence_ready) { slot.release = None; }
            let Some(completion) = slot.completion.as_ref() else { continue };
            match completion.poll() {
                None => continue,
                Some(Err(error)) => { log!("wm: embedded upload completion failed: {error}"); continue; }
                Some(Ok(())) => {}
            }
            if completion.sync_file().as_ref().is_some_and(|fd| !wayland_server::compositor::fence_ready(fd)) { continue; }
            if !self.mapped { slot.completion = None; slot.busy = false; continue; }
            if !slot.sent {
                let Ok((fd, words)) = cx.shared_texture_dmabuf(&slot.texture) else { continue };
                let desc = NativeAppBuffer { width: w32(words[0]), height: w32(words[1]), fourcc: w32(words[2]), modifier: words[3],
                    planes: vec![NativeAppPlane { offset: w32(words[4]), stride: w32(words[5]) }] };
                self.outbox.push_back(NativeDescriptors { event: NativeAppEvent::Buffer { id: self.id, buffer: slot.id, desc },
                    fds: vec![(PresentableImageId::from_u64(slot.id), fd)] });
                slot.sent = true;
            }
            slot.completion = None;
            self.current = Some(slot.id);
            self.outbox.push_back(NativeDescriptors { event: NativeAppEvent::Commit { id: self.id, buffer: slot.id, commit: slot.commit }, fds: Vec::new() });
        }
        self.upload(cx);
        while let Some(batch) = self.outbox.pop_front() {
            if let Err(batch) = makepad_app_host::hosted::native_descriptors(self.owner, batch) { self.outbox.push_front(batch); break; }
        }
        // A sync_file becomes readable after its submission notification. Keep polling on
        // frame beats only while waiting for that GPU edge; an idle terminal must progress
        // without another key, pointer event or WM redraw to wake it.
        if self.next_frame.is_none() && (self.closing || self.slots.iter().any(|s| s.completion.is_some() || s.release.is_some())) {
            self.next_frame = Some(cx.new_next_frame());
        }
    }
}
fn w32(v: u64) -> u32 { v as u32 }

impl App {
    pub(crate) fn native_app_request(&mut self, cx: &mut Cx, owner: ClientId, request: NativeAppRequest) {
        if let NativeAppRequest::Create { id, app } = request {
            if let Err(error) = self.native_app_launch(cx, owner, id, &app) {
                if let Some(sender) = self.state_mut().clients.get(&owner).and_then(|c| c.sender.as_ref()) {
                    makepad_app_host::send_to_app(sender, vec![StudioToApp::NativeApp(NativeAppEvent::State { id, what: state::CRASHED, a: 0, data: error.into_bytes() }),
                        StudioToApp::NativeApp(NativeAppEvent::State { id, what: state::REALIZED, a: mode::UNSUPPORTED, data: Vec::new() })]);
                }
            }
            return;
        }
        with(|s| {
            let id = match &request { NativeAppRequest::Destroy { id } | NativeAppRequest::Op { id, .. } | NativeAppRequest::Configure { id, .. } |
                NativeAppRequest::Release { id, .. } | NativeAppRequest::Presented { id, .. } => *id, _ => return };
            let key = (owner, id);
            let Some(mut e) = s.embedded.remove(&key) else {
                if let NativeAppRequest::Release { buffer, fence: true, .. } = request {
                    makepad_app_host::hosted::native_discard_release_fence(owner, buffer);
                }
                return;
            };
            match request {
                NativeAppRequest::Destroy { .. } => {
                    e.closing = true;
                    e.release_keys(s);
                    if let Some(main) = e.main { host_send(s, Command::CloseToplevel(main)); }
                    if let Some(client) = e.connection { host_send(s, Command::Disconnect(client)); }
                }
                NativeAppRequest::Configure { width, height, .. } => {
                    if width > 0 && height > 0 && width <= i32::MAX as u32 && height <= i32::MAX as u32 && e.size != (width, height) {
                        e.size = (width, height); e.configure(s);
                    }
                }
                NativeAppRequest::Release { buffer, fence, .. } => e.releases.push((buffer, fence)),
                NativeAppRequest::Presented { commit, .. } => {
                    if let Some(surface) = e.main { host_send(s, Command::FrameDoneThrough { surface, serial: commit, time_ms: now_ms() }); }
                }
                NativeAppRequest::Op { op: operation, data, .. } => match operation {
                    op::FOCUS => {
                        if hal::hosted_native::trace_enabled() { log!("native-app focus owner={owner}"); }
                        if let Some(previous) = s.embedded_focus.replace(key).filter(|k| *k != key) {
                            if let Some(mut old) = s.embedded.remove(&previous) {
                                old.release_keys(s); old.focused = false; old.configure(s); old.state(state::FOCUS_OUT, 0, &[]);
                                s.embedded.insert(previous, old);
                            }
                        }
                        e.focused = true; host_send(s, Command::KeyboardFocus(e.main)); e.configure(s); e.state(state::FOCUS_IN, 0, &[]);
                    }
                    op::BLUR => {
                        if hal::hosted_native::trace_enabled() { log!("native-app blur owner={owner}"); }
                        e.release_keys(s);
                        if s.embedded_focus == Some(key) { s.embedded_focus = None; host_send(s, Command::KeyboardFocus(None)); }
                        e.focused = false; e.configure(s); e.state(state::FOCUS_OUT, 0, &[]);
                    }
                    op::INPUT => e.input(s, &data),
                    _ => {}
                },
                _ => {}
            }
            e.poll(cx, s);
            s.embedded.insert(key, e);
        });
    }

    fn native_app_launch(&mut self, cx: &mut Cx, owner: ClientId, id: NativeAppId, app: &str) -> Result<(), String> {
        if with(|s| s.embedded.contains_key(&(owner, id))) { return Ok(()); }
        let entry = catalog_apps().into_iter().find(|a| a.id == app).ok_or_else(|| format!("No registered Wayland app '{app}'"))?;
        let (socket, dir) = self.wayland_server(cx)?;
        let (server, client) = UnixStream::pair().map_err(|e| e.to_string())?;
        let fd = client.as_raw_fd();
        let mut process = Process::new(&entry.argv[0]);
        makepad_app_registry::child_env::scrub(&mut process);
        process.args(&entry.argv[1..]).envs(entry.env).env("WAYLAND_SOCKET", fd.to_string()).env("WAYLAND_DISPLAY", socket)
            .env("XDG_RUNTIME_DIR", &dir).env("XDG_SESSION_TYPE", "wayland").env_remove("DISPLAY")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit());
        // The dedicated inherited connection authenticates this launch even before app_id/title.
        unsafe {
            process.pre_exec(move || {
                unsafe extern "C" { fn fcntl(fd: i32, cmd: i32, ...) -> i32; }
                if fcntl(fd, 2, 0) < 0 { return Err(std::io::Error::last_os_error()); }
                Ok(())
            });
        }
        let child = process.spawn().map_err(|e| format!("Launch {app}: {e}"))?;
        log!("wm: embedded {app} pid={} owner={owner} layer={id:?}", child.id());
        with(|s| {
            s.next_embedded += 1;
            let token = s.next_embedded;
            s.embedded_pending.insert(token, (owner, id));
            s.embedded.insert((owner, id), Embedded { owner, id, child, connection: None, main: None, size: (640, 480), focused: false, pointer: false, keys: HashSet::new(),
                buffers: HashMap::new(), held: HashMap::new(), current: None, serial: 0, mapped: false, pixels: Vec::new(), pixels_size: (0, 0), dirty: false,
                slots: Vec::new(), upload_waiting: false, outbox: VecDeque::new(), releases: Vec::new(), closing: false, next_frame: None });
            host_send(s, Command::Attach { stream: server, token });
        });
        Ok(())
    }

    pub(super) fn native_app_poll(&mut self, cx: &mut Cx) {
        with(|s| {
            for key in s.embedded.keys().copied().collect::<Vec<_>>() {
                let Some(mut e) = s.embedded.remove(&key) else { continue };
                e.poll(cx, s);
                if matches!(e.child.try_wait(), Ok(Some(_))) {
                    e.release_keys(s);
                    if s.embedded_focus == Some(key) { s.embedded_focus = None; }
                    for (buffer, fenced) in &e.releases {
                        if *fenced { makepad_app_host::hosted::native_discard_release_fence(e.owner, *buffer); }
                    }
                    if let Some(connection) = e.connection { s.embedded_connections.remove(&connection); host_send(s, Command::Disconnect(connection)); }
                    e.state(state::CLOSED, 0, &[]); e.poll(cx, s);
                    log!("wm: embedded client exited owner={} layer={:?}", e.owner, e.id);
                } else { s.embedded.insert(key, e); }
            }
        });
    }

    pub(crate) fn native_app_tick(&mut self, cx: &mut Cx, event: &Event) {
        if !matches!(event, Event::NextFrame(_)) { return; }
        let due = with(|s| {
            let mut due = false;
            for e in s.embedded.values_mut() {
                if e.next_frame.is_some_and(|n| n.is_event(event).is_some()) { e.next_frame = None; due = true; }
            }
            due
        });
        if due { self.native_app_poll(cx); }
    }

    pub(crate) fn native_app_owner_gone(&mut self, owner: ClientId) {
        with(|s| {
            for key in s.embedded.keys().copied().filter(|k| k.0 == owner).collect::<Vec<_>>() {
                if let Some(e) = s.embedded.get_mut(&key) {
                    e.closing = true;
                    let connection = e.connection;
                    if let Some(connection) = connection { host_send(s, Command::Disconnect(connection)); }
                }
            }
        });
    }

    pub(super) fn native_app_event(&mut self, cx: &mut Cx, event: UiEvent) -> Option<UiEvent> {
        if let UiEvent::ClientAttached { client, token } = event {
            with(|s| { if let Some(key) = s.embedded_pending.remove(&token) {
                s.embedded_connections.insert(client, key);
                if let Some(e) = s.embedded.get_mut(&key) { e.connection = Some(client); }
            } });
            return None;
        }
        let client = match &event {
            UiEvent::ToplevelNew { surface } | UiEvent::ToplevelTitle { surface, .. } | UiEvent::ToplevelAppId { surface, .. } |
            UiEvent::Commit { surface, .. } | UiEvent::Mapped { surface } | UiEvent::Unmapped { surface } | UiEvent::SurfaceGone { surface } |
            UiEvent::WindowGeometry { surface, .. } | UiEvent::ToplevelLimits { surface, .. } | UiEvent::ToplevelRequest { surface, .. } |
            UiEvent::PopupNew { surface, .. } | UiEvent::PopupMoved { surface, .. } => surface.0,
            UiEvent::SubsurfaceTree { parent, .. } => parent.0,
            UiEvent::ClientGone(client) | UiEvent::ClientKilled { client, .. } | UiEvent::SetCursorShape { client, .. } |
            UiEvent::SetCursor { client, .. } | UiEvent::BufferGone { client, .. } => *client,
            _ => return Some(event),
        };
        let Some(key) = with(|s| s.embedded_connections.get(&client).copied()) else { return Some(event) };
        with(|s| {
            let Some(mut e) = s.embedded.remove(&key) else { return };
            match event {
                UiEvent::ToplevelNew { surface } if e.main.is_none() => { e.main = Some(surface); e.configure(s); }
                UiEvent::Commit { surface, serial, buffer, .. } if e.main == Some(surface) => e.commit(cx, serial, buffer),
                UiEvent::ToplevelTitle { title, .. } => e.state(state::TITLE, 0, title.as_bytes()),
                UiEvent::SetCursorShape { shape, .. } => e.state(state::CURSOR, shape as u64, &[]),
                UiEvent::BufferGone { buffer, .. } => {
                    if let Some(buffer) = e.buffers.remove(&buffer) {
                        e.held.remove(&buffer); e.event(NativeAppEvent::Forget { id: e.id, buffer });
                    }
                }
                UiEvent::ClientGone(_) => { e.closing = true; e.release_keys(s); e.state(state::CLOSED, 0, &[]); }
                UiEvent::SurfaceGone { surface } if e.main == Some(surface) => {
                    e.closing = true; e.release_keys(s); e.state(state::CLOSED, 0, &[]);
                    host_send(s, Command::Disconnect(client));
                }
                UiEvent::ClientKilled { why, .. } => e.state(state::CRASHED, 0, why.as_bytes()),
                _ => {}
            }
            e.poll(cx, s);
            s.embedded.insert(key, e);
        });
        None
    }
}

impl Embedded {
    fn key(&mut self, s: &Shared, time: u32, evdev: u16, pressed: bool) {
        let changed = if pressed { self.keys.insert(evdev) } else { self.keys.remove(&evdev) };
        if changed { host_send(s, Command::Key { time, evdev, pressed }); }
    }
    fn release_keys(&mut self, s: &Shared) {
        for evdev in self.keys.drain() { host_send(s, Command::Key { time: now_ms(), evdev, pressed: false }); }
    }
    fn input(&mut self, s: &Shared, packed: &[u8]) {
        let Some(main) = self.main else { return };
        let Some((raw, payload)) = hal::buffers::unpack_forward(packed) else { return };
        let Some(event) = raw.decode(payload) else { return };
        let time = now_ms();
        // Remote input and a focus transfer can carry modifiers without a separate key press.
        // Feed the same evdev tracker as ordinary Wayland tiles so keymap/modifier events agree.
        let mods = match event {
            hal::HalEvent::KeyDown { mods, .. } | hal::HalEvent::KeyUp { mods, .. } |
            hal::HalEvent::MouseDown { mods, .. } | hal::HalEvent::MouseUp { mods, .. } |
            hal::HalEvent::MouseMove { mods, .. } | hal::HalEvent::Scroll { mods, .. } => Some(mods),
            _ => None,
        };
        if let Some(mods) = mods.filter(|_| self.focused) {
            for (mask, evdev) in [(hal::input::MOD_SHIFT, 42), (hal::input::MOD_CONTROL, 29), (hal::input::MOD_ALT, 56), (hal::input::MOD_LOGO, 125)] {
                self.key(s, time, evdev, mods & mask != 0);
            }
        }
        let point = match event {
            hal::HalEvent::MouseDown { x, y, .. } | hal::HalEvent::MouseUp { x, y, .. } | hal::HalEvent::MouseMove { x, y, .. } |
            hal::HalEvent::Scroll { x, y, .. } => Some((x, y)), _ => None,
        };
        if let Some((x, y)) = point {
            host_send(s, Command::PointerEnter { surface: main, x, y });
            host_send(s, Command::PointerMotion { time, x, y });
            self.pointer = true;
        }
        match event {
            hal::HalEvent::MouseDown { button, .. } | hal::HalEvent::MouseUp { button, .. } => {
                let button = crate::wayland_view::evdev_button(button);
                let pressed = matches!(event, hal::HalEvent::MouseDown { .. });
                host_send(s, Command::PointerButton { time, button, pressed });
            }
            hal::HalEvent::MouseLeave { .. } => { if self.pointer { host_send(s, Command::PointerLeave); } self.pointer = false; }
            hal::HalEvent::Scroll { dx, dy, precise, .. } => {
                for (axis, v) in [(0, dy), (1, dx)] {
                    if v != 0.0 { let (value, v120) = crate::wayland_view::axis_value(v, precise); host_send(s, Command::PointerAxis { time, axis, value, v120 }); }
                }
            }
            hal::HalEvent::KeyDown { key, repeat: false, .. } | hal::HalEvent::KeyUp { key, .. } if self.focused => {
                if let Some(evdev) = crate::wayland_view::evdev_of(KeyCode::from_index(key)) {
                    self.key(s, time, evdev, matches!(event, hal::HalEvent::KeyDown { .. }));
                }
            }
            _ => {}
        }
    }
}
