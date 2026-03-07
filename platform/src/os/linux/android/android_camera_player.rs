//! Android NDK camera as a video playback source — captures YUV frames and uploads to GL textures.

use {
    super::super::gl_sys::LibGl,
    super::super::gl_video_upload::upload_i420_slices_to_gl,
    super::acamera_sys::ANativeWindow,
    super::android_camera::AndroidCameraAccess,
    crate::{
        makepad_live_id::LiveId,
        texture::{CxTexturePool, TextureId},
        video::*,
    },
    std::sync::{Arc, Mutex},
};

/// Camera player that captures Android NDK camera frames and uploads Y/U/V planes to GL textures.
pub struct AndroidCameraPlayer {
    pub video_id: LiveId,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
    width: u32,
    height: u32,
    prepared: bool,
    prepare_notified: bool,
    native_preview: bool,
    yuv_rotation_steps: f32,
    i420_frames: Option<CameraFrameLatest>,
    camera_access: Option<Arc<Mutex<AndroidCameraAccess>>>,
}

impl AndroidCameraPlayer {
    pub fn new(
        video_id: LiveId,
        tex_y_id: TextureId,
        tex_u_id: TextureId,
        tex_v_id: TextureId,
        input_id: VideoInputId,
        format_id: VideoFormatId,
        native_preview: bool,
        preview_window: Option<*mut ANativeWindow>,
        camera_access: Arc<Mutex<AndroidCameraAccess>>,
    ) -> Self {
        let i420_frames = if native_preview {
            None
        } else {
            Some(CameraFrameLatest::new(4))
        };

        let frame_cb = i420_frames.as_ref().map(|frames| {
            let frame_ring = frames.ring();
            Box::new(move |frame_ref: CameraFrameRef<'_>| {
                let _ = frame_ring.publish_i420_copy(frame_ref);
            }) as CameraFrameInputFn
        });

        let (width, height, yuv_rotation_steps) = {
            let mut cam = camera_access.lock().unwrap();
            let (width, height) = cam.format_size(input_id, format_id).unwrap_or((0, 0));
            let sensor_orientation = cam.sensor_orientation_for_input(input_id).rem_euclid(360);
            let yuv_rotation_steps = ((sensor_orientation / 90) % 4) as f32;

            cam.register_preview(video_id, input_id, format_id, frame_cb, preview_window);

            (width, height, yuv_rotation_steps)
        };

        Self {
            video_id,
            tex_y_id,
            tex_u_id,
            tex_v_id,
            width,
            height,
            prepared: native_preview,
            prepare_notified: false,
            native_preview,
            yuv_rotation_steps,
            i420_frames,
            camera_access: Some(camera_access),
        }
    }

    pub fn uses_textures(&self) -> bool {
        !self.native_preview
    }

    pub fn yuv_rotation_steps(&self) -> f32 {
        self.yuv_rotation_steps
    }

    pub fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
        if self.prepare_notified {
            return None;
        }

        if self.native_preview && self.prepared {
            self.prepare_notified = true;
            return Some(Ok((
                self.width,
                self.height,
                0,
                false,
                vec!["camera".to_string()],
                vec![],
            )));
        }

        let frames = self.i420_frames.as_mut()?;
        if !frames.prime_pending_from_latest() {
            return None;
        }

        let (width, height) = {
            let frame = frames.pending_frame()?;
            (frame.width as u32, frame.height as u32)
        };
        self.width = width;
        self.height = height;
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
        if self.native_preview {
            return false;
        }

        let Some(frame) = self
            .i420_frames
            .as_mut()
            .and_then(CameraFrameLatest::take_pending_or_latest)
        else {
            return false;
        };

        if frame.width == 0 || frame.height == 0 || frame.plane_count < 3 {
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

        self.width = width;
        self.height = height;
        true
    }

    pub fn set_preview_window(&mut self, preview_window: Option<*mut ANativeWindow>) {
        if let Some(cam) = self.camera_access.as_ref() {
            cam.lock()
                .unwrap()
                .update_preview_window(self.video_id, preview_window);
        }
    }

    pub fn cleanup(&mut self) {
        if let Some(cam) = self.camera_access.take() {
            cam.lock().unwrap().unregister_preview(self.video_id);
        }
    }
}

impl Drop for AndroidCameraPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}
