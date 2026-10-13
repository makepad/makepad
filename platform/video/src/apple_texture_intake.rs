//! Pictures for the encoder straight from shared GPU textures (IOSurfaces):
//! a region of the source is copied on the GPU into an IOSurface-backed
//! BGRA buffer, which VideoToolbox converts on the GPU into an NV12 buffer
//! (the encoder's own format, 1.5 bytes a pixel, so a picture held waiting
//! costs what a CPU NV12 picture did). No CPU touches a pixel, and the
//! source is free again as soon as `copy` returns.

use {
    crate::VideoFileError,
    makepad_apple_sys::*,
    std::ffi::c_void,
};

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferPoolCreate(allocator: *const c_void, pool_attributes: *const c_void, buffer_attributes: *const c_void, pool_out: *mut *mut c_void) -> i32;
    fn CVPixelBufferPoolCreatePixelBuffer(allocator: *const c_void, pool: *mut c_void, buffer_out: *mut CVPixelBufferRef) -> i32;
    fn CVPixelBufferPoolRelease(pool: *mut c_void);
    fn CVPixelBufferGetIOSurface(buffer: CVPixelBufferRef) -> *mut c_void;
}

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    fn VTPixelTransferSessionCreate(allocator: *const c_void, session_out: *mut *mut c_void) -> i32;
    fn VTPixelTransferSessionTransferImage(session: *mut c_void, source: CVPixelBufferRef, destination: CVPixelBufferRef) -> i32;
    fn VTPixelTransferSessionInvalidate(session: *mut c_void);
}

const MTL_PIXEL_FORMAT_BGRA8_UNORM: u64 = 80;
const MTL_TEXTURE_TYPE_2D: u64 = 2;
const MTL_STORAGE_MODE_SHARED: u64 = 0;
const MTL_TEXTURE_USAGE_SHADER_READ: u64 = 1;
const MTL_TEXTURE_USAGE_RENDER_TARGET: u64 = 4;

pub(crate) struct TextureIntake {
    device: ObjcId,
    queue: ObjcId,
    /// BGRA buffers of `pool_size`, the size of the regions copied.
    pool: *mut c_void,
    pool_size: (u32, u32),
    /// NV12 buffers of the same size, and the GPU conversion into them.
    nv12_pool: *mut c_void,
    transfer: *mut c_void,
    /// Source surfaces wrapped once, by handle: (handle, iosurface, texture).
    sources: Vec<(u64, *mut c_void, ObjcId)>,
}

/// A +1 retained Metal texture over `iosurface`.
unsafe fn texture_for(device: ObjcId, iosurface: *mut c_void, width: usize, height: usize) -> ObjcId {
    unsafe {
        let descriptor: ObjcId = msg_send![class!(MTLTextureDescriptor), new];
        if descriptor.is_null() {
            return std::ptr::null_mut();
        }
        let () = msg_send![descriptor, setTextureType: MTL_TEXTURE_TYPE_2D];
        let () = msg_send![descriptor, setPixelFormat: MTL_PIXEL_FORMAT_BGRA8_UNORM];
        let () = msg_send![descriptor, setWidth: width as u64];
        let () = msg_send![descriptor, setHeight: height as u64];
        let () = msg_send![descriptor, setDepth: 1u64];
        let () = msg_send![descriptor, setMipmapLevelCount: 1u64];
        let () = msg_send![descriptor, setStorageMode: MTL_STORAGE_MODE_SHARED];
        let () = msg_send![descriptor, setUsage: (MTL_TEXTURE_USAGE_SHADER_READ | MTL_TEXTURE_USAGE_RENDER_TARGET)];
        let texture: ObjcId = msg_send![device, newTextureWithDescriptor: descriptor iosurface: iosurface plane: 0u64];
        let () = msg_send![descriptor, release];
        texture
    }
}

