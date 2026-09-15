use makepad_fabric_measure::{BodyMesh, Line, Measured, Ring};
use makepad_widgets::*;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawBodyPoint::script_shader(vm)) {
        ..mod.draw.DrawQuad
        pixel: fn() {
            let d = length(self.pos - vec2(0.5, 0.5))
            let a = clamp((0.5 - d) * 7.0, 0.0, 1.0)
            let near = #x80e7ff
            let far = #x27435d
            let c = far.mix(near, 1.0 - self.depth)
            return vec4(c.xyz * a, a)
        }
    }

    set_type_default() do #(DrawFabricLine::script_shader(vm)) {
        ..mod.draw.DrawQuad
        pixel: fn() {
            // A line as a distance field inside its bounding quad. The
            // endpoints are LOCAL to the quad (the turtle may still shift
            // rect_pos after the instance is written), like the chart's
            // segment shader.
            let p = self.pos * self.rect_size
            let ab = self.p1 - self.p0
            let t = clamp(dot(p - self.p0, ab) / max(dot(ab, ab), 0.0001), 0.0, 1.0)
            let d = length(p - (self.p0 + ab * t))
            let aa = 1.0 - smoothstep(self.half_width - 0.6, self.half_width + 0.6, d)
            let alpha = aa * self.color.w
            return vec4(self.color.xyz * alpha, alpha)
        }
    }

    mod.widgets.FabricBodyViewBase = #(FabricBodyView::register_widget(vm))
    mod.widgets.FabricBodyView = set_type_default() do mod.widgets.FabricBodyViewBase {
        width: Fill
        height: Fill
        draw_bg +: {color: #x11161d}
        draw_point +: {}
        draw_line +: {}
        draw_text +: {
            color: #x9aa8b7
            text_style: theme.font_regular{font_size: 9.0}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawBodyPoint {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    depth: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFabricLine {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    p0: Vec2f,
    #[live]
    p1: Vec2f,
    #[live]
    half_width: f32,
}

impl DrawFabricLine {
    pub fn segment(&mut self, cx: &mut Cx2d, from: DVec2, to: DVec2, width: f64) {
        if (to - from).length() < 0.01 {
            return;
        }
        let half = width * 0.5;
        let pad = half + 1.0;
        let min = dvec2(from.x.min(to.x) - pad, from.y.min(to.y) - pad);
        let max = dvec2(from.x.max(to.x) + pad, from.y.max(to.y) + pad);
        self.p0 = v2f(from - min);
        self.p1 = v2f(to - min);
        self.half_width = half as f32;
        self.draw_abs(cx, Rect { pos: min, size: max - min });
    }
}

fn v2f(value: DVec2) -> Vec2f {
    Vec2f {
        x: value.x as f32,
        y: value.y as f32,
    }
}

#[derive(Clone, Copy)]
struct BodyDrag {
    from: DVec2,
    yaw: f64,
    pitch: f64,
    pan: DVec2,
    panning: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BodyPoseMapping {
    ring_vertices: Vec<Vec<usize>>,
    line_vertices: Vec<[usize; 2]>,
}

pub(crate) fn map_measurements_to_vertices(
    mesh: &BodyMesh,
    measured: &Measured,
) -> BodyPoseMapping {
    let nearest = |point| nearest_vertex_index(&mesh.vertices, measured.scale, point);
    BodyPoseMapping {
        ring_vertices: measured
            .rings
            .iter()
            .map(|ring| ring.points.iter().copied().map(nearest).collect())
            .collect(),
        line_vertices: measured
            .lines
            .iter()
            .map(|line| [nearest(line.from), nearest(line.to)])
            .collect(),
    }
}

fn nearest_vertex_index(vertices: &[[f32; 3]], scale: f32, point: [f32; 3]) -> usize {
    vertices
        .iter()
        .enumerate()
        .map(|(index, vertex)| {
            let dx = vertex[0] * scale - point[0];
            let dy = vertex[1] * scale - point[1];
            let dz = vertex[2] * scale - point[2];
            (index, dx * dx + dy * dy + dz * dz)
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn mirror_x(value: f64, mirrored: bool) -> f64 {
    if mirrored { -value } else { value }
}

fn bounds(points: &[[f32; 3]], scale: f32) -> Option<([f32; 3], f32)> {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for point in points {
        for axis in 0..3 {
            let value = point[axis] * scale;
            min[axis] = min[axis].min(value);
            max[axis] = max[axis].max(value);
        }
    }
    if !min[0].is_finite() {
        return None;
    }
    let centre = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    let dx = max[0] - min[0];
    let dy = max[1] - min[1];
    let dz = max[2] - min[2];
    let radius = (dx * dx + dy * dy + dz * dz).sqrt().max(1.0) * 0.5;
    Some((centre, radius))
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabricBodyView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[area]
    area: Area,
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_point: DrawBodyPoint,
    #[live]
    draw_line: DrawFabricLine,
    #[live]
    draw_text: DrawText,
    #[rust]
    mesh: Option<Arc<BodyMesh>>,
    #[rust]
    posed: Option<Arc<Vec<[f32; 3]>>>,
    #[rust]
    rings: Vec<Ring>,
    #[rust]
    lines: Vec<Line>,
    #[rust]
    pose_mapping: BodyPoseMapping,
    #[rust]
    mesh_scale: f32,
    #[rust]
    centre: [f32; 3],
    #[rust]
    radius: f32,
    #[rust(0.35)]
    yaw: f64,
    #[rust(-0.06)]
    pitch: f64,
    #[rust(1.0)]
    zoom: f64,
    #[rust]
    pan: DVec2,
    #[rust]
    drag: Option<BodyDrag>,
    #[rust(true)]
    mirrored: bool,
}

impl FabricBodyView {
    pub fn set_body(
        &mut self,
        cx: &mut Cx,
        mesh: Arc<BodyMesh>,
        measured: &Measured,
        pose_mapping: BodyPoseMapping,
    ) {
        self.mesh_scale = measured.scale;
        self.rings = measured.rings.clone();
        self.lines = measured.lines.clone();
        self.pose_mapping = pose_mapping;
        if self.posed.is_none() {
            self.fit_bounds(&mesh.vertices);
            self.reset_camera();
        }
        self.mesh = Some(mesh);
        self.redraw(cx);
    }

    pub fn set_pose(&mut self, cx: &mut Cx, posed: Option<Arc<Vec<[f32; 3]>>>) {
        match posed {
            Some(posed)
                if self
                    .mesh
                    .as_ref()
                    .is_some_and(|mesh| mesh.vertices.len() == posed.len()) =>
            {
                if self.posed.is_none() {
                    self.fit_bounds(posed.as_slice());
                    self.reset_camera();
                }
                self.posed = Some(posed);
            }
            _ => {
                self.posed = None;
                if let Some(fit) = self
                    .mesh
                    .as_ref()
                    .and_then(|mesh| bounds(&mesh.vertices, self.mesh_scale))
                {
                    (self.centre, self.radius) = fit;
                    self.reset_camera();
                }
            }
        }
        self.redraw(cx);
    }

    pub fn set_mirrored(&mut self, cx: &mut Cx, mirrored: bool) {
        self.mirrored = mirrored;
        self.redraw(cx);
    }

    fn fit_bounds(&mut self, points: &[[f32; 3]]) {
        if let Some((centre, radius)) = bounds(points, self.mesh_scale) {
            self.centre = centre;
            self.radius = radius;
        }
    }

    fn reset_camera(&mut self) {
        self.yaw = 0.35;
        self.pitch = -0.06;
        self.zoom = 1.0;
        self.pan = dvec2(0.0, 0.0);
    }

    fn project(&self, point: [f32; 3], mesh_point: bool, rect: Rect) -> Option<(DVec2, f32)> {
        let scale = if mesh_point { self.mesh_scale } else { 1.0 } as f64;
        let x = point[0] as f64 * scale - self.centre[0] as f64;
        let y = point[1] as f64 * scale - self.centre[1] as f64;
        let z = point[2] as f64 * scale - self.centre[2] as f64;
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let rx = mirror_x(cy * x + sy * z, self.mirrored);
        let rz = -sy * x + cy * z;
        let ry = cp * y - sp * rz;
        let rz = sp * y + cp * rz;
        let fov = 35.0_f64.to_radians();
        let fit_distance = self.radius as f64 / (fov * 0.5).tan() * 1.2;
        let camera_z = fit_distance / self.zoom.max(0.08) - rz;
        if camera_z <= 0.01 {
            return None;
        }
        let focal = rect.size.y.max(1.0) * 0.5 / (fov * 0.5).tan();
        let centre = rect.pos + rect.size * 0.5 + self.pan;
        let screen = dvec2(centre.x + focal * rx / camera_z, centre.y - focal * ry / camera_z);
        Some((screen, camera_z as f32))
    }

    fn projected_polyline(&self, points: &[[f32; 3]], rect: Rect) -> Vec<(DVec2, f32)> {
        points
            .iter()
            .filter_map(|point| self.project(*point, false, rect))
            .collect()
    }
}

impl Widget for FabricBodyView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        cx.push_clip_rect(rect);
        let Some(mesh) = self.mesh.as_ref() else {
            self.draw_text.color = Vec4f {
                x: 0.48,
                y: 0.55,
                z: 0.62,
                w: 1.0,
            };
            self.draw_text.draw_abs(
                cx,
                rect.pos + rect.size * 0.5 - dvec2(70.0, 5.0),
                "drop a photo to start",
            );
            cx.pop_clip_rect();
            cx.add_aligned_rect_area(&mut self.area, rect);
            return DrawStep::done();
        };
        let posed = self
            .posed
            .as_deref()
            .filter(|posed| posed.len() == mesh.vertices.len())
            .map(Vec::as_slice);
        let display_vertices = posed.unwrap_or(mesh.vertices.as_slice());

        let mut points: Vec<(DVec2, f32)> = display_vertices
            .iter()
            .filter_map(|point| self.project(*point, true, rect))
            .collect();
        points.sort_by(|a, b| b.1.total_cmp(&a.1));
        let min_depth = points.iter().map(|point| point.1).fold(f32::INFINITY, f32::min);
        let max_depth = points
            .iter()
            .map(|point| point.1)
            .fold(f32::NEG_INFINITY, f32::max);
        let depth_span = (max_depth - min_depth).max(0.001);
        self.draw_point.begin_many_instances(cx);
        for (point, depth) in points {
            self.draw_point.depth = (depth - min_depth) / depth_span;
            self.draw_point.draw_abs(
                cx,
                Rect {
                    pos: point - dvec2(1.25, 1.25),
                    size: dvec2(2.5, 2.5),
                },
            );
        }
        self.draw_point.end_many_instances(cx);

        let rings: Vec<(String, Vec<(DVec2, f32)>)> = self
            .rings
            .iter()
            .enumerate()
            .map(|(ring_index, ring)| {
                let points = match (
                    posed,
                    self.pose_mapping.ring_vertices.get(ring_index),
                ) {
                    (Some(posed), Some(indices)) if indices.len() == ring.points.len() => indices
                        .iter()
                        .filter_map(|&index| self.project(*posed.get(index)?, true, rect))
                        .collect(),
                    _ => self.projected_polyline(&ring.points, rect),
                };
                (ring.key.replace('_', " "), points)
            })
            .collect();
        let lines: Vec<(DVec2, DVec2)> = self
            .lines
            .iter()
            .enumerate()
            .filter_map(|(line_index, line)| {
                let (from, to, mesh_points) = match (
                    posed,
                    self.pose_mapping.line_vertices.get(line_index),
                ) {
                    (Some(posed), Some([from, to])) =>
                        (*posed.get(*from)?, *posed.get(*to)?, true),
                    _ => (line.from, line.to, false),
                };
                Some((
                    self.project(from, mesh_points, rect)?.0,
                    self.project(to, mesh_points, rect)?.0,
                ))
            })
            .collect();

        self.draw_line.begin_many_instances(cx);
        self.draw_line.color = Vec4f {
            x: 1.0,
            y: 0.38,
            z: 0.18,
            w: 0.92,
        };
        for (_, points) in &rings {
            for pair in points.windows(2) {
                self.draw_line.segment(cx, pair[0].0, pair[1].0, 1.5);
            }
            if let (Some(first), Some(last)) = (points.first(), points.last()) {
                self.draw_line.segment(cx, last.0, first.0, 1.5);
            }
        }
        self.draw_line.color = Vec4f {
            x: 0.42,
            y: 0.78,
            z: 1.0,
            w: 0.9,
        };
        for (from, to) in lines {
            self.draw_line.segment(cx, from, to, 1.25);
        }
        self.draw_line.end_many_instances(cx);

        self.draw_text.color = Vec4f {
            x: 1.0,
            y: 0.66,
            z: 0.48,
            w: 1.0,
        };
        for (key, points) in rings {
            if let Some(front) = points.iter().min_by(|a, b| a.1.total_cmp(&b.1)) {
                self.draw_text
                    .draw_abs(cx, front.0 + dvec2(4.0, -5.0), &key);
            }
        }
        cx.pop_clip_rect();
        cx.add_aligned_rect_area(&mut self.area, rect);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerDown(event) if event.device.is_primary_hit() => {
                if event.tap_count >= 2 {
                    self.reset_camera();
                    self.redraw(cx);
                    return;
                }
                self.drag = Some(BodyDrag {
                    from: event.abs,
                    yaw: self.yaw,
                    pitch: self.pitch,
                    pan: self.pan,
                    panning: event.modifiers.shift,
                });
            }
            Hit::FingerMove(event) => {
                if let Some(drag) = self.drag {
                    let delta = event.abs - drag.from;
                    if drag.panning {
                        self.pan = drag.pan + delta;
                    } else {
                        self.yaw = drag.yaw - delta.x * 0.008;
                        self.pitch = (drag.pitch + delta.y * 0.007).clamp(-1.35, 1.35);
                    }
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(_) => self.drag = None,
            Hit::FingerScroll(event) => {
                self.zoom = (self.zoom * (-event.scroll.y * 0.004).exp()).clamp(0.15, 12.0);
                self.redraw(cx);
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Grab);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_vertex_mapping_uses_scaled_rest_mesh_positions() {
        let mesh = BodyMesh {
            vertices: vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [9.0, 0.0, 0.0]],
            faces: Vec::new(),
            landmarks: None,
        };
        let measured = Measured {
            values: makepad_fabric_measure::Measurements::sample(),
            scale: 2.0,
            rings: vec![Ring {
                key: "test_ring",
                y_cm: 0.0,
                points: vec![[0.2, 0.0, 0.0], [7.5, 0.0, 0.0]],
                skin_perimeter_cm: 0.0,
                tape_perimeter_cm: 0.0,
            }],
            lines: vec![Line {
                key: "test_line",
                from: [7.5, 0.0, 0.0],
                to: [17.0, 0.0, 0.0],
            }],
        };
        let mapping = map_measurements_to_vertices(&mesh, &measured);
        assert_eq!(mapping.ring_vertices, vec![vec![0, 1]]);
        assert_eq!(mapping.line_vertices, vec![[1, 2]]);
    }

    #[test]
    fn mirror_transform_only_flips_horizontal_view_axis() {
        assert_eq!(mirror_x(12.5, false), 12.5);
        assert_eq!(mirror_x(12.5, true), -12.5);
        assert_eq!(mirror_x(-3.0, true), 3.0);
    }
}
