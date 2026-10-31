//! Playback control and inspection: GSAP's `Animation` methods
//! ([`AnimMut`], [`AnimRef`]), `Timeline` building methods
//! ([`TimelineMut`]), and the engine-level stepping calls.
//!
//! Every control renders synchronously, like GSAP. Events a control causes
//! land in the engine's queue and are read with
//! [`TweenEngine::swap_events`].

use crate::easing::Easing;
use crate::engine::*;
use crate::ids::{PropKey, Tag, TargetId, TweenId};
use crate::spec::{Emit, EventMask, KeyStep, Position, PropTo, Reduce, Seek, Targets, TweenOpts};
use crate::{animation_cycle, round7, BIG, TINY};

impl TweenEngine {
    // ------------------------------------------------------------------
    // Engine-level entry points
    // ------------------------------------------------------------------

    /// Steps every animation by `dt` seconds of wall time: GSAP's ticker
    /// rendering the global timeline. `dt` is what the host's clock and
    /// [`crate::TweenTicker`] produced; negative, NaN and infinite values
    /// count as 0. Allocation-free.
    pub fn advance(&mut self, dt: f64) {
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        if self.root == NIL {
            return; // nothing was ever built
        }
        let live = self.tr_slot.len() as u32 - self.dead_tracks;
        if self.dead_tracks > 64 && self.dead_tracks > live {
            self.compact_tracks();
        }
        let r = self.root;
        if self.cold[r as usize].first == NIL && self.hot[r as usize].ttime == 0.0 {
            // Idle and already rebased: the root's clock stays at 0 (a
            // detached kept animation's raw time then stands still, where
            // GSAP's monotonic clock would keep moving it; only `pause()` of
            // a reverse-completed animation with a delay can tell).
            return;
        }
        // Settle the root first: a negative child start shifts its playhead.
        self.total_duration(r);
        // Integrate unrounded; resync when something else moved the root.
        let now = self.hot[r as usize].ttime;
        let base = if round7(self.root_clock) == now {
            self.root_clock
        } else {
            now
        };
        self.root_clock = base + dt * self.hot[r as usize].ts;
        let t = self.root_clock;
        self.render_timeline(r, t, false, false);
        self.flush_reap();
        if self.cold[r as usize].first == NIL {
            self.rebase_root();
        }
    }

    /// Rebases the idle root's clock to 0 (design 5.1: root time stays small
    /// so round7 stays exact). Kept animations detached from the root (a
    /// completed or reverse-completed `keep` timeline) still measure their
    /// start against that clock (GSAP `rawTime()` folds through `_dp`, whose
    /// clock is monotonic), so their starts move with it: `pause()` then
    /// `resume()` on such a timeline stays where it stopped.
    fn rebase_root(&mut self) {
        let r = self.root;
        let shift = self.hot[r as usize].time;
        self.root_clock = 0.0;
        if shift != 0.0 {
            for n in 0..self.hot.len() {
                let c = &self.cold[n];
                if c.dp == r && c.parent == NIL && self.hot[n].flags & F_FREE == 0 {
                    let h = &mut self.hot[n];
                    h.start = round7(h.start - shift);
                    self.cold[n].end = round7(self.cold[n].end - shift);
                }
            }
        }
        {
            let h = &mut self.hot[r as usize];
            h.time = 0.0;
            h.ttime = 0.0;
            h.start = 0.0;
            h.dur = 0.0;
            h.tdur = 0.0;
        }
    }

    /// Whether anything will move on the next [`TweenEngine::advance`]: the
    /// root holds a linked, unpaused child (one scheduled for later counts).
    pub fn is_active(&self) -> bool {
        if self.root == NIL {
            return false;
        }
        let mut c = self.cold[self.root as usize].first;
        while c != NIL {
            let h = &self.hot[c as usize];
            if h.ts != 0.0 && h.flags & F_KILLED == 0 {
                return true;
            }
            c = h.next;
        }
        false
    }

    /// Whether a linked, unpaused, unfinished tween animates target `t`
    /// (GSAP `gsap.isTweening`).
    /// Visits only `t`'s own slots and their tracks (O(tracks on `t`)).
    pub fn is_tweening(&self, t: TargetId) -> bool {
        let mut s = self.first_slot_of(t);
        while s != NIL {
            let mut k = self.sl_first_track[s as usize];
            while k != NIL {
                if self.tr_meta[k as usize].flags & T_ALIVE != 0 {
                    let n = self.tr_node[k as usize];
                    let h = &self.hot[n as usize];
                    if self.attached(n) && self.no_paused_ancestors(n) && h.ttime < h.tdur {
                        return true;
                    }
                }
                k = self.tr_next_in_slot[k as usize];
            }
            s = self.sl_next_of_target[s as usize];
        }
        false
    }

    /// Reduced motion for one animation (design 5.17), by its
    /// [`Reduce`] policy: `JumpToEnd` renders the end in one step with
    /// events (an infinite repeat ends its first iteration and pauses),
    /// `Freeze` returns to the start and kills without Interrupt, `Keep`
    /// does nothing.
    pub fn finish(&mut self, a: TweenId) {
        if let Some(n) = self.node(a) {
            self.finish_node(n);
            self.after_control();
        }
    }

    /// [`TweenEngine::finish`] for every root-level animation.
    pub fn finish_all(&mut self) {
        if self.root == NIL {
            return;
        }
        let mut c = self.cold[self.root as usize].first;
        while c != NIL {
            let next = self.hot[c as usize].next;
            if self.hot[c as usize].flags & F_KILLED == 0 {
                self.finish_node(c);
            }
            c = next;
        }
        self.after_control();
    }

    /// [`TweenEngine::finish`] for the root-level animations that are
    /// playing: the ones [`TweenEngine::is_active`] counts (linked, not
    /// killed, not paused, time scale not 0). A host applying reduced motion
    /// on every frame calls this, so a paused (or scrubbed) timeline keeps
    /// its playhead instead of jumping to its end.
    pub fn finish_all_playing(&mut self) {
        if self.root == NIL {
            return;
        }
        let mut c = self.cold[self.root as usize].first;
        while c != NIL {
            let h = &self.hot[c as usize];
            let next = h.next;
            if h.ts != 0.0 && h.flags & F_KILLED == 0 {
                self.finish_node(c);
            }
            c = next;
        }
        self.after_control();
    }

