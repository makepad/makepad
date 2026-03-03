//! Android NDK camera as a video playback source — captures YUV frames and uploads to GL textures.

use {
    super::android_camera::AndroidCameraAccess,
    super::super::gl_sys::LibGl,
    super::super::gl_video_upload::upload_yuv_to_gl,
    crate::{
        makepad_live_id::LiveId,
        texture::{CxTexturePool, TextureId},
        video::*,
        video_decode::yuv::{YuvColorMatrix, YuvLayout, YuvPlaneData},
    },
    std::sync::{Arc, Mutex},
};

/// Camera player that captures Android NDK camera frames and uploads YUV planes to GL textures.
pub struct AndroidCameraPlayer {
    pub video_id: LiveId,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
    width: u32,
    height: u32,
    active: bool,
    prepared: bool,
    prepare_notified: bool,
    frame_buf: Arc<Mutex<CameraFrame>>,
    camera_access: Option<Arc<Mutex<AndroidCameraAccess>>>,
}

struct CameraFrame {
    y_data: Vec<u8>,
    u_data: Vec<u8>,
    v_data: Vec<u8>,
    width: usize,
    height: usize,
    new: bool,
}

impl Default for CameraFrame {
    fn default() -> Self {
        Self {
            y_data: Vec::new(),
            u_data: Vec::new(),
            v_data: Vec::new(),
            width: 0,
            height: 0,
            new: false,
        }
    }
}

impl AndroidCameraPlayer {
    pub fn new(
        video_id: LiveId,
        tex_y_id: TextureId,
        tex_u_id: TextureId,
        tex_v_id: TextureId,
        input_id: VideoInputId,
        format_id: VideoFormatId,
        camera_access: Arc<Mutex<AndroidCameraAccess>>,
    ) -> Self {
        let frame_buf = Arc::new(Mutex::new(CameraFrame::default()));

        let frame_buf_clone = frame_buf.clone();
        let cb: VideoInputFn = Box::new(move |buffer: VideoBufferRef| {
            let w = buffer.format.width;
            let h = buffer.format.height;

            match buffer.format.pixel_format {
                VideoPixelFormat::YUV420 => {
                    // Data arrives as concatenated [Y|U|V] packed in U8
                    if let VideoBufferRefData::U8(data) = &buffer.data {
                        let y_size = w * h;
                        let uv_size = (w / 2) * (h / 2);
                        if data.len() >= y_size + uv_size * 2 {
                            let mut frame = frame_buf_clone.lock().unwrap();
                            frame.y_data = data[..y_size].to_vec();
                            frame.u_data = data[y_size..y_size + uv_size].to_vec();
                            frame.v_data = data[y_size + uv_size..y_size + uv_size * 2].to_vec();
                            frame.width = w;
                            frame.height = h;
                            frame.new = true;
                        }
                    }
                }
                _ => {
                    // MJPEG or other: store empty planes to signal frame arrival
                    let mut frame = frame_buf_clone.lock().unwrap();
                    frame.y_data = vec![0u8; w * h];
                    frame.u_data = vec![128u8; (w / 2) * (h / 2)];
                    frame.v_data = vec![128u8; (w / 2) * (h / 2)];
                    frame.width = w;
                    frame.height = h;
                    frame.new = true;
                }
            }
        });

        {
            let mut cam = camera_access.lock().unwrap();
            *cam.video_input_cb[0].lock().unwrap() = Some(cb);
            cam.use_video_input(&[(input_id, format_id)]);
        }

        Self {
            video_id,
            tex_y_id,
            tex_u_id,
            tex_v_id,
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
        let width = frame.width as u32;
        let height = frame.height as u32;

        let planes = YuvPlaneData {
            y: std::mem::take(&mut frame.y_data),
            u: std::mem::take(&mut frame.u_data),
            v: std::mem::take(&mut frame.v_data),
            width,
            height,
            layout: YuvLayout::I420,
            matrix: YuvColorMatrix::BT601,
        };

        upload_yuv_to_gl(gl, textures, self.tex_y_id, self.tex_u_id, self.tex_v_id, &planes);

        self.width = width;
        self.height = height;
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
