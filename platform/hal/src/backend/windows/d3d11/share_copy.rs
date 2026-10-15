//! Shared textures outside any session (frame rings): a browser helper's paint into a slot a
//! host made shareable (`TextureShare`), and a slot read back for a still or a check. The
//! Windows counterpart of `dmabuf::copy`. Slots are named by their wire key (the NT name
//! `Local\makepad-hal-<pid>-<serial>`, as hosted images) and opened on one D3D11 device of this
//! process, made on first use on the adapter the HAL renders on. Every access to a slot takes
//! its keyed mutex (key 0) as the HAL does; the HAL never waits for it, so a writer holds it
//! only for the copy it issues.

use super::device::{hr, lock, Device};
use super::resources::shared_name;
use makepad_windows_sys::Win32::Foundation::HANDLE;
use makepad_windows_sys::Win32::Graphics::Direct3D11::*;
use makepad_windows_sys::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use makepad_windows_sys::Win32::Graphics::Dxgi::{IDXGIKeyedMutex, DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// How long a writer waits for a slot the HAL is sampling (it holds it for one pass).
const ACQUIRE_MS: u32 = 1000;

struct Slot {
    res: ID3D11Texture2D,
    mutex: IDXGIKeyedMutex,
    w: u32,
    h: u32,
}

// SAFETY: D3D11 resources of a multithread-protected device are free-threaded
unsafe impl Send for Slot {}
unsafe impl Sync for Slot {}

fn process_device() -> Result<Arc<Device>, String> {
    static DEV: OnceLock<Result<Arc<Device>, String>> = OnceLock::new();
    DEV.get_or_init(Device::new).clone()
}

fn slots() -> &'static Mutex<HashMap<u64, Arc<Slot>>> {
    static SLOTS: OnceLock<Mutex<HashMap<u64, Arc<Slot>>>> = OnceLock::new();
    SLOTS.get_or_init(Default::default)
}

fn device1(dev: &Device) -> Result<ID3D11Device1, String> {
    unsafe { ID3D11Device1::query(dev.device.as_raw()) }.map_err(|e| hr("ID3D11Device1", e))
}

/// The slot of wire key `key`, opened once.
fn slot(dev: &Device, key: u64) -> Result<Arc<Slot>, String> {
    if let Some(s) = lock(slots()).get(&key) {
        return Ok(s.clone());
    }
    let name = shared_name(key);
    let access = (DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0) as u32;
    let res = unsafe { device1(dev)?.OpenSharedResourceByName(makepad_windows_sys::core::PCWSTR(name.as_ptr()), access, &ID3D11Texture2D::IID) }
        .and_then(|raw| unsafe { ID3D11Texture2D::from_raw(raw) })
        .map_err(|e| hr(&format!("OpenSharedResourceByName {key:#x}"), e))?;
    let mut td = D3D11_TEXTURE2D_DESC::default();
    unsafe { res.GetDesc(&mut td) };
    if td.Format != DXGI_FORMAT_B8G8R8A8_UNORM || td.ArraySize != 1 || td.MipLevels != 1 {
        return Err(format!("shared texture {key:#x} is not single-level BGRA8"));
    }
    let mutex = unsafe { IDXGIKeyedMutex::query(res.as_raw()) }.map_err(|e| hr("IDXGIKeyedMutex", e))?;
    let s = Arc::new(Slot { res, mutex, w: td.Width, h: td.Height });
    lock(slots()).insert(key, s.clone());
    Ok(s)
}

/// Makes this process's device now (a recorder calls it before its first frame, so the capture
/// does not start with a device creation).
pub fn prepare() -> bool {
    process_device().is_ok()
}

/// Forgets an opened slot (its ring closed).
pub fn release(key: u64) {
    lock(slots()).remove(&key);
}

/// The slot's keyed mutex, held for the commands issued while this lives.
struct Held<'a>(&'a IDXGIKeyedMutex);

impl Drop for Held<'_> {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.ReleaseSync(0);
        }
    }
}

fn take(s: &Slot) -> Result<Held<'_>, String> {
    // WAIT_TIMEOUT / WAIT_ABANDONED are success codes: read the HRESULT itself
    let r = unsafe { (s.mutex.vtable().AcquireSync)(s.mutex.as_raw(), 0, ACQUIRE_MS) };
    if r.0 != 0 {
        return Err(format!("shared texture busy ({:#x})", r.0 as u32));
    }
    Ok(Held(&s.mutex))
}