    pub(crate) fn finish_node(&mut self, n: u32) {
        match self.cold[n as usize].reduce {
            Reduce::JumpToEnd => {
                let tdur = self.total_duration(n);
                if tdur < BIG {
                    let t = if self.cold[n as usize].rts < 0.0 {
                        0.0
                    } else {
                        tdur
                    };
                    let forcing = self.has(n, F_FORCING);
                    self.set_flag(n, F_FORCING, true);
                    self.set_total_time(n, t, false);
                    self.set_flag(n, F_FORCING, forcing);
                } else {
                    let d = self.hot[n as usize].dur;
                    self.set_total_time(n, d, false);
                    self.set_paused(n, true);
                }
            }
            Reduce::Freeze => {
                self.set_total_time(n, 0.0, true);
                self.kill_node(n, false);
            }
            Reduce::Keep => {}
        }
    }

    /// Housekeeping after a control: reclaim completed / killed nodes.
    pub(crate) fn after_control(&mut self) {
        self.flush_reap();
    }

    /// Whether every ancestor link of `n` reaches the root.
    pub(crate) fn attached(&self, n: u32) -> bool {
        let mut x = n;
        while x != self.root {
            if self.hot[x as usize].flags & (F_KILLED | F_FREE) != 0 {
                return false;
            }
            let p = self.cold[x as usize].parent;
            if p == NIL {
                return false;
            }
            x = p;
        }
        true
    }

