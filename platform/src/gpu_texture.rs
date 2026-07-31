//! Public GPU texture adopt hooks for app-owned video / camera surfaces.
//!
//! Makepad already adopts external GPU resources internally (`VideoExternal`,
//! OES, CVMetal, `SharedBGRAu8`). These APIs expose the same capability to
//! applications so hard-decode / camera / effect pipelines can present without
//! a CPU round-trip.
//!
//! # Ownership
//!
//! - Adopted GPU objects are **borrowed** by default: Makepad will **not**
//!   `Release` / `glDeleteTextures` them when the `Texture` is freed or
//!   re-adopted (Android sets `gl_texture_owned = false`).
//! - The caller must keep the underlying resource alive while any draw call
//!   may still sample the `Texture` (typically until the next successful adopt
//!   of a newer frame, plus one frame of latency).
//! - Resources must come from **Makepad's** D3D11 device / GL context
//!   (`Cx::d3d11_device` / `Cx::with_gl`). Cross-device sharing is out of scope.
//!
//! # Platforms
//!
//! | Platform | Entry points |
//! |----------|----------------|
//! | Windows  | [`Cx::d3d11_device`], [`Texture::adopt_d3d11_bgra`], [`Texture::adopt_d3d11_plane`] |
//! | Android  | [`Cx::with_gl`], [`Texture::adopt_oes_texture`], [`Texture::adopt_gl_texture_2d`] |
//!
//! Other platforms can be added later; unsupported targets compile these
//! helpers out via `cfg`.

use std::{
    cell::{Cell, RefCell},
    sync::{Mutex, MutexGuard},
};

use crate::{
    texture::{
        CxTexturePool, Texture, TextureAlloc, TextureCategory, TextureFormat, TextureId,
        TexturePixel,
    },
    Cx,
};

/// Serializes hard-decode / media GPU work with Makepad present copies on the
/// shared D3D11 device. Recursive so the same thread may nest lock calls
/// (common when a decoder callback re-enters present).
static MEDIA_D3D11_MUTEX: Mutex<()> = Mutex::new(());

thread_local! {
    static MEDIA_D3D11_LOCK_DEPTH: Cell<u32> = const { Cell::new(0) };
    static MEDIA_D3D11_LOCK_GUARD: RefCell<Option<MutexGuard<'static, ()>>> =
        const { RefCell::new(None) };
}

/// Acquire the shared media D3D11 lock (recursive on the same thread).
pub fn media_d3d11_lock() {
    MEDIA_D3D11_LOCK_DEPTH.with(|depth| {
        if depth.get() == 0 {
            let guard = MEDIA_D3D11_MUTEX.lock().unwrap();
            // Mutex is 'static; hold the guard across paired lock/unlock calls.
            let guard: MutexGuard<'static, ()> = unsafe { std::mem::transmute(guard) };
            MEDIA_D3D11_LOCK_GUARD.with(|slot| {
                *slot.borrow_mut() = Some(guard);
            });
        }
        depth.set(depth.get().saturating_add(1));
    });
}

/// Release the shared media D3D11 lock.
pub fn media_d3d11_unlock() {
    MEDIA_D3D11_LOCK_DEPTH.with(|depth| {
        let next = depth.get().saturating_sub(1);
        depth.set(next);
        if next == 0 {
            MEDIA_D3D11_LOCK_GUARD.with(|slot| {
                drop(slot.borrow_mut().take());
            });
        }
    });
}

/// Run `f` while holding the shared media D3D11 lock.
pub fn with_media_d3d11_lock<R>(f: impl FnOnce() -> R) -> R {
    media_d3d11_lock();
    let out = f();
    media_d3d11_unlock();
    out
}

/// C ABI wrapper around [`media_d3d11_lock`] for native decoder device-context
/// lock callbacks (function-pointer slots).
pub unsafe extern "C" fn media_d3d11_c_lock(_lock_ctx: *mut std::ffi::c_void) {
    media_d3d11_lock();
}