/// Held from a staging copy to its map (the staging textures are shared by size).
fn read_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    lock(&LOCK)
}

/// A staging texture of `w` x `h`, kept for the next read of that size (a recorder reads every
/// frame of one size; a new one per frame cost a stall at the start of a capture).
fn staging(dev: &Device, w: u32, h: u32) -> Result<ID3D11Texture2D, String> {
    static CACHE: OnceLock<Mutex<Vec<([u32; 2], ID3D11Texture2D)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((_, t)) = lock(cache).iter().find(|(size, _)| *size == [w, h]) {
        return Ok(t.clone());
    }
    let t = make_staging(dev, w, h)?;
    let mut c = lock(cache);
    if c.len() >= 4 {
        c.remove(0);
    }
    c.push(([w, h], t.clone()));
    Ok(t)
}

fn make_staging(dev: &Device, w: u32, h: u32) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w.max(1),
        Height: h.max(1),
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut out = None;
    unsafe { dev.device.CreateTexture2D(&desc, None, Some(&mut out)) }.map_err(|e| hr("CreateTexture2D(staging)", e))?;
    out.ok_or_else(|| "CreateTexture2D(staging) returned nothing".into())
}

/// `w` x `h` BGRA rows of a staging texture (waits for the GPU).
fn map_rows(dev: &Device, t: &ID3D11Texture2D, w: u32, h: u32) -> Result<Vec<u8>, String> {
    let row = w as usize * 4;
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { dev.ctx.Map(t.as_ID3D11Resource(), 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.map_err(|e| hr("Map(staging)", e))?;
    let mut out = vec![0u8; row * h as usize];
    unsafe {
        for y in 0..h as usize {
            let src = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
            std::ptr::copy_nonoverlapping(src, out.as_mut_ptr().add(y * row), row);
        }
        dev.ctx.Unmap(t.as_ID3D11Resource(), 0);
    }
    Ok(out)
}

/// The BGRA texels of `src` at `points` (`[x, y]`; outside it reads as zero).
fn read_points(dev: &Device, src: &ID3D11Texture2D, w: u32, h: u32, points: &[[u32; 2]]) -> Result<Vec<[u8; 4]>, String> {
    if points.is_empty() {
        return Ok(Vec::new());
    }
    let _reading = read_lock();
    let st = staging(dev, points.len() as u32, 1)?;
    for (i, [x, y]) in points.iter().enumerate() {
        if *x < w && *y < h {
            let bx = D3D11_BOX { left: *x, top: *y, front: 0, right: x + 1, bottom: y + 1, back: 1 };
            unsafe { dev.ctx.CopySubresourceRegion(st.as_ID3D11Resource(), 0, i as u32, 0, 0, src.as_ID3D11Resource(), 0, Some(&bx)) };
        }
    }
    let row = map_rows(dev, &st, points.len() as u32, 1)?;
    Ok(points.iter().enumerate().map(|(i, [x, y])| if *x < w && *y < h { row[i * 4..i * 4 + 4].try_into().unwrap() } else { [0; 4] }).collect())
}

/// Copies another process's D3D11 shared texture (`src`, an NT handle valid for this call:
/// CEF's accelerated paint) into slot `key`, top-left aligned, clipped to both. Returns the BGRA
/// texels of `src` at `probes`.
pub fn copy(src: isize, key: u64, probes: &[[u32; 2]]) -> Result<Vec<[u8; 4]>, String> {
    let dev = process_device()?;
    let s = slot(&dev, key)?;
    let src = unsafe { device1(&dev)?.OpenSharedResource1(HANDLE(src as _), &ID3D11Texture2D::IID) }
        .and_then(|raw| unsafe { ID3D11Texture2D::from_raw(raw) })
        .map_err(|e| hr("OpenSharedResource1", e))?;
    let mut td = D3D11_TEXTURE2D_DESC::default();
    unsafe { src.GetDesc(&mut td) };
    let (w, h) = (td.Width.min(s.w), td.Height.min(s.h));
    {
        let _held = take(&s)?;
        let bx = D3D11_BOX { left: 0, top: 0, front: 0, right: w, bottom: h, back: 1 };
        unsafe { dev.ctx.CopySubresourceRegion(s.res.as_ID3D11Resource(), 0, 0, 0, 0, src.as_ID3D11Resource(), 0, Some(&bx)) };
    }
    read_points(&dev, &src, td.Width, td.Height, probes)
}

/// Uploads `width` x `height` BGRA rows (`stride` bytes apart) into slot `key`, clipped to it.
pub fn upload(key: u64, pixels: &[u8], width: u32, height: u32, stride: usize) -> Result<(), String> {
    let dev = process_device()?;
    let s = slot(&dev, key)?;
    let (w, h) = (width.min(s.w), height.min(s.h));
    if w == 0 || h == 0 {
        return Ok(());
    }
    if pixels.len() < stride * (h as usize - 1) + w as usize * 4 {
        return Err("upload has fewer bytes than its rows".into());
    }
    let _held = take(&s)?;
    let bx = D3D11_BOX { left: 0, top: 0, front: 0, right: w, bottom: h, back: 1 };
    unsafe {
        dev.ctx.UpdateSubresource(s.res.as_ID3D11Resource(), 0, Some(&bx), pixels.as_ptr() as _, stride as u32, 0);
        dev.ctx.Flush();
    }
    Ok(())
}

/// The device this module's textures live on (an encoder reading [`picture`]s opens on it).
pub fn device() -> Result<ID3D11Device, String> {
    Ok(process_device()?.device.clone())
}

/// `region` (x, y, width, height) of slot `key` copied on the GPU into a new BGRA texture of
/// the region's size on [`device`]: a picture the slot may be written over at once after.
pub fn picture(key: u64, region: [u32; 4]) -> Result<ID3D11Texture2D, String> {
    let dev = process_device()?;
    let s = slot(&dev, key)?;
    let [x, y, w, h] = region;
    if w == 0 || h == 0 || x.checked_add(w).is_none_or(|v| v > s.w) || y.checked_add(h).is_none_or(|v| v > s.h) {
        return Err(format!("region {region:?} outside the {}x{} slot", s.w, s.h));
    }
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        // what Media Foundation's video processor reads its input as
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut out = None;
    unsafe { dev.device.CreateTexture2D(&desc, None, Some(&mut out)) }.map_err(|e| hr("CreateTexture2D(picture)", e))?;
    let t: ID3D11Texture2D = out.ok_or("CreateTexture2D(picture) returned nothing")?;
    {
        let _held = take(&s)?;
        let bx = D3D11_BOX { left: x, top: y, front: 0, right: x + w, bottom: y + h, back: 1 };
        unsafe { dev.ctx.CopySubresourceRegion(t.as_ID3D11Resource(), 0, 0, 0, 0, s.res.as_ID3D11Resource(), 0, Some(&bx)) };
    }
    unsafe { dev.ctx.Flush() };
    Ok(t)
}


/// A video processor from `input` to `output` (sizes), kept for the next conversion of the
/// same sizes (a recorder converts every frame of one size).
struct Converter {
    sizes: [u32; 4],
    vdev: ID3D11VideoDevice,
    vctx: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
}

// SAFETY: video interfaces of a multithread-protected device
unsafe impl Send for Converter {}

fn converter(dev: &Device, input: [u32; 2], output: [u32; 2]) -> Result<std::sync::MutexGuard<'static, Option<Converter>>, String> {
    static CONVERTER: Mutex<Option<Converter>> = Mutex::new(None);
    let mut guard = lock(&CONVERTER);
    let sizes = [input[0], input[1], output[0], output[1]];
    if guard.as_ref().is_none_or(|c| c.sizes != sizes) {
        use makepad_windows_sys::Win32::Graphics::Dxgi::Common::DXGI_RATIONAL;
        let vdev = unsafe { ID3D11VideoDevice::query(dev.device.as_raw()) }.map_err(|e| hr("ID3D11VideoDevice", e))?;
        let vctx = unsafe { ID3D11VideoContext::query(dev.ctx.as_raw()) }.map_err(|e| hr("ID3D11VideoContext", e))?;
        let rate = DXGI_RATIONAL { Numerator: 60, Denominator: 1 };
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: input[0],
            InputHeight: input[1],
            OutputFrameRate: rate,
            OutputWidth: output[0],
            OutputHeight: output[1],
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_QUALITY,
        };
        let enumerator = unsafe { vdev.CreateVideoProcessorEnumerator(&desc) }.map_err(|e| hr("CreateVideoProcessorEnumerator", e))?;
        let processor = unsafe { vdev.CreateVideoProcessor(&enumerator, 0) }.map_err(|e| hr("CreateVideoProcessor", e))?;
        *guard = Some(Converter { sizes, vdev, vctx, enumerator, processor });
    }
    Ok(guard)
}

