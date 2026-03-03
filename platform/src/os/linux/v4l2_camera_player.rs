//! V4L2 camera as a video playback source — captures frames and uploads YUV planes to GL.

use {
    super::gl_sys::LibGl,
    super::gl_video_upload::upload_i420_slices_to_gl,
    super::v4l2_camera::V4l2CameraAccess,
    crate::{
        makepad_live_id::LiveId,
        texture::{CxTexturePool, TextureId},
        video::*,
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
    frame_pool: Arc<Mutex<CameraFramePool>>,
    camera_access: Option<Arc<Mutex<V4l2CameraAccess>>>,
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
        let frame_pool = Arc::new(Mutex::new(CameraFramePool::new(4)));

        let frame_pool_clone = frame_pool.clone();
        let cb: CameraFrameInputFn = Box::new(move |frame_ref: CameraFrameRef<'_>| {
            let mut pool = frame_pool_clone.lock().unwrap();
            let mut frame = pool.checkout();

            if !convert_to_i420(&mut frame, frame_ref) {
                pool.recycle(frame);
                return;
            }

            pool.publish_latest(frame);
        });

        {
            let mut cam = camera_access.lock().unwrap();
            *cam.camera_frame_input_cb[0].lock().unwrap() = Some(cb);
            *cam.video_input_cb[0].lock().unwrap() = None;
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
            frame_pool,
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
        let mut pool = self.frame_pool.lock().unwrap();
        let frame = pool.take_latest()?;
        self.width = frame.width as u32;
        self.height = frame.height as u32;
        self.prepared = true;
        self.prepare_notified = true;
        pool.publish_latest(frame);
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
        let frame = {
            let mut pool = self.frame_pool.lock().unwrap();
            match pool.take_latest() {
                Some(frame) => frame,
                None => return false,
            }
        };

        if frame.width == 0 || frame.height == 0 || frame.plane_count < 3 {
            let mut pool = self.frame_pool.lock().unwrap();
            pool.recycle(frame);
            return false;
        }

        let width = frame.width as u32;
        let height = frame.height as u32;
        upload_i420_slices_to_gl(
            gl,
            textures,
            self.tex_y_id,
            self.tex_u_id,
            self.tex_v_id,
            &frame.planes[0].bytes,
            &frame.planes[1].bytes,
            &frame.planes[2].bytes,
            width,
            height,
        );

        let mut pool = self.frame_pool.lock().unwrap();
        pool.recycle(frame);

        self.width = width;
        self.height = height;
        true
    }

    pub fn cleanup(&mut self) {
        if let Some(cam) = self.camera_access.take() {
            let mut cam = cam.lock().unwrap();
            cam.use_video_input(&[]);
            *cam.camera_frame_input_cb[0].lock().unwrap() = None;
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

fn copy_strided_plane(
    src: CameraFramePlaneRef<'_>,
    width: usize,
    height: usize,
    dst: &mut Vec<u8>,
) {
    dst.resize(width * height, 0);
    if src.bytes.is_empty() || width == 0 || height == 0 {
        return;
    }

    if src.pixel_stride == 1 && src.row_stride == width {
        let n = dst.len().min(src.bytes.len());
        dst[..n].copy_from_slice(&src.bytes[..n]);
        return;
    }

    for row in 0..height {
        let src_row = row.saturating_mul(src.row_stride);
        let dst_row = row * width;
        for col in 0..width {
            let src_idx = src_row + col.saturating_mul(src.pixel_stride.max(1));
            dst[dst_row + col] = src.bytes.get(src_idx).copied().unwrap_or(0);
        }
    }
}

fn convert_to_i420(dst: &mut CameraFrameOwned, src: CameraFrameRef<'_>) -> bool {
    let w = src.width;
    let h = src.height;
    if w == 0 || h == 0 {
        return false;
    }

    dst.timestamp_ns = src.timestamp_ns;
    dst.width = w;
    dst.height = h;
    dst.layout = CameraFrameLayout::I420;
    dst.matrix = src.matrix;
    dst.plane_count = 3;
    dst.planes[0].row_stride = w;
    dst.planes[0].pixel_stride = 1;
    dst.planes[1].row_stride = w.div_ceil(2);
    dst.planes[1].pixel_stride = 1;
    dst.planes[2].row_stride = w.div_ceil(2);
    dst.planes[2].pixel_stride = 1;

    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);

    match src.layout {
        CameraFrameLayout::I420 => {
            if src.plane_count < 3 {
                return false;
            }
            copy_strided_plane(src.planes[0], w, h, &mut dst.planes[0].bytes);
            copy_strided_plane(src.planes[1], cw, ch, &mut dst.planes[1].bytes);
            copy_strided_plane(src.planes[2], cw, ch, &mut dst.planes[2].bytes);
            true
        }
        CameraFrameLayout::NV12 => {
            if src.plane_count < 2 {
                return false;
            }
            copy_strided_plane(src.planes[0], w, h, &mut dst.planes[0].bytes);

            dst.planes[1].bytes.resize(cw * ch, 128);
            dst.planes[2].bytes.resize(cw * ch, 128);
            let uv = src.planes[1];
            for row in 0..ch {
                let src_row = row.saturating_mul(uv.row_stride);
                let dst_row = row * cw;
                for col in 0..cw {
                    let base = src_row + col.saturating_mul(uv.pixel_stride.max(2));
                    dst.planes[1].bytes[dst_row + col] = uv.bytes.get(base).copied().unwrap_or(128);
                    dst.planes[2].bytes[dst_row + col] =
                        uv.bytes.get(base + 1).copied().unwrap_or(128);
                }
            }
            true
        }
        CameraFrameLayout::YUY2 => {
            if src.plane_count < 1 {
                return false;
            }
            let packed = src.planes[0];
            dst.planes[0].bytes.resize(w * h, 16);
            dst.planes[1].bytes.resize(cw * ch, 128);
            dst.planes[2].bytes.resize(cw * ch, 128);

            let half_w = w.div_ceil(2);
            let mut u_full = vec![128u8; half_w * h];
            let mut v_full = vec![128u8; half_w * h];

            for row in 0..h {
                let src_row = row.saturating_mul(packed.row_stride);
                for pair in 0..half_w {
                    let base = src_row + pair * 4;
                    let y0 = packed.bytes.get(base).copied().unwrap_or(16);
                    let u = packed.bytes.get(base + 1).copied().unwrap_or(128);
                    let y1 = packed.bytes.get(base + 2).copied().unwrap_or(16);
                    let v = packed.bytes.get(base + 3).copied().unwrap_or(128);

                    let px0 = row * w + pair * 2;
                    if px0 < dst.planes[0].bytes.len() {
                        dst.planes[0].bytes[px0] = y0;
                    }
                    let px1 = px0 + 1;
                    if px1 < dst.planes[0].bytes.len() {
                        dst.planes[0].bytes[px1] = y1;
                    }

                    u_full[row * half_w + pair] = u;
                    v_full[row * half_w + pair] = v;
                }
            }

            for row in 0..ch {
                let r0 = row * 2;
                let r1 = (r0 + 1).min(h - 1);
                for col in 0..cw {
                    let u0 = u_full[r0 * half_w + col.min(half_w - 1)] as u16;
                    let u1 = u_full[r1 * half_w + col.min(half_w - 1)] as u16;
                    dst.planes[1].bytes[row * cw + col] = ((u0 + u1 + 1) / 2) as u8;

                    let v0 = v_full[r0 * half_w + col.min(half_w - 1)] as u16;
                    let v1 = v_full[r1 * half_w + col.min(half_w - 1)] as u16;
                    dst.planes[2].bytes[row * cw + col] = ((v0 + v1 + 1) / 2) as u8;
                }
            }
            true
        }
        _ => false,
    }
}