/// C ABI wrapper around [`media_d3d11_unlock`].
pub unsafe extern "C" fn media_d3d11_c_unlock(_lock_ctx: *mut std::ffi::c_void) {
    media_d3d11_unlock();
}

impl Texture {
    /// Allocate a [`TextureFormat::VideoExternal`] slot for later `adopt_*`.
    pub fn new_video_external(cx: &mut Cx) -> Self {
        Texture::new_with_format(cx, TextureFormat::VideoExternal)
    }

    /// Allocate a [`TextureFormat::VideoYuvPlane`] slot for Y / U / V (or NV12 UV).
    pub fn new_video_yuv_plane(cx: &mut Cx) -> Self {
        Texture::new_with_format(cx, TextureFormat::VideoYuvPlane)
    }
}

/// Zero-copy NV12 frame handed from a [`crate::MediaPlaybackSession`] to the
/// Windows video poll path. `keep_alive` must outlive GPU sampling of
/// `texture` (usually until the next frame replaces it).
#[cfg(target_os = "windows")]
pub struct D3d11Nv12Frame {
    pub texture: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    pub array_slice: u32,
    pub width: u32,
    pub height: u32,
    pub matrix: crate::video_decode::yuv::YuvColorMatrix,
    pub keep_alive: std::sync::Arc<dyn std::any::Any + Send + Sync>,
}

/// Ping-pong NV12 present targets for hardware-decoded frames.
///
/// Y and UV use **separate** NV12 textures (each with its own plane SRV). Binding
/// R8 + R8G8 views of the *same* NV12 resource in one draw TDRs some GPUs
/// (`DXGI_ERROR_DEVICE_REMOVED`).
#[cfg(target_os = "windows")]
#[derive(Default)]
pub struct D3d11Nv12PresentCache {
    y_slots: [Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>; 2],
    uv_slots: [Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>; 2],
    width: u32,
    height: u32,
    idx: usize,
}

/// Zero-copy OES frame for Android `sample_video` present.
#[cfg(target_os = "android")]
pub struct OesFrame {
    pub tex_id: u32,
    pub width: u32,
    pub height: u32,
    pub keep_alive: std::sync::Arc<dyn std::any::Any + Send + Sync>,
}

#[cfg(target_os = "windows")]
mod windows_api {
    use super::*;
    use windows::{
        core::Interface,
        Win32::Graphics::{
            Direct3D::D3D_SRV_DIMENSION,
            Direct3D11::{
                ID3D11Device, ID3D11Resource, ID3D11ShaderResourceView,
                ID3D11Texture2D, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
                D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC_0,
                D3D11_TEX2D_SRV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
            },
            Dxgi::Common::{
                DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_FORMAT_R8G8_UNORM, DXGI_FORMAT_R8_UNORM,
                DXGI_SAMPLE_DESC,
            },
        },
    };

    /// Makepad video shaders bind `Texture2D` (not `Texture2DArray`). Always use this.
    const SRV_DIMENSION_TEXTURE2D: D3D_SRV_DIMENSION = D3D_SRV_DIMENSION(4);

    impl Cx {
        /// Makepad's shared D3D11 device (same device used for UI rendering).
        ///
        /// Hard-decode and GPU present must use this device for zero-copy adopt
        /// into Makepad textures.
        pub fn d3d11_device(&self) -> Option<ID3D11Device> {
            self.os.d3d11_device.clone()
        }

        /// Publish the UI D3D11 device for media plugins / hard-decode threads
        /// that cannot hold `&Cx` (same device as [`Cx::d3d11_device`]).
        ///
        /// Also enables `ID3D11Multithread` protection so decode worker threads
        /// can safely share the device with the UI render thread.
        pub fn publish_d3d11_device_for_media(&self) {
            if let Some(device) = self.d3d11_device() {
                enable_d3d11_multithread_protected(&device);
                *MEDIA_D3D11_DEVICE.lock().unwrap() = Some(device);
            }
        }
    }

