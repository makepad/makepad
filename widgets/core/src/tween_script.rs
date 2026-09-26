//! `mod.tween`: GSAP-style tweens and timelines written in Splash script, a
//! thin layer over [`TweenHost`].
//!
//! Every widget script sees it as `tween` (the widgets prelude says
//! `tween: mod.tween`):
//!
//! ```text
//! on_click: || {
//!     // gsap.to(card, {...}): `ui` is the handler's own widget, `ui.card` a sibling
//!     tween.to(ui.card, {duration: theme.motion_medium_2, ease: theme.motion_ease_standard,
//!                        draw_bg: {color: #x3fb8af}, margin: {left: 120.0}})
//!     let tl = tween.timeline({defaults: {duration: 0.3, ease: "power2.out"}, paused: true})
//!     tl.to(ui.a, {width: 80.0}).to(ui.b, {width: 80.0}, "+=0.2")
//!       .add_label(@shown).call(|| ui.status.set_text("shown"), "shown+=0.1")
//!     tl.on_complete(|| ui.status.set_text("done"))
//!     tl.play()
//! }
//! ```
//!
//! # The API
//!
//! | Script | GSAP |
//! |---|---|
//! | `tween.to(targets, vars)` / `from` / `from_to(targets, from_vars, to_vars)` / `set` | `gsap.to` / `from` / `fromTo` / `set`. Unlike GSAP (`overwrite: false`), root tweens default to `overwrite: @auto` (a new tween of a key retargets the running one), as [`TweenHost`] does |
//! | `tween.timeline(vars)` | `gsap.timeline(vars)`: `defaults`, `delay`, `repeat`, `repeat_delay`, `repeat_refresh`, `yoyo`, `paused`, `reversed`, `time_scale`, `smooth_child_timing`, `auto_remove_children`, `keep`, `reduce`, `id` / `tag`, `on_*` |
//! | `tween.kill_tweens_of(targets, props?)` | `gsap.killTweensOf` (`props`: `"margin.left, width"` or an array of such strings / ids) |
//! | `tween.clear_props(targets, props?)` | `clearProps`: kills the tweens of those keys, forgets them, and puts the widget's DSL values back (`props` as above; none, `true` or `"all"` for every key the tween layer holds) |
//! | `tween.is_tweening(target)` | `gsap.isTweening` |
//! | `tween.ticker({time_scale, paused, reduced_motion, lag_smoothing})`, `tween.ticker()` | `gsap.globalTimeline.timeScale()` / `pause()`, `gsap.ticker.lagSmoothing()`; returns `{time_scale, paused, reduced_motion}`. App-wide: a Splash isolate may read it, not write it. `lag_smoothing` is in SECONDS (GSAP takes ms): a number caps every frame delta (0 turns it off), `[threshold, adjusted]` replaces deltas above `threshold` with `adjusted` |
//! | `tl.to(targets, vars, position?)` / `from` / `from_to(targets, from, to, position?)` / `set` | `tl.to()` ...; return the timeline for chaining |
//! | `tl.add_label(label, position?)`, `tl.call(fn, position?)`, `tl.add_pause(position?, fn?)` | `addLabel`, `call` (no params array: the closure captures what it needs), `addPause` |
//! | `play(from?)`, `pause(at?)`, `resume()`, `reverse(from?)`, `restart(include_delay?, suppress_events?)`, `seek(position, suppress_events?)`, `kill()` | the `Animation` methods |
//! | `time(t?)`, `total_time(t?)`, `progress(p?)`, `total_progress(p?)`, `time_scale(s?)`, `paused(b?)`, `reversed(b?)`, `duration(d?)` | getters without an argument, setters (returning the handle) with one |
//! | `total_duration()`, `iteration()`, `is_active()`, `current_label()`, `is_alive()` | getters; `is_alive()` (not GSAP) is false once the animation is gone: killed, completed and released, or all its widgets gone |
//! | `on_start(fn)`, `on_update(fn)`, `on_repeat(fn)`, `on_complete(fn)`, `on_reverse_complete(fn)`, `on_interrupt(fn)` | `eventCallback`; also as `vars` keys; `nil` removes |
//! | `motion_path: "M0,0 C.."` / `[[x, y], ..]` / `{path, type, curviness, align, auto_rotate, radians, start, end, offset, x, y, z, rotation}` in `to()` vars | `motionPath` (MotionPathPlugin); see "Motion paths" below |
//!
//! `tween.to(..)` returns a handle with the same methods as a timeline
//! handle (the builders answer an error on a tween). As in GSAP, a root
//! tween stays addressable after it completes while script holds its
//! handle (`h.restart()`, `h.reverse()` work; vars `keep: false` opts out);
//! once the handle is collected it is freed. A handle whose animation is
//! gone makes every method a no-op and every getter answer 0 / false / nil.
//!
//! Targets are `ui` handles (`ui`, `ui.name`, `ui.a.b`) or an array of them
//! (in stagger order), built by the calling script's own VM: a widget whose
//! DSL source lives in another VM's heap (a Splash isolate's widgets seen
//! from the host, the host-built Splash widget seen from its isolate) is
//! refused. In `vars`, the keys `duration, delay, ease, ease_each, repeat,
//! repeat_delay, repeat_refresh, yoyo, yoyo_ease, stagger, overwrite,
//! immediate_render, paused, reversed, time_scale, color_space, reduce,
//! keep, inherit, id, tag` and `on_*` are options, `motion_path` makes the
//! tween follow a path (below), and every other key is a property: a nested
//! object is a path (`draw_bg: {color: #f00}` animates `draw_bg.color`, up
//! to four ids deep). Values: numbers, colours,
//! `vec2`/`vec3`/`vec4` (finite lanes), and `"+=n"` / `"-=n"` for relative
//! numeric ends. An ease is an `Ease` value (`Ease.OutCubic`,
//! `theme.motion_ease_*`), a GSAP string (`"power2.out"`, `"back.out(1.7)"`,
//! `"steps(5)"`, `"cubic-bezier(.2,0,0,1)"`) or a CSS preset name
//! (`"ease_out_cubic"`). A stagger is a number (`each`; negative staggers
//! from the end, as in GSAP) or `{each | amount, from: @start | @center |
//! @edges | @end | @random | index | [x, y], grid: [rows, cols], axis: @x |
//! @y, ease, repeat, yoyo, repeat_delay}`. Positions are numbers, `@labels`
//! or GSAP strings (`"<"`, `">-0.1"`, `"+=0.2"`, `"shown+=0.1"`).
//!
//! # Motion paths
//!
//! ```text
//! tween.to(ui.box, {duration: 2.0, ease: "power1.inOut",
//!     motion_path: {path: [[0, 0], [120, -40], [240, 40], [360, 0]], curviness: 1.25,
//!                   align: @start, auto_rotate: true}})
//! ```
//!
//! `motion_path` (GSAP `motionPath`, `to()` and `tl.to()` only) takes an SVG
//! path string (`"M0,0 C50,-80 150,80 200,0"`: `M L H V C S Q T A Z`, arcs
//! become cubics), an array of points (`[[x, y], ..]` or `[[x, y, z], ..]`:
//! a smooth curve through them), or an object:
//! - `path`: the string or the array (required);
//! - `type: @thru | @cubic` (arrays only; default `@thru`; `@cubic` reads
//!   anchor, control, control, anchor, .. with 3k + 1 points) and
//!   `curviness` (`@thru` only; default 1, 0 draws straight lines);
//! - `align: @start | @none | true | false` (default `@none`): `@start`
//!   moves the path so its start sits on the widget's current x / y;
//! - `auto_rotate: false | true | degrees` (default false; a number is true
//!   with that offset) and `radians: true` to write radians;
//! - `start`, `end` (default 0, 1): fractions of the path's length, may
//!   leave 0..1 on a closed path and may be reversed; `offset: [dx, dy]`
//!   (or `[dx, dy, dz]`);
//! - the keys it drives, `@id` or `"a.b.c"`: `x` (default `margin.left`),
//!   `y` (default `margin.top`), `rotation` (default `draw_bg.rotation`,
//!   degrees, written with `auto_rotate`) and `z` (none unless given).
//!
//! A `flow: Overlay` parent makes `margin.left` / `margin.top` an x / y
//! (the child sits at the parent's origin plus its margin; each moving frame
//! relayouts it). The path is parsed once, when the tween is built, and
//! lives as long as the tween: `kill`, `clear_props`, a widget going away
//! free it with the tween. A vars property on a key the path drives is an
//! error, as are `from`, `from_to` and `set` with a `motion_path`. The
//! eased ratio is clamped to the path (an overshooting ease rests at its
//! ends); GSAP's element `align`, `alignOrigin` and `fromCurrent` are not
//! supported (use `offset`).
//!
//! # How it runs
//!
//! Nothing runs inside the VM beyond building: a native turns its arguments
//! into plain `TweenOpts` / `PropTo` data (walking `vars` once), builds into
//! the calling VM's [`TweenHost`] and returns. The script-side driver, one
//! `TweenHost` per VM in a `Cx` global, is stepped by [`handle_event`] at the
//! top of `Root::handle_event` on its `NextFrame` (and on `LiveEdit` /
//! `ScriptReapply`, which re-push every value): it pushes each changed
//! target with [`TweenHost::apply_to_ref`] and redraws it, then drains the
//! engine's events and calls the script closures they name, each inside
//! `contain_isolate_panic`, the VM that owns it and the widget instruction
//! limit. [`draw_check`], from the root's `Event::Draw`, pushes what a build
//! changed since the last frame before anything draws (a `from()` or a
//! `set()` never shows one frame late) and re-arms hosts a ticker pause held.
//! Only `Root` calls the two: an app whose top widget is something else
//! calls them itself, or its script tweens never move.
//!
//! A target is its widget's uid: the widget is looked up with
//! `cx.widget_tree().widget(uid)` (a weak ref caches the last answer). A
//! target whose widget is gone is forgotten (its tweens killed, its slots,
//! apply object and binds freed) at the first of: its next push, any
//! `tween` call of that VM, a `LiveEdit` / `ScriptReapply`. A script-built
//! animation whose widgets are all gone is killed with them (a timeline left
//! with only `call()`s included), so a page rebuilt with new widgets does
//! not keep the old page's timelines and closures alive.
//!
//! # Limitations
//!
//! - Callbacks run after the frame that fired them, not synchronously inside
//!   the render as in GSAP; a callback that seeks or pauses reacts one frame
//!   late at most. `on_update` calls script on every rendered frame: keep it
//!   small and write only what changed.
//! - A `to()` starts from the tween layer's last value, else from the
//!   target's DSL source object at that path (an `instance(..)` default
//!   included). A value the Animator or Rust code changed since the widget
//!   was applied is not seen, and a key the DSL never set has nothing to
//!   start from (debug builds log it once): use `from_to`. A relative end in
//!   `set()` is resolved when the set is built.
//! - Every value the tween layer holds for a widget is pushed with each push
//!   of that widget (and again after a `LiveEdit` / `ScriptReapply`), until
//!   `clear_props` forgets it. The driver pushes before the widgets handle
//!   the frame, so a running Animator animating the same key writes after it
//!   and wins every frame; an idle Animator leaves the tween's value.
//! - Every pushed target is redrawn on each changed frame (layout fields
//!   need it). A push enters the target's VM once per target (an isolate's
//!   costs an install each); a few hundred moving targets is the budget.
//! - An animation outlives its handle (GSAP): once the handle is collected it
//!   plays on and is freed when it completes, or at once if it is paused.
//!   A timeline whose own callbacks reference it stays alive until
//!   `tl.kill()` or until its widgets are gone.
//! - Not supported: array keyframes, function-based values, `snap`, `"*="`,
//!   `call()` params, targets other than `ui` handles, the root widget.

use crate::{
    animator::Ease,
    makepad_draw::*,
    makepad_script::{
        pod::{ScriptPodTy, ScriptPodVec},
        script_err_invalid_args, script_err_type_mismatch, ScriptFnRef,
    },
    tween::{
        parse_gsap_ease, prop_path, set_tween_ticker, tween_ticker, Anchor, AnimMut, AnimRef,
        ColorSpace, Easing, Emit, End, EventKind, EventMask, LagSmoothing, MotionPath, Offset,
        Overwrite, PathAlign, PathOpts, Position, PropKey, PropTo, Reduce, Rgba, Seek, Stagger,
        StaggerAxis, StaggerFrom, Tag, TargetId, Targets, TimelineOpts, TweenEvent, TweenHost,
        TweenId, TweenOpts, TweenValue, ValueKind, YoyoEase,
    },
    widget::{WidgetRef, WidgetUid, WidgetWeakRef},
    widget_async::{
        contain_isolate_panic, current_splash_vm_id, splash_vm_is_live, ui_handle_uid,
        CxSplashVmExt, SplashVmId, MAIN_SPLASH_VM_ID, WIDGET_SCRIPT_INSTRUCTION_LIMIT,
    },
    widget_tree::CxWidgetExt,
};
use std::cell::RefCell;

// ---------------------------------------------------------------------------
// Driver state
// ---------------------------------------------------------------------------

/// The `on_*` callbacks an animation can report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CbKind {
    Start,
    Update,
    Repeat,
    Complete,
    ReverseComplete,
    Interrupt,
}

impl CbKind {
    fn from_key(key: LiveId) -> Option<CbKind> {
        Some(match key {
            live_id!(on_start) => CbKind::Start,
            live_id!(on_update) => CbKind::Update,
            live_id!(on_repeat) => CbKind::Repeat,
            live_id!(on_complete) => CbKind::Complete,
            live_id!(on_reverse_complete) => CbKind::ReverseComplete,
            live_id!(on_interrupt) => CbKind::Interrupt,
            _ => return None,
        })
    }

