//! The Linux side: the server thread, per-surface textures, launching client processes and
//! turning a mapped toplevel into a client slot with a tile.

use super::*;
use crate::{clients, App};
use makepad_widgets::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use makepad_widgets::makepad_platform::foreign_dmabuf::{ForeignDmabuf, ForeignDmabufPlane, ForeignDmabufFence};
use wayland_server::compositor::{Board, BufferUpdate, DmabufConfig, DmabufFrame, Frame, OutputInfo, Rect as WRect, UiEvent};
use wayland_server::host::{Command, Host};
use wayland_server::compositor::xdg::{STATE_ACTIVATED, STATE_TILED_BOTTOM, STATE_TILED_LEFT, STATE_TILED_RIGHT, STATE_TILED_TOP};

struct Image {
    texture: Texture,
    /// Logical size and viewport crop from the last commit.
    logical: (i32, i32),
    src: Option<(f64, f64, f64, f64)>,
    /// The client dmabuf buffer this image shows: released when the surface moves on.
    held: Option<(u32, u32)>,
    flip: bool,
    w: usize,
    h: usize,
    opaque: bool,
}

#[derive(Default)]
struct Win {
    main: Surf,
    title: String,
    app_id: String,
    popups: Vec<(Surf, WRect)>,
}

#[derive(Default)]
struct Shared {
    host: Option<Host>,
    dir: PathBuf,
    images: HashMap<Surf, Image>,
    geoms: HashMap<Surf, WRect>,
    trees: HashMap<Surf, Vec<Option<(Surf, (i32, i32))>>>,
    wins: HashMap<ClientId, Win>,
    owner: HashMap<Surf, ClientId>,
    /// Toplevels that exist but have not mapped yet: title and app id so far.
    unmapped: HashMap<Surf, (String, String)>,
    children: Vec<Child>,
    /// Surfaces with new content whose frame callbacks fire once a draw showed them.
    owed: HashSet<Surf>,
    /// Cursor shape per wayland client.
    cursors: HashMap<u32, u32>,
    /// Imported client dmabufs, by (client, wl_buffer id).
    imports: HashMap<(u32, u32), Texture>,
    releases: Vec<(u32, u32, ForeignDmabufFence)>,
    release_retry: Vec<(u32, u32)>,
    /// The last text the makepad apps copied (clipboard) and selected (primary), handed to the
    /// server when it starts: a browser opened after a copy pastes it.
    selection: [Option<String>; 2],
}

thread_local! {
    static STATE: RefCell<Shared> = RefCell::new(Shared::default());
}
fn with<R>(f: impl FnOnce(&mut Shared) -> R) -> R { STATE.with(|s| f(&mut s.borrow_mut())) }

pub fn owns(client: ClientId) -> bool { with(|s| s.wins.contains_key(&client)) }
pub fn cursor_shape(client: ClientId) -> u32 {
    with(|s| s.wins.get(&client).and_then(|w| s.cursors.get(&w.main.0)).copied().unwrap_or(0))
}
pub fn main_surface(client: ClientId) -> Option<Surf> { with(|s| s.wins.get(&client).map(|w| w.main)) }

fn host_send(s: &Shared, cmd: Command) { if let Some(h) = &s.host { h.send(cmd); } }

