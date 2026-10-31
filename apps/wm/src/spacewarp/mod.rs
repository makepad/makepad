//! Spacewarp: on every display the edges are parking zones. A
//! window dragged into a zone becomes smaller with its distance into it and
//! is tidied into columns; the centre band stays 1:1.
//!
//! One model, two projections. A window has a DESK rect (the size its
//! client renders at; the client is never resized while parked) and a
//! [`Pose`] derived from it by the pure fn [`Warp::pose`]. The flat desk
//! draws the pose's scaled rect (`panel.rs`), the glasses place the same pose
//! ([`vr_placement`]). Everything here is pure geometry (unit tests below);
//! `input.rs` carries the pointer remap and the App gestures.
pub mod compact;
pub mod input;
pub mod panel;
pub mod shelf;

use crate::layout::LRect;

/// The smallest scale a window is drawn at.
pub const MIN_SCALE: f64 = 0.1;
/// The recede in the glasses at the smallest scale, as a share of the base depth.
pub const MAX_RECEDE: f32 = 0.06;
/// Space between parked windows and to the band.
pub const GAP: f64 = 10.0;
/// Below this scale a drop parks the window; above it the window is nudged
/// back into the band (a window barely in the zone would sit at an untidy
/// near-1:1 scale).
pub const PARK_BELOW: f64 = 0.95;
/// The logical strip of a zone is unbounded; positions past this many zone
/// widths are all "at the screen edge".
const MAX_V: f64 = 12.0;
/// The `t = d / Z` at logical distance `MAX_V` zone widths.
const T_MAX: f64 = 0.9705286535;

/// `v(t) / Z` of the warp law.
fn v_of_t(t: f64) -> f64 {
    (2.0 / 9.0) * ((1.0 + 2.0 * t) / (1.0 - t)).ln() + t / (3.0 * (1.0 - t))
}

/// The warp law, owned here and nowhere else (the backdrop's `warp_map`
/// shader mirrors it in GLSL-style code and its unit test checks the two
/// agree).
///
/// An edge zone of shown width `Z` is the compressed image of an infinite
/// logical strip. At shown distance `d = t Z` past the band edge the scale
/// is the smoothstep `s(t) = (1 - t)^2 (1 + 2t) = 1 - 3t^2 + 2t^3`: 1 at the
/// band edge with zero slope there (no corner where the band meets a zone,
/// so the grid and the windows bend smoothly), 0 at the screen edge with
/// zero slope too. Logical distance is `v = integral of 1/s`,
/// `v(t) / Z = (2/9) ln((1 + 2t) / (1 - t)) + t / (3 (1 - t))`, closed
/// form (the shader runs it as is); its inverse `d(v)` is found by
/// bisection here. Nothing is ever shown past the screen edge. The band is
/// the identity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zones {
    /// The display's tiling area.
    pub screen: LRect,
    /// The centre band `[left, right]`.
    pub left: f64,
    pub right: f64,
}

impl Zones {
    /// The centred focus band: 1.5 screen heights (a window of ordinary
    /// size fits it 1:1), between 50% and 72% of the width, so every display
    /// has zones: 14% each side on a 16:9 or 16:10 panel, 19% on an
    /// ultra-wide.
    pub fn new(screen: LRect) -> Zones {
        let band = (screen.h * 1.5).clamp(screen.w * 0.5, screen.w * 0.72);
        let left = screen.x + (screen.w - band) * 0.5;
        Zones { screen, left, right: left + band }
    }
    /// The width of one edge zone.
    pub fn zone_w(&self) -> f64 {
        self.left - self.screen.x
    }
    /// -1 left zone, 1 right zone, 0 band, and the distance into the zone.
    pub fn depth_at(&self, x: f64) -> (i8, f64) {
        if x < self.left {
            (-1, self.left - x)
        } else if x > self.right {
            (1, x - self.right)
        } else {
            (0, 0.0)
        }
    }
    /// The scale at a SHOWN x: the smoothstep of `d / Z`, 1 in the band.
    pub fn scale_at(&self, x: f64) -> f64 {
        let z = self.zone_w();
        let (_, d) = self.depth_at(x);
        if z <= 1e-9 || d <= 0.0 {
            return 1.0;
        }
        let t = (d / z).min(1.0);
        (1.0 - t) * (1.0 - t) * (1.0 + 2.0 * t)
    }
    /// Shown distance of a logical distance `v >= 0` (bisection on the
    /// monotone `v(t)`).
    fn d_of_v(&self, v: f64) -> f64 {
        let z = self.zone_w();
        let target = v.min(MAX_V * z) / z;
        let (mut lo, mut hi) = (0.0f64, T_MAX);
        for _ in 0..48 {
            let mid = 0.5 * (lo + hi);
            if v_of_t(mid) < target { lo = mid } else { hi = mid }
        }
        z * 0.5 * (lo + hi)
    }
    /// Logical distance of a shown distance `0 <= d < Z`.
    fn v_of_d(&self, d: f64) -> f64 {
        let z = self.zone_w();
        z * v_of_t((d / z).clamp(0.0, T_MAX))
    }
    /// Logical coordinate of a shown x (the layout space, the grid).
    pub fn u_of_x(&self, x: f64) -> f64 {
        let (side, d) = self.depth_at(x);
        if side == 0 || self.zone_w() <= 1e-9 {
            return x;
        }
        let v = self.v_of_d(d);
        if side < 0 { self.left - v } else { self.right + v }
    }
    /// Shown x of a logical coordinate: monotone, the identity in the band.
    pub fn x_of_u(&self, u: f64) -> f64 {
        let (side, v) = self.depth_at(u);
        if side == 0 || self.zone_w() <= 1e-9 {
            return u;
        }
        let d = self.d_of_v(v);
        if side < 0 { self.left - d } else { self.right + d }
    }
}

/// The centre band of a display's area, full height: where tiles, snapped
/// and maximized windows live and where new windows open. The edge zones
/// beside it belong to parked windows.
pub fn band_of(area: LRect) -> LRect {
    let z = Zones::new(area);
    LRect::new(z.left, area.y, z.right - z.left, area.h)
}

/// Where a window is shown, from its desk rect: an axis-aligned rect, only
/// scaled and moved (never turned or skewed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub shown: LRect,
    pub scale: f64,
    /// Recede in the glasses, as a share of the base depth (0..MAX_RECEDE).
    pub recede: f32,
    /// The zone it is parked in: -1 left, 1 right, 0 none.
    pub side: i8,
    /// A compact window shown 1:1 in its parked slot (`compact.rs`): not
    /// scaled, but not where its desk rect is either.
    pub compact: bool,
}

impl Pose {
    pub fn identity(desk: LRect) -> Pose {
        Pose { shown: desk, scale: 1.0, recede: 0.0, side: 0, compact: false }
    }
    pub fn warped(&self) -> bool {
        self.scale < 1.0
    }
    /// Drawn somewhere other than its desk rect: scaled, or compact.
    pub fn displaced(&self) -> bool {
        self.warped() || self.compact
    }
    pub fn side(&self) -> i8 {
        self.side
    }
    /// The drawn rect.
    pub fn bounds(&self) -> LRect {
        self.shown
    }
    /// Is the shown point inside the rect?
    pub fn contains(&self, px: f64, py: f64) -> bool {
        let s = self.shown;
        px >= s.x && px <= s.x + s.w && py >= s.y && py <= s.y + s.h
    }
    /// The desk point under a shown point (the map extends past the rect so
    /// hover-out still works).
    pub fn unmap(&self, desk: LRect, px: f64, py: f64) -> (f64, f64) {
        let u = (px - self.shown.x) / self.shown.w.max(1e-6);
        let v = (py - self.shown.y) / self.shown.h.max(1e-6);
        (desk.x + u * desk.w, desk.y + v * desk.h)
    }
}

/// The next parked window to bring to the centre: the one nearest the
/// centre (`parked`: id and distance from the centre) that has not had its
/// turn yet in `visited`; when all had one the round starts over.
pub fn next_parked(parked: &[(u64, f64)], visited: &mut Vec<u64>) -> Option<u64> {
    let pick = |skip: &[u64]| {
        parked.iter().filter(|(c, _)| !skip.contains(c)).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(c, _)| *c)
    };
    pick(visited).or_else(|| {
        visited.clear();
        pick(visited)
    })
}

/// How far `x` is inside the band edge that faces the zone `side` (negative
/// while it is still in the zone).
pub fn inward_of(z: &Zones, side: i8, x: f64) -> f64 {
    if side < 0 { x - z.left } else { z.right - x }
}

/// A parked window being pulled out of its zone: the full-size desk rect is
/// anchored under the pointer at the grab point, the window is SHOWN scaled
/// about that point (the grabbed pixel stays under the pointer) and grows to
/// full size over the first half zone width inside the band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leave {
    pub client: u64,
    pub side: i8,
    /// The scale when it began to leave (what it was shown at).
    pub s0: f64,
    /// How far inside the band edge the pointer was when the window left
    /// the shelf (negative while that was still in the zone): the growth
    /// starts from here.
    pub from: f64,
    /// The grab point as fractions of the window.
    pub fx: f64,
    pub fy: f64,
    /// The full desk size.
    pub size: (f64, f64),
    /// The pointer.
    pub cursor: (f64, f64),
}

