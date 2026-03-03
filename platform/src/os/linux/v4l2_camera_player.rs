//! V4L2 camera as a video playback source — captures frames and uploads to GL texture.

use {
    super::gl_sys,
    super::gl_sys::LibGl,
    super::v4l2_camera::V4l2CameraAccess,
    crate::{
        makepad_live_id::LiveId,
        texture::{CxTexturePool, TextureAlloc, TextureCategory, TextureId, TexturePixel},
        video::*,
    },
    std::{
        ffi::c_void,
        sync::{Arc, Mutex},
    },
};

/// Camera player that captures V4L2 frames into a GL texture,
/// matching the video playback texture path.
pub struct V4l2CameraPlayer {
    pub video_id: LiveId,
    texture_id: TextureId,
    _input_id: VideoInputId,
    _format_id: VideoFormatId,
    width: u32,
    height: u32,
    active: bool,
    prepared: bool,
    prepare_notified: bool,
    /// Shared buffer: capture thread writes frames here, poll_frame reads them.
    frame_buf: Arc<Mutex<CameraFrame>>,
    /// Handle to the V4L2 camera access for cleanup.
    camera_access: Option<Arc<Mutex<V4l2CameraAccess>>>,
}

struct CameraFrame {
    data: Vec<u8>,
    width: usize,
    height: usize,
    new: bool,
}

impl Default for CameraFrame {
    fn default() -> Self {
        Self {
            data: Vec::new(),
            width: 0,
            height: 0,
            new: false,
        }
    }
}

