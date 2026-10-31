//! Rendering: iteration maths, the tween / zero-duration / group / timeline
//! renders, the child walk and the track writes. This is the only code that
//! runs every frame, and it never allocates.
//!
//! The rules are GSAP 3.15's `Tween.render`, `_renderZeroDurationTween` and
//! `Timeline.render` (design sections 5.3 to 5.11), with the reconciled
//! deviations noted where they apply.

use crate::easing::{Easing, YoyoEase};
use crate::engine::*;
use crate::event::EventKind;
use crate::path::PathPoint;
use crate::spec::{EventMask, PathAlign};
use crate::value::{decode, encode_pair, lerp_lanes, ColorSpace, Rgba, TweenValue, ValueKind};
use crate::{animation_cycle, round7, TINY};

/// One child walk: the parent's new and previous local times, its incoming
/// total time (negative before its start), the render flags, a child to skip
/// (the pause being stopped on) and the lending stagger group (see
/// [`TweenEngine::render`]).
#[derive(Clone, Copy)]
pub(crate) struct Walk {
    pub time: f64,
    pub prev_time: f64,
    pub total: f64,
    pub suppress: bool,
    pub force: bool,
    pub skip: u32,
    pub yo: u32,
}

impl TweenEngine {
    /// Renders node `n` at its own total time `total` (GSAP `render()`).
    /// `yo` is the stagger group whose odd yoyo iteration lends its yoyo ease
    /// to this node (NIL when none).
    pub(crate) fn render(&mut self, n: u32, total: f64, suppress: bool, force: bool, yo: u32) {
        if self.kind(n) == K_TIMELINE {
            self.render_timeline(n, total, suppress, force);
        } else if self.hot[n as usize].dur == 0.0 {
            self.render_zero(n, total, suppress, force);
        } else {
            self.render_tween(n, total, suppress, force, yo);
        }
    }

    /// GSAP `_parentToChildTotalTime`: the child's total time when its parent
    /// is at local time `t` (a reversed child runs from its total duration down).
    #[inline]
    pub(crate) fn child_time(&mut self, t: f64, c: u32) -> f64 {
        let h = self.hot[c as usize];
        if h.ts > 0.0 {
            (t - h.start) * h.ts
        } else {
            let tdur = if h.flags & F_DIRTY != 0 {
                self.total_duration(c)
            } else {
                h.tdur
            };
            tdur + (t - h.start) * h.ts
        }
    }

    /// [`Self::child_time`] without the duration recompute.
    pub(crate) fn parent_to_child(&mut self, t: f64, c: u32) -> f64 {
        let h = self.hot[c as usize];
        let tdur = if h.ts >= 0.0 {
            0.0
        } else {
            self.total_duration(c)
        };
        (t - h.start) * h.ts + tdur
    }

    /// The same without mutating (getters): a dirty timeline's total
    /// duration is derived on the fly.
    pub(crate) fn parent_to_child_pure(&self, t: f64, c: u32) -> f64 {
        let h = &self.hot[c as usize];
        let tdur = if h.ts >= 0.0 {
            0.0
        } else {
            self.pure_total_duration(c)
        };
        (t - h.start) * h.ts + tdur
    }

    /// GSAP `rawTime()`: the node's total time as its parent's playhead
    /// implies it (its own total time when paused or parentless).
    pub(crate) fn raw_time(&mut self, n: u32) -> f64 {
        let c = self.cold[n as usize];
        let p = if c.parent != NIL { c.parent } else { c.dp };
        let h = self.hot[n as usize];
        if p == NIL || h.ts == 0.0 {
            return h.ttime;
        }
        let pr = self.raw_time(p);
        self.parent_to_child(pr, n)
    }

    /// [`Self::raw_time`] without mutating.
    pub(crate) fn raw_time_pure(&self, n: u32) -> f64 {
        let c = &self.cold[n as usize];
        let p = if c.parent != NIL { c.parent } else { c.dp };
        let h = &self.hot[n as usize];
        if p == NIL || h.ts == 0.0 {
            return h.ttime;
        }
        self.parent_to_child_pure(self.raw_time_pure(p), n)
    }

    /// GSAP `globalTime(local)`: folds a local time up through every parent.
    pub(crate) fn global_time(&self, n: u32, local: f64) -> f64 {
        let mut t = local;
        let mut x = n;
        while x != NIL {
            let h = &self.hot[x as usize];
            let s = if h.ts == 0.0 { 1.0 } else { h.ts.abs() };
            t = h.start + t / s;
            x = self.cold[x as usize].dp;
        }
        t
    }

