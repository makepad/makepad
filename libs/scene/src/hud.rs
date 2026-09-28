//! The retained HUD document: named elements, layout, and values that read
//! themselves off the game. The design record is `apps/sandbox/UI_DESIGN.md`.
//!
//! The older `hud_slots`/`hud_bars` are a line of text and a 140x10 gauge, and
//! they stay exactly as they are — every game that draws with them keeps
//! working, and `game.text`/`game.bar` remain their sugar. This is the layer
//! above: panels with grounds and borders, gauges that carry their own label,
//! readouts with icons, a message log, screen flashes, hit markers.
//!
//! Three decisions, all made for the same customer — a small model writing one
//! file in one pass, with no debugger and no second attempt:
//!
//! - **Named, retained and idempotent.** `game.hud_bar("health", …)` declares
//!   an element once; re-issuing the name MERGES into it, and absent keys keep
//!   their value (the rule `game.tune`/`game.trim` already use). A model that
//!   forgets to re-issue every tick still has a HUD; a model that issues twice
//!   still has one of it. Under immediate mode both mistakes are a blank
//!   screen or a flicker, and both are mistakes small models make.
//! - **Bindings, not per-tick writes.** `bind: "hp"` makes an element read the
//!   player's hit points itself, every frame, forever. The commonest way a
//!   hand-updated HUD breaks is that the `on_tick` updating it stops running —
//!   after a script error, during a death, inside a branch the author forgot.
//!   A bound element cannot go stale because nothing has to remember it.
//! - **A flat list that declares a tree by reference.** Elements are addressed
//!   by name and name their container with `in:`. A model can emit them in any
//!   order, forward-reference a panel, and edit one line without re-deriving
//!   the shape of the document.
//!
//! Layout is measure-then-place over that reference. Nothing here knows about
//! fonts or textures: text is measured through a callback the renderer
//! supplies, so the whole layout is testable with no GPU and no window.
//!
//! **Units.** Every size and offset is in HUD units, `1 unit = 1/1080` of the
//! pane height, uniformly scaled. A HUD authored once is the same size on a
//! phone and on a wall, and an author never writes a device check.

use makepad_math::*;

/// The reference pane height HUD units are a fraction of.
pub const HUD_REFERENCE_HEIGHT: f32 = 1080.0;

/// What an element draws as. Deliberately few: each is something a game HUD
/// cannot be built without, and a model that remembers only four of them can
/// still build a status bar. An inventory grid is a panel of icons; a big
/// number is a text with `style: "numeral"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HudKind {
    /// A ground with a border and a padding box that stacks its children.
    #[default]
    Panel,
    /// A linear fill.
    Bar,
    /// The same fill, radial.
    Ring,
    /// A string, a number, or both with a caption.
    Text,
    /// One image: a built-in glyph, an SVG resource, or a catalog image.
    /// Never a shader.
    Icon,
    /// A rolling strip of recent lines, fed by engine events or by hand.
    Log,
    /// A full-screen or edge tint fired by an engine event.
    Flash,
    /// A mark at the crosshair fired by a hit or a kill.
    Marker,
    /// A minimap: a world polyline (a track, a route) fitted into the box,
    /// with live dots for the racers, players or tagged entities the host
    /// supplies — the subject highlighted.
    Map,
    /// A fighter head-up display over the whole pane: pitch ladder,
    /// flight-path marker, tapes, target boxes and lock, warnings. The host
    /// projects the subject aircraft's state (see the renderer's
    /// `FlightHud`).
    Flight,
}

impl HudKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HudKind::Panel => "panel",
            HudKind::Bar => "bar",
            HudKind::Ring => "ring",
            HudKind::Text => "text",
            HudKind::Icon => "icon",
            HudKind::Log => "log",
            HudKind::Flash => "flash",
            HudKind::Marker => "marker",
            HudKind::Map => "map",
            HudKind::Flight => "flight",
        }
    }

    /// Nothing about a flash or a marker occupies space: they are drawn over
    /// the pane on their own terms and must not push a panel's children
    /// around.
    pub fn is_laid_out(self) -> bool {
        !matches!(self, HudKind::Flash | HudKind::Marker | HudKind::Flight)
    }
}

/// How a panel stacks its children. Column is the default because a HUD
/// column (a value over its caption, a list of readouts) is the shape authors
/// reach for most, and because a row that wanted a column is obvious on
/// screen while the reverse looks like a layout bug.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HudStack {
    #[default]
    Column,
    Row,
    /// Not at all: each child sits at its own `at` inside the panel. The
    /// escape hatch for a layout that is a picture rather than a list.
    Free,
}

impl HudStack {
    pub fn parse(name: &str) -> Option<HudStack> {
        Some(match name {
            "column" | "col" | "down" | "vertical" => HudStack::Column,
            "row" | "across" | "horizontal" => HudStack::Row,
            "free" | "none" => HudStack::Free,
            _ => return None,
        })
    }
}

/// Cross-axis placement of a panel's children.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HudAlign {
    Start,
    #[default]
    Center,
    End,
}

impl HudAlign {
    pub fn parse(name: &str) -> Option<HudAlign> {
        Some(match name {
            "start" | "left" | "top" | "begin" => HudAlign::Start,
            "center" | "centre" | "middle" => HudAlign::Center,
            "end" | "right" | "bottom" => HudAlign::End,
            _ => return None,
        })
    }

    fn offset(self, outer: f32, inner: f32) -> f32 {
        match self {
            HudAlign::Start => 0.0,
            HudAlign::Center => (outer - inner) * 0.5,
            HudAlign::End => outer - inner,
        }
    }
}

/// Where a value comes from. `Bind` names a live engine value the host
/// resolves every frame; `Fixed` is a number the script wrote.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum HudValue {
    #[default]
    None,
    Fixed(f32),
    Bind(String),
}

impl HudValue {
    pub fn is_none(&self) -> bool {
        matches!(self, HudValue::None)
    }
}

/// One resolved line of a log.
#[derive(Clone, Debug)]
pub struct HudLine {
    pub text: String,
    pub color: Vec4f,
    pub icon: String,
    /// Which log element it belongs to.
    pub target: String,
    pub secs: f32,
    pub age: f32,
}

/// A live tint or mark: the decaying part of a Flash/Marker element.
#[derive(Clone, Copy, Debug, Default)]
pub struct HudPulse {
    pub secs: f32,
    pub age: f32,
}

impl HudPulse {
    pub fn strength(&self) -> f32 {
        if self.secs <= 0.0 {
            return 0.0;
        }
        (1.0 - self.age / self.secs).clamp(0.0, 1.0)
    }
    pub fn alive(&self) -> bool {
        self.secs > 0.0 && self.age < self.secs
    }
}

// ---------------------------------------------------------------------------
// Motion: tweens, transforms, punches and count-ups
// ---------------------------------------------------------------------------

/// The curve a tween runs along, `t` in 0..1 to progress (which may pass 1:
/// `OutBack` and `OutElastic` overshoot, which is what makes a pop pop).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HudEase {
    #[default]
    OutCubic,
    Linear,
    OutBack,
    OutExpo,
    OutElastic,
    InOutSine,
}

impl HudEase {
    pub fn parse(name: &str) -> Option<HudEase> {
        Some(match name {
            "out_cubic" | "out" => HudEase::OutCubic,
            "linear" => HudEase::Linear,
            "out_back" | "back" => HudEase::OutBack,
            "out_expo" | "expo" => HudEase::OutExpo,
            "out_elastic" | "elastic" => HudEase::OutElastic,
            "in_out_sine" | "sine" => HudEase::InOutSine,
            _ => return None,
        })
    }
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            HudEase::Linear => t,
            HudEase::OutCubic => 1.0 - (1.0 - t).powi(3),
            HudEase::OutBack => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
            }
            HudEase::OutExpo => {
                if t >= 1.0 { 1.0 } else { 1.0 - 2f32.powf(-10.0 * t) }
            }
            HudEase::OutElastic => {
                if t <= 0.0 || t >= 1.0 {
                    t
                } else {
                    2f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * (2.0 * std::f32::consts::PI / 3.0)).sin() + 1.0
                }
            }
            HudEase::InOutSine => -((std::f32::consts::PI * t).cos() - 1.0) * 0.5,
        }
    }
}

