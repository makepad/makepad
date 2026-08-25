//! # The inter-lane contract. FROZEN.
//!
//! Every lane (A scene/loader, B viewport, C navigation, D UI, E tools/sheets,
//! F path tracer) talks to the others only through the types in this file and
//! the `libs/fab_scene` crate. Rules:
//!
//! * **Additive only.** A lane may append variants/fields *inside its own
//!   marked region* at the bottom of this file (`// ---- LANE X EXTENSIONS`).
//!   Nothing above the regions is edited by a lane; nothing is renamed,
//!   reordered or removed. Use `Edit` with a unique anchor; never rewrite the
//!   file. The integrator reconciles the regions at the end.
//! * **State flows one way.** Widgets read `AppState` through
//!   `scope.data.get_mut::<AppState>()` during draw/handle and mutate it only
//!   through the hot-path exceptions listed on [`AppState`] or by emitting a
//!   [`ShellAction`] with `cx.action(...)`. The app applies actions in
//!   `main.rs::App::dispatch` → `AppState::apply_core` plus each lane's
//!   `apply` hook.
//! * **GPU handles never cross threads** and never live in `AppState`.
//!   `AppState` is plain data; the viewport owns its geometries/textures/passes.
//! * **Coordinates:** world is right-handed, Z up, meters (`fab_scene` law).
//!   Screen/window coordinates are layout points, y down (`Rect`, `DVec2`).
//!   NDC is x right, y up, both −1..1.

use makepad_widgets::*;
use std::path::PathBuf;
use std::sync::Arc;

pub use crate::model::{
    aabb_center, aabb_empty, aabb_extent, aabb_is_empty, aabb_radius, aabb_union, Bvh,
    ElementClass, ElementId, Frustum, LayerId, LoadError, LoadProgress, MaterialId,
    MeasureKind, Ray, RayHit, RenderBatch, Scene, SceneSnapshot, SceneState, SectionPlane,
    SectionState, Selection, SheetId, SnapHit, SnapKind, SnapOptions, StoryId, Units, Vertex,
    VERTEX_STRIDE,
};
// Added 2026-08-24 (lane A report §4.7, lane E report R4): the property /
// quantity types the Properties editor shows and the display-unit enum.
pub use crate::model::{LengthUnit, Property, PropertyValue, Quantity};
pub use crate::model::makepad_math::{Aabb, Plane};
pub use crate::model::state::{ExplodeMode, ExplodeState};
pub use makepad_render::sky::{days_in_month, noaa_solar_position, SkyDate};

pub const MAX_VIEWPORTS: usize = 4;

// ===========================================================================
// Camera
// ===========================================================================

/// A look-at camera. `target` is the orbit pivot; `eye - target` is the
/// boom. Perspective uses `fov_y_deg`; orthographic uses `ortho_height`
/// (world meters visible vertically). Lane C mutates it; lanes B/E/F read it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub eye: Vec3f,
    pub target: Vec3f,
    pub up: Vec3f,
    pub fov_y_deg: f32,
    pub ortho: bool,
    pub ortho_height: f32,
    pub near: f32,
    pub far: f32,
    /// Depth of field (lane F): aperture as an f-stop. `0` = pinhole.
    pub f_stop: f32,
    /// Depth of field focus distance in meters along `forward()`.
    pub focus_distance: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            eye: vec3(18.0, -20.0, 12.0),
            target: vec3(5.0, 3.5, 2.5),
            up: vec3(0.0, 0.0, 1.0),
            fov_y_deg: 40.0,
            ortho: false,
            ortho_height: 20.0,
            near: 0.05,
            far: 4000.0,
            f_stop: 0.0,
            focus_distance: 25.0,
        }
    }
}

impl Camera {
    pub fn forward(&self) -> Vec3f {
        (self.target - self.eye).normalize()
    }

    pub fn right(&self) -> Vec3f {
        Vec3f::cross(self.forward(), self.up).normalize()
    }

    /// Camera-space up (orthogonal to forward), not the world up hint.
    pub fn true_up(&self) -> Vec3f {
        Vec3f::cross(self.right(), self.forward()).normalize()
    }

    pub fn distance(&self) -> f32 {
        (self.target - self.eye).length()
    }

    pub fn view(&self) -> Mat4f {
        Mat4f::look_at(self.eye, self.target, self.up)
    }

    /// Standard GL-style clip (z in −1..1, w = −z_view), matching
    /// `Mat4f::perspective` so both projections share depth semantics.
    pub fn projection(&self, aspect: f32) -> Mat4f {
        let aspect = aspect.max(1e-4);
        if self.ortho {
            let h = self.ortho_height.max(1e-3) * 0.5;
            let w = h * aspect;
            let (n, f) = (self.near, self.far);
            Mat4f {
                v: [
                    1.0 / w,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    1.0 / h,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    2.0 / (n - f),
                    0.0,
                    0.0,
                    0.0,
                    (f + n) / (n - f),
                    1.0,
                ],
            }
        } else {
            Mat4f::perspective(self.fov_y_deg, aspect, self.near, self.far)
        }
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4f {
        Mat4f::mul(&self.projection(aspect), &self.view())
    }

    /// Pick near/far from the scene bounds so depth precision follows the
    /// model. Call before `projection()` when the scene or eye changed.
    pub fn fit_clip_planes(&mut self, bounds: &Aabb) {
        if aabb_is_empty(bounds) {
            return;
        }
        let r = aabb_radius(bounds).max(1.0);
        let d = (aabb_center(bounds) - self.eye).length();
        self.far = (d + r * 2.0).max(50.0);
        self.near = (self.far * 1e-5).max(0.02).min(0.5);
    }

    /// Frame a bounding box: keeps the view direction, moves the eye so the
    /// bounding sphere fits the vertical field of view.
    pub fn frame_bounds(&mut self, bounds: &Aabb, aspect: f32) {
        if aabb_is_empty(bounds) {
            return;
        }
        let center = aabb_center(bounds);
        let radius = aabb_radius(bounds).max(0.1);
        let dir = self.forward();
        let half_fov = (self.fov_y_deg.to_radians() * 0.5).max(0.01);
        let fit = if aspect < 1.0 {
            (half_fov.tan() * aspect).atan()
        } else {
            half_fov
        };
        let distance = radius / fit.sin() * 1.05;
        self.target = center;
        self.eye = center - dir * distance;
        self.ortho_height = radius * 2.1 / aspect.min(1.0);
        self.fit_clip_planes(bounds);
    }

    /// World-space ray through an NDC point.
    pub fn ray_at_ndc(&self, ndc: Vec2f, aspect: f32) -> Ray {
        let inv = self.view_projection(aspect).invert();
        let unproject = |z: f32| {
            let v = inv.transform_vec4(vec4(ndc.x, ndc.y, z, 1.0));
            let w = if v.w.abs() < 1e-12 { 1e-12 } else { v.w };
            vec3(v.x / w, v.y / w, v.z / w)
        };
        let a = unproject(-1.0);
        let b = unproject(1.0);
        Ray::new(a, b - a)
    }

    /// Project a world point to NDC. `None` when the point is not on screen
    /// depth-wise: behind the camera (perspective w test) or outside the
    /// near/far clip range (NDC z test — the only test that catches points
    /// behind an *orthographic* eye, where w stays 1; review minor).
    pub fn project(&self, p: Vec3f, aspect: f32) -> Option<Vec3f> {
        let v = self.view_projection(aspect).transform_vec4(vec4(p.x, p.y, p.z, 1.0));
        if v.w <= 1e-6 {
            return None;
        }
        let ndc = vec3(v.x / v.w, v.y / v.w, v.z / v.w);
        if ndc.z < -1.0 || ndc.z > 1.0 {
            return None;
        }
        Some(ndc)
    }
}

/// Projection helper for anything drawn in the 2D pass over a viewport
/// (measurement labels, section handles, gizmos). Built by the viewport once
/// per frame from its camera and rect.
#[derive(Clone, Copy, Debug)]
pub struct ViewProjector {
    pub camera: Camera,
    pub rect: Rect,
    pub aspect: f32,
    pub view_proj: Mat4f,
}

impl ViewProjector {
    pub fn new(camera: Camera, rect: Rect) -> Self {
        let aspect = (rect.size.x / rect.size.y.max(1.0)) as f32;
        ViewProjector {
            camera,
            rect,
            aspect,
            view_proj: camera.view_projection(aspect),
        }
    }

    /// Window-space point (layout points) for a world point, or `None` when
    /// behind the camera or outside the clip depth range (same depth rules
    /// as [`Camera::project`] — ortho behind-the-eye included).
    pub fn project(&self, p: Vec3f) -> Option<DVec2> {
        let v = self.view_proj.transform_vec4(vec4(p.x, p.y, p.z, 1.0));
        if v.w <= 1e-6 {
            return None;
        }
        let nz = v.z / v.w;
        if nz < -1.0 || nz > 1.0 {
            return None;
        }
        let nx = (v.x / v.w) as f64;
        let ny = (v.y / v.w) as f64;
        Some(dvec2(
            self.rect.pos.x + (nx * 0.5 + 0.5) * self.rect.size.x,
            self.rect.pos.y + (0.5 - ny * 0.5) * self.rect.size.y,
        ))
    }

    pub fn ndc(&self, screen: DVec2) -> Vec2f {
        let x = ((screen.x - self.rect.pos.x) / self.rect.size.x.max(1.0)) * 2.0 - 1.0;
        let y = 1.0 - ((screen.y - self.rect.pos.y) / self.rect.size.y.max(1.0)) * 2.0;
        vec2(x as f32, y as f32)
    }

    pub fn ray(&self, screen: DVec2) -> Ray {
        self.camera.ray_at_ndc(self.ndc(screen), self.aspect)
    }

    /// Screen-space size in points of one world meter at `p`.
    pub fn points_per_meter_at(&self, p: Vec3f) -> f64 {
        let r = self.camera.right();
        match (self.project(p), self.project(p + r)) {
            (Some(a), Some(b)) => (b - a).length(),
            _ => 0.0,
        }
    }
}

// ===========================================================================
// View modes
// ===========================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Shading {
    Wireframe,
    #[default]
    Solid,
    Material,
    /// Interactive PBR view drawn by `makepad-render` with the scene's
    /// textures, sun, sky, shadows, and available baked lighting.
    Realtime,
    /// Progressive path-traced preview (lane F). Resets when the camera or
    /// scene changes (`ViewportState::render_dirty`).
    Rendered,
    /// CAD ink: white paper, silhouette + crease lines, hidden lines dashed.
    HiddenLine,
}

