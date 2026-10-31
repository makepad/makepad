//! Session-side segment storage. View creation, geometry and presentation are main-thread work.
use super::super::{media::layers, shell};
use crate::{buffers::CmdView, native_layer::{mode, state}, types::*};
use makepad_apple_sys::*;
use std::{collections::{HashMap, HashSet}, sync::{Arc, Mutex, OnceLock}, time::{Duration, Instant}};

const IDLE: Duration = Duration::from_secs(3);
const BUDGET: usize = 4;

struct Surface {
    layer: RcObjcId,
    hidden: Option<Instant>,
    depth: Option<super::resources::Tex>,
    depth_size: [u32; 2],
}

#[derive(Default)]
pub(super) struct Windows {
    windows: HashMap<(u32, Id), Window>,
    rejected: HashSet<(u32, Id)>,
}

struct Window {
    surfaces: HashMap<usize, Surface>,
    fallback_logged: bool,
    active: Arc<std::sync::atomic::AtomicBool>,
    alive: Arc<std::sync::atomic::AtomicBool>,
    parent: Option<(u32, Id)>,
}
impl Default for Window {
    fn default() -> Self {
        Self { surfaces: HashMap::new(), fallback_logged: false, active: Arc::new(false.into()), alive: Arc::new(true.into()), parent: None }
    }
}

pub(super) struct Segment {
    pub draws: Range,
    pub rect: [f32; 4],
    pub drawable: RcObjcId,
    pub texture: ObjcId,
    pub depth: Option<ObjcId>,
}

pub(super) struct Prepared {
    pub segments: Vec<Segment>,
    pub frame: layers::Frame,
}

impl Windows {
    pub fn contains(&self, sid: u32, window: Id) -> bool { self.windows.get(&(sid, window)).is_some_and(|w| w.active.load(std::sync::atomic::Ordering::Acquire)) }

    /// A badge/composition frame belongs to the physical host, while its native views
    /// may belong to a borrower. Preserve their last committed geometry and hide upper surfaces.
    pub fn composition(&mut self, parent: (u32, Id), dpi: f32) -> Option<Prepared> {
        let (&(sid, window), state) = self.windows.iter_mut().find(|(_, w)| w.parent == Some(parent) && w.active.load(std::sync::atomic::Ordering::Acquire))?;
        if !state.fallback_logged {
            eprintln!("[metal] native window {}:{} uses seg0 only while composited by its host; captures contain Makepad pixels only", sid, window.index);
            state.fallback_logged = true;
        }
        for surface in state.surfaces.values_mut() { surface.hidden.get_or_insert_with(Instant::now); }
        let mut frame = layers::Frame::new(sid, window, dpi);
        frame.parent = parent;
        frame.active = true;
        frame.preserve_hosts = true;
        frame.alive = Some(state.alive.clone());
        frame.active_state = Some(state.active.clone());
        Some(Prepared { segments: Vec::new(), frame })
    }

    pub fn destroyed(&mut self, sid: u32, id: Id) { self.rejected.remove(&(sid, id)); }

    pub fn close(&mut self, sid: u32, window: Option<Id>) {
        self.windows.retain(|&(owner, id), state| {
            let keep = (owner != sid || window.is_some_and(|w| w != id)) && !window.is_some_and(|w| state.parent == Some((sid, w)));
            if !keep { state.alive.store(false, std::sync::atomic::Ordering::Release); }
            keep
        });
        if window.is_none() { self.rejected.retain(|&(owner, _)| owner != sid); }
        cancel(sid, window);
    }

    /// Only the latest unsent frame owns drawables. Release it before asking Metal for more.
    pub fn discard_pending(&self, sid: u32, window: Id) {
        let old = shell::lock(pending()).frames.remove(&(sid, window));
        if let Some(old) = &old { layers::trace_replaced(old); }
        drop(old);
    }

    pub fn expire(&mut self) {
        for window in self.windows.values_mut() {
            window.surfaces.retain(|_, surface| surface.hidden.is_none_or(|t| t.elapsed() < IDLE));
        }
    }

    pub fn next_due(&self, sid: u32, now: u64) -> Option<u64> {
        self.windows.iter().filter(|(key, _)| key.0 == sid).flat_map(|(_, w)| w.surfaces.values())
            .filter_map(|s| s.hidden.map(|t| now + IDLE.saturating_sub(t.elapsed()).as_nanos() as u64)).min()
    }

