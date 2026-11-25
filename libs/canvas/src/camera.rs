use crate::{CameraState, Geometry};
use makepad_widgets_core::*;

/// World-space origin offset that keeps world coordinates positive inside a
/// zoomable draw list while the camera pans across a very large space.
pub const ORIGIN: f64 = 32768.0;

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
    pub fn new(view: Rect, c: CameraState) -> Self {
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

    /// Use the given positive scale while keeping the same world point under the window anchor.
    pub fn zoom_about(&self, anchor: DVec2, scale: f64) -> Option<Self> {
        Self::anchored(
            self.view,
            anchor - self.view.pos,
            self.world_at(anchor),
            scale,
        )
    }

    /// Centre a nonempty world rect at the largest scale that fits inside the insets.
    /// Returns `None` when there is no finite, positive fit.
    pub fn fit(view: Rect, world: Rect, insets: Inset) -> Option<Self> {
        let available = view.size - insets.size();
        if ![world.size.x, world.size.y, available.x, available.y]
            .iter()
            .all(|&size| size.is_finite() && size > 0.0)
        {
            return None;
        }
        let scale = (available.x / world.size.x).min(available.y / world.size.y);
        Self::anchored(
            view,
            insets.left_top() + available * 0.5,
            world.pos + world.size * 0.5,
            scale,
        )
    }

    fn anchored(view: Rect, in_view: DVec2, world: DVec2, scale: f64) -> Option<Self> {
        let pan = in_view - world * scale;
        if !(scale.is_finite() && scale > 0.0 && pan.x.is_finite() && pan.y.is_finite()) {
            return None;
        }
        let camera = Self::new(
            view,
            CameraState {
                pan_x: pan.x,
                pan_y: pan.y,
                zoom: scale,
            },
        );
        (camera.rebase.x.is_finite() && camera.rebase.y.is_finite()).then_some(camera)
    }

    pub fn screen_rect(&self, g: Geometry) -> Rect {
        Rect {
            pos: self.view.pos + self.pan + dvec2(g.x, g.y) * self.scale,
            size: dvec2(g.w, g.h) * self.scale,
        }
    }
    pub fn local_rect(&self, g: Geometry) -> Rect {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebased_camera_round_trips_zooms_and_fits() {
        let view = Rect {
            pos: dvec2(23.0, 47.0),
            size: dvec2(640.0, 480.0),
        };
        let camera = Camera::new(
            view,
            CameraState {
                pan_x: -96_000.0,
                pan_y: 42_000.0,
                zoom: 1.5,
            },
        );
        let anchor = dvec2(517.0, 102.0);
        let world = camera.world_at(anchor);
        let local = camera.world_to_local(world);
        assert_eq!(
            local,
            camera.screen_to_local(anchor),
            "world and screen must identify the same local point"
        );
        assert!(
            (camera.local_to_screen(local) - anchor).length() < 1e-8,
            "local point must return to its screen position"
        );
        let zoomed = camera.zoom_about(anchor, 0.375).unwrap();
        assert_eq!(zoomed.scale, 0.375);
        assert!(
            (zoomed.world_at(anchor) - world).length() < 1e-8,
            "zoom must preserve the world point under the anchor"
        );

        let insets = Inset {
            left: 80.0,
            right: 16.0,
            top: 24.0,
            bottom: 56.0,
        };
        let inner = view.size - insets.size();
        for size in [dvec2(320.0, 80.0), dvec2(80.0, 320.0)] {
            let rect = Rect { pos: world, size };
            let fit = Camera::fit(view, rect, insets).unwrap();
            let start = fit.local_to_screen(fit.world_to_local(rect.pos));
            let end = fit.local_to_screen(fit.world_to_local(rect.pos + rect.size));
            let centre = view.pos + insets.left_top() + inner * 0.5;
            assert!(
                ((start + end) * 0.5 - centre).length() < 1e-8,
                "fit must centre the rect between the insets"
            );
            let slack = inner - (end - start);
            assert!(
                slack.x >= -1e-8 && slack.y >= -1e-8,
                "fit must keep both axes inside the insets"
            );
            assert!(
                slack.x.min(slack.y).abs() < 1e-8,
                "fit must fill the limiting axis"
            );
        }
    }
}