/// A pool of IOSurface-backed, Metal-compatible buffers.
unsafe fn make_pool(format: u32, width: u32, height: u32) -> Result<*mut c_void, VideoFileError> {
    unsafe {
        let number = |v: u32| -> ObjcId { msg_send![class!(NSNumber), numberWithUnsignedInt: v] };
        let empty: ObjcId = msg_send![class!(NSDictionary), dictionary];
        let yes: ObjcId = msg_send![class!(NSNumber), numberWithBool: YES];
        let keys = [
            kCVPixelBufferPixelFormatTypeKey as ObjcId,
            kCVPixelBufferWidthKey as ObjcId,
            kCVPixelBufferHeightKey as ObjcId,
            kCVPixelBufferIOSurfacePropertiesKey as ObjcId,
            kCVPixelBufferMetalCompatibilityKey as ObjcId,
        ];
        let values = [number(format), number(width), number(height), empty, yes];
        let attributes: ObjcId = msg_send![
            class!(NSDictionary),
            dictionaryWithObjects: values.as_ptr()
            forKeys: keys.as_ptr()
            count: keys.len()
        ];
        let mut pool = std::ptr::null_mut();
        let status = CVPixelBufferPoolCreate(std::ptr::null(), std::ptr::null(), attributes as *const c_void, &mut pool);
        if status != 0 || pool.is_null() {
            return Err(VideoFileError::with_code("CVPixelBufferPoolCreate", status));
        }
        Ok(pool)
    }
}

/// A picture ready for the encoder, in GPU memory (an NV12 IOSurface
/// `CVPixelBuffer`): made from a shared texture by [`GpuPictureMaker`] and
/// pushed with `VideoFileEncoder::push_gpu_picture`, any time later and on
/// any thread.
pub struct GpuPicture {
    pub(crate) buffer: CVPixelBufferRef,
    pub width: u32,
    pub height: u32,
}

unsafe impl Send for GpuPicture {}
unsafe impl Send for TextureIntake {}

impl Drop for GpuPicture {
    fn drop(&mut self) {
        unsafe { CVPixelBufferRelease(self.buffer) };
    }
}

impl TextureIntake {
    pub(crate) fn new() -> Result<TextureIntake, VideoFileError> {
        unsafe {
            let device = MTLCreateSystemDefaultDevice();
            if device.is_null() {
                return Err(VideoFileError::new("texture intake: no Metal device"));
            }
            let queue: ObjcId = msg_send![device, newCommandQueue];
            if queue.is_null() {
                return Err(VideoFileError::new("texture intake: no Metal command queue"));
            }
            let mut transfer = std::ptr::null_mut();
            let status = VTPixelTransferSessionCreate(std::ptr::null(), &mut transfer);
            if status != 0 || transfer.is_null() {
                return Err(VideoFileError::with_code("VTPixelTransferSessionCreate", status));
            }
            Ok(TextureIntake { device, queue, pool: std::ptr::null_mut(), pool_size: (0, 0), nv12_pool: std::ptr::null_mut(), transfer, sources: Vec::new() })
        }
    }

    unsafe fn ensure_pool(&mut self, width: u32, height: u32) -> Result<(), VideoFileError> {
        if !self.pool.is_null() && self.pool_size == (width, height) {
            return Ok(());
        }
        unsafe {
            for pool in [&mut self.pool, &mut self.nv12_pool] {
                if !pool.is_null() {
                    CVPixelBufferPoolRelease(*pool);
                    *pool = std::ptr::null_mut();
                }
            }
            self.pool = make_pool(kCVPixelFormatType_32BGRA, width, height)?;
            self.nv12_pool = make_pool(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, width, height)?;
            self.pool_size = (width, height);
        }
        Ok(())
    }

    unsafe fn source(&mut self, handle: u64) -> Result<(*mut c_void, ObjcId), VideoFileError> {
        if let Some((_, surface, texture)) = self.sources.iter().find(|(h, _, _)| *h == handle) {
            return Ok((*surface, *texture));
        }
        unsafe {
            let id = u32::try_from(handle).map_err(|_| VideoFileError::new("texture intake: not an IOSurface id"))?;
            let surface = IOSurfaceLookup(id) as *mut c_void;
            if surface.is_null() {
                return Err(VideoFileError::new("texture intake: the shared texture is gone"));
            }
            let texture = texture_for(self.device, surface, IOSurfaceGetWidth(surface as _), IOSurfaceGetHeight(surface as _));
            if texture.is_null() {
                CFRelease(surface as *const c_void);
                return Err(VideoFileError::new("texture intake: the shared texture cannot be read by Metal"));
            }
            // A handful of ring slots: keep the newest few wrapped.
            if self.sources.len() >= 16 {
                let (_, old_surface, old_texture) = self.sources.remove(0);
                let () = msg_send![old_texture, release];
                CFRelease(old_surface as *const c_void);
            }
            self.sources.push((handle, surface, texture));
            Ok((surface, texture))
        }
    }