impl Shading {
    pub const ALL: [Shading; 6] = [
        Shading::Wireframe,
        Shading::Solid,
        Shading::Material,
        Shading::Realtime,
        Shading::Rendered,
        Shading::HiddenLine,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Shading::Wireframe => "Wireframe",
            Shading::Solid => "Solid",
            Shading::Material => "Material",
            Shading::Realtime => "Realtime",
            Shading::Rendered => "Raytraced",
            Shading::HiddenLine => "Hidden Line",
        }
    }
}

/// Viewport overlay toggles (the "Overlays" popover in the header).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overlays {
    pub grid: bool,
    pub axes: bool,
    pub outlines: bool,
    pub wire_on_shaded: bool,
    pub cavity: bool,
    pub ssao: bool,
    pub shadows: bool,
    pub dof: bool,
    pub section_planes: bool,
    pub section_caps: bool,
    pub measurements: bool,
    pub statistics: bool,
    pub text_info: bool,
    pub nav_gizmo: bool,
    pub pivot: bool,
    pub floor_shadow: bool,
}

impl Default for Overlays {
    fn default() -> Self {
        Overlays {
            grid: true,
            axes: true,
            outlines: true,
            wire_on_shaded: false,
            cavity: true,
            ssao: true,
            shadows: false,
            dof: false,
            section_planes: true,
            section_caps: true,
            measurements: true,
            statistics: true,
            text_info: true,
            nav_gizmo: true,
            pivot: true,
            floor_shadow: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NavMode {
    #[default]
    Orbit,
    Fly,
    Walk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OrbitStyle {
    #[default]
    Turntable,
    Trackball,
}

/// Numpad-style preset views. `Isometric` is our extra.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetView {
    Front,
    Back,
    Right,
    Left,
    Top,
    Bottom,
    Isometric,
}

impl PresetView {
    pub fn label(self) -> &'static str {
        match self {
            PresetView::Front => "Front",
            PresetView::Back => "Back",
            PresetView::Right => "Right",
            PresetView::Left => "Left",
            PresetView::Top => "Top",
            PresetView::Bottom => "Bottom",
            PresetView::Isometric => "Isometric",
        }
    }

    /// Direction the camera looks *along* (eye = target − dir·d) and the up
    /// hint, Z up world. Front looks along +Y (from −Y), as in Fab.
    pub fn look_dir_and_up(self) -> (Vec3f, Vec3f) {
        match self {
            PresetView::Front => (vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, 1.0)),
            PresetView::Back => (vec3(0.0, -1.0, 0.0), vec3(0.0, 0.0, 1.0)),
            PresetView::Right => (vec3(-1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0)),
            PresetView::Left => (vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0)),
            PresetView::Top => (vec3(0.0, 0.0, -1.0), vec3(0.0, 1.0, 0.0)),
            PresetView::Bottom => (vec3(0.0, 0.0, 1.0), vec3(0.0, -1.0, 0.0)),
            PresetView::Isometric => (vec3(-1.0, 1.0, -0.82).normalize(), vec3(0.0, 0.0, 1.0)),
        }
    }
}

/// The T-panel tools. `Walk` switches the active viewport to `NavMode::Walk`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Measure(MeasureKind),
    Section,
    Walk,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Measure(MeasureKind::Distance) => "Measure Distance",
            Tool::Measure(MeasureKind::Area) => "Measure Area",
            Tool::Measure(MeasureKind::Angle) => "Measure Angle",
            Tool::Section => "Section",
            Tool::Walk => "Walk",
        }
    }
}

/// Per-viewport state. Up to [`MAX_VIEWPORTS`] of these (quad view).
#[derive(Clone, Debug)]
pub struct ViewportState {
    pub name: String,
    pub camera: Camera,
    pub nav_mode: NavMode,
    pub orbit_style: OrbitStyle,
    pub shading: Shading,
    pub overlays: Overlays,
    pub xray: bool,
    /// Preset the view is currently aligned to, cleared by any orbit.
    pub preset: Option<PresetView>,
    /// What is under the pointer right now (hot path: written by the viewport
    /// widget directly, read by everyone).
    pub hover: Option<RayHit>,
    /// Bumped by lane C on every camera change. Consumers diff it.
    pub camera_revision: u64,
    /// Set whenever the path tracer's accumulation must restart (camera,
    /// scene, visibility, section, sun). Lane F clears it.
    pub render_dirty: bool,
    /// User-owned Stop/Resume state for this viewport's Rendered preview.
    pub rendered_paused: bool,
    /// Last progress reported by this viewport's tracer.
    pub rendered_samples: u32,
    pub rendered_done: bool,
    /// Resolution-ladder rung the tracer is on (0 = native).
    pub rendered_stage: u32,
}

impl Default for ViewportState {
    fn default() -> Self {
        ViewportState {
            name: "User Perspective".into(),
            camera: Camera::default(),
            nav_mode: NavMode::Orbit,
            orbit_style: OrbitStyle::Turntable,
            shading: Shading::Solid,
            overlays: Overlays::default(),
            xray: false,
            preset: None,
            hover: None,
            camera_revision: 0,
            render_dirty: true,
            rendered_paused: false,
            rendered_samples: 0,
            rendered_done: false,
            rendered_stage: 0,
        }
    }
}

impl ViewportState {
    /// Lane C calls this after every camera mutation.
    pub fn mark_camera_changed(&mut self) {
        self.camera_revision = self.camera_revision.wrapping_add(1);
        self.render_dirty = true;
        self.preset = None;
    }

    pub fn view_label(&self) -> String {
        let proj = if self.camera.ortho {
            "Orthographic"
        } else {
            "Perspective"
        };
        match self.preset {
            Some(p) => format!("{} {}", p.label(), proj),
            None => format!("User {}", proj),
        }
    }

    pub fn rendered_badge(&self) -> String {
        if self.rendered_paused {
            format!("stopped · {} spp", self.rendered_samples)
        } else if self.rendered_done {
            format!("done · {} spp", self.rendered_samples)
        } else if self.rendered_stage > 0 {
            // The resolution ladder's coarse rungs: the whole frame is
            // traced at native >> stage and sharpens rung by rung.
            format!("tracing · 1/{}", 1u32 << self.rendered_stage)
        } else {
            format!("converging · {} spp", self.rendered_samples)
        }
    }
}

// ===========================================================================
// Sun, render, load, ui, stats
// ===========================================================================

/// Civil sun/sky state shared by realtime and path-traced views.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyState {
    pub date: SkyDate,
    pub time_local: f32,
    /// Effective civil offset in hours east of UTC, including DST.
    pub tz_offset: f32,
    pub latitude: f32,
    pub longitude: f32,
    /// Project north clockwise from model +Y, in degrees.
    pub north_deg: f32,
    pub turbidity: f32,
    /// Distance-haze amount, 0 = off and 1 = the renderer's full outdoor
    /// fog density. Presentation-only: it does not change sky radiance or
    /// path transport.
    pub haze: f32,
    /// User compensation applied by the engine sky model.
    pub exposure_ev: f32,
}

/// Compatibility name for the tool and viewport APIs.
pub type SunSettings = SkyState;

impl Default for SkyState {
    fn default() -> Self {
        Self {
            date: SkyDate::default(),
            time_local: 14.0,
            tz_offset: 2.0,
            latitude: 52.37,
            longitude: 4.9,
            north_deg: 0.0,
            turbidity: 2.5,
            haze: 0.35,
            exposure_ev: 0.0,
        }
    }
}

impl SkyState {
    /// How much brighter the sun is than the sky it hangs in, at full
    /// daylight — see [`makepad_game_sim::SunConfig::daylight_balance`].
    ///
    /// The engine's stock split is about 2.6:1, a soft key that suits a
    /// stylised world and reads as a bright overcast day: shadows fill in,
    /// facades flatten, and a sunlit roof is barely brighter than the wall
    /// under it. Measured clear daylight is nothing like that — diffuse
    /// horizontal illuminance is roughly a tenth of the global — so a
    /// building viewer, whose whole job includes "where does the sun fall
    /// at 14:00", asks for the clear sky.
    pub const DAYLIGHT_BALANCE: f32 = 9.0;

    pub fn solar_position(&self) -> (f32, f32) {
        noaa_solar_position(
            self.date,
            self.time_local,
            self.tz_offset,
            self.latitude,
            self.longitude,
        )
    }

    /// Unit vector toward the sun in Fab's Z-up coordinates.
    pub fn direction(&self) -> Vec3f {
        let (elevation, azimuth) = self.solar_position();
        let elevation = elevation.to_radians();
        let azimuth = (azimuth + self.north_deg).to_radians();
        let horizontal = elevation.cos();
        vec3(
            horizontal * azimuth.sin(),
            horizontal * azimuth.cos(),
            elevation.sin(),
        )
        .normalize()
    }

    pub fn elevation_deg(&self) -> f32 {
        self.solar_position().0
    }

    pub fn azimuth_deg(&self) -> f32 {
        (self.solar_position().1 + self.north_deg).rem_euclid(360.0)
    }