pub fn send(cmd: ViewCmd) {
    with(|s| {
        let t = now_ms();
        match cmd {
            ViewCmd::Configure { client, w, h, activated } => {
                if let Some(win) = s.wins.get(&client) {
                    // A tiling WM: every edge is shared with the layout.
                    let mut states = vec![STATE_TILED_LEFT, STATE_TILED_RIGHT, STATE_TILED_TOP, STATE_TILED_BOTTOM];
                    if activated { states.push(STATE_ACTIVATED); }
                    host_send(s, Command::ConfigureToplevel { surface: win.main, width: w, height: h, states });
                }
            }
            ViewCmd::Scale(v) => host_send(s, Command::SetScale(v)),
            ViewCmd::PopupBounds { x, y, w, h } => host_send(s, Command::PopupBounds(WRect::new(x, y, w, h))),
            ViewCmd::KeyboardFocus(k) => host_send(s, Command::KeyboardFocus(k)),
            ViewCmd::PointerEnter { surf, x, y } => host_send(s, Command::PointerEnter { surface: surf, x, y }),
            ViewCmd::PointerLeave => host_send(s, Command::PointerLeave),
            ViewCmd::PointerMotion { x, y } => host_send(s, Command::PointerMotion { time: t, x, y }),
            ViewCmd::PointerButton { button, pressed } => host_send(s, Command::PointerButton { time: t, button, pressed }),
            ViewCmd::PointerAxis { axis, value, v120 } => host_send(s, Command::PointerAxis { time: t, axis, value, v120 }),
            ViewCmd::Key { evdev, pressed } => host_send(s, Command::Key { time: t, evdev, pressed }),
            ViewCmd::Selection { primary, text } => {
                let board = if primary { Board::Primary } else { Board::Clipboard };
                s.selection[board as usize] = Some(text.clone());
                host_send(s, Command::SetSelection { board, text });
            }
            ViewCmd::DismissPopups(client) => {
                if let Some(win) = s.wins.get(&client) {
                    for (p, _) in win.popups.iter().rev() { host_send(s, Command::DismissPopup(*p)); }
                }
            }
        }
    })
}

fn now_ms() -> u32 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u32
}

pub fn frames_presented(client: ClientId) {
    with(|s| {
        let t = now_ms();
        let Some(win) = s.wins.get(&client) else { return };
        let mut shown = Vec::new();
        flatten(s, win.main, (0, 0), &mut shown, 0);
        let mut surfs: Vec<Surf> = shown.into_iter().map(|(surf, _)| surf).collect();
        surfs.extend(win.popups.iter().map(|(p, _)| *p));
        for surface in surfs {
            if s.owed.remove(&surface) { host_send(s, Command::FrameDone { surface, time_ms: t }); }
        }
    })
}

fn flatten(s: &Shared, surf: Surf, origin: (i32, i32), out: &mut Vec<(Surf, (i32, i32))>, depth: usize) {
    if depth > 8 { return; }
    match s.trees.get(&surf) {
        None => out.push((surf, origin)),
        Some(stack) => {
            for item in stack {
                match item {
                    None => out.push((surf, origin)),
                    Some((child, off)) => flatten(s, *child, (origin.0 + off.0, origin.1 + off.1), out, depth + 1),
                }
            }
        }
    }
}

pub fn layers(client: ClientId) -> Vec<Layer> {
    with(|s| {
        let Some(win) = s.wins.get(&client) else { return Vec::new() };
        let g = s.geoms.get(&win.main).copied().unwrap_or_default();
        let mut flat = Vec::new();
        flatten(s, win.main, (0, 0), &mut flat, 0);
        let mut out = Vec::new();
        let mut push = |surf: Surf, pos: (i32, i32), popup: bool| {
            if let Some(img) = s.images.get(&surf) {
                let mut uv = match img.src {
                    Some((x, y, w, h)) => ((x / img.w as f64) as f32, (y / img.h as f64) as f32, (w / img.w as f64) as f32, (h / img.h as f64) as f32),
                    None => (0.0, 0.0, 1.0, 1.0),
                };
                if img.flip { uv = (uv.0, uv.1 + uv.3, uv.2, -uv.3); }
                out.push(Layer { surf, pos, size: img.logical, uv, opaque: img.opaque, popup, texture: img.texture.clone() });
            }
        };
        // The tile's origin is the main window's geometry origin.
        for (surf, o) in flat { push(surf, (o.0 - g.x, o.1 - g.y), false); }
        // Popups: rects are relative to the geometry origin already; the popup's own geometry offsets its surface.
        for (surf, r) in &win.popups {
            let pg = s.geoms.get(surf).copied().unwrap_or_default();
            push(*surf, (r.x - pg.x, r.y - pg.y), true);
        }
        out
    })
}