impl Leave {
    /// 0..1: how far the window has grown, by the pointer's depth in the
    /// band, eased like the warp law (smoothstep, no corner at either end).
    pub fn growth(&self, z: &Zones) -> f64 {
        let t = ((inward_of(z, self.side, self.cursor.0) - self.from) / (z.zone_w() * 1.5).max(300.0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
    pub fn scale(&self, z: &Zones) -> f64 {
        self.s0 + (1.0 - self.s0) * self.growth(z)
    }
    /// The desk rect: full size, the grab point under the pointer, inside
    /// the display; horizontally inside the band too (a window overlapping
    /// a zone would be warped again, so full size means inside the band).
    pub fn desk(&self, z: &Zones) -> LRect {
        let (w, h) = self.size;
        let (x, y) = (self.cursor.0 - self.fx * w, self.cursor.1 - self.fy * h);
        let s = z.screen;
        let (lo, hi) = (z.left.max(s.x), z.right.min(s.x + s.w));
        LRect::new(
            x.clamp(lo, (hi - w).max(lo)),
            y.clamp(s.y, (s.y + s.h - h).max(s.y)),
            w,
            h,
        )
    }
    /// Where it is shown: scaled about the pointer, kept on the display;
    /// the horizontal limit eases from the screen edge to the band edge as
    /// it grows, so at full size it is exactly the desk rect.
    pub fn shown(&self, z: &Zones) -> LRect {
        let k = self.scale(z);
        let g = self.growth(z);
        let (w, h) = (self.size.0 * k, self.size.1 * k);
        let s = z.screen;
        let x = self.cursor.0 - self.fx * w;
        let y = self.cursor.1 - self.fy * h;
        let lo = s.x + (z.left.max(s.x) - s.x) * g;
        let hi = s.x + s.w - (s.x + s.w - z.right.min(s.x + s.w)) * g;
        LRect::new(
            x.clamp(lo, (hi - w).max(lo)),
            y.clamp(s.y, (s.y + s.h - h).max(s.y)),
            w,
            h,
        )
    }
    /// Where a drop would land it, while it is still scaled: the full-size
    /// rect in the band, for the grid's quiet cue (the parking cue's mirror).
    /// Once it has grown to full size the window itself is the landing.
    pub fn landing(&self, z: &Zones) -> Option<LRect> {
        (self.growth(z) < 1.0 - 1e-3).then(|| self.desk(z))
    }
    /// Letting go: the rect it is shown at now. The desk starts the
    /// window's spring here and sends it to the full-size rect, so a release
    /// mid-growth eases instead of jumping.
    pub fn release_from(&self, z: &Zones) -> LRect {
        self.shown(z)
    }
    pub fn pose(&self, z: &Zones) -> Pose {
        let k = self.scale(z);
        if k >= 1.0 - 1e-9 {
            return Pose::identity(self.desk(z));
        }
        Pose { shown: self.shown(z), scale: k, recede: MAX_RECEDE * ((1.0 - k) / (1.0 - MIN_SCALE)) as f32, side: self.side, compact: false }
    }
}

/// A window being dragged toward an OPEN shelf shrinks continuously toward
/// the slot it would take, by the pointer's distance from the shelf (the same
/// smooth law as pulling out of it, mirrored): full size at `SPAN` away from
/// the shelf's inner edge, the slot's size at it, about the grab point so the
/// grab point stays under the pointer. A shelf that is closed (auto-hidden)
/// only opens when the pointer touches its screen edge; nothing changes size
/// until it is open. A release in the zone shelves the window at the size it
/// is shown at.
#[derive(Clone, Copy, Debug)]
pub struct Enter {
    pub client: u64,
    pub side: i8,
    /// The grab point as fractions of the window, and its full size.
    pub fx: f64,
    pub fy: f64,
    pub size: (f64, f64),
    pub cursor: (f64, f64),
    /// The scale of the slot a drop would give.
    pub s_slot: f64,
    /// 0 full size .. 1 the slot's size as SHOWN: it follows `target` (the
    /// pointer's distance law) on a short ease, so a drag that starts inside
    /// the zone, or a quick move, never steps the size.
    pub e: f64,
    pub target: f64,
    /// The velocity of `e` (a critically damped spring follows the target).
    pub v: f64,
}

pub const ENTER_TAU: f64 = 0.07;


/// `Enter::e` for a pointer `away` points beyond the shelf's inner edge, the
/// law spanning `span` points.
pub fn enter_progress(away: f64, span: f64) -> f64 {
    let t = (away / span.max(1.0)).clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    1.0 - s
}

impl Enter {
    pub fn new(client: u64, side: i8, fx: f64, fy: f64, size: (f64, f64), cursor: (f64, f64), s_slot: f64) -> Enter {
        Enter { client, side, fx, fy, size, cursor, s_slot, e: 0.0, target: 0.0, v: 0.0 }
    }
    /// Advance toward the target on the shared spring law (it keeps its
    /// velocity when the target moves, so nothing steps); true while moving.
    pub fn step(&mut self, dt: f64) -> bool {
        // Critically damped, softer than the windows' spring (OMEGA 17): the
        // first frames of the shrink are a few pixels, not a pop.
        const W: f64 = 11.0;
        if !self.busy() {
            return false;
        }
        // A long frame slows the shrink instead of lurching it.
        let dt = dt.clamp(0.0, 0.025);
        let d0 = self.e - self.target;
        let b = self.v + W * d0;
        let k = (-W * dt).exp();
        let d = (d0 + b * dt) * k;
        let v = (self.v - W * b * dt) * k;
        if d.abs() < 0.002 && v.abs() < 0.02 {
            self.e = self.target;
            self.v = 0.0;
            return false;
        }
        self.e = self.target + d;
        self.v = v;
        true
    }
    pub fn busy(&self) -> bool {
        self.e != self.target || self.v != 0.0
    }
    /// Folded back to full size and not wanted any more.
    pub fn gone(&self) -> bool {
        self.target <= 0.0 && self.e <= 0.0
    }
    pub fn e(&self) -> f64 {
        self.e.clamp(0.0, 1.0)
    }
    pub fn scale(&self) -> f64 {
        1.0 + (self.s_slot - 1.0) * self.e()
    }
    pub fn shown(&self, z: &Zones) -> LRect {
        let k = self.scale();
        let (w, h) = (self.size.0 * k, self.size.1 * k);
        let s = z.screen;
        let (ux, uy) = (self.cursor.0 - self.fx * w, self.cursor.1 - self.fy * h);
        let (cx, cy) = (ux.clamp(s.x, (s.x + s.w - w).max(s.x)), uy.clamp(s.y, (s.y + s.h - h).max(s.y)));
        // Kept on the display only as far as the preview has come: at its
        // start the window is where the plain drag has it (no step).
        let e = self.e();
        LRect::new(ux + (cx - ux) * e, uy + (cy - uy) * e, w, h)
    }
    pub fn pose(&self, z: &Zones) -> Pose {
        let k = self.scale();
        let shown = self.shown(z);
        if k >= 1.0 - 1e-9 {
            return Pose::identity(shown);
        }
        Pose { shown, scale: k, recede: MAX_RECEDE * ((1.0 - k) / (1.0 - MIN_SCALE)) as f32, side: self.side, compact: false }
    }
}

/// How the desktop behaves: `Warp` has the centre band and the side zones
/// (parking, the warped grid); `Classic` is a flat desktop of ordinary
/// floating windows with the edge snap zones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Warp,
    Classic,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Warp => "warp",
            Mode::Classic => "classic",
        }
    }
    pub fn from_name(name: &str) -> Option<Mode> {
        match name.trim() {
            "warp" => Some(Mode::Warp),
            "classic" => Some(Mode::Classic),
            _ => None,
        }
    }
}

/// Where a parked window's full-size rect goes when the desktop turns
/// Classic: its shown centre, pulled inside the display. Windows that are
/// not parked stay as they are.
pub fn restore_inside(z: &Zones, desk: LRect) -> LRect {
    let pose = pose_in(z, desk);
    if !pose.warped() {
        return desk;
    }
    let (cx, cy) = pose.shown.center();
    let s = z.screen;
    let (w, h) = (desk.w.min(s.w), desk.h.min(s.h));
    LRect::new(
        (cx - w * 0.5).clamp(s.x, s.x + s.w - w),
        (cy - h * 0.5).clamp(s.y, s.y + s.h - h),
        w,
        h,
    )
}

/// The per-display zones (every display has them).
#[derive(Clone, Debug, Default)]
pub struct Warp {
    /// Classic has no zones at all.
    pub mode: Mode,
    /// The desktop tiles (a tiling style): the shelves are off whatever `mode`, the
    /// user's shelf setting, says; it is left as it is, so leaving tiling restores it.
    pub tiling: bool,
    zones: Vec<Zones>,
    /// The parked window a drag is pulling out of its zone.
    pub leave: Option<Leave>,
    /// The shelf window unfolded to its half size, if one is.
    pub unfold: Option<shelf::Unfold>,
    /// The windows the layout has put on a shelf. Shelf membership is this
    /// explicit state and nothing else: a window whose rect merely reaches a
    /// side zone (a resize, a maximise, a wide window, a spring in flight) is
    /// a normal window drawn 1:1.
    pub parked: std::collections::HashSet<u64>,
    /// The dragged window's shelf preview while the pointer dwells at an edge.
    pub enter: Option<Enter>,
    /// The windows being dragged now: never shelf items, however small they are shown.
    pub dragged: Vec<u64>,
    /// The window whose drag has passed its threshold (a press that has not
    /// moved yet is not a drag: a tile pressed stays a tile).
    pub armed_drag: Option<u64>,
    /// Each app's half height, as the person last resized it (by app id).
    pub half_heights: std::collections::HashMap<String, f64>,
    /// A resize of the unfolded window in progress: the grab (corner or
    /// edge), the pointer and half height it began at.
    pub unfold_resize: Option<(bool, (f64, f64), f64)>,
    /// A plain press on a shelf item the WM keeps until it is known to be a
    /// click: the client and the press.
    pub shelf_press: Option<(u64, makepad_widgets::MouseDownEvent)>,
    /// Where that item was SHOWN (swollen by the fish-eye) when pressed: the
    /// grab point is a fraction of that rect, kept on the resting one.
    pub press_pose: Option<(u64, LRect)>,
    swollen: std::collections::HashMap<u64, Pose>,
    /// The rect (half size) the unfolded client is configured and drawn 1:1 at once open.
    half_rect: Option<(u64, LRect)>,
    /// How much of each display's left and right shelf its auto-hide hides
    /// (0..1), by the display's rect.
    pub shelf_hide: Vec<(LRect, [f64; 2])>,
    /// How far each display's shelves reach in from its edge, as of the last
    /// refresh (what the auto-hide keeps the pointer near).
    pub shelf_reach: Vec<(LRect, [f64; 2])>,
    /// The item being called out (client, seconds since it began).
    pub pulse: Option<(u64, f64)>,
    /// The parked windows already brought to the centre in this cycling round.
    pub cycled: Vec<u64>,
    /// Which parked clients were told, and which answered with a compact view.
    pub compact: compact::Compact,
    /// The frame a window adds around its client area (chrome insets), so a
    /// compact client area is exactly what the app asked for.
    pub frame: (f64, f64),
}