    /// Iteration maths (5.4): the 0-based iteration, the (yoyo-mirrored)
    /// iteration time and whether this is an odd yoyo pass, for a total time
    /// already clamped to `[0, tdur]`.
    #[inline]
    pub(crate) fn iterate(&self, n: u32, tt: f64) -> (u32, f64, bool) {
        let c = &self.cold[n as usize];
        let h = &self.hot[n as usize];
        if c.repeat == 0 {
            return (0, tt, false);
        }
        let dur = h.dur;
        let cycle = dur + c.rdelay;
        let quot = tt / cycle;
        // `tt % cycle` is exact (JavaScript's `%`); the first iteration
        // needs no fmod.
        let rem = if tt < cycle { tt } else { tt % cycle };
        let mut time = round7(rem);
        let it;
        // The yoyo parity comes from the exact (f64) iteration: the u32
        // count saturates on a repeat-forever animation sought very far.
        let odd;
        if tt == h.tdur {
            it = if c.repeat > 0 {
                c.repeat as u32
            } else {
                animation_cycle(tt, cycle)
            };
            odd = it & 1 == 1;
            time = dur;
        } else {
            let q = round7(quot);
            let mut w = crate::floor_small(q);
            if w != 0.0 && w == q {
                time = dur;
                w -= 1.0;
            } else if time > dur {
                time = dur;
            }
            it = w as u32;
            // Exact: w <= INFINITE / 1e-7 is far below 2^63.
            odd = (w as u64) & 1 == 1;
        }
        let yodd = h.flags & F_YOYO != 0 && odd;
        if yodd {
            time = dur - time;
        }
        (it, time, yodd)
    }

    /// The eased ratio at linear progress `p` in (0, 1): the node's ease, or
    /// on an odd yoyo pass `1 - Y(1 - p)` with the yoyo ease `Y` (its own, or
    /// the one its stagger group lends it through `yo`).
    #[inline]
    pub(crate) fn ratio(&self, n: u32, p: f64, yodd: bool, yo: u32) -> f64 {
        let c = &self.cold[n as usize];
        let src = |y: YoyoEase| -> Easing {
            match y {
                YoyoEase::Invert => c.ease,
                YoyoEase::Ease(e) => e,
            }
        };
        if yodd {
            if let Some(y) = c.yoyo {
                return 1.0 - src(y).map(1.0 - p);
            }
        }
        if yo != NIL {
            if let Some(y) = self.cold[yo as usize].yoyo {
                return 1.0 - src(y).map(1.0 - p);
            }
        }
        c.ease.map(p)
    }

    /// GSAP `Tween.render` for a non-zero duration (5.8).
    pub(crate) fn render_tween(
        &mut self,
        n: u32,
        total: f64,
        suppress: bool,
        force_in: bool,
        yo: u32,
    ) {
        let mut force = force_in;
        let h = self.hot[n as usize];
        let prev_time = h.time;
        let tdur = h.tdur;
        let dur = h.dur;
        let neg = total < 0.0;
        let tt = if total > tdur - TINY && !neg {
            tdur
        } else if total < TINY {
            0.0
        } else {
            total
        };
        let initted = h.flags & F_INITTED != 0;
        // GSAP's "same time" test: a render exactly at 0 always goes
        // through (it re-writes the start values and reports Update).
        if !(tt != h.ttime || total == 0.0 || force || (!initted && h.ttime != 0.0)) {
            return;
        }
        let (it, time, yodd) = self.iterate(n, tt);
        let repeat = self.cold[n as usize].repeat;
        let mut prev_it = 0;
        if repeat != 0 {
            let cycle = dur + self.cold[n as usize].rdelay;
            // A tween's `iter` is kept equal to animation_cycle(ttime).
            prev_it = h.iter;
            debug_assert_eq!(prev_it, animation_cycle(h.ttime, cycle));
            if time == prev_time && !force && initted && it == prev_it {
                // Inside the repeat delay: nothing moves.
                self.hot[n as usize].ttime = tt;
                return;
            }
            if it != prev_it
                && h.flags & F_REFRESH != 0
                && !yodd
                && self.lock(n) == 0
                && time != cycle
                && initted
            {
                // repeatRefresh: land the ended iteration exactly, then re-capture.
                self.set_lock(n, 1);
                self.render_tween(n, round7(cycle * it as f64), true, true, yo);
                self.invalidate_node(n);
                self.set_lock(n, 0);
                force = true;
            }
        }
        let flags = self.hot[n as usize].flags;
        // A pre-rendered from()/fromTo() has captured its values already;
        // its initialisation (overwrite Auto) waits for a render past 0.
        if flags & F_INITTED == 0 && !(flags & F_PRE != 0 && tt <= 0.0) {
            let local = if neg { total } else { time };
            self.init(n, local);
            if flags & F_KEYFRAMES != 0 && local <= 0.0 {
                // GSAP renders a keyframes timeline to its end at init, so
                // each step captures the previous step's end value.
                let end = self.cold[n as usize].inner_dur;
                self.render_group_inner(n, end, end, true, true, NIL);
            }
        }
        let p = time / dur;
        let e = if p >= 1.0 {
            1.0
        } else if p <= 0.0 {
            0.0
        } else {
            self.ratio(n, p, yodd, yo)
        };
        {
            let hm = &mut self.hot[n as usize];
            hm.ttime = tt;
            hm.time = time;
            hm.iter = it;
            if tt > 0.0 && tt < tdur {
                hm.flags |= F_ACT;
            } else {
                hm.flags &= !F_ACT;
            }
        }
        // Most tweens report nothing: one mask test skips every emit.
        let reports = !suppress && self.cold[n as usize].events.0 != 0;
        if reports && prev_time == 0.0 && tt != 0.0 && prev_it == 0 {
            self.emit(n, EventKind::Start);
        }
        self.write_tracks(n, p, e);
        if self.kind(n) == K_GROUP {
            let c = self.cold[n as usize];
            let inner = if p >= 1.0 {
                c.inner_dur
            } else if p <= 0.0 {
                0.0
            } else {
                c.inner_dur * c.remap.map(p)
            };
            let child_yo = if yodd && c.yoyo.is_some() { n } else { yo };
            self.render_group_inner(n, total, inner, suppress, force, child_yo);
        }
        if reports {
            self.emit(n, EventKind::Update);
            if repeat != 0 && it != prev_it && self.cold[n as usize].parent != NIL {
                self.emit(n, EventKind::Repeat);
            }
        }
        if (tt == tdur || tt == 0.0) && self.hot[n as usize].ttime == tt {
            let ts = self.hot[n as usize].ts;
            if (total != 0.0 || dur == 0.0) && ((tt == tdur && ts > 0.0) || (tt == 0.0 && ts < 0.0))
            {
                self.remove_from_parent(n, true);
            }
            if reports && !(neg && prev_time == 0.0) && (tt != 0.0 || prev_time != 0.0 || yodd) {
                let kind = if tt == tdur {
                    EventKind::Complete
                } else {
                    EventKind::ReverseComplete
                };
                self.emit(n, kind);
            }
        }
    }