/// What a tween does to its element.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HudTweenKind {
    #[default]
    None,
    /// Scale up from small with an overshoot: "GO!", a combo count.
    Pop,
    Fade,
    /// Slide in FROM that side (and out TO it).
    SlideLeft,
    SlideRight,
    SlideUp,
    SlideDown,
    /// A scale kick that settles back: a value that just changed.
    Punch,
    /// A decaying jitter: a hit, a miss, a fumble.
    Shake,
}

impl HudTweenKind {
    pub fn parse(name: &str) -> Option<HudTweenKind> {
        Some(match name {
            "none" | "" => HudTweenKind::None,
            "pop" => HudTweenKind::Pop,
            "fade" => HudTweenKind::Fade,
            "slide_left" | "left" => HudTweenKind::SlideLeft,
            "slide_right" | "right" => HudTweenKind::SlideRight,
            "slide_up" | "up" => HudTweenKind::SlideUp,
            "slide_down" | "down" => HudTweenKind::SlideDown,
            "punch" => HudTweenKind::Punch,
            "shake" => HudTweenKind::Shake,
            _ => return None,
        })
    }
}

/// One tween: what, how long, along which curve, and how far (scale kick
/// for a punch, HUD units for a slide or a shake; 0 = the kind's default).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudTween {
    pub kind: HudTweenKind,
    pub secs: f32,
    pub ease: HudEase,
    pub amount: f32,
}

impl HudTween {
    /// The kind with the duration, curve and reach it reads best at: out-back
    /// for pops, out-expo for slides.
    pub fn of(kind: HudTweenKind) -> HudTween {
        let (secs, ease, amount) = match kind {
            HudTweenKind::None => (0.0, HudEase::OutCubic, 0.0),
            HudTweenKind::Pop => (0.45, HudEase::OutBack, 0.25),
            HudTweenKind::Fade => (0.25, HudEase::OutCubic, 0.0),
            HudTweenKind::SlideLeft | HudTweenKind::SlideRight | HudTweenKind::SlideUp | HudTweenKind::SlideDown => {
                (0.5, HudEase::OutExpo, 90.0)
            }
            HudTweenKind::Punch => (0.35, HudEase::OutCubic, 0.35),
            HudTweenKind::Shake => (0.4, HudEase::Linear, 10.0),
        };
        HudTween { kind, secs, ease, amount }
    }
    pub fn is_none(&self) -> bool {
        self.kind == HudTweenKind::None || self.secs <= 0.0
    }
}

/// How an element moves. The default is perfectly still, and a still element
/// costs the renderer nothing beyond one comparison.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudMotion {
    /// Horizontal shear, as x offset per unit of height (-0.2 leans it
    /// forward like italic type): the static slant of a trick callout.
    pub skew: f32,
    /// Static rotation, radians, clockwise on screen.
    pub rotate: f32,
    /// Played when the element appears (declared, `show`/`when` turning on).
    pub enter: HudTween,
    /// Played when it goes (`show`/`when` turning off, a `ms` running out):
    /// the enter curve backwards.
    pub exit: HudTween,
    /// Played whenever its number or text changes.
    pub change: HudTween,
    /// Seconds a changed number takes to count to its new value (0 = snaps).
    pub count: f32,
}

impl HudMotion {
    pub fn is_still(&self) -> bool {
        self.skew == 0.0
            && self.rotate == 0.0
            && self.enter.is_none()
            && self.exit.is_none()
            && self.change.is_none()
            && self.count <= 0.0
    }
}

/// The live half of [`HudMotion`], owned by the document between frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudMotionState {
    /// Seconds since the enter / exit / change tween (or the punch) started;
    /// infinite when none is running.
    pub enter_age: f32,
    pub exit_age: f32,
    pub change_age: f32,
    /// One-shot `hud_punch` kicks, independent of the declared tweens: a
    /// scale punch and a shake, which may play together.
    pub punch: HudTween,
    pub punch_age: f32,
    pub shake: HudTween,
    pub shake_age: f32,
    /// Seen on screen last frame, and where (pane-local pixels x, y, w, h).
    pub shown: bool,
    pub rect: [f32; 4],
    /// The number/text last seen, for change detection.
    pub value: f32,
    pub text_key: u64,
    /// Count-up: from, to, and seconds into it.
    pub count_from: f32,
    pub count_to: f32,
    pub count_age: f32,
    /// The element goes away for good once its exit has played.
    pub remove_after_exit: bool,
}

impl Default for HudMotionState {
    fn default() -> Self {
        Self {
            enter_age: f32::INFINITY,
            exit_age: f32::INFINITY,
            change_age: f32::INFINITY,
            punch: HudTween::default(),
            punch_age: f32::INFINITY,
            shake: HudTween::default(),
            shake_age: f32::INFINITY,
            shown: false,
            rect: [0.0; 4],
            value: f32::NAN,
            text_key: 0,
            count_from: 0.0,
            count_to: 0.0,
            count_age: f32::INFINITY,
            remove_after_exit: false,
        }
    }
}

/// Where the motion puts an element this frame, applied about its centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudPose {
    /// Offset in pane pixels.
    pub dx: f32,
    pub dy: f32,
    pub scale: f32,
    pub rotate: f32,
    pub skew: f32,
    pub opacity: f32,
}

impl HudPose {
    pub const IDENTITY: HudPose = HudPose { dx: 0.0, dy: 0.0, scale: 1.0, rotate: 0.0, skew: 0.0, opacity: 1.0 };
    /// True when the pose moves nothing (opacity aside).
    pub fn is_rigid_identity(&self) -> bool {
        self.dx == 0.0 && self.dy == 0.0 && self.scale == 1.0 && self.rotate == 0.0 && self.skew == 0.0
    }
    /// The 2D affine `[a, b, c, d, tx, ty]` (x' = a x + c y + tx,
    /// y' = b x + d y + ty) that applies this pose about `pivot`.
    pub fn affine(&self, pivot: Vec2f) -> [f32; 6] {
        let (s, c) = self.rotate.sin_cos();
        let k = self.scale;
        // R * Shear * S: shear leans x by y.
        let a = c * k;
        let b = s * k;
        let cc = (c * self.skew - s) * k;
        let d = (s * self.skew + c) * k;
        let tx = pivot.x + self.dx - (a * pivot.x + cc * pivot.y);
        let ty = pivot.y + self.dy - (b * pivot.x + d * pivot.y);
        [a, b, cc, d, tx, ty]
    }
}

/// Compose two affines: `outer` applied after `inner`.
pub fn hud_affine_mul(outer: [f32; 6], inner: [f32; 6]) -> [f32; 6] {
    let [a1, b1, c1, d1, x1, y1] = outer;
    let [a2, b2, c2, d2, x2, y2] = inner;
    [
        a1 * a2 + c1 * b2,
        b1 * a2 + d1 * b2,
        a1 * c2 + c1 * d2,
        b1 * c2 + d1 * d2,
        a1 * x2 + c1 * y2 + x1,
        b1 * x2 + d1 * y2 + y1,
    ]
}

pub const HUD_AFFINE_IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Fold one tween at progress `p` (0 = start of an enter, 1 = at rest) into
/// `pose`. `units` converts HUD units to pixels.
fn tween_pose(pose: &mut HudPose, tw: &HudTween, p: f32, linear: f32, units: f32) {
    let reach = if tw.amount != 0.0 { tw.amount } else { HudTween::of(tw.kind).amount };
    let fade_in = (linear * 2.5).clamp(0.0, 1.0);
    match tw.kind {
        HudTweenKind::None => {}
        HudTweenKind::Pop => {
            pose.scale *= reach + (1.0 - reach) * p;
            pose.opacity *= fade_in;
        }
        HudTweenKind::Fade => pose.opacity *= p.clamp(0.0, 1.0),
        HudTweenKind::SlideLeft => {
            pose.dx -= reach * units * (1.0 - p);
            pose.opacity *= fade_in;
        }
        HudTweenKind::SlideRight => {
            pose.dx += reach * units * (1.0 - p);
            pose.opacity *= fade_in;
        }
        HudTweenKind::SlideUp => {
            pose.dy += reach * units * (1.0 - p);
            pose.opacity *= fade_in;
        }
        HudTweenKind::SlideDown => {
            pose.dy -= reach * units * (1.0 - p);
            pose.opacity *= fade_in;
        }
        HudTweenKind::Punch => pose.scale *= 1.0 + reach * (1.0 - p),
        HudTweenKind::Shake => {
            let decay = 1.0 - p;
            let w = linear * tw.secs * 60.0;
            pose.dx += reach * units * decay * (w * 1.3).sin();
            pose.dy += reach * units * decay * 0.6 * (w * 1.7 + 1.1).cos();
        }
    }
}

