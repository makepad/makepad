//! Screen-capture surfaces on D3D11 (the backend half of
//! [`crate::screen_capture::add_screen_capture_surface`]).
//!
//! While a surface sink is due a frame, the window pass copies its back
//! buffer on the GPU (`CopySubresourceRegion`, queued behind the frame's draws
//! and before `Present`) into the next free texture of a small ring and hands
//! that texture to the sinks at once. Nothing is mapped or read on the render
//! thread. The recorder's encoder (Media Foundation with the app's device,
//! `VideoFileEncoder::new_d3d11`) reads the texture on the GPU; ordering is
//! the device's own, since both go through its immediate context.
//!
//! A ring texture is free when the ring holds its only reference: sinks,
//! the encoder and Media Foundation's samples each hold one while they use
//! it. With none free the frame is skipped, never waited for.

use {
    crate::{
        os::windows::d3d11::D3d11Cx,
        screen_capture::{ScreenCaptureSurface, SurfaceInner},
        windows::{
            Win32::Graphics::{
                Direct3D11::{
                    ID3D11Device, ID3D11Multithread, ID3D11Resource, ID3D11Texture2D,
                    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
                    D3D11_MAP, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE,
                },
                Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
            },
        },
    },
    std::cell::RefCell,
    std::sync::atomic::{AtomicU64, Ordering},
};

/// Textures in use at once: the sink's pending slot, the one the encoder
/// repeats, and the frames Media Foundation has queued but not yet read.
const RING_TEXTURES: usize = 6;

/// Frames skipped because every ring texture was in use. `MAKEPAD_TRACE=screencap`.
static SKIPPED: AtomicU64 = AtomicU64::new(0);

struct Ring {
    device: ID3D11Device,
    size: (u32, u32),
    textures: Vec<ID3D11Texture2D>,
}

thread_local! {
    static RING: RefCell<Option<Ring>> = const { RefCell::new(None) };
}

/// Copy `back_buffer` into a capture texture for the due surface sinks of
/// `window_id`. Call after the frame's draws and before `Present`.
pub(crate) fn encode_capture_surface(
    d3d11_cx: &D3d11Cx,
    back_buffer: Option<&ID3D11Texture2D>,
    window_id: Option<usize>,
) {
    if !crate::screen_capture::capture_wants_surface(window_id) {
        if !crate::screen_capture::screen_capture_active() {
            RING.with(|ring| ring.borrow_mut().take());
        }
        return;
    }
    let Some(back_buffer) = back_buffer else {
        return;
    };
    let started = std::time::Instant::now();
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { back_buffer.GetDesc(&mut desc) };
    let (width, height) = (desc.Width & !1, desc.Height & !1);
    if width < 2 || height < 2 || desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM || desc.SampleDesc.Count != 1 {
        return;
    }
    let Some(texture) = RING.with(|ring| {
        let mut ring = ring.borrow_mut();
        if ring.as_ref().is_some_and(|r| r.size != (width, height) || r.device != d3d11_cx.device) {
            *ring = None;
        }
        let ring = ring.get_or_insert_with(|| {
            // Media Foundation reads these textures from its own threads.
            if let Ok(multithread) = unsafe { ID3D11Multithread::query(d3d11_cx.device.as_raw()) } {
                let _ = unsafe { multithread.SetMultithreadProtected(true) };
            }
            Ring { device: d3d11_cx.device.clone(), size: (width, height), textures: Vec::new() }
        });
        ring.next_free()
    }) else {
        SKIPPED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let (Ok(destination), Ok(source)) = (unsafe { ID3D11Resource::query(texture.as_raw()) }, unsafe { ID3D11Resource::query(back_buffer.as_raw()) }) else {
        return;
    };
    let region = D3D11_BOX { left: 0, top: 0, front: 0, right: width, bottom: height, back: 1 };
    unsafe {
        d3d11_cx
            .context
            .CopySubresourceRegion(&destination, 0, 0, 0, 0, &source, 0, Some(&region));
    }
    crate::screen_capture::deliver_capture_surface(
        window_id,
        ScreenCaptureSurface {
            inner: SurfaceInner { texture, device: d3d11_cx.device.clone() },
            width,
            height,
            time_ns: 0,
        },
    );
    crate::trace!(
        "screencap",
        "surface {}x{} encoded in {:.3} ms (skipped so far: {})",
        width,
        height,
        started.elapsed().as_secs_f64() * 1000.0,
        SKIPPED.load(Ordering::Relaxed)
    );
}

impl Ring {
    /// A texture nobody but the ring holds, or a new one while the ring is
    /// not full.
    fn next_free(&mut self) -> Option<ID3D11Texture2D> {
        if let Some(free) = self.textures.iter().find(|t| only_ring_holds(t)) {
            return Some(free.clone());
        }
        if self.textures.len() >= RING_TEXTURES {
            return None;
        }
        let (width, height) = self.size;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE(0), // D3D11_USAGE_DEFAULT
            // What Media Foundation's video processor reads its input as.
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture: Option<ID3D11Texture2D> = None;
        if let Err(err) = unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut texture)) } {
            crate::error!("screen capture: CreateTexture2D({}x{}) failed: {}", width, height, err);
            return None;
        }
        let texture = texture?;
        self.textures.push(texture.clone());
        Some(texture)
    }
}

/// Whether the ring's own reference is the texture's only one (sinks, the
/// encoder and Media Foundation's samples hold theirs while they use it).
fn only_ring_holds(texture: &ID3D11Texture2D) -> bool {
    unsafe {
        let raw = texture.as_raw();
        let vtable = *(raw as *const *const crate::windows::core::IUnknown_Vtbl);
        ((*vtable).AddRef)(raw);
        ((*vtable).Release)(raw) == 1
    }
}

/// [`ScreenCaptureSurface::read_rgba`] on D3D11: a staging copy on the
/// surface's device, mapped (waits for the copy) on the caller's thread.
pub(crate) fn read_texture_rgba(surface: &ScreenCaptureSurface, out: &mut Vec<u8>) -> bool {
    let SurfaceInner { texture, device } = &surface.inner;
    let (width, height) = (surface.width, surface.height);
    unsafe {
        let Ok(context) = device.GetImmediateContext() else {
            return false;
        };
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE(3), // D3D11_USAGE_STAGING
            BindFlags: 0,
            CPUAccessFlags: 0x20000, // D3D11_CPU_ACCESS_READ
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        if device.CreateTexture2D(&desc, None, Some(&mut staging)).is_err() {
            return false;
        }
        let (Some(Ok(staging)), Ok(source)) = (staging.map(|s| ID3D11Resource::query(s.as_raw())), ID3D11Resource::query(texture.as_raw())) else {
            return false;
        };
        context.CopyResource(&staging, &source);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        if context.Map(&staging, 0, D3D11_MAP(1), 0, Some(&mut mapped)).is_err() {
            return false;
        }
        let row_bytes = width as usize * 4;
        for (y, row) in out.chunks_exact_mut(row_bytes).enumerate().take(height as usize) {
            let src = std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                row_bytes,
            );
            crate::screen_capture::swizzle_opaque(row, src, true);
        }
        context.Unmap(&staging, 0);
        true
    }
}