    /// A handle for building into timeline `tl` (GSAP `tl.to()`,
    /// `tl.add()`, ...). A stale or non-timeline handle gives a builder whose
    /// calls do nothing.
    pub fn tl(&mut self, tl: TweenId) -> TimelineMut<'_> {
        let n = self
            .node(tl)
            .filter(|&n| self.kind(n) == K_TIMELINE)
            .unwrap_or(NIL);
        TimelineMut { e: self, tl: n }
    }

    /// A handle for controlling animation `a` (GSAP `Animation` methods).
    pub fn anim(&mut self, a: TweenId) -> AnimMut<'_> {
        let n = self.node(a).unwrap_or(NIL);
        AnimMut { e: self, n }
    }

    /// A read-only view of animation `a`.
    pub fn anim_ref(&self, a: TweenId) -> AnimRef<'_> {
        let n = self.node(a).unwrap_or(NIL);
        AnimRef { e: self, n }
    }

    /// Kills the property tracks of `props` (all when `None`) on `t` in every
    /// linked tween, in any state (GSAP `gsap.killTweensOf`). A tween left
    /// with nothing to animate is killed and reports Interrupt below
    /// progress 1.
    pub fn kill_tweens_of(&mut self, t: Targets, props: Option<&[PropKey]>) {
        self.kill_tracks_matching(NIL, t, props);
        self.after_control();
    }

    // ------------------------------------------------------------------
    // Control algorithms (GSAP Animation methods, 5.12)
    // ------------------------------------------------------------------

    /// GSAP `totalTime(t, suppressEvents)`: aligns under a smooth parent,
    /// re-enables completed ancestors, relinks an auto-removed node, then
    /// renders. On a timeline `add_pause` is ignored (GSAP `_forcing`).
    pub(crate) fn set_total_time(&mut self, n: u32, t: f64, suppress: bool) {
        if !t.is_finite() {
            return; // garbage in (NaN seek): ignored, never spread
        }
        let is_tl = self.kind(n) == K_TIMELINE;
        let was_forcing = self.has(n, F_FORCING);
        if is_tl {
            self.set_flag(n, F_FORCING, true);
        }
        let dp = self.cold[n as usize].dp;
        if dp != NIL && self.has(dp, F_SMOOTH) && self.hot[n as usize].ts != 0.0 {
            self.align(n, t);
            if self.cold[dp as usize].dp != NIL && self.cold[dp as usize].parent == NIL {
                self.post_add_checks(dp, n);
            }
            let mut x = dp;
            while x != NIL && self.cold[x as usize].parent != NIL {
                let p = self.cold[x as usize].parent;
                let hx = self.hot[x as usize];
                let local = if hx.ts >= 0.0 {
                    hx.ttime / hx.ts
                } else {
                    (self.total_duration(x) - hx.ttime) / -hx.ts
                };
                if self.hot[p as usize].time != hx.start + local {
                    self.set_total_time(x, hx.ttime, true);
                }
                x = p;
            }
            let h = self.hot[n as usize];
            let tdur = self.total_duration(n);
            if h.flags & F_LINKED == 0
                && self.has(dp, F_AUTO_REMOVE)
                && ((h.ts > 0.0 && t < tdur)
                    || (h.ts < 0.0 && t > 0.0)
                    || (tdur == 0.0 && t == 0.0))
            {
                let at = h.start - self.cold[n as usize].delay;
                self.add_child(dp, n, at, false);
            }
        }
        let h = self.hot[n as usize];
        let initted = h.flags & F_INITTED != 0;
        let zt = self.cold[n as usize].ztime;
        if h.ttime != t
            || (h.dur == 0.0 && !suppress)
            || (initted && zt.abs() == TINY)
            || (!initted && h.dur != 0.0 && t != 0.0)
            || (t == 0.0 && !initted)
        {
            if h.ts == 0.0 {
                self.cold[n as usize].ptime = t;
            }
            self.render(n, t, suppress, false, NIL);
        }
        // Driven directly: n may now be ACT ahead of its parent's playhead.
        let p = self.cold[n as usize].parent;
        if p != NIL && self.has(n, F_ACT) {
            self.hot[p as usize].flags |= F_ACT_AHEAD;
        }
        if is_tl && !was_forcing {
            self.set_flag(n, F_FORCING, false);
        }
    }

    /// GSAP `_alignPlayhead`: under a smooth parent, moves the start so the
    /// parent's playhead lands on total time `tt` (the child is re-sorted).
    pub(crate) fn align(&mut self, n: u32, tt: f64) {
        let dp = self.cold[n as usize].dp;
        let ts = self.hot[n as usize].ts;
        if dp == NIL || !self.has(dp, F_SMOOTH) || ts == 0.0 {
            return;
        }
        let local = if ts > 0.0 {
            tt / ts
        } else {
            (self.total_duration(n) - tt) / -ts
        };
        self.hot[n as usize].start = round7(self.hot[dp as usize].time - local);
        self.set_end(n);
        if self.has(n, F_LINKED) {
            let p = self.cold[n as usize].parent;
            if self.has(n, F_ACT) {
                self.hot[p as usize].flags |= F_ACT_AHEAD;
            }
            self.unlink_raw(n);
            self.insert_sorted(p, n);
        }
        self.uncache(dp);
    }

    /// GSAP `paused(p)`. Resuming renders at the recorded time with events
    /// (except at progress 1, where GSAP nudges the playhead and
    /// suppresses); the parent re-derives its duration at once.
    pub(crate) fn set_paused(&mut self, n: u32, p: bool) {
        if self.has(n, F_PAUSED) == p {
            return;
        }
        if p {
            let tt = self.hot[n as usize].ttime;
            let pt = if tt != 0.0 {
                tt
            } else {
                (-self.cold[n as usize].delay).max(self.raw_time(n))
            };
            self.cold[n as usize].ptime = pt;
            let h = &mut self.hot[n as usize];
            h.ts = 0.0;
            h.flags &= !F_ACT;
            h.flags |= F_PAUSED;
        } else {
            let rts = self.cold[n as usize].rts;
            let h = &mut self.hot[n as usize];
            h.ts = if rts == -TINY { 0.0 } else { rts };
            h.flags &= !F_PAUSED;
            let parent = self.cold[n as usize].parent;
            let tt = self.hot[n as usize].ttime;
            let t = if parent != NIL && !self.has(parent, F_SMOOTH) {
                self.raw_time(n)
            } else if tt != 0.0 {
                tt
            } else {
                self.cold[n as usize].ptime
            };
            let suppress =
                if self.progress_of(n) == 1.0 && self.cold[n as usize].ztime.abs() != TINY {
                    self.hot[n as usize].ttime -= TINY;
                    true
                } else {
                    false
                };
            if parent != NIL {
                self.uncache(parent);
            }
            self.set_total_time(n, t, suppress);
        }
    }

    /// GSAP `timeScale(s)`: keeps the value in place (the start re-aligns
    /// under a smooth parent), a negative scale plays backward.
    pub(crate) fn set_time_scale(&mut self, n: u32, s: f64) {
        if !s.is_finite() || self.cold[n as usize].rts == s {
            return;
        }
        let parent = self.cold[n as usize].parent;
        let tt = if parent != NIL && self.hot[n as usize].ts != 0.0 {
            let pt = self.hot[parent as usize].time;
            self.parent_to_child(pt, n)
        } else {
            self.hot[n as usize].ttime
        };
        self.cold[n as usize].rts = s;
        let paused = self.has(n, F_PAUSED);
        self.hot[n as usize].ts = if paused || s == -TINY { 0.0 } else { s };
        let tdur = self.total_duration(n);
        let lo = -self.cold[n as usize].delay.abs();
        self.set_total_time(n, tt.max(lo).min(tdur), true);
        self.set_end(n);
        self.recache_ancestors(n);
    }

    /// GSAP `_recacheAncestors`: every ancestor below the root recomputes
    /// its duration now.
    pub(crate) fn recache_ancestors(&mut self, n: u32) {
        let mut p = self.cold[n as usize].parent;
        while p != NIL && self.cold[p as usize].parent != NIL {
            self.hot[p as usize].flags |= F_DIRTY;
            self.total_duration(p);
            p = self.cold[p as usize].parent;
        }
    }

    /// GSAP `reversed(v)`: through a negative time scale.
    pub(crate) fn set_reversed(&mut self, n: u32, v: bool) {
        let rts = self.cold[n as usize].rts;
        if (rts < 0.0) != v {
            let s = if rts != 0.0 {
                -rts
            } else if v {
                -TINY
            } else {
                0.0
            };
            self.set_time_scale(n, s);
        }
    }

    /// The reported time scale (GSAP `timeScale()`).
    pub(crate) fn time_scale_of(&self, n: u32) -> f64 {
        let rts = self.cold[n as usize].rts;
        if rts == -TINY {
            0.0
        } else {
            rts
        }
    }

    pub(crate) fn cycle_of(&self, n: u32) -> f64 {
        self.pure_duration(n) + self.cold[n as usize].rdelay
    }

    /// GSAP `iteration()` (1-based).
    pub(crate) fn iteration_of(&self, n: u32) -> u32 {
        if self.cold[n as usize].repeat != 0 {
            animation_cycle(self.hot[n as usize].ttime, self.cycle_of(n)).saturating_add(1)
        } else {
            1
        }
    }

    /// GSAP `_elapsedCycleDuration`.
    pub(crate) fn elapsed_cycle(&self, n: u32) -> f64 {
        if self.cold[n as usize].repeat != 0 {
            let c = self.cycle_of(n);
            animation_cycle(self.hot[n as usize].ttime, c) as f64 * c
        } else {
            0.0
        }
    }

    /// GSAP `progress()`: of the current iteration (mirrored on yoyo returns).
    pub(crate) fn progress_of(&self, n: u32) -> f64 {
        let d = self.pure_duration(n);
        if d > 0.0 {
            (self.hot[n as usize].time / d).min(1.0)
        } else if self.raw_time_pure(n) > 0.0 {
            1.0
        } else {
            0.0
        }
    }

    /// GSAP `totalProgress()`.
    pub(crate) fn total_progress_of(&self, n: u32) -> f64 {
        let td = self.pure_total_duration(n);
        if td > 0.0 {
            (self.hot[n as usize].ttime / td).min(1.0)
        } else if self.has(n, F_INITTED) && self.raw_time_pure(n) >= 0.0 {
            1.0
        } else {
            0.0
        }
    }

    pub(crate) fn set_progress(&mut self, n: u32, p: f64, suppress: bool) {
        self.total_duration(n);
        let d = self.hot[n as usize].dur;
        let even = self.iteration_of(n).is_multiple_of(2);
        let q = if self.has(n, F_YOYO) && even {
            1.0 - p
        } else {
            p
        };
        let t = d * q + self.elapsed_cycle(n);
        self.set_total_time(n, t, suppress);
    }

    pub(crate) fn set_total_progress(&mut self, n: u32, p: f64, suppress: bool) {
        let td = self.total_duration(n);
        self.set_total_time(n, td * p, suppress);
    }

    /// Moves the playhead to `t` inside the CURRENT iteration (deviation:
    /// GSAP 3.15's `time(v)` always lands in the first iteration).
    pub(crate) fn set_time(&mut self, n: u32, t: f64, suppress: bool) {
        let td = self.total_duration(n);
        let d = self.hot[n as usize].dur;
        let tt = (self.elapsed_cycle(n) + t.max(0.0).min(d)).min(td);
        self.set_total_time(n, tt, suppress);
    }

    pub(crate) fn seek_node(&mut self, n: u32, s: Seek, suppress: bool) {
        match s {
            Seek::Time(t) => self.set_total_time(n, t, suppress),
            Seek::Label(l, o) => {
                if let Some(t) = self.label_time(n, l) {
                    self.set_total_time(n, t + o, suppress);
                }
            }
            Seek::Progress(p) => self.set_progress(n, p, suppress),
            Seek::TotalProgress(p) => self.set_total_progress(n, p, suppress),
        }
    }

    /// GSAP `_setDuration(n, d)` for a tween (total progress kept), or
    /// `duration(d)` on a timeline (through its time scale).
    pub(crate) fn set_duration(&mut self, n: u32, d: f64) {
        if !d.is_finite() {
            return;
        }
        if self.kind(n) == K_TIMELINE {
            if d > 0.0 {
                let repeat = self.cold[n as usize].repeat;
                let rd = self.cold[n as usize].rdelay;
                let cur = if repeat < 0 {
                    self.total_duration(n);
                    self.hot[n as usize].dur
                } else {
                    self.total_duration(n)
                };
                let v = if repeat > 0 {
                    d + (d + rd) * repeat as f64
                } else {
                    d
                };
                let rev = self.cold[n as usize].rts < 0.0;
                self.set_time_scale(n, cur / if rev { -v } else { v });
            }
            return;
        }
        let h = self.hot[n as usize];
        let dur = round7(d.max(0.0));
        let tp = if h.tdur > 0.0 { h.ttime / h.tdur } else { 0.0 };
        if tp != 0.0 && h.dur != 0.0 {
            self.hot[n as usize].time *= dur / h.dur;
        }
        let parent = self.cold[n as usize].parent;
        self.set_duration_raw(n, dur);
        if tp > 0.0 {
            let tt = self.hot[n as usize].tdur * tp;
            self.hot[n as usize].ttime = tt;
            self.sync_iter(n);
            self.align(n, tt);
        }
        if parent != NIL {
            self.set_end(n);
            self.uncache(parent);
        }
    }

    /// Keeps a tween's cached iteration equal to `animation_cycle(ttime)`
    /// after its total time was rewritten outside a render.
    pub(crate) fn sync_iter(&mut self, n: u32) {
        let it = if self.cold[n as usize].repeat != 0 {
            animation_cycle(self.hot[n as usize].ttime, self.cycle_of(n))
        } else {
            0
        };
        self.hot[n as usize].iter = it;
    }
}

