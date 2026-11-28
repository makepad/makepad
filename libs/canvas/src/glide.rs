//! A camera glide between two views of one canvas: the smooth zoom-and-pan
//! curve (van Wijk and Nuij, "Smooth and efficient zooming and panning",
//! 2003). Far-apart views at a similar scale zoom out on the way and back in,
//! so the eye keeps its place; views about one centre only zoom, and the scale
//! moves in octaves. The path is a pure function of its two ends.

use crate::{Camera, CameraState};
use makepad_widgets_core::*;

/// The curvature of the path: how far it zooms out to travel. The paper's
/// recommended value, near the one people found most natural.
const RHO: f64 = std::f64::consts::SQRT_2;

/// A view as the curve sees it: its centre in world units and its width in
/// world units (the larger side of the view over the scale).
#[derive(Clone, Copy, Debug)]
struct Frame {
    centre: DVec2,
    width: f64,
}

impl Frame {
    fn of(camera: &Camera) -> Self {
        Self {
            centre: camera.world_at(camera.view.pos + camera.view.size * 0.5),
            width: camera.view.size.x.max(camera.view.size.y) / camera.scale,
        }
    }
}

/// The path from one camera to another over the same canvas.
#[derive(Clone, Copy, Debug)]
pub struct GlidePath {
    from: Camera,
    to: Camera,
    a: Frame,
    /// The distance between the centres, in world units.
    distance: f64,
    /// The path's parameters: its start `r0` and its length `s` in the
    /// curve's own measure (screens travelled and octaves zoomed).
    r0: f64,
    s: f64,
    /// The centres coincide: the path only zooms, in (+1) or out (-1).
    zoom_only: Option<f64>,
}

impl GlidePath {
    /// The path from `from` to `to`. A degenerate camera (a zero or
    /// non-finite scale or view) gives a path that jumps to `to`.
    pub fn new(from: Camera, to: Camera) -> Self {
        let (a, b) = (Frame::of(&from), Frame::of(&to));
        let jump = Self { from, to, a, distance: 0.0, r0: 0.0, s: 0.0, zoom_only: None };
        let finite = |f: Frame| f.centre.x.is_finite() && f.centre.y.is_finite() && f.width.is_finite() && f.width > 0.0;
        if !finite(a) || !finite(b) {
            return jump;
        }
        let distance = (b.centre - a.centre).length();
        // Centres this close (relative to the views) only zoom: the general
        // form divides by the distance.
        if distance <= 1e-9 * a.width.min(b.width) {
            let octaves = (b.width / a.width).ln();
            return Self { distance, s: octaves.abs() / RHO, zoom_only: Some(octaves.signum()), ..jump };
        }
        let rho2 = RHO * RHO;
        let u2 = distance * distance;
        let b0 = (b.width * b.width - a.width * a.width + rho2 * rho2 * u2) / (2.0 * a.width * rho2 * distance);
        let b1 = (b.width * b.width - a.width * a.width - rho2 * rho2 * u2) / (2.0 * b.width * rho2 * distance);
        let r = |b: f64| (-b + (b * b + 1.0).sqrt()).ln();
        let (r0, r1) = (r(b0), r(b1));
        let s = (r1 - r0) / RHO;
        if !(s.is_finite() && s >= 0.0) {
            return jump;
        }
        Self { from, to, a, distance, r0, s, zoom_only: None }
    }

    /// The path's length in the curve's measure: about 1.25 for a pan of one
    /// screen at one scale and 0.49 for a zoom of one octave.
    pub fn length(&self) -> f64 {
        self.s
    }

    /// A glide's duration in seconds: a short base plus a share of its
    /// length, so a long trip takes longer but never drags.
    pub fn seconds(&self) -> f64 {
        if self.s == 0.0 {
            0.0
        } else {
            0.45 + 0.35 * self.s
        }
    }

    /// The camera at `u` of the glide's time (0 to 1, clamped), eased in and
    /// out. The ends are the two cameras exactly.
    pub fn at(&self, u: f64) -> Camera {
        if u >= 1.0 {
            return self.to;
        }
        if !(u > 0.0) || self.s == 0.0 {
            return self.from;
        }
        let eased = u * u * (3.0 - 2.0 * u);
        let s = eased * self.s;
        let (along, width) = match self.zoom_only {
            Some(direction) => (0.0, self.a.width * (direction * RHO * s).exp()),
            None => {
                let w0 = self.a.width;
                let rho2 = RHO * RHO;
                let along = w0 / rho2 * self.r0.cosh() * (RHO * s + self.r0).tanh() - w0 / rho2 * self.r0.sinh();
                (along, w0 * self.r0.cosh() / (RHO * s + self.r0).cosh())
            }
        };
        let b = Frame::of(&self.to);
        let centre = if self.distance > 0.0 {
            self.a.centre + (b.centre - self.a.centre) * (along / self.distance)
        } else {
            self.a.centre
        };
        // Draw through the target's view (the canvas it arrives in).
        let view = self.to.view;
        let scale = view.size.x.max(view.size.y) / width;
        let pan = view.size * 0.5 - centre * scale;
        Camera::new(view, CameraState { pan_x: pan.x, pan_y: pan.y, zoom: scale })
    }
}

