//! Shaped lights (phase 1b): the lights the shape core draws. Polygons come
//! first (B2); Lumi-curve strokes (B3) and scrims (B5) join them.
//!
//! `studio::PreparedLight` keeps a `ShapedLight` for a polygon, stroke or
//! scrim light and hands it the radiance composed so far, in list order, so
//! these lights blend exactly like A3's rect, disc and ring:
//! - the same soft edge: a band of softness × the shape's inside depth,
//!   centred on the edge;
//! - the same hotspot: exp(-4 · hotspot · r²), with r = 1 at the edge;
//! - the same Multiply rule: what is behind is scaled by the light's colour,
//!   as far as it covers (a polygon flag's coverage takes its hotspot in, as
//!   A3's flags' does);
//! - the same peak: the centre of an unshaded light is exactly its colour.
//!
//! A polygon lives in the light's unit box (-1..1 on both axes, y up, x
//! toward increasing azimuth), stretched to the light's width and height in
//! the gnomonic tangent plane (half extents tan(w/2) and tan(h/2), in the
//! frame `light_frame` and `project` define). Its corners are rounded there,
//! after the stretch, so a fillet is a round arc on the sky and the fill's
//! distance is Euclidean in tangent units: the soft edge is equally wide on
//! every side, whatever the light's aspect.
//!
//! A stroke is placed by its own points (`stroke_of`): a spline through
//! (azimuth, elevation) directions, cut into great-circle pieces, so its
//! distances are true angles on the sphere and its half width is its inside
//! depth. Its key light is not integrated in the light's box, as a polygon's
//! is, but over the cones round its centre line (`ShapedLight::stroke_cones`),
//! where studio.rs samples the map's drawing of it along its band
//! (`ShapedLight::stroke_path`).
//!
//! A scrim is a rect in the light's box (A3's rounding and soft edge) of
//! diffusion cloth lit from behind: it glows with a Gaussian from its middle,
//! which stands in for the hotspot, and it is the one Add light that hides
//! what is behind it (`acc × (1 − mask) + colour × glow`). With Multiply it is
//! a flag of its shape, by the rule above.

use super::*;
use super::shape::*;
use super::studio::{held_light_color, light_frame, project, LightFrame};
use super::spectrum::fixed_source;

/// The narrowest soft edge, in tangent units or radians (about 0.006°). It
/// is studio.rs's MIN_EDGE, so a hard shaped light is as hard as a hard rect.
pub const EDGE_MIN: f32 = 1.0e-4;

/// With hotspot 1 a light's edge is e^-4 (about 2 %) of its centre, as in studio.rs.
const HOTSPOT_FALLOFF: f32 = 4.0;

/// What a shaped light does at one direction.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShapeSample {
    /// How much of the shape covers the direction, 0..1: what a scrim's cloth
    /// hides, and how far a stroke or scrim flag (Multiply) scales what is behind.
    pub mask: f32,
    /// The weight that multiplies the light's colour: the mask shaped by the
    /// hotspot (and, for strokes and scrims, by their own falloff). A polygon
    /// flag covers by it, hotspot and all, as A3's flags do.
    pub emit: f32,
}

/// One light the shape core draws.
#[derive(Clone, Debug)]
pub struct ShapedLight {
    kind: ShapedKind,
    cull: CullCone,
    blend: Blend,
    /// A scrim hides what is behind it; the other shapes add, as A3's lights do.
    occludes: bool,
    /// P1: how far a picture's Penumbra widens the soft edge inward, as a
    /// share of the shape's own depth (`with_inset`, `depth`); 0 draws the
    /// bare shape. `compose` and `hide` draw with it, `sample` without.
    inset: f32,
}

#[derive(Clone, Debug)]
enum ShapedKind {
    Fill(FillLight),
    Stroke(StrokeLight),
    Scrim(ScrimLight),
}

impl ShapedLight {
    /// The evaluator for `light`, or None for a disabled light, for a shape
    /// studio.rs draws itself (rect, disc, ring), or for an outline that
    /// cannot be built (a stroke with fewer than two usable points). The
    /// light is read as clamp() holds it (`LightParams::clamped`, D17): the
    /// builders below read its box, softness, corner, hotspot, colour and
    /// blocks as they are.
    pub fn new(light: &LightParams) -> Option<ShapedLight> {
        if !light.enabled || !matches!(light.shape(), LightShape::Polygon | LightShape::Stroke | LightShape::Scrim) {
            return None;
        }
        let light = &light.clamped();
        let (kind, cull) = match light.shape() {
            LightShape::Polygon => {
                let fill = FillLight::polygon(light)?;
                let cull = fill.cull();
                (ShapedKind::Fill(fill), cull)
            }
            LightShape::Stroke => {
                let stroke = StrokeLight::new(light)?;
                let cull = stroke.path.cull();
                (ShapedKind::Stroke(stroke), cull)
            }
            LightShape::Scrim => {
                let scrim = ScrimLight::new(light);
                let cull = scrim.cull();
                (ShapedKind::Scrim(scrim), cull)
            }
            _ => return None,
        };
        let occludes = matches!(kind, ShapedKind::Scrim(_));
        Some(ShapedLight { kind, cull, blend: light.blend(), occludes, inset: 0.0 })
    }

    /// The light with its soft edge widened inward by `inset` of its depth
    /// (P1: a picture in stretch or fill, whose border is the light's edge,
    /// `LightImage::edge_inset`): every edge ramp of the shape then runs
    /// from half its band plus `inset` x the depth inside to half its band
    /// outside, one smoothstep, so the light's reach and its cull stay. A
    /// polygon's distance then reaches as deep as its ramp starts
    /// (`FillLight::widened`); at 0 it is the bare shape, untouched.
    pub fn with_inset(self, inset: f32) -> ShapedLight {
        let kind = match self.kind {
            ShapedKind::Fill(fill) if inset > 0.0 => ShapedKind::Fill(fill.widened(inset)),
            kind => kind,
        };
        ShapedLight { kind, inset, ..self }
    }

    /// The depth the shape's Softness takes, tangent units (a stroke's in
    /// radians): how deep a polygon's centre sits inside it, a stroke's half
    /// width (before its profile), a scrim's shorter half extent. Penumbra 1
    /// widens the edge that far in (`with_inset`).
    pub fn depth(&self) -> f32 {
        match &self.kind {
            ShapedKind::Fill(fill) => fill.depth,
            ShapedKind::Stroke(stroke) => stroke.half_width,
            ShapedKind::Scrim(scrim) => scrim.short,
        }
    }

    /// Every direction the light can touch lies in this cone.
    pub fn cull(&self) -> CullCone {
        self.cull
    }

    /// Mask and emission at `dir` (unit, in the map's own frame), without
    /// the cull: the bare shape, whatever picture widens its edge.
    pub fn sample(&self, dir: Vec3f) -> ShapeSample {
        self.sample_inset(dir, 0.0)
    }

    /// `sample` with the edge widened inward by `inset` of the depth.
    fn sample_inset(&self, dir: Vec3f, inset: f32) -> ShapeSample {
        match &self.kind {
            ShapedKind::Fill(fill) => fill.sample(dir, inset),
            ShapedKind::Stroke(stroke) => stroke.sample_at(dir, inset).0,
            ShapedKind::Scrim(scrim) => scrim.sample(dir, inset),
        }
    }

    /// This light over `acc`, the radiance of everything before it in the
    /// list. `color` is the light's `light_color`; a stroke with a ramp
    /// carries its own colour along its length instead.
    pub fn compose(&self, dir: Vec3f, acc: Vec3f, color: Vec3f) -> Vec3f {
        if !self.cull.contains(dir) {
            return acc;
        }
        // How much a flag of this shape covers: a polygon's coverage takes its
        // hotspot in, as A3's flags' does (studio.rs's `mask`, D10, L:26); a
        // stroke and a scrim flag by their bare shape.
        let (s, color, flag) = match &self.kind {
            ShapedKind::Fill(fill) => {
                let s = fill.sample(dir, self.inset);
                (s, color, s.emit)
            }
            ShapedKind::Stroke(stroke) => {
                let (s, t) = stroke.sample_at(dir, self.inset);
                (s, stroke.color_at(t, color), s.mask)
            }
            ShapedKind::Scrim(scrim) => {
                let s = scrim.sample(dir, self.inset);
                (s, color, s.mask)
            }
        };
        match self.blend {
            // studio.rs's rule: where the shape covers, what is behind is scaled by the colour.
            Blend::Multiply => acc * (Vec3f::all(1.0) + (color - Vec3f::all(1.0)) * flag),
            // Cloth in front of what is behind it: it hides that, then glows.
            Blend::Add if self.occludes => acc * (1.0 - s.mask) + color * s.emit,
            Blend::Add => acc + color * s.emit,
        }
    }

