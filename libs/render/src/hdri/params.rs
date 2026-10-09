//! Parameters of the procedural HDRI generator.
//!
//! One serialisable struct is the in-app state, the preset file format and
//! the AI tool format at once. Every field has a documented range, and
//! [`HdriParams::clamp`] enforces the ranges and reports what it changed, so
//! a hand-edited file or an AI patch can never feed NaN or an unknown choice
//! into the model.
//!
//! micro-serde rules this file follows:
//! - There is no default attribute: a missing field fails to load unless its
//!   type is `Option<..>`. Every field added after phase 1a is therefore an
//!   `Option`, so files saved by 1a keep loading.
//! - Rust enums serialise as `{"Variant":[]}`, which is awkward to write by
//!   hand or from a model, so choices are lowercase strings with typed
//!   accessors (`HdriParams::mode`, `LightParams::shape`, ...).
//! - NaN and infinity serialise as `null` and then fail to load: clamp
//!   before saving.
//! - No `pub(crate)` on these structs or their fields; it breaks the derive.

use makepad_draw::makepad_micro_serde::*;
use std::collections::{HashMap, HashSet};

/// Most lights a map holds (16 since phase 1b).
pub const LIGHT_LIMIT: usize = 16;
/// The params format this build reads and writes; clamp() pins `version` to it.
pub const PARAMS_VERSION: u32 = 1;
/// Accepted values of `HdriParams::mode`; the first is where unknown strings fall back to.
pub const MODE_NAMES: &[&str] = &["sky", "studio"];
/// Accepted values of `SunParams::mode`; the first is the fallback.
pub const SUN_MODE_NAMES: &[&str] = &["time", "manual"];
/// Accepted values of `LightParams::shape`; the first is the fallback. Phase 1b
/// added polygon, stroke and scrim, which the shape core draws (hdri/shape.rs).
pub const LIGHT_SHAPE_NAMES: &[&str] = &["rect", "disc", "ring", "polygon", "stroke", "scrim"];
/// Accepted values of `LightParams::blend`; the first is the fallback.
pub const BLEND_NAMES: &[&str] = &["add", "multiply"];
/// Accepted values of `GoboParams::kind`; the first is the fallback.
pub const GOBO_KIND_NAMES: &[&str] = &["window", "blinds", "leaves", "fern", "grid", "image"];

/// Accepted values of `LightImageParams::fit`; the first is the fallback.
pub const IMAGE_FIT_NAMES: &[&str] = &["stretch", "fit", "fill"];
/// Longest image key kept, in characters: a path, with room to spare. A
/// light's picture (`LightImageParams::key`) and a gobo's (`GoboParams::image`).
const IMAGE_KEY_LIMIT: usize = 1024;

/// Longest light name kept, in characters; a list row has no room for more.
const LIGHT_NAME_LIMIT: usize = 64;

/// Everything that defines one environment map.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct HdriParams {
    /// Always PARAMS_VERSION (1).
    pub version: u32,
    /// "sky" | "studio": the base layer under the lights.
    pub mode: String,
    /// -10..=10. Scales the whole map by 2^intensity_ev.
    pub intensity_ev: f32,
    /// -180..=180, wrapped. Turns the content about +Y, positive
    /// counter-clockwise seen from above (the sign of `EnvMap::procedural`
    /// and the engine's `Ibl.rotation_deg`): content at azimuth a appears at
    /// a - rotation_deg, so +90 moves a light at north (-Z) to west (-X).
    pub rotation_deg: f32,
    /// Seed of every procedural layer (clouds, stars).
    pub seed: u32,
    pub sky: SkyParams,
    pub studio: StudioParams,
    /// At most LIGHT_LIMIT. An overlay over the base layer in both modes, evaluated in list order.
    pub lights: Vec<LightParams>,
}

/// The outdoor base layer.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct SkyParams {
    pub sun: SunParams,
    pub atmosphere: AtmosphereParams,
    pub sun_disc: SunDiscParams,
    pub clouds: CloudParams,
    pub night: NightParams,
}

/// Where the sun is: from a date, time and place (NOAA), or set by hand.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct SunParams {
    /// "time" (position from the date, hour and place) | "manual" (elevation_deg, azimuth_deg).
    pub mode: String,
    /// 1900..=2100, the range the NOAA formulas are good for.
    pub year: i32,
    /// 1..=12.
    pub month: u32,
    /// 1..=days in that month.
    pub day: u32,
    /// 0..=24, local clock time.
    pub hour: f32,
    /// -12..=14, hours east of UTC.
    pub tz_offset: f32,
    /// -90..=90, degrees north.
    pub latitude: f32,
    /// -180..=180, degrees east.
    pub longitude: f32,
    /// -90..=90 (manual mode).
    pub elevation_deg: f32,
    /// 0..360 clockwise from north, wrapped (manual mode).
    pub azimuth_deg: f32,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct AtmosphereParams {
    /// 0..=10, aerosol (Mie) density; 1 = a clear day.
    pub haze: f32,
    /// 0..=10, air (Rayleigh) density; 1 = Earth.
    pub air: f32,
    /// 0..=10, ozone absorption; 1 = Earth.
    pub ozone: f32,
    /// Linear 0..=1, the ground below the horizon.
    pub ground_color: [f32; 3],
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct SunDiscParams {
    /// 0.1..=20 degrees across (the real sun is 0.53). The disc keeps its energy as it grows.
    pub size_deg: f32,
    /// 0..=1, limb softness.
    pub softness: f32,
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct CloudParams {
    /// 0..=1; 0 means no cloud layer.
    pub coverage: f32,
    /// 0..=1, edge sharpness.
    pub sharpness: f32,
    /// 0.1..=10, feature size.
    pub scale: f32,
    /// 500..=12000 metres, the height of the cloud layer.
    pub altitude_m: f32,
    /// 0..=1, high wispy cloud.
    pub cirrus: f32,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct NightParams {
    /// 0..=1, star density.
    pub stars: f32,
    /// 0..=10.
    pub star_brightness: f32,
    pub moon: bool,
    /// -90..=90.
    pub moon_elevation_deg: f32,
    /// 0..360 clockwise from north, wrapped.
    pub moon_azimuth_deg: f32,
    /// 0.1..=10 degrees across (the real moon is 0.52).
    pub moon_size_deg: f32,
    /// 0..=10.
    pub moon_brightness: f32,
    /// Linear 0..=1, airglow colour.
    pub glow_color: [f32; 3],
    /// 0..=10.
    pub glow_strength: f32,
}

/// The studio base layer: a vertical gradient backdrop.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct StudioParams {
    /// Linear 0..=10, straight up.
    pub top: [f32; 3],
    /// Linear 0..=10, at the horizon.
    pub horizon: [f32; 3],
    /// Linear 0..=10, straight down.
    pub floor: [f32; 3],
    /// 0.01..=1, how far the horizon colour spreads.
    pub horizon_softness: f32,
}

/// One shaped light, placed on the sphere in its own angular frame.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct LightParams {
    pub enabled: bool,
    /// Up to 64 characters.
    pub name: String,
    /// "rect" | "disc" | "ring" (phase 1b adds more). A strip is a thin rect.
    pub shape: String,
    /// 0..=1: rounds a rect's or a polygon's corners.
    pub corner: f32,
    /// 0..=0.95 (ring only).
    pub inner: f32,
    /// 0..360 clockwise from north, wrapped.
    pub azimuth_deg: f32,
    /// -90..=90.
    pub elevation_deg: f32,
    /// 0.1..=170 degrees.
    pub width_deg: f32,
    /// 0.1..=170 degrees.
    pub height_deg: f32,
    /// -180..=180, wrapped.
    pub roll_deg: f32,
    /// 0..=1, edge falloff.
    pub softness: f32,
    /// 0..=1, centre falloff.
    pub hotspot: f32,
    /// -10..=20.
    pub intensity_ev: f32,
    /// 1000..=20000.
    pub kelvin: f32,
    /// -1..=1: negative is green, positive magenta.
    pub tint: f32,
    /// "add" | "multiply" (multiply darkens everything drawn before it: flags).
    pub blend: String,
    /// At most one light is the key; clamp() keeps the first.
    pub key: bool,
    /// Explicit linear colour, 0..=1 per channel; overrides kelvin and tint when Some.
    pub rgb: Option<[f32; 3]>,
    /// Shape "polygon" (1b): the outline. None draws the default hexagon.
    pub polygon: Option<PolygonParams>,
    /// Shape "stroke" (1b), a Lumi-curve. None, or a block with fewer than
    /// two points, draws the arch `StrokeParams::around` the light's place.
    pub stroke: Option<StrokeParams>,
    /// Shape "scrim" (1b): the source behind the cloth and the frame. None is ScrimParams::default().
    pub scrim: Option<ScrimParams>,
    /// Gels in front of the light (1b): up to `gels::GEL_LIMIT` names from
    /// `gels::GEL_NAMES`. None or an empty list is a bare light.
    pub gels: Option<Vec<String>>,
    /// A named light source from `spectrum::SOURCE_NAMES` (1b): its spectrum
    /// gives the colour, in place of `kelvin`. `tint` still applies; `rgb`
    /// still wins. None = the Kelvin blackbody. The source `led` is the
    /// parametric LED: it keeps `kelvin` and reads `source_quality`.
    pub source: Option<String>,
    /// The parametric LED's quality knob, 0..=1 (1b): 0 a cheap LED (CRI
    /// about 53), 0.5 about 80, 1 a full-spectrum one (about 95). Only the
    /// source `led` reads it. None = `spectrum::LED_QUALITY_DEFAULT`.
    pub source_quality: Option<f32>,
    /// A picture as the light's content (1b): see `LightImageParams`.
    pub image: Option<LightImageParams>,
    /// A pattern hung in front of the light (1b): a window, blinds, leaves,
    /// a fern, a grid of bars or an image mask. None: nothing.
    pub gobo: Option<GoboParams>,
    /// Which way the light's face points, and its grid and barn doors (1b). None: facing the centre, bare.
    pub aim: Option<AimParams>,
    /// The wear of the light's diffuser: wrinkles, dust and weave (1b). None: a clean face.
    pub imperfections: Option<ImperfectionParams>,
}

/// A picture shown by a light: it fills the light's box, is cut by the
/// light's shape, and is scaled to mean luminance 1 so the light's colour
/// and `intensity_ev` keep their meaning.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct LightImageParams {
    /// The picture's key in the `ImageSet` given to `Env::new_with` (the app
    /// uses the file's path), at most 1024 characters. "" or a key that is
    /// not loaded draws the plain light.
    pub key: String,
    /// "stretch" (fill the box, ignoring the picture's aspect) | "fit" (the
    /// whole picture inside the box) | "fill" (cover the box, cropping).
    pub fit: String,
    /// The picture's alpha cuts the light out; false ignores alpha.
    pub alpha_as_mask: bool,
}

impl Default for LightImageParams {
    fn default() -> Self {
        LightImageParams { key: String::new(), fit: "stretch".to_string(), alpha_as_mask: false }
    }
}

/// Typed view of `LightImageParams::fit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFit {
    Stretch,
    Fit,
    Fill,
}

impl ImageFit {
    pub fn as_str(self) -> &'static str {
        match self {
            ImageFit::Stretch => "stretch",
            ImageFit::Fit => "fit",
            ImageFit::Fill => "fill",
        }
    }
}

impl LightImageParams {
    pub fn fit(&self) -> ImageFit {
        if is_choice(&self.fit, "fit") {
            ImageFit::Fit
        } else if is_choice(&self.fit, "fill") {
            ImageFit::Fill
        } else {
            ImageFit::Stretch
        }
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        if self.key.chars().count() > IMAGE_KEY_LIMIT {
            self.key = self.key.chars().take(IMAGE_KEY_LIMIT).collect();
            f.push(prefix, "key");
        }
        f.choice(prefix, "fit", &mut self.fit, IMAGE_FIT_NAMES);
    }
}

/// Widest a polygon or a scrim may be, each way, in degrees. clamp() holds
/// them there and reports the change; the panel resyncs its Width and Height
/// sliders to it (A11 resyncs whenever clamp() changed something).
pub const SHAPED_SPAN_MAX_DEG: f32 = 120.0;

/// A polygon light's outline, in the light's unit box: x and y from -1 to 1,
/// y up and x toward increasing azimuth (the viewer's right on the panorama),
/// stretched to the light's width and height.
#[derive(Clone, Debug, PartialEq, DeJson)]
pub struct PolygonParams {
    /// 3..=64 corners of the regular polygon or star; its first corner points straight up.
    pub sides: u32,
    /// 0..=0.95. 0 is a regular polygon. Above 0 it becomes a star whose inner
    /// corners sit at (1 - star) of the polygon's own inner radius.
    pub star: f32,
    /// Up to 64 corners in the box, in order. Three or more replace the regular
    /// polygon; clamp() drops a list of fewer than three.
    pub points: Vec<[f32; 2]>,
}