    /// Apply the loader's `arch.*` scene metadata. The stored UTC offset is
    /// standard time; `arch.site.dst` advances it by one civil hour.
    pub fn apply_site_metadata(&mut self, metadata: &[(String, String)]) -> bool {
        let get = |key: &str| {
            metadata
                .iter()
                .find(|(candidate, _)| candidate == key)
                .map(|(_, value)| value.as_str())
        };
        let mut found = false;
        let mut has_site_coordinates = false;
        if let Some(value) = get("arch.site.lat").and_then(|value| value.parse().ok()) {
            self.latitude = value;
            found = true;
            has_site_coordinates = true;
        }
        if let Some(value) = get("arch.site.lon").and_then(|value| value.parse().ok()) {
            self.longitude = value;
            found = true;
            has_site_coordinates = true;
        }
        if let Some(value) = get("arch.north_deg").and_then(|value| value.parse().ok()) {
            self.north_deg = value;
            found = true;
        }

        // The converter writes the same facts under its own names
        // (`summer_time`, `timezone_min`, `date_local`, `minute_of_day`);
        // either spelling is accepted.
        let truthy = |value: &str| value.eq_ignore_ascii_case("true") || value == "1";
        let dst = get("arch.site.dst").or_else(|| get("arch.site.summer_time")).is_some_and(|value| truthy(value));
        self.tz_offset = match get("arch.site.utc_offset_hours")
            .and_then(|value| value.parse::<f32>().ok())
            .or_else(|| get("arch.site.timezone_min").and_then(|value| value.parse::<f32>().ok()).map(|m| m / 60.0))
        {
            Some(hours) => {
                found = true;
                hours + if dst { 1.0 } else { 0.0 }
            }
            None if has_site_coordinates => {
                (self.longitude / 15.0).round() + if dst { 1.0 } else { 0.0 }
            }
            None => self.tz_offset,
        };

        if let Some(date) = get("arch.site.date").or_else(|| get("arch.site.date_local")).and_then(parse_site_date) {
            self.date = date;
            found = true;
        }
        let minute_of_day = get("arch.site.minute_of_day")
            .and_then(|value| value.parse::<u32>().ok())
            .map(|m| format!("{:02}:{:02}", (m / 60) % 24, m % 60));
        if let Some(time) = get("arch.site.time").map(str::to_owned).or(minute_of_day).as_deref().and_then(parse_site_time) {
            self.time_local = time;
            found = true;
        }
        self.date.day = self
            .date
            .day
            .clamp(1, days_in_month(self.date.year, self.date.month));
        found
    }

    pub fn exposure(&self) -> f32 {
        2.0f32.powf(self.exposure_ev.clamp(-12.0, 12.0))
    }
}

fn parse_site_date(value: &str) -> Option<SkyDate> {
    let mut parts = value.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse::<u8>().ok()?.clamp(1, 12);
    let day = parts
        .next()?
        .parse::<u8>()
        .ok()?
        .clamp(1, days_in_month(year, month));
    if parts.next().is_some() {
        return None;
    }
    Some(SkyDate { year, month, day })
}

fn parse_site_time(value: &str) -> Option<f32> {
    let mut parts = value.split(':');
    let hour = parts.next()?.parse::<f32>().ok()?;
    let minute = parts.next().unwrap_or("0").parse::<f32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((hour + minute / 60.0).clamp(0.0, 24.0))
}
/// Path tracer settings (lane F). `camera.f_stop` / `focus_distance` live on
/// the active viewport's camera so click-to-focus is a camera edit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderSettings {
    pub max_samples: u32,
    pub bounces: u32,
    /// Output size for F12 renders; the viewport preview uses its own rect.
    pub width: u32,
    pub height: u32,
    /// Preview resolution scale, 0.25..1.
    pub preview_scale: f32,
    pub denoise: bool,
    /// True while an F12 render is accumulating.
    pub running: bool,
    pub samples_done: u32,
    pub elapsed_s: f32,
}

pub const MIN_PREVIEW_SPP: u32 = 64;
pub const MAX_PREVIEW_SPP: u32 = 8192;

impl RenderSettings {
    pub fn clamp_max_samples(samples: u32) -> u32 {
        samples.clamp(MIN_PREVIEW_SPP, MAX_PREVIEW_SPP)
    }