    /// This light over `acc` with its own light left out. That is what the
    /// key leaves of the map in the engine's keyless lighting copy (plan 2's
    /// N3), and what a scrim listed after the key leaves of the key's light
    /// in its integral (`studio::key_emission`, B5's A1): only Add lights
    /// are left out this way. A scrim's cloth still hides what is behind it
    /// and its glow is gone (`compose` without its `color * s.emit` term); a
    /// polygon or a stroke leaves `acc` as it was. It is not `compose` in
    /// black: a stroke with a colour ramp (B3) takes its colour from the ramp
    /// (`color_at`), not from the colour it is handed, so composed in black
    /// it would stay in the keyless copy and the engine would light it twice.
    pub fn hide(&self, dir: Vec3f, acc: Vec3f) -> Vec3f {
        if !self.hides_behind() || !self.cull.contains(dir) {
            return acc;
        }
        acc * (1.0 - self.sample_inset(dir, self.inset).mask)
    }

    /// Whether this is an Add light that hides what is behind it (a scrim):
    /// the one shape that dims the lights listed before it as an Add light,
    /// by `hide`'s factor, so `studio::key_emission` takes it as it takes a
    /// flag (and as it takes a window filled with sky, which its gobo makes).
    pub fn hides_behind(&self) -> bool {
        self.occludes && self.blend == Blend::Add
    }

    /// A stroke's cones round its centre line, `(axis, cos_radius,
    /// cos_cover)`: the line's mean direction, the key cone (the line's
    /// widest angle from it plus the widest half width: how far the stroke
    /// spreads from its middle) and the cone that holds everything it draws,
    /// soft edge included (at most the key cone's cosine). None for a filled
    /// shape and a scrim, which lie in their light's box, where studio.rs
    /// integrates them like a rect (a scrim's glow from its middle included).
    /// A stroke's points place it, not its frame, so studio.rs integrates its
    /// key over `cos_cover` instead (`studio::over_cone`).
    pub fn stroke_cones(&self) -> Option<(Vec3f, f32, f32)> {
        match &self.kind {
            ShapedKind::Fill(_) | ShapedKind::Scrim(_) => None,
            ShapedKind::Stroke(stroke) => Some((stroke.key.0, stroke.key.1, stroke.key_cos_cover)),
        }
    }

    /// D2: the share of a thin line along a stroke's centre line that a
    /// surface facing its key's axis receives (∫ cosθ dℓ / ∫ dℓ, θ from the
    /// axis): how far out round its middle the stroke lies, whatever its
    /// width. None for the other shapes.
    pub fn stroke_line_facing(&self) -> Option<f32> {
        match &self.kind {
            ShapedKind::Fill(_) | ShapedKind::Scrim(_) => None,
            ShapedKind::Stroke(stroke) => Some(stroke.line_facing),
        }
    }

    /// A stroke's centre line, with its reach (how far from the line it
    /// draws, soft edge included): what tells the key's integral where along
    /// its cone the stroke is. None for the other shapes.
    pub fn stroke_path(&self) -> Option<&ArcPath> {
        match &self.kind {
            ShapedKind::Fill(_) | ShapedKind::Scrim(_) => None,
            ShapedKind::Stroke(stroke) => Some(&stroke.path),
        }
    }
}

/// A filled outline in a light's tangent plane.
#[derive(Clone, Debug)]
struct FillLight {
    frame: LightFrame,
    /// The rounded outline, tangent units, that `shape` is built from: kept
    /// so `widened` can build it again with more reach.
    outline: Contours,
    shape: FillShape,
    /// Half extents tan(w/2), tan(h/2): the hotspot's unit radius.
    half: Vec2f,
    /// Half the soft band, tangent units.
    edge: f32,
    /// How deep the centre sits inside the outline, tangent units: the
    /// depth the Softness (and a picture's Penumbra) takes.
    depth: f32,
    hotspot: f32,
}

