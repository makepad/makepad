//! Shaped lights (phase 1b): the lights the shape core draws. Polygons come
//! first (B2); Lumi-curve strokes (B3) and scrims (B5) join them.
//!
//! `studio::PreparedLight` keeps a `ShapedLight` for a polygon, stroke or
//! scrim light and hands it the radiance composed so far, in list order, so
//! these lights blend exactly like A3's rect, disc and ring:
//! - the same soft edge: a band of softness × the shape's inside depth,
//!   centred on the edge;
//! - the same hotspot: exp(-4 · hotspot · r²), with r = 1 at the edge;
//! - the same Multiply rule: what is behind is scaled by the light's colour;
//! - the same peak: the centre of an unshaded light is exactly its colour.
//!
//! A polygon lives in the light's unit box (-1..1 on both axes, y up, x
//! toward increasing azimuth), stretched to the light's width and height in
//! the gnomonic tangent plane (half extents tan(w/2) and tan(h/2), in the
//! frame `light_frame` and `project` define). Its corners are rounded there,
//! after the stretch, so a fillet is a round arc on the sky and the fill's
//! distance is Euclidean in tangent units: the soft edge is equally wide on
//! every side, whatever the light's aspect.

use super::*;
use super::shape::*;
use super::studio::{light_frame, project, LightFrame};

/// The narrowest soft edge, in tangent units or radians (about 0.006°). It
/// is studio.rs's MIN_EDGE, so a hard shaped light is as hard as a hard rect.
pub const EDGE_MIN: f32 = 1.0e-4;

/// With hotspot 1 a light's edge is e^-4 (about 2 %) of its centre, as in studio.rs.
const HOTSPOT_FALLOFF: f32 = 4.0;

/// What a shaped light does at one direction.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShapeSample {
    /// How much of the light covers the direction, 0..1. Multiply uses it.
    pub mask: f32,
    /// The weight that multiplies the light's colour: the mask shaped by the
    /// hotspot (and, for strokes and scrims, by their own falloff).
    pub emit: f32,
}

/// One light the shape core draws.
#[derive(Clone, Debug)]
pub struct ShapedLight {
    kind: ShapedKind,
    cull: CullCone,
    blend: Blend,
}

#[derive(Clone, Debug)]
enum ShapedKind {
    Fill(FillLight),
}

impl ShapedLight {
    /// The evaluator for `light`, or None for a disabled light, for a shape
    /// studio.rs draws itself (rect, disc, ring), or for an outline that
    /// cannot be built.
    pub fn new(light: &LightParams) -> Option<ShapedLight> {
        if !light.enabled {
            return None;
        }
        let (kind, cull) = match light.shape() {
            LightShape::Polygon => {
                let fill = FillLight::polygon(light)?;
                let cull = fill.cull();
                (ShapedKind::Fill(fill), cull)
            }
            _ => return None,
        };
        Some(ShapedLight { kind, cull, blend: light.blend() })
    }

    /// Every direction the light can touch lies in this cone.
    pub fn cull(&self) -> CullCone {
        self.cull
    }

    /// Mask and emission at `dir` (unit, in the map's own frame), without the cull.
    pub fn sample(&self, dir: Vec3f) -> ShapeSample {
        match &self.kind {
            ShapedKind::Fill(fill) => fill.sample(dir),
        }
    }

    /// This light over `acc`, the radiance of everything before it in the
    /// list. `color` is the light's `light_color`.
    pub fn compose(&self, dir: Vec3f, acc: Vec3f, color: Vec3f) -> Vec3f {
        if !self.cull.contains(dir) {
            return acc;
        }
        let s = self.sample(dir);
        match self.blend {
            Blend::Add => acc + color * s.emit,
            // studio.rs's rule: where the shape covers, what is behind is scaled by the colour.
            Blend::Multiply => acc * (Vec3f::all(1.0) + (color - Vec3f::all(1.0)) * s.mask),
        }
    }