fn to_u32_rows(f: &Frame) -> Vec<u32> {
    let swap = matches!(f.format, shm_abgr::ABGR8888 | shm_abgr::XBGR8888);
    f.data.chunks_exact(4).map(|c| {
        let p = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        if swap { ((p & 0xff) << 16) | (p & 0xff00_ff00) | ((p >> 16) & 0xff) } else { p }
    }).collect()
}
mod shm_abgr {
    pub use wayland_server::shm::{ABGR8888, XBGR8888};
}

impl Shared {
    /// A dmabuf commit: import the buffer once, acquire it, show it, and release the one it replaces.
    fn apply_dmabuf(&mut self, cx: &mut Cx, surf: Surf, f: DmabufFrame) {
        let key = (surf.0, f.buffer);
        let data = f.data.clone();
        if !self.imports.contains_key(&key) {
            let planes: Option<Vec<ForeignDmabufPlane>> = data.planes.iter()
                .map(|p| p.fd.try_clone().ok().map(|fd| ForeignDmabufPlane { fd, offset: p.offset, stride: p.stride })).collect();
            let Some(planes) = planes else { return };
            let buffer = ForeignDmabuf { width: data.width, height: data.height, fourcc: data.fourcc, modifier: data.modifier, planes };
            match cx.foreign_dmabuf_import(buffer) {
                Ok(t) => {
                    log!("wm: dmabuf imported {}x{} {:#x}/{:#x} for {:?}", data.width, data.height, data.fourcc, data.modifier, key);
                    self.imports.insert(key, t);
                }
                Err(e) => { log!("wm: dmabuf import failed: {e}"); self.release(cx, surf.0, f.buffer); return; }
            }
        }
        let texture = self.imports[&key].clone();
        if let Err(e) = cx.foreign_dmabuf_acquire(&texture) { log!("wm: dmabuf acquire failed: {e}"); self.release(cx, surf.0, f.buffer); return; }
        let prev = self.images.insert(surf, Image {
            texture, logical: (data.width as i32, data.height as i32), src: None, held: Some(key), flip: data.flags & 1 != 0,
            w: data.width as usize, h: data.height as usize, opaque: data.fourcc == u32::from_le_bytes(*b"XR24"),
        });
        if let Some(Image { held: Some(old), .. }) = prev { if old != key { self.release(cx, old.0, old.1); } }
    }

    /// Tells the client its dmabuf buffer may be reused once what the GPU has been given is done.
    fn release(&mut self, cx: &mut Cx, client: u32, buffer: u32) {
        match cx.foreign_dmabuf_fence() {
            Ok(mut release) => match release.poll() {
                Some(Ok(fence)) => host_send(self, Command::BufferDone { client, buffer, fence }),
                Some(Err(error)) => {
                    log!("wm: dmabuf release deferred: {error}");
                    self.release_retry.push((client, buffer));
                }
                None => self.releases.push((client, buffer, release)),
            },
            Err(error) => {
                log!("wm: dmabuf release deferred: {error}");
                self.release_retry.push((client, buffer));
            }
        }
    }

    fn poll_releases(&mut self, cx: &mut Cx) {
        for (client, buffer) in std::mem::take(&mut self.release_retry) { self.release(cx, client, buffer); }
        let mut pending = Vec::new();
        for (client, buffer, mut release) in std::mem::take(&mut self.releases) {
            match release.poll() {
                Some(Ok(fence)) => host_send(self, Command::BufferDone { client, buffer, fence }),
                Some(Err(error)) => {
                    log!("wm: dmabuf release deferred: {error}");
                    self.release_retry.push((client, buffer));
                }
                None => pending.push((client, buffer, release)),
            }
        }
        self.releases = pending;
    }