impl FillLight {
    /// `light` is held by clamp() (`ShapedLight::new`).
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
        // The corners lie in the box already (`polygon_unit_points`).
        let pts: Vec<Vec2f> = unit.iter().map(|p| vec2f(sx * p[0] * hx, p[1] * hy)).collect();
        let outline: Contours = vec![round_corners(&pts, light.corner)];
        // Softness 1 ramps the edge right to the deepest point, as studio.rs
        // does: the band is softness × how deep the centre sits inside.
        let depth = inside_depth(&FillShape::new(&outline, EDGE_MIN), &pts, hx.min(hy));
        let edge = 0.5 * (light.softness * depth).max(EDGE_MIN);
        Some(FillLight {
            frame: light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg),
            // Past half the band every distance gives the same coverage, so
            // that is all the reach the bare shape needs (`widened` for a
            // picture's Penumbra, whose ramp starts deeper).
            shape: FillShape::new(&outline, edge),
            outline,
            half: vec2f(hx, hy),
            edge,
            depth,
            hotspot: HOTSPOT_FALLOFF * light.hotspot,
        })
    }

    /// The fill whose distance reaches as deep as its edge ramp starts once
    /// a picture's Penumbra widens it inward by `inset` of the depth
    /// (`ShapedLight::with_inset`, P1). The shape caps every distance at its
    /// reach, half the band for the bare shape; read through that cap, the
    /// whole inside sat one value short of the widened ramp's start, so the
    /// light was dimmed flat, or black at Softness 0. Built to the ramp's
    /// start, every distance the ramp reads is exact, and a deeper one,
    /// capped there, is full coverage, as it should be. Only the bins' padding
    /// grows: the outline, its cull and every distance within the old reach
    /// (the bare `sample`) stay as they were, bit for bit.
    fn widened(self, inset: f32) -> FillLight {
        let shape = FillShape::new(&self.outline, self.edge + inset * self.depth);
        FillLight { shape, ..self }
    }

    fn cull(&self) -> CullCone {
        // A tangent-plane point r from the centre is atan(r) away on the sky.
        CullCone::new(self.frame.center, (self.shape.outline().bounding_radius() + self.edge).atan())
    }

    /// Mask and emission at `dir`, the edge widened inward by `inset` of the depth.
    fn sample(&self, dir: Vec3f, inset: f32) -> ShapeSample {
        let Some(t) = project(&self.frame, dir) else {
            return ShapeSample::default();
        };
        let d = self.shape.sdf(t);
        let mask = 1.0 - smoothstep(-self.edge - inset * self.depth, self.edge, d);
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

/// Largest piece of a stroke's centre line, in degrees of arc: short enough
/// that a great-circle piece follows the (azimuth, elevation) curve the
/// panorama shows.
const MAX_PIECE_DEG: f32 = 1.0;
/// Most points a centre line is cut into; a very long stroke gets longer pieces.
const MAX_LINE_POINTS: f32 = 4096.0;
/// Taps of a stroke's colour ramp, joined by straight lines. Kelvin to RGB is
/// smooth, so 32 steps are far below a visible band.
const RAMP_TAPS: usize = 33;

/// A Lumi-curve: a stroke of light along great-circle pieces.
#[derive(Clone, Debug)]
struct StrokeLight {
    path: ArcPath,
    /// Half the stroke's width before the profile, radians.
    half_width: f32,
    softness: f32,
    hotspot: f32,
    profile: Vec<f32>,
    /// The colour along the stroke (x 2^intensity_ev, like `light_color`), at
    /// RAMP_TAPS even taps over t. None is the light's one colour all along.
    ramp: Option<Vec<Vec3f>>,
    /// The key cone: the centre line's mean direction and cos of its radius.
    key: (Vec3f, f32),
    /// Plan 2's `EnvSun.cos_cover`: the cone round the key's axis that holds
    /// the whole stroke (at most the key cone's cosine).
    key_cos_cover: f32,
    /// D2: the share of a thin line along the centre line that a surface
    /// facing the key's axis receives, ∫ cosθ dℓ / ∫ dℓ with θ from the axis.
    line_facing: f32,
}

/// What places a stroke and how far from its centre line it draws: shared by
/// its drawing (`StrokeLight`) and the box its extras are laid in
/// (`stroke_box`), so the two always agree on where it is.
struct StrokeLine {
    /// The centre line, as unit directions.
    line: Vec<Vec3f>,
    /// Half the stroke's width before the profile, radians.
    half_width: f32,
    softness: f32,
    profile: Vec<f32>,
    /// The widest half width along it (the profile's widest), radians.
    widest_half: f32,
    /// How far from the line it draws, soft edge included.
    reach: f32,
}

impl StrokeLine {
    /// None with fewer than two usable points. `light` is held by clamp()
    /// (its softness is read as it is), and `stroke` is the light's stroke as
    /// `stroke_of` gives it, held to its ranges by clamp()'s rule: its width,
    /// curviness and profile are read as they are.
    fn new(light: &LightParams, stroke: &StrokeParams) -> Option<StrokeLine> {
        let line = stroke_centre_line(stroke);
        if line.len() < 2 {
            return None;
        }
        let half_width = (0.5 * stroke.width_deg).to_radians();
        let softness = light.softness;
        let profile = stroke.profile.clone();
        let widest = if profile.is_empty() { 1.0 } else { profile.iter().fold(0.0f32, |m, v| m.max(*v)) };
        let widest_half = half_width * widest;
        // Past the widest half width plus half its soft band nothing is lit.
        let reach = widest_half + 0.5 * (softness * widest_half).max(EDGE_MIN) + 1.0e-6;
        Some(StrokeLine { line, half_width, softness, profile, widest_half, reach })
    }

    /// The key's cones round the line, `(axis, cos_radius, cos_cover)`: the
    /// line's mean direction, the key cone (its widest angle from it plus the
    /// widest half width) and the covering cone (plan 2's `cos_cover`).
    ///
    /// Every lit direction lies within `reach` of the centre line, and a
    /// great-circle piece bows at most half its length from its ends, so the
    /// line's widest angle from the axis plus both holds the whole stroke,
    /// soft edge included. `around` over the same line gives the same axis
    /// for both cones. A stroke all round the sky has no cone: the whole
    /// sphere then.
    fn cones(&self) -> (Vec3f, f32, f32) {
        let cone = CullCone::around(&self.line, self.widest_half);
        let key_cos = cone.cos_max.max(-1.0);
        let piece = self.line.windows(2).fold(0.0f32, |m, w| m.max(arc_between(w[0], w[1])));
        let cos_cover = CullCone::around(&self.line, self.reach + 0.5 * piece).cos_max.max(-1.0).min(key_cos);
        (cone.axis, key_cos, cos_cover)
    }
}

/// The angle between two unit directions, radians.
fn arc_between(a: Vec3f, b: Vec3f) -> f32 {
    a.dot(b).clamp(-1.0, 1.0).acos()
}

/// D3 (I1): the box a stroke's extras are laid in, its gobo, aim, barn doors
/// and wear, and a picture as its content: `(frame, half extents)` in the
/// tangent plane of the stroke's own middle (its key's axis), turned by the
/// light's roll, and the square there that holds its covering cone, which
/// holds everything the stroke draws (as the key's share once laid a
/// stroke's extras). A stroke's points place it, not the light's azimuth,
/// elevation, width and height, so that nominal box is never used: laid in
/// it, a framed window blacked the stroke out where the stroke left the box,
/// and past 87 degrees from it left the stroke bare.
///
/// A stroke that reaches further than the tangent plane does (about 87
/// degrees from its middle) gets the plane's widest square; past it the face
/// is held at the plane's limit (`studio::project_held`). None for any other
/// light, and for a stroke that cannot be built, which draws nothing. Of a
/// light as clamp() holds it, read as it is (`half_extents`), as
/// `ShapedLight::new` draws it.
pub fn stroke_box(light: &LightParams) -> Option<(LightFrame, Vec2f)> {
    if light.shape() != LightShape::Stroke {
        return None;
    }
    let (axis, _, cos_cover) = StrokeLine::new(light, &stroke_of(light))?.cones();
    let c = cos_cover.max(super::studio::PROJECT_MIN_COS);
    let half = (1.0 - c * c).max(0.0).sqrt() / c;
    let (azimuth_deg, elevation_deg) = az_el_from_dir(axis);
    Some((light_frame(azimuth_deg, elevation_deg, light.roll_deg), vec2f(half, half)))
}

impl StrokeLight {
    /// `light` is held by clamp() (`ShapedLight::new`).
    fn new(light: &LightParams) -> Option<StrokeLight> {
        let stroke = stroke_of(light);
        let geometry = StrokeLine::new(light, &stroke)?;
        let path = ArcPath::new(&geometry.line, false, geometry.reach);
        if path.is_empty() {
            return None;
        }
        // The key's own light is not summed here: studio.rs integrates the
        // map's drawing of the stroke over its covering cone (D1), its band
        // lapping over itself in a tight bend, the flags after it and what
        // hangs in front of it all included.
        let (axis, key_cos, key_cos_cover) = geometry.cones();
        let hotspot = HOTSPOT_FALLOFF * light.hotspot;
        let ramp = stroke_ramp(light, &stroke);
        // D2: how far out round the axis the line lies, by arc length (its
        // pieces are only roughly even): a ring at elevation 10 gives 0.17,
        // an arch through its own middle far more.
        let (mut along, mut length) = (Vec3f::default(), 0.0f32);
        for w in geometry.line.windows(2) {
            let len = arc_between(w[0], w[1]);
            along += (w[0] + w[1]) * (0.5 * len);
            length += len;
        }
        let line_facing = if length > 0.0 { (along.dot(axis) / length).max(0.0).min(1.0) } else { 1.0 };
        let StrokeLine { half_width, softness, profile, .. } = geometry;
        Some(StrokeLight { path, half_width, softness, hotspot, profile, ramp, key: (axis, key_cos), key_cos_cover, line_facing })
    }

    /// Mask and emission at `dir`, and how far along the stroke the nearest
    /// point lies (t, 0..1 by arc length). `inset` widens the edge inward by
    /// that share of the half width there.
    fn sample_at(&self, dir: Vec3f, inset: f32) -> (ShapeSample, f32) {
        let hit = self.path.nearest(dir);
        // Written negated so a NaN distance is dark too.
        if !(hit.dist < self.path.reach()) {
            return (ShapeSample::default(), 0.0);
        }
        let w = profile_at(&self.profile, hit.t);
        let hw = self.half_width * w;
        // As a ring's edge: the band is softness x the half width, centred on the edge.
        let edge = 0.5 * (self.softness * hw).max(EDGE_MIN);
        let mask = 1.0 - smoothstep(hw - edge - inset * hw, hw + edge, hit.dist);
        if !(mask > 0.0) {
            return (ShapeSample::default(), hit.t);
        }
        // A tapering stroke also fades, so a zero-width tip leaves no bright dot.
        let mut emit = mask * w.min(1.0);
        if self.hotspot > 0.0 {
            let across = hit.dist / hw.max(EDGE_MIN);
            emit *= (-self.hotspot * across * across).exp();
        }
        (ShapeSample { mask, emit }, hit.t)
    }

    /// The stroke's colour at `t`: the ramp's, or `color` (the light's one
    /// colour) without a ramp.
    fn color_at(&self, t: f32, color: Vec3f) -> Vec3f {
        match &self.ramp {
            Some(ramp) => ramp_at(ramp, t),
            None => color,
        }
    }
}

/// The stroke a light draws: its own block as clamp() holds it
/// (`StrokeParams::clamped`: at most `POINT_LIMIT` finite points, the width,
/// curviness, profile and ends in their ranges), with the arch over the
/// light's place standing in for points that are missing (no block, or a
/// block with fewer than two points). The block's other settings are kept,
/// so a stroke that was only given a width or a ramp still follows its
/// light's azimuth, elevation and width. The renderer, the panel and the
/// point editor all read a stroke through this.
pub fn stroke_of(light: &LightParams) -> StrokeParams {
    let arch = StrokeParams::around(light.azimuth_deg, light.elevation_deg, light.width_deg);
    match light.stroke.as_ref().map(StrokeParams::clamped) {
        Some(stroke) if stroke.points.len() >= 2 => stroke,
        Some(stroke) => StrokeParams { points: arch.points, ..stroke },
        None => arch,
    }
}

/// The width multiplier at `t` (0..1 along the stroke): the profile's
/// samples spread evenly over 0..1, joined by straight lines; 1 without a profile.
pub fn profile_at(profile: &[f32], t: f32) -> f32 {
    match profile.len() {
        0 => 1.0,
        1 => profile[0],
        n => {
            let x = t.max(0.0).min(1.0) * (n - 1) as f32;
            let i = (x.floor() as usize).min(n - 2);
            let f = x - i as f32;
            profile[i] + (profile[i + 1] - profile[i]) * f
        }
    }
}

/// The ramp's colour at `t` (0..1): its taps joined by straight lines.
fn ramp_at(ramp: &[Vec3f], t: f32) -> Vec3f {
    let n = ramp.len();
    if n < 2 {
        return ramp.first().copied().unwrap_or_default();
    }
    let x = t.max(0.0).min(1.0) * (n - 1) as f32;
    let i = (x.floor() as usize).min(n - 2);
    let f = x - i as f32;
    ramp[i] + (ramp[i + 1] - ramp[i]) * f
}

/// What `light_color` gives this light with another colour: the explicit
/// `rgb` when Some, else its colour at the Kelvin `kelvin`, which a named
/// lamp with a temperature of its own does not read (`fixed_source`), so a
/// Kelvin end is a blackbody's there, as on a Kelvin light (D10, L:99); the
/// knob LED is built at it. Everything else that shapes a light's colour and
/// level (tint, intensity, gels) stays the light's own, so a ramp is filtered
/// like the light itself. `light` is held by clamp(), and so is a ramp's end
/// (`stroke_of`); a tap's Kelvin, between two held ones, is in range but for
/// a rounding step, which `kelvin_to_rgb` and `led` hold to their own
/// domains, so the copy is coloured as it is.
fn recolored(light: &LightParams, kelvin: f32, rgb: Option<[f32; 3]>) -> Vec3f {
    let source = if fixed_source(light).is_some() { None } else { light.source.clone() };
    held_light_color(&LightParams { kelvin, rgb, source, ..light.clone() })
}

/// The colour along a stroke at RAMP_TAPS even taps, or None for one colour.
/// - `end_rgb` blends the light's colour to it in linear RGB.
/// - `end_kelvin` slides the temperature evenly in mired (1e6 / K), the scale
///   on which equal steps look equal (it is what gels are rated in). Every
///   tap is a luminance-1 colour at the light's level, so the brightness
///   holds along the whole ramp. A light with an explicit `rgb`, or a named
///   lamp with its own colour temperature, has no temperature to slide from,
///   so it blends from its colour to the end temperature's.
///
/// `light` is held by clamp() (`ShapedLight::new`), and so are the stroke's
/// ends (`stroke_of`), so the light and each tap's recoloured copy are
/// coloured as they are (`held_light_color`, `recolored`).
fn stroke_ramp(light: &LightParams, stroke: &StrokeParams) -> Option<Vec<Vec3f>> {
    let start = held_light_color(light);
    let at = |i: usize| i as f32 / (RAMP_TAPS - 1) as f32;
    let blend_to = |end: Vec3f| (0..RAMP_TAPS).map(|i| start + (end - start) * at(i)).collect::<Vec<Vec3f>>();
    if let Some(end) = stroke.end_rgb {
        return Some(blend_to(recolored(light, light.kelvin, Some(end))));
    }
    // Held to the light Kelvin range by `stroke_of`.
    let end_kelvin = stroke.end_kelvin?;
    if light.rgb.is_some() || fixed_source(light).is_some() {
        return Some(blend_to(recolored(light, end_kelvin, None)));
    }
    // The held Kelvin, the one light_color reads.
    let start_kelvin = light.kelvin;
    let (m0, m1) = (1.0e6 / start_kelvin, 1.0e6 / end_kelvin);
    Some((0..RAMP_TAPS).map(|i| recolored(light, 1.0e6 / (m0 + (m1 - m0) * at(i)), None)).collect())
}

/// `x` in -180..=180 degrees.
fn wrap180(x: f32) -> f32 {
    x - 360.0 * (x / 360.0).round()
}

/// The length of a straight (azimuth, elevation) step in degrees of arc,
/// azimuth shrunk by cos(elevation) the way the sphere shrinks it.
fn piece_len(a: [f32; 2], b: [f32; 2]) -> f32 {
    let el_mid = (0.5 * (a[1] + b[1])).max(-90.0).min(90.0).to_radians();
    ((b[0] - a[0]) * el_mid.cos()).hypot(b[1] - a[1])
}

/// The stroke's centre line as unit directions: the spline through its
/// (azimuth, elevation) points, flattened, then cut into pieces of at most
/// MAX_PIECE_DEG. Empty with fewer than two points. `s` is held to its
/// ranges (`stroke_of`): its points are finite, at most `POINT_LIMIT`, with
/// elevations inside -90..=90.
fn stroke_centre_line(s: &StrokeParams) -> Vec<Vec3f> {
    // Azimuths unwrapped along the list: each point takes the turn nearest the
    // previous one, so 350 then 10 becomes 350 then 370.
    let mut pts: Vec<[f32; 2]> = Vec::with_capacity(s.points.len() + 1);
    for p in &s.points {
        let az = match pts.last() {
            Some(last) => last[0] + wrap180(p[0] - last[0]),
            None => p[0],
        };
        pts.push([az, p[1]]);
    }
    if pts.len() < 2 {
        return Vec::new();
    }
    if s.closed && pts.len() >= 3 {
        // Back to the first point, unwrapped from the last. When the loop does
        // not wrap this lands on the first point (tween closes a path whose
        // last point is within 1e-3 of its first) and `through` makes a
        // smooth closed curve.
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        pts.push([last[0] + wrap180(first[0] - last[0]), first[1]]);
    }
    let longest = pts.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).fold(0.0f32, f32::max);
    let steps = ((longest / 2.0).ceil() as u32).clamp(4, 32);
    let curve = match flatten_through(&pts, s.curviness, steps) {
        Ok(curve) if curve.len() >= 2 => curve,
        _ => return Vec::new(),
    };
    let total: f32 = curve.windows(2).map(|w| piece_len(w[0], w[1])).sum();
    let piece = MAX_PIECE_DEG.max(total / MAX_LINE_POINTS);
    let mut out = Vec::with_capacity((total / piece) as usize + curve.len() + 1);
    out.push(dir_from_az_el(curve[0][0], curve[0][1].max(-90.0).min(90.0)));
    for w in curve.windows(2) {
        let (a, b) = (w[0], w[1]);
        let n = ((piece_len(a, b) / piece).ceil() as usize).max(1);
        for k in 1..=n {
            let f = k as f32 / n as f32;
            let az = a[0] + (b[0] - a[0]) * f;
            let el = (a[1] + (b[1] - a[1]) * f).max(-90.0).min(90.0);
            out.push(dir_from_az_el(az, el));
        }
    }
    out
}