    /// A group's inner timeline (GSAP's nested `tween.timeline.render`): the
    /// children walk at inner time `inner`, or at the negative total when
    /// the group is rendered before its start.
    fn render_group_inner(
        &mut self,
        g: u32,
        total_outer: f64,
        inner: f64,
        suppress: bool,
        force: bool,
        yo: u32,
    ) {
        let total = if total_outer < 0.0 {
            total_outer
        } else {
            inner
        };
        let c = self.cold[g as usize];
        let mut tt = if total <= 0.0 { 0.0 } else { round7(total) };
        if tt > c.inner_dur && total >= 0.0 {
            tt = c.inner_dur;
        }
        let initted = self.has(g, F_INNER_INIT);
        let crossing = (c.iztime < 0.0) != (total < 0.0) && (initted || c.inner_dur == 0.0);
        if !(tt != c.itime || force || crossing) {
            return;
        }
        let mut prev = c.itime;
        if crossing {
            if c.inner_dur == 0.0 {
                prev = c.iztime;
            }
            if total != 0.0 || !suppress {
                self.cold[g as usize].iztime = total;
            }
        }
        self.cold[g as usize].itime = tt;
        if !initted {
            self.set_flag(g, F_INNER_INIT, true);
            self.cold[g as usize].iztime = total;
            prev = 0.0;
        }
        let w = Walk {
            time: tt,
            prev_time: prev,
            total,
            suppress,
            force,
            skip: NIL,
            yo,
        };
        self.walk(g, w);
    }

    /// GSAP `_parentPlayheadIsBeforeStart`: some linked, unpaused, initted,
    /// unlocked ancestor's real playhead ([`Self::raw_time_pure`]) is before
    /// its own start.
    fn parent_playhead_before_start(&self, n: u32) -> bool {
        let mut p = self.cold[n as usize].parent;
        while p != NIL {
            let h = &self.hot[p as usize];
            if h.ts == 0.0 || h.flags & F_INITTED == 0 || h.flags & F_LOCK != 0 {
                return false;
            }
            if self.raw_time_pure(p) < 0.0 {
                return true;
            }
            p = self.cold[p as usize].parent;
        }
        false
    }