    /// This light over `acc` with its own light left out: what the key leaves
    /// of the map in the engine's keyless lighting copy (plan 2's N3). A
    /// polygon only adds light over what is behind it, so it leaves `acc` as
    /// it was; B5's scrim keeps its cloth here. It is not `compose` in black:
    /// a stroke with a colour ramp (B3) takes its colour from the ramp, not
    /// from the colour it is handed, and would stay in the copy.
    pub fn hide(&self, _dir: Vec3f, acc: Vec3f) -> Vec3f {
        acc
    }

    /// The direction and cos(angular radius) the engine's key light takes.
    pub fn key_cone(&self) -> (Vec3f, f32) {
        match &self.kind {
            ShapedKind::Fill(fill) => (fill.frame.center, fill.key_cos),
        }
    }
}

/// A filled outline in a light's tangent plane.
#[derive(Clone, Debug)]
struct FillLight {
    frame: LightFrame,
    shape: FillShape,
    /// Half extents tan(w/2), tan(h/2): the hotspot's unit radius.
    half: Vec2f,
    /// Half the soft band, tangent units.
    edge: f32,
    hotspot: f32,
    key_cos: f32,
}

impl FillLight {
    fn polygon(light: &LightParams) -> Option<FillLight> {
        let poly = light.polygon.clone().unwrap_or_default();
        let unit = polygon_unit_points(&poly);
        if unit.len() < 3 {
            return None;
        }
        let (hx, hy) = half_extents(light);
        let sx = box_x_sign();
        // The box's x runs toward increasing azimuth; `project` measures along
        // the frame's `right`, so the outline is mirrored by the frame's sign.
        let pts: Vec<Vec2f> = unit.iter().map(|p| vec2f(sx * p[0].clamp(-1.0, 1.0) * hx, p[1].clamp(-1.0, 1.0) * hy)).collect();
        let outline: Contours = vec![round_corners(&pts, clamp01(light.corner))];
        // Softness 1 ramps the edge right to the deepest point, as studio.rs
        // does: the band is softness × how deep the centre sits inside.
        let depth = inside_depth(&FillShape::new(&outline, EDGE_MIN), &pts, hx.min(hy));
        let edge = 0.5 * (clamp01(light.softness) * depth).max(EDGE_MIN);
        Some(FillLight {
            frame: light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg),
            // Past half the band every distance gives the same coverage, so
            // that is all the reach the shape needs.
            shape: FillShape::new(&outline, edge),
            half: vec2f(hx, hy),
            edge,
            hotspot: HOTSPOT_FALLOFF * clamp01(light.hotspot),
            key_cos: key_cos(light),
        })
    }

    fn cull(&self) -> CullCone {
        // A tangent-plane point r from the centre is atan(r) away on the sky.
        CullCone::new(self.frame.center, (self.shape.outline().bounding_radius() + self.edge).atan())
    }

    fn sample(&self, dir: Vec3f) -> ShapeSample {
        let Some(t) = project(&self.frame, dir) else {
            return ShapeSample::default();
        };
        let d = self.shape.sdf(t);
        let mask = 1.0 - smoothstep(-self.edge, self.edge, d);
        if !(mask > 0.0) {
            return ShapeSample::default();
        }
        let mut emit = mask;
        if self.hotspot > 0.0 {
            let nx = t.x / self.half.x;
            let ny = t.y / self.half.y;
            emit *= (-self.hotspot * (nx * nx + ny * ny)).exp();
        }
        ShapeSample { mask, emit }
    }
}

/// 0..1, with NaN as 0.
fn clamp01(x: f32) -> f32 {
    x.max(0.0).min(1.0)
}

/// The key cone's cosine for a light of this size: cos(max(w, h) / 2), as
/// studio.rs gives a rect.
fn key_cos(light: &LightParams) -> f32 {
    (0.5 * light.width_deg.max(light.height_deg).max(0.1).min(170.0)).to_radians().cos()
}