    pub fn prepare(&mut self, s: &mut crate::session::Session, window: Id, parent: (u32, Id), device: ObjcId,
        pass: &Pass, records: &[Layer], view: &CmdView, mut frame: layers::Frame, fallback: bool) -> Option<Prepared>
    {
        let sid = s.id;
        let state = self.windows.entry((sid, window)).or_default();
        if fallback && !records.is_empty() && !state.fallback_logged {
            eprintln!("[metal] native window {}:{} uses seg0 only while overlaid, badged, lent, captured or offscreen; captures contain Makepad pixels only", sid, window.index);
            state.fallback_logged = true;
        }
        state.parent = Some(parent);
        frame.parent = parent;
        if !records.is_empty() { state.active.store(true, std::sync::atomic::Ordering::Release); }
        frame.active_state = Some(state.active.clone());
        frame.alive = Some(state.alive.clone());
        let mut count = 0;
        let mut ranges: Vec<Range> = vec![Range { off: pass.draws.off, len: 0 }];
        for g in records {
            if g.kind == Layer::NATIVE {
                if count == BUDGET {
                    if self.rejected.insert((sid, g.native)) {
                        eprintln!("[metal] native window {}:{} layer {} exceeds the four-layer compositor budget; unsupported placeholder", sid, window.index, g.native.index);
                        s.events.native_layer_state(g.native, state::REALIZED, mode::UNSUPPORTED, 0, b"");
                    }
                    frame.placements.retain(|p| p.geometry.native != g.native);
                    continue;
                }
                count += 1;
                frame.order.push(layers::Item::Native(g.native));
                ranges.push(Range { off: g.draws.off, len: 0 });
            } else {
                let r = &mut ranges[count];
                if r.len == 0 { r.off = g.draws.off; }
                r.len += g.draws.len;
                // The actual surface is inserted after the native record at this slot.
                if !fallback && count != 0 && g.draws.len != 0 {
                    if !frame.order.iter().any(|item| matches!(item, layers::Item::Segment(n) if *n == count)) {
                        frame.order.push(layers::Item::Segment(count));
                    }
                }
            }
        }
        let mut segments = Vec::new();
        if !fallback {
            let full = [0.0, 0.0, pass.viewport[2], pass.viewport[3]];
            for (gap, draws) in ranges.into_iter().enumerate().skip(1) {
                let Some(rect) = bounds(draws, view, full) else { continue };
                let surface = state.surfaces.entry(gap).or_insert_with(|| {
                    // This worker has no run loop to commit implicit Core Animation transactions.
                    // Publish the layer's configuration before acquiring its first drawable;
                    // the main thread separately commits placement and presentation together.
                    let layer = unsafe {
                        let () = msg_send![class!(CATransaction), begin];
                        let () = msg_send![class!(CATransaction), setDisableActions: YES];
                        let layer: ObjcId = msg_send![class!(CAMetalLayer), new];
                        let () = msg_send![layer, setDevice: device];
                        let () = msg_send![layer, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
                        let () = msg_send![layer, setOpaque: NO];
                        let () = msg_send![layer, setFramebufferOnly: YES];
                        let () = msg_send![layer, setMaximumDrawableCount: 3u64];
                        let () = msg_send![layer, setAllowsNextDrawableTimeout: YES];
                        let () = msg_send![layer, setPresentsWithTransaction: YES];
                        let () = msg_send![layer, setContentsScale: frame.dpi as f64];
                        let () = msg_send![layer, setDrawableSize: CGSize { width: rect[2] as f64, height: rect[3] as f64 }];
                        let () = msg_send![class!(CATransaction), commit];
                        layer
                    };
                    Surface { layer: RcObjcId::from_owned(NonNull::new(layer).expect("CAMetalLayer")), hidden: None, depth: None, depth_size: [0; 2] }
                });
                surface.hidden = None;
                if !pass.depth.is_none() && surface.depth_size != [rect[2] as u32, rect[3] as u32] {
                    let source = s.textures.get(sid, pass.depth).ok()?.val.clone();
                    let obj = crate::session::TextureObj { w: rect[2] as u32, h: rect[3] as u32, ..source };
                    surface.depth = Some(super::resources::create_texture(device, &obj, pass.depth.gen, "native segment depth").ok()?);
                    surface.depth_size = [obj.w, obj.h];
                }
                let drawable = unsafe {
                    let layer = surface.layer.as_id();
                    let scale: f64 = msg_send![layer, contentsScale];
                    let size: CGSize = msg_send![layer, drawableSize];
                    if scale != frame.dpi as f64 || size.width != rect[2] as f64 || size.height != rect[3] as f64 {
                        let () = msg_send![class!(CATransaction), begin];
                        let () = msg_send![class!(CATransaction), setDisableActions: YES];
                        let () = msg_send![layer, setContentsScale: frame.dpi as f64];
                        let () = msg_send![layer, setDrawableSize: CGSize { width: rect[2] as f64, height: rect[3] as f64 }];
                        let () = msg_send![class!(CATransaction), commit];
                    }
                    let drawable: ObjcId = msg_send![layer, nextDrawable];
                    RcObjcId::from_unowned(NonNull::new(drawable)?)
                };
                let texture: ObjcId = unsafe { msg_send![drawable.as_id(), texture] };
                frame.surfaces.push(layers::Surface { gap, layer: surface.layer.clone(), rect });
                segments.push(Segment { draws, rect, drawable, texture, depth: surface.depth.as_ref().map(|d| d.obj.as_id()) });
            }
        }
        for (gap, surface) in &mut state.surfaces {
            if !frame.surfaces.iter().any(|s| s.gap == *gap) && surface.hidden.is_none() {
                surface.hidden = Some(Instant::now());
            }
        }
        // A frame with no records retires the synchronized path after its clearing transaction.
        frame.active = count != 0;
        Some(Prepared { segments, frame })
    }
}

fn bounds(range: Range, view: &CmdView, full: [f32; 4]) -> Option<[f32; 4]> {
    let draws = view.draws.get(range.off as usize..(range.off + range.len) as usize)?;
    let mut b = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for d in draws {
        let r = d.bounds;
        if r.iter().any(|v| !v.is_finite()) { return Some(full); }
        if r[2] <= 0.0 || r[3] <= 0.0 { continue; }
        b[0] = b[0].min(r[0]); b[1] = b[1].min(r[1]);
        b[2] = b[2].max(r[0] + r[2]); b[3] = b[3].max(r[1] + r[3]);
    }
    let x = b[0].floor().max(0.0); let y = b[1].floor().max(0.0);
    let w = b[2].ceil().min(full[2]) - x; let h = b[3].ceil().min(full[3]) - y;
    (w > 0.0 && h > 0.0).then_some([x, y, w, h])
}

#[derive(Default)]
struct Pending {
    frames: HashMap<(u32, Id), layers::Frame>,
    queued: bool,
}
fn pending() -> &'static Mutex<Pending> {
    static PENDING: OnceLock<Mutex<Pending>> = OnceLock::new();
    PENDING.get_or_init(Default::default)
}
fn cancel(sid: u32, window: Option<Id>) {
    let removed = {
        let mut p = shell::lock(pending());
        let keys: Vec<_> = p.frames.keys().copied().filter(|&(owner, id)| owner == sid && window.is_none_or(|w| w == id)).collect();
        keys.into_iter().filter_map(|key| p.frames.remove(&key)).collect::<Vec<_>>()
    };
    drop(removed);
}