    /// GSAP `_renderZeroDurationTween` (5.9, reconciled): sets, calls, pauses
    /// and zero-duration tweens. At exactly 0 the ratio is 0 when the node or
    /// its parent is reversed, or when the node sits at its parent's start
    /// while an ancestor's real playhead is before that ancestor's start (a
    /// wrap sweep renders a nested timeline at exactly 0 although the outer
    /// playhead is before it); `ztime` fires on arriving at or leaving the
    /// exact spot, never both.
    pub(crate) fn render_zero(&mut self, n: u32, total: f64, suppress_in: bool, force: bool) {
        let h = self.hot[n as usize];
        let dp = self.cold[n as usize].dp;
        let reversed = h.ts < 0.0 || (dp != NIL && self.hot[dp as usize].ts < 0.0);
        let ratio1 = !(total < 0.0
            || (total == 0.0
                && (reversed || (h.start == 0.0 && self.parent_playhead_before_start(n)))));
        let prev1 = h.flags & F_RATIO1 != 0;
        let zt = self.cold[n as usize].ztime;
        if !(ratio1 != prev1 || force || zt == TINY || (total == 0.0 && zt != 0.0)) {
            if zt == 0.0 {
                self.cold[n as usize].ztime = total;
            }
            return;
        }
        if h.flags & F_INITTED == 0 {
            self.init(n, total);
        }
        self.cold[n as usize].ztime = if total != 0.0 {
            total
        } else if suppress_in {
            TINY
        } else {
            0.0
        };
        let suppress = suppress_in || (total != 0.0 && zt == 0.0);
        {
            let hm = &mut self.hot[n as usize];
            if ratio1 {
                hm.flags |= F_RATIO1;
            } else {
                hm.flags &= !F_RATIO1;
            }
            hm.time = 0.0;
            hm.ttime = 0.0;
            hm.iter = 0;
        }
        let r = if ratio1 { 1.0 } else { 0.0 };
        self.write_tracks(n, r, r);
        let marker = h.flags & (F_CALL | F_PAUSE_NODE);
        if !suppress && marker == 0 {
            self.emit(n, EventKind::Update);
        }
        if ratio1 {
            self.remove_from_parent(n, true);
        }
        if !suppress {
            if marker & F_CALL != 0 {
                self.emit(n, EventKind::Call { forward: ratio1 });
            } else if marker == 0 {
                let kind = if ratio1 {
                    EventKind::Complete
                } else {
                    EventKind::ReverseComplete
                };
                self.emit(n, kind);
            }
        }
    }