    fn of(kind: EventKind) -> Option<CbKind> {
        Some(match kind {
            EventKind::Start => CbKind::Start,
            EventKind::Update => CbKind::Update,
            EventKind::Repeat => CbKind::Repeat,
            EventKind::Complete => CbKind::Complete,
            EventKind::ReverseComplete => CbKind::ReverseComplete,
            EventKind::Interrupt => CbKind::Interrupt,
            _ => return None,
        })
    }

    fn mask(self) -> EventMask {
        match self {
            CbKind::Start => EventMask::START,
            CbKind::Update => EventMask::UPDATE,
            CbKind::Repeat => EventMask::REPEAT,
            CbKind::Complete => EventMask::COMPLETE,
            CbKind::ReverseComplete => EventMask::REVERSE_COMPLETE,
            CbKind::Interrupt => EventMask::INTERRUPT,
        }
    }
}

/// An `on_*` closure of one animation.
struct Callback {
    id: TweenId,
    kind: CbKind,
    f: ScriptFnRef,
}

/// A `tl.call()` / `tl.add_pause()` closure, found by its minted tag and
/// released with the timeline that owns it.
struct CallFn {
    owner: TweenId,
    tag: Tag,
    f: ScriptFnRef,
}

/// A path a target's values land at, as it was tweened.
#[derive(Clone, Copy)]
struct KeyPath {
    key: PropKey,
    path: [LiveId; 4],
    len: u8,
}

impl KeyPath {
    fn path(&self) -> &[LiveId] {
        &self.path[..self.len as usize]
    }
}

/// One widget a VM animates.
struct Target {
    uid: WidgetUid,
    tid: TargetId,
    /// The widget as last found; the uid lookup is the fallback.
    weak: WidgetWeakRef,
    /// Every key the tween layer holds for this widget (multi-id paths are
    /// bound with `bind_path` when first added).
    keys: Vec<KeyPath>,
    /// Marked by the push's pass over the change list.
    dirty: bool,
}

/// A script-built root animation and the targets its tracks animate.
struct RootAnim {
    id: TweenId,
    tids: Vec<TargetId>,
}

/// A `clear_props` waiting for the driver: the DSL values to put back.
struct Restore {
    uid: WidgetUid,
    weak: WidgetWeakRef,
    paths: Vec<KeyPath>,
}

/// Everything one script VM (the app's, or a Splash isolate's) animates.
struct VmTweens {
    vm_id: SplashVmId,
    host: TweenHost,
    /// Sorted by `tid` (TargetIds grow and are never reused).
    targets: Vec<Target>,
    next_tid: u32,
    callbacks: Vec<Callback>,
    calls: Vec<CallFn>,
    roots: Vec<RootAnim>,
    restores: Vec<Restore>,
    /// Animations whose script handle was collected.
    orphans: Vec<TweenId>,
    handle_type: Option<ScriptHandleType>,
    /// (widget, key) pairs already reported as having no start value.
    warned: Vec<(WidgetUid, PropKey)>,
    gone: bool,
}

impl VmTweens {
    fn new(vm_id: SplashVmId) -> Self {
        let mut host = TweenHost::new().inspect_named(if vm_id == MAIN_SPLASH_VM_ID {
            "Splash tweens".to_string()
        } else {
            format!("Splash tweens (vm {})", vm_id.0)
        });
        host.set_vm_id(vm_id);
        Self {
            vm_id,
            host,
            targets: Vec::new(),
            next_tid: 0,
            callbacks: Vec::new(),
            calls: Vec::new(),
            roots: Vec::new(),
            restores: Vec::new(),
            orphans: Vec::new(),
            handle_type: None,
            warned: Vec::new(),
            gone: false,
        }
    }

    fn target_ix(&self, uid: WidgetUid) -> Option<usize> {
        self.targets.iter().position(|t| t.uid == uid)
    }

    /// The target of `uid`, created (with the widget `w` as its weak ref)
    /// when missing. TargetIds are never reused.
    fn target(&mut self, uid: WidgetUid, w: &WidgetRef) -> usize {
        if let Some(i) = self.target_ix(uid) {
            return i;
        }
        let tid = TargetId(self.next_tid);
        self.next_tid += 1;
        self.targets.push(Target {
            uid,
            tid,
            weak: w.downgrade(),
            keys: Vec::new(),
            dirty: false,
        });
        self.targets.len() - 1
    }

    /// The widget of target `ix` (empty when it is gone).
    fn resolve(&mut self, cx: &Cx, ix: usize) -> WidgetRef {
        let t = &mut self.targets[ix];
        if let Some(w) = t.weak.upgrade() {
            return w;
        }
        let w = cx.widget_tree().widget(t.uid);
        if !w.is_empty() {
            t.weak = w.downgrade();
        }
        w
    }

    /// Forgets every target whose widget is gone.
    fn sweep(&mut self, cx: &mut Cx) {
        let mut i = 0;
        while i < self.targets.len() {
            if self.resolve(cx, i).is_empty() {
                self.forget(cx, i);
            } else {
                i += 1;
            }
        }
    }

    /// Forgets target `ix`: its tweens' tracks, slots, apply object and
    /// binds go, and a script-built animation left with no widget is killed.
    fn forget(&mut self, cx: &mut Cx, ix: usize) {
        let t = self.targets.remove(ix);
        self.host.forget_target(cx, t.tid);
        self.warned.retain(|(uid, _)| *uid != t.uid);
        for r in self.roots.iter_mut() {
            if let Some(p) = r.tids.iter().position(|x| *x == t.tid) {
                r.tids.swap_remove(p);
                if r.tids.is_empty() {
                    self.host.kill(cx, r.id);
                }
            }
        }
        self.release(cx);
    }

    /// Adds the targets of a build to the root animation `id` (a new entry
    /// for a root tween or timeline, the timeline's for a child).
    fn add_root_targets(&mut self, id: TweenId, tids: &[TargetId]) {
        let r = match self.roots.iter().position(|r| r.id == id) {
            Some(i) => &mut self.roots[i],
            None => {
                self.roots.push(RootAnim {
                    id,
                    tids: Vec::new(),
                });
                self.roots.last_mut().unwrap()
            }
        };
        for tid in tids {
            if !r.tids.contains(tid) {
                r.tids.push(*tid);
            }
        }
    }

    /// Whether `id` still exists, or has events waiting to be drained (a
    /// completed or killed animation reports after it is gone).
    fn reachable(&self, id: TweenId) -> bool {
        self.host.engine.anim_ref(id).is_alive()
            || self.host.engine.events().iter().any(|e| e.id == id)
    }

    /// Pushes every target with changed values onto its widget, and forgets
    /// the targets whose widget is gone.
    fn push(&mut self, cx: &mut Cx, root_uid: WidgetUid) {
        // One pass over the change list marks the targets (sorted by tid);
        // a deferred push can concern a target with no listed change.
        let deferred = self.host.has_deferred_push();
        for &s in self.host.engine.changes() {
            let tid = self.host.engine.slot_key(s).0;
            if let Ok(i) = self.targets.binary_search_by_key(&tid, |t| t.tid) {
                self.targets[i].dirty = true;
            }
        }
        let mut i = 0;
        while i < self.targets.len() {
            let t = &mut self.targets[i];
            let due = std::mem::take(&mut t.dirty) || (deferred && self.host.target_changed(t.tid));
            if !due {
                i += 1;
                continue;
            }
            let w = self.resolve(cx, i);
            // The root is borrowed while this runs: it could never be pushed.
            if w.is_empty() || self.targets[i].uid == root_uid {
                if !w.is_empty() {
                    log!("tween: the root widget cannot be tweened; its tweens are dropped");
                }
                self.forget(cx, i);
                continue;
            }
            let tid = self.targets[i].tid;
            if self.host.apply_to_ref(cx, tid, &w) && w.try_widget_uid().is_some() {
                // Layout fields (width, margin) need a redraw; an instance
                // patch alone would not need one, a redraw costs little.
                w.redraw(cx);
            }
            i += 1;
        }
    }

    /// Puts the DSL values of `clear_props` keys back on their widgets.
    fn restore(&mut self, cx: &mut Cx) {
        if self.restores.is_empty() || cx.is_script_vm_held() {
            return;
        }
        let vm_id = self.vm_id;
        for r in std::mem::take(&mut self.restores) {
            let w = r
                .weak
                .upgrade()
                .unwrap_or_else(|| cx.widget_tree().widget(r.uid));
            if w.is_empty() || w.try_widget_uid().is_none() {
                continue;
            }
            let applied = cx.with_script_vm_id(vm_id, |vm| {
                let key = w.script_source_heap_key();
                let src = w.script_source();
                if key != vm.bx.heap.heap_key() || src == ScriptObject::ZERO {
                    return false;
                }
                let obj = vm.bx.heap.new_object();
                for p in &r.paths {
                    if let Some(v) = source_raw(&vm.bx.heap, src, p.path()) {
                        set_path(vm, obj, p.path(), v);
                    }
                }
                w.script_apply_animate(vm, obj.into())
            });
            if applied {
                w.redraw(cx);
            }
        }
    }

    /// Queues the closures `events` name, in firing order.
    fn collect(&self, events: &[TweenEvent], pending: &mut Vec<(SplashVmId, ScriptFnRef)>) {
        for ev in events {
            match ev.kind {
                EventKind::Call { .. } | EventKind::Pause => {
                    if ev.tag == Tag::NONE {
                        continue;
                    }
                    for c in self.calls.iter().filter(|c| c.tag == ev.tag) {
                        pending.push((self.vm_id, c.f.clone()));
                    }
                }
                kind => {
                    let Some(k) = CbKind::of(kind) else {
                        continue;
                    };
                    for cb in self
                        .callbacks
                        .iter()
                        .filter(|cb| cb.id == ev.id && cb.kind == k)
                    {
                        pending.push((self.vm_id, cb.f.clone()));
                    }
                }
            }
        }
    }

