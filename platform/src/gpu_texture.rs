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
//! - Resources must come from **Makepad's** D3D11 device / GL / Metal context
//!   (`Cx::d3d11_device` / `Cx::with_gl` / `Cx::metal_device`). Cross-device
//!   sharing is out of scope.
//!
//! # Platforms
//!
//! | Platform | Entry points |
//! |----------|----------------|
//! | Windows  | [`Cx::d3d11_device`], [`Texture::adopt_d3d11_bgra`], [`Texture::adopt_d3d11_plane`] |
//! | Android  | [`Cx::with_gl`], [`Texture::adopt_oes_texture`], [`Texture::adopt_gl_texture_2d`] |
//! | Linux    | [`Cx::with_gl`], [`Texture::adopt_gl_texture_2d`], [`Texture::adopt_gl_video_external`], [`Texture::adopt_gl_r8_plane`], [`Texture::adopt_gl_rg8_plane`], [`crate::os::linux::linux_video_gpu`] (DMA-Buf NV12 / GLMemory) |
//! | macOS/iOS | [`Cx::metal_device`], [`Texture::adopt_metal_r8_plane`], [`Texture::adopt_metal_rg8_plane`], [`adopt_metal_nv12_biplanar`] |
//!
//! Other platforms can be added later; unsupported targets compile these
//! helpers out via `cfg`.

use std::{
    cell::{Cell, RefCell},
    sync::{Mutex, MutexGuard},
};

use crate::{
    texture::{Texture, TextureAlloc, TextureCategory, TextureFormat, TexturePixel},
    Cx,
};

#[cfg(any(
    target_os = "windows",
    all(target_os = "linux", not(any(target_env = "ohos", linux_direct))),
    target_os = "macos",
    target_os = "ios",
))]
use crate::texture::{CxTexturePool, TextureId};

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
/// Used when true zero-copy is unavailable (no SRV bind on decoder surfaces,
/// AMD Texture2DArray quirks, or `MAKEPAD_D3D11_NV12_BLIT`). Y and UV use
/// **separate** NV12 textures (each with its own plane SRV). Binding R8 + R8G8
/// views of the *same* NV12 resource in one draw TDRs some GPUs
/// (`DXGI_ERROR_DEVICE_REMOVED`).
#[cfg(target_os = "windows")]
#[derive(Default)]
pub struct D3d11Nv12PresentCache {
    y_slots: [Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>; 2],
    uv_slots: [Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>; 2],
    width: u32,
    height: u32,
    idx: usize,
    /// Last successful present used Texture2DArray SRVs on the decoder surface.
    pub zero_copy: bool,
}

/// Zero-copy OES frame for Android `sample_video` present.
#[cfg(target_os = "android")]
pub struct OesFrame {
    pub tex_id: u32,
    pub width: u32,
    pub height: u32,
    pub keep_alive: std::sync::Arc<dyn std::any::Any + Send + Sync>,
}

/// Zero-copy NV12 / biplanar frame from VideoToolbox (or any CVPixelBuffer
/// producer) for Metal present. `keep_alive` must outlive GPU sampling of the
/// adopted Metal textures (usually until the next frame replaces it).
#[cfg(all(any(target_os = "macos", target_os = "ios"), not(headless)))]
pub struct MetalNv12Frame {
    pub pixel_buffer: crate::os::apple::apple_sys::CVPixelBufferRef,
    pub width: u32,
    pub height: u32,
    pub matrix: crate::video_decode::yuv::YuvColorMatrix,
    /// `true` for `420f` (JPEG/full); `false` for `420v` (video/limited).
    pub full_range: bool,
    /// Owns the `CVPixelBuffer` (and/or source `AVFrame`). `pixel_buffer` is an
    /// alias into this keep-alive — do not `CVPixelBufferRelease` it separately.
    pub keep_alive: std::sync::Arc<dyn std::any::Any + Send + Sync>,
}

// CVPixelBuffer / IOSurface handoff across decode → UI threads (same process).
#[cfg(all(any(target_os = "macos", target_os = "ios"), not(headless)))]
unsafe impl Send for MetalNv12Frame {}
#[cfg(all(any(target_os = "macos", target_os = "ios"), not(headless)))]
unsafe impl Sync for MetalNv12Frame {}

#[cfg(all(any(target_os = "macos", target_os = "ios"), not(headless)))]
struct CvPixelBufferKeepAlive(crate::os::apple::apple_sys::CVPixelBufferRef);

#[cfg(all(any(target_os = "macos", target_os = "ios"), not(headless)))]
unsafe impl Send for CvPixelBufferKeepAlive {}
#[cfg(all(any(target_os = "macos", target_os = "ios"), not(headless)))]
unsafe impl Sync for CvPixelBufferKeepAlive {}

