use {
    super::apple_sys::*,
    super::apple_video_playback::AppleVideoPlayer,
    super::dav1d_apple_allocator,
    crate::{
        event::video_playback::VideoSource,
        makepad_live_id::LiveId,
        texture::{CxTexturePool, TextureAlloc, TextureCategory, TextureId, TexturePixel},
        video_decode::software_av1::SoftwareAv1Player,
        video_decode::yuv::{YuvPlaneData, YuvColorMatrix},
    },
    std::{ffi::c_void, ptr::NonNull},
};

pub struct AppleUnifiedVideoPlayer {
    pub(crate) video_id: LiveId,
    texture_id: TextureId,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
    yuv_matrix: f32,
    metal_device: ObjcId,
    source: VideoSource,
    autoplay: bool,
    is_looping: bool,
    mode: ApplePlayerMode,
    /// Consecutive poll_frame calls with no pixel buffer after the native player is playing.
    /// Used to detect codecs (e.g. AV1) that AVPlayer can play but not expose via video output.
    null_frame_count: u32,
    /// CVMetalTextureCache for zero-copy IOSurface wrapping.
    texture_cache: CVMetalTextureCacheRef,
    /// Current CVMetalTexture refs (released each frame).
    cv_y_texture: CVMetalTextureRef,
    cv_uv_texture: CVMetalTextureRef,
    zero_copy_logged_ok: bool,
    zero_copy_fail_count: u32,
}

enum ApplePlayerMode {
    Native(AppleVideoPlayer),
    Software(SoftwareAv1Player),
}

impl AppleUnifiedVideoPlayer {
    pub fn new(
        metal_device: ObjcId,
        video_id: LiveId,
        texture_id: TextureId,
        tex_y_id: TextureId,
        tex_u_id: TextureId,
        tex_v_id: TextureId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Self {
        // Create CVMetalTextureCache for zero-copy IOSurface wrapping
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
                crate::log!("VIDEO: CVMetalTextureCacheCreate failed: {} device={:p}", ret, metal_device);
                std::ptr::null_mut()
            } else {
                crate::log!("VIDEO: CVMetalTextureCacheCreate OK cache={:p} device={:p}", cache, metal_device);
                cache
            }
        };

        let force_software = std::env::var_os("MAKEPAD_FORCE_SOFTWARE_AV1").is_some();
        let mode = if force_software {
            crate::log!("VIDEO: MAKEPAD_FORCE_SOFTWARE_AV1 set, using software AV1 decoder");
            let allocator = dav1d_apple_allocator::create_cv_pic_allocator();
            ApplePlayerMode::Software(SoftwareAv1Player::new_with_allocator(
                video_id,
                texture_id,
                source.clone(),
                autoplay,
                is_looping,
                allocator,
            ))
        } else {
            ApplePlayerMode::Native(AppleVideoPlayer::new(
                metal_device,
                video_id,
                texture_id,
                source.clone(),
                autoplay,
                is_looping,
            ))
        };

