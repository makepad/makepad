//! A copy of what a window shows, taken on the GPU at its next paint: the
//! frozen picture a transition fades out over whatever is drawn next.
//!
//! Nothing is allocated or copied for a window that never asks. A request
//! repaints the window once; the backend copies the frame it presents into
//! the request's texture in the same command buffer, so the copy is exactly
//! the picture on screen and is ready for any later pass to sample. Where
//! the backend cannot copy its presented frame the request says so at once
//! ([`WindowSnapshotState::Unavailable`]) and the caller simply goes on.

use crate::{
    cx::Cx,
    cx_api::CxOsApi,
    texture::{Texture, TextureFormat, TextureSize},
    window::WindowId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowSnapshotState {
    /// Waiting for the window's next paint.
    Pending,
    /// The texture holds the frame the window presented.
    Captured,
    /// No snapshot will come (this backend cannot copy its frame, or the
    /// request was released).
    Unavailable,
}

pub(crate) struct WindowSnapshotRequest {
    pub window_id: WindowId,
    pub texture: Texture,
    pub state: WindowSnapshotState,
}

impl Cx {
    /// Copy what `window_id` shows into a new texture at its next paint
    /// (which this requests). Poll [`Cx::window_snapshot_state`]; release it
    /// with [`Cx::release_window_snapshot`] when done.
    pub fn request_window_snapshot(&mut self, window_id: WindowId) -> Texture {
        let texture = Texture::new_with_format(self, TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true });
        let supported = cfg!(any(target_os = "macos", target_os = "ios", target_os = "tvos"));
        let state = if supported { WindowSnapshotState::Pending } else { WindowSnapshotState::Unavailable };
        self.window_snapshots.push(WindowSnapshotRequest { window_id, texture: texture.clone(), state });
        if supported {
            if let Some(pass) = self.windows[window_id].main_pass_id {
                self.repaint_pass(pass);
            }
        }
        texture
    }

    pub fn window_snapshot_state(&self, texture: &Texture) -> WindowSnapshotState {
        self.window_snapshots
            .iter()
            .find(|r| r.texture.texture_id() == texture.texture_id())
            .map(|r| r.state)
            .unwrap_or(WindowSnapshotState::Unavailable)
    }

    /// Forget a snapshot; its GPU copy goes with the texture's last handle.
    pub fn release_window_snapshot(&mut self, texture: &Texture) {
        self.window_snapshots.retain(|r| r.texture.texture_id() != texture.texture_id());
    }

    /// For the next `secs`, present only whole window frames: one with a
    /// draw still waiting for its pipeline is not shown, and the last whole
    /// frame stays up. For a change (a new layout) that should appear in one
    /// step, not build up over several frames while the driver compiles.
    pub fn present_whole_frames_for(&mut self, secs: f64) {
        let now = self.seconds_since_app_start();
        let until = now + secs;
        if until > self.whole_frames_until {
            self.whole_frames_until = until;
        }
        self.whole_hold_began.get_or_insert(now);
    }

    /// The frame being drawn is not finished (the app lays out again on the
    /// next one): within a `present_whole_frames_for`, it is not shown.
    pub fn hold_this_frame(&mut self) {
        if self.seconds_since_app_start() < self.whole_frames_until {
            self.hold_next_paint = true;
        }
    }

    /// Draws left out of frames so far because they were not ready (their
    /// pipeline compiling, their instances not yet on the GPU).
    pub fn pipeline_skips(&self) -> u64 {
        self.pipeline_skips
    }

    /// Whether a paint of window `index` should copy its frame.
    #[allow(dead_code)]
    pub(crate) fn window_snapshot_wanted(&self, index: usize) -> bool {
        self.window_snapshots.iter().any(|r| r.window_id.0 == index && r.state == WindowSnapshotState::Pending)
    }
}