    /// Releases the closures (and root records) of animations that are gone
    /// and have nothing left to report, and kills the orphans (their handle
    /// was collected) that finished or sit paused.
    fn release(&mut self, cx: &mut Cx) {
        if !self.callbacks.is_empty() {
            let mut callbacks = std::mem::take(&mut self.callbacks);
            callbacks.retain(|c| self.reachable(c.id));
            self.callbacks = callbacks;
        }
        if !self.calls.is_empty() {
            let engine = &self.host.engine;
            self.calls.retain(|c| {
                engine.anim_ref(c.owner).is_alive()
                    || engine.events().iter().any(|e| e.tag == c.tag)
            });
        }
        if !self.roots.is_empty() {
            let engine = &self.host.engine;
            self.roots.retain(|r| engine.anim_ref(r.id).is_alive());
        }
        let mut i = 0;
        while i < self.orphans.len() {
            let id = self.orphans[i];
            let a = self.host.engine.anim_ref(id);
            let finished = a.paused()
                || (!a.reversed() && a.total_progress() >= 1.0)
                || (a.reversed() && a.total_time() <= 0.0);
            if !a.is_alive() {
                self.orphans.swap_remove(i);
            } else if finished {
                self.host.kill(cx, id);
                self.orphans.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }
}

/// The script tween driver: one [`VmTweens`] per script VM, stepped by
/// `Root::handle_event`. A `Cx` global.
#[derive(Default)]
struct ScriptTweenDriver {
    vms: Vec<VmTweens>,
    /// The engine events of the frame (capacity reused).
    events: Vec<TweenEvent>,
    /// The closures the frame's events name (capacity reused).
    pending: Vec<(SplashVmId, ScriptFnRef)>,
    /// Isolates reclaimed while the driver was out of the global.
    dead_vms: Vec<SplashVmId>,
    /// Set on the placeholder left in the global while the driver is out:
    /// a native reached then (from inside a widget apply) must not build
    /// into the placeholder.
    taken: bool,
}

impl ScriptTweenDriver {
    fn vm_ix(&mut self, vm_id: SplashVmId) -> usize {
        match self.vms.iter().position(|v| v.vm_id == vm_id) {
            Some(i) => i,
            None => {
                self.vms.push(VmTweens::new(vm_id));
                self.vms.len() - 1
            }
        }
    }
}

/// Takes the driver out of the global so its hosts can take `&mut Cx`,
/// leaving a placeholder marked `taken`.
fn take_driver(cx: &mut Cx) -> ScriptTweenDriver {
    let slot = cx.global::<ScriptTweenDriver>();
    let mut d = std::mem::take(slot);
    slot.taken = true;
    // Already applied by `gc_vms` while the driver was in place.
    d.dead_vms.clear();
    d
}

/// Puts the driver back, applying the isolate reclamations that landed on
/// the placeholder meanwhile.
fn put_driver(cx: &mut Cx, mut d: ScriptTweenDriver) {
    let slot = cx.global::<ScriptTweenDriver>();
    for vm_id in slot.dead_vms.drain(..) {
        d.vms.retain(|v| v.vm_id != vm_id);
    }
    d.taken = false;
    *slot = d;
}

/// Whether the driver is in the global (not out for a step).
fn driver_in_place(cx: &mut Cx) -> bool {
    !cx.global::<ScriptTweenDriver>().taken
}

/// Runs `f` with the calling VM's tweens and `Cx` (the driver taken out of
/// the global meanwhile), after forgetting the targets whose widget is gone.
/// No script may run inside `f`. `None` when the driver is out already (a
/// native reached from inside the driver's own step).
fn with_vm_tweens<R>(
    cx: &mut Cx,
    vm_id: SplashVmId,
    f: impl FnOnce(&mut Cx, &mut VmTweens) -> R,
) -> Option<R> {
    if !driver_in_place(cx) {
        return None;
    }
    let mut d = take_driver(cx);
    let i = d.vm_ix(vm_id);
    d.vms[i].sweep(cx);
    let r = f(cx, &mut d.vms[i]);
    put_driver(cx, d);
    Some(r)
}

thread_local! {
    /// Animations whose script handle the GC collected (a handle's `gc()`
    /// has no `Cx`); the next step adopts them as orphans.
    static DROPPED: RefCell<Vec<(SplashVmId, TweenId)>> = const { RefCell::new(Vec::new()) };
}

/// The payload of a `tween_anim` handle: one tween or timeline of one VM.
struct TweenHandleGc {
    vm_id: SplashVmId,
    id: TweenId,
    timeline: bool,
}

impl ScriptHandleGc for TweenHandleGc {
    fn gc(&mut self) {
        let entry = (self.vm_id, self.id);
        let _ = DROPPED.try_with(|d| d.borrow_mut().push(entry));
    }
}

// ---------------------------------------------------------------------------
// The frame: stepped by Root
// ---------------------------------------------------------------------------

/// Steps every VM's script tweens. `Root::handle_event` calls it first (the
/// VM is free and no widget but the root, `root_uid`, is borrowed there); a
/// top widget other than `Root` must call it the same way. Acts on
/// `NextFrame` (a host's own frame), `LiveEdit` and `ScriptReapply` (the
/// tree was re-applied: gone widgets are forgotten and every value is pushed
/// again); any other event costs one match.
pub fn handle_event(cx: &mut Cx, event: &Event, root_uid: WidgetUid) {
    let reapply = matches!(event, Event::LiveEdit | Event::ScriptReapply);
    if !reapply && !matches!(event, Event::NextFrame(_)) {
        return;
    }
    if !cx.has_global::<ScriptTweenDriver>()
        || current_splash_vm_id(cx) != MAIN_SPLASH_VM_ID
        || !driver_in_place(cx)
    {
        return;
    }
    let mut d = take_driver(cx);
    if d.vms.is_empty() {
        put_driver(cx, d);
        return;
    }
    let _ = DROPPED.try_with(|q| {
        for (vm_id, id) in q.borrow_mut().drain(..) {
            if let Some(vt) = d.vms.iter_mut().find(|v| v.vm_id == vm_id) {
                if !vt.orphans.contains(&id) {
                    vt.orphans.push(id);
                }
            }
        }
    });
    let mut events = std::mem::take(&mut d.events);
    let mut pending = std::mem::take(&mut d.pending);
    let mut gone = false;
    for vt in d.vms.iter_mut() {
        // A reclaimed isolate the purge has not reached yet: entering it
        // would panic.
        if !splash_vm_is_live(cx, vt.vm_id) {
            vt.gone = true;
            gone = true;
            continue;
        }
        if reapply {
            vt.sweep(cx);
        }
        vt.restore(cx);
        let action = vt.host.handle_event(cx, event);
        if action.changed() {
            if vt.vm_id == MAIN_SPLASH_VM_ID {
                vt.push(cx, root_uid);
            } else {
                contain_isolate_panic("tween push", || vt.push(cx, root_uid));
            }
            vt.host.clear_changes();
            vt.host.swap_events(&mut events);
            vt.collect(&events, &mut pending);
        }
        if action.changed() || !vt.orphans.is_empty() {
            vt.release(cx);
        }
    }
    if gone {
        d.vms.retain(|v| !v.gone);
    }
    events.clear();
    d.events = events;
    put_driver(cx, d);
    // The driver is back in place: a callback may build, seek or kill.
    for (vm_id, f) in pending.drain(..) {
        call_closure(cx, vm_id, &f);
    }
    cx.global::<ScriptTweenDriver>().pending = pending;
}

/// Called from the root's `Event::Draw`, before anything draws: pushes what
/// a build or a control changed since the last frame (a `from()` renders
/// its start at once, and a handler that shows a panel and tweens it in the
/// same breath must not draw it one frame at its end state), without
/// stepping time, and re-arms hosts a ticker pause held (`set_tween_ticker`
/// redraws everything once the pause is lifted).
pub fn draw_check(cx: &mut Cx, root_uid: WidgetUid) {
    if !cx.has_global::<ScriptTweenDriver>()
        || current_splash_vm_id(cx) != MAIN_SPLASH_VM_ID
        || !driver_in_place(cx)
    {
        return;
    }
    let mut d = take_driver(cx);
    for vt in d.vms.iter_mut() {
        if !splash_vm_is_live(cx, vt.vm_id) {
            continue;
        }
        vt.host.draw_check(cx);
        vt.restore(cx);
        if vt.host.engine.changes().is_empty() && !vt.host.has_deferred_push() {
            continue;
        }
        if vt.vm_id == MAIN_SPLASH_VM_ID {
            vt.push(cx, root_uid);
        } else {
            contain_isolate_panic("tween push", || vt.push(cx, root_uid));
        }
        vt.host.clear_changes();
    }
    put_driver(cx, d);
}

/// Calls one script closure the way every host callback in this crate is
/// called: in the VM that owns it, with the widget instruction limit and a
/// contained panic, its errors logged.
fn call_closure(cx: &mut Cx, vm_id: SplashVmId, f: &ScriptFnRef) {
    if !splash_vm_is_live(cx, vm_id) {
        return;
    }
    let key = f.heap_key();
    contain_isolate_panic("tween callback", || {
        cx.with_script_vm_id(vm_id, |vm| {
            // Never call a closure against a heap that did not mint it.
            if key != 0 && vm.bx.heap.heap_key() != key {
                return;
            }
            vm.with_instruction_limit(WIDGET_SCRIPT_INSTRUCTION_LIMIT, |vm| {
                vm.call(f.as_object().into(), &[]);
            });
            for err in vm.take_errors() {
                error!("tween callback error: {err}");
            }
        });
    });
}

/// Drops the script tweens of reclaimed isolates (their host, closures and
/// handle type). Called from `gc_dead_splash_isolates`.
pub(crate) fn gc_vms(cx: &mut Cx, dead: &[SplashVmId]) {
    if dead.is_empty() || !cx.has_global::<ScriptTweenDriver>() {
        return;
    }
    let d = cx.global::<ScriptTweenDriver>();
    d.vms.retain(|v| !dead.contains(&v.vm_id));
    d.pending.retain(|(v, _)| !dead.contains(v));
    // For a driver that is out of the global right now (see put_driver).
    d.dead_vms.extend_from_slice(dead);
}

// ---------------------------------------------------------------------------
// Reading script values into plain data
// ---------------------------------------------------------------------------

type Parse<T> = Result<T, String>;

/// A property's value: absolute, or relative (`"+=n"`).
#[derive(Clone, Copy, Debug)]
enum PVal {
    Abs(TweenValue),
    Rel(f64),
}

/// What a vars property is: a value, or a key a `motion_path` drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PRole {
    Value,
    PathX,
    PathY,
    PathZ,
    PathRot,
}

/// One property of a vars object: its path and value.
#[derive(Clone, Copy, Debug)]
struct PProp {
    path: [LiveId; 4],
    len: u8,
    val: PVal,
    role: PRole,
}

impl PProp {
    fn path(&self) -> &[LiveId] {
        &self.path[..self.len as usize]
    }

    fn key(&self) -> PropKey {
        prop_path(self.path())
    }
}

/// A key path of up to four ids (`margin.left`).
#[derive(Clone, Copy, Debug)]
struct IdPath {
    path: [LiveId; 4],
    len: u8,
}

impl IdPath {
    fn new(ids: &[LiveId]) -> Self {
        let mut path = [LiveId(0); 4];
        path[..ids.len()].copy_from_slice(ids);
        Self {
            path,
            len: ids.len() as u8,
        }
    }

    fn ids(&self) -> &[LiveId] {
        &self.path[..self.len as usize]
    }
}

/// A parsed `motion_path`: the geometry (built once, while parsing), its
/// GSAP options and the keys it drives.
struct PathSpec {
    geom: MotionPath,
    start: f64,
    end: f64,
    offset: [f64; 3],
    align: PathAlign,
    /// `auto_rotate`: the offset in degrees, and whether to write radians.
    rotate: Option<(f64, bool)>,
    x: IdPath,
    y: IdPath,
    z: Option<IdPath>,
    rot: IdPath,
}

/// A parsed tween vars object.
#[derive(Default)]
struct TweenSpec {
    opts: TweenOpts,
    props: Vec<PProp>,
    cbs: Vec<(CbKind, ScriptFnRef)>,
    path: Option<PathSpec>,
}

fn path_text(path: &[LiveId]) -> String {
    let mut s = String::new();
    for (i, id) in path.iter().enumerate() {
        if i > 0 {
            s.push('.');
        }
        s.push_str(&id.to_string());
    }
    s
}

/// The own keyed entries of `obj` (a vars literal's `key: value` pairs).
fn own_entries(vm: &ScriptVm, obj: ScriptObject) -> Vec<(LiveId, ScriptValue)> {
    vm.bx
        .heap
        .map_ref(obj)
        .iter()
        .filter_map(|(k, v)| Some((k.as_id()?, v.value)))
        .collect()
}

fn fn_object(vm: &ScriptVm, v: ScriptValue) -> Option<ScriptObject> {
    v.as_object().filter(|o| vm.bx.heap.is_fn(*o))
}

fn string_of(vm: &ScriptVm, v: ScriptValue) -> Option<String> {
    if !v.is_string_like() {
        return None;
    }
    vm.bx.heap.string_with(v, |_, s| s.to_string())
}

fn number(v: ScriptValue, what: LiveId) -> Parse<f64> {
    match v.as_number() {
        Some(n) if n.is_finite() => Ok(n),
        _ => Err(format!("{what} takes a number")),
    }
}

fn boolean(v: ScriptValue, what: LiveId) -> Parse<bool> {
    if let Some(b) = v.as_bool() {
        return Ok(b);
    }
    match v.as_number() {
        Some(n) => Ok(n != 0.0),
        None => Err(format!("{what} takes true or false")),
    }
}

/// A `repeat` count: -1 (or less) repeats forever.
fn repeat_count(v: ScriptValue, what: LiveId) -> Parse<i32> {
    Ok(number(v, what)?.max(-1.0).min(i32::MAX as f64) as i32)
}

/// An id or a string, as a name (`@center`, `"center"`).
fn name_of(vm: &ScriptVm, v: ScriptValue) -> Option<LiveId> {
    if let Some(s) = string_of(vm, v) {
        return Some(LiveId::from_str(s.trim()));
    }
    v.as_id()
}

/// A number, colour or f32 vector: the value a tween can carry.
fn tween_value(heap: &ScriptHeap, v: ScriptValue) -> Option<TweenValue> {
    if let Some(c) = v.as_color() {
        return Some(TweenValue::Color(Rgba::from_u32(c)));
    }
    if let Some(n) = v.as_number() {
        return n.is_finite().then_some(TweenValue::F64(n));
    }
    let p = v.as_pod()?;
    let (ty, data) = heap.pod_data(p);
    let f = |i: usize| f32::from_bits(data[i]) as f64;
    let (value, lanes) = match ty.ty {
        ScriptPodTy::Vec(ScriptPodVec::Vec2f) => (TweenValue::Vec2([f(0), f(1)]), 2),
        ScriptPodTy::Vec(ScriptPodVec::Vec3f) => (TweenValue::Vec3([f(0), f(1), f(2)]), 3),
        ScriptPodTy::Vec(ScriptPodVec::Vec4f) => (TweenValue::Vec4([f(0), f(1), f(2), f(3)]), 4),
        _ => return None,
    };
    // Like numbers: a NaN or infinite lane is not a value to animate.
    (0..lanes).all(|i| f(i).is_finite()).then_some(value)
}

fn prop_value(vm: &ScriptVm, v: ScriptValue, path: &[LiveId]) -> Parse<PVal> {
    if let Some(t) = tween_value(&vm.bx.heap, v) {
        return Ok(PVal::Abs(t));
    }
    if let Some(s) = string_of(vm, v) {
        let s = s.trim();
        let parsed = if let Some(rest) = s.strip_prefix("+=") {
            rest.trim().parse::<f64>().ok()
        } else if let Some(rest) = s.strip_prefix("-=") {
            rest.trim().parse::<f64>().ok().map(|d| -d)
        } else {
            None
        };
        return match parsed {
            Some(d) if d.is_finite() => Ok(PVal::Rel(d)),
            _ => Err(format!(
                "{}: \"{s}\" is not a relative value (\"+=10\", \"-=10\")",
                path_text(path)
            )),
        };
    }
    Err(format!(
        "{}: a tween animates numbers, colours and vec2/vec3/vec4, not {:?}",
        path_text(path),
        v.value_type()
    ))
}

/// Collects the properties under `obj` (a nested vars object) at `prefix`.
fn collect_props(
    vm: &ScriptVm,
    obj: ScriptObject,
    prefix: &[LiveId],
    out: &mut Vec<PProp>,
) -> Parse<()> {
    for (key, v) in own_entries(vm, obj) {
        let mut path = [LiveId(0); 4];
        let len = prefix.len() + 1;
        if len > 4 {
            return Err(format!(
                "{}.{key}: a property path is at most four ids deep",
                path_text(prefix)
            ));
        }
        path[..prefix.len()].copy_from_slice(prefix);
        path[prefix.len()] = key;
        add_prop(vm, v, &path[..len], out)?;
    }
    Ok(())
}

fn add_prop(vm: &ScriptVm, v: ScriptValue, path: &[LiveId], out: &mut Vec<PProp>) -> Parse<()> {
    if let Some(obj) = v.as_object() {
        if vm.bx.heap.is_fn(obj) {
            return Err(format!(
                "{}: a function is not a property value (callbacks are on_start, on_update, on_repeat, on_complete, on_reverse_complete, on_interrupt)",
                path_text(path)
            ));
        }
        return collect_props(vm, obj, path, out);
    }
    let val = prop_value(vm, v, path)?;
    let mut p = [LiveId(0); 4];
    p[..path.len()].copy_from_slice(path);
    if let Some(old) = out.iter_mut().find(|q| q.path() == path) {
        old.val = val;
    } else {
        out.push(PProp {
            path: p,
            len: path.len() as u8,
            val,
            role: PRole::Value,
        });
    }
    Ok(())
}

/// An ease: an `Ease` value, a GSAP string or a CSS preset name.
fn parse_ease(vm: &mut ScriptVm, v: ScriptValue) -> Parse<Easing> {
    if v.as_object().is_some() && Ease::script_type_check(&vm.bx.heap, v) {
        let e = Ease::script_from_value(vm, v);
        return Ok(Easing::from(&e));
    }
    if let Some(s) = string_of(vm, v) {
        let s = s.trim();
        return parse_gsap_ease(s)
            .or_else(|| Easing::css_preset(s))
            .ok_or_else(|| format!("ease \"{s}\" is not a GSAP ease or a CSS preset name"));
    }
    Err("ease takes an Ease value (Ease.OutCubic, theme.motion_ease_*), a GSAP string (\"power2.out\") or a CSS preset name".into())
}

fn parse_stagger(vm: &mut ScriptVm, v: ScriptValue) -> Parse<Stagger> {
    // A negative each (or amount) staggers from the end, as in GSAP.
    if v.as_number().is_some() {
        return Ok(Stagger::each(number(v, live_id!(stagger))?));
    }
    let Some(obj) = v.as_object() else {
        return Err(
            "stagger takes a number (each) or {each, amount, from, grid, axis, ease}".into(),
        );
    };
    let (mut each, mut amount) = (None, None);
    let mut from = StaggerFrom::Index(0);
    let mut grid = None;
    let mut axis = None;
    let mut ease = None;
    let (mut repeat, mut yoyo, mut repeat_delay) = (None, None, None);
    for (key, v) in own_entries(vm, obj) {
        match key {
            live_id!(each) => each = Some(number(v, key)?),
            live_id!(amount) => amount = Some(number(v, key)?),
            live_id!(from) => from = parse_from(vm, v)?,
            live_id!(grid) => grid = Some(pair(vm, v, "grid takes [rows, cols]")?),
            live_id!(axis) => {
                axis = Some(match name_of(vm, v) {
                    Some(live_id!(x)) => StaggerAxis::X,
                    Some(live_id!(y)) => StaggerAxis::Y,
                    _ => return Err("stagger axis takes @x or @y".into()),
                })
            }
            live_id!(ease) => ease = Some(parse_ease(vm, v)?),
            live_id!(repeat) => repeat = Some(repeat_count(v, key)?),
            live_id!(yoyo) => yoyo = Some(boolean(v, key)?),
            live_id!(repeat_delay) => repeat_delay = Some(number(v, key)?),
            _ => return Err(format!("stagger has no option {key}")),
        }
    }
    let mut s = match (amount, each) {
        (Some(a), _) => Stagger::amount(a),
        (None, Some(e)) => Stagger::each(e),
        (None, None) => return Err("stagger needs each or amount".into()),
    };
    s = s.from(from);
    if let Some((rows, cols)) = grid {
        if rows < 1.0 || cols < 1.0 {
            return Err("stagger grid needs at least one row and one column".into());
        }
        s = s.grid(rows as u32, cols as u32);
    }
    if let Some(a) = axis {
        s = s.axis(a);
    }
    if let Some(e) = ease {
        s = s.ease(e);
    }
    s.repeat = repeat;
    s.yoyo = yoyo;
    s.repeat_delay = repeat_delay;
    Ok(s)
}

/// Two numbers from a two-element array.
fn pair(vm: &ScriptVm, v: ScriptValue, what: &str) -> Parse<(f64, f64)> {
    let Some(arr) = v.as_array() else {
        return Err(what.into());
    };
    let heap = &vm.bx.heap;
    if heap.array_len(arr) != 2 {
        return Err(what.into());
    }
    let a = heap.array_index(arr, 0, NoTrap).as_number();
    let b = heap.array_index(arr, 1, NoTrap).as_number();
    match (a, b) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() => Ok((a, b)),
        _ => Err(what.into()),
    }
}