    static MEDIA_D3D11_DEVICE: std::sync::Mutex<Option<ID3D11Device>> =
        std::sync::Mutex::new(None);

    /// D3D11 device previously published via [`Cx::publish_d3d11_device_for_media`].
    pub fn media_d3d11_device() -> Option<ID3D11Device> {
        MEDIA_D3D11_DEVICE.lock().unwrap().clone()
    }

    /// Same QI path as Media Foundation video playback (`windows_video_playback`).
    fn enable_d3d11_multithread_protected(device: &ID3D11Device) {
        use std::ffi::c_void;
        use windows::core::{GUID, HRESULT, Interface};

        unsafe {
            let raw = Interface::as_raw(device);
            // IID_ID3D11Multithread
            let iid = GUID::from_u128(0x9B7E4E00_342C_4106_A19F_4F2704F689F0u128);
            let mut mt: *mut c_void = std::ptr::null_mut();
            let vtbl = *(raw as *const *const usize);
            let qi: unsafe extern "system" fn(
                *mut c_void,
                *const GUID,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(*vtbl);
            let hr = qi(raw, &iid, &mut mt);
            if hr.is_ok() && !mt.is_null() {
                // ID3D11Multithread::SetMultithreadProtected is vtable index 4.
                let mt_vtbl = *(mt as *const *const usize);
                let set_protected: unsafe extern "system" fn(*mut c_void, i32) -> i32 =
                    std::mem::transmute(*mt_vtbl.add(4));
                set_protected(mt, 1);
                // Release the QI'd interface.
                let release: unsafe extern "system" fn(*mut c_void) -> u32 =
                    std::mem::transmute(*mt_vtbl.add(2));
                release(mt);
            }
        }
    }

    impl Texture {
        /// Adopt a BGRA8 `ID3D11Texture2D` (+ optional SRV) as
        /// [`TextureFormat::VideoExternal`] for `sample_video` shaders.
        ///
        /// If `srv` is `None`, an SRV is created on Makepad's device.
        ///
        /// `width` / `height` are the logical video size (for callers); the
        /// internal alloc uses the platform `VideoExternal` sentinel (0×0).
        pub fn adopt_d3d11_bgra(
            &self,
            cx: &mut Cx,
            texture: ID3D11Texture2D,
            srv: Option<ID3D11ShaderResourceView>,
            width: usize,
            height: usize,
        ) -> Result<(), String> {
            if width == 0 || height == 0 {
                return Err("adopt_d3d11_bgra: width/height must be non-zero".into());
            }
            let device = cx
                .d3d11_device()
                .ok_or_else(|| "adopt_d3d11_bgra: D3D11 device not ready".to_string())?;

            let srv = match srv {
                Some(s) => s,
                None => {
                    let resource: ID3D11Resource = texture
                        .cast()
                        .map_err(|e| format!("adopt_d3d11_bgra: cast to resource failed: {e}"))?;
                    let mut out: Option<ID3D11ShaderResourceView> = None;
                    unsafe {
                        device
                            .CreateShaderResourceView(&resource, None, Some(&mut out))
                            .map_err(|e| {
                                format!("adopt_d3d11_bgra: CreateShaderResourceView failed: {e:?}")
                            })?;
                    }
                    out.ok_or_else(|| {
                        "adopt_d3d11_bgra: CreateShaderResourceView returned null".to_string()
                    })?
                }
            };

            let _ = (width, height);
            let cxtex = &mut cx.textures[self.texture_id()];
            cxtex.format = TextureFormat::VideoExternal;
            cxtex.os.texture = Some(texture);
            cxtex.os.shader_resource_view = Some(srv);
            cxtex.alloc = Some(TextureAlloc {
                width: 0,
                height: 0,
                pixel: TexturePixel::VideoExternal,
                category: TextureCategory::Video,
            });
            Ok(())
        }

        /// Adopt a single-plane (or plane-view) D3D11 texture as
        /// [`TextureFormat::VideoYuvPlane`] for YUV / NV12 shaders (`tex_y` /
        /// `tex_u` / `tex_v`).
        ///
        /// Typical NV12 zero-copy:
        /// - Y plane SRV: `DXGI_FORMAT_R8_UNORM`
        /// - UV plane SRV: `DXGI_FORMAT_R8G8_UNORM`
        /// then adopt each into a `VideoYuvPlane` texture.
        pub fn adopt_d3d11_plane(
            &self,
            cx: &mut Cx,
            texture: ID3D11Texture2D,
            srv: ID3D11ShaderResourceView,
            width: usize,
            height: usize,
        ) -> Result<(), String> {
            adopt_d3d11_plane_raw(
                &mut cx.textures,
                self.texture_id(),
                texture,
                srv,
                width,
                height,
            )
        }
    }

    pub(crate) fn adopt_d3d11_plane_raw(
        textures: &mut CxTexturePool,
        texture_id: TextureId,
        texture: ID3D11Texture2D,
        srv: ID3D11ShaderResourceView,
        width: usize,
        height: usize,
    ) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Err("adopt_d3d11_plane: width/height must be non-zero".into());
        }
        let cxtex = &mut textures[texture_id];
        cxtex.format = TextureFormat::VideoYuvPlane;
        cxtex.os.texture = Some(texture);
        cxtex.os.shader_resource_view = Some(srv);
        cxtex.alloc = Some(TextureAlloc {
            width,
            height,
            pixel: TexturePixel::VideoYuvPlane,
            category: TextureCategory::Video,
        });
        Ok(())
    }

    fn make_plane_srv(
        device: &ID3D11Device,
        texture: &ID3D11Texture2D,
        format: DXGI_FORMAT,
    ) -> Result<ID3D11ShaderResourceView, String> {
        let mut tex_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            texture.GetDesc(&mut tex_desc);
        }
        if tex_desc.Format != DXGI_FORMAT_NV12 {
            return Err(format!(
                "NV12 plane SRV: expected DXGI_FORMAT_NV12, got {:?}",
                tex_desc.Format
            ));
        }
        if (tex_desc.BindFlags & D3D11_BIND_SHADER_RESOURCE.0 as u32) == 0 {
            return Err(
                "NV12 plane SRV: texture missing D3D11_BIND_SHADER_RESOURCE (cannot sample)"
                    .into(),
            );
        }
        if tex_desc.ArraySize != 1 {
            return Err(format!(
                "NV12 plane SRV: expected ArraySize 1 for Texture2D shaders, got {}",
                tex_desc.ArraySize
            ));
        }

        let resource: ID3D11Resource = texture
            .cast()
            .map_err(|e| format!("NV12 plane SRV: cast failed: {e}"))?;

        // Must be TEXTURE2D — Makepad `texture_2d` shaders cannot sample Texture2DArray SRVs
        // (binding an array view caused STATUS_ACCESS_VIOLATION on first present).
        let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: format,
            ViewDimension: SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                },
            },
        };

        let mut out: Option<ID3D11ShaderResourceView> = None;
        unsafe {
            device
                .CreateShaderResourceView(&resource, Some(&desc), Some(&mut out))
                .map_err(|e| format!("NV12 plane SRV: CreateShaderResourceView failed: {e:?}"))?;
        }
        out.ok_or_else(|| "NV12 plane SRV: null".into())
    }

    fn ensure_nv12_present_tex(
        device: &ID3D11Device,
        slot: &mut Option<ID3D11Texture2D>,
        width: u32,
        height: u32,
    ) -> Result<ID3D11Texture2D, String> {
        if slot.is_none() {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut tex: Option<ID3D11Texture2D> = None;
            unsafe {
                device
                    .CreateTexture2D(&desc, None, Some(&mut tex))
                    .map_err(|e| format!("NV12 present: CreateTexture2D failed: {e:?}"))?;
            }
            *slot = Some(tex.ok_or_else(|| "NV12 present: null texture".to_string())?);
        }
        slot.as_ref()
            .cloned()
            .ok_or_else(|| "NV12 present: missing slot".to_string())
    }

    /// Copy one NV12 texture-array slice into two ArraySize=1 NV12 textures
    /// (separate Y + UV present targets).
    fn copy_nv12_slice_to_present(
        device: &ID3D11Device,
        src: &ID3D11Texture2D,
        array_slice: u32,
        width: u32,
        height: u32,
        present: &mut D3d11Nv12PresentCache,
    ) -> Result<(ID3D11Texture2D, ID3D11Texture2D), String> {
        let mut src_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            src.GetDesc(&mut src_desc);
        }
        if src_desc.Format != DXGI_FORMAT_NV12 {
            return Err(format!(
                "NV12 present: expected DXGI_FORMAT_NV12, got {:?}",
                src_desc.Format
            ));
        }
        if array_slice >= src_desc.ArraySize {
            return Err(format!(
                "NV12 present: array_slice {array_slice} >= ArraySize {}",
                src_desc.ArraySize
            ));
        }
        let copy_w = width.min(src_desc.Width);
        let copy_h = height.min(src_desc.Height);
        if copy_w == 0 || copy_h == 0 {
            return Err("NV12 present: empty copy region".into());
        }

        if present.width != copy_w || present.height != copy_h {
            present.y_slots = [None, None];
            present.uv_slots = [None, None];
            present.width = copy_w;
            present.height = copy_h;
            present.idx = 0;
        }

        let write = present.idx;
        present.idx = 1 - present.idx;

        let y_tex = ensure_nv12_present_tex(device, &mut present.y_slots[write], copy_w, copy_h)?;
        let uv_tex = ensure_nv12_present_tex(device, &mut present.uv_slots[write], copy_w, copy_h)?;

        let context = unsafe { device.GetImmediateContext() }
            .map_err(|e| format!("NV12 present: GetImmediateContext failed: {e:?}"))?;

        let src_res: ID3D11Resource = src
            .cast()
            .map_err(|e| format!("NV12 present: src cast failed: {e}"))?;
        let y_res: ID3D11Resource = y_tex
            .cast()
            .map_err(|e| format!("NV12 present: y cast failed: {e}"))?;
        let uv_res: ID3D11Resource = uv_tex
            .cast()
            .map_err(|e| format!("NV12 present: uv cast failed: {e}"))?;

        let src_box = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: copy_w,
            bottom: copy_h,
            back: 1,
        };
        let src_sub = array_slice;
        // Hold the shared media lock so present copies do not race decoder GPU work.
        with_media_d3d11_lock(|| unsafe {
            context.CopySubresourceRegion(
                &y_res,
                0,
                0,
                0,
                0,
                &src_res,
                src_sub,
                Some(&src_box as *const _),
            );
            context.CopySubresourceRegion(
                &uv_res,
                0,
                0,
                0,
                0,
                &src_res,
                src_sub,
                Some(&src_box as *const _),
            );
        });
        Ok((y_tex, uv_tex))
    }

    /// Adopt an NV12 `ID3D11Texture2D` as biplanar Y + UV into two YUV plane slots.
    pub(crate) fn adopt_d3d11_nv12_biplanar(
        device: &ID3D11Device,
        textures: &mut CxTexturePool,
        tex_y: TextureId,
        tex_u: TextureId,
        frame: &D3d11Nv12Frame,
        present_cache: &mut D3d11Nv12PresentCache,
    ) -> Result<(), String> {
        let w = frame.width as usize;
        let h = frame.height as usize;
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);

        let (y_tex, uv_tex) = copy_nv12_slice_to_present(
            device,
            &frame.texture,
            frame.array_slice,
            frame.width,
            frame.height,
            present_cache,
        )?;

        let y_srv = make_plane_srv(device, &y_tex, DXGI_FORMAT_R8_UNORM)?;
        let uv_srv = make_plane_srv(device, &uv_tex, DXGI_FORMAT_R8G8_UNORM)?;

        adopt_d3d11_plane_raw(textures, tex_y, y_tex, y_srv, w, h)?;
        adopt_d3d11_plane_raw(textures, tex_u, uv_tex, uv_srv, cw, ch)?;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub use windows_api::media_d3d11_device;