/// A scrim: a rounded rect of cloth, glowing where the source behind it
/// shines through, with an optional dark frame.
#[derive(Clone, Debug)]
struct ScrimLight {
    frame: LightFrame,
    /// Half extents tan(w/2), tan(h/2).
    half: Vec2f,
    /// The shorter of them: the depth the Softness (and a picture's
    /// Penumbra) takes, as a rect's.
    short: f32,
    /// Corner radius, tangent units (studio.rs's rect rounding).
    round: f32,
    /// Half the soft band, tangent units.
    edge: f32,
    /// The frame's width, tangent units.
    frame_w: f32,
    /// Half the band of the frame's inner edge, where the cloth starts.
    inner_edge: f32,
    /// 1 / (2 sigma²), sigma = source_size in unit-box units.
    inv_two_sigma2: f32,
}

impl ScrimLight {
    /// `light` is held by clamp() (`ShapedLight::new`), its scrim block too,
    /// so its ranges and defaults are written once (L:71).
    fn new(light: &LightParams) -> ScrimLight {
        let scrim = light.scrim.clone().unwrap_or_default();
        let (hx, hy) = half_extents(light);
        let short = hx.min(hy);
        let sigma = scrim.source_size;
        let frame_w = scrim.frame * short;
        let inv_two_sigma2 = 1.0 / (2.0 * sigma * sigma);
        // A rect's inside depth is its shorter half extent, as in studio.rs.
        let edge = 0.5 * (light.softness * short).max(EDGE_MIN);
        // D9 (L:70): the cloth meets a rigid frame, so the frame's inner edge
        // is a quarter of its width, never softer than the outline: a frame
        // thinner than the outline's soft band (0.1 of the shorter half size
        // each way at the default softness) still reads as a border, dark
        // across its width with the cloth glowing right inside it, where the
        // outline's own band made it a dark gradient. Just past no frame the
        // band hands over from the outline's own, falling three times as fast
        // as the frame grows, so the Frame slider's first steps do not jump.
        let inner_edge = (0.25 * frame_w).max(edge - 3.0 * frame_w).min(edge).max(0.5 * EDGE_MIN);
        ScrimLight {
            frame: light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg),
            half: vec2f(hx, hy),
            short,
            round: light.corner * short,
            edge,
            frame_w,
            inner_edge,
            inv_two_sigma2,
        }
    }

    fn cull(&self) -> CullCone {
        // The corner of the box is the farthest point from the centre.
        CullCone::new(self.frame.center, (self.half.x.hypot(self.half.y) + self.edge).atan())
    }

    /// Mask and emission at `dir`. `inset` widens both of its edges inward
    /// by that share of the depth: the outline's, where the cloth ends, and
    /// the frame's inner one, where it starts to glow (a picture's Penumbra
    /// fades the whole light, as its alpha does).
    fn sample(&self, dir: Vec3f, inset: f32) -> ShapeSample {
        let Some(t) = project(&self.frame, dir) else {
            return ShapeSample::default();
        };
        // iq's rounded box, as studio.rs draws a rect.
        let qx = t.x.abs() - self.half.x + self.round;
        let qy = t.y.abs() - self.half.y + self.round;
        let (ox, oy) = (qx.max(0.0), qy.max(0.0));
        let d = (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - self.round;
        let inset = inset * self.short;
        let mask = 1.0 - smoothstep(-self.edge - inset, self.edge, d);
        if !(mask > 0.0) {
            return ShapeSample::default();
        }
        // The cloth inside the frame glows; the frame itself is dark.
        let cloth = 1.0 - smoothstep(-self.inner_edge - inset, self.inner_edge, d + self.frame_w);
        let ux = t.x / self.half.x;
        let uy = t.y / self.half.y;
        let glow = (-(ux * ux + uy * uy) * self.inv_two_sigma2).exp();
        ShapeSample { mask, emit: cloth * glow }
    }
}

