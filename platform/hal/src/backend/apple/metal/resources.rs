//! Textures and buffers of the Metal backend: HAL formats to Metal formats, storage creation
//! (render targets, depth, sampled vec textures, mip chains, cube faces, IOSurface-backed shared
//! textures), buffers with copy-on-write against the GPU. Moved from os/apple/metal.rs
//! (`update_render_target`, `update_depth_stencil`, `update_vec_texture`, `update_shared_texture`,
//! `update_from_shared_handle`, `MetalBuffer`).

use crate::session::TextureObj;
use crate::types::*;
use makepad_apple_sys::*;

/// The Metal pixel format of a HAL format.
pub fn mtl_format(f: u32) -> Option<MTLPixelFormat> {
    Some(match f {
        format::RGBA8 => MTLPixelFormat::RGBA8Unorm,
        format::BGRA8 => MTLPixelFormat::BGRA8Unorm,
        format::R8 => MTLPixelFormat::R8Unorm,
        format::RG8 => MTLPixelFormat::RG8Unorm,
        format::RGBA16F => MTLPixelFormat::RGBA16Float,
        format::RGBA32F => MTLPixelFormat::RGBA32Float,
        // Metal's 24-bit depth is not on every GPU: both depth formats are 32-bit float
        format::DEPTH32F | format::DEPTH24_STENCIL8 => MTLPixelFormat::Depth32Float,
        format::R32F => MTLPixelFormat::R32Float,
        format::RG16F => MTLPixelFormat::RG16Float,
        format::R32UINT => MTLPixelFormat::R32Uint,
        format::BC7 => MTLPixelFormat::BC7_RGBAUnorm,
        format::BC5 => MTLPixelFormat::BC5_RGUnorm,
        format::ASTC4X4 => MTLPixelFormat::ASTC_4x4_LDR,
        _ => return None,
    })
}

/// The storage of a texture.
pub struct Tex {
    pub gen: u32,
    pub obj: RcObjcId,
    /// the IOSurface a shared texture is backed by (kept alive here)
    pub surface: Option<IOSurfaceRef>,
    pub surface_id: IOSurfaceID,
    /// the newest command buffer that bound it
    pub last_bound: u64,
    /// CoreVideo objects the texture is a view of (a decoded video frame): released with it
    pub _keep: Option<Box<dyn std::any::Any + Send>>,
}

// SAFETY: a Metal texture and a CF surface are usable from any thread
unsafe impl Send for Tex {}

impl Drop for Tex {
    fn drop(&mut self) {
        if let Some(s) = self.surface.take() {
            unsafe { CFRelease(s) };
        }
    }
}

fn nsnumber(v: u64) -> ObjcId {
    unsafe { msg_send![class!(NSNumber), numberWithUnsignedInteger: v] }
}

fn set(dict: ObjcId, key: &str, value: ObjcId) {
    unsafe {
        let _: () = msg_send![dict, setObject: value forKey: str_to_nsstring(key)];
    }
}