    /// GSAP `Timeline.render` (5.10, reconciled).
    pub(crate) fn render_timeline(&mut self, n: u32, total_in: f64, suppress: bool, force: bool) {
        let mut total = total_in;
        // Read before the recompute: a negative-start shift moves the playhead.
        let mut prev_time = self.hot[n as usize].time;
        let mut tdur = self.total_duration(n);
        let mut dur = self.hot[n as usize].dur;
        let root = self.has(n, F_ROOT);
        let mut tt = if total <= 0.0 { 0.0 } else { round7(total) };
        let crossing = (self.cold[n as usize].ztime < 0.0) != (total < 0.0)
            && (self.has(n, F_INITTED) || dur == 0.0);
        if !root && tt > tdur && total >= 0.0 {
            tt = tdur;
        }
        if !(tt != self.hot[n as usize].ttime || force || crossing) {
            return;
        }
        let shifted = self.hot[n as usize].time;
        if shifted != prev_time && dur != 0.0 {
            let d = shifted - prev_time;
            tt += d;
            total += d;
        }
        let mut time = tt;
        let prev_start = self.hot[n as usize].start;
        let time_scale = self.hot[n as usize].ts;
        let prev_paused = time_scale == 0.0;
        if crossing {
            if dur == 0.0 {
                prev_time = self.cold[n as usize].ztime;
            }
            if total != 0.0 || !suppress {
                self.cold[n as usize].ztime = total;
            }
        }
        let mut it = 0u32;
        let mut prev_it = 0u32;
        let mut to_completion = false;
        let repeat = self.cold[n as usize].repeat;
        if repeat != 0 {
            let yoyo = self.has(n, F_YOYO);
            let cycle = dur + self.cold[n as usize].rdelay;
            let (i, t, yodd) = self.iterate(n, tt);
            it = i;
            time = t;
            let ttime = self.hot[n as usize].ttime;
            let mut pit = animation_cycle(ttime, cycle);
            if prev_time == 0.0
                && ttime != 0.0
                && pit != it
                && ttime - pit as f64 * cycle - dur <= 0.0
            {
                pit = it;
            }
            prev_it = pit;
            if it != pit && self.lock(n) == 0 {
                // Iteration crossing: sweep to the end (or start) of the
                // iteration being left, report Repeat, then wrap.
                let mut rewinding = yoyo && pit & 1 == 1;
                let wraps = rewinding == (yoyo && it & 1 == 1);
                if it < pit {
                    rewinding = !rewinding;
                }
                let m = tt % dur;
                prev_time = if rewinding {
                    0.0
                } else if m != 0.0 && !m.is_nan() {
                    dur
                } else {
                    tt
                };
                self.set_lock(n, 1);
                let target = if prev_time != 0.0 {
                    prev_time
                } else if yodd {
                    0.0
                } else {
                    round7(it as f64 * cycle)
                };
                self.render_timeline(n, target, suppress, dur == 0.0);
                self.set_lock(n, 0);
                self.hot[n as usize].ttime = tt;
                if !suppress && self.cold[n as usize].parent != NIL {
                    self.emit(n, EventKind::Repeat);
                }
                if self.has(n, F_REFRESH) && !yodd {
                    self.invalidate_node(n);
                    self.set_lock(n, 1);
                    prev_it = it;
                }
                if prev_time != 0.0 && prev_time != self.hot[n as usize].time {
                    // The sweep's target was tt itself: it already rendered the
                    // final state. GSAP returns here and so never fires
                    // Complete on such a jump (bug); the engine still runs the
                    // completion step.
                    prev_time = self.hot[n as usize].time;
                    self.set_lock(n, 0);
                    to_completion = true;
                } else if prev_paused != (self.hot[n as usize].ts == 0.0) {
                    self.set_lock(n, 0);
                    return;
                } else {
                    dur = self.hot[n as usize].dur;
                    tdur = self.hot[n as usize].tdur;
                    if wraps {
                        self.set_lock(n, 2);
                        prev_time = if rewinding { dur } else { -0.0001 };
                        self.render_timeline(n, prev_time, true, false);
                        if self.has(n, F_REFRESH) && !yodd {
                            self.invalidate_node(n);
                        }
                    }
                    self.set_lock(n, 0);
                    if self.hot[n as usize].ts == 0.0 && !prev_paused {
                        return;
                    }
                }
            }
        }
        if !to_completion {
            let mut pause = NIL;
            if self.has(n, F_HAS_PAUSE) && !self.has(n, F_FORCING) && self.lock(n) < 2 {
                pause = self.find_next_pause(n, round7(prev_time), round7(time));
                if pause != NIL {
                    let ps = self.hot[pause as usize].start;
                    tt -= time - ps;
                    time = ps;
                }
            }
            {
                // GSAP `_act = !!timeScale`: a rendered, unpaused timeline
                // stays active (every walk of its parent renders it, even
                // before its start) until it completes or is removed.
                let hm = &mut self.hot[n as usize];
                hm.ttime = tt;
                hm.time = time;
                hm.iter = it;
                if time_scale != 0.0 {
                    hm.flags |= F_ACT;
                } else {
                    hm.flags &= !F_ACT;
                }
            }
            if !self.has(n, F_INITTED) {
                self.set_flag(n, F_INITTED, true);
                self.cold[n as usize].ztime = total;
                prev_time = 0.0; // the first render always walks forward
            }
            if prev_time == 0.0 && tt != 0.0 && dur != 0.0 && !suppress && prev_it == 0 {
                self.emit(n, EventKind::Start);
            }
            if !suppress && self.cold[n as usize].events.has(EventMask::LABELS) {
                self.emit_labels(n, prev_time, time);
            }
            let w = Walk {
                time,
                prev_time,
                total,
                suppress,
                force,
                skip: pause,
                yo: NIL,
            };
            self.walk(n, w);
            if pause != NIL && !suppress {
                // add_pause: the playhead stops exactly on it.
                let forward = time >= prev_time;
                self.set_paused(n, true);
                self.render_zero(pause, if forward { 0.0 } else { -TINY }, true, false);
                self.cold[pause as usize].ztime = if forward { 1.0 } else { -1.0 };
                self.emit(pause, EventKind::Pause);
            }
            if !suppress {
                self.emit(n, EventKind::Update);
            }
        }
        // Completion.
        let h = self.hot[n as usize];
        let full = self.total_duration(n);
        if ((tt == tdur && h.ttime >= full) || (tt == 0.0 && prev_time != 0.0))
            && (prev_start == h.start || time_scale.abs() != h.ts.abs())
            && self.lock(n) == 0
            && !root
        {
            if (total != 0.0 || dur == 0.0)
                && ((tt == tdur && h.ts > 0.0) || (tt == 0.0 && h.ts < 0.0))
            {
                self.remove_from_parent(n, true);
            }
            if !(suppress || (total < 0.0 && prev_time == 0.0))
                && (tt != 0.0 || prev_time != 0.0 || tdur == 0.0)
            {
                let kind = if tt == tdur && total >= 0.0 {
                    EventKind::Complete
                } else {
                    EventKind::ReverseComplete
                };
                self.emit(n, kind);
            }
        }
    }

