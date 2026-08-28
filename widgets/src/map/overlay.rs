//! Map overlay layer: route polyline (casing + fill, traveled portion
//! dimmed), drop markers and the current-position puck. Everything here is
//! immediate-mode `DrawVector` geometry rebuilt per frame in screen space —
//! route scale is a few hundred visible points, well within budget.

use crate::makepad_draw::vector::{LineCap, LineJoin};
use crate::makepad_draw::*;
use crate::DrawVector;

/// The "space warp" mode (close-3D): a unified fold + perspective camera.
///
/// The base renderer is ORTHOGRAPHIC (screen y = rel_y*cos(tilt) −
/// lift_px*sin(tilt): an axonometric camera pitched 90°−tilt below the
/// horizon, at infinite distance). This struct is that same camera pulled in
/// to a finite dolly distance D = 1/kappa (scale 1 at the pivot, ortho as
/// kappa→0), looking at a ground surface that FOLDS: beyond `start_px` the
/// ground curls up along a circle of radius `radius_px` until its tangent is
/// PERPENDICULAR to the view axis (cap angle = tilt), then continues straight.
/// Past the cap, z along the view axis is constant, so the risen far field
/// renders as an undistorted, uniform-scale, face-on flat map — near field
/// stays true perspective street view; the fold is the hinge between them.
///
/// One math, two implementations — this struct is the CPU twin of the
/// `space_warp`/`space_warp2` uniform branch in DrawMapVector's vertex fn;
/// keep them in LOCKSTEP or CPU-projected overlays/labels detach from the
/// GPU-warped tiles.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SpaceWarp {
    /// Tween 0..1 (eased); 0 compiles to the exact identity path.
    pub amount: f64,
    /// Fold start r0: pre-tilt ground px up-screen from the pivot.
    pub start_px: f64,
    /// Curl radius R in pre-tilt ground px.
    pub radius_px: f64,
    /// cos(tilt) — doubles as cos of the bend cap.
    pub cos_t: f64,
    /// sin(tilt) — doubles as sin of the bend cap.
    pub sin_t: f64,
    /// Bend cap angle = tilt in radians: the wall stops curling exactly
    /// face-on to the camera (view axis pitch below horizon is 90°−tilt).
    pub cap: f64,
    /// Perspective strength 1/D in px⁻¹, ALREADY amount-scaled; 0 = ortho.
    pub kappa: f64,
}

impl SpaceWarp {
    pub fn is_on(&self) -> bool {
        self.amount > 1e-4
    }

    /// Fold the ground surface: ground distance g (pre-tilt px ahead of the
    /// pivot) with height lift_px above it → (forward, up, applied via the
    /// LOCAL surface normal so wall buildings point out of the wall), already
    /// blended toward flat by `amount`.
    fn fold(&self, g: f64, lift_px: f64) -> (f64, f64) {
        let a = g - self.start_px;
        let (f, u, nx, ny) = if a > 0.0 {
            let r = self.radius_px.max(1.0);
            let th = (a / r).min(self.cap);
            let (sth, cth) = (th.sin(), th.cos());
            let mut f = self.start_px + r * sth;
            let mut u = r * (1.0 - cth);
            let e = a - r * self.cap;
            if e > 0.0 {
                // straight, face-on continuation: 1 ground px = 1 wall px
                f += e * self.cos_t;
                u += e * self.sin_t;
            }
            (f, u, -sth, cth)
        } else {
            (g, 0.0, 0.0, 1.0)
        };
        let (pf, pu) = (f + lift_px * nx, u + lift_px * ny);
        (
            g + (pf - g) * self.amount,
            lift_px + (pu - lift_px) * self.amount,
        )
    }

    /// The ON-path camera: pre-tilt rotated rel offset from the pivot
    /// (x lateral, y: up-screen = negative = forward) plus vertical lift in
    /// ground px → screen offset from the pivot. Callers keep the legacy
    /// ortho expression when `!is_on()` (byte-identical flat mode).
    pub fn project(&self, rel_x: f64, rel_y: f64, lift_px: f64) -> Vec2d {
        let (bf, bu) = self.fold(-rel_y, lift_px);
        // z along the view axis rel to the pivot plane; w = D/z, scale 1 at
        // the pivot. Floor keeps geometry behind the eye finite (off-screen
        // anyway — must not blow up or flip).
        let zrel = bf * self.sin_t - bu * self.cos_t;
        let w = 1.0 / (1.0 + self.kappa * zrel).max(0.12);
        dvec2(rel_x * w, -(bf * self.cos_t + bu * self.sin_t) * w)
    }