/// Builds into one timeline: GSAP's `Timeline` methods (`tl.to()`,
/// `tl.add()`, `tl.addLabel()`, ...). Every method returns `&mut Self` so
/// calls chain like GSAP's; a stale handle makes every call a no-op.
pub struct TimelineMut<'a> {
    e: &'a mut TweenEngine,
    tl: u32,
}

impl<'a> TimelineMut<'a> {
    fn place(at: impl Into<Position>) -> Place {
        Place::Pos(at.into())
    }

    /// GSAP `tl.to()` / `from()` / `fromTo()`: a tween (or stagger /
    /// keyframes group) at position `at`, inheriting this timeline's
    /// `defaults`.
    pub fn tween(
        &mut self,
        t: Targets,
        props: &[PropTo],
        o: TweenOpts,
        at: impl Into<Position>,
    ) -> &mut Self {
        if self.tl != NIL {
            self.e.build_tween(self.tl, t, props, &o, Self::place(at));
            self.e.finish_build();
        }
        self
    }

    /// GSAP `tl.to()`.
    pub fn to(
        &mut self,
        t: Targets,
        props: &[PropTo],
        o: TweenOpts,
        at: impl Into<Position>,
    ) -> &mut Self {
        self.tween(t, props, o, at)
    }

    /// GSAP `tl.from()`: renders its start values at once.
    pub fn from(
        &mut self,
        t: Targets,
        props: &[PropTo],
        o: TweenOpts,
        at: impl Into<Position>,
    ) -> &mut Self {
        self.tween(t, props, o, at)
    }

    /// GSAP `tl.fromTo()`: renders its start values at once.
    pub fn from_to(
        &mut self,
        t: Targets,
        props: &[PropTo],
        o: TweenOpts,
        at: impl Into<Position>,
    ) -> &mut Self {
        self.tween(t, props, o, at)
    }

    /// GSAP `tl.set()`: a zero-duration tween applied when the playhead
    /// reaches it (not at once, unless `immediate_render(true)`).
    pub fn set(
        &mut self,
        t: Targets,
        props: &[PropTo],
        o: TweenOpts,
        at: impl Into<Position>,
    ) -> &mut Self {
        let mut o = o.duration(0.0).repeat(0);
        if o.immediate_render.is_none() {
            o.immediate_render = Some(false);
        }
        self.tween(t, props, o, at)
    }

    /// GSAP array keyframes at position `at` (see
    /// [`TweenEngine::keyframes`]).
    pub fn keyframes(
        &mut self,
        t: Targets,
        steps: &[KeyStep],
        o: TweenOpts,
        at: impl Into<Position>,
    ) -> &mut Self {
        if self.tl != NIL {
            self.e
                .build_keyframes(self.tl, t, steps, &o, Self::place(at));
            self.e.finish_build();
        }
        self
    }

    /// GSAP `tl.add(child, position)`: (re)parents a tween or timeline.
    /// Adding a timeline to itself or to its own descendant does nothing.
    pub fn add(&mut self, child: TweenId, at: impl Into<Position>) -> &mut Self {
        let Some(c) = self.e.node(child) else {
            return self;
        };
        if self.tl == NIL || self.is_self_or_ancestor(c) {
            return self;
        }
        let pos = self.e.resolve_position(self.tl, at.into(), Some(c));
        self.e.add_child(self.tl, c, pos, false);
        self.e.finish_build();
        self
    }

    fn is_self_or_ancestor(&self, c: u32) -> bool {
        let mut x = self.tl;
        while x != NIL {
            if x == c {
                return true;
            }
            x = self.e.cold[x as usize].dp;
        }
        false
    }

    /// GSAP `tl.addLabel(label, position)`; re-adding moves it. Does not
    /// change `recent`.
    pub fn add_label(&mut self, label: Tag, at: impl Into<Position>) -> &mut Self {
        if self.tl != NIL {
            let t = self.e.resolve_position(self.tl, at.into(), None);
            self.e.set_label(self.tl, label, t);
            self.e.update_weights(self.tl);
            self.e.finish_build();
        }
        self
    }

    /// GSAP `tl.removeLabel(label)`.
    pub fn remove_label(&mut self, label: Tag) -> &mut Self {
        let tl = self.tl;
        self.e.labels.retain(|l| !(l.tl == tl && l.tag == label));
        self
    }

    /// GSAP `tl.call(fn, params, position)`: reports
    /// [`crate::EventKind::Call`] with `tag` whenever the playhead crosses
    /// it (both directions).
    pub fn call(&mut self, tag: Tag, at: impl Into<Position>) -> &mut Self {
        if self.tl != NIL {
            self.e
                .build_marker(self.tl, F_CALL, tag, 0.0, Self::place(at));
            self.e.finish_build();
        }
        self
    }

