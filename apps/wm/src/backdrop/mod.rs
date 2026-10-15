//! 3D procedural backgrounds for the desktop: a full-screen shader quad in
//! the wallpaper layer, in place of the theme's gradient, selectable with
//! the background cycler (SUPER+CTRL+SPACE) after the image backgrounds.
//!
//! Cost rules: one pass, no textures, short loops; the background is a still
//! picture that costs nothing while nothing changes: it is drawn with the
//! desk when the zones, the focus, the theme or the size change,
//! and eased frames run only while the zones are easing.
use makepad_widgets::*;
use std::path::PathBuf;

mod shader;
mod widget;
pub use widget::WmBackdrop;

pub fn script_mod(vm: &mut ScriptVm) {
    shader::script_mod(vm);
    widget::script_mod(vm);
}

/// The backgrounds, in cycler order (the shader's `kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    WarpGrid,
    Starfield,
    Terrain,
    Crystal,
    Flow,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::WarpGrid, Kind::Starfield, Kind::Terrain, Kind::Crystal, Kind::Flow];

    pub fn name(self) -> &'static str {
        match self {
            Kind::WarpGrid => "warp-grid",
            Kind::Starfield => "starfield",
            Kind::Terrain => "terrain",
            Kind::Crystal => "crystal",
            Kind::Flow => "flow",
        }
    }

    pub fn from_name(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }

    /// Does the background react to the pointer (so it redraws when it moves)?
    pub fn follows_pointer(self) -> bool {
        self == Kind::Crystal
    }

    /// The direction the background's light comes from, toward the light,
    /// as a unit vector (x right, y up, -z away from the viewer). The
    /// shader lights its own surfaces with it, and a window skin that
    /// wants its highlights to agree reads the same one.
    pub fn light_dir(self) -> [f32; 3] {
        let v: [f32; 3] = match self {
            Kind::WarpGrid => [0.0, 0.5, 0.85],
            Kind::Starfield => [0.3, 0.6, -0.74],
            Kind::Terrain => [-0.45, 0.35, -0.82],
            Kind::Crystal => [0.0, 0.8, 0.6],
            Kind::Flow => [0.0, 0.3, 0.95],
        };
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / n, v[1] / n, v[2] / n]
    }
}

/// The background named by `MAKEPAD_WM_BACKDROP`, which picks one at start.
pub fn env_kind() -> Option<Kind> {
    std::env::var("MAKEPAD_WM_BACKDROP").ok().and_then(|n| Kind::from_name(&n))
}

/// Seconds the target outline takes to fade in and out.
const DRAG_FADE_SECONDS: f32 = 0.3;
/// Seconds the outline's rect takes to follow a changed landing rect.
const DRAG_FOLLOW_SECONDS: f32 = 0.12;
/// Seconds the grid takes to absorb a dropped window.
const ABSORB_SECONDS: f32 = 0.3;

/// One entry of the background cycle.
#[derive(Clone, Debug)]
pub enum Choice {
    /// The theme's plain gradient (only when the theme has no pictures).
    Plain,
    Image(PathBuf),
    Procedural(Kind),
}

/// The cycle: the theme's pictures, then the procedural backgrounds.
pub fn choices(images: Vec<PathBuf>) -> Vec<Choice> {
    let mut out: Vec<Choice> = Vec::new();
    if images.is_empty() {
        out.push(Choice::Plain);
    }
    out.extend(images.into_iter().map(Choice::Image));
    out.extend(Kind::ALL.into_iter().map(Choice::Procedural));
    out
}

/// The spacewarp zones the backgrounds bend with, in the desk's pixels: the
/// centre band `[center_l, center_r]`, the edge zones of width `zone_w`
/// either side, and the display's mid-height `mid_y` the windows scale about.
/// The spacewarp feeds `Env::warp`; the law itself is `spacewarp::Zones`,
/// which this reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarpUniforms {
    pub center_l: f32,
    pub center_r: f32,
    pub zone_w: f32,
    pub mid_y: f32,
    pub enabled: bool,
}

impl WarpUniforms {
    /// Spacewarp is off: the background is not bent.
    pub fn off() -> Self {
        Self { center_l: 0.0, center_r: 1.0, zone_w: 0.0, mid_y: 0.5, enabled: false }
    }