    fn apply_frame(&mut self, cx: &mut Cx, surf: Surf, f: Frame) {
        let (w, h) = (f.width as usize, f.height as usize);
        let opaque = !wayland_server::shm::has_alpha(f.format);
        let rows = to_u32_rows(&f);
        match self.images.get_mut(&surf) {
            Some(img) if img.w == w && img.h == h => {
                img.opaque = opaque;
                let mut data = img.texture.take_vec_u32(cx);
                let start = f.y0 as usize * w;
                data[start..start + rows.len()].copy_from_slice(&rows);
                let dirty = RectUsize::new(PointUsize::new(0, f.y0 as usize), SizeUsize::new(w, (f.y1 - f.y0) as usize));
                img.texture.put_back_vec_u32(cx, data, Some(dirty));
            }
            _ => {
                if !f.is_full() { return; }
                let texture = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 { width: w, height: h, data: Some(rows), updated: TextureUpdated::Full });
                self.images.insert(surf, Image { texture, logical: (w as i32, h as i32), src: None, held: None, flip: false, w, h, opaque });
            }
        }
    }
}

impl App {
    /// Drains the server's events: images, window meta, mapping and closing.
    pub(crate) fn wayland_poll(&mut self, cx: &mut Cx) {
        with(|s| s.poll_releases(cx));
        let events: Vec<UiEvent> = with(|s| {
            s.children.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
            s.host.as_ref().map(|h| h.events.try_iter().collect()).unwrap_or_default()
        });
        if events.is_empty() { return; }
        for ev in events { self.wayland_event(cx, ev); }
        with(|s| s.poll_releases(cx));
        self.redraw_all(cx);
    }