fn parse_from(vm: &ScriptVm, v: ScriptValue) -> Parse<StaggerFrom> {
    if let Some(n) = v.as_number() {
        if n.is_finite() && n >= 0.0 {
            return Ok(StaggerFrom::Index(n as u32));
        }
        return Err("stagger from index must be 0 or more".into());
    }
    if v.as_array().is_some() {
        let (x, y) = pair(vm, v, "stagger from takes [x, y] ratios")?;
        return Ok(StaggerFrom::Ratio(x, y));
    }
    Ok(match name_of(vm, v) {
        Some(live_id!(start)) => StaggerFrom::Start,
        Some(live_id!(center)) => StaggerFrom::Center,
        Some(live_id!(edges)) => StaggerFrom::Edges,
        Some(live_id!(end)) => StaggerFrom::End,
        Some(live_id!(random)) => StaggerFrom::Random(LiveId::unique().0),
        _ => {
            return Err(
                "stagger from takes @start, @center, @edges, @end, @random, an index or [x, y]"
                    .into(),
            )
        }
    })
}

fn parse_callback(
    vm: &mut ScriptVm,
    kind: CbKind,
    v: ScriptValue,
    events: &mut EventMask,
    cbs: &mut Vec<(CbKind, ScriptFnRef)>,
) -> Parse<()> {
    if v.is_nil() {
        return Ok(());
    }
    let Some(obj) = fn_object(vm, v) else {
        return Err(format!("{kind:?} callback must be a function"));
    };
    *events = events.with(kind.mask());
    cbs.push((kind, vm.bx.heap.new_fn_ref(obj)));
    Ok(())
}

/// Applies option `key` of a tween vars object; `Ok(false)` when `key` is
/// not an option (so it is a property).
fn tween_option(
    vm: &mut ScriptVm,
    key: LiveId,
    v: ScriptValue,
    o: &mut TweenOpts,
    cbs: &mut Vec<(CbKind, ScriptFnRef)>,
) -> Parse<bool> {
    if let Some(kind) = CbKind::from_key(key) {
        parse_callback(vm, kind, v, &mut o.events, cbs)?;
        return Ok(true);
    }
    match key {
        live_id!(duration) => o.duration = Some(number(v, key)?.max(0.0)),
        live_id!(delay) => o.delay = Some(number(v, key)?),
        live_id!(ease) => o.ease = Some(parse_ease(vm, v)?),
        live_id!(ease_each) => o.ease_each = Some(parse_ease(vm, v)?),
        live_id!(yoyo_ease) => {
            o.yoyo_ease = Some(match v.as_bool() {
                Some(true) => YoyoEase::Invert,
                Some(false) => return Ok(true),
                None => YoyoEase::Ease(parse_ease(vm, v)?),
            })
        }
        live_id!(repeat) => o.repeat = Some(repeat_count(v, key)?),
        live_id!(repeat_delay) => o.repeat_delay = Some(number(v, key)?),
        live_id!(repeat_refresh) => o.repeat_refresh = Some(boolean(v, key)?),
        live_id!(yoyo) => o.yoyo = Some(boolean(v, key)?),
        live_id!(stagger) => o.stagger = Some(parse_stagger(vm, v)?),
        live_id!(overwrite) => {
            o.overwrite = Some(match v.as_bool() {
                Some(true) => Overwrite::All,
                Some(false) => Overwrite::None,
                None => match name_of(vm, v) {
                    Some(live_id!(auto)) => Overwrite::Auto,
                    Some(live_id!(all)) => Overwrite::All,
                    Some(live_id!(none)) => Overwrite::None,
                    _ => return Err("overwrite takes true, false or @auto".into()),
                },
            })
        }
        live_id!(immediate_render) => o.immediate_render = Some(boolean(v, key)?),
        live_id!(paused) => o.paused = Some(boolean(v, key)?),
        live_id!(reversed) => o.reversed = Some(boolean(v, key)?),
        live_id!(time_scale) => o.time_scale = Some(number(v, key)?),
        live_id!(color_space) => {
            o.color_space = Some(match name_of(vm, v) {
                Some(live_id!(srgb)) => ColorSpace::Srgb,
                Some(live_id!(linear)) => ColorSpace::Linear,
                Some(live_id!(hsv)) => ColorSpace::Hsv,
                Some(live_id!(oklab)) => ColorSpace::Oklab,
                Some(live_id!(oklch)) => ColorSpace::Oklch,
                _ => return Err("color_space takes @srgb, @linear, @hsv, @oklab or @oklch".into()),
            })
        }
        live_id!(reduce) => o.reduce = Some(parse_reduce(vm, v)?),
        live_id!(keep) => o.keep = Some(boolean(v, key)?),
        live_id!(inherit) => o.inherit = Some(boolean(v, key)?),
        live_id!(id) | live_id!(tag) => o.tag = parse_tag(vm, v)?,
        _ => return Ok(false),
    }
    Ok(true)
}

fn parse_reduce(vm: &ScriptVm, v: ScriptValue) -> Parse<Reduce> {
    Ok(match name_of(vm, v) {
        Some(live_id!(jump_to_end)) | Some(live_id!(end)) => Reduce::JumpToEnd,
        Some(live_id!(freeze)) => Reduce::Freeze,
        Some(live_id!(keep)) => Reduce::Keep,
        _ => return Err("reduce takes @jump_to_end, @freeze or @keep".into()),
    })
}

/// A label or GSAP `id`: an id or a string (hashed like `@name`).
fn parse_tag(vm: &ScriptVm, v: ScriptValue) -> Parse<Tag> {
    match name_of(vm, v) {
        Some(id) if id.0 != 0 => Ok(Tag(id.0)),
        _ => Err("a label or id takes @name or \"name\"".into()),
    }
}

/// A tween vars object: options and properties (and callbacks).
fn parse_tween_vars(vm: &mut ScriptVm, v: ScriptValue) -> Parse<TweenSpec> {
    let mut spec = TweenSpec::default();
    if v.is_nil() {
        return Ok(spec);
    }
    let Some(obj) = v.as_object() else {
        return Err("vars must be an object ({duration: 0.3, x: 10.0})".into());
    };
    for (key, v) in own_entries(vm, obj) {
        if key == live_id!(motion_path) {
            spec.path = Some(parse_motion_path(vm, v)?);
            continue;
        }
        if tween_option(vm, key, v, &mut spec.opts, &mut spec.cbs)? {
            continue;
        }
        add_prop(vm, v, &[key], &mut spec.props)?;
    }
    // The keys a motion path drives join the props (after every vars key,
    // so a vars property on one of them is found).
    if let Some(ps) = &spec.path {
        let mut keys = vec![(PRole::PathX, ps.x), (PRole::PathY, ps.y)];
        if let Some(z) = ps.z {
            keys.push((PRole::PathZ, z));
        }
        if ps.rotate.is_some() {
            keys.push((PRole::PathRot, ps.rot));
        }
        for (role, ip) in keys {
            if let Some(q) = spec.props.iter().find(|q| q.path() == ip.ids()) {
                return Err(if q.role == PRole::Value {
                    format!(
                        "motion_path drives {}; remove it from vars",
                        path_text(ip.ids())
                    )
                } else {
                    format!(
                        "motion_path: x, y, z and rotation need different keys ({} twice)",
                        path_text(ip.ids())
                    )
                });
            }
            spec.props.push(PProp {
                path: ip.path,
                len: ip.len,
                val: PVal::Abs(TweenValue::F64(0.0)),
                role,
            });
        }
    }
    Ok(spec)
}

/// A key path for a `motion_path` key: `@id` or `"a.b.c"` (at most four ids).
fn motion_key(vm: &ScriptVm, v: ScriptValue, what: &str) -> Parse<IdPath> {
    if let Some(s) = string_of(vm, v) {
        let ids: Vec<LiveId> = s
            .split('.')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(LiveId::from_str)
            .collect();
        if ids.is_empty() {
            return Err(format!("motion_path {what} takes @id or \"a.b.c\""));
        }
        if ids.len() > 4 {
            return Err(format!(
                "motion_path {what}: \"{s}\" is deeper than four ids"
            ));
        }
        return Ok(IdPath::new(&ids));
    }
    match v.as_id() {
        Some(id) if id.0 != 0 => Ok(IdPath::new(&[id])),
        _ => Err(format!("motion_path {what} takes @id or \"a.b.c\"")),
    }
}

/// The points of a `motion_path` array: `[[x, y], ..]` or `[[x, y, z], ..]`.
fn motion_points(vm: &ScriptVm, arr: ScriptArray) -> Parse<Vec<[f64; 3]>> {
    let heap = &vm.bx.heap;
    let n = heap.array_len(arr);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let item = heap.array_index(arr, i, NoTrap);
        let bad = || format!("motion_path: point {i} is not [x, y] or [x, y, z]");
        let Some(p) = item.as_array() else {
            return Err(bad());
        };
        let len = heap.array_len(p);
        if len != 2 && len != 3 {
            return Err(bad());
        }
        let mut q = [0.0; 3];
        for (k, c) in q.iter_mut().enumerate().take(len) {
            match heap.array_index(p, k, NoTrap).as_number() {
                Some(x) if x.is_finite() => *c = x,
                _ => return Err(bad()),
            }
        }
        out.push(q);
    }
    Ok(out)
}

