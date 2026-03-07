//! GStreamer-based video player for Linux desktop (X11/Wayland).
//!
//! Uses `playbin` + `appsink` to decode video and pull RGBA frames,
//! which are uploaded to OpenGL textures via `glTexImage2D`.

use {
    super::gl_sys,
    super::gl_sys::LibGl,
    super::gstreamer_sys::*,
    crate::{
        event::video_playback::VideoSource,
        makepad_error_log::*,
        makepad_live_id::LiveId,
        texture::{CxTexturePool, TextureAlloc, TextureCategory, TextureId, TexturePixel},
    },
    std::{
        ffi::{c_void, CStr, CString},
        path::PathBuf,
    },
};

pub struct GStreamerVideoPlayer {
    gst: *const LibGStreamer,
    pipeline: *mut GstElement,
    video_sink: *mut GstElement,
    bus: *mut GstBus,
    pub(crate) video_id: LiveId,
    texture_id: TextureId,
    is_prepared: bool,
    prepare_notified: bool,
    eos_notified: bool,
    autoplay: bool,
    is_looping: bool,
    video_width: u32,
    video_height: u32,
    duration_ns: i64,
    temp_file_path: Option<PathBuf>,
    /// Reusable buffer for copying pixel data from GStreamer before GL upload.
    /// Avoids a per-frame heap allocation.
    pixel_buf: Vec<u8>,
    /// Dimensions of the currently allocated GL texture (0x0 = not yet allocated).
    /// Used to choose between glTexImage2D (realloc) and glTexSubImage2D (update).
    tex_width: usize,
    tex_height: usize,
}