#[cfg(target_os = "windows")]
pub(crate) use windows_api::adopt_d3d11_nv12_biplanar;

#[cfg(target_os = "android")]
mod android_api {
    use super::*;
    use crate::os::gl_sys::LibGl;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    struct MediaOesEntry {
        surface: usize,
    }

    fn media_oes_map() -> &'static Mutex<HashMap<u32, MediaOesEntry>> {
        static MAP: OnceLock<Mutex<HashMap<u32, MediaOesEntry>>> = OnceLock::new();
        MAP.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// Most recently published entry — used when the decoder session is
    /// created immediately after [`publish_media_oes_surface`].
    static MEDIA_OES_LATEST_TEX: Mutex<Option<u32>> = Mutex::new(None);

    /// Publish a Java `Surface` global-ref pointer for OES zero-copy present.
    ///
    /// Ownership of the JNI global ref stays with the caller (OES bridge);
    /// the decoder only borrows the Surface for its lifetime.
    /// Entries are keyed by `oes_tex_id` so multiple players do not clobber
    /// each other; [`media_oes_surface`] still returns the latest publish for
    /// the prepare→open handshake.
    pub fn publish_media_oes_surface(surface: *mut std::ffi::c_void, oes_tex_id: u32) {
        if oes_tex_id == 0 {
            return;
        }
        let ptr = surface as usize;
        if ptr == 0 {
            return;
        }
        media_oes_map()
            .lock()
            .unwrap()
            .insert(oes_tex_id, MediaOesEntry { surface: ptr });
        *MEDIA_OES_LATEST_TEX.lock().unwrap() = Some(oes_tex_id);
    }

