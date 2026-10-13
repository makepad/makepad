//! Textures and buffers: the D3D11 objects behind the session tables' ids. Moved from
//! os/windows/d3d11.rs (`CxTexture::update_vec_texture` / `update_render_target` /
//! `update_depth_stencil`, `D3d11Buffer`); views are made with the resource. A resource's COM
//! objects are released when the last `Arc` goes: a frame in flight keeps what it bound.

use super::device::{dxgi, hr, Device};
use crate::session::TextureObj;
use crate::types::*;
use makepad_windows_sys::Win32::Graphics::Direct3D::*;
use makepad_windows_sys::Win32::Graphics::Direct3D11::*;
use makepad_windows_sys::Win32::Graphics::Dxgi::Common::*;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// Shapes a texture is sampled as (`hlsl::HlslPipeline::textures`).
pub mod shape {
    pub const TEX2D: u32 = 0;
    pub const CUBE: u32 = 1;
    pub const ARRAY: u32 = 2;
    pub const DEPTH: u32 = 3;
    pub const DEPTH_ARRAY: u32 = 4;
    pub const UINT: u32 = 5;
}

/// A texture shared with another process (HOSTED windows): a named NT handle and its keyed mutex.
/// Both sides take key 0 around their GPU use: never waiting (a busy image is retried later).
pub struct Shared {
    /// the wire handle: (pid << 32 | serial), naming `Local\makepad-hal-<pid>-<serial>`
    pub key: u64,
    /// the owner's NT handle (keeps the name alive); none on the importing side
    handle: Option<makepad_windows_sys::Win32::Foundation::HANDLE>,
    pub mutex: makepad_windows_sys::Win32::Graphics::Dxgi::IDXGIKeyedMutex,
}

// SAFETY: the NT handle is a process-wide kernel object reference
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Drop for Shared {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            unsafe {
                let _ = makepad_windows_sys::Win32::Foundation::CloseHandle(h);
            }
        }
    }
}

/// The NT object name of a shared texture's wire handle.
pub fn shared_name(key: u64) -> Vec<u16> {
    format!("Local\\makepad-hal-{}-{}", key >> 32, key & 0xffff_ffff).encode_utf16().chain(std::iter::once(0)).collect()
}

impl Shared {
    /// Takes the image for this side's GPU use without waiting; false when the other side holds it.
    pub fn acquire(&self) -> bool {
        // WAIT_TIMEOUT / WAIT_ABANDONED are success codes: read the HRESULT itself
        let hr = unsafe { (self.mutex.vtable().AcquireSync)(self.mutex.as_raw(), 0, 0) };
        hr.0 == 0
    }
    /// Gives the image back after the GPU commands that use it were issued.
    pub fn release(&self) {
        unsafe {
            let _ = self.mutex.ReleaseSync(0);
        }
    }
}

pub struct Texture {
    pub dev: Arc<Device>,
    pub desc: TextureObj,
    pub res: ID3D11Texture2D,
    /// render-target views, one per layer (colour render targets)
    pub rtv: Vec<ID3D11RenderTargetView>,
    /// depth views, one per layer (depth targets)
    pub dsv: Vec<ID3D11DepthStencilView>,
    /// shader-resource views by shape, made on first use
    srv: Mutex<Vec<(u32, ID3D11ShaderResourceView)>>,
    pub bytes: u64,
    /// made with D3D11_RESOURCE_MISC_TEXTURECUBE
    pub cube: bool,
    pub shared: Option<Shared>,
}

