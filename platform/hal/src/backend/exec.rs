//! The command executor every backend shares: it validates a command against the session
//! tables, keeps the tables and the ledger, answers the commands that need no GPU, and hands
//! what needs a device to the backend's [`Gpu`] hooks. The headless backend implements the hooks
//! without a device; the Metal backend implements them with one.

use crate::buffers::{pack_domains, read_items, read_strs, CmdView, PipelineBlob};
use crate::cmds::HalCmd;
use crate::ledger::{Kind, LedgerEntry};
use crate::session::*;
use crate::types::*;

/// How a pipeline create ended, or that it will end later (the backend then pushes
/// `PipelineReady` / `PipelineFailed` itself).
pub enum PipeStatus {
    Ready,
    Failed(String),
    Pending,
}

pub struct SubmittedFrame {
    pub deps: Vec<LentRef>,
    /// A window pass was encoded, rather than skipped for a missing drawable.
    pub window: bool,
}

/// What the executor hands to the backend's window system. A backend without one (headless,
/// offscreen windows) takes none of these ([`Gpu::os`] answers false) and the executor answers
/// what it can itself: geometry events, an immediate `Closed`, empty dialog results.
#[derive(Clone, Debug)]
pub enum OsCmd {
    WindowCreate { id: Id, kind: u32, parent: Id, geom: [f32; 4] },
    WindowClose { id: Id },
    WindowGeom { id: Id, geom: [f32; 4] },
    WindowState { id: Id, state: u32 },
    WindowTitle { id: Id, title: String },
    WindowVisuals { id: Id, visuals: u32, params: [f32; 4] },
    WindowTopmost { id: Id, on: bool },
    WindowButtons { id: Id, show: bool },
    WindowButtonsCenter { id: Id, y_center: f32 },
    WindowDpiOverride { id: Id, dpi_override: Option<f64> },
    ChromelessWhenMax { id: Id, on: bool },
    Cursor { id: Id, cursor: u32 },
    PointerLock { id: Id, mode: u32 },
    ShowInDock { on: bool },
    ActivateApp,
    ImeShow { id: Id, area: [f32; 4], config: u32 },
    ImeHide { id: Id },
    ImeSync { id: Id, selection: [u32; 2], composition: [u32; 2], text: String },
    ClipSet { text: String },
    ClipRead { req: u64, primary: bool },
    SelectionSet { text: String },
    DragStart { id: Id, items: Vec<(u32, String)> },
    MenuSet { items: Vec<(MenuItem, String)> },
    FileDialog { req: u32, kind: u32, title: String, location: String, filename: String, filters: Vec<String> },
    ShareText { text: String },
    ClipboardActions { id: Id, show: bool, area: [f32; 4], selection: bool },
    AppIcon { w: u32, h: u32, rgba: Vec<u8> },
    DragReply { id: Id, to: u32, answer: u32 },
    FrameRequest { id: Id },
    MobileUi { op: u32, value: u32 },
    BackUnhandled,
    SelectionHandles { id: Id, op: u32, start: [f32; 2], end: [f32; 2] },
    #[cfg(target_os = "linux")]
    DirectDisplay { req: u64, command: crate::direct::DisplayCommand },
    #[cfg(target_os = "linux")]
    DirectPointerSpeed { touchpad: bool, hundredths: u32 },
    #[cfg(target_os = "linux")]
    DirectNaturalScroll { on: bool },
    #[cfg(target_os = "linux")]
    DirectTapToClick { on: bool },
}

/// A window position the window system was to choose (NaN) is the origin where nothing places windows
/// (offscreen backends, embedded child windows): the app hears real coordinates.
fn unplaced_at_origin(mut geom: [f32; 4]) -> [f32; 4] {
    for v in &mut geom[..2] {
        if v.is_nan() {
            *v = 0.0;
        }
    }
    geom
}

/// What the executor needs a device for. Every hook runs inside `hal_submit` with the state lock
/// held, so it must not wait for the GPU. Borrowed slices are valid for the call only.
pub trait Gpu {
    /// Native device totals when available; otherwise admission uses the session ledgers.
    fn gpu_budget(&self) -> Option<(u64, u64)> { None }
    /// A texture's create failed with `error`. True when the backend keeps it waiting for room
    /// (the device was out of memory: `gpu_pressure`) and tells its app later; false sends
    /// `TextureEvicted` now.
    fn texture_starved(&mut self, _sid: u32, _id: Id, _t: &TextureObj, _error: &str) -> bool { false }
    /// A texture was validated; the hook creates its storage and may refine `t.bytes` to what the
    /// device really holds. `Err` refuses the create (counted as a bad argument).
    fn texture_create(&mut self, s: &mut Session, id: Id, t: &mut TextureObj) -> Result<(), String>;
    /// `data` is tightly packed rows of `rect` at `mip` / `layer`; already bounds-checked.
    #[allow(clippy::too_many_arguments)]
    fn texture_update(&mut self, s: &mut Session, id: Id, t: &TextureObj, mip: u16, rect: [u32; 4], layer: u32, data: &[u8]);
    fn texture_destroy(&mut self, sid: u32, id: Id);
    /// Answers with a `Readback` event (now or when the GPU is done).
    fn texture_readback(&mut self, s: &mut Session, id: Id, req: u32, rect: [u32; 4]);
    fn texture_share(&mut self, s: &mut Session, id: Id);
    /// The object an imported handle stands for, `None` when it cannot be imported.
    fn texture_import(&mut self, s: &mut Session, id: Id, handle: u64, kind: u32) -> Option<TextureObj>;
    /// The object a GL texture name of the GLES backend's share group stands for (`TextureAdoptGl`,
    /// validated); `None` on every other backend: GL interop is not available there.
    #[allow(clippy::too_many_arguments)]
    fn texture_adopt_gl(&mut self, _s: &mut Session, _id: Id, _name: u32, _format: u32, _w: u32, _h: u32, _owned: bool) -> Option<TextureObj> {
        None
    }
    /// A fence of the app's GL context the following frames wait for (the GLES backend only).
    fn gl_fence(&mut self, s: &mut Session, _sync: u64) {
        s.bad();
    }
    fn buffer_create(&mut self, s: &mut Session, id: Id, b: &mut BufferObj) -> Result<(), String>;
    fn buffer_update(&mut self, s: &mut Session, id: Id, b: &mut BufferObj, off: u64, data: &[u8]);
    fn buffer_destroy(&mut self, sid: u32, id: Id);
    fn buffer_evictable(&mut self, sid: u32, id: Id);
    fn pipeline_create(&mut self, s: &mut Session, id: Id, dialect: u32, blob: &PipelineBlob, attr: Attr) -> PipeStatus;
    /// Whether the pipeline was still compiling (it no longer counts as pending).
    fn pipeline_destroy(&mut self, sid: u32, id: Id) -> bool;
    /// Executes one frame packet for a live target and answers `FrameDone`. `None` = the packet
    /// was malformed (no `FrameDone`); otherwise the surfaces lent by other sessions that the
    /// frame sampled and whether it encoded a window pass (see [`lent_state`]).
    #[allow(clippy::too_many_arguments)]
    fn frame(&mut self, s: &mut Session, sid: u32, target: Id, kind: u32, packet: u32, view: &CmdView, now: u64) -> Option<SubmittedFrame>;
    /// True when a frame is finished as `frame` returns (no GPU to wait for): the executor then
    /// reports it through `World::frame_done`. Backends with a GPU report from their completion
    /// handler through the same call.
    fn completes_inline(&self) -> bool {
        false
    }
    fn pick(&mut self, s: &mut Session, window: Id, req: u32, pos: [f32; 2]);
    /// Frame capture ([`crate::framecap`]): a copy of the texture's storage as everything submitted
    /// so far leaves it, before the frame that follows. Every mip level of every layer, tightly
    /// packed rows (whole 4x4 blocks for block formats), level-major then layer. `done` runs once
    /// the GPU has it (on any thread); `None`: no storage yet, or this backend cannot read it.
    /// Never waits for the GPU.
    fn capture_texture(&mut self, _s: &mut Session, _id: Id, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>) {
        done(None)
    }
    /// Frame capture: the buffer's bytes as the next frame reads them (`None`: no storage, or this
    /// backend cannot read it).
    fn capture_buffer(&mut self, _sid: u32, _id: Id) -> Option<Vec<u8>> {
        None
    }
    fn window_copy(&mut self, s: &mut Session, _window: Id, _texture: Id, req: u32) {
        s.events.readback(req, readback::NO_RASTER, [0; 4], &[]);
    }
    fn snapshot(&mut self, s: &mut Session, window: Id, req: u32);
    fn window_closed(&mut self, sid: u32, id: Id);
    /// Attach/detach the session's alias of a permanent host window. The physical window,
    /// swapchain and last presented image stay with the host.
    fn window_lent(&mut self, _sid: u32, _id: Id, _host: Id, _attach: bool) {}
    /// Configure the separate host surface; `None` restores the last base image
    /// immediately, including when the borrower has stopped producing frames.
    /// `layer`: the window layer's texture of session 0 (NONE: an opaque panel at `rect`).
    fn window_overlay(&mut self, s: &mut Session, id: Id, rect: Option<[f32; 4]>, layer: Id, now: u64);
    /// Mobile shells lend their permanent host window even without an explicit adopt flag.
    fn auto_adopt(&self) -> bool { false }
    /// The window system composites session 0's badges (`window_kind::BADGE`) over its windows.
    fn badges(&self) -> bool { false }
    /// A command for the window system, validated against the tables. True when the backend's
    /// window system took it and reports the outcome itself (events from the OS).
    fn os(&mut self, _sid: u32, _cmd: OsCmd) -> bool {
        false
    }
    /// A media command (video playback, native views, audio, MIDI, camera, game input, location,
    /// permissions). False when the backend has no media: the executor then answers with the
    /// failure the command's event table gives ([`media_unavailable`]).
    fn media(&mut self, _s: &mut Session, _c: &HalCmd) -> bool {
        false
    }
}