/// `motion_path` (GSAP `motionPath`): an SVG path string, an array of
/// points (a `thru` curve) or `{path, type, curviness, align, auto_rotate,
/// radians, start, end, offset, x, y, z, rotation}`. The geometry is built
/// here, once.
fn parse_motion_path(vm: &mut ScriptVm, v: ScriptValue) -> Parse<PathSpec> {
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        Thru,
        Cubic,
    }
    let mut ps = PathSpec {
        geom: MotionPath::default(),
        start: 0.0,
        end: 1.0,
        offset: [0.0; 3],
        align: PathAlign::None,
        rotate: None,
        x: IdPath::new(&[live_id!(margin), live_id!(left)]),
        y: IdPath::new(&[live_id!(margin), live_id!(top)]),
        z: None,
        rot: IdPath::new(&[live_id!(draw_bg), live_id!(rotation)]),
    };
    let shorthand = string_of(vm, v).is_some() || v.as_array().is_some();
    let mut path = shorthand.then_some(v);
    let (mut kind, mut curviness) = (None, None);
    let (mut auto, mut radians) = (None, false);
    if !shorthand {
        let Some(obj) = v.as_object() else {
            return Err("motion_path takes an SVG path string, [[x, y], ..] or {path, ..}".into());
        };
        for (key, v) in own_entries(vm, obj) {
            match key {
                live_id!(path) => path = Some(v),
                live_id!(type) => {
                    kind = Some(match name_of(vm, v) {
                        Some(live_id!(thru)) => Kind::Thru,
                        Some(live_id!(cubic)) => Kind::Cubic,
                        _ => return Err("motion_path type takes @thru or @cubic".into()),
                    })
                }
                live_id!(curviness) => curviness = Some(number(v, key)?),
                live_id!(align) => {
                    ps.align = match v.as_bool() {
                        Some(true) => PathAlign::Start,
                        Some(false) => PathAlign::None,
                        None => match name_of(vm, v) {
                            Some(live_id!(start)) => PathAlign::Start,
                            Some(live_id!(none)) => PathAlign::None,
                            _ => {
                                return Err(
                                    "motion_path align takes @start, @none, true or false".into()
                                )
                            }
                        },
                    }
                }
                live_id!(auto_rotate) => {
                    auto = match v.as_bool() {
                        Some(b) => b.then_some(0.0),
                        None => match v.as_number() {
                            Some(n) if n.is_finite() => Some(n),
                            _ => {
                                return Err(
                                    "motion_path auto_rotate takes true, false or degrees".into()
                                )
                            }
                        },
                    }
                }
                live_id!(radians) => radians = boolean(v, key)?,
                live_id!(start) => ps.start = number(v, key)?,
                live_id!(end) => ps.end = number(v, key)?,
                live_id!(offset) => {
                    let bad = "motion_path offset takes [dx, dy] or [dx, dy, dz]";
                    let Some(arr) = v.as_array() else {
                        return Err(bad.into());
                    };
                    let heap = &vm.bx.heap;
                    let len = heap.array_len(arr);
                    if len != 2 && len != 3 {
                        return Err(bad.into());
                    }
                    for k in 0..len {
                        match heap.array_index(arr, k, NoTrap).as_number() {
                            Some(x) if x.is_finite() => ps.offset[k] = x,
                            _ => return Err(bad.into()),
                        }
                    }
                }
                live_id!(x) => ps.x = motion_key(vm, v, "x")?,
                live_id!(y) => ps.y = motion_key(vm, v, "y")?,
                live_id!(z) => ps.z = Some(motion_key(vm, v, "z")?),
                live_id!(rotation) => ps.rot = motion_key(vm, v, "rotation")?,
                _ => return Err(format!("motion_path has no option {key}")),
            }
        }
    }
    let Some(path) = path else {
        return Err("motion_path needs a path (an SVG path string or [[x, y], ..])".into());
    };
    let geom = if let Some(d) = string_of(vm, path) {
        if kind.is_some() || curviness.is_some() {
            return Err("motion_path type and curviness are for point arrays".into());
        }
        MotionPath::from_svg(&d)
    } else if let Some(arr) = path.as_array() {
        let pts = motion_points(vm, arr)?;
        match kind.unwrap_or(Kind::Thru) {
            Kind::Thru => MotionPath::through3(&pts, curviness.unwrap_or(1.0)),
            Kind::Cubic => {
                if curviness.is_some() {
                    return Err("motion_path curviness is for type @thru".into());
                }
                MotionPath::cubic_points3(&pts)
            }
        }
    } else {
        return Err("motion_path path takes an SVG path string or [[x, y], ..]".into());
    };
    ps.geom = geom.map_err(|e| format!("motion_path: {e}"))?;
    ps.rotate = auto.map(|deg| (deg, radians));
    Ok(ps)
}

/// A timeline vars object: options only (`defaults` holds tween options).
fn parse_timeline_vars(
    vm: &mut ScriptVm,
    v: ScriptValue,
) -> Parse<(TimelineOpts, Vec<(CbKind, ScriptFnRef)>)> {
    let mut o = TimelineOpts::new();
    let mut cbs = Vec::new();
    if v.is_nil() {
        return Ok((o, cbs));
    }
    let Some(obj) = v.as_object() else {
        return Err("timeline vars must be an object".into());
    };
    for (key, v) in own_entries(vm, obj) {
        if let Some(kind) = CbKind::from_key(key) {
            parse_callback(vm, kind, v, &mut o.events, &mut cbs)?;
            continue;
        }
        match key {
            live_id!(defaults) => {
                let Some(dobj) = v.as_object() else {
                    return Err("defaults takes an object of tween options".into());
                };
                let mut d = TweenOpts::new();
                let mut none = Vec::new();
                for (k, v) in own_entries(vm, dobj) {
                    if CbKind::from_key(k).is_some() || !tween_option(vm, k, v, &mut d, &mut none)?
                    {
                        return Err(format!("defaults takes tween options; {k} is not one"));
                    }
                }
                o.defaults = d;
            }
            live_id!(delay) => o.delay = number(v, key)?,
            live_id!(repeat) => o.repeat = repeat_count(v, key)?,
            live_id!(repeat_delay) => o.repeat_delay = number(v, key)?,
            live_id!(repeat_refresh) => o.repeat_refresh = boolean(v, key)?,
            live_id!(yoyo) => o.yoyo = boolean(v, key)?,
            live_id!(paused) => o.paused = boolean(v, key)?,
            live_id!(reversed) => o.reversed = boolean(v, key)?,
            live_id!(time_scale) => o.time_scale = number(v, key)?,
            live_id!(smooth_child_timing) => o.smooth_child_timing = boolean(v, key)?,
            live_id!(auto_remove_children) => o.auto_remove_children = boolean(v, key)?,
            live_id!(keep) => o.keep = boolean(v, key)?,
            live_id!(reduce) => o.reduce = parse_reduce(vm, v)?,
            live_id!(id) | live_id!(tag) => o.tag = parse_tag(vm, v)?,
            _ => return Err(format!("timeline vars take options only; {key} is not one")),
        }
    }
    Ok((o, cbs))
}

/// Targets: a `ui` handle or an array of them, deduplicated in order.
fn parse_targets(vm: &ScriptVm, v: ScriptValue) -> Parse<Vec<WidgetUid>> {
    let mut out = Vec::new();
    if let Some(uid) = ui_handle_uid(vm, v) {
        out.push(uid);
        return Ok(out);
    }
    if let Some(arr) = v.as_array() {
        let len = vm.bx.heap.array_len(arr);
        for i in 0..len {
            let item = vm.bx.heap.array_index(arr, i, NoTrap);
            let Some(uid) = ui_handle_uid(vm, item) else {
                return Err(format!("target {i} is not a ui handle (ui, ui.name)"));
            };
            if !out.contains(&uid) {
                out.push(uid);
            }
        }
        if out.is_empty() {
            return Err("no targets".into());
        }
        return Ok(out);
    }
    Err("targets are ui handles (ui, ui.name) or an array of them; self is a widget's source object, not a widget".into())
}

/// A timeline position: nil (the end), a number, `@label` or a GSAP string.
fn parse_position(vm: &ScriptVm, v: ScriptValue) -> Parse<Position> {
    if v.is_nil() {
        return Ok(Position::END);
    }
    if let Some(s) = string_of(vm, v) {
        return Position::parse(&s, |name| Tag(LiveId::from_str(name).0))
            .map_err(|e| format!("position \"{s}\": {e:?}"));
    }
    if let Some(n) = v.as_number() {
        if n.is_finite() {
            return Ok(Position::at(n));
        }
    }
    if let Some(id) = v.as_id() {
        return Ok(Position::label(Tag(id.0)));
    }
    Err("a position is a number, @label or a string (\"<\", \"+=0.2\", \"label+=0.1\")".into())
}

/// A seek target: a time, `@label` or `"label+=n"`.
fn parse_seek(vm: &ScriptVm, v: ScriptValue) -> Parse<Seek> {
    let p = parse_position(vm, v)?;
    match (p.anchor, p.offset) {
        (Anchor::Zero, Offset::Secs(t)) if !v.is_nil() => Ok(Seek::Time(t)),
        (Anchor::Label(l), Offset::Secs(d)) => Ok(Seek::Label(l, d)),
        _ => Err("seek takes a time, @label or \"label+=n\"".into()),
    }
}

/// Property keys for `kill_tweens_of`: `"margin.left, width"`, or an array
/// of such strings and ids.
fn parse_keys(vm: &ScriptVm, v: ScriptValue) -> Parse<Option<Vec<PropKey>>> {
    if v.is_nil() {
        return Ok(None);
    }
    fn split(s: &str, out: &mut Vec<PropKey>) {
        for part in s.split(',') {
            let ids: Vec<LiveId> = part
                .split('.')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(LiveId::from_str)
                .collect();
            if !ids.is_empty() {
                out.push(prop_path(&ids));
            }
        }
    }
    let mut out = Vec::new();
    if let Some(s) = string_of(vm, v) {
        split(&s, &mut out);
    } else if let Some(arr) = v.as_array() {
        for i in 0..vm.bx.heap.array_len(arr) {
            let item = vm.bx.heap.array_index(arr, i, NoTrap);
            if let Some(s) = string_of(vm, item) {
                split(&s, &mut out);
            } else if let Some(id) = item.as_id() {
                out.push(prop_path(&[id]));
            } else {
                return Err("kill_tweens_of props take strings or ids".into());
            }
        }
    } else {
        return Err("kill_tweens_of props take \"a.b, c\" or an array".into());
    }
    Ok(Some(out))
}

/// The value at `path` of a widget's `#[source]` object `src`, as the DSL
/// wrote it: `instance(..)` / `uniform(..)` defaults are unwrapped, and a
/// number at a prefix covers the rest of the path (`margin: 8` gives
/// `margin.left`). `None` when the source does not set it.
fn source_raw(heap: &ScriptHeap, src: ScriptObject, path: &[LiveId]) -> Option<ScriptValue> {
    let unwrap = |v: ScriptValue| -> ScriptValue {
        if let Some(o) = v.as_object() {
            let p = heap.proto(o);
            if p.is_color() || p.as_number().is_some() || p.as_pod().is_some() {
                return p;
            }
        }
        v
    };
    let mut v: ScriptValue = src.into();
    for (i, id) in path.iter().enumerate() {
        let Some(obj) = v.as_object() else {
            // A scalar that covers the rest of the path.
            return (i > 0 && v.as_number().is_some()).then_some(v);
        };
        v = unwrap(heap.value(obj, (*id).into(), NoTrap));
    }
    (v.is_color() || v.as_number().is_some() || v.as_pod().is_some()).then_some(v)
}

/// The widget's applied value at `path`, read from its `#[source]` object
/// (see [`source_raw`]). `None` also when the widget is empty, borrowed
/// right now (a View running its on_render), or its source lives in another
/// heap than `vm`'s.
fn source_value(vm: &ScriptVm, w: &WidgetRef, path: &[LiveId]) -> Option<TweenValue> {
    w.try_widget_uid()?;
    if w.script_source_heap_key() != vm.bx.heap.heap_key() {
        return None;
    }
    let src = w.script_source();
    if src == ScriptObject::ZERO {
        return None;
    }
    let v = source_raw(&vm.bx.heap, src, path)?;
    tween_value(&vm.bx.heap, v)
}

/// Writes `v` at `path` of `obj`, creating the intermediate objects.
fn set_path(vm: &mut ScriptVm, obj: ScriptObject, path: &[LiveId], v: ScriptValue) {
    let Some((last, prefix)) = path.split_last() else {
        return;
    };
    let mut parent = obj;
    for id in prefix {
        parent = match vm.bx.heap.value(parent, (*id).into(), NoTrap).as_object() {
            Some(child) => child,
            None => {
                let child = vm.bx.heap.new_object();
                vm.bx.heap.set_value_def(parent, (*id).into(), child.into());
                child
            }
        };
    }
    vm.bx.heap.set_value_def(parent, (*last).into(), v);
}

/// How many lanes a value of `kind` carries.
fn lanes(kind: ValueKind) -> u8 {
    match kind {
        ValueKind::F64 | ValueKind::Int => 1,
        ValueKind::Vec2 => 2,
        ValueKind::Vec3 => 3,
        ValueKind::Vec4 | ValueKind::Color => 4,
    }
}

