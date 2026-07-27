//! Map overlay layer: route polyline (casing + fill, traveled portion
//! dimmed), drop markers and the current-position puck. Everything here is
//! immediate-mode `DrawVector` geometry rebuilt per frame in screen space —
//! route scale is a few hundred visible points, well within budget.

use crate::makepad_draw::vector::{LineCap, LineJoin};
use crate::makepad_draw::*;
use crate::DrawVector;

/// Screen-space camera for one overlay frame; built by `MapView::draw_walk`
/// from the same numbers the tile pass uses.
pub struct OverlayCamera {
    /// Pixels per normalized-mercator unit at the current view zoom.
    pub world_size: f64,
    /// Screen offset: `screen = norm * world_size + offset`.
    pub offset: Vec2d,
    pub rect: Rect,
    /// Ground meters per screen pixel at the view center latitude.
    pub meters_per_px: f64,
}

impl OverlayCamera {
    pub fn norm_to_screen(&self, p: Vec2d) -> Vec2d {
        p * self.world_size + self.offset
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
        draw_route(dv, camera, route, &mut state.scratch_screen);
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

    // Two stroke passes (casing then fill), each split at the traveled
    // boundary so the behind-us part fades out.
    let split = route.traveled_index.min(screen.len());
    for (width, color) in [(9.0f32, ROUTE_CASING), (5.5f32, ROUTE_FILL)] {
        for (range, alpha) in [
            (0..split.saturating_add(1).min(screen.len()), ROUTE_DIM_ALPHA),
            (split..screen.len(), 1.0),
        ] {
            if range.len() < 2 {
                continue;
            }
            dv.set_color(color.x, color.y, color.z, color.w * alpha);
            let mut pen_down = false;
            let start = range.start;
            let end = range.end;
            for i in start..end - 1 {
                let a = screen[i];
                let b = screen[i + 1];
                // Thin sub-pixel steps while keeping corners.
                if pen_down && (b - a).length() < 1.5 && i + 2 < end {
                    continue;
                }
                if !seg_visible(a, b) {
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

    // Heading wedge behind the dot.
    if let Some(heading) = puck.heading_deg {
        let rad = heading.to_radians();
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