impl GStreamerVideoPlayer {
    pub fn new(
        gst: &LibGStreamer,
        video_id: LiveId,
        texture_id: TextureId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Self {
        let gst_ptr = gst as *const LibGStreamer;

        // Resolve the URI from the source
        let (uri, temp_file_path) = Self::uri_from_source(video_id, &source);

        // Build pipeline: create playbin and appsink separately, then set
        // the appsink as the video-sink property. playbin's internal playsink
        // automatically inserts videoconvert to satisfy our caps.
        let (pipeline, video_sink, bus) = unsafe {
            let playbin_name = CString::new("playbin").unwrap();
            let pipeline = (gst.gst_element_factory_make)(
                playbin_name.as_ptr(),
                std::ptr::null(),
            );
            if pipeline.is_null() {
                error!("Failed to create GStreamer playbin for video {:?}", video_id);
                return Self::null_player(gst_ptr, video_id, texture_id, autoplay, is_looping, temp_file_path);
            }

            // Set the URI on playbin
            let uri_prop = CString::new("uri").unwrap();
            let uri_cstr = CString::new(uri.as_str()).unwrap();
            (gst.g_object_set_string)(
                pipeline,
                uri_prop.as_ptr(),
                uri_cstr.as_ptr(),
                std::ptr::null(),
            );

            // Create appsink element
            let appsink_type = CString::new("appsink").unwrap();
            let appsink_name = CString::new("videosink").unwrap();
            let video_sink = (gst.gst_element_factory_make)(
                appsink_type.as_ptr(),
                appsink_name.as_ptr(),
            );
            if video_sink.is_null() {
                error!("Failed to create GStreamer appsink for video {:?}", video_id);
                (gst.gst_object_unref)(pipeline as *mut c_void);
                return Self::null_player(gst_ptr, video_id, texture_id, autoplay, is_looping, temp_file_path);
            }

            // Request RGBA frames — matches GL's RGBA upload format directly.
            let caps_str = CString::new("video/x-raw,format=RGBA").unwrap();
            let caps = (gst.gst_caps_from_string)(caps_str.as_ptr());
            if !caps.is_null() {
                (gst.gst_app_sink_set_caps)(video_sink, caps);
                (gst.gst_caps_unref)(caps);
            }

            // Configure appsink: max-buffers=2, drop=true
            let max_buffers_prop = CString::new("max-buffers").unwrap();
            (gst.g_object_set_int)(video_sink, max_buffers_prop.as_ptr(), 2, std::ptr::null());
            let drop_prop = CString::new("drop").unwrap();
            (gst.g_object_set_int)(video_sink, drop_prop.as_ptr(), 1, std::ptr::null());

            // Set appsink as playbin's video-sink property
            let video_sink_prop = CString::new("video-sink").unwrap();
            (gst.g_object_set_ptr)(
                pipeline,
                video_sink_prop.as_ptr(),
                video_sink as *mut c_void,
                std::ptr::null(),
            );

            // Get the bus
            let bus = (gst.gst_element_get_bus)(pipeline);

            // Set to PAUSED for preroll (this triggers decoding until the first frame)
            (gst.gst_element_set_state)(pipeline, GST_STATE_PAUSED);

            (pipeline, video_sink, bus)
        };

        Self {
            gst: gst_ptr,
            pipeline,
            video_sink,
            bus,
            video_id,
            texture_id,
            is_prepared: false,
            prepare_notified: false,
            eos_notified: false,
            autoplay,
            is_looping,
            video_width: 0,
            video_height: 0,
            duration_ns: 0,
            temp_file_path,
            pixel_buf: Vec::new(),
            tex_width: 0,
            tex_height: 0,
        }
    }

    fn null_player(
        gst: *const LibGStreamer,
        video_id: LiveId,
        texture_id: TextureId,
        autoplay: bool,
        is_looping: bool,
        temp_file_path: Option<PathBuf>,
    ) -> Self {
        Self {
            gst,
            pipeline: std::ptr::null_mut(),
            video_sink: std::ptr::null_mut(),
            bus: std::ptr::null_mut(),
            video_id,
            texture_id,
            is_prepared: false,
            prepare_notified: false,
            eos_notified: false,
            autoplay,
            is_looping,
            video_width: 0,
            video_height: 0,
            duration_ns: 0,
            temp_file_path,
            pixel_buf: Vec::new(),
            tex_width: 0,
            tex_height: 0,
        }
    }

    fn uri_from_source(video_id: LiveId, source: &VideoSource) -> (String, Option<PathBuf>) {
        match source {
            VideoSource::Network(url) => (url.clone(), None),
            VideoSource::Filesystem(path) => {
                if path.starts_with("file://") {
                    (path.clone(), None)
                } else {
                    (format!("file://{}", path), None)
                }
            }
            VideoSource::InMemory(data) => {
                let tmp_path =
                    std::env::temp_dir().join(format!("makepad_video_{}.mp4", video_id.0));
                if let Err(e) = std::fs::write(&tmp_path, data.as_ref()) {
                    error!("Failed to write video to temp file: {}", e);
                }
                let uri = format!("file://{}", tmp_path.to_string_lossy());
                (uri, Some(tmp_path))
            }
        }
    }

    /// Extract video dimensions from a GstSample's caps.
    fn extract_dims_from_sample(&mut self, gst: &LibGStreamer, sample: *mut GstSample) {
        unsafe {
            let caps = (gst.gst_sample_get_caps)(sample);
            if caps.is_null() { return; }
            let structure = (gst.gst_caps_get_structure)(caps, 0);
            if structure.is_null() { return; }
            let width_key = CString::new("width").unwrap();
            let height_key = CString::new("height").unwrap();
            let mut w: i32 = 0;
            let mut h: i32 = 0;
            (gst.gst_structure_get_int)(structure, width_key.as_ptr(), &mut w);
            (gst.gst_structure_get_int)(structure, height_key.as_ptr(), &mut h);
            if w > 0 && h > 0 {
                self.video_width = w as u32;
                self.video_height = h as u32;
            }
        }
    }

    /// Check if the player has finished prerolling and is ready to play.
    /// Returns (width, height, duration_ms) if newly prepared.
    pub fn check_prepared(&mut self) -> Option<(u32, u32, u128)> {
        if self.prepare_notified || self.pipeline.is_null() {
            return None;
        }

        let gst = unsafe { &*self.gst };

        unsafe {
            // Check for errors on the bus
            let msg = (gst.gst_bus_pop_filtered)(self.bus, GST_MESSAGE_ERROR);
            if !msg.is_null() {
                let mut error: *mut GError = std::ptr::null_mut();
                let mut debug: *mut std::os::raw::c_char = std::ptr::null_mut();
                (gst.gst_message_parse_error)(msg, &mut error, &mut debug);
                if !error.is_null() {
                    let msg_ptr = (*error).message;
                    if !msg_ptr.is_null() {
                        let msg_str = CStr::from_ptr(msg_ptr).to_string_lossy();
                        error!("GStreamer error: {}", msg_str);
                    }
                    (gst.g_error_free)(error);
                }
                if !debug.is_null() {
                    (gst.g_free)(debug as *mut c_void);
                }
                (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);
                self.prepare_notified = true;
                return None;
            }

            // Non-blocking state check
            let mut state: u32 = 0;
            let mut pending: u32 = 0;
            let ret = (gst.gst_element_get_state)(self.pipeline, &mut state, &mut pending, 0);

            if ret == GST_STATE_CHANGE_FAILURE {
                return None;
            }

            // Need at least PAUSED for preroll to be done
            if state < GST_STATE_PAUSED || self.is_prepared {
                return None;
            }

            self.is_prepared = true;
            self.prepare_notified = true;

            // Pull the preroll sample to get video dimensions.
            // try_pull_preroll works in PAUSED state (try_pull_sample does NOT).
            if !self.video_sink.is_null() {
                let sample = (gst.gst_app_sink_try_pull_preroll)(self.video_sink, 0);
                if !sample.is_null() {
                    self.extract_dims_from_sample(gst, sample);
                    (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                }
            }

            // Fallback dimensions
            if self.video_width == 0 || self.video_height == 0 {
                self.video_width = 1920;
                self.video_height = 1080;
            }

            // Query duration
            let mut duration_ns: i64 = 0;
            (gst.gst_element_query_duration)(self.pipeline, GST_FORMAT_TIME, &mut duration_ns);
            self.duration_ns = duration_ns;

            let duration_ms = if duration_ns > 0 {
                (duration_ns / 1_000_000) as u128
            } else {
                0u128
            };

            // Start playback immediately if autoplay — this transitions to PLAYING
            // which makes try_pull_sample start returning decoded frames.
            if self.autoplay {
                (gst.gst_element_set_state)(self.pipeline, GST_STATE_PLAYING);
            }

            Some((self.video_width, self.video_height, duration_ms))
        }
    }

    /// Pull a frame from appsink and upload it to the GL texture.
    /// Returns true if a new frame was uploaded.
    pub fn poll_frame(&mut self, gl: &LibGl, textures: &mut CxTexturePool) -> bool {
        if self.pipeline.is_null() || self.video_sink.is_null() {
            return false;
        }

        let gst = unsafe { &*self.gst };

        // Check actual GStreamer pipeline state — only pull frames when PLAYING.
        // This is more robust than relying on an internal flag which could become stale
        // if the player is replaced or state gets out of sync.
        unsafe {
            let mut state: u32 = 0;
            let mut pending: u32 = 0;
            (gst.gst_element_get_state)(self.pipeline, &mut state, &mut pending, 0);
            if state < GST_STATE_PLAYING {
                return false;
            }
        }

        // Check for EOS and loop if needed
        if self.is_looping {
            unsafe {
                if (gst.gst_app_sink_is_eos)(self.video_sink) != 0 {
                    (gst.gst_element_seek_simple)(
                        self.pipeline,
                        GST_FORMAT_TIME,
                        GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_KEY_UNIT,
                        0,
                    );
                }
            }
        }

        unsafe {
            // Pull next decoded frame — non-blocking (timeout=0).
            let sample = (gst.gst_app_sink_try_pull_sample)(self.video_sink, 0);
            if sample.is_null() {
                return false;
            }

            let buffer = (gst.gst_sample_get_buffer)(sample);
            if buffer.is_null() {
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                return false;
            }

            // Extract dimensions from the sample's caps
            self.extract_dims_from_sample(gst, sample);

            // Map the buffer to read pixel data
            let mut map_info = GstMapInfo::default();
            if (gst.gst_buffer_map)(buffer, &mut map_info, GST_MAP_READ) == 0 {
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                return false;
            }

            let width = self.video_width as usize;
            let height = self.video_height as usize;
            let row_bytes = width * 4; // RGBA = 4 bytes per pixel
            let packed_size = row_bytes * height;

            if map_info.data.is_null() || width == 0 || height == 0
                || map_info.size < packed_size
            {
                (gst.gst_buffer_unmap)(buffer, &mut map_info);
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                return false;
            }

            // GStreamer buffers may have a stride (row pitch) larger than width * 4
            // due to hardware alignment. Detect this and copy row-by-row if needed.
            let stride = if height > 1 {
                map_info.size / height
            } else {
                row_bytes
            };

            // Reuse the pixel buffer to avoid per-frame allocation
            self.pixel_buf.clear();
            self.pixel_buf.reserve(packed_size);
            let src = std::slice::from_raw_parts(map_info.data, map_info.size);
            if stride == row_bytes {
                self.pixel_buf.extend_from_slice(&src[..packed_size]);
            } else {
                for y in 0..height {
                    let row_start = y * stride;
                    self.pixel_buf.extend_from_slice(&src[row_start..row_start + row_bytes]);
                }
            }

            // Done with the GStreamer buffer
            (gst.gst_buffer_unmap)(buffer, &mut map_info);
            (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);

            // Ensure the GL texture exists
            let cxtexture = &mut textures[self.texture_id];
            let needs_alloc = if cxtexture.os.gl_texture.is_none() {
                let mut gl_texture = std::mem::MaybeUninit::uninit();
                (gl.glGenTextures)(1, gl_texture.as_mut_ptr());
                let gl_texture = gl_texture.assume_init();
                cxtexture.os.gl_texture = Some(gl_texture);

                // Set texture parameters once at creation
                (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_WRAP_S, gl_sys::CLAMP_TO_EDGE as i32);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_WRAP_T, gl_sys::CLAMP_TO_EDGE as i32);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MIN_FILTER, gl_sys::LINEAR as i32);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MAG_FILTER, gl_sys::LINEAR as i32);
                true
            } else {
                self.tex_width != width || self.tex_height != height
            };

            let gl_texture = cxtexture.os.gl_texture.unwrap();
            (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);

            // CRITICAL: Reset all pixel-store unpack parameters. The makepad renderer
            // may have set UNPACK_ROW_LENGTH to a different texture's width, causing
            // glTexImage2D to read past our buffer and crash in memcpy.
            (gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, 4);
            (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, 0);
            (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_PIXELS, 0);
            (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_ROWS, 0);

            if needs_alloc {
                // First frame or dimension change — allocate new texture storage
                (gl.glTexImage2D)(
                    gl_sys::TEXTURE_2D, 0, gl_sys::RGBA as i32,
                    width as i32, height as i32, 0,
                    gl_sys::RGBA, gl_sys::UNSIGNED_BYTE,
                    self.pixel_buf.as_ptr() as *const c_void,
                );
                self.tex_width = width;
                self.tex_height = height;
            } else {
                // Same dimensions — update in place (faster, no realloc)
                (gl.glTexSubImage2D)(
                    gl_sys::TEXTURE_2D, 0,
                    0, 0, width as i32, height as i32,
                    gl_sys::RGBA, gl_sys::UNSIGNED_BYTE,
                    self.pixel_buf.as_ptr() as *const c_void,
                );
            }

            (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);

            // Update the texture allocation metadata
            cxtexture.alloc = Some(TextureAlloc {
                width,
                height,
                pixel: TexturePixel::VideoRGB,
                category: TextureCategory::Video,
            });

            true
        }
    }

    /// Check if this player has reached end of stream (non-looping only).
    /// Returns true once per EOS event.
    pub fn check_eos(&mut self) -> bool {
        if self.eos_notified || self.is_looping || self.video_sink.is_null() {
            return false;
        }
        let is_eos = unsafe { ((*self.gst).gst_app_sink_is_eos)(self.video_sink) != 0 };
        if is_eos {
            self.eos_notified = true;
        }
        is_eos
    }

    /// Whether this player has an active (non-null) pipeline.
    pub fn is_active(&self) -> bool {
        !self.pipeline.is_null()
    }

    pub fn play(&self) {
        if self.pipeline.is_null() { return; }
        unsafe { ((*self.gst).gst_element_set_state)(self.pipeline, GST_STATE_PLAYING); }
    }

    pub fn pause(&self) {
        if self.pipeline.is_null() { return; }
        unsafe { ((*self.gst).gst_element_set_state)(self.pipeline, GST_STATE_PAUSED); }
    }

    pub fn resume(&self) {
        self.play();
    }

    pub fn mute(&self) {
        if self.pipeline.is_null() { return; }
        unsafe {
            let prop = CString::new("mute").unwrap();
            ((*self.gst).g_object_set_int)(self.pipeline, prop.as_ptr(), 1, std::ptr::null());
        }
    }

    pub fn unmute(&self) {
        if self.pipeline.is_null() { return; }
        unsafe {
            let prop = CString::new("mute").unwrap();
            ((*self.gst).g_object_set_int)(self.pipeline, prop.as_ptr(), 0, std::ptr::null());
        }
    }

    pub fn seek_to(&self, position_ms: u64) {
        if self.pipeline.is_null() { return; }
        unsafe {
            let position_ns = position_ms as i64 * 1_000_000;
            ((*self.gst).gst_element_seek_simple)(
                self.pipeline,
                GST_FORMAT_TIME,
                GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_ACCURATE,
                position_ns,
            );
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        if self.pipeline.is_null() { return 0; }
        unsafe {
            let mut position_ns: i64 = 0;
            if ((*self.gst).gst_element_query_position)(
                self.pipeline, GST_FORMAT_TIME, &mut position_ns,
            ) != 0 && position_ns >= 0 {
                (position_ns / 1_000_000) as u128
            } else {
                0
            }
        }
    }

    pub fn cleanup(&mut self) {
        if !self.pipeline.is_null() {
            unsafe {
                let gst = &*self.gst;
                (gst.gst_element_set_state)(self.pipeline, GST_STATE_NULL);
                if !self.bus.is_null() {
                    (gst.gst_object_unref)(self.bus as *mut c_void);
                    self.bus = std::ptr::null_mut();
                }
                // video_sink is owned by playbin — unreffed when pipeline is destroyed
                self.video_sink = std::ptr::null_mut();
                (gst.gst_object_unref)(self.pipeline as *mut c_void);
                self.pipeline = std::ptr::null_mut();
            }
        }

        if let Some(path) = self.temp_file_path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Drop for GStreamerVideoPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}
