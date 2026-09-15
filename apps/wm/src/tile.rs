//! What every tile kind offers the window manager, whatever hosts the
//! window inside it.
//!
//! A tile is one of two widgets: `MpRunView` presents a child PROCESS's
//! shared swapchain and forwards its input over the studio protocol;
//! `MpModuleView` draws an in-process MODULE instance's root as a subtree
//! of the desk. The desk, the focus logic and the status plumbing in
//! `main.rs` never care which: they go through this trait, and the
//! process-only calls (`set_run_target`, `app_ready`, presented frames)
//! stay on `MpRunView` where only the hub's messages reach them.

use crate::hub::ClientId;
use makepad_widgets::*;

pub trait TileHost {
    /// The client this tile shows, once it has one.
    fn client(&self) -> Option<ClientId>;
    /// The line under "starting…" while the window has nothing to show.
    fn set_status_line(&mut self, cx: &mut Cx, line: &str);
    /// Claim the compositor's key focus for this window. False while the
    /// tile has no live area yet; the WM keeps such a focus pending.
    fn focus_keyboard(&mut self, cx: &mut Cx) -> bool;
    /// Give the keyboard back if this tile holds it.
    fn release_keyboard(&mut self, cx: &mut Cx);
    /// FOCUS RULE: a Quick-Look panel never takes the keyboard.
    fn set_takes_key_focus(&mut self, on: bool);
    /// The cursor the window asked for (a process child sends it; an
    /// in-process module sets the cursor itself, so this is a no-op there).
    fn set_remote_cursor(&mut self, cx: &mut Cx, cursor: MouseCursor);
    /// The window has drawn at least once: the desk stops painting its
    /// starting wash.
    fn has_frame(&self) -> bool;
    /// 0→1 over the first frames after `has_frame`, for the arrival crossfade.
    fn arrival_fade(&self) -> f32;
    /// The layout's settled size while the rect tweens (resize-sync).
    fn set_target_size(&mut self, size: Option<Vec2d>);
    /// While closing: the quad's place inside the original tile rect.
    fn set_close_crop(&mut self, crop: Option<(Vec2d, Vec2d)>);
    /// The popin fade the desk drives during open/close (1 = solid).
    fn set_fade(&mut self, fade: f32);
}
