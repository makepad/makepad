use {
    crate::{
        event::video_playback::VideoSource,
        gpu_texture::{
            adopt_metal_nv12_biplanar, cv_pixel_buffer_is_biplanar_nv12,
            detach_metal_nv12_present, MetalNv12Frame, MetalNv12PresentCache,
        },
        makepad_error_log::*,
        makepad_live_id::LiveId,
        os::apple::apple_sys::*,
        texture::{CxTexturePool, TextureAlloc, TextureCategory, TextureId, TexturePixel},
        video_decode::yuv::YuvColorMatrix,
        PlaybackPrepared,
    },
    std::{
        ffi::c_void,
        ptr::NonNull,
        sync::{
            atomic::{AtomicBool, AtomicU32, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    },
};

/// After a playing seek, AVPlayer surfaces a sync frame before decode can
/// sustain motion. Presenting that lone keyframe before the pipeline can
/// sustain playback is the freeze the user sees.
///
/// Hold the previous texture until either:
/// - two distinct display PTS samples arrive, or
/// - ≥1 sample and AVPlayer reports it can keep up / is already playing.
///
/// Same policy for local files and network streams. Hard timeout clears the
/// gate without presenting.
struct PostSeekGate {
    /// Distinct post-seek display PTS samples consumed while holding.
    new_samples: u32,
    last_display_secs: Option<f64>,
    started: Instant,
}

/// Escape hatch: drop the gate (keep previous texture) so playback / fallback
/// can proceed. Must be reachable even when `hasNew` stays false.
const POST_SEEK_HARD_TIMEOUT: Duration = Duration::from_millis(3000);
/// Distinct display PTS samples that always count as warm.
const POST_SEEK_MIN_SAMPLES: u32 = 2;
const POST_SEEK_PTS_EPS: f64 = 1.0 / 240.0;
/// Preferred forward buffer so keep-up becomes true before we present.
const PREFERRED_FORWARD_BUFFER_SECS: f64 = 2.0;

fn cmtime_secs(time: CMTime) -> Option<f64> {
    if (time.flags & kCMTimeFlags_Valid) == 0 {
        return None;
    }
    let secs = unsafe { CMTimeGetSeconds(time) };
    if secs.is_finite() && secs >= 0.0 {
        Some(secs)
    } else {
        None
    }
}

/// Returns the canPlayType string for the given MIME type on Apple platforms (AVPlayer backend).
/// AVFoundation supports MP4/MOV/M4V containers with H.264/H.265/AV1 video and AAC/ALAC/FLAC/MP3
/// audio. It does **not** support WebM, Ogg, or Matroska containers.
pub fn can_play_type(mime: &str) -> &'static str {
    let base = mime.split(';').next().unwrap_or("").trim();
    match base {
        // AVPlayer handles these natively
        "video/mp4" | "video/x-m4v" | "video/quicktime" => "probably",
        "audio/mp4" | "audio/x-m4a" | "audio/aac" => "probably",
        "audio/mpeg" => "probably",
        "audio/wav" | "audio/x-wav" => "probably",
        "audio/flac" | "audio/x-flac" => "probably",
        // AVFoundation cannot play these container formats
        "video/webm" | "video/ogg" | "video/x-matroska" => "",
        "audio/webm" | "audio/ogg" | "audio/vorbis" | "audio/opus" => "",
        // Unknown audio/video type — AVFoundation might handle it
        _ if base.starts_with("video/") || base.starts_with("audio/") => "maybe",
        _ => "",
    }
}

pub struct AppleVideoPlayer {
    player: RcObjcId,
    player_item: RcObjcId,
    video_output: RcObjcId,
    texture_cache: CVMetalTextureCacheRef,
    cv_texture: CVMetalTextureRef,
    texture_id: TextureId,
    is_prepared: bool,
    prepare_notified: bool,
    /// "Start playback as soon as the asset is ready." One-shot intent consumed
    /// in `check_prepared`. Do NOT use to drive stall recovery in `poll_frame` —
    /// once a user-initiated pause has occurred, autoplay must not resurrect playback.
    autoplay: bool,
    /// Tracks the most recent user/widget play-vs-pause command. `poll_frame`
    /// uses this (not `autoplay`) to decide whether a rate-0 AVPlayer should be
    /// nudged back into playing, so a user-initiated pause sticks.
    should_play: AtomicBool,
    /// Desired playback rate (1.0 = normal). Restored after seek via `setRate:`.
    playback_rate: AtomicU32,
    is_looping: bool,
    temp_file_path: Option<std::path::PathBuf>,
    metal_device: ObjcId,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
    nv12_present: MetalNv12PresentCache,
    /// Last frame was presented via NV12 Y/UV planes (not BGRA `texture_id`).
    presents_yuv: bool,
    yuv_full_range: bool,
    gpu_frame_keep_alive: Option<Arc<dyn std::any::Any + Send + Sync>>,
    /// When set, `poll_frame` holds the previous texture until post-seek warm-up
    /// completes (or the hard timeout clears it).
    post_seek_gate: Option<PostSeekGate>,
    /// Output timestamps are the UI's clock. AVPlayerItem.currentTime can
    /// synchronously wait on MediaToolbox's decoder lock for hundreds of ms.
    position_secs: f64,
    duration_secs: f64,
    frame_pending: bool,
    has_presented_frame: bool,
    cleaned_up: bool,
}

impl AppleVideoPlayer {
    pub fn new(
        metal_device: ObjcId,
        _video_id: LiveId,
        texture_id: TextureId,
        tex_y_id: TextureId,
        tex_u_id: TextureId,
        tex_v_id: TextureId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Self {
        unsafe {
            let _: () = msg_send![metal_device, retain];

            // Create CVMetalTextureCache
            let mut texture_cache: CVMetalTextureCacheRef = std::ptr::null_mut();
            let status = CVMetalTextureCacheCreate(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                metal_device,
                std::ptr::null_mut(),
                &mut texture_cache,
            );
            if status != 0 {
                error!("CVMetalTextureCacheCreate failed with status {}", status);
            }

            // Create URL from source
            let (url, temp_file_path) = Self::url_from_source(&source);

            // Create AVPlayerItem
            let player_item: ObjcId = msg_send![class!(AVPlayerItem), playerItemWithURL: url];
            let _: () = msg_send![player_item, retain];

            // Create AVPlayerItemVideoOutput — request BGRA with Metal/IOSurface backing.
            // iOS often delivers NV12 anyway; we handle that in `present_pixel_buffer`.
            let pixel_attrs: ObjcId = Self::video_output_pixel_buffer_attributes();

            let video_output: ObjcId = msg_send![class!(AVPlayerItemVideoOutput), alloc];
            let video_output: ObjcId = msg_send![
                video_output,
                initWithPixelBufferAttributes: pixel_attrs
            ];
            let _: () = msg_send![pixel_attrs, release];

            // Add output to player item
            let _: () = msg_send![player_item, addOutput: video_output];

            // Create AVPlayer
            let player: ObjcId = msg_send![
                class!(AVPlayer),
                playerWithPlayerItem: player_item
            ];
            let _: () = msg_send![player, retain];

            // Let AVPlayer delay playback until it can sustain the rate. With
            // `NO`, seek snaps to a sync frame then micro-stalls — exactly the
            // hitch we are trying to hide. Our post-seek gate holds the previous
            // texture until samples are warm, so waiting here is the right UX.
            let _: () = msg_send![player, setAutomaticallyWaitsToMinimizeStalling: YES];
            let _: () = msg_send![
                player_item,
                setPreferredForwardBufferDuration: PREFERRED_FORWARD_BUFFER_SECS
            ];

            // If source was InMemory, we created a temp file - the URL retains it

            Self {
                // The two convenience-created objects were retained above;
                // the output is owned by alloc/init. Adopt each retain once.
                player: RcObjcId::from_owned(NonNull::new(player).unwrap()),
                player_item: RcObjcId::from_owned(NonNull::new(player_item).unwrap()),
                video_output: RcObjcId::from_owned(NonNull::new(video_output).unwrap()),
                texture_cache,
                cv_texture: std::ptr::null_mut(),
                texture_id,
                is_prepared: false,
                prepare_notified: false,
                autoplay,
                should_play: AtomicBool::new(autoplay),
                playback_rate: AtomicU32::new(1.0f32.to_bits()),
                is_looping,
                temp_file_path,
                metal_device,
                tex_y_id,
                tex_u_id,
                tex_v_id,
                nv12_present: MetalNv12PresentCache::new(metal_device),
                presents_yuv: false,
                yuv_full_range: false,
                gpu_frame_keep_alive: None,
                post_seek_gate: None,
                position_secs: 0.0,
                duration_secs: 0.0,
                frame_pending: true,
                has_presented_frame: false,
                cleaned_up: false,
            }
        }
    }

    unsafe fn video_output_pixel_buffer_attributes() -> ObjcId {
        let dict: ObjcId = msg_send![class!(NSMutableDictionary), new];
        let fmt: ObjcId = msg_send![
            class!(NSNumber),
            numberWithUnsignedInt: kCVPixelFormatType_32BGRA
        ];
        let _: () = msg_send![
            dict,
            setObject: fmt
            forKey: kCVPixelBufferPixelFormatTypeKey as ObjcId
        ];
        let yes: ObjcId = msg_send![class!(NSNumber), numberWithBool: true];
        let _: () = msg_send![
            dict,
            setObject: yes
            forKey: kCVPixelBufferMetalCompatibilityKey as ObjcId
        ];
        let io_props: ObjcId = msg_send![class!(NSDictionary), dictionary];
        let _: () = msg_send![
            dict,
            setObject: io_props
            forKey: kCVPixelBufferIOSurfacePropertiesKey as ObjcId
        ];
        dict
    }

    pub fn presents_yuv(&self) -> bool {
        self.presents_yuv
    }

    pub fn native_yuv_full_range(&self) -> bool {
        self.yuv_full_range
    }

    pub fn take_gpu_keep_alive(&mut self) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        self.gpu_frame_keep_alive.take()
    }

    fn clear_yuv_present(&mut self, textures: &mut CxTexturePool) {
        detach_metal_nv12_present(
            textures,
            self.tex_y_id,
            self.tex_u_id,
            &mut self.nv12_present,
        );
        self.presents_yuv = false;
        self.yuv_full_range = false;
        self.gpu_frame_keep_alive = None;
    }

    /// Bind `pixel_buffer` into the texture pool (BGRA zero-copy/CPU or NV12 Metal).
    /// Takes ownership of `pixel_buffer` (one `CFRelease` equivalent via keep-alive or release).
    unsafe fn present_pixel_buffer(
        &mut self,
        textures: &mut CxTexturePool,
        pixel_buffer: CVPixelBufferRef,
    ) -> bool {
        if pixel_buffer.is_null() {
            return false;
        }

        let width = CVPixelBufferGetWidth(pixel_buffer);
        let height = CVPixelBufferGetHeight(pixel_buffer);
        if width == 0 || height == 0 {
            CFRelease(pixel_buffer as *const c_void);
            return false;
        }

        if cv_pixel_buffer_is_biplanar_nv12(pixel_buffer) {
            self.clear_bgra_cv_wrap();
            if let Some(frame) = MetalNv12Frame::from_owned_cv_pixel_buffer(
                pixel_buffer,
                width as u32,
                height as u32,
                YuvColorMatrix::BT709,
            ) {
                self.yuv_full_range = frame.full_range;
                match adopt_metal_nv12_biplanar(
                    textures,
                    self.tex_y_id,
                    self.tex_u_id,
                    self.tex_v_id,
                    &frame,
                    &mut self.nv12_present,
                ) {
                    Ok(()) => {
                        self.presents_yuv = true;
                        self.gpu_frame_keep_alive = Some(frame.keep_alive);
                        return true;
                    }
                    Err(e) => {
                        error!("adopt_metal_nv12 (AVPlayer): {}", e);
                    }
                }
            }
            CFRelease(pixel_buffer as *const c_void);
            return false;
        }

        self.clear_yuv_present(textures);

        let mut cv_texture: CVMetalTextureRef = std::ptr::null_mut();
        let status = CVMetalTextureCacheCreateTextureFromImage(
            std::ptr::null_mut(),
            self.texture_cache,
            pixel_buffer,
            std::ptr::null_mut(),
            MTLPixelFormat::BGRA8Unorm as u64,
            width,
            height,
            0,
            &mut cv_texture,
        );

        if status == 0 {
            let mtl_texture: ObjcId = CVMetalTextureGetTexture(cv_texture);
            if !mtl_texture.is_null() {
                let _: () = msg_send![mtl_texture, retain];
                if !self.cv_texture.is_null() {
                    CFRelease(self.cv_texture as *const c_void);
                }
                self.cv_texture = cv_texture;
                self.presents_yuv = false;

                let cxtexture = &mut textures[self.texture_id];
                cxtexture.os.texture =
                    Some(RcObjcId::from_owned(NonNull::new(mtl_texture).unwrap()));
                cxtexture.format = crate::texture::TextureFormat::VideoExternal;
                cxtexture.alloc = Some(TextureAlloc {
                    width,
                    height,
                    pixel: TexturePixel::VideoExternal,
                    category: TextureCategory::Video,
                });
                CFRelease(pixel_buffer as *const c_void);
                return true;
            }
            CFRelease(cv_texture as *const c_void);
        } else {
            log!(
                "CVMetalTextureCacheCreateTextureFromImage failed: {} — trying CPU BGRA upload",
                status
            );
        }

        let ok = self.upload_bgra_cpu(textures, pixel_buffer, width, height);
        CFRelease(pixel_buffer as *const c_void);
        if ok {
            self.presents_yuv = false;
        }
        ok
    }

    unsafe fn clear_bgra_cv_wrap(&mut self) {
        if !self.cv_texture.is_null() {
            CFRelease(self.cv_texture as *const c_void);
            self.cv_texture = std::ptr::null_mut();
        }
    }

    unsafe fn upload_bgra_cpu(
        &self,
        textures: &mut CxTexturePool,
        pixel_buffer: CVPixelBufferRef,
        width: usize,
        height: usize,
    ) -> bool {
        const LOCK_READ_ONLY: CVPixelBufferLockFlags = 1;
        if CVPixelBufferLockBaseAddress(pixel_buffer, LOCK_READ_ONLY) != 0 {
            return false;
        }
        let base = CVPixelBufferGetBaseAddress(pixel_buffer);
        let bpr = CVPixelBufferGetBytesPerRow(pixel_buffer);
        if base.is_null() || bpr == 0 {
            let _ = CVPixelBufferUnlockBaseAddress(pixel_buffer, LOCK_READ_ONLY);
            return false;
        }

        let cxtexture = &mut textures[self.texture_id];
        let need_alloc = cxtexture.alloc.as_ref().map_or(true, |a| {
            a.width != width || a.height != height || !matches!(a.pixel, TexturePixel::VideoExternal)
        }) || cxtexture.os.texture.is_none();

        if need_alloc {
            let descriptor: ObjcId = msg_send![class!(MTLTextureDescriptor), new];
            let _: () = msg_send![descriptor, setTextureType: MTLTextureType::D2];
            let _: () = msg_send![descriptor, setWidth: width as u64];
            let _: () = msg_send![descriptor, setHeight: height as u64];
            let _: () = msg_send![descriptor, setDepth: 1u64];
            let _: () = msg_send![descriptor, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let _: () = msg_send![descriptor, setStorageMode: MTLStorageMode::Shared];
            let _: () = msg_send![descriptor, setUsage: MTLTextureUsage::ShaderRead];
            let mtl_texture: ObjcId =
                msg_send![self.metal_device, newTextureWithDescriptor: descriptor];
            let _: () = msg_send![descriptor, release];
            if mtl_texture.is_null() {
                let _ = CVPixelBufferUnlockBaseAddress(pixel_buffer, LOCK_READ_ONLY);
                return false;
            }
            cxtexture.os.texture = Some(RcObjcId::from_owned(NonNull::new(mtl_texture).unwrap()));
            cxtexture.format = crate::texture::TextureFormat::VideoExternal;
            cxtexture.alloc = Some(TextureAlloc {
                width,
                height,
                pixel: TexturePixel::VideoExternal,
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
            withBytes: base as *const c_void
            bytesPerRow: bpr as u64
        ];
        let _ = CVPixelBufferUnlockBaseAddress(pixel_buffer, LOCK_READ_ONLY);
        true
    }

    unsafe fn url_from_source(source: &VideoSource) -> (ObjcId, Option<std::path::PathBuf>) {
        match source {
            VideoSource::Network(url_string) => {
                let ns_string = Self::to_nsstring(url_string);
                let url: ObjcId = msg_send![class!(NSURL), URLWithString: ns_string];
                let _: () = msg_send![ns_string, release];
                (url, None)
            }
            VideoSource::Filesystem(path) => {
                let ns_string = Self::to_nsstring(path);
                let url: ObjcId = msg_send![class!(NSURL), fileURLWithPath: ns_string];
                let _: () = msg_send![ns_string, release];
                (url, None)
            }
            VideoSource::InMemory(data) => {
                // Detect container format from magic bytes for correct file extension.
                let ext = detect_container_extension(data);
                let tmp_path = std::env::temp_dir().join(format!(
                    "makepad_video_{}.{}",
                    LiveId::unique().0,
                    ext
                ));
                let tmp_path_str = tmp_path.to_string_lossy().to_string();
                std::fs::write(&tmp_path, data.as_ref()).unwrap_or_else(|e| {
                    error!("Failed to write video to temp file: {}", e);
                });
                let ns_string = Self::to_nsstring(&tmp_path_str);
                let url: ObjcId = msg_send![class!(NSURL), fileURLWithPath: ns_string];
                let _: () = msg_send![ns_string, release];
                (url, Some(tmp_path))
            }
            VideoSource::Camera(..) => {
                error!("VIDEO: Camera source not supported on macOS/iOS");
                let ns_string = Self::to_nsstring("about:blank");
                let url: ObjcId = msg_send![class!(NSURL), URLWithString: ns_string];
                let _: () = msg_send![ns_string, release];
                (url, None)
            }
            VideoSource::PlaybackSession(..) | VideoSource::Session(..) => {
                error!("VIDEO: session sources are handled by the software video player");
                let ns_string = Self::to_nsstring("about:blank");
                let url: ObjcId = msg_send![class!(NSURL), URLWithString: ns_string];
                let _: () = msg_send![ns_string, release];
                (url, None)
            }
        }
    }

    unsafe fn to_nsstring(s: &str) -> ObjcId {
        let ns_string: ObjcId = msg_send![class!(NSString), alloc];
        msg_send![
            ns_string,
            initWithBytes: s.as_ptr()
            length: s.len()
            encoding: 4u64 // NSUTF8StringEncoding
        ]
    }

    /// Check if playback reached end and loop back to start if needed.
    /// Called during poll_frame. Must not treat `automaticallyWaits` rate-0
    /// buffering as end-of-media.
    unsafe fn check_looping(&mut self) {
        if !self.is_looping || !self.is_prepared || self.post_seek_gate.is_some() {
            return;
        }
        if !self.should_play.load(Ordering::Acquire) {
            return;
        }
        // AVPlayerTimeControlStatus: Paused=0, WaitingToPlayAtSpecifiedRate=1, Playing=2.
        // Waiting means buffering / stall recovery — not ended.
        let time_control: i64 = msg_send![self.player.as_id(), timeControlStatus];
        if time_control == 1 {
            return;
        }
        let current_sec = self.position_secs;
        let duration_sec = self.duration_secs;
        if !(duration_sec.is_finite()
            && duration_sec > 0.0
            && current_sec.is_finite()
            && current_sec >= duration_sec - 0.05)
        {
            return;
        }
        // Natural end leaves the player paused at EOF while we still want play.
        let rate: f32 = msg_send![self.player.as_id(), rate];
        if rate != 0.0 && time_control == 2 {
            return;
        }
        // Route through seek_to so the post-seek gate arms the same way.
        self.seek_to(0);
    }

    /// If the post-seek gate has exceeded the hard timeout, clear it without
    /// presenting. Returns true when the gate was cleared this call.
    fn clear_post_seek_gate_if_timed_out(&mut self) -> bool {
        let timed_out = self
            .post_seek_gate
            .as_ref()
            .is_some_and(|g| g.started.elapsed() >= POST_SEEK_HARD_TIMEOUT);
        if timed_out {
            self.post_seek_gate = None;
            true
        } else {
            false
        }
    }

    /// Ready to present after a playing seek: two distinct PTS, or one sample
    /// once AVPlayer can keep up / is already playing.
    unsafe fn post_seek_ready(&self, new_samples: u32) -> bool {
        if new_samples >= POST_SEEK_MIN_SAMPLES {
            return true;
        }
        if new_samples < 1 {
            return false;
        }
        let likely: bool = msg_send![self.player_item.as_id(), isPlaybackLikelyToKeepUp];
        if likely {
            return true;
        }
        // AVPlayerTimeControlStatusPlaying = 2
        let time_control: i64 = msg_send![self.player.as_id(), timeControlStatus];
        time_control == 2
    }

    /// Check if the player item has become ready to play or has failed.
    /// Returns `Ok(...)` with metadata when ready, `Err(msg)` on failure, `None` if still loading.
    pub fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        if self.prepare_notified {
            return None;
        }

        unsafe {
            let status: i64 = msg_send![self.player_item.as_id(), status];
            // AVPlayerItemStatusReadyToPlay = 1
            if status == 1 && !self.is_prepared {
                self.is_prepared = true;
                self.prepare_notified = true;

                // Get video dimensions from the asset's video track
                let asset: ObjcId = msg_send![self.player_item.as_id(), asset];
                let media_type_vid = Self::to_nsstring("vide");
                let video_tracks_obj: ObjcId =
                    msg_send![asset, tracksWithMediaType: media_type_vid];
                let _: () = msg_send![media_type_vid, release];

                let video_track_count: usize = msg_send![video_tracks_obj, count];
                let (width, height) = if video_track_count > 0 {
                    let track: ObjcId = msg_send![video_tracks_obj, objectAtIndex: 0usize];
                    let size: NSSize = msg_send![track, naturalSize];
                    (size.width as u32, size.height as u32)
                } else {
                    (0, 0) // audio-only
                };

                // Check for audio tracks
                let media_type_aud = Self::to_nsstring("soun");
                let audio_tracks_obj: ObjcId =
                    msg_send![asset, tracksWithMediaType: media_type_aud];
                let _: () = msg_send![media_type_aud, release];
                let audio_track_count: usize = msg_send![audio_tracks_obj, count];

                // Get duration
                let duration: CMTime = msg_send![self.player_item.as_id(), duration];
                let duration_seconds = CMTimeGetSeconds(duration);
                self.duration_secs = cmtime_secs(duration).unwrap_or(0.0);
                let duration_ms = if duration_seconds.is_finite() && duration_seconds > 0.0 {
                    (duration_seconds * 1000.0) as u128
                } else {
                    0u128
                };

                // Query seekable ranges
                let seekable_ranges: ObjcId =
                    msg_send![self.player_item.as_id(), seekableTimeRanges];
                let seekable_count: usize = msg_send![seekable_ranges, count];
                let is_seekable = seekable_count > 0 && duration_ms > 0;

                if self.autoplay {
                    self.play();
                }

                let video_tracks = if width > 0 && height > 0 {
                    vec!["video".to_string()]
                } else {
                    vec![]
                };
                let audio_tracks = if audio_track_count > 0 {
                    vec!["audio".to_string()]
                } else {
                    vec![]
                };

                return Some(Ok(PlaybackPrepared::new(
                    width,
                    height,
                    duration_ms,
                    is_seekable,
                    video_tracks,
                    audio_tracks,
                )));
            }

            // AVPlayerItemStatusFailed = 2
            if status == 2 {
                self.prepare_notified = true;
                let error: ObjcId = msg_send![self.player_item.as_id(), error];
                let err_str = if error != nil {
                    let desc: ObjcId = msg_send![error, localizedDescription];
                    let c_str: *const u8 = msg_send![desc, UTF8String];
                    if !c_str.is_null() {
                        std::ffi::CStr::from_ptr(c_str as *const _)
                            .to_string_lossy()
                            .to_string()
                    } else {
                        "Unknown playback error".to_string()
                    }
                } else {
                    "Unknown playback error".to_string()
                };
                error!("AVPlayer failed to prepare: {}", err_str);
                return Some(Err(err_str));
            }
        }
        None
    }

    /// Returns seekable time ranges as (start_secs, end_secs) pairs.
    pub fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        if !self.is_prepared {
            return vec![];
        }
        unsafe {
            let ranges: ObjcId = msg_send![self.player_item.as_id(), seekableTimeRanges];
            let count: usize = msg_send![ranges, count];
            let mut result = Vec::with_capacity(count);
            for i in 0..count {
                let range_val: ObjcId = msg_send![ranges, objectAtIndex: i];
                let range: CMTimeRange = msg_send![range_val, CMTimeRangeValue];
                let start = CMTimeGetSeconds(range.start);
                let end = CMTimeGetSeconds(CMTimeRangeGetEnd(range));
                if start.is_finite() && end.is_finite() && end > start {
                    result.push((start, end));
                }
            }
            result
        }
    }

    /// Returns buffered (loaded) time ranges as (start_secs, end_secs) pairs.
    pub fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        if !self.is_prepared {
            return vec![];
        }
        unsafe {
            let ranges: ObjcId = msg_send![self.player_item.as_id(), loadedTimeRanges];
            let count: usize = msg_send![ranges, count];
            let mut result = Vec::with_capacity(count);
            for i in 0..count {
                let range_val: ObjcId = msg_send![ranges, objectAtIndex: i];
                let range: CMTimeRange = msg_send![range_val, CMTimeRangeValue];
                let start = CMTimeGetSeconds(range.start);
                let end = CMTimeGetSeconds(CMTimeRangeGetEnd(range));
                if start.is_finite() && end.is_finite() && end > start {
                    result.push((start, end));
                }
            }
            result
        }
    }

    /// Poll for a new video frame. Returns true if a new frame was bound to the texture.
    pub fn poll_frame(&mut self, textures: &mut CxTexturePool) -> bool {
        if !self.is_prepared || !self.needs_poll() {
            return false;
        }

        unsafe {
            self.check_looping();
        }

        unsafe {
            // Hard timeout must run even when hasNew stays false — otherwise the
            // gate (and software-fallback suppression) lasts forever. Clear the
            // gate and keep the previous texture this frame; do not present the
            // lone keyframe on the same poll.
            if self.clear_post_seek_gate_if_timed_out() {
                if self.should_play.load(Ordering::Acquire) {
                    let rate: f32 = msg_send![self.player.as_id(), rate];
                    if rate == 0.0 {
                        self.apply_play_rate();
                    }
                }
                return false;
            }

            let gating = self.post_seek_gate.is_some();
            // While post-seek warming, do not fight AVPlayer's temporary rate-0.
            if !gating {
                let rate: f32 = msg_send![self.player.as_id(), rate];
                if rate == 0.0 && self.should_play.load(Ordering::Acquire) {
                    self.apply_play_rate();
                }
            }

            // Never ask AVPlayerItem for currentTime on the UI thread. Before
            // the output's clock is ready, use the last position/seek target.
            let host_time = CACurrentMediaTime();
            let output_time: CMTime =
                msg_send![self.video_output.as_id(), itemTimeForHostTime: host_time];
            let query_time = if let Some(seconds) = cmtime_secs(output_time) {
                self.position_secs = seconds;
                output_time
            } else {
                CMTimeMakeWithSeconds(self.position_secs, 600)
            };

            // During the gate, only consume fresh samples — unless we already
            // have ≥1 sample and are ready to present: then re-copy the current
            // buffer without waiting for another hasNew.
            let has_new: bool = msg_send![
                self.video_output.as_id(),
                hasNewPixelBufferForItemTime: query_time
            ];
            if !has_new {
                let samples = self
                    .post_seek_gate
                    .as_ref()
                    .map(|g| g.new_samples)
                    .unwrap_or(0);
                if !gating || !self.post_seek_ready(samples) {
                    return false;
                }
            }

            let mut display_time = kCMTimeInvalid;
            let pixel_buffer: CVPixelBufferRef = msg_send![
                self.video_output.as_id(),
                copyPixelBufferForItemTime: query_time
                itemTimeForDisplay: &mut display_time
            ];

            if pixel_buffer.is_null() {
                // Timeout path already handled above; keep previous texture.
                return false;
            }

            // Display PTS only — never fall back to host-mapped query time.
            let display_secs = cmtime_secs(display_time);

            let samples_after = if let Some(gate) = self.post_seek_gate.as_mut() {
                let pts_changed = match (gate.last_display_secs, display_secs) {
                    (None, Some(pts)) => {
                        gate.last_display_secs = Some(pts);
                        true
                    }
                    (Some(prev), Some(pts)) => {
                        let changed = (pts - prev).abs() > POST_SEEK_PTS_EPS;
                        if changed {
                            gate.last_display_secs = Some(pts);
                        }
                        changed
                    }
                    // Invalid display PTS is not progress — do not count.
                    _ => false,
                };
                if pts_changed {
                    gate.new_samples = gate.new_samples.saturating_add(1);
                }
                Some(gate.new_samples)
            } else {
                None
            };
            let hold_post_seek = match samples_after {
                Some(samples) => !self.post_seek_ready(samples),
                None => false,
            };
            if hold_post_seek {
                CFRelease(pixel_buffer as *const c_void);
                return false;
            }
            if self.post_seek_gate.take().is_some()
                && self.should_play.load(Ordering::Acquire)
            {
                let rate: f32 = msg_send![self.player.as_id(), rate];
                if rate == 0.0 {
                    self.apply_play_rate();
                }
            }

            let presented = self.present_pixel_buffer(textures, pixel_buffer);
            if presented {
                self.frame_pending = false;
                self.has_presented_frame = true;
                if let Some(seconds) = display_secs {
                    self.position_secs = seconds;
                }
            }
            presented
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        (self.position_secs * 1000.0) as u128
    }

    /// Paused players with a displayed frame do no work until play or seek.
    pub fn needs_poll(&self) -> bool {
        !self.prepare_notified
            || (self.is_prepared
                && (self.frame_pending || self.should_play.load(Ordering::Acquire)))
    }

    pub fn is_waiting_for_first_frame(&self) -> bool {
        self.is_prepared && !self.has_presented_frame
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        unsafe {
            let seconds = position_ms as f64 / 1000.0;
            self.position_secs = seconds;
            self.frame_pending = true;
            let time = CMTimeMakeWithSeconds(seconds, 600);
            // `cancelPendingSeeks` lives on AVPlayerItem, not AVPlayer.
            let _: () = msg_send![self.player_item.as_id(), cancelPendingSeeks];

            // Snap to a nearby sync frame. Avoid completionHandler/preroll gating:
            // those paths can leave playback suppressed forever if a callback is
            // dropped (seen as "no frames after 60 polls" → broken software fallback).
            let _: () = msg_send![
                self.player.as_id(),
                seekToTime: time
                toleranceBefore: kCMTimePositiveInfinity
                toleranceAfter: kCMTimePositiveInfinity
            ];
            let _: () = msg_send![
                self.video_output.as_id(),
                requestNotificationOfMediaDataChangeWithAdvanceInterval: 0.0f64
            ];
            // Only gate while playing: paused seek should show the target frame.
            // `play`/`setRate` once; do not keep nudging during the gate.
            if self.should_play.load(Ordering::Acquire) {
                self.post_seek_gate = Some(PostSeekGate {
                    new_samples: 0,
                    last_display_secs: None,
                    started: Instant::now(),
                });
                self.apply_play_rate();
            } else {
                self.post_seek_gate = None;
            }
        }
    }

    /// True while intentionally holding the previous texture after a playing seek.
    /// Callers must not treat these polls as "no frames" for software fallback.
    /// After hard timeout the gate is cleared, so fallback can run again.
    pub fn is_post_seek_holding(&self) -> bool {
        self.post_seek_gate.is_some()
    }

    fn apply_play_rate(&self) {
        unsafe {
            let rate = f32::from_bits(self.playback_rate.load(Ordering::Relaxed)).max(0.05);
            let _: () = msg_send![self.player.as_id(), setRate: rate];
        }
    }

    pub fn play(&mut self) {
        self.should_play.store(true, Ordering::Release);
        // Defer setRate while post-seek warming — restores when the gate opens.
        if self.post_seek_gate.is_none() {
            self.apply_play_rate();
        }
    }

    pub fn pause(&mut self) {
        self.should_play.store(false, Ordering::Release);
        self.post_seek_gate = None;
        unsafe {
            let _: () = msg_send![self.player.as_id(), pause];
        }
    }

    pub fn resume(&mut self) {
        self.play();
    }

    pub fn mute(&self) {
        unsafe {
            let _: () = msg_send![self.player.as_id(), setMuted: YES];
        }
    }

    pub fn unmute(&self) {
        unsafe {
            let _: () = msg_send![self.player.as_id(), setMuted: NO];
        }
    }

    pub fn set_volume(&self, volume: f64) {
        unsafe {
            let vol = volume.clamp(0.0, 1.0) as f32;
            let _: () = msg_send![self.player.as_id(), setVolume: vol];
        }
    }

    pub fn set_playback_rate(&self, rate: f64) {
        let rate = (rate as f32).max(0.05);
        self.playback_rate.store(rate.to_bits(), Ordering::Relaxed);
        // Store desired rate always; apply immediately only when not warming.
        if self.should_play.load(Ordering::Acquire) && self.post_seek_gate.is_none() {
            unsafe {
                let _: () = msg_send![self.player.as_id(), setRate: rate];
            }
        }
    }

    pub fn cleanup(&mut self) {
        // Explicit cleanup is followed by Drop (also through the unified
        // player). Release the Metal device and output attachment only once.
        if self.cleaned_up {
            return;
        }
        self.cleaned_up = true;
        self.is_prepared = false;
        self.prepare_notified = true;
        unsafe {
            self.should_play.store(false, Ordering::Release);
            let _: () = msg_send![self.player_item.as_id(), cancelPendingSeeks];

            // Pause playback
            let _: () = msg_send![self.player.as_id(), pause];

            // Remove video output from player item
            let _: () =
                msg_send![self.player_item.as_id(), removeOutput: self.video_output.as_id()];

            // Release CVMetalTexture
            self.clear_bgra_cv_wrap();

            // Flush texture cache
            if !self.texture_cache.is_null() {
                CVMetalTextureCacheFlush(self.texture_cache, 0);
                CFRelease(self.texture_cache as *const c_void);
                self.texture_cache = std::ptr::null_mut();
            }
        }

        // Clean up temp file from InMemory source
        if let Some(path) = self.temp_file_path.take() {
            let _ = std::fs::remove_file(path);
        }
        self.nv12_present.release_textures();
        unsafe {
            let _: () = msg_send![self.metal_device, release];
            self.metal_device = nil;
        }
    }
}

impl Drop for AppleVideoPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Detect container format from magic bytes and return an appropriate file extension.
fn detect_container_extension(data: &[u8]) -> &'static str {
    if data.len() < 12 {
        return "mp4";
    }
    // WebM/Matroska: starts with EBML header 0x1A45DFA3
    if data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return "webm";
    }
    // Ogg: starts with "OggS"
    if data.starts_with(b"OggS") {
        return "ogg";
    }
    // RIFF/AVI/WAV: starts with "RIFF"
    if data.starts_with(b"RIFF") {
        if data.len() >= 12 && &data[8..12] == b"AVI " {
            return "avi";
        }
        return "wav";
    }
    // FLAC: starts with "fLaC"
    if data.starts_with(b"fLaC") {
        return "flac";
    }
    // MP3: ID3 tag or sync word
    if data.starts_with(b"ID3") || (data[0] == 0xFF && (data[1] & 0xE0) == 0xE0) {
        return "mp3";
    }
    // QuickTime/MP4: check for ftyp box
    if data.len() >= 8 && &data[4..8] == b"ftyp" {
        let brand = &data[8..12];
        if brand == b"qt  " {
            return "mov";
        }
        return "mp4";
    }
    // Default to mp4
    "mp4"
}