/// Called only after commit + waitUntilScheduled, on the existing Metal submission worker.
/// One queued closure drains all windows. Replacements never wait for the main thread.
pub(super) fn scheduled(frames: Vec<layers::Frame>, scheduled: Option<Instant>) {
    let (queue, replaced) = {
        let mut p = shell::lock(pending());
        let mut replaced = Vec::new();
        for mut frame in frames {
            frame.scheduled = scheduled;
            let key = (frame.sid, frame.window);
            if let Some(mut old) = p.frames.remove(&key) {
                if frame.preserve_hosts {
                    // Host pixels can supersede an app's queued frame, but not the placement
                    // that those pixels were composed from. Transfer that placement atomically.
                    std::mem::swap(&mut frame.placements, &mut old.placements);
                    std::mem::swap(&mut frame.order, &mut old.order);
                    frame.preserve_hosts = old.preserve_hosts;
                    frame.active = old.active;
                }
                layers::trace_replaced(&old);
                replaced.push(old);
            }
            p.frames.insert(key, frame);
        }
        let queue = !p.queued;
        p.queued = true;
        (queue, replaced)
    };
    drop(replaced);
    if queue && !shell::on_main(|| {
        let frames = {
            let mut p = shell::lock(pending());
            p.queued = false;
            std::mem::take(&mut p.frames)
        };
        unsafe {
            let () = msg_send![class!(CATransaction), begin];
            let () = msg_send![class!(CATransaction), setDisableActions: YES];
        }
        let presented: Vec<_> = frames.into_values().filter_map(layers::frame).collect();
        unsafe { let () = msg_send![class!(CATransaction), commit]; }
        let committed = layers::trace_enabled().then(Instant::now);
        for frame in presented {
            layers::trace_commit(frame.trace, committed);
            if let Some(state) = frame.active_state { state.store(frame.active, std::sync::atomic::Ordering::Release); }
            drop(frame.drawables);
        }
    }) {
        let frames = {
            let mut p = shell::lock(pending());
            p.queued = false;
            std::mem::take(&mut p.frames)
        };
        drop(frames);
    }
}