    fn wayland_event(&mut self, cx: &mut Cx, ev: UiEvent) {
        match ev {
            UiEvent::ToplevelNew { surface } => with(|s| { s.unmapped.insert(surface, (String::new(), String::new())); }),
            UiEvent::ToplevelTitle { surface, title } => {
                let client = with(|s| {
                    if let Some(m) = s.unmapped.get_mut(&surface) { m.0 = title.clone(); }
                    let c = s.owner.get(&surface).copied();
                    if let Some(w) = c.and_then(|c| s.wins.get_mut(&c)) { w.title = title.clone(); }
                    c
                });
                if let Some(c) = client {
                    if let Some(slot) = self.state_mut().clients.get_mut(&c) { slot.title = title; }
                    self.update_bar(cx);
                }
            }
            UiEvent::ToplevelAppId { surface, app_id } => with(|s| {
                if let Some(m) = s.unmapped.get_mut(&surface) { m.1 = app_id.clone(); }
                if let Some(w) = s.owner.get(&surface).copied().and_then(|c| s.wins.get_mut(&c)) { w.app_id = app_id; }
            }),
            UiEvent::Commit { surface, buffer, size, src, .. } => {
                with(|s| {
                    match buffer {
                        BufferUpdate::Keep => {}
                        BufferUpdate::Detach => {
                            if let Some(Image { held: Some(b), .. }) = s.images.remove(&surface) { s.release(cx, b.0, b.1); }
                        }
                        BufferUpdate::Dmabuf(f) => s.apply_dmabuf(cx, surface, f),
                        BufferUpdate::Attach(f) => {
                            // A shm frame replacing a dmabuf image: drop it and release the buffer.
                            if s.images.get(&surface).is_some_and(|i| i.held.is_some()) {
                                if let Some(Image { held: Some(b), .. }) = s.images.remove(&surface) { s.release(cx, b.0, b.1); }
                            }
                            s.apply_frame(cx, surface, f)
                        }
                    }
                    if let Some(img) = s.images.get_mut(&surface) { img.logical = size; img.src = src; }
                    s.owed.insert(surface);
                });
            }
            UiEvent::SubsurfaceTree { parent, stack } => with(|s| {
                for item in stack.iter().flatten() { 
                    if let Some(c) = s.owner.get(&parent).copied() { s.owner.insert(item.0, c); }
                }
                s.trees.insert(parent, stack);
            }),
            UiEvent::WindowGeometry { surface, rect } => with(|s| { s.geoms.insert(surface, rect); }),
            UiEvent::Mapped { surface } => self.wayland_map(cx, surface),
            UiEvent::Unmapped { surface } => self.wayland_unmapped(cx, surface),
            UiEvent::SurfaceGone { surface } => self.wayland_gone(cx, surface),
            // A Wayland client copied (selected): the makepad apps paste it from the WM's clipboard.
            UiEvent::Selection { board, text } => {
                log!("wm: a wayland client {} {} bytes; the makepad apps paste it", if board == Board::Primary { "selected" } else { "copied" }, text.len());
                match board {
                    Board::Clipboard => cx.copy_to_clipboard(&text),
                    Board::Primary => cx.set_primary_selection(&text),
                }
            }
            UiEvent::ClientKilled { client, why } => log!("wm: wayland client {client} was cut off: {why}"),
            UiEvent::PopupNew { surface, parent, rect, .. } => with(|s| {
                let Some(client) = s.owner.get(&parent).copied() else { return };
                // Resolve a popup-on-popup to window coordinates.
                let origin = s.wins.get(&client).and_then(|w| w.popups.iter().find(|(p, _)| *p == parent).map(|(_, r)| (r.x, r.y))).unwrap_or((0, 0));
                let rect = WRect::new(rect.x + origin.0, rect.y + origin.1, rect.w, rect.h);
                s.owner.insert(surface, client);
                if let Some(w) = s.wins.get_mut(&client) { w.popups.push((surface, rect)); }
            }),
            UiEvent::PopupMoved { surface, rect } => with(|s| {
                if let Some(c) = s.owner.get(&surface).copied() {
                    if let Some(w) = s.wins.get_mut(&c) { if let Some(p) = w.popups.iter_mut().find(|(p, _)| *p == surface) { p.1 = rect; } }
                }
            }),
            UiEvent::ClientGone(c) => {
                with(|s| {
                    s.imports.retain(|k, _| k.0 != c);
                    s.releases.retain(|(client, _, _)| *client != c);
                    s.release_retry.retain(|(client, _)| *client != c);
                });
                let gone: Vec<Surf> = with(|s| s.wins.values().map(|w| w.main).filter(|m| m.0 == c).collect());
                for m in gone { self.wayland_gone(cx, m); }
            }
            UiEvent::BufferGone { client, buffer } => with(|s| { s.imports.remove(&(client, buffer)); }),
            UiEvent::SetCursorShape { client, shape } => with(|s| { s.cursors.insert(client, shape); }),
            // xdg_toplevel.set_fullscreen / unset_fullscreen: a fullscreen space.
            UiEvent::ToplevelRequest { surface, what: wayland_server::compositor::ToplevelRequest::Fullscreen(on) } => {
                if let Some(client) = with(|s| s.owner.get(&surface).copied()) {
                    self.fullscreen_request(cx, client, on);
                }
            }
            UiEvent::ToplevelLimits { .. } | UiEvent::ToplevelRequest { .. } | UiEvent::SetCursor { .. } => {}
        }
    }

    /// A toplevel got its first buffer: it becomes a client slot with a tile.
    fn wayland_map(&mut self, cx: &mut Cx, surface: Surf) {
        let Some((title, app_id)) = with(|s| s.unmapped.remove(&surface)) else { return };
        let id = self.next_id;
        self.next_id += 1;
        // A window of a catalog entry groups under the entry's id (its launcher icon); any other under its app id.
        let app = match catalog::for_window(&catalog_apps(), &app_id) {
            Some(entry) => entry.id.clone(),
            None if app_id.is_empty() => "wayland".to_string(),
            None => app_id.clone(),
        };
        let label = if title.is_empty() { app.clone() } else { title.clone() };
        with(|s| {
            s.owner.insert(surface, id);
            s.wins.insert(id, Win { main: surface, title: title.clone(), app_id, ..Default::default() });
        });
        self.state_mut().clients.insert(id, clients::ClientSlot::module(id, &app, &label));
        let area = self.desk_area(cx);
        let gap = self.state_mut().gap;
        self.state_mut().layout_mut().insert(id, area, gap);
        if let Some(mut d) = self.desk(cx).borrow_mut::<crate::desk::WmDesk>() { d.mark_wayland(id); }
        log!("wm: wayland window {:?} '{}' as client {}", surface, label, id);
        self.activate_client(cx, id);
        self.update_bar(cx);
    }

