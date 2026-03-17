//! Android NDK camera as a video playback source.

use {
    super::super::gl_sys::LibGl,
    super::super::gl_video_upload::upload_i420_slices_to_gl,
    super::acamera_sys::ANativeWindow,
    super::android_camera::{
        AndroidCameraAccess, AndroidCameraHardwareBufferFrame, CameraHardwareBufferInputFn,
    },
    crate::{
        makepad_live_id::LiveId,
        texture::{CxTexturePool, TextureFormat, TextureId, TextureUpdated},
        video::*,
        PlaybackPrepared,
    },
    std::{
        sync::{Arc, Mutex},
        time::Instant,
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum AndroidCameraTextureMode {
    GlYuv,
    CpuYuv,
    HardwareBufferExternal,
}

pub struct AndroidCameraPlayer {
    pub video_id: LiveId,
    texture_id: TextureId,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
    input_id: VideoInputId,
    format_id: VideoFormatId,
    width: u32,
    height: u32,
    prepared: bool,
    prepare_notified: bool,
    native_preview: bool,
    texture_mode: AndroidCameraTextureMode,
    yuv_rotation_steps: f32,
    i420_frames: Option<CameraFrameLatest>,
    hardware_buffer_frame: Option<Arc<Mutex<Option<AndroidCameraHardwareBufferFrame>>>>,
    camera_access: Option<Arc<Mutex<AndroidCameraAccess>>>,
    created_at: Instant,
    warned_waiting_for_first_frame: bool,
    logged_first_hardware_buffer_consume: bool,
}

impl AndroidCameraPlayer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        video_id: LiveId,
        texture_id: TextureId,
        tex_y_id: TextureId,
        tex_u_id: TextureId,
        tex_v_id: TextureId,
        input_id: VideoInputId,
        format_id: VideoFormatId,
        native_preview: bool,
        use_hardware_buffer_texture: bool,
        use_cpu_plane_textures: bool,
        preview_window: Option<*mut ANativeWindow>,
        camera_access: Arc<Mutex<AndroidCameraAccess>>,
    ) -> Self {
        let texture_mode = if native_preview {
            AndroidCameraTextureMode::GlYuv
        } else if use_hardware_buffer_texture {
            AndroidCameraTextureMode::HardwareBufferExternal
        } else if use_cpu_plane_textures {
            AndroidCameraTextureMode::CpuYuv
        } else {
            AndroidCameraTextureMode::GlYuv
        };

        let i420_frames =
            if native_preview || texture_mode == AndroidCameraTextureMode::HardwareBufferExternal {
                None
            } else {
                Some(CameraFrameLatest::new(4))
            };
        let hardware_buffer_frame =
            if texture_mode == AndroidCameraTextureMode::HardwareBufferExternal {
                Some(Arc::new(Mutex::new(None)))
            } else {
                None
            };

        let frame_cb = i420_frames.as_ref().map(|frames| {
            let frame_ring = frames.ring();
            Box::new(move |frame_ref: CameraFrameRef<'_>| {
                let _ = frame_ring.publish_i420_copy(frame_ref);
            }) as CameraFrameInputFn
        });
        let hardware_buffer_cb = hardware_buffer_frame.as_ref().map(|latest| {
            let latest = latest.clone();
            Box::new(move |frame: AndroidCameraHardwareBufferFrame| {
                *latest.lock().unwrap() = Some(frame);
            }) as CameraHardwareBufferInputFn
        });

        let (width, height, yuv_rotation_steps) = {
            let mut cam = camera_access.lock().unwrap();
            let (width, height) = cam.format_size(input_id, format_id).unwrap_or((0, 0));
            let sensor_orientation = cam.sensor_orientation_for_input(input_id).rem_euclid(360);
            let yuv_rotation_steps = ((sensor_orientation / 90) % 4) as f32;

            match hardware_buffer_cb {
                Some(hardware_buffer_cb) => cam.register_preview_hardware_buffer(
                    video_id,
                    input_id,
                    format_id,
                    hardware_buffer_cb,
                    preview_window,
                ),
                None => {
                    cam.register_preview(video_id, input_id, format_id, frame_cb, preview_window)
                }
            }

            (width, height, yuv_rotation_steps)
        };

        Self {
            video_id,
            texture_id,
            tex_y_id,
            tex_u_id,
            tex_v_id,
            input_id,
            format_id,
            width,
            height,
            prepared: native_preview,
            prepare_notified: false,
            native_preview,
            texture_mode,
            yuv_rotation_steps,
            i420_frames,
            hardware_buffer_frame,
            camera_access: Some(camera_access),
            created_at: Instant::now(),
            warned_waiting_for_first_frame: false,
            logged_first_hardware_buffer_consume: false,
        }
    }

    pub fn tex_y_id(&self) -> TextureId {
        self.tex_y_id
    }

    pub fn texture_id(&self) -> TextureId {
        self.texture_id
    }

    pub fn tex_u_id(&self) -> TextureId {
        self.tex_u_id
    }

    pub fn tex_v_id(&self) -> TextureId {
        self.tex_v_id
    }

    pub fn uses_textures(&self) -> bool {
        !self.native_preview
    }

    pub fn needs_gl_upload(&self) -> bool {
        !self.native_preview && self.texture_mode == AndroidCameraTextureMode::GlYuv
    }

    pub fn uses_hardware_buffer_texture(&self) -> bool {
        self.texture_mode == AndroidCameraTextureMode::HardwareBufferExternal
    }

    pub fn fallback_to_cpu_yuv(&mut self) -> Result<(), String> {
        if self.texture_mode != AndroidCameraTextureMode::HardwareBufferExternal {
            return Ok(());
        }
        let Some(camera_access) = self.camera_access.as_ref().cloned() else {
            return Err(
                "Android headset camera fallback failed: missing camera access".to_string(),
            );
        };

        let frames = CameraFrameLatest::new(4);
        let frame_ring = frames.ring();
        let frame_cb = Box::new(move |frame_ref: CameraFrameRef<'_>| {
            let _ = frame_ring.publish_i420_copy(frame_ref);
        }) as CameraFrameInputFn;

        {
            let mut cam = camera_access.lock().unwrap();
            cam.unregister_preview(self.video_id);
            cam.register_preview(
                self.video_id,
                self.input_id,
                self.format_id,
                Some(frame_cb),
                None,
            );
        }

        self.texture_mode = AndroidCameraTextureMode::CpuYuv;
        self.i420_frames = Some(frames);
        self.hardware_buffer_frame = None;
        self.warned_waiting_for_first_frame = false;
        self.logged_first_hardware_buffer_consume = false;
        crate::warning!(
            "Android headset camera player: falling back to cpu-yuv video_id={} size={}x{}",
            self.video_id.0,
            self.width,
            self.height,
        );
        Ok(())
    }

    pub fn yuv_rotation_steps(&self) -> f32 {
        self.yuv_rotation_steps
    }

    pub fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        if self.prepare_notified {
            return None;
        }

        if self.native_preview && self.prepared {
            self.prepare_notified = true;
            return Some(Ok(PlaybackPrepared::new(
                self.width,
                self.height,
                0,
                false,
                vec!["camera".to_string()],
                vec![],
            )));
        }

        if let Some(latest) = self.hardware_buffer_frame.as_ref() {
            let guard = latest.lock().unwrap();
            let Some(frame) = guard.as_ref() else {
                if !self.warned_waiting_for_first_frame
                    && self.created_at.elapsed().as_secs_f32() >= 2.0
                {
                    self.warned_waiting_for_first_frame = true;
                }
                return None;
            };
            self.width = frame.width;
            self.height = frame.height;
            self.prepared = true;
            self.prepare_notified = true;
            return Some(Ok(PlaybackPrepared::new(
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
        Some(Ok(PlaybackPrepared::new(
            self.width,
            self.height,
            0,
            false,
            vec!["camera".to_string()],
            vec![],
        )))
    }

    pub fn take_hardware_buffer_frame(&mut self) -> Option<AndroidCameraHardwareBufferFrame> {
        let latest = self.hardware_buffer_frame.as_ref()?;
        let frame = latest.lock().unwrap().take()?;
        self.logged_first_hardware_buffer_consume = true;
        self.width = frame.width;
        self.height = frame.height;
        self.prepared = true;
        Some(frame)
    }

    pub fn poll_frame(&mut self, gl: Option<&LibGl>, textures: &mut CxTexturePool) -> bool {
        if self.native_preview || self.uses_hardware_buffer_texture() {
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

        if self.texture_mode == AndroidCameraTextureMode::CpuYuv {
            replace_r8_plane_texture(
                textures,
                self.tex_y_id,
                width as usize,
                height as usize,
                frame.planes[0].bytes.clone(),
            );
            replace_r8_plane_texture(
                textures,
                self.tex_u_id,
                width.div_ceil(2) as usize,
                height.div_ceil(2) as usize,
                frame.planes[1].bytes.clone(),
            );
            replace_r8_plane_texture(
                textures,
                self.tex_v_id,
                width.div_ceil(2) as usize,
                height.div_ceil(2) as usize,
                frame.planes[2].bytes.clone(),
            );
        } else {
            let Some(gl) = gl else {
                return false;
            };
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
        }

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

fn replace_r8_plane_texture(
    textures: &mut CxTexturePool,
    texture_id: TextureId,
    width: usize,
    height: usize,
    data: Vec<u8>,
) {
    let texture = &mut textures[texture_id];
    match &mut texture.format {
        TextureFormat::VecRu8 {
            width: texture_width,
            height: texture_height,
            data: texture_data,
            updated,
            ..
        } => {
            *texture_width = width;
            *texture_height = height;
            *texture_data = Some(data);
            *updated = updated.clone().update(None);
        }
        TextureFormat::VideoYuvPlane => {
            texture.format = TextureFormat::VecRu8 {
                width,
                height,
                data: Some(data),
                unpack_row_length: None,
                updated: TextureUpdated::Full,
            };
        }
        _ => {}
    }
}