#[cfg(all(any(target_os = "macos", target_os = "ios"), not(headless)))]
impl Drop for CvPixelBufferKeepAlive {
    fn drop(&mut self) {
        use crate::os::apple::apple_sys::CVPixelBufferRelease;
        unsafe {
            if !self.0.is_null() {
                CVPixelBufferRelease(self.0);
                self.0 = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(all(any(target_os = "macos", target_os = "ios"), not(headless)))]
impl MetalNv12Frame {
    /// Take ownership of a biplanar NV12 `CVPixelBuffer` for Metal adopt.
    ///
    /// On success, `pixel_buffer` is released when the last `keep_alive` clone
    /// drops. On `None`, the caller still owns `pixel_buffer` and must release it.
    pub fn from_owned_cv_pixel_buffer(
        pixel_buffer: crate::os::apple::apple_sys::CVPixelBufferRef,
        width: u32,
        height: u32,
        matrix: crate::video_decode::yuv::YuvColorMatrix,
    ) -> Option<Self> {
        if pixel_buffer.is_null() || width == 0 || height == 0 {
            return None;
        }
        if !apple_api::cv_pixel_buffer_is_biplanar_nv12(pixel_buffer) {
            return None;
        }
        let full_range = apple_api::cv_pixel_buffer_is_full_range(pixel_buffer);
        Some(Self {
            pixel_buffer,
            width,
            height,
            matrix,
            full_range,
            keep_alive: std::sync::Arc::new(CvPixelBufferKeepAlive(pixel_buffer)),
        })
    }
}

/// Retains `CVMetalTextureCache` + last wrap refs so Metal textures stay valid
/// while the UI samples them.
#[cfg(all(any(target_os = "macos", target_os = "ios"), not(headless)))]
pub struct MetalNv12PresentCache {
    metal_device: crate::os::apple::apple_sys::ObjcId,
    texture_cache: crate::os::apple::apple_sys::CVMetalTextureCacheRef,
    cv_y_texture: crate::os::apple::apple_sys::CVMetalTextureRef,
    cv_uv_texture: crate::os::apple::apple_sys::CVMetalTextureRef,
}

#[cfg(all(any(target_os = "macos", target_os = "ios"), not(headless)))]
impl MetalNv12PresentCache {
    pub fn new(metal_device: crate::os::apple::apple_sys::ObjcId) -> Self {
        use crate::os::apple::apple_sys::*;
        let texture_cache = unsafe {
            let mut cache: CVMetalTextureCacheRef = std::ptr::null_mut();
            let status = CVMetalTextureCacheCreate(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                metal_device,
                std::ptr::null_mut(),
                &mut cache,
            );
            if status != 0 {
                crate::error!("CVMetalTextureCacheCreate failed: {status}");
                std::ptr::null_mut()
            } else {
                cache
            }
        };
        Self {
            metal_device,
            texture_cache,
            cv_y_texture: std::ptr::null_mut(),
            cv_uv_texture: std::ptr::null_mut(),
        }
    }

    pub fn is_ready(&self) -> bool {
        !self.texture_cache.is_null()
    }

    /// Drop retained `CVMetalTexture` wraps (call before releasing `keep_alive`
    /// or when switching to a CPU upload path).
    ///
    /// Prefer [`detach_metal_nv12_present`] when the texture pool still holds the
    /// adopted MTLTextures — releasing wraps alone can leave IOSurface-backed
    /// textures in the pool that CPU `replaceRegion` must not reuse.
    pub fn release_textures(&mut self) {
        unsafe {
            if !self.cv_y_texture.is_null() {
                crate::os::apple::apple_sys::CFRelease(self.cv_y_texture);
                self.cv_y_texture = std::ptr::null_mut();
            }
            if !self.cv_uv_texture.is_null() {
                crate::os::apple::apple_sys::CFRelease(self.cv_uv_texture);
                self.cv_uv_texture = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(all(any(target_os = "macos", target_os = "ios"), not(headless)))]
impl Drop for MetalNv12PresentCache {
    fn drop(&mut self) {
        self.release_textures();
        unsafe {
            if !self.texture_cache.is_null() {
                crate::os::apple::apple_sys::CFRelease(self.texture_cache);
                self.texture_cache = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_api {
    use super::*;
    use crate::os::windows::d3d11_texture;
    use windows::{
        core::Interface,
        Win32::Graphics::{
            Direct3D::D3D_SRV_DIMENSION,
            Direct3D11::{
                ID3D11Device, ID3D11Resource, ID3D11ShaderResourceView,
                ID3D11Texture2D, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
                D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC_0,
                D3D11_TEX2D_ARRAY_SRV, D3D11_TEX2D_SRV, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_DEFAULT,
            },
            Dxgi::Common::{
                DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_FORMAT_R8G8_UNORM, DXGI_FORMAT_R8_UNORM,
                DXGI_SAMPLE_DESC,
            },
        },
    };

    /// Makepad blit / CPU paths bind `Texture2D`.
    const SRV_DIMENSION_TEXTURE2D: D3D_SRV_DIMENSION = D3D_SRV_DIMENSION(4);
    /// D3D11VA decoder pools are Texture2DArray — true zero-copy uses this.
    const SRV_DIMENSION_TEXTURE2DARRAY: D3D_SRV_DIMENSION = D3D_SRV_DIMENSION(5);

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

        /// Withdraws the published device, so nothing hands a removed one to a decoder while
        /// the backend is rebuilding. Republished by `publish_d3d11_device_for_media` once
        /// there is a live device again.
        pub fn unpublish_d3d11_device_for_media(&self) {
            *MEDIA_D3D11_DEVICE.lock().unwrap() = None;
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
                        d3d11_texture::create_shader_resource_view(
                            &device,
                            &resource,
                            None,
                            Some(&mut out),
                        )
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
            d3d11_texture::texture2d_get_desc(texture, &mut tex_desc);
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
            d3d11_texture::create_shader_resource_view(
                device,
                &resource,
                Some(&desc),
                Some(&mut out),
            )
            .map_err(|e| format!("NV12 plane SRV: CreateShaderResourceView failed: {e:?}"))?;
        }
        out.ok_or_else(|| "NV12 plane SRV: null".into())
    }

    /// Plane SRV over one slice of a D3D11VA NV12 Texture2DArray (true zero-copy).
    /// Video widget samples these via `texture_2d_array` (`yuv_sample_mode ≈ 2`).
    fn make_plane_srv_array(
        device: &ID3D11Device,
        texture: &ID3D11Texture2D,
        format: DXGI_FORMAT,
        array_slice: u32,
    ) -> Result<ID3D11ShaderResourceView, String> {
        let mut tex_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            d3d11_texture::texture2d_get_desc(texture, &mut tex_desc);
        }
        if tex_desc.Format != DXGI_FORMAT_NV12 {
            return Err(format!(
                "NV12 array SRV: expected DXGI_FORMAT_NV12, got {:?}",
                tex_desc.Format
            ));
        }
        if (tex_desc.BindFlags & D3D11_BIND_SHADER_RESOURCE.0 as u32) == 0 {
            return Err(
                "NV12 array SRV: texture missing D3D11_BIND_SHADER_RESOURCE (cannot sample)"
                    .into(),
            );
        }
        if array_slice >= tex_desc.ArraySize {
            return Err(format!(
                "NV12 array SRV: array_slice {array_slice} >= ArraySize {}",
                tex_desc.ArraySize
            ));
        }

        let resource: ID3D11Resource = texture
            .cast()
            .map_err(|e| format!("NV12 array SRV: cast failed: {e}"))?;

        let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: format,
            ViewDimension: SRV_DIMENSION_TEXTURE2DARRAY,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2DArray: D3D11_TEX2D_ARRAY_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                    FirstArraySlice: array_slice,
                    ArraySize: 1,
                },
            },
        };

        let mut out: Option<ID3D11ShaderResourceView> = None;
        unsafe {
            d3d11_texture::create_shader_resource_view(
                device,
                &resource,
                Some(&desc),
                Some(&mut out),
            )
            .map_err(|e| format!("NV12 array SRV: CreateShaderResourceView failed: {e:?}"))?;
        }
        out.ok_or_else(|| "NV12 array SRV: null".into())
    }

    fn prefer_nv12_zero_copy(_device: &ID3D11Device) -> bool {
        if std::env::var_os("MAKEPAD_D3D11_NV12_BLIT").is_some() {
            return false;
        }
        // Default: try Texture2DArray zero-copy. Force blit with MAKEPAD_D3D11_NV12_BLIT=1
        // (some AMD drivers historically mishandled NV12 array sampling — VLC disabled it).
        true
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
                d3d11_texture::create_texture_2d(device, &desc, None, Some(&mut tex))
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
            d3d11_texture::texture2d_get_desc(src, &mut src_desc);
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

        let context = unsafe { d3d11_texture::device_get_immediate_context(device) }
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
            d3d11_texture::copy_subresource_region(
                &context,
                &y_res,
                0,
                0,
                0,
                0,
                &src_res,
                src_sub,
                Some(&src_box as *const _),
            );
            d3d11_texture::copy_subresource_region(
                &context,
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

    fn adopt_nv12_zero_copy(
        device: &ID3D11Device,
        textures: &mut CxTexturePool,
        tex_y: TextureId,
        tex_u: TextureId,
        frame: &D3d11Nv12Frame,
    ) -> Result<(), String> {
        let w = frame.width as usize;
        let h = frame.height as usize;
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);

        let y_srv = make_plane_srv_array(
            device,
            &frame.texture,
            DXGI_FORMAT_R8_UNORM,
            frame.array_slice,
        )?;
        let uv_srv = make_plane_srv_array(
            device,
            &frame.texture,
            DXGI_FORMAT_R8G8_UNORM,
            frame.array_slice,
        )?;

        adopt_d3d11_plane_raw(textures, tex_y, frame.texture.clone(), y_srv, w, h)?;
        adopt_d3d11_plane_raw(textures, tex_u, frame.texture.clone(), uv_srv, cw, ch)?;
        Ok(())
    }

    fn adopt_nv12_blit(
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

    /// Adopt an NV12 `ID3D11Texture2D` as biplanar Y + UV into two YUV plane slots.
    ///
    /// Prefers true zero-copy (`Texture2DArray` plane SRVs on the decoder surface).
    /// Falls back to GPU blit into ArraySize=1 present textures when ZC is disabled
    /// or fails. Returns `Ok(true)` for zero-copy, `Ok(false)` for blit.
    pub(crate) fn adopt_d3d11_nv12_biplanar(
        device: &ID3D11Device,
        textures: &mut CxTexturePool,
        tex_y: TextureId,
        tex_u: TextureId,
        frame: &D3d11Nv12Frame,
        present_cache: &mut D3d11Nv12PresentCache,
    ) -> Result<bool, String> {
        if prefer_nv12_zero_copy(device) {
            match adopt_nv12_zero_copy(device, textures, tex_y, tex_u, frame) {
                Ok(()) => {
                    present_cache.zero_copy = true;
                    static LOGGED: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        crate::log!(
                            "VIDEO: D3D11 NV12 true zero-copy (Texture2DArray plane SRVs)"
                        );
                    }
                    return Ok(true);
                }
                Err(err) => {
                    static LOGGED: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        crate::log!(
                            "VIDEO: D3D11 NV12 zero-copy unavailable ({err}); using GPU blit"
                        );
                    }
                }
            }
        }

        adopt_nv12_blit(device, textures, tex_y, tex_u, frame, present_cache)?;
        present_cache.zero_copy = false;
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            crate::log!("VIDEO: D3D11 NV12 present via GPU blit (ArraySize=1 copies)");
        }
        Ok(false)
    }

    /// Adopt an ArraySize=1 NV12 texture (e.g. Media Engine `TransferVideoFrame`
    /// destination) via **separate** Y/UV present targets. Same-resource dual
    /// plane SRVs can TDR some GPUs; always blit into the present cache.
    pub(crate) fn adopt_d3d11_nv12_texture2d_biplanar(
        device: &ID3D11Device,
        textures: &mut CxTexturePool,
        tex_y: TextureId,
        tex_u: TextureId,
        texture: &ID3D11Texture2D,
        width: u32,
        height: u32,
        present_cache: &mut D3d11Nv12PresentCache,
    ) -> Result<(), String> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            d3d11_texture::texture2d_get_desc(texture, &mut desc);
        }
        if desc.Format != DXGI_FORMAT_NV12 {
            return Err(format!(
                "MF NV12 adopt: expected DXGI_FORMAT_NV12, got {:?}",
                desc.Format
            ));
        }
        if desc.ArraySize != 1 {
            return Err(format!(
                "MF NV12 adopt: expected ArraySize 1, got {}",
                desc.ArraySize
            ));
        }
        let frame = D3d11Nv12Frame {
            texture: texture.clone(),
            array_slice: 0,
            width,
            height,
            matrix: crate::video_decode::yuv::YuvColorMatrix::BT709,
            keep_alive: std::sync::Arc::new(()),
        };
        adopt_nv12_blit(device, textures, tex_y, tex_u, &frame, present_cache)?;
        present_cache.zero_copy = false;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub use windows_api::media_d3d11_device;

#[cfg(target_os = "windows")]
pub(crate) use windows_api::{adopt_d3d11_nv12_biplanar, adopt_d3d11_nv12_texture2d_biplanar};

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

/// Linux desktop (X11 / Wayland) GL texture adopt hooks for app-owned video /
/// camera / effect surfaces. Same ownership rules as the module docs: borrowed
/// by default (`gl_texture_owned = false`).
#[cfg(all(target_os = "linux", not(any(target_env = "ohos", linux_direct))))]
mod linux_api {
    use super::*;
    use crate::os::gl_sys::LibGl;

    impl Cx {
        /// Run `f` with Makepad's current GL function table.
        ///
        /// Call from the UI / render thread where Makepad's EGL context is
        /// current (e.g. while handling draw / video poll). Returns `None` if
        /// GL is not available yet.
        pub fn with_gl<R>(&mut self, f: impl FnOnce(&LibGl) -> R) -> Option<R> {
            let opengl_cx = self.os.opengl_cx.as_ref()?;
            opengl_cx.make_current();
            Some(f(&opengl_cx.libgl))
        }
    }

    impl Texture {
        /// Adopt an externally owned `GL_TEXTURE_2D` id for ordinary
        /// `texture_2d` / `sample` shaders (RGBA/BGRA color).
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

        /// Adopt an externally owned `GL_TEXTURE_2D` as
        /// [`TextureFormat::VideoExternal`] for `sample_video` shaders
        /// (e.g. GStreamer GLMemory / app hard-decode RGBA).
        ///
        /// Makepad will **not** delete `tex_id`. Keep the producer buffer alive
        /// until the next successful adopt (plus one frame of present latency).
        ///
        /// `width` / `height` are logical sizes for callers; internal alloc uses
        /// the `VideoExternal` sentinel (0×0) so draw-time `setup_video_texture`
        /// does not fight a mismatched size.
        pub fn adopt_gl_video_external(
            &self,
            cx: &mut Cx,
            tex_id: u32,
            width: usize,
            height: usize,
        ) -> Result<(), String> {
            if tex_id == 0 {
                return Err("adopt_gl_video_external: tex_id must be non-zero".into());
            }
            if width == 0 || height == 0 {
                return Err("adopt_gl_video_external: width/height must be non-zero".into());
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

        /// Adopt an externally owned single-channel `GL_TEXTURE_2D` (R8) as a
        /// YUV plane slot ([`TextureFormat::VideoYuvPlane`]).
        ///
        /// Use for planar Y, U, or V. Makepad will **not** delete `tex_id`.
        pub fn adopt_gl_r8_plane(
            &self,
            cx: &mut Cx,
            tex_id: u32,
            width: usize,
            height: usize,
        ) -> Result<(), String> {
            adopt_gl_plane(self, cx, tex_id, width, height, TexturePixel::Ru8, "adopt_gl_r8_plane")
        }

        /// Adopt an externally owned RG8 `GL_TEXTURE_2D` as a biplanar UV plane
        /// for NV12-style present ([`TextureFormat::VideoYuvPlane`]).
        ///
        /// Makepad will **not** delete `tex_id`.
        pub fn adopt_gl_rg8_plane(
            &self,
            cx: &mut Cx,
            tex_id: u32,
            width: usize,
            height: usize,
        ) -> Result<(), String> {
            adopt_gl_plane(
                self,
                cx,
                tex_id,
                width,
                height,
                TexturePixel::RGu8,
                "adopt_gl_rg8_plane",
            )
        }
    }

    fn adopt_gl_plane(
        texture: &Texture,
        cx: &mut Cx,
        tex_id: u32,
        width: usize,
        height: usize,
        pixel: TexturePixel,
        api_name: &str,
    ) -> Result<(), String> {
        if tex_id == 0 {
            return Err(format!("{api_name}: tex_id must be non-zero"));
        }
        if width == 0 || height == 0 {
            return Err(format!("{api_name}: width/height must be non-zero"));
        }

        let old_owned = {
            let cxtex = &mut cx.textures[texture.texture_id()];
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

        adopt_gl_plane_raw(
            &mut cx.textures,
            texture.texture_id(),
            tex_id,
            width,
            height,
            pixel,
        )
    }

    /// Pool-level adopt used by [`Texture::adopt_gl_r8_plane`] /
    /// [`Texture::adopt_gl_rg8_plane`] (and internal video present paths).
    pub(crate) fn adopt_gl_plane_raw(
        textures: &mut CxTexturePool,
        texture_id: TextureId,
        tex_id: u32,
        width: usize,
        height: usize,
        pixel: TexturePixel,
    ) -> Result<(), String> {
        if tex_id == 0 {
            return Err("adopt_gl_plane: tex_id must be non-zero".into());
        }
        if width == 0 || height == 0 {
            return Err("adopt_gl_plane: width/height must be non-zero".into());
        }

        let cxtex = &mut textures[texture_id];
        cxtex.format = TextureFormat::VideoYuvPlane;
        cxtex.os.gl_texture = Some(tex_id);
        cxtex.os.gl_texture_owned = false;
        cxtex.alloc = Some(TextureAlloc {
            width,
            height,
            pixel,
            category: TextureCategory::Video,
        });
        Ok(())
    }
}

#[cfg(all(target_os = "linux", not(any(target_env = "ohos", linux_direct))))]
pub use crate::os::linux::linux_video_gpu::{
    present_dmabuf_nv12, present_gl_memory_rgba, LinuxDmabufNv12Frame, LinuxDmabufPlane,
    LinuxDmabufPresentCache, LinuxGlMemoryPresentCache, LinuxGlMemoryRgbaFrame,
    LinuxGlTextureTarget, DRM_FORMAT_MOD_LINEAR, DRM_FORMAT_NV12, DRM_FORMAT_R8, DRM_FORMAT_RG88,
};

#[cfg(all(any(target_os = "macos", target_os = "ios"), not(headless)))]
mod apple_api {
    use super::*;
    use crate::os::apple::apple_sys::*;
    use std::{ptr::NonNull, sync::Mutex};

    static MEDIA_METAL_DEVICE: Mutex<Option<usize>> = Mutex::new(None);

    impl Cx {
        /// Makepad's shared Metal device (same device used for UI rendering).
        pub fn metal_device(&self) -> Option<ObjcId> {
            #[cfg(target_os = "macos")]
            {
                self.os.metal_device
            }
            #[cfg(target_os = "ios")]
            {
                crate::os::apple::ios::ios_app::try_metal_device()
            }
        }

        /// Publish the UI Metal device for media plugins / hard-decode threads
        /// that cannot hold `&Cx` (same device as [`Cx::metal_device`]).
        ///
        /// Retains the device so the published pointer stays valid for the app
        /// lifetime even if callers only held an unretained reference.
        pub fn publish_metal_device_for_media(&self) {
            let Some(device) = self.metal_device() else {
                return;
            };
            if device.is_null() {
                return;
            }
            let mut slot = MEDIA_METAL_DEVICE.lock().unwrap();
            if *slot == Some(device as usize) {
                return;
            }
            unsafe {
                let _: ObjcId = msg_send![device, retain];
                if let Some(old) = *slot {
                    let old_id = old as ObjcId;
                    if !old_id.is_null() {
                        let _: () = msg_send![old_id, release];
                    }
                }
            }
            *slot = Some(device as usize);
        }
    }

    /// Metal device previously published via [`Cx::publish_metal_device_for_media`].
    pub fn media_metal_device() -> Option<ObjcId> {
        MEDIA_METAL_DEVICE
            .lock()
            .unwrap()
            .map(|p| p as ObjcId)
            .filter(|p| !p.is_null())
    }

    impl Texture {
        /// Adopt an externally owned `MTLTexture` as a YUV / R8 plane
        /// ([`TextureFormat::VideoYuvPlane`]).
        ///
        /// Makepad will **Release** the previous texture (if any) and **retain**
        /// `texture`. For biplanar UV planes use [`Texture::adopt_metal_rg8_plane`].
        pub fn adopt_metal_r8_plane(
            &self,
            cx: &mut Cx,
            texture: ObjcId,
            width: usize,
            height: usize,
        ) -> Result<(), String> {
            adopt_metal_texture_raw(
                &mut cx.textures,
                self.texture_id(),
                texture,
                width,
                height,
                TexturePixel::Ru8,
            )
        }

        /// Adopt an externally owned `MTLTexture` as an RG8 chroma plane
        /// (NV12 UV).
        pub fn adopt_metal_rg8_plane(
            &self,
            cx: &mut Cx,
            texture: ObjcId,
            width: usize,
            height: usize,
        ) -> Result<(), String> {
            adopt_metal_texture_raw(
                &mut cx.textures,
                self.texture_id(),
                texture,
                width,
                height,
                TexturePixel::RGu8,
            )
        }

        /// Adopt an externally owned BGRA `MTLTexture` as
        /// [`TextureFormat::VideoExternal`].
        pub fn adopt_metal_bgra(
            &self,
            cx: &mut Cx,
            texture: ObjcId,
            width: usize,
            height: usize,
        ) -> Result<(), String> {
            if texture.is_null() {
                return Err("adopt_metal_bgra: null MTLTexture".into());
            }
            if width == 0 || height == 0 {
                return Err("adopt_metal_bgra: width/height must be non-zero".into());
            }
            unsafe {
                let _: ObjcId = msg_send![texture, retain];
            }
            let cxtex = &mut cx.textures[self.texture_id()];
            if let Some(old) = cxtex.os.texture.take() {
                drop(old);
            }
            cxtex.os.texture = Some(RcObjcId::from_owned(
                NonNull::new(texture)
                    .ok_or_else(|| "adopt_metal_bgra: null after retain".to_string())?,
            ));
            cxtex.format = TextureFormat::VideoExternal;
            let _ = (width, height);
            cxtex.alloc = Some(TextureAlloc {
                width: 0,
                height: 0,
                pixel: TexturePixel::VideoExternal,
                category: TextureCategory::Video,
            });
            Ok(())
        }
    }

    pub(crate) fn adopt_metal_texture_raw(
        textures: &mut CxTexturePool,
        texture_id: TextureId,
        texture: ObjcId,
        width: usize,
        height: usize,
        pixel: TexturePixel,
    ) -> Result<(), String> {
        if texture.is_null() {
            return Err("adopt_metal_texture: null MTLTexture".into());
        }
        if width == 0 || height == 0 {
            return Err("adopt_metal_texture: width/height must be non-zero".into());
        }
        unsafe {
            let _: ObjcId = msg_send![texture, retain];
        }
        let cxtex = &mut textures[texture_id];
        if let Some(old) = cxtex.os.texture.take() {
            drop(old);
        }
        cxtex.os.texture = Some(RcObjcId::from_owned(
            NonNull::new(texture).ok_or_else(|| "adopt_metal_texture: null after retain".to_string())?,
        ));
        // Keep VideoYuvPlane format for YUV plane slots; otherwise mirror pixel.
        cxtex.format = match pixel {
            TexturePixel::VideoYuvPlane => TextureFormat::VideoYuvPlane,
            TexturePixel::VideoExternal => TextureFormat::VideoExternal,
            TexturePixel::Ru8 | TexturePixel::RGu8 => TextureFormat::VideoYuvPlane,
            _ => cxtex.format.clone(),
        };
        cxtex.alloc = Some(TextureAlloc {
            width,
            height,
            pixel,
            category: TextureCategory::Video,
        });
        Ok(())
    }

    fn ensure_dummy_v_texture(
        metal_device: ObjcId,
        textures: &mut CxTexturePool,
        tex_v: TextureId,
    ) -> Result<(), String> {
        let cxtex = &mut textures[tex_v];
        if cxtex.os.texture.is_some() {
            return Ok(());
        }
        unsafe {
            let descriptor: ObjcId = msg_send![class!(MTLTextureDescriptor), new];
            let _: () = msg_send![descriptor, setTextureType: MTLTextureType::D2];
            let _: () = msg_send![descriptor, setWidth: 1u64];
            let _: () = msg_send![descriptor, setHeight: 1u64];
            let _: () = msg_send![descriptor, setDepth: 1u64];
            let _: () = msg_send![descriptor, setPixelFormat: MTLPixelFormat::R8Unorm];
            let _: () = msg_send![descriptor, setStorageMode: MTLStorageMode::Shared];
            let _: () = msg_send![descriptor, setUsage: MTLTextureUsage::ShaderRead];
            let tex: ObjcId = msg_send![metal_device, newTextureWithDescriptor: descriptor];
            let _: () = msg_send![descriptor, release];
            if tex.is_null() {
                return Err("adopt_metal_nv12: dummy V texture alloc failed".into());
            }
            cxtex.os.texture = Some(RcObjcId::from_owned(NonNull::new(tex).unwrap()));
            cxtex.format = TextureFormat::VideoYuvPlane;
            cxtex.alloc = Some(TextureAlloc {
                width: 1,
                height: 1,
                pixel: TexturePixel::Ru8,
                category: TextureCategory::Video,
            });
        }
        Ok(())
    }

    /// Whether `pixel_buffer` is a biplanar 8-bit NV12-style buffer we can wrap.
    pub fn cv_pixel_buffer_is_biplanar_nv12(pixel_buffer: CVPixelBufferRef) -> bool {
        if pixel_buffer.is_null() {
            return false;
        }
        unsafe {
            if !CVPixelBufferIsPlanar(pixel_buffer) || CVPixelBufferGetPlaneCount(pixel_buffer) < 2
            {
                return false;
            }
            let fmt = CVPixelBufferGetPixelFormatType(pixel_buffer);
            fmt == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
                || fmt == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
        }
    }

    /// `true` when the buffer is full-range (`420f`); `false` for video-range (`420v`)
    /// or unknown formats.
    pub fn cv_pixel_buffer_is_full_range(pixel_buffer: CVPixelBufferRef) -> bool {
        if pixel_buffer.is_null() {
            return false;
        }
        unsafe {
            CVPixelBufferGetPixelFormatType(pixel_buffer)
                == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
        }
    }

    /// Wrap a biplanar NV12 `CVPixelBuffer` into Y (`R8`) + UV (`RG8`) Metal
    /// textures via `CVMetalTextureCache` and adopt them into `tex_y` / `tex_u`.
    ///
    /// `tex_v` gets a 1×1 dummy R8 texture (shaders still bind three planes;
    /// biplanar mode ignores V). Caller must keep `frame.keep_alive` alive until
    /// the next successful adopt.
    ///
    /// Lifetime note (Apple): keep the `CVMetalTextureRef` alive for as long as
    /// the derived `MTLTexture` may be sampled. We retain the MTLTexture into the
    /// texture pool **and** store the CVMetalTextureRefs in `cache` until the
    /// next adopt / `release_textures`.
    ///
    /// On failure the previous pool textures + cache wraps are left intact
    /// (partial adopts are rolled back).
    pub fn adopt_metal_nv12_biplanar(
        textures: &mut CxTexturePool,
        tex_y: TextureId,
        tex_u: TextureId,
        tex_v: TextureId,
        frame: &MetalNv12Frame,
        cache: &mut MetalNv12PresentCache,
    ) -> Result<(), String> {
        if cache.texture_cache.is_null() {
            return Err("adopt_metal_nv12: CVMetalTextureCache not ready".into());
        }
        if frame.pixel_buffer.is_null() {
            return Err("adopt_metal_nv12: null CVPixelBuffer".into());
        }
        if !cv_pixel_buffer_is_biplanar_nv12(frame.pixel_buffer) {
            return Err("adopt_metal_nv12: CVPixelBuffer is not biplanar 420".into());
        }

        // Prefer the buffer's plane sizes (handles coded vs display padding).
        let (w, h, cw, ch) = unsafe {
            let w = CVPixelBufferGetWidthOfPlane(frame.pixel_buffer, 0);
            let h = CVPixelBufferGetHeightOfPlane(frame.pixel_buffer, 0);
            let cw = CVPixelBufferGetWidthOfPlane(frame.pixel_buffer, 1);
            let ch = CVPixelBufferGetHeightOfPlane(frame.pixel_buffer, 1);
            (w, h, cw, ch)
        };
        if w == 0 || h == 0 || cw == 0 || ch == 0 {
            return Err("adopt_metal_nv12: empty CVPixelBuffer plane".into());
        }

        unsafe {
            let mut cv_y: CVMetalTextureRef = std::ptr::null_mut();
            let ret_y = CVMetalTextureCacheCreateTextureFromImage(
                std::ptr::null_mut(),
                cache.texture_cache,
                frame.pixel_buffer,
                std::ptr::null_mut(),
                MTLPixelFormat::R8Unorm as u64,
                w,
                h,
                0,
                &mut cv_y,
            );
            if ret_y != 0 || cv_y.is_null() {
                return Err(format!(
                    "adopt_metal_nv12: Y CVMetalTextureCacheCreateTextureFromImage failed: {ret_y}"
                ));
            }

            let mut cv_uv: CVMetalTextureRef = std::ptr::null_mut();
            let ret_uv = CVMetalTextureCacheCreateTextureFromImage(
                std::ptr::null_mut(),
                cache.texture_cache,
                frame.pixel_buffer,
                std::ptr::null_mut(),
                MTLPixelFormat::RG8Unorm as u64,
                cw,
                ch,
                1,
                &mut cv_uv,
            );
            if ret_uv != 0 || cv_uv.is_null() {
                CFRelease(cv_y);
                return Err(format!(
                    "adopt_metal_nv12: UV CVMetalTextureCacheCreateTextureFromImage failed: {ret_uv}"
                ));
            }

            let mtl_y: ObjcId = CVMetalTextureGetTexture(cv_y);
            let mtl_uv: ObjcId = CVMetalTextureGetTexture(cv_uv);
            if mtl_y.is_null() || mtl_uv.is_null() {
                CFRelease(cv_y);
                CFRelease(cv_uv);
                return Err("adopt_metal_nv12: CVMetalTextureGetTexture returned null".into());
            }

            // Snapshot previous pool slots so a partial adopt can roll back
            // without tearing down the last good zero-copy frame.
            let prev_y_tex = textures[tex_y].os.texture.clone();
            let prev_y_alloc = textures[tex_y].alloc.clone();
            let prev_y_fmt = textures[tex_y].format.clone();
            let prev_u_tex = textures[tex_u].os.texture.clone();
            let prev_u_alloc = textures[tex_u].alloc.clone();
            let prev_u_fmt = textures[tex_u].format.clone();

            let restore_prev = |textures: &mut CxTexturePool| {
                textures[tex_y].os.texture = prev_y_tex.clone();
                textures[tex_y].alloc = prev_y_alloc.clone();
                textures[tex_y].format = prev_y_fmt.clone();
                textures[tex_u].os.texture = prev_u_tex.clone();
                textures[tex_u].alloc = prev_u_alloc.clone();
                textures[tex_u].format = prev_u_fmt.clone();
            };

            // Adopt new MTLTextures first (drops previous RcObjcId from the pool
            // slots — clones above keep them alive for rollback), then release
            // the previous CVMetalTextureRefs — never the reverse.
            if let Err(err) =
                adopt_metal_texture_raw(textures, tex_y, mtl_y, w, h, TexturePixel::Ru8)
            {
                CFRelease(cv_y);
                CFRelease(cv_uv);
                return Err(err);
            }
            if let Err(err) =
                adopt_metal_texture_raw(textures, tex_u, mtl_uv, cw, ch, TexturePixel::RGu8)
            {
                restore_prev(textures);
                CFRelease(cv_y);
                CFRelease(cv_uv);
                return Err(err);
            }
            if let Err(err) = ensure_dummy_v_texture(cache.metal_device, textures, tex_v) {
                restore_prev(textures);
                CFRelease(cv_y);
                CFRelease(cv_uv);
                return Err(err);
            }

            let old_y = cache.cv_y_texture;
            let old_uv = cache.cv_uv_texture;
            cache.cv_y_texture = cv_y;
            cache.cv_uv_texture = cv_uv;
            if !old_y.is_null() {
                CFRelease(old_y);
            }
            if !old_uv.is_null() {
                CFRelease(old_uv);
            }
        }
        Ok(())
    }

    /// Detach biplanar NV12 present from the texture pool before dropping
    /// `CVMetalTexture` wraps / `CVPixelBuffer` keep-alive.
    ///
    /// Clears Y/U pool textures first so a subsequent CPU `replaceRegion`
    /// cannot reuse an IOSurface-backed MTLTexture after the buffer is released.
    pub fn detach_metal_nv12_present(
        textures: &mut CxTexturePool,
        tex_y: TextureId,
        tex_u: TextureId,
        cache: &mut MetalNv12PresentCache,
    ) {
        textures[tex_y].os.texture = None;
        textures[tex_u].os.texture = None;
        textures[tex_y].alloc = None;
        textures[tex_u].alloc = None;
        cache.release_textures();
    }
}

#[cfg(all(any(target_os = "macos", target_os = "ios"), not(headless)))]
pub use apple_api::{
    adopt_metal_nv12_biplanar, cv_pixel_buffer_is_biplanar_nv12, cv_pixel_buffer_is_full_range,
    detach_metal_nv12_present, media_metal_device,
};