/// The camera at `u` (0 to 1) of the glide from `a` to `b`.
pub fn glide(a: Camera, b: Camera, u: f64) -> Camera {
    GlidePath::new(a, b).at(u)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Rect {
        Rect { pos: dvec2(23.0, 47.0), size: dvec2(1280.0, 720.0) }
    }

    /// A camera showing `centre` at `scale`.
    fn at(centre: DVec2, scale: f64) -> Camera {
        let v = view();
        let pan = v.size * 0.5 - centre * scale;
        Camera::new(v, CameraState { pan_x: pan.x, pan_y: pan.y, zoom: scale })
    }

    fn centre(c: &Camera) -> DVec2 {
        c.world_at(c.view.pos + c.view.size * 0.5)
    }

    #[test]
    fn glide_keeps_its_ends_zooms_in_octaves_and_arrives_on_the_target() {
        let cases = [
            ("pan at one scale", at(dvec2(0.0, 0.0), 1.0), at(dvec2(12_000.0, -3_000.0), 1.0)),
            ("zoom in about one centre", at(dvec2(500.0, 500.0), 0.25), at(dvec2(500.0, 500.0), 64.0)),
            ("zoom out about one centre", at(dvec2(-7.5, 3.0), 32.0), at(dvec2(-7.5, 3.0), 1.0 / 512.0)),
            ("far away and deep", at(dvec2(1e9, -1e9), 0.5), at(dvec2(1e9 + 40.0, -1e9 + 9.0), 48.0)),
        ];
        for (name, a, b) in cases {
            let path = GlidePath::new(a, b);
            assert!(path.length() > 0.0 && path.seconds() > 0.0, "{name}: a real trip has a length and a duration");
            let start = path.at(0.0);
            let end = path.at(1.0);
            assert!(start.pan == a.pan && start.scale == a.scale, "{name}: u = 0 is the start camera exactly");
            assert!(end.pan == b.pan && end.scale == b.scale, "{name}: u = 1 is the target camera exactly");
            let mut last = (centre(&a), a.scale.ln());
            for k in 1..=64 {
                let c = path.at(k as f64 / 64.0);
                let here = (centre(&c), c.scale.ln());
                assert!(here.1.is_finite() && here.0.x.is_finite() && here.0.y.is_finite(), "{name}: finite at step {k}");
                // No jumps: per 1/64 of the glide, the scale moves under an
                // octave and the centre under a few views of the wider end.
                let wide = view().size.x / a.scale.min(b.scale).min(c.scale);
                assert!((here.1 - last.1).abs() < std::f64::consts::LN_2, "{name}: scale jumps at step {k}");
                assert!((here.0 - last.0).length() < wide, "{name}: centre jumps at step {k}");
                last = here;
            }
        }
    }

    #[test]
    fn glide_between_equal_centres_is_a_monotone_zoom_and_far_views_zoom_out_on_the_way() {
        let a = at(dvec2(40.0, 60.0), 0.5);
        let b = at(dvec2(40.0, 60.0), 40.0);
        let mut last = a.scale;
        for k in 1..=100 {
            let c = glide(a, b, k as f64 / 100.0);
            assert!(c.scale >= last, "equal-centre zoom must not turn back (step {k})");
            assert!((centre(&c) - dvec2(40.0, 60.0)).length() < 1e-6, "equal-centre zoom keeps its centre (step {k})");
            last = c.scale;
        }

        // Ten screens apart at one scale: the path rises above both ends.
        let a = at(dvec2(0.0, 0.0), 2.0);
        let b = at(dvec2(10.0 * 1280.0 / 2.0, 0.0), 2.0);
        let lowest = (1..100).map(|k| glide(a, b, k as f64 / 100.0).scale).fold(f64::INFINITY, f64::min);
        assert!(lowest < 2.0 * 0.5, "a far pan zooms out at least an octave on the way (lowest scale {lowest})");
        let mut last = 0.0;
        for k in 1..=100 {
            let x = centre(&glide(a, b, k as f64 / 100.0)).x;
            assert!(x >= last - 1e-6, "the centre moves toward the target without turning back (step {k})");
            last = x;
        }

        // Arriving on a fitted item: the item is inside the view at the end.
        let item = Rect { pos: dvec2(9_000.0, 4_000.0), size: dvec2(16.0, 9.0) };
        let fit = Camera::fit(view(), item, Inset { left: 24.0, right: 24.0, top: 24.0, bottom: 24.0 }).unwrap();
        let end = glide(a, fit, 1.0);
        let lo = end.local_to_screen(end.world_to_local(item.pos));
        let hi = end.local_to_screen(end.world_to_local(item.pos + item.size));
        let v = view();
        assert!(lo.x >= v.pos.x && lo.y >= v.pos.y && hi.x <= v.pos.x + v.size.x && hi.y <= v.pos.y + v.size.y, "the target is inside the view at u = 1");
    }
}
