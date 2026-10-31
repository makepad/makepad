//! DirectComposition: one device per process (made without a rendering device, so a D3D device
//! reset keeps every visual; made again after its own loss), a target per HWND and a visual tree
//! per window: `root -> seg0 -> native A -> seg1 -> native B -> ...`. Until a window has native
//! layers seg0 shows its swapchain and nothing is committed per frame. From its first native
//! layer on, every segment is a composition surface: seg0 at the window's size, an upper segment
//! premultiplied at its draws' bounds (left out of the tree while empty). Surface updates belong
//! to the device's commit, so a frame's pixels and its layers' geometry reach the screen in one
//! DWM frame (measured: a swapchain present and a commit do not; see the M4b report). The window
//! stays on surfaces, so it never shows a stale swapchain buffer on the way back. A native layer is two visuals:
//! `outer` carries the clip rectangle, the transform and the opacity, `inner` is offset to the
//! layer's rect, clips to its rounded corners and is the system web view's root visual target.

use super::device::{hr, Device};
use super::resources::Texture;
use crate::backend::windows::win32::{Key, LayerRegion, Shared};
use crate::native_layer::layer_flags;
use crate::{Id, Layer};
use makepad_windows_sys::composition::Win32::Graphics::Direct2D::Common::*;
use makepad_windows_sys::composition::Win32::Graphics::DirectComposition::*;
use makepad_windows_sys::Win32::Foundation::{HWND, POINT};
use makepad_windows_sys::Win32::Graphics::Direct3D11::{ID3D11Texture2D, D3D11_BOX};
use makepad_windows_sys::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM};
use makepad_windows_sys::Win32::Graphics::Dxgi::IDXGISwapChain1;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// How long an empty upper segment keeps its surface.
const IDLE: Duration = Duration::from_secs(3);

/// A DirectComposition object handed between threads. DirectComposition objects are
/// free-threaded: the HAL's submit path builds and commits the trees under the HAL lock while
/// the shell thread's web view attaches its own visuals under a layer's `inner` visual (its root
/// visual target) on the same device; that attach shows with the window's next commit, which the
/// REALIZED event's redraw brings.
pub(crate) struct Agile<T>(pub T);
// SAFETY: DirectComposition objects are not bound to an apartment (see above)
unsafe impl<T> Send for Agile<T> {}
unsafe impl<T> Sync for Agile<T> {}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn current() -> &'static Mutex<Option<Agile<IDCompositionDesktopDevice>>> {
    static DEVICE: OnceLock<Mutex<Option<Agile<IDCompositionDesktopDevice>>>> = OnceLock::new();
    DEVICE.get_or_init(Default::default)
}

/// The process's DirectComposition device (made on first use, and again after a loss).
pub(crate) fn device() -> Result<IDCompositionDesktopDevice, String> {
    let mut d = lock(current());
    if let Some(dev) = d.as_ref() { return Ok(dev.0.clone()); }
    let dev = unsafe {
        let raw = DCompositionCreateDevice2(None, &IDCompositionDesktopDevice::IID).map_err(|e| hr("DCompositionCreateDevice2", e))?;
        IDCompositionDesktopDevice::from_raw(raw).map_err(|e| hr("DCompositionCreateDevice2", e))?
    };
    *d = Some(Agile(dev.clone()));
    Ok(dev)
}

/// A commit failed or a window was told to repaint (DirectComposition sends WM_PAINT to the
/// windows of a lost device): the HAL checks the device on its next turn.
static CHECK: AtomicBool = AtomicBool::new(false);
static LOST: AtomicBool = AtomicBool::new(false);

pub(crate) fn request_check() { CHECK.store(true, Ordering::Release); }

/// A simulated device loss (`MAKEPAD_HAL_SIMULATE_DEVICE_LOSS_MS`) takes the composition device
/// too, so the whole recovery is exercised.
pub(crate) fn simulate_loss() {
    if !LOST.swap(true, Ordering::AcqRel) {
        eprintln!("[hal-dcomp] simulated device loss; every window's composition is made again");
    }
}