/// A kick-style tween (punch, shake) runs from its peak to rest; everything
/// else runs from "not there yet" to rest. `age` in seconds.
fn tween_at(pose: &mut HudPose, tw: &HudTween, age: f32, units: f32) {
    if tw.is_none() || !age.is_finite() || age >= tw.secs {
        return;
    }
    let linear = (age / tw.secs).clamp(0.0, 1.0);
    tween_pose(pose, tw, tw.ease.apply(linear), linear, units);
}

/// The element's pose this frame, from its static transform and whatever
/// tweens are running. `units` converts HUD units to pixels.
pub fn hud_pose(e: &HudElement, units: f32) -> HudPose {
    let m = &e.motion;
    let st = &e.motion_state;
    let mut pose = HudPose { skew: m.skew, rotate: m.rotate, ..HudPose::IDENTITY };
    if st.exit_age.is_finite() && !m.exit.is_none() {
        // The exit is the enter played backwards: progress runs 1 -> 0.
        let linear = (st.exit_age / m.exit.secs).clamp(0.0, 1.0);
        let back = 1.0 - linear;
        tween_pose(&mut pose, &m.exit, m.exit.ease.apply(back), back, units);
    } else {
        tween_at(&mut pose, &m.enter, st.enter_age, units);
    }
    tween_at(&mut pose, &m.change, st.change_age, units);
    tween_at(&mut pose, &st.punch, st.punch_age, units);
    tween_at(&mut pose, &st.shake, st.shake_age, units);
    pose
}

/// The number a counting element shows this frame instead of `target`.
pub fn hud_counted(e: &HudElement, target: f32) -> f32 {
    let st = &e.motion_state;
    if e.motion.count <= 0.0 || !st.count_age.is_finite() || st.count_age >= e.motion.count || st.count_to != target {
        return target;
    }
    let p = HudEase::OutExpo.apply(st.count_age / e.motion.count);
    st.count_from + (st.count_to - st.count_from) * p
}

/// What the renderer saw of one moving element this frame, for
/// [`HudDoc::settle_motion`].
#[derive(Clone, Debug, PartialEq)]
pub struct HudSeen {
    pub name: String,
    pub visible: bool,
    pub rect: [f32; 4],
    /// Its number (a gauge's value, a numeric readout), when it has one.
    pub number: Option<f32>,
    /// A hash of the text it drew, 0 for none.
    pub text_key: u64,
}

/// One declared element. Every field is optional: an element that sets only
/// its kind and a bind still draws something sensible, which is the property
/// that lets a small model get a working HUD on the first try.
///
/// Re-declaring a name MERGES — the caller starts from the existing element
/// and overwrites only the keys the author wrote.
#[derive(Clone, Debug)]
pub struct HudElement {
    pub name: String,
    pub kind: HudKind,
    /// The `in:` key — the panel this lives in. Empty pins it to the pane.
    pub parent: String,
    /// Where it pins when it has no parent. Ignored when `parent` is set,
    /// except under a `Free` panel where `at` is measured from its corner.
    pub anchor: HudAnchor,
    /// The `at:` offset, measured INWARD from the anchor, so both components
    /// are positive at every corner. An author never has to remember which
    /// corner needs a negative number.
    pub at: Vec2f,
    /// Explicit size in HUD units. Zero on either axis means "measure me".
    pub size: Vec2f,
    pub stack: HudStack,
    pub align: HudAlign,
    pub gap: f32,
    pub pad: f32,
    /// The look, per kind: panel `plate|frame|bare`, bar `plate|bare|
    /// segmented`, text `plain|numeral|caption|banner`, ring `plate|bare`,
    /// flash `vignette|full|edge`, marker `cross|x|ring`.
    pub style: String,
    /// Foreground: bar fill, text ink, icon tint.
    pub color: Vec4f,
    /// The `track:` colour of a gauge, or a panel's ground. `w == 0` = the
    /// style's own default.
    pub track: Vec4f,
    pub border: f32,
    pub border_color: Vec4f,
    pub radius: f32,
    /// Literal string for a Text element.
    pub text: String,
    /// Small caption drawn ahead of the value.
    pub label: String,
    pub prefix: String,
    pub suffix: String,
    /// Decimal places. 0 draws an integer.
    pub format: u8,
    pub value: HudValue,
    pub max: HudValue,
    /// Whose value to read (0 = the local player's entity).
    pub of: u64,
    /// Below this fraction the element draws in `low_color` and pulses.
    pub low: f32,
    pub low_color: Vec4f,
    /// Write the number on the gauge.
    pub show_value: bool,
    /// A gauge drawn as this many pips instead of one continuous fill.
    pub segments: u32,
    /// Ease the fill toward its target instead of snapping — a health bar
    /// that snaps reads as a prototype.
    pub chip: bool,
    /// Built-in glyph name, or `svg:`/`image:` resolved by the host.
    pub icon: String,
    pub svg: String,
    pub image: String,
    /// Draw as a not-yet-collected ghost.
    pub dim: bool,
    /// Badge number on an icon. `None` draws none.
    pub count: HudValue,
    /// Glyph size for text, edge length for an icon, radius for a ring.
    pub glyph: f32,
    pub thickness: f32,
    /// Ring arc: start angle and how far it sweeps, radians.
    pub from: f32,
    pub sweep: f32,
    /// Log: which engine events feed it, and how much it keeps.
    pub events: Vec<String>,
    pub lines: u32,
    /// Milliseconds a log line, toast, flash or marker lives.
    pub ms: f32,
    /// Flash/Marker: which engine event fires it.
    pub on: String,
    pub strength: f32,
    /// A bind name that gates visibility; a leading `!` inverts it.
    pub when: String,
    pub show: bool,
    /// Declaration order, so layout is stable however the host stores the
    /// document. `order:` overrides it.
    pub order: i32,
    /// Live decay for a Flash/Marker, and the chip bar's lagging value.
    pub pulse: HudPulse,
    pub chip_value: f32,
    /// Set when the element expires on its own (`ms` on a text banner).
    pub expires: f32,
    /// Map: the world polyline as (x, z), decimated at declaration
    /// ([`decimate_polyline`]), and whether it closes on itself.
    pub points: Vec<Vec2f>,
    pub closed: bool,
    /// Map: which entities draw as dots — `racers`, `players`, `tag:<tag>`,
    /// `ids` (then `dot_ids`), or empty for none. The host resolves it.
    pub dots: String,
    pub dot_ids: Vec<u64>,
    /// Map: dot palette and size (HUD units); zero alpha = the style's own.
    pub dot_color: Vec4f,
    pub self_color: Vec4f,
    pub lead_color: Vec4f,
    pub dot_size: f32,
    /// Map: turn the whole map so the subject's heading points up.
    pub rotate: bool,
    /// Map: write each dot's race position beside it.
    pub labels: bool,
    /// Map as RADAR: metres from the pane's own subject to the edge of a
    /// round map centred on it. 0 = fit the whole polyline in the box.
    pub range: f32,
    /// Radar: world (x, z) points of interest (the next gate, a drop-off),
    /// drawn in place when in range and as an arrow on the rim when not.
    pub targets: Vec<Vec2f>,
    pub target_color: Vec4f,
    /// How it moves (tweens, transform, count-up) and the live state of that.
    pub motion: HudMotion,
    pub motion_state: HudMotionState,
}