impl V4l2CameraPlayer {
    pub fn new(
        video_id: LiveId,
        texture_id: TextureId,
        input_id: VideoInputId,
        format_id: VideoFormatId,
        camera_access: Arc<Mutex<V4l2CameraAccess>>,
    ) -> Self {
        let frame_buf = Arc::new(Mutex::new(CameraFrame::default()));

        // Set up the capture callback that converts frames to RGBA and stores them
        let frame_buf_clone = frame_buf.clone();
        let cb: VideoInputFn = Box::new(move |buffer: VideoBufferRef| {
            let w = buffer.format.width;
            let h = buffer.format.height;
            let mut rgba = Vec::with_capacity(w * h * 4);

            match buffer.format.pixel_format {
                VideoPixelFormat::NV12 => {
                    if let VideoBufferRefData::U32(data) = &buffer.data {
                        // NV12: Y plane followed by interleaved UV
                        let mut tmp = Vec::new();
                        buffer.format.pixel_format.buffer_to_bgra_32(data, w, h, &mut tmp);
                        // buffer_to_bgra_32 produces BGRA u32, convert to RGBA u8
                        for pixel in &tmp {
                            let b = (pixel & 0xff) as u8;
                            let g = ((pixel >> 8) & 0xff) as u8;
                            let r = ((pixel >> 16) & 0xff) as u8;
                            let a = ((pixel >> 24) & 0xff) as u8;
                            rgba.push(r);
                            rgba.push(g);
                            rgba.push(b);
                            rgba.push(a);
                        }
                    }
                }
                VideoPixelFormat::YUY2 => {
                    if let VideoBufferRefData::U32(data) = &buffer.data {
                        // YUY2: packed YUYV, 2 pixels per u32
                        for i in 0..(w * h / 2) {
                            if i >= data.len() { break; }
                            let packed = data[i];
                            let y0 = (packed & 0xff) as i32;
                            let u  = ((packed >> 8) & 0xff) as i32;
                            let y1 = ((packed >> 16) & 0xff) as i32;
                            let v  = ((packed >> 24) & 0xff) as i32;
                            for y in [y0, y1] {
                                let c = y - 16;
                                let d = u - 128;
                                let e = v - 128;
                                let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
                                let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
                                let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
                                rgba.push(r);
                                rgba.push(g);
                                rgba.push(b);
                                rgba.push(255);
                            }
                        }
                    }
                }
                VideoPixelFormat::RGB24 => {
                    if let VideoBufferRefData::U8(data) = &buffer.data {
                        for chunk in data.chunks(3) {
                            if chunk.len() == 3 {
                                rgba.push(chunk[0]);
                                rgba.push(chunk[1]);
                                rgba.push(chunk[2]);
                                rgba.push(255);
                            }
                        }
                    }
                }
                VideoPixelFormat::MJPEG => {
                    // MJPEG: skip — would need a JPEG decoder.
                    // Fill with black as a safe fallback.
                    rgba.resize(w * h * 4, 0);
                }
                _ => {
                    rgba.resize(w * h * 4, 0);
                }
            }

            if rgba.len() == w * h * 4 {
                let mut frame = frame_buf_clone.lock().unwrap();
                frame.data = rgba;
                frame.width = w;
                frame.height = h;
                frame.new = true;
            }
        });

        // Start the V4L2 capture by registering callback at index 0 and calling use_video_input
        {
            let mut cam = camera_access.lock().unwrap();
            // Use a dedicated callback slot (index 0) for camera playback
            *cam.video_input_cb[0].lock().unwrap() = Some(cb);
            cam.use_video_input(&[(input_id, format_id)]);
        }

        Self {
            video_id,
            texture_id,
            _input_id: input_id,
            _format_id: format_id,
            width: 0,
            height: 0,
            active: true,
            prepared: false,
            prepare_notified: false,
            frame_buf,
            camera_access: Some(camera_access),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
        if self.prepare_notified {
            return None;
        }
        // Wait for first frame to know dimensions
        let frame = self.frame_buf.lock().unwrap();
        if !frame.new || frame.width == 0 {
            return None;
        }
        self.width = frame.width as u32;
        self.height = frame.height as u32;
        self.prepared = true;
        self.prepare_notified = true;
        Some(Ok((
            self.width,
            self.height,
            0, // duration: live camera has no duration
            false, // not seekable
            vec!["camera".to_string()],
            vec![],
        )))
    }

    pub fn poll_frame(&mut self, gl: &LibGl, textures: &mut CxTexturePool) -> bool {
        let mut frame = self.frame_buf.lock().unwrap();
        if !frame.new || frame.width == 0 || frame.height == 0 {
            return false;
        }
        frame.new = false;
        let width = frame.width;
        let height = frame.height;
        let expected = width * height * 4;
        if frame.data.len() != expected {
            return false;
        }

        unsafe {
            let cxtexture = &mut textures[self.texture_id];

            // Mark alloc so setup_video_texture in the draw loop doesn't
            // free our texture and create an empty one.
            if cxtexture.alloc.is_none() {
                cxtexture.alloc = Some(TextureAlloc {
                    width: 0,
                    height: 0,
                    pixel: TexturePixel::VideoRGB,
                    category: TextureCategory::Video,
                });
            }

            // Ensure GL texture exists (setup_video_texture may have created it already)
            if cxtexture.os.gl_texture.is_none() {
                let mut gl_texture = std::mem::MaybeUninit::uninit();
                (gl.glGenTextures)(1, gl_texture.as_mut_ptr());
                let gl_texture = gl_texture.assume_init();
                cxtexture.os.gl_texture = Some(gl_texture);
                (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_WRAP_S, gl_sys::CLAMP_TO_EDGE as i32);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_WRAP_T, gl_sys::CLAMP_TO_EDGE as i32);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MIN_FILTER, gl_sys::LINEAR as i32);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MAG_FILTER, gl_sys::LINEAR as i32);
            }

            let gl_texture = cxtexture.os.gl_texture.unwrap();
            (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);

            (gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, 4);
            (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, 0);
            (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_PIXELS, 0);
            (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_ROWS, 0);

            // Always use glTexImage2D — glTexSubImage2D returns GL_INVALID_OPERATION
            // on some EGL/Wayland drivers when the texture was created in a prior draw cycle.
            (gl.glTexImage2D)(
                gl_sys::TEXTURE_2D, 0, gl_sys::RGBA as i32,
                width as i32, height as i32, 0,
                gl_sys::RGBA, gl_sys::UNSIGNED_BYTE,
                frame.data.as_ptr() as *const c_void,
            );
            (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);

            self.width = width as u32;
            self.height = height as u32;
        }
        true
    }

    pub fn cleanup(&mut self) {
        if let Some(cam) = self.camera_access.take() {
            let mut cam = cam.lock().unwrap();
            cam.use_video_input(&[]); // stop all sessions
            *cam.video_input_cb[0].lock().unwrap() = None;
        }
        self.active = false;
    }
}

impl Drop for V4l2CameraPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}