impl Warp {
    /// The zones of a display (none only for a degenerate area).
    pub fn zones_for(&self, area: LRect) -> Option<Zones> {
        let z = Zones::new(area);
        (self.shelves_on() && z.zone_w() >= 1.0).then_some(z)
    }
    /// Shelves (edge zones, parking): the user's setting, and never while tiling.
    pub fn shelves_on(&self) -> bool {
        self.mode == Mode::Warp && !self.tiling
    }
    /// Set the displays' areas (once per desk draw).
    pub fn set_areas(&mut self, areas: impl IntoIterator<Item = LRect>) {
        self.zones = areas.into_iter().filter_map(|a| self.zones_for(a)).collect();
    }
    pub fn active(&self) -> bool {
        !self.zones.is_empty()
    }
    /// The first display's zones (what the background bends with).
    pub fn first_zones(&self) -> Option<Zones> {
        self.zones.first().copied()
    }
    /// The display a window (by its LOGICAL rect) belongs to: the nearest
    /// by horizontal distance, then by vertical. A parked window's logical
    /// rect lies far past the screen edge, and (the zone being scaled about
    /// the mid-height) above or below it too, so containment is not asked.
    pub fn zones_of(&self, desk: LRect) -> Option<&Zones> {
        let (cx, cy) = desk.center();
        let dist = |z: &Zones| {
            let dx = (z.screen.x - cx).max(cx - (z.screen.x + z.screen.w)).max(0.0);
            let dy = (z.screen.y - cy).max(cy - (z.screen.y + z.screen.h)).max(0.0);
            (dx, dy)
        };
        self.zones.iter().min_by(|a, b| {
            let (da, db) = (dist(a), dist(b));
            da.0.total_cmp(&db.0).then(da.1.total_cmp(&db.1))
        })
    }
    /// The pose: one pure function of the desk rect and the display.
    pub fn pose(&self, desk: LRect) -> Pose {
        match self.zones_of(desk) {
            Some(z) => pose_in(z, desk),
            None => Pose::identity(desk),
        }
    }
    /// The pose of a client's window: its shelf pose, magnified by the
    /// fish-eye while the pointer is over the shelf.
    pub fn pose_for(&self, client: u64, desk: LRect) -> Pose {
        if let Some(p) = self.swollen.get(&client) {
            return *p;
        }
        if let Some(r) = self.compact_rect(client, desk) {
            return Pose::compact(r, self.base_pose(client, desk).side);
        }
        self.pose_drawn(client, desk)
    }
    pub fn pulse_start(&mut self, client: u64) {
        self.pulse = Some((client, 0.0));
    }
    /// Advance the call-out; true while it shows.
    pub fn pulse_step(&mut self, dt: f64) -> bool {
        if let Some((c, t)) = self.pulse {
            self.pulse = (t + dt < shelf::PULSE_SECS).then_some((c, t + dt));
        }
        self.pulse.is_some()
    }
    /// Is any shelf item drawn other than at rest (swollen or hidden)?
    pub fn shelf_moves(&self) -> bool {
        self.unfold.is_some() || self.shelf_hide.iter().any(|(_, h)| h.iter().any(|h| *h > 0.0))
    }
    /// The pose the fish-eye gives a shelf item now, if it shows.
    pub fn swollen_pose(&self, client: u64) -> Option<Pose> {
        self.swollen.get(&client).copied()
    }
    /// How far the shelves of the display holding `at` reach in.
    pub fn reach_at(&self, at: (f64, f64)) -> [f64; 2] {
        let inside = |r: &LRect| at.0 >= r.x && at.0 <= r.x + r.w && at.1 >= r.y && at.1 <= r.y + r.h;
        self.shelf_reach.iter().find(|(r, _)| inside(r)).map_or([0.0; 2], |(_, v)| *v)
    }
    /// Set how much of the shelves of the display `screen` are hidden.
    pub fn set_hidden(&mut self, screen: LRect, hidden: [f64; 2]) {
        match self.shelf_hide.iter_mut().find(|(r, _)| *r == screen) {
            Some(e) => e.1 = hidden,
            None => self.shelf_hide.push((screen, hidden)),
        }
    }
    /// A drag armed: the unfold is dropped at once (the window leaves).
    pub fn swell_clear(&mut self) {
        self.unfold = None;
        self.unfold_resize = None;
        self.swollen.clear();
    }
    /// Is `client` the window unfolded and taking input?
    pub fn is_unfolded(&self, client: u64) -> bool {
        self.unfold.is_some_and(|u| u.client == client && u.open())
    }
    /// The unfolded window's current shown rect.
    pub fn unfolded_rect(&self) -> Option<(u64, LRect)> {
        let u = self.unfold.filter(|u| u.open())?;
        self.swollen.get(&u.client).map(|p| (u.client, p.shown))
    }
    /// The unfolded client's half-size rect (its real window size there): what
    /// the client is configured at and its capture is made at, from the
    /// moment it starts to unfold until it has folded back.
    pub fn half_rect_of(&self, client: u64) -> Option<LRect> {
        self.half_rect.filter(|(c, _)| *c == client).map(|(_, r)| r)
    }
    /// A drag's start point, moved from the fish-eye pose the press landed
    /// on to the same fraction of the resting rect `resting`.
    pub fn grab_start(&self, client: u64, start: (f64, f64), resting: LRect) -> (f64, f64) {
        match self.press_pose {
            Some((c, sh)) if c == client && sh.w > 1e-6 && sh.h > 1e-6 => {
                (resting.x + (start.0 - sh.x) / sh.w * resting.w, resting.y + (start.1 - sh.y) / sh.h * resting.h)
            }
            _ => start,
        }
    }
    /// Recompute the shelf poses (the unfold's push, the auto-hide shift of
    /// every display) from the items' resting poses: `slots` are every
    /// shelf item with its display. `desk_aspect` gives a client's window
    /// aspect (w / h) and full height, for its half size.
    pub fn swell_refresh(&mut self, slots: &[(u64, Pose, LRect)], desk_of: &dyn Fn(u64) -> Option<(f64, f64)>) {
        self.swollen.clear();
        self.half_rect = None;
        self.shelf_reach.clear();
        let slots: Vec<(u64, Pose, LRect)> = slots.iter().filter(|s| !self.dragged.contains(&s.0)).copied().collect();
        let slots = &slots[..];
        if self.unfold.is_some_and(|u| !slots.iter().any(|s| s.0 == u.client)) {
            self.unfold = None;
        }
        let mut screens: Vec<LRect> = Vec::new();
        for (_, _, s) in slots {
            if !screens.contains(s) {
                screens.push(*s);
            }
        }
        for screen in screens {
            let mine: Vec<(u64, Pose)> = slots.iter().filter(|s| s.2 == screen).map(|s| (s.0, s.1)).collect();
            let reach = |side: i8, items: &[(u64, Pose)]| {
                let edge = shelf::inner_edge(side, items.iter().filter(|(_, p)| p.side == side).map(|(_, p)| p.shown));
                edge.map_or(0.0, |e| if side < 0 { e - screen.x } else { screen.x + screen.w - e } + GAP)
            };
            let hide = self.shelf_hide.iter().find(|(r, _)| *r == screen).map_or([0.0; 2], |(_, h)| *h);
            let mut laid = mine.clone();
            if let Some(u) = self.unfold.filter(|u| u.screen == screen) {
                if let (Some(tile), Some((w, full_h))) = (mine.iter().find(|(c, _)| *c == u.client), desk_of(u.client)) {
                    let aspect = w / full_h.max(1.0);
                    let avail = LRect::new(screen.x, screen.y, screen.w, (screen.h - u.reserve).max(1.0));
                    let half = shelf::half_size(aspect, tile.1.shown.h, Some(u.half_h), full_h, avail);
                    laid = shelf::unfold_layout(&mine, u.client, half, u.t(), avail);
                    if let Some((_, r)) = shelf::unfold_layout(&mine, u.client, half, 1.0, avail).into_iter().find(|(c, _)| *c == u.client) {
                        self.half_rect = Some((u.client, r.shown));
                        // Drawn from a capture at the half size: scale 1 at rest.
                        if let Some((_, p)) = laid.iter_mut().find(|(c, _)| *c == u.client) {
                            p.scale = (p.shown.w / r.shown.w.max(1.0)).min(1.0);
                            p.compact = p.scale >= 0.999;
                        }
                    }
                }
            }
            self.shelf_reach.push((screen, [reach(-1, &mine), reach(1, &mine)]));
            let reach = self.shelf_reach.last().map_or([0.0; 2], |(_, r)| *r);
            if self.unfold.is_none() && hide.iter().all(|h| *h <= 0.0) {
                continue;
            }
            for (id, mut p) in laid {
                let k = (p.side > 0) as usize;
                p.shown.x += shelf::hide_shift(p.side, hide[k], reach[k]);
                self.swollen.insert(id, p);
            }
        }
    }
    /// The pose before the compact and unfold overrides: the growing pose of a
    /// window being pulled out of its zone, else its parked pose.
    fn base_pose(&self, client: u64, desk: LRect) -> Pose {
        self.pose_drawn(client, desk)
    }
    /// The pose a client's window is drawn with before the compact and unfold
    /// overrides: its parked pose, or the growing pose of a window being
    /// pulled out of its zone.
    pub fn pose_drawn(&self, client: u64, desk: LRect) -> Pose {
        match (self.leave.filter(|l| l.client == client), self.zones_of(desk)) {
            (Some(l), Some(z)) => return l.pose(z),
            _ => {}
        }
        // A window being dragged is a normal window wherever it is, until
        // the pointer reaches an open shelf (its shelf preview).
        if self.dragged.contains(&client) {
            return match (self.enter.filter(|e| e.client == client), self.zones_of(desk)) {
                (Some(e), Some(z)) => e.pose(z),
                _ => Pose::identity(desk),
            };
        }
        if !self.parked.contains(&client) {
            return Pose::identity(desk);
        }
        self.pose(desk)
    }
    /// The pose of a makepad rect.
    pub fn pose_rect(&self, r: makepad_widgets::Rect) -> Pose {
        self.pose(LRect::new(r.pos.x, r.pos.y, r.size.x, r.size.y))
    }
    /// The slot a parked client's window packs at while it is compact: its
    /// whole window (client area, frame and handle).
    pub fn compact_slot(&self, client: u64, _z: &Zones) -> Option<(f64, f64)> {
        Some(compact::slot_size(self.compact.size(client)?, self.frame))
    }
    /// The rect a compact surface is drawn at (1:1, centred on the parked
    /// slot, inside its zone), from the swap to the unpark's swap.
    pub fn compact_rect(&self, client: u64, desk: LRect) -> Option<LRect> {
        self.compact_rect_of(client, desk, self.compact.drawn_size(client))
    }
    /// The rect the client area is CONFIGURED for while compact (from the
    /// answer on): what the app renders at.
    pub fn compact_config_rect(&self, client: u64, desk: LRect) -> Option<LRect> {
        self.compact_rect_of(client, desk, self.compact.size(client))
    }
    fn compact_rect_of(&self, client: u64, desk: LRect, size: Option<(f64, f64)>) -> Option<LRect> {
        let z = self.zones_of(desk)?;
        let slot = compact::slot_size(size?, self.frame);
        let pose = self.base_pose(client, desk);
        let span = match pose.side {
            s if s < 0 => (z.screen.x, z.left),
            s if s > 0 => (z.right, z.screen.x + z.screen.w),
            _ => (z.screen.x, z.screen.x + z.screen.w),
        };
        pose.warped().then(|| compact::compact_rect(pose.shown, slot, span, z.screen))
    }
    /// Where a client's window is DRAWN (and hit): `on_screen`, or its
    /// compact rect.
    pub fn placed(&self, client: u64, desk: LRect) -> LRect {
        if let Some(r) = self.half_rect_of(client) {
            return r;
        }
        if let Some(r) = self.compact_rect(client, desk) {
            return r;
        }
        if self.leave.is_some_and(|l| l.client == client) {
            return desk;
        }
        self.on_screen(client, desk)
    }
    /// Where a parked window is DRAWN (and hit): its desk rect moved inside
    /// its display, so the capture holds all of it whatever part of the
    /// desk rect lies beyond the screen edge. The pose does not depend on
    /// it.
    pub fn on_screen(&self, client: u64, desk: LRect) -> LRect {
        let Some(z) = self.zones_of(desk).filter(|_| self.parked.contains(&client)) else { return desk };
        let s = z.screen;
        LRect::new(
            desk.x.clamp(s.x, (s.x + s.w - desk.w).max(s.x)),
            desk.y.clamp(s.y, (s.y + s.h - desk.h).max(s.y)),
            desk.w,
            desk.h,
        )
    }
}

