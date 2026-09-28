//! The plain data a tween or timeline is built from: GSAP's `vars` object,
//! split into `Copy` values (targets, property ends, options, positions).
//!
//! Everything here is inert data with `const` builders, so a motion can be a
//! `const` item and a later script layer only has to fill these structs.

use crate::easing::{Easing, YoyoEase};
use crate::ids::{PathId, PropKey, Tag, TargetId};
use crate::stagger::Stagger;
use crate::value::{ColorSpace, TweenValue};

/// The targets of a tween: GSAP's first argument (`gsap.to(targets, vars)`).
#[derive(Clone, Copy, Debug)]
pub enum Targets<'a> {
    /// One target.
    One(TargetId),
    /// `count` consecutive targets starting at `first`.
    Range {
        /// The first target id.
        first: u32,
        /// How many targets.
        count: u32,
    },
    /// An explicit list, in stagger order.
    List(&'a [TargetId]),
}

impl From<TargetId> for Targets<'_> {
    fn from(t: TargetId) -> Self {
        Targets::One(t)
    }
}

impl<'a> From<&'a [TargetId]> for Targets<'a> {
    fn from(list: &'a [TargetId]) -> Self {
        Targets::List(list)
    }
}

impl Targets<'_> {
    /// How many targets.
    pub fn len(&self) -> u32 {
        match *self {
            Targets::One(_) => 1,
            Targets::Range { count, .. } => count,
            Targets::List(l) => l.len() as u32,
        }
    }

    /// Whether there are no targets.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The `i`-th target (`i < len()`; `One` answers its target for any `i`).
    pub fn get(&self, i: u32) -> TargetId {
        match *self {
            Targets::One(t) => t,
            Targets::Range { first, .. } => TargetId(first.wrapping_add(i)),
            Targets::List(l) => l[i as usize],
        }
    }
}

/// Where a property ends up: the value side of a GSAP vars entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum End<'a> {
    /// An absolute value (`x: 100`).
    To(TweenValue),
    /// Relative to the captured start (`x: "+=100"`; negative for `"-="`).
    By(TweenValue),
    /// The slot's value when the tween captures its start (`gsap.from` ends
    /// where the target currently is).
    Current,
    /// One value per target: target `i` gets `values[min(i, len - 1)]` (the
    /// plain-data form of GSAP function-based values).
    Each(&'a [TweenValue]),
    /// Percent keyframes (`keyframes: {"0%": .., "50%": ..}`).
    Keys(&'a [Key]),
    /// Value keyframes (`x: [0, 100, 50]`), evenly spaced.
    Values(&'a [TweenValue]),
    /// A motion path (GSAP `motionPath`): the prop's key is x, the path
    /// drives y (and z, and a rotation) as well. See [`PropTo::path`].
    Path(PathTo),
}

/// Where a motion path sits relative to its target (GSAP `align`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PathAlign {
    /// The path's own coordinates (GSAP's default: no `align`).
    #[default]
    None,
    /// The path is moved so its point at `start` sits on the target's
    /// current x / y (/ z), captured when the tween starts (GSAP
    /// `align: self`); each key aligns on its own slot.
    Start,
}

/// GSAP `autoRotate`: a property that follows the path's xy heading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoRotate {
    /// The property written (a rotation).
    pub key: PropKey,
    /// Added to the heading, in degrees (GSAP `autoRotate: 90`).
    pub offset_deg: f64,
    /// Write radians instead of degrees.
    pub radians: bool,
}

/// How a tween follows a motion path: GSAP's `motionPath` options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathOpts {
    /// GSAP `start`: the fraction of the path's arc length where the tween
    /// starts (may leave 0..1 on a closed path, which wraps).
    pub start: f64,
    /// GSAP `end`: where it ends (may be below `start`: the path is
    /// travelled backwards).
    pub end: f64,
    /// GSAP `offsetX` / `offsetY` (and z): added to every written position.
    pub offset: [f64; 3],
    /// GSAP `align`.
    pub align: PathAlign,
    /// GSAP `autoRotate`.
    pub auto_rotate: Option<AutoRotate>,
    /// A z property, written from the path's third coordinate (3D paths);
    /// `None` for 2D use.
    pub z: Option<PropKey>,
}

impl Default for PathOpts {
    fn default() -> Self {
        Self::new()
    }
}