impl Default for PolygonParams {
    fn default() -> Self {
        PolygonParams { sides: 6, star: 0.0, points: Vec::new() }
    }
}

/// Written by hand, as the derive would write it: micro-serde has SerJson for
/// slices (`impl<T> SerJson for [T]`) but not for fixed-size arrays, so a
/// derived `Vec<[f32; 2]>` field does not compile; DeJson for `[T; N]` exists.
impl SerJson for PolygonParams {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.st_pre();
        s.field(d + 1, "sides");
        self.sides.ser_json(d + 1, s);
        s.conl();
        s.field(d + 1, "star");
        self.star.ser_json(d + 1, s);
        s.conl();
        s.field(d + 1, "points");
        ser_points(&self.points, d + 1, s);
        s.st_post(d);
    }
}

/// A list of 2D points as a JSON array of [x, y] arrays, laid out like a Vec.
fn ser_points(points: &[[f32; 2]], d: usize, s: &mut SerJsonState) {
    s.out.push('[');
    if !points.is_empty() {
        let last = points.len() - 1;
        for (i, p) in points.iter().enumerate() {
            s.indent(d + 1);
            p[..].ser_json(d + 1, s);
            if i != last {
                s.out.push(',');
            }
        }
        s.indent(d);
    }
    s.out.push(']');
}

impl PolygonParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = PolygonParams::default();
        f.int(prefix, "sides", &mut self.sides, 3, 64);
        f.range(prefix, "star", &mut self.star, 0.0, 0.95, d.star);
        let points = unit_box_points(&self.points);
        if points != self.points {
            self.points = points;
            f.push(prefix, "points");
        }
    }
}

/// Finite points only, the first 64, each held inside the unit box. Fewer
/// than three make no outline, so then none are kept.
fn unit_box_points(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let kept: Vec<[f32; 2]> = points
        .iter()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .take(64)
        .map(|p| [p[0].clamp(-1.0, 1.0), p[1].clamp(-1.0, 1.0)])
        .collect();
    if kept.len() < 3 {
        Vec::new()
    } else {
        kept
    }
}

/// A pattern hung in front of a light (a gobo, or cookie), laid out in the
/// light's own rectangle: its azimuth, elevation, width, height and roll.
/// Fields a kind does not use are kept, so switching kinds loses nothing.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct GoboParams {
    /// "window" | "blinds" | "leaves" | "fern" | "grid" | "image".
    pub kind: String,
    /// 1..=16 panes across (window, grid).
    pub panes_x: u32,
    /// 1..=16 panes down (window, grid).
    pub panes_y: u32,
    /// 0..=0.5: the muntin bars' width, as a fraction of the smaller pane pitch.
    pub bar: f32,
    /// 0..=0.5: the window frame's width, as a fraction of the light's smaller half size.
    pub frame: f32,
    /// 2..=64 blind slats across the light.
    pub slats: u32,
    /// 0..=90 degrees: the slats' tilt from edge-on. 0 is open, 90 shut.
    pub tilt_deg: f32,
    /// 0.5..=1.5: a slat's width (its depth when open), in slat pitches. The
    /// pitch is the light's size over `slats`, so this also sets the gap:
    /// above 1 shut blinds overlap and pass nothing, below 1 a gap of
    /// (1 - slat_width) pitches stays open even when they are shut.
    pub slat_width: f32,
    /// Vertical slats; horizontal otherwise.
    pub vertical: bool,
    /// 0..=1: how much of the light leaves or fern fronds cover.
    pub density: f32,
    /// 0.1..=10: the size of leaves and fronds (1: a leaf about a sixth of the light's smaller side).
    pub scale: f32,
    /// Where leaves and fronds fall and how they turn.
    pub seed: u32,
    /// 0..=1: how far in front of the light the gobo hangs. 0 gives sharp
    /// edges, 1 a penumbra about an eighth of the light's smaller half size.
    pub distance: f32,
    /// Image gobo: the name of an image the app has loaded; "" is none.
    pub image: String,
    /// The light is a window: it shows the map's own sky through the gobo
    /// instead of its colour. At intensity_ev 0 the sky shows as it is outside.
    pub fill_sky: bool,
    /// Studio mode: the gobo's shadow also falls on the backdrop, on the
    /// floor under the light (`gobo::BackdropGobo`).
    pub project_on_backdrop: bool,
}

impl Default for GoboParams {
    fn default() -> Self {
        GoboParams {
            kind: "window".to_string(),
            panes_x: 3,
            panes_y: 2,
            bar: 0.06,
            frame: 0.08,
            slats: 12,
            tilt_deg: 30.0,
            slat_width: 1.08,
            vertical: false,
            density: 0.5,
            scale: 1.0,
            seed: 1,
            distance: 0.1,
            image: String::new(),
            fill_sky: false,
            project_on_backdrop: false,
        }
    }
}

/// Typed view of `GoboParams::kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoboKind {
    /// A frame and muntin bars between panes.
    Window,
    /// Venetian blinds.
    Blinds,
    /// Scattered leaves (foliage.rs).
    Leaves,
    /// Fern fronds (foliage.rs).
    Fern,
    /// Bars between panes, without a frame.
    Grid,
    /// An image mask from the app's ImageSet.
    Image,
}

impl GoboKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GoboKind::Window => "window",
            GoboKind::Blinds => "blinds",
            GoboKind::Leaves => "leaves",
            GoboKind::Fern => "fern",
            GoboKind::Grid => "grid",
            GoboKind::Image => "image",
        }
    }
}

impl GoboParams {
    pub fn kind(&self) -> GoboKind {
        if is_choice(&self.kind, "blinds") {
            GoboKind::Blinds
        } else if is_choice(&self.kind, "leaves") {
            GoboKind::Leaves
        } else if is_choice(&self.kind, "fern") {
            GoboKind::Fern
        } else if is_choice(&self.kind, "grid") {
            GoboKind::Grid
        } else if is_choice(&self.kind, "image") {
            GoboKind::Image
        } else {
            GoboKind::Window
        }
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = GoboParams::default();
        f.choice(prefix, "kind", &mut self.kind, GOBO_KIND_NAMES);
        f.int(prefix, "panes_x", &mut self.panes_x, 1, 16);
        f.int(prefix, "panes_y", &mut self.panes_y, 1, 16);
        f.range(prefix, "bar", &mut self.bar, 0.0, 0.5, d.bar);
        f.range(prefix, "frame", &mut self.frame, 0.0, 0.5, d.frame);
        f.int(prefix, "slats", &mut self.slats, 2, 64);
        f.range(prefix, "tilt_deg", &mut self.tilt_deg, 0.0, 90.0, d.tilt_deg);
        f.range(prefix, "slat_width", &mut self.slat_width, 0.5, 1.5, d.slat_width);
        f.range(prefix, "density", &mut self.density, 0.0, 1.0, d.density);
        f.range(prefix, "scale", &mut self.scale, 0.1, 10.0, d.scale);
        f.range(prefix, "distance", &mut self.distance, 0.0, 1.0, d.distance);
        // The same kind of key as an image light's (a path into the ImageSet),
        // so the same limit: a name cut shorter than its key is no gobo.
        if self.image.chars().count() > IMAGE_KEY_LIMIT {
            self.image = self.image.chars().take(IMAGE_KEY_LIMIT).collect();
            f.push(prefix, "image");
        }
    }
}

/// Which way a light's face points, and what hangs on it.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct AimParams {
    /// -90..=90 degrees: the face turned off the direction to the centre,
    /// positive toward the viewer's right.
    pub yaw_deg: f32,
    /// -90..=90 degrees: the face tilted, positive upward.
    pub pitch_deg: f32,
    /// 0..=1: a honeycomb grid on the face, from none (0) to a 15 degree grid (1).
    pub grid: f32,
    /// Barn doors top, right, bottom, left as the viewer sees the light,
    /// each 0..=90: 0 folded flat (no door), 45 straight out, 90 closed.
    pub barn_doors: [f32; 4],
}

impl Default for AimParams {
    fn default() -> Self {
        AimParams { yaw_deg: 0.0, pitch_deg: 0.0, grid: 0.0, barn_doors: [0.0; 4] }
    }
}

impl AimParams {
    /// True when the aim changes nothing: facing the centre, no grid, no doors.
    pub fn is_neutral(&self) -> bool {
        self.yaw_deg == 0.0 && self.pitch_deg == 0.0 && self.grid == 0.0 && self.barn_doors.iter().all(|d| *d == 0.0)
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        f.range(prefix, "yaw_deg", &mut self.yaw_deg, -90.0, 90.0, 0.0);
        f.range(prefix, "pitch_deg", &mut self.pitch_deg, -90.0, 90.0, 0.0);
        f.range(prefix, "grid", &mut self.grid, 0.0, 1.0, 0.0);
        for (i, door) in self.barn_doors.iter_mut().enumerate() {
            f.range(prefix, &format!("barn_doors[{i}]"), door, 0.0, 90.0, 0.0);
        }
    }
}

/// The wear of a light's diffuser. The patterns vary the light around its
/// mean, which stays true, so intensity_ev keeps its meaning.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ImperfectionParams {
    /// 0..=1: folds in the cloth.
    pub wrinkle: f32,
    /// 0..=1: specks and smudges.
    pub dust: f32,
    /// 0..=1: the cloth's weave.
    pub weave: f32,
    /// Where the folds and the specks fall.
    pub seed: u32,
}

impl Default for ImperfectionParams {
    fn default() -> Self {
        ImperfectionParams { wrinkle: 0.0, dust: 0.0, weave: 0.0, seed: 1 }
    }
}

impl ImperfectionParams {
    /// True when nothing is set: the face is clean.
    pub fn is_clear(&self) -> bool {
        !(self.wrinkle > 0.0) && !(self.dust > 0.0) && !(self.weave > 0.0)
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        f.range(prefix, "wrinkle", &mut self.wrinkle, 0.0, 1.0, 0.0);
        f.range(prefix, "dust", &mut self.dust, 0.0, 1.0, 0.0);
        f.range(prefix, "weave", &mut self.weave, 0.0, 1.0, 0.0);
    }
}

/// A stroke light (a Lumi-curve): a smooth curve through directions on the sky.
#[derive(Clone, Debug, PartialEq, DeJson)]
pub struct StrokeParams {
    /// Up to 64 (azimuth, elevation) directions in degrees, in the map's own
    /// frame, that the curve passes through. Azimuth wraps into 0..360,
    /// elevation is -90..=90. Fewer than two draw the arch over the light's
    /// place instead (`shaped::stroke_of`).
    pub points: Vec<[f32; 2]>,
    /// 0..=2: 0 straight lines between the points, 1 a natural curve, 2 a loose one.
    pub curviness: f32,
    /// 0.1..=45 degrees, the full width.
    pub width_deg: f32,
    /// Joins the last point back to the first.
    pub closed: bool,
    /// Up to 32 width multipliers 0..=4, spread evenly along the stroke;
    /// empty is 1 all along. The brightness follows min(multiplier, 1).
    pub profile: Vec<f32>,
    /// 1000..=20000. A Kelvin ramp: the colour temperature slides from the
    /// light's `kelvin` at the first point to this at the last, evenly in
    /// mired, at the light's tint and brightness. None is one colour all along.
    pub end_kelvin: Option<f32>,
    /// Linear 0..=1 per channel. A colour ramp: the light's colour at the
    /// first point blends to this one at the last, at the light's level.
    /// Wins over `end_kelvin` when both are set.
    pub end_rgb: Option<[f32; 3]>,
}

impl Default for StrokeParams {
    /// No points: the arch over the light's place is drawn until some are
    /// given, so a block made by a width slider or by a partial patch does
    /// not move the stroke away from its light.
    fn default() -> Self {
        StrokeParams {
            points: Vec::new(),
            curviness: 1.0,
            width_deg: 2.0,
            closed: false,
            profile: Vec::new(),
            end_kelvin: None,
            end_rgb: None,
        }
    }
}

/// An azimuth folded into 0..360.
fn wrap_azimuth(azimuth_deg: f32) -> f32 {
    let wrapped = azimuth_deg.rem_euclid(360.0);
    // rem_euclid rounds up to exactly 360 for tiny negative inputs.
    if wrapped >= 360.0 {
        0.0
    } else {
        wrapped
    }
}

