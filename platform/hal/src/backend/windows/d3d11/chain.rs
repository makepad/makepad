//! Swapchains: one flip-discard DXGI chain per native window, made from the shell's HWND on
//! the window's first presented frame and resized to its client pixels. A main window's chain is
//! waitable (buffer count = latency + 1; the shell's pacer waits on its credits); a popup's is not.
//! A frame is the window's retained image copied into the back buffer and presented without
//! waiting. Moved from os/windows/d3d11.rs (`D3d11Window::swap_chain_desc`, `create_swap_chain`,
//! `resize_buffers`, `present`).

use super::device::{hr, Device};
use super::resources::Texture;
use crate::backend::windows::win32::{Key, Native, Shared, Waitable};
use makepad_windows_sys::Win32::Foundation::*;
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
}

#[derive(Default)]
pub struct Chains {
    map: HashMap<Key, Chain>,
}

impl Chains {
    fn desc(size: [u32; 2], waitable: bool) -> DXGI_SWAP_CHAIN_DESC1 {
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
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: if waitable { DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32 } else { 0 },
        }
    }

    fn create(dev: &Device, shell: &Shared, key: Key, native: Arc<Native>) -> Result<Chain, String> {
        let size = native.size();
        let waitable = !native.popup;
        let desc = Self::desc(size, waitable);
        let swap = unsafe { dev.factory.CreateSwapChainForHwnd(dev.device.as_IUnknown(), HWND(native.hwnd as _), &desc, None, None) }
            .map_err(|e| hr("CreateSwapChainForHwnd", e))?;
        let swap2 = unsafe { IDXGISwapChain2::query(swap.as_raw()) }.ok();
        if let Some(s2) = &swap2 {
            unsafe {
                let _ = s2.SetMaximumFrameLatency(if waitable { latency() } else { 1 });
            }
        }
        if waitable {
            let handle = swap2.as_ref().map(|s2| unsafe { s2.GetFrameLatencyWaitableObject() });
            match handle.filter(|h| !h.0.is_null()) {
                Some(h) => shell.set_waitable(key, Some(Arc::new(Waitable(h)))),
                None => shell.set_waitable(key, None),
            }
        } else {
            shell.set_waitable(key, None);
        }
        Ok(Chain { swap, native, size, waitable, buffers: desc.BufferCount, logged_present: false })
    }

    /// Copies `image` into the window's back buffer and presents it. False when the window has
    /// no native surface (yet / any more) or DXGI did not take the frame.
    pub fn present(&mut self, dev: &Device, shell: &Shared, key: Key, image: &Texture) -> bool {
        let Some(native) = shell.native(key).filter(|n| n.alive.load(Ordering::Acquire)) else {
            self.remove(shell, key);
            return false;
        };
        let size = native.size();
        if size[0] == 0 || size[1] == 0 {
            return false;
        }
        if self.map.get(&key).is_some_and(|c| c.native.token != native.token) {
            self.remove(shell, key);
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

    pub fn remove(&mut self, shell: &Shared, key: Key) {
        if self.map.remove(&key).is_some() {
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
    }
}