impl PathOpts {
    /// The whole path (`start` 0, `end` 1), no offset, no align, no
    /// rotation, no z.
    pub const fn new() -> Self {
        Self {
            start: 0.0,
            end: 1.0,
            offset: [0.0; 3],
            align: PathAlign::None,
            auto_rotate: None,
            z: None,
        }
    }

    /// GSAP `start` / `end`.
    pub const fn span(self, start: f64, end: f64) -> Self {
        Self { start, end, ..self }
    }

    /// GSAP `offsetX` / `offsetY` (the z offset is kept).
    pub const fn offset(self, dx: f64, dy: f64) -> Self {
        Self {
            offset: [dx, dy, self.offset[2]],
            ..self
        }
    }

    /// An x / y / z offset.
    pub const fn offset3(self, dx: f64, dy: f64, dz: f64) -> Self {
        Self {
            offset: [dx, dy, dz],
            ..self
        }
    }

    /// GSAP `align`.
    pub const fn align(self, a: PathAlign) -> Self {
        Self { align: a, ..self }
    }

    /// GSAP `autoRotate: offset_deg`: `key` gets the heading plus
    /// `offset_deg`, in degrees.
    pub const fn auto_rotate(self, key: PropKey, offset_deg: f64) -> Self {
        Self {
            auto_rotate: Some(AutoRotate {
                key,
                offset_deg,
                radians: false,
            }),
            ..self
        }
    }

    /// [`PathOpts::auto_rotate`], written in radians (GSAP `useRadians`).
    pub const fn auto_rotate_radians(self, key: PropKey, offset_deg: f64) -> Self {
        Self {
            auto_rotate: Some(AutoRotate {
                key,
                offset_deg,
                radians: true,
            }),
            ..self
        }
    }

    /// Writes the path's z coordinate to `key` (a 3D path).
    pub const fn z(self, key: PropKey) -> Self {
        Self {
            z: Some(key),
            ..self
        }
    }
}

/// The motion path of an [`End::Path`] prop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathTo {
    /// The y property (the prop's own key is x).
    pub y: PropKey,
    /// The path, from [`crate::TweenEngine::add_path`].
    pub path: PathId,
    /// Span, offset, align, rotation and z.
    pub opts: PathOpts,
}

/// One animated property of a tween: key, optional explicit start, end, and
/// snap increment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PropTo<'a> {
    /// The property.
    pub key: PropKey,
    /// The explicit start (`gsap.from` / `fromTo`); `None` captures the
    /// current value when the tween first renders.
    pub from: Option<TweenValue>,
    /// Where it ends.
    pub to: End<'a>,
    /// GSAP `snap`: the output is rounded to a multiple of this; 0 = off.
    pub snap: f64,
}

impl<'a> PropTo<'a> {
    /// `gsap.to`: from the current value to `v`.
    pub const fn to(key: PropKey, v: TweenValue) -> Self {
        Self {
            key,
            from: None,
            to: End::To(v),
            snap: 0.0,
        }
    }

    /// `gsap.to` with a plain number.
    pub const fn to_f64(key: PropKey, v: f64) -> Self {
        Self::to(key, TweenValue::F64(v))
    }

    /// Relative `"+="`: from the current value to current + `dv`.
    pub const fn by(key: PropKey, dv: TweenValue) -> Self {
        Self {
            key,
            from: None,
            to: End::By(dv),
            snap: 0.0,
        }
    }

    /// `gsap.from`: from `v` to the value the target has when the tween captures.
    pub const fn from(key: PropKey, v: TweenValue) -> Self {
        Self {
            key,
            from: Some(v),
            to: End::Current,
            snap: 0.0,
        }
    }

    /// `gsap.fromTo`: from `a` to `b`.
    pub const fn from_to(key: PropKey, a: TweenValue, b: TweenValue) -> Self {
        Self {
            key,
            from: Some(a),
            to: End::To(b),
            snap: 0.0,
        }
    }

    /// One end value per target (see [`End::Each`]).
    pub const fn each(key: PropKey, vs: &'a [TweenValue]) -> Self {
        Self {
            key,
            from: None,
            to: End::Each(vs),
            snap: 0.0,
        }
    }

    /// Percent keyframes (see [`End::Keys`]).
    pub const fn keys(key: PropKey, ks: &'a [Key]) -> Self {
        Self {
            key,
            from: None,
            to: End::Keys(ks),
            snap: 0.0,
        }
    }

    /// Value keyframes (see [`End::Values`]).
    pub const fn values(key: PropKey, vs: &'a [TweenValue]) -> Self {
        Self {
            key,
            from: None,
            to: End::Values(vs),
            snap: 0.0,
        }
    }