impl Drop for Texture {
    fn drop(&mut self) {
        self.dev.allocated.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

impl Texture {
    pub fn new(dev: Arc<Device>, desc: TextureObj) -> Result<Arc<Texture>, String> {
        let fmt = dxgi(desc.format).ok_or_else(|| format!("format {} has no D3D11 equivalent", format::name(desc.format)))?;
        if desc.w == 0 || desc.h == 0 {
            return Err("zero-sized texture".into());
        }
        let depth = format::is_depth(desc.format);
        let target = desc.usage & usage::RENDER_TARGET != 0;
        let layers = desc.depth.max(1);
        // a six-layer sampled colour texture is a cube when the app samples it so; D3D11 needs
        // the flag at creation, and a cube-capable texture can still be viewed as an array
        let cube = layers == 6 && desc.w == desc.h && !depth;
        let mut bind = D3D11_BIND_SHADER_RESOURCE.0 as u32;
        if target {
            bind |= if depth { D3D11_BIND_DEPTH_STENCIL.0 as u32 } else { D3D11_BIND_RENDER_TARGET.0 as u32 };
        }
        let mips = desc.mips.max(1);
        let shared = desc.usage & usage::SHARED != 0;
        if shared && (depth || desc.format != format::BGRA8 || layers != 1 || mips != 1) {
            return Err("only single-level BGRA8 textures are shared across processes".into());
        }
        let mut misc = if cube { D3D11_RESOURCE_MISC_TEXTURECUBE.0 as u32 } else { 0 };
        if shared {
            misc |= (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32;
            bind |= D3D11_BIND_RENDER_TARGET.0 as u32;
        }
        if mips > 1 && target && !depth {
            misc |= D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32;
        }
        let td = D3D11_TEXTURE2D_DESC {
            Width: desc.w,
            Height: desc.h,
            MipLevels: mips,
            ArraySize: layers,
            Format: fmt.resource,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind,
            CPUAccessFlags: 0,
            MiscFlags: misc,
        };
        let mut res = None;
        unsafe { dev.device.CreateTexture2D(&td, None, Some(&mut res)) }
            .map_err(|e| hr(&format!("CreateTexture2D {}x{}x{} {} mips {}", desc.w, desc.h, layers, format::name(desc.format), mips), e))?;
        let res: ID3D11Texture2D = res.ok_or("CreateTexture2D returned nothing")?;
        let shared = if shared { Some(export(&res)?) } else { None };
        let target = target || shared.is_some();
        let mut rtv = Vec::new();
        let mut dsv = Vec::new();
        if target {
            for layer in 0..layers {
                unsafe {
                    if depth {
                        let mut d = D3D11_DEPTH_STENCIL_VIEW_DESC { Format: fmt.dsv, ViewDimension: D3D11_DSV_DIMENSION_TEXTURE2DARRAY, Flags: 0, Anonymous: Default::default() };
                        d.Anonymous.Texture2DArray = D3D11_TEX2D_ARRAY_DSV { MipSlice: 0, FirstArraySlice: layer, ArraySize: 1 };
                        let mut v = None;
                        dev.device.CreateDepthStencilView(res.as_ID3D11Resource(), Some(&d), Some(&mut v)).map_err(|e| hr("CreateDepthStencilView", e))?;
                        dsv.push(v.ok_or("CreateDepthStencilView returned nothing")?);
                    } else {
                        let mut d = D3D11_RENDER_TARGET_VIEW_DESC { Format: fmt.rtv, ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2DARRAY, Anonymous: Default::default() };
                        d.Anonymous.Texture2DArray = D3D11_TEX2D_ARRAY_RTV { MipSlice: 0, FirstArraySlice: layer, ArraySize: 1 };
                        let mut v = None;
                        dev.device.CreateRenderTargetView(res.as_ID3D11Resource(), Some(&d), Some(&mut v)).map_err(|e| hr("CreateRenderTargetView", e))?;
                        rtv.push(v.ok_or("CreateRenderTargetView returned nothing")?);
                    }
                }
            }
        }
        let bytes = crate::backend::exec::texture_bytes(desc.format, desc.w, desc.h, layers, mips);
        dev.allocated.fetch_add(bytes, Ordering::Relaxed);
        Ok(Arc::new(Texture { dev, desc, res, rtv, dsv, srv: Mutex::new(Vec::new()), bytes, cube, shared }))
    }

    /// Opens another process's shared texture by its wire handle.
    pub fn import(dev: Arc<Device>, key: u64) -> Result<Arc<Texture>, String> {
        let dev1 = unsafe { ID3D11Device1::query(dev.device.as_raw()) }.map_err(|e| hr("ID3D11Device1", e))?;
        let name = shared_name(key);
        let access = (makepad_windows_sys::Win32::Graphics::Dxgi::DXGI_SHARED_RESOURCE_READ.0 | makepad_windows_sys::Win32::Graphics::Dxgi::DXGI_SHARED_RESOURCE_WRITE.0) as u32;
        let res = unsafe { dev1.OpenSharedResourceByName(makepad_windows_sys::core::PCWSTR(name.as_ptr()), access, &ID3D11Texture2D::IID) }
            .and_then(|raw| unsafe { ID3D11Texture2D::from_raw(raw) })
            .map_err(|e| hr(&format!("OpenSharedResourceByName {key:#x}"), e))?;
        let mut td = D3D11_TEXTURE2D_DESC::default();
        unsafe { res.GetDesc(&mut td) };
        if td.Format != DXGI_FORMAT_B8G8R8A8_UNORM || td.ArraySize != 1 || td.MipLevels != 1 {
            return Err(format!("shared texture {key:#x} is not single-level BGRA8"));
        }
        let mutex = unsafe { makepad_windows_sys::Win32::Graphics::Dxgi::IDXGIKeyedMutex::query(res.as_raw()) }.map_err(|e| hr("IDXGIKeyedMutex", e))?;
        let mut rtv = Vec::new();
        let mut v = None;
        unsafe { dev.device.CreateRenderTargetView(res.as_ID3D11Resource(), None, Some(&mut v)) }.map_err(|e| hr("CreateRenderTargetView(shared)", e))?;
        rtv.push(v.ok_or("CreateRenderTargetView returned nothing")?);
        let desc = TextureObj { format: format::BGRA8, w: td.Width, h: td.Height, depth: 1, mips: 1, usage: usage::SAMPLED | usage::RENDER_TARGET,
            bytes: 0, imported: true, uploaded: 0, lent: None };
        Ok(Arc::new(Texture { dev, desc, res, rtv, dsv: Vec::new(), srv: Mutex::new(Vec::new()), bytes: 0, cube: false,
            shared: Some(Shared { key, handle: None, mutex }) }))
    }

    /// A texture made elsewhere (the media module's present textures): `res` is sampled as `fmt`
    /// at `w` x `h` (an NV12 resource viewed as R8 is its Y plane, as RG8 its UV plane).
    pub fn wrap(dev: Arc<Device>, res: ID3D11Texture2D, fmt: u32, w: u32, h: u32) -> Arc<Texture> {
        let desc = TextureObj { format: fmt, w, h, depth: 1, mips: 1, usage: usage::SAMPLED, bytes: format::level_bytes(fmt, w, h), imported: true, uploaded: 0, lent: None };
        let bytes = desc.bytes;
        dev.allocated.fetch_add(bytes, Ordering::Relaxed);
        Arc::new(Texture { dev, desc, res, rtv: Vec::new(), dsv: Vec::new(), srv: Mutex::new(Vec::new()), bytes, cube: false, shared: None })
    }

    /// The view a draw samples this texture by, for the shape the pipeline declares.
    pub fn view(&self, shape: u32) -> Result<ID3D11ShaderResourceView, String> {
        let mut views = super::device::lock(&self.srv);
        if let Some((_, v)) = views.iter().find(|(s, _)| *s == shape) {
            return Ok(v.clone());
        }
        let fmt = dxgi(self.desc.format).ok_or("format without a view")?;
        let mips = self.desc.mips.max(1);
        let layers = self.desc.depth.max(1);
        let mut d = D3D11_SHADER_RESOURCE_VIEW_DESC { Format: fmt.srv, ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D, Anonymous: Default::default() };
        match shape {
            shape::CUBE => {
                if !self.cube {
                    return Err(format!("a {}x{}x{} texture sampled as a cube", self.desc.w, self.desc.h, layers));
                }
                d.ViewDimension = D3D_SRV_DIMENSION_TEXTURECUBE;
                d.Anonymous.TextureCube = D3D11_TEXCUBE_SRV { MostDetailedMip: 0, MipLevels: mips };
            }
            shape::ARRAY | shape::DEPTH_ARRAY => {
                d.ViewDimension = D3D_SRV_DIMENSION_TEXTURE2DARRAY;
                d.Anonymous.Texture2DArray = D3D11_TEX2D_ARRAY_SRV { MostDetailedMip: 0, MipLevels: mips, FirstArraySlice: 0, ArraySize: layers };
            }
            _ => {
                d.Anonymous.Texture2D = D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: mips };
            }
        }
        let mut v = None;
        unsafe { self.dev.device.CreateShaderResourceView(self.res.as_ID3D11Resource(), Some(&d), Some(&mut v)) }.map_err(|e| hr("CreateShaderResourceView", e))?;
        let v = v.ok_or("CreateShaderResourceView returned nothing")?;
        views.push((shape, v.clone()));
        Ok(v)
    }

    /// The subresource index of `mip` of `layer`.
    pub fn sub(&self, mip: u32, layer: u32) -> u32 {
        mip + layer * self.desc.mips.max(1)
    }
}

pub struct Buffer {
    pub dev: Arc<Device>,
    pub buf: ID3D11Buffer,
    /// instance buffers: the word view the vertex shader reads records through
    pub words: Option<ID3D11ShaderResourceView>,
    pub size: u64,
    pub bytes: u64,
    pub kind: u16,
}

impl Drop for Buffer {
    fn drop(&mut self) {
        self.dev.allocated.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

impl Buffer {
    /// A retained buffer of `kind` (`buffer_kind`). Uniform buffers are constant buffers (sized up
    /// to 16 bytes); instance buffers are read by the vertex shader as words.
    pub fn new(dev: Arc<Device>, kind: u16, size: u64) -> Result<Arc<Buffer>, String> {
        let bytes = match kind {
            buffer_kind::UNIFORM => size.max(16).div_ceil(16) * 16,
            _ => size.max(4).div_ceil(4) * 4,
        };
        if bytes > u32::MAX as u64 {
            return Err(format!("a {bytes}-byte buffer exceeds D3D11's 4 GiB resource limit"));
        }
        let bind = match kind {
            buffer_kind::VERTEX => D3D11_BIND_VERTEX_BUFFER.0 as u32,
            buffer_kind::INDEX => D3D11_BIND_INDEX_BUFFER.0 as u32,
            buffer_kind::UNIFORM => D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            _ => D3D11_BIND_SHADER_RESOURCE.0 as u32,
        };
        let desc = D3D11_BUFFER_DESC { ByteWidth: bytes as u32, Usage: D3D11_USAGE_DEFAULT, BindFlags: bind, CPUAccessFlags: 0, MiscFlags: 0, StructureByteStride: 0 };
        let mut buf = None;
        unsafe { dev.device.CreateBuffer(&desc, None, Some(&mut buf)) }.map_err(|e| hr(&format!("CreateBuffer {bytes} bytes kind {kind}"), e))?;
        let buf: ID3D11Buffer = buf.ok_or("CreateBuffer returned nothing")?;
        let words = if kind == buffer_kind::INSTANCE { Some(word_view(&dev, &buf, (bytes / 4) as u32)?) } else { None };
        dev.allocated.fetch_add(bytes, Ordering::Relaxed);
        Ok(Arc::new(Buffer { dev, buf, words, size, bytes, kind }))
    }

    pub fn write(&self, off: u64, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        // constant buffers are updated whole (D3D11 updates no part of one)
        if self.kind == buffer_kind::UNIFORM {
            let mut whole = vec![0u8; self.bytes as usize];
            let end = (off as usize + data.len()).min(whole.len());
            whole[off as usize..end].copy_from_slice(&data[..end - off as usize]);
            unsafe { self.dev.ctx.UpdateSubresource(self.buf.as_ID3D11Resource(), 0, None, whole.as_ptr() as _, 0, 0) };
            return;
        }
        let bx = D3D11_BOX { left: off as u32, top: 0, front: 0, right: (off + data.len() as u64) as u32, bottom: 1, back: 1 };
        unsafe { self.dev.ctx.UpdateSubresource(self.buf.as_ID3D11Resource(), 0, Some(&bx), data.as_ptr() as _, 0, 0) };
    }
}

/// Makes a texture made SHARED_NTHANDLE | SHARED_KEYEDMUTEX nameable by another process.
fn export(res: &ID3D11Texture2D) -> Result<Shared, String> {
    use makepad_windows_sys::Win32::Graphics::Dxgi::*;
    use std::sync::atomic::AtomicU32;
    static SERIAL: AtomicU32 = AtomicU32::new(1);
    let key = (std::process::id() as u64) << 32 | SERIAL.fetch_add(1, Ordering::Relaxed) as u64;
    let name = shared_name(key);
    let r1 = unsafe { IDXGIResource1::query(res.as_raw()) }.map_err(|e| hr("IDXGIResource1", e))?;
    let access = (DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0) as u32;
    let handle = unsafe { r1.CreateSharedHandle(None, access, Some(makepad_windows_sys::core::PCWSTR(name.as_ptr()))) }.map_err(|e| hr("CreateSharedHandle", e))?;
    let mutex = unsafe { IDXGIKeyedMutex::query(res.as_raw()) }.map_err(|e| hr("IDXGIKeyedMutex", e))?;
    Ok(Shared { key, handle: Some(handle), mutex })
}

/// A `Buffer<uint>` view of `count` words of `buf`.
pub fn word_view(dev: &Device, buf: &ID3D11Buffer, count: u32) -> Result<ID3D11ShaderResourceView, String> {
    let mut d = D3D11_SHADER_RESOURCE_VIEW_DESC { Format: DXGI_FORMAT_R32_UINT, ViewDimension: D3D_SRV_DIMENSION_BUFFER, Anonymous: Default::default() };
    d.Anonymous.Buffer = D3D11_BUFFER_SRV { Anonymous1: D3D11_BUFFER_SRV_0 { FirstElement: 0 }, Anonymous2: D3D11_BUFFER_SRV_1 { NumElements: count } };
    let mut v = None;
    unsafe { dev.device.CreateShaderResourceView(buf.as_ID3D11Resource(), Some(&d), Some(&mut v)) }.map_err(|e| hr("CreateShaderResourceView(words)", e))?;
    v.ok_or_else(|| "CreateShaderResourceView(words) returned nothing".into())
}

/// One frame's transient bytes: inline instances (a word buffer) and uniform blocks (a
/// constant buffer bound by 256-byte windows, D3D11.1). Grown when a frame needs more; written
/// with one map per frame.
pub struct Arena {
    pub buf: ID3D11Buffer,
    pub words: Option<ID3D11ShaderResourceView>,
    pub size: u64,
    constant: bool,
}

impl Arena {
    pub fn new(dev: &Device, size: u64, constant: bool) -> Result<Arena, String> {
        let size = size.max(64 * 1024).div_ceil(256) * 256;
        if size > u32::MAX as u64 {
            return Err(format!("a {size}-byte frame arena exceeds D3D11's resource limit"));
        }
        let bind = if constant { D3D11_BIND_CONSTANT_BUFFER.0 as u32 } else { D3D11_BIND_SHADER_RESOURCE.0 as u32 };
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: size as u32,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: bind,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let mut buf = None;
        unsafe { dev.device.CreateBuffer(&desc, None, Some(&mut buf)) }.map_err(|e| hr("CreateBuffer(frame arena)", e))?;
        let buf: ID3D11Buffer = buf.ok_or("CreateBuffer(frame arena) returned nothing")?;
        let words = if constant { None } else { Some(word_view(dev, &buf, (size / 4) as u32)?) };
        Ok(Arena { buf, words, size, constant })
    }

    /// Replaces the arena's contents with `bytes` (a discard map: frames in flight keep theirs).
    pub fn fill(&mut self, dev: &Device, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() as u64 > self.size {
            *self = Arena::new(dev, (bytes.len() as u64).next_power_of_two(), self.constant)?;
        }
        if bytes.is_empty() {
            return Ok(());
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            dev.ctx.Map(self.buf.as_ID3D11Resource(), 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped)).map_err(|e| hr("Map(frame arena)", e))?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.pData as *mut u8, bytes.len());
            dev.ctx.Unmap(self.buf.as_ID3D11Resource(), 0);
        }
        Ok(())
    }
}
