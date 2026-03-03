//! V4L2 camera as a video playback source — captures frames and uploads YUV planes to GL.

use {
    super::gl_sys::LibGl,
    super::gl_video_upload::upload_yuv_to_gl,
    super::v4l2_camera::V4l2CameraAccess,
    crate::{
        makepad_live_id::LiveId,
        texture::{CxTexturePool, TextureId},
        video::*,
        video_decode::yuv::{YuvColorMatrix, YuvLayout, YuvPlaneData},
    },
    std::sync::{Arc, Mutex},
};

/// Camera player that captures V4L2 frames and uploads Y/U/V planes to GL textures.
pub struct V4l2CameraPlayer {
    pub video_id: LiveId,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
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

impl V4l2CameraPlayer {
    pub fn new(
        video_id: LiveId,
        tex_y_id: TextureId,
        tex_u_id: TextureId,
        tex_v_id: TextureId,
        input_id: VideoInputId,
        format_id: VideoFormatId,
        camera_access: Arc<Mutex<V4l2CameraAccess>>,
    ) -> Self {
        let frame_buf = Arc::new(Mutex::new(CameraFrame::default()));

        // Set up the capture callback that deinterleaves frames into Y/U/V planes
        let frame_buf_clone = frame_buf.clone();
        let cb: VideoInputFn = Box::new(move |buffer: VideoBufferRef| {
            let w = buffer.format.width;
            let h = buffer.format.height;

            let mut y_data = Vec::new();
            let mut u_data = Vec::new();
            let mut v_data = Vec::new();

            match buffer.format.pixel_format {
                VideoPixelFormat::YUY2 => {
                    if let VideoBufferRefData::U32(data) = &buffer.data {
                        // YUY2: packed Y0 U0 Y1 V0 per u32 (2 pixels).
                        // Y plane: full resolution (w*h)
                        // U,V planes after vertical subsampling: (w/2 * h/2) for I420
                        let half_w = (w + 1) / 2;
                        y_data = Vec::with_capacity(w * h);
                        // Temporary full-height chroma for 4:2:2
                        let mut u_full = Vec::with_capacity(half_w * h);
                        let mut v_full = Vec::with_capacity(half_w * h);

                        for row in 0..h {
                            let row_off = row * (w / 2);
                            for col_pair in 0..half_w {
                                let i = row_off + col_pair;
                                if i >= data.len() { break; }
                                let packed = data[i];
                                let y0 = (packed & 0xff) as u8;
                                let u  = ((packed >> 8) & 0xff) as u8;
                                let y1 = ((packed >> 16) & 0xff) as u8;
                                let v  = ((packed >> 24) & 0xff) as u8;
                                y_data.push(y0);
                                if 2 * col_pair + 1 < w {
                                    y_data.push(y1);
                                }
                                u_full.push(u);
                                v_full.push(v);
                            }
                        }

                        // Vertically subsample U and V: average pairs of rows → h/2
                        let half_h = (h + 1) / 2;
                        u_data = Vec::with_capacity(half_w * half_h);
                        v_data = Vec::with_capacity(half_w * half_h);
                        for row in 0..half_h {
                            let r0 = row * 2;
                            let r1 = (r0 + 1).min(h - 1);
                            for col in 0..half_w {
                                let u0 = u_full[r0 * half_w + col] as u16;
                                let u1 = u_full[r1 * half_w + col] as u16;
                                u_data.push(((u0 + u1 + 1) / 2) as u8);
                                let v0 = v_full[r0 * half_w + col] as u16;
                                let v1 = v_full[r1 * half_w + col] as u16;
                                v_data.push(((v0 + v1 + 1) / 2) as u8);
                            }
                        }
                    }
                }
                VideoPixelFormat::NV12 => {
                    if let VideoBufferRefData::U32(data) = &buffer.data {
                        // NV12: Y plane (w*h bytes) then interleaved UV (w/2 * h/2 pairs)
                        let bytes: &[u8] = unsafe {
                            std::slice::from_raw_parts(
                                data.as_ptr() as *const u8,
                                data.len() * 4,
                            )
                        };
                        let y_size = w * h;
                        if bytes.len() >= y_size {
                            y_data = bytes[..y_size].to_vec();
                        }
                        let half_w = (w + 1) / 2;
                        let half_h = (h + 1) / 2;
                        let uv_start = y_size;
                        let uv_size = half_w * half_h;
                        u_data = Vec::with_capacity(uv_size);
                        v_data = Vec::with_capacity(uv_size);
                        let uv_bytes = &bytes[uv_start..];
                        for i in 0..uv_size {
                            let idx = i * 2;
                            if idx + 1 < uv_bytes.len() {
                                u_data.push(uv_bytes[idx]);
                                v_data.push(uv_bytes[idx + 1]);
                            } else {
                                u_data.push(128);
                                v_data.push(128);
                            }
                        }
                    }
                }
                _ => {
                    // MJPEG, RGB24, etc.: fill with black YUV as fallback
                    y_data = vec![16; w * h];
                    let half_w = (w + 1) / 2;
                    let half_h = (h + 1) / 2;
                    u_data = vec![128; half_w * half_h];
                    v_data = vec![128; half_w * half_h];
                }
            }

            let half_w = (w + 1) / 2;
            let half_h = (h + 1) / 2;
            if y_data.len() == w * h
                && u_data.len() == half_w * half_h
                && v_data.len() == half_w * half_h
            {
                let mut frame = frame_buf_clone.lock().unwrap();
                frame.y_data = y_data;
                frame.u_data = u_data;
                frame.v_data = v_data;
                frame.width = w;
                frame.height = h;
                frame.new = true;
            }
        });

        // Start the V4L2 capture by registering callback at index 0 and calling use_video_input
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
        // Drop lock before GL calls
        drop(frame);

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

impl Drop for V4l2CameraPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}
