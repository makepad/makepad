use {
    super::apple_sys::*,
    crate::texture::{CxTexturePool, TextureAlloc, TextureCategory, TextureId, TexturePixel},
    std::{ffi::c_void, ptr::NonNull},
};

pub(crate) struct AppleYuvMetal {
    metal_device: ObjcId,
    texture_cache: CVMetalTextureCacheRef,
    cv_y_texture: CVMetalTextureRef,
    cv_uv_texture: CVMetalTextureRef,
}

impl AppleYuvMetal {
    pub(crate) fn new(metal_device: ObjcId, log_context: &str) -> Self {
        let texture_cache = unsafe {
            let mut cache: CVMetalTextureCacheRef = std::ptr::null_mut();
            let ret = CVMetalTextureCacheCreate(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                metal_device,
                std::ptr::null_mut(),
                &mut cache,
            );
            if ret != 0 {
                crate::log!(
                    "VIDEO: {} CVMetalTextureCacheCreate failed: {}",
                    log_context,
                    ret
                );
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

    pub(crate) fn has_biplanar_wrap(&self) -> bool {
        !self.cv_y_texture.is_null()
    }

    pub(crate) fn cleanup(&mut self) {
        unsafe {
            self.release_wrapped_textures();
            if !self.texture_cache.is_null() {
                CFRelease(self.texture_cache);
                self.texture_cache = std::ptr::null_mut();
            }
        }
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn wrap_nv12_cv_pixel_buffer(
        &mut self,
        textures: &mut CxTexturePool,
        tex_y_id: TextureId,
        tex_u_id: TextureId,
        tex_v_id: TextureId,
        pixel_buffer: CVPixelBufferRef,
        width: u32,
        height: u32,
    ) -> bool {
        if self.texture_cache.is_null() {
            return false;
        }

        let w = width as usize;
        let h = height as usize;
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);

        unsafe {
            self.release_wrapped_textures();

            let mut cv_y: CVMetalTextureRef = std::ptr::null_mut();
            let ret_y = CVMetalTextureCacheCreateTextureFromImage(
                std::ptr::null_mut(),
                self.texture_cache,
                pixel_buffer,
                std::ptr::null_mut(),
                MTLPixelFormat::R8Unorm as u64,
                w,
                h,
                0,
                &mut cv_y,
            );
            if ret_y != 0 || cv_y.is_null() {
                return false;
            }

            let mut cv_uv: CVMetalTextureRef = std::ptr::null_mut();
            let ret_uv = CVMetalTextureCacheCreateTextureFromImage(
                std::ptr::null_mut(),
                self.texture_cache,
                pixel_buffer,
                std::ptr::null_mut(),
                MTLPixelFormat::RG8Unorm as u64,
                cw,
                ch,
                1,
                &mut cv_uv,
            );
            if ret_uv != 0 || cv_uv.is_null() {
                CFRelease(cv_y);
                return false;
            }

            let mtl_y: ObjcId = CVMetalTextureGetTexture(cv_y);
            let mtl_uv: ObjcId = CVMetalTextureGetTexture(cv_uv);
            if mtl_y.is_null() || mtl_uv.is_null() {
                CFRelease(cv_y);
                CFRelease(cv_uv);
                return false;
            }

            let _: ObjcId = msg_send![mtl_y, retain];
            let _: ObjcId = msg_send![mtl_uv, retain];

            {
                let cxtex = &mut textures[tex_y_id];
                if let Some(old) = cxtex.os.texture.take() {
                    drop(old);
                }
                cxtex.os.texture = Some(RcObjcId::from_owned(NonNull::new(mtl_y).unwrap()));
                cxtex.alloc = Some(TextureAlloc {
                    width: w,
                    height: h,
                    pixel: TexturePixel::Ru8,
                    category: TextureCategory::Video,
                });
            }

            {
                let cxtex = &mut textures[tex_u_id];
                if let Some(old) = cxtex.os.texture.take() {
                    drop(old);
                }
                cxtex.os.texture = Some(RcObjcId::from_owned(NonNull::new(mtl_uv).unwrap()));
                cxtex.alloc = Some(TextureAlloc {
                    width: cw,
                    height: ch,
                    pixel: TexturePixel::RGu8,
                    category: TextureCategory::Video,
                });
            }

            Self::ensure_dummy_v_texture(self.metal_device, textures, tex_v_id);

            self.cv_y_texture = cv_y;
            self.cv_uv_texture = cv_uv;
            true
        }
    }

    pub(crate) fn upload_r8_plane(
        &self,
        textures: &mut CxTexturePool,
        texture_id: TextureId,
        data: &[u8],
        width: u32,
        height: u32,
    ) {
        let w = width as usize;
        let h = height as usize;
        if data.len() < w * h {
            return;
        }

        unsafe {
            let cxtexture = &mut textures[texture_id];
            let need_alloc = cxtexture
                .alloc
                .as_ref()
                .map_or(true, |alloc| alloc.width != w || alloc.height != h)
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
                bytesPerRow: width as u64
            ];
        }
    }

    unsafe fn release_wrapped_textures(&mut self) {
        if !self.cv_y_texture.is_null() {
            CFRelease(self.cv_y_texture);
            self.cv_y_texture = std::ptr::null_mut();
        }
        if !self.cv_uv_texture.is_null() {
            CFRelease(self.cv_uv_texture);
            self.cv_uv_texture = std::ptr::null_mut();
        }
    }

    #[cfg(target_os = "ios")]
    unsafe fn ensure_dummy_v_texture(
        metal_device: ObjcId,
        textures: &mut CxTexturePool,
        tex_v_id: TextureId,
    ) {
        let cxtex = &mut textures[tex_v_id];
        if cxtex.os.texture.is_some() {
            return;
        }

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
            return;
        }

        cxtex.os.texture = Some(RcObjcId::from_owned(NonNull::new(tex).unwrap()));
        cxtex.alloc = Some(TextureAlloc {
            width: 1,
            height: 1,
            pixel: TexturePixel::Ru8,
            category: TextureCategory::Video,
        });
    }
}

impl Drop for AppleYuvMetal {
    fn drop(&mut self) {
        self.cleanup();
    }
}