    /// Copy `region` (`x, y, width, height`) of the shared texture `handle`
    /// into a fresh encoder buffer (+1 retained) and wait for the copy.
    pub(crate) unsafe fn copy(&mut self, handle: u64, region: [u32; 4]) -> Result<CVPixelBufferRef, VideoFileError> {
        let [x, y, width, height] = region;
        if width == 0 || height == 0 {
            return Err(VideoFileError::new("texture intake: empty region"));
        }
        unsafe {
            let (surface, source) = self.source(handle)?;
            let (source_width, source_height) = (IOSurfaceGetWidth(surface as _) as u32, IOSurfaceGetHeight(surface as _) as u32);
            if x + width > source_width || y + height > source_height {
                return Err(VideoFileError::new(format!(
                    "texture intake: region {x},{y} {width}x{height} outside the {source_width}x{source_height} texture"
                )));
            }
            self.ensure_pool(width, height)?;
            let mut buffer: CVPixelBufferRef = std::ptr::null_mut();
            let status = CVPixelBufferPoolCreatePixelBuffer(std::ptr::null(), self.pool, &mut buffer);
            if status != 0 || buffer.is_null() {
                return Err(VideoFileError::with_code("CVPixelBufferPoolCreatePixelBuffer", status));
            }
            let target_surface = CVPixelBufferGetIOSurface(buffer);
            let target = if target_surface.is_null() { std::ptr::null_mut() } else { texture_for(self.device, target_surface, width as usize, height as usize) };
            if target.is_null() {
                CVPixelBufferRelease(buffer);
                return Err(VideoFileError::new("texture intake: the encoder buffer has no Metal view"));
            }
            let command_buffer: ObjcId = msg_send![self.queue, commandBuffer];
            let encoder: ObjcId = msg_send![command_buffer, blitCommandEncoder];
            let () = msg_send![
                encoder,
                copyFromTexture: source
                sourceSlice: 0u64
                sourceLevel: 0u64
                sourceOrigin: MTLOrigin { x: x as u64, y: y as u64, z: 0 }
                sourceSize: MTLSize { width: width as u64, height: height as u64, depth: 1 }
                toTexture: target
                destinationSlice: 0u64
                destinationLevel: 0u64
                destinationOrigin: MTLOrigin { x: 0, y: 0, z: 0 }
            ];
            let () = msg_send![encoder, endEncoding];
            let () = msg_send![command_buffer, commit];
            let () = msg_send![command_buffer, waitUntilCompleted];
            let () = msg_send![target, release];
            Ok(buffer)
        }
    }

    /// `copy`, then converted on the GPU into an NV12 buffer of the same
    /// size: the picture an encoder takes, holding nothing of the source.
    pub(crate) unsafe fn picture(&mut self, handle: u64, region: [u32; 4]) -> Result<GpuPicture, VideoFileError> {
        unsafe {
            let bgra = self.copy(handle, region)?;
            let mut nv12: CVPixelBufferRef = std::ptr::null_mut();
            let status = CVPixelBufferPoolCreatePixelBuffer(std::ptr::null(), self.nv12_pool, &mut nv12);
            if status != 0 || nv12.is_null() {
                CVPixelBufferRelease(bgra);
                return Err(VideoFileError::with_code("CVPixelBufferPoolCreatePixelBuffer(nv12)", status));
            }
            let status = VTPixelTransferSessionTransferImage(self.transfer, bgra, nv12);
            CVPixelBufferRelease(bgra);
            if status != 0 {
                CVPixelBufferRelease(nv12);
                return Err(VideoFileError::with_code("VTPixelTransferSessionTransferImage(BGRA -> NV12)", status));
            }
            Ok(GpuPicture { buffer: nv12, width: region[2], height: region[3] })
        }
    }
}

impl Drop for TextureIntake {
    fn drop(&mut self) {
        unsafe {
            for (_, surface, texture) in self.sources.drain(..) {
                let () = msg_send![texture, release];
                CFRelease(surface as *const c_void);
            }
            for pool in [self.pool, self.nv12_pool] {
                if !pool.is_null() {
                    CVPixelBufferPoolRelease(pool);
                }
            }
            VTPixelTransferSessionInvalidate(self.transfer);
            CFRelease(self.transfer as *const c_void);
            let () = msg_send![self.queue, release];
            let () = msg_send![self.device, release];
        }
    }
}