/// What a texture id a draw samples stands for.
pub enum LentState {
    /// the session's own texture (or none)
    Own,
    /// a surface lent by another session (an embedded window of a child)
    Lent(LentRef),
    /// a lent id whose surface is gone (its session ended or was relaunched): never sampled
    Dead,
}

/// Resolves a texture id of a draw: ids from [`LENT_BASE`] on are the HAL's own aliases of other
/// sessions' surfaces.
/// A pass larger than one of its attachments (`size` vs the attachments' common `fits`): it is
/// drawn into the attachments' common area (render area and scissors clamp to it; the viewport
/// keeps the pass's mapping), as Metal does, never dropped -- a dropped pass drew nothing every
/// frame (a target sized a pixel under its pass at a fractional scale). Counted, and logged once
/// per target: a pass is sized as its target (`Cx::pass_pixel_size`), so this is a caller's bug.
pub fn clamped_pass(s: &mut Session, target: Id, size: [u32; 2], fits: [u32; 2], dpi: f32) {
    s.stats.clamped_passes += 1;
    if s.clamped_pass_targets.contains(&target) { return; }
    s.clamped_pass_targets.push(target);
    eprintln!("[hal] pass {size:?} at dpi {dpi} is larger than its attachments {fits:?} (target {target:?}); drawn clipped to them");
}

pub fn lent_state(s: &Session, sid: u32, id: Id) -> LentState {
    if id.is_none() || id.index < LENT_BASE {
        return LentState::Own;
    }
    match s.textures.get(sid, id) {
        Ok(e) => e.val.lent.map_or(LentState::Own, LentState::Lent),
        Err(_) => LentState::Dead,
    }
}

/// Bytes of a mip chain (`depth` layers, a cube has 6).
pub fn texture_bytes(format: u32, w: u32, h: u32, depth: u32, mips: u32) -> u64 {
    let (mut w, mut h) = (w.max(1), h.max(1));
    let mut total = 0;
    for _ in 0..mips.max(1) {
        total += format::level_bytes(format, w, h) * depth.max(1) as u64;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    total
}

pub fn texture_detail(format: u32, usage: u16) -> &'static str {
    if format::is_depth(format) {
        "depth"
    } else if usage & usage::SHARED != 0 {
        "shared surface"
    } else if usage & usage::RENDER_TARGET != 0 {
        "render target"
    } else {
        "texture"
    }
}

pub fn buffer_detail(kind: u16) -> &'static str {
    match kind {
        buffer_kind::INSTANCE => "instance buffers",
        buffer_kind::VERTEX | buffer_kind::INDEX => "geometry",
        _ => "uniform buffers",
    }
}