    /// Target changes continue the current accumulation; estimator changes
    /// invalidate it. Size changes are handled by the tracer's target-size
    /// gate and exposure/denoise are post-process settings.
    pub fn accumulation_changed(self, other: Self) -> bool {
        self.bounces != other.bounces
    }
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings {
            max_samples: 1024,
            bounces: 6,
            width: 1920,
            height: 1080,
            preview_scale: 1.0,
            // Off until the denoiser is proven on the seam: the raw
            // accumulation is what the customer's first screenshot needs.
            denoise: false,
            running: false,
            samples_done: 0,
            elapsed_s: 0.0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum LoadStatus {
    #[default]
    Idle,
    Loading {
        path: PathBuf,
        progress: LoadProgress,
    },
    Failed {
        path: PathBuf,
        error: String,
    },
    Loaded {
        path: Option<PathBuf>,
    },
}

impl LoadStatus {
    pub fn is_loading(&self) -> bool {
        matches!(self, LoadStatus::Loading { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Workspace {
    #[default]
    Quad,
    Walkthrough,
    Sections,
    Sheets,
    SunStudy,
    Render,
}

impl Workspace {
    pub const ALL: [Workspace; 6] = [
        Workspace::Quad,
        Workspace::Walkthrough,
        Workspace::Sections,
        Workspace::Sheets,
        Workspace::SunStudy,
        Workspace::Render,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Workspace::Quad => "Quad",
            Workspace::Walkthrough => "Walkthrough",
            Workspace::Sections => "Sections",
            Workspace::Sheets => "Sheets",
            Workspace::SunStudy => "Sun Study",
            Workspace::Render => "Render",
        }
    }

    fn index(self) -> usize {
        match self {
            Workspace::Quad => 0,
            Workspace::Walkthrough => 1,
            Workspace::Sections => 2,
            Workspace::Sheets => 3,
            Workspace::SunStudy => 4,
            Workspace::Render => 5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PropertiesTab {
    #[default]
    Object,
    Element,
    Material,
    Quantities,
    Scene,
    Render,
}

impl PropertiesTab {
    pub const ALL: [PropertiesTab; 6] = [
        PropertiesTab::Object,
        PropertiesTab::Element,
        PropertiesTab::Material,
        PropertiesTab::Quantities,
        PropertiesTab::Scene,
        PropertiesTab::Render,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PropertiesTab::Object => "Object",
            PropertiesTab::Element => "Element",
            PropertiesTab::Material => "Material",
            PropertiesTab::Quantities => "Quantities",
            PropertiesTab::Scene => "Scene",
            PropertiesTab::Render => "Render",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SidebarTab {
    #[default]
    Item,
    View,
    Tool,
}

#[derive(Clone, Copy, Debug)]
struct SidebarPreference {
    open: bool,
    tab: SidebarTab,
}

/// UI chrome state (lane D). Everything a panel needs to draw itself.
#[derive(Clone, Debug)]
pub struct UiState {
    pub workspace: Workspace,
    pub sidebar_open: bool,
    pub sidebar_tab: SidebarTab,
    /// Session-only chrome choices, one per workspace. Sun Study starts with
    /// the sun-bearing Tool page open; after that every workspace restores the
    /// last sidebar state the user left there.
    sidebar_by_workspace: [SidebarPreference; Workspace::ALL.len()],
    pub toolbar_open: bool,
    pub properties_tab: PropertiesTab,
    pub outliner_filter: String,
    /// Mouse-button hints for the status bar, e.g. "LMB Select · MMB Orbit".
    pub status_hint: String,
    /// Transient message (load progress, errors), empty = none.
    pub status_message: String,
    pub active_sheet: Option<SheetId>,
    pub show_perf: bool,
    pub quad_view: bool,
    pub area_maximized: bool,
    pub command_palette_open: bool,
    pub file_browser_open: bool,
    pub keymap_help_open: bool,
    /// "Lock views": every viewport follows the camera of the one being
    /// navigated. On by default so the realtime | path-traced pair converge
    /// on the same framing. Off = independent cameras.
    pub lock_views: bool,
}

impl Default for UiState {
    fn default() -> Self {
        let mut sidebar_by_workspace = [SidebarPreference {
            open: false,
            tab: SidebarTab::Item,
        }; Workspace::ALL.len()];
        sidebar_by_workspace[Workspace::SunStudy.index()] = SidebarPreference {
            open: true,
            tab: SidebarTab::Tool,
        };
        UiState {
            workspace: Workspace::Quad,
            sidebar_open: false,
            sidebar_tab: SidebarTab::Item,
            sidebar_by_workspace,
            toolbar_open: true,
            properties_tab: PropertiesTab::Object,
            outliner_filter: String::new(),
            status_hint: "LMB Select · MMB Orbit · Shift+MMB Pan · Wheel Zoom".into(),
            status_message: String::new(),
            active_sheet: None,
            show_perf: false,
            quad_view: false,
            area_maximized: false,
            command_palette_open: false,
            file_browser_open: false,
            keymap_help_open: false,
            lock_views: false,
        }
    }
}

impl UiState {
    fn remember_sidebar(&mut self) {
        self.sidebar_by_workspace[self.workspace.index()] = SidebarPreference {
            open: self.sidebar_open,
            tab: self.sidebar_tab,
        };
    }

    fn restore_sidebar(&mut self) {
        let preference = self.sidebar_by_workspace[self.workspace.index()];
        self.sidebar_open = preference.open;
        self.sidebar_tab = preference.tab;
    }
}

/// The default Quad workspace: realtime (`libs/render` + bakes) on the
/// left, progressive path tracer on the right, same camera.
pub const DEFAULT_VIEWPORTS: usize = 2;

/// The two default viewports.
pub fn default_views() -> Vec<ViewportState> {
    let realtime = ViewportState {
        name: "Realtime".into(),
        shading: Shading::Realtime,
        ..Default::default()
    };
    let rendered = ViewportState {
        name: "Raytraced".into(),
        shading: Shading::Realtime,
        ..Default::default()
    };
    vec![realtime, rendered]
}

/// Written by the viewport (lane B) every frame; read by the status bar.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    pub fps: f32,
    pub frame_ms: f32,
    pub triangles_drawn: u64,
    pub draw_calls: u32,
    pub visible_elements: u32,
    pub gpu_bytes: u64,
}

/// One key of a camera track (lane G produces, C plays, F renders).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraKey {
    /// Seconds from the start of the track.
    pub t: f32,
    pub pos: Vec3f,
    pub look_at: Vec3f,
    pub up: Vec3f,
    pub fov_y_deg: f32,
}

/// A cinematic camera path. Keys are sorted by `t`; between keys the
/// sampler interpolates linearly — lane G bakes C2-smooth, constant-speed
/// tracks densely enough (≥ 30 keys/s) that linear is invisible, and lane F
/// renders exactly the sampled cameras.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CameraTrack {
    pub name: String,
    /// Shot kind label for the Tours panel ("Drone reveal", "Walkthrough" …).
    pub kind: String,
    pub keys: Vec<CameraKey>,
    /// Frames per second the track was generated for (render sequence).
    pub fps: f32,
}

impl CameraTrack {
    pub fn duration(&self) -> f32 {
        self.keys.last().map(|k| k.t).unwrap_or(0.0)
    }

    /// Sample at `t` (clamped). `None` for an empty track.
    pub fn sample(&self, t: f32) -> Option<CameraKey> {
        let n = self.keys.len();
        if n == 0 {
            return None;
        }
        if n == 1 || t <= self.keys[0].t {
            return Some(self.keys[0]);
        }
        if t >= self.keys[n - 1].t {
            return Some(self.keys[n - 1]);
        }
        let i = self.keys.partition_point(|k| k.t <= t).max(1);
        let a = self.keys[i - 1];
        let b = self.keys[i];
        let span = (b.t - a.t).max(1e-6);
        let f = ((t - a.t) / span).clamp(0.0, 1.0);
        Some(CameraKey {
            t,
            pos: Vec3f::from_lerp(a.pos, b.pos, f),
            look_at: Vec3f::from_lerp(a.look_at, b.look_at, f),
            up: Vec3f::from_lerp(a.up, b.up, f).normalize(),
            fov_y_deg: a.fov_y_deg + (b.fov_y_deg - a.fov_y_deg) * f,
        })
    }

    /// Apply a sampled key to a camera (keeps clip planes, DOF, ortho off).
    pub fn apply(key: &CameraKey, cam: &mut Camera) {
        cam.eye = key.pos;
        cam.target = key.look_at;
        cam.up = key.up;
        cam.fov_y_deg = key.fov_y_deg;
        cam.ortho = false;
    }
}

/// Tours (lane G): generated tracks, transport, follow target.
#[derive(Clone, Debug, Default)]
pub struct TourState {
    pub tracks: Vec<CameraTrack>,
    pub active: Option<usize>,
    pub playing: bool,
    /// Playhead, seconds.
    pub time: f32,
    /// Viewport that follows the playhead (usually the realtime one).
    pub follow_view: usize,
    /// Set while lane G's analysis/generation runs on its worker.
    pub generating: bool,
    pub status: String,
}

impl TourState {
    pub fn active_track(&self) -> Option<&CameraTrack> {
        self.active.and_then(|i| self.tracks.get(i))
    }
}

/// A finished measurement (lane E).
#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    pub kind: MeasureKind,
    pub points: Vec<Vec3f>,
    /// Meters, square meters or degrees depending on `kind`.
    pub value: f64,
    pub label: String,
}

/// Loader-built data used by walk entry and every navigation ray. The costly
/// voxel/room analysis is reduced to immutable, O(1)-lookup data before it
/// crosses to the UI thread.
pub struct WalkSceneAnalysis {
    pub scene_generation: u64,
    /// The full site result is retained so entrance/tour consumers never pay
    /// the voxel/room analysis twice for one scene revision. `None` exists
    /// only for the focused navigation performance fixture.
    pub site: Option<Arc<makepad_fab_tour::SiteAnalysis>>,
    pub entry: makepad_fab_tour::WalkEntryPose,
    pub building: Aabb,
    /// Lowest legitimate walking surface. Mirrored terrain skirts below this
    /// are never accepted as floors or walls.
    pub floor_min_z: f32,
    passable_elements: Arc<[bool]>,
    pub analyse_ms: f32,
}

impl std::fmt::Debug for WalkSceneAnalysis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalkSceneAnalysis")
            .field("scene_generation", &self.scene_generation)
            .field("has_site", &self.site.is_some())
            .field("entry", &self.entry)
            .field("building", &self.building)
            .field("floor_min_z", &self.floor_min_z)
            .field("analyse_ms", &self.analyse_ms)
            .finish()
    }
}

impl WalkSceneAnalysis {
    fn passable_mask(scene: &Scene) -> Arc<[bool]> {
        scene
            .elements
            .iter()
            .map(|element| {
                matches!(
                    &element.class,
                    ElementClass::Door | ElementClass::Opening | ElementClass::Zone
                )
            })
            .collect::<Vec<_>>()
            .into()
    }

    /// Run the tour crate's opening/portal analysis. Loader-only: this can
    /// take hundreds of milliseconds on a large architectural model.
    pub fn analyse(scene: &Scene, eye_height: f32) -> WalkSceneAnalysis {
        let started = std::time::Instant::now();
        let tour_scene = makepad_fab_tour::TourScene::from(scene);
        let site = Arc::new(makepad_fab_tour::SiteAnalysis::analyse(
            &tour_scene,
            &makepad_fab_tour::AnalysisConfig::default(),
        ));
        let building = if aabb_is_empty(&site.building) {
            framing_bounds(scene)
        } else {
            site.building
        };
        let entry = if site.entrance.is_some() {
            site.walk_entry_pose(eye_height, 1.5)
        } else if aabb_is_empty(&building) {
            makepad_fab_tour::WalkEntryPose {
                eye: vec3(0.0, 0.0, eye_height),
                forward: vec3(0.0, 1.0, 0.0),
            }
        } else {
            makepad_fab_tour::WalkEntryPose {
                eye: vec3(
                    (building.min.x + building.max.x) * 0.5,
                    (building.min.y + building.max.y) * 0.5,
                    building.min.z + eye_height,
                ),
                forward: vec3(0.0, 1.0, 0.0),
            }
        };
        WalkSceneAnalysis {
            scene_generation: scene.generation,
            site: Some(site),
            entry,
            building,
            floor_min_z: if aabb_is_empty(&building) {
                f32::NEG_INFINITY
            } else {
                building.min.z - 1.0
            },
            passable_elements: Self::passable_mask(scene),
            analyse_ms: started.elapsed().as_secs_f32() * 1000.0,
        }
    }

    #[inline]
    pub fn element_is_passable(&self, id: ElementId) -> bool {
        self.passable_elements
            .get(id.index())
            .copied()
            .unwrap_or(false)
    }

    #[cfg(test)]
    pub(crate) fn for_nav_test(
        scene: &Scene,
        building: Aabb,
        entry: makepad_fab_tour::WalkEntryPose,
    ) -> WalkSceneAnalysis {
        WalkSceneAnalysis {
            scene_generation: scene.generation,
            site: None,
            entry,
            building,
            floor_min_z: building.min.z - 1.0,
            passable_elements: Self::passable_mask(scene),
            analyse_ms: 0.0,
        }
    }
}

// ===========================================================================
// AppState
// ===========================================================================

/// The single source of truth, owned by `App`, passed to every widget via
/// `Scope::with_data`. Plain data.
///
/// Hot-path direct writes allowed (everything else goes through actions):
/// * viewport (B): `views[i].hover`, `stats`
/// * navigator (C): `views[i].camera`, `views[i].nav_mode`, `views[i].preset`
///   + `mark_camera_changed()`
/// * tools (E): `measurements`, `scene_state.section` via `set_section`
/// * path tracer (F): `render.samples_done/elapsed_s`, `views[i].render_dirty = false`
pub struct AppState {
    /// Never `None`: `Scene::empty()` until a load completes.
    pub scene: Arc<Scene>,
    /// Renderer-facing flat copy, built on the loader thread with the scene.
    pub snapshot: Option<Arc<SceneSnapshot>>,
    pub scene_state: SceneState,
    pub views: Vec<ViewportState>,
    pub active_view: usize,
    pub sun: SunSettings,
    /// Realtime shadow presentation toggle; not part of the physical sky.
    pub sun_shadows: bool,
    pub render: RenderSettings,
    pub load: LoadStatus,
    pub recent: Vec<PathBuf>,
    pub ui: UiState,
    pub stats: FrameStats,
    pub tool: Tool,
    pub snap: SnapOptions,
    pub measurements: Vec<Measurement>,
    pub tour: TourState,
    /// Bumped by `set_scene`; widgets holding per-scene caches diff it.
    pub scene_revision: u64,
    /// Entrance and navigation-query cache for exactly `walk_analysis_revision`.
    pub walk_analysis: Option<Arc<WalkSceneAnalysis>>,
    pub walk_analysis_revision: u64,
    /// The display half of `Units` as a UI preference (lane E report R4):
    /// `scene.units` is immutable inside the `Arc<Scene>`, so "show
    /// measurements in mm" lives here. Copied from the scene on `set_scene`
    /// (`source_to_meters` informational), edited via `SetDisplayUnit`.
    /// Every length/area label formats through `units.display`/`precision`.
    pub units: Units,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            scene: Arc::new(Scene::empty()),
            snapshot: None,
            scene_state: SceneState::default(),
            views: default_views(),
            active_view: 0,
            sun: SunSettings::default(),
            sun_shadows: true,
            render: RenderSettings::default(),
            load: LoadStatus::Idle,
            recent: Vec::new(),
            ui: UiState::default(),
            stats: FrameStats::default(),
            tool: Tool::Select,
            snap: SnapOptions::default(),
            measurements: Vec::new(),
            tour: TourState::default(),
            scene_revision: 0,
            walk_analysis: None,
            walk_analysis_revision: 0,
            units: Units {
                source_to_meters: 1.0,
                display: LengthUnit::Meter,
                precision: 2,
            },
        }
    }
}


/// The box a camera should frame: per axis the 4th..96th percentile of a
/// vertex sample, padded 5 %. `Scene::bounds` stays the truth for culling
/// and clip planes, but one stray decode-artifact mesh must not push the
/// camera away from the building (Woodside: ~530 site vertices mirrored to
/// z = −134 beneath a hillside whose real box is 89 × 120 × 23 m).
pub fn framing_bounds(scene: &Scene) -> Aabb {
    let stride = crate::model::VERTEX_STRIDE;
    let total: usize = scene.batches.iter().map(|b| b.vertices.len() / stride).sum();
    if total < 64 {
        return scene.bounds;
    }
    let step = (total / 200_000).max(1);
    let mut axes: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for b in &scene.batches {
        let v = &b.vertices;
        for i in (0..v.len() / stride).step_by(step) {
            let o = i * stride;
            axes[0].push(v[o]);
            axes[1].push(v[o + 1]);
            axes[2].push(v[o + 2]);
        }
    }
    let mut lo = [0.0f32; 3];
    let mut hi = [0.0f32; 3];
    for (k, a) in axes.iter_mut().enumerate() {
        let n = a.len();
        if n < 64 {
            return scene.bounds;
        }
        // 4..96 %: Woodside's mirrored skirt is 2.6 % of its vertices, all
        // at one extreme; a 1 % cut leaves it in the frame.
        let li = ((n - 1) as f32 * 0.04) as usize;
        let hi_i = ((n - 1) as f32 * 0.96) as usize;
        let (_, l, _) = a.select_nth_unstable_by(li, |x, y| x.total_cmp(y));
        lo[k] = *l;
        let (_, h, _) = a.select_nth_unstable_by(hi_i, |x, y| x.total_cmp(y));
        hi[k] = *h;
        let pad = ((hi[k] - lo[k]) * 0.05).max(0.5);
        lo[k] -= pad;
        hi[k] += pad;
    }
    let out = Aabb {
        min: vec3(lo[0], lo[1], lo[2]),
        max: vec3(hi[0], hi[1], hi[2]),
    };
    if aabb_is_empty(&out) {
        scene.bounds
    } else {
        out
    }
}

impl AppState {
    pub fn view(&self) -> &ViewportState {
        &self.views[self.active_view.min(self.views.len() - 1)]
    }

    pub fn view_mut(&mut self) -> &mut ViewportState {
        let i = self.active_view.min(self.views.len() - 1);
        &mut self.views[i]
    }

    pub fn view_at(&self, index: usize) -> &ViewportState {
        &self.views[index.min(self.views.len() - 1)]
    }

    pub fn view_at_mut(&mut self, index: usize) -> &mut ViewportState {
        let i = index.min(self.views.len() - 1);
        &mut self.views[i]
    }

    /// Install a freshly loaded scene: resets selection/visibility/measures,
    /// frames every viewport on the new bounds, marks everything dirty.
    pub fn set_scene(&mut self, scene: Arc<Scene>) {
        let framing = framing_bounds(&scene);
        self.snapshot = Some(scene.snapshot());
        self.scene_state = SceneState::default();
        for (i, l) in scene.layers.iter().enumerate() {
            if !l.visible {
                self.scene_state.set_layer_hidden(LayerId::from_index(i), true);
            }
        }
        self.measurements.clear();
        self.tour = TourState::default();
        self.walk_analysis = None;
        self.walk_analysis_revision = 0;
        let bounds = framing;
        for v in &mut self.views {
            v.camera = Camera::default();
            if !aabb_is_empty(&bounds) {
                v.camera.frame_bounds(&bounds, 16.0 / 9.0);
                v.camera.focus_distance = v.camera.distance();
            }
            v.mark_camera_changed();
        }
        self.load = LoadStatus::Loaded {
            path: scene.source_path.clone(),
        };
        if let Some(p) = &scene.source_path {
            self.recent.retain(|r| r != p);
            self.recent.insert(0, p.clone());
            self.recent.truncate(10);
        }
        // Keep the user's display unit across loads; take the source scale
        // (informational) from the new scene.
        self.units.source_to_meters = scene.units.source_to_meters;
        self.sun.apply_site_metadata(&scene.metadata);
        self.scene = scene;
        self.scene_revision = self.scene_revision.wrapping_add(1);
        self.ui.status_message.clear();
        self.mark_render_dirty();
    }

    /// Analysis for the currently installed scene, never stale data from the
    /// previous load.
    pub fn current_walk_analysis(&self) -> Option<&WalkSceneAnalysis> {
        (self.walk_analysis_revision == self.scene_revision)
            .then(|| self.walk_analysis.as_deref())
            .flatten()
            .filter(|analysis| analysis.scene_generation == self.scene.generation)
    }

    /// Mark every viewport's path-traced accumulation stale.
    pub fn mark_render_dirty(&mut self) {
        for v in &mut self.views {
            v.render_dirty = true;
        }
    }

    /// Lock-views propagation: copy viewport `from`'s camera (and projection
    /// mode) to every other viewport. Call after any camera change when
    /// `ui.lock_views` is set. Returns true when something changed.
    pub fn sync_locked_cameras(&mut self, from: usize) -> bool {
        if !self.ui.lock_views || from >= self.views.len() {
            return false;
        }
        let src = self.views[from].camera;
        let preset = self.views[from].preset;
        let mut changed = false;
        for (i, v) in self.views.iter_mut().enumerate() {
            if i != from && v.camera != src {
                v.camera = src;
                v.mark_camera_changed();
                v.preset = preset;
                changed = true;
            }
        }
        changed
    }

    /// Bounds of the current selection, if any element in it has geometry.
    pub fn selection_bounds(&self) -> Option<Aabb> {
        self.scene
            .bounds_of(self.scene_state.selection.set.iter().copied())
    }

    /// Visibility predicate for picking/culling (closure-friendly).
    pub fn is_visible(&self, id: ElementId) -> bool {
        self.scene_state.is_visible(&self.scene, id)
    }

    /// Apply the actions that only touch plain state. Returns `true` when
    /// something changed and the UI should redraw. Lane-specific actions are
    /// handled by each lane's `apply` in `main.rs::App::dispatch`.
    pub fn apply_core(&mut self, action: &ShellAction) -> bool {
        use ShellAction::*;
        match action {
            LoadStarted(path) => {
                self.load = LoadStatus::Loading {
                    path: path.clone(),
                    progress: crate::model::LoadProgress::Opening,
                };
                self.ui.status_message = format!("Opening {}…", path.display());
                true
            }
            LoadProgress(p) => {
                if let LoadStatus::Loading { progress, path } = &mut self.load {
                    *progress = p.clone();
                    self.ui.status_message = match p {
                        crate::model::LoadProgress::Opening => {
                            format!("Opening {}…", path.display())
                        }
                        crate::model::LoadProgress::Parsing(f) => {
                            format!("Parsing… {:.0}%", f * 100.0)
                        }
                        crate::model::LoadProgress::Meshing { done, total } => {
                            format!("Meshes {done}/{total}")
                        }
                        crate::model::LoadProgress::Building { stage, fraction } => {
                            format!("Building {stage}… {:.0}%", fraction * 100.0)
                        }
                        crate::model::LoadProgress::Done => "Done".into(),
                    };
                }
                true
            }
            Loaded(scene) => {
                self.set_scene(scene.clone());
                true
            }
            WalkAnalysisReady(analysis)
                if analysis.scene_generation == self.scene.generation =>
            {
                self.walk_analysis = Some(analysis.clone());
                self.walk_analysis_revision = self.scene_revision;
                true
            }
            WalkAnalysisReady(_) => false,
            LoadFailed { path, error } => {
                self.load = LoadStatus::Failed {
                    path: path.clone(),
                    error: error.clone(),
                };
                self.ui.status_message = format!("Failed: {error}");
                true
            }
            SelectOnly(id) => {
                // NOTE: no `mark_render_dirty()` here — selection is drawn as
                // an overlay (like the other Select* actions); restarting the
                // path-traced accumulation on every click was review bug M4.
                self.scene_state.select_only(*id);
                true
            }
            SelectToggle(id) => {
                self.scene_state.select_toggle(*id);
                true
            }
            SelectAdd(id) => {
                self.scene_state.select_add(*id);
                true
            }
            SelectSet(ids) => {
                self.scene_state.select_set(ids.iter().copied());
                // Vec order is meaningful at the UI boundary: grouped and
                // range selections put the clicked row's first element first,
                // giving Properties a deterministic representative instead of
                // whichever ID a HashSet happens to yield.
                self.scene_state.selection.active = ids.first().copied();
                true
            }
            ClearSelection => {
                self.scene_state.clear_selection();
                true
            }
            HideSelected => {
                self.scene_state.hide_selected();
                self.mark_render_dirty();
                true
            }
            UnhideAll => {
                self.scene_state.unhide_all();
                self.mark_render_dirty();
                true
            }
            IsolateSelected => {
                self.scene_state.isolate_selected();
                self.mark_render_dirty();
                true
            }
            SetHidden(id, hidden) => {
                self.scene_state.set_hidden(*id, *hidden);
                self.mark_render_dirty();
                true
            }
            SetLayerHidden(id, hidden) => {
                self.scene_state.set_layer_hidden(*id, *hidden);
                self.mark_render_dirty();
                true
            }
            SetStoryHidden(id, hidden) => {
                self.scene_state.set_story_hidden(*id, *hidden);
                self.mark_render_dirty();
                true
            }
            SetSection(section) => {
                self.scene_state.set_section(section.clone());
                self.mark_render_dirty();
                true
            }
            SetExplode(explode) => {
                self.scene_state.set_explode(*explode);
                self.mark_render_dirty();
                true
            }
            SetShading(view, shading) => {
                self.view_at_mut(*view).shading = *shading;
                true
            }
            SetOverlays(view, overlays) => {
                self.view_at_mut(*view).overlays = *overlays;
                true
            }
            ToggleXray(view) => {
                let v = self.view_at_mut(*view);
                v.xray = !v.xray;
                true
            }
            SetNavMode(view, mode) => {
                self.view_at_mut(*view).nav_mode = *mode;
                true
            }
            SetActiveView(view) => {
                self.active_view = (*view).min(self.views.len() - 1);
                true
            }
            SetTool(tool) => {
                self.tool = *tool;
                if let Tool::Walk = tool {
                    self.view_mut().nav_mode = NavMode::Walk;
                } else if self.view().nav_mode == NavMode::Walk {
                    self.view_mut().nav_mode = NavMode::Orbit;
                }
                true
            }
            SetSun(sun) => {
                let mut previous_physical = self.sun;
                let mut next_physical = *sun;
                // Distance haze is a realtime presentation control. Every
                // other sky input changes the environment accumulated by the
                // tracer, including exposure inside the engine sky model.
                previous_physical.haze = 0.0;
                next_physical.haze = 0.0;
                self.sun = *sun;
                if previous_physical != next_physical {
                    self.mark_render_dirty();
                }
                true
            }
            SetSunShadows(on) => {
                self.sun_shadows = *on;
                true
            }
            SetMaterialBaseColor(id, rgba) => {
                // The scene Arc is normally unshared between edits (loaders
                // and bakes hold clones only transiently), so make_mut is a
                // plain mutation; when something does hold a ref it pays one
                // clone rather than corrupting a reader.
                let changed =
                    Arc::make_mut(&mut self.scene).set_material_base_color(*id, *rgba);
                if changed {
                    // The traced pane uploads `snapshot` keyed by its
                    // generation. A colour edit touches one material and
                    // nothing else, so patch the snapshot in place instead
                    // of rebuilding it (a rebuild copies every triangle and
                    // every texture — far too slow per drag step).
                    let generation = self.scene.generation;
                    if let Some(snap) = self.snapshot.as_mut() {
                        let snap = Arc::make_mut(snap);
                        snap.generation = generation;
                        if let Some(sm) = snap.materials.get_mut(id.index()) {
                            sm.albedo = *rgba;
                        }
                    }
                    self.mark_render_dirty();
                }
                changed
            }
            SetRenderSettings(r) => {
                let mut next = *r;
                next.max_samples = RenderSettings::clamp_max_samples(next.max_samples);
                let invalidates = self.render.accumulation_changed(next);
                self.render = next;
                if invalidates {
                    self.mark_render_dirty();
                }
                true
            }
            SetRenderedPaused(view, paused) => {
                self.view_at_mut(*view).rendered_paused = *paused;
                true
            }
            SetWorkspace(w) => {
                self.ui.remember_sidebar();
                self.ui.workspace = *w;
                self.ui.restore_sidebar();
                true
            }
            ToggleSidebar => {
                self.ui.sidebar_open = !self.ui.sidebar_open;
                self.ui.remember_sidebar();
                true
            }
            ToggleToolbar => {
                self.ui.toolbar_open = !self.ui.toolbar_open;
                true
            }
            SetSidebarTab(t) => {
                self.ui.sidebar_tab = *t;
                self.ui.sidebar_open = true;
                self.ui.remember_sidebar();
                true
            }
            SetPropertiesTab(t) => {
                self.ui.properties_tab = *t;
                true
            }
            SetOutlinerFilter(s) => {
                self.ui.outliner_filter = s.clone();
                true
            }
            StatusHint(s) => {
                self.ui.status_hint = s.clone();
                true
            }
            StatusMessage(s) => {
                self.ui.status_message = s.clone();
                true
            }
            SelectSheet(id) => {
                self.ui.active_sheet = *id;
                true
            }
            TogglePerf => {
                self.ui.show_perf = !self.ui.show_perf;
                true
            }
            ToggleQuadView => {
                self.ui.quad_view = !self.ui.quad_view;
                let n = if self.ui.quad_view { MAX_VIEWPORTS } else { DEFAULT_VIEWPORTS };
                while self.views.len() < n {
                    let mut v = ViewportState::default();
                    v.camera = self.views[0].camera;
                    self.views.push(v);
                }
                self.views.truncate(n);
                self.active_view = self.active_view.min(n - 1);
                true
            }
            ToggleLockViews => {
                self.ui.lock_views = !self.ui.lock_views;
                let from = self.active_view;
                self.sync_locked_cameras(from);
                true
            }
            ToggleMaximizeArea => {
                self.ui.area_maximized = !self.ui.area_maximized;
                true
            }
            ToggleCommandPalette => {
                self.ui.command_palette_open = !self.ui.command_palette_open;
                true
            }
            ShowFileBrowser(open) => {
                self.ui.file_browser_open = *open;
                true
            }
            ShowKeymapHelp(open) => {
                self.ui.keymap_help_open = *open;
                true
            }
            AddMeasurement(m) => {
                self.measurements.push(m.clone());
                true
            }
            ClearMeasurements => {
                self.measurements.clear();
                true
            }
            SetSnap(s) => {
                self.snap = *s;
                true
            }
            SetDisplayUnit(unit, precision) => {
                self.units.display = *unit;
                self.units.precision = *precision;
                true
            }
            TourTracks(tracks) => {
                self.tour.tracks = tracks.clone();
                self.tour.generating = false;
                if self.tour.active.map_or(true, |i| i >= self.tour.tracks.len()) {
                    self.tour.active = if self.tour.tracks.is_empty() { None } else { Some(0) };
                }
                self.tour.time = 0.0;
                true
            }
            TourSelect(i) => {
                self.tour.active = Some(*i).filter(|i| *i < self.tour.tracks.len());
                self.tour.time = 0.0;
                self.tour.playing = false;
                true
            }
            TourPlay(play) => {
                self.tour.playing = *play && self.tour.active_track().is_some();
                true
            }
            TourSeek(t) => {
                let d = self.tour.active_track().map(|t| t.duration()).unwrap_or(0.0);
                self.tour.time = t.clamp(0.0, d);
                true
            }
            TourStatus(s) => {
                self.tour.status = s.clone();
                true
            }
            TourGenerate => {
                self.tour.generating = true;
                self.tour.status = "Analysing model…".into();
                true
            }
            _ => false,
        }
    }
}

// ===========================================================================
// Rendering seams (frozen 2026-08-24: review blockers B1 + B3)
// ===========================================================================

/// # The element-id vertex lane + per-element lookup texture (blocker B1)
///
/// The one CPU→GPU path for visibility / selection / hover / explode with
/// **zero geometry re-upload**. Three parties build against this exact spec:
/// lane A packs (`RenderBatch`, loader side), lane B uploads + fetches
/// (`libs/render` extension), lane B's lookup builder refreshes it once per
/// `SceneState.revision`.
///
/// **Vertex stream.** `RenderBatch.vertices` (48 B `fab::model::Vertex`,
/// `VERTEX_STRIDE` = 12 floats) is the loader-side interchange format. On
/// upload — once per `Scene::generation`, never per frame — lane B repacks it
/// into the `libs/render` static-mesh POD **extended with an element lane**:
/// an 8-float, 32 B/vertex layout (`geom.GameMeshVertexElem` in
/// `libs/render`, kept generic and documented there):
///
/// | float | content |
/// |---|---|
/// | 0..3 | position xyz (world, meters) |
/// | 3 | oct-encoded normal (as `GameMeshVertex`) |
/// | 4 | packed f16×2 uv |
/// | 5 | packed unorm8×4 vertex color (material base color) |
/// | 6 | **element id**: `ElementId.0 as f32` (exact below 2^24) |
/// | 7 | reserved, 0.0 |
///
/// **Lookup texture.** One `RGBAf32` data texture per scene generation,
/// [`ELEMENT_LUT_WIDTH`] texels wide, [`ELEMENT_LUT_TEXELS_PER_ELEMENT`]
/// consecutive texels per element, row-major by element index:
///
/// * texel 0: `x` = visibility (0 hidden, 1 visible), `y` = selection state
///   (0 none, 1 selected, 2 active), `z` = hover (0/1), `w` = reserved;
/// * texel 1: `xyz` = explode offset in world meters (the single source of
///   truth is lane A's `ExplodeState::offset` — CPU queries and this texture
///   must agree), `w` = reserved.
///
/// The shader fetches with `element_lut_coord()` math: element `e`, texel `t`
/// → `x = (e * 2 + t) % ELEMENT_LUT_WIDTH`, `y = (e * 2 + t) /
/// ELEMENT_LUT_WIDTH`. Refresh = one small `Texture` upload when
/// `SceneState.revision` changes; never a geometry touch.
pub const ELEMENT_LUT_TEXELS_PER_ELEMENT: usize = 2;
/// Texels per row of the element lookup texture (1024 elements per row).
pub const ELEMENT_LUT_WIDTH: usize = 2048;

/// What one `RenderedPreviewApi::draw` call hands back to the viewport
/// (blocker B3). `texture` is the tonemapped accumulation target the
/// viewport composites *instead of* its own color target.
pub struct RenderedFrame {
    pub texture: Texture,
    /// True while accumulation continues (samples_done < max). The viewport
    /// keeps its `NextFrame` alive iff this is set — once converged the app
    /// goes idle (§5: zero redraws with nothing animating).
    pub converging: bool,
    pub done: bool,
    pub samples_done: u32,
    /// Resolution-ladder rung: 0 = tracing at native, k = at native >> k
    /// (the badge shows "tracing · 1/2ᵏ" while the picture sharpens).
    pub stage_shift: u32,
}

/// # The B↔F seam (blocker B3). FROZEN.
///
/// Lane F implements this on `render::RenderedPreview`; lane B's viewport
/// owns one instance per viewport and calls `draw` from its `draw_walk`
/// whenever `views[view].shading == Shading::Rendered`. The contract:
///
/// * **Pass parenting:** F parents its trace/accumulate/tonemap passes as
///   children of `parent_pass` via `CxDrawPassParent::DrawPass` — a child
///   pass renders before its parent (pass order is parent-distance, NOT
///   creation order; the `apps/vj/src/flow_tween.rs:2567` law), so F's
///   result is ready when the viewport blits it. **Verified 2026-08-25:**
///   under the viewport's own offscreen pass the tonemap never wrote its
///   target on Metal; under the WINDOW pass (`RenderedPreview::
///   draw_under_current_pass`, the host the tracer's selftest runs in) it
///   paints. The viewport uses that call; this trait method stays as the
///   formal seam until lane F explains the difference. F owns every accumulation/tonemap texture; B owns its
///   color/depth targets. GPU handles stay inside each widget, never in
///   `AppState`.
/// * **Camera parity (gate):** F renders with `state.views[view].camera
///   .view_projection(aspect)` where `aspect = (rect.size.x /
///   rect.size.y.max(1.0)) as f32` — bit-identical to [`ViewProjector::new`]
///   — so Rendered framing == Solid framing pixel-for-pixel.
/// * **`render_dirty` ownership:** everyone else *sets*
///   `views[view].render_dirty` (via `mark_camera_changed` / `apply_core`);
///   **only F clears it**, in `draw`, the same frame it restarts
///   accumulation.
/// * **Data:** the `SceneSnapshot` is uploaded once per
///   `SceneSnapshot::generation`, never per frame. Sun from
///   `state.sun.direction()`, settings from `state.render`, DOF from the
///   view camera's `f_stop`/`focus_distance`.
/// * **Fallback:** `None` (no snapshot yet, zero-sized rect) → the viewport
///   composites its own raster target as `Shading::Realtime`.
pub trait RenderedPreviewApi {
    fn draw(
        &mut self,
        cx: &mut Cx2d,
        state: &mut AppState,
        view: usize,
        rect: Rect,
        parent_pass: &DrawPass,
    ) -> Option<RenderedFrame>;

    /// Exact engine-sky transcription currently accumulated by the tracer.
    fn sky_params(&self) -> Option<makepad_raytrace::sky::SkyUniforms>;
}

// ===========================================================================
// Viewport ↔ navigation / tools
// ===========================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
}

/// Input as the viewport widget delivers it to the navigator and the tools.
/// Positions are window points; `hit` is the pick under the pointer for
/// `PointerDown`/`PointerMove` when the viewport computed one.
#[derive(Clone, Copy, Debug)]
pub enum ViewportInputKind {
    PointerDown {
        button: PointerButton,
        pos: DVec2,
        mods: KeyModifiers,
        tap_count: u32,
    },
    PointerMove {
        pos: DVec2,
        /// Delta since the previous move, in points.
        delta: DVec2,
        /// Relative motion while the pointer is locked (macOS); zero otherwise.
        lock_delta: DVec2,
        mods: KeyModifiers,
        buttons: u8,
    },
    PointerUp {
        button: PointerButton,
        pos: DVec2,
        mods: KeyModifiers,
    },
    HoverOut,
    Scroll {
        delta: DVec2,
        pos: DVec2,
        mods: KeyModifiers,
    },
    KeyDown {
        key: KeyCode,
        mods: KeyModifiers,
        repeat: bool,
    },
    KeyUp {
        key: KeyCode,
        mods: KeyModifiers,
    },
    /// Once per drawn frame while the viewport is alive. `dt` in seconds.
    Frame {
        dt: f32,
        time: f64,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct ViewportInput {
    pub view: usize,
    pub rect: Rect,
    pub kind: ViewportInputKind,
    pub hit: Option<RayHit>,
}

/// What a controller wants the viewport to do after handling input.
#[derive(Clone, Copy, Debug, Default)]
pub struct InputResponse {
    /// The input was used; do not pass it on.
    pub consumed: bool,
    pub redraw: bool,
    /// `Some(true)` = lock the pointer (fly/walk look), `Some(false)` = release.
    pub lock_pointer: Option<bool>,
    pub cursor: Option<MouseCursor>,
    /// Keep receiving `Frame` inputs (inertia, walk physics, animation).
    pub wants_frames: bool,
}

impl InputResponse {
    pub fn consumed() -> Self {
        InputResponse {
            consumed: true,
            redraw: true,
            ..Default::default()
        }
    }
}

/// Lane C. One instance per viewport widget (holds inertia/animation state).
pub trait NavController {
    fn handle(&mut self, cx: &mut Cx, input: &ViewportInput, state: &mut AppState) -> InputResponse;

    /// Animate the view's camera to frame `bounds` (Home / period key).
    fn frame(&mut self, cx: &mut Cx, state: &mut AppState, view: usize, bounds: Aabb, animate: bool);

    /// Snap/animate to a preset (numpad views). Keeps the distance.
    fn preset(&mut self, cx: &mut Cx, state: &mut AppState, view: usize, preset: PresetView, animate: bool);

    /// Toggle or set orthographic while preserving apparent size.
    fn set_ortho(&mut self, cx: &mut Cx, state: &mut AppState, view: usize, ortho: bool);

    /// Orbit by a screen-space delta (the nav gizmo drags call this).
    fn orbit_by(&mut self, cx: &mut Cx, state: &mut AppState, view: usize, dx: f32, dy: f32);

    /// True while an animation/inertia is in flight (viewport keeps framing).
    fn is_animating(&self) -> bool;

    /// Put the view's camera on a tour track at time `t` (lane G's playback,
    /// called by the viewport every `Frame` while `state.tour.playing`).
    /// Default: sample + apply. Lane C may override to blend out of / into
    /// user navigation.
    fn follow_track(&mut self, _cx: &mut Cx, state: &mut AppState, view: usize, track: &CameraTrack, t: f32) {
        if let Some(key) = track.sample(t) {
            let vs = state.view_at_mut(view);
            CameraTrack::apply(&key, &mut vs.camera);
            vs.mark_camera_changed();
        }
    }
}

/// Lane E. One instance per viewport widget; persistent results live in
/// `AppState` (`measurements`, `scene_state.section`). Drawing of dimension
/// lines / section handles is done by lane E's own overlay widget
/// (`FabToolOverlay`) stacked over the viewport by lane D, reading
/// `AppState` and building a [`ViewProjector`] from the view's camera.
pub trait ToolController {
    fn handle(&mut self, cx: &mut Cx, input: &ViewportInput, state: &mut AppState) -> InputResponse;
}

// ===========================================================================
// Actions
// ===========================================================================

/// Everything that can happen. Emitted with `cx.action(ShellAction::X)` from
/// widgets or `Cx::post_action(ShellAction::X)` from the loader thread.
#[derive(Debug)]
pub enum ShellAction {
    // ---- files / loading (A + D) ----
    OpenFile(PathBuf),
    OpenDemo,
    LoadStarted(PathBuf),
    LoadProgress(LoadProgress),
    Loaded(Arc<Scene>),
    LoadFailed { path: PathBuf, error: String },

    // ---- selection / visibility (core) ----
    SelectOnly(ElementId),
    SelectToggle(ElementId),
    SelectAdd(ElementId),
    SelectSet(Vec<ElementId>),
    ClearSelection,
    HideSelected,
    UnhideAll,
    IsolateSelected,
    SetHidden(ElementId, bool),
    SetLayerHidden(LayerId, bool),
    SetStoryHidden(StoryId, bool),
    /// Scroll the outliner to an element and flash it (double-click in 3D).
    RevealInOutliner(ElementId),

    // ---- scene edits (E) ----
    SetSection(SectionState),
    SetExplode(ExplodeState),
    AddMeasurement(Measurement),
    ClearMeasurements,
    SetSnap(SnapOptions),
    SetSun(SunSettings),
    SetSunShadows(bool),
    /// Live material edit from the colour picker: the scene material's base
    /// colour. Bumps `Scene::generation`, so both viewports re-upload (the
    /// base colour is folded into the vertex tint at pack time) and the
    /// traced pane restarts its accumulation.
    SetMaterialBaseColor(MaterialId, [f32; 4]),
    /// Display unit + decimal places for every length/area label
    /// (`AppState::units`; lane E report R4). Never touches geometry.
    SetDisplayUnit(LengthUnit, u8),

    // ---- viewport (B/C) ----
    SetShading(usize, Shading),
    SetOverlays(usize, Overlays),
    ToggleXray(usize),
    SetNavMode(usize, NavMode),
    SetActiveView(usize),
    /// Handled by the viewport widget that owns the view (via its navigator).
    FrameAll(usize),
    FrameSelected(usize),
    /// Frame the current selection in every viewport (outliner double-click).
    FrameSelectedAll,
    PresetView(usize, PresetView),
    ToggleOrtho(usize),
    /// Screen-space orbit request (nav gizmo drag).
    OrbitBy(usize, f32, f32),
    /// Stats written by the viewport each frame (bare action path for tests).
    Stats(FrameStats),

    // ---- tools (E) ----
    SetTool(Tool),

    // ---- render (F) ----
    SetRenderSettings(RenderSettings),
    SetRenderedPaused(usize, bool),
    RenderStart,
    RenderStop,
    RenderProgress { samples: u32, elapsed_s: f32 },
    RenderFinished,
    ExportPng(PathBuf),
    ClickToFocus(usize, Vec3f),

    // ---- ui chrome (D) ----
    SetWorkspace(Workspace),
    ToggleSidebar,
    ToggleToolbar,
    SetSidebarTab(SidebarTab),
    SetPropertiesTab(PropertiesTab),
    SetOutlinerFilter(String),
    StatusHint(String),
    StatusMessage(String),
    SelectSheet(Option<SheetId>),
    TogglePerf,
    ToggleQuadView,
    ToggleLockViews,
    ToggleMaximizeArea,
    ToggleCommandPalette,
    ShowFileBrowser(bool),
    ShowKeymapHelp(bool),
    /// Run a named command from the F3 palette (`"view.frame_all"` etc.).
    Command(String),
    Quit,

    // ---- tours (G) ----
    /// Analyse the scene and generate every shot (lane G's worker).
    TourGenerate,
    /// The generated tracks (posted from lane G's worker).
    TourTracks(Vec<CameraTrack>),
    TourSelect(usize),
    TourPlay(bool),
    TourSeek(f32),
    TourStatus(String),
    /// Hand the active track to lane F for an image sequence.
    TourRenderAnimation,
    // ---- LANE A EXTENSIONS (append below, never above) ----

    // ---- LANE B EXTENSIONS ----

    // ---- LANE C EXTENSIONS ----
    /// Tour-site entrance analysis produced alongside the scene on the loader
    /// worker. `apply_core` accepts it only for the installed generation.
    WalkAnalysisReady(Arc<WalkSceneAnalysis>),
    /// App-level key routed to the active viewport navigator while walking,
    /// independent of widget key focus.
    NavKey {
        view: usize,
        key: KeyCode,
        down: bool,
        mods: KeyModifiers,
        repeat: bool,
    },
    /// Give back any pointer owned by a first-person viewport, without
    /// necessarily leaving walk mode (modal/focus transitions use this).
    NavReleaseCapture,

    // ---- LANE D EXTENSIONS ----

    // ---- LANE E EXTENSIONS ----
    /// Sheet (and tool) hover: highlight this element in every viewport.
    /// `None` clears the hover. Written by `FabSheetView` on pointer move.
    HoverElement(Option<ElementId>),

    // ---- LANE F EXTENSIONS ----
}

/// Convenience for widgets: find every `ShellAction` in an actions list.
pub fn shell_actions(actions: &Actions) -> impl Iterator<Item = &ShellAction> {
    actions
        .iter()
        .filter_map(|a| a.downcast_ref::<ShellAction>())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Framing ignores a stray decode-artifact mesh: a few hundred vertices
    /// far below the site must not widen the box the camera frames.
    #[test]
    fn framing_bounds_ignore_an_outlier_mesh() {
        let mut scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let honest = framing_bounds(&scene);
        assert!(!aabb_is_empty(&honest));
        assert!(honest.min.z >= scene.bounds.min.z - 1.0 && honest.max.z <= scene.bounds.max.z + 1.0);
        // 3 % of the vertices mirrored to z = -134 (Woodside's skirt is 2.6 %).
        let stride = crate::model::VERTEX_STRIDE;
        let total: usize = scene.batches.iter().map(|b| b.vertices.len() / stride).sum();
        let n_out = (total * 3 / 100).max(8);
        let mut extra = Vec::with_capacity(n_out * stride);
        for i in 0..n_out {
            let mut v = vec![0.0f32; stride];
            v[0] = (i % 7) as f32;
            v[1] = (i % 5) as f32;
            v[2] = -134.0;
            extra.extend_from_slice(&v);
        }
        scene.batches[0].vertices.extend_from_slice(&extra);
        let framed = framing_bounds(&scene);
        assert!(framed.min.z > -20.0, "outlier leaked into the framing box: {:?}", framed);
        assert!((framed.min.z - honest.min.z).abs() < 2.0 && (framed.max.z - honest.max.z).abs() < 2.0);
    }

    #[test]
    fn camera_ray_hits_target() {
        let cam = Camera::default();
        let ray = cam.ray_at_ndc(vec2(0.0, 0.0), 1.5);
        let to_target = (cam.target - ray.origin).normalize();
        assert!(ray.dir.dot(to_target) > 0.999, "{:?} vs {:?}", ray.dir, to_target);
    }

    #[test]
    fn project_center_is_origin() {
        let cam = Camera::default();
        let ndc = cam.project(cam.target, 1.5).unwrap();
        assert!(ndc.x.abs() < 1e-4 && ndc.y.abs() < 1e-4);
        let proj = ViewProjector::new(cam, Rect { pos: dvec2(10.0, 20.0), size: dvec2(300.0, 200.0) });
        let p = proj.project(cam.target).unwrap();
        assert!((p.x - 160.0).abs() < 0.01 && (p.y - 120.0).abs() < 0.01);
    }

    #[test]
    fn frame_bounds_contains_sphere() {
        let mut cam = Camera::default();
        let b = Aabb { min: vec3(-5.0, -5.0, 0.0), max: vec3(5.0, 5.0, 6.0) };
        cam.frame_bounds(&b, 1.6);
        for i in 0..8 {
            let c = vec3(
                if i & 1 == 0 { b.min.x } else { b.max.x },
                if i & 2 == 0 { b.min.y } else { b.max.y },
                if i & 4 == 0 { b.min.z } else { b.max.z },
            );
            let ndc = cam.project(c, 1.6).unwrap();
            assert!(ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0, "{ndc:?}");
        }
    }

    #[test]
    fn sun_is_up_at_noon_in_summer() {
        let s = SkyState {
            time_local: 12.0,
            longitude: 0.0,
            tz_offset: 0.0,
            north_deg: 0.0,
            ..Default::default()
        };
        assert!(s.elevation_deg() > 50.0, "{}", s.elevation_deg());
        let d = s.direction();
        // Northern hemisphere: the sun is to the south (−Y) at noon.
        assert!(d.y < 0.0);
        let night = SkyState {
            time_local: 1.0,
            ..s
        };
        assert!(night.elevation_deg() < 0.0);
    }

    fn angular_error(a: f32, b: f32) -> f32 {
        ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs()
    }

    fn assert_noaa(case: &str, got: (f32, f32), expected: (f32, f32)) {
        assert!(
            (got.0 - expected.0).abs() <= 0.5,
            "{case}: elevation {got:?} expected {expected:?}"
        );
        assert!(
            angular_error(got.1, expected.1) <= 0.5,
            "{case}: azimuth {got:?} expected {expected:?}"
        );
    }

    #[test]
    fn noaa_reference_golden_colorado() {
        assert_noaa(
            "Golden 2003-10-17 12:30:30 MST",
            noaa_solar_position(
                SkyDate {
                    year: 2003,
                    month: 10,
                    day: 17,
                },
                12.0 + 30.5 / 60.0,
                -7.0,
                39.742476,
                -105.1786,
            ),
            (39.8884, 194.3402),
        );
    }

    #[test]
    fn noaa_reference_greenwich_equinox() {
        assert_noaa(
            "Greenwich 2024-03-20 12:00 UTC",
            noaa_solar_position(
                SkyDate {
                    year: 2024,
                    month: 3,
                    day: 20,
                },
                12.0,
                0.0,
                51.4779,
                0.0,
            ),
            (38.61, 177.65),
        );
    }

    #[test]
    fn site_metadata_applies_dst_and_longitude_fallback() {
        let mut sky = SkyState::default();
        let metadata = vec![
            ("arch.site.lat".into(), "47.579".into()),
            ("arch.site.lon".into(), "-122.241".into()),
            ("arch.north_deg".into(), "77.4".into()),
            ("arch.site.utc_offset_hours".into(), "-8".into()),
            ("arch.site.dst".into(), "true".into()),
            ("arch.site.date".into(), "2024-06-21".into()),
            ("arch.site.time".into(), "14:30".into()),
        ];
        assert!(sky.apply_site_metadata(&metadata));
        assert_eq!(sky.tz_offset, -7.0);
        assert_eq!(sky.date, SkyDate { year: 2024, month: 6, day: 21 });
        assert_eq!(sky.time_local, 14.5);

        let fallback = vec![
            ("arch.site.lat".into(), "47.579".into()),
            ("arch.site.lon".into(), "-122.241".into()),
        ];
        sky.apply_site_metadata(&fallback);
        assert_eq!(sky.tz_offset, -8.0);
    }

    #[test]
    fn ordered_selection_uses_the_clicked_rows_first_element_as_active() {
        let mut state = AppState::default();
        let first = ElementId::from_index(7);
        let second = ElementId::from_index(2);

        assert!(state.apply_core(&ShellAction::SelectSet(vec![first, second])));
        assert_eq!(state.scene_state.selection.active, Some(first));
        assert!(state.scene_state.selection.contains(first));
        assert!(state.scene_state.selection.contains(second));
    }

    #[test]
    fn rendered_preview_pause_badge_and_limit_are_stateful() {
        let mut state = AppState::default();
        assert_eq!(state.render.max_samples, 1024);
        assert_eq!(state.views[0].rendered_badge(), "converging · 0 spp");

        assert!(state.apply_core(&ShellAction::SetRenderedPaused(0, true)));
        state.views[0].rendered_samples = 37;
        assert_eq!(state.views[0].rendered_badge(), "stopped · 37 spp");

        assert!(state.apply_core(&ShellAction::SetRenderedPaused(0, false)));
        state.views[0].rendered_done = true;
        assert_eq!(state.views[0].rendered_badge(), "done · 37 spp");

        let mut settings = state.render;
        settings.max_samples = 1;
        assert!(state.apply_core(&ShellAction::SetRenderSettings(settings)));
        assert_eq!(state.render.max_samples, MIN_PREVIEW_SPP);
        settings.max_samples = u32::MAX;
        assert!(state.apply_core(&ShellAction::SetRenderSettings(settings)));
        assert_eq!(state.render.max_samples, MAX_PREVIEW_SPP);
    }

    #[test]
    fn changing_only_sample_limit_preserves_accumulation() {
        let mut state = AppState::default();
        state.views[0].render_dirty = false;
        let mut settings = state.render;
        settings.max_samples = 2048;
        assert!(state.apply_core(&ShellAction::SetRenderSettings(settings)));
        assert!(!state.views[0].render_dirty);

        settings.bounces += 1;
        assert!(state.apply_core(&ShellAction::SetRenderSettings(settings)));
        assert!(state.views[0].render_dirty);
    }

    #[test]
    fn sun_time_and_exposure_restart_both_tracers_but_haze_does_not() {
        let mut state = AppState::default();
        for view in &mut state.views {
            view.render_dirty = false;
        }
        let mut sun = state.sun;
        sun.haze += 0.05;
        assert!(state.apply_core(&ShellAction::SetSun(sun)));
        assert!(state.views.iter().all(|view| !view.render_dirty));

        sun.time_local += 0.25;
        assert!(state.apply_core(&ShellAction::SetSun(sun)));
        assert!(state.views.iter().all(|view| view.render_dirty));

        for view in &mut state.views {
            view.render_dirty = false;
        }
        sun.exposure_ev += 0.5;
        assert!(state.apply_core(&ShellAction::SetSun(sun)));
        assert!(state.views.iter().all(|view| view.render_dirty));
    }

    #[test]
    fn sun_study_sidebar_defaults_to_tools_and_remembers_session_choice() {
        let mut state = AppState::default();
        assert!(!state.ui.sidebar_open);
        assert_eq!(state.ui.sidebar_tab, SidebarTab::Item);

        assert!(state.apply_core(&ShellAction::SetWorkspace(Workspace::SunStudy)));
        assert!(state.ui.sidebar_open);
        assert_eq!(state.ui.sidebar_tab, SidebarTab::Tool);

        assert!(state.apply_core(&ShellAction::ToggleSidebar));
        assert!(!state.ui.sidebar_open);
        assert!(state.apply_core(&ShellAction::SetWorkspace(Workspace::Quad)));
        assert!(!state.ui.sidebar_open);
        assert_eq!(state.ui.sidebar_tab, SidebarTab::Item);

        assert!(state.apply_core(&ShellAction::SetWorkspace(Workspace::SunStudy)));
        assert!(!state.ui.sidebar_open);
        assert_eq!(state.ui.sidebar_tab, SidebarTab::Tool);
    }
}