    /// Re-project a point the label funnel already carried to ortho GROUND
    /// screen space (rot + tilt applied, lift NOT yet applied): recover the
    /// pre-tilt rel, then run the full camera with the point's lift.
    pub fn warp_screen_point(&self, p: Vec2d, pivot: Vec2d, lift_px: f64) -> Vec2d {
        if !self.is_on() {
            return dvec2(p.x, p.y - lift_px * self.sin_t);
        }
        let rel_x = p.x - pivot.x;
        let rel_y = (p.y - pivot.y) / self.cos_t.max(1e-6);
        pivot + self.project(rel_x, rel_y, lift_px)
    }

    /// Perspective factor w (screen scale) at an ortho GROUND screen point;
    /// 1 when off. Label lifts/badges scale by this so far-wall pins don't
    /// tower over their perspective-shrunken buildings.
    pub fn screen_w(&self, p: Vec2d, pivot: Vec2d) -> f64 {
        if !self.is_on() {
            return 1.0;
        }
        let rel_y = (p.y - pivot.y) / self.cos_t.max(1e-6);
        let (bf, bu) = self.fold(-rel_y, 0.0);
        let zrel = bf * self.sin_t - bu * self.cos_t;
        1.0 / (1.0 + self.kappa * zrel).max(0.12)
    }

    /// Tile-culling extents under the warp. `half_h_flat` is the flat-mode
    /// pre-tilt ground reach (screen_half/tilt_cos); returns (ground reach,
    /// lateral widen factor ≥1). The wall advances up-screen slower than the
    /// flat ortho compression at low tilt, so the fold can SEE FURTHER than
    /// the flat frustum — cull honestly or the wall runs out of city
    /// (perf-never-breaks-the-picture).
    pub fn cull_extents(&self, screen_half: f64, half_h_flat: f64) -> (f64, f64) {
        if !self.is_on() {
            return (half_h_flat, 1.0);
        }
        // End of the bend (lift 0, amount folded in):
        let r = self.radius_px.max(1.0);
        let g_cap = self.start_px + r * self.cap;
        let (f_cap, u_cap) = self.fold(g_cap, 0.0);
        let z_cap = f_cap * self.sin_t - u_cap * self.cos_t;
        let w_wall = 1.0 / (1.0 + self.kappa * z_cap).max(0.12);
        let y_cap = f_cap * self.cos_t + u_cap * self.sin_t;
        // On the wall screen-y advances 1:1·amount·w_wall per ground px
        // (blended toward the flat cos_t rate when amount < 1).
        let rate = (self.amount + (1.0 - self.amount) * self.cos_t) * w_wall;
        let need = screen_half - y_cap * w_wall;
        let reach = g_cap + (need / rate.max(1e-3)).max(0.0);
        (reach.max(half_h_flat), (1.0 / w_wall).max(1.0))
    }
}

/// Screen-space camera for one overlay frame; built by `MapView::draw_walk`
/// from the same numbers the tile pass uses.
pub struct OverlayCamera {
    /// Pixels per normalized-mercator unit at the current view zoom.
    pub world_size: f64,
    /// Screen offset: `screen = norm * world_size + offset` (before rotation).
    pub offset: Vec2d,
    pub rect: Rect,
    /// Ground meters per screen pixel at the view center latitude.
    pub meters_per_px: f64,
    /// (cos, sin) of the heading-up screen rotation; identity = north-up.
    pub rot: (f64, f64),
    pub rot_pivot: Vec2d,
    /// Map bearing pointing up, degrees (for billboard heading math).
    pub rotation_deg: f64,
    /// cos(tilt) of the 2.5D camera; 1.0 = top-down.
    pub tilt_cos: f64,
    /// The Inception fold, identity when off — every CPU ground projection
    /// funnels through it so overlays/markers/terrain track the GPU tiles.
    pub warp: SpaceWarp,
}

impl OverlayCamera {
    pub fn norm_to_screen(&self, p: Vec2d) -> Vec2d {
        self.norm_to_screen_with_rel(p).0
    }