/// `v` as a value of `kind`: a number fills every lane (as `Apply` does),
/// colours and vec4s share their lanes, vectors keep the lanes they have.
fn coerce(v: TweenValue, kind: ValueKind) -> TweenValue {
    if v.kind() == kind {
        return v;
    }
    let l = v.to_lanes();
    let l = match v.kind() {
        ValueKind::F64 | ValueKind::Int => [l[0]; 4],
        _ => l,
    };
    TweenValue::from_lanes(kind, l)
}

// ---------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Verb {
    To,
    From,
    FromTo,
    Set,
}

fn err(vm: &mut ScriptVm, msg: String) -> ScriptValue {
    script_err_invalid_args!(vm.trap(), "tween: {}", msg)
}

/// Builds one tween (root level when `into` is `None`, else a child of the
/// timeline `into` at `position`). Returns the new animation.
fn build_tween(
    vm: &mut ScriptVm,
    verb: Verb,
    targets: ScriptValue,
    vars: ScriptValue,
    from_vars: ScriptValue,
    into: Option<(TweenId, ScriptValue)>,
) -> Parse<TweenId> {
    let vm_id = current_splash_vm_id(vm.cx_mut());
    if !driver_in_place(vm.cx_mut()) {
        return Err(REENTERED.into());
    }
    let uids = parse_targets(vm, targets)?;
    let mut spec = parse_tween_vars(vm, vars)?;
    if spec.path.is_some() && verb != Verb::To {
        return Err("motion_path works with to()".into());
    }
    let mut from = Vec::new();
    if verb == Verb::FromTo {
        let Some(obj) = from_vars.as_object() else {
            return Err("from_to(targets, from_vars, to_vars) needs a from_vars object".into());
        };
        collect_props(vm, obj, &[], &mut from)?;
    }
    let position = match into {
        Some((_, p)) => parse_position(vm, p)?,
        None => Position::END,
    };
    // Validate per verb before anything is built.
    let mut starts = Vec::with_capacity(spec.props.len());
    for p in &spec.props {
        let start = match verb {
            Verb::FromTo => match from.iter().find(|f| f.path() == p.path()) {
                Some(PProp {
                    val: PVal::Abs(a), ..
                }) => Some(*a),
                Some(_) => {
                    return Err(format!(
                        "{}: a from value must be absolute",
                        path_text(p.path())
                    ))
                }
                None => {
                    return Err(format!(
                        "{}: from_to needs a start for every key of to_vars",
                        path_text(p.path())
                    ))
                }
            },
            _ => None,
        };
        if verb == Verb::From && matches!(p.val, PVal::Rel(_)) {
            return Err(format!(
                "{}: from() takes absolute values",
                path_text(p.path())
            ));
        }
        starts.push(start);
    }
    // Which props need the widget's current value: to, from (its end is the
    // current value) and a relative set. An absolute set carries its value
    // as an explicit start too (it renders when its time comes, and never
    // counts as unseeded), a from_to its own start.
    // A motion path reads the widget's x / y (/ z) only to align its start
    // on them (align @start); its rotation never.
    let path_align = spec
        .path
        .as_ref()
        .is_some_and(|p| p.align == PathAlign::Start);
    let needs_current = |p: &PProp| match p.role {
        PRole::Value => match verb {
            Verb::To | Verb::From => true,
            Verb::Set => matches!(p.val, PVal::Rel(_)),
            Verb::FromTo => false,
        },
        PRole::PathX | PRole::PathY | PRole::PathZ => path_align,
        PRole::PathRot => false,
    };
    // The widgets, each built by this VM: a source in another heap (a
    // Splash isolate's widget seen from its host, the host-built Splash seen
    // from its isolate) is not this script's to animate, nor to read.
    let own_heap = vm.bx.heap.heap_key();
    let mut widgets = Vec::with_capacity(uids.len());
    for uid in &uids {
        let w = vm.cx().widget_tree().widget(*uid);
        let key = w.script_source_heap_key();
        if key != 0 && key != own_heap {
            return Err(format!(
                "widget {uid:?} was built by another script VM (a Splash isolate and its host \
                 animate only their own widgets)"
            ));
        }
        widgets.push(w);
    }
    // Known slots: (target, prop) -> the seeded slot's kind.
    let n = spec.props.len();
    let mut known: Vec<Option<ValueKind>> = vec![None; uids.len() * n];
    {
        let d = vm.cx_mut().global::<ScriptTweenDriver>();
        let i = d.vm_ix(vm_id);
        let vt = &mut d.vms[i];
        for (ti, uid) in uids.iter().enumerate() {
            let tid = vt.target_ix(*uid).map(|ix| vt.targets[ix].tid);
            for (pi, p) in spec.props.iter().enumerate() {
                let slot = tid.and_then(|tid| vt.host.slot(tid, p.key()));
                if let Some(s) = slot.filter(|s| vt.host.engine.is_seeded(*s)) {
                    known[ti * n + pi] = Some(vt.host.value(s).kind());
                }
            }
        }
    }
    // Start values read from the widgets' sources, where nothing is known.
    let mut seeds: Vec<Option<TweenValue>> = vec![None; uids.len() * n];
    for (ti, w) in widgets.iter().enumerate() {
        for (pi, p) in spec.props.iter().enumerate() {
            if known[ti * n + pi].is_none() && needs_current(p) {
                seeds[ti * n + pi] = source_value(vm, w, p.path());
            }
        }
    }
    if into.is_none() {
        // GSAP keeps an animation while script holds it: `h.restart()` after
        // it completed works. The orphan finaliser frees it once the handle
        // is collected.
        spec.opts.keep = spec.opts.keep.or(Some(true));
    }
    // The motion path: its geometry goes to the engine inside the build, its
    // y key and options ride on the PathX prop.
    let path = spec.path.as_mut().map(|ps| {
        let mut o = PathOpts::new()
            .span(ps.start, ps.end)
            .offset3(ps.offset[0], ps.offset[1], ps.offset[2])
            .align(ps.align);
        if let Some((deg, radians)) = ps.rotate {
            let rot = prop_path(ps.rot.ids());
            o = if radians {
                o.auto_rotate_radians(rot, deg)
            } else {
                o.auto_rotate(rot, deg)
            };
        }
        if let Some(z) = ps.z {
            o = o.z(prop_path(z.ids()));
        }
        (std::mem::take(&mut ps.geom), prop_path(ps.y.ids()), o)
    });
    let cx = vm.cx_mut();
    let built = with_vm_tweens(cx, vm_id, |cx, vt| {
        // Held by this call until the build is done: the tweens that follow
        // it keep it alive after that, and free it with them.
        let path = path.map(|(geom, y, o)| (vt.host.engine.add_path(geom), y, o));
        let mut tix = Vec::with_capacity(uids.len());
        let mut tids = Vec::with_capacity(uids.len());
        for (ti, uid) in uids.iter().enumerate() {
            let ix = vt.target(*uid, &widgets[ti]);
            tix.push(ix);
            tids.push(vt.targets[ix].tid);
        }
        let mut props = Vec::with_capacity(n);
        for (pi, p) in spec.props.iter().enumerate() {
            let key = p.key();
            // One kind per property: a known slot's, unless the value has
            // more lanes (a colour over a slot that held a number would be
            // cut to grey), else the value's own.
            let own = match (p.val, starts[pi]) {
                (_, Some(a)) => a.kind(),
                (PVal::Abs(v), None) => v.kind(),
                (PVal::Rel(_), None) => ValueKind::F64,
            };
            let kind = match (0..uids.len()).find_map(|ti| known[ti * n + pi]) {
                // A path writes plain numbers: a slot of another kind is
                // converted below.
                _ if p.role != PRole::Value => ValueKind::F64,
                Some(k) if lanes(k) >= lanes(own) => k,
                _ => own,
            };
            for (ti, tid) in tids.iter().enumerate() {
                let t = &mut vt.targets[tix[ti]];
                if !t.keys.iter().any(|k| k.key == key) {
                    t.keys.push(KeyPath {
                        key,
                        path: p.path,
                        len: p.len,
                    });
                    if p.len > 1 {
                        vt.host.bind_path(*tid, key, p.path());
                    }
                }
                match known[ti * n + pi] {
                    Some(k) if k != kind => {
                        // A slot of another kind (a colour animated as a vec4
                        // elsewhere): convert it in place.
                        if let Some(cur) = vt.host.get(*tid, key) {
                            vt.host.seed(*tid, key, coerce(cur, kind));
                        }
                    }
                    Some(_) => {}
                    None => {
                        if let Some(s) = seeds[ti * n + pi] {
                            vt.host.seed(*tid, key, coerce(s, kind));
                        } else if needs_current(p) {
                            let uid = uids[ti];
                            if cfg!(debug_assertions) && !vt.warned.contains(&(uid, key)) {
                                vt.warned.push((uid, key));
                                let hint = if p.role == PRole::Value {
                                    "use from_to"
                                } else {
                                    "motion_path align @start cannot align on it"
                                };
                                log!(
                                    "tween: {} of widget {:?} has nothing to start from (its DSL source does not set it); {}",
                                    path_text(p.path()),
                                    uid,
                                    hint
                                );
                            }
                        }
                    }
                }
            }
            match (p.role, path) {
                (PRole::Value, _) => {}
                // One PropTo::path carries every key of the path.
                (PRole::PathX, Some((pid, y, o))) => {
                    props.push(PropTo::path(key, y, pid, o));
                    continue;
                }
                _ => continue,
            }
            props.push(match (verb, p.val) {
                (Verb::FromTo, PVal::Abs(v)) => {
                    PropTo::from_to(key, coerce(starts[pi].unwrap(), kind), coerce(v, kind))
                }
                (Verb::FromTo, PVal::Rel(dv)) => PropTo {
                    key,
                    from: Some(coerce(starts[pi].unwrap(), kind)),
                    to: End::By(coerce(TweenValue::F64(dv), kind)),
                    snap: 0.0,
                },
                (Verb::From, PVal::Abs(v)) => PropTo::from(key, coerce(v, kind)),
                (Verb::Set, PVal::Abs(v)) => PropTo::from_to(key, coerce(v, kind), coerce(v, kind)),
                (_, PVal::Abs(v)) => PropTo::to(key, coerce(v, kind)),
                (_, PVal::Rel(dv)) => PropTo::by(key, coerce(TweenValue::F64(dv), kind)),
            });
        }
        let t = Targets::List(&tids);
        let o = spec.opts;
        let id = match into {
            None => match verb {
                Verb::To => vt.host.to(cx, t, &props, o),
                Verb::From => vt.host.from(cx, t, &props, o),
                Verb::FromTo => vt.host.from_to(cx, t, &props, o),
                // GSAP set: a zero-duration tween, applied at once unless it
                // has a delay.
                Verb::Set => {
                    let at_once = o.delay.unwrap_or(0.0) <= 0.0;
                    let o = TweenOpts {
                        immediate_render: o.immediate_render.or(Some(at_once)),
                        ..o
                    };
                    vt.host.tween(cx, t, &props, o.duration(0.0).repeat(0))
                }
            },
            Some((tl, _)) => {
                {
                    let mut b = vt.host.tl(tl);
                    match verb {
                        Verb::To => b.to(t, &props, o, position),
                        Verb::From => b.from(t, &props, o, position),
                        Verb::FromTo => b.from_to(t, &props, o, position),
                        Verb::Set => b.set(t, &props, o, position),
                    };
                }
                let child = vt.host.tl(tl).last();
                // Arms the host's frame: the timeline may be playing.
                let _ = vt.host.control(cx, tl);
                child
            }
        };
        if let Some((pid, _, _)) = path {
            vt.host.engine.release_path(pid);
        }
        // The root animation these targets belong to (a timeline's for a
        // child), so it can go with its widgets.
        vt.add_root_targets(into.map_or(id, |(tl, _)| tl), &tids);
        // A stale timeline builds nothing: no closure is kept for it.
        if vt.reachable(id) {
            for (kind, f) in spec.cbs {
                vt.callbacks.push(Callback { id, kind, f });
            }
        }
        id
    });
    built.ok_or_else(|| REENTERED.into())
}

/// The error of a native reached while the driver is out of the global
/// (from inside the driver's own step).
const REENTERED: &str =
    "called from inside the tween driver's own step (a widget apply); nothing was done";

/// The calling VM's handle type, registered on first use (never from
/// `theme_mod`: `new_handle_type` is not idempotent and its slots are few).
fn handle_type(vm: &mut ScriptVm, vm_id: SplashVmId) -> ScriptHandleType {
    {
        let d = vm.cx_mut().global::<ScriptTweenDriver>();
        let i = d.vm_ix(vm_id);
        if let Some(ty) = d.vms[i].handle_type {
            return ty;
        }
    }
    let ty = register_handle_type(vm);
    let d = vm.cx_mut().global::<ScriptTweenDriver>();
    let i = d.vm_ix(vm_id);
    d.vms[i].handle_type = Some(ty);
    ty
}

fn mint(vm: &mut ScriptVm, id: TweenId, timeline: bool) -> ScriptValue {
    let vm_id = current_splash_vm_id(vm.cx_mut());
    let ty = handle_type(vm, vm_id);
    vm.bx
        .heap
        .new_handle(
            ty,
            Box::new(TweenHandleGc {
                vm_id,
                id,
                timeline,
            }),
        )
        .into()
}

// ---------------------------------------------------------------------------
// mod.tween
// ---------------------------------------------------------------------------