/// The pose of a window whose LOGICAL desk rect is `desk`. Its shown span is
/// the image of its logical span under the warp, drawn at the uniform scale
/// `(X(x1) - X(x0)) / w`; vertically it is scaled by that same scale about
/// the display's mid-height, from its logical centre.
pub fn pose_in(z: &Zones, desk: LRect) -> Pose {
    // A window as wide as its display (fullscreen) is never warped.
    if desk.x <= z.screen.x + 1.0 && desk.x + desk.w >= z.screen.x + z.screen.w - 1.0 {
        return Pose::identity(desk);
    }
    let (a, b) = (z.x_of_u(desk.x), z.x_of_u(desk.x + desk.w));
    let s = ((b - a) / desk.w.max(1e-6)).clamp(MIN_SCALE, 1.0);
    if s >= 1.0 - 1e-12 {
        return Pose::identity(desk);
    }
    let mid = z.screen.y + z.screen.h * 0.5;
    let (w, h) = (desk.w * s, desk.h * s);
    let cy = mid + (desk.y + desk.h * 0.5 - mid) * s;
    // A window at the minimum scale can be wider than the sliver of the
    // zone its span maps to: keep it on the screen.
    let x = (0.5 * (a + b) - w * 0.5).clamp(z.screen.x, (z.screen.x + z.screen.w - w).max(z.screen.x));
    let (lcx, _) = desk.center();
    let mut side = z.depth_at(lcx).0;
    if side == 0 {
        side = if desk.x < z.left { -1 } else if desk.x + desk.w > z.right { 1 } else { 0 };
    }
    let t = (1.0 - s) / (1.0 - MIN_SCALE);
    Pose {
        shown: LRect::new(x, cy - h * 0.5, w, h),
        scale: s,
        recede: MAX_RECEDE * t as f32,
        side,
        compact: false,
    }
}

/// The desk rect of a window dragged by the pixel `start` (on its shown
/// rect at the drag start) to `cursor`: the grabbed pixel stays under the
/// cursor while the window's scale follows the warp.
pub fn drag_rect(z: &Zones, start_desk: LRect, start: (f64, f64), cursor: (f64, f64)) -> LRect {
    let p0 = pose_in(z, start_desk);
    let fx = ((start.0 - p0.shown.x) / p0.shown.w.max(1.0)).clamp(0.0, 1.0);
    let fy = ((start.1 - p0.shown.y) / p0.shown.h.max(1.0)).clamp(0.0, 1.0);
    // Plain 1:1 drag in the band: exactly the unwarped arithmetic.
    let plain = LRect::new(cursor.0 - fx * start_desk.w, cursor.1 - fy * start_desk.h, start_desk.w, start_desk.h);
    if !pose_in(z, plain).warped() {
        return LRect::new(plain.x, cursor.1 - fy * start_desk.h, plain.w, plain.h);
    }
    // The grabbed shown x is monotone in the window's logical x: bisect.
    let at = |x0: f64| {
        let p = pose_in(z, LRect::new(x0, start_desk.y, start_desk.w, start_desk.h));
        p.shown.x + fx * p.shown.w
    };
    let reach = MAX_V * z.zone_w() + start_desk.w;
    let (mut lo, mut hi) = (z.screen.x - reach, z.screen.x + z.screen.w + reach);
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        if at(mid) < cursor.0 { lo = mid } else { hi = mid }
    }
    let x0 = 0.5 * (lo + hi);
    let s = pose_in(z, LRect::new(x0, start_desk.y, start_desk.w, start_desk.h)).scale;
    // Vertical: shown y of the grabbed pixel = mid + (cy - mid) s + (fy - 1/2) h s.
    let mid = z.screen.y + z.screen.h * 0.5;
    let cy = mid + (cursor.1 - mid) / s - (fy - 0.5) * start_desk.h;
    LRect::new(x0, cy - start_desk.h * 0.5, start_desk.w, start_desk.h)
}

/// One window to tidy: its id and LOGICAL desk rect.
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub id: u64,
    pub rect: LRect,
    /// A compact window's shown size (`Warp::compact_slot`): it is packed at
    /// that size, 1:1, instead of the scaled desk rect.
    pub fixed: Option<(f64, f64)>,
    /// The window being dropped: the grab point as a fraction of its height
    /// and the pointer's y. Its slot keeps that point under the pointer
    /// (unless the column has no room there).
    pub anchor: Option<(f64, f64)>,
}

/// Pack the windows of one zone (`side` -1 or 1) into columns, inner column
/// first, top to bottom by SHOWN height. A column that would overflow the
/// display starts a new one further out (so outer columns are smaller, as
/// the warp makes them). A dragged window joins the column nearest its
/// drop; the others keep their columns. Returns each window's new logical
/// desk rect (full size).
pub fn tidy(z: &Zones, side: i8, items: &[Item], dragged: Option<u64>) -> Vec<(u64, LRect)> {
    let zw = z.zone_w();
    // Logical distance of a window's inner edge past the band edge, and the
    // shown distance of the same.
    let v_in = |r: &LRect| if side < 0 { z.left - (r.x + r.w) } else { r.x - z.right };
    let shown_in = |r: &LRect| z.d_of_v(v_in(r).max(0.0));
    let shown_mid = |r: &LRect| {
        let p = pose_in(z, *r);
        if side < 0 { z.left - (p.shown.x + p.shown.w * 0.5) } else { p.shown.x + p.shown.w * 0.5 - z.right }
    };
    let mut others: Vec<&Item> = items.iter().filter(|i| Some(i.id) != dragged).collect();
    others.sort_by(|a, b| v_in(&a.rect).total_cmp(&v_in(&b.rect)));
    let mut cols: Vec<Vec<Item>> = Vec::new();
    let mut col_d: Vec<f64> = Vec::new();
    for it in others {
        let d = shown_in(&it.rect);
        let width = it.fixed.map_or_else(|| pose_in(z, it.rect).shown.w, |f| f.0);
        match cols.last_mut() {
            Some(c) if d - col_d.last().copied().unwrap_or(0.0) <= width * 0.5 => c.push(*it),
            _ => {
                cols.push(vec![*it]);
                col_d.push(d);
            }
        }
    }
    if let Some(it) = items.iter().find(|i| Some(i.id) == dragged) {
        if cols.is_empty() {
            cols.push(vec![*it]);
        } else {
            let d = shown_mid(&it.rect);
            let mean = |c: &Vec<Item>| c.iter().map(|i| shown_mid(&i.rect)).sum::<f64>() / c.len() as f64;
            let best = (0..cols.len())
                .min_by(|a, b| (mean(&cols[*a]) - d).abs().total_cmp(&(mean(&cols[*b]) - d).abs()))
                .unwrap_or(0);
            cols[best].push(*it);
        }
    }
    let order: Vec<Item> = cols
        .into_iter()
        .flat_map(|mut c| {
            let cy = |i: &Item| pose_in(z, i.rect).shown.y;
            c.sort_by(|a, b| cy(a).total_cmp(&cy(b)));
            c
        })
        .collect();
    let avail = z.screen.h - 2.0 * GAP;
    let mid = z.screen.y + z.screen.h * 0.5;
    let mut out = Vec::new();
    let mut d_in = GAP;
    let mut i = 0;
    while i < order.len() {
        // The column's inner edge, logically; each member's scale follows.
        let v0 = z.v_of_d(d_in.min(zw * 0.97));
        let scale = |w: f64| (((z.d_of_v(v0 + w) - z.d_of_v(v0)) / w).clamp(MIN_SCALE, 1.0)).max(MIN_SCALE);
        let shown_h = |it: &Item| it.fixed.map_or(it.rect.h * scale(it.rect.w), |f| f.1);
        let mut end = i + 1;
        let mut y = shown_h(&order[i]);
        while end < order.len() {
            let hs = shown_h(&order[end]);
            if y + GAP + hs > avail { break }
            y += GAP + hs;
            end += 1;
        }
        // Each member keeps the height it is shown at (the dropped one the
        // height that keeps its grab point under the pointer); overlaps push
        // the members apart, and the column stays inside the display.
        let col = &order[i..end];
        let heights: Vec<f64> = col.iter().map(|it| shown_h(it)).collect();
        let wanted: Vec<f64> = col
            .iter()
            .zip(&heights)
            .map(|(it, &hs)| match it.anchor {
                Some((fy, py)) => py - fy * hs,
                None => {
                    let p = pose_in(z, it.rect);
                    p.shown.y + p.shown.h * 0.5 - hs * 0.5
                }
            })
            .collect();
        let tops = settle_column(&wanted, &heights, z.screen.y + GAP, z.screen.y + z.screen.h - GAP);
        let mut d_out = d_in;
        for ((it, &hs), &top) in col.iter().zip(&heights).zip(&tops) {
            let (w, h) = (it.rect.w, it.rect.h);
            let s = scale(w);
            if let Some((fw, _)) = it.fixed {
                // Packed 1:1: its shown centre goes in the middle of its slot.
                let sx = if side < 0 { z.left - d_in - fw * 0.5 } else { z.right + d_in + fw * 0.5 };
                out.push((it.id, desk_at_centre(z, w, h, sx, top + hs * 0.5)));
                d_out = d_out.max(d_in + fw);
                continue;
            }
            let cy_shown = top + hs * 0.5;
            let cy = mid + (cy_shown - mid) / s;
            let x = if side < 0 { z.left - v0 - w } else { z.right + v0 };
            out.push((it.id, LRect::new(x, cy - h * 0.5, w, h)));
            d_out = d_out.max(z.d_of_v(v0 + w));
        }
        d_in = d_out + GAP;
        i = end;
    }
    out
}