    /// GSAP `tl.addPause(position, callback)`: the frame walk stops exactly
    /// there, pauses the timeline and reports [`crate::EventKind::Pause`].
    /// Seeks and other controls jump over it (GSAP `_forcing`).
    pub fn add_pause(&mut self, at: impl Into<Position>, tag: Tag) -> &mut Self {
        if self.tl != NIL {
            self.e
                .build_marker(self.tl, F_PAUSE_NODE, tag, 0.0, Self::place(at));
            self.e.set_flag(self.tl, F_HAS_PAUSE, true);
            self.e.finish_build();
        }
        self
    }

    /// GSAP `tl.remove(child)`: unlinks it (it stays alive, unparented);
    /// removing the most recent child makes the last child recent.
    pub fn remove(&mut self, child: TweenId) -> &mut Self {
        if let Some(c) = self.e.node(child) {
            if self.tl != NIL && self.e.cold[c as usize].parent == self.tl {
                self.e.unlink(c);
            }
        }
        self
    }

    /// GSAP `tl.clear(labels)`: kills every child (no Interrupt) and, when
    /// asked, drops the labels.
    pub fn clear(&mut self, labels: bool) -> &mut Self {
        let tl = self.tl;
        if tl == NIL {
            return self;
        }
        let mut c = self.e.cold[tl as usize].first;
        while c != NIL {
            let next = self.e.hot[c as usize].next;
            self.e.kill_node(c, false);
            c = next;
        }
        self.e.flush_reap();
        if self.e.cold[tl as usize].dp != NIL {
            let h = &mut self.e.hot[tl as usize];
            h.time = 0.0;
            h.ttime = 0.0;
            self.e.cold[tl as usize].ptime = 0.0;
        }
        if labels {
            self.e.labels.retain(|l| l.tl != tl);
        }
        self.e.cold[tl as usize].recent = NIL;
        self.e.uncache(tl);
        self
    }

    /// GSAP `tl.shiftChildren(amount, adjustLabels, ignoreBeforeTime)`.
    pub fn shift_children(
        &mut self,
        amount: f64,
        adjust_labels: bool,
        ignore_before: f64,
    ) -> &mut Self {
        let tl = self.tl;
        if tl == NIL {
            return self;
        }
        self.e.shift_children_raw(tl, amount, ignore_before);
        if adjust_labels {
            let a = round7(amount);
            let (lo, hi) = self.e.label_range(tl);
            for l in &mut self.e.labels[lo..hi] {
                if l.time >= ignore_before {
                    l.time += a;
                }
            }
            self.e.resort_labels(tl);
        }
        self.e.uncache(tl);
        self
    }

    /// GSAP `tl.recent()`: the most recently added child.
    pub fn last(&self) -> TweenId {
        if self.tl == NIL {
            return TweenId::NONE;
        }
        let r = self.e.cold[self.tl as usize].recent;
        if r == NIL {
            TweenId::NONE
        } else {
            self.e.id_of(r)
        }
    }

    /// This timeline's handle.
    pub fn id(&self) -> TweenId {
        if self.tl == NIL {
            TweenId::NONE
        } else {
            self.e.id_of(self.tl)
        }
    }
}

/// Controls one animation: GSAP's `Animation` methods. Every method renders
/// synchronously and returns `&mut Self`; a stale handle makes every call a
/// no-op.
pub struct AnimMut<'a> {
    e: &'a mut TweenEngine,
    n: u32,
}

impl<'a> AnimMut<'a> {
    #[inline]
    fn run(&mut self, f: impl FnOnce(&mut TweenEngine, u32)) -> &mut Self {
        if self.n != NIL && self.e.hot[self.n as usize].flags & (F_FREE | F_KILLED) == 0 {
            f(self.e, self.n);
            self.e.after_control();
        }
        self
    }

    /// GSAP `play()`: plays forward (un-reverses) and unpauses.
    pub fn play(&mut self) -> &mut Self {
        self.run(|e, n| {
            e.set_reversed(n, false);
            e.set_paused(n, false);
        })
    }

    /// GSAP `play(from)`: seeks, then plays forward.
    pub fn play_from(&mut self, at: Seek, ev: Emit) -> &mut Self {
        self.run(|e, n| {
            e.seek_node(n, at, ev == Emit::Suppress);
            if e.hot[n as usize].flags & (F_FREE | F_KILLED) == 0 {
                e.set_reversed(n, false);
                e.set_paused(n, false);
            }
        })
    }

    /// GSAP `pause()`.
    pub fn pause(&mut self) -> &mut Self {
        self.run(|e, n| e.set_paused(n, true))
    }

    /// GSAP `pause(atTime)`: seeks, then pauses.
    pub fn pause_at(&mut self, at: Seek, ev: Emit) -> &mut Self {
        self.run(|e, n| {
            e.seek_node(n, at, ev == Emit::Suppress);
            if e.hot[n as usize].flags & (F_FREE | F_KILLED) == 0 {
                e.set_paused(n, true);
            }
        })
    }

    /// GSAP `resume()`: unpauses, keeping the direction.
    pub fn resume(&mut self) -> &mut Self {
        self.run(|e, n| e.set_paused(n, false))
    }

    /// GSAP `reverse()`: plays backward and unpauses. Not a toggle: calling
    /// it twice stays reversed (toggle with `set_reversed(!reversed)`).
    pub fn reverse(&mut self) -> &mut Self {
        self.run(|e, n| {
            e.set_reversed(n, true);
            e.set_paused(n, false);
        })
    }

    /// GSAP `reverse(from)`: seeks (`None` = the end), then reverses.
    pub fn reverse_from(&mut self, at: Option<Seek>, ev: Emit) -> &mut Self {
        self.run(|e, n| {
            let s = at.unwrap_or(Seek::TotalProgress(1.0));
            e.seek_node(n, s, ev == Emit::Suppress);
            if e.hot[n as usize].flags & (F_FREE | F_KILLED) == 0 {
                e.set_reversed(n, true);
                e.set_paused(n, false);
            }
        })
    }

    /// GSAP `restart(includeDelay, suppressEvents)`: plays forward from the
    /// start (or from before the delay). GSAP suppresses by default.
    pub fn restart(&mut self, include_delay: bool, ev: Emit) -> &mut Self {
        self.run(|e, n| {
            e.set_reversed(n, false);
            e.set_paused(n, false);
            if e.hot[n as usize].flags & (F_FREE | F_KILLED) != 0 {
                return;
            }
            let t = if include_delay {
                -e.cold[n as usize].delay
            } else {
                0.0
            };
            e.set_total_time(n, t, ev == Emit::Suppress);
            if e.hot[n as usize].dur == 0.0 {
                e.cold[n as usize].ztime = -TINY;
            }
        })
    }