/// Registers `mod.tween` once per VM. Called from `theme_mod`, which runs
/// again on every live edit and in every Splash isolate: natives are never
/// freed, so a VM that has the module keeps it.
pub fn script_mod(vm: &mut ScriptVm) {
    if vm.module(id!(tween)) != ScriptObject::ZERO {
        return;
    }
    let tween = vm.new_module(id!(tween));
    for (name, verb) in [
        (id_lut!(to), Verb::To),
        (id_lut!(from), Verb::From),
        (id_lut!(set), Verb::Set),
    ] {
        vm.add_method(
            tween,
            name,
            script_args_def!(targets = NIL, vars = NIL),
            move |vm, args| root_build(vm, args, verb),
        );
    }
    vm.add_method(
        tween,
        id_lut!(from_to),
        script_args_def!(targets = NIL, from_vars = NIL, vars = NIL),
        |vm, args| root_build(vm, args, Verb::FromTo),
    );
    vm.add_method(
        tween,
        id_lut!(timeline),
        script_args_def!(vars = NIL),
        |vm, args| {
            let vars = script_value!(vm, args.vars);
            let (opts, cbs) = match parse_timeline_vars(vm, vars) {
                Ok(v) => v,
                Err(e) => return err(vm, e),
            };
            let vm_id = current_splash_vm_id(vm.cx_mut());
            let id = with_vm_tweens(vm.cx_mut(), vm_id, |cx, vt| {
                let id = vt.host.timeline(opts);
                // GSAP timelines play at once unless paused: arm the frame.
                let _ = vt.host.control(cx, id);
                vt.add_root_targets(id, &[]);
                for (kind, f) in cbs {
                    vt.callbacks.push(Callback { id, kind, f });
                }
                id
            });
            match id {
                Some(id) => mint(vm, id, true),
                None => err(vm, REENTERED.into()),
            }
        },
    );
    vm.add_method(
        tween,
        id_lut!(kill_tweens_of),
        script_args_def!(targets = NIL, props = NIL),
        |vm, args| {
            let targets = script_value!(vm, args.targets);
            let props = script_value!(vm, args.props);
            let (uids, keys) = match parse_targets(vm, targets)
                .and_then(|u| parse_keys(vm, props).map(|k| (u, k)))
            {
                Ok(v) => v,
                Err(e) => return err(vm, e),
            };
            let vm_id = current_splash_vm_id(vm.cx_mut());
            let done = with_vm_tweens(vm.cx_mut(), vm_id, |cx, vt| {
                let tids: Vec<TargetId> = uids
                    .iter()
                    .filter_map(|uid| vt.target_ix(*uid).map(|i| vt.targets[i].tid))
                    .collect();
                if !tids.is_empty() {
                    vt.host.kill_of(cx, Targets::List(&tids), keys.as_deref());
                    vt.release(cx);
                }
            });
            match done {
                Some(()) => NIL,
                None => err(vm, REENTERED.into()),
            }
        },
    );
    vm.add_method(
        tween,
        id_lut!(clear_props),
        script_args_def!(targets = NIL, props = NIL),
        |vm, args| {
            let targets = script_value!(vm, args.targets);
            let props = script_value!(vm, args.props);
            // nil, true or "all": every key the tween layer holds.
            let all = props.is_nil()
                || props.as_bool() == Some(true)
                || string_of(vm, props).is_some_and(|s| s.trim() == "all");
            let (uids, keys) = match parse_targets(vm, targets).and_then(|u| {
                if all {
                    Ok((u, None))
                } else {
                    parse_keys(vm, props).map(|k| (u, k))
                }
            }) {
                Ok(v) => v,
                Err(e) => return err(vm, e),
            };
            let vm_id = current_splash_vm_id(vm.cx_mut());
            let done = with_vm_tweens(vm.cx_mut(), vm_id, |cx, vt| {
                for uid in &uids {
                    let Some(ix) = vt.target_ix(*uid) else {
                        continue;
                    };
                    let t = &mut vt.targets[ix];
                    let cleared: Vec<KeyPath> = t
                        .keys
                        .iter()
                        .filter(|k| keys.as_ref().map_or(true, |ks| ks.contains(&k.key)))
                        .copied()
                        .collect();
                    if cleared.is_empty() {
                        continue;
                    }
                    t.keys.retain(|k| !cleared.iter().any(|c| c.key == k.key));
                    let (tid, weak) = (t.tid, t.weak.clone());
                    let ks: Vec<PropKey> = cleared.iter().map(|k| k.key).collect();
                    vt.host.forget_props(cx, tid, &ks);
                    vt.warned.retain(|(u, k)| !(u == uid && ks.contains(k)));
                    vt.restores.push(Restore {
                        uid: *uid,
                        weak,
                        paths: cleared,
                    });
                }
                vt.release(cx);
            });
            match done {
                Some(()) => NIL,
                None => err(vm, REENTERED.into()),
            }
        },
    );
    vm.add_method(
        tween,
        id_lut!(is_tweening),
        script_args_def!(target = NIL),
        |vm, args| {
            let target = script_value!(vm, args.target);
            let Some(uid) = ui_handle_uid(vm, target) else {
                return err(vm, "is_tweening takes a ui handle".into());
            };
            let vm_id = current_splash_vm_id(vm.cx_mut());
            let d = vm.cx_mut().global::<ScriptTweenDriver>();
            let Some(vt) = d.vms.iter().find(|v| v.vm_id == vm_id) else {
                return false.into();
            };
            vt.target_ix(uid)
                .is_some_and(|i| vt.host.engine.is_tweening(vt.targets[i].tid))
                .into()
        },
    );
    vm.add_method(
        tween,
        id_lut!(ticker),
        script_args_def!(vars = NIL),
        |vm, args| {
            let vars = script_value!(vm, args.vars);
            if let Some(obj) = vars.as_object() {
                let entries = own_entries(vm, obj);
                if !entries.is_empty() {
                    // The ticker is app-wide: a mini-app must not pause, slow
                    // or reduce the motion of its host.
                    if current_splash_vm_id(vm.cx_mut()) != MAIN_SPLASH_VM_ID {
                        return err(
                            vm,
                            "the ticker is app-wide: a Splash isolate may read tween.ticker(), not change it"
                                .into(),
                        );
                    }
                    let (mut scale, mut paused, mut reduced, mut lag) = (None, None, None, None);
                    for (key, v) in entries {
                        let r = match key {
                            live_id!(time_scale) => number(v, key).map(|n| scale = Some(n)),
                            live_id!(paused) => boolean(v, key).map(|b| paused = Some(b)),
                            live_id!(reduced_motion) => {
                                boolean(v, key).map(|b| reduced = Some(b))
                            }
                            live_id!(lag_smoothing) => parse_lag(vm, v).map(|l| lag = Some(l)),
                            _ => Err(format!(
                                "ticker takes time_scale, paused, reduced_motion, lag_smoothing; {key} is not one"
                            )),
                        };
                        if let Err(e) = r {
                            return err(vm, e);
                        }
                    }
                    set_tween_ticker(vm.cx_mut(), |t| {
                        if let Some(s) = scale {
                            t.time_scale = s.max(0.0);
                        }
                        if let Some(p) = paused {
                            t.paused = p;
                        }
                        if let Some(r) = reduced {
                            t.reduced_motion = r;
                        }
                        if let Some(l) = lag {
                            t.lag = l;
                        }
                    });
                }
            } else if !vars.is_nil() {
                return err(
                    vm,
                    "ticker takes {time_scale, paused, reduced_motion, lag_smoothing}".into(),
                );
            }
            let t = tween_ticker(vm.cx_mut());
            let obj = vm.bx.heap.new_object();
            vm.bx
                .heap
                .set_value_def(obj, id!(time_scale).into(), t.time_scale.into());
            vm.bx
                .heap
                .set_value_def(obj, id!(paused).into(), t.paused.into());
            vm.bx
                .heap
                .set_value_def(obj, id!(reduced_motion).into(), t.reduced_motion.into());
            obj.into()
        },
    );
}

/// `lag_smoothing`, in seconds (GSAP takes milliseconds): a number caps
/// every frame delta (0 or less turns smoothing off); `[threshold,
/// adjusted]` replaces a delta above `threshold` with `adjusted`.
fn parse_lag(vm: &ScriptVm, v: ScriptValue) -> Parse<LagSmoothing> {
    if v.as_array().is_some() {
        let (threshold, adjusted) = pair(
            vm,
            v,
            "lag_smoothing takes seconds, or [threshold, adjusted] in seconds",
        )?;
        return Ok(LagSmoothing {
            threshold: threshold.max(0.0),
            adjusted: adjusted.max(0.0),
        });
    }
    let s = number(v, live_id!(lag_smoothing))?;
    Ok(if s > 0.0 {
        LagSmoothing::clamp(s)
    } else {
        LagSmoothing::OFF
    })
}

fn root_build(vm: &mut ScriptVm, args: ScriptObject, verb: Verb) -> ScriptValue {
    let targets = script_value!(vm, args.targets);
    let vars = script_value!(vm, args.vars);
    let from_vars = if verb == Verb::FromTo {
        script_value!(vm, args.from_vars)
    } else {
        NIL
    };
    match build_tween(vm, verb, targets, vars, from_vars, None) {
        Ok(id) => mint(vm, id, false),
        Err(e) => err(vm, e),
    }
}

// ---------------------------------------------------------------------------
// The handle: tl.to(..), tl.play(), tl.progress(), tl.on_complete(..), ...
// ---------------------------------------------------------------------------

/// The handle a method was called on.
#[derive(Clone, Copy)]
struct Recv {
    vm_id: SplashVmId,
    id: TweenId,
    timeline: bool,
}

fn recv(vm: &mut ScriptVm, args: ScriptObject) -> Option<Recv> {
    let h = script_value!(vm, args.self).as_handle()?;
    vm.downcast_handle_gc::<TweenHandleGc>(h).map(|g| Recv {
        vm_id: g.vm_id,
        id: g.id,
        timeline: g.timeline,
    })
}

fn this(vm: &mut ScriptVm, args: ScriptObject) -> ScriptValue {
    script_value!(vm, args.self)
}

fn bad_handle(vm: &mut ScriptVm) -> ScriptValue {
    script_err_type_mismatch!(vm.trap(), "tween: not a tween handle")
}

/// Runs a control on the receiver (arming its host's frame) and returns the
/// handle for chaining.
fn control(
    vm: &mut ScriptVm,
    args: ScriptObject,
    f: impl FnOnce(&mut Cx, &mut TweenHost, TweenId),
) -> ScriptValue {
    let Some(r) = recv(vm, args) else {
        return bad_handle(vm);
    };
    match with_vm_tweens(vm.cx_mut(), r.vm_id, |cx, vt| f(cx, &mut vt.host, r.id)) {
        Some(()) => this(vm, args),
        None => err(vm, REENTERED.into()),
    }
}

/// Reads the receiver (0 / false / nil answers for a stale handle).
fn getter(
    vm: &mut ScriptVm,
    args: ScriptObject,
    f: impl FnOnce(&TweenHost, TweenId) -> ScriptValue,
) -> ScriptValue {
    let Some(r) = recv(vm, args) else {
        return bad_handle(vm);
    };
    let d = vm.cx_mut().global::<ScriptTweenDriver>();
    match d.vms.iter().find(|v| v.vm_id == r.vm_id) {
        Some(vt) => f(&vt.host, r.id),
        // The placeholder of a driver that is out: answer as for a stale
        // handle (a fresh host).
        None => f(&TweenHost::new(), r.id),
    }
}

/// `suppress_events` arguments (a bool, or a number as `boolean` reads
/// it): GSAP's per-method default when nil.
fn emit(v: ScriptValue, suppress_by_default: bool) -> Emit {
    let suppress = if v.is_nil() {
        suppress_by_default
    } else {
        boolean(v, live_id!(suppress_events)).unwrap_or(suppress_by_default)
    };
    if suppress {
        Emit::Suppress
    } else {
        Emit::Fire
    }
}

fn tl_build(vm: &mut ScriptVm, args: ScriptObject, verb: Verb) -> ScriptValue {
    let Some(r) = recv(vm, args) else {
        return bad_handle(vm);
    };
    if !r.timeline {
        return err(
            vm,
            "to/from/from_to/set/add_label/call/add_pause build into a timeline; this is a tween"
                .into(),
        );
    }
    let targets = script_value!(vm, args.targets);
    let position = script_value!(vm, args.position);
    let (vars, from_vars) = if verb == Verb::FromTo {
        (
            script_value!(vm, args.vars),
            script_value!(vm, args.from_vars),
        )
    } else {
        (script_value!(vm, args.vars), NIL)
    };
    match build_tween(vm, verb, targets, vars, from_vars, Some((r.id, position))) {
        Ok(_) => this(vm, args),
        Err(e) => err(vm, e),
    }
}

/// A handle's timeline, or the error to return.
fn timeline_recv(vm: &mut ScriptVm, args: ScriptObject) -> Result<Recv, ScriptValue> {
    let Some(r) = recv(vm, args) else {
        return Err(bad_handle(vm));
    };
    if !r.timeline {
        return Err(err(
            vm,
            "this builds into a timeline; this handle is a tween".into(),
        ));
    }
    Ok(r)
}

/// A callback argument: a function, or nil.
fn opt_fn(vm: &mut ScriptVm, v: ScriptValue, what: &str) -> Result<Option<ScriptFnRef>, String> {
    if v.is_nil() {
        return Ok(None);
    }
    match fn_object(vm, v) {
        Some(obj) => Ok(Some(vm.bx.heap.new_fn_ref(obj))),
        None => Err(format!("{what} takes a function")),
    }
}

/// A fresh tag for one `call()` / `add_pause()` closure, outside the hash
/// range labels use.
fn call_tag() -> Tag {
    Tag(LiveId::unique().0 | (1 << 62))
}

