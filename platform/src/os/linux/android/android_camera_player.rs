//! Android NDK camera as a video playback source — captures frames and uploads to GL texture.

use {
    super::android_camera::AndroidCameraAccess,
    super::super::gl_sys,
    super::super::gl_sys::LibGl,
    crate::{
        makepad_live_id::LiveId,
        texture::{CxTexturePool, TextureAlloc, TextureCategory, TexturePixel, TextureId},
        video::*,
    },
    std::{
        ffi::c_void,
        sync::{Arc, Mutex},
    },
};

/// Camera player that captures Android NDK camera frames into a GL texture,
/// matching the video playback texture path.
pub struct AndroidCameraPlayer {
    pub video_id: LiveId,
    texture_id: TextureId,
    width: u32,
    height: u32,
    active: bool,
    prepared: bool,
    prepare_notified: bool,
    frame_buf: Arc<Mutex<CameraFrame>>,
    camera_access: Option<Arc<Mutex<AndroidCameraAccess>>>,
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

impl AndroidCameraPlayer {
    pub fn new(
        video_id: LiveId,
        texture_id: TextureId,
        input_id: VideoInputId,
        format_id: VideoFormatId,
        camera_access: Arc<Mutex<AndroidCameraAccess>>,
    ) -> Self {
        let frame_buf = Arc::new(Mutex::new(CameraFrame::default()));

        let frame_buf_clone = frame_buf.clone();
        let cb: VideoInputFn = Box::new(move |buffer: VideoBufferRef| {
            let w = buffer.format.width;
            let h = buffer.format.height;
            let mut rgba = Vec::with_capacity(w * h * 4);

            match buffer.format.pixel_format {
                VideoPixelFormat::YUV420 => {
                    // Android YUV_420_888 arrives as u32 via AImageReader
                    if let VideoBufferRefData::U32(data) = &buffer.data {
                        // Each u32 is an RGBA pixel from the NV21-style conversion
                        for &pixel in data.iter() {
                            rgba.push((pixel & 0xff) as u8);
                            rgba.push(((pixel >> 8) & 0xff) as u8);
                            rgba.push(((pixel >> 16) & 0xff) as u8);
                            rgba.push(((pixel >> 24) & 0xff) as u8);
                        }
                    }
                }
                VideoPixelFormat::MJPEG => {
                    // MJPEG: skip, fill black
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

        {
            let mut cam = camera_access.lock().unwrap();
            *cam.video_input_cb[0].lock().unwrap() = Some(cb);
            cam.use_video_input(&[(input_id, format_id)]);
        }

        Self {
            video_id,
            texture_id,
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
            0,
            false,
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
            let needs_alloc = if cxtexture.os.gl_texture.is_none() {
                let mut gl_texture = std::mem::MaybeUninit::uninit();
                (gl.glGenTextures)(1, gl_texture.as_mut_ptr());
                let gl_texture = gl_texture.assume_init();
                cxtexture.os.gl_texture = Some(gl_texture);
                (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_WRAP_S, gl_sys::CLAMP_TO_EDGE as i32);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_WRAP_T, gl_sys::CLAMP_TO_EDGE as i32);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MIN_FILTER, gl_sys::LINEAR as i32);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MAG_FILTER, gl_sys::LINEAR as i32);
                true
            } else {
                self.width as usize != width || self.height as usize != height
            };

            let gl_texture = cxtexture.os.gl_texture.unwrap();
            (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);
            (gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, 4);
            (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, 0);
            (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_PIXELS, 0);
            (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_ROWS, 0);

            if needs_alloc {
                (gl.glTexImage2D)(
                    gl_sys::TEXTURE_2D, 0, gl_sys::RGBA as i32,
                    width as i32, height as i32, 0,
                    gl_sys::RGBA, gl_sys::UNSIGNED_BYTE,
                    frame.data.as_ptr() as *const c_void,
                );
            } else {
                (gl.glTexSubImage2D)(
                    gl_sys::TEXTURE_2D, 0, 0, 0,
                    width as i32, height as i32,
                    gl_sys::RGBA, gl_sys::UNSIGNED_BYTE,
                    frame.data.as_ptr() as *const c_void,
                );
            }
            (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);

            cxtexture.alloc = Some(TextureAlloc {
                width,
                height,
                pixel: TexturePixel::VideoRGB,
                category: TextureCategory::Video,
            });

            self.width = width as u32;
            self.height = height as u32;
        }
        true
    }

    pub fn cleanup(&mut self) {
        if let Some(cam) = self.camera_access.take() {
            let mut cam = cam.lock().unwrap();
            cam.use_video_input(&[]);
            *cam.video_input_cb[0].lock().unwrap() = None;
        }
        self.active = false;
    }
}

impl Drop for AndroidCameraPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}
