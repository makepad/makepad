use {
    crate::{
        event::video_playback::VideoSource,
        makepad_error_log::*,
        makepad_live_id::LiveId,
        os::apple::apple_sys::*,
        texture::{CxTexturePool, TextureAlloc, TextureCategory, TextureId, TexturePixel},
    },
    std::{ffi::c_void, ptr::NonNull},
};

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
    autoplay: bool,
    is_looping: bool,
    temp_file_path: Option<std::path::PathBuf>,
}

impl AppleVideoPlayer {
    pub fn new(
        metal_device: ObjcId,
        _video_id: LiveId,
        texture_id: TextureId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Self {
        unsafe {
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

            // Create AVPlayerItemVideoOutput with BGRA pixel format
            let pixel_format_key: ObjcId = msg_send![
                class!(NSNumber),
                numberWithUnsignedInt: kCVPixelFormatType_32BGRA
            ];

            let keys: &[ObjcId] = &[kCVPixelBufferPixelFormatTypeKey as ObjcId];
            let values: &[ObjcId] = &[pixel_format_key];
            let pixel_attrs: ObjcId = msg_send![
                class!(NSDictionary),
                dictionaryWithObjects: values.as_ptr()
                forKeys: keys.as_ptr()
                count: 1usize
            ];

            let video_output: ObjcId = msg_send![class!(AVPlayerItemVideoOutput), alloc];
            let video_output: ObjcId = msg_send![
                video_output,
                initWithPixelBufferAttributes: pixel_attrs
            ];

            // Add output to player item
            let _: () = msg_send![player_item, addOutput: video_output];

            // Create AVPlayer
            let player: ObjcId = msg_send![
                class!(AVPlayer),
                playerWithPlayerItem: player_item
            ];
            let _: () = msg_send![player, retain];

            // If source was InMemory, we created a temp file - the URL retains it

            Self {
                player: RcObjcId::from_unowned(NonNull::new(player).unwrap()),
                player_item: RcObjcId::from_unowned(NonNull::new(player_item).unwrap()),
                video_output: RcObjcId::from_unowned(NonNull::new(video_output).unwrap()),
                texture_cache,
                cv_texture: std::ptr::null_mut(),
                texture_id,
                is_prepared: false,
                prepare_notified: false,
                autoplay,
                is_looping,
                temp_file_path,
            }
        }
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
    /// Called during poll_frame.
    unsafe fn check_looping(&self) {
        if !self.is_looping || !self.is_prepared {
            return;
        }
        // Check if player rate is 0 (paused/ended) while we expect it to be playing
        let rate: f32 = msg_send![self.player.as_id(), rate];
        if rate == 0.0 {
            // Check if we're at or near the end
            let current: CMTime = msg_send![self.player_item.as_id(), currentTime];
            let duration: CMTime = msg_send![self.player_item.as_id(), duration];
            let current_sec = CMTimeGetSeconds(current);
            let duration_sec = CMTimeGetSeconds(duration);
            if duration_sec.is_finite() && current_sec >= duration_sec - 0.1 {
                // Seek back to beginning and play
                let zero = CMTimeMakeWithSeconds(0.0, 600);
                let _: () = msg_send![self.player_item.as_id(), seekToTime: zero];
                let _: () = msg_send![self.player.as_id(), play];
            }
        }
    }

    /// Check if the player item has become ready to play or has failed.
    /// Returns `Ok(...)` with metadata when ready, `Err(msg)` on failure, `None` if still loading.
    pub fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
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

                return Some(Ok((
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
        if !self.is_prepared {
            return false;
        }

        unsafe {
            self.check_looping();
        }

        unsafe {
            // Force play if rate dropped to 0 (e.g. AVPlayer stalled or was never started)
            let rate: f32 = msg_send![self.player.as_id(), rate];
            if rate == 0.0 && self.autoplay {
                let _: () = msg_send![self.player.as_id(), play];
            }

            let current_time: CMTime = msg_send![self.player_item.as_id(), currentTime];

            // Try to copy the pixel buffer directly. hasNewPixelBufferForItemTime:
            // can return NO even when frames are available (observed with AV1 content
            // and short videos). copyPixelBufferForItemTime: returns null when there
            // is genuinely no frame, so it is the reliable check.
            let pixel_buffer: CVPixelBufferRef = msg_send![
                self.video_output.as_id(),
                copyPixelBufferForItemTime: current_time
                itemTimeForDisplay: std::ptr::null_mut::<CMTime>()
            ];

            if pixel_buffer.is_null() {
                return false;
            }

            let width = CVPixelBufferGetWidth(pixel_buffer);
            let height = CVPixelBufferGetHeight(pixel_buffer);

            // Create CVMetalTexture from the pixel buffer (zero-copy)
            let mut cv_texture: CVMetalTextureRef = std::ptr::null_mut();
            let status = CVMetalTextureCacheCreateTextureFromImage(
                std::ptr::null_mut(),
                self.texture_cache,
                pixel_buffer,
                std::ptr::null_mut(),
                MTLPixelFormat::BGRA8Unorm as u64,
                width,
                height,
                0, // planeIndex
                &mut cv_texture,
            );

            // Release the pixel buffer (CVMetalTexture retains what it needs)
            CFRelease(pixel_buffer as *const c_void);

            if status != 0 {
                error!(
                    "CVMetalTextureCacheCreateTextureFromImage failed: {}",
                    status
                );
                return false;
            }

            // Get the MTLTexture from the CVMetalTexture
            let mtl_texture: ObjcId = CVMetalTextureGetTexture(cv_texture);
            if mtl_texture.is_null() {
                CFRelease(cv_texture as *const c_void);
                return false;
            }

            // Retain the MTLTexture since CVMetalTexture owns it
            let _: () = msg_send![mtl_texture, retain];

            // Release previous CVMetalTexture
            if !self.cv_texture.is_null() {
                CFRelease(self.cv_texture as *const c_void);
            }
            self.cv_texture = cv_texture;

            // Swap the backing MTLTexture in the Makepad texture pool
            let cxtexture = &mut textures[self.texture_id];
            cxtexture.os.texture = Some(RcObjcId::from_owned(NonNull::new(mtl_texture).unwrap()));
            cxtexture.alloc = Some(TextureAlloc {
                width,
                height,
                pixel: TexturePixel::VideoYuvPlane,
                category: TextureCategory::Video,
            });

            true
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        unsafe {
            let current: CMTime = msg_send![self.player_item.as_id(), currentTime];
            let seconds = CMTimeGetSeconds(current);
            if seconds.is_finite() && seconds >= 0.0 {
                (seconds * 1000.0) as u128
            } else {
                0
            }
        }
    }

    pub fn seek_to(&self, position_ms: u64) {
        unsafe {
            let seconds = position_ms as f64 / 1000.0;
            let time = CMTimeMakeWithSeconds(seconds, 600);
            let _: () = msg_send![self.player.as_id(), seekToTime: time];
        }
    }

    pub fn play(&self) {
        unsafe {
            let _: () = msg_send![self.player.as_id(), play];
        }
    }

    pub fn pause(&self) {
        unsafe {
            let _: () = msg_send![self.player.as_id(), pause];
        }
    }

    pub fn resume(&self) {
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
        unsafe {
            let r = rate as f32;
            let _: () = msg_send![self.player.as_id(), setRate: r];
        }
    }

    pub fn cleanup(&mut self) {
        unsafe {
            // Pause playback
            let _: () = msg_send![self.player.as_id(), pause];

            // Remove video output from player item
            let _: () =
                msg_send![self.player_item.as_id(), removeOutput: self.video_output.as_id()];

            // Release CVMetalTexture
            if !self.cv_texture.is_null() {
                CFRelease(self.cv_texture as *const c_void);
                self.cv_texture = std::ptr::null_mut();
            }

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
