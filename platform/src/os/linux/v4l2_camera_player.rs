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

            if !frame.convert_to_i420(frame_ref) {
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