    /// GSAP `motionPath`: `x` and `y` (and `opts.z`, and the auto-rotate
    /// key) follow path `path` from its point at `opts.start` to its point
    /// at `opts.end`; the tween's eased ratio is clamped to 0..1 on the path
    /// (an overshooting ease rests at the ends). The start is the path's,
    /// not the target's current value (unless `opts.align` is
    /// [`PathAlign::Start`]). `snap` rounds the positions (x, y, z), never
    /// the rotation.
    pub const fn path(x: PropKey, y: PropKey, path: PathId, opts: PathOpts) -> Self {
        Self {
            key: x,
            from: None,
            to: End::Path(PathTo { y, path, opts }),
            snap: 0.0,
        }
    }

    /// GSAP `snap` / `roundProps`: round the output to multiples of `increment`.
    pub const fn snap(self, increment: f64) -> Self {
        Self {
            snap: increment,
            ..self
        }
    }
}

/// One percent keyframe: GSAP `keyframes: {"50%": {x: 100, ease: ..}}`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    /// Position in percent of the tween's duration, 0..=100.
    pub at: f64,
    /// The value at that point.
    pub value: TweenValue,
    /// The ease into this key (GSAP per-keyframe `ease`); `None` uses `ease_each`.
    pub ease: Option<Easing>,
}

/// One step of an array keyframe tween: GSAP `keyframes: [{x: 100, duration: 1}, ..]`.
#[derive(Clone, Copy, Debug)]
pub struct KeyStep<'a> {
    /// The properties this step moves.
    pub props: &'a [PropTo<'a>],
    /// This step's options (duration, ease, delay, events, ...).
    pub opts: TweenOpts,
}

/// GSAP `overwrite`: what a new tween does to other tweens of the same
/// properties.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Overwrite {
    /// `false` (GSAP default): nothing is killed; the last render wins.
    #[default]
    None,
    /// `"auto"`: at its first render, kill the overlapping properties of
    /// active rivals.
    Auto,
    /// `true`: at creation, kill every tween of the same targets.
    All,
}

/// What reduced motion (or `finish`) does to an animation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Reduce {
    /// Jump to the end state, firing the end callbacks.
    #[default]
    JumpToEnd,
    /// Stay at the start state and stop.
    Freeze,
    /// Keep animating (essential motion).
    Keep,
}

/// Whether a playhead jump fires callbacks: GSAP's `suppressEvents` argument,
/// inverted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Emit {
    /// Fire nothing (GSAP `suppressEvents: true`, the `seek` default).
    Suppress,
    /// Fire every crossed callback (GSAP `suppressEvents: false`).
    Fire,
}

/// Which callbacks an animation reports as [`crate::TweenEvent`]s: GSAP's
/// `onStart`, `onUpdate`, ... given as a bit set (events are opt-in).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct EventMask(pub u8);

impl EventMask {
    /// No events.
    pub const NONE: Self = Self(0);
    /// `onStart`.
    pub const START: Self = Self(1);
    /// `onUpdate`.
    pub const UPDATE: Self = Self(2);
    /// `onRepeat`.
    pub const REPEAT: Self = Self(4);
    /// `onComplete`.
    pub const COMPLETE: Self = Self(8);
    /// `onReverseComplete`.
    pub const REVERSE_COMPLETE: Self = Self(16);
    /// `onInterrupt` (killed before completing).
    pub const INTERRUPT: Self = Self(32);
    /// Labels crossed by a timeline's playhead (not GSAP; timelines only).
    pub const LABELS: Self = Self(64);
    /// Every edge event: start, repeat, complete, reverse complete, interrupt.
    pub const EDGES: Self = Self(1 | 4 | 8 | 16 | 32);
    /// Everything.
    pub const ALL: Self = Self(127);

    /// The union of both masks.
    pub const fn with(self, o: Self) -> Self {
        Self(self.0 | o.0)
    }

    /// Whether every bit of `o` is set.
    pub const fn has(self, o: Self) -> bool {
        self.0 & o.0 == o.0
    }
}