/// The sign that turns the frame's `right` into the box's +x, which points to
/// increasing azimuth: the viewer's right on the panorama. A3 builds its frame
/// like ibl's softbox (`right = cross(+Y, center)`), and that vector points
/// the other way, so the sign is measured from a reference light on the
/// horizon rather than assumed: whichever way A3 turns `right`, a drawn point
/// at x = +1 lands east of a light facing north. Every frame `light_frame`
/// builds turns its `right` the same way, rolled or not, so this is also the
/// sign that turns any light's `right` into the viewer's right
/// (`light_fx::face_axes`).
pub fn box_x_sign() -> f32 {
    // A light due north on the horizon: increasing azimuth is +X there.
    if light_frame(0.0, 0.0, 0.0).right.x >= 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// The light's half extents in its tangent plane: tan(width/2), tan(height/2).
/// Of a light as clamp() holds it, read as it is (D17): the builders that
/// hold a light once (`ShapedLight::new`, `LightFx::new`, `Studio`'s
/// prepared light) and the panel, whose lights clamp() holds after every
/// edit, pass it so; this, `unit_to_dir`, `dir_to_unit`, `stroke_box` and
/// `light_fx::face_axes` hold nothing of their own.
pub fn half_extents(light: &LightParams) -> (f32, f32) {
    let half = |deg: f32| (0.5 * deg).to_radians().tan();
    (half(light.width_deg), half(light.height_deg))
}

/// A point of the light's unit box (-1..1, y up, x toward increasing
/// azimuth) as a direction on the sky, of a light as clamp() holds it
/// (`half_extents`).
pub fn unit_to_dir(light: &LightParams, unit: [f32; 2]) -> Vec3f {
    let f = light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg);
    let (hx, hy) = half_extents(light);
    (f.center + f.right * (box_x_sign() * unit[0] * hx) + f.up * (unit[1] * hy)).normalize()
}

/// The inverse of `unit_to_dir`, held inside the light's box: a direction off
/// the box comes back on the edge nearest to it. Behind the light's tangent
/// plane there is no gnomonic point, so such a direction is first pushed far
/// out along its bearing. Of a light as clamp() holds it (`half_extents`).
pub fn dir_to_unit(light: &LightParams, dir: Vec3f) -> [f32; 2] {
    let f = light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg);
    let (hx, hy) = half_extents(light);
    let c = dir.dot(f.center);
    // The box's +x runs toward increasing azimuth; the frame's `right` may
    // point the other way (box_x_sign), exactly as unit_to_dir applies it.
    let (x, y) = (box_x_sign() * dir.dot(f.right), dir.dot(f.up));
    let (tx, ty) = if c > 1.0e-3 {
        (x / c, y / c)
    } else {
        let l = (x * x + y * y).sqrt().max(1.0e-9);
        (x / l * 1.0e3, y / l * 1.0e3)
    };
    let hold = |v: f32| if v.is_finite() { v.max(-1.0).min(1.0) } else { 0.0 };
    [hold(tx / hx), hold(ty / hy)]
}

