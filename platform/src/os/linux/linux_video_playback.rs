//! GStreamer-based video player for Linux desktop (X11/Wayland).
//!
//! Uses `playbin` + `appsink` to decode video and pull RGBA frames,
//! then uploads them to OpenGL textures. When frame memory layout is compatible,
//! upload uses the mapped GStreamer buffer directly (no intermediate CPU copy).

use {
    super::gl_sys,
    super::gl_sys::LibGl,
    super::gl_video_upload::upload_i420_slices_to_gl,
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

/// Returns the canPlayType string for the given MIME type on Linux (GStreamer backend).
/// Uses a hardcoded table covering common formats supported by typical GStreamer installs.
pub fn can_play_type(mime: &str) -> &'static str {
    let base = mime.split(';').next().unwrap_or("").trim();
    match base {
        // Containers + codecs GStreamer handles well with base/good/bad plugins
        "video/mp4" | "video/x-m4v" => "probably",
        "video/webm" => "probably",
        "video/ogg" => "probably",
        "video/x-matroska" | "video/x-msvideo" | "video/quicktime" => "maybe",
        "audio/mp4" | "audio/x-m4a" => "probably",
        "audio/mpeg" => "probably",
        "audio/ogg" | "audio/vorbis" => "probably",
        "audio/webm" => "probably",
        "audio/wav" | "audio/x-wav" => "probably",
        "audio/flac" | "audio/x-flac" => "probably",
        "audio/opus" | "audio/ogg; codecs=opus" => "probably",
        _ if base.starts_with("video/") || base.starts_with("audio/") => "maybe",
        _ => "",
    }
}

#[derive(Clone, Copy, Debug)]
pub struct YuvTextureIds {
    pub tex_y_id: TextureId,
    pub tex_u_id: TextureId,
    pub tex_v_id: TextureId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoCapsProfile {
    GlMemoryRgba2D,
    GlMemoryRgba,
    SystemI420,
    SystemRgba,
}

impl VideoCapsProfile {
    fn caps_text(self) -> &'static str {
        match self {
            Self::GlMemoryRgba2D => "video/x-raw(memory:GLMemory),format=RGBA,texture-target=2D",
            Self::GlMemoryRgba => "video/x-raw(memory:GLMemory),format=RGBA",
            Self::SystemI420 => "video/x-raw,format=I420",
            Self::SystemRgba => "video/x-raw,format=RGBA",
        }
    }

    fn is_gl_memory(self) -> bool {
        matches!(self, Self::GlMemoryRgba2D | Self::GlMemoryRgba)
    }

    fn next_fallback(self) -> Option<Self> {
        match self {
            Self::GlMemoryRgba2D => Some(Self::GlMemoryRgba),
            Self::GlMemoryRgba => Some(Self::SystemI420),
            Self::SystemI420 => Some(Self::SystemRgba),
            Self::SystemRgba => None,
        }
    }
}

pub struct GStreamerVideoPlayer {
    gst: *const LibGStreamer,
    pipeline: *mut GstElement,
    video_sink: *mut GstElement,
    bus: *mut GstBus,
    pub(crate) video_id: LiveId,
    texture_id: TextureId,
    yuv_ids: Option<YuvTextureIds>,
    is_prepared: bool,
    prepare_notified: bool,
    eos_notified: bool,
    autoplay: bool,
    is_looping: bool,
    /// Audio-only mode: no appsink, no texture updates.
    audio_only: bool,
    video_width: u32,
    video_height: u32,
    duration_ns: i64,
    source_uri: String,
    temp_file_path: Option<PathBuf>,
    /// Scratch buffer used only when direct upload is not possible.
    /// Reused to avoid per-frame heap allocation in fallback row-pack path.
    pixel_buf: Vec<u8>,
    /// Dimensions of the currently allocated GL texture (0x0 = not yet allocated).
    /// Used to choose between glTexImage2D (realloc) and glTexSubImage2D (update).
    tex_width: usize,
    tex_height: usize,
    /// Log first successful upload once per player.
    logged_first_upload: bool,
    /// Current caps profile used to build the GStreamer pipeline.
    caps_profile: VideoCapsProfile,
    /// Current YUV matrix selector for shader path (0.0 = BT.709).
    yuv_matrix: f32,
    /// Last retained GLMemory sample. Retaining it keeps the texture alive.
    retained_gl_sample: *mut GstSample,
}