/// A tween's options: the non-property keys of GSAP's `vars`.
///
/// Every field is optional; an unset field inherits from the parent
/// timeline's `defaults`, then the engine defaults (GSAP's defaults chain).
/// `tag` and `events` are never inherited.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct TweenOpts {
    /// `duration` in seconds (engine default 0.5).
    pub duration: Option<f64>,
    /// `delay` in seconds.
    pub delay: Option<f64>,
    /// `ease` (engine default `power1.out`).
    pub ease: Option<Easing>,
    /// `easeEach`: the default ease between keyframes.
    pub ease_each: Option<Easing>,
    /// `yoyoEase`; setting it implies `yoyo`.
    pub yoyo_ease: Option<YoyoEase>,
    /// `repeat`: extra iterations, -1 forever.
    pub repeat: Option<i32>,
    /// `repeatDelay` in seconds.
    pub repeat_delay: Option<f64>,
    /// `repeatRefresh`: recapture start values on every repeat.
    pub repeat_refresh: Option<bool>,
    /// `yoyo`: odd iterations run backwards.
    pub yoyo: Option<bool>,
    /// `stagger`: spread the tween over its targets.
    pub stagger: Option<Stagger>,
    /// `overwrite`.
    pub overwrite: Option<Overwrite>,
    /// `immediateRender`.
    pub immediate_render: Option<bool>,
    /// `paused`.
    pub paused: Option<bool>,
    /// `reversed`.
    pub reversed: Option<bool>,
    /// `timeScale`.
    pub time_scale: Option<f64>,
    /// Colour interpolation space (not GSAP).
    pub color_space: Option<ColorSpace>,
    /// Reduced-motion policy (not GSAP).
    pub reduce: Option<Reduce>,
    /// Keep the animation alive after it completes (not GSAP; GSAP keeps
    /// every referenced animation).
    pub keep: Option<bool>,
    /// `inherit`: whether to inherit the parent timeline's defaults.
    pub inherit: Option<bool>,
    /// GSAP `id`: the tag events carry and `get_by_tag` finds.
    pub tag: Tag,
    /// The callbacks to report.
    pub events: EventMask,
}

impl TweenOpts {
    /// Every field unset.
    pub const fn new() -> Self {
        Self {
            duration: None,
            delay: None,
            ease: None,
            ease_each: None,
            yoyo_ease: None,
            repeat: None,
            repeat_delay: None,
            repeat_refresh: None,
            yoyo: None,
            stagger: None,
            overwrite: None,
            immediate_render: None,
            paused: None,
            reversed: None,
            time_scale: None,
            color_space: None,
            reduce: None,
            keep: None,
            inherit: None,
            tag: Tag::NONE,
            events: EventMask::NONE,
        }
    }

    /// Sets `duration`.
    pub const fn duration(self, s: f64) -> Self {
        Self {
            duration: Some(s),
            ..self
        }
    }

    /// Sets `delay`.
    pub const fn delay(self, s: f64) -> Self {
        Self {
            delay: Some(s),
            ..self
        }
    }

    /// Sets `ease`.
    pub const fn ease(self, e: Easing) -> Self {
        Self {
            ease: Some(e),
            ..self
        }
    }

    /// Sets `easeEach`.
    pub const fn ease_each(self, e: Easing) -> Self {
        Self {
            ease_each: Some(e),
            ..self
        }
    }

    /// Sets `yoyoEase`.
    pub const fn yoyo_ease(self, y: YoyoEase) -> Self {
        Self {
            yoyo_ease: Some(y),
            ..self
        }
    }

    /// Sets `repeat` (-1 = forever).
    pub const fn repeat(self, n: i32) -> Self {
        Self {
            repeat: Some(n),
            ..self
        }
    }

    /// Sets `repeatDelay`.
    pub const fn repeat_delay(self, s: f64) -> Self {
        Self {
            repeat_delay: Some(s),
            ..self
        }
    }

    /// Sets `repeatRefresh`.
    pub const fn repeat_refresh(self, b: bool) -> Self {
        Self {
            repeat_refresh: Some(b),
            ..self
        }
    }

    /// Sets `yoyo`.
    pub const fn yoyo(self, b: bool) -> Self {
        Self {
            yoyo: Some(b),
            ..self
        }
    }

    /// Sets `stagger`.
    pub const fn stagger(self, s: Stagger) -> Self {
        Self {
            stagger: Some(s),
            ..self
        }
    }

    /// Sets `overwrite`.
    pub const fn overwrite(self, o: Overwrite) -> Self {
        Self {
            overwrite: Some(o),
            ..self
        }
    }

    /// Sets `immediateRender`.
    pub const fn immediate_render(self, b: bool) -> Self {
        Self {
            immediate_render: Some(b),
            ..self
        }
    }

    /// Sets `paused`.
    pub const fn paused(self, b: bool) -> Self {
        Self {
            paused: Some(b),
            ..self
        }
    }