    /// A toplevel unmapped (its buffer went away, or its role did): the window leaves the desk, but the
    /// surface and its role state stay, so the same toplevel mapping again becomes a window again.
    fn wayland_unmapped(&mut self, cx: &mut Cx, surface: Surf) {
        let client = with(|s| {
            let c = s.owner.get(&surface).copied().filter(|c| s.wins.get(c).is_some_and(|w| w.main == surface))?;
            let win = s.wins.remove(&c)?;
            s.owner.remove(&surface);
            s.unmapped.insert(surface, (win.title, win.app_id));
            Some(c)
        });
        if let Some(client) = client {
            self.remove_client(cx, client);
            self.update_bar(cx);
        }
    }

    /// The surface itself is gone: nothing about it is kept.
    fn wayland_gone(&mut self, cx: &mut Cx, surface: Surf) {
        let client = with(|s| {
            if let Some(Image { held: Some(b), .. }) = s.images.remove(&surface) { s.release(cx, b.0, b.1); }
            s.trees.remove(&surface);
            s.geoms.remove(&surface);
            s.unmapped.remove(&surface);
            let c = s.owner.remove(&surface);
            for w in s.wins.values_mut() { w.popups.retain(|(p, _)| *p != surface); }
            s.owed.remove(&surface);
            c.filter(|c| s.wins.get(c).is_some_and(|w| w.main == surface))
        });
        if let Some(client) = client {
            with(|s| { s.wins.remove(&client); });
            self.remove_client(cx, client);
            self.update_bar(cx);
        }
    }

    /// Asks a foreign window to close; it ends when the client destroys its surface.
    pub(crate) fn wayland_close(&mut self, client: ClientId) -> bool {
        let Some(main) = main_surface(client) else { return false };
        with(|s| host_send(s, Command::CloseToplevel(main)));
        true
    }