    /// The child walk shared by timelines and groups (5.11): forward in start
    /// order (stopping at the first child that has not started and is not in
    /// flight), backward from the last child.
    pub(crate) fn walk(&mut self, n: u32, w: Walk) {
        let Walk {
            time,
            prev_time,
            total,
            suppress,
            force,
            skip,
            yo,
        } = w;
        // Whether a rendered child is left active with its start after the
        // playhead (a nested timeline stays active once rendered, GSAP
        // `_act = !!timeScale`): the next forward walk must then visit it.
        let mut ahead = false;
        if time >= prev_time && total >= 0.0 {
            // GSAP visits every child (`_act || time >= _start`). Sorted by
            // start, nothing after the first unstarted child has started, and
            // F_ACT_AHEAD marks a timeline that may hold an active child
            // ahead of the playhead; only then does the walk go on past it.
            let full = self.has(n, F_ACT_AHEAD);
            let mut c = self.cold[n as usize].first;
            while c != NIL {
                let h = self.hot[c as usize];
                let next = h.next;
                if h.flags & F_KILLED == 0 && h.ts != 0.0 && c != skip {
                    if h.flags & F_ACT != 0 || time >= h.start {
                        let ctt = self.child_time(time, c);
                        self.render(c, ctt, suppress, force, yo);
                        // Only a child rendered ahead of the playhead can be
                        // left active there (checked after its render).
                        ahead |= h.start > time && self.has(c, F_ACT);
                    } else if !full && h.start > time {
                        // (A NaN start compares false: the walk goes on.)
                        break;
                    }
                }
                c = next;
            }
            if full && !ahead {
                self.hot[n as usize].flags &= !F_ACT_AHEAD;
            }
        } else {
            let adj = if total < 0.0 { total } else { time };
            let mut c = self.cold[n as usize].last;
            while c != NIL {
                let h = self.hot[c as usize];
                let prev = h.prev;
                if h.flags & F_KILLED == 0
                    && h.ts != 0.0
                    && c != skip
                    && (h.flags & F_ACT != 0 || adj <= self.cold[c as usize].end)
                {
                    let ctt = self.child_time(adj, c);
                    self.render(c, ctt, suppress, force, yo);
                    ahead |= h.start > time && self.has(c, F_ACT);
                }
                c = prev;
            }
        }
        if ahead {
            self.hot[n as usize].flags |= F_ACT_AHEAD;
        }
    }

    /// GSAP `_findNextPauseTween`: the first pause crossed between the two
    /// (rounded) times, in the direction of travel.
    fn find_next_pause(&self, n: u32, prev: f64, time: f64) -> u32 {
        if time > prev {
            let mut c = self.cold[n as usize].first;
            while c != NIL && self.hot[c as usize].start <= time {
                let h = &self.hot[c as usize];
                if h.flags & F_PAUSE_NODE != 0 && h.flags & F_KILLED == 0 && h.start > prev {
                    return c;
                }
                c = h.next;
            }
        } else {
            let mut c = self.cold[n as usize].last;
            while c != NIL && self.hot[c as usize].start >= time {
                let h = &self.hot[c as usize];
                if h.flags & F_PAUSE_NODE != 0 && h.flags & F_KILLED == 0 && h.start < prev {
                    return c;
                }
                c = h.prev;
            }
        }
        NIL
    }

    /// Label events (non-GSAP, opt-in): every label crossed, in the order
    /// of travel (labels at the same time in the order they were added).
    /// The timeline's labels are a time-sorted range: two binary searches
    /// find the crossed ones, O(log L + crossed), no allocation.
    fn emit_labels(&mut self, n: u32, prev: f64, time: f64) {
        if time == prev {
            return;
        }
        let (lo, hi) = self.label_range(n);
        if time > prev {
            // prev < t <= time, ascending.
            let a = lo + self.labels[lo..hi].partition_point(|l| l.time <= prev);
            let b = lo + self.labels[lo..hi].partition_point(|l| l.time <= time);
            for i in a..b {
                let tag = self.labels[i].tag;
                self.emit(n, EventKind::Label(tag));
            }
        } else {
            // time <= t < prev, descending by time; equal times keep their
            // insertion order.
            let a = lo + self.labels[lo..hi].partition_point(|l| l.time < time);
            let mut b = lo + self.labels[lo..hi].partition_point(|l| l.time < prev);
            while b > a {
                let t = self.labels[b - 1].time;
                let mut g = b - 1;
                while g > a && self.labels[g - 1].time == t {
                    g -= 1;
                }
                for i in g..b {
                    let tag = self.labels[i].tag;
                    self.emit(n, EventKind::Label(tag));
                }
                b = g;
            }
        }
    }

    /// GSAP `_initTween` (5.7): captures every unresolved track, runs
    /// overwrite Auto at the node's global time, marks it initted.
    pub(crate) fn init(&mut self, n: u32, local: f64) {
        let (s, e) = self.track_range(n);
        for k in s..e {
            let f = self.tr_meta[k as usize].flags;
            if f & T_ALIVE != 0 && f & T_RESOLVED == 0 {
                self.resolve_track(k);
            }
        }
        let h = &mut self.hot[n as usize];
        h.flags |= F_INITTED;
        h.flags &= !F_PRE;
        let c = &self.cold[n as usize];
        if c.overwrite == crate::spec::Overwrite::Auto && c.live_tracks > 0 {
            let g = self.global_time(n, local);
            self.overwrite_auto(n, g);
        }
    }