    /// Remove one published Surface after its session teardown.
    pub fn clear_media_oes_surface(oes_tex_id: u32) {
        if oes_tex_id == 0 {
            return;
        }
        let mut map = media_oes_map().lock().unwrap();
        map.remove(&oes_tex_id);
        let fallback = map.keys().next().copied();
        drop(map);
        let mut latest = MEDIA_OES_LATEST_TEX.lock().unwrap();
        if *latest == Some(oes_tex_id) {
            *latest = fallback;
        }
    }

    /// Clear all published Surfaces (process teardown / tests).
    pub fn clear_all_media_oes_surfaces() {
        media_oes_map().lock().unwrap().clear();
        *MEDIA_OES_LATEST_TEX.lock().unwrap() = None;
    }

    /// Latest Surface previously published via [`publish_media_oes_surface`].
    pub fn media_oes_surface() -> Option<*mut std::ffi::c_void> {
        let tex = (*MEDIA_OES_LATEST_TEX.lock().unwrap())?;
        media_oes_map()
            .lock()
            .unwrap()
            .get(&tex)
            .map(|e| e.surface as *mut std::ffi::c_void)
    }

    /// Surface for a specific OES texture id.
    pub fn media_oes_surface_for_tex(oes_tex_id: u32) -> Option<*mut std::ffi::c_void> {
        media_oes_map()
            .lock()
            .unwrap()
            .get(&oes_tex_id)
            .map(|e| e.surface as *mut std::ffi::c_void)
    }