    /// Sets `reversed`.
    pub const fn reversed(self, b: bool) -> Self {
        Self {
            reversed: Some(b),
            ..self
        }
    }

    /// Sets `timeScale`.
    pub const fn time_scale(self, k: f64) -> Self {
        Self {
            time_scale: Some(k),
            ..self
        }
    }

    /// Sets the colour interpolation space.
    pub const fn color_space(self, c: ColorSpace) -> Self {
        Self {
            color_space: Some(c),
            ..self
        }
    }

    /// Sets the reduced-motion policy.
    pub const fn reduce(self, r: Reduce) -> Self {
        Self {
            reduce: Some(r),
            ..self
        }
    }

    /// Sets whether the animation stays alive after completing.
    pub const fn keep(self, b: bool) -> Self {
        Self {
            keep: Some(b),
            ..self
        }
    }

    /// Sets `inherit`.
    pub const fn inherit(self, b: bool) -> Self {
        Self {
            inherit: Some(b),
            ..self
        }
    }

    /// Sets the tag (GSAP `id`).
    pub const fn tag(self, t: Tag) -> Self {
        Self { tag: t, ..self }
    }

    /// Replaces the event mask.
    pub const fn events(self, m: EventMask) -> Self {
        Self { events: m, ..self }
    }

    /// Reports `onStart`.
    pub const fn on_start(self) -> Self {
        self.events(self.events.with(EventMask::START))
    }

    /// Reports `onUpdate`.
    pub const fn on_update(self) -> Self {
        self.events(self.events.with(EventMask::UPDATE))
    }

    /// Reports `onRepeat`.
    pub const fn on_repeat(self) -> Self {
        self.events(self.events.with(EventMask::REPEAT))
    }

    /// Reports `onComplete`.
    pub const fn on_complete(self) -> Self {
        self.events(self.events.with(EventMask::COMPLETE))
    }

    /// Reports `onReverseComplete`.
    pub const fn on_reverse_complete(self) -> Self {
        self.events(self.events.with(EventMask::REVERSE_COMPLETE))
    }

    /// Reports `onInterrupt`.
    pub const fn on_interrupt(self) -> Self {
        self.events(self.events.with(EventMask::INTERRUPT))
    }

    /// Field-wise inheritance: every field set here wins, every unset field
    /// takes `parent`'s. `tag` and `events` are never inherited.
    pub fn or(self, parent: &TweenOpts) -> Self {
        Self {
            duration: self.duration.or(parent.duration),
            delay: self.delay.or(parent.delay),
            ease: self.ease.or(parent.ease),
            ease_each: self.ease_each.or(parent.ease_each),
            yoyo_ease: self.yoyo_ease.or(parent.yoyo_ease),
            repeat: self.repeat.or(parent.repeat),
            repeat_delay: self.repeat_delay.or(parent.repeat_delay),
            repeat_refresh: self.repeat_refresh.or(parent.repeat_refresh),
            yoyo: self.yoyo.or(parent.yoyo),
            stagger: self.stagger.or(parent.stagger),
            overwrite: self.overwrite.or(parent.overwrite),
            immediate_render: self.immediate_render.or(parent.immediate_render),
            paused: self.paused.or(parent.paused),
            reversed: self.reversed.or(parent.reversed),
            time_scale: self.time_scale.or(parent.time_scale),
            color_space: self.color_space.or(parent.color_space),
            reduce: self.reduce.or(parent.reduce),
            keep: self.keep.or(parent.keep),
            inherit: self.inherit.or(parent.inherit),
            tag: self.tag,
            events: self.events,
        }
    }

    /// The same options with every non-finite number unset, so it falls
    /// through to the inherited or built-in value.
    pub(crate) fn finite(self) -> Self {
        let f = |x: Option<f64>| x.filter(|v| v.is_finite());
        Self {
            duration: f(self.duration),
            delay: f(self.delay),
            repeat_delay: f(self.repeat_delay),
            time_scale: f(self.time_scale),
            ..self
        }
    }
}

