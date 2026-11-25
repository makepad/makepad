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
    draw_pass::{DrawPassId, PassLayout},
    draw_shader::DrawShaderId,
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

    /// Shaders whose GPU pipelines are still compiling (0 where the
    /// backend compiles synchronously).
    pub fn pipelines_pending(&self) -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        { self.hal.frames.pipelines_pending() }
        #[cfg(target_arch = "wasm32")]
        { 0 }
    }

    /// Whether a draw shader can draw now. Metal builds pipelines
    /// asynchronously and leaves a draw out of the frame until they exist;
    /// a caller with a fallback (another shader for the same geometry) asks
    /// this and keeps drawing the fallback until the new one is ready, so
    /// content never vanishes while a shader compiles. `float16` asks for
    /// the RGBA16Float (HDR target) pipeline too, and starts building it.
    /// True where the backend compiles synchronously. After a `false`,
    /// [`Event::DrawShadersReady`](crate::event::Event::DrawShadersReady) says
    /// when to ask again: no need to ask every frame.
    pub fn draw_shader_ready(&self, shader: crate::draw_shader::DrawShaderId, float16: bool) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = float16;
            self.is_draw_shader_window_ready(shader)
        }
        #[cfg(target_arch = "wasm32")]
        { let _ = (shader, float16); true }
    }

    /// Whether this shader can draw into `pass` this frame without skipping the draw or
    /// compiling pipeline state at submit. True on wasm and where compilation is synchronous.
    /// After a `false`, [`Event::DrawShadersReady`](crate::event::Event::DrawShadersReady)
    /// tells the caller when to ask again, as for [`Self::draw_shader_ready`].
    pub fn draw_shader_ready_in(&self, shader: DrawShaderId, pass: DrawPassId) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        { self.is_draw_shader_pass_ready(shader, pass) }
        #[cfg(target_arch = "wasm32")]
        { let _ = (shader, pass); true }
    }

    /// The driver's terminal diagnostic for this shader in the pass's current layout.
    /// A base shader failure applies to every pass. On a first query the HAL can
    /// still be matching a variant built by an earlier draw to this layout. In that
    /// case, as while compiling, `DrawShadersReady` wakes the caller to query again.
    pub fn draw_shader_failure_in(&self, shader: DrawShaderId, pass: DrawPassId) -> Option<&str> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(pipe) = self.draw_shaders[shader.index].pipeline_id else {
                self.hal.frames.asked_unready();
                return None;
            };
            if let Some(log) = self.hal.frames.pipeline_failure(pipe) { return Some(log); }
            if self.hal.device_lost || self.hal.info.is_some_and(|i| i.backend == makepad_hal::backend_id::HEADLESS) {
                return None;
            }
            let layout = self.fb_pass_layout(pass)?;
            match self.hal.frames.pipeline_status_in(pipe, layout) {
                Err(log) => Some(log),
                Ok(ready) => {
                    if !ready { self.hal.frames.asked_unready(); }
                    None
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        { let _ = (shader, pass); None }
    }

    /// The pipeline layout of the pass's current attachments, when its target is known.
    pub fn pass_layout(&self, pass: DrawPassId) -> Option<PassLayout> {
        #[cfg(not(target_arch = "wasm32"))]
        { self.fb_pass_layout(pass).map(PassLayout) }
        #[cfg(target_arch = "wasm32")]
        { let _ = pass; Some(PassLayout(())) }
    }

    /// Whether this shader can draw into `layout` without skipping the draw or compiling
    /// pipeline state at submit. After a `false`, [`Event::DrawShadersReady`](crate::event::Event::DrawShadersReady)
    /// tells the caller when to ask again. True on wasm and where compilation is synchronous.
    pub fn draw_shader_ready_for(&self, shader: DrawShaderId, layout: &PassLayout) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        { self.is_draw_shader_layout_ready(shader, || Some(layout.0)) }
        #[cfg(target_arch = "wasm32")]
        { let _ = (shader, layout); true }
    }

    /// Whether `shader`'s pipeline failed to build. It never draws, and no
    /// [`Event::DrawShadersReady`](crate::event::Event::DrawShadersReady) follows
    /// for it, so a caller waiting on [`Cx::draw_shader_ready`] stops waiting.
    /// False while it is still being built.
    pub fn draw_shader_failed(&self, shader: crate::draw_shader::DrawShaderId) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.is_draw_shader_window_failed(shader)
        }
        #[cfg(target_arch = "wasm32")]
        { let _ = shader; false }
    }

    /// What the backend said when `shader`'s pipeline failed to build (its
    /// compiler's log), while [`Self::draw_shader_failed`] holds: for a host
    /// to show the author why a shader draws nothing.
    pub fn draw_shader_failure(&self, shader: crate::draw_shader::DrawShaderId) -> Option<&str> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.draw_shader_window_failure(shader)
        }
        #[cfg(target_arch = "wasm32")]
        { let _ = shader; None }
    }

    /// Draws left out of frames so far because they were not ready (their
    /// pipeline compiling, their instances not yet on the GPU).
    pub fn pipeline_skips(&self) -> u64 {
        self.pipeline_skips
    }

    /// Those draws by reason: [instances not presentable yet, no compiled
    /// shader, pipeline still compiling].
    pub fn draw_skip_reasons(&self) -> [u64; 3] {
        self.skip_reasons
    }

    /// Whether a paint of window `index` should copy its frame.
    #[allow(dead_code)]
    pub(crate) fn window_snapshot_wanted(&self, index: usize) -> bool {
        self.window_snapshots.iter().any(|r| r.window_id.0 == index && r.state == WindowSnapshotState::Pending)
    }
}