/// `picture` (a BGRA texture of [`device`], a [`picture`]) converted and scaled on the GPU into
/// a new NV12 texture of `width` x `height` (even): what a video encoder takes without a
/// conversion of its own. One video-processor blit, nothing else.
pub fn to_nv12(picture: &ID3D11Texture2D, width: u32, height: u32) -> Result<ID3D11Texture2D, String> {
    use makepad_windows_sys::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;
    let dev = process_device()?;
    let mut td = D3D11_TEXTURE2D_DESC::default();
    unsafe { picture.GetDesc(&mut td) };
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut out = None;
    unsafe { dev.device.CreateTexture2D(&desc, None, Some(&mut out)) }.map_err(|e| hr("CreateTexture2D(NV12)", e))?;
    let nv12: ID3D11Texture2D = out.ok_or("CreateTexture2D(NV12) returned nothing")?;
    let guard = converter(&dev, [td.Width, td.Height], [width, height])?;
    let c = guard.as_ref().expect("made above");
    let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC { FourCC: 0, ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D, Anonymous: Default::default() };
    let mut in_view = None;
    unsafe { c.vdev.CreateVideoProcessorInputView(picture.as_ID3D11Resource(), &c.enumerator, &in_desc, Some(&mut in_view)) }
        .map_err(|e| hr("CreateVideoProcessorInputView", e))?;
    let in_view: ID3D11VideoProcessorInputView = in_view.ok_or("CreateVideoProcessorInputView returned nothing")?;
    let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC { ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D, Anonymous: Default::default() };
    let mut out_view = None;
    unsafe { c.vdev.CreateVideoProcessorOutputView(nv12.as_ID3D11Resource(), &c.enumerator, &out_desc, Some(&mut out_view)) }
        .map_err(|e| hr("CreateVideoProcessorOutputView", e))?;
    let out_view: ID3D11VideoProcessorOutputView = out_view.ok_or("CreateVideoProcessorOutputView returned nothing")?;
    let stream = D3D11_VIDEO_PROCESSOR_STREAM { Enable: super::device::b(true), pInputSurface: in_view.as_raw(), ..Default::default() };
    unsafe {
        c.vctx.VideoProcessorSetStreamFrameFormat(&c.processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        c.vctx.VideoProcessorBlt(&c.processor, &out_view, 0, &[stream]).map_err(|e| hr("VideoProcessorBlt", e))?;
        dev.ctx.Flush();
    }
    Ok(nv12)
}

/// `region` (x, y, width, height) of slot `key` as BGRA rows (waits for the GPU).
pub fn read(key: u64, region: [u32; 4]) -> Result<Vec<u8>, String> {
    let dev = process_device()?;
    let s = slot(&dev, key)?;
    let [x, y, w, h] = region;
    if w == 0 || h == 0 || x.checked_add(w).is_none_or(|v| v > s.w) || y.checked_add(h).is_none_or(|v| v > s.h) {
        return Err(format!("region {region:?} outside the {}x{} slot", s.w, s.h));
    }
    let _reading = read_lock();
    let st = staging(&dev, w, h)?;
    {
        let _held = take(&s)?;
        let bx = D3D11_BOX { left: x, top: y, front: 0, right: x + w, bottom: y + h, back: 1 };
        unsafe { dev.ctx.CopySubresourceRegion(st.as_ID3D11Resource(), 0, 0, 0, 0, s.res.as_ID3D11Resource(), 0, Some(&bx)) };
    }
    map_rows(&dev, &st, w, h)
}

/// Reads `region` (x, y, width, height) of the shared texture of wire key `key` into `out` as
/// BGRA: the bytes written, or -1 (an unknown key, a region outside it, `out` too short, a GPU
/// error).
#[makepad_relax_api::relax_api]
pub fn hal_shared_read(key: u64, region: &[u32], out: &mut [u8]) -> i64 {
    let Ok(region) = <[u32; 4]>::try_from(region) else { return -1 };
    match read(key, region) {
        Ok(pixels) if pixels.len() <= out.len() => {
            out[..pixels.len()].copy_from_slice(&pixels);
            pixels.len() as i64
        }
        Ok(_) => -1,
        Err(error) => {
            eprintln!("[hal-d3d11] shared read: {error}");
            -1
        }
    }
}
