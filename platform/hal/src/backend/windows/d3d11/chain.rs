//! Swapchains: one flip-discard composition swapchain per native window, shown by the window's
//! DirectComposition tree (`dcomp`), made on the window's first presented frame and resized to
//! its client pixels. A main window's chain is waitable (buffer count = latency + 1; the shell's
//! pacer waits on its credits); a popup's is not. A frame is the window's retained image copied
//! into the back buffer and presented without waiting. From a window's first native layer on,
//! its frames go through composition surfaces instead (seg0 and the upper segments), committed
//! once per frame with the layers' geometry, and the pacer follows the compositor's clock.

use super::dcomp::{self, Item, Segment, Tree};
use super::device::{hr, Device};
use super::resources::Texture;
use crate::Layer;
use crate::backend::windows::win32::{Key, Native, Shared, Waitable};
use makepad_windows_sys::Win32::Graphics::Direct3D11::*;
use makepad_windows_sys::Win32::Graphics::Dxgi::Common::*;
use makepad_windows_sys::Win32::Graphics::Dxgi::*;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// The frame latency of a main window: 2 lets the CPU build frame N+1 while the compositor still
/// holds N; `MAKEPAD_WIN_LATENCY=1` trades that for the lowest latency (drawing/ink).
fn latency() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| if std::env::var("MAKEPAD_WIN_LATENCY").ok().as_deref() == Some("1") { 1 } else { 2 })
}

struct Chain {
    swap: IDXGISwapChain1,
    native: Arc<Native>,
    size: [u32; 2],
    waitable: bool,
    buffers: u32,
    logged_present: bool,
    logged_composed: bool,
    /// premultiplied for a see-through window, opaque otherwise
    alpha: DXGI_ALPHA_MODE,
    tree: Tree,
    /// the window has had native layers: its frames go through surfaces (`dcomp`)
    composed: bool,
    /// upper segments whose last update failed (logged once per failure streak)
    failed_uppers: Vec<usize>,
}

/// A frame of a window with native layers: the placed NATIVE records (with their occluders),
/// the upper segments that had draws (rendered at their rects) and the z order above seg0.
pub struct Layered<'a> {
    /// the session whose layers these are (a lent window's borrower, not its host)
    pub session: u32,
    pub layers: &'a [(Layer, Vec<[f32; 4]>)],
    pub uppers: &'a [(Segment, Arc<Texture>)],
    pub order: &'a [Item],
}

#[derive(Default)]
pub struct Chains {
    map: HashMap<Key, Chain>,
    /// a window frame did not fully reach its surfaces for a reason other than a lost device:
    /// its app draws again (`take_retry`)
    retry: bool,
}

impl Chains {
    fn desc(size: [u32; 2], waitable: bool, alpha: DXGI_ALPHA_MODE) -> DXGI_SWAP_CHAIN_DESC1 {
        DXGI_SWAP_CHAIN_DESC1 {
            Width: size[0].max(1),
            Height: size[1].max(1),
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: super::device::b(false),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: if waitable { latency() + 1 } else { 2 },
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: alpha,
            Flags: if waitable { DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32 } else { 0 },
        }
    }

    fn create(dev: &Device, shell: &Shared, key: Key, native: Arc<Native>) -> Result<Chain, String> {
        let size = native.size();
        let waitable = !native.popup;
        let alpha = if native.see_through.load(Ordering::Acquire) { DXGI_ALPHA_MODE_PREMULTIPLIED } else { DXGI_ALPHA_MODE_IGNORE };
        let desc = Self::desc(size, waitable, alpha);
        let swap = unsafe { dev.factory.CreateSwapChainForComposition(dev.device.as_IUnknown(), &desc, None) }
            .map_err(|e| hr("CreateSwapChainForComposition", e))?;
        let tree = Tree::new(native.hwnd, &swap)?;
        let swap2 = unsafe { IDXGISwapChain2::query(swap.as_raw()) }.ok();
        if let Some(s2) = &swap2 {
            unsafe {
                let _ = s2.SetMaximumFrameLatency(if waitable { latency() } else { 1 });
            }
        }
        let latency = if waitable {
            let handle = swap2.as_ref().map(|s2| unsafe { s2.GetFrameLatencyWaitableObject() });
            handle.filter(|h| !h.0.is_null()).map(|h| Arc::new(Waitable(h)))
        } else {
            None
        };
        shell.set_waitable(key, latency);
        Ok(Chain { swap, native, size, waitable, buffers: desc.BufferCount, logged_present: false, logged_composed: false, alpha, tree, composed: false, failed_uppers: Vec::new() })
    }