impl StrokeParams {
    /// The default stroke with a gentle arch of three points centred on
    /// (azimuth, elevation) and `span_deg` wide: what a stroke light draws
    /// until it has points of its own.
    pub fn around(azimuth_deg: f32, elevation_deg: f32, span_deg: f32) -> StrokeParams {
        let az = if azimuth_deg.is_finite() { azimuth_deg } else { 0.0 };
        let el = if elevation_deg.is_finite() { elevation_deg } else { 30.0 };
        let half = if span_deg.is_finite() { (0.5 * span_deg).clamp(1.0, 80.0) } else { 15.0 };
        let lift = 0.2 * half;
        StrokeParams {
            points: vec![
                [wrap_azimuth(az - half), (el - lift).clamp(-90.0, 90.0)],
                [wrap_azimuth(az), (el + lift).clamp(-90.0, 90.0)],
                [wrap_azimuth(az + half), (el - lift).clamp(-90.0, 90.0)],
            ],
            ..StrokeParams::default()
        }
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = StrokeParams::default();
        let points: Vec<[f32; 2]> = self
            .points
            .iter()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .take(64)
            .map(|p| [wrap_azimuth(p[0]), p[1].clamp(-90.0, 90.0)])
            .collect();
        if points != self.points {
            self.points = points;
            f.push(prefix, "points");
        }
        f.range(prefix, "curviness", &mut self.curviness, 0.0, 2.0, d.curviness);
        f.range(prefix, "width_deg", &mut self.width_deg, 0.1, 45.0, d.width_deg);
        let profile: Vec<f32> = self
            .profile
            .iter()
            .take(32)
            .map(|v| if v.is_finite() { v.clamp(0.0, 4.0) } else { 1.0 })
            .collect();
        // NaN never equals itself, so a profile holding one is replaced too.
        if profile != self.profile {
            self.profile = profile;
            f.push(prefix, "profile");
        }
        if let Some(kelvin) = self.end_kelvin.as_mut() {
            f.range(prefix, "end_kelvin", kelvin, 1000.0, 20_000.0, 6500.0);
        }
        if let Some(rgb) = self.end_rgb.as_mut() {
            f.color(prefix, "end_rgb", rgb, 1.0, [1.0; 3]);
        }
    }
}

/// Written by hand for the `Vec<[f32; 2]>` field, as for PolygonParams. The
/// two optional ends are written only when set, the way the derive writes an
/// `Option` field, so a stroke without a ramp stays as short as before.
impl SerJson for StrokeParams {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.st_pre();
        s.field(d + 1, "points");
        ser_points(&self.points, d + 1, s);
        s.conl();
        s.field(d + 1, "curviness");
        self.curviness.ser_json(d + 1, s);
        s.conl();
        s.field(d + 1, "width_deg");
        self.width_deg.ser_json(d + 1, s);
        s.conl();
        s.field(d + 1, "closed");
        self.closed.ser_json(d + 1, s);
        s.conl();
        s.field(d + 1, "profile");
        self.profile.ser_json(d + 1, s);
        if let Some(kelvin) = &self.end_kelvin {
            s.conl();
            s.field(d + 1, "end_kelvin");
            kelvin.ser_json(d + 1, s);
        }
        if let Some(rgb) = &self.end_rgb {
            s.conl();
            s.field(d + 1, "end_rgb");
            rgb[..].ser_json(d + 1, s);
        }
        s.st_post(d);
    }
}

/// A scrim light: a rect of diffusion cloth lit from behind.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ScrimParams {
    /// 0.05..=1: the spread of the glow the light behind the cloth makes, as a
    /// fraction of the half size (a Gaussian's sigma). 0.05 is a tight hot
    /// spot, 1 nearly even.
    pub source_size: f32,
    /// 0..=0.2: a dark frame inside the outline, as a fraction of the shorter half size.
    pub frame: f32,
}

impl Default for ScrimParams {
    fn default() -> Self {
        ScrimParams { source_size: 0.5, frame: 0.0 }
    }
}

impl ScrimParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = ScrimParams::default();
        f.range(prefix, "source_size", &mut self.source_size, 0.05, 1.0, d.source_size);
        f.range(prefix, "frame", &mut self.frame, 0.0, 0.2, d.frame);
    }
}

/// Typed view of `HdriParams::mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Sky,
    Studio,
}

/// Typed view of `SunParams::mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SunMode {
    Time,
    Manual,
}

/// Typed view of `LightParams::shape`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightShape {
    Rect,
    Disc,
    Ring,
    /// A regular polygon, a star, or up to 64 points (shaped.rs).
    Polygon,
    /// A Lumi-curve: a stroke through (azimuth, elevation) points (shaped.rs).
    Stroke,
    /// A rect of diffusion cloth lit from behind (shaped.rs).
    Scrim,
}

/// Typed view of `LightParams::blend`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Add,
    Multiply,
}

impl Mode {
    /// The string stored in `HdriParams::mode`.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Sky => "sky",
            Mode::Studio => "studio",
        }
    }
}

impl SunMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SunMode::Time => "time",
            SunMode::Manual => "manual",
        }
    }
}

impl LightShape {
    pub fn as_str(self) -> &'static str {
        match self {
            LightShape::Rect => "rect",
            LightShape::Disc => "disc",
            LightShape::Ring => "ring",
            LightShape::Polygon => "polygon",
            LightShape::Stroke => "stroke",
            LightShape::Scrim => "scrim",
        }
    }
}

impl Blend {
    pub fn as_str(self) -> &'static str {
        match self {
            Blend::Add => "add",
            Blend::Multiply => "multiply",
        }
    }
}

impl Default for HdriParams {
    fn default() -> Self {
        HdriParams {
            version: PARAMS_VERSION,
            mode: "sky".to_string(),
            intensity_ev: 0.0,
            rotation_deg: 0.0,
            seed: 1,
            sky: SkyParams::default(),
            studio: StudioParams::default(),
            lights: Vec::new(),
        }
    }
}

impl Default for SunParams {
    fn default() -> Self {
        SunParams {
            mode: "time".to_string(),
            year: 2026,
            month: 6,
            day: 21,
            hour: 13.0,
            tz_offset: 0.0,
            latitude: 45.0,
            longitude: 0.0,
            elevation_deg: 45.0,
            azimuth_deg: 180.0,
        }
    }
}

impl Default for AtmosphereParams {
    fn default() -> Self {
        AtmosphereParams { haze: 1.0, air: 1.0, ozone: 1.0, ground_color: [0.18, 0.17, 0.15] }
    }
}

impl Default for SunDiscParams {
    fn default() -> Self {
        SunDiscParams { size_deg: 0.53, softness: 0.2, visible: true }
    }
}

impl Default for CloudParams {
    fn default() -> Self {
        CloudParams { coverage: 0.0, sharpness: 0.5, scale: 1.0, altitude_m: 2000.0, cirrus: 0.0 }
    }
}

impl Default for NightParams {
    fn default() -> Self {
        NightParams {
            stars: 0.5,
            star_brightness: 1.0,
            moon: true,
            moon_elevation_deg: 30.0,
            moon_azimuth_deg: 135.0,
            moon_size_deg: 0.52,
            moon_brightness: 1.0,
            glow_color: [0.02, 0.03, 0.06],
            glow_strength: 1.0,
        }
    }
}

impl Default for StudioParams {
    fn default() -> Self {
        StudioParams { top: [0.05; 3], horizon: [0.2; 3], floor: [0.1; 3], horizon_softness: 0.2 }
    }
}

impl Default for LightParams {
    fn default() -> Self {
        LightParams {
            enabled: true,
            name: "Light".to_string(),
            shape: "rect".to_string(),
            corner: 0.2,
            inner: 0.5,
            azimuth_deg: 0.0,
            elevation_deg: 30.0,
            width_deg: 30.0,
            height_deg: 20.0,
            roll_deg: 0.0,
            softness: 0.2,
            hotspot: 0.0,
            intensity_ev: 3.0,
            kelvin: 6500.0,
            tint: 0.0,
            blend: "add".to_string(),
            key: false,
            rgb: None,
            polygon: None,
            stroke: None,
            scrim: None,
            gels: None,
            source: None,
            source_quality: None,
            image: None,
            gobo: None,
            aim: None,
            imperfections: None,
        }
    }
}

/// Case-insensitive, whitespace-tolerant choice test. The accessors use it so
/// they read loose spellings correctly even before clamp() normalises them.
fn is_choice(value: &str, name: &str) -> bool {
    value.trim().eq_ignore_ascii_case(name)
}

impl HdriParams {
    pub fn mode(&self) -> Mode {
        if is_choice(&self.mode, "studio") {
            Mode::Studio
        } else {
            Mode::Sky
        }
    }

    /// Sanitises NaN/∞, clamps every field into its documented range, maps unknown
    /// enum strings to the default ("sky", "time", "rect", "add"), truncates lights
    /// to LIGHT_LIMIT, and keeps at most one key light (the first).
    /// Returns the dotted paths it changed, e.g. "sky.clouds.coverage", "lights[2].shape".
    ///
    /// NaN becomes the field's default, infinities the nearest bound. Angles
    /// wrap rather than clamp: azimuths into [0, 360), rotation and roll into
    /// [-180, 180]. Choice strings are trimmed and lowercased; a spelling fix
    /// counts as a change.
    pub fn clamp(&mut self) -> Vec<String> {
        let defaults = HdriParams::default();
        let mut f = Fixes::default();
        if self.version != PARAMS_VERSION {
            self.version = PARAMS_VERSION;
            f.push("", "version");
        }
        f.choice("", "mode", &mut self.mode, MODE_NAMES);
        f.range("", "intensity_ev", &mut self.intensity_ev, -10.0, 10.0, defaults.intensity_ev);
        f.wrap180("", "rotation_deg", &mut self.rotation_deg, defaults.rotation_deg);
        self.sky.clamp_fields("sky", &mut f);
        self.studio.clamp_fields("studio", &mut f);
        if self.lights.len() > LIGHT_LIMIT {
            self.lights.truncate(LIGHT_LIMIT);
            f.push("", "lights");
        }
        let mut have_key = false;
        for (i, light) in self.lights.iter_mut().enumerate() {
            let prefix = format!("lights[{i}]");
            light.clamp_fields(&prefix, &mut f);
            // The engine takes one key light; the first one claimed wins.
            if light.key {
                if have_key {
                    light.key = false;
                    f.push(&prefix, "key");
                }
                have_key = true;
            }
        }
        f.changed
    }

    /// Pretty JSON: the preset file format.
    pub fn to_json(&self) -> String {
        self.serialize_json_pretty()
    }

    /// Loads JSON leniently (keys this build does not know are skipped, so
    /// files from newer builds load), then clamps.
    pub fn from_json(json: &str) -> Result<HdriParams, String> {
        let mut params = HdriParams::deserialize_json_lenient(json).map_err(|e| e.to_string())?;
        params.clamp();
        Ok(params)
    }

    /// Deep-merges a partial JSON object into a copy of self: objects merge key by key,
    /// arrays and scalars replace. Unknown keys are dropped and reported as "ignored: <path>".
    /// The merged copy is then clamped; clamped paths are reported too.
    /// "lights" given as an array replaces the whole list; a light object missing fields
    /// is completed from LightParams::default().
    ///
    /// A value of the wrong JSON kind (a string for a number, a scalar for an
    /// object) is an error naming its path. Numbers aimed at integer fields are
    /// rounded (a rounding counts as a clamp). `null` clears an optional field
    /// such as a light's `rgb`; on a required field it is an error.
    pub fn merge_json(&self, patch: &str) -> Result<(HdriParams, Vec<String>), String> {
        let parsed = JsonValue::deserialize_json_strict(patch)
            .map_err(|e| format!("the patch is not valid JSON: {e}"))?;
        let patch_fields = match &parsed {
            JsonValue::Object(fields) => fields,
            _ => return Err("the patch must be a JSON object, e.g. {\"intensity_ev\": 1}".to_string()),
        };
        // Start from a clamped copy: an unclamped NaN would serialise as null
        // and the merged tree would not load back.
        let mut base = self.clone();
        let mut report = base.clamp();
        let mut tree = json_tree(&base)?;
        match &mut tree {
            JsonValue::Object(root) => merge_object(root, patch_fields, "", &mut report)?,
            _ => return Err("internal: the settings did not serialise as an object".to_string()),
        }
        let mut merged = HdriParams::deserialize_json_lenient(&tree.serialize_json()).map_err(|e| {
            format!(
                "the patched settings do not fit ({}); null only clears optional fields such as a light's rgb",
                e.msg
            )
        })?;
        // The typed struct keeps every key it knows, so a patch key missing
        // from its round trip is unknown: report it instead of dropping it silently.
        collect_ignored(&json_tree(&merged)?, patch_fields, "", &mut report);
        report.extend(merged.clamp());
        // Report each path once, at its first mention.
        let mut seen = HashSet::new();
        report.retain(|path| seen.insert(path.clone()));
        Ok((merged, report))
    }
}

