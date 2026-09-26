//! Tweens and timelines for widgets: the Makepad adapter of the
//! `makepad_tween` engine (GSAP 3's model: `gsap.to`, `gsap.timeline`,
//! labels, staggers, eases, repeat / yoyo, callbacks, playback control).
//!
//! Everything of the engine is re-exported here (`use
//! makepad_widgets::tween::*;`); nothing is glob-exported from the crate
//! root, so `Timeline`-like names never clash with widget names.
//!
//! What this module adds on top of the engine:
//!
//! | Here | GSAP |
//! |---|---|
//! | [`TweenHost`] | a widget's own `gsap.context()`: the engine, its frame clock and its sinks |
//! | [`TweenHost::to`] / [`from`](TweenHost::from) / [`from_to`](TweenHost::from_to) / [`set`](TweenHost::set) / [`keyframes`](TweenHost::keyframes) | `gsap.to` / `from` / `fromTo` / `set` / keyframes, `overwrite: "auto"` by default |
//! | [`TweenHost::timeline`] + [`TweenHost::tl`] + [`TweenHost::play`] | `gsap.timeline({paused})`, `tl.to(..)`, `tl.play()` |
//! | [`TweenHost::control`] | the `Animation` methods (`pause`, `reverse`, `seek`, `progress`, `timeScale`, ...) |
//! | [`TweenHost::swap_events`] | `onStart`, `onComplete`, `onRepeat`, ..., label crossings (queued, drained after the frame) |
//! | [`TweenClock`], [`tween_ticker`], [`set_tween_ticker`] | `gsap.ticker` (`lagSmoothing`), `globalTimeline.timeScale()` / `pause()` |
//! | [`TweenHost::push_instances`], [`TweenHost::apply_to`], [`TweenHost::apply_to_ref`] | GSAP's property setters: where the values go |
//! | [`QuickTo`] | `gsap.quickTo` (engine-free, for per-frame retargeting) |
//! | [`MotionPath`], [`PropTo::path`], [`PathOpts`] (engine) | `MotionPathPlugin`: `motionPath: {path, align, autoRotate, start, end}` (see "Motion paths" below) |
//! | [`prop`], [`prop_path`], [`tag`], [`pos`] | property names, `id`s / labels, position strings (`"<"`, `"+=0.2"`, `"intro-=0.1"`) |
//!
//! # The frame loop
//!
//! A host lives in a `#[rust]` field of the widget ([`TweenHost::new`]
//! allocates nothing, and apply walks never touch `#[rust]` fields, so a
//! running motion survives Reload / Rebake / ScriptReapply). Building calls
//! arm the host's `NextFrame`; the widget forwards every event to
//! [`TweenHost::handle_event`], which steps the engine once per frame by the
//! frame's `ne.time` delta (lag-smoothed and scaled by the app-wide
//! [`TweenTicker`]) and re-arms only while something moves: an idle host
//! costs nothing. The answer, a [`TweenAction`], says whether values changed;
//! the widget then pushes them (or pulls them in `draw_walk`), drains the
//! events, and calls [`TweenHost::clear_changes`].
//!
//! Sinks, cheapest first:
//! - pull: [`TweenHost::f64`] / [`TweenHost::rgba`] / [`TweenHost::value`] in
//!   `draw_walk` (plain memory reads; NextFrame runs before Draw with the
//!   same time). Items drawn in a loop from one draw struct MUST pull and
//!   redraw: an instance patch reaches only the last-drawn instance.
//! - [`TweenHost::push_instances`]: script `instance()` shader inputs of one
//!   draw struct, patched into the GPU buffer with no VM, no redraw.
//! - [`TweenHost::apply_to`]: one `Apply::Animate` of a host-owned, rooted
//!   script object (built once per target, leaves rewritten in place) onto
//!   any `ScriptApply` target: nested paths (`draw_bg.color`), uniforms,
//!   `#[live]` fields. Colours and vectors travel as full-precision f32 pods.
//! - [`TweenHost::apply_to_ref`]: the same onto a `WidgetRef`, without the
//!   redraw `WidgetRef` applies add.
//!
//! Rules:
//! - A key is animated by the Animator OR a tween, never both: the last
//!   writer of a frame wins and the value flickers (no runtime detection).
//! - Seed a slot ([`TweenHost::seed`]) before a `to()` reads it: GSAP reads
//!   the target's current value, the host cannot. An unseeded `to()` does
//!   not move (debug builds log it once).
//! - Reduced motion (the ticker's flag or [`TweenHost::set_reduced_motion`])
//!   finishes animations instead of playing them, with their events.
//! - Budget: the engine handles thousands of tweens per frame; the push is
//!   the limit (one `Apply::Animate` per target per frame does not scale
//!   past a few hundred targets).
//!
//! # Motion paths
//!
//! A tween can follow a [`MotionPath`] (GSAP `motionPath`). Build the path
//! once ([`MotionPath::from_svg`] for SVG path data, [`MotionPath::through`]
//! for a curve through points, [`MotionPath::cubic_points`], a
//! [`PathBuilder`]; the `*3` forms take z), store it in the host's engine
//! with `host.engine.add_path(path)`, build with
//! `PropTo::path(x_key, y_key, id, PathOpts::new()..)` and drop the caller's
//! hold with `host.engine.release_path(id)`: the geometry then lives exactly
//! as long as the tweens that follow it (a host that rebuilds often keeps
//! its hold and releases the old path when it replaces it). [`PathOpts`]
//! carries GSAP's `start` / `end`, `offsetX` / `offsetY`, `align`
//! ([`PathAlign::Start`] is `align: self`: the path moves so its start sits
//! on the target's current x / y), `autoRotate` (degrees, or radians with
//! [`PathOpts::auto_rotate_radians`]) and an optional z key for 3D paths.
//! The eased ratio is clamped to the path (an overshooting ease rests at
//! the ends) and both ends land exactly.
//!
//! Which keys a path can drive (x and y are two scalar keys):
//! - `margin.left` / `margin.top` of a child in a `flow: Overlay` parent:
//!   the child sits at the parent's origin plus its margin. The script
//!   layer's default (`motion_path` in [`crate::tween_script`]); a layout
//!   field, so each changed frame relayouts and redraws the target (a few
//!   hundred targets is the budget).
//! - draw-shader `instance()` pairs (`draw_bg.shift_x` / `draw_bg.shift_y`,
//!   a `draw_bg.rotation`) with [`TweenHost::push_instances`]: the cheapest
//!   sink, no relayout, for custom-drawn content.
//! - pulled with [`TweenHost::f64`] in `draw_walk` for a canvas that draws
//!   many items (the storybook's Motion paths page pulls x, y and the
//!   rotation this way: one redraw per changed frame, no layout).
//! - Not `abs_pos`: `Walk.abs_pos` is an absolute pass position (an
//!   `Option<Vec2d>`, neither parent-relative nor two scalar keys).
//!
//! ```text
//! const PX: PropKey = prop(live_id!(px));
//! const PY: PropKey = prop(live_id!(py));
//! const ROT: PropKey = prop(live_id!(rot));
//! // Parse once (a building call), never per frame.
//! let path = MotionPath::from_svg("M0,100 C100,0 200,200 300,100").unwrap();
//! let id = self.motion.engine.add_path(path);
//! let o = PathOpts::new().auto_rotate(ROT, 0.0); // degrees
//! self.motion.to(cx, ME.into(), &[PropTo::path(PX, PY, id, o)], TweenOpts::new().duration(2.0));
//! self.motion.engine.release_path(id); // freed with the tween
//! // draw_walk: let (x, y) = (self.motion.f64(ME, PX, 0.0), self.motion.f64(ME, PY, 0.0));
//! ```
//!
//! # Example: a hover tween on `draw_bg.hover`
//!
//! ```text
//! use makepad_widgets::tween::*;
//!
//! const ME: TargetId = TargetId(0);
//! const HOVER: PropKey = prop(live_id!(hover)); // a script `hover: instance(0.0)` of draw_bg
//!
//! #[derive(Script, ScriptHook, Widget)]
//! pub struct HoverCard {
//!     #[source] source: ScriptObjectRef,
//!     #[walk] walk: Walk,
//!     #[layout] layout: Layout,
//!     #[redraw] #[live] draw_bg: DrawQuad,
//!     #[live] hover_secs: f64,          // theme.motion_short_2
//!     #[live] hover_ease: Ease,         // theme.motion_ease_standard
//!     #[rust] motion: TweenHost,
//! }
//!
//! impl Widget for HoverCard {
//!     fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
//!         // Step (only on this host's own NextFrame), then push what changed.
//!         if self.motion.handle_event(cx, event).changed() {
//!             self.motion.push_instances(cx, ME, &mut self.draw_bg);
//!             self.motion.clear_changes();
//!         }
//!         let to = match event.hits(cx, self.draw_bg.area()) {
//!             Hit::FingerHoverIn(_) => 1.0,
//!             Hit::FingerHoverOut(_) => 0.0,
//!             _ => return,
//!         };
//!         if self.motion.slot(ME, HOVER).is_none() {
//!             self.motion.seed(ME, HOVER, 0.0); // the value `to()` starts from
//!         }
//!         // gsap.to(card, {hover: to, duration, ease}); overwrite "auto"
//!         // retargets from wherever the running tween has got to.
//!         let o = TweenOpts::new().duration(self.hover_secs).ease((&self.hover_ease).into());
//!         self.motion.to(cx, ME.into(), &[PropTo::to_f64(HOVER, to)], o);
//!     }
//!
//!     fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
//!         self.motion.draw_check(cx); // re-arms a chain a closed container cut
//!         self.draw_bg.draw_walk(cx, walk);
//!         DrawStep::done()
//!     }
//! }
//! ```
//!
//! # Example: a timeline with labels, drained through `swap_events`
//!
//! ```text
//! const INTRO: Tag = tag(live_id!(intro));
//! const OUTRO: Tag = tag(live_id!(outro));
//! const OPACITY: PropKey = prop(live_id!(opacity));
//! const ROWS: Targets<'static> = Targets::Range { first: 1, count: 5 };
//!
//! // #[rust] events: Vec<TweenEvent>, #[rust] show: TweenId
//! fn build(&mut self, cx: &mut Cx) {
//!     self.motion.kill(cx, self.show); // one timeline per click, never a pile
//!     for i in 1..=5 { self.motion.seed(TargetId(i), OPACITY, 0.0); }
//!     // gsap.timeline({paused: true, onComplete}) with label events
//!     let tl = self.motion.timeline(TimelineOpts::new().paused(true).watch_labels().on_complete());
//!     self.motion.tl(tl)
//!         .add_label(INTRO, 0.0)
//!         .to(ROWS, &[PropTo::to_f64(OPACITY, 1.0)],
//!             TweenOpts::new().duration(0.3).stagger(Stagger::each(0.06)), INTRO)
//!         .add_label(OUTRO, pos("+=1"))
//!         .to(ROWS, &[PropTo::to_f64(OPACITY, 0.0)], TweenOpts::new().duration(0.2), pos("outro"));
//!     self.show = tl;
//!     self.motion.play(cx, tl);
//! }
//!
//! fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
//!     if self.motion.handle_event(cx, event).changed() {
//!         self.redraw(cx); // rows are drawn in a loop: pull in draw_walk
//!     }
//!     self.motion.swap_events(&mut self.events); // allocation-free drain
//!     for ev in &self.events {
//!         match ev.kind {
//!             EventKind::Label(OUTRO) => log!("outro starts at {}", ev.total_time),
//!             EventKind::Complete if ev.id == self.show => { /* emit a widget action */ }
//!             _ => {}
//!         }
//!     }
//!     // Controls render synchronously and report on the next frame:
//!     // self.motion.control(cx, self.show).reverse();
//!     // self.motion.control(cx, self.show).seek(Seek::Label(INTRO, 0.0), Emit::Suppress);
//! }
//!
//! fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
//!     self.motion.draw_check(cx);
//!     for i in 1..=5 {
//!         let a = self.motion.f64(TargetId(i), OPACITY, 0.0);
//!         // draw row i with opacity a
//!     }
//!     self.motion.clear_changes();
//!     DrawStep::done()
//! }
//! ```

