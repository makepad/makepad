//! What every tile kind offers the window manager, whatever hosts the
//! window inside it.
//!
//! `MpRunView` presents a child process's swapchain, `MpDomainView` samples a HAL child's
//! lent surface, `MpWaylandView` presents a foreign window, and `MpModuleView` hosts a linked
//! module. Desk layout, focus and animation use this trait; transport-specific events stay
//! with the view that owns their protocol.

use makepad_app_host::ClientId;
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
    /// The window is fullscreen (a space of its own): a makepad app draws no caption then.
    fn set_fullscreen(&mut self, _on: bool) {}
    /// A drag is resizing windows (`WmState::live_resize`).
    fn set_live_resize(&mut self, _on: bool) {}
    /// While closing: the quad's place inside the original tile rect.
    fn set_close_crop(&mut self, crop: Option<(Vec2d, Vec2d)>);
    /// The popin fade the desk drives during open/close (1 = solid).
    fn set_fade(&mut self, fade: f32);
    /// Tell the client whether any of its window shows. False when it
    /// cannot be told (not a makepad child, or no live window yet).
    fn set_visible(&mut self, _cx: &mut Cx, _visible: bool) -> bool {
        false
    }
    /// Changes whenever the tile's content does, and None while it cannot
    /// say (always repaint): a parked window is only re-captured when this
    /// moved.
    fn content_serial(&self) -> Option<u64> {
        None
    }
    /// The texture of the frame the app last presented and its size in
    /// pixels (its window's own image, whatever covers it on the desk).
    fn presented_image(&self) -> Option<(Texture, [u32; 2])> {
        None
    }
    /// Files dropped on the tile at `abs` (desk coordinates): the app gets
    /// them as a drop into its window. False when it cannot take any.
    fn drop_files(&mut self, _cx: &mut Cx, _abs: Vec2d, _paths: Vec<String>) -> bool {
        false
    }
}