impl Default for HudElement {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: HudKind::Panel,
            parent: String::new(),
            anchor: HudAnchor::TopLeft,
            at: vec2f(0.0, 0.0),
            size: vec2f(0.0, 0.0),
            stack: HudStack::Column,
            align: HudAlign::Center,
            gap: 6.0,
            pad: 8.0,
            style: String::new(),
            color: vec4(0.0, 0.0, 0.0, 0.0),
            track: vec4(0.0, 0.0, 0.0, 0.0),
            border: -1.0,
            border_color: vec4(0.0, 0.0, 0.0, 0.0),
            radius: -1.0,
            text: String::new(),
            label: String::new(),
            prefix: String::new(),
            suffix: String::new(),
            format: 0,
            value: HudValue::None,
            max: HudValue::None,
            of: 0,
            low: 0.0,
            low_color: vec4(0.0, 0.0, 0.0, 0.0),
            show_value: false,
            segments: 0,
            chip: true,
            icon: String::new(),
            svg: String::new(),
            image: String::new(),
            dim: false,
            count: HudValue::None,
            glyph: 0.0,
            thickness: 0.0,
            from: 0.0,
            sweep: 0.0,
            events: Vec::new(),
            lines: 5,
            ms: 0.0,
            on: String::new(),
            strength: 1.0,
            when: String::new(),
            show: true,
            order: i32::MIN,
            pulse: HudPulse::default(),
            chip_value: f32::NAN,
            expires: 0.0,
            points: Vec::new(),
            closed: false,
            dots: String::new(),
            dot_ids: Vec::new(),
            dot_color: vec4(0.0, 0.0, 0.0, 0.0),
            self_color: vec4(0.0, 0.0, 0.0, 0.0),
            lead_color: vec4(0.0, 0.0, 0.0, 0.0),
            dot_size: 0.0,
            rotate: false,
            labels: false,
            range: 0.0,
            targets: Vec::new(),
            target_color: vec4(0.0, 0.0, 0.0, 0.0),
            motion: HudMotion::default(),
            motion_state: HudMotionState::default(),
        }
    }
}

/// How the aiming mark draws. The crosshair is its own thing rather than an
/// element because it is about the gun, not about the layout: the engine
/// turns it off when you die and on when you hold a weapon, and a game should
/// not have to re-declare it to keep that.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CrosshairStyle {
    #[default]
    Dot,
    Cross,
    Ring,
    None,
}