fn set_callback(vm: &mut ScriptVm, args: ScriptObject, kind: CbKind) -> ScriptValue {
    let Some(r) = recv(vm, args) else {
        return bad_handle(vm);
    };
    let v = script_value!(vm, args.callback);
    let f = match opt_fn(vm, v, "an on_* callback") {
        Ok(f) => f,
        Err(e) => return err(vm, e),
    };
    let done = with_vm_tweens(vm.cx_mut(), r.vm_id, |_, vt| {
        vt.callbacks.retain(|c| !(c.id == r.id && c.kind == kind));
        let engine = &mut vt.host.engine;
        let m = engine.anim_ref(r.id).events();
        let m = if f.is_some() {
            m.with(kind.mask())
        } else {
            EventMask(m.0 & !kind.mask().0)
        };
        engine.anim(r.id).set_events(m);
        if let Some(f) = f {
            if engine.anim_ref(r.id).is_alive() {
                vt.callbacks.push(Callback { id: r.id, kind, f });
            }
        }
    });
    match done {
        Some(()) => this(vm, args),
        None => err(vm, REENTERED.into()),
    }
}

/// A number getter/setter pair (`tl.time()`, `tl.time(1.5)`), with the
/// `suppress_events` default GSAP gives that method.
type NumGet = fn(&AnimRef) -> f64;
type NumSet = fn(&mut AnimMut, f64, Emit);

fn add_number_prop(
    vm: &mut ScriptVm,
    ty: ScriptHandleType,
    name: LiveId,
    get: NumGet,
    set: NumSet,
    suppress_by_default: bool,
) {
    vm.add_handle_method(
        ty,
        name,
        script_args_def!(value = NIL, suppress_events = NIL),
        move |vm, args| {
            let v = script_value!(vm, args.value);
            if v.is_nil() {
                return getter(vm, args, |h, id| get(&h.engine.anim_ref(id)).into());
            }
            let x = match number(v, name) {
                Ok(x) => x,
                Err(e) => return err(vm, e),
            };
            let ev = emit(script_value!(vm, args.suppress_events), suppress_by_default);
            control(vm, args, |cx, host, id| {
                set(&mut host.control(cx, id), x, ev)
            })
        },
    );
}

/// A flag getter/setter pair (`tl.paused()`, `tl.paused(true)`).
fn add_flag_prop(
    vm: &mut ScriptVm,
    ty: ScriptHandleType,
    name: LiveId,
    get: fn(&AnimRef) -> bool,
    set: fn(&mut AnimMut, bool),
) {
    vm.add_handle_method(ty, name, script_args_def!(value = NIL), move |vm, args| {
        let v = script_value!(vm, args.value);
        if v.is_nil() {
            return getter(vm, args, |h, id| get(&h.engine.anim_ref(id)).into());
        }
        let b = match boolean(v, name) {
            Ok(b) => b,
            Err(e) => return err(vm, e),
        };
        control(vm, args, |cx, host, id| set(&mut host.control(cx, id), b))
    });
}

/// An optional seek argument (`play(from)`, `pause(at)`, `reverse(from)`).
fn opt_seek(vm: &mut ScriptVm, v: ScriptValue) -> Result<Option<Seek>, ScriptValue> {
    if v.is_nil() {
        return Ok(None);
    }
    parse_seek(vm, v).map(Some).map_err(|e| err(vm, e))
}

fn register_handle_type(vm: &mut ScriptVm) -> ScriptHandleType {
    let ty = vm.new_handle_type(id_lut!(tween_anim));

    // Building (timelines): to / from / set take (targets, vars, position),
    // from_to (targets, from_vars, vars, position).
    for (name, verb) in [
        (id_lut!(to), Verb::To),
        (id_lut!(from), Verb::From),
        (id_lut!(set), Verb::Set),
    ] {
        vm.add_handle_method(
            ty,
            name,
            script_args_def!(targets = NIL, vars = NIL, position = NIL),
            move |vm, args| tl_build(vm, args, verb),
        );
    }
    vm.add_handle_method(
        ty,
        id_lut!(from_to),
        script_args_def!(targets = NIL, from_vars = NIL, vars = NIL, position = NIL),
        |vm, args| tl_build(vm, args, Verb::FromTo),
    );
    vm.add_handle_method(
        ty,
        id_lut!(add_label),
        script_args_def!(label = NIL, position = NIL),
        |vm, args| {
            let r = match timeline_recv(vm, args) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let label = script_value!(vm, args.label);
            let position = script_value!(vm, args.position);
            let (tag, pos) = match parse_tag(vm, label)
                .and_then(|t| parse_position(vm, position).map(|p| (t, p)))
            {
                Ok(v) => v,
                Err(e) => return err(vm, e),
            };
            match with_vm_tweens(vm.cx_mut(), r.vm_id, |_, vt| {
                vt.host.tl(r.id).add_label(tag, pos);
            }) {
                Some(()) => this(vm, args),
                None => err(vm, REENTERED.into()),
            }
        },
    );
    // call(fn, position) and add_pause(position, fn?): the closure is found
    // by a minted tag when the playhead crosses the marker.
    vm.add_handle_method(
        ty,
        id_lut!(call),
        script_args_def!(callback = NIL, position = NIL),
        |vm, args| {
            let callback = script_value!(vm, args.callback);
            let position = script_value!(vm, args.position);
            add_marker(vm, args, callback, position, false)
        },
    );
    vm.add_handle_method(
        ty,
        id_lut!(add_pause),
        script_args_def!(position = NIL, callback = NIL),
        |vm, args| {
            let callback = script_value!(vm, args.callback);
            let position = script_value!(vm, args.position);
            add_marker(vm, args, callback, position, true)
        },
    );

    // Playback
    vm.add_handle_method(
        ty,
        id_lut!(play),
        script_args_def!(from = NIL),
        |vm, args| {
            let from = script_value!(vm, args.from);
            let at = match opt_seek(vm, from) {
                Ok(at) => at,
                Err(e) => return e,
            };
            control(vm, args, |cx, host, id| match at {
                Some(s) => {
                    host.control(cx, id).play_from(s, Emit::Suppress);
                }
                None => host.play(cx, id),
            })
        },
    );
    vm.add_handle_method(
        ty,
        id_lut!(pause),
        script_args_def!(at = NIL),
        |vm, args| {
            let at = script_value!(vm, args.at);
            let at = match opt_seek(vm, at) {
                Ok(at) => at,
                Err(e) => return e,
            };
            control(vm, args, |cx, host, id| {
                let mut c = host.control(cx, id);
                match at {
                    Some(s) => c.pause_at(s, Emit::Suppress),
                    None => c.pause(),
                };
            })
        },
    );
    vm.add_handle_method(ty, id_lut!(resume), script_args_def!(), |vm, args| {
        control(vm, args, |cx, host, id| {
            host.control(cx, id).resume();
        })
    });
    vm.add_handle_method(
        ty,
        id_lut!(reverse),
        script_args_def!(from = NIL),
        |vm, args| {
            let from = script_value!(vm, args.from);
            let at = match opt_seek(vm, from) {
                Ok(at) => at,
                Err(e) => return e,
            };
            control(vm, args, |cx, host, id| {
                let mut c = host.control(cx, id);
                match at {
                    Some(s) => c.reverse_from(Some(s), Emit::Suppress),
                    None => c.reverse(),
                };
            })
        },
    );
    vm.add_handle_method(
        ty,
        id_lut!(restart),
        script_args_def!(include_delay = NIL, suppress_events = NIL),
        |vm, args| {
            let include_delay = script_value!(vm, args.include_delay)
                .as_bool()
                .unwrap_or(false);
            let ev = emit(script_value!(vm, args.suppress_events), true);
            control(vm, args, |cx, host, id| {
                host.control(cx, id).restart(include_delay, ev);
            })
        },
    );
    vm.add_handle_method(
        ty,
        id_lut!(seek),
        script_args_def!(position = NIL, suppress_events = NIL),
        |vm, args| {
            let position = script_value!(vm, args.position);
            let s = match parse_seek(vm, position) {
                Ok(s) => s,
                Err(e) => return err(vm, e),
            };
            let ev = emit(script_value!(vm, args.suppress_events), true);
            control(vm, args, |cx, host, id| {
                host.control(cx, id).seek(s, ev);
            })
        },
    );
    vm.add_handle_method(ty, id_lut!(kill), script_args_def!(), |vm, args| {
        let Some(r) = recv(vm, args) else {
            return bad_handle(vm);
        };
        // Kill, then release what the animation roots at once (an
        // Interrupt it queued keeps its closure until it is drained).
        match with_vm_tweens(vm.cx_mut(), r.vm_id, |cx, vt| {
            vt.host.kill(cx, r.id);
            vt.release(cx);
        }) {
            Some(()) => this(vm, args),
            None => err(vm, REENTERED.into()),
        }
    });
    vm.add_handle_method(ty, id_lut!(is_alive), script_args_def!(), |vm, args| {
        let Some(r) = recv(vm, args) else {
            return bad_handle(vm);
        };
        // After forgetting the widgets that are gone: an animation whose
        // widgets all went is gone too.
        with_vm_tweens(vm.cx_mut(), r.vm_id, |_, vt| {
            vt.host.engine.anim_ref(r.id).is_alive()
        })
        .unwrap_or(false)
        .into()
    });

    // Getters and setters (GSAP: no argument reads, one argument writes).
    // time / total_time / progress / total_progress fire the crossed
    // callbacks unless told otherwise, as in GSAP.
    let numbers: [(LiveId, NumGet, NumSet); 4] = [
        (
            id_lut!(time),
            |a| a.time(),
            |c, x, ev| {
                c.set_time(x, ev);
            },
        ),
        (
            id_lut!(total_time),
            |a| a.total_time(),
            |c, x, ev| {
                c.set_total_time(x, ev);
            },
        ),
        (
            id_lut!(progress),
            |a| a.progress(),
            |c, x, ev| {
                c.set_progress(x, ev);
            },
        ),
        (
            id_lut!(total_progress),
            |a| a.total_progress(),
            |c, x, ev| {
                c.set_total_progress(x, ev);
            },
        ),
    ];
    for (name, get, set) in numbers {
        add_number_prop(vm, ty, name, get, set, false);
    }
    add_number_prop(
        vm,
        ty,
        id_lut!(time_scale),
        |a| a.time_scale(),
        |c, s, _| {
            c.set_time_scale(s);
        },
        true,
    );
    add_number_prop(
        vm,
        ty,
        id_lut!(duration),
        |a| a.duration(),
        |c, d, _| {
            c.set_duration(d.max(0.0));
        },
        true,
    );
    add_flag_prop(
        vm,
        ty,
        id_lut!(paused),
        |a| a.paused(),
        |c, p| {
            c.set_paused(p);
        },
    );
    add_flag_prop(
        vm,
        ty,
        id_lut!(reversed),
        |a| a.reversed(),
        |c, r| {
            c.set_reversed(r);
        },
    );
    let reads: [(LiveId, fn(&AnimRef) -> ScriptValue); 4] = [
        (id_lut!(total_duration), |a| a.total_duration().into()),
        (id_lut!(iteration), |a| (a.iteration() as f64).into()),
        (id_lut!(is_active), |a| a.is_active().into()),
        (id_lut!(current_label), |a| match a.current_label() {
            Some(t) => LiveId(t.0).escape(),
            None => NIL,
        }),
    ];
    for (name, read) in reads {
        vm.add_handle_method(ty, name, script_args_def!(), move |vm, args| {
            getter(vm, args, |h, id| read(&h.engine.anim_ref(id)))
        });
    }

    // Callbacks: on_*(fn) sets one, on_*(nil) removes it.
    for (name, kind) in [
        (id_lut!(on_start), CbKind::Start),
        (id_lut!(on_update), CbKind::Update),
        (id_lut!(on_repeat), CbKind::Repeat),
        (id_lut!(on_complete), CbKind::Complete),
        (id_lut!(on_reverse_complete), CbKind::ReverseComplete),
        (id_lut!(on_interrupt), CbKind::Interrupt),
    ] {
        vm.add_handle_method(
            ty,
            name,
            script_args_def!(callback = NIL),
            move |vm, args| set_callback(vm, args, kind),
        );
    }
    ty
}

/// `tl.call(fn, position)` (a function is required) and
/// `tl.add_pause(position, fn?)`.
fn add_marker(
    vm: &mut ScriptVm,
    args: ScriptObject,
    callback: ScriptValue,
    position: ScriptValue,
    pause: bool,
) -> ScriptValue {
    let r = match timeline_recv(vm, args) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let what = if pause { "add_pause()" } else { "call()" };
    let f = match opt_fn(vm, callback, what) {
        Ok(None) if !pause => return err(vm, "call() takes a function".into()),
        Ok(f) => f,
        Err(e) => return err(vm, e),
    };
    let pos = match parse_position(vm, position) {
        Ok(p) => p,
        Err(e) => return err(vm, e),
    };
    let done = with_vm_tweens(vm.cx_mut(), r.vm_id, |cx, vt| {
        let tag = if f.is_some() { call_tag() } else { Tag::NONE };
        if pause {
            vt.host.tl(r.id).add_pause(pos, tag);
        } else {
            vt.host.tl(r.id).call(tag, pos);
        }
        // A stale timeline adds no marker: keep no closure for it.
        if let Some(f) = f.filter(|_| vt.host.engine.anim_ref(r.id).is_alive()) {
            vt.calls.push(CallFn {
                owner: r.id,
                tag,
                f,
            });
        }
        let _ = vt.host.control(cx, r.id);
    });
    match done {
        Some(()) => this(vm, args),
        None => err(vm, REENTERED.into()),
    }
}
