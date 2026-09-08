//! The presentation seam shared by the Structured and Architecture
//! presentations of one live workspace and by the zoomable tasks view. A
//! presenter changes geometry and input coordinates; it never owns
//! documents, processes or Dock items.
use crate::workspace;
use makepad_widgets::*;

/// World-space origin offset that keeps world coordinates positive inside a
/// zoomable draw list while the camera pans across a very large space.
pub(crate) const ORIGIN: f64 = 32768.0;

/// A zoomable presentation's camera: window-local view rectangle, pan, scale
/// and the 8192-unit rebase that keeps draw-list coordinates precise at
/// extreme pans.
#[derive(Clone, Copy, Debug, Default)]
pub struct Camera {
    pub view: Rect,
    pub pan: DVec2,
    pub scale: f64,
    rebase: DVec2,
}

impl Camera {
    pub(crate) fn new(view: Rect, c: workspace::Camera) -> Self {
        let world = dvec2(-c.pan_x / c.zoom, -c.pan_y / c.zoom);
        Self {
            view,
            pan: dvec2(c.pan_x, c.pan_y),
            scale: c.zoom,
            rebase: dvec2(
                (world.x / 8192.0).floor() * 8192.0,
                (world.y / 8192.0).floor() * 8192.0,
            ),
        }
    }
    pub fn screen_to_local(&self, p: DVec2) -> DVec2 {
        self.world_to_local((p - self.view.pos - self.pan) / self.scale)
    }
    pub fn local_to_screen(&self, p: DVec2) -> DVec2 {
        self.view.pos + self.pan + (p - dvec2(ORIGIN, ORIGIN) + self.rebase) * self.scale
    }
    pub fn world_to_local(&self, p: DVec2) -> DVec2 {
        p - self.rebase + dvec2(ORIGIN, ORIGIN)
    }
    pub fn world_at(&self, p: DVec2) -> DVec2 {
        (p - self.view.pos - self.pan) / self.scale
    }
    pub fn screen_rect(&self, g: workspace::Geometry) -> Rect {
        Rect {
            pos: self.view.pos + self.pan + dvec2(g.x, g.y) * self.scale,
            size: dvec2(g.w, g.h) * self.scale,
        }
    }
    pub fn local_rect(&self, g: workspace::Geometry) -> Rect {
        Rect {
            pos: self.world_to_local(dvec2(g.x, g.y)),
            size: dvec2(g.w, g.h),
        }
    }
    /// The popup/IME anchor transform a body drawn through this camera
    /// reports to its host.
    pub fn transform(&self) -> PopupAnchorTransform {
        PopupAnchorTransform {
            scale: self.scale,
            translation: self.view.pos
                + self.pan
                + (self.rebase - dvec2(ORIGIN, ORIGIN)) * self.scale,
        }
    }
    /// The draw-list view transform of this camera.
    pub fn matrix(&self) -> Mat4f {
        let t = self.transform();
        let mut m = Mat4f::identity();
        m.v[0] = t.scale as f32;
        m.v[5] = t.scale as f32;
        m.v[12] = t.translation.x as f32;
        m.v[13] = t.translation.y as f32;
        m
    }
}

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