impl SkyParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        self.sun.clamp_fields(&join_path(prefix, "sun"), f);
        self.atmosphere.clamp_fields(&join_path(prefix, "atmosphere"), f);
        self.sun_disc.clamp_fields(&join_path(prefix, "sun_disc"), f);
        self.clouds.clamp_fields(&join_path(prefix, "clouds"), f);
        self.night.clamp_fields(&join_path(prefix, "night"), f);
    }
}

impl SunParams {
    pub fn mode(&self) -> SunMode {
        if is_choice(&self.mode, "manual") {
            SunMode::Manual
        } else {
            SunMode::Time
        }
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = SunParams::default();
        f.choice(prefix, "mode", &mut self.mode, SUN_MODE_NAMES);
        f.int(prefix, "year", &mut self.year, 1900, 2100);
        f.int(prefix, "month", &mut self.month, 1, 12);
        // After the month: a day is only valid for its month (and leap year).
        let days = crate::sky::days_in_month(self.year, self.month as u8) as u32;
        f.int(prefix, "day", &mut self.day, 1, days);
        f.range(prefix, "hour", &mut self.hour, 0.0, 24.0, d.hour);
        f.range(prefix, "tz_offset", &mut self.tz_offset, -12.0, 14.0, d.tz_offset);
        f.range(prefix, "latitude", &mut self.latitude, -90.0, 90.0, d.latitude);
        f.range(prefix, "longitude", &mut self.longitude, -180.0, 180.0, d.longitude);
        f.range(prefix, "elevation_deg", &mut self.elevation_deg, -90.0, 90.0, d.elevation_deg);
        f.wrap360(prefix, "azimuth_deg", &mut self.azimuth_deg, d.azimuth_deg);
    }
}

impl AtmosphereParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = AtmosphereParams::default();
        f.range(prefix, "haze", &mut self.haze, 0.0, 10.0, d.haze);
        f.range(prefix, "air", &mut self.air, 0.0, 10.0, d.air);
        f.range(prefix, "ozone", &mut self.ozone, 0.0, 10.0, d.ozone);
        f.color(prefix, "ground_color", &mut self.ground_color, 1.0, d.ground_color);
    }
}

impl SunDiscParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = SunDiscParams::default();
        f.range(prefix, "size_deg", &mut self.size_deg, 0.1, 20.0, d.size_deg);
        f.range(prefix, "softness", &mut self.softness, 0.0, 1.0, d.softness);
    }
}

impl CloudParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = CloudParams::default();
        f.range(prefix, "coverage", &mut self.coverage, 0.0, 1.0, d.coverage);
        f.range(prefix, "sharpness", &mut self.sharpness, 0.0, 1.0, d.sharpness);
        f.range(prefix, "scale", &mut self.scale, 0.1, 10.0, d.scale);
        f.range(prefix, "altitude_m", &mut self.altitude_m, 500.0, 12_000.0, d.altitude_m);
        f.range(prefix, "cirrus", &mut self.cirrus, 0.0, 1.0, d.cirrus);
    }
}

impl NightParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = NightParams::default();
        f.range(prefix, "stars", &mut self.stars, 0.0, 1.0, d.stars);
        f.range(prefix, "star_brightness", &mut self.star_brightness, 0.0, 10.0, d.star_brightness);
        f.range(prefix, "moon_elevation_deg", &mut self.moon_elevation_deg, -90.0, 90.0, d.moon_elevation_deg);
        f.wrap360(prefix, "moon_azimuth_deg", &mut self.moon_azimuth_deg, d.moon_azimuth_deg);
        f.range(prefix, "moon_size_deg", &mut self.moon_size_deg, 0.1, 10.0, d.moon_size_deg);
        f.range(prefix, "moon_brightness", &mut self.moon_brightness, 0.0, 10.0, d.moon_brightness);
        f.color(prefix, "glow_color", &mut self.glow_color, 1.0, d.glow_color);
        f.range(prefix, "glow_strength", &mut self.glow_strength, 0.0, 10.0, d.glow_strength);
    }
}

impl StudioParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = StudioParams::default();
        f.color(prefix, "top", &mut self.top, 10.0, d.top);
        f.color(prefix, "horizon", &mut self.horizon, 10.0, d.horizon);
        f.color(prefix, "floor", &mut self.floor, 10.0, d.floor);
        f.range(prefix, "horizon_softness", &mut self.horizon_softness, 0.01, 1.0, d.horizon_softness);
    }
}

impl LightParams {
    pub fn shape(&self) -> LightShape {
        if is_choice(&self.shape, "disc") {
            LightShape::Disc
        } else if is_choice(&self.shape, "ring") {
            LightShape::Ring
        } else if is_choice(&self.shape, "polygon") {
            LightShape::Polygon
        } else if is_choice(&self.shape, "stroke") {
            LightShape::Stroke
        } else if is_choice(&self.shape, "scrim") {
            LightShape::Scrim
        } else {
            LightShape::Rect
        }
    }

    pub fn blend(&self) -> Blend {
        if is_choice(&self.blend, "multiply") {
            Blend::Multiply
        } else {
            Blend::Add
        }
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = LightParams::default();
        if self.name.chars().count() > LIGHT_NAME_LIMIT {
            self.name = self.name.chars().take(LIGHT_NAME_LIMIT).collect();
            f.push(prefix, "name");
        }
        f.choice(prefix, "shape", &mut self.shape, LIGHT_SHAPE_NAMES);
        f.range(prefix, "corner", &mut self.corner, 0.0, 1.0, d.corner);
        f.range(prefix, "inner", &mut self.inner, 0.0, 0.95, d.inner);
        f.wrap360(prefix, "azimuth_deg", &mut self.azimuth_deg, d.azimuth_deg);
        f.range(prefix, "elevation_deg", &mut self.elevation_deg, -90.0, 90.0, d.elevation_deg);
        f.range(prefix, "width_deg", &mut self.width_deg, 0.1, 170.0, d.width_deg);
        f.range(prefix, "height_deg", &mut self.height_deg, 0.1, 170.0, d.height_deg);
        f.wrap180(prefix, "roll_deg", &mut self.roll_deg, d.roll_deg);
        f.range(prefix, "softness", &mut self.softness, 0.0, 1.0, d.softness);
        f.range(prefix, "hotspot", &mut self.hotspot, 0.0, 1.0, d.hotspot);
        f.range(prefix, "intensity_ev", &mut self.intensity_ev, -10.0, 20.0, d.intensity_ev);
        f.range(prefix, "kelvin", &mut self.kelvin, 1000.0, 20_000.0, d.kelvin);
        f.range(prefix, "tint", &mut self.tint, -1.0, 1.0, d.tint);
        f.choice(prefix, "blend", &mut self.blend, BLEND_NAMES);
        if let Some(rgb) = self.rgb.as_mut() {
            f.color(prefix, "rgb", rgb, 1.0, [1.0; 3]);
        }
        if let Some(polygon) = self.polygon.as_mut() {
            polygon.clamp_fields(&join_path(prefix, "polygon"), f);
        }
        if let Some(stroke) = self.stroke.as_mut() {
            stroke.clamp_fields(&join_path(prefix, "stroke"), f);
        }
        if let Some(scrim) = self.scrim.as_mut() {
            scrim.clamp_fields(&join_path(prefix, "scrim"), f);
        }
        // A polygon or a scrim is drawn in the light's gnomonic box, which
        // stretches toward its edges: past about 120 degrees across a shape
        // distorts (the spec's limit for one shaped light). A stroke may be
        // longer, and rect, disc and ring keep A1's 170 above.
        if matches!(self.shape(), LightShape::Polygon | LightShape::Scrim) {
            f.range(prefix, "width_deg", &mut self.width_deg, 0.1, SHAPED_SPAN_MAX_DEG, d.width_deg);
            f.range(prefix, "height_deg", &mut self.height_deg, 0.1, SHAPED_SPAN_MAX_DEG, d.height_deg);
        }
        if let Some(gels) = self.gels.as_mut() {
            // Known names only, spelled as the table spells them, at most GEL_LIMIT.
            let clean = super::gels::clean_names(gels);
            if clean != *gels {
                *gels = clean;
                f.push(prefix, "gels");
            }
        }
        if let Some(name) = self.source.take() {
            // A known source, spelled as the table spells it. An unknown one
            // hands the colour back to the Kelvin.
            self.source = super::spectrum::source(&name).map(|row| row.name.to_string());
            if self.source.as_deref() != Some(name.as_str()) {
                f.push(prefix, "source");
            }
        }
        if let Some(quality) = self.source_quality.as_mut() {
            f.range(prefix, "source_quality", quality, 0.0, 1.0, super::spectrum::LED_QUALITY_DEFAULT);
        }
        if let Some(image) = self.image.as_mut() {
            image.clamp_fields(&join_path(prefix, "image"), f);
        }
        if let Some(gobo) = self.gobo.as_mut() {
            gobo.clamp_fields(&join_path(prefix, "gobo"), f);
        }
        if let Some(aim) = self.aim.as_mut() {
            aim.clamp_fields(&join_path(prefix, "aim"), f);
        }
        if let Some(imperfections) = self.imperfections.as_mut() {
            imperfections.clamp_fields(&join_path(prefix, "imperfections"), f);
        }
    }
}

/// The dotted paths clamp() changed, in visiting order.
#[derive(Default)]
struct Fixes {
    changed: Vec<String>,
}

impl Fixes {
    fn push(&mut self, prefix: &str, field: &str) {
        self.changed.push(join_path(prefix, field));
    }

    /// NaN becomes the field's default; everything else (infinities included) is clamped.
    fn range(&mut self, prefix: &str, field: &str, value: &mut f32, lo: f32, hi: f32, fallback: f32) {
        let old = *value;
        let new = if old.is_nan() { fallback } else { old.clamp(lo, hi) };
        if old.is_nan() || new != old {
            *value = new;
            self.push(prefix, field);
        }
    }

    /// Azimuths wrap into [0, 360) instead of clamping: 370 means 10.
    fn wrap360(&mut self, prefix: &str, field: &str, value: &mut f32, fallback: f32) {
        let old = *value;
        if (0.0..360.0).contains(&old) {
            return;
        }
        *value = if old.is_finite() {
            let wrapped = old.rem_euclid(360.0);
            // rem_euclid rounds up to exactly 360 for tiny negative inputs.
            if wrapped >= 360.0 { 0.0 } else { wrapped }
        } else {
            fallback
        };
        self.push(prefix, field);
    }

    /// Yaw and roll wrap into [-180, 180]; both ends are kept as given.
    fn wrap180(&mut self, prefix: &str, field: &str, value: &mut f32, fallback: f32) {
        let old = *value;
        if (-180.0..=180.0).contains(&old) {
            return;
        }
        *value = if old.is_finite() { (old + 180.0).rem_euclid(360.0) - 180.0 } else { fallback };
        self.push(prefix, field);
    }

    fn int<T: PartialOrd + Copy>(&mut self, prefix: &str, field: &str, value: &mut T, lo: T, hi: T) {
        let old = *value;
        let new = if old < lo {
            lo
        } else if old > hi {
            hi
        } else {
            old
        };
        if new != old {
            *value = new;
            self.push(prefix, field);
        }
    }

    /// A colour: each channel from 0 to `hi` like range(), reported once.
    fn color(&mut self, prefix: &str, field: &str, value: &mut [f32; 3], hi: f32, fallback: [f32; 3]) {
        let mut changed = false;
        for (channel, default) in value.iter_mut().zip(fallback) {
            let old = *channel;
            let new = if old.is_nan() { default } else { old.clamp(0.0, hi) };
            if old.is_nan() || new != old {
                *channel = new;
                changed = true;
            }
        }
        if changed {
            self.push(prefix, field);
        }
    }

    /// Trims and lowercases a known choice; anything else falls back to the first choice.
    fn choice(&mut self, prefix: &str, field: &str, value: &mut String, choices: &[&str]) {
        let normal = value.trim().to_ascii_lowercase();
        let new = if choices.contains(&normal.as_str()) { normal } else { choices[0].to_string() };
        if *value != new {
            *value = new;
            self.push(prefix, field);
        }
    }
}

/// `prefix.field`; the root has no prefix.
fn join_path(prefix: &str, field: &str) -> String {
    if prefix.is_empty() {
        field.to_string()
    } else {
        format!("{prefix}.{field}")
    }
}