/// Whether the device is lost. A pending check asks the device (`IDCompositionDevice::
/// CheckDeviceState`, when the desktop device answers for that interface) and commits.
pub(crate) fn lost() -> bool {
    if CHECK.swap(false, Ordering::AcqRel) {
        let dev = lock(current()).as_ref().map(|d| d.0.clone());
        if let Some(dev) = dev {
            static CHECKABLE: OnceLock<bool> = OnceLock::new();
            let state = unsafe { IDCompositionDevice::query(dev.as_raw()) }.ok();
            let checkable = *CHECKABLE.get_or_init(|| {
                eprintln!("[hal-dcomp] device state check {}", if state.is_some() { "available" } else { "unavailable: a failed commit detects a loss" });
                state.is_some()
            });
            match state.filter(|_| checkable).map(|d| unsafe { d.CheckDeviceState() }) {
                Some(Ok(valid)) if !valid.as_bool() => mark_lost(makepad_windows_sys::core::HRESULT(0x887a0005u32 as i32)),
                Some(Err(e)) => mark_lost(e),
                _ => {}
            }
            if let Err(e) = unsafe { dev.Commit() } { mark_lost(e); }
        }
    }
    LOST.load(Ordering::Acquire)
}

fn mark_lost(e: makepad_windows_sys::core::HRESULT) {
    if !LOST.swap(true, Ordering::AcqRel) {
        eprintln!("[hal-dcomp] device lost ({e}); every window's composition is made again");
    }
}

/// After a loss, once every tree is gone: a new device, and new visuals for every native layer
/// (their content must render into them again: the caller hands them to the shell).
pub(crate) fn replace_device() -> Vec<(Key, Agile<IDCompositionVisual2>)> {
    *lock(current()) = None;
    LOST.store(false, Ordering::Release);
    let keys: Vec<Key> = lock(natives()).keys().copied().collect();
    let mut out = Vec::new();
    for key in keys {
        let old = lock(natives()).remove(&key);
        drop(old);
        match native_create(key) {
            Ok(visual) => out.push((key, visual)),
            Err(e) => eprintln!("[hal-dcomp] layer {key:?} visuals: {e}"),
        }
    }
    out
}

/// What the shell last heard of a native layer: HWND, client origin, content size, visible.
#[derive(Clone, Copy, PartialEq)]
struct Placed {
    hwnd: usize,
    origin: [i32; 2],
    size: [i32; 2],
    visible: bool,
}

/// The visuals of one native layer, made when its content is created.
pub(crate) struct NativeVisual {
    outer: IDCompositionVisual2,
    inner: IDCompositionVisual2,
    corners: IDCompositionRectangleClip,
    effect: IDCompositionEffectGroup,
    /// the window whose tree holds `outer`
    window: Option<Key>,
    placed: Option<Placed>,
}

impl NativeVisual {
    /// Tells the shell the layer is hidden (it kept its last place).
    fn hide(&mut self, shell: &Shared, key: Key) {
        if let Some(p) = self.placed.filter(|p| p.visible) {
            self.placed = Some(Placed { visible: false, ..p });
            shell.web_view_place(key, p.hwnd, p.origin, p.size, false);
        }
    }
}

// SAFETY: DirectComposition objects are free-threaded (see `Agile`); a tree (and its DXGI
// swapchain and surfaces) is used by the HAL's submit path only, a native visual under the
// registry's lock
unsafe impl Send for NativeVisual {}
unsafe impl Send for Tree {}

fn natives() -> &'static Mutex<HashMap<Key, NativeVisual>> {
    static NATIVES: OnceLock<Mutex<HashMap<Key, NativeVisual>>> = OnceLock::new();
    NATIVES.get_or_init(Default::default)
}

/// Makes a native layer's visuals; answers the visual the content renders into.
pub(crate) fn native_create(key: Key) -> Result<Agile<IDCompositionVisual2>, String> {
    let dev = device()?;
    unsafe {
        let outer = dev.CreateVisual().map_err(|e| hr("CreateVisual", e))?;
        let inner = dev.CreateVisual().map_err(|e| hr("CreateVisual", e))?;
        let corners = dev.CreateRectangleClip().map_err(|e| hr("CreateRectangleClip", e))?;
        let effect = dev.CreateEffectGroup().map_err(|e| hr("CreateEffectGroup", e))?;
        outer.AddVisual(inner.as_IDCompositionVisual(), false, None).map_err(|e| hr("AddVisual", e))?;
        inner.SetClip(Some(corners.as_IDCompositionClip())).map_err(|e| hr("SetClip", e))?;
        outer.SetEffect(Some(effect.as_IDCompositionEffect())).map_err(|e| hr("SetEffect", e))?;
        let _ = inner.SetBorderMode(DCOMPOSITION_BORDER_MODE_SOFT);
        let visual = Agile(inner.clone());
        lock(natives()).insert(key, NativeVisual { outer, inner, corners, effect, window: None, placed: None });
        Ok(visual)
    }
}