/// The sign that turns the frame's `right` into the box's +x, which points to
/// increasing azimuth: the viewer's right on the panorama. A3 builds its frame
/// like ibl's softbox (`right = cross(+Y, center)`), and that vector points
/// the other way, so the sign is measured from a reference light on the
/// horizon rather than assumed: whichever way A3 turns `right`, a drawn point
/// at x = +1 lands east of a light facing north.
fn box_x_sign() -> f32 {
    // A light due north on the horizon: increasing azimuth is +X there.
    if light_frame(0.0, 0.0, 0.0).right.x >= 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// The light's half extents in its tangent plane: tan(width/2), tan(height/2).
pub fn half_extents(light: &LightParams) -> (f32, f32) {
    let half = |deg: f32| (0.5 * deg.max(0.1).min(170.0)).to_radians().tan();
    (half(light.width_deg), half(light.height_deg))
}

/// A point of the light's unit box (-1..1, y up, x toward increasing
/// azimuth) as a direction on the sky.
pub fn unit_to_dir(light: &LightParams, unit: [f32; 2]) -> Vec3f {
    let f = light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg);
    let (hx, hy) = half_extents(light);
    (f.center + f.right * (box_x_sign() * unit[0] * hx) + f.up * (unit[1] * hy)).normalize()
}

/// The polygon's corners in the unit box, before rounding: the explicit
/// points when there are at least three, else the regular polygon or star,
/// counter-clockwise from the corner straight up.
pub fn polygon_unit_points(p: &PolygonParams) -> Vec<[f32; 2]> {
    if p.points.len() >= 3 {
        return p.points.clone();
    }
    let n = p.sides.clamp(3, 64) as usize;
    let star = if p.star.is_finite() { p.star.clamp(0.0, 0.95) } else { 0.0 };
    let step = std::f32::consts::TAU / n as f32;
    // The regular polygon's inner radius (its apothem, with corners on the unit circle).
    let inner = (std::f32::consts::PI / n as f32).cos() * (1.0 - star);
    let mut out = Vec::with_capacity(2 * n);
    for k in 0..n {
        let a = std::f32::consts::FRAC_PI_2 + step * k as f32;
        out.push([a.cos(), a.sin()]);
        if star > 0.0 {
            let b = a + 0.5 * step;
            out.push([inner * b.cos(), inner * b.sin()]);
        }
    }
    out
}

/// Rounds every corner of a closed polygon with a circular arc. `corner` is
/// the fraction of the shorter neighbouring edge each arc takes, so two arcs
/// on one edge never overlap (at 1 they meet in its middle). A concave
/// corner is rounded the same way: its arc's centre lies outside the shape
/// and the notch gets a round bottom.
fn round_corners(pts: &[Vec2f], corner: f32) -> Vec<[f32; 2]> {
    use std::f32::consts::{PI, TAU};
    let n = pts.len();
    if !(corner > 0.0) || n < 3 {
        return pts.iter().map(|p| [p.x, p.y]).collect();
    }
    let corner = corner.min(1.0);
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n {
        let prev = pts[(i + n - 1) % n];
        let v = pts[i];
        let next = pts[(i + 1) % n];
        let (lp, ln) = ((prev - v).length(), (next - v).length());
        if lp < 1.0e-9 || ln < 1.0e-9 {
            out.push([v.x, v.y]);
            continue;
        }
        let u1 = (prev - v) * (1.0 / lp);
        let u2 = (next - v) * (1.0 / ln);
        // The angle between the two edges at v, and half of it.
        let full = (u1.x * u2.x + u1.y * u2.y).max(-1.0).min(1.0).acos();
        let half = 0.5 * full;
        // A straight-through corner has nothing to round; a hairpin would need
        // an arc of almost no radius.
        if half > 1.5700 || half < 1.0e-3 {
            out.push([v.x, v.y]);
            continue;
        }
        // A circle tangent to both edges touches them `tangent` from v; its
        // radius is tangent × tan(half) and its centre r / sin(half) from v,
        // along the bisector.
        let tangent = corner * 0.5 * lp.min(ln);
        let radius = tangent * half.tan();
        let centre = v + (u1 + u2).normalize() * (radius / half.sin());
        let p1 = v + u1 * tangent;
        let p2 = v + u2 * tangent;
        let a1 = (p1.y - centre.y).atan2(p1.x - centre.x);
        let a2 = (p2.y - centre.y).atan2(p2.x - centre.x);
        let mut da = a2 - a1;
        // The arc turns by pi - full (< pi): the short way round.
        while da > PI {
            da -= TAU;
        }
        while da < -PI {
            da += TAU;
        }
        // About one point per 15° of turn.
        let steps = (((PI - full) / 0.26).ceil() as usize).clamp(1, 12);
        for k in 0..=steps {
            let a = a1 + da * (k as f32 / steps as f32);
            out.push([centre.x + radius * a.cos(), centre.y + radius * a.sin()]);
        }
    }
    out
}

