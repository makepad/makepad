//! The presentation seam shared by the Structured and Architecture
//! presentations of one live workspace and by the zoomable tasks view. A
//! presenter changes geometry and input coordinates; it never owns
//! documents, processes or Dock items.
pub use makepad_canvas::{Camera, ORIGIN};
use makepad_widgets_core::*;

/// What a presentation shows of its whole space: the map bounds and, when
/// the presentation has a camera, the visible window inside them. Both in
/// the presentation's own world coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Navigator {
    pub bounds: Rect,
    pub viewport: Option<Rect>,
}

/// One presentation of the shared workspace. Presenters draw and take input
/// only while visible; async delivery to resident bodies is the surface's
/// pump, not the presenter's job.
pub trait Presenter {
    fn camera(&self) -> Option<Camera>;
    fn navigator(&self) -> Option<Navigator>;
    fn draw(&mut self, cx: &mut Cx2d, scope: &mut Scope, rect: Rect);
    fn input(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope);
    /// Clear captures, IME anchors and pending ticks; keep every body alive.
    fn deactivate(&mut self, cx: &mut Cx);
}