    /// Screen position AND the pre-tilt, UN-warped ground rel-y — depth
    /// must stay a function of the original ground plane (the tile shader
    /// computes depth from unwarped `ground_rel_y`), so callers that build
    /// depth ladders take the second value instead of un-compressing the
    /// warped screen y.
    pub fn norm_to_screen_with_rel(&self, p: Vec2d) -> (Vec2d, f64) {
        let s = p * self.world_size + self.offset;
        if self.rot == (1.0, 0.0) && self.tilt_cos == 1.0 && !self.warp.is_on() {
            return (s, s.y - self.rot_pivot.y);
        }
        let rel = s - self.rot_pivot;
        let rotated = dvec2(
            rel.x * self.rot.0 - rel.y * self.rot.1,
            rel.x * self.rot.1 + rel.y * self.rot.0,
        );
        let screen = if self.warp.is_on() {
            self.rot_pivot + self.warp.project(rotated.x, rotated.y, 0.0)
        } else {
            self.rot_pivot + dvec2(rotated.x, rotated.y * self.tilt_cos)
        };
        (screen, rotated.y)
    }

    /// Ground point with a vertical lift (in GROUND px) through the warp
    /// camera — terrain/overlay callers use this when the warp is on so the
    /// lift rides the fold normal and the perspective divide; when it is
    /// off they keep their legacy straight-up `lift_m * ppm * sin(tilt)`
    /// screen offset (byte-identical flat path).
    pub fn norm_to_screen_lifted(&self, p: Vec2d, lift_px: f64) -> (Vec2d, f64) {
        let s = p * self.world_size + self.offset;
        let rel = s - self.rot_pivot;
        let rotated = dvec2(
            rel.x * self.rot.0 - rel.y * self.rot.1,
            rel.x * self.rot.1 + rel.y * self.rot.0,
        );
        (
            self.rot_pivot + self.warp.project(rotated.x, rotated.y, lift_px),
            rotated.y,
        )
    }
}

#[derive(Clone, Debug)]
pub struct MapMarker {
    pub id: u64,
    pub lon: f64,
    pub lat: f64,
    /// Normalized mercator, cached at set time.
    pub pos_norm: Vec2d,
    pub color: Vec4f,
}