impl GStreamerVideoPlayer {
    pub fn new(
        gst: &LibGStreamer,
        video_id: LiveId,
        texture_id: TextureId,
        yuv_ids: Option<YuvTextureIds>,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Self {
        Self::new_impl(
            gst, video_id, texture_id, yuv_ids, source, autoplay, is_looping, false,
        )
    }

    pub fn new_audio_only(
        gst: &LibGStreamer,
        video_id: LiveId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Self {
        // Use a placeholder texture_id — audio-only never uploads frames
        Self::new_impl(
            gst,
            video_id,
            TextureId::default(),
            None,
            source,
            autoplay,
            is_looping,
            true,
        )
    }

    fn new_impl(
        gst: &LibGStreamer,
        video_id: LiveId,
        texture_id: TextureId,
        yuv_ids: Option<YuvTextureIds>,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
        audio_only: bool,
    ) -> Self {
        let gst_ptr = gst as *const LibGStreamer;

        // Resolve the URI from the source
        let (uri, temp_file_path) = Self::uri_from_source(video_id, &source);

        let caps_profile = if !audio_only
            && gst.gst_is_gl_memory.is_some()
            && gst.gst_gl_memory_get_texture_id.is_some()
        {
            VideoCapsProfile::GlMemoryRgba2D
        } else {
            VideoCapsProfile::SystemRgba
        };

        let Some((pipeline, video_sink, bus)) =
            Self::build_pipeline(gst, video_id, &uri, audio_only, caps_profile)
        else {
            return Self::null_player(
                gst_ptr,
                video_id,
                texture_id,
                yuv_ids,
                autoplay,
                is_looping,
                audio_only,
                temp_file_path,
            );
        };

        Self {
            gst: gst_ptr,
            pipeline,
            video_sink,
            bus,
            video_id,
            texture_id,
            yuv_ids,
            is_prepared: false,
            prepare_notified: false,
            eos_notified: false,
            autoplay,
            is_looping,
            audio_only,
            video_width: 0,
            video_height: 0,
            duration_ns: 0,
            source_uri: uri,
            temp_file_path,
            pixel_buf: Vec::new(),
            tex_width: 0,
            tex_height: 0,
            logged_first_upload: false,
            caps_profile,
            yuv_matrix: 0.0,
            retained_gl_sample: std::ptr::null_mut(),
        }
    }

    fn build_pipeline(
        gst: &LibGStreamer,
        video_id: LiveId,
        uri: &str,
        audio_only: bool,
        caps_profile: VideoCapsProfile,
    ) -> Option<(*mut GstElement, *mut GstElement, *mut GstBus)> {
        unsafe {
            let playbin_name = CString::new("playbin").unwrap();
            let pipeline = (gst.gst_element_factory_make)(playbin_name.as_ptr(), std::ptr::null());
            if pipeline.is_null() {
                error!(
                    "Failed to create GStreamer playbin for video {:?}",
                    video_id
                );
                return None;
            }

            let uri_prop = CString::new("uri").unwrap();
            let uri_cstr = CString::new(uri).unwrap();
            (gst.g_object_set_string)(
                pipeline,
                uri_prop.as_ptr(),
                uri_cstr.as_ptr(),
                std::ptr::null(),
            );

            let video_sink = if audio_only {
                let fakesink_type = CString::new("fakesink").unwrap();
                let fakesink =
                    (gst.gst_element_factory_make)(fakesink_type.as_ptr(), std::ptr::null());
                if !fakesink.is_null() {
                    let video_sink_prop = CString::new("video-sink").unwrap();
                    (gst.g_object_set_ptr)(
                        pipeline,
                        video_sink_prop.as_ptr(),
                        fakesink as *mut c_void,
                        std::ptr::null(),
                    );
                }
                std::ptr::null_mut()
            } else {
                let appsink_type = CString::new("appsink").unwrap();
                let appsink_name = CString::new("videosink").unwrap();
                let video_sink =
                    (gst.gst_element_factory_make)(appsink_type.as_ptr(), appsink_name.as_ptr());
                if video_sink.is_null() {
                    error!(
                        "Failed to create GStreamer appsink for video {:?}",
                        video_id
                    );
                    (gst.gst_object_unref)(pipeline as *mut c_void);
                    return None;
                }

                let caps_text = caps_profile.caps_text();
                let caps_str = CString::new(caps_text).unwrap();
                let caps = (gst.gst_caps_from_string)(caps_str.as_ptr());
                if !caps.is_null() {
                    (gst.gst_app_sink_set_caps)(video_sink, caps);
                    (gst.gst_caps_unref)(caps);
                }

                let max_buffers_prop = CString::new("max-buffers").unwrap();
                (gst.g_object_set_int)(video_sink, max_buffers_prop.as_ptr(), 2, std::ptr::null());
                let drop_prop = CString::new("drop").unwrap();
                (gst.g_object_set_int)(video_sink, drop_prop.as_ptr(), 1, std::ptr::null());

                let video_sink_prop = CString::new("video-sink").unwrap();
                (gst.g_object_set_ptr)(
                    pipeline,
                    video_sink_prop.as_ptr(),
                    video_sink as *mut c_void,
                    std::ptr::null(),
                );
                video_sink
            };

            let bus = (gst.gst_element_get_bus)(pipeline);
            (gst.gst_element_set_state)(pipeline, GST_STATE_PAUSED);
            Some((pipeline, video_sink, bus))
        }
    }

    fn destroy_pipeline(&mut self) {
        if self.pipeline.is_null() {
            return;
        }
        unsafe {
            let gst = &*self.gst;
            (gst.gst_element_set_state)(self.pipeline, GST_STATE_NULL);
            if !self.bus.is_null() {
                (gst.gst_object_unref)(self.bus as *mut c_void);
                self.bus = std::ptr::null_mut();
            }
            self.video_sink = std::ptr::null_mut();
            (gst.gst_object_unref)(self.pipeline as *mut c_void);
            self.pipeline = std::ptr::null_mut();
        }
    }

    fn null_player(
        gst: *const LibGStreamer,
        video_id: LiveId,
        texture_id: TextureId,
        yuv_ids: Option<YuvTextureIds>,
        autoplay: bool,
        is_looping: bool,
        audio_only: bool,
        temp_file_path: Option<PathBuf>,
    ) -> Self {
        Self {
            gst,
            pipeline: std::ptr::null_mut(),
            video_sink: std::ptr::null_mut(),
            bus: std::ptr::null_mut(),
            video_id,
            texture_id,
            yuv_ids,
            is_prepared: false,
            prepare_notified: false,
            eos_notified: false,
            autoplay,
            is_looping,
            audio_only,
            video_width: 0,
            video_height: 0,
            duration_ns: 0,
            source_uri: String::new(),
            temp_file_path,
            pixel_buf: Vec::new(),
            tex_width: 0,
            tex_height: 0,
            logged_first_upload: false,
            caps_profile: VideoCapsProfile::SystemRgba,
            yuv_matrix: 0.0,
            retained_gl_sample: std::ptr::null_mut(),
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
            VideoSource::Camera(..) => {
                // Camera sources are handled by V4l2CameraPlayer, not GStreamer.
                ("".to_string(), None)
            }
        }
    }

    /// Extract video dimensions from a GstSample's caps.
    fn extract_dims_from_sample(&mut self, gst: &LibGStreamer, sample: *mut GstSample) {
        unsafe {
            let caps = (gst.gst_sample_get_caps)(sample);
            if caps.is_null() {
                return;
            }
            let structure = (gst.gst_caps_get_structure)(caps, 0);
            if structure.is_null() {
                return;
            }
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
    /// Returns `Ok(...)` with metadata when ready, `Err(msg)` on failure, `None` if still loading.
    pub fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
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
                let err_str = if !error.is_null() {
                    let msg_ptr = (*error).message;
                    let s = if !msg_ptr.is_null() {
                        CStr::from_ptr(msg_ptr).to_string_lossy().to_string()
                    } else {
                        "Unknown GStreamer error".to_string()
                    };
                    (gst.g_error_free)(error);
                    s
                } else {
                    "Unknown GStreamer error".to_string()
                };
                let debug_str = if !debug.is_null() {
                    CStr::from_ptr(debug).to_string_lossy().to_string()
                } else {
                    String::new()
                };
                if !debug.is_null() {
                    (gst.g_free)(debug as *mut c_void);
                }
                (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);

                // If this caps profile fails negotiation, retry with the next
                // fallback profile before reporting failure.
                if let Some(next_profile) = self.caps_profile.next_fallback() {
                    self.destroy_pipeline();
                    if let Some((pipeline, video_sink, bus)) = Self::build_pipeline(
                        gst,
                        self.video_id,
                        &self.source_uri,
                        self.audio_only,
                        next_profile,
                    ) {
                        self.pipeline = pipeline;
                        self.video_sink = video_sink;
                        self.bus = bus;
                        self.caps_profile = next_profile;
                        return None;
                    }
                }

                if debug_str.is_empty() {
                    error!("GStreamer error id={} msg={}", self.video_id.0, err_str);
                } else {
                    error!(
                        "GStreamer error id={} msg={} debug={}",
                        self.video_id.0, err_str, debug_str
                    );
                }

                self.prepare_notified = true;
                return Some(Err(err_str));
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

            // Fallback dimensions for video (audio-only stays 0x0)
            if !self.audio_only && (self.video_width == 0 || self.video_height == 0) {
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

            // Query seekable range
            let is_seekable = self.query_is_seekable(gst);

            // Start playback immediately if autoplay
            if self.autoplay {
                (gst.gst_element_set_state)(self.pipeline, GST_STATE_PLAYING);
            }

            let video_tracks =
                if self.audio_only || (self.video_width == 0 && self.video_height == 0) {
                    vec![]
                } else {
                    vec!["video".to_string()]
                };
            let audio_tracks = vec!["audio".to_string()];

            Some(Ok((
                self.video_width,
                self.video_height,
                duration_ms,
                is_seekable,
                video_tracks,
                audio_tracks,
            )))
        }
    }

    /// Query GStreamer for whether the current source is seekable.
    unsafe fn query_is_seekable(&self, gst: &LibGStreamer) -> bool {
        if self.pipeline.is_null() {
            return false;
        }
        let query = (gst.gst_query_new_seeking)(GST_FORMAT_TIME);
        if query.is_null() {
            return false;
        }
        let res = (gst.gst_element_query)(self.pipeline, query);
        if res == 0 {
            (gst.gst_mini_object_unref)(query as *mut GstMiniObject);
            return false;
        }
        let mut format: std::os::raw::c_int = 0;
        let mut seekable: std::os::raw::c_int = 0;
        let mut start: i64 = 0;
        let mut stop: i64 = 0;
        (gst.gst_query_parse_seeking)(query, &mut format, &mut seekable, &mut start, &mut stop);
        (gst.gst_mini_object_unref)(query as *mut GstMiniObject);
        seekable != 0
    }

    /// Pull a frame from appsink and upload it to the GL texture.
    /// Returns true if a new frame was uploaded.
    pub fn poll_frame(&mut self, gl: &LibGl, textures: &mut CxTexturePool) -> bool {
        if self.pipeline.is_null() || self.video_sink.is_null() {
            return false;
        }

        let gst = unsafe { &*self.gst };

        // Do not gate frame pulls on gst_element_get_state(timeout=0):
        // PLAYING transitions are asynchronous and can stay in PAUSED/ASYNC while
        // decoded samples are already available on appsink (common on desktop setups
        // without a stable audio sink). Non-blocking try_pull_sample below naturally
        // returns null when no new frame is ready.

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

            // Zero-copy path: GLMemory sample already backed by a GL texture.
            if self.caps_profile.is_gl_memory() {
                if let (Some(is_gl_memory), Some(get_gl_texture_id)) =
                    (gst.gst_is_gl_memory, gst.gst_gl_memory_get_texture_id)
                {
                    let memory = (gst.gst_buffer_peek_memory)(buffer, 0);
                    if !memory.is_null() && is_gl_memory(memory) != 0 {
                        let gl_texture = get_gl_texture_id(memory);
                        if gl_texture != 0 {
                            let width = self.video_width as usize;
                            let height = self.video_height as usize;
                            let cxtexture = &mut textures[self.texture_id];

                            // If we previously owned a CPU-upload texture, release it now.
                            if let Some(old) = cxtexture.os.gl_texture {
                                if old != gl_texture && cxtexture.os.gl_texture_owned {
                                    (gl.glDeleteTextures)(1, &old);
                                }
                            }

                            cxtexture.os.gl_texture = Some(gl_texture);
                            cxtexture.os.gl_texture_owned = false;
                            cxtexture.alloc = Some(TextureAlloc {
                                width,
                                height,
                                pixel: TexturePixel::VideoExternal,
                                category: TextureCategory::Video,
                            });

                            if !self.retained_gl_sample.is_null() {
                                (gst.gst_mini_object_unref)(
                                    self.retained_gl_sample as *mut GstMiniObject,
                                );
                            }
                            self.retained_gl_sample = sample;

                            if !self.logged_first_upload {
                                self.logged_first_upload = true;
                            }

                            return true;
                        }
                    }
                }
            }

            // Fallback path: map buffer and upload either I420 planes or RGBA bytes.
            if !self.retained_gl_sample.is_null() {
                (gst.gst_mini_object_unref)(self.retained_gl_sample as *mut GstMiniObject);
                self.retained_gl_sample = std::ptr::null_mut();
            }
            let mut map_info = GstMapInfo::default();
            if (gst.gst_buffer_map)(buffer, &mut map_info, GST_MAP_READ) == 0 {
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                return false;
            }

            let width = self.video_width as usize;
            let height = self.video_height as usize;

            if map_info.data.is_null() || width == 0 || height == 0 {
                (gst.gst_buffer_unmap)(buffer, &mut map_info);
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                return false;
            }

            if self.caps_profile == VideoCapsProfile::SystemI420 {
                let Some(yuv_ids) = self.yuv_ids else {
                    (gst.gst_buffer_unmap)(buffer, &mut map_info);
                    (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                    return false;
                };

                let y_size = width * height;
                let cw = width.div_ceil(2);
                let ch = height.div_ceil(2);
                let uv_size = cw * ch;
                let required = y_size + uv_size + uv_size;
                if map_info.size < required {
                    (gst.gst_buffer_unmap)(buffer, &mut map_info);
                    (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                    return false;
                }

                let data = std::slice::from_raw_parts(map_info.data, map_info.size);
                let y = &data[0..y_size];
                let u = &data[y_size..y_size + uv_size];
                let v = &data[y_size + uv_size..y_size + uv_size + uv_size];
                upload_i420_slices_to_gl(
                    gl,
                    textures,
                    yuv_ids.tex_y_id,
                    yuv_ids.tex_u_id,
                    yuv_ids.tex_v_id,
                    y,
                    u,
                    v,
                    width as u32,
                    height as u32,
                );

                self.yuv_matrix = 0.0;

                (gst.gst_buffer_unmap)(buffer, &mut map_info);
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);

                if !self.logged_first_upload {
                    self.logged_first_upload = true;
                }
                return true;
            }

            let row_bytes = width * 4; // RGBA = 4 bytes per pixel
            let packed_size = row_bytes * height;

            // Derive row stride from mapped size. appsink can hand us padded rows.
            // Keep this conservative: if size/shape is inconsistent, drop the frame.
            let stride = if height > 1 {
                map_info.size / height
            } else {
                row_bytes
            };
            let min_size = stride
                .saturating_mul(height.saturating_sub(1))
                .saturating_add(row_bytes);
            if stride < row_bytes || map_info.size < min_size {
                (gst.gst_buffer_unmap)(buffer, &mut map_info);
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                return false;
            }

            // Ensure the GL texture exists.
            let cxtexture = &mut textures[self.texture_id];
            // If we were previously bound to a zero-copy external texture,
            // detach and allocate our own upload texture.
            if cxtexture.os.gl_texture.is_some() && !cxtexture.os.gl_texture_owned {
                cxtexture.os.gl_texture = None;
                cxtexture.os.gl_texture_owned = true;
            }
            let needs_alloc = if cxtexture.os.gl_texture.is_none() {
                let mut gl_texture = std::mem::MaybeUninit::uninit();
                (gl.glGenTextures)(1, gl_texture.as_mut_ptr());
                let gl_texture = gl_texture.assume_init();
                cxtexture.os.gl_texture = Some(gl_texture);
                cxtexture.os.gl_texture_owned = true;

                // Set texture parameters once at creation
                (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_2D,
                    gl_sys::TEXTURE_WRAP_S,
                    gl_sys::CLAMP_TO_EDGE as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_2D,
                    gl_sys::TEXTURE_WRAP_T,
                    gl_sys::CLAMP_TO_EDGE as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_2D,
                    gl_sys::TEXTURE_MIN_FILTER,
                    gl_sys::LINEAR as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_2D,
                    gl_sys::TEXTURE_MAG_FILTER,
                    gl_sys::LINEAR as i32,
                );
                true
            } else {
                self.tex_width != width || self.tex_height != height
            };

            let gl_texture = cxtexture.os.gl_texture.unwrap();
            (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);

            // CRITICAL: Reset all pixel-store unpack parameters. The renderer may
            // have left non-default unpack state from unrelated texture uploads.
            (gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, 4);
            (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_PIXELS, 0);
            (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_ROWS, 0);

            // Prefer direct upload from mapped GStreamer memory.
            // Fallback to a repacked scratch buffer when row stride is not
            // representable via UNPACK_ROW_LENGTH.
            let can_direct_upload = stride % 4 == 0;
            let upload_ptr: *const c_void = if can_direct_upload {
                (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, (stride / 4) as i32);
                map_info.data as *const c_void
            } else {
                (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, 0);
                self.pixel_buf.clear();
                self.pixel_buf.resize(packed_size, 0);
                let src = std::slice::from_raw_parts(map_info.data, map_info.size);
                for y in 0..height {
                    let row_start = y * stride;
                    let src_row = &src[row_start..row_start + row_bytes];
                    let dst_row_start = y * row_bytes;
                    self.pixel_buf[dst_row_start..dst_row_start + row_bytes]
                        .copy_from_slice(src_row);
                }
                self.pixel_buf.as_ptr() as *const c_void
            };

            if needs_alloc {
                // First frame or dimension change — allocate new texture storage
                (gl.glTexImage2D)(
                    gl_sys::TEXTURE_2D,
                    0,
                    gl_sys::RGBA as i32,
                    width as i32,
                    height as i32,
                    0,
                    gl_sys::RGBA,
                    gl_sys::UNSIGNED_BYTE,
                    upload_ptr,
                );
                self.tex_width = width;
                self.tex_height = height;
            } else {
                // Same dimensions — update in place (faster, no realloc)
                (gl.glTexSubImage2D)(
                    gl_sys::TEXTURE_2D,
                    0,
                    0,
                    0,
                    width as i32,
                    height as i32,
                    gl_sys::RGBA,
                    gl_sys::UNSIGNED_BYTE,
                    upload_ptr,
                );
            }

            // Restore defaults and release mapped sample memory.
            (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, 0);
            (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);
            (gst.gst_buffer_unmap)(buffer, &mut map_info);
            (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);

            // Update the texture allocation metadata
            cxtexture.alloc = Some(TextureAlloc {
                width,
                height,
                pixel: TexturePixel::VideoExternal,
                category: TextureCategory::Video,
            });

            if !self.logged_first_upload {
                self.logged_first_upload = true;
            }

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

    pub fn is_yuv_mode(&self) -> bool {
        self.caps_profile == VideoCapsProfile::SystemI420
    }

    pub fn yuv_matrix(&self) -> f32 {
        self.yuv_matrix
    }

    pub fn play(&self) {
        if self.pipeline.is_null() {
            return;
        }
        unsafe {
            ((*self.gst).gst_element_set_state)(self.pipeline, GST_STATE_PLAYING);
        }
    }

    pub fn pause(&self) {
        if self.pipeline.is_null() {
            return;
        }
        unsafe {
            ((*self.gst).gst_element_set_state)(self.pipeline, GST_STATE_PAUSED);
        }
    }

    pub fn resume(&self) {
        self.play();
    }

    pub fn mute(&self) {
        if self.pipeline.is_null() {
            return;
        }
        unsafe {
            let prop = CString::new("mute").unwrap();
            ((*self.gst).g_object_set_int)(self.pipeline, prop.as_ptr(), 1, std::ptr::null());
        }
    }

    pub fn unmute(&self) {
        if self.pipeline.is_null() {
            return;
        }
        unsafe {
            let prop = CString::new("mute").unwrap();
            ((*self.gst).g_object_set_int)(self.pipeline, prop.as_ptr(), 0, std::ptr::null());
        }
    }

    pub fn seek_to(&self, position_ms: u64) {
        if self.pipeline.is_null() {
            return;
        }
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

    pub fn set_volume(&self, volume: f64) {
        if self.pipeline.is_null() {
            return;
        }
        unsafe {
            // GStreamer playbin "volume" property: 0.0–10.0, 1.0 = 100%
            let prop = CString::new("volume").unwrap();
            ((*self.gst).g_object_set_double)(
                self.pipeline,
                prop.as_ptr(),
                volume.clamp(0.0, 10.0),
                std::ptr::null(),
            );
        }
    }

    pub fn set_playback_rate(&self, rate: f64) {
        if self.pipeline.is_null() {
            return;
        }
        let rate = if rate == 0.0 { 1.0 } else { rate };
        unsafe {
            let gst = &*self.gst;
            // Query current position to seek in-place with new rate
            let mut pos_ns: i64 = 0;
            (gst.gst_element_query_position)(self.pipeline, GST_FORMAT_TIME, &mut pos_ns);
            if pos_ns < 0 {
                pos_ns = 0;
            }
            (gst.gst_element_seek)(
                self.pipeline,
                rate,
                GST_FORMAT_TIME,
                GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_ACCURATE,
                GST_SEEK_TYPE_SET,
                pos_ns,
                GST_SEEK_TYPE_NONE,
                -1,
            );
        }
    }

    /// Returns seekable time ranges as (start_secs, end_secs) pairs.
    pub fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        if self.pipeline.is_null() || self.duration_ns <= 0 {
            return vec![];
        }
        let gst = unsafe { &*self.gst };
        let is_seekable = unsafe { self.query_is_seekable(gst) };
        if is_seekable {
            let end = self.duration_ns as f64 / 1_000_000_000.0;
            vec![(0.0, end)]
        } else {
            vec![]
        }
    }

    /// Returns buffered time ranges as (start_secs, end_secs) pairs.
    pub fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        if self.pipeline.is_null() {
            return vec![];
        }
        let gst = unsafe { &*self.gst };
        unsafe {
            let query = (gst.gst_query_new_buffering)(GST_FORMAT_TIME);
            if query.is_null() {
                return vec![];
            }
            let ok = (gst.gst_element_query)(self.pipeline, query);
            if ok == 0 {
                (gst.gst_mini_object_unref)(query as *mut GstMiniObject);
                // Fallback: assume buffered to current position
                let pos = self.current_position_ms() as f64 / 1000.0;
                return if pos > 0.0 { vec![(0.0, pos)] } else { vec![] };
            }
            let n = (gst.gst_query_get_n_buffering_ranges)(query);
            let mut ranges = Vec::with_capacity(n as usize);
            for i in 0..n {
                let mut start: i64 = 0;
                let mut stop: i64 = 0;
                let ok = (gst.gst_query_parse_nth_buffering_range)(query, i, &mut start, &mut stop);
                if ok != 0 && start >= 0 && stop > start {
                    let start_s = start as f64 / 1_000_000_000.0;
                    let stop_s = stop as f64 / 1_000_000_000.0;
                    ranges.push((start_s, stop_s));
                }
            }
            (gst.gst_mini_object_unref)(query as *mut GstMiniObject);
            ranges
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        if self.pipeline.is_null() {
            return 0;
        }
        unsafe {
            let mut position_ns: i64 = 0;
            if ((*self.gst).gst_element_query_position)(
                self.pipeline,
                GST_FORMAT_TIME,
                &mut position_ns,
            ) != 0
                && position_ns >= 0
            {
                (position_ns / 1_000_000) as u128
            } else {
                0
            }
        }
    }

    pub fn cleanup(&mut self) {
        if !self.retained_gl_sample.is_null() {
            unsafe {
                let gst = &*self.gst;
                (gst.gst_mini_object_unref)(self.retained_gl_sample as *mut GstMiniObject);
            }
            self.retained_gl_sample = std::ptr::null_mut();
        }

        self.destroy_pipeline();

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