    /// `wayland=<command line>` or a catalog id `wl.<name>` starts a Wayland client of the WM's own server.
    pub(crate) fn wayland_launch(&mut self, cx: &mut Cx, app_id: &str) -> bool {
        // Our own browser's id opens the Chromium that replaces it, when it is listed.
        let app_id = if app_id == "browser" && catalog_apps().iter().any(|a| a.id == "wl.chromium") { "wl.chromium" } else { app_id };
        // A listed app that already has a window is focused instead of started again.
        if app_id.starts_with("wl.") {
            let existing = self.state_mut().clients.iter().filter(|(_, s)| s.app == app_id && s.closing.is_none()).map(|(id, _)| *id).min();
            if let Some(client) = existing { self.activate_client(cx, client); return true; }
        }
        let (argv, env): (Vec<String>, Vec<(String, String)>) = if let Some(line) = app_id.strip_prefix("wayland=") {
            (line.split_whitespace().map(str::to_string).collect(), Vec::new())
        } else if app_id.starts_with("wl.") {
            match catalog_apps().into_iter().find(|a| a.id == app_id) {
                Some(a) => (a.argv, a.env),
                None => { log!("wm: no foreign app '{app_id}' (see ~/.makepad/wm/wayland_apps.txt and the .desktop entries)"); return true; }
            }
        } else {
            return false;
        };
        let Some(program) = argv.first().cloned() else { return true };
        let line = argv.join(" ");
        let area = self.desk_area(cx);
        let socket = match with(|s| -> std::io::Result<String> {
            if s.host.is_none() {
                let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
                let output = OutputInfo { width: area.w as i32, height: area.h as i32, ..Default::default() };
                let dmabuf = dmabuf_config(cx);
                let host = Host::spawn(&dir, "wayland-wm", output, keymap_layout(), dmabuf, || SignalToUI::set_ui_signal())?;
                for (board, text) in [Board::Clipboard, Board::Primary].into_iter().zip(s.selection.clone()) {
                    if let Some(text) = text { host.send(Command::SetSelection { board, text }); }
                }
                s.dir = dir;
                s.host = Some(host);
            }
            Ok(s.host.as_ref().unwrap().socket.clone())
        }) {
            Ok(n) => n,
            Err(e) => { log!("wm: wayland server failed to start: {e}"); return true; }
        };
        let dir = with(|s| s.dir.clone());
        let mut p = Process::new(&program);
        makepad_app_registry::child_env::scrub(&mut p);
        p.args(&argv[1..]).envs(env).env("WAYLAND_DISPLAY", &socket).env("XDG_RUNTIME_DIR", &dir).env("XDG_SESSION_TYPE", "wayland")
            .env_remove("DISPLAY").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        // A WM started from a console has no session bus address; its apps find the user bus
        // (and the WM's portal: dark mode, file dialogs) at the standard place.
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() && dir.join("bus").exists() {
            p.env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", dir.join("bus").display()));
        }
        match p.spawn() {
            Ok(child) => { log!("wm: started wayland client '{}' on {}", line, socket); with(|s| s.children.push(child)); }
            Err(e) => log!("wm: cannot start '{}': {}", program, e),
        }
        true
    }
}

thread_local! {
    static CATALOG: RefCell<Option<(std::time::Instant, Vec<catalog::ForeignApp>)>> = const { RefCell::new(None) };
}

/// The catalog, rescanned when it is older than 5 seconds (a config edit shows up in the launcher).
fn catalog_apps() -> Vec<catalog::ForeignApp> {
    CATALOG.with(|c| {
        let mut c = c.borrow_mut();
        if c.as_ref().is_none_or(|(at, _)| at.elapsed() > std::time::Duration::from_secs(5)) {
            let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
            *c = Some((std::time::Instant::now(), catalog::load(&home)));
        }
        c.as_ref().unwrap().1.clone()
    })
}

pub fn icon_file(id: &str) -> Option<PathBuf> { catalog_apps().iter().find(|a| a.id == id).and_then(catalog::icon_path) }

pub fn menu_apps() -> Vec<(String, String)> { catalog_apps().into_iter().map(|a| (a.id, a.label)).collect() }

/// What the GPU can import, to offer as linux-dmabuf; `MAKEPAD_WM_NO_DMABUF=1` keeps clients on shm.
fn dmabuf_config(cx: &mut Cx) -> Option<DmabufConfig> {
    if std::env::var_os("MAKEPAD_WM_NO_DMABUF").is_some() { return None; }
    let support = cx.foreign_dmabuf_support()?;
    log!("wm: dmabuf import offered: {} format/modifier pairs, render node {}", support.formats.len(), support.render_node.display());
    Some(DmabufConfig { main_device: support.render_device, formats: support.formats, render_node: Some(support.render_node) })
}

/// The layout clients get: `XKB_DEFAULT_LAYOUT` (first entry) or `MAKEPAD_WM_KEYMAP`, else US.
fn keymap_layout() -> &'static xkb_keymap::Layout {
    let name = std::env::var("MAKEPAD_WM_KEYMAP").or_else(|_| std::env::var("XKB_DEFAULT_LAYOUT")).unwrap_or_default();
    let first = name.split(',').next().unwrap_or("").trim();
    match xkb_keymap::by_name(first) {
        Some(l) => l,
        None => { if !first.is_empty() { log!("wm: keymap '{first}' is not one of us, gb, de, fr, dvorak; using us"); } &xkb_keymap::US }
    }
}