/// The path with every list index blanked (`lights[3].rgb` becomes
/// `lights[].rgb`): the form template_for() and integer_range() match on.
fn schema_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut in_index = false;
    for ch in path.chars() {
        match ch {
            '[' => {
                in_index = true;
                out.push('[');
            }
            ']' => {
                in_index = false;
                out.push(']');
            }
            _ if in_index => {}
            _ => out.push(ch),
        }
    }
    out
}

/// A value as a JSON tree, through its own serialiser.
fn json_tree<T: SerJson>(value: &T) -> Result<JsonValue, String> {
    JsonValue::deserialize_json(&value.serialize_json()).map_err(|e| format!("internal: {e}"))
}

/// The complete object a patch starts from when it creates something that is
/// not there yet. Every light in a patched "lights" list starts from the
/// default light, so a patch names only the fields it changes; so does a
/// light's optional shape block.
fn template_for(schema: &str) -> Option<JsonValue> {
    match schema {
        "lights[]" => json_tree(&LightParams::default()).ok(),
        "lights[].polygon" => json_tree(&PolygonParams::default()).ok(),
        "lights[].stroke" => json_tree(&StrokeParams::default()).ok(),
        "lights[].scrim" => json_tree(&ScrimParams::default()).ok(),
        "lights[].image" => json_tree(&LightImageParams::default()).ok(),
        "lights[].gobo" => json_tree(&GoboParams::default()).ok(),
        "lights[].aim" => json_tree(&AimParams::default()).ok(),
        "lights[].imperfections" => json_tree(&ImperfectionParams::default()).ok(),
        _ => None,
    }
}

/// Integer fields, with the range their Rust type holds. micro-serde refuses
/// `3.6` or `-1` for a `u32`, so numbers aimed at these are rounded into range
/// before the typed parse.
fn integer_range(schema: &str) -> Option<(f64, f64)> {
    match schema {
        "version" | "seed" | "sky.sun.month" | "sky.sun.day" | "lights[].polygon.sides" => Some((0.0, u32::MAX as f64)),
        "sky.sun.year" => Some((i32::MIN as f64, i32::MAX as f64)),
        "lights[].gobo.panes_x" | "lights[].gobo.panes_y" | "lights[].gobo.slats" | "lights[].gobo.seed" => Some((0.0, u32::MAX as f64)),
        "lights[].imperfections.seed" => Some((0.0, u32::MAX as f64)),
        _ => None,
    }
}

/// Keys in sorted order, so reports come out the same every time.
fn sorted_keys(map: &HashMap<String, JsonValue>) -> Vec<&String> {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    keys
}

fn kind_name(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Object(_) => "an object",
        JsonValue::Array(items) if items.first().is_some_and(|item| item.is_number()) => "a list of numbers",
        JsonValue::Array(_) => "a list",
        JsonValue::String(_) => "a string",
        JsonValue::Bool(_) => "true or false",
        JsonValue::Null => "null",
        other if other.is_number() => "a number",
        _ => "a value",
    }
}

/// A patch value must have the JSON kind of the value it replaces, so the
/// error names the field instead of a position in an internal string.
fn check_kind(existing: &JsonValue, value: &JsonValue, path: &str) -> Result<(), String> {
    let fits = match existing {
        JsonValue::Object(_) => value.is_object(),
        JsonValue::Array(items) => match value.as_array() {
            // A colour stays a list of numbers.
            Some(new_items) => {
                !items.first().is_some_and(|item| item.is_number())
                    || new_items.iter().all(|item| item.is_number())
            }
            None => false,
        },
        JsonValue::String(_) => value.is_string(),
        JsonValue::Bool(_) => value.as_bool().is_some(),
        other if other.is_number() => value.is_number(),
        _ => true,
    };
    if fits {
        Ok(())
    } else {
        Err(format!("{path} must be {}", kind_name(existing)))
    }
}

/// Rounds a number aimed at an integer field into that field's type, and
/// reports the path when that changed it. Any other value passes through.
fn coerce_number(value: &JsonValue, schema: &str, path: &str, report: &mut Vec<String>) -> JsonValue {
    match (integer_range(schema), value.as_f64()) {
        (Some((lo, hi)), Some(number)) => {
            let fixed = number.round().clamp(lo, hi);
            if fixed != number {
                report.push(path.to_string());
            }
            if lo >= 0.0 {
                JsonValue::U64(fixed as u64)
            } else {
                JsonValue::I64(fixed as i64)
            }
        }
        _ => value.clone(),
    }
}

/// Merges `patch` into `base` key by key: objects recurse, everything else replaces.
fn merge_object(
    base: &mut HashMap<String, JsonValue>,
    patch: &HashMap<String, JsonValue>,
    prefix: &str,
    report: &mut Vec<String>,
) -> Result<(), String> {
    for key in sorted_keys(patch) {
        let value = &patch[key];
        let path = join_path(prefix, key);
        let schema = schema_path(&path);
        // A list whose items have a template (the lights) is replaced whole,
        // and each item starts from the template.
        if let (Some(items), Some(template)) = (value.as_array(), template_for(&format!("{schema}[]"))) {
            let mut list = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let item_path = format!("{path}[{i}]");
                let fields = item.as_object().ok_or_else(|| format!("{item_path} must be an object"))?;
                let mut merged = template.clone();
                if let JsonValue::Object(target) = &mut merged {
                    merge_object(target, fields, &item_path, report)?;
                }
                list.push(merged);
            }
            base.insert(key.clone(), JsonValue::Array(list));
            continue;
        }
        match base.get_mut(key.as_str()) {
            Some(existing) if !existing.is_null() && !value.is_null() => match (existing, value) {
                (JsonValue::Object(target), JsonValue::Object(fields)) => {
                    merge_object(target, fields, &path, report)?;
                }
                (existing, value) => {
                    check_kind(existing, value, &path)?;
                    *existing = coerce_number(value, &schema, &path, report);
                }
            },
            _ => {
                // Not in the current settings: an optional field that is off,
                // or an unknown key (merge_json's typed round trip tells them
                // apart). A null clears an optional field.
                let fresh = match (value, template_for(&schema)) {
                    (JsonValue::Object(fields), Some(mut template)) => {
                        if let JsonValue::Object(target) = &mut template {
                            merge_object(target, fields, &path, report)?;
                        }
                        template
                    }
                    _ => coerce_number(value, &schema, &path, report),
                };
                base.insert(key.clone(), fresh);
            }
        }
    }
    Ok(())
}