impl MapMarker {
    pub fn new(id: u64, lon: f64, lat: f64, color: Vec4f) -> Self {
        let pos_norm = super::geometry::lon_lat_to_normalized(lon, lat);
        Self {
            id,
            lon,
            lat,
            pos_norm,
            color,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MapRouteOverlay {
    /// Normalized mercator polyline.
    pub points_norm: Vec<Vec2d>,
    /// Points before this index are drawn dimmed (already traveled).
    pub traveled_index: usize,
}

#[derive(Clone, Debug)]
pub struct MapPuck {
    pub lon: f64,
    pub lat: f64,
    pub pos_norm: Vec2d,
    /// Compass heading in degrees (0 = north, clockwise); draws the wedge.
    pub heading_deg: Option<f64>,
    pub accuracy_m: f64,
}

impl MapPuck {
    pub fn new(lon: f64, lat: f64, heading_deg: Option<f64>, accuracy_m: f64) -> Self {
        let pos_norm = super::geometry::lon_lat_to_normalized(lon, lat);
        Self {
            lon,
            lat,
            pos_norm,
            heading_deg,
            accuracy_m,
        }
    }
}

#[derive(Default)]
pub struct MapOverlayState {
    pub markers: Vec<MapMarker>,
    pub route: Option<MapRouteOverlay>,
    pub puck: Option<MapPuck>,
    /// shiny.md T5b: additive halo passes under the route stroke (no HDR,
    /// no bloom — premultiplied rgb with alpha 0 is pure additive over
    /// whatever is underneath). Stamped from the theme per frame.
    pub route_glow: bool,
    /// Scratch screen-space buffer reused across frames.
    scratch_screen: Vec<Vec2d>,
}

impl MapOverlayState {
    pub fn is_empty(&self) -> bool {
        self.markers.is_empty() && self.route.is_none() && self.puck.is_none()
    }

    /// Topmost marker whose pin head is within tap distance of `abs`.
    pub fn marker_at(&self, camera: &OverlayCamera, abs: Vec2d) -> Option<u64> {
        for marker in self.markers.iter().rev() {
            let p = camera.norm_to_screen(marker.pos_norm);
            let head = dvec2(p.x, p.y - PIN_HEAD_LIFT);
            if (abs - head).length() < 16.0 {
                return Some(marker.id);
            }
        }
        None
    }
}

const ROUTE_CASING: Vec4f = Vec4f {
    x: 0.06,
    y: 0.27,
    z: 0.55,
    w: 1.0,
};
const ROUTE_FILL: Vec4f = Vec4f {
    x: 0.20,
    y: 0.51,
    z: 0.95,
    w: 1.0,
};
/// Traveled portion: same hue, mostly transparent.
const ROUTE_DIM_ALPHA: f32 = 0.30;
const PIN_HEAD_LIFT: f64 = 15.0;

/// Draw the whole overlay. Order: route under markers under puck.
pub fn draw_map_overlay(
    cx: &mut Cx2d,
    dv: &mut DrawVector,
    camera: &OverlayCamera,
    state: &mut MapOverlayState,
) {
    if state.is_empty() {
        return;
    }
    let rect = camera.rect;
    // DrawVector geometry maps through the current turtle; pin one to our
    // rect (same pattern as PerfGraph) so paths land where we compute them.
    cx.begin_turtle(
        Walk {
            abs_pos: Some(rect.pos),
            width: Size::Fixed(rect.size.x),
            height: Size::Fixed(rect.size.y),
            margin: Inset::default(),
            metrics: Metrics::default(),
        },
        Layout {
            clip_x: true,
            clip_y: true,
            ..Layout::default()
        },
    );
    dv.begin();

    let route = state.route.take();
    if let Some(route) = &route {
        draw_route(dv, camera, route, state.route_glow, &mut state.scratch_screen);
    }
    state.route = route;

    for marker in &state.markers {
        draw_marker(dv, camera, marker);
    }
    if let Some(puck) = &state.puck {
        draw_puck(dv, camera, puck);
    }

    dv.end(cx);
    cx.end_turtle();
}

fn draw_route(
    dv: &mut DrawVector,
    camera: &OverlayCamera,
    route: &MapRouteOverlay,
    glow: bool,
    screen: &mut Vec<Vec2d>,
) {
    if route.points_norm.len() < 2 {
        return;
    }
    screen.clear();
    for p in &route.points_norm {
        screen.push(camera.norm_to_screen(*p));
    }
    let margin = 24.0;
    let min_x = camera.rect.pos.x - margin;
    let min_y = camera.rect.pos.y - margin;
    let max_x = camera.rect.pos.x + camera.rect.size.x + margin;
    let max_y = camera.rect.pos.y + camera.rect.size.y + margin;
    let seg_visible = |a: Vec2d, b: Vec2d| -> bool {
        !(a.x < min_x && b.x < min_x
            || a.x > max_x && b.x > max_x
            || a.y < min_y && b.y < min_y
            || a.y > max_y && b.y > max_y)
    };

    // Halo passes (widest first) then casing then fill, each split at the
    // traveled boundary so the behind-us part fades out. Halo colors are
    // premultiplied-additive (rgb energy, alpha 0): roads underneath
    // BRIGHTEN instead of being covered — the no-HDR glow trick.
    let mut passes: Vec<(f32, Vec4f)> = Vec::with_capacity(4);
    if glow {
        passes.push((
            26.0,
            Vec4f { x: ROUTE_FILL.x * 0.07, y: ROUTE_FILL.y * 0.07, z: ROUTE_FILL.z * 0.07, w: 0.0 },
        ));
        passes.push((
            14.0,
            Vec4f { x: ROUTE_FILL.x * 0.16, y: ROUTE_FILL.y * 0.16, z: ROUTE_FILL.z * 0.16, w: 0.0 },
        ));
    }
    passes.push((9.0, ROUTE_CASING));
    passes.push((5.5, ROUTE_FILL));
    let split = route.traveled_index.min(screen.len());
    for (width, color) in passes {
        for (range, alpha) in [
            (0..split.saturating_add(1).min(screen.len()), ROUTE_DIM_ALPHA),
            (split..screen.len(), 1.0),
        ] {
            if range.len() < 2 {
                continue;
            }
            // Additive halos (w = 0) carry their energy in rgb, so the
            // traveled-portion dim must scale rgb, not the no-op alpha.
            if color.w == 0.0 {
                dv.set_color(color.x * alpha, color.y * alpha, color.z * alpha, 0.0);
            } else {
                dv.set_color(color.x, color.y, color.z, color.w * alpha);
            }
            let mut pen_down = false;
            let mut last_drawn = dvec2(0.0, 0.0);
            let start = range.start;
            let end = range.end;
            for i in start..end - 1 {
                let a = screen[i];
                let b = screen[i + 1];
                // Decimate against the last DRAWN point so error stays
                // bounded (~1.5px) — neighbor-pairwise skipping compounded
                // and visibly reshaped the route when zoomed out.
                if pen_down && (b - last_drawn).length() < 1.5 && i + 2 < end {
                    continue;
                }
                if !seg_visible(last_drawn, b) && !seg_visible(a, b) {
                    if pen_down {
                        dv.stroke_opts(width, LineCap::Round, LineJoin::Round, 4.0, 1.0);
                        dv.clear();
                        pen_down = false;
                    }
                    continue;
                }
                if !pen_down {
                    dv.move_to(a.x as f32, a.y as f32);
                    pen_down = true;
                }
                dv.line_to(b.x as f32, b.y as f32);
                last_drawn = b;
            }
            if pen_down {
                dv.stroke_opts(width, LineCap::Round, LineJoin::Round, 4.0, 1.0);
                dv.clear();
            }
        }
    }

    // Destination dot at the very end of the line.
    if let Some(&end) = screen.last() {
        if end.x > min_x && end.x < max_x && end.y > min_y && end.y < max_y {
            dv.set_color(ROUTE_CASING.x, ROUTE_CASING.y, ROUTE_CASING.z, 1.0);
            dv.circle(end.x as f32, end.y as f32, 6.0);
            dv.fill();
            dv.set_color(1.0, 1.0, 1.0, 1.0);
            dv.circle(end.x as f32, end.y as f32, 2.6);
            dv.fill();
        }
    }
}

fn draw_marker(dv: &mut DrawVector, camera: &OverlayCamera, marker: &MapMarker) {
    let p = camera.norm_to_screen(marker.pos_norm);
    let margin = 30.0;
    if p.x < camera.rect.pos.x - margin
        || p.y < camera.rect.pos.y - margin
        || p.x > camera.rect.pos.x + camera.rect.size.x + margin
        || p.y > camera.rect.pos.y + camera.rect.size.y + margin
    {
        return;
    }
    let (x, y) = (p.x as f32, p.y as f32);
    let head_y = y - PIN_HEAD_LIFT as f32;
    let c = marker.color;
    // Soft ground shadow.
    dv.set_color(0.0, 0.0, 0.0, 0.18);
    dv.ellipse(x, y + 1.5, 5.0, 2.2);
    dv.fill();
    // Tail triangle + head disc read as one pin shape.
    dv.set_color(c.x, c.y, c.z, c.w);
    dv.move_to(x, y);
    dv.line_to(x - 7.2, head_y + 3.0);
    dv.line_to(x + 7.2, head_y + 3.0);
    dv.close();
    dv.fill();
    dv.circle(x, head_y, 8.6);
    dv.fill();
    // White pip.
    dv.set_color(1.0, 1.0, 1.0, 0.95);
    dv.circle(x, head_y, 3.4);
    dv.fill();
}

fn draw_puck(dv: &mut DrawVector, camera: &OverlayCamera, puck: &MapPuck) {
    let p = camera.norm_to_screen(puck.pos_norm);
    let margin = 60.0;
    if p.x < camera.rect.pos.x - margin
        || p.y < camera.rect.pos.y - margin
        || p.x > camera.rect.pos.x + camera.rect.size.x + margin
        || p.y > camera.rect.pos.y + camera.rect.size.y + margin
    {
        return;
    }
    let (x, y) = (p.x as f32, p.y as f32);

    // Accuracy circle in map space (scales with zoom).
    if puck.accuracy_m > 0.0 && camera.meters_per_px > 0.0 {
        let r = (puck.accuracy_m / camera.meters_per_px) as f32;
        let max_r = (camera.rect.size.x + camera.rect.size.y) as f32;
        if r > 12.0 && r < max_r {
            dv.set_color(0.20, 0.51, 0.95, 0.10);
            dv.circle(x, y, r);
            dv.fill();
            dv.set_color(0.20, 0.51, 0.95, 0.28);
            dv.circle(x, y, r);
            dv.stroke(1.0);
        }
    }

    // Heading wedge behind the dot; under a heading-up camera the wedge
    // shows the heading relative to the rotated map.
    if let Some(heading) = puck.heading_deg {
        let rad = (heading - camera.rotation_deg).to_radians();
        let (dir_x, dir_y) = (rad.sin() as f32, -(rad.cos()) as f32);
        let (side_x, side_y) = (-dir_y, dir_x);
        let tip = 20.0f32;
        let half = 8.5f32;
        dv.set_color(0.20, 0.51, 0.95, 0.55);
        dv.move_to(x + dir_x * tip, y + dir_y * tip);
        dv.line_to(x + side_x * half, y + side_y * half);
        dv.line_to(x - side_x * half, y - side_y * half);
        dv.close();
        dv.fill();
    }

    // White ring + blue dot.
    dv.set_color(1.0, 1.0, 1.0, 1.0);
    dv.circle(x, y, 9.0);
    dv.fill();
    dv.set_color(0.13, 0.45, 0.92, 1.0);
    dv.circle(x, y, 6.2);
    dv.fill();
}