pub use makepad_tween::*;

use crate::{
    makepad_platform::{
        log, makepad_script::ScriptPod, Apply, Cx, DrawShaderAttrFormat, DrawVars, Event, LiveId,
        NextFrame, Scope, ScriptApply, ScriptObject, ScriptObjectRef, ScriptValue, ScriptVm,
    },
    widget::WidgetRef,
    widget_async::{CxSplashVmExt, SplashVmId, MAIN_SPLASH_VM_ID},
};

/// A property key from a Makepad id: `const HOVER: PropKey = prop(live_id!(hover));`.
/// With no [`TweenHost::bind_path`] the id is also where the value lands
/// (`draw_bg.hover` for [`TweenHost::push_instances`], `hover` for
/// [`TweenHost::apply_to`]). GSAP: a vars key such as `x` or `opacity`.
pub const fn prop(id: LiveId) -> PropKey {
    PropKey(id.0)
}

/// A tag from a Makepad id: a timeline label, a callback identity or a GSAP
/// `id`. `const INTRO: Tag = tag(live_id!(intro));` (the same hash [`pos`]
/// gives the label name `"intro"`).
pub const fn tag(id: LiveId) -> Tag {
    Tag(id.0)
}

/// A property key for a nested path (`prop_path(&[live_id!(draw_bg),
/// live_id!(color)])`): [`PropKey::path`] over the raw ids, so a one-id path
/// is [`prop`] of that id. Bind it with [`TweenHost::bind_path`] to say where
/// it lands. GSAP: a nested vars key.
pub const fn prop_path(ids: &[LiveId]) -> PropKey {
    if ids.len() == 1 {
        return PropKey(ids[0].0);
    }
    let mut h = 0u64;
    let mut i = 0;
    while i < ids.len() {
        h = splitmix64(h ^ ids[i].0);
        i += 1;
    }
    PropKey(h)
}

/// A GSAP position parameter from its string form: `"1.5"`, `"+=0.2"`,
/// `"-=10%"`, `"<"`, `">-0.1"`, `"intro"`, `"intro+=0.3"`. Label names hash
/// like [`tag`] (`pos("intro")` finds `tag(live_id!(intro))`). A malformed
/// string panics in debug builds and means "at the end" (`Position::END`)
/// in release builds. Parse once, not per frame.
pub fn pos(s: &str) -> Position {
    match Position::parse(s, |name| Tag(LiveId::from_str(name).0)) {
        Ok(p) => p,
        Err(e) => {
            if cfg!(debug_assertions) {
                panic!("tween::pos: bad position {s:?}: {e:?}");
            }
            Position::END
        }
    }
}

/// The app-wide ticker (GSAP `gsap.ticker` + `globalTimeline` controls):
/// time scale, pause, lag smoothing, reduced motion. Inserts the default
/// the first time. Read it; change it with [`set_tween_ticker`].
pub fn tween_ticker(cx: &mut Cx) -> TweenTicker {
    *cx.global::<TweenTicker>()
}

/// [`tween_ticker`] from a shared `&Cx` (the default when none was set yet;
/// nothing is inserted).
pub fn tween_ticker_ref(cx: &Cx) -> TweenTicker {
    cx.get_global_ref::<TweenTicker>()
        .copied()
        .unwrap_or_default()
}

