use {
    super::apple_sys::*,
    crate::texture::{CxTexturePool, TextureAlloc, TextureCategory, TextureId, TexturePixel},
    std::{ffi::c_void, ptr::NonNull},
};

/// CPU YUV upload helper for Apple platforms.
///
/// Biplanar NV12 zero-copy present goes through [`crate::gpu_texture`]
/// (`MetalNv12PresentCache` / `adopt_metal_nv12_biplanar`) instead of this type.
pub(crate) struct AppleYuvMetal {
    metal_device: ObjcId,
}

impl AppleYuvMetal {
    pub(crate) fn new(metal_device: ObjcId, _log_context: &str) -> Self {
        Self { metal_device }
    }

    pub(crate) fn cleanup(&mut self) {}

    pub(crate) fn upload_r8_plane(
        &self,
        textures: &mut CxTexturePool,
        texture_id: TextureId,
        data: &[u8],
        width: u32,
        height: u32,
        bytes_per_row: u32,
    ) {
        let w = width as usize;
        let h = height as usize;
        let bpr = bytes_per_row.max(width) as usize;
        if w == 0 || h == 0 {
            return;
        }
        // Require at least the last row's visible width, allowing padded strides.
        if data.len() < bpr * (h - 1) + w {
            return;
        }

        unsafe {
            let cxtexture = &mut textures[texture_id];
            let need_alloc = cxtexture
                .alloc
                .as_ref()
                .map_or(true, |alloc| {
                    alloc.width != w
                        || alloc.height != h
                        || !matches!(alloc.pixel, TexturePixel::Ru8)
                })
                || cxtexture.os.texture.is_none();

            if need_alloc {
                let descriptor: ObjcId = msg_send![class!(MTLTextureDescriptor), new];
                let _: () = msg_send![descriptor, setTextureType: MTLTextureType::D2];
                let _: () = msg_send![descriptor, setDepth: 1u64];
                let _: () = msg_send![descriptor, setStorageMode: MTLStorageMode::Shared];
                let _: () = msg_send![descriptor, setUsage: MTLTextureUsage::ShaderRead];
                let _: () = msg_send![descriptor, setWidth: width as u64];
                let _: () = msg_send![descriptor, setHeight: height as u64];
                let _: () = msg_send![descriptor, setPixelFormat: MTLPixelFormat::R8Unorm];
                let texture: ObjcId =
                    msg_send![self.metal_device, newTextureWithDescriptor: descriptor];
                let _: () = msg_send![descriptor, release];

                if texture.is_null() {
                    return;
                }

                cxtexture.os.texture = Some(RcObjcId::from_owned(NonNull::new(texture).unwrap()));
                cxtexture.alloc = Some(TextureAlloc {
                    width: w,
                    height: h,
                    pixel: TexturePixel::Ru8,
                    category: TextureCategory::Video,
                });
            }

            let region = MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize {
                    width: width as u64,
                    height: height as u64,
                    depth: 1,
                },
            };

            let texture = cxtexture.os.texture.as_ref().unwrap().as_id();
            let _: () = msg_send![
                texture,
                replaceRegion: region
                mipmapLevel: 0u64
                withBytes: data.as_ptr() as *const c_void
                bytesPerRow: bpr as u64
            ];
        }
    }
}

impl Drop for AppleYuvMetal {
    fn drop(&mut self) {
        self.cleanup();
    }
}