        Self {
            video_id,
            texture_id,
            tex_y_id,
            tex_u_id,
            tex_v_id,
            yuv_matrix: YuvColorMatrix::BT709.as_f32(),
            metal_device,
            source,
            autoplay,
            is_looping,
            mode,
            null_frame_count: 0,
            texture_cache,
            cv_y_texture: std::ptr::null_mut(),
            cv_uv_texture: std::ptr::null_mut(),
            zero_copy_logged_ok: false,
            zero_copy_fail_count: 0,
        }
    }

    fn switch_to_software(&mut self, reason: &str) {
        crate::log!(
            "VIDEO: Apple native playback failed, falling back to software AV1 decoder: {}",
            reason
        );
        let allocator = dav1d_apple_allocator::create_cv_pic_allocator();
        self.mode = ApplePlayerMode::Software(SoftwareAv1Player::new_with_allocator(
            self.video_id,
            self.texture_id,
            self.source.clone(),
            self.autoplay,
            self.is_looping,
            allocator,
        ));
        self.zero_copy_logged_ok = false;
        self.zero_copy_fail_count = 0;
    }

    pub fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
        let result = match &mut self.mode {
            ApplePlayerMode::Native(player) => match player.check_prepared() {
                Some(Err(err)) => {
                    self.switch_to_software(&err);
                    if let ApplePlayerMode::Software(software) = &mut self.mode {
                        software.check_prepared()
                    } else {
                        Some(Err(err))
                    }
                }
                other => other,
            },
            ApplePlayerMode::Software(player) => player.check_prepared(),
        };
        result
    }

    pub fn poll_frame(&mut self, textures: &mut CxTexturePool) -> bool {
        match &mut self.mode {
            ApplePlayerMode::Native(player) => {
                let got_frame = player.poll_frame(textures);
                if got_frame {
                    self.null_frame_count = 0;
                    return true;
                }
                // Track consecutive null frames. AVPlayer may report ready and
                // advance time but never produce pixel buffers for codecs it
                // cannot expose via AVPlayerItemVideoOutput (e.g. AV1 on some
                // macOS versions). After enough failed polls, fall back to
                // software decoding.
                self.null_frame_count += 1;
                if self.null_frame_count >= 60 {
                    self.switch_to_software(
                        "native player produced no frames after 60 polls",
                    );
                    self.null_frame_count = 0;
                } else {
                    return false;
                }
                // Fall through to software path after switch
                self.poll_software_frame(textures)
            }
            ApplePlayerMode::Software(_) => {
                self.poll_software_frame(textures)
            }
        }
    }

    fn poll_software_frame(&mut self, textures: &mut CxTexturePool) -> bool {
        let (mut decoded_pic, yuv_planes) = {
            let player = match &mut self.mode {
                ApplePlayerMode::Software(p) => p,
                _ => return false,
            };

            if !player.poll_frame() {
                return false;
            }

            (player.take_decoded_picture(), player.take_yuv_frame())
        };

        if let Some(planes) = yuv_planes.as_ref() {
            self.yuv_matrix = planes.matrix.as_f32();
        }

        // Zero-copy path: wrap custom-allocator CVPixelBuffer planes as Metal textures.
        if !self.texture_cache.is_null() {
            if let Some(mut pic) = decoded_pic.take() {
                let cv_pixel_buffer = unsafe { dav1d_apple_allocator::finalize_nv12(&mut pic.pic) };
                if !cv_pixel_buffer.is_null() {
                    if self.zero_copy_fail_count == 0 && !self.zero_copy_logged_ok {
                        // Log CVPixelBuffer properties once from main thread
                        unsafe {
                            let plane_count = CVPixelBufferGetPlaneCount(cv_pixel_buffer);
                            let y_stride = CVPixelBufferGetBytesPerRowOfPlane(cv_pixel_buffer, 0);
                            let y_base = CVPixelBufferGetBaseAddressOfPlane(cv_pixel_buffer, 0);
                            let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(cv_pixel_buffer, 1);
                            let uv_base = CVPixelBufferGetBaseAddressOfPlane(cv_pixel_buffer, 1);
                            crate::log!(
                                "VIDEO: CVPixelBuffer {:p} {}x{} planes={} Y(base={:p} stride={} align={}) UV(base={:p} stride={})",
                                cv_pixel_buffer, pic.width(), pic.height(), plane_count,
                                y_base, y_stride, (y_base as usize) & 63,
                                uv_base, uv_stride
                            );
                        }
                    }
                    let ok = self.wrap_cv_pixel_buffer_as_metal(
                        textures,
                        cv_pixel_buffer,
                        pic.width(),
                        pic.height(),
                    );
                    unsafe {
                        CVPixelBufferRelease(cv_pixel_buffer);
                    }
                    if ok {
                        if !self.zero_copy_logged_ok {
                            crate::log!(
                                "VIDEO: zero-copy YUV path active ({}x{})",
                                pic.width(),
                                pic.height()
                            );
                            self.zero_copy_logged_ok = true;
                        }
                        return true;
                    }
                    self.zero_copy_fail_count += 1;
                    if self.zero_copy_fail_count <= 5 {
                        crate::log!(
                            "VIDEO: zero-copy wrap failed, falling back to CPU upload (count={})",
                            self.zero_copy_fail_count
                        );
                    }
                    // fall through to CPU upload
                } else {
                    self.zero_copy_fail_count += 1;
                    if self.zero_copy_fail_count <= 5 {
                        crate::log!(
                            "VIDEO: zero-copy finalize_nv12 returned null, falling back to CPU upload (count={})",
                            self.zero_copy_fail_count
                        );
                    }
                }
            } else {
                self.zero_copy_fail_count += 1;
                if self.zero_copy_fail_count <= 5 {
                    crate::log!(
                        "VIDEO: zero-copy decoded picture unavailable, falling back to CPU upload (count={})",
                        self.zero_copy_fail_count
                    );
                }
            }
        }

        // Fallback: CPU upload via replaceRegion
        if let Some(planes) = yuv_planes {
            self.upload_yuv_to_metal(textures, &planes);
            return true;
        }

        false
    }

    /// Wrap a CVPixelBuffer's NV12 planes as Metal textures via IOSurface
    /// (zero-copy for Y plane, zero-copy for UV plane).
    fn wrap_cv_pixel_buffer_as_metal(
        &mut self,
        textures: &mut CxTexturePool,
        pixel_buffer: CVPixelBufferRef,
        width: u32,
        height: u32,
    ) -> bool {
        let w = width as usize;
        let h = height as usize;
        let cw = (w + 1) / 2;
        let ch = (h + 1) / 2;

        crate::log!(
            "VIDEO: wrap_cv_pixel_buffer_as_metal pb={:p} {}x{} chroma={}x{} cache={:p}",
            pixel_buffer, w, h, cw, ch, self.texture_cache
        );

        unsafe {
            // Release previous CVMetalTexture refs
            if !self.cv_y_texture.is_null() {
                CFRelease(self.cv_y_texture);
                self.cv_y_texture = std::ptr::null_mut();
            }
            if !self.cv_uv_texture.is_null() {
                CFRelease(self.cv_uv_texture);
                self.cv_uv_texture = std::ptr::null_mut();
            }

            // Plane 0: Y as R8Unorm
            let mut cv_y: CVMetalTextureRef = std::ptr::null_mut();
            let ret_y = CVMetalTextureCacheCreateTextureFromImage(
                std::ptr::null_mut(),
                self.texture_cache,
                pixel_buffer,
                std::ptr::null_mut(),
                MTLPixelFormat::R8Unorm as u64,
                w,
                h,
                0, // plane 0
                &mut cv_y,
            );
            if ret_y != 0 || cv_y.is_null() {
                crate::log!(
                    "VIDEO: wrap Y plane failed ret={} cv_y={:p} ({}x{} R8Unorm plane=0)",
                    ret_y, cv_y, w, h
                );
                return false;
            }

            // Plane 1: UV interleaved as RG8Unorm
            let mut cv_uv: CVMetalTextureRef = std::ptr::null_mut();
            let ret_uv = CVMetalTextureCacheCreateTextureFromImage(
                std::ptr::null_mut(),
                self.texture_cache,
                pixel_buffer,
                std::ptr::null_mut(),
                MTLPixelFormat::RG8Unorm as u64,
                cw,
                ch,
                1, // plane 1
                &mut cv_uv,
            );
            if ret_uv != 0 || cv_uv.is_null() {
                crate::log!(
                    "VIDEO: wrap UV plane failed ret={} cv_uv={:p} ({}x{} RG8Unorm plane=1)",
                    ret_uv, cv_uv, cw, ch
                );
                CFRelease(cv_y);
                return false;
            }

            // Extract Metal texture objects
            let mtl_y: ObjcId = CVMetalTextureGetTexture(cv_y);
            let mtl_uv: ObjcId = CVMetalTextureGetTexture(cv_uv);

            crate::log!(
                "VIDEO: wrap_cv OK Y={}x{} UV={}x{} mtl_y={:p} mtl_uv={:p}",
                w, h, cw, ch, mtl_y, mtl_uv
            );

            if mtl_y.is_null() || mtl_uv.is_null() {
                crate::log!("VIDEO: CVMetalTextureGetTexture returned null (y={:p} uv={:p})", mtl_y, mtl_uv);
                CFRelease(cv_y);
                CFRelease(cv_uv);
                return false;
            }

            // Retain Metal textures (CVMetalTextureGetTexture returns autoreleased)
            let _: ObjcId = msg_send![mtl_y, retain];
            let _: ObjcId = msg_send![mtl_uv, retain];

            // Assign Y texture
            {
                let cxtex = &mut textures[self.tex_y_id];
                // Release old texture
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

            // Assign UV texture to tex_u slot (shader samples .r for U, .g for V)
            {
                let cxtex = &mut textures[self.tex_u_id];
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

            // Ensure tex_v has a valid 1x1 dummy texture. The shader evaluates
            // tex_v.sample() even in biplanar mode (both mix() args execute).
            // On older GPUs (A8) sampling an uninitialized texture slot crashes
            // the Metal command buffer.
            {
                let cxtex = &mut textures[self.tex_v_id];
                if cxtex.os.texture.is_none() {
                    let descriptor: ObjcId = msg_send![class!(MTLTextureDescriptor), new];
                    let _: () = msg_send![descriptor, setTextureType: MTLTextureType::D2];
                    let _: () = msg_send![descriptor, setWidth: 1u64];
                    let _: () = msg_send![descriptor, setHeight: 1u64];
                    let _: () = msg_send![descriptor, setDepth: 1u64];
                    let _: () = msg_send![descriptor, setPixelFormat: MTLPixelFormat::R8Unorm];
                    let _: () = msg_send![descriptor, setStorageMode: MTLStorageMode::Shared];
                    let _: () = msg_send![descriptor, setUsage: MTLTextureUsage::ShaderRead];
                    let tex: ObjcId = msg_send![self.metal_device, newTextureWithDescriptor: descriptor];
                    let _: () = msg_send![descriptor, release];
                    if !tex.is_null() {
                        cxtex.os.texture = Some(RcObjcId::from_owned(NonNull::new(tex).unwrap()));
                        cxtex.alloc = Some(TextureAlloc {
                            width: 1,
                            height: 1,
                            pixel: TexturePixel::Ru8,
                            category: TextureCategory::Video,
                        });
                    }
                }
            }

            self.cv_y_texture = cv_y;
            self.cv_uv_texture = cv_uv;

            crate::log!(
                "VIDEO: textures assigned to pool: tex_y={:?} tex_u={:?} (biplanar NV12)",
                self.tex_y_id, self.tex_u_id
            );

            true
        }
    }

    fn upload_yuv_to_metal(
        &mut self,
        textures: &mut CxTexturePool,
        planes: &YuvPlaneData,
    ) {
        let (cw, ch) = planes.layout.chroma_size(planes.width, planes.height);
        self.upload_r8_plane_to_metal(textures, self.tex_y_id, &planes.y, planes.width, planes.height);
        self.upload_r8_plane_to_metal(textures, self.tex_u_id, &planes.u, cw, ch);
        self.upload_r8_plane_to_metal(textures, self.tex_v_id, &planes.v, cw, ch);
    }

    fn upload_r8_plane_to_metal(
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

    pub fn is_software_mode(&self) -> bool {
        matches!(self.mode, ApplePlayerMode::Software(_))
    }

    /// Returns 1.0 when the zero-copy biplanar NV12 path is active.
    pub fn yuv_biplanar(&self) -> f32 {
        if !self.cv_y_texture.is_null() { 1.0 } else { 0.0 }
    }

    pub fn yuv_matrix(&self) -> f32 {
        self.yuv_matrix
    }

    pub fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.seekable_ranges(),
            ApplePlayerMode::Software(player) => player.seekable_ranges(),
        }
    }

    pub fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.buffered_ranges(),
            ApplePlayerMode::Software(player) => player.buffered_ranges(),
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.current_position_ms(),
            ApplePlayerMode::Software(player) => player.current_position_ms(),
        }
    }

    pub fn play(&mut self) {
        match &mut self.mode {
            ApplePlayerMode::Native(player) => player.play(),
            ApplePlayerMode::Software(player) => player.play(),
        }
    }

    pub fn pause(&mut self) {
        match &mut self.mode {
            ApplePlayerMode::Native(player) => player.pause(),
            ApplePlayerMode::Software(player) => player.pause(),
        }
    }

    pub fn resume(&mut self) {
        match &mut self.mode {
            ApplePlayerMode::Native(player) => player.resume(),
            ApplePlayerMode::Software(player) => player.resume(),
        }
    }

    pub fn mute(&self) {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.mute(),
            ApplePlayerMode::Software(player) => player.mute(),
        }
    }

    pub fn unmute(&self) {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.unmute(),
            ApplePlayerMode::Software(player) => player.unmute(),
        }
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        match &mut self.mode {
            ApplePlayerMode::Native(player) => player.seek_to(position_ms),
            ApplePlayerMode::Software(player) => player.seek_to(position_ms),
        }
    }

    pub fn set_volume(&self, volume: f64) {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.set_volume(volume),
            ApplePlayerMode::Software(player) => player.set_volume(volume),
        }
    }

    pub fn set_playback_rate(&self, rate: f64) {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.set_playback_rate(rate),
            ApplePlayerMode::Software(player) => player.set_playback_rate(rate),
        }
    }

    pub fn cleanup(&mut self) {
        unsafe {
            if !self.cv_y_texture.is_null() {
                CFRelease(self.cv_y_texture);
                self.cv_y_texture = std::ptr::null_mut();
            }
            if !self.cv_uv_texture.is_null() {
                CFRelease(self.cv_uv_texture);
                self.cv_uv_texture = std::ptr::null_mut();
            }
            if !self.texture_cache.is_null() {
                CFRelease(self.texture_cache);
                self.texture_cache = std::ptr::null_mut();
            }
        }
        match &mut self.mode {
            ApplePlayerMode::Native(player) => player.cleanup(),
            ApplePlayerMode::Software(player) => player.cleanup(),
        }
    }
}

impl Drop for AppleUnifiedVideoPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}