/// Tops for a column's members (in their order, sorted by `wanted` here):
/// each as close to its wanted top as possible, `GAP` apart, inside
/// `[top, bottom]` (members that do not fit run past the bottom).
fn settle_column(wanted: &[f64], heights: &[f64], top: f64, bottom: f64) -> Vec<f64> {
    let mut idx: Vec<usize> = (0..wanted.len()).collect();
    idx.sort_by(|a, b| wanted[*a].total_cmp(&wanted[*b]));
    let mut y = vec![0.0; wanted.len()];
    // Down: no overlap with the one above, not above the top.
    let mut floor = top;
    for &k in &idx {
        y[k] = wanted[k].max(floor);
        floor = y[k] + heights[k] + GAP;
    }
    // Up: not past the bottom, no overlap with the one below, not above the top.
    let mut ceil = bottom;
    for &k in idx.iter().rev() {
        y[k] = y[k].min(ceil - heights[k]).max(top);
        ceil = y[k] - GAP;
    }
    // A column taller than the display: back down from the top in order.
    let mut floor = top;
    for &k in &idx {
        y[k] = y[k].max(floor);
        floor = y[k] + heights[k] + GAP;
    }
    y
}

/// The full-size desk rect whose shown pose has its centre at `(sx, sy)`
/// (the grabbed-pixel bisection of `drag_rect`, for the centre).
pub fn desk_at_centre(z: &Zones, w: f64, h: f64, sx: f64, sy: f64) -> LRect {
    let at = |x0: f64| {
        let p = pose_in(z, LRect::new(x0, 0.0, w, h));
        p.shown.x + p.shown.w * 0.5
    };
    let reach = MAX_V * z.zone_w() + w;
    let (mut lo, mut hi) = (z.screen.x - reach, z.screen.x + z.screen.w + reach);
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        if at(mid) < sx { lo = mid } else { hi = mid }
    }
    let x0 = 0.5 * (lo + hi);
    let s = pose_in(z, LRect::new(x0, 0.0, w, h)).scale;
    let mid = z.screen.y + z.screen.h * 0.5;
    let cy = mid + (sy - mid) / s;
    LRect::new(x0, cy - h * 0.5, w, h)
}

/// The logical desk rect of a window of `size` whose parked pose has scale
/// `s` and its centre at height `sy` (the horizontal place follows the warp
/// law: size, not position, is what a drop onto the shelf keeps continuous).
pub fn desk_with_scale(z: &Zones, w: f64, h: f64, s: f64, sy: f64, side: i8) -> LRect {
    let (mut lo, mut hi) = (-w, MAX_V * z.zone_w());
    // The scale falls as the window sits further in the zone.
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        let x = if side < 0 { z.left - mid - w } else { z.right + mid };
        if pose_in(z, LRect::new(x, 0.0, w, h)).scale > s { lo = mid } else { hi = mid }
    }
    let v = 0.5 * (lo + hi);
    let x = if side < 0 { z.left - v - w } else { z.right + v };
    let mid = z.screen.y + z.screen.h * 0.5;
    let cy = mid + (sy - mid) / s.max(MIN_SCALE);
    LRect::new(x, cy - h * 0.5, w, h)
}

/// A desk rect of `size` whose centre sits at `(cx, cy)` pulled inside the
/// band: where a window returns to when it leaves a zone.
pub fn into_band(z: &Zones, w: f64, h: f64, cx: f64, cy: f64) -> LRect {
    let band = z.right - z.left;
    let x = if w >= band { (z.left + z.right) * 0.5 } else { cx.clamp(z.left + w * 0.5, z.right - w * 0.5) };
    let y = cy.clamp(z.screen.y + h.min(z.screen.h) * 0.5, z.screen.y + z.screen.h - h.min(z.screen.h) * 0.5);
    LRect::new(x - w * 0.5, y - h * 0.5, w, h)
}

/// A logical desk rect whose inner edge is the shown distance `d` past the
/// band edge on `side`, with its shown centre at height `shown_cy` (a first
/// slot for a window being parked).
pub fn slot_at(z: &Zones, side: i8, d: f64, w: f64, h: f64) -> LRect {
    let v0 = z.v_of_d(d.clamp(0.0, z.zone_w() * 0.97));
    let x = if side < 0 { z.left - v0 - w } else { z.right + v0 };
    LRect::new(x, z.screen.y + z.screen.h * 0.5 - h * 0.5, w, h)
}