/// A timeline's options: GSAP `gsap.timeline(vars)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineOpts {
    /// GSAP `defaults`: inherited by children created through this timeline.
    pub defaults: TweenOpts,
    /// `delay` in seconds.
    pub delay: f64,
    /// `repeat`: extra iterations, -1 forever.
    pub repeat: i32,
    /// `repeatDelay` in seconds.
    pub repeat_delay: f64,
    /// `repeatRefresh`.
    pub repeat_refresh: bool,
    /// `yoyo`.
    pub yoyo: bool,
    /// `paused`.
    pub paused: bool,
    /// `reversed`.
    pub reversed: bool,
    /// `timeScale` (1.0).
    pub time_scale: f64,
    /// `smoothChildTiming` (GSAP default false).
    pub smooth_child_timing: bool,
    /// `autoRemoveChildren` (GSAP default false).
    pub auto_remove_children: bool,
    /// A completed root-level timeline stays alive to be restarted or
    /// reversed later (default true).
    pub keep: bool,
    /// Reduced-motion policy.
    pub reduce: Reduce,
    /// GSAP `id`.
    pub tag: Tag,
    /// The callbacks to report ([`EventMask::LABELS`] watches labels).
    pub events: EventMask,
}

impl TimelineOpts {
    /// Time scale 1, `keep` true, everything else off / zero / none.
    pub const fn new() -> Self {
        Self {
            defaults: TweenOpts::new(),
            delay: 0.0,
            repeat: 0,
            repeat_delay: 0.0,
            repeat_refresh: false,
            yoyo: false,
            paused: false,
            reversed: false,
            time_scale: 1.0,
            smooth_child_timing: false,
            auto_remove_children: false,
            keep: true,
            reduce: Reduce::JumpToEnd,
            tag: Tag::NONE,
            events: EventMask::NONE,
        }
    }

    /// Sets `defaults`.
    pub const fn defaults(self, d: TweenOpts) -> Self {
        Self {
            defaults: d,
            ..self
        }
    }

    /// Sets `delay`.
    pub const fn delay(self, s: f64) -> Self {
        Self { delay: s, ..self }
    }

    /// Sets `repeat` (-1 = forever).
    pub const fn repeat(self, n: i32) -> Self {
        Self { repeat: n, ..self }
    }

    /// Sets `repeatDelay`.
    pub const fn repeat_delay(self, s: f64) -> Self {
        Self {
            repeat_delay: s,
            ..self
        }
    }

    /// Sets `repeatRefresh`.
    pub const fn repeat_refresh(self, b: bool) -> Self {
        Self {
            repeat_refresh: b,
            ..self
        }
    }

    /// Sets `yoyo`.
    pub const fn yoyo(self, b: bool) -> Self {
        Self { yoyo: b, ..self }
    }

    /// Sets `paused`.
    pub const fn paused(self, b: bool) -> Self {
        Self { paused: b, ..self }
    }

    /// Sets `reversed`.
    pub const fn reversed(self, b: bool) -> Self {
        Self {
            reversed: b,
            ..self
        }
    }

    /// Sets `timeScale`.
    pub const fn time_scale(self, k: f64) -> Self {
        Self {
            time_scale: k,
            ..self
        }
    }

    /// Sets `smoothChildTiming`.
    pub const fn smooth_child_timing(self, b: bool) -> Self {
        Self {
            smooth_child_timing: b,
            ..self
        }
    }

    /// Sets `autoRemoveChildren`.
    pub const fn auto_remove_children(self, b: bool) -> Self {
        Self {
            auto_remove_children: b,
            ..self
        }
    }

    /// Sets whether the completed timeline stays alive.
    pub const fn keep(self, b: bool) -> Self {
        Self { keep: b, ..self }
    }

    /// Sets the reduced-motion policy.
    pub const fn reduce(self, r: Reduce) -> Self {
        Self { reduce: r, ..self }
    }

    /// Sets the tag (GSAP `id`).
    pub const fn tag(self, t: Tag) -> Self {
        Self { tag: t, ..self }
    }

    /// Replaces the event mask.
    pub const fn events(self, m: EventMask) -> Self {
        Self { events: m, ..self }
    }

    /// Reports labels the playhead crosses ([`EventMask::LABELS`]).
    pub const fn watch_labels(self) -> Self {
        self.events(self.events.with(EventMask::LABELS))
    }

    /// Reports `onStart`.
    pub const fn on_start(self) -> Self {
        self.events(self.events.with(EventMask::START))
    }

    /// Reports `onUpdate`.
    pub const fn on_update(self) -> Self {
        self.events(self.events.with(EventMask::UPDATE))
    }

    /// Reports `onRepeat`.
    pub const fn on_repeat(self) -> Self {
        self.events(self.events.with(EventMask::REPEAT))
    }

    /// Reports `onComplete`.
    pub const fn on_complete(self) -> Self {
        self.events(self.events.with(EventMask::COMPLETE))
    }

