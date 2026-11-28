use crate::{CameraState, Geometry};
use makepad_widgets_core::*;

/// A zoomable presentation's camera: window-local view rectangle, pan and
/// scale, plus the rebase that keeps draw-list coordinates precise.
///
/// Draw lists hold `f32` coordinates, so a world point is drawn in camera-local
/// units: `local = world - rebase + origin`, and the view transform maps local
/// units to the window. The rebase cell and the origin follow the zoom: the cell
/// is the power of two just above the view's extent in world units, the rebase
/// is the world point at the view's corner snapped down to that cell, and the
/// origin is two cells. Visible local coordinates and the transform's
/// translation then stay within a few window sizes at every zoom, which keeps
/// the `f32` path within a small fraction of a pixel of the `f64` one at any
/// pan.
#[derive(Clone, Copy, Debug, Default)]
pub struct Camera {
    pub view: Rect,
    pub pan: DVec2,
    pub scale: f64,
    rebase: DVec2,
    origin: f64,
}

/// The origin in rebase cells: a content edge up to this many cells above or
/// left of the view still has positive local coordinates.
const ORIGIN_CELLS: f64 = 2.0;

/// The power of two at or above a positive world extent; one unit when the
/// extent is not a finite positive number (an empty view or a degenerate scale).
fn rebase_cell(extent: f64) -> f64 {
    if !(extent.is_finite() && extent > 0.0) {
        return 1.0;
    }
    2f64.powi((extent.log2().ceil() as i32).clamp(-1000, 1000))
}

impl Camera {
    pub fn new(view: Rect, c: CameraState) -> Self {
        let world = dvec2(-c.pan_x / c.zoom, -c.pan_y / c.zoom);
        let cell = rebase_cell(view.size.x.max(view.size.y) / c.zoom);
        Self {
            view,
            pan: dvec2(c.pan_x, c.pan_y),
            scale: c.zoom,
            rebase: dvec2(
                (world.x / cell).floor() * cell,
                (world.y / cell).floor() * cell,
            ),
            origin: cell * ORIGIN_CELLS,
        }
    }
    /// The local coordinate of the rebase point on both axes; the content of a
    /// layer begins around here.
    pub fn origin(&self) -> f64 {
        self.origin
    }
    pub fn screen_to_local(&self, p: DVec2) -> DVec2 {
        self.world_to_local((p - self.view.pos - self.pan) / self.scale)
    }
    pub fn local_to_screen(&self, p: DVec2) -> DVec2 {
        self.view.pos + self.pan + (p - dvec2(self.origin, self.origin) + self.rebase) * self.scale
    }
    pub fn world_to_local(&self, p: DVec2) -> DVec2 {
        p - self.rebase + dvec2(self.origin, self.origin)
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
                + (self.rebase - dvec2(self.origin, self.origin)) * self.scale,
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

    /// The draw path is `f32`: a draw list holds local coordinates and the
    /// view transform's entries. Through it, a world point must land within
    /// 1/16 px of where the `f64` camera puts it, at every zoom from 2^-16 to
    /// the top and at pans out to 1e9 world units.
    #[test]
    fn f32_draw_path_stays_within_a_sixteenth_pixel_at_every_zoom_and_pan() {
        let view = Rect {
            pos: dvec2(23.0, 47.0),
            size: dvec2(1920.0, 1080.0),
        };
        for k in -16..=6 {
            let scale = 2f64.powi(k);
            assert!(scale <= crate::MAX_ZOOM, "2^{k} must be a reachable zoom");
            for centre in [0.0, -1234.5, 1e6 + 0.3, -1e9 - 0.7, 1e9 + 0.1] {
                let centre = dvec2(centre, -centre * 0.5);
                let pan = view.size * 0.5 - centre * scale;
                let camera = Camera::new(
                    view,
                    CameraState {
                        pan_x: pan.x,
                        pan_y: pan.y,
                        zoom: scale,
                    },
                );
                let m = camera.matrix();
                let extent = view.size / scale;
                for (fx, fy) in [(0.0, 0.0), (1.0, 1.0), (0.5, 0.5), (0.013, 0.97), (0.731, 0.211)] {
                    let world = centre + dvec2((fx - 0.5) * extent.x, (fy - 0.5) * extent.y);
                    let exact = camera.local_to_screen(camera.world_to_local(world));
                    let local = camera.world_to_local(world);
                    let (lx, ly) = (local.x as f32, local.y as f32);
                    let drawn = dvec2(
                        (m.v[0] * lx + m.v[12]) as f64,
                        (m.v[5] * ly + m.v[13]) as f64,
                    );
                    let error = (drawn - exact).length();
                    assert!(
                        error <= 1.0 / 16.0,
                        "zoom 2^{k}, centre {centre:?}: f32 path is {error} px off"
                    );
                }
            }
        }
    }

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