    /// Captures one track's endpoints from its spec and its slot's current
    /// value; an unknown (unseeded or mismatched) value does not move.
    pub(crate) fn resolve_track(&mut self, k: u32) {
        let ki = k as usize;
        let spec = self.tr_spec[ki];
        let m = self.tr_meta[ki];
        if m.flags & T_PATH != 0 {
            self.resolve_path_track(ki);
            return;
        }
        let s = self.tr_slot[ki] as usize;
        let cur = if self.sl_seeded[s] && self.sl_kind[s] == m.kind {
            Some(self.sl_val[s])
        } else {
            None
        };
        let from = spec.from.or(cur);
        let to = match spec.end {
            EndKind::To => Some(spec.val),
            EndKind::By => from.map(|f| {
                [
                    f[0] + spec.val[0],
                    f[1] + spec.val[1],
                    f[2] + spec.val[2],
                    f[3] + spec.val[3],
                ]
            }),
            EndKind::Current => cur,
        };
        let (f, t) = match (from, to) {
            (Some(f), Some(t)) => (f, t),
            (Some(f), None) => {
                self.stats.unseeded_starts += 1;
                (f, f)
            }
            (None, Some(t)) => {
                self.stats.unseeded_starts += 1;
                (t, t)
            }
            (None, None) => {
                self.stats.unseeded_starts += 1;
                (spec.val, spec.val)
            }
        };
        let (f, t) = if m.kind == ValueKind::Color && m.space != ColorSpace::Srgb {
            self.tr_spec[ki].land = [f, t];
            let rgba = |l: [f64; 4]| Rgba::new(l[0], l[1], l[2], l[3]);
            encode_pair(rgba(f), rgba(t), m.space)
        } else {
            (f, t)
        };
        self.tr_from[ki] = f;
        self.tr_to[ki] = t;
        self.tr_meta[ki].flags |= T_RESOLVED;
    }

    /// Captures a path track: its ends are the path's points at the bind's
    /// `start` and `end` plus the lane's offset. With `PathAlign::Start` an
    /// x / y / z lane moves the path so its start sits on the slot's
    /// current value (an unseeded slot counts in `unseeded_starts` and does
    /// not align).
    fn resolve_path_track(&mut self, k: usize) {
        let spec = self.tr_spec[k];
        let b = spec.bind as usize;
        let pb = self.pbinds[b];
        let g = &self.paths[pb.path as usize].geom;
        let a = g.sample_span(pb.span[0], pb.span[1], 0.0);
        let z = g.sample_span(pb.span[0], pb.span[1], 1.0);
        let (from, to, off) = if spec.lane == LANE_ROT {
            let off = pb.rot;
            (
                a.angle_deg * pb.rot_scale + off,
                z.angle_deg * pb.rot_scale + off,
                off,
            )
        } else {
            let i = spec.lane as usize;
            let (base0, base1) = (a.pos[i], z.pos[i]);
            let (off, from) = if pb.align == PathAlign::Start {
                let s = self.tr_slot[k] as usize;
                if self.sl_seeded[s] && self.sl_kind[s] == ValueKind::F64 {
                    let c = self.sl_val[s][0];
                    (c - base0 + pb.offset[i], c + pb.offset[i])
                } else {
                    self.stats.unseeded_starts += 1;
                    (pb.offset[i], base0 + pb.offset[i])
                }
            } else {
                (pb.offset[i], base0 + pb.offset[i])
            };
            (from, base1 + off, off)
        };
        self.pbinds[b].off[spec.lane as usize] = off;
        self.tr_from[k] = [from, 0.0, 0.0, 0.0];
        self.tr_to[k] = [to, 0.0, 0.0, 0.0];
        self.tr_meta[k].flags |= T_RESOLVED;
    }

    /// The value of path track `k` at eased ratio `e` (clamped to 0..=1 on
    /// the path), sampling through `memo` when it holds the track's bind.
    #[inline]
    fn path_value(&self, k: usize, e: f64, memo: &mut (u32, PathPoint)) -> f64 {
        let spec = &self.tr_spec[k];
        let pb = &self.pbinds[spec.bind as usize];
        if memo.0 != spec.bind {
            let g = &self.paths[pb.path as usize].geom;
            *memo = (spec.bind, g.sample_span(pb.span[0], pb.span[1], e));
        }
        let v = if spec.lane == LANE_ROT {
            memo.1.angle_deg * pb.rot_scale
        } else {
            memo.1.pos[spec.lane as usize]
        };
        v + pb.off[spec.lane as usize]
    }