    /// Reports `onReverseComplete`.
    pub const fn on_reverse_complete(self) -> Self {
        self.events(self.events.with(EventMask::REVERSE_COMPLETE))
    }

    /// Reports `onInterrupt`.
    pub const fn on_interrupt(self) -> Self {
        self.events(self.events.with(EventMask::INTERRUPT))
    }
}

impl Default for TimelineOpts {
    fn default() -> Self {
        Self::new()
    }
}

/// What a timeline position is measured from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// The timeline's start (an absolute time such as `2`).
    Zero,
    /// The timeline's end (no position, `"+=1"`, `"-=1"`).
    End,
    /// The start of the most recently added child (`"<"`).
    PrevStart,
    /// The end of the most recently added child, repeats included (`">"`).
    PrevEnd,
    /// A label (`"intro"`); a missing label is created at the timeline's end.
    Label(Tag),
}

/// The offset of a timeline position from its [`Anchor`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Offset {
    /// Seconds.
    Secs(f64),
    /// Percent (50.0 = 50 %) of the previous child's total duration
    /// (`"<25%"`, `">50%"`).
    PercentOfPrev(f64),
    /// Percent (50.0 = 50 %) of the inserted child's total duration
    /// (`"+=50%"`, `"<+=25%"`, `"intro+=50%"`).
    PercentOfChild(f64),
}

/// Where a child goes in a timeline: GSAP's position parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    /// What it is measured from.
    pub anchor: Anchor,
    /// How far from it.
    pub offset: Offset,
}

/// Why [`Position::parse`] rejected a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionError {
    /// The string was empty.
    Empty,
    /// The offset part was not a finite number.
    BadNumber,
    /// The string matched no position form.
    BadForm,
}

impl Position {
    /// The omitted position: the timeline's end.
    pub const END: Position = Position {
        anchor: Anchor::End,
        offset: Offset::Secs(0.0),
    };

    /// An absolute time: `2`.
    pub const fn at(t: f64) -> Self {
        Self {
            anchor: Anchor::Zero,
            offset: Offset::Secs(t),
        }
    }

    /// Relative to the timeline's end: `"+=1"`, `"-=1"`.
    pub const fn rel(d: f64) -> Self {
        Self {
            anchor: Anchor::End,
            offset: Offset::Secs(d),
        }
    }

    /// Relative to the end by a percent of the inserted child: `"+=50%"`.
    pub const fn rel_percent(p: f64) -> Self {
        Self {
            anchor: Anchor::End,
            offset: Offset::PercentOfChild(p),
        }
    }

    /// The previous child's start: `"<"`, `"<0.5"`, `"<-0.5"`, `"<+=0.5"`.
    pub const fn prev_start(d: f64) -> Self {
        Self {
            anchor: Anchor::PrevStart,
            offset: Offset::Secs(d),
        }
    }

    /// The previous child's start plus a percent of its duration: `"<25%"`.
    pub const fn prev_start_percent(p: f64) -> Self {
        Self {
            anchor: Anchor::PrevStart,
            offset: Offset::PercentOfPrev(p),
        }
    }

    /// The previous child's start plus a percent of the inserted child: `"<+=25%"`.
    pub const fn prev_start_percent_of_child(p: f64) -> Self {
        Self {
            anchor: Anchor::PrevStart,
            offset: Offset::PercentOfChild(p),
        }
    }

    /// The previous child's end: `">"`, `">0.2"`, `">-0.2"`.
    pub const fn prev_end(d: f64) -> Self {
        Self {
            anchor: Anchor::PrevEnd,
            offset: Offset::Secs(d),
        }
    }

    /// The previous child's end plus a percent of its duration: `">50%"`.
    pub const fn prev_end_percent(p: f64) -> Self {
        Self {
            anchor: Anchor::PrevEnd,
            offset: Offset::PercentOfPrev(p),
        }
    }

    /// The previous child's end plus a percent of the inserted child: `">-=50%"`.
    pub const fn prev_end_percent_of_child(p: f64) -> Self {
        Self {
            anchor: Anchor::PrevEnd,
            offset: Offset::PercentOfChild(p),
        }
    }

    /// A label: `"intro"`.
    pub const fn label(t: Tag) -> Self {
        Self {
            anchor: Anchor::Label(t),
            offset: Offset::Secs(0.0),
        }
    }