/// Changes the app-wide ticker (GSAP `globalTimeline.timeScale(s)`,
/// `.pause()`, `.resume()`, `ticker.lagSmoothing(..)`), bumps its epoch and
/// redraws everything, so hosts held by a pause re-arm from their
/// [`TweenHost::draw_check`].
///
/// ```text
/// set_tween_ticker(cx, |t| t.time_scale = 0.25);   // slow motion everywhere
/// set_tween_ticker(cx, |t| t.paused = true);       // freeze every host
/// set_tween_ticker(cx, |t| t.reduced_motion = true);
/// ```
pub fn set_tween_ticker(cx: &mut Cx, f: impl FnOnce(&mut TweenTicker)) {
    let t = cx.global::<TweenTicker>();
    f(t);
    t.epoch = t.epoch.wrapping_add(1);
    cx.redraw_all();
}

/// A frame clock on `Event::NextFrame`: GSAP's ticker for one host. It turns
/// the frame times of its own NextFrame chain into animation deltas
/// (lag-smoothed, scaled and paused by the [`TweenTicker`] per its
/// [`ClockPolicy`]) and keeps the chain armed only while asked to.
///
/// ```text
/// if let Some(dt) = self.clock.tick(cx, event) {
///     let moving = self.glide.step(dt);
///     self.clock.finish_frame(cx, moving);
///     self.redraw(cx);
/// }
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct TweenClock {
    /// The pending frame request (`NextFrame(0)` when none).
    pub next_frame: NextFrame,
    last_time: Option<f64>,
    policy: ClockPolicy,
    held: bool,
    epoch: u64,
    ticker: TweenTicker,
}

impl TweenClock {
    /// A stopped clock with `policy`.
    pub fn new(policy: ClockPolicy) -> Self {
        Self {
            policy,
            ..Self::default()
        }
    }

    /// Requests the next frame (the previous frame time is kept, so a
    /// running chain keeps its deltas).
    pub fn arm(&mut self, cx: &mut Cx) {
        self.next_frame = cx.new_next_frame();
        self.held = false;
    }

    /// Forgets the last frame time: the next tick after the clock is armed
    /// again yields `policy.first_dt` (hidden time is never replayed).
    pub fn stop(&mut self) {
        self.last_time = None;
    }

    /// The animation delta when `event` is this clock's frame, else `None`.
    /// Reads the ticker once. Matches the frame by reference (it never
    /// clones the frame set, unlike `NextFrame::is_event`), and consumes it,
    /// so a frame delivered twice steps once.
    pub fn tick(&mut self, cx: &mut Cx, event: &Event) -> Option<f64> {
        let Event::NextFrame(ne) = event else {
            return None;
        };
        if self.next_frame.0 == 0 || !ne.set.contains(&self.next_frame) {
            return None;
        }
        self.next_frame = NextFrame::default();
        self.ticker = *cx.global::<TweenTicker>();
        let raw = self.last_time.map(|l| ne.time - l);
        self.last_time = Some(ne.time);
        Some(self.ticker.dt(raw, self.policy))
    }

    /// Ends a ticked frame: keeps the chain while `moving` (and the ticker,
    /// when followed, is not paused); otherwise stops. A clock stopped by a
    /// pause while moving is "held" and re-arms from [`TweenClock::draw_check`]
    /// once the ticker resumes.
    pub fn finish_frame(&mut self, cx: &mut Cx, moving: bool) {
        let paused = self.ticker.paused && self.policy.follow_ticker;
        if moving && !paused {
            self.arm(cx);
            self.epoch = self.ticker.epoch;
        } else {
            self.stop();
            self.held = moving && paused;
        }
    }

    /// Call from `draw_walk` with whether work remains. Re-arms a chain that
    /// was cut (a container that stopped forwarding events swallowed the
    /// frame, `cx.next_frame_is_pending` is false) or held by a ticker pause
    /// that has since been lifted (the ticker's epoch moved); a cut chain
    /// restarts its clock so the hidden time is not replayed. A chain that is
    /// still pending only adopts the new epoch: it keeps its time, so
    /// dragging the global time scale never drops frames.
    pub fn draw_check(&mut self, cx: &mut Cx, active: bool) {
        let tk = tween_ticker(cx);
        if !active || (tk.paused && self.policy.follow_ticker) {
            return;
        }
        if !cx.next_frame_is_pending(self.next_frame) {
            self.stop();
            self.arm(cx);
        }
        self.epoch = tk.epoch;
    }

    /// Whether a frame is requested and not delivered yet.
    pub fn is_armed(&self, cx: &Cx) -> bool {
        self.next_frame.0 != 0 && cx.next_frame_is_pending(self.next_frame)
    }

    /// Whether a ticker pause stopped this clock while it was moving.
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// The clock policy.
    pub fn policy(&self) -> ClockPolicy {
        self.policy
    }

    /// Replaces the clock policy (takes effect on the next tick).
    pub fn set_policy(&mut self, policy: ClockPolicy) {
        self.policy = policy;
    }

    /// The ticker as read at the last tick.
    pub fn ticker(&self) -> TweenTicker {
        self.ticker
    }
}

/// What a host frame did: the widget's cue to push (or redraw) and to drain
/// events. Returned by [`TweenHost::handle_event`] and [`TweenHost::advance`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TweenAction {
    /// Nothing changed and nothing fired (not this host's frame, or idle).
    #[default]
    None,
    /// Values changed (or events fired) and the host keeps moving.
    Changed,
    /// Values changed (or events fired) on the frame the host went idle:
    /// push once more; no further frames follow (GSAP `onComplete` of
    /// everything).
    Settled,
}

impl TweenAction {
    /// Whether values should be pushed or redrawn (`Changed` or `Settled`).
    pub fn changed(self) -> bool {
        !matches!(self, TweenAction::None)
    }

    /// Whether the host went idle this frame.
    pub fn settled(self) -> bool {
        matches!(self, TweenAction::Settled)
    }
}

/// One leaf of an [`ApplyCache`]: a key in a host-owned object, holding a
/// number or a pod that is rewritten in place.
#[derive(Clone, Copy, Debug)]
struct ApplyLeaf {
    parent: ScriptObject,
    key: LiveId,
    pod: Option<ScriptPod>,
    /// 0 for a number, else the pod's lane count (2, 3 or 4).
    dims: u8,
    /// The engine slot this leaf mirrors (`u32::MAX` for a hand-built leaf).
    slot: u32,
}

const NO_SLOT: u32 = u32::MAX;

/// A reusable apply object: GSAP's property setter for script targets.
///
/// The object tree (`{draw_bg: {hover: 0.5, color: vec4(..)}}`) is built on
/// the script heap once, rooted against GC, and its leaves are rewritten in
/// place; one `Apply::Animate` pushes them all. Numbers go in as numbers;
/// vectors and colours as f32 pods (full precision, never the 8-bit
/// `from_color`). [`TweenHost`] keeps one per target; it is public for code
/// that pushes values without an engine.
///
/// ```text
/// let hover = cache.leaf(cx, &[live_id!(draw_bg), live_id!(hover)], ValueKind::F64);
/// cache.set(cx, hover, 0.5.into());
/// cache.apply(cx, &mut self.draw_bg_owner);
/// ```
///
/// The objects live in one script VM: the main VM unless
/// [`ApplyCache::with_vm_id`] names an isolate (a widget built in a Splash
/// isolate passes `cx.script_ref_vm_id(&self.source).unwrap_or(MAIN_SPLASH_VM_ID)`).
pub struct ApplyCache {
    vm_id: SplashVmId,
    root: Option<ScriptObjectRef>,
    /// (parent, key, child) of every intermediate object.
    nodes: Vec<(ScriptObject, LiveId, ScriptObject)>,
    leaves: Vec<ApplyLeaf>,
    shape_gen: u32,
    bind_gen: u32,
    /// The host filled it from the engine (possibly with no leaves).
    built: bool,
    /// Every leaf must be rewritten before the next apply (a deferred push).
    stale: bool,
}