/// Reports every patch key that the typed round trip (`kept`) dropped.
fn collect_ignored(kept: &JsonValue, patch: &HashMap<String, JsonValue>, prefix: &str, report: &mut Vec<String>) {
    for key in sorted_keys(patch) {
        let value = &patch[key];
        let path = join_path(prefix, key);
        match (kept.get(key), value) {
            (None, _) => {
                // A null on an absent key is either an optional field that is
                // already off or an unknown key; neither changes anything.
                if !value.is_null() {
                    report.push(format!("ignored: {path}"));
                }
            }
            (Some(kept_value), JsonValue::Object(fields)) if kept_value.is_object() => {
                collect_ignored(kept_value, fields, &path, report);
            }
            (Some(JsonValue::Array(kept_items)), JsonValue::Array(items)) => {
                for (i, (kept_item, item)) in kept_items.iter().zip(items).enumerate() {
                    if let (JsonValue::Object(_), JsonValue::Object(fields)) = (kept_item, item) {
                        collect_ignored(kept_item, fields, &format!("{path}[{i}]"), report);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A studio set-up with a key light and a coloured multiply light.
    fn studio_params() -> HdriParams {
        let mut p = HdriParams {
            mode: "studio".to_string(),
            intensity_ev: 1.5,
            rotation_deg: -12.5,
            seed: 42,
            ..Default::default()
        };
        p.studio.top = [0.3, 0.3, 0.35];
        p.lights.push(LightParams {
            name: "Key".to_string(),
            key: true,
            azimuth_deg: 37.5,
            elevation_deg: 20.0,
            kelvin: 4500.0,
            ..LightParams::default()
        });
        p.lights.push(LightParams {
            name: "Flag".to_string(),
            shape: "ring".to_string(),
            blend: "multiply".to_string(),
            rgb: Some([0.9, 0.8, 0.7]),
            ..LightParams::default()
        });
        p
    }

    /// Every f32 field (colours included) set to `v`, with one light.
    fn all_numbers(v: f32) -> HdriParams {
        let mut p = HdriParams {
            intensity_ev: v,
            rotation_deg: v,
            ..Default::default()
        };
        let sun = &mut p.sky.sun;
        sun.hour = v;
        sun.tz_offset = v;
        sun.latitude = v;
        sun.longitude = v;
        sun.elevation_deg = v;
        sun.azimuth_deg = v;
        let atmosphere = &mut p.sky.atmosphere;
        atmosphere.haze = v;
        atmosphere.air = v;
        atmosphere.ozone = v;
        atmosphere.ground_color = [v; 3];
        p.sky.sun_disc.size_deg = v;
        p.sky.sun_disc.softness = v;
        let clouds = &mut p.sky.clouds;
        clouds.coverage = v;
        clouds.sharpness = v;
        clouds.scale = v;
        clouds.altitude_m = v;
        clouds.cirrus = v;
        let night = &mut p.sky.night;
        night.stars = v;
        night.star_brightness = v;
        night.moon_elevation_deg = v;
        night.moon_azimuth_deg = v;
        night.moon_size_deg = v;
        night.moon_brightness = v;
        night.glow_color = [v; 3];
        night.glow_strength = v;
        let studio = &mut p.studio;
        studio.top = [v; 3];
        studio.horizon = [v; 3];
        studio.floor = [v; 3];
        studio.horizon_softness = v;
        p.lights.push(LightParams {
            corner: v,
            inner: v,
            azimuth_deg: v,
            elevation_deg: v,
            width_deg: v,
            height_deg: v,
            roll_deg: v,
            softness: v,
            hotspot: v,
            intensity_ev: v,
            kelvin: v,
            tint: v,
            rgb: Some([v; 3]),
            ..LightParams::default()
        });
        p
    }

    #[test]
    fn defaults_and_a_studio_setup_pass_clamp_unchanged() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams::default());
        let before = p.clone();
        assert_eq!(p.clamp(), Vec::<String>::new());
        assert_eq!(p, before);
        let mut s = studio_params();
        assert!(s.clamp().is_empty());
        assert_eq!(s.mode(), Mode::Studio);
        assert_eq!(s.lights[1].shape(), LightShape::Ring);
        assert_eq!(s.lights[1].blend(), Blend::Multiply);
        assert_eq!(HdriParams::default().mode(), Mode::Sky);
        assert_eq!(HdriParams::default().sky.sun.mode(), SunMode::Time);
    }

    #[test]
    fn every_numeric_field_is_bounded_and_reported() {
        let mut p = all_numbers(1.0e9);
        p.sky.sun.year = 99_999;
        p.sky.sun.month = 99;
        p.sky.sun.day = 99;
        let changed = p.clamp();
        let want = [
            "intensity_ev", "rotation_deg",
            "sky.sun.year", "sky.sun.month", "sky.sun.day", "sky.sun.hour", "sky.sun.tz_offset",
            "sky.sun.latitude", "sky.sun.longitude", "sky.sun.elevation_deg", "sky.sun.azimuth_deg",
            "sky.atmosphere.haze", "sky.atmosphere.air", "sky.atmosphere.ozone", "sky.atmosphere.ground_color",
            "sky.sun_disc.size_deg", "sky.sun_disc.softness",
            "sky.clouds.coverage", "sky.clouds.sharpness", "sky.clouds.scale", "sky.clouds.altitude_m",
            "sky.clouds.cirrus",
            "sky.night.stars", "sky.night.star_brightness", "sky.night.moon_elevation_deg",
            "sky.night.moon_azimuth_deg", "sky.night.moon_size_deg", "sky.night.moon_brightness",
            "sky.night.glow_color", "sky.night.glow_strength",
            "studio.top", "studio.horizon", "studio.floor", "studio.horizon_softness",
            "lights[0].corner", "lights[0].inner", "lights[0].azimuth_deg", "lights[0].elevation_deg",
            "lights[0].width_deg", "lights[0].height_deg", "lights[0].roll_deg", "lights[0].softness",
            "lights[0].hotspot", "lights[0].intensity_ev", "lights[0].kelvin", "lights[0].tint",
            "lights[0].rgb",
        ];
        assert_eq!(changed, want);
        assert_eq!(p.intensity_ev, 10.0);
        assert_eq!((p.sky.sun.year, p.sky.sun.month, p.sky.sun.day), (2100, 12, 31));
        assert_eq!(p.sky.clouds.altitude_m, 12_000.0);
        assert_eq!(p.studio.top, [10.0; 3]);
        assert_eq!(p.sky.night.glow_color, [1.0; 3]);
        assert_eq!(p.lights[0].kelvin, 20_000.0);
        assert_eq!(p.lights[0].inner, 0.95);
        assert_eq!(p.lights[0].rgb, Some([1.0; 3]));
        assert!((0.0..360.0).contains(&p.sky.sun.azimuth_deg));
        assert!((-180.0..=180.0).contains(&p.rotation_deg));
        assert!(p.clamp().is_empty(), "clamping twice changes nothing");
    }

    #[test]
    fn nan_becomes_the_default_and_infinity_the_bound() {
        let mut p = all_numbers(f32::NAN);
        let changed = p.clamp();
        // Every float field was NaN; the three integer date fields were not touched.
        assert_eq!(changed.len(), 44, "{changed:?}");
        assert_eq!(p.sky, SkyParams::default());
        assert_eq!(p.studio, StudioParams::default());
        assert_eq!((p.intensity_ev, p.rotation_deg), (0.0, 0.0));
        assert_eq!(p.lights[0], LightParams { rgb: Some([1.0; 3]), ..LightParams::default() });
        // Clean params serialise without null, so they load again.
        let json = p.to_json();
        assert!(!json.contains("null"), "{json}");
        assert_eq!(HdriParams::from_json(&json), Ok(p.clone()));

        let mut q = all_numbers(f32::NEG_INFINITY);
        q.clamp();
        assert_eq!(q.intensity_ev, -10.0);
        assert_eq!(q.sky.clouds.altitude_m, 500.0);
        assert_eq!(q.sky.sun_disc.size_deg, 0.1);
        assert_eq!(q.lights[0].width_deg, 0.1);
        assert_eq!(q.studio.top, [0.0; 3]);
        // An infinite angle has no direction: it falls back to the default.
        assert_eq!(q.sky.sun.azimuth_deg, SunParams::default().azimuth_deg);
        assert_eq!(q.rotation_deg, 0.0);
    }

    #[test]
    fn clamp_wraps_angles_instead_of_pinning_them() {
        let mut p = HdriParams { rotation_deg: 190.0, ..Default::default() };
        p.sky.sun.azimuth_deg = 370.0;
        p.sky.night.moon_azimuth_deg = -90.0;
        p.lights.push(LightParams { azimuth_deg: -30.0, roll_deg: 200.0, ..LightParams::default() });
        let changed = p.clamp();
        assert_eq!(p.rotation_deg, -170.0);
        assert_eq!(p.sky.sun.azimuth_deg, 10.0);
        assert_eq!(p.sky.night.moon_azimuth_deg, 270.0);
        assert_eq!(p.lights[0].azimuth_deg, 330.0);
        assert_eq!(p.lights[0].roll_deg, -160.0);
        assert_eq!(
            changed,
            ["rotation_deg", "sky.sun.azimuth_deg", "sky.night.moon_azimuth_deg", "lights[0].azimuth_deg", "lights[0].roll_deg"]
        );
        // The ends of the documented ranges stay put.
        let mut edge = HdriParams { rotation_deg: 180.0, ..Default::default() };
        edge.lights.push(LightParams { roll_deg: -180.0, azimuth_deg: 0.0, ..LightParams::default() });
        assert!(edge.clamp().is_empty());
    }

    #[test]
    fn clamp_keeps_the_date_real() {
        let date = |year: i32, month: u32, day: u32| {
            let mut p = HdriParams::default();
            p.sky.sun.year = year;
            p.sky.sun.month = month;
            p.sky.sun.day = day;
            let changed = p.clamp();
            ((p.sky.sun.year, p.sky.sun.month, p.sky.sun.day), changed)
        };
        assert_eq!(date(2026, 6, 31).0, (2026, 6, 30));
        assert_eq!(date(2026, 2, 29).0, (2026, 2, 28));
        assert_eq!(date(2028, 2, 29), ((2028, 2, 29), Vec::<String>::new()));
        assert_eq!(date(2026, 13, 5).0, (2026, 12, 5));
        assert_eq!(date(2026, 0, 0), ((2026, 1, 1), vec!["sky.sun.month".to_string(), "sky.sun.day".to_string()]));
        assert_eq!(date(1500, 6, 21).0, (1900, 6, 21));
    }

    #[test]
    fn unknown_choices_fall_back_and_case_is_forgiven() {
        let mut p = HdriParams { mode: " Studio".to_string(), ..Default::default() };
        p.sky.sun.mode = "sundial".to_string();
        p.lights.push(LightParams { shape: "hexagon".to_string(), blend: "Multiply".to_string(), ..LightParams::default() });
        p.lights.push(LightParams { shape: "DISC".to_string(), ..LightParams::default() });
        // The accessors already read the loose spellings before clamp().
        assert_eq!(p.mode(), Mode::Studio);
        assert_eq!(p.lights[1].shape(), LightShape::Disc);
        let changed = p.clamp();
        assert_eq!((p.mode.as_str(), p.sky.sun.mode.as_str()), ("studio", "time"));
        assert_eq!((p.lights[0].shape.as_str(), p.lights[0].blend.as_str()), ("rect", "multiply"));
        assert_eq!(p.lights[1].shape, "disc");
        assert_eq!(changed, ["mode", "sky.sun.mode", "lights[0].shape", "lights[0].blend", "lights[1].shape"]);
        assert_eq!(p.lights[0].blend(), Blend::Multiply);
        assert_eq!(Mode::Studio.as_str(), MODE_NAMES[1]);
        assert_eq!(SunMode::Manual.as_str(), "manual");
        assert_eq!(LightShape::Ring.as_str(), "ring");
        assert_eq!(Blend::Add.as_str(), BLEND_NAMES[0]);
    }

    #[test]
    fn every_choice_name_round_trips_through_its_accessor() {
        // Each listed name reads back through its typed accessor as a variant whose
        // as_str is that name, and clamp() keeps it as it is. An unknown string reads
        // as the list's first name, and every variant is listed. A name added to a
        // list but not to its accessor (or the other way round) fails here.
        let light = |shape: &str, blend: &str| HdriParams {
            lights: vec![LightParams { shape: shape.to_string(), blend: blend.to_string(), ..LightParams::default() }],
            ..Default::default()
        };
        let mut seen = Vec::new();
        for name in MODE_NAMES {
            let mut p = HdriParams { mode: name.to_string(), ..Default::default() };
            seen.push(p.mode().as_str());
            assert!(p.clamp().is_empty() && p.mode == *name, "mode {name}");
        }
        assert_eq!(seen, MODE_NAMES);
        seen.clear();
        for name in SUN_MODE_NAMES {
            let mut p = HdriParams::default();
            p.sky.sun.mode = name.to_string();
            seen.push(p.sky.sun.mode().as_str());
            assert!(p.clamp().is_empty() && p.sky.sun.mode == *name, "sun mode {name}");
        }
        assert_eq!(seen, SUN_MODE_NAMES);
        seen.clear();
        for name in LIGHT_SHAPE_NAMES {
            let mut p = light(name, BLEND_NAMES[0]);
            seen.push(p.lights[0].shape().as_str());
            assert!(p.clamp().is_empty() && p.lights[0].shape == *name, "shape {name}");
        }
        assert_eq!(seen, LIGHT_SHAPE_NAMES);
        seen.clear();
        for name in BLEND_NAMES {
            let mut p = light(LIGHT_SHAPE_NAMES[0], name);
            seen.push(p.lights[0].blend().as_str());
            assert!(p.clamp().is_empty() && p.lights[0].blend == *name, "blend {name}");
        }
        assert_eq!(seen, BLEND_NAMES);

        let unknown = light("?", "?");
        assert_eq!(HdriParams { mode: "?".to_string(), ..Default::default() }.mode().as_str(), MODE_NAMES[0]);
        let sun = SunParams { mode: "?".to_string(), ..Default::default() };
        assert_eq!(sun.mode().as_str(), SUN_MODE_NAMES[0]);
        assert_eq!(unknown.lights[0].shape().as_str(), LIGHT_SHAPE_NAMES[0]);
        assert_eq!(unknown.lights[0].blend().as_str(), BLEND_NAMES[0]);

        // Every variant has its name listed. The matches stop compiling when a
        // variant is added, until it is added to the arrays beside them too.
        for m in [Mode::Sky, Mode::Studio] {
            match m {
                Mode::Sky | Mode::Studio => assert!(MODE_NAMES.contains(&m.as_str()), "{m:?}"),
            }
        }
        for m in [SunMode::Time, SunMode::Manual] {
            match m {
                SunMode::Time | SunMode::Manual => assert!(SUN_MODE_NAMES.contains(&m.as_str()), "{m:?}"),
            }
        }
        for s in [LightShape::Rect, LightShape::Disc, LightShape::Ring, LightShape::Polygon, LightShape::Stroke, LightShape::Scrim] {
            match s {
                LightShape::Rect | LightShape::Disc | LightShape::Ring | LightShape::Polygon | LightShape::Stroke | LightShape::Scrim => {
                    assert!(LIGHT_SHAPE_NAMES.contains(&s.as_str()), "{s:?}")
                }
            }
        }
        for b in [Blend::Add, Blend::Multiply] {
            match b {
                Blend::Add | Blend::Multiply => assert!(BLEND_NAMES.contains(&b.as_str()), "{b:?}"),
            }
        }
    }

    #[test]
    fn lights_are_capped_and_only_the_first_key_survives() {
        let mut p = HdriParams::default();
        for i in 0..(LIGHT_LIMIT + 2) {
            p.lights.push(LightParams { name: format!("L{i}"), key: i == 0 || i == 2, ..LightParams::default() });
        }
        p.lights[1].name = "x".repeat(100);
        let changed = p.clamp();
        assert_eq!(p.lights.len(), LIGHT_LIMIT);
        assert_eq!(p.lights[7].name, "L7", "the list is cut at the end");
        assert!(p.lights[0].key && !p.lights[2].key);
        assert_eq!(p.lights.iter().filter(|l| l.key).count(), 1);
        assert_eq!(p.lights[1].name, "x".repeat(64));
        assert_eq!(changed, ["lights", "lights[1].name", "lights[2].key"]);
    }

    #[test]
    fn json_round_trips_and_tolerates_newer_files() {
        let p = studio_params();
        let json = p.to_json();
        assert!(json.contains('\n'), "preset files are pretty-printed");
        assert_eq!(json.matches("\"rgb\"").count(), 1, "Some(rgb) is written, None is left out");
        assert_eq!(HdriParams::from_json(&json), Ok(p.clone()));
        // A file from a newer build with a field this build does not know still loads.
        let newer = json.replacen('{', "{\"future_field\": {\"x\": [1, 2]},", 1);
        assert_eq!(HdriParams::from_json(&newer), Ok(p.clone()));
        // Loading clamps.
        let mut wild = HdriParams::default();
        wild.sky.clouds.coverage = 5.0;
        assert_eq!(HdriParams::from_json(&wild.to_json()).unwrap().sky.clouds.coverage, 1.0);
        // Missing fields and broken JSON are errors, not silent defaults.
        assert!(HdriParams::from_json("{\"version\": 1}").is_err());
        assert!(HdriParams::from_json("{\"mode\": ").is_err());
    }

    #[test]
    fn merge_changes_only_what_the_patch_names() {
        let base = studio_params();
        let (merged, report) = base
            .merge_json(r#"{"sky": {"clouds": {"coverage": 0.4}}, "studio": {"top": [1, 1, 1]}}"#)
            .unwrap();
        let mut want = base.clone();
        want.sky.clouds.coverage = 0.4;
        want.studio.top = [1.0; 3];
        assert_eq!(merged, want);
        assert!(report.is_empty(), "{report:?}");
        let (same, report) = base.merge_json("{}").unwrap();
        assert_eq!(same, base);
        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn merge_reports_unknown_keys() {
        let base = HdriParams::default();
        let (merged, report) = base
            .merge_json(r#"{"colour": "red", "sky": {"clouds": {"fluffiness": 1, "cirrus": 0.5}}}"#)
            .unwrap();
        assert_eq!(merged.sky.clouds.cirrus, 0.5);
        assert_eq!(report, vec!["ignored: colour", "ignored: sky.clouds.fluffiness"]);
        let mut want = base.clone();
        want.sky.clouds.cirrus = 0.5;
        assert_eq!(merged, want, "unknown keys change nothing");
    }

    #[test]
    fn merge_clamps_and_rounds_integers() {
        let (merged, report) = HdriParams::default()
            .merge_json(r#"{"intensity_ev": 99, "seed": -5, "sky": {"sun": {"month": 3.6, "year": 2030.0}}}"#)
            .unwrap();
        assert_eq!(merged.intensity_ev, 10.0);
        assert_eq!(merged.seed, 0);
        assert_eq!(merged.sky.sun.month, 4);
        assert_eq!(merged.sky.sun.year, 2030);
        for path in ["intensity_ev", "seed", "sky.sun.month"] {
            assert!(report.iter().any(|r| r == path), "{path} missing from {report:?}");
        }
        assert!(!report.iter().any(|r| r == "sky.sun.year"), "2030.0 is already a whole year: {report:?}");
    }

    #[test]
    fn merge_completes_new_lights_from_the_default() {
        let base = studio_params();
        let (merged, report) = base
            .merge_json(r#"{"lights": [{"shape": "disc", "azimuth_deg": 90, "rgb": [1, 0.5, 0.25]}]}"#)
            .unwrap();
        assert_eq!(
            merged.lights,
            vec![LightParams {
                shape: "disc".to_string(),
                azimuth_deg: 90.0,
                rgb: Some([1.0, 0.5, 0.25]),
                ..LightParams::default()
            }]
        );
        assert!(report.is_empty(), "{report:?}");
        let (cleared, _) = base.merge_json(r#"{"lights": []}"#).unwrap();
        assert!(cleared.lights.is_empty(), "an array replaces the whole list");
        let (unknown, report) = base.merge_json(r#"{"lights": [{"colour": "red"}]}"#).unwrap();
        assert_eq!(unknown.lights, vec![LightParams::default()]);
        assert_eq!(report, vec!["ignored: lights[0].colour"]);
    }

    #[test]
    fn merge_refuses_malformed_patches() {
        let base = HdriParams::default();
        assert!(base.merge_json("[1, 2]").is_err(), "the patch must be an object");
        assert!(base.merge_json("{\"sky\": ").is_err(), "broken JSON");
        let err = base.merge_json(r#"{"sky": {"clouds": {"coverage": "lots"}}}"#).unwrap_err();
        assert!(err.contains("sky.clouds.coverage"), "{err}");
        let err = base.merge_json(r#"{"lights": [3]}"#).unwrap_err();
        assert!(err.contains("lights[0]"), "{err}");
        let err = base.merge_json(r#"{"sky": {"atmosphere": {"ground_color": ["a", "b", "c"]}}}"#).unwrap_err();
        assert!(err.contains("ground_color"), "{err}");
        assert!(base.merge_json(r#"{"intensity_ev": null}"#).is_err(), "null only clears optional fields");
    }
}

#[cfg(test)]
mod shape_param_tests {
    use super::*;

    #[test]
    fn a_map_holds_sixteen_lights() {
        assert_eq!(LIGHT_LIMIT, 16);
        let mut p = HdriParams::default();
        p.lights = vec![LightParams::default(); 20];
        let changed = p.clamp();
        assert_eq!(p.lights.len(), 16);
        assert_eq!(changed, ["lights"]);
    }

    #[test]
    fn the_shaped_light_names_are_choices() {
        assert_eq!(LIGHT_SHAPE_NAMES, &["rect", "disc", "ring", "polygon", "stroke", "scrim"]);
        for (name, shape) in [("polygon", LightShape::Polygon), ("stroke", LightShape::Stroke), ("scrim", LightShape::Scrim)] {
            let mut p = HdriParams::default();
            p.lights.push(LightParams { shape: name.to_ascii_uppercase(), ..LightParams::default() });
            assert_eq!(p.lights[0].shape(), shape, "the accessor reads a loose spelling");
            let changed = p.clamp();
            assert_eq!(p.lights[0].shape, name);
            assert_eq!(changed, ["lights[0].shape"], "only the spelling was fixed");
            assert_eq!(shape.as_str(), name);
        }
    }

    #[test]
    fn polygon_params_clamp_and_round_trip() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            shape: "polygon".to_string(),
            polygon: Some(PolygonParams { sides: 1, star: 2.0, points: vec![[f32::NAN, 0.0], [5.0, 0.0], [0.0, 1.0], [-1.0, -1.0]] }),
            ..LightParams::default()
        });
        let changed = p.clamp();
        let poly = p.lights[0].polygon.clone().unwrap();
        assert_eq!(poly.sides, 3);
        assert_eq!(poly.star, 0.95);
        assert_eq!(poly.points, vec![[1.0, 0.0], [0.0, 1.0], [-1.0, -1.0]], "NaN dropped, the rest held in the box");
        assert_eq!(changed, ["lights[0].polygon.sides", "lights[0].polygon.star", "lights[0].polygon.points"]);
        // Fewer than three points make no outline: they go, and the regular polygon shows.
        p.lights[0].polygon = Some(PolygonParams { sides: 5, star: 0.0, points: vec![[0.0, 0.0], [1.0, 1.0]] });
        assert_eq!(p.clamp(), ["lights[0].polygon.points"]);
        assert!(p.lights[0].polygon.as_ref().unwrap().points.is_empty());
        // JSON: pretty and compact both load back, and a missing polygon stays None.
        p.lights[0].polygon = Some(poly.clone());
        assert_eq!(HdriParams::from_json(&p.to_json()).unwrap(), p);
        assert_eq!(PolygonParams::deserialize_json(&poly.serialize_json()).unwrap(), poly);
        let plain = HdriParams { lights: vec![LightParams::default()], ..HdriParams::default() };
        assert!(!plain.to_json().contains("polygon"), "an absent polygon is not written");
        assert_eq!(HdriParams::from_json(&plain.to_json()).unwrap().lights[0].polygon, None);
    }

    #[test]
    fn a_partial_polygon_patch_is_completed_from_the_defaults() {
        let (merged, report) = HdriParams::default()
            .merge_json(r#"{"lights": [{"shape": "polygon", "polygon": {"sides": 5.4, "star": 0.5}}]}"#)
            .unwrap();
        assert_eq!(merged.lights[0].polygon, Some(PolygonParams { sides: 5, star: 0.5, points: Vec::new() }));
        assert_eq!(report, ["lights[0].polygon.sides"], "5.4 was rounded to a whole number of sides");
    }

    #[test]
    fn polygons_and_scrims_stay_under_120_degrees_but_strokes_do_not() {
        let mut p = HdriParams::default();
        for shape in ["polygon", "scrim", "stroke", "rect"] {
            p.lights.push(LightParams { shape: shape.to_string(), width_deg: 150.0, height_deg: 130.0, ..LightParams::default() });
        }
        let changed = p.clamp();
        let span = |i: usize| (p.lights[i].width_deg, p.lights[i].height_deg);
        assert_eq!(span(0), (SHAPED_SPAN_MAX_DEG, SHAPED_SPAN_MAX_DEG), "a polygon lives in its gnomonic box");
        assert_eq!(span(1), (SHAPED_SPAN_MAX_DEG, SHAPED_SPAN_MAX_DEG), "so does a scrim");
        // A stroke may be longer (its points place it), and A3's rect keeps A1's 170.
        assert_eq!(span(2), (150.0, 130.0));
        assert_eq!(span(3), (150.0, 130.0));
        assert_eq!(changed, ["lights[0].width_deg", "lights[0].height_deg", "lights[1].width_deg", "lights[1].height_deg"]);
    }

    #[test]
    fn stroke_params_clamp_and_round_trip() {
        let stroke = StrokeParams {
            points: (0..70).map(|i| [400.0 + i as f32, 100.0]).collect(),
            curviness: 5.0,
            width_deg: 0.0,
            closed: true,
            profile: (0..40).map(|i| if i == 0 { f32::NAN } else { -1.0 }).collect(),
            end_kelvin: Some(500.0),
            end_rgb: Some([2.0, f32::NAN, -1.0]),
        };
        let mut p = HdriParams::default();
        p.lights.push(LightParams { shape: "stroke".to_string(), stroke: Some(stroke), ..LightParams::default() });
        let changed = p.clamp();
        let s = p.lights[0].stroke.clone().unwrap();
        assert_eq!(s.points.len(), 64);
        assert_eq!(s.points[0], [40.0, 90.0], "azimuth wrapped, elevation held");
        assert_eq!((s.curviness, s.width_deg), (2.0, 0.1));
        assert_eq!(s.profile.len(), 32);
        assert_eq!((s.profile[0], s.profile[1]), (1.0, 0.0), "NaN becomes 1, negatives 0");
        assert_eq!(s.end_kelvin, Some(1000.0));
        assert_eq!(s.end_rgb, Some([1.0, 1.0, 0.0]), "each channel into 0..1, NaN to 1");
        assert_eq!(
            changed,
            [
                "lights[0].stroke.points",
                "lights[0].stroke.curviness",
                "lights[0].stroke.width_deg",
                "lights[0].stroke.profile",
                "lights[0].stroke.end_kelvin",
                "lights[0].stroke.end_rgb"
            ]
        );
        assert_eq!(HdriParams::from_json(&p.to_json()).unwrap(), p);
        assert_eq!(StrokeParams::deserialize_json(&s.serialize_json()).unwrap(), s);
        // The default block has no points (the arch over the light is drawn) and
        // no ramp: it writes neither end, and loads back as it was.
        let plain = StrokeParams::default();
        assert!(plain.points.is_empty());
        assert!(!plain.serialize_json().contains("end_"), "{}", plain.serialize_json());
        assert_eq!(StrokeParams::deserialize_json(&plain.serialize_json()).unwrap(), plain);
        let arch = StrokeParams::around(350.0, 85.0, 40.0);
        assert_eq!(arch.points.len(), 3);
        assert!(arch.points.iter().all(|q| (0.0..360.0).contains(&q[0]) && q[1] <= 90.0), "{:?}", arch.points);
        assert_eq!(StrokeParams { points: Vec::new(), ..arch }, plain, "the arch is the default plus its points");
        // A partial patch makes a block without points, so the stroke stays over its light.
        let (merged, _) = HdriParams::default()
            .merge_json(r#"{"lights": [{"shape": "stroke", "stroke": {"width_deg": 5, "end_kelvin": 3000}}]}"#)
            .unwrap();
        let got = merged.lights[0].stroke.clone().unwrap();
        assert_eq!((got.width_deg, got.end_kelvin, got.end_rgb), (5.0, Some(3000.0), None));
        assert!(got.points.is_empty(), "no points were given: the rest is the default block");
    }

    #[test]
    fn scrim_params_clamp_and_round_trip() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            shape: "scrim".to_string(),
            scrim: Some(ScrimParams { source_size: 0.0, frame: f32::NAN }),
            ..LightParams::default()
        });
        let changed = p.clamp();
        assert_eq!(p.lights[0].scrim, Some(ScrimParams { source_size: 0.05, frame: 0.0 }));
        assert_eq!(changed, ["lights[0].scrim.source_size", "lights[0].scrim.frame"]);
        assert_eq!(HdriParams::from_json(&p.to_json()).unwrap(), p);
        let (merged, _) = HdriParams::default()
            .merge_json(r#"{"lights": [{"shape": "scrim", "scrim": {"frame": 0.1}}]}"#)
            .unwrap();
        assert_eq!(merged.lights[0].scrim, Some(ScrimParams { source_size: 0.5, frame: 0.1 }));
    }
}

#[cfg(test)]
mod source_param_tests {
    use super::*;

    #[test]
    fn gel_lists_are_cleaned_and_round_trip() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            gels: Some(vec![" CTO_Half".to_string(), "fog".to_string(), "nd_0.3".to_string(), "red".to_string()]),
            ..LightParams::default()
        });
        assert_eq!(p.clamp(), ["lights[0].gels"]);
        assert_eq!(p.lights[0].gels, Some(vec!["cto_half".to_string(), "nd_0.3".to_string()]));
        assert!(p.clamp().is_empty(), "a clean list stays as it is");
        // An empty list is a bare light and is left alone, so a patch can clear the gels.
        p.lights[0].gels = Some(Vec::new());
        assert!(p.clamp().is_empty());
        p.lights[0].gels = Some(vec!["amber".to_string()]);
        assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()));
        // A light without gels writes no key, so 1a files and 1b files look alike.
        let plain = HdriParams { lights: vec![LightParams::default()], ..HdriParams::default() };
        assert!(!plain.to_json().contains("gels"));
        assert_eq!(HdriParams::from_json(&plain.to_json()).unwrap().lights[0].gels, None);
    }

    #[test]
    fn a_patch_sets_and_clears_gels() {
        let base = HdriParams::default();
        let (merged, report) = base.merge_json(r#"{"lights": [{"gels": ["ctb_quarter", "Blue", "pink"]}]}"#).unwrap();
        assert_eq!(merged.lights[0].gels, Some(vec!["ctb_quarter".to_string(), "blue".to_string()]));
        assert_eq!(report, ["lights[0].gels"], "the spelling fix and the dropped third gel are reported once");
        let (cleared, report) = merged.merge_json(r#"{"lights": [{"gels": []}]}"#).unwrap();
        assert_eq!(cleared.lights[0].gels, Some(Vec::new()));
        assert!(report.is_empty(), "{report:?}");
        let err = base.merge_json(r#"{"lights": [{"gels": "red"}]}"#).unwrap_err();
        assert!(err.contains("do not fit"), "{err}");
    }

    #[test]
    fn sources_are_known_names_or_nothing() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams { source: Some(" Sodium_Low_Pressure".to_string()), ..LightParams::default() });
        p.lights.push(LightParams { source: Some("candle".to_string()), ..LightParams::default() });
        p.lights.push(LightParams { source: Some("halogen".to_string()), ..LightParams::default() });
        // The parametric LED's knob is held to 0..1; NaN is the default quality.
        p.lights.push(LightParams { source: Some("led".to_string()), source_quality: Some(1.5), ..LightParams::default() });
        p.lights.push(LightParams { source: Some("led".to_string()), source_quality: Some(f32::NAN), ..LightParams::default() });
        assert_eq!(p.clamp(), ["lights[0].source", "lights[1].source", "lights[3].source_quality", "lights[4].source_quality"]);
        assert_eq!(p.lights[0].source.as_deref(), Some("sodium_low_pressure"));
        assert_eq!(p.lights[1].source, None, "an unknown source hands the colour back to the Kelvin");
        assert_eq!(p.lights[2].source.as_deref(), Some("halogen"));
        assert_eq!(p.lights[3].source_quality, Some(1.0));
        assert_eq!(p.lights[4].source_quality, Some(crate::hdri::spectrum::LED_QUALITY_DEFAULT));
        assert!(p.clamp().is_empty());
        assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()));
        let plain = HdriParams { lights: vec![LightParams::default()], ..HdriParams::default() };
        assert!(!plain.to_json().contains("source"));
    }

    #[test]
    fn a_patch_sets_and_clears_the_source() {
        let (merged, report) = HdriParams::default()
            .merge_json(r#"{"lights": [{"source": "led_warm_cri95", "tint": 0.1}]}"#)
            .unwrap();
        assert_eq!(merged.lights[0].source.as_deref(), Some("led_warm_cri95"));
        assert!(report.is_empty(), "{report:?}");
        // A lights array replaces the list, so leaving the key out clears it; so does null.
        let (cleared, _) = merged.merge_json(r#"{"lights": [{"source": null}]}"#).unwrap();
        assert_eq!(cleared.lights[0].source, None);
        let (unknown, report) = merged.merge_json(r#"{"lights": [{"source": "candle"}]}"#).unwrap();
        assert_eq!(unknown.lights[0].source, None);
        assert_eq!(report, ["lights[0].source"]);
        // The parametric LED with its knob.
        let (knob, report) = HdriParams::default()
            .merge_json(r#"{"lights": [{"source": "led", "kelvin": 3000.0, "source_quality": 0.9}]}"#)
            .unwrap();
        assert!(report.is_empty(), "{report:?}");
        assert_eq!((knob.lights[0].source.as_deref(), knob.lights[0].source_quality), (Some("led"), Some(0.9)));
    }

    #[test]
    fn image_blocks_clamp_and_round_trip() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            image: Some(LightImageParams { key: "C:\\pics\\window.png".to_string(), fit: " FILL".to_string(), alpha_as_mask: true }),
            ..LightParams::default()
        });
        assert_eq!(p.lights[0].image.as_ref().unwrap().fit(), ImageFit::Fill, "the accessor reads a loose spelling");
        assert_eq!(p.clamp(), ["lights[0].image.fit"]);
        let image = p.lights[0].image.clone().unwrap();
        assert_eq!((image.key.as_str(), image.fit.as_str(), image.alpha_as_mask), ("C:\\pics\\window.png", "fill", true));
        // A key is a path: backslashes and spaces survive the file format.
        assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()));
        p.lights[0].image.as_mut().unwrap().fit = "tile".to_string();
        p.lights[0].image.as_mut().unwrap().key = "k".repeat(2000);
        assert_eq!(p.clamp(), ["lights[0].image.key", "lights[0].image.fit"]);
        assert_eq!(p.lights[0].image.as_ref().unwrap().fit, "stretch");
        assert_eq!(p.lights[0].image.as_ref().unwrap().key.len(), 1024);
        assert_eq!(ImageFit::Fit.as_str(), IMAGE_FIT_NAMES[1]);
        let plain = HdriParams { lights: vec![LightParams::default()], ..HdriParams::default() };
        assert!(!plain.to_json().contains("image"));
    }

    #[test]
    fn a_partial_image_patch_is_completed_from_the_defaults() {
        let (merged, report) = HdriParams::default()
            .merge_json(r#"{"lights": [{"image": {"key": "a.png"}}]}"#)
            .unwrap();
        assert_eq!(merged.lights[0].image, Some(LightImageParams { key: "a.png".to_string(), ..LightImageParams::default() }));
        assert!(report.is_empty(), "{report:?}");
        let (merged, report) = merged.merge_json(r#"{"lights": [{"image": {"key": "a.png", "fit": "fit", "zoom": 2}}]}"#).unwrap();
        assert_eq!(merged.lights[0].image.as_ref().unwrap().fit(), ImageFit::Fit);
        assert_eq!(report, ["ignored: lights[0].image.zoom"]);
    }
}

#[cfg(test)]
mod fx_param_tests {
    use super::*;

    #[test]
    fn clamp_fixes_gobo_fields() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            gobo: Some(GoboParams { kind: "portal".to_string(), panes_x: 99, bar: f32::NAN, tilt_deg: 400.0, slat_width: 3.0, ..GoboParams::default() }),
            ..LightParams::default()
        });
        let changed = p.clamp();
        let g = p.lights[0].gobo.as_ref().expect("the gobo stays");
        assert_eq!(g.kind, "window", "an unknown kind falls back to the first");
        assert_eq!(g.panes_x, 16);
        assert_eq!(g.bar, GoboParams::default().bar, "NaN becomes the default");
        assert_eq!(g.tilt_deg, 90.0);
        assert_eq!(g.slat_width, 1.5);
        assert_eq!(
            changed,
            ["lights[0].gobo.kind", "lights[0].gobo.panes_x", "lights[0].gobo.bar", "lights[0].gobo.tilt_deg", "lights[0].gobo.slat_width"]
        );
        // The typed accessor reads a loose spelling before clamp() fixes it.
        let loose = GoboParams { kind: " Blinds".to_string(), ..GoboParams::default() };
        assert_eq!(loose.kind(), GoboKind::Blinds);
        for (name, kind) in GOBO_KIND_NAMES.iter().zip([GoboKind::Window, GoboKind::Blinds, GoboKind::Leaves, GoboKind::Fern, GoboKind::Grid, GoboKind::Image]) {
            assert_eq!(GoboParams { kind: name.to_string(), ..GoboParams::default() }.kind(), kind);
            assert_eq!(kind.as_str(), *name);
        }
    }

    #[test]
    fn a_partial_gobo_patch_is_completed_from_the_defaults() {
        let patch = r#"{"lights": [{"name": "Window", "gobo": {"kind": "blinds", "slats": 20.4}}]}"#;
        let (merged, report) = HdriParams::default().merge_json(patch).expect("the patch merges");
        let g = merged.lights[0].gobo.as_ref().expect("a gobo");
        assert_eq!(g.kind, "blinds");
        assert_eq!(g.slats, 20);
        assert_eq!(g.panes_x, GoboParams::default().panes_x, "a field left out takes its default");
        assert_eq!(report, ["lights[0].gobo.slats"], "20.4 was rounded to a whole number of slats");
        // null takes the gobo away again.
        let (cleared, _) = merged.merge_json(r#"{"lights": [{"gobo": null}]}"#).expect("null merges");
        assert_eq!(cleared.lights[0].gobo, None);
    }

    #[test]
    fn a_gobo_round_trips_through_json() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            gobo: Some(GoboParams { kind: "blinds".to_string(), slats: 20, vertical: true, fill_sky: true, ..GoboParams::default() }),
            ..LightParams::default()
        });
        assert_eq!(HdriParams::from_json(&p.to_json()).expect("loads"), p);
        let plain = HdriParams { lights: vec![LightParams::default()], ..HdriParams::default() };
        assert!(!plain.to_json().contains("gobo"), "an absent gobo is not written");
        assert_eq!(HdriParams::from_json(&plain.to_json()).expect("loads").lights[0].gobo, None);
    }

    #[test]
    fn a_gobo_picture_name_is_kept_as_long_as_a_light_pictures_key() {
        // Both are keys of the same ImageSet (a path as text): a name cut
        // shorter than the key the picture was loaded under is no gobo.
        let mut p = HdriParams::default();
        let long = format!("C:\\{}\\mask.png", "d".repeat(990));
        p.lights.push(LightParams { gobo: Some(GoboParams { kind: "image".to_string(), image: long.clone(), ..GoboParams::default() }), ..LightParams::default() });
        assert!(p.clamp().is_empty());
        assert_eq!(p.lights[0].gobo.as_ref().unwrap().image, long);
        p.lights[0].gobo.as_mut().unwrap().image = "k".repeat(2000);
        assert_eq!(p.clamp(), ["lights[0].gobo.image"]);
        assert_eq!(p.lights[0].gobo.as_ref().unwrap().image.len(), IMAGE_KEY_LIMIT);
    }

    #[test]
    fn clamp_fixes_aim_fields() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            aim: Some(AimParams { yaw_deg: 200.0, grid: -1.0, barn_doors: [0.0, 120.0, f32::NAN, 45.0], ..AimParams::default() }),
            ..LightParams::default()
        });
        let changed = p.clamp();
        let a = p.lights[0].aim.clone().expect("the aim stays");
        assert_eq!((a.yaw_deg, a.grid, a.barn_doors), (90.0, 0.0, [0.0, 90.0, 0.0, 45.0]));
        assert_eq!(changed, ["lights[0].aim.yaw_deg", "lights[0].aim.grid", "lights[0].aim.barn_doors[1]", "lights[0].aim.barn_doors[2]"]);
        assert!(!a.is_neutral());
        assert!(AimParams::default().is_neutral());
    }

    #[test]
    fn a_partial_aim_patch_is_completed_and_round_trips() {
        let (merged, report) = HdriParams::default().merge_json(r#"{"lights": [{"aim": {"grid": 0.5, "barn_doors": [90, 0, 0, 0]}}]}"#).expect("merges");
        assert_eq!(merged.lights[0].aim, Some(AimParams { grid: 0.5, barn_doors: [90.0, 0.0, 0.0, 0.0], ..AimParams::default() }));
        assert!(report.is_empty(), "{report:?}");
        assert_eq!(HdriParams::from_json(&merged.to_json()).expect("loads"), merged);
        // Four doors, no fewer: a short list does not fit.
        assert!(HdriParams::default().merge_json(r#"{"lights": [{"aim": {"barn_doors": [90, 0]}}]}"#).is_err());
    }

    #[test]
    fn imperfection_params_clamp_merge_and_round_trip() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            imperfections: Some(ImperfectionParams { wrinkle: 3.0, dust: f32::NAN, weave: -1.0, seed: 4 }),
            ..LightParams::default()
        });
        let changed = p.clamp();
        let im = p.lights[0].imperfections.clone().expect("it stays");
        assert_eq!((im.wrinkle, im.dust, im.weave, im.seed), (1.0, 0.0, 0.0, 4));
        assert_eq!(changed, ["lights[0].imperfections.wrinkle", "lights[0].imperfections.dust", "lights[0].imperfections.weave"]);
        assert!(!im.is_clear() && ImperfectionParams::default().is_clear());
        let (merged, report) = HdriParams::default().merge_json(r#"{"lights": [{"imperfections": {"dust": 0.3, "seed": 8.2}}]}"#).expect("merges");
        assert_eq!(merged.lights[0].imperfections, Some(ImperfectionParams { dust: 0.3, seed: 8, ..ImperfectionParams::default() }));
        assert_eq!(report, ["lights[0].imperfections.seed"], "8.2 was rounded");
        assert_eq!(HdriParams::from_json(&merged.to_json()).expect("loads"), merged);
    }
}