/// Creates the storage of a HAL texture. `label` names it in GPU tools.
pub fn create_texture(device: ObjcId, t: &TextureObj, gen: u32, label: &str) -> Result<Tex, String> {
    let Some(pixel) = mtl_format(t.format) else { return Err(format!("no Metal format for {}", format::name(t.format))) };
    let (render, sampled, shared) = (t.usage & usage::RENDER_TARGET != 0, t.usage & usage::SAMPLED != 0, t.usage & usage::SHARED != 0);
    let depth = format::is_depth(t.format);
    let descriptor = RcObjcId::from_owned(NonNull::new(unsafe { msg_send![class!(MTLTextureDescriptor), new] }).ok_or("no texture descriptor")?);
    let d = descriptor.as_id();
    let texture_type = if t.depth == 6 && !depth {
        MTLTextureType::Cube
    } else if t.depth > 1 {
        MTLTextureType::D2Array
    } else {
        MTLTextureType::D2
    };
    let mut usage_bits = 0u64;
    if render {
        usage_bits |= MTLTextureUsage::RenderTarget as u64;
        // a colour target is always readable (blits, later passes); depth only when sampled
        if !depth || sampled {
            usage_bits |= MTLTextureUsage::ShaderRead as u64;
        }
        // a mip chain of 8-bit (display-encoded) pixels is built through an sRGB view of the
        // target, so its levels average light
        if t.mips > 1 {
            usage_bits |= MTLTextureUsage::PixelFormatView as u64;
        }
    } else {
        usage_bits |= MTLTextureUsage::ShaderRead as u64;
    }
    unsafe {
        let _: () = msg_send![d, setTextureType: texture_type];
        let _: () = msg_send![d, setWidth: t.w as u64];
        let _: () = msg_send![d, setHeight: t.h as u64];
        let _: () = msg_send![d, setDepth: 1u64];
        if texture_type == MTLTextureType::D2Array {
            let _: () = msg_send![d, setArrayLength: t.depth as u64];
        }
        let _: () = msg_send![d, setMipmapLevelCount: t.mips as u64];
        let _: () = msg_send![d, setPixelFormat: pixel];
        let _: () = msg_send![d, setUsage: usage_bits];
    }
    let (texture, surface, surface_id): (ObjcId, Option<IOSurfaceRef>, IOSurfaceID) = if shared {
        if t.format != format::BGRA8 {
            return Err("a shared texture is BGRA8".into());
        }
        let props: ObjcId = unsafe { msg_send![class!(NSMutableDictionary), new] };
        set(props, "IOSurfaceWidth", nsnumber(t.w as u64));
        set(props, "IOSurfaceHeight", nsnumber(t.h as u64));
        set(props, "IOSurfaceBytesPerElement", nsnumber(4));
        // aligned to 16 bytes as Metal requires
        set(props, "IOSurfaceBytesPerRow", nsnumber(((t.w as u64) * 4 + 15) & !15));
        // 'BGRA'
        set(props, "IOSurfacePixelFormat", nsnumber(0x42475241));
        let yes: ObjcId = unsafe { msg_send![class!(NSNumber), numberWithBool: true] };
        // GL interop of the importers
        set(props, "IOSurfaceOpenGLESTextureCompatibility", yes);
        set(props, "IOSurfaceOpenGLESFBOCompatibility", yes);
        // the global id other processes look it up by
        set(props, "IOSurfaceIsGlobal", yes);
        let surface = unsafe { IOSurfaceCreate(props) };
        unsafe {
            let _: () = msg_send![props, release];
        }
        if surface.is_null() {
            return Err(format!("IOSurface {}x{} could not be created", t.w, t.h));
        }
        let id = unsafe { IOSurfaceGetID(surface) };
        unsafe {
            let _: () = msg_send![d, setStorageMode: MTLStorageMode::Private];
        }
        let texture: ObjcId = unsafe { msg_send![device, newTextureWithDescriptor: d iosurface: surface plane: 0u64] };
        if texture.is_null() {
            unsafe { CFRelease(surface) };
            return Err("a Metal texture over the IOSurface could not be created".into());
        }
        (texture, Some(surface), id)
    } else {
        // Private: written by the GPU (render targets) or by blits on the queue (uploads), so
        // hazard tracking orders every writer against every reader
        unsafe {
            let _: () = msg_send![d, setStorageMode: MTLStorageMode::Private];
        }
        let texture: ObjcId = unsafe { msg_send![device, newTextureWithDescriptor: d] };
        if texture.is_null() {
            return Err(format!("a {}x{} {} texture could not be allocated", t.w, t.h, format::name(t.format)));
        }
        (texture, None, 0)
    };
    unsafe {
        let _: () = msg_send![texture, setLabel: str_to_nsstring(label)];
    }
    Ok(Tex { gen, obj: RcObjcId::from_owned(NonNull::new(texture).expect("checked")), surface, surface_id, last_bound: 0, _keep: None })
}