    /// The zones of a display, in the desk's pixels.
    pub fn from_zones(z: &crate::spacewarp::Zones) -> Self {
        Self {
            center_l: z.left as f32,
            center_r: z.right as f32,
            zone_w: z.zone_w() as f32,
            mid_y: (z.screen.y + z.screen.h * 0.5) as f32,
            enabled: true,
        }
    }

    /// The zones with the shelves `hide` (left, right: 0 shown, 1 away)
    /// slid out: the band edge moves out by that much of the zone's width,
    /// so at 1 the zone lies wholly past the display's edge and the grid is
    /// flat there. Driven by the shelves' own ease, so grid and shelf go
    /// together.
    pub fn hidden(&self, hide: [f32; 2]) -> Self {
        Self {
            center_l: self.center_l - hide[0].clamp(0.0, 1.0) * self.zone_w,
            center_r: self.center_r + hide[1].clamp(0.0, 1.0) * self.zone_w,
            ..*self
        }
    }

    /// No zones: one band as wide as the display, so the zones ease in from
    /// this and out to it (a flat desktop).
    pub fn flat(area: &crate::layout::LRect) -> Self {
        Self {
            center_l: area.x as f32,
            center_r: (area.x + area.w) as f32,
            zone_w: 0.0,
            mid_y: (area.y + area.h * 0.5) as f32,
            enabled: true,
        }
    }

    /// The default when the desk has said nothing: the zones the spacewarp
    /// picks for a screen of this size.
    pub fn default_for(width: f64, height: f64) -> Self {
        Self::from_zones(&crate::spacewarp::Zones::new(crate::layout::LRect::new(0.0, 0.0, width, height)))
    }

    /// The shader's `warp` uniform for the backdrop's `rect`: centre left,
    /// centre right, zone width (fractions of the width) and the mid-height
    /// (fraction of the height).
    pub fn in_rect(&self, rect: Rect) -> [f32; 4] {
        let (w, h) = (rect.size.x.max(1.0) as f32, rect.size.y.max(1.0) as f32);
        [
            (self.center_l - rect.pos.x as f32) / w,
            (self.center_r - rect.pos.x as f32) / w,
            self.zone_w / w,
            (self.mid_y - rect.pos.y as f32) / h,
        ]
    }

    /// The map the shader's `warp_map` runs, for the horizontal axis: the
    /// scale `s` at shown position `x` and its logical position `u`.
    pub fn map_x(&self, x: f32) -> (f32, f32) {
        if !self.enabled || self.zone_w <= 0.0 {
            return (1.0, x);
        }
        let zones = crate::spacewarp::Zones {
            screen: crate::layout::LRect::new((self.center_l - self.zone_w) as f64, 0.0, 1.0, 1.0),
            left: self.center_l as f64,
            right: self.center_r as f64,
        };
        (zones.scale_at(x as f64) as f32, zones.u_of_x(x as f64) as f32)
    }

    fn ease_to(&mut self, goal: &WarpUniforms, k: f32) -> bool {
        if self.enabled != goal.enabled {
            *self = *goal;
            return true;
        }
        let mut moving = false;
        for (v, g) in [
            (&mut self.center_l, goal.center_l),
            (&mut self.center_r, goal.center_r),
            (&mut self.zone_w, goal.zone_w),
            (&mut self.mid_y, goal.mid_y),
        ] {
            let d = g - *v;
            if d.abs() < 0.05 {
                *v = g;
            } else {
                *v += d * k;
                moving = true;
            }
        }
        moving
    }
}