    /// Latest OES texture id paired with [`media_oes_surface`].
    pub fn media_oes_tex_id() -> Option<u32> {
        *MEDIA_OES_LATEST_TEX.lock().unwrap()
    }

    impl Cx {
        /// Run `f` with Makepad's current GL function table.
        ///
        /// Call from the UI / render thread where Makepad's EGL context is
        /// current (e.g. while handling draw / video poll). Returns `None` if
        /// GL is not available yet.
        pub fn with_gl<R>(&mut self, f: impl FnOnce(&LibGl) -> R) -> Option<R> {
            Some(f(self.os.gl()))
        }
    }

    impl Texture {
        /// Adopt an externally owned `GL_TEXTURE_EXTERNAL_OES` texture id as
        /// [`TextureFormat::VideoExternal`] for `sample_video` shaders.
        ///
        /// Makepad will **not** delete `tex_id`. Configure wrap/filter on the
        /// OES target before sampling if the producer has not already.
        ///
        /// `width` / `height` are logical sizes for callers; internal alloc uses
        /// the `VideoExternal` sentinel (0×0) so draw-time setup does not
        /// recreate the texture.
        pub fn adopt_oes_texture(
            &self,
            cx: &mut Cx,
            tex_id: u32,
            width: usize,
            height: usize,
        ) -> Result<(), String> {
            if tex_id == 0 {
                return Err("adopt_oes_texture: tex_id must be non-zero".into());
            }
            if width == 0 || height == 0 {
                return Err("adopt_oes_texture: width/height must be non-zero".into());
            }

            let old_owned = {
                let cxtex = &mut cx.textures[self.texture_id()];
                if cxtex.os.gl_texture_owned {
                    cxtex.os.gl_texture.take().filter(|&old| old != tex_id)
                } else {
                    None
                }
            };
            if let Some(old) = old_owned {
                let _ = cx.with_gl(|gl| unsafe {
                    (gl.glDeleteTextures)(1, &old);
                });
            }

            let _ = (width, height);
            let cxtex = &mut cx.textures[self.texture_id()];
            cxtex.format = TextureFormat::VideoExternal;
            cxtex.os.gl_texture = Some(tex_id);
            cxtex.os.gl_texture_owned = false;
            cxtex.alloc = Some(TextureAlloc {
                width: 0,
                height: 0,
                pixel: TexturePixel::VideoExternal,
                category: TextureCategory::Video,
            });
            Ok(())
        }