/// A texture over an IOSurface another process (or the host) owns, found by its global id.
pub fn import_surface(device: ObjcId, id: IOSurfaceID, gen: u32) -> Option<(Tex, u32, u32, u32)> {
    let surface = unsafe { IOSurfaceLookup(id) };
    if surface.is_null() {
        return None;
    }
    let (w, h) = unsafe { (IOSurfaceGetWidth(surface) as u64, IOSurfaceGetHeight(surface) as u64) };
    let Some(descriptor) = NonNull::new(unsafe { msg_send![class!(MTLTextureDescriptor), new] }) else {
        unsafe { CFRelease(surface) };
        return None;
    };
    let descriptor = RcObjcId::from_owned(descriptor);
    let d = descriptor.as_id();
    unsafe {
        let _: () = msg_send![d, setTextureType: MTLTextureType::D2];
        let _: () = msg_send![d, setWidth: w];
        let _: () = msg_send![d, setHeight: h];
        let _: () = msg_send![d, setDepth: 1u64];
        let _: () = msg_send![d, setStorageMode: MTLStorageMode::Private];
        let _: () = msg_send![d, setUsage: (MTLTextureUsage::RenderTarget as u64 | MTLTextureUsage::ShaderRead as u64)];
        let _: () = msg_send![d, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
    }
    let texture: ObjcId = unsafe { msg_send![device, newTextureWithDescriptor: d iosurface: surface plane: 0u64] };
    let Some(texture) = NonNull::new(texture) else {
        unsafe { CFRelease(surface) };
        return None;
    };
    Some((Tex { gen, obj: RcObjcId::from_owned(texture), surface: Some(surface), surface_id: id, last_bound: 0, _keep: None }, w as u32, h as u32, format::BGRA8))
}

/// What the device holds for a texture.
pub fn allocated_size(object: ObjcId) -> u64 {
    unsafe { msg_send![object, allocatedSize] }
}

// ------------------------------------------------------------------ buffers

/// A Shared-storage buffer. A CPU write while an earlier command buffer may still read it goes
/// to a fresh buffer (copy on write), so the GPU never sees a half-written one.
pub struct Buf {
    pub gen: u32,
    pub buffer: RcObjcId,
    pub contents: *mut u8,
    pub size: u64,
    /// the newest command buffer that bound it
    pub last_bound: u64,
    pub evictable: bool,
}

// SAFETY: a shared Metal buffer's memory is usable from any thread
unsafe impl Send for Buf {}

impl Buf {
    pub fn new(device: ObjcId, gen: u32, size: u64) -> Option<Buf> {
        let alloc = size.max(16);
        let buffer = RcObjcId::from_owned(NonNull::new(unsafe { msg_send![device, newBufferWithLength: alloc options: MTLResourceOptions::StorageModeShared] })?);
        let contents: *mut u8 = unsafe { msg_send![buffer.as_id(), contents] };
        if contents.is_null() {
            return None;
        }
        Some(Buf { gen, buffer, contents, size, last_bound: 0, evictable: false })
    }

    pub fn allocated(&self) -> u64 {
        allocated_size(self.buffer.as_id())
    }
}

/// Whether the buffer may be written in place: no command buffer that bound it is still pending.
pub fn writable_in_place(b: &Buf, completed: u64) -> bool {
    b.last_bound <= completed
}

pub fn fill(b: &mut Buf, off: u64, data: &[u8]) {
    // SAFETY: the caller checked off + len <= size <= the allocation
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), b.contents.add(off as usize), data.len()) };
}

/// A fresh buffer holding the old one's bytes, for a write that cannot go in place.
pub fn copy_on_write(device: ObjcId, old: &Buf, covers_all: bool) -> Option<Buf> {
    let mut new = Buf::new(device, old.gen, old.size)?;
    if !covers_all {
        // SAFETY: both allocations are at least `size` bytes
        unsafe { std::ptr::copy_nonoverlapping(old.contents, new.contents, old.size as usize) };
    }
    new.evictable = old.evictable;
    Some(new)
}

/// Bytes of one row of a `w`-texel-wide rect (whole 4x4 blocks for the block formats), and the
/// rows the rect spans.
pub fn row_layout(fmt: u32, w: u32, h: u32) -> (u64, u64) {
    if format::is_block(fmt) {
        ((w as u64).div_ceil(4) * 16, (h as u64).div_ceil(4))
    } else {
        (w as u64 * format::texel_bytes(fmt), h as u64)
    }
}

/// An upload waiting for the next blit encoder.
pub struct Upload {
    pub texture: RcObjcId,
    pub staging: ObjcId,
    pub offset: u64,
    pub bytes_per_row: u64,
    pub rows: u64,
    pub size: [u64; 2],
    pub mip: u64,
    pub layer: u64,
    pub origin: [u64; 2],
}

/// Encodes uploads on one blit encoder of `cb`.
pub fn encode_uploads(cb: ObjcId, uploads: &mut Vec<Upload>) {
    if uploads.is_empty() {
        return;
    }
    unsafe {
        let blit: ObjcId = msg_send![cb, blitCommandEncoder];
        for u in uploads.drain(..) {
            let size = MTLSize { width: u.size[0], height: u.size[1], depth: 1 };
            let origin = MTLOrigin { x: u.origin[0], y: u.origin[1], z: 0 };
            let _: () = msg_send![blit,
                copyFromBuffer: u.staging
                sourceOffset: u.offset
                sourceBytesPerRow: u.bytes_per_row
                sourceBytesPerImage: u.bytes_per_row * u.rows
                sourceSize: size
                toTexture: u.texture.as_id()
                destinationSlice: u.layer
                destinationLevel: u.mip
                destinationOrigin: origin];
        }
        let _: () = msg_send![blit, endEncoding];
    }
}