impl CrosshairStyle {
    pub fn parse(name: &str) -> Option<CrosshairStyle> {
        Some(match name {
            "dot" => CrosshairStyle::Dot,
            "cross" | "plus" => CrosshairStyle::Cross,
            "ring" | "circle" => CrosshairStyle::Ring,
            "none" | "off" => CrosshairStyle::None,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Crosshair {
    pub style: CrosshairStyle,
    pub size: f32,
    pub gap: f32,
    pub thickness: f32,
    pub color: Vec4f,
    /// Bloom the reticle with the active gun's real spread.
    pub spread: bool,
}

impl Default for Crosshair {
    fn default() -> Self {
        Self {
            style: CrosshairStyle::Dot,
            size: 10.0,
            gap: 5.0,
            thickness: 2.0,
            color: vec4(1.0, 1.0, 1.0, 0.85),
            spread: false,
        }
    }
}

/// Everything the HUD layer holds. Retained across ticks, cleared with the
/// rest of the world's authored content.
#[derive(Clone, Debug, Default)]
pub struct HudDoc {
    pub elements: Vec<HudElement>,
    pub lines: Vec<HudLine>,
    pub crosshair: Option<Crosshair>,
    /// Which preset installed this, for `game.hud("none")` and for reporting.
    pub preset: String,
    next_order: i32,
}

/// Ceiling on live log lines across every log element.
pub const MAX_LINES: usize = 24;
/// A log line's life when nothing said otherwise.
pub const DEFAULT_LINE_SECS: f32 = 6.0;

impl HudDoc {
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty() && self.lines.is_empty()
    }

    /// The element under `name`, for a caller assembling a merge.
    pub fn get(&self, name: &str) -> Option<&HudElement> {
        self.elements.iter().find(|e| e.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut HudElement> {
        self.elements.iter_mut().find(|e| e.name == name)
    }

    /// Install a fully-merged element. The caller has already folded the
    /// author's keys over whatever [`Self::get`] returned, which is where the
    /// "absent keys keep their value" rule lives.
    ///
    /// The document grows with what the author declares; there is no fixed
    /// ceiling (a competitive HUD with a scoreboard and a kill feed runs to a
    /// few hundred elements, and layout is linear in them).
    pub fn set(&mut self, mut element: HudElement) {
        if let Some(slot) = self.elements.iter_mut().find(|e| e.name == element.name) {
            // A re-declaration keeps its place in the layout unless the author
            // asked for a different one: an update must never make a HUD jump.
            if element.order == i32::MIN {
                element.order = slot.order;
            }
            element.pulse = slot.pulse;
            element.chip_value = slot.chip_value;
            element.motion_state = slot.motion_state;
            *slot = element;
            return;
        }
        if element.order == i32::MIN {
            element.order = self.next_order;
        }
        self.next_order += 1;
        self.elements.push(element);
    }

    /// Remove an element and everything inside it — orphaning a child would
    /// leave it floating at the pane's corner, which reads as corruption
    /// rather than as a removal.
    pub fn remove(&mut self, name: &str) {
        let mut doomed = vec![name.to_string()];
        let mut i = 0;
        while i < doomed.len() {
            let parent = doomed[i].clone();
            for e in &self.elements {
                if e.parent == parent && !doomed.contains(&e.name) {
                    doomed.push(e.name.clone());
                }
            }
            i += 1;
        }
        self.elements.retain(|e| !doomed.contains(&e.name));
        self.lines.retain(|l| !doomed.contains(&l.target));
    }

    pub fn clear(&mut self) {
        self.elements.clear();
        self.lines.clear();
        self.crosshair = None;
        self.preset.clear();
        self.next_order = 0;
    }

    /// Push a line into one log element (or into every log element, when
    /// `target` is empty — which is what an engine event does).
    pub fn say(&mut self, line: HudLine) {
        self.lines.push(line);
        while self.lines.len() > MAX_LINES {
            self.lines.remove(0);
        }
    }

    /// Fire a Flash or Marker element's pulse. A second fire while one is
    /// fading RESTARTS it rather than queueing: taking two hits in a row
    /// should read as one continuous alarm, not as a stutter.
    pub fn pulse(&mut self, name: &str, secs: f32) {
        if let Some(e) = self.get_mut(name) {
            e.pulse = HudPulse { secs: secs.max(0.01), age: 0.0 };
        }
    }

    /// Fire every Flash/Marker element listening for `event`.
    pub fn fire(&mut self, event: &str) {
        for e in &mut self.elements {
            if e.on == event && matches!(e.kind, HudKind::Flash | HudKind::Marker) {
                let secs = if e.ms > 0.0 { e.ms / 1000.0 } else { 0.3 };
                e.pulse = HudPulse { secs, age: 0.0 };
            }
        }
    }

    /// Which log elements are listening for `event`.
    pub fn logs_for(&self, event: &str) -> Vec<String> {
        self.elements
            .iter()
            .filter(|e| e.kind == HudKind::Log && e.events.iter().any(|x| x == event))
            .map(|e| e.name.clone())
            .collect()
    }

    /// Age the time-based parts. Separate from layout so a paused game does
    /// not expire its messages.
    pub fn advance(&mut self, dt: f32) {
        for line in &mut self.lines {
            line.age += dt;
        }
        self.lines.retain(|l| l.age < l.secs);
        let mut expired: Vec<String> = Vec::new();
        for e in &mut self.elements {
            if e.pulse.secs > 0.0 {
                e.pulse.age += dt;
                if !e.pulse.alive() {
                    e.pulse = HudPulse::default();
                }
            }
            if e.expires > 0.0 {
                e.expires -= dt;
                if e.expires <= 0.0 {
                    if !e.motion.exit.is_none() && e.motion_state.shown {
                        // Play the exit first; settle_motion removes it.
                        e.show = false;
                        e.motion_state.remove_after_exit = true;
                    } else {
                        expired.push(e.name.clone());
                    }
                }
            }
        }
        for name in expired {
            self.remove(&name);
        }
    }

    /// Kick one element with a one-shot tween on top of whatever else it
    /// is doing: a `Shake` goes in the shake slot, anything else in the
    /// punch slot, so a punch and a shake can play at once.
    pub fn punch(&mut self, name: &str, tween: HudTween) {
        if let Some(e) = self.get_mut(name) {
            if tween.kind == HudTweenKind::Shake {
                e.motion_state.shake = tween;
                e.motion_state.shake_age = 0.0;
            } else {
                e.motion_state.punch = tween;
                e.motion_state.punch_age = 0.0;
            }
        }
    }

    /// Advance every moving element by `dt` against what the renderer just
    /// drew (`seen`): start enter tweens on appearing, exits on
    /// disappearing, change tweens and count-ups on a new number or text,
    /// and drop elements whose exit has played out. Still elements are
    /// never in `seen` and are not touched. Returns true while anything is
    /// still in motion, so the host knows to keep drawing frames.
    pub fn settle_motion(&mut self, dt: f32, seen: &[HudSeen]) -> bool {
        let mut animating = false;
        let mut done: Vec<String> = Vec::new();
        for s in seen {
            let Some(e) = self.get_mut(&s.name) else { continue };
            let m = e.motion;
            let st = &mut e.motion_state;
            for age in [&mut st.enter_age, &mut st.exit_age, &mut st.change_age, &mut st.punch_age, &mut st.shake_age, &mut st.count_age] {
                if age.is_finite() {
                    *age += dt;
                }
            }
            if s.visible {
                if s.rect[2] > 0.0 {
                    st.rect = s.rect;
                }
                if !st.shown {
                    st.enter_age = if m.enter.is_none() { f32::INFINITY } else { 0.0 };
                    st.exit_age = f32::INFINITY;
                } else {
                    let number_changed = matches!(s.number, Some(v) if st.value.is_finite() && v != st.value);
                    let text_changed = s.text_key != 0 && st.text_key != 0 && s.text_key != st.text_key;
                    if (number_changed || text_changed) && !m.change.is_none() {
                        st.change_age = 0.0;
                    }
                    if number_changed && m.count > 0.0 {
                        let now = if st.count_age.is_finite() && st.count_age < m.count {
                            let p = HudEase::OutExpo.apply(st.count_age / m.count);
                            st.count_from + (st.count_to - st.count_from) * p
                        } else {
                            st.value
                        };
                        st.count_from = now;
                        st.count_to = s.number.unwrap_or(now);
                        st.count_age = 0.0;
                    }
                }
                if let Some(v) = s.number {
                    st.value = v;
                }
                if s.text_key != 0 {
                    st.text_key = s.text_key;
                }
                st.shown = true;
            } else if st.shown {
                st.shown = false;
                st.exit_age = if m.exit.is_none() { f32::INFINITY } else { 0.0 };
            }
            let exiting = st.exit_age.is_finite() && st.exit_age < m.exit.secs;
            if st.remove_after_exit && !exiting && !st.shown {
                done.push(s.name.clone());
                continue;
            }
            animating |= exiting
                || (st.enter_age.is_finite() && st.enter_age < m.enter.secs)
                || (st.change_age.is_finite() && st.change_age < m.change.secs)
                || (st.punch_age.is_finite() && st.punch_age < st.punch.secs)
                || (st.shake_age.is_finite() && st.shake_age < st.shake.secs)
                || (st.count_age.is_finite() && st.count_age < m.count);
        }
        for name in done {
            self.remove(&name);
        }
        animating
    }

    /// Ease every chip gauge toward the value it was just drawn with. The
    /// renderer writes `chip_value`; this is what makes it lag.
    pub fn settle_chips(&mut self, dt: f32, targets: &[(String, f32)]) {
        for (name, target) in targets {
            if let Some(e) = self.get_mut(name) {
                if !e.chip {
                    e.chip_value = *target;
                    continue;
                }
                if e.chip_value.is_nan() {
                    e.chip_value = *target;
                    continue;
                }
                // Fall fast enough to be read as one event, slow enough to be
                // seen: a full bar empties in about two thirds of a second.
                let rate = 1.6 * dt;
                if e.chip_value > *target {
                    e.chip_value = (e.chip_value - rate).max(*target);
                } else {
                    e.chip_value = *target;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// A placed element: an index into the document and the box it occupies, in
/// pane-local PIXELS with y down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudPlaced {
    pub index: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Default sizes in HUD units, in the one place a renderer and a test can
/// both read them.
pub const BAR_W: f32 = 190.0;
pub const BAR_H: f32 = 18.0;
pub const RING_R: f32 = 34.0;
pub const ICON_D: f32 = 26.0;
/// A map's edge when `size` gives none.
pub const MAP_D: f32 = 220.0;
/// Most polyline points a map keeps.
pub const MAX_MAP_POINTS: usize = 256;
pub const TEXT_SIZE: f32 = 17.0;
pub const NUMERAL_SIZE: f32 = 34.0;
pub const CAPTION_SIZE: f32 = 12.0;
pub const BANNER_SIZE: f32 = 40.0;
/// Distance a root element keeps from the pane's edge when `at` is zero.
pub const MARGIN: f32 = 18.0;

/// The default glyph size for a text element's style.
pub fn text_size_for(style: &str) -> f32 {
    match style {
        "numeral" => NUMERAL_SIZE,
        "caption" => CAPTION_SIZE,
        "banner" => BANNER_SIZE,
        _ => TEXT_SIZE,
    }
}

/// Measure and place every visible element.
///
/// `scale` converts HUD units to pixels (`pane_h / 1080`). `measure` returns
/// the pixel width and height of a string at a PIXEL font size — the one
/// thing layout cannot know on its own, and the only reason this is not a
/// pure function. The renderer passes its text engine; a test passes
/// arithmetic.
///
/// `text_of` supplies the string an element will actually draw, because a
/// bound readout's width depends on the number it shows. Nesting is resolved
/// by walking `parent`; a cycle is dropped rather than hung on.
pub fn layout(
    doc: &HudDoc,
    pane_w: f32,
    pane_h: f32,
    scale: f32,
    measure: &mut dyn FnMut(&str, f32) -> (f32, f32),
    text_of: &mut dyn FnMut(&HudElement) -> String,
) -> Vec<HudPlaced> {
    let n = doc.elements.len();
    let mut order: Vec<usize> = (0..n)
        .filter(|i| doc.elements[*i].show && doc.elements[*i].kind.is_laid_out())
        .collect();
    order.sort_by_key(|i| (doc.elements[*i].order, *i));

    let visible = |name: &str| -> Option<usize> {
        order
            .iter()
            .copied()
            .find(|i| doc.elements[*i].name == name)
    };
    // A parent chain that does not terminate is a cycle; those elements are
    // skipped entirely rather than laid out somewhere arbitrary.
    let rooted = |start: usize| -> bool {
        let mut cur = start;
        for _ in 0..=n {
            let parent = &doc.elements[cur].parent;
            if parent.is_empty() {
                return true;
            }
            match visible(parent) {
                Some(p) => cur = p,
                // A missing or hidden parent falls back to the pane rather
                // than vanishing: half a HUD beats none while an author is
                // still typing.
                None => return true,
            }
        }
        false
    };

    let mut sizes: Vec<(f32, f32)> = vec![(0.0, 0.0); n];
    let mut ctx = Measure { doc, order: &order, scale, measure, text_of };
    for &i in &order {
        if rooted(i) {
            sizes[i] = ctx.of(i, &mut Vec::new(), 0);
        }
    }
    // The child measurements the panel pass computed are the ones to place
    // with, so recompute into the shared table.
    let mut sized = sizes.clone();
    for &i in &order {
        if rooted(i) {
            sized[i] = ctx.of(i, &mut Vec::new(), 0);
            fill_children(&mut ctx, i, &mut sized, 0);
        }
    }

    let mut out = Vec::with_capacity(order.len());
    for &i in &order {
        let e = &doc.elements[i];
        if !e.parent.is_empty() && visible(&e.parent).is_some() {
            continue; // placed by its parent
        }
        if !rooted(i) {
            continue;
        }
        let (w, h) = sized[i];
        let (x, y) = anchor_origin(e.anchor, pane_w, pane_h, w, h, e.at * scale);
        place(doc, i, x, y, w, h, &order, &sized, scale, &mut out);
    }
    out
}

/// Where a root element's box goes. The corner an anchor names is the corner
/// that meets the pane's, and `at` moves INWARD from it — so an author writes
/// positive numbers everywhere and never has to remember which corner needs a
/// negative one.
fn anchor_origin(
    anchor: HudAnchor,
    pane_w: f32,
    pane_h: f32,
    w: f32,
    h: f32,
    at: Vec2f,
) -> (f32, f32) {
    let m = MARGIN;
    match anchor {
        HudAnchor::TopLeft => (m + at.x, m + at.y),
        HudAnchor::Top => ((pane_w - w) * 0.5 + at.x, m + at.y),
        HudAnchor::TopRight => (pane_w - w - m - at.x, m + at.y),
        HudAnchor::Center => ((pane_w - w) * 0.5 + at.x, (pane_h - h) * 0.5 + at.y),
        HudAnchor::BottomLeft => (m + at.x, pane_h - h - m - at.y),
        HudAnchor::Bottom => ((pane_w - w) * 0.5 + at.x, pane_h - h - m - at.y),
        HudAnchor::BottomRight => (pane_w - w - m - at.x, pane_h - h - m - at.y),
    }
}

struct Measure<'a> {
    doc: &'a HudDoc,
    order: &'a [usize],
    scale: f32,
    measure: &'a mut dyn FnMut(&str, f32) -> (f32, f32),
    text_of: &'a mut dyn FnMut(&HudElement) -> String,
}

impl Measure<'_> {
    fn children(&self, name: &str) -> Vec<usize> {
        if name.is_empty() {
            return Vec::new();
        }
        self.order
            .iter()
            .copied()
            .filter(|i| self.doc.elements[*i].parent == name)
            .collect()
    }

    fn of(&mut self, i: usize, seen: &mut Vec<usize>, depth: usize) -> (f32, f32) {
        let e = &self.doc.elements[i];
        if depth > 8 || seen.contains(&i) {
            return (0.0, 0.0);
        }
        seen.push(i);
        let s = self.scale;
        let intrinsic = match e.kind {
            HudKind::Bar => {
                let cap = if e.label.is_empty() {
                    0.0
                } else {
                    self.measure_text(&e.label, CAPTION_SIZE * s).1 + 2.0 * s
                };
                (BAR_W * s, BAR_H * s + cap)
            }
            HudKind::Ring => {
                let r = if e.glyph > 0.0 { e.glyph } else { RING_R };
                (r * 2.0 * s, r * 2.0 * s)
            }
            HudKind::Text => {
                let size = if e.glyph > 0.0 { e.glyph } else { text_size_for(&e.style) } * s;
                let text = (self.text_of)(e);
                let shown = if text.is_empty() { " " } else { text.as_str() };
                let (tw, th) = self.measure_text(shown, size);
                let (lw, lh) = if e.label.is_empty() {
                    (0.0, 0.0)
                } else {
                    self.measure_text(&e.label, CAPTION_SIZE * s)
                };
                let icon = if e.icon.is_empty() && e.svg.is_empty() && e.image.is_empty() {
                    0.0
                } else {
                    size * 0.95 + 4.0 * s
                };
                (tw.max(lw) + icon, th + if lh > 0.0 { lh + 1.0 * s } else { 0.0 })
            }
            HudKind::Icon => {
                let d = if e.glyph > 0.0 { e.glyph } else { ICON_D } * s;
                (d, d)
            }
            HudKind::Log => {
                let size = if e.glyph > 0.0 { e.glyph } else { TEXT_SIZE } * s;
                let rows = e.lines.max(1) as f32;
                (220.0 * s, rows * size * 1.35)
            }
            HudKind::Flash | HudKind::Marker | HudKind::Flight => (0.0, 0.0),
            // Square unless both axes were given: `size: 200` means a
            // 200-unit map, not a 200-wide strip.
            HudKind::Map => {
                let d = if e.size.x > 0.0 { e.size.x } else { MAP_D } * s;
                (d, d)
            }
            HudKind::Panel => {
                let kids = self.children(&e.name);
                let (gap, pad) = (e.gap * s, e.pad * s);
                let (stack, at_scale) = (e.stack, s);
                let mut main = 0.0f32;
                let mut cross = 0.0f32;
                let mut free_w = 0.0f32;
                let mut free_h = 0.0f32;
                for (k, c) in kids.iter().enumerate() {
                    let (cw, ch) = self.of(*c, seen, depth + 1);
                    match stack {
                        HudStack::Row => {
                            main += cw + if k > 0 { gap } else { 0.0 };
                            cross = cross.max(ch);
                        }
                        HudStack::Column => {
                            main += ch + if k > 0 { gap } else { 0.0 };
                            cross = cross.max(cw);
                        }
                        HudStack::Free => {
                            let kid = &self.doc.elements[*c];
                            free_w = free_w.max(kid.at.x * at_scale + cw);
                            free_h = free_h.max(kid.at.y * at_scale + ch);
                        }
                    }
                }
                let (w, h) = match stack {
                    HudStack::Row => (main, cross),
                    HudStack::Column => (cross, main),
                    HudStack::Free => (free_w, free_h),
                };
                (w + pad * 2.0, h + pad * 2.0)
            }
        };
        seen.pop();
        (
            if e.size.x > 0.0 { e.size.x * s } else { intrinsic.0 },
            if e.size.y > 0.0 { e.size.y * s } else { intrinsic.1 },
        )
    }

    fn measure_text(&mut self, text: &str, size: f32) -> (f32, f32) {
        (self.measure)(text, size)
    }
}

/// Re-run the measure over a panel's children so `sized` holds every element,
/// not just the roots. Cheap: the tree is a HUD, not a document.
fn fill_children(ctx: &mut Measure, i: usize, sized: &mut [(f32, f32)], depth: usize) {
    if depth > 8 {
        return;
    }
    let name = ctx.doc.elements[i].name.clone();
    for c in ctx.children(&name) {
        sized[c] = ctx.of(c, &mut Vec::new(), 0);
        fill_children(ctx, c, sized, depth + 1);
    }
}

#[allow(clippy::too_many_arguments)]
fn place(
    doc: &HudDoc,
    i: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    order: &[usize],
    sized: &[(f32, f32)],
    scale: f32,
    out: &mut Vec<HudPlaced>,
) {
    out.push(HudPlaced { index: i, x, y, w, h });
    let e = &doc.elements[i];
    if e.kind != HudKind::Panel {
        return;
    }
    let kids: Vec<usize> = order
        .iter()
        .copied()
        .filter(|c| doc.elements[*c].parent == e.name && !e.name.is_empty())
        .collect();
    if kids.is_empty() {
        return;
    }
    let pad = e.pad * scale;
    let gap = e.gap * scale;
    let inner_x = x + pad;
    let inner_y = y + pad;
    let inner_w = (w - pad * 2.0).max(0.0);
    let inner_h = (h - pad * 2.0).max(0.0);
    match e.stack {
        HudStack::Free => {
            for &c in &kids {
                let kid = &doc.elements[c];
                let (cw, ch) = sized[c];
                place(
                    doc,
                    c,
                    inner_x + kid.at.x * scale,
                    inner_y + kid.at.y * scale,
                    cw,
                    ch,
                    order,
                    sized,
                    scale,
                    out,
                );
            }
        }
        HudStack::Row => {
            let total: f32 =
                kids.iter().map(|c| sized[*c].0).sum::<f32>() + gap * kids.len().saturating_sub(1) as f32;
            let mut cx = inner_x + e.align.offset(inner_w, total).max(0.0);
            for &c in &kids {
                let (cw, ch) = sized[c];
                let cy = inner_y + e.align.offset(inner_h, ch).max(0.0);
                place(doc, c, cx, cy, cw, ch, order, sized, scale, out);
                cx += cw + gap;
            }
        }
        HudStack::Column => {
            let total: f32 =
                kids.iter().map(|c| sized[*c].1).sum::<f32>() + gap * kids.len().saturating_sub(1) as f32;
            let mut cy = inner_y + e.align.offset(inner_h, total).max(0.0);
            for &c in &kids {
                let (cw, ch) = sized[c];
                let cx = inner_x + e.align.offset(inner_w, cw).max(0.0);
                place(doc, c, cx, cy, cw, ch, order, sized, scale, out);
                cy += ch + gap;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Map
// ---------------------------------------------------------------------------

/// One dot a host hands a map for this frame: a world position (x, z), the
/// entity's forward direction on the ground, its race position (0 = none)
/// and whether it is the pane's own subject.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudMapDot {
    pub x: f32,
    pub z: f32,
    pub fx: f32,
    pub fz: f32,
    pub rank: u32,
    pub is_self: bool,
}

/// Thin a world polyline to at most `max` points, keeping the shape: evenly
/// spaced along its length, so a centreline sampled every metre and one
/// sampled every hundred reduce to the same outline. Always keeps the ends.
pub fn decimate_polyline(points: &[Vec2f], max: usize) -> Vec<Vec2f> {
    let max = max.max(2);
    if points.len() <= max {
        return points.to_vec();
    }
    let mut out = Vec::with_capacity(max);
    let last = (points.len() - 1) as f32;
    for k in 0..max {
        let i = ((k as f32 / (max - 1) as f32) * last).round() as usize;
        out.push(points[i.min(points.len() - 1)]);
    }
    out
}

/// World → map-box transform: centre of the polyline's bounds and the
/// pixels-per-metre that fits it inside `w`×`h` minus `pad` on every side.
/// With `rotating`, the fit uses the bounding CIRCLE so the outline stays
/// inside the box at every heading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudMapFit {
    pub cx: f32,
    pub cz: f32,
    pub scale: f32,
    /// Rotation applied after centring (screen radians).
    pub cos: f32,
    pub sin: f32,
}

impl HudMapFit {
    pub fn new(points: &[Vec2f], w: f32, h: f32, pad: f32, rotating: bool) -> Option<HudMapFit> {
        let first = points.first()?;
        let (mut lo, mut hi) = (*first, *first);
        for p in points {
            lo = vec2f(lo.x.min(p.x), lo.y.min(p.y));
            hi = vec2f(hi.x.max(p.x), hi.y.max(p.y));
        }
        let (cx, cz) = ((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5);
        let (aw, ah) = ((w - 2.0 * pad).max(1.0), (h - 2.0 * pad).max(1.0));
        let scale = if rotating {
            let r = points
                .iter()
                .map(|p| ((p.x - cx).powi(2) + (p.y - cz).powi(2)).sqrt())
                .fold(0.0f32, f32::max)
                .max(1.0e-3);
            aw.min(ah) * 0.5 / r
        } else {
            let (sw, sh) = ((hi.x - lo.x).max(1.0e-3), (hi.y - lo.y).max(1.0e-3));
            (aw / sw).min(ah / sh)
        };
        Some(HudMapFit { cx, cz, scale, cos: 1.0, sin: 0.0 })
    }

    /// A radar: centred on (x, z), `range` metres from the centre to a
    /// circle of `radius` pixels.
    pub fn radar(x: f32, z: f32, range: f32, radius: f32) -> HudMapFit {
        HudMapFit { cx: x, cz: z, scale: radius.max(1.0) / range.max(1.0e-3), cos: 1.0, sin: 0.0 }
    }

    /// Turn the map so a ground-forward (fx, fz) points up the screen.
    /// North-up is +x right, −z up (screen y = world z).
    pub fn heading_up(mut self, fx: f32, fz: f32) -> Self {
        let len = (fx * fx + fz * fz).sqrt();
        if len > 1.0e-4 {
            let a = -std::f32::consts::FRAC_PI_2 - fz.atan2(fx);
            self.cos = a.cos();
            self.sin = a.sin();
        }
        self
    }

    /// A world (x, z) as a pixel offset from the box centre (y down).
    pub fn apply(&self, x: f32, z: f32) -> (f32, f32) {
        let (dx, dz) = ((x - self.cx) * self.scale, (z - self.cz) * self.scale);
        (dx * self.cos - dz * self.sin, dx * self.sin + dz * self.cos)
    }

    /// A ground direction in screen space (unit length is kept).
    pub fn apply_dir(&self, fx: f32, fz: f32) -> (f32, f32) {
        (fx * self.cos - fz * self.sin, fx * self.sin + fz * self.cos)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn map_polyline_is_decimated_evenly_with_both_ends() {
        let pts: Vec<Vec2f> = (0..1000).map(|i| vec2f(i as f32, 0.0)).collect();
        let d = decimate_polyline(&pts, MAX_MAP_POINTS);
        assert_eq!(d.len(), MAX_MAP_POINTS);
        assert_eq!(d[0], pts[0]);
        assert_eq!(*d.last().unwrap(), pts[999]);
        assert_eq!(decimate_polyline(&pts[..10], 256).len(), 10);
    }

    #[test]
    fn map_fit_keeps_the_track_inside_the_box_at_any_heading() {
        let pts = vec![vec2f(-100.0, -40.0), vec2f(100.0, -40.0), vec2f(100.0, 40.0), vec2f(-100.0, 40.0)];
        let fit = HudMapFit::new(&pts, 200.0, 200.0, 10.0, false).unwrap();
        // North-up fits the long axis: 200 m into 180 px.
        assert!((fit.scale - 0.9).abs() < 1e-4);
        let (x, y) = fit.apply(100.0, 40.0);
        assert!((x - 90.0).abs() < 1e-3 && (y - 36.0).abs() < 1e-3);
        for &(fx, fz) in &[(1.0, 0.0), (0.0, 1.0), (0.7, -0.7), (-1.0, 0.2)] {
            let rot = HudMapFit::new(&pts, 200.0, 200.0, 10.0, true).unwrap().heading_up(fx, fz);
            for p in &pts {
                let (x, y) = rot.apply(p.x, p.y);
                assert!(x.abs() <= 90.01 && y.abs() <= 90.01, "{x} {y}");
            }
            // The heading itself points up the screen.
            let (dx, dy) = rot.apply_dir(fx, fz);
            assert!(dx.abs() < 1e-3 && dy < 0.0, "{dx} {dy}");
        }
    }

    #[test]
    fn map_measures_square_from_one_size_axis() {
        let mut doc = HudDoc::default();
        doc.set(HudElement { name: "map".into(), kind: HudKind::Map, size: vec2f(150.0, 0.0), ..Default::default() });
        let placed = layout(&doc, 1920.0, 1080.0, 1.0, &mut |_, s| (s, s), &mut |_| String::new());
        assert_eq!(placed.len(), 1);
        assert_eq!((placed[0].w, placed[0].h), (150.0, 150.0));
    }

    use super::*;

    /// A stand-in text engine: every glyph is half the font size wide, every
    /// line is the font size tall. Enough to pin layout without a GPU.
    fn measurer() -> impl FnMut(&str, f32) -> (f32, f32) {
        |text: &str, size: f32| (text.chars().count() as f32 * size * 0.5, size)
    }

    fn texter() -> impl FnMut(&HudElement) -> String {
        |e: &HudElement| e.text.clone()
    }

    fn el(name: &str, kind: HudKind) -> HudElement {
        HudElement { name: name.into(), kind, ..Default::default() }
    }

    fn run(doc: &HudDoc, w: f32, h: f32) -> Vec<HudPlaced> {
        let mut m = measurer();
        let mut t = texter();
        layout(doc, w, h, 1.0, &mut m, &mut t)
    }

    #[test]
    fn redeclaring_an_element_keeps_its_place_in_the_layout() {
        let mut doc = HudDoc::default();
        doc.set(HudElement { text: "one".into(), ..el("t", HudKind::Text) });
        doc.set(el("other", HudKind::Text));
        doc.set(HudElement { text: "two".into(), ..el("t", HudKind::Text) });
        assert_eq!(doc.elements.len(), 2, "a name is an identity, not an append");
        assert_eq!(doc.get("t").unwrap().text, "two");
        assert!(doc.get("t").unwrap().order < doc.get("other").unwrap().order);
    }

    #[test]
    fn a_panel_measures_its_children_and_stacks_them_in_a_row() {
        let mut doc = HudDoc::default();
        doc.set(HudElement {
            pad: 10.0,
            gap: 6.0,
            stack: HudStack::Row,
            anchor: HudAnchor::BottomLeft,
            ..el("bar", HudKind::Panel)
        });
        for name in ["a", "b"] {
            doc.set(HudElement {
                parent: "bar".into(),
                size: vec2f(40.0, 20.0),
                ..el(name, HudKind::Icon)
            });
        }
        let placed = run(&doc, 800.0, 600.0);
        let panel = placed.iter().find(|p| p.index == 0).unwrap();
        assert_eq!(panel.w, 106.0); // 10 + 40 + 6 + 40 + 10
        assert_eq!(panel.h, 40.0);
        assert_eq!(panel.x, MARGIN);
        assert_eq!(panel.y, 600.0 - 40.0 - MARGIN);
        let a = placed.iter().find(|p| p.index == 1).unwrap();
        let b = placed.iter().find(|p| p.index == 2).unwrap();
        assert_eq!(a.x, panel.x + 10.0);
        assert_eq!(b.x, a.x + 46.0);
    }

    /// `at` moves inward from whichever corner the anchor names, so an author
    /// writes positive numbers everywhere. Getting this wrong sends half the
    /// HUDs ever written off-screen.
    #[test]
    fn at_moves_inward_from_every_corner() {
        let mut doc = HudDoc::default();
        doc.set(HudElement {
            size: vec2f(100.0, 50.0),
            anchor: HudAnchor::BottomRight,
            at: vec2f(20.0, 30.0),
            ..el("p", HudKind::Panel)
        });
        let p = run(&doc, 800.0, 600.0)[0];
        assert_eq!(p.x + p.w, 800.0 - MARGIN - 20.0);
        assert_eq!(p.y + p.h, 600.0 - MARGIN - 30.0);
        doc.get_mut("p").unwrap().anchor = HudAnchor::TopLeft;
        let p = run(&doc, 800.0, 600.0)[0];
        assert_eq!(p.x, MARGIN + 20.0);
        assert_eq!(p.y, MARGIN + 30.0);
    }

    /// Everything is in units of 1/1080 of the pane, so the same declaration
    /// is the same size on a phone and on a wall.
    #[test]
    fn hud_units_scale_with_the_pane() {
        let mut doc = HudDoc::default();
        doc.set(HudElement {
            size: vec2f(100.0, 50.0),
            anchor: HudAnchor::TopLeft,
            ..el("p", HudKind::Panel)
        });
        let mut m = measurer();
        let mut t = texter();
        let small = layout(&doc, 960.0, 540.0, 0.5, &mut m, &mut t)[0];
        let big = layout(&doc, 3840.0, 2160.0, 2.0, &mut m, &mut t)[0];
        assert_eq!(small.w, 50.0);
        assert_eq!(big.w, 200.0);
    }

    #[test]
    fn a_free_panel_places_children_at_their_own_offsets() {
        let mut doc = HudDoc::default();
        doc.set(HudElement {
            stack: HudStack::Free,
            pad: 4.0,
            size: vec2f(200.0, 60.0),
            ..el("p", HudKind::Panel)
        });
        doc.set(HudElement {
            parent: "p".into(),
            at: vec2f(30.0, 10.0),
            size: vec2f(20.0, 20.0),
            ..el("k", HudKind::Icon)
        });
        let placed = run(&doc, 800.0, 600.0);
        let panel = placed.iter().find(|p| p.index == 0).unwrap();
        let kid = placed.iter().find(|p| p.index == 1).unwrap();
        assert_eq!(kid.x, panel.x + 4.0 + 30.0);
        assert_eq!(kid.y, panel.y + 4.0 + 10.0);
    }

    #[test]
    fn removing_a_panel_removes_what_it_held() {
        let mut doc = HudDoc::default();
        doc.set(el("p", HudKind::Panel));
        doc.set(HudElement { parent: "p".into(), ..el("k", HudKind::Panel) });
        doc.set(HudElement { parent: "k".into(), ..el("deep", HudKind::Icon) });
        doc.set(el("other", HudKind::Icon));
        doc.remove("p");
        assert_eq!(doc.elements.len(), 1);
        assert!(doc.get("other").is_some());
    }

    /// A generated HUD that parents a panel to itself must not hang the
    /// renderer; it simply does not draw.
    #[test]
    fn a_parent_cycle_is_dropped_not_hung_on() {
        let mut doc = HudDoc::default();
        doc.set(HudElement { parent: "b".into(), ..el("a", HudKind::Panel) });
        doc.set(HudElement { parent: "a".into(), ..el("b", HudKind::Panel) });
        doc.set(el("fine", HudKind::Icon));
        let placed = run(&doc, 800.0, 600.0);
        assert_eq!(placed.len(), 1);
        assert_eq!(doc.elements[placed[0].index].name, "fine");
    }

    /// A flash covers the pane and a marker sits at the crosshair; neither may
    /// push a panel's children around by occupying a slot.
    #[test]
    fn flashes_and_markers_take_up_no_room() {
        let mut doc = HudDoc::default();
        doc.set(HudElement { stack: HudStack::Row, pad: 0.0, gap: 0.0, ..el("p", HudKind::Panel) });
        doc.set(HudElement {
            parent: "p".into(),
            size: vec2f(10.0, 10.0),
            ..el("i", HudKind::Icon)
        });
        doc.set(HudElement { parent: "p".into(), ..el("f", HudKind::Flash) });
        let placed = run(&doc, 800.0, 600.0);
        assert_eq!(placed.iter().find(|p| p.index == 0).unwrap().w, 10.0);
        assert!(placed.iter().all(|p| p.index != 2));
    }

    #[test]
    fn a_pulse_restarts_rather_than_stuttering() {
        let mut doc = HudDoc::default();
        doc.set(HudElement { on: "damage".into(), ms: 400.0, ..el("hurt", HudKind::Flash) });
        doc.fire("damage");
        doc.advance(0.3);
        assert!(doc.get("hurt").unwrap().pulse.strength() < 0.3);
        doc.fire("damage");
        assert_eq!(doc.get("hurt").unwrap().pulse.strength(), 1.0);
        doc.advance(0.5);
        assert_eq!(doc.get("hurt").unwrap().pulse.strength(), 0.0);
    }

    #[test]
    fn a_log_keeps_only_what_it_can_show() {
        let mut doc = HudDoc::default();
        doc.set(HudElement { events: vec!["kill".into()], ..el("feed", HudKind::Log) });
        assert_eq!(doc.logs_for("kill"), vec!["feed".to_string()]);
        assert!(doc.logs_for("lap").is_empty());
        for i in 0..40 {
            doc.say(HudLine {
                text: format!("line {i}"),
                color: vec4(1.0, 1.0, 1.0, 1.0),
                icon: String::new(),
                target: "feed".into(),
                secs: 4.0,
                age: 0.0,
            });
        }
        assert_eq!(doc.lines.len(), MAX_LINES);
        doc.advance(5.0);
        assert!(doc.lines.is_empty(), "lines expire on their own");
    }

    /// A banner given a lifetime removes itself, which is the whole point of
    /// `ms:` — no `game.after` and no counter in `on_tick`.
    #[test]
    fn a_timed_element_removes_itself() {
        let mut doc = HudDoc::default();
        doc.set(HudElement { expires: 0.5, text: "GO".into(), ..el("go", HudKind::Text) });
        doc.advance(0.3);
        assert!(doc.get("go").is_some());
        doc.advance(0.3);
        assert!(doc.get("go").is_none());
    }

    #[test]
    fn a_chip_gauge_lags_down_and_snaps_up() {
        let mut doc = HudDoc::default();
        doc.set(HudElement { chip: true, ..el("hp", HudKind::Bar) });
        doc.settle_chips(0.016, &[("hp".into(), 1.0)]);
        assert_eq!(doc.get("hp").unwrap().chip_value, 1.0);
        doc.settle_chips(0.016, &[("hp".into(), 0.2)]);
        let after = doc.get("hp").unwrap().chip_value;
        assert!(after < 1.0 && after > 0.2, "it falls, but not instantly: {after}");
        doc.settle_chips(1.0, &[("hp".into(), 0.2)]);
        assert_eq!(doc.get("hp").unwrap().chip_value, 0.2);
        // Healing is not a wound: the trailing bar jumps straight up.
        doc.settle_chips(0.016, &[("hp".into(), 0.9)]);
        assert_eq!(doc.get("hp").unwrap().chip_value, 0.9);
    }

    #[test]
    fn the_document_grows_with_what_the_author_declares() {
        let mut doc = HudDoc::default();
        for i in 0..500 {
            doc.set(el(&format!("e{i}"), HudKind::Icon));
        }
        assert_eq!(doc.elements.len(), 500);
        // Re-declaring an element updates it in place.
        doc.set(el("e7", HudKind::Text));
        assert_eq!(doc.elements.len(), 500);
    }
}

/// Where a HUD slot pins to the pane. Slots sharing an anchor stack downward
/// in insertion order.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum HudAnchor {
    TopLeft,
    Top,
    TopRight,
    #[default]
    Center,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl HudAnchor {
    pub fn parse(name: &str) -> HudAnchor {
        match name {
            "top_left" => HudAnchor::TopLeft,
            "top" => HudAnchor::Top,
            "top_right" => HudAnchor::TopRight,
            "bottom_left" => HudAnchor::BottomLeft,
            "bottom" => HudAnchor::Bottom,
            "bottom_right" => HudAnchor::BottomRight,
            _ => HudAnchor::Center,
        }
    }
}

/// One line of screen text. `size`/`color.w` of 0 mean "use the slot default".
#[derive(Clone, Debug, Default)]
pub struct HudSlot {
    pub text: String,
    pub color: Vec4f,
    pub size: f32,
    pub anchor: HudAnchor,
}

/// A HUD gauge (speedometer, boost). Fraction 0..1 fills left to right.
#[derive(Clone, Debug)]
pub struct HudBar {
    pub name: String,
    pub fraction: f32,
    pub color: Vec4f,
    pub anchor: HudAnchor,
}