    /// A label plus seconds: `"intro+=1"`, `"intro-=1"`.
    pub const fn label_rel(t: Tag, d: f64) -> Self {
        Self {
            anchor: Anchor::Label(t),
            offset: Offset::Secs(d),
        }
    }

    /// A label plus a percent of the inserted child: `"intro+=50%"`.
    pub const fn label_rel_percent(t: Tag, p: f64) -> Self {
        Self {
            anchor: Anchor::Label(t),
            offset: Offset::PercentOfChild(p),
        }
    }

    /// Parses a GSAP position string; `label` turns a label name into its tag.
    ///
    /// Forms: a number (absolute); `"+=x"`, `"-=x"`, `"+=x%"`, `"-=x%"`;
    /// `"<"`, `"<x"`, `"<-x"`, `"<+=x"`, `"<-=x"`, `"<x%"`, `"<+=x%"` and the
    /// same with `">"`; `"name"`, `"name+=x"`, `"name-=x"`, `"name+=x%"`,
    /// `"name-=x%"`. In the `<`/`>` forms a `%` without `=` is a percent of
    /// the previous child, with `=` of the inserted child (GSAP
    /// `_parsePosition`). A numeric string is always an absolute time (GSAP
    /// would prefer an existing label of that name).
    pub fn parse(s: &str, mut label: impl FnMut(&str) -> Tag) -> Result<Position, PositionError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(PositionError::Empty);
        }
        if let Ok(t) = s.parse::<f64>() {
            return if t.is_finite() {
                Ok(Position::at(t))
            } else {
                Err(PositionError::BadNumber)
            };
        }
        let (body, percent) = match s.strip_suffix('%') {
            Some(b) => (b, true),
            None => (s, false),
        };
        let number = |n: &str| match n.parse::<f64>() {
            Ok(v) if v.is_finite() => Ok(v),
            _ => Err(PositionError::BadNumber),
        };
        if let Some(prev_end) = match body.as_bytes().first() {
            Some(b'<') => Some(false),
            Some(b'>') => Some(true),
            _ => None,
        } {
            let rest = &body[1..];
            // "<+=x" / "<-=x": GSAP drops the "=", leaving a signed number.
            let has_eq = rest.contains('=');
            let v = if has_eq {
                let sign = match rest.as_bytes() {
                    [b'+', b'=', ..] => 1.0,
                    [b'-', b'=', ..] => -1.0,
                    _ => return Err(PositionError::BadForm),
                };
                sign * number(&rest[2..])?
            } else if rest.is_empty() && !percent {
                0.0
            } else {
                number(rest)?
            };
            return Ok(match (prev_end, percent, has_eq) {
                (false, false, _) => Position::prev_start(v),
                (false, true, false) => Position::prev_start_percent(v),
                (false, true, true) => Position::prev_start_percent_of_child(v),
                (true, false, _) => Position::prev_end(v),
                (true, true, false) => Position::prev_end_percent(v),
                (true, true, true) => Position::prev_end_percent_of_child(v),
            });
        }
        let Some(eq) = body.find('=') else {
            // A bare name (a trailing % belongs to the name, as in GSAP).
            return Ok(Position::label(label(s)));
        };
        if eq == 0 {
            return Err(PositionError::BadForm);
        }
        let sign = match body.as_bytes()[eq - 1] {
            b'+' => 1.0,
            b'-' => -1.0,
            _ => return Err(PositionError::BadForm),
        };
        let v = sign * number(&body[eq + 1..])?;
        let name = &body[..eq - 1];
        Ok(match (name.is_empty(), percent) {
            (true, false) => Position::rel(v),
            (true, true) => Position::rel_percent(v),
            (false, false) => Position::label_rel(label(name), v),
            (false, true) => Position::label_rel_percent(label(name), v),
        })
    }
}

impl Default for Position {
    fn default() -> Self {
        Position::END
    }
}

impl From<f64> for Position {
    fn from(t: f64) -> Self {
        Position::at(t)
    }
}

impl From<Tag> for Position {
    fn from(t: Tag) -> Self {
        Position::label(t)
    }
}

/// A playhead target for seeks and control calls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seek {
    /// A total time in seconds (repeats included), GSAP `seek(number)` / `totalTime()`.
    Time(f64),
    /// A label plus seconds, GSAP `seek("label")`.
    Label(Tag, f64),
    /// Progress of the current iteration, GSAP `progress()`.
    Progress(f64),
    /// Progress of the whole animation, repeats included, GSAP `totalProgress()`.
    TotalProgress(f64),
}