pub(crate) fn native_known(key: Key) -> bool {
    lock(natives()).contains_key(&key)
}

/// The window whose tree holds native layer `key`.
pub(crate) fn native_window(key: Key) -> Option<Key> {
    lock(natives()).get(&key).and_then(|v| v.window)
}

/// The layer's content is gone (its holding tree let it go first).
pub(crate) fn native_forget(key: Key) {
    let removed = lock(natives()).remove(&key);
    drop(removed);
}

/// The window tree that holds each of `keys` now, where that is not `window`.
pub(crate) fn held_elsewhere(window: Key, keys: impl Iterator<Item = Key>) -> Vec<(Key, Key)> {
    let natives = lock(natives());
    keys.filter_map(|k| natives.get(&k).and_then(|v| v.window).filter(|w| *w != window).map(|w| (k, w))).collect()
}

/// Every native layer of session `sid`.
pub(crate) fn natives_of(sid: u32) -> Vec<Key> {
    lock(natives()).keys().filter(|k| k.0 == sid).copied().collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Item {
    Native(Key),
    Segment(usize),
}

struct Upper {
    visual: IDCompositionVisual2,
    surface: IDCompositionSurface,
    size: [u32; 2],
    hidden: Option<Instant>,
}

/// One window's composition: its target and visual tree.
pub(crate) struct Tree {
    _target: IDCompositionTarget,
    root: IDCompositionVisual2,
    seg0: IDCompositionVisual2,
    /// seg0's surface once the window has had native layers
    surface0: Option<(IDCompositionSurface, [u32; 2])>,
    factory: Option<IDCompositionSurfaceFactory>,
    hwnd: usize,
    /// z order above seg0, as the tree holds it
    order: Vec<Item>,
    uppers: HashMap<usize, Upper>,
    /// the input regions the shell has (none: nothing published)
    regions: bool,
    /// the session whose layers the tree holds (a lent window's borrower)
    pub session: Option<u32>,
    /// the last commit failed (logged once per streak)
    commit_failed: bool,
}

/// One upper segment of a frame: window-pixel rect and the image to show there.
pub(crate) struct Segment {
    pub gap: usize,
    pub rect: [f32; 4],
}

/// Copies `image` into a composition surface; the update shows with the device's next commit.
unsafe fn draw(dev: &Device, surface: &IDCompositionSurface, image: &Texture, size: [u32; 2]) -> Result<(), String> {
    let mut raw = std::ptr::null_mut();
    let mut offset = POINT::default();
    surface.BeginDraw(None, &ID3D11Texture2D::IID, &mut raw, &mut offset).map_err(|e| hr("BeginDraw", e))?;
    let target = ID3D11Texture2D::from_raw(raw).map_err(|e| hr("BeginDraw", e));
    if let Ok(target) = &target {
        let bx = D3D11_BOX { left: 0, top: 0, front: 0, right: size[0].min(image.desc.w), bottom: size[1].min(image.desc.h), back: 1 };
        dev.ctx.CopySubresourceRegion(target.as_ID3D11Resource(), 0, offset.x as u32, offset.y as u32, 0, image.res.as_ID3D11Resource(), 0, Some(&bx));
    }
    let made = target.map(drop);
    let ended = surface.EndDraw().map_err(|e| hr("EndDraw", e));
    made.and(ended)
}

impl Tree {
    pub fn new(hwnd: usize, seg0: &IDXGISwapChain1) -> Result<Self, String> {
        let dev = device()?;
        unsafe {
            let target = dev.CreateTargetForHwnd(HWND(hwnd as _), true).map_err(|e| hr("CreateTargetForHwnd", e))?;
            let root = dev.CreateVisual().map_err(|e| hr("CreateVisual", e))?;
            let visual = dev.CreateVisual().map_err(|e| hr("CreateVisual", e))?;
            visual.SetContent(Some(seg0.as_IUnknown())).map_err(|e| hr("SetContent", e))?;
            root.AddVisual(visual.as_IDCompositionVisual(), false, None).map_err(|e| hr("AddVisual", e))?;
            target.SetRoot(Some(root.as_IDCompositionVisual())).map_err(|e| hr("SetRoot", e))?;
            dev.Commit().map_err(|e| hr("Commit", e))?;
            Ok(Self { _target: target, root, seg0: visual, surface0: None, factory: None, hwnd, order: Vec::new(),
                uppers: HashMap::new(), regions: false, session: None, commit_failed: false })
        }
    }

    /// A new surface with `image` in it; nothing shows it yet.
    fn surface(&mut self, dev: &Device, image: &Texture, size: [u32; 2], alpha: DXGI_ALPHA_MODE) -> Result<IDCompositionSurface, String> {
        if self.factory.is_none() {
            self.factory = Some(unsafe { device()?.CreateSurfaceFactory(dev.device.as_IUnknown()) }.map_err(|e| hr("CreateSurfaceFactory", e))?);
        }
        let surface = unsafe { self.factory.as_ref().unwrap().CreateSurface(size[0].max(1), size[1].max(1), DXGI_FORMAT_B8G8R8A8_UNORM, alpha) }
            .map_err(|e| hr("CreateSurface", e))?;
        unsafe { draw(dev, &surface, image, size)? };
        Ok(surface)
    }

    /// seg0 shows `image` through a surface (the window has, or has had, native layers). A
    /// failure leaves seg0 showing what it showed.
    pub fn seg0_surface(&mut self, dev: &Device, image: &Texture, size: [u32; 2], alpha: DXGI_ALPHA_MODE) -> Result<(), String> {
        match &self.surface0 {
            Some((surface, s)) if *s == size => unsafe { draw(dev, surface, image, size) },
            _ => {
                let surface = self.surface(dev, image, size, alpha)?;
                unsafe { self.seg0.SetContent(Some(surface.as_IUnknown())) }.map_err(|e| hr("SetContent", e))?;
                self.surface0 = Some((surface, size));
                Ok(())
            }
        }
    }

    /// Whether upper segment `gap` has a picture: when a draw (or a resize) fails it stays, at
    /// its last place and size, until a later frame draws it.
    pub fn has_upper(&self, gap: usize) -> bool {
        // a segment hidden since an earlier frame (a closed popup) has nothing current to keep
        self.uppers.get(&gap).is_some_and(|u| u.hidden.is_none())
    }

    /// Upper segment `gap` shows `image` (its size) through a premultiplied surface. A failure
    /// leaves the segment's last picture.
    pub fn upper(&mut self, dev: &Device, gap: usize, image: &Texture) -> Result<(), String> {
        let size = [image.desc.w, image.desc.h];
        if let Some(u) = self.uppers.get_mut(&gap).filter(|u| u.size == size) {
            u.hidden = None;
            return unsafe { draw(dev, &u.surface, image, size) };
        }
        let surface = self.surface(dev, image, size, DXGI_ALPHA_MODE_PREMULTIPLIED)?;
        match self.uppers.get_mut(&gap) {
            Some(u) => {
                unsafe { u.visual.SetContent(Some(surface.as_IUnknown())) }.map_err(|e| hr("SetContent", e))?;
                u.surface = surface;
                u.size = size;
                u.hidden = None;
            }
            None => {
                let visual = unsafe {
                    let v = device()?.CreateVisual().map_err(|e| hr("CreateVisual", e))?;
                    v.SetContent(Some(surface.as_IUnknown())).map_err(|e| hr("SetContent", e))?;
                    v
                };
                self.uppers.insert(gap, Upper { visual, surface, size, hidden: None });
            }
        }
        Ok(())
    }

    fn commit_device(&mut self, dev: &IDCompositionDesktopDevice) {
        match unsafe { dev.Commit() } {
            Ok(()) => self.commit_failed = false,
            Err(e) => {
                if !self.commit_failed { eprintln!("[hal-dcomp] Commit: {e}"); }
                self.commit_failed = true;
                request_check();
            }
        }
    }

    /// Puts the tree's children in `order` above seg0 (when it changed). Layers that left the
    /// order are hidden from their content too.
    fn reorder(&mut self, shell: &Shared, window: Key, order: &[Item], natives: &mut HashMap<Key, NativeVisual>) {
        if order == self.order.as_slice() { return; }
        unsafe {
            let _ = self.root.RemoveAllVisuals();
            let _ = self.root.AddVisual(self.seg0.as_IDCompositionVisual(), false, None);
            let mut below = self.seg0.clone();
            for item in order {
                let visual = match item {
                    Item::Segment(gap) => self.uppers.get(gap).map(|u| u.visual.clone()),
                    Item::Native(key) => natives.get_mut(key).map(|v| {
                        v.window = Some(window);
                        v.outer.clone()
                    }),
                };
                let Some(visual) = visual else { continue };
                if self.root.AddVisual(visual.as_IDCompositionVisual(), true, Some(below.as_IDCompositionVisual())).is_ok() {
                    below = visual;
                }
            }
        }
        for item in &self.order {
            if let Item::Native(key) = item {
                if !order.contains(item) {
                    if let Some(v) = natives.get_mut(key) {
                        if v.window == Some(window) { v.window = None; }
                        v.hide(shell, *key);
                    }
                }
            }
        }
        self.order = order.to_vec();
    }

    /// Places the frame's native layers and upper segments in z order and commits it all once.
    /// `layers` are the placed NATIVE records with their occluders; `order` interleaves them
    /// (`Item::Native`) with the upper segments that had draws this frame.
    pub fn commit(&mut self, shell: &Shared, window: Key, session: u32, layers: &[(Layer, Vec<[f32; 4]>)], segments: &[Segment], order: &[Item]) {
        let Ok(dev) = device() else { return };
        if !layers.is_empty() { self.session = Some(session); }
        let mut natives = lock(natives());
        unsafe {
            for s in segments {
                if let Some(u) = self.uppers.get(&s.gap) {
                    let _ = u.visual.SetOffsetX2(s.rect[0]);
                    let _ = u.visual.SetOffsetY2(s.rect[1]);
                }
            }
        }
        for (g, _) in layers {
            let Some(v) = natives.get_mut(&(session, g.native)) else { continue };
            let origin = unsafe { place(v, g) };
            let size = [g.rect[2].round() as i32, g.rect[3].round() as i32];
            let visible = g.flags & layer_flags::VISIBLE != 0 && g.clip[2] > 0.0 && g.clip[3] > 0.0;
            let placed = Placed { hwnd: self.hwnd, origin, size, visible };
            if v.placed != Some(placed) {
                v.placed = Some(placed);
                shell.web_view_place((session, g.native), self.hwnd, origin, size, visible);
            }
        }
        self.reorder(shell, window, order, &mut natives);
        let now = Instant::now();
        for (gap, u) in &mut self.uppers {
            if !order.contains(&Item::Segment(*gap)) && u.hidden.is_none() { u.hidden = Some(now); }
        }
        self.uppers.retain(|_, u| u.hidden.is_none_or(|t| t.elapsed() < IDLE));
        self.commit_device(&dev);
        drop(natives);
        let regions: Vec<LayerRegion> = layers.iter().map(|(g, occluders)| LayerRegion {
            key: (session, g.native), rect: g.rect, clip: g.clip, radius: g.radius,
            transform: [g.transform[0], g.transform[1], g.transform[4], g.transform[5], g.transform[12], g.transform[13]],
            flags: g.flags, occluders: occluders.clone(),
        }).collect();
        if !regions.is_empty() || self.regions {
            self.regions = !regions.is_empty();
            shell.set_layer_regions(self.hwnd, regions);
        }
    }

    /// A frame that is not the layers' own (the host's frame into a window lent to another
    /// session): seg0's new picture commits with the layers where they were; the upper segments
    /// hide, since that picture holds its own Makepad content.
    pub fn keep(&mut self, shell: &Shared, window: Key) {
        let Ok(dev) = device() else { return };
        let order: Vec<Item> = self.order.iter().copied().filter(|i| matches!(i, Item::Native(_))).collect();
        let mut natives = lock(natives());
        self.reorder(shell, window, &order, &mut natives);
        drop(natives);
        let now = Instant::now();
        for u in self.uppers.values_mut() { u.hidden.get_or_insert(now); }
        self.commit_device(&dev);
    }

    /// A badge composition: its picture holds every Makepad segment, drawn over the window's
    /// last frame. Native layers above seg0 would hide what it draws over them and their input
    /// regions would take clicks meant for it, so they hide and give up their input until the
    /// app's next own frame places them again; the upper segments hide too.
    pub fn suspend(&mut self, shell: &Shared, window: Key) {
        let Ok(dev) = device() else { return };
        let mut natives = lock(natives());
        self.reorder(shell, window, &[], &mut natives);
        drop(natives);
        let now = Instant::now();
        for u in self.uppers.values_mut() { u.hidden.get_or_insert(now); }
        if self.regions {
            shell.set_layer_regions(self.hwnd, Vec::new());
            self.regions = false;
        }
        self.commit_device(&dev);
    }

    /// Lets native layer `key` go: it moved to another window's tree.
    pub fn release_native(&mut self, key: Key) {
        if let Some(v) = lock(natives()).get_mut(&key) {
            unsafe { let _ = self.root.RemoveVisual(v.outer.as_IDCompositionVisual()); }
            v.window = None;
        }
        self.order.retain(|i| *i != Item::Native(key));
    }

    /// The tree's native layers are released and hidden (the window or its chain goes).
    pub fn detach(&mut self, shell: &Shared) {
        let mut natives = lock(natives());
        for item in &self.order {
            if let Item::Native(key) = item {
                if let Some(v) = natives.get_mut(key) {
                    unsafe { let _ = self.root.RemoveVisual(v.outer.as_IDCompositionVisual()); }
                    v.window = None;
                    v.hide(shell, *key);
                    v.placed = None;
                }
            }
        }
        drop(natives);
        self.order.clear();
        if self.regions {
            shell.set_layer_regions(self.hwnd, Vec::new());
            self.regions = false;
        }
        if let Ok(dev) = device() { unsafe { let _ = dev.Commit(); } }
    }
}

/// A native layer's geometry: `outer` clips to the clip rectangle and carries the transform and
/// opacity (all in window pixels); `inner` sits at the rect and rounds its corners. A pure
/// translation is whole pixels, so the page is not resampled. Answers the rect's client origin.
unsafe fn place(v: &NativeVisual, g: &Layer) -> [i32; 2] {
    let t = &g.transform;
    let translation = t[0] == 1.0 && t[1] == 0.0 && t[4] == 0.0 && t[5] == 1.0;
    let (dx, dy) = if translation { (t[12].round(), t[13].round()) } else { (t[12], t[13]) };
    let mut m = D2D_MATRIX_3X2_F::default();
    m.Anonymous.m = [t[0], t[1], t[4], t[5], dx, dy];
    let _ = v.outer.SetTransform2(&m);
    let clip = D2D_RECT_F { left: g.clip[0], top: g.clip[1], right: g.clip[0] + g.clip[2].max(0.0), bottom: g.clip[1] + g.clip[3].max(0.0) };
    let _ = v.outer.SetClip2(&clip);
    let _ = v.effect.SetOpacity2(if g.flags & layer_flags::VISIBLE != 0 { g.opacity.clamp(0.0, 1.0) } else { 0.0 });
    let (x, y) = (g.rect[0].round(), g.rect[1].round());
    let _ = v.inner.SetOffsetX2(x);
    let _ = v.inner.SetOffsetY2(y);
    let (w, h) = (g.rect[2].round().max(0.0), g.rect[3].round().max(0.0));
    let c = &v.corners;
    let _ = c.SetLeft2(0.0);
    let _ = c.SetTop2(0.0);
    let _ = c.SetRight2(w);
    let _ = c.SetBottom2(h);
    // radius order: top-left, top-right, bottom-right, bottom-left
    let r = g.radius.map(|r| r.max(0.0).min(w.min(h) * 0.5));
    let _ = c.SetTopLeftRadiusX2(r[0]);
    let _ = c.SetTopLeftRadiusY2(r[0]);
    let _ = c.SetTopRightRadiusX2(r[1]);
    let _ = c.SetTopRightRadiusY2(r[1]);
    let _ = c.SetBottomRightRadiusX2(r[2]);
    let _ = c.SetBottomRightRadiusY2(r[2]);
    let _ = c.SetBottomLeftRadiusX2(r[3]);
    let _ = c.SetBottomLeftRadiusY2(r[3]);
    // where the transform puts the rect's top left in the client
    [(t[0] * x + t[4] * y + dx).round() as i32, (t[1] * x + t[5] * y + dy).round() as i32]
}

/// The native records of a window frame in z order, at most `budget` of them, and the draw
/// range of every gap (gap 0 = seg0). Records past the budget are answered by the caller.
pub(crate) fn split(records: &[Layer], window_draws: crate::Range, budget: usize, mut rejected: impl FnMut(Id)) -> (Vec<Layer>, Vec<crate::Range>) {
    let mut natives = Vec::new();
    let mut ranges = vec![crate::Range { off: window_draws.off, len: 0 }];
    for g in records {
        if g.kind == Layer::NATIVE {
            if natives.len() == budget {
                rejected(g.native);
                continue;
            }
            natives.push(*g);
            ranges.push(crate::Range { off: g.draws.off, len: 0 });
        } else {
            let r = ranges.last_mut().unwrap();
            if r.len == 0 { r.off = g.draws.off; }
            r.len += g.draws.len;
        }
    }
    (natives, ranges)
}
