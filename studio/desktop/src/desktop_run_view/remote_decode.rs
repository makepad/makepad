use super::*;
use makepad_studio_protocol::hub_protocol::FrameCodec;
use makepad_studio_protocol::RunViewFrameRequest;
use crate::makepad_widgets::image_cache::{
    load_image_from_cache, load_image_from_data_async, process_async_image_load, AsyncImageLoad,
    AsyncLoadResult,
};

impl DesktopRunView {
    pub(crate) fn set_remote_frame(&mut self, cx: &mut Cx, build_id: QueryId, frame: RunViewFrameData) {
        let Some(target) = self.current_target else {
            return;
        };
        if target.build_id != build_id || target.window_id != frame.window_id {
            return;
        }
        if frame.frame_id < self.remote_current_frame_id {
            return;
        }
        let codec = frame.codec.clone().unwrap_or(FrameCodec::Png);
        self.remote_mode = true;
        self.remote_frame_request_in_flight = false;
        self.remote_requested_frame_id = None;

        let ext = match codec {
            FrameCodec::Png => "png",
            FrameCodec::Jpeg => "jpg",
            FrameCodec::ZstdRgba => return,
        };
        if let Some(prev_path) = self.remote_current_path.take() {
            Self::clear_cached_remote_path(cx, &prev_path);
        }
        if let Some(pending) = self.remote_pending_decode.take() {
            Self::clear_cached_remote_path(cx, &pending.path);
        }
        let path = PathBuf::from(format!(
            "studio_remote_runview://build-{}-window-{}-frame-{}.{}",
            build_id.0, frame.window_id, frame.frame_id, ext
        ));
        let bytes = Arc::new(frame.data);
        match load_image_from_data_async(cx, &path, bytes) {
            Ok(AsyncLoadResult::Loaded) => {
                if let Some(texture) = load_image_from_cache(cx, &path) {
                    let y_flip = if cfg!(all(target_os = "linux", not(target_env = "ohos"))) {
                        1.0
                    } else {
                        0.0
                    };
                    self.apply_remote_texture(cx, &texture, frame.width, frame.height, y_flip);
                    self.remote_current_frame_id = frame.frame_id;
                    self.remote_current_path = Some(path);
                    return;
                }
            }
            Ok(AsyncLoadResult::Loading(_, _)) => {}
            Err(_) => {
                crate::log!(
                    "runview remote frame decode start failed build={} frame={}",
                    build_id.0,
                    frame.frame_id,
                );
                Self::clear_cached_remote_path(cx, &path);
                return;
            }
        }
        self.remote_pending_decode = Some(PendingRemoteDecode {
            path,
            frame_id: frame.frame_id,
            width: frame.width,
            height: frame.height,
        });
        self.redraw(cx);
    }

    pub(crate) fn handle_remote_decode_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(AsyncImageLoad { image_path, result }) = action.downcast_ref() else {
                continue;
            };
            let Some((pending_path, pending_frame_id, pending_width, pending_height)) =
                self.remote_pending_decode.as_ref().map(|pending| {
                    (
                        pending.path.clone(),
                        pending.frame_id,
                        pending.width,
                        pending.height,
                    )
                })
            else {
                continue;
            };
            if image_path != &pending_path {
                continue;
            }
            if let Some(result) = result.borrow_mut().take() {
                process_async_image_load(cx, image_path, result);
            }
            if let Some(texture) = load_image_from_cache(cx, image_path) {
                let y_flip = if cfg!(all(target_os = "linux", not(target_env = "ohos"))) {
                    1.0
                } else {
                    0.0
                };
                self.apply_remote_texture(cx, &texture, pending_width, pending_height, y_flip);
                self.remote_current_frame_id = pending_frame_id;
                self.remote_current_path = Some(pending_path);
                self.remote_pending_decode = None;
            }
        }
    }

    pub(crate) fn apply_remote_texture(
        &mut self,
        cx: &mut Cx,
        texture: &Texture,
        width: u32,
        height: u32,
        y_flip: f32,
    ) {
        self.draw_app.set_texture(0, texture);
        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(tex_scale), &[1.0f32, 1.0f32]);
        self.draw_app.draw_vars.set_dyn_instance(
            cx,
            id!(tex_size),
            &[width.max(1) as f32, height.max(1) as f32],
        );
        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(y_flip), &[y_flip]);
        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(packed_header), &[0.0f32]);
        self.redraw_countdown = self.redraw_countdown.max(20);
        self.redraw(cx);
    }

    pub(crate) fn clear_cached_remote_path(cx: &mut Cx, path: &PathBuf) {
        cx.global::<crate::makepad_widgets::image_cache::ImageCache>()
            .map
            .remove(path);
    }

    pub(crate) fn request_remote_frame_if_needed(&mut self, target: RunTarget) -> Option<StudioToApp> {
        if self.last_rect.size.x <= 0.0 || self.last_rect.size.y <= 0.0 {
            return None;
        }
        if self.remote_frame_request_in_flight || self.remote_pending_decode.is_some() {
            return None;
        }
        if !self.remote_mode && self.debug_present_ok_count > 0 {
            return None;
        }
        let frame_id = self.remote_next_frame_id.max(1);
        self.remote_next_frame_id = frame_id.wrapping_add(1).max(1);
        self.remote_frame_request_in_flight = true;
        self.remote_requested_frame_id = Some(frame_id);
        Some(StudioToApp::RunViewFrameRequest(RunViewFrameRequest {
            window_id: target.window_id,
            frame_id,
            width: (self.last_rect.size.x * self.last_dpi_factor)
                .ceil()
                .max(1.0) as u32,
            height: (self.last_rect.size.y * self.last_dpi_factor)
                .ceil()
                .max(1.0) as u32,
            dpi_factor: self.last_dpi_factor,
        }))
    }
}