impl Default for ApplyCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ApplyCache {
    /// An empty cache for the main VM. Allocates nothing.
    pub fn new() -> Self {
        Self::with_vm_id(MAIN_SPLASH_VM_ID)
    }

    /// An empty cache whose objects live in VM `vm_id`.
    pub fn with_vm_id(vm_id: SplashVmId) -> Self {
        Self {
            vm_id,
            root: None,
            nodes: Vec::new(),
            leaves: Vec::new(),
            shape_gen: 0,
            bind_gen: 0,
            built: false,
            stale: false,
        }
    }

    /// The VM the objects live in.
    pub fn vm_id(&self) -> SplashVmId {
        self.vm_id
    }

    /// The leaf at `path` (1 or more ids; `[draw_bg, hover]` is
    /// `{draw_bg: {hover: ..}}`), created with a zero value of `kind` when
    /// missing. Builds the object chain once; returns the leaf index for
    /// [`ApplyCache::set`]. Must not run while the VM is held (inside an
    /// apply walk); use [`ApplyCache::leaf_with_vm`] there.
    pub fn leaf(&mut self, cx: &mut Cx, path: &[LiveId], kind: ValueKind) -> usize {
        let vm_id = self.vm_id;
        cx.with_script_vm_id(vm_id, |vm| self.leaf_with_vm(vm, path, kind))
    }

    /// [`ApplyCache::leaf`] with the VM in hand.
    pub fn leaf_with_vm(&mut self, vm: &mut ScriptVm, path: &[LiveId], kind: ValueKind) -> usize {
        assert!(!path.is_empty(), "ApplyCache::leaf: empty path");
        let mut parent = match &self.root {
            Some(r) => r.as_object(),
            None => {
                let root = vm.bx.heap.new_object();
                self.root = Some(vm.bx.heap.new_object_ref(root));
                root
            }
        };
        let (last, prefix) = path.split_last().unwrap();
        for &id in prefix {
            parent = match self.nodes.iter().find(|(p, k, _)| *p == parent && *k == id) {
                Some(&(_, _, child)) => child,
                None => {
                    let child = vm.bx.heap.new_object();
                    vm.bx.heap.set_value_def(parent, id.into(), child.into());
                    self.nodes.push((parent, id, child));
                    child
                }
            };
        }
        if let Some(i) = self
            .leaves
            .iter()
            .position(|l| l.parent == parent && l.key == *last)
        {
            return i;
        }
        let mut leaf = ApplyLeaf {
            parent,
            key: *last,
            pod: None,
            dims: 0,
            slot: NO_SLOT,
        };
        write_leaf(vm, &mut leaf, TweenValue::from_lanes(kind, [0.0; 4]));
        self.leaves.push(leaf);
        self.leaves.len() - 1
    }

    /// Writes `v` into leaf `leaf` (a map entry overwrite or pod rewrite; no
    /// allocation unless the value changes between number and vector).
    pub fn set(&mut self, cx: &mut Cx, leaf: usize, v: TweenValue) {
        let vm_id = self.vm_id;
        cx.with_script_vm_id(vm_id, |vm| self.set_with_vm(vm, leaf, v));
    }

    /// [`ApplyCache::set`] with the VM in hand.
    pub fn set_with_vm(&mut self, vm: &mut ScriptVm, leaf: usize, v: TweenValue) {
        if let Some(l) = self.leaves.get_mut(leaf) {
            write_leaf(vm, l, v);
        }
    }

    /// One `Apply::Animate` of the object onto `obj`. Answers `false` (and
    /// applies nothing) while the VM is held: retry from the next event.
    pub fn apply<T: ScriptApply + ?Sized>(&mut self, cx: &mut Cx, obj: &mut T) -> bool {
        if cx.is_script_vm_held() {
            self.stale = true;
            return false;
        }
        let vm_id = self.vm_id;
        cx.with_script_vm_id(vm_id, |vm| self.apply_with_vm(vm, obj));
        true
    }

    /// [`ApplyCache::apply`] with the VM in hand (an `on_after_apply`).
    pub fn apply_with_vm<T: ScriptApply + ?Sized>(&mut self, vm: &mut ScriptVm, obj: &mut T) {
        if let Some(root) = &self.root {
            let value: ScriptValue = root.as_object().into();
            obj.script_apply(vm, &Apply::Animate, &mut Scope::empty(), value);
        }
        self.stale = false;
    }

    /// The object as a script value (`None` before the first leaf).
    pub fn value(&self) -> Option<ScriptValue> {
        self.root.as_ref().map(|r| r.as_object().into())
    }

    /// How many leaves the object has.
    pub fn len(&self) -> usize {
        self.leaves.len()
    }

    /// Whether the object has no leaves.
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    /// Drops the object (unrooted; the GC reclaims it) and every leaf.
    pub fn clear(&mut self) {
        self.root = None;
        self.nodes.clear();
        self.leaves.clear();
        self.built = false;
    }
}

/// Writes `v` into a leaf: numbers overwrite the map entry, vectors and
/// colours rewrite the leaf's pod words (a new pod only when the lane count
/// changes).
fn write_leaf(vm: &mut ScriptVm, leaf: &mut ApplyLeaf, v: TweenValue) {
    let dims: u8 = match v.kind() {
        ValueKind::F64 | ValueKind::Int => 0,
        ValueKind::Vec2 => 2,
        ValueKind::Vec3 => 3,
        ValueKind::Vec4 | ValueKind::Color => 4,
    };
    let lanes = v.to_lanes();
    if dims == 0 {
        leaf.pod = None;
        leaf.dims = 0;
        vm.bx.heap.set_value_def(
            leaf.parent,
            leaf.key.into(),
            ScriptValue::from_f64(lanes[0]),
        );
        return;
    }
    let pod = match leaf.pod {
        Some(p) if leaf.dims == dims => p,
        _ => {
            let pods = &vm.bx.code.builtins.pod;
            let ty = match dims {
                2 => pods.pod_vec2f,
                3 => pods.pod_vec3f,
                _ => pods.pod_vec4f,
            };
            let p = vm.bx.heap.new_pod(ty);
            vm.bx
                .heap
                .set_value_def(leaf.parent, leaf.key.into(), p.into());
            leaf.pod = Some(p);
            leaf.dims = dims;
            p
        }
    };
    let words = vm.bx.heap.pod_data_mut(pod);
    for (w, l) in words.iter_mut().zip(lanes.iter()).take(dims as usize) {
        *w = (*l as f32).to_bits();
    }
}

/// Where a key of a target lands: its bound path, or its own id.
#[derive(Clone, Copy, Debug)]
struct Bind {
    target: TargetId,
    key: PropKey,
    path: [LiveId; 4],
    len: u8,
}

/// The path of (t, k): the bound one, else `[LiveId(k.0)]`.
fn path_of(binds: &[Bind], t: TargetId, k: PropKey) -> ([LiveId; 4], usize) {
    match binds.iter().find(|b| b.target == t && b.key == k) {
        Some(b) => (b.path, b.len as usize),
        None => ([LiveId(k.0), LiveId(0), LiveId(0), LiveId(0)], 1),
    }
}