    /// GSAP `seek(position, suppressEvents)` (GSAP suppresses by default).
    pub fn seek(&mut self, at: Seek, ev: Emit) -> &mut Self {
        self.run(|e, n| e.seek_node(n, at, ev == Emit::Suppress))
    }

    /// GSAP `progress(p)`: progress of the current iteration (yoyo-aware:
    /// a mirrored iteration reads back the value it was given).
    pub fn set_progress(&mut self, p: f64, ev: Emit) -> &mut Self {
        self.run(|e, n| e.set_progress(n, p, ev == Emit::Suppress))
    }

    /// GSAP `totalProgress(p)`.
    pub fn set_total_progress(&mut self, p: f64, ev: Emit) -> &mut Self {
        self.run(|e, n| e.set_total_progress(n, p, ev == Emit::Suppress))
    }

    /// GSAP `time(t)`, landing in the current iteration (GSAP 3.15 lands in
    /// the first one; see the crate docs).
    pub fn set_time(&mut self, t: f64, ev: Emit) -> &mut Self {
        self.run(|e, n| e.set_time(n, t, ev == Emit::Suppress))
    }

    /// GSAP `totalTime(t)`.
    pub fn set_total_time(&mut self, t: f64, ev: Emit) -> &mut Self {
        self.run(|e, n| e.set_total_time(n, t, ev == Emit::Suppress))
    }

    /// GSAP `iteration(k)` (1-based): the same place in iteration `k`.
    pub fn set_iteration(&mut self, k: u32, ev: Emit) -> &mut Self {
        self.run(|e, n| {
            let t = e.hot[n as usize].time + (k as f64 - 1.0) * e.cycle_of(n);
            e.set_total_time(n, t, ev == Emit::Suppress);
        })
    }

    /// GSAP `timeScale(s)`: a negative scale reverses.
    pub fn set_time_scale(&mut self, s: f64) -> &mut Self {
        self.run(|e, n| e.set_time_scale(n, s))
    }

    /// GSAP `reversed(r)`.
    pub fn set_reversed(&mut self, r: bool) -> &mut Self {
        self.run(|e, n| e.set_reversed(n, r))
    }

    /// GSAP `paused(p)`.
    pub fn set_paused(&mut self, p: bool) -> &mut Self {
        self.run(|e, n| e.set_paused(n, p))
    }

    /// GSAP `duration(d)`: a tween keeps its total progress; a timeline is
    /// time-scaled to fit.
    pub fn set_duration(&mut self, d: f64) -> &mut Self {
        self.run(|e, n| e.set_duration(n, d))
    }

    /// GSAP `delay(d)`: under a smooth parent the start moves with it;
    /// otherwise only the stored delay changes.
    pub fn set_delay(&mut self, d: f64) -> &mut Self {
        if !d.is_finite() {
            return self;
        }
        self.run(|e, n| {
            let parent = e.cold[n as usize].parent;
            if parent != NIL && e.has(parent, F_SMOOTH) {
                let start = e.hot[n as usize].start + d - e.cold[n as usize].delay;
                e.cold[n as usize].delay = d;
                e.set_start_time(n, start);
            } else {
                e.cold[n as usize].delay = d;
            }
        })
    }

    /// GSAP `startTime(t)`: re-inserted at `t` (sorted), in its last parent.
    pub fn set_start_time(&mut self, t: f64) -> &mut Self {
        self.run(|e, n| e.set_start_time(n, t))
    }

    /// GSAP `repeat(k)` (-1 forever).
    pub fn set_repeat(&mut self, k: i32) -> &mut Self {
        self.run(|e, n| {
            e.cold[n as usize].repeat = k;
            e.on_update_total_duration(n);
            e.update_weights(n);
        })
    }

    /// GSAP `repeatDelay(d)`.
    pub fn set_repeat_delay(&mut self, d: f64) -> &mut Self {
        if !d.is_finite() {
            return self;
        }
        self.run(|e, n| {
            let time = e.hot[n as usize].time;
            e.cold[n as usize].rdelay = d.max(0.0);
            e.on_update_total_duration(n);
            if time != 0.0 {
                e.set_time(n, time, false);
            }
        })
    }

    /// GSAP `yoyo(b)`.
    pub fn set_yoyo(&mut self, y: bool) -> &mut Self {
        self.run(|e, n| e.set_flag(n, F_YOYO, y))
    }

    /// GSAP `invalidate()`: the next render re-captures start values.
    pub fn invalidate(&mut self) -> &mut Self {
        self.run(|e, n| e.invalidate_node(n))
    }

    /// Re-renders from 0 to the current total time, events suppressed and
    /// forced (GSAP `render(0, true, true)` then `render(totalTime, true,
    /// true)`): after a child's start or duration changed on a paused,
    /// scrubbed timeline, the values at the playhead are right again (a child
    /// moved past the playhead shows its start value). Captured start values
    /// are NOT re-captured (GSAP neither; use [`AnimMut::invalidate`]).
    pub fn refresh(&mut self) -> &mut Self {
        self.run(|e, n| {
            let t = e.hot[n as usize].ttime;
            e.render(n, 0.0, true, true, NIL);
            if e.hot[n as usize].flags & (F_FREE | F_KILLED) == 0 {
                e.render(n, t, true, true, NIL);
            }
        })
    }

    /// Replaces the callbacks this animation reports (GSAP
    /// `eventCallback(type, fn)` after creation: a script layer that
    /// attaches `onComplete` to a built timeline). Events already queued
    /// stay queued.
    pub fn set_events(&mut self, m: EventMask) -> &mut Self {
        self.run(|e, n| {
            e.cold[n as usize].events = m;
            e.update_weights(n);
            e.reserve_events();
        })
    }

    /// GSAP `kill()`: reports Interrupt (below progress 1) and frees the
    /// animation; its handle goes stale. Slots keep their values.
    pub fn kill(self) {
        if self.n != NIL && self.e.hot[self.n as usize].flags & (F_FREE | F_KILLED) == 0 {
            self.e.kill_node(self.n, true);
            self.e.after_control();
        }
    }
}

impl TweenEngine {
    pub(crate) fn set_start_time(&mut self, n: u32, t: f64) {
        if !t.is_finite() {
            return;
        }
        self.hot[n as usize].start = round7(t);
        let c = self.cold[n as usize];
        let p = if c.parent != NIL { c.parent } else { c.dp };
        if p != NIL {
            let at = self.hot[n as usize].start - c.delay;
            self.add_child(p, n, at, false);
        }
    }