/// Stereo: the pose's placement. `place(rect, depth_metres)` is
/// `DesktopView::place`; the pose's shown rect recedes by its share of the
/// base depth (no turn).
pub fn vr_placement(
    pose: &Pose,
    base_depth: f32,
    place: impl Fn(crate::layout::LRect, f32) -> crate::stereo::Placement,
) -> crate::stereo::Placement {
    place(pose.shown, base_depth * (1.0 + pose.recede))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_pull_out_cue_traces_the_band_landing_until_fully_grown() {
        let z = Zones::new(LRect::new(0.0, 0.0, 3440.0, 1440.0));
        let mut l = Leave { client: 1, side: -1, s0: 0.3, from: 6.0, fx: 0.5, fy: 0.5, size: (800.0, 600.0), cursor: (z.left + 10.0, 700.0) };
        let r = l.landing(&z).expect("still scaled");
        assert_eq!((r.w, r.h), (800.0, 600.0));
        assert!(r.x >= z.left, "lands inside the band");
        l.cursor.0 = z.left + 4.0 * z.zone_w();
        assert_eq!(l.landing(&z), None, "fully grown: the window is its own landing");
    }

    #[test]
    fn classic_brings_parked_windows_back_full_size_inside_the_screen() {
        let screen = LRect::new(0.0, 0.0, 1920.0, 1080.0);
        let z = Zones::new(screen);
        // A window parked deep in the right zone: its desk rect lies past the edge.
        let parked = slot_at(&z, 1, z.zone_w() * 0.6, 800.0, 600.0);
        assert!(pose_in(&z, parked).warped());
        let back = restore_inside(&z, parked);
        assert_eq!((back.w, back.h), (800.0, 600.0), "full size");
        assert!(back.x >= 0.0 && back.y >= 0.0 && back.x + back.w <= 1920.0 && back.y + back.h <= 1080.0, "{back:?}");
        // It lands where its slot is shown: on the right side of the display.
        assert!(back.center().0 > z.right - 400.0);
        // A window in the band stays put.
        let band = LRect::new(600.0, 200.0, 700.0, 500.0);
        assert_eq!(restore_inside(&z, band), band);
        // Wider than the display: clamped to it.
        let huge = restore_inside(&z, slot_at(&z, -1, z.zone_w() * 0.5, 3000.0, 600.0));
        assert!(huge.x >= 0.0 && huge.x + huge.w <= 1920.0);
    }

    /// Tiling turns the shelves off whatever the user's shelf setting is, and leaving
    /// tiling gives back exactly that setting: on comes back on, off stays off.
    #[test]
    fn tiling_forces_the_shelves_off_and_leaving_it_restores_the_setting() {
        let screen = LRect::new(0.0, 0.0, 1610.0, 980.0);
        for setting in [Mode::Warp, Mode::Classic] {
            let mut w = Warp::default();
            w.mode = setting;
            w.tiling = true;
            assert!(!w.shelves_on());
            assert!(w.zones_for(screen).is_none(), "{setting:?}: no zones while tiling");
            w.set_areas([screen]);
            assert!(!w.active(), "{setting:?}: no shelf drawn or parked while tiling");
            assert_eq!(w.mode, setting, "the setting is kept");
            w.tiling = false;
            w.set_areas([screen]);
            assert_eq!(w.shelves_on(), setting == Mode::Warp);
            assert_eq!(w.active(), setting == Mode::Warp, "{setting:?}: restored as set");
        }
    }

    #[test]
    fn classic_has_no_zones_and_the_mode_names_round_trip() {
        let screen = LRect::new(0.0, 0.0, 1920.0, 1080.0);
        let mut w = Warp::default();
        assert_eq!(w.mode, Mode::Warp);
        assert!(w.zones_for(screen).is_some());
        w.mode = Mode::Classic;
        assert!(w.zones_for(screen).is_none());
        w.set_areas([screen]);
        assert!(!w.active() && w.first_zones().is_none());
        for m in [Mode::Warp, Mode::Classic] {
            assert_eq!(Mode::from_name(m.name()), Some(m));
            assert_eq!(Mode::from_name(&format!("{}\n", m.name())), Some(m), "a state file ends in a newline");
        }
        assert_eq!(Mode::from_name("nonsense"), None);
        // A parked pose is identity without zones: the window is drawn as it is.
        assert!(!w.pose(LRect::new(-2000.0, 100.0, 800.0, 600.0)).warped());
    }

    use super::*;

    fn wide() -> Zones {
        Zones::new(LRect::new(0.0, 0.0, 3440.0, 1440.0))
    }

    #[test]
    fn band_and_zones() {
        let z = wide();
        assert!((z.right - z.left - 2160.0).abs() < 1e-9);
        assert!((z.zone_w() - 640.0).abs() < 1e-9);
        // A 16:9 and a 16:10 panel get proportional zones (14% each side).
        let hd = Zones::new(LRect::new(0.0, 0.0, 1920.0, 1080.0));
        assert!((hd.zone_w() / 1920.0 - 0.14).abs() < 1e-9);
        let laptop = Zones::new(LRect::new(0.0, 0.0, 1440.0, 900.0));
        assert!((laptop.zone_w() / 1440.0 - 0.14).abs() < 1e-9);
        assert_eq!(z.scale_at(1720.0), 1.0);
        assert!(z.scale_at(0.0).abs() < 1e-12);
        assert!((z.scale_at(z.left - 320.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn the_law_is_smoothstep_its_derivative_is_the_scale_and_it_inverts() {
        let z = wide();
        let zw = z.zone_w();
        for v in [0.0, 10.0, 100.0, 640.0, 2000.0] {
            let d = z.d_of_v(v);
            assert!((z.v_of_d(d) - v.min(MAX_V * zw)).abs() < 1e-6 * (1.0 + v) && (d - z.d_of_v(v)).abs() < 1e-9);
            assert!((z.v_of_d(d) - v).abs() < 1e-6 * (1.0 + v));
            // d'(v) = s(d / Z).
            let h = 1e-4;
            let slope = (z.d_of_v(v + h) - z.d_of_v((v - h).max(0.0))) / (if v > 0.0 { 2.0 * h } else { h });
            assert!((slope - z.scale_at(z.left - d)).abs() < 1e-6, "{v}: {slope}");
            // Both sides, and shown never reaches the screen edge.
            let xl = z.x_of_u(z.left - v);
            let xr = z.x_of_u(z.right + v);
            assert!(xl > z.screen.x && xr < z.screen.x + z.screen.w);
            assert!((z.u_of_x(xl) - (z.left - v)).abs() < 1e-5 * (1.0 + v));
            assert!((z.u_of_x(xr) - (z.right + v)).abs() < 1e-5 * (1.0 + v));
        }
        assert_eq!(z.x_of_u(1700.0), 1700.0);
        assert_eq!(z.u_of_x(1700.0), 1700.0);
        // Monotone over the whole line.
        let mut last = f64::MIN;
        for i in -400..=400 {
            let x = z.x_of_u(1720.0 + i as f64 * 20.0);
            assert!(x > last);
            last = x;
        }
    }

    #[test]
    fn the_scale_has_no_corner_at_the_band_edge() {
        let z = wide();
        // C1: the slope of the scale is 0 on both sides of the edge, and the
        // second difference stays small (no jump in direction).
        for side in [-1.0f64, 1.0] {
            let edge = if side < 0.0 { z.left } else { z.right };
            let s = |e: f64| z.scale_at(edge + side * e);
            let h = 0.5;
            assert!((s(h) - 1.0).abs() < 1e-5 && (s(2.0 * h) - 1.0).abs() < 4e-5);
            // The shown position is the identity's tangent: d x / d u = 1 at the edge.
            let (a, b) = (z.x_of_u(edge + side * 1e-3), z.x_of_u(edge));
            assert!(((a - b) / (side * 1e-3) - 1.0).abs() < 1e-6);
        }
        // The grid's y = mid + (y - mid) / s(x) is therefore smooth: its slope in x at the edge is 0.
        let y = |x: f64| 1.0 / z.scale_at(x);
        assert!((y(z.left - 1.0) - y(z.left)).abs() < 1e-5);
    }

    #[test]
    fn pose_is_identity_in_the_band_and_continuous_across_its_edge() {
        let z = wide();
        let desk = LRect::new(1000.0, 200.0, 800.0, 600.0);
        assert_eq!(pose_in(&z, desk), Pose::identity(desk));
        // A window sliding out of the band shrinks continuously.
        let at = |x: f64| pose_in(&z, LRect::new(x, 200.0, 800.0, 600.0));
        let edge = at(z.left);
        assert!(edge.scale == 1.0);
        let slight = at(z.left - 5.0);
        assert!(slight.scale < 1.0 && 1.0 - slight.scale < 0.01 && slight.side < 0);
        let mut prev = 1.0;
        for i in 0..60 {
            let p = at(z.left - 20.0 * i as f64);
            assert!(p.scale <= prev + 1e-12);
            prev = p.scale;
        }
        // Deep in the strip it is small, receded, uniformly scaled.
        let far = at(z.left - 3000.0);
        assert!(far.scale < 0.3 && far.scale >= MIN_SCALE && far.side < 0 && far.recede > 0.03);
        assert!((far.shown.w / 800.0 - far.scale).abs() < 1e-9 && (far.shown.h / 600.0 - far.scale).abs() < 1e-9);
        let right = pose_in(&z, LRect::new(z.right + 100.0, 200.0, 800.0, 600.0));
        assert!(right.side > 0);
        // Vertically: scaled about the display's mid-height from the logical centre.
        let tall = pose_in(&z, LRect::new(z.left - 900.0, 0.0, 800.0, 400.0));
        let mid = 720.0;
        assert!((tall.shown.y + tall.shown.h * 0.5 - (mid + (200.0 - mid) * tall.scale)).abs() < 1e-9);
    }

    #[test]
    fn no_window_ever_crosses_the_screen_edge() {
        let z = wide();
        for w in [200.0, 800.0, 2400.0] {
            for i in -200..400 {
                let x = z.screen.x - 8000.0 + i as f64 * 40.0;
                let p = pose_in(&z, LRect::new(x, 100.0, w, 500.0));
                assert!(p.shown.x >= z.screen.x - 1e-6 && p.shown.x + p.shown.w <= z.screen.x + z.screen.w + 1e-6, "x {x} w {w}: {:?}", p.shown);
                assert!(p.scale >= MIN_SCALE);
            }
        }
    }

    #[test]
    fn inverse_map_round_trips_the_scaled_rect() {
        let z = wide();
        let desk = LRect::new(z.left - 1400.0, 100.0, 1000.0, 700.0);
        let pose = pose_in(&z, desk);
        assert!(pose.warped());
        let (sx, sy, sw, sh) = (pose.shown.x, pose.shown.y, pose.shown.w, pose.shown.h);
        let near = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6;
        assert!(near(pose.unmap(desk, sx + sw * 0.5, sy + sh * 0.5), (desk.x + desk.w * 0.5, desk.y + desk.h * 0.5)));
        assert!(near(pose.unmap(desk, sx, sy), (desk.x, desk.y)));
        assert!(near(pose.unmap(desk, sx + sw, sy + sh), (desk.x + desk.w, desk.y + desk.h)));
        assert!(pose.contains(sx + sw * 0.5, sy + sh * 0.5));
        assert!(!pose.contains(sx - 1.0, sy + sh * 0.5));
        assert!(!pose.contains(sx + 1.0, sy + sh + 1.0));
        // Axis-aligned: equal horizontal and vertical scale, no skew.
        assert!((sw / desk.w - sh / desk.h).abs() < 1e-9);
    }

    #[test]
    fn drag_keeps_the_grabbed_pixel_under_the_cursor() {
        let z = wide();
        let start_rect = LRect::new(1000.0, 200.0, 800.0, 600.0);
        let start = (1200.0, 230.0);
        // In the band it is the plain 1:1 drag, exactly.
        let r = drag_rect(&z, start_rect, start, (1300.0, 330.0));
        assert_eq!((r.x, r.y), (1100.0, 300.0));
        let (fx, fy) = (0.25, 0.05);
        for cursor in [(500.0, 400.0), (200.0, 300.0), (30.0, 900.0), (3300.0, 500.0), (3350.0, 1300.0)] {
            let r = drag_rect(&z, start_rect, start, cursor);
            let p = pose_in(&z, r);
            let under = (p.shown.x + fx * p.shown.w, p.shown.y + fy * p.shown.h);
            assert!((under.0 - cursor.0).abs() < 1e-3 && (under.1 - cursor.1).abs() < 1e-3, "{cursor:?} {under:?}");
            assert_eq!((r.w, r.h), (800.0, 600.0));
        }
        // The scale follows the cursor continuously and shrinks outward.
        let s = |x: f64| pose_in(&z, drag_rect(&z, start_rect, start, (x, 400.0))).scale;
        assert!(s(500.0) > s(300.0) && s(300.0) > s(100.0) && s(100.0) >= MIN_SCALE);
        assert!((s(400.0) - s(401.0)).abs() < 0.01);
        // A drag that STARTS on a parked window grabs its shown pixel.
        let parked = drag_rect(&z, start_rect, start, (150.0, 400.0));
        let sp = pose_in(&z, parked);
        let grab = (sp.shown.x + sp.shown.w * 0.5, sp.shown.y + sp.shown.h * 0.5);
        let back = drag_rect(&z, parked, grab, (1700.0, 700.0));
        assert!((back.x + 400.0 - 1700.0).abs() < 1e-6 && (back.y + 300.0 - 700.0).abs() < 1e-6);
    }

    fn item(id: u64, cx: f64, cy: f64, w: f64, h: f64) -> Item {
        Item { id, rect: LRect::new(cx - w * 0.5, cy - h * 0.5, w, h), fixed: None, anchor: None }
    }

    fn overlaps(a: LRect, b: LRect) -> bool {
        !(a.x + a.w <= b.x + 1e-6 || b.x + b.w <= a.x + 1e-6 || a.y + a.h <= b.y + 1e-6 || b.y + b.h <= a.y + 1e-6)
    }

    #[test]
    fn tidy_packs_columns_without_overlap_and_keeps_the_desk_size() {
        let z = wide();
        // Five ordinary windows, two of them the big 994x638 kind.
        let sizes = [(994.0, 638.0), (994.0, 638.0), (700.0, 450.0), (600.0, 400.0), (800.0, 500.0)];
        for side in [-1i8, 1] {
            let items: Vec<Item> = sizes
                .iter()
                .enumerate()
                .map(|(i, (w, h))| item(i as u64, if side < 0 { 300.0 } else { 3100.0 }, 100.0 + i as f64 * 50.0, *w, *h))
                .collect();
            let out = tidy(&z, side, &items, None);
            assert_eq!(out.len(), items.len());
            let poses: Vec<Pose> = out.iter().map(|(_, r)| pose_in(&z, *r)).collect();
            for ((id, r), p) in out.iter().zip(&poses) {
                assert_eq!((r.w, r.h), (sizes[*id as usize].0, sizes[*id as usize].1));
                assert!(p.shown.y >= z.screen.y - 1e-6 && p.shown.y + p.shown.h <= z.screen.y + z.screen.h + 1e-6, "{p:?}");
                assert!(p.shown.x >= z.screen.x - 1e-6 && p.shown.x + p.shown.w <= z.screen.x + z.screen.w + 1e-6);
                // Inside its zone, clear of the band.
                if side < 0 { assert!(p.shown.x + p.shown.w <= z.left + 1e-6) } else { assert!(p.shown.x >= z.right - 1e-6) }
            }
            for i in 0..poses.len() {
                for j in i + 1..poses.len() {
                    assert!(!overlaps(poses[i].shown, poses[j].shown), "{:?} overlaps {:?}", poses[i].shown, poses[j].shown);
                }
            }
            // Every window is clear of the band by a gap.
            for p in poses.iter() {
                let inner = if side < 0 { z.left - (p.shown.x + p.shown.w) } else { p.shown.x - z.right };
                assert!(inner >= GAP - 1e-6, "{inner}");
            }
        }
    }

    #[test]
    fn tidy_is_stable_and_drops_into_the_nearest_column() {
        let z = wide();
        let items: Vec<Item> = (0..8).map(|i| item(i, 300.0, 100.0 + i as f64 * 100.0, 500.0, 360.0)).collect();
        let first = tidy(&z, -1, &items, None);
        // Feeding the result back changes nothing.
        let again: Vec<Item> = first.iter().map(|(id, r)| Item { id: *id, rect: *r, fixed: None, anchor: None }).collect();
        let second = tidy(&z, -1, &again, None);
        for ((a, ra), (b, rb)) in first.iter().zip(&second) {
            assert_eq!(a, b);
            assert!((ra.x - rb.x).abs() < 1e-6 && (ra.y - rb.y).abs() < 1e-6);
        }
        // A new window dropped at the very top joins the nearest column
        // and lands first in it.
        let mut with = again.clone();
        with.push(Item { id: 9, rect: LRect::new(z.left - 700.0, -3000.0, 500.0, 360.0), fixed: None, anchor: None });
        let out = tidy(&z, -1, &with, Some(9));
        assert_eq!(out.len(), 9);
        let nine = out.iter().find(|(id, _)| *id == 9).unwrap().1;
        let shown = pose_in(&z, nine).shown;
        assert!((shown.y - (z.screen.y + GAP)).abs() < 1e-6);
    }

    #[test]
    fn a_window_dropped_in_a_zone_keeps_its_grab_point_under_the_pointer() {
        let z = wide();
        // Two windows already parked in the left zone, high up.
        let parked = tidy(&z, -1, &[item(1, 300.0, 100.0, 500.0, 360.0), item(2, 300.0, 300.0, 500.0, 360.0)], None);
        let mut items: Vec<Item> = parked.iter().map(|(id, r)| Item { id: *id, rect: *r, fixed: None, anchor: None }).collect();
        for (fy, py) in [(0.0, 900.0), (0.25, 1000.0), (0.5, 1100.0), (0.9, 1200.0)] {
            let mut with = items.clone();
            with.push(Item { id: 9, rect: LRect::new(z.left - 650.0, py, 500.0, 360.0), fixed: None, anchor: Some((fy, py)) });
            let out = tidy(&z, -1, &with, Some(9));
            let shown = pose_in(&z, out.iter().find(|(id, _)| *id == 9).unwrap().1).shown;
            let grabbed = shown.y + fy * shown.h;
            assert!((grabbed - py).abs() < 1e-6, "fy {fy}: grabbed point at {grabbed}, pointer at {py}");
            // The others did not move.
            for (id, r) in &parked {
                let now = out.iter().find(|(o, _)| o == id).unwrap().1;
                assert!((now.y - r.y).abs() < 1e-6 && (now.x - r.x).abs() < 1e-6);
            }
        }
        // Dropped over a parked window: it lands next to it, never on top.
        items.push(Item { id: 9, rect: LRect::new(z.left - 650.0, 0.0, 500.0, 360.0), fixed: None, anchor: Some((0.5, parked[0].1.y)) });
        let out = tidy(&z, -1, &items, Some(9));
        let shown: Vec<LRect> = out.iter().map(|(_, r)| pose_in(&z, *r).shown).collect();
        for a in 0..shown.len() {
            for b in a + 1..shown.len() {
                let (p, q) = (shown[a], shown[b]);
                let apart = p.x + p.w <= q.x + 1e-6 || q.x + q.w <= p.x + 1e-6 || p.y + p.h <= q.y + 1e-6 || q.y + q.h <= p.y + 1e-6;
                assert!(apart, "{p:?} overlaps {q:?}");
            }
        }
    }

    #[test]
    fn leaving_the_zone_returns_to_the_band() {
        let z = wide();
        let r = into_band(&z, 800.0, 600.0, 10.0, 700.0);
        assert!(r.x >= z.left - 1e-9 && r.x + r.w <= z.right + 1e-9);
        assert_eq!(pose_in(&z, r).scale, 1.0);
    }

    #[test]
    fn stereo_only_recedes_a_parked_window() {
        use crate::stereo::Placement;
        let place = |r: LRect, depth: f32| {
            let cx = (r.x + r.w * 0.5 - 1720.0) as f32 * 0.001;
            Placement { center: [cx, 0.0, -depth], right: [r.w as f32 * 0.0005, 0.0, 0.0], up: [0.0, r.h as f32 * 0.0005, 0.0] }
        };
        let z = wide();
        let pose = pose_in(&z, LRect::new(z.left - 1500.0, 100.0, 800.0, 600.0));
        let placed = vr_placement(&pose, 2.0, place);
        let flat = place(pose.shown, 2.0);
        assert!(placed.center[2] < flat.center[2]);
        assert_eq!(placed.right, place(pose.shown, 3.0).right);
        let id = Pose::identity(LRect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(vr_placement(&id, 2.0, place), place(id.shown, 2.0));
    }

    #[test]
    fn a_screen_wide_window_is_not_warped() {
        let z = wide();
        assert!(!pose_in(&z, z.screen).warped());
        assert!(pose_in(&z, LRect::new(-3000.0, 0.0, 900.0, 600.0)).warped());
    }

    #[test]
    fn warp_is_on_for_a_laptop() {
        let laptop = LRect::new(0.0, 0.0, 1440.0, 900.0);
        let mut w = Warp::default();
        let z = w.zones_for(laptop).unwrap();
        assert!(z.zone_w() > 100.0 && z.zone_w() < 300.0);
        w.set_areas([laptop]);
        assert!(w.active());
        let desk = LRect::new(-100.0, 0.0, 500.0, 400.0);
        assert!(w.pose(desk).warped());
    }

    #[test]
    fn a_parked_windows_logical_rect_far_off_screen_still_finds_its_display() {
        let mut w = Warp::default();
        w.set_areas([LRect::new(0.0, 0.0, 3440.0, 1440.0)]);
        w.parked.insert(1);
        let far = LRect::new(-4000.0, 300.0, 900.0, 600.0);
        assert!(w.pose(far).warped());
        // Also far above the screen (the zone is scaled about the mid-height).
        assert!(w.pose(LRect::new(-1500.0, -3000.0, 900.0, 600.0)).warped());
        let drawn = w.on_screen(1, far);
        assert!(drawn.x >= 0.0 && drawn.x + drawn.w <= 3440.0 && (drawn.w, drawn.h) == (900.0, 600.0));
    }

    #[test]
    fn a_compact_window_is_drawn_one_to_one_in_its_slot_and_a_plain_one_stays_scaled() {
        let z = wide();
        let mut w = Warp::default();
        w.set_areas([z.screen]);
        w.frame = (6.0, 6.0);
        w.parked.insert(1);
        let parked = LRect::new(z.left - 1600.0, 300.0, 1000.0, 720.0);
        let scaled = w.pose_for(1, parked);
        assert!(scaled.warped() && !scaled.compact);
        w.compact.park_compact(1, vec![makepad_widgets::makepad_platform::ime::CompactForm::new("strip", 360.0, 64.0)], 0);
        let pose = w.pose_for(1, parked);
        assert!(pose.compact && !pose.warped() && pose.displaced());
        assert_eq!((pose.shown.w, pose.shown.h), (366.0, 70.0));
        // Centred on the slot, pushed in where the slot is nearer the edge than half of it.
        assert_eq!(pose.shown.center().1, scaled.shown.center().1);
        assert!(pose.shown.x >= z.screen.x);
        assert_eq!(w.placed(1, parked), pose.shown);
        // Another parked client at the same rect did not opt in.
        w.parked.insert(2);
        assert_eq!(w.pose_for(2, parked), scaled);
        assert_eq!(w.placed(2, parked), w.on_screen(2, parked));
        // In the band the opt-in does not apply: full size, full rect.
        let band = LRect::new(z.left + 100.0, 200.0, 900.0, 600.0);
        assert_eq!(w.pose_for(1, band), Pose::identity(band));
        assert_eq!(w.placed(1, band), band);
        // The pointer lands 1:1 inside the compact rect.
        let (x, y) = pose.unmap(w.placed(1, parked), pose.shown.x + 10.0, pose.shown.y + 20.0);
        assert_eq!((x, y), (pose.shown.x + 10.0, pose.shown.y + 20.0));
    }

    #[test]
    fn compact_windows_pack_at_their_own_size_inside_the_zone() {
        let z = Zones::new(LRect::new(0.0, 0.0, 3440.0, 1440.0));
        let mut w = Warp::default();
        w.set_areas([z.screen]);
        w.frame = (0.0, 0.0);
        let mut items = Vec::new();
        for id in [1u64, 2, 3] {
            w.parked.insert(id);
            let rect = LRect::new(z.left - 900.0 - 100.0 * id as f64, 100.0 * id as f64, 1000.0, 720.0);
            w.compact.park_compact(id, vec![makepad_widgets::makepad_platform::ime::CompactForm::new("strip", 320.0, 100.0)], 0);
            items.push(Item { id, rect, fixed: w.compact_slot(id, &z), anchor: None });
        }
        let slot = w.compact_slot(1, &z).unwrap();
        assert_eq!(slot, (320.0, 100.0));
        let placed = tidy(&z, -1, &items, None);
        let shown: Vec<LRect> = placed.iter().map(|(id, r)| w.compact_rect(*id, *r).expect("compact")).collect();
        for (i, a) in shown.iter().enumerate() {
            assert!(a.x >= -1e-6 && a.x + a.w <= z.left + 1e-6, "inside the zone: {a:?}");
            assert!((a.w - slot.0).abs() < 1e-9 && (a.h - slot.1).abs() < 1e-9, "exactly its slot");
            for b in &shown[i + 1..] {
                assert!(!overlaps(*a, *b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn the_content_point_of_a_parked_window_maps_to_the_app() {
        // And the content point of a parked window maps to the app inside its desk rect.
        let z = wide();
        let desk = LRect::new(z.left - 1600.0, 300.0, 1000.0, 720.0);
        let pose = pose_in(&z, desk);
        let (cx, cy) = pose.shown.center();
        let (x, y) = pose.unmap(desk, cx, cy);
        assert!((x - (desk.x + 500.0)).abs() < 1e-6 && (y - (desk.y + 360.0)).abs() < 1e-6);
    }

    #[test]
    fn cycling_takes_the_nearest_parked_window_each_once_per_round() {
        let parked = [(1u64, 900.0), (2, 300.0), (3, 600.0)];
        let mut seen = Vec::new();
        assert_eq!(next_parked(&parked, &mut seen), Some(2));
        seen.push(2);
        assert_eq!(next_parked(&parked, &mut seen), Some(3));
        seen.push(3);
        assert_eq!(next_parked(&parked, &mut seen), Some(1));
        seen.push(1);
        // All had their turn: the round starts over.
        assert_eq!(next_parked(&parked, &mut seen), Some(2));
        assert!(seen.is_empty());
        assert_eq!(next_parked(&[], &mut seen), None);
    }

    fn leave_at(x: f64, y: f64) -> Leave {
        Leave { client: 1, side: -1, s0: 0.3, from: 6.0, fx: 0.25, fy: 0.4, size: (600.0, 400.0), cursor: (x, y) }
    }

    #[test]
    fn leaving_grows_continuously_and_keeps_the_grabbed_pixel_under_the_pointer() {
        let z = wide();
        let y = 400.0;
        let mut last = 0.3;
        for i in 0..=100 {
            let x = z.left + 6.0 + i as f64 * 20.0;
            let l = leave_at(x, y);
            let k = l.scale(&z);
            assert!(k >= last - 1e-9 && k - last < 0.05 && (0.3..=1.0).contains(&k), "step {i}: {last} -> {k}");
            last = k;
            let sh = l.shown(&z);
            assert!((sh.w - 600.0 * k).abs() < 1e-9);
            // The grab point of the shown rect is the pointer (the window is
            // clear of the display edges here).
            if k < 1.0 - 1e-9 && sh.x > z.left {
                assert!((sh.x + 0.25 * sh.w - x).abs() < 1e-6 && (sh.y + 0.4 * sh.h - y).abs() < 1e-6, "step {i}");
            }
        }
        assert_eq!(last, 1.0);
        assert_eq!(leave_at(z.left + 6.0, y).scale(&z), 0.3, "no jump at the start");
    }

    #[test]
    fn a_window_is_let_go_where_it_is_shown_at_full_size_inside_the_display() {
        let z = wide();
        let l = leave_at(z.left + 2.0 * z.zone_w() + 400.0, 400.0);
        let d = l.desk(&z);
        assert_eq!((d.w, d.h), (600.0, 400.0));
        assert_eq!((d.x + 0.25 * d.w, d.y + 0.4 * d.h), (z.left + 2.0 * z.zone_w() + 400.0, 400.0), "grab point under the release");
        assert!(!pose_in(&z, d).warped(), "full size stays unwarped");
        assert_eq!(l.pose(&z).shown, d);
        // Near the edges it is pushed inside the display (and the band).
        for (x, y) in [(z.left + 10.0, 5.0), (z.right - 3.0, 1435.0), (z.left + 10.0, 400.0)] {
            let l = leave_at(x, y);
            let d = l.desk(&z);
            assert!(d.x >= z.left - 1e-9 && d.x + d.w <= z.right + 1e-9, "{d:?}");
            assert!(d.y >= z.screen.y - 1e-9 && d.y + d.h <= z.screen.y + z.screen.h + 1e-9);
            assert!(!pose_in(&z, d).warped());
        }
    }

    #[test]
    fn a_window_dragged_toward_a_hidden_shelf_is_not_slid_away_with_it() {
        let screen = LRect::new(0.0, 0.0, 1920.0, 1080.0);
        let pose = |x: f64| Pose { shown: LRect::new(x, 300.0, 200.0, 150.0), scale: 0.3, recede: 0.0, side: -1, compact: false };
        let slots = vec![(1u64, pose(10.0), screen), (2u64, pose(40.0), screen)];
        let mut w = Warp::default();
        w.set_hidden(screen, [1.0, 0.0]);
        w.swell_refresh(&slots, &|_| Some((800.0, 600.0)));
        let moved = |w: &Warp, c: u64| w.swollen_pose(c).map(|p| p.shown.x);
        assert!(moved(&w, 1).unwrap() < -100.0 && moved(&w, 2).unwrap() < -100.0, "hidden shelf items slide out");
        // Window 2 is being dragged: it keeps its shown pose, the shelf's hiding is not its.
        w.dragged = vec![2];
        w.swell_refresh(&slots, &|_| Some((800.0, 600.0)));
        assert!(moved(&w, 1).unwrap() < -100.0);
        assert_eq!(moved(&w, 2), None, "no override: it is drawn where the drag puts it");
    }

    #[test]
    fn pulling_out_and_back_changes_the_size_monotonically_in_small_steps() {
        let z = wide();
        let mut prev: Option<(f64, f64)> = None;
        // Out: the pointer walks from the tile over the shelf and across the band.
        let xs: Vec<f64> = (0..400).map(|i| 20.0 + i as f64 * 5.0).collect();
        for &x in xs.iter().chain(xs.iter().rev()) {
            let l = Leave { client: 1, side: -1, s0: 0.25, from: -10.0, fx: 0.3, fy: 0.5, size: (800.0, 600.0), cursor: (x, 500.0) };
            let w: f64 = l.shown(&z).w;
            if let Some((px, pw)) = prev {
                let rising = x > px;
                assert!(if rising { w >= pw - 1e-9 } else { w <= pw + 1e-9 }, "monotone at {x}: {pw} -> {w}");
                assert!((w - pw).abs() <= 12.0, "a step of {} at {x}", (w - pw).abs());
            }
            prev = Some((x, w));
        }
        assert!((Leave { client: 1, side: -1, s0: 0.25, from: -10.0, fx: 0.3, fy: 0.5, size: (800.0, 600.0), cursor: (xs[xs.len() - 1], 500.0) }.shown(&z).w - 800.0).abs() < 1e-6);
    }

    #[test]
    fn a_drop_back_on_the_shelf_keeps_the_size_it_was_shown_at() {
        let z = wide();
        for s in [0.15, 0.25, 0.5, 0.8] {
            for side in [-1i8, 1] {
                let r = desk_with_scale(&z, 800.0, 600.0, s, 500.0, side);
                let p = pose_in(&z, r);
                assert!((p.scale - s).abs() < 1e-3, "{s}: {}", p.scale);
                assert!((p.shown.y + p.shown.h * 0.5 - 500.0).abs() < 1.0);
                assert_eq!(p.side, side);
            }
        }
    }

    #[test]
    fn a_dragged_window_is_a_normal_window_everywhere_and_only_the_edge_preview_shrinks_it() {
        let z = wide();
        let mut w = Warp::default();
        w.set_areas([z.screen]);
        // A window that merely has a rect deep in the left zone is a normal window...
        let deep = slot_at(&z, -1, z.zone_w() * 0.5, 800.0, 600.0);
        assert!(!w.pose_drawn(1, deep).warped(), "geometry alone never shelves a window");
        // ...a parked one is a tile.
        w.parked.insert(1);
        assert!(w.pose_drawn(1, deep).warped());
        // ...but dragged, it is shown as it is: no shrink wherever the pointer goes.
        w.dragged = vec![1];
        let p = w.pose_drawn(1, deep);
        assert!(!p.warped() && p.shown == deep);
        // Near an open shelf it shrinks continuously about the grab point.
        let mut last = 2.0;
        for step in 0..=100 {
            let away = 600.0 - step as f64 * 6.0;
            let mut e = Enter::new(1, -1, 0.3, 0.4, (800.0, 600.0), (400.0, 500.0), 0.25);
            e.e = enter_progress(away, 450.0);
            let k = e.scale();
            assert!(k <= last + 1e-9 && k >= 0.25 - 1e-9, "monotone: {away} {k}");
            last = k;
            let sh = e.shown(&z);
            assert!((sh.x + 0.3 * sh.w - 400.0).abs() < 1e-6 && (sh.y + 0.4 * sh.h - 500.0).abs() < 1e-6, "grab point stays under the pointer");
        }
        assert_eq!(enter_progress(450.0, 450.0), 0.0, "full size at the span");
        assert_eq!(enter_progress(-10.0, 450.0), 1.0, "the slot's size at the shelf");
        let mut e = Enter::new(1, -1, 0.3, 0.4, (800.0, 600.0), (400.0, 500.0), 0.25);
        e.e = 0.0;
        w.enter = Some(e);
        assert!(!w.pose_drawn(1, deep).warped(), "full size away from the shelf");
        e.e = 1.0;
        w.enter = Some(e);
        assert!(w.pose_drawn(1, deep).warped());
        // Another window is unaffected by it.
        w.enter = Some(Enter::new(2, -1, 0.5, 0.5, (800.0, 600.0), (0.0, 0.0), 0.25));
        assert!(!w.pose_drawn(1, deep).warped());
    }
}