    /// Writes node `n`'s tracks at progress `p` with eased ratio `e` (6.2):
    /// `p >= 1` lands `to`, `p <= 0` lands `from`, exactly. A path samples
    /// once per bind, whatever the number of lanes it writes.
    pub(crate) fn write_tracks(&mut self, n: u32, p: f64, e: f64) {
        let (s, end) = self.track_range(n);
        let mut memo = (NIL, PathPoint::default());
        for k in s as usize..end as usize {
            let m = self.tr_meta[k];
            if m.flags & (T_ALIVE | T_RESOLVED) != (T_ALIVE | T_RESOLVED) {
                continue;
            }
            let mut v = if p >= 1.0 {
                self.tr_to[k]
            } else if p <= 0.0 {
                self.tr_from[k]
            } else if m.flags & T_PATH != 0 {
                [self.path_value(k, e, &mut memo), 0.0, 0.0, 0.0]
            } else {
                lerp_lanes(self.tr_from[k], self.tr_to[k], e)
            };
            match m.kind {
                ValueKind::Int => v[0] = v[0].round(),
                ValueKind::Color => v = self.colour_lanes(k, v, p),
                _ => {}
            }
            if m.flags & T_SNAP != 0 {
                let snap = self.tr_spec[k].snap;
                for l in v.iter_mut().take(m.lanes as usize) {
                    *l = (*l / snap).round() * snap;
                }
            }
            let slot = self.tr_slot[k];
            self.sl_seeded[slot as usize] = true;
            self.write_slot(slot, v);
        }
    }

    /// A colour track's written lanes: the exact captured ends at p <= 0 /
    /// p >= 1, else the interpolated lanes decoded to sRGB; every channel
    /// (alpha included) clamped to 0..1, so an overshooting ease stays
    /// displayable.
    #[inline]
    fn colour_lanes(&self, k: usize, v: [f64; 4], p: f64) -> [f64; 4] {
        let space = self.tr_meta[k].space;
        let v = if space == ColorSpace::Srgb {
            v
        } else if p >= 1.0 {
            self.tr_spec[k].land[1]
        } else if p <= 0.0 {
            self.tr_spec[k].land[0]
        } else {
            let c = decode(v, space);
            [c.r, c.g, c.b, c.a]
        };
        v.map(|c| c.clamp(0.0, 1.0))
    }

    /// GSAP `invalidate()`: the next render re-captures (children too).
    pub(crate) fn invalidate_node(&mut self, n: u32) {
        {
            let h = &mut self.hot[n as usize];
            h.flags &= !(F_INITTED | F_ACT | F_PRE | F_INNER_INIT | F_LOCK);
        }
        let c = &mut self.cold[n as usize];
        c.ztime = -TINY;
        c.iztime = -TINY;
        let (s, e) = self.track_range(n);
        for k in s..e {
            self.tr_meta[k as usize].flags &= !T_RESOLVED;
        }
        let mut ch = self.cold[n as usize].first;
        while ch != NIL {
            self.invalidate_node(ch);
            ch = self.hot[ch as usize].next;
        }
    }

    /// The value tween `a` would give slot `s` at its own total time `t`,
    /// computed without rendering, writing or reporting. `None` when `a` is
    /// stale, has no captured track on `s`, or is a timeline or group.
    pub fn sample(
        &self,
        a: crate::ids::TweenId,
        s: crate::ids::SlotId,
        t: f64,
    ) -> Option<TweenValue> {
        let n = self.node(a)?;
        if self.kind(n) != K_TWEEN {
            return None;
        }
        let (start, end) = self.track_range(n);
        let k = (start..end).find(|&k| {
            let m = self.tr_meta[k as usize];
            self.tr_slot[k as usize] == s.0
                && m.flags & (T_ALIVE | T_RESOLVED) == (T_ALIVE | T_RESOLVED)
        })? as usize;
        let h = &self.hot[n as usize];
        let (p, e) = if h.dur == 0.0 {
            let r = if t < 0.0 { 0.0 } else { 1.0 };
            (r, r)
        } else {
            let tt = if t > h.tdur - TINY {
                h.tdur
            } else if t < TINY {
                0.0
            } else {
                t
            };
            let (_, time, yodd) = self.iterate(n, tt);
            let p = time / h.dur;
            let e = if p >= 1.0 {
                1.0
            } else if p <= 0.0 {
                0.0
            } else {
                self.ratio(n, p, yodd, NIL)
            };
            (p, e)
        };
        let m = self.tr_meta[k];
        let mut v = if p >= 1.0 {
            self.tr_to[k]
        } else if p <= 0.0 {
            self.tr_from[k]
        } else if m.flags & T_PATH != 0 {
            let mut memo = (NIL, PathPoint::default());
            [self.path_value(k, e, &mut memo), 0.0, 0.0, 0.0]
        } else {
            lerp_lanes(self.tr_from[k], self.tr_to[k], e)
        };
        if m.kind == ValueKind::Color {
            v = self.colour_lanes(k, v, p);
        }
        if m.flags & T_SNAP != 0 {
            let snap = self.tr_spec[k].snap;
            for l in v.iter_mut().take(m.lanes as usize) {
                *l = (*l / snap).round() * snap;
            }
        }
        Some(TweenValue::from_lanes(m.kind, v))
    }
}