/// Seconds the zones take to follow a change (an exponential ease, no overshoot, settled to the pixel by then).
const ZONE_SECONDS: f32 = 0.7;
/// What the desk tells the background each frame (`WmState::backdrop`).
#[derive(Default)]
pub struct Env {
    /// The spacewarp's zones; none means the default band.
    pub warp: Option<WarpUniforms>,
    /// The zones as drawn: eased toward `warp`.
    pub warp_shown: Option<WarpUniforms>,
    /// Where the dragged window will land, in the desk's pixels as
    /// `[x0, y0, x1, y1]` (none: no drag).
    drag_goal: Option<[f32; 4]>,
    /// The landing outline as drawn: eased toward the goal, held after the drop.
    pub drag_rect: [f32; 4],
    /// Its fade 0..1 and the drop's absorb 1..0.
    pub drag_level: f32,
    pub drag_absorb: f32,
    /// The desktop is Classic: the warp grid hands over to the plain
    /// background once the zones have flattened.
    pub classic: bool,
    /// The focus window's rect, in window coordinates.
    pub focus: Option<Rect>,
    /// How much of the display's left and right shelf is slid away (0..1):
    /// the warp zone goes with it.
    pub hide: [f32; 2],
    /// Something the background shows changed since it was last drawn.
    dirty: bool,
    /// The procedural background on show, for what follows its light.
    pub kind: Option<Kind>,
}

impl Env {
    /// The focus window's rect on the desk.
    /// The warp grid does not show it, so a moving window costs it no redraw.
    pub fn observe(&mut self, focus: Option<Rect>) {
        self.dirty |= self.focus != focus && self.kind != Some(Kind::WarpGrid);
        self.focus = focus;
    }

    /// Where the window being dragged would land if dropped now (its shown
    /// rect), or none when nothing is dragged. A drag that ends starts the
    /// absorb at the last rect.
    pub fn observe_drag(&mut self, landing: Option<Rect>) {
        if self.kind != Some(Kind::WarpGrid) {
            return;
        }
        let goal = landing.map(|r| [r.pos.x as f32, r.pos.y as f32, (r.pos.x + r.size.x) as f32, (r.pos.y + r.size.y) as f32]);
        match (self.drag_goal, goal) {
            (None, Some(g)) => {
                self.drag_rect = g;
                self.dirty = true;
            }
            (Some(_), None) => {
                self.drag_absorb = 1.0;
                self.dirty = true;
            }
            (a, b) => self.dirty |= a != b,
        }
        self.drag_goal = goal;
    }

    /// The shelves' hidden share changed (their auto-hide's ease).
    /// True when it did (the background must be redrawn).
    pub fn observe_hide(&mut self, hide: [f32; 2]) -> bool {
        let changed = self.hide != hide;
        self.dirty |= changed;
        self.hide = hide;
        changed
    }

    /// The spacewarp's zones for the display.
    pub fn set_warp(&mut self, warp: WarpUniforms) {
        self.dirty |= self.warp != Some(warp);
        self.warp = Some(warp);
    }

    /// True once, when a Classic desktop's zones have flattened out under
    /// the warp grid: the desk then swaps to the plain background.
    pub fn take_flattened(&mut self) -> bool {
        self.classic && self.kind == Some(Kind::WarpGrid) && self.warp.is_some() && !self.moving() && self.warp_shown == self.warp
    }

    /// Did anything the background shows change since this was last asked?
    /// (The desk then redraws it once.)
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Are the zones still easing (the desk runs frames for it)?
    pub fn moving(&self) -> bool {
        self.warp.is_some_and(|w| self.warp_shown != Some(w)) || self.drag_moving()
    }

    fn drag_moving(&self) -> bool {
        let goal = if self.drag_goal.is_some() { 1.0 } else { 0.0 };
        self.drag_goal.is_some_and(|g| g != self.drag_rect) || self.drag_level != goal || self.drag_absorb > 0.0
    }