    /// GSAP `_onUpdateTotalDuration`: a timeline re-derives lazily, a tween
    /// keeps its total progress.
    pub(crate) fn on_update_total_duration(&mut self, n: u32) {
        if self.kind(n) == K_TIMELINE {
            self.uncache(n);
            if self.cold[n as usize].parent != NIL {
                self.total_duration(n);
                self.set_end(n);
            }
        } else {
            let d = self.hot[n as usize].dur;
            let h = self.hot[n as usize];
            let tp = if h.tdur > 0.0 { h.ttime / h.tdur } else { 0.0 };
            let parent = self.cold[n as usize].parent;
            self.set_duration_raw(n, d);
            if tp > 0.0 {
                let tt = self.hot[n as usize].tdur * tp;
                self.hot[n as usize].ttime = tt;
                self.align(n, tt);
            }
            self.sync_iter(n);
            if parent != NIL {
                self.uncache(parent);
            }
        }
    }
}

/// A read-only view of one animation: GSAP's getters. A stale handle
/// answers the defaults (false, 0, `None`, [`Tag::NONE`]).
pub struct AnimRef<'a> {
    e: &'a TweenEngine,
    n: u32,
}

impl<'a> AnimRef<'a> {
    #[inline]
    fn live(&self) -> bool {
        self.n != NIL
    }

    /// Whether the handle still refers to a live animation.
    pub fn is_alive(&self) -> bool {
        self.live()
    }

    /// GSAP `isActive()`: unpaused, initted and its parent's playhead is
    /// inside it.
    pub fn is_active(&self) -> bool {
        // An unlinked animation (completed and removed, or removed by hand)
        // is not active: the root's clock is rebased when it empties.
        self.live() && self.e.attached(self.n) && self.active(self.n)
    }

    fn active(&self, n: u32) -> bool {
        let e = self.e;
        let c = &e.cold[n as usize];
        let p = if c.parent != NIL { c.parent } else { c.dp };
        if p == NIL {
            return true;
        }
        let h = &e.hot[n as usize];
        if h.ts == 0.0 || h.flags & F_INITTED == 0 || !self.active(p) {
            return false;
        }
        let raw = self.raw_wrapped(p);
        raw >= h.start && raw < e.end_time(n, true) - TINY
    }

    /// GSAP `rawTime(true)`.
    fn raw_wrapped(&self, n: u32) -> f64 {
        let e = self.e;
        let c = &e.cold[n as usize];
        let p = if c.parent != NIL { c.parent } else { c.dp };
        let h = &e.hot[n as usize];
        if p == NIL {
            return h.ttime;
        }
        if h.ts == 0.0 || (c.repeat != 0 && h.time != 0.0 && e.total_progress_of(n) < 1.0) {
            return h.ttime % e.cycle_of(n);
        }
        e.parent_to_child_pure(self.raw_wrapped(p), n)
    }

    /// GSAP `paused()`.
    pub fn paused(&self) -> bool {
        self.live() && self.e.has(self.n, F_PAUSED)
    }

    /// GSAP `reversed()` (a negative time scale).
    pub fn reversed(&self) -> bool {
        self.live() && self.e.cold[self.n as usize].rts < 0.0
    }

    /// GSAP `time()`: iteration time (yoyo-mirrored).
    pub fn time(&self) -> f64 {
        if self.live() {
            self.e.hot[self.n as usize].time
        } else {
            0.0
        }
    }

    /// GSAP `totalTime()`.
    pub fn total_time(&self) -> f64 {
        if self.live() {
            self.e.hot[self.n as usize].ttime
        } else {
            0.0
        }
    }

    /// GSAP `progress()`.
    pub fn progress(&self) -> f64 {
        if self.live() {
            self.e.progress_of(self.n)
        } else {
            0.0
        }
    }

    /// GSAP `totalProgress()`.
    pub fn total_progress(&self) -> f64 {
        if self.live() {
            self.e.total_progress_of(self.n)
        } else {
            0.0
        }
    }

    /// GSAP `duration()`: one iteration.
    pub fn duration(&self) -> f64 {
        if self.live() {
            self.e.pure_duration(self.n)
        } else {
            0.0
        }
    }

    /// GSAP `totalDuration()`: repeats included (1e10 for `repeat: -1`).
    pub fn total_duration(&self) -> f64 {
        if self.live() {
            self.e.pure_total_duration(self.n)
        } else {
            0.0
        }
    }

    /// GSAP `timeScale()`.
    pub fn time_scale(&self) -> f64 {
        if self.live() {
            self.e.time_scale_of(self.n)
        } else {
            0.0
        }
    }

    /// GSAP `iteration()` (1-based).
    pub fn iteration(&self) -> u32 {
        if self.live() {
            self.e.iteration_of(self.n)
        } else {
            0
        }
    }

    /// GSAP `startTime()`: in the parent's time, delay included.
    pub fn start_time(&self) -> f64 {
        if self.live() {
            self.e.hot[self.n as usize].start
        } else {
            0.0
        }
    }

    /// GSAP `endTime(includeRepeats)`.
    pub fn end_time(&self, include_repeats: bool) -> f64 {
        if self.live() {
            self.e.end_time(self.n, include_repeats)
        } else {
            0.0
        }
    }

    /// GSAP `delay()`.
    pub fn delay(&self) -> f64 {
        if self.live() {
            self.e.cold[self.n as usize].delay
        } else {
            0.0
        }
    }

    /// GSAP `repeat()`.
    pub fn repeat(&self) -> i32 {
        if self.live() {
            self.e.cold[self.n as usize].repeat
        } else {
            0
        }
    }

    /// GSAP `yoyo()`.
    pub fn yoyo(&self) -> bool {
        self.live() && self.e.has(self.n, F_YOYO)
    }

    /// The callbacks this animation reports ([`EventMask::NONE`] for a
    /// stale handle).
    pub fn events(&self) -> EventMask {
        if self.live() {
            self.e.cold[self.n as usize].events
        } else {
            EventMask::NONE
        }
    }

    /// GSAP `vars.id`.
    pub fn tag(&self) -> Tag {
        if self.live() {
            self.e.cold[self.n as usize].tag
        } else {
            Tag::NONE
        }
    }

    /// GSAP `globalTime(local)`: a local time as root time.
    pub fn global_time(&self, local: f64) -> f64 {
        if self.live() {
            self.e.global_time(self.n, local)
        } else {
            0.0
        }
    }

    /// GSAP `currentLabel()`: the latest label at or before the playhead.
    pub fn current_label(&self) -> Option<Tag> {
        self.live().then_some(())?;
        let t = self.e.hot[self.n as usize].time;
        self.e.label_in_direction(self.n, t + TINY, true)
    }

    /// GSAP `nextLabel()`: the nearest label strictly after the playhead.
    pub fn next_label(&self) -> Option<Tag> {
        self.live().then_some(())?;
        let t = self.e.hot[self.n as usize].time;
        self.e.label_in_direction(self.n, t, false)
    }