/// Brings target `t`'s cache in line with the engine and writes the leaves
/// that changed (all of them after a rebuild or a deferred push). The object
/// is rebuilt only when `t` gained a slot or the binds changed. Returns
/// whether the object has anything to apply.
fn fill_cache(
    engine: &TweenEngine,
    binds: &[Bind],
    bind_gen: u32,
    cache: &mut ApplyCache,
    vm: &mut ScriptVm,
    t: TargetId,
) -> bool {
    let gen = engine.slot_generation();
    let mut all = cache.stale;
    if !cache.built || cache.bind_gen != bind_gen || cache.shape_gen != gen {
        let slots = engine.slot_count();
        let count = (0..slots)
            .filter(|&s| engine.slot_key(SlotId(s)).0 == t)
            .count();
        if !cache.built || cache.bind_gen != bind_gen || count != cache.leaves.len() {
            cache.clear();
            for s in 0..slots {
                let (st, k) = engine.slot_key(SlotId(s));
                if st != t {
                    continue;
                }
                let (path, len) = path_of(binds, t, k);
                let kind = engine.value(SlotId(s)).kind();
                let i = cache.leaf_with_vm(vm, &path[..len], kind);
                cache.leaves[i].slot = s;
            }
            cache.bind_gen = bind_gen;
            cache.built = true;
            all = true;
        }
        cache.shape_gen = gen;
    }
    for leaf in cache.leaves.iter_mut() {
        let s = SlotId(leaf.slot);
        if leaf.slot != NO_SLOT && (all || engine.is_changed(s)) {
            write_leaf(vm, leaf, engine.value(s));
        }
    }
    cache.stale = false;
    !cache.leaves.is_empty()
}

/// A widget's tween host: GSAP's global timeline scoped to one widget (a
/// `gsap.context()`), with its frame clock and its sinks.
///
/// Keep it in a `#[rust]` field (`#[rust] motion: TweenHost`): `new()`
/// allocates nothing, and a running motion survives reloads. Build with
/// [`to`](TweenHost::to) / [`timeline`](TweenHost::timeline), forward every
/// event to [`handle_event`](TweenHost::handle_event), push or pull the
/// values it reports, drain [`swap_events`](TweenHost::swap_events), call
/// [`draw_check`](TweenHost::draw_check) from `draw_walk`. See the module
/// docs for complete widgets.
///
/// Root-level `to` / `from` / `from_to` / `set` / `keyframes` default to
/// GSAP `overwrite: "auto"` (a new hover tween retargets the running one);
/// set an explicit `overwrite` (or host defaults) to change that.
pub struct TweenHost {
    /// The engine (direct access for everything the host does not wrap).
    pub engine: TweenEngine,
    /// The frame clock.
    pub clock: TweenClock,
    caches: Vec<(TargetId, ApplyCache)>,
    binds: Vec<Bind>,
    bind_gen: u32,
    vm_id: SplashVmId,
    reduced: bool,
    pending_push: bool,
    warned_unseeded: u64,
    warned_unplaced: bool,
    /// The inspector's identity for this host (0 until it first publishes).
    inspect_id: u64,
    /// The name the inspector lists it under.
    inspect_name: Option<String>,
    /// The names of the tags this host's owner uses, registered with the
    /// id interner when the inspector first sees the host (so the inspector
    /// shows `intro`, not a hash). Nothing is registered while it is closed.
    inspect_tags: &'static [&'static str],
    /// When it last published to the inspector (app seconds).
    inspect_at: f64,
}

impl Default for TweenHost {
    fn default() -> Self {
        Self::new()
    }
}

impl TweenHost {
    /// An empty host with the default clock policy (the ticker's lag
    /// smoothing, pause and time scale; first frame after a start moves 0).
    /// Allocates nothing.
    pub fn new() -> Self {
        Self::with_clock(ClockPolicy::default())
    }

    /// An empty host whose clock follows `policy` (for example
    /// `ClockPolicy { follow_ticker: false, .. }` for motion that must not
    /// pause with the app, or `first_dt: 1.0 / 60.0` for hand-offs).
    pub fn with_clock(policy: ClockPolicy) -> Self {
        Self {
            engine: TweenEngine::new(),
            clock: TweenClock::new(policy),
            caches: Vec::new(),
            binds: Vec::new(),
            bind_gen: 0,
            vm_id: MAIN_SPLASH_VM_ID,
            reduced: false,
            pending_push: false,
            warned_unseeded: 0,
            warned_unplaced: false,
            inspect_id: 0,
            inspect_name: None,
            inspect_tags: &[],
            inspect_at: f64::NEG_INFINITY,
        }
    }

    /// The name the tween inspector lists this host under (by default
    /// "tweens #N"). Owners name their motion, e.g. "StackNavigation slide".
    pub fn set_inspect_name(&mut self, name: impl Into<String>) {
        self.inspect_name = Some(name.into());
    }

    /// [`TweenHost::set_inspect_name`] as a builder, for field initialisers.
    pub fn inspect_named(mut self, name: impl Into<String>) -> Self {
        self.set_inspect_name(name);
        self
    }