    /// Copies `image` into the window's back buffer and presents it. False when the window has
    /// no native surface (yet / any more) or DXGI did not take the frame.
    pub fn present(&mut self, dev: &Device, shell: &Shared, key: Key, image: &Texture, layered: Option<Layered>) -> bool {
        let Some(native) = shell.native(key).filter(|n| n.alive.load(Ordering::Acquire)) else {
            self.remove(shell, key);
            return false;
        };
        let size = native.size();
        if size[0] == 0 || size[1] == 0 {
            return false;
        }
        let see_through = native.see_through.load(Ordering::Acquire);
        if self.map.get(&key).is_some_and(|c| c.native.token != native.token || (c.alpha == DXGI_ALPHA_MODE_PREMULTIPLIED) != see_through) {
            self.remove(shell, key);
        }
        if let Some(l) = &layered {
            // a layer moving here from another window leaves that window's tree first
            for (layer, window) in dcomp::held_elsewhere(key, l.layers.iter().map(|(g, _)| (l.session, g.native))) {
                if let Some(other) = self.map.get_mut(&window) { other.tree.release_native(layer); }
            }
        }
        if !self.map.contains_key(&key) {
            match Self::create(dev, shell, key, native.clone()) {
                Ok(c) => {
                    self.map.insert(key, c);
                }
                Err(e) => {
                    eprintln!("[hal-d3d11] swapchain for {key:?}: {e}");
                    return false;
                }
            }
        }
        let chain = self.map.get_mut(&key).unwrap();
        if chain.composed || layered.as_ref().is_some_and(|l| !l.layers.is_empty()) {
            if !chain.composed {
                // no present from here on: the pacer follows the compositor's clock
                chain.composed = true;
                shell.set_waitable(key, None);
                shell.set_compositor_paced(key, true);
            }
            if let Err(e) = chain.tree.seg0_surface(dev, image, size, chain.alpha) {
                eprintln!("[hal-d3d11] window {key:?} composition surface: {e}");
                // seg0 keeps its last picture; unless the device is lost, the app draws again
                self.retry |= !dev.check_lost();
                return false;
            }
            // only the layers' own session places them; the host's frame into a window lent out
            // keeps their last placement; a badge composition (no layers to place) hides them
            match layered {
                Some(l) if !l.layers.is_empty() || chain.tree.session.is_none_or(|s| s == l.session) => {
                    self.retry |= !Self::present_layers(dev, shell, key, chain, l);
                }
                Some(_) => chain.tree.keep(shell, key),
                None => chain.tree.suspend(shell, key),
            }
            if !chain.logged_composed {
                chain.logged_composed = true;
                eprintln!("[hal-d3d11] window {key:?} composes through surfaces from here on ({size:?})");
            }
            shell.presented(key);
            return true;
        }
        if chain.size != size {
            let flags = if chain.waitable { DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT } else { DXGI_SWAP_CHAIN_FLAG(0) };
            // no reference to a back buffer is held between frames
            if let Err(e) = unsafe { chain.swap.ResizeBuffers(chain.buffers, size[0], size[1], DXGI_FORMAT_B8G8R8A8_UNORM, flags) } {
                eprintln!("[hal-d3d11] ResizeBuffers {size:?}: {e}");
                return false;
            }
            chain.size = size;
        }
        let back: ID3D11Texture2D = match unsafe { chain.swap.GetBuffer(0, &ID3D11Texture2D::IID) }.and_then(|raw| unsafe { ID3D11Texture2D::from_raw(raw) }) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[hal-d3d11] GetBuffer: {e}");
                return false;
            }
        };
        // the overlap of the rendered image and the client (they differ while a resize lands)
        let (w, h) = (image.desc.w.min(size[0]), image.desc.h.min(size[1]));
        let bx = D3D11_BOX { left: 0, top: 0, front: 0, right: w, bottom: h, back: 1 };
        unsafe {
            dev.ctx.CopySubresourceRegion(back.as_ID3D11Resource(), 0, 0, 0, 0, image.res.as_ID3D11Resource(), 0, Some(&bx));
        }
        drop(back);
        let r = unsafe { chain.swap.Present(1, DXGI_PRESENT_DO_NOT_WAIT) };
        if r == DXGI_ERROR_WAS_STILL_DRAWING {
            // the compositor holds every buffer: the frame stays in the retained image
            return false;
        }
        if r.is_err() {
            eprintln!("[hal-d3d11] Present: {r}");
            dev.check_lost();
            return false;
        }
        if !chain.logged_present {
            chain.logged_present = true;
            if crate::backend::windows::win32::log_enabled() {
                eprintln!("[hal-d3d11] first frame presented key={key:?} size={size:?} waitable={}", chain.waitable);
            }
        }
        // S_OK, or DXGI_STATUS_OCCLUDED (the window is not visible; the frame still counts)
        shell.presented(key);
        true
    }

    /// The upper segments' surfaces get their images, then the tree commits once: seg0's surface
    /// (drawn by the caller), the uppers and every layer's geometry show in the same DWM frame.
    /// False when an upper segment could not be updated (it keeps its last picture).
    fn present_layers(dev: &Device, shell: &Shared, key: Key, chain: &mut Chain, l: Layered) -> bool {
        let mut segments = Vec::new();
        let mut kept = Vec::new();
        for (segment, image) in l.uppers {
            match chain.tree.upper(dev, segment.gap, image) {
                Ok(()) => {
                    chain.failed_uppers.retain(|g| *g != segment.gap);
                    segments.push(Segment { gap: segment.gap, rect: segment.rect });
                }
                Err(e) => {
                    if !chain.failed_uppers.contains(&segment.gap) {
                        chain.failed_uppers.push(segment.gap);
                        eprintln!("[hal-d3d11] window {key:?} segment {}: {e}", segment.gap);
                    }
                    // its last picture (any size) stays where it was; a segment that never had
                    // one is left out this frame
                    if chain.tree.has_upper(segment.gap) {
                        kept.push(segment.gap);
                    }
                }
            }
        }
        let order: Vec<Item> = l.order.iter().copied().filter(|i| match i {
            Item::Segment(gap) => segments.iter().any(|s| s.gap == *gap) || kept.contains(gap),
            Item::Native(_) => true,
        }).collect();
        chain.tree.commit(shell, key, l.session, l.layers, &segments, &order);
        let complete = segments.len() == l.uppers.len();
        if !complete { dev.check_lost(); }
        complete
    }

    /// Whether a window frame since the last call needs drawing again (see `retry`).
    pub fn take_retry(&mut self) -> bool {
        std::mem::take(&mut self.retry)
    }

    /// A native layer's content goes: the tree holding it lets it go.
    pub fn native_destroyed(&mut self, layer: Key) {
        if let Some(window) = dcomp::native_window(layer) {
            if let Some(chain) = self.map.get_mut(&window) {
                chain.tree.release_native(layer);
                if let Ok(d) = dcomp::device() { unsafe { let _ = d.Commit(); } }
            }
        }
        dcomp::native_forget(layer);
    }

    pub fn remove(&mut self, shell: &Shared, key: Key) {
        if let Some(mut chain) = self.map.remove(&key) {
            chain.tree.detach(shell);
            drop(chain);
            // the target lets the HWND go with this commit (a new chain may make one at once)
            if let Ok(d) = dcomp::device() { unsafe { let _ = d.Commit(); } }
            shell.set_waitable(key, None);
        }
    }

    /// Every swapchain goes (a device reset: the next present makes them on the new device).
    pub fn clear(&mut self, shell: &Shared) {
        let keys: Vec<Key> = self.map.keys().copied().collect();
        for k in keys {
            self.remove(shell, k);
        }
    }

    pub fn close_session(&mut self, shell: &Shared, sid: u32) {
        let keys: Vec<Key> = self.map.keys().filter(|k| k.0 == sid).copied().collect();
        for k in keys {
            self.remove(shell, k);
        }
        for layer in dcomp::natives_of(sid) {
            self.native_destroyed(layer);
        }
    }
}