    /// GSAP `previousLabel()`: the nearest label strictly before the playhead.
    pub fn previous_label(&self) -> Option<Tag> {
        self.live().then_some(())?;
        let t = self.e.hot[self.n as usize].time;
        self.e.label_in_direction(self.n, t, true)
    }

    /// The time of label `l` (GSAP `tl.labels[l]`).
    pub fn label_time(&self, l: Tag) -> Option<f64> {
        self.live().then_some(())?;
        self.e.label_time(self.n, l)
    }

    /// How many children are linked (GSAP `getChildren(false).length`).
    pub fn child_count(&self) -> u32 {
        if !self.live() {
            return 0;
        }
        let mut k = 0;
        let mut c = self.e.cold[self.n as usize].first;
        while c != NIL {
            k += 1;
            c = self.e.hot[c as usize].next;
        }
        k
    }

    /// What this animation is (`None` for a stale handle): a timeline, a
    /// stagger or keyframes group, a call, a pause, or a plain tween.
    pub fn kind(&self) -> Option<AnimKind> {
        if !self.live() {
            return None;
        }
        let f = self.e.hot[self.n as usize].flags;
        Some(match f & K_MASK {
            K_TIMELINE => AnimKind::Timeline,
            K_GROUP if f & F_KEYFRAMES != 0 => AnimKind::Keyframes,
            K_GROUP => AnimKind::Stagger,
            _ if f & F_CALL != 0 => AnimKind::Call,
            _ if f & F_PAUSE_NODE != 0 => AnimKind::Pause,
            _ => AnimKind::Tween,
        })
    }

    /// The linked parent (GSAP `parent`): a timeline or a stagger /
    /// keyframes group. `None` at root level (built with `TweenEngine::to`,
    /// `timeline`, ..., or removed from its timeline) and for a stale
    /// handle.
    pub fn parent(&self) -> Option<TweenId> {
        if !self.live() {
            return None;
        }
        let p = self.e.cold[self.n as usize].parent;
        if p == NIL || p == self.e.root {
            None
        } else {
            Some(self.e.id_of(p))
        }
    }

    /// The linked children in start order (GSAP `getChildren(false)`): a
    /// timeline's tweens, timelines, calls and pauses, or a group's
    /// per-target tweens (stagger) or steps (keyframes). Empty for a leaf
    /// and for a stale handle. Allocation-free.
    pub fn children(&self) -> ChildIter<'a> {
        let c = if self.live() {
            self.e.cold[self.n as usize].first
        } else {
            NIL
        };
        ChildIter { e: self.e, c }
    }

    /// The labels as `(tag, time)` in time order, ties in the order they
    /// were added (GSAP `tl.labels`). Empty for anything but a timeline.
    /// Allocation-free.
    pub fn labels(&self) -> LabelIter<'a> {
        let s: &'a [Label] = if self.live() {
            self.e.labels_of(self.n)
        } else {
            &[]
        };
        LabelIter { it: s.iter() }
    }

    /// GSAP `repeatDelay()`.
    pub fn repeat_delay(&self) -> f64 {
        if self.live() {
            self.e.cold[self.n as usize].rdelay
        } else {
            0.0
        }
    }

    /// A group's content duration (its children's extent in the group's
    /// inner time, which its outer ease maps onto [`AnimRef::duration`]);
    /// [`AnimRef::duration`] for everything else.
    pub fn inner_duration(&self) -> f64 {
        if self.live() && self.e.kind(self.n) == K_GROUP {
            self.e.cold[self.n as usize].inner_dur
        } else {
            self.duration()
        }
    }

    /// The ease (GSAP `vars.ease`; a group's children carry the tween
    /// ease, the group itself runs linear). `Easing::Linear` for a stale
    /// handle.
    pub fn ease(&self) -> Easing {
        if self.live() {
            self.e.cold[self.n as usize].ease
        } else {
            Easing::Linear
        }
    }

    /// The target a tween animates: the target of its first track (a
    /// killed track counts until the storage is compacted). A group
    /// answers its first child's target; a timeline, a call, a pause and a
    /// stale handle `None`.
    pub fn first_target(&self) -> Option<TargetId> {
        if !self.live() {
            return None;
        }
        self.e.first_target_of(self.n)
    }

    /// Whether this tween follows a motion path (`PropTo::path`).
    pub fn has_path(&self) -> bool {
        self.live() && self.e.has(self.n, F_PATH)
    }
}

impl TweenEngine {
    fn first_target_of(&self, n: u32) -> Option<TargetId> {
        match self.kind(n) {
            K_TIMELINE => None,
            K_GROUP => {
                let mut c = self.cold[n as usize].first;
                while c != NIL {
                    if self.hot[c as usize].flags & (F_KILLED | F_FREE) == 0 {
                        return self.first_target_of(c);
                    }
                    c = self.hot[c as usize].next;
                }
                None
            }
            _ => {
                let cold = &self.cold[n as usize];
                if cold.n_tracks == 0 {
                    return None;
                }
                let s = *self.tr_slot.get(cold.tracks as usize)?;
                self.sl_target.get(s as usize).copied()
            }
        }
    }
}

/// What an animation is ([`AnimRef::kind`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimKind {
    /// A plain tween (zero-duration `set()` tweens included).
    Tween,
    /// A timeline (GSAP `gsap.timeline()`).
    Timeline,
    /// A stagger group: one tween per target, spread in time.
    Stagger,
    /// A keyframes group (array or percent keyframes).
    Keyframes,
    /// A call marker (`tl.call()`, `delayed_call`).
    Call,
    /// A pause marker (`tl.add_pause()`).
    Pause,
}

/// The linked children of an animation, in start order
/// ([`AnimRef::children`]). Killed children are skipped.
pub struct ChildIter<'a> {
    e: &'a TweenEngine,
    c: u32,
}

impl<'a> Iterator for ChildIter<'a> {
    type Item = TweenId;

    fn next(&mut self) -> Option<TweenId> {
        while self.c != NIL {
            let c = self.c;
            let h = &self.e.hot[c as usize];
            self.c = h.next;
            if h.flags & (F_KILLED | F_FREE) == 0 {
                return Some(self.e.id_of(c));
            }
        }
        None
    }
}

/// A timeline's labels as `(tag, time)` in time order
/// ([`AnimRef::labels`]).
pub struct LabelIter<'a> {
    it: std::slice::Iter<'a, Label>,
}

impl<'a> Iterator for LabelIter<'a> {
    type Item = (Tag, f64);

    fn next(&mut self) -> Option<(Tag, f64)> {
        self.it.next().map(|l| (l.tag, l.time))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
}

impl ExactSizeIterator for LabelIter<'_> {}