    /// The names behind the owner's tags (labels, animation ids), for the
    /// inspector: a tag built with the const `live_id!` carries only its
    /// hash. Registered when the inspector first opens on this host.
    pub fn inspect_tags(mut self, names: &'static [&'static str]) -> Self {
        self.inspect_tags = names;
        self
    }

    /// While the inspector is open: applies the commands it queued for this
    /// host and republishes what plays. True when a command changed the
    /// playhead (the caller reports the new values).
    fn inspect_sync(&mut self, cx: &mut Cx) -> bool {
        use crate::tween_inspect::{InspectCommand, TweenInspectRegistry, REPUBLISH_SECS};
        if self.inspect_id == 0 {
            self.inspect_id = crate::tween_inspect::next_host_id();
            for name in self.inspect_tags {
                // A name another id already holds keeps that id's name.
                let _ = LiveId::from_str_with_lut(name);
            }
        }
        let now = crate::makepad_platform::CxOsApi::seconds_since_app_start(cx);
        let mut applied = false;
        {
            let registry = cx.global::<TweenInspectRegistry>();
            if let Some(h) = registry.hosts.iter_mut().find(|h| h.id == self.inspect_id) {
                for cmd in h.commands.drain(..) {
                    applied = true;
                    match cmd {
                        InspectCommand::Scrub { id, time } => {
                            self.engine.anim(id).pause().seek(Seek::Time(time), Emit::Suppress);
                        }
                        InspectCommand::Pause(id) => {
                            self.engine.anim(id).pause();
                        }
                        InspectCommand::Resume(id) => {
                            self.engine.anim(id).resume();
                        }
                        InspectCommand::Restart(id) => {
                            self.engine.anim(id).restart(false, Emit::Suppress);
                        }
                        InspectCommand::TimeScale { id, scale } => {
                            self.engine.anim(id).set_time_scale(scale);
                        }
                        InspectCommand::Repeat { id, count } => {
                            // Fewer repeats can leave the playhead past the
                            // new end (Loop off after a few loops): fold it
                            // into the last pass at the same point of it.
                            let local = self.engine.anim_ref(id).time();
                            self.engine.anim(id).set_repeat(count);
                            let a = self.engine.anim_ref(id);
                            let (total, one) = (a.total_duration(), a.duration());
                            if a.total_time() > total && total < BIG {
                                let at = (total - one + local).clamp(0.0, total);
                                self.engine.anim(id).seek(Seek::Time(at), Emit::Suppress);
                            }
                        }
                        InspectCommand::Yoyo { id, on } => {
                            self.engine.anim(id).set_yoyo(on);
                        }
                    }
                }
            }
        }
        let due = now - self.inspect_at >= REPUBLISH_SECS;
        if applied || due || !self.engine.changes().is_empty() {
            self.inspect_at = now;
            let name = match &self.inspect_name {
                Some(name) => name.clone(),
                None => format!("tweens #{}", self.inspect_id),
            };
            cx.global::<TweenInspectRegistry>().publish(self.inspect_id, &name, now, &self.engine);
        }
        if applied && (self.engine.is_active() || self.pending_push) && !self.clock.is_armed(cx) {
            // A resume (or a restart) plays again from here: the first
            // frame moves by nothing, not by the time spent paused.
            self.clock.stop();
            self.clock.arm(cx);
        }
        applied
    }

    /// GSAP `gsap.defaults()` for this host's tweens.
    pub fn set_defaults(&mut self, o: TweenOpts) {
        self.engine.set_defaults(o);
    }

    /// The VM [`TweenHost::apply_to`] builds its objects in: the main VM by
    /// default; a widget built in a Splash isolate passes
    /// `cx.script_ref_vm_id(&self.source).unwrap_or(MAIN_SPLASH_VM_ID)`.
    /// Changing it drops the objects; a push deferred until then is not
    /// lost: every value is listed as changed, so the next reported frame
    /// (armed by the deferral) pushes everything into the new VM.
    pub fn set_vm_id(&mut self, vm_id: SplashVmId) {
        if vm_id != self.vm_id {
            self.vm_id = vm_id;
            self.caches.clear();
            if self.pending_push {
                self.pending_push = false;
                self.engine.mark_all_changed();
            }
        }
    }

    // ------------------------------------------------------------------
    // Building (root level)
    // ------------------------------------------------------------------

    /// `overwrite: "auto"` unless the call or the host defaults say otherwise.
    fn root_opts(&self, o: TweenOpts) -> TweenOpts {
        if o.overwrite.is_none() && self.engine.defaults().overwrite.is_none() {
            o.overwrite(Overwrite::Auto)
        } else {
            o
        }
    }

    fn reduced_now(&self, cx: &mut Cx) -> bool {
        self.reduced || tween_ticker(cx).reduced_motion
    }

    fn ensure_armed(&mut self, cx: &mut Cx) {
        if !self.clock.is_armed(cx) {
            self.clock.arm(cx);
        }
    }

    /// After a root-level build: finish at once under reduced motion (unless
    /// it was built paused: nothing plays), and arm so the change is reported
    /// on the next frame.
    fn started(&mut self, cx: &mut Cx, id: TweenId) -> TweenId {
        if self.reduced_now(cx) && !self.engine.anim_ref(id).paused() {
            self.engine.finish(id);
        }
        self.ensure_armed(cx);
        id
    }

    /// GSAP `gsap.to(targets, vars)`: from the current (seeded) values to
    /// `props`. `overwrite: "auto"` by default.
    ///
    /// ```text
    /// host.to(cx, ME.into(), &[PropTo::to_f64(HOVER, 1.0)], TweenOpts::new().duration(0.2));
    /// ```
    pub fn to(&mut self, cx: &mut Cx, t: Targets, props: &[PropTo], o: TweenOpts) -> TweenId {
        let o = self.root_opts(o);
        let id = self.engine.to(t, props, o);
        self.started(cx, id)
    }

    /// GSAP `gsap.from(targets, vars)`: from `props` (`PropTo::from`) to the
    /// current values; the start renders at once.
    pub fn from(&mut self, cx: &mut Cx, t: Targets, props: &[PropTo], o: TweenOpts) -> TweenId {
        let o = self.root_opts(o);
        let id = self.engine.from(t, props, o);
        self.started(cx, id)
    }

    /// GSAP `gsap.fromTo(targets, fromVars, toVars)`: explicit starts and
    /// ends (`PropTo::from_to`); the start renders at once.
    pub fn from_to(&mut self, cx: &mut Cx, t: Targets, props: &[PropTo], o: TweenOpts) -> TweenId {
        let o = self.root_opts(o);
        let id = self.engine.from_to(t, props, o);
        self.started(cx, id)
    }

    /// A root-level tween of any mix of prop forms ([`PropTo::to`],
    /// [`PropTo::by`], explicit starts): what [`TweenHost::to`] /
    /// [`TweenHost::from_to`] build, without their checks on the forms (a
    /// script `set` of `{x: 1, y: "+=2"}`). `overwrite: "auto"` by default.
    pub fn tween(&mut self, cx: &mut Cx, t: Targets, props: &[PropTo], o: TweenOpts) -> TweenId {
        let o = self.root_opts(o);
        let id = self.engine.tween(t, props, o);
        self.started(cx, id)
    }

    /// GSAP `gsap.set(targets, vars)`: applies `props` at once (reported on
    /// the next frame).
    pub fn set(&mut self, cx: &mut Cx, t: Targets, props: &[PropTo]) -> TweenId {
        let o = self.root_opts(TweenOpts::new());
        let id = self.engine.set(t, props, o);
        self.started(cx, id)
    }

    /// GSAP array keyframes (`gsap.to(t, {keyframes: [..]})`): the steps run
    /// back to back; `o.ease` eases the whole run.
    pub fn keyframes(
        &mut self,
        cx: &mut Cx,
        t: Targets,
        steps: &[KeyStep],
        o: TweenOpts,
    ) -> TweenId {
        let o = self.root_opts(o);
        let id = self.engine.keyframes(t, steps, o);
        self.started(cx, id)
    }

    /// GSAP `gsap.timeline(vars)`: an empty timeline. Fill it with
    /// [`TweenHost::tl`] and start it with [`TweenHost::play`] (build it
    /// `paused(true)` so nothing moves before it is complete).
    pub fn timeline(&mut self, o: TimelineOpts) -> TweenId {
        self.engine.timeline(o)
    }

    /// GSAP `tl.to()`, `tl.add()`, `tl.addLabel()`, `tl.call()`,
    /// `tl.addPause()`, ... on timeline `id`.
    pub fn tl(&mut self, id: TweenId) -> TimelineMut<'_> {
        self.engine.tl(id)
    }

    /// GSAP `anim.play()`: plays `id` and arms the clock (under reduced
    /// motion it finishes at once instead).
    pub fn play(&mut self, cx: &mut Cx, id: TweenId) {
        self.engine.anim(id).play();
        self.started(cx, id);
    }

    /// GSAP's `Animation` methods on `id` (`pause`, `resume`, `reverse`,
    /// `restart`, `seek`, `progress`, `timeScale`, `kill`, ...). Controls
    /// render synchronously; the clock is armed first so the result (a
    /// paused seek included) is reported on the next frame.
    ///
    /// ```text
    /// host.control(cx, tl).reverse();
    /// host.control(cx, tl).seek(Seek::Progress(0.5), Emit::Suppress);
    /// ```
    pub fn control(&mut self, cx: &mut Cx, id: TweenId) -> AnimMut<'_> {
        self.ensure_armed(cx);
        self.engine.anim(id)
    }

    /// Sets the current value of (t, k): the value a later `to()` starts
    /// from (GSAP reads it from the target; the host cannot). Listed as a
    /// change, reported on the host's next armed frame: seeds normally
    /// precede a build, which arms. A lone seed arms nothing; pull it, or
    /// follow it with [`TweenHost::draw_check`] / [`TweenHost::control`] to
    /// have it reported.
    pub fn seed(&mut self, t: TargetId, k: PropKey, v: impl Into<TweenValue>) -> SlotId {
        self.engine.seed(t, k, v.into())
    }

    /// GSAP `anim.kill()`. Arms the clock, so an `Interrupt` event the kill
    /// queues (its callback mask asked for it) is reported on the next frame.
    pub fn kill(&mut self, cx: &mut Cx, id: TweenId) {
        self.engine.anim(id).kill();
        self.ensure_armed(cx);
    }

    /// GSAP `gsap.killTweensOf(targets, props)` (`None`: every property).
    /// Arms the clock like [`TweenHost::kill`].
    pub fn kill_of(&mut self, cx: &mut Cx, t: Targets, keys: Option<&[PropKey]>) {
        self.engine.kill_tweens_of(t, keys);
        self.ensure_armed(cx);
    }

    /// Forgets target `t` (the object behind it is gone): the tracks on it
    /// die (an animation left with nothing to animate is killed), and its
    /// slots, apply object and path binds go (see
    /// [`TweenEngine::forget_target`]). The other slots are renumbered, so
    /// every apply object is rebuilt on its next push. Arms the clock like
    /// [`TweenHost::kill`]. Not for the frame loop.
    pub fn forget_target(&mut self, cx: &mut Cx, t: TargetId) {
        self.engine.forget_target(t);
        self.caches.retain(|(ct, _)| *ct != t);
        self.binds.retain(|b| b.target != t);
        self.after_forget(cx);
    }

    /// [`TweenHost::forget_target`] for some properties of `t` (GSAP
    /// `clearProps`): their tracks, slots and binds go; the pushed object of
    /// `t` no longer carries them.
    pub fn forget_props(&mut self, cx: &mut Cx, t: TargetId, keys: &[PropKey]) {
        self.engine.forget_props(t, keys);
        self.binds
            .retain(|b| !(b.target == t && keys.contains(&b.key)));
        self.after_forget(cx);
    }

    fn after_forget(&mut self, cx: &mut Cx) {
        // Slot ids were renumbered: rebuild every apply object on its next
        // push (a rebuilt object rewrites all its leaves).
        self.bind_gen = self.bind_gen.wrapping_add(1);
        self.refresh_pending();
        self.ensure_armed(cx);
    }

    /// Whether a push was deferred (the VM was held or the target borrowed)
    /// and waits for the next frame: [`TweenHost::target_changed`] may then
    /// be true for a target with no listed change.
    pub fn has_deferred_push(&self) -> bool {
        self.pending_push
    }

    /// Kills every animation of this host (values are kept). Arms the clock
    /// like [`TweenHost::kill`].
    pub fn kill_all(&mut self, cx: &mut Cx) {
        self.engine.kill_all();
        self.ensure_armed(cx);
    }

    // ------------------------------------------------------------------
    // Frame
    // ------------------------------------------------------------------

    /// Steps the host on its own `NextFrame` and reports what happened.
    ///
    /// - `Event::LiveEdit` / `Event::ScriptReapply` (the tree was just
    ///   re-applied from the DSL, overwriting pushed values): every slot is
    ///   listed as changed so the widget re-pushes in this same event (the VM
    ///   is free there); a running chain is re-armed if the reload cut it.
    ///   A reload is not a restart: the timelines keep their time.
    /// - This host's frame: advance by the clock's delta (or, under reduced
    ///   motion, finish what is playing: a paused or scrubbed timeline keeps
    ///   its playhead), keep the chain while anything moves (or a deferred
    ///   push waits), report.
    /// - Anything else: `None`, at the cost of one match.
    ///
    /// The host never clears changes: the owner clears after pushing.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> TweenAction {
        if matches!(event, Event::LiveEdit | Event::ScriptReapply) {
            self.engine.mark_all_changed();
            if (self.engine.is_active() || self.pending_push) && !self.clock.is_armed(cx) {
                self.clock.stop();
                self.clock.arm(cx);
            }
            return if self.engine.changes().is_empty() {
                TweenAction::None
            } else {
                TweenAction::Changed
            };
        }
        // The inspector: one relaxed load while it is closed.
        let inspected =
            crate::tween_inspect::INSPECT_OPEN.load(std::sync::atomic::Ordering::Relaxed)
                && self.inspect_sync(cx);
        let Some(dt) = self.clock.tick(cx, event) else {
            // A scrub moved the playhead outside a frame: report the values.
            return if inspected { self.action() } else { TweenAction::None };
        };
        self.step(dt);
        let moving = self.engine.is_active() || self.pending_push;
        self.clock.finish_frame(cx, moving);
        self.action()
    }

    /// Steps the engine by `dt` seconds without a frame event (time-injected
    /// code: a driver that owns its clock). Reduced motion (the host's, or
    /// the ticker's as last read) finishes what is playing instead.
    pub fn advance(&mut self, dt: f64) -> TweenAction {
        self.step(dt);
        self.action()
    }

    fn step(&mut self, dt: f64) {
        if self.reduced || self.clock.ticker().reduced_motion {
            // Only what plays: a paused timeline (built paused, paused or
            // scrubbed by the user) keeps its playhead and its events.
            self.engine.finish_all_playing();
        } else {
            self.engine.advance(dt);
        }
        let unseeded = self.engine.stats().unseeded_starts;
        if cfg!(debug_assertions) && unseeded > self.warned_unseeded {
            log!("tween: to() on an unseeded slot does not move; call TweenHost::seed first");
            self.warned_unseeded = unseeded;
        }
    }

    fn action(&self) -> TweenAction {
        if self.engine.changes().is_empty() && self.engine.events().is_empty() && !self.pending_push
        {
            TweenAction::None
        } else if self.engine.is_active() || self.pending_push {
            TweenAction::Changed
        } else {
            TweenAction::Settled
        }
    }

    /// Call from `draw_walk`: re-arms a chain a container cut (it stopped
    /// forwarding events) or a ticker pause held, while work remains.
    pub fn draw_check(&mut self, cx: &mut Cx) {
        let active = self.engine.is_active() || self.pending_push;
        self.clock.draw_check(cx, active);
    }

    /// Whether anything moves on the next frame (or a deferred push waits).
    pub fn is_active(&self) -> bool {
        self.engine.is_active() || self.pending_push
    }

    /// Reduced motion for this host (OR-ed with the ticker's flag): new and
    /// playing animations finish at once, with their events; paused ones
    /// keep their playhead until they play.
    pub fn set_reduced_motion(&mut self, on: bool) {
        self.reduced = on;
    }

    // ------------------------------------------------------------------
    // Pull
    // ------------------------------------------------------------------

    /// The current value of (t, k).
    pub fn get(&self, t: TargetId, k: PropKey) -> Option<TweenValue> {
        self.engine.get(t, k)
    }

    /// The current value of (t, k) as a number, or `default`.
    pub fn f64(&self, t: TargetId, k: PropKey, default: f64) -> f64 {
        self.engine.get_f64(t, k).unwrap_or(default)
    }

    /// The current value of (t, k) as a colour (a `Vec4` reads as one), or
    /// `default`.
    pub fn rgba(&self, t: TargetId, k: PropKey, default: Rgba) -> Rgba {
        match self.engine.get(t, k) {
            Some(TweenValue::Color(c)) => c,
            Some(TweenValue::Vec4([r, g, b, a])) => Rgba::new(r, g, b, a),
            _ => default,
        }
    }

    /// The slot of (t, k), to read with [`TweenHost::value`] without a
    /// lookup. Cache it until `engine.slot_generation()` changes.
    pub fn slot(&self, t: TargetId, k: PropKey) -> Option<SlotId> {
        self.engine.slot(t, k)
    }

    /// The value in slot `s`.
    pub fn value(&self, s: SlotId) -> TweenValue {
        self.engine.value(s)
    }

    /// Whether target `t` needs a push: a value of `t` is in the change
    /// list, or an earlier [`TweenHost::apply_to`] / [`TweenHost::apply_to_ref`]
    /// of `t` was deferred (those changes may be cleared already; gate pushes
    /// on this, never on the change list alone).
    pub fn target_changed(&self, t: TargetId) -> bool {
        (self.pending_push && self.caches.iter().any(|(ct, c)| *ct == t && c.stale))
            || self
                .engine
                .changes()
                .iter()
                .any(|&s| self.engine.slot_key(s).0 == t)
    }

    /// Empties the change list. The owner calls this after pushing every
    /// target (a host that only pulls may never call it; the list is
    /// bounded by the slot count).
    pub fn clear_changes(&mut self) {
        self.engine.clear_changes();
    }

    /// Drains the fired callbacks into `buf` (GSAP `onStart`,
    /// `onComplete`, ..., in firing order), allocation-free: keep one
    /// `#[rust] events: Vec<TweenEvent>` and pass it every frame.
    pub fn swap_events(&mut self, buf: &mut Vec<TweenEvent>) {
        self.engine.swap_events(buf);
    }

    // ------------------------------------------------------------------
    // Push
    // ------------------------------------------------------------------

    /// Says where key `k` of target `t` lands: `path` is 1 to 4 ids
    /// (`[draw_bg, color]`). Unbound keys land at their own id. The last id
    /// is the shader input [`TweenHost::push_instances`] writes; the whole
    /// path is the object [`TweenHost::apply_to`] builds. Bind at build
    /// time, not per frame.
    pub fn bind_path(&mut self, t: TargetId, k: PropKey, path: &[LiveId]) {
        assert!(
            (1..=4).contains(&path.len()),
            "TweenHost::bind_path: 1 to 4 ids"
        );
        let mut p = [LiveId(0); 4];
        p[..path.len()].copy_from_slice(path);
        let b = Bind {
            target: t,
            key: k,
            path: p,
            len: path.len() as u8,
        };
        match self.binds.iter_mut().find(|b| b.target == t && b.key == k) {
            Some(old) => *old = b,
            None => self.binds.push(b),
        }
        self.bind_gen = self.bind_gen.wrapping_add(1);
    }

    /// Writes every changed value of target `t` into the script
    /// `instance()` inputs of `draw` (the last id of the key's path) and
    /// patches the GPU buffer of its last draw: no VM, no redraw. Values are
    /// written the way `Apply::Animate` writes them: a number fills every
    /// lane of a vector input, integer inputs take the bit pattern. Returns
    /// how many values it placed: a key whose id is not a script
    /// `instance()` input of the draw's shader (a Rust `#[live]` instance
    /// field, a uniform, an unbound nested path) is skipped and not counted
    /// (debug builds log the first one). For Rust `#[live]` instance
    /// fields, pull instead: `self.draw_bg.hover = v as f32;
    /// self.draw_bg.update_instance_area_value(cx, ids!(hover))`.
    pub fn push_instances(&mut self, cx: &mut Cx, t: TargetId, draw: &mut DrawVars) -> usize {
        let Some(shader) = draw.draw_shader_id else {
            return 0;
        };
        let mut placed = 0;
        let mut unplaced = None;
        for &s in self.engine.changes() {
            let (st, k) = self.engine.slot_key(s);
            if st != t {
                continue;
            }
            let (path, len) = path_of(&self.binds, t, k);
            let id = path[len - 1];
            let inputs = &cx.draw_shaders[shader.index].mapping.dyn_instances.inputs;
            let Some(input) = inputs.iter().find(|i| i.id == id) else {
                unplaced.get_or_insert(id);
                continue;
            };
            let (slots, format) = (input.slots.min(4), input.attr_format);
            let v = self.engine.value(s);
            let l = v.to_lanes();
            let mut vals = [0f32; 4];
            match v.kind() {
                ValueKind::F64 | ValueKind::Int => {
                    let x = match format {
                        DrawShaderAttrFormat::U32x1 => f32::from_bits(l[0] as u32),
                        DrawShaderAttrFormat::I32x1 => f32::from_bits(l[0] as i32 as u32),
                        _ => l[0] as f32,
                    };
                    vals = [x; 4];
                }
                _ => {
                    for (d, l) in vals.iter_mut().zip(l.iter()) {
                        *d = *l as f32;
                    }
                }
            }
            draw.set_dyn_instance(cx, id, &vals[..slots]);
            draw.update_instance_area_value(cx, &[id]);
            placed += 1;
        }
        if let Some(id) = unplaced {
            if cfg!(debug_assertions) && !self.warned_unplaced {
                log!(
                    "tween: push_instances: {id} is not a script instance() input of this draw; \
                     bind_path it, or pull it"
                );
                self.warned_unplaced = true;
            }
        }
        placed
    }

    fn cache_ix(&mut self, t: TargetId) -> usize {
        match self.caches.iter().position(|(ct, _)| *ct == t) {
            Some(i) => i,
            None => {
                self.caches.push((t, ApplyCache::with_vm_id(self.vm_id)));
                self.caches.len() - 1
            }
        }
    }

    fn refresh_pending(&mut self) {
        self.pending_push = self.caches.iter().any(|(_, c)| c.stale);
    }

    /// Marks target cache `ix` for a full re-push and arms the next frame,
    /// which then reports `Changed` with [`TweenHost::target_changed`] true.
    fn defer_push(&mut self, cx: &mut Cx, ix: usize) {
        self.caches[ix].1.stale = true;
        self.pending_push = true;
        self.ensure_armed(cx);
    }

    /// Runs `f` with target `t`'s filled object; defers (see
    /// [`TweenHost::defer_push`]) while the VM is held or when `f` answers
    /// `false`. `false` only when deferred.
    fn push_with(
        &mut self,
        cx: &mut Cx,
        t: TargetId,
        f: impl FnOnce(&mut ScriptVm, ScriptValue) -> bool,
    ) -> bool {
        let ix = self.cache_ix(t);
        if cx.is_script_vm_held() {
            self.defer_push(cx, ix);
            return false;
        }
        let engine = &self.engine;
        let binds = &self.binds[..];
        let bind_gen = self.bind_gen;
        let cache = &mut self.caches[ix].1;
        let done = cx.with_script_vm_id(self.vm_id, |vm| {
            if fill_cache(engine, binds, bind_gen, cache, vm, t) {
                if let Some(value) = cache.value() {
                    return f(vm, value);
                }
            }
            true
        });
        if !done {
            self.defer_push(cx, ix);
            return false;
        }
        if self.pending_push {
            self.refresh_pending();
        }
        true
    }

    /// Pushes target `t`'s values onto `obj` with one `Apply::Animate` of a
    /// reused object (every key of `t` at its bound path; changed leaves
    /// rewritten). Returns `false` when the VM is held (inside an apply
    /// walk): the host keeps `t` pending and arms the next frame, which
    /// reports `Changed` with [`TweenHost::target_changed`]`(t)` true; call
    /// `apply_to(t)` again then (the owner may clear changes meanwhile).
    /// Uniforms pushed this way redraw their area; `#[live]` layout fields
    /// need the caller's redraw.
    ///
    /// Never pass the calling widget itself while its host is borrowed;
    /// take the host out first (`TweenHost::default()` allocates nothing):
    ///
    /// ```text
    /// let mut m = std::mem::take(&mut self.motion);
    /// m.apply_to(cx, ME, self);
    /// self.motion = m;
    /// ```
    ///
    /// or target the draw field directly (cheaper):
    /// `self.motion.apply_to(cx, ME, &mut self.draw_bg)` with paths relative
    /// to it.
    pub fn apply_to<T: ScriptApply + ?Sized>(
        &mut self,
        cx: &mut Cx,
        t: TargetId,
        obj: &mut T,
    ) -> bool {
        self.push_with(cx, t, |vm, value| {
            obj.script_apply(vm, &Apply::Animate, &mut Scope::empty(), value);
            true
        })
    }

    /// [`TweenHost::apply_to`] onto the widget behind `w`, without the
    /// redraw `WidgetRef` applies add. A ref that is borrowed right now
    /// answers `false` and is deferred like a held VM (never pass the
    /// calling widget's own ref from inside its `handle_event`: it stays
    /// borrowed on every retry). An empty ref answers `false` and nothing is
    /// kept pending: there is nothing to retry onto.
    pub fn apply_to_ref(&mut self, cx: &mut Cx, t: TargetId, w: &WidgetRef) -> bool {
        if w.is_empty() {
            return false;
        }
        self.push_with(cx, t, |vm, value| w.script_apply_animate(vm, value))
    }

    /// Target `t`'s object with the VM in hand (a widget's `on_after_apply`,
    /// where `apply_to` would re-enter the VM): fills it and returns it for
    /// the caller to apply with `Apply::Animate`. `None` when `t` has no
    /// values. The VM must be the host's (see [`TweenHost::set_vm_id`]).
    pub fn apply_value(&mut self, vm: &mut ScriptVm, t: TargetId) -> Option<ScriptValue> {
        let ix = self.cache_ix(t);
        let engine = &self.engine;
        let binds = &self.binds[..];
        let cache = &mut self.caches[ix].1;
        let has = fill_cache(engine, binds, self.bind_gen, cache, vm, t);
        let value = if has { cache.value() } else { None };
        if self.pending_push {
            self.refresh_pending();
        }
        value
    }
}