    /// Advance the zones by `dt` seconds; true while they are still easing.
    pub fn step(&mut self, dt: f32) -> bool {
        let mut moving = false;
        {
            let goal = if self.drag_goal.is_some() { 1.0 } else { 0.0 };
            let kl = 1.0 - (-dt * 3.0 / DRAG_FADE_SECONDS).exp();
            let d = goal - self.drag_level;
            self.drag_level = if d.abs() < 0.004 { goal } else { self.drag_level + d * kl };
            if let Some(g) = self.drag_goal {
                let kf = 1.0 - (-dt * 3.0 / DRAG_FOLLOW_SECONDS).exp();
                for (v, t) in self.drag_rect.iter_mut().zip(g) {
                    *v = if (t - *v).abs() < 0.05 { t } else { *v + (t - *v) * kf };
                }
            }
            if self.drag_absorb > 0.0 {
                self.drag_absorb = (self.drag_absorb - dt / ABSORB_SECONDS).max(0.0);
            }
        }
        if let Some(goal) = self.warp {
            let kz = 1.0 - (-dt * 8.5 / ZONE_SECONDS).exp();
            match &mut self.warp_shown {
                Some(shown) => moving |= shown.ease_to(&goal, kz),
                None => self.warp_shown = Some(goal),
            }
        }
        moving | self.drag_moving()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide() -> WarpUniforms {
        WarpUniforms::default_for(3440.0, 1440.0)
    }

    #[test]
    fn a_hidden_shelf_slides_its_zone_out_with_it() {
        let w = WarpUniforms::from_zones(&crate::spacewarp::Zones::new(crate::layout::LRect::new(0.0, 0.0, 1920.0, 1080.0)));
        assert_eq!(w.hidden([0.0, 0.0]), w);
        let h = w.hidden([1.0, 0.0]);
        // Fully hidden on the left: the zone lies past the display's edge, the grid is flat in view.
        assert!((h.center_l - (w.center_l - w.zone_w)).abs() < 1e-3);
        assert!(h.center_l <= 0.5);
        assert_eq!(h.center_r, w.center_r, "the other side stays");
        let (s, u) = h.map_x(10.0);
        assert!((s - 1.0).abs() < 1e-3 && (u - 10.0).abs() < 1e-2, "{s} {u}");
        // Half way: half the zone's width has gone.
        assert!((w.hidden([0.5, 0.5]).center_l - (w.center_l - w.zone_w * 0.5)).abs() < 1e-3);
    }

    #[test]
    fn default_band_comes_from_the_spacewarp() {
        let w = wide();
        assert!(w.enabled);
        let z = crate::spacewarp::Zones::new(crate::layout::LRect::new(0.0, 0.0, 3440.0, 1440.0));
        assert!((w.center_l - z.left as f32).abs() < 1e-3);
        assert!((w.center_r - w.center_l - 2160.0).abs() < 1e-2);
        assert!((w.mid_y - 720.0).abs() < 1e-3);
        // A 16:9 screen has zones too.
        assert!(WarpUniforms::default_for(1920.0, 1080.0).enabled);
        assert!(!WarpUniforms::off().enabled);
    }

    #[test]
    fn logical_position_follows_the_scale() {
        // dx/du = s everywhere: the grid shows the warp the windows follow.
        let w = wide();
        for i in 1..200 {
            let x = i as f32 / 200.0 * 3440.0;
            let (s, _) = w.map_x(x);
            if s < 0.1 { continue; }
            let h = 0.5;
            let (_, u1) = w.map_x(x + h);
            let (_, u0) = w.map_x(x - h);
            let dxdu = 2.0 * h / (u1 - u0);
            assert!((dxdu - s).abs() < 5e-2, "x {x}: dx/du {dxdu} vs s {s}");
        }
        let (s, u) = w.map_x(0.0);
        assert!(s.abs() < 2e-4 && u < -1.0);
        let (s, u) = w.map_x(1720.0);
        assert!(s == 1.0 && u == 1720.0);
    }

    #[test]
    fn the_shader_law_matches_the_spacewarp_law() {
        // The shader's warp_map in f32: logical x = cl + Z ln(1 - f).
        let w = WarpUniforms::default_for(1920.0, 1080.0);
        let z = crate::spacewarp::Zones::new(crate::layout::LRect::new(0.0, 0.0, 1920.0, 1080.0));
        let f = w.in_rect(Rect { pos: dvec2(0.0, 0.0), size: dvec2(1920.0, 1080.0) });
        for i in 1..100 {
            let x = i as f32 / 100.0;
            let (cl, cr, zw) = (f[0], f[1], f[2]);
            let ux = if x < cl {
                { let f = ((cl - x) / zw).min(0.998); cl - zw * (0.2222222 * ((1.0 + 2.0 * f) / (1.0 - f)).ln() + f / (3.0 * (1.0 - f))) }
            } else if x > cr {
                { let f = ((x - cr) / zw).min(0.998); cr + zw * (0.2222222 * ((1.0 + 2.0 * f) / (1.0 - f)).ln() + f / (3.0 * (1.0 - f))) }
            } else {
                x
            };
            let want = z.u_of_x(x as f64 * 1920.0) as f32 / 1920.0;
            if (x as f64 * 1920.0 - z.left).abs() < 1.0 { continue; }
            assert!((ux - want).abs() < 2e-3 || ((cl - x) / zw > 0.99 || (x - cr) / zw > 0.99), "x {x}: {ux} vs {want}");
        }
        assert!((f[3] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_still_desk_draws_nothing_more_and_runs_no_frames() {
        let mut e = Env::default();
        let w = WarpUniforms::default_for(1920.0, 1080.0);
        e.set_warp(w);
        assert!(e.take_dirty() && e.moving(), "the first zones are news");
        while e.step(0.033) {}
        assert!(!e.moving());
        // The same facts again are not news.
        e.set_warp(w);
        e.observe(None);
        assert!(!e.take_dirty() && !e.moving());
        let r = Rect { pos: dvec2(10.0, 10.0), size: dvec2(100.0, 80.0) };
        e.observe(Some(r));
        assert!(e.take_dirty() && !e.moving(), "a focus change redraws once, with no frames");
    }

    #[test]
    fn the_outline_follows_the_drag_absorbs_the_drop_and_then_rests() {
        let mut e = Env::default();
        e.kind = Some(Kind::WarpGrid);
        assert!(!e.moving());
        let r = Rect { pos: dvec2(100.0, 100.0), size: dvec2(200.0, 150.0) };
        e.observe_drag(Some(r));
        assert!(e.take_dirty() && e.moving());
        for _ in 0..30 {
            e.step(0.033);
        }
        assert!(e.drag_level > 0.9 && e.drag_rect == [100.0, 100.0, 300.0, 250.0]);
        // A new landing rect is followed, not jumped to.
        e.observe_drag(Some(Rect { pos: dvec2(400.0, 100.0), size: dvec2(200.0, 150.0) }));
        e.step(0.016);
        assert!(e.drag_rect[0] > 100.0 && e.drag_rect[0] < 400.0);
        for _ in 0..30 {
            e.step(0.033);
        }
        assert_eq!(e.drag_rect[0], 400.0);
        // A held drag whose cue has settled draws no more frames.
        assert!(!e.moving());
        // The drop: the absorb runs ~0.35 s from the last rect, then no frames.
        e.observe_drag(None);
        assert_eq!(e.drag_absorb, 1.0);
        let mut t = 0.0;
        while e.step(0.016) {
            t += 0.016;
            assert!(t < 2.0);
        }
        assert!(t > 0.28 && e.drag_level == 0.0 && e.drag_absorb == 0.0 && !e.moving());
        assert_eq!(e.drag_rect[0], 400.0, "the outline stays where it landed");
    }

    #[test]
    fn zones_ease_when_they_change() {
        let mut e = Env::default();
        e.warp = Some(WarpUniforms::default_for(1920.0, 1080.0));
        e.step(0.033);
        assert_eq!(e.warp_shown, e.warp);
        e.warp = Some(WarpUniforms::default_for(2560.0, 1600.0));
        assert!(e.step(0.033));
        assert_ne!(e.warp_shown, e.warp);
        let mut n = 0;
        while e.step(0.033) { n += 1; assert!(n < 100); }
        assert_eq!(e.warp_shown, e.warp);
    }

    #[test]
    fn the_cycle_lists_pictures_then_the_procedural_ones() {
        let c = choices(vec![PathBuf::from("a.png")]);
        assert!(matches!(c[0], Choice::Image(_)) && matches!(c[1], Choice::Procedural(Kind::WarpGrid)) && c.len() == 6);
        assert!(matches!(choices(Vec::new())[0], Choice::Plain));
        for k in Kind::ALL {
            assert_eq!(Kind::from_name(k.name()), Some(k));
            let l = k.light_dir();
            assert!((l[0] * l[0] + l[1] * l[1] + l[2] * l[2] - 1.0).abs() < 1e-5);
        }
    }
}