/// Executes one batch.
pub fn submit(gpu: &mut dyn Gpu, world: &mut World, sid: u32, view: &CmdView) {
    // glyphs the workers finished since the last look go to the atlas before this batch draws
    crate::font::service(gpu, world);
    let now = world.now();
    if let Some(s) = world.session_mut(sid) {
        s.events.set_time(now);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let prof = crate::gpu_prof::submit_begin(sid);
    for cmd in view.cmds {
        match cmd.decode(view.arena) {
            Some(c) => exec(gpu, world, sid, c, view, now),
            None => {
                if let Some(s) = world.session_mut(sid) {
                    s.stats.unknown_cmds += 1;
                }
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    crate::gpu_prof::submit_end(prof);
}

/// Fires the due timers of a session (a late pump fires once and re-arms from now).
pub fn step_timers(world: &mut World, sid: u32) {
    let now = world.now();
    let Some(s) = world.session_mut(sid) else { return };
    s.events.set_time(now);
    let mut due: Vec<u32> = s.timers.iter().filter(|(_, t)| t.due_ns <= now).map(|(id, _)| *id).collect();
    due.sort();
    for id in due {
        s.events.timer(id);
        let t = s.timers.get_mut(&id).expect("due timer");
        if t.repeat {
            t.due_ns = now + t.interval_ns.max(1);
        } else {
            s.timers.remove(&id);
        }
    }
}

/// The nearest timer of a session.
pub fn next_timer(world: &World, sid: u32) -> Option<u64> {
    world.session(sid)?.timers.values().map(|t| t.due_ns).min()
}

/// The two device hooks a backend gives [`device_reset`]; the rest of a reset is shared.
pub trait DeviceReset: Gpu {
    /// Replaces the lost device: every event the old device still owed is queued (answered as
    /// failed where it has no result), the device objects are dropped (the tables keep their
    /// entries), windows lose their swapchains and retained images (made again by the next
    /// frame) and the new device is made. `Err`: no device yet, the caller tries again later.
    fn replace_device(&mut self, world: &mut World) -> Result<(), String>;
    /// Compiles every live pipeline again on the new device (from what the backend kept of its
    /// create); each answers with `PipelineReady` / `PipelineFailed` as on its first create.
    fn rebuild_pipelines(&mut self, world: &mut World);
}

/// A backend noticed its device is gone: every session hears `DeviceLost` once, with the
/// generation of the device that was lost. Returns that generation.
pub fn device_lost(world: &mut World) -> u32 {
    let generation = world.device_generation;
    for sid in world.session_ids() {
        world.to_session(sid, |q| q.device_lost(generation));
    }
    generation
}

/// Brings every session back after a device loss ([`device_lost`] was posted): the backend
/// replaces the device, every texture and buffer gets new storage under its id (contents are the
/// app's to upload again), shared textures answer with their new handle, imported ones as not
/// imported (the app imports again), pipelines are rebuilt, the font atlases are restored from
/// the font service's own copy, then every session hears `DeviceReset`. False when the device
/// could not be made yet (nothing changed; call again later).
pub fn device_reset(b: &mut dyn DeviceReset, world: &mut World) -> bool {
    if let Err(e) = b.replace_device(world) {
        eprintln!("[hal] device reset: {e}; trying again");
        return false;
    }
    for sid in world.session_ids() {
        let Some(s) = world.session_mut(sid) else { continue };
        for id in s.textures.keys_of(sid) {
            let Ok(e) = s.textures.get(sid, id) else { continue };
            // aliases of other sessions' objects are restored with their owner
            if e.owner_session != sid || e.val.lent.is_some() {
                continue;
            }
            if e.val.imported {
                s.events.texture_imported(id, false);
                continue;
            }
            let mut obj = e.val.clone();
            match b.texture_create(s, id, &mut obj) {
                Ok(()) if obj.usage & usage::SHARED != 0 => b.texture_share(s, id),
                Ok(()) => {}
                Err(e) => eprintln!("[hal] device reset: texture {id:?} of session {sid}: {e}"),
            }
        }
        for id in s.buffers.keys_of(sid) {
            let Ok(e) = s.buffers.get(sid, id) else { continue };
            if e.owner_session != sid {
                continue;
            }
            let mut obj = e.val.clone();
            match b.buffer_create(s, id, &mut obj) {
                Ok(()) if obj.evictable => b.buffer_evictable(sid, id),
                Ok(()) => {}
                Err(e) => eprintln!("[hal] device reset: buffer {id:?} of session {sid}: {e}"),
            }
        }
    }
    b.rebuild_pipelines(world);
    if let Some(mut fonts) = world.fonts.take() {
        fonts.device_reset(b, world);
        world.fonts = Some(fonts);
    }
    world.device_generation += 1;
    let generation = world.device_generation;
    for sid in world.session_ids() {
        if let Some(s) = world.session_mut(sid) {
            s.device_generation = generation;
        }
        world.to_session(sid, |q| q.device_reset(generation));
    }
    true
}

/// Frees a session's table entries and ledger rows (device objects are the backend's).
pub fn free_session(world: &mut World, sid: u32) {
    if let Some(s) = world.session_mut(sid) {
        for id in s.textures.keys_of(sid) {
            let _ = s.textures.remove(sid, id);
            s.ledger.remove(Kind::Texture, id.index);
        }
        for id in s.buffers.keys_of(sid) {
            let _ = s.buffers.remove(sid, id);
            s.ledger.remove(Kind::Buffer, id.index);
        }
        for id in s.pipelines.keys_of(sid) {
            let _ = s.pipelines.remove(sid, id);
            s.ledger.remove(Kind::Pipeline, id.index);
        }
        s.timers.clear();
    }
}

fn exec(gpu: &mut dyn Gpu, world: &mut World, sid: u32, c: HalCmd, view: &CmdView, now: u64) {
    // window commands that move objects between sessions work on the world
    match c {
        HalCmd::GpuBudget { epoch } => {
            let (device_bytes, used_bytes) = gpu.gpu_budget().unwrap_or_else(|| {
                let used = world.session_ids().into_iter().filter_map(|id| world.session(id))
                    .fold(0u64, |bytes, s| bytes.saturating_add(s.ledger.total_bytes()));
                (0, used)
            });
            world.to_session(sid, |q| q.gpu_budget(epoch, device_bytes, used_bytes));
            return;
        }
        HalCmd::WindowCreate { id, kind, parent, geom, adopt } => {
            if kind & 0xff == window_kind::HOSTED {
                let Some(s) = world.session_mut(sid) else { return };
                if s.windows.insert(sid, id, sid, WindowObj { kind, parent, geom, ..Default::default() }).is_err() {
                    s.stale();
                }
                // The external compositor supplies the first geometry and every frame beat.
                return;
            }
            if kind & 0xff == window_kind::BADGE {
                // session 0's, on one of its windows, where the window system composites badges
                let Some(s) = world.session_mut(sid) else { return };
                if sid != HOST || !gpu.badges() || s.windows.get(HOST, parent).is_err() {
                    s.bad();
                    return;
                }
                if s.windows.insert(sid, id, sid, WindowObj { kind, parent, geom, dpi: Some(s.dpi), ..Default::default() }).is_err() {
                    s.stale();
                    return;
                }
                gpu.os(sid, OsCmd::WindowCreate { id, kind, parent, geom });
                return;
            }
            // a child domain's window is a render target lent to its parent, never an OS window
            if world.session(sid).is_some_and(|s| s.parent != 0) {
                let geom = unplaced_at_origin(geom);
                let made = world.create_embedded(sid, id, kind, parent, geom, now);
                let Some(s) = world.session_mut(sid) else { return };
                match made {
                    Some(_) => {
                        let dpi = s.dpi;
                        s.events.display(id, dpi, 60.0);
                        s.events.geom(id, geom[0] as f64, geom[1] as f64, geom[2] as f64, geom[3] as f64, dpi, window_state::RESTORE);
                        s.events.frame_ready(id, 0);
                    }
                    None => s.stale(),
                }
                return;
            }
            let adopted = (adopt || gpu.auto_adopt()).then(|| world.adopt_window(sid, id)).flatten();
            let Some(s) = world.session_mut(sid) else { return };
            let geom = match adopted {
                Some(g) => g,
                None => {
                    if s.windows.insert(sid, id, sid, WindowObj { kind, parent, geom, adoptable: sid == HOST, ..Default::default() }).is_err() {
                        s.stale();
                        return;
                    }
                    geom
                }
            };
            // an adopted window exists already; a new one is the window system's to make
            let handled = if let Some(host) = s.windows.get(sid, id).ok().and_then(|w| w.val.alias_of) {
                gpu.window_lent(sid, id, host, true);
                false
            } else {
                gpu.os(sid, OsCmd::WindowCreate { id, kind, parent, geom })
            };
            if !handled {
                // no window system to place it: a NaN position (the app asked the window system to
                // choose) is the origin
                let geom = unplaced_at_origin(geom);
                let dpi = s.dpi;
                if let Ok(w) = s.windows.get_mut(sid, id) { w.val.geom = geom; w.val.dpi = Some(dpi); }
                s.events.geom(id, geom[0] as f64, geom[1] as f64, geom[2] as f64, geom[3] as f64, dpi, window_state::RESTORE);
                s.events.frame_ready(id, 0);
            }
            return;
        }
        HalCmd::WindowClose { id } => {
            let hosted = world.session(sid).and_then(|s| s.windows.get(sid, id).ok()).is_some_and(|w| w.val.is_hosted());
            let host = world.session(sid).and_then(|s| s.windows.get(sid, id).ok()).and_then(|w| w.val.alias_of);
            if let Some(host) = host {
                if let Some(s) = world.session_mut(HOST) { gpu.window_overlay(s, host, None, Id::NONE, now); }
            }
            let closed = world.release_window(sid, id).is_ok();
            let mut handled = false;
            if closed {
                if let Some(host) = host {
                    gpu.window_lent(sid, id, host, false);
                } else {
                    gpu.window_closed(sid, id);
                    handled = !hosted && gpu.os(sid, OsCmd::WindowClose { id });
                }
            }
            if let Some(s) = world.session_mut(sid) {
                if !closed {
                    s.stale();
                } else if !handled {
                    s.events.closed(id);
                }
            }
            return;
        }
        HalCmd::WindowGeom { id, geom } => {
            let Some(s) = world.session(sid) else { return };
            let Ok(w) = s.windows.get(sid, id) else {
                if let Some(s) = world.session_mut(sid) { s.stale(); }
                return;
            };
            if w.val.is_hosted() { return; }
            let (dpi, state) = (w.val.dpi.unwrap_or(s.dpi), w.val.state);
            if !gpu.os(sid, OsCmd::WindowGeom { id, geom }) {
                let geom = unplaced_at_origin(geom);
                world.window_geom(sid, id, geom, dpi, state);
                world.to_session(sid, |q| q.geom(id, geom[0] as f64, geom[1] as f64, geom[2] as f64, geom[3] as f64, dpi, state));
            }
            return;
        }
        HalCmd::WindowChrome { id, own } => {
            let Some(s) = world.session_mut(sid) else { return };
            let Ok(w) = s.windows.get_mut(sid, id) else { s.stale(); return };
            w.val.own_chrome = own;
            if w.val.embedded.is_some() {
                let parent = s.parent;
                world.to_session(parent, |q| q.child_chrome(id, sid, own));
            }
            return;
        }
        HalCmd::ParentMessage { message } => {
            let Some(s) = world.session(sid) else { return };
            if s.parent == 0 { deny(world, sid); return; }
            let parent = s.parent;
            world.to_session(parent, |q| q.child_message(sid, message));
            return;
        }
        HalCmd::ChildDragReply { window, request, answer } => {
            let Some(s) = world.session(sid) else { return };
            if s.windows.get(sid, window).is_ok_and(|w| w.val.embedded.is_some()) {
                let parent = s.parent;
                world.to_session(parent, |q| q.child_drag_answer(window, sid, request, answer));
            } else { deny(world, sid); }
            return;
        }
        HalCmd::WindowState { id, state } if world.session(sid)
            .is_some_and(|s| s.windows.get(sid, id).is_ok_and(|w| w.val.embedded.is_some())) => {
            let parent = world.session(sid).unwrap().parent;
            world.to_session(parent, |q| q.child_window_state(id, sid, state));
            return;
        }
        HalCmd::WindowTitle { id, title } => {
            let Some(s) = world.session_mut(sid) else { return };
            let embedded = match s.windows.get_mut(sid, id) {
                Ok(e) => {
                    e.val.title = title;
                    Some(e.val.embedded.is_some())
                }
                Err(_) => {
                    s.stale();
                    None
                }
            };
            if embedded == Some(true) {
                world.announce_embedded(sid, id);
            } else if embedded == Some(false) {
                let Some(s) = world.session(sid) else { return };
                if s.windows.get(sid, id).is_ok_and(|w| w.val.is_hosted()) {
                    return;
                }
                let text = s.string(title).unwrap_or("").to_string();
                gpu.os(sid, OsCmd::WindowTitle { id, title: text });
            }
            return;
        }
        HalCmd::WindowOverlay { id, on, rect, layer } => {
            if sid != HOST { deny(world, sid); return; }
            let Some(s) = world.session_mut(HOST) else { return };
            let layer_ok = layer.is_none() || s.textures.get(HOST, layer).is_ok();
            let Ok(w) = s.windows.get_mut(HOST, id) else { s.stale(); return };
            // a panel covers a borrower's frame; a layer may begin before the window is lent (the
            // borrower's first frame then goes to the app image, never straight to the window)
            let bad = if layer.is_none() {
                w.lent_to == 0 || !w.val.borrower_frame || !rect.iter().all(|v| v.is_finite()) || rect[2] <= 0.0 || rect[3] <= 0.0
            } else {
                !layer_ok || !w.val.adoptable
            };
            if on && bad {
                s.bad();
                return;
            }
            let next = on.then_some((rect, layer));
            if w.val.overlay.as_ref().map(|o| (o.rect, o.layer)) == next { return; }
            w.val.overlay = next.map(|(rect, layer)| WindowOverlay { rect, frame: None, layer });
            gpu.window_overlay(s, id, on.then_some(rect), layer, now);
            return;
        }
        HalCmd::Frame { target, mut kind, packet } => {
            let Some(s) = world.session_mut(sid) else { return };
            // Only the executor selects this target: clients cannot bypass lending.
            if kind & target::OVERLAY != 0 { s.bad(); return; }
            let overlay = sid == HOST && kind & !target::FLAGS == target::WINDOW
                && kind & target::PICK == 0
                && s.windows.get(sid, target).is_ok_and(|w| w.val.overlay.is_some());
            if overlay { kind |= target::OVERLAY; }
            // Adoption may race the owner's last draw. Once the borrower encodes
            // a window frame, only it may replace that image. Uploads remain valid.
            if sid == HOST && kind & !target::FLAGS == target::WINDOW
                && s.windows.get(sid, target).is_ok_and(|w| w.lent_to != 0 && w.val.borrower_frame && w.val.overlay.is_none()) {
                return;
            }
            let live = match kind & !target::FLAGS {
                target::WINDOW => s.windows.get(sid, target).is_ok(),
                target::TEXTURE => s.textures.get(sid, target).is_ok(),
                _ => false,
            };
            if !live {
                s.stale();
                return;
            }
            if kind & !target::FLAGS == target::WINDOW && s.windows.get(sid, target).is_ok_and(|w| w.val.is_hosted()) {
                // A HOSTED packet must name an imported, renderable image. Never fall back to
                // an OS drawable or acknowledge a frame whose shared target does not exist.
                let configured = s.windows.get(sid, target).is_ok_and(|w| w.val.hosted_dpi > 0.0);
                let passes = view.frames.get(packet as usize).and_then(|p| {
                    let start = p.passes.off as usize;
                    start.checked_add(p.passes.len as usize).and_then(|end| view.passes.get(start..end))
                });
                let valid = configured && passes.is_some_and(|passes| !passes.is_empty() && passes.iter().all(|pass| {
                    s.textures.get(sid, pass.color[0]).is_ok_and(|t| {
                        t.val.imported && t.val.usage & usage::RENDER_TARGET != 0
                            && pass.viewport.iter().all(|v| v.is_finite() && *v >= 0.0)
                            && pass.viewport[0] + pass.viewport[2] <= t.val.w as f32
                            && pass.viewport[1] + pass.viewport[3] <= t.val.h as f32
                    })
                }));
                if !valid {
                    s.bad();
                    return;
                }
            }
            // an embedded window's surface now holds this frame
            if kind & !target::FLAGS == target::WINDOW {
                let frame = view.frames.get(packet as usize).map_or(0, |p| p.frame);
                let tex = s.windows.get_mut(sid, target).ok().and_then(|w| {
                    w.val.embedded.as_mut().map(|em| {
                        em.frames += 1;
                        em.last_frame = frame;
                        em.tex
                    })
                });
                if let Some(tex) = tex {
                    s.ledger.touch(Kind::Texture, tex.index, tex.gen, frame, now);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            if crate::framecap::armed() {
                crate::framecap::on_frame(gpu, s, sid, target, kind, packet, view, now);
            }
            let Some(submitted) = gpu.frame(s, sid, target, kind, packet, view, now) else { return };
            let host = submitted.window.then(||
                s.windows.get(sid, target).ok().and_then(|w| w.val.alias_of)).flatten();
            let frame = s.frame;
            let trace = world.trace_exec;
            let mut layers = Vec::new();
            if submitted.window {
                let physical = host.unwrap_or(target);
                let owner = if host.is_some() { HOST } else { sid };
                if let Some(w) = world.session_mut(owner).and_then(|s| s.windows.get_mut(owner, physical).ok()) {
                    if overlay {
                        if let Some(o) = w.val.overlay.as_mut() { o.frame = Some(frame); }
                    } else {
                        w.val.base_frame = Some((sid, target, frame));
                    }
                    if trace {
                        if let Some(base) = w.val.base_frame { layers.push(base); }
                        if let Some(frame) = w.val.overlay.as_ref().and_then(|o| o.frame) {
                            layers.push((HOST, physical, frame));
                        }
                    }
                }
            }
            if let Some(id) = host {
                if let Some(w) = world.session_mut(HOST).and_then(|s| s.windows.get_mut(HOST, id).ok()) {
                    w.val.borrower_frame = true;
                }
            }
            if gpu.completes_inline() {
                if submitted.window && !overlay {
                    world.window_presented(sid, target, frame, 0, now);
                }
                world.frame_done(sid, target, kind, frame, 0, now, [0.0; 2]);
            }
            let mut seen = Vec::new();
            for l in submitted.deps {
                let Some(cs) = world.session_mut(l.child) else { continue };
                let last = cs.windows.get(l.child, l.window).ok().and_then(|w| w.val.embedded).map_or(0, |e| e.last_frame);
                cs.ledger.touch(Kind::Texture, l.tex.index, l.tex.gen, last, now);
                seen.push((l.child, l.window, last));
            }
            if world.trace_exec {
                let seq = world.exec_log.len() as u64;
                world.exec_log.push(ExecRecord { seq, session: sid, frame, target, deps: seen, layers });
            }
            return;
        }
        // ---- parent -> child
        HalCmd::ChildGeom { window, child, size, dpi } => {
            if !world.is_child(sid, child) {
                deny(world, sid);
            } else if !world.resize_embedded(child, window, size, dpi, now) {
                if let Some(s) = world.session_mut(sid) {
                    s.stale();
                }
            }
            return;
        }
        HalCmd::Forward { window, child, event } => {
            if !world.forward(sid, child, window, event) {
                deny(world, sid);
            }
            return;
        }
        HalCmd::ChildFocus { child } => {
            if child == 0 || world.is_child(sid, child) {
                world.focus_child(sid, child);
            } else {
                deny(world, sid);
            }
            return;
        }
        HalCmd::ChildClose { window, child } => {
            if !world.is_child(sid, child) {
                deny(world, sid);
            } else if world.session(child).is_some_and(|c| c.windows.get(child, window).is_ok_and(|w| w.val.embedded.is_some())) {
                world.to_session(child, |q| q.close_requested(window));
            } else if let Some(s) = world.session_mut(sid) {
                s.stale();
            }
            return;
        }
        // ---- the font service (the host's glyph atlases)
        HalCmd::FontOpen { .. } | HalCmd::GlyphRequest { .. } | HalCmd::GlyphRelease { .. } | HalCmd::GlyphTouch { .. } => {
            crate::font::command(gpu, world, sid, c);
            return;
        }
        // ---- domain control: a privileged session only
        HalCmd::DomainStart { .. } | HalCmd::DomainStop { .. } | HalCmd::DomainRelaunch { .. } | HalCmd::DomainList { .. } => {
            if !world.session(sid).is_some_and(|s| s.privileged) {
                deny(world, sid);
                return;
            }
            domain_control(world, sid, c);
            return;
        }
        _ => {}
    }
    let Some(s) = world.session_mut(sid) else { return };
    // an embedded session's keyboard, IME, cursor and clipboard requests act only while its parent
    // has granted it the focus
    if s.parent != 0
        && matches!(
            c,
            HalCmd::ImeShow { .. } | HalCmd::ImeHide { .. } | HalCmd::ImeSync { .. } | HalCmd::Cursor { .. } | HalCmd::PointerLock { .. } | HalCmd::ClipSet { .. } | HalCmd::ClipRead { .. } | HalCmd::SelectionSet { .. } | HalCmd::ClipboardActions { .. } | HalCmd::SelectionHandles { .. }
        )
    {
        if !s.focused {
            s.stats.input_dropped += 1;
            if let HalCmd::ClipRead { req, .. } = c {
                s.events.clipboard(req, false, &[]);
            }
            return;
        }
        s.stats.input_applied += 1;
    }
    match c {
        HalCmd::StrDef { sid: id, bytes } => {
            s.strings.insert(id, String::from_utf8_lossy(bytes).into_owned());
        }
        HalCmd::Quit {} => s.quit = true,
        HalCmd::TimerStart { timer, interval_ns, repeat } => {
            s.timers.insert(timer, TimerObj { interval_ns, repeat, due_ns: now + interval_ns });
        }
        HalCmd::TimerStop { timer } => {
            s.timers.remove(&timer);
        }
        HalCmd::WindowCreate { .. }
        | HalCmd::WindowClose { .. }
        | HalCmd::WindowGeom { .. }
        | HalCmd::WindowTitle { .. }
        | HalCmd::WindowChrome { .. }
        | HalCmd::ChildDragReply { .. }
        | HalCmd::ParentMessage { .. }
        | HalCmd::WindowOverlay { .. }
        | HalCmd::Frame { .. }
        | HalCmd::ChildGeom { .. }
        | HalCmd::Forward { .. }
        | HalCmd::ChildFocus { .. }
        | HalCmd::ChildClose { .. }
        | HalCmd::DomainStart { .. }
        | HalCmd::DomainStop { .. }
        | HalCmd::DomainRelaunch { .. }
        | HalCmd::DomainList { .. }
        | HalCmd::GpuBudget { .. }
        | HalCmd::FontOpen { .. }
        | HalCmd::GlyphRequest { .. }
        | HalCmd::GlyphRelease { .. }
        | HalCmd::GlyphTouch { .. } => unreachable!("handled above"),
        HalCmd::WindowHostedGeom { id, geom, dpi } => match s.windows.get_mut(sid, id) {
            Ok(e) if e.val.is_hosted() && dpi.is_finite() && dpi > 0.0
                && geom.iter().all(|v| v.is_finite()) && geom[2] >= 0.0 && geom[3] >= 0.0 => {
                e.val.geom = geom;
                e.val.hosted_dpi = dpi;
            }
            Ok(_) => s.bad(),
            Err(_) => s.stale(),
        },
        HalCmd::WindowState { id, state } => match s.windows.get_mut(sid, id) {
            Ok(e) => {
                if e.val.is_hosted() { return; }
                e.val.state = state;
                let g = e.val.geom;
                if !gpu.os(sid, OsCmd::WindowState { id, state }) {
                    let dpi = s.dpi;
                    s.events.geom(id, g[0] as f64, g[1] as f64, g[2] as f64, g[3] as f64, dpi, state);
                }
            }
            Err(_) => s.stale(),
        },
        // window chrome and OS interaction: the id must be live, then the window system acts
        HalCmd::WindowVisuals { id, visuals, params } => os_window(gpu, s, id, OsCmd::WindowVisuals { id, visuals, params }),
        HalCmd::WindowTopmost { id, on } => os_window(gpu, s, id, OsCmd::WindowTopmost { id, on }),
        HalCmd::WindowButtons { id, show } => os_window(gpu, s, id, OsCmd::WindowButtons { id, show }),
        HalCmd::WindowButtonsCenter { id, y_center } => {
            if y_center.is_finite() {
                os_window(gpu, s, id, OsCmd::WindowButtonsCenter { id, y_center });
            } else {
                s.bad();
            }
        }
        HalCmd::WindowDpiOverride { id, dpi } => {
            if dpi.is_finite() && dpi >= 0.0 {
                os_window(gpu, s, id, OsCmd::WindowDpiOverride { id, dpi_override: (dpi > 0.0).then_some(dpi) });
            } else {
                s.bad();
            }
        }
        HalCmd::ChromelessWhenMax { id, on } => os_window(gpu, s, id, OsCmd::ChromelessWhenMax { id, on }),
        HalCmd::Cursor { id, cursor } => os_window(gpu, s, id, OsCmd::Cursor { id, cursor }),
        HalCmd::PointerLock { id, mode } => os_window(gpu, s, id, OsCmd::PointerLock { id, mode }),
        HalCmd::ImeShow { id, area, config } => os_window(gpu, s, id, OsCmd::ImeShow { id, area, config }),
        HalCmd::ImeHide { id } => os_window(gpu, s, id, OsCmd::ImeHide { id }),
        HalCmd::ImeSync { id, sel_start, sel_end, comp_start, comp_end, text } => {
            let text = String::from_utf8_lossy(text).into_owned();
            os_window(gpu, s, id, OsCmd::ImeSync { id, selection: [sel_start, sel_end], composition: [comp_start, comp_end], text })
        }
        HalCmd::DragStart { id, count, items } => {
            let items: Vec<(u32, String)> = read_items::<DragItem>(items, count).map(|i| (i.kind, s.string(i.str).unwrap_or("").to_string())).collect();
            os_window(gpu, s, id, OsCmd::DragStart { id, items })
        }
        HalCmd::AppIcon { w, h, rgba } => {
            gpu.os(sid, OsCmd::AppIcon { w, h, rgba: rgba.to_vec() });
        }
        HalCmd::ClipboardActions { id, show, area, selection } => os_window(gpu, s, id, OsCmd::ClipboardActions { id, show, area, selection }),
        HalCmd::SelectionHandles { id, op, start, end } => os_window(gpu, s, id, OsCmd::SelectionHandles { id, op, start, end }),
        HalCmd::MobileUi { op, value } => {
            gpu.os(sid, OsCmd::MobileUi { op, value });
        }
        HalCmd::BackUnhandled {} => {
            gpu.os(sid, OsCmd::BackUnhandled);
        }
        HalCmd::ShowInDock { on } => {
            gpu.os(sid, OsCmd::ShowInDock { on });
        }
        HalCmd::ActivateApp {} => {
            gpu.os(sid, OsCmd::ActivateApp);
        }
        HalCmd::MenuSet { count, items } => {
            let items: Vec<(MenuItem, String)> = read_items::<MenuItem>(items, count).map(|i| (i, s.string(i.str).unwrap_or("").to_string())).collect();
            gpu.os(sid, OsCmd::MenuSet { items });
        }
        HalCmd::ShareText { text } => {
            let text = String::from_utf8_lossy(text).into_owned();
            gpu.os(sid, OsCmd::ShareText { text });
        }
        HalCmd::SelectionSet { text } => {
            let text = String::from_utf8_lossy(text).into_owned();
            gpu.os(sid, OsCmd::SelectionSet { text });
        }
        HalCmd::ClipSet { text } => {
            s.clipboard = String::from_utf8_lossy(text).into_owned();
            gpu.os(sid, OsCmd::ClipSet { text: s.clipboard.clone() });
        }
        HalCmd::ClipRead { req, primary } => {
            if !gpu.os(sid, OsCmd::ClipRead { req, primary }) {
                s.events.clipboard(req, false, &[]);
            }
        }
        HalCmd::DirectDisplaySource { req, name } => {
            direct_display(gpu, s, req, crate::direct::DisplayCommand::Source(String::from_utf8_lossy(name).into_owned()));
        }
        HalCmd::DirectDisplayOrder { req, count, names } => {
            let order = read_strs(names, count);
            if order.len() != count as usize {
                s.bad();
                s.events.direct_display_result(req, crate::cmds::cmd_tag::DirectDisplayOrder as u32, false, b"malformed display order");
            } else {
                direct_display(gpu, s, req, crate::direct::DisplayCommand::Order(order));
            }
        }
        HalCmd::DirectDisplayPacing { req, args } => {
            match crate::direct::DisplayPacing::from_args(&read_strs(args, 2)) {
                Some(pacing) => direct_display(gpu, s, req, crate::direct::DisplayCommand::Pacing(pacing)),
                None => {
                    s.bad();
                    s.events.direct_display_result(req, crate::cmds::cmd_tag::DirectDisplayPacing as u32, false, b"malformed display pacing");
                }
            }
        }
        HalCmd::DirectDisplayArrangement { req, name } => {
            use crate::direct::DisplayArrangement;
            let name = String::from_utf8_lossy(name).into_owned();
            let arrangement = if name.is_empty() { DisplayArrangement::Extended } else { DisplayArrangement::Mirror { optimise_for: name } };
            direct_display(gpu, s, req, crate::direct::DisplayCommand::Arrangement(arrangement));
        }
        HalCmd::DirectDisplayMode { req, args } => {
            let mut args = read_strs(args, 2).into_iter();
            if let (Some(name), Some(mode)) = (args.next(), args.next()) {
                direct_display(gpu, s, req, crate::direct::DisplayCommand::Mode { name, mode: (!mode.is_empty()).then_some(mode) });
            } else {
                s.bad();
                s.events.direct_display_result(req, crate::cmds::cmd_tag::DirectDisplayMode as u32, false, b"malformed display mode request");
            }
        }
        HalCmd::DirectPointerSpeed { touchpad, hundredths } => {
            #[cfg(target_os = "linux")]
            if gpu.os(sid, OsCmd::DirectPointerSpeed { touchpad, hundredths }) { return; }
            let _ = (touchpad, hundredths);
            s.bad();
        }
        HalCmd::DirectNaturalScroll { on } => {
            #[cfg(target_os = "linux")]
            if gpu.os(sid, OsCmd::DirectNaturalScroll { on }) { return; }
            let _ = on;
            s.bad();
        }
        HalCmd::DirectTapToClick { on } => {
            #[cfg(target_os = "linux")]
            if gpu.os(sid, OsCmd::DirectTapToClick { on }) { return; }
            let _ = on;
            s.bad();
        }
        HalCmd::FileDialog { req, kind, title, location, filename, count, filters } => {
            let filters = read_strs(filters, count);
            let title = s.string(title).unwrap_or("").to_string();
            let location = s.string(location).unwrap_or("").to_string();
            let filename = s.string(filename).unwrap_or("").to_string();
            // without a window system: answered at once, with no paths
            if !gpu.os(sid, OsCmd::FileDialog { req, kind, title, location, filename, filters }) {
                s.events.dialog_result(req, 0, &[]);
            }
        }
        HalCmd::FrameRequest { id } => match s.windows.get(sid, id) {
            Ok(e) if e.val.is_hosted() => {},
            // an embedded window has no OS window to pace it: Hal::pump answers at display rate
            Ok(e) if e.val.embedded.is_some() => {
                let now = world.now();
                world.request_embedded_frame(sid, id, now);
            }
            Ok(_) => {
                if !gpu.os(sid, OsCmd::FrameRequest { id }) {
                    s.events.frame_ready(id, 0);
                }
            }
            Err(_) => s.stale(),
        },

        HalCmd::TextureCreate { id, usage, format: fmt, w, h, depth_mips, attr } => {
            let [depth, mips] = depth_mips;
            if format::texel_bytes(fmt) == 0 || w == 0 || h == 0 || id.index >= LENT_BASE {
                s.bad();
                return;
            }
            let bytes = texture_bytes(fmt, w, h, depth as u32, mips as u32);
            let obj = TextureObj { format: fmt, w, h, depth: (depth as u32).max(1), mips: (mips as u32).max(1), usage, bytes, imported: false, uploaded: 0, lent: None };
            insert_texture(gpu, s, id, obj, attr, now);
        }
        // the HAL's own ids (embedded window surfaces and their aliases) are not Cx's to touch
        HalCmd::TextureUpdate { id, .. } | HalCmd::TextureDestroy { id } if id.index >= LENT_BASE => s.bad(),
        HalCmd::TextureUpdate { id, mip, rect, layer, bytes } => {
            // SAFETY: the Slice is the caller's memory, borrowed for this hal_submit call
            let data = unsafe { bytes.as_bytes() };
            match s.textures.get_mut(sid, id) {
                Ok(e) => {
                    let t = e.val.clone();
                    let [x, y, rw, rh] = rect;
                    let level = mip as u32;
                    let (mw, mh) = ((t.w >> level.min(t.mips - 1)).max(1), (t.h >> level.min(t.mips - 1)).max(1));
                    let fits = level < t.mips && layer < t.depth && x.checked_add(rw).is_some_and(|v| v <= mw) && y.checked_add(rh).is_some_and(|v| v <= mh);
                    if !fits || data.len() as u64 != format::level_bytes(t.format, rw, rh) {
                        s.stats.bad_args += 1;
                    } else {
                        e.val.uploaded += data.len() as u64;
                        gpu.texture_update(s, id, &t, mip, rect, layer, data);
                    }
                }
                Err(_) => s.stale(),
            }
        }
        HalCmd::TextureDestroy { id } => match s.textures.remove(sid, id) {
            Ok(_) => {
                s.ledger.remove(Kind::Texture, id.index);
                gpu.texture_destroy(sid, id);
            }
            Err(_) => s.stale(),
        },
        HalCmd::TextureReadback { id, req, rect } => {
            if s.textures.get(sid, id).is_ok() {
                gpu.texture_readback(s, id, req, rect);
            } else {
                s.stale();
                s.events.readback(req, readback::STALE, rect, &[]);
            }
        }
        HalCmd::TextureShare { id } => match s.textures.get(sid, id) {
            Ok(_) => gpu.texture_share(s, id),
            Err(_) => s.stale(),
        },
        HalCmd::TextureImport { id, handle, kind } => {
            let ok = if id.index >= LENT_BASE {
                s.bad();
                false
            } else if !s.textures.can_insert(sid, id) {
                s.stale();
                false
            } else if let Some(obj) = gpu.texture_import(s, id, handle, kind) {
                insert_texture(gpu, s, id, obj, Attr::default(), now);
                true
            } else {
                s.bad();
                false
            };
            s.events.texture_imported(id, ok);
        }
        HalCmd::TextureAdoptGl { id, name, format: fmt, w, h, owned } => {
            let valid = id.index < LENT_BASE && name != 0 && w > 0 && h > 0 && matches!(fmt, format::RGBA8 | format::R8 | format::RG8);
            let ok = if !valid {
                s.bad();
                false
            } else if !s.textures.can_insert(sid, id) {
                s.stale();
                false
            } else if let Some(obj) = gpu.texture_adopt_gl(s, id, name, fmt, w, h, owned) {
                insert_texture(gpu, s, id, obj, Attr::default(), now);
                true
            } else {
                false
            };
            s.events.texture_imported(id, ok);
        }
        HalCmd::GlFence { sync } => gpu.gl_fence(s, sync),
        HalCmd::BufferCreate { id, kind, size, attr } => {
            let obj = BufferObj { kind, size, bytes: size, evictable: false, uploaded: 0 };
            let replaced = match s.buffers.insert(sid, id, sid, obj.clone()) {
                Ok(old) => old.is_some(),
                Err(_) => {
                    s.stale();
                    return;
                }
            };
            if replaced {
                s.ledger.remove(Kind::Buffer, id.index);
            }
            let mut obj = obj;
            if gpu.buffer_create(s, id, &mut obj).is_err() {
                let _ = s.buffers.remove(sid, id);
                gpu.buffer_destroy(sid, id);
                s.bad();
                return;
            }
            if let Ok(e) = s.buffers.get_mut(sid, id) {
                e.val = obj.clone();
            }
            s.ledger.add(LedgerEntry {
                kind: Kind::Buffer,
                detail: buffer_detail(kind),
                index: id.index,
                gen: id.gen,
                bytes: obj.bytes,
                format: 0,
                width: 0,
                height: 0,
                mips: 0,
                attr,
                created_ns: now,
                created_frame: s.frame,
                last_used_frame: 0,
                last_used_ns: 0,
            });
        }
        HalCmd::BufferUpdate { id, off, bytes } => {
            // SAFETY: as TextureUpdate
            let data = unsafe { bytes.as_bytes() };
            match s.buffers.get_mut(sid, id) {
                Ok(e) => {
                    if off.checked_add(data.len() as u64).is_some_and(|end| end <= e.val.size) {
                        e.val.uploaded += data.len() as u64;
                        let mut b = e.val.clone();
                        gpu.buffer_update(s, id, &mut b, off, data);
                        // the device may have replaced the storage: its size is the ledger's
                        if let Ok(e) = s.buffers.get_mut(sid, id) {
                            e.val = b.clone();
                        }
                        if let Some(l) = s.ledger.get_mut(Kind::Buffer, id.index) {
                            if l.gen == id.gen {
                                l.bytes = b.bytes;
                            }
                        }
                    } else {
                        s.stats.bad_args += 1;
                    }
                }
                Err(_) => s.stale(),
            }
        }
        HalCmd::BufferDestroy { id } => match s.buffers.remove(sid, id) {
            Ok(_) => {
                s.ledger.remove(Kind::Buffer, id.index);
                gpu.buffer_destroy(sid, id);
            }
            Err(_) => s.stale(),
        },
        HalCmd::BufferEvictable { id } => match s.buffers.get_mut(sid, id) {
            Ok(e) => {
                e.val.evictable = true;
                gpu.buffer_evictable(sid, id);
            }
            Err(_) => s.stale(),
        },
        HalCmd::PipelineCreate { id, dialect, attr, desc } => match PipelineBlob::parse(desc) {
            Some(b) => {
                let obj = PipelineObj { dialect, desc: b.desc, blob: desc.into() };
                match s.pipelines.insert(sid, id, sid, obj) {
                    Ok(old) => {
                        if old.is_some() {
                            s.ledger.remove(Kind::Pipeline, id.index);
                            if gpu.pipeline_destroy(sid, id) {
                                s.pipelines_pending = s.pipelines_pending.saturating_sub(1);
                            }
                        }
                        let mut attr = attr;
                        if attr.label == 0 {
                            attr.label = b.desc.name;
                        }
                        if attr.site == 0 {
                            attr.site = b.desc.source;
                        }
                        s.ledger.add(LedgerEntry {
                            kind: Kind::Pipeline,
                            detail: "pipeline",
                            index: id.index,
                            gen: id.gen,
                            bytes: 0,
                            format: 0,
                            width: 0,
                            height: 0,
                            mips: 0,
                            attr,
                            created_ns: now,
                            created_frame: s.frame,
                            last_used_frame: 0,
                            last_used_ns: 0,
                        });
                        match gpu.pipeline_create(s, id, dialect, &b, attr) {
                            PipeStatus::Ready => s.events.pipeline_ready(id),
                            PipeStatus::Failed(log) => s.events.pipeline_failed(id, log.as_bytes()),
                            PipeStatus::Pending => s.pipelines_pending += 1,
                        }
                    }
                    Err(_) => s.stale(),
                }
            }
            None => {
                s.bad();
                s.events.pipeline_failed(id, b"malformed pipeline blob");
            }
        },
        HalCmd::PipelineDestroy { id } => match s.pipelines.remove(sid, id) {
            Ok(_) => {
                s.ledger.remove(Kind::Pipeline, id.index);
                if gpu.pipeline_destroy(sid, id) {
                    s.pipelines_pending = s.pipelines_pending.saturating_sub(1);
                }
            }
            Err(_) => s.stale(),
        },

        HalCmd::Pick { window, req, pos } => {
            if s.windows.get(sid, window).is_err() {
                s.stale();
            }
            gpu.pick(s, window, req, pos);
        }
        HalCmd::Snapshot { window, req } => match s.windows.get(sid, window) {
            Ok(_) => gpu.snapshot(s, window, req),
            Err(_) => {
                s.stale();
                s.events.readback(req, readback::STALE, [0; 4], &[]);
            }
        },
        HalCmd::WindowCopy { window, texture, req } => {
            if s.windows.get(sid, window).is_ok() && s.textures.get(sid, texture).is_ok() {
                gpu.window_copy(s, window, texture, req);
            } else {
                s.events.readback(req, readback::STALE, [0; 4], &[]);
            }
        }
        HalCmd::QueryReply { req, bytes } => s.replies.push((req, bytes.to_vec())),
        HalCmd::TraceMark { text } => {
            let t = s.string(text).unwrap_or("").to_string();
            s.marks.push((now, t));
        }
        HalCmd::Budget { what, value } => s.budgets.push((what, value)),
        HalCmd::DragReply { window, to, answer } => os_window(gpu, s, window, OsCmd::DragReply { id: window, to, answer }),

        // ---- media
        HalCmd::VideoPrepare { .. } | HalCmd::VideoControl { .. } | HalCmd::AudioUse { .. } | HalCmd::AudioEntry { .. } | HalCmd::Midi { .. } | HalCmd::GameOut { .. } | HalCmd::Location { .. } | HalCmd::Permission { .. } | HalCmd::ViewOp { .. } | HalCmd::VideoInput { .. } => {
            if !gpu.media(s, &c) {
                media_unavailable(s, &c);
            }
        }
        // a backend without haptics has nothing to do
        HalCmd::Haptic { .. } => { gpu.media(s, &c); }
    }
}

fn direct_display(_gpu: &mut dyn Gpu, s: &mut Session, req: u64, command: crate::direct::DisplayCommand) {
    let tag = command.tag() as u32;
    #[cfg(target_os = "linux")]
    if _gpu.os(s.id, OsCmd::DirectDisplay { req, command }) { return; }
    s.events.direct_display_result(req, tag, false, b"this backend does not own direct displays");
}

/// What a command of the media section does on a backend without media: the ones that expect an
/// answer get their failure event, the rest are counted.
fn media_unavailable(s: &mut Session, c: &HalCmd) {
    match c {
        HalCmd::VideoPrepare { video, .. } => s.events.video_status(*video, crate::media::video_status::ERROR, b"this backend has no media playback"),
        _ => {}
    }
    s.stats.media_unsupported += 1;
}

/// A command for a window of the session: counted as stale when the window is gone.
fn os_window(gpu: &mut dyn Gpu, s: &mut Session, id: Id, cmd: OsCmd) {
    match s.windows.get(s.id, id) {
        Ok(e) if !e.val.is_hosted() => { gpu.os(s.id, cmd); }
        Ok(_) => {},
        Err(_) => s.stale(),
    }
}

pub(crate) fn insert_texture(gpu: &mut dyn Gpu, s: &mut Session, id: Id, obj: TextureObj, attr: Attr, now: u64) {
    let imported = obj.imported;
    // the table first: a create with a stale generation touches nothing, a newer one replaces
    // the older object under the same slot (the device create replaces its native object too)
    let replaced = match s.textures.insert(s.id, id, s.id, obj.clone()) {
        Ok(old) => old.is_some(),
        Err(_) => {
            s.stale();
            return;
        }
    };
    if replaced {
        s.ledger.remove(Kind::Texture, id.index);
    }
    let mut obj = obj;
    if !imported {
        if let Err(error) = gpu.texture_create(s, id, &mut obj) {
            let _ = s.textures.remove(s.id, id);
            gpu.texture_destroy(s.id, id);
            s.bad();
            // The device had no room (a full VRAM next to another process): the app is told the
            // object is gone, as for an evicted target, so it creates it again when it next needs
            // it instead of sampling and rendering into an id that never got storage. A backend
            // that asks for room first (`gpu_pressure`) tells it once the room is there.
            if !gpu.texture_starved(s.id, id, &obj, &error) {
                s.events.texture_evicted(id);
            }
            return;
        }
        if let Ok(e) = s.textures.get_mut(s.id, id) {
            e.val = obj.clone();
        }
    }
    s.ledger.add(LedgerEntry {
        kind: Kind::Texture,
        detail: texture_detail(obj.format, obj.usage),
        index: id.index,
        gen: id.gen,
        bytes: obj.bytes,
        format: obj.format,
        width: obj.w,
        height: obj.h,
        mips: obj.mips,
        attr,
        created_ns: now,
        created_frame: s.frame,
        last_used_frame: 0,
        last_used_ns: 0,
    });
}

fn deny(world: &mut World, sid: u32) {
    if let Some(s) = world.session_mut(sid) {
        s.stats.denied += 1;
    }
}

/// The four domain commands of a privileged session.
fn domain_control(world: &mut World, sid: u32, c: HalCmd) {
    match c {
        HalCmd::DomainStart { req, app, args } => {
            let text = |world: &World, id: StrId| world.session(sid).and_then(|s| s.string(id)).map(str::to_string);
            let (Some(app), args) = (text(world, app), text(world, args).unwrap_or_default()) else {
                if let Some(s) = world.session_mut(sid) {
                    s.bad();
                }
                return;
            };
            let launch = world.start_domain(sid, req, app, args);
            world.push_host_call(HostCall::Start(launch));
        }
        HalCmd::DomainStop { domain, mode } => {
            if !world.domain(domain).is_some_and(|r| r.parent == sid) {
                deny(world, sid);
                return;
            }
            if mode == domain_stop::KILL {
                world.schedule_close(domain, CloseAs::Exit("killed".into()));
            } else {
                world.to_session(domain, |q| q.lifecycle(lifecycle::QUIT_REQUESTED));
            }
            world.push_host_call(HostCall::Stop { domain, mode });
        }
        HalCmd::DomainRelaunch { domain } => {
            if !world.domain(domain).is_some_and(|r| r.parent == sid) {
                deny(world, sid);
                return;
            }
            // the old app is asked to quit and the host waits for its run to end (`DomainHost::stop`)
            // before the session is freed and the app started again (`Hal::run_host_calls`)
            world.to_session(domain, |q| q.lifecycle(lifecycle::QUIT_REQUESTED));
            world.push_host_call(HostCall::Stop { domain, mode: domain_stop::QUIT });
            if let Some(launch) = world.launch_of(domain) {
                world.push_host_call(HostCall::Relaunch(launch));
            }
        }
        HalCmd::DomainList { req } => {
            let list = world.domains_of(sid);
            world.to_session(sid, |q| q.domain_listed(req, list.len() as u32, &pack_domains(&list)));
        }
        _ => {}
    }
}