        /// Adopt an externally owned `GL_TEXTURE_2D` id for ordinary
        /// `texture_2d` / `sample` shaders (e.g. after an OES→RGB blit).
        ///
        /// Makepad will **not** delete `tex_id`.
        pub fn adopt_gl_texture_2d(
            &self,
            cx: &mut Cx,
            tex_id: u32,
            width: usize,
            height: usize,
        ) -> Result<(), String> {
            if tex_id == 0 {
                return Err("adopt_gl_texture_2d: tex_id must be non-zero".into());
            }
            if width == 0 || height == 0 {
                return Err("adopt_gl_texture_2d: width/height must be non-zero".into());
            }

            let old_owned = {
                let cxtex = &mut cx.textures[self.texture_id()];
                if cxtex.os.gl_texture_owned {
                    cxtex.os.gl_texture.take().filter(|&old| old != tex_id)
                } else {
                    None
                }
            };
            if let Some(old) = old_owned {
                let _ = cx.with_gl(|gl| unsafe {
                    (gl.glDeleteTextures)(1, &old);
                });
            }

            let cxtex = &mut cx.textures[self.texture_id()];
            cxtex.format = TextureFormat::RenderBGRAu8 {
                size: crate::texture::TextureSize::Fixed { width, height },
                initial: false,
            };
            cxtex.os.gl_texture = Some(tex_id);
            cxtex.os.gl_texture_owned = false;
            cxtex.alloc = Some(TextureAlloc {
                width,
                height,
                pixel: TexturePixel::BGRAu8,
                category: TextureCategory::Render,
            });
            Ok(())
        }
    }
}

#[cfg(target_os = "android")]
pub use android_api::{
    clear_all_media_oes_surfaces, clear_media_oes_surface, media_oes_surface,
    media_oes_surface_for_tex, media_oes_tex_id, publish_media_oes_surface,
};