/// The polygon's corners in the unit box, before rounding: the explicit
/// points when there are at least three, else the regular polygon or star,
/// counter-clockwise from the corner straight up. The block is read as
/// clamp() holds it (`PolygonParams::clamped`): finite points inside the box,
/// 3..=64 sides, a star of at most 0.95.
pub fn polygon_unit_points(p: &PolygonParams) -> Vec<[f32; 2]> {
    let p = p.clamped();
    if p.points.len() >= 3 {
        return p.points;
    }
    let n = p.sides as usize;
    let star = p.star;
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

    /// D10 (L:26): a polygon flag takes its hotspot in as A3's flags do (their
    /// coverage is the soft edge times exp(-4 hotspot r²)), so a square
    /// polygon flag and a rect flag with the same settings pass the same light
    /// everywhere, and Hotspot fades a polygon gel toward its edge.
    #[test]
    fn a_polygon_flag_honours_its_hotspot() {
        let square = PolygonParams { sides: 4, star: 0.0, points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]] };
        let flag = LightParams { blend: "multiply".to_string(), rgb: Some([0.0; 3]), intensity_ev: 0.0, hotspot: 0.5, polygon: Some(square), ..polygon_light() };
        let rect = LightParams { shape: "rect".to_string(), polygon: None, ..flag.clone() };
        let over_white = |light: &LightParams| {
            let mut p = dark_studio(vec![light.clone()]);
            p.studio.top = [1.0; 3];
            p.studio.horizon = [1.0; 3];
            p.studio.floor = [1.0; 3];
            Env::new(&p)
        };
        let (polygon, rect) = (over_white(&flag), over_white(&rect));
        for u in [[0.0, 0.0], [0.5, 0.0], [0.8, 0.3], [-0.6, -0.7], [0.95, 0.0], [0.0, 0.9]] {
            let d = unit_to_dir(&flag, u);
            let (a, b) = (polygon.radiance(d), rect.radiance(d));
            assert!((a - b).abs().max_elem() < 1.0e-4, "{u:?}: the polygon flag passes {a:?}, the rect flag {b:?}");
        }
        // The middle is blacked out; at 0.8 of the way to the edge a hotspot of
        // 0.5 (exp(-2 r²)) covers e^-1.28 and leaves the rest of what is behind.
        assert!(polygon.radiance(unit_to_dir(&flag, [0.0, 0.0])).max_elem() < 1.0e-6);
        let edge = polygon.radiance(unit_to_dir(&flag, [0.8, 0.0]));
        assert!((edge.x - (1.0 - (-1.28f32).exp())).abs() < 1.0e-3, "{edge:?}");
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

    /// P1: a polygon whose soft edge a picture's Penumbra widens inward
    /// (`with_inset`) ramps over its true signed distance as deep as that
    /// ramp starts. The drawing's distance is capped at its reach, which for
    /// the bare shape is half its band: read through that cap, a widened
    /// polygon's whole inside was one value short of the ramp's start, so
    /// the light was dimmed flat, or black at Softness 0. A rounded star, so
    /// the bins are used and the outline is not convex, against the exact
    /// distance by brute force. Its bare sample (a window's glass,
    /// `PreparedLight::cover`) stays the bare shape's, bit for bit.
    #[test]
    fn a_widened_polygon_ramps_over_its_true_distance() {
        for softness in [0.0f32, 0.3] {
            let light = LightParams {
                softness,
                corner: 0.3,
                polygon: Some(PolygonParams { sides: 5, star: 0.5, points: Vec::new() }),
                ..polygon_light()
            };
            let bare = ShapedLight::new(&light).expect("the light builds");
            let ShapedKind::Fill(fill) = &bare.kind else { panic!("a polygon is a fill") };
            let shape = fill.shape.clone();
            assert!(shape.outline().segment_count() > GRID_MIN_SEGMENTS, "the rounded star uses the bins: {} segments", shape.outline().segment_count());
            let (frame, edge, depth) = (fill.frame, fill.edge, fill.depth);
            for inset in [0.3f32, 0.6, 1.0] {
                let widened = bare.clone().with_inset(inset);
                let mut full = 0;
                for j in 0..=80 {
                    for i in 0..=80 {
                        let unit = [-1.1 + 2.2 * i as f32 / 80.0, -1.1 + 2.2 * j as f32 / 80.0];
                        let dir = unit_to_dir(&light, unit);
                        let t = project(&frame, dir).expect("in front of the light");
                        let exact = shape.outline().exact_distance(t);
                        let sdf = if shape.winding_brute(t) != 0 { -exact } else { exact };
                        let want = 1.0 - smoothstep(-edge - inset * depth, edge, sdf);
                        let got = widened.compose(dir, Vec3f::default(), Vec3f::all(1.0)).x;
                        assert!(
                            (got - want).abs() < 1.0e-5,
                            "softness {softness}, inset {inset}, at {unit:?} (sdf {sdf}): {got}, the ramp over the true distance says {want}"
                        );
                        assert_eq!(widened.sample(dir), bare.sample(dir), "softness {softness}, inset {inset}, at {unit:?}: the bare sample moved");
                        full += (got == 1.0) as usize;
                    }
                }
                // Deeper than the ramp's start the light is whole (the
                // star's middle, at the smaller insets).
                assert!(inset == 1.0 || full > 0, "softness {softness}, inset {inset}: nothing is fully lit");
            }
        }
    }

    /// A hard 4°-wide straight stroke (curviness 0) through `points`.
    fn stroke_light(points: Vec<[f32; 2]>) -> LightParams {
        LightParams {
            shape: "stroke".to_string(),
            softness: 0.0,
            hotspot: 0.0,
            stroke: Some(StrokeParams { points, curviness: 0.0, width_deg: 4.0, ..StrokeParams::default() }),
            ..LightParams::default()
        }
    }

    fn stroke_at(light: &LightParams, azimuth_deg: f32, elevation_deg: f32) -> ShapeSample {
        ShapedLight::new(light).expect("the stroke builds").sample(dir_from_az_el(azimuth_deg, elevation_deg))
    }

    #[test]
    fn a_stroke_is_lit_along_its_line_and_dark_beside_it() {
        let light = stroke_light(vec![[80.0, 0.0], [100.0, 0.0]]);
        assert_eq!(stroke_at(&light, 90.0, 0.0).mask, 1.0);
        assert_eq!(stroke_at(&light, 90.0, 1.5).mask, 1.0, "inside the 4° width");
        assert_eq!(stroke_at(&light, 90.0, 3.0).mask, 0.0, "beside it");
        assert_eq!(stroke_at(&light, 70.0, 0.0).mask, 0.0, "past its end");
        let env = Env::new(&dark_studio(vec![light.clone()]));
        let lit = env.radiance(dir_from_az_el(90.0, 0.0));
        assert!((lit - light_color(&light)).length() < 1.0e-4 * lit.length(), "the core is the light's colour: {lit:?}");
    }

    #[test]
    fn a_stroke_across_north_takes_the_short_way_and_stays_continuous() {
        let mut light = stroke_light(vec![[350.0, 20.0], [10.0, 20.0]]);
        assert_eq!(stroke_at(&light, 0.0, 20.0).mask, 1.0);
        assert_eq!(stroke_at(&light, 359.5, 20.0).mask, 1.0);
        assert_eq!(stroke_at(&light, 0.5, 20.0).mask, 1.0);
        assert_eq!(stroke_at(&light, 180.0, 20.0).mask, 0.0, "it must not go the long way round");
        // With a soft edge, a point part way down the edge reads the same on both sides of north.
        light.softness = 1.0;
        let (west, east) = (stroke_at(&light, 359.9, 21.9).mask, stroke_at(&light, 0.1, 21.9).mask);
        assert!(west > 0.05 && west < 0.95, "{west}");
        assert!((west - east).abs() < 0.02, "{west} vs {east}");
    }

    #[test]
    fn the_profile_tapers_width_and_brightness_along_the_stroke() {
        let mut light = stroke_light(vec![[60.0, 0.0], [120.0, 0.0]]);
        if let Some(stroke) = light.stroke.as_mut() {
            stroke.width_deg = 6.0;
            stroke.profile = vec![1.0, 0.0];
        }
        let start = stroke_at(&light, 65.0, 0.0).emit;
        let end = stroke_at(&light, 115.0, 0.0).emit;
        assert!(start > 0.85 && end > 0.0 && end < 0.12, "{start} {end}");
        assert_eq!(stroke_at(&light, 65.0, 1.0).mask, 1.0, "wide near the start");
        assert_eq!(stroke_at(&light, 115.0, 1.0).mask, 0.0, "thin near the end");
        // The same lookup the panel's three width sliders read.
        assert_eq!(profile_at(&[], 0.3), 1.0);
        assert_eq!(profile_at(&[2.0, 1.0, 1.0, 1.0, 0.0], 0.5), 1.0);
        assert_eq!(profile_at(&[2.0, 1.0, 1.0, 1.0, 0.0], 1.0), 0.0);
    }

    #[test]
    fn a_closed_stroke_returns_to_its_first_point() {
        let mut light = stroke_light(vec![[80.0, 0.0], [100.0, 0.0], [90.0, 15.0]]);
        assert_eq!(stroke_at(&light, 85.0, 7.5).mask, 0.0, "open: no side back to the start");
        if let Some(stroke) = light.stroke.as_mut() {
            stroke.closed = true;
        }
        assert_eq!(stroke_at(&light, 85.0, 7.5).mask, 1.0, "closed: the side from the last point back to the first");
    }

    #[test]
    fn a_stroke_without_points_draws_the_arch_over_the_light() {
        let mut light = LightParams { shape: "stroke".to_string(), azimuth_deg: 90.0, elevation_deg: 10.0, ..LightParams::default() };
        let middle = StrokeParams::around(90.0, 10.0, light.width_deg).points[1];
        assert_eq!(stroke_at(&light, middle[0], middle[1]).mask, 1.0);
        // A block that only sets a width has no points either: the arch stays,
        // with that width, and follows the light when it moves.
        light.stroke = Some(StrokeParams { width_deg: 6.0, ..StrokeParams::default() });
        assert_eq!(stroke_of(&light).points, StrokeParams::around(90.0, 10.0, light.width_deg).points);
        assert_eq!(stroke_at(&light, middle[0], middle[1] + 2.5).mask, 1.0, "inside the 6° width");
        light.azimuth_deg = 200.0;
        assert_eq!(stroke_at(&light, middle[0], middle[1]).mask, 0.0, "the arch left with the light");
        assert_eq!(stroke_at(&light, middle[0] + 110.0, middle[1]).mask, 1.0);
        // Two points or more place the stroke themselves.
        light.stroke = Some(StrokeParams { points: vec![[10.0, 0.0], [30.0, 0.0]], ..StrokeParams::default() });
        assert_eq!(stroke_of(&light).points, vec![[10.0, 0.0], [30.0, 0.0]]);
    }

    #[test]
    fn a_kelvin_ramp_slides_the_colour_along_the_stroke() {
        let mut light = stroke_light(vec![[60.0, 0.0], [120.0, 0.0]]);
        light.kelvin = 2700.0;
        if let Some(stroke) = light.stroke.as_mut() {
            stroke.end_kelvin = Some(9000.0);
        }
        let env = Env::new(&dark_studio(vec![light.clone()]));
        let at = |az: f32| env.radiance(dir_from_az_el(az, 0.0));
        let warm = light_color(&light);
        let cool = light_color(&LightParams { kelvin: 9000.0, ..light.clone() });
        // Read half a degree inside each end: the ends themselves are hard caps.
        assert!((at(60.5) - warm).length() < 0.02 * warm.length(), "{:?} vs {warm:?}", at(60.5));
        assert!((at(119.5) - cool).length() < 0.02 * cool.length(), "{:?} vs {cool:?}", at(119.5));
        assert!(at(61.0).x > at(61.0).z && at(119.0).z > at(119.0).x, "warm at the start, cool at the end");
        // Half way along is half way in mired, not in kelvin.
        let middle_kelvin = 2.0e6 / (1.0e6 / 2700.0 + 1.0e6 / 9000.0);
        let middle = light_color(&LightParams { kelvin: middle_kelvin, ..light.clone() });
        assert!((at(90.0) - middle).length() < 0.01 * middle.length(), "{:?} vs {middle:?}", at(90.0));
        // A Kelvin ramp changes the hue, never the brightness.
        let level = crate::sky::luminance(warm);
        for az in [61.0, 75.0, 90.0, 105.0, 119.0] {
            let l = crate::sky::luminance(at(az));
            assert!((l - level).abs() < 1.0e-3 * level, "luminance {l} at {az}, {level} at the start");
        }
    }

    /// D10 (L:99): End Kelvin works on a named lamp. A lamp has its own colour
    /// temperature (only the knob LED is built at the light's Kelvin), so, as
    /// an explicit colour does, it blends from its own colour to the end
    /// temperature's in linear RGB; and an RGB light that keeps a named source
    /// under its colour ramps to the end temperature, not to that source.
    #[test]
    fn end_kelvin_works_on_a_named_lamp() {
        let mut lamp = stroke_light(vec![[60.0, 0.0], [120.0, 0.0]]);
        lamp.source = Some("sodium_low_pressure".to_string());
        if let Some(stroke) = lamp.stroke.as_mut() {
            stroke.end_kelvin = Some(9000.0);
        }
        let along = |light: &LightParams| {
            let env = Env::new(&dark_studio(vec![light.clone()]));
            move |az: f32| env.radiance(dir_from_az_el(az, 0.0))
        };
        let at = along(&lamp);
        let start = light_color(&lamp);
        let end = light_color(&LightParams { source: None, kelvin: 9000.0, ..lamp.clone() });
        assert!(start.x > 2.0 * start.y && end.z > end.x, "sodium is orange, 9000 K blue-white: {start:?} {end:?}");
        // Half a degree inside each end: the ends themselves are hard caps.
        assert!((at(60.5) - start).length() < 0.02 * start.length(), "{:?} vs {start:?}", at(60.5));
        assert!((at(119.5) - end).length() < 0.02 * end.length(), "{:?} vs {end:?}", at(119.5));
        let middle = (start + end) * 0.5;
        assert!((at(90.0) - middle).length() < 0.01 * middle.length(), "{:?} vs {middle:?}", at(90.0));

        // B12's case: daylight D65 switched to RGB and set to blue, ending at
        // 2000 K, ends amber, not at the D65 white still kept under the colour.
        let mut blue = lamp.clone();
        blue.source = Some("daylight_d65".to_string());
        blue.rgb = Some([0.0, 0.0, 1.0]);
        if let Some(stroke) = blue.stroke.as_mut() {
            stroke.end_kelvin = Some(2000.0);
        }
        let amber = light_color(&LightParams { rgb: None, source: None, kelvin: 2000.0, ..blue.clone() });
        let got = along(&blue)(119.5);
        assert!((got - amber).length() < 0.02 * amber.length(), "{got:?} vs {amber:?}");

        // The knob LED is built at the Kelvin, so its ramp slides the LED's own
        // temperature, in mired, as a Kelvin light's does.
        let mut knob = lamp.clone();
        knob.source = Some(crate::hdri::spectrum::LED_KNOB.to_string());
        knob.kelvin = 3000.0;
        let led_end = light_color(&LightParams { kelvin: 9000.0, ..knob.clone() });
        let led_middle = light_color(&LightParams { kelvin: 2.0e6 / (1.0e6 / 3000.0 + 1.0e6 / 9000.0), ..knob.clone() });
        let at = along(&knob);
        assert!((at(119.5) - led_end).length() < 0.02 * led_end.length(), "{:?} vs {led_end:?}", at(119.5));
        assert!((at(90.0) - led_middle).length() < 0.01 * led_middle.length(), "{:?} vs {led_middle:?}", at(90.0));
    }

    #[test]
    fn a_colour_ramp_blends_to_the_end_colour_and_wins_over_kelvin() {
        let mut light = stroke_light(vec![[60.0, 0.0], [120.0, 0.0]]);
        light.rgb = Some([1.0, 0.0, 0.0]);
        light.intensity_ev = 1.0;
        if let Some(stroke) = light.stroke.as_mut() {
            stroke.end_rgb = Some([0.0, 0.0, 1.0]);
            stroke.end_kelvin = Some(3000.0);
        }
        let shaped = ShapedLight::new(&light).unwrap();
        // A quarter along: three parts red and one part blue, at the light's level (2^1).
        let got = shaped.compose(dir_from_az_el(75.0, 0.0), Vec3f::default(), light_color(&light));
        assert!((got - vec3f(1.5, 0.0, 0.5)).length() < 1.0e-3, "{got:?}");
        // A Multiply stroke scales what is behind it by the ramp's colour there.
        light.blend = "multiply".to_string();
        light.intensity_ev = 0.0;
        let flag = ShapedLight::new(&light).unwrap();
        let behind = flag.compose(dir_from_az_el(105.0, 0.0), Vec3f::all(1.0), light_color(&light));
        assert!((behind - vec3f(0.25, 0.0, 0.75)).length() < 1.0e-3, "{behind:?}");
    }

    #[test]
    fn a_key_stroke_gives_the_engine_its_middle_and_its_share_of_the_cone() {
        let mut light = stroke_light(vec![[60.0, 0.0], [120.0, 0.0]]);
        light.key = true;
        let studio = super::super::studio::Studio::new(&dark_studio(Vec::new()).studio, &[light.clone()]);
        let sun = studio.key().expect("the stroke is the key light");
        assert!(sun.dir.dot(dir_from_az_el(90.0, 0.0)) > 0.999, "{:?}", sun.dir);
        // 30° to either end plus the 2° half width.
        assert!(sun.cos_radius < 31.5f32.to_radians().cos() && sun.cos_radius > 32.5f32.to_radians().cos(), "{}", sun.cos_radius);
        // The engine lights with radiance x the cone's solid angle. A 60° x 4° stroke
        // covers 2 * 60° * sin(2°) = 0.0731 sr, and its two round ends together a
        // 2° disc, 2π (1 - cos 2°) = 0.0038 sr: 0.0769 sr of the 0.955 sr cone, 8.05 %.
        // studio.rs measures it from the stroke's drawing, jittered along its hard
        // edge (D1), to within a few tenths of a per cent.
        let fill = crate::sky::luminance(sun.radiance) / crate::sky::luminance(light_color(&light));
        assert!((fill - 0.0805).abs() < 0.001, "{fill}");
        // Plan 2's other two fields come from the same integral and the shape core:
        // the 60° arc faces its axis with the mean cosine over ±30°, sin 30° / (π/6)
        // = 0.955, its ends with cos 30°, together 0.950 (the map's, baked at 4K:
        // 0.9500), and its covering cone reaches past the key cone by its edge and
        // half a piece.
        assert!(sun.validate().is_ok(), "{sun:?}");
        assert!((sun.facing - 0.950).abs() < 0.002, "{}", sun.facing);
        assert!(sun.cos_cover < sun.cos_radius && sun.cos_cover > 33.5f32.to_radians().cos(), "{} vs {}", sun.cos_cover, sun.cos_radius);
    }

    #[test]
    fn a_direction_maps_back_into_the_light_box() {
        let mut light = polygon_light();
        light.roll_deg = 20.0;
        light.width_deg = 60.0;
        for u in [[0.0, 0.0], [0.5, -0.25], [-0.9, 0.9]] {
            let back = dir_to_unit(&light, unit_to_dir(&light, u));
            assert!((back[0] - u[0]).abs() < 1.0e-4 && (back[1] - u[1]).abs() < 1.0e-4, "{u:?} -> {back:?}");
        }
        // Far to the east of a light facing north, even behind its tangent plane,
        // the corner holds on the box's east edge: x = +1, whichever way the
        // frame's `right` points.
        let side = LightParams { azimuth_deg: 0.0, elevation_deg: 30.0, ..polygon_light() };
        assert_eq!(dir_to_unit(&side, dir_from_az_el(90.0, 0.0))[0], 1.0);
        assert_eq!(dir_to_unit(&side, dir_from_az_el(270.0, 0.0))[0], -1.0);
    }

    /// A hard 40x40° scrim due east with a small source and a frame a tenth of its half size.
    fn scrim_light() -> LightParams {
        LightParams {
            shape: "scrim".to_string(),
            polygon: None,
            scrim: Some(ScrimParams { source_size: 0.3, frame: 0.1 }),
            ..polygon_light()
        }
    }

    #[test]
    fn a_scrim_glows_from_its_middle_and_hides_what_is_behind() {
        let light = scrim_light();
        let color = light_color(&light);
        let mut p = dark_studio(vec![light.clone()]);
        p.studio.top = [2.0; 3];
        p.studio.horizon = [2.0; 3];
        p.studio.floor = [2.0; 3];
        let env = Env::new(&p);
        let centre = env.radiance(unit_to_dir(&light, [0.0, 0.0]));
        assert!((centre - color).length() < 1.0e-3 * color.length(), "the cloth hides the backdrop and peaks at the colour: {centre:?}");
        let edge = env.radiance(unit_to_dir(&light, [0.8, 0.0]));
        assert!(edge.x < 0.1 * centre.x, "a small source leaves the edge dim: {edge:?}");
        let frame = env.radiance(unit_to_dir(&light, [0.97, 0.0]));
        assert!(frame.length() < 1.0e-4, "the frame is dark and hides the backdrop: {frame:?}");
        let outside = env.radiance(unit_to_dir(&light, [1.3, 0.0]));
        assert!((outside - vec3f(2.0, 2.0, 2.0)).length() < 1.0e-3, "{outside:?}");
    }

    /// D9 (L:70): a thin frame reads as a frame at the default softness: dark
    /// across its width, the cloth glowing right inside it, not a dark
    /// gradient from the outline's soft band (whose half, 0.1 of the shorter
    /// half size at softness 0.2, is as wide as a 0.1 frame). The first steps
    /// of the Frame slider still hand over smoothly from no frame.
    #[test]
    fn a_thin_scrim_frame_reads_as_a_frame() {
        let with_frame = |frame: f32| LightParams { softness: 0.2, scrim: Some(ScrimParams { source_size: 1.0, frame }), ..scrim_light() };
        // How much of the glow shows at unit-box x (y = 0): the cloth, emit over glow.
        let cloth = |light: &LightParams, x: f32| {
            let s = ShapedLight::new(light).unwrap().sample(unit_to_dir(light, [x, 0.0]));
            s.emit / (-0.5 * x * x).exp()
        };
        for frame in [0.05f32, 0.1] {
            let light = with_frame(frame);
            // The square's half size is its shorter one: the frame is `frame` unit-box units wide.
            let (middle, inside) = (cloth(&light, 1.0 - 0.5 * frame), cloth(&light, 1.0 - 1.5 * frame));
            assert!(middle < 0.05, "frame {frame}: its middle shows {middle} of the glow");
            assert!(inside > 0.9, "frame {frame}: the cloth inside it shows {inside} of the glow");
            // The frame hides what is behind it across its inner half.
            let s = ShapedLight::new(&light).unwrap().sample(unit_to_dir(&light, [1.0 - 0.75 * frame, 0.0]));
            assert!(s.mask > 0.5, "frame {frame}: {}", s.mask);
        }
        // No frame and the narrowest one draw almost the same.
        let (none, hair) = (with_frame(0.0), with_frame(0.002));
        for i in 0..=100 {
            let x = 0.7 + 0.5 * i as f32 / 100.0;
            assert!((cloth(&none, x) - cloth(&hair, x)).abs() < 0.08, "at {x}: {} against {}", cloth(&none, x), cloth(&hair, x));
        }
    }

    #[test]
    fn a_larger_source_spreads_the_glow() {
        let mut tight = scrim_light();
        tight.scrim = Some(ScrimParams { source_size: 0.1, frame: 0.0 });
        let mut broad = tight.clone();
        broad.scrim = Some(ScrimParams { source_size: 1.0, frame: 0.0 });
        let at = |l: &LightParams| ShapedLight::new(l).unwrap().sample(unit_to_dir(l, [0.6, 0.0])).emit;
        assert!(at(&tight) < 1.0e-6, "{}", at(&tight));
        assert!((at(&broad) - (-0.18f32).exp()).abs() < 1.0e-3, "{}", at(&broad));
    }

    #[test]
    fn a_key_scrim_hands_the_engine_its_glow() {
        let key_of = |source_size: f32| {
            let mut light = scrim_light();
            light.key = true;
            light.scrim = Some(ScrimParams { source_size, frame: 0.0 });
            let sun = super::super::studio::Studio::new(&dark_studio(Vec::new()).studio, &[light.clone()])
                .key()
                .expect("the scrim is the key light");
            (sun, crate::sky::luminance(light_color(&light)))
        };
        let (even, level) = key_of(1.0);
        assert!((even.dir - dir_from_az_el(90.0, 0.0)).length() < 1.0e-5, "{:?}", even.dir);
        assert!((even.cos_radius - 20.0f32.to_radians().cos()).abs() < 1.0e-6, "a rect's cone: {}", even.cos_radius);
        // EnvSun's meaning: radiance x the cone's solid angle is the cloth's whole
        // glow. exp(-(u² + v²) / 2) over the 40° square, with u, v = x, y / tan 20°
        // and dΩ = dx dy / (1 + x² + y²)^(3/2), is 0.3485 sr (a 1000 x 1000 midpoint
        // sum); the 20° cone is 2π(1 - cos 20°) = 0.3789 sr; so 0.920 of the colour.
        let fill = crate::sky::luminance(even.radiance) / level;
        assert!((fill - 0.920).abs() < 0.01, "{fill}");
        // A tight source glows over far less of the cloth (0.0219 of the colour by
        // the same sum).
        let (tight, level) = key_of(0.1);
        assert!(crate::sky::luminance(tight.radiance) < 0.03 * level);
    }
}