/// How deep the light's centre sits inside the outline, in tangent units:
/// or, when the centre is outside (a custom outline off to one side), the
/// corners' centroid. `fallback` when neither is inside.
fn inside_depth(shape: &FillShape, pts: &[Vec2f], fallback: f32) -> f32 {
    let mut centroid = vec2f(0.0, 0.0);
    for p in pts {
        centroid = centroid + *p;
    }
    centroid = centroid * (1.0 / pts.len().max(1) as f32);
    for probe in [vec2f(0.0, 0.0), centroid] {
        if shape.winding_brute(probe) != 0 {
            return shape.outline().exact_distance(probe).max(EDGE_MIN);
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::studio::light_color;

    /// Studio mode over a black backdrop, so a pixel shows the lights alone.
    fn dark_studio(lights: Vec<LightParams>) -> HdriParams {
        let mut p = HdriParams::default();
        p.mode = "studio".to_string();
        p.studio.top = [0.0; 3];
        p.studio.horizon = [0.0; 3];
        p.studio.floor = [0.0; 3];
        p.lights = lights;
        p
    }

    /// A hard 40x40° hexagon due east on the horizon.
    fn polygon_light() -> LightParams {
        LightParams {
            shape: "polygon".to_string(),
            azimuth_deg: 90.0,
            elevation_deg: 0.0,
            width_deg: 40.0,
            height_deg: 40.0,
            corner: 0.0,
            softness: 0.0,
            hotspot: 0.0,
            polygon: Some(PolygonParams::default()),
            ..LightParams::default()
        }
    }

    fn mask_at(light: &LightParams, unit: [f32; 2]) -> f32 {
        ShapedLight::new(light).expect("the light builds").sample(unit_to_dir(light, unit)).mask
    }

    /// Directions spread evenly over the sphere (a Fibonacci lattice).
    fn sphere_point(i: usize, n: usize) -> Vec3f {
        let y = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let a = i as f32 * 2.399_963;
        vec3f(r * a.cos(), y, r * a.sin())
    }

    #[test]
    fn a_polygon_light_is_drawn_through_the_studio() {
        let light = polygon_light();
        let color = light_color(&light);
        let env = Env::new(&dark_studio(vec![light]));
        let centre = env.radiance(dir_from_az_el(90.0, 0.0));
        assert!((centre - color).length() < 1.0e-4 * color.length(), "the centre is the light's colour: {centre:?} vs {color:?}");
        // The hexagon's top corner is 20° up; 30° up is outside it.
        let above = env.radiance(dir_from_az_el(90.0, 30.0));
        assert!(above.length() < 1.0e-6, "{above:?}");
    }

    #[test]
    fn unit_points_map_through_the_light_frame() {
        let mut light = polygon_light();
        light.roll_deg = 25.0;
        light.width_deg = 50.0;
        light.height_deg = 30.0;
        let frame = light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg);
        let (hx, hy) = half_extents(&light);
        let sx = box_x_sign();
        for u in [[0.0, 0.0], [1.0, 0.0], [0.0, -1.0], [0.6, 0.7]] {
            let t = project(&frame, unit_to_dir(&light, u)).expect("in front of the light");
            assert!((t.x - sx * u[0] * hx).abs() < 1.0e-5 && (t.y - u[1] * hy).abs() < 1.0e-5, "{u:?} -> {t:?}");
        }
        // Whichever way A3 points its frame's `right`, the box's +x is east of a
        // light facing north on the horizon, and +y is above it.
        let north = LightParams { azimuth_deg: 0.0, elevation_deg: 0.0, roll_deg: 0.0, ..polygon_light() };
        let (az, el) = az_el_from_dir(unit_to_dir(&north, [1.0, 0.0]));
        assert!(az > 1.0 && az < 90.0, "x = +1 lies east of north: az {az}");
        assert!(el.abs() < 1.0e-3, "{el}");
        let (_, up) = az_el_from_dir(unit_to_dir(&north, [0.0, 1.0]));
        assert!(up > 1.0, "y = +1 lies above: el {up}");
    }

    #[test]
    fn a_star_has_dark_notches_between_lit_arms() {
        let mut light = polygon_light();
        light.polygon = Some(PolygonParams { sides: 5, star: 0.8, points: Vec::new() });
        assert_eq!(mask_at(&light, [0.0, 0.8]), 1.0, "up the first arm");
        assert_eq!(mask_at(&light, [0.0, 0.0]), 1.0, "the centre");
        // Half way between the first two arms (90° and 162°), well out from the centre.
        let a = 126.0f32.to_radians();
        assert_eq!(mask_at(&light, [0.6 * a.cos(), 0.6 * a.sin()]), 0.0, "in the notch");
    }

    #[test]
    fn explicit_points_replace_the_regular_polygon() {
        let mut light = polygon_light();
        assert_eq!(mask_at(&light, [0.8, 0.0]), 1.0, "inside the default hexagon");
        light.polygon = Some(PolygonParams { sides: 6, star: 0.0, points: vec![[-1.0, -1.0], [1.0, -1.0], [0.0, 1.0]] });
        assert_eq!(mask_at(&light, [0.8, 0.0]), 0.0, "outside the triangle");
        assert_eq!(mask_at(&light, [0.0, -0.5]), 1.0, "inside the triangle");
    }

    #[test]
    fn full_rounding_turns_a_square_into_its_incircle() {
        let mut light = polygon_light();
        light.polygon = Some(PolygonParams { sides: 4, star: 0.0, points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]] });
        assert_eq!(mask_at(&light, [0.9, 0.9]), 1.0, "a sharp square keeps its corner");
        light.corner = 1.0;
        assert_eq!(mask_at(&light, [0.9, 0.9]), 0.0, "the rounded corner is cut away");
        assert_eq!(mask_at(&light, [0.95, 0.0]), 1.0, "an edge's middle stays");
        assert_eq!(mask_at(&light, [0.0, -0.95]), 1.0);
    }

    #[test]
    fn a_multiply_polygon_is_a_flag_that_never_adds_light() {
        let mut flag = polygon_light();
        flag.blend = "multiply".to_string();
        flag.rgb = Some([0.0; 3]);
        let mut p = dark_studio(vec![flag]);
        p.studio.top = [1.0; 3];
        p.studio.horizon = [1.0; 3];
        p.studio.floor = [1.0; 3];
        let env = Env::new(&p);
        assert!(env.radiance(dir_from_az_el(90.0, 0.0)).length() < 1.0e-6, "the flag blacks out its middle");
        let bare = Env::new(&HdriParams { lights: Vec::new(), ..p.clone() });
        for i in 0..400 {
            let d = sphere_point(i, 400);
            let (with, without) = (env.radiance(d), bare.radiance(d));
            assert!(with.x <= without.x + 1.0e-6 && with.y <= without.y + 1.0e-6 && with.z <= without.z + 1.0e-6, "{d:?}");
        }
    }

    #[test]
    fn the_cull_cone_holds_every_lit_direction() {
        let mut light = polygon_light();
        light.softness = 0.6;
        light.width_deg = 70.0;
        light.roll_deg = 30.0;
        light.polygon = Some(PolygonParams { sides: 7, star: 0.4, points: Vec::new() });
        let shaped = ShapedLight::new(&light).unwrap();
        let cull = shaped.cull();
        let mut lit = 0;
        for i in 0..20000 {
            let d = sphere_point(i, 20000);
            if shaped.sample(d).mask > 0.0 {
                lit += 1;
                assert!(cull.contains(d), "{d:?}");
            }
        }
        assert!(lit > 100, "the test must see the light ({lit})");
    }
}
