//! The engine: storage, building, insertion, positions, the duration cache,
//! the value store and memory reclamation.
//!
//! [`TweenEngine`] is GSAP's global timeline plus everything GSAP keeps on
//! its `Animation` objects, laid out for speed (section 4 of the design):
//! a 64-byte hot record per node that the child walk reads, a cold record for
//! everything else, structure-of-arrays property tracks and an
//! open-addressing value store. Rendering lives in `render.rs`, playback
//! control in `control.rs` and overwriting in `overwrite.rs`.

use crate::easing::{Easing, YoyoEase};
use crate::event::{EventKind, Stats, TweenEvent};
use crate::ids::{PropKey, SlotId, Tag, TargetId, TweenId};
use crate::spec::{
    Anchor, End, EventMask, KeyStep, Offset, Overwrite, Position, PropTo, Reduce, Targets,
    TimelineOpts, TweenOpts,
};
use crate::value::{ColorSpace, TweenValue, ValueKind};
use crate::{round7, splitmix64, BIG, INFINITE, TINY};

/// "No node" / "no track" / "no slot" in the intrusive links.
pub(crate) const NIL: u32 = u32::MAX;

// ---- node flags (Hot::flags) ----
/// Kind bits: a plain tween (zero-duration tweens, calls and pauses included).
pub(crate) const K_TWEEN: u32 = 0;
/// Kind bits: a timeline.
pub(crate) const K_TIMELINE: u32 = 1;
/// Kind bits: a stagger or keyframes group (a tween with an inner timeline).
pub(crate) const K_GROUP: u32 = 2;
pub(crate) const K_MASK: u32 = 3;
pub(crate) const F_INITTED: u32 = 1 << 2;
pub(crate) const F_ACT: u32 = 1 << 3;
pub(crate) const F_PAUSED: u32 = 1 << 4;
pub(crate) const F_KILLED: u32 = 1 << 5;
pub(crate) const F_LINKED: u32 = 1 << 6;
pub(crate) const F_DIRTY: u32 = 1 << 7;
pub(crate) const F_YOYO: u32 = 1 << 8;
pub(crate) const F_RATIO1: u32 = 1 << 9;
pub(crate) const F_LOCK_SHIFT: u32 = 10;
pub(crate) const F_LOCK: u32 = 3 << F_LOCK_SHIFT;
pub(crate) const F_FORCING: u32 = 1 << 12;
pub(crate) const F_HAS_PAUSE: u32 = 1 << 13;
pub(crate) const F_SMOOTH: u32 = 1 << 14;
pub(crate) const F_AUTO_REMOVE: u32 = 1 << 15;
pub(crate) const F_KEEP: u32 = 1 << 16;
pub(crate) const F_ROOT: u32 = 1 << 17;
/// Values were captured and written by an immediate render at creation, but
/// the tween is not initted yet (GSAP's `from()` / `fromTo()` start-at
/// render): initialisation (and overwrite Auto) waits for a render with
/// total time above 0.
pub(crate) const F_PRE: u32 = 1 << 18;
pub(crate) const F_REFRESH: u32 = 1 << 19;
pub(crate) const F_PAUSE_NODE: u32 = 1 << 20;
pub(crate) const F_CALL: u32 = 1 << 21;
pub(crate) const F_FREE: u32 = 1 << 23;
/// A group's inner timeline has rendered once.
pub(crate) const F_INNER_INIT: u32 = 1 << 24;
pub(crate) const F_IN_REAP: u32 = 1 << 25;

// ---- track flags (TrackMeta::flags) ----
pub(crate) const T_ALIVE: u8 = 1;
pub(crate) const T_RESOLVED: u8 = 2;
pub(crate) const T_SNAP: u8 = 4;

/// The record the child walk reads for every visited child: exactly one
/// 64-byte cache line.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Hot {
    /// Start in the parent's local time, delay included (round7).
    pub start: f64,
    /// Effective time scale: 0 while paused, negative when reversed.
    pub ts: f64,
    /// One iteration (a timeline's cached content duration).
    pub dur: f64,
    /// Total duration, repeats included.
    pub tdur: f64,
    /// Total time at the last render.
    pub ttime: f64,
    /// Iteration time at the last render (yoyo-mirrored).
    pub time: f64,
    /// Next sibling (sorted by start).
    pub next: u32,
    /// Previous sibling.
    pub prev: u32,
    /// Kind and state bits.
    pub flags: u32,
    /// 0-based iteration of the last render.
    pub iter: u32,
}

const _: () = assert!(std::mem::size_of::<Hot>() == 64);

/// Everything a node needs besides the walk data: touched by building,
/// controls, callbacks and by nodes that actually render.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cold {
    /// End in the parent's time (round7).
    pub end: f64,
    /// Requested time scale (kept while paused; negative = reversed).
    pub rts: f64,
    pub delay: f64,
    pub rdelay: f64,
    /// Total time recorded when paused.
    pub ptime: f64,
    /// GSAP `_zTime`: the "exactly at zero" bookkeeping.
    pub ztime: f64,
    /// A group's inner timeline time and zero-time bookkeeping.
    pub itime: f64,
    pub iztime: f64,
    /// A group's inner (content) duration.
    pub inner_dur: f64,
    pub repeat: i32,
    pub gen: u32,
    /// Linked parent (NIL when unlinked) and last parent (GSAP `_dp`).
    pub parent: u32,
    pub dp: u32,
    pub first: u32,
    pub last: u32,
    /// The most recently added child (GSAP `recent()`).
    pub recent: u32,
    /// First track and track count (contiguous range).
    pub tracks: u32,
    pub n_tracks: u32,
    pub live_tracks: u32,
    pub ease: Easing,
    pub yoyo: Option<YoyoEase>,
    /// A group's outer ease: inner time = inner_dur * remap(time / dur).
    pub remap: Easing,
    pub overwrite: Overwrite,
    pub reduce: Reduce,
    pub tag: Tag,
    pub events: EventMask,
    /// This node's share of the event reserve (section 6.3).
    pub weight: u64,
}

impl Cold {
    const fn new(gen: u32) -> Self {
        Cold {
            end: 0.0,
            rts: 1.0,
            delay: 0.0,
            rdelay: 0.0,
            ptime: 0.0,
            ztime: -TINY,
            itime: 0.0,
            iztime: -TINY,
            inner_dur: 0.0,
            repeat: 0,
            gen,
            parent: NIL,
            dp: NIL,
            first: NIL,
            last: NIL,
            recent: NIL,
            tracks: 0,
            n_tracks: 0,
            live_tracks: 0,
            ease: Easing::Linear,
            yoyo: None,
            remap: Easing::Linear,
            overwrite: Overwrite::None,
            reduce: Reduce::JumpToEnd,
            tag: Tag::NONE,
            events: EventMask::NONE,
            weight: 0,
        }
    }
}

/// Per-track metadata (4 bytes).
#[derive(Clone, Copy, Debug)]
pub(crate) struct TrackMeta {
    pub kind: ValueKind,
    pub flags: u8,
    pub space: ColorSpace,
    pub lanes: u8,
}

/// How a track's end is found at capture time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EndKind {
    To,
    By,
    Current,
}

/// The build-time description of a track, read only when it captures.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TrackSpec {
    pub from: Option<[f64; 4]>,
    pub end: EndKind,
    pub val: [f64; 4],
    pub snap: f64,
}

/// A timeline label.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Label {
    pub tl: u32,
    pub tag: Tag,
    pub time: f64,
}

/// Options resolved through the defaults chain (built-ins filled in).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Resolved {
    pub duration: f64,
    pub delay: f64,
    pub ease: Easing,
    pub explicit_ease: Option<Easing>,
    pub ease_each: Option<Easing>,
    pub yoyo_ease: Option<YoyoEase>,
    pub repeat: i32,
    pub repeat_delay: f64,
    pub repeat_refresh: bool,
    pub yoyo: bool,
    pub overwrite: Overwrite,
    pub immediate_render: Option<bool>,
    pub paused: bool,
    pub reversed: bool,
    pub time_scale: f64,
    pub space: ColorSpace,
    pub reduce: Reduce,
    pub keep: bool,
    pub tag: Tag,
    pub events: EventMask,
}

/// Where a new child goes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Place {
    /// A GSAP position resolved in the parent (timelines and groups).
    Pos(Position),
    /// The parent's current time (root-level building).
    Now,
}

/// The tween and timeline engine: GSAP's global timeline and every animation
/// in it.
///
/// Build with [`TweenEngine::to`], [`TweenEngine::timeline`] and friends
/// (GSAP `gsap.to` / `gsap.timeline`), drive with [`TweenEngine::advance`]
/// once per frame (GSAP's ticker), read animated values from the slot store
/// ([`TweenEngine::get`], [`TweenEngine::changes`]) and drain callbacks with
/// [`TweenEngine::swap_events`]. Control a built animation through
/// [`TweenEngine::anim`] and inspect it through [`TweenEngine::anim_ref`].
///
/// `advance`, every control and every getter are allocation-free once the
/// animations are built (see the crate docs for the precise guarantee).
pub struct TweenEngine {
    pub(crate) hot: Vec<Hot>,
    pub(crate) cold: Vec<Cold>,
    pub(crate) free_nodes: Vec<u32>,
    pub(crate) reap: Vec<u32>,
    // tracks (SoA)
    pub(crate) tr_from: Vec<[f64; 4]>,
    pub(crate) tr_to: Vec<[f64; 4]>,
    pub(crate) tr_slot: Vec<u32>,
    pub(crate) tr_meta: Vec<TrackMeta>,
    pub(crate) tr_spec: Vec<TrackSpec>,
    pub(crate) tr_node: Vec<u32>,
    pub(crate) tr_next_in_slot: Vec<u32>,
    pub(crate) dead_tracks: u32,
    // slot store
    pub(crate) sl_val: Vec<[f64; 4]>,
    pub(crate) sl_kind: Vec<ValueKind>,
    pub(crate) sl_target: Vec<TargetId>,
    pub(crate) sl_key: Vec<PropKey>,
    pub(crate) sl_first_track: Vec<u32>,
    pub(crate) sl_stamp: Vec<u64>,
    pub(crate) sl_seeded: Vec<bool>,
    pub(crate) sl_index: Vec<u32>,
    pub(crate) slot_gen: u32,
    pub(crate) labels: Vec<Label>,
    pub(crate) tl_defaults: Vec<(u32, TweenOpts)>,
    pub(crate) changed: Vec<SlotId>,
    pub(crate) change_gen: u64,
    pub(crate) events: Vec<TweenEvent>,
    pub(crate) event_weight: u64,
    pub(crate) build_scratch: Vec<f64>,
    pub(crate) root: u32,
    pub(crate) defaults: TweenOpts,
    pub(crate) stats: Stats,
}

impl Default for TweenEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TweenEngine {
    /// An empty engine (GSAP's global timeline with nothing on it). Only the
    /// root node is allocated.
    pub fn new() -> Self {
        Self::with_capacity(0, 0, 0)
    }

    /// An empty engine with room for `nodes` animations, `tracks` property
    /// tracks and `slots` value slots before any storage grows.
    pub fn with_capacity(nodes: usize, tracks: usize, slots: usize) -> Self {
        let nodes = nodes + 1;
        let mut e = TweenEngine {
            hot: Vec::with_capacity(nodes),
            cold: Vec::with_capacity(nodes),
            free_nodes: Vec::with_capacity(nodes),
            reap: Vec::with_capacity(nodes),
            tr_from: Vec::with_capacity(tracks),
            tr_to: Vec::with_capacity(tracks),
            tr_slot: Vec::with_capacity(tracks),
            tr_meta: Vec::with_capacity(tracks),
            tr_spec: Vec::with_capacity(tracks),
            tr_node: Vec::with_capacity(tracks),
            tr_next_in_slot: Vec::with_capacity(tracks),
            dead_tracks: 0,
            sl_val: Vec::with_capacity(slots),
            sl_kind: Vec::with_capacity(slots),
            sl_target: Vec::with_capacity(slots),
            sl_key: Vec::with_capacity(slots),
            sl_first_track: Vec::with_capacity(slots),
            sl_stamp: Vec::with_capacity(slots),
            sl_seeded: Vec::with_capacity(slots),
            sl_index: Vec::new(),
            slot_gen: 0,
            labels: Vec::new(),
            tl_defaults: Vec::new(),
            changed: Vec::with_capacity(slots),
            change_gen: 1,
            events: Vec::new(),
            event_weight: 0,
            build_scratch: Vec::new(),
            root: 0,
            defaults: TweenOpts::new(),
            stats: Stats::default(),
        };
        let root = e.alloc_node(K_TIMELINE);
        e.hot[root as usize].flags |= F_ROOT | F_SMOOTH | F_AUTO_REMOVE | F_KEEP;
        e.root = root;
        if slots > 0 {
            e.sl_index = vec![NIL; (slots * 2).next_power_of_two()];
        }
        e
    }

    /// GSAP `gsap.defaults()`: options every new tween inherits (after its
    /// enclosing timelines' `defaults`). The built-ins behind it are GSAP's:
    /// duration 0.5, ease `power1.out` ([`Easing::OutQuad`]), no overwrite.
    pub fn set_defaults(&mut self, d: TweenOpts) {
        self.defaults = d;
    }

    /// The options set with [`TweenEngine::set_defaults`].
    pub fn defaults(&self) -> TweenOpts {
        self.defaults
    }

    // ------------------------------------------------------------------
    // Handles and node storage
    // ------------------------------------------------------------------

    /// The node a handle refers to, if it is still alive.
    #[inline]
    pub(crate) fn node(&self, id: TweenId) -> Option<u32> {
        let ix = id.ix;
        if ix == NIL || ix as usize >= self.cold.len() || ix == self.root {
            return None;
        }
        let c = &self.cold[ix as usize];
        if c.gen != id.gen || self.hot[ix as usize].flags & (F_FREE | F_KILLED) != 0 {
            return None;
        }
        Some(ix)
    }

    /// The handle of a live node.
    #[inline]
    pub(crate) fn id_of(&self, n: u32) -> TweenId {
        TweenId {
            ix: n,
            gen: self.cold[n as usize].gen,
        }
    }

    #[inline]
    pub(crate) fn kind(&self, n: u32) -> u32 {
        self.hot[n as usize].flags & K_MASK
    }

    #[inline]
    pub(crate) fn has(&self, n: u32, f: u32) -> bool {
        self.hot[n as usize].flags & f != 0
    }

    #[inline]
    pub(crate) fn set_flag(&mut self, n: u32, f: u32, on: bool) {
        let h = &mut self.hot[n as usize];
        if on {
            h.flags |= f;
        } else {
            h.flags &= !f;
        }
    }

    #[inline]
    pub(crate) fn lock(&self, n: u32) -> u32 {
        (self.hot[n as usize].flags & F_LOCK) >> F_LOCK_SHIFT
    }

    #[inline]
    pub(crate) fn set_lock(&mut self, n: u32, l: u32) {
        let h = &mut self.hot[n as usize];
        h.flags = (h.flags & !F_LOCK) | (l << F_LOCK_SHIFT);
    }

    pub(crate) fn alloc_node(&mut self, kind: u32) -> u32 {
        let hot = Hot {
            next: NIL,
            prev: NIL,
            ts: 1.0,
            flags: kind,
            ..Hot::default()
        };
        let n = if let Some(n) = self.free_nodes.pop() {
            let gen = self.cold[n as usize].gen;
            self.hot[n as usize] = hot;
            self.cold[n as usize] = Cold::new(gen);
            n
        } else {
            let n = self.hot.len() as u32;
            self.hot.push(hot);
            self.cold.push(Cold::new(1));
            // Keep the reclamation lists able to hold every node without growing.
            let cap = self.hot.capacity();
            if self.free_nodes.capacity() < cap {
                self.free_nodes.reserve(cap - self.free_nodes.len());
            }
            if self.reap.capacity() < cap {
                self.reap.reserve(cap - self.reap.len());
            }
            n
        };
        self.stats.live_nodes += 1;
        n
    }

    // ------------------------------------------------------------------
    // Options
    // ------------------------------------------------------------------

    /// GSAP `_inheritDefaults`: the call's options, then each enclosing
    /// timeline's `defaults` innermost first (unless `inherit: false`), then
    /// the engine defaults, then the built-ins.
    pub(crate) fn resolve_opts(&self, parent: u32, o: &TweenOpts) -> Resolved {
        let mut r = *o;
        if o.inherit != Some(false) {
            let mut p = parent;
            while p != NIL {
                if let Some(d) = self.tl_defaults.iter().find(|d| d.0 == p) {
                    r = r.or(&d.1);
                }
                let c = &self.cold[p as usize];
                p = if c.parent != NIL { c.parent } else { c.dp };
            }
        }
        r = r.or(&self.defaults);
        let ease = r.ease.unwrap_or_default();
        let repeat = r.repeat.unwrap_or(0);
        Resolved {
            duration: r.duration.unwrap_or(0.5).max(0.0),
            delay: r.delay.unwrap_or(0.0),
            ease,
            explicit_ease: o.ease,
            ease_each: r.ease_each,
            yoyo_ease: r.yoyo_ease,
            repeat,
            repeat_delay: r.repeat_delay.unwrap_or(0.0).max(0.0),
            repeat_refresh: r.repeat_refresh.unwrap_or(false),
            yoyo: r.yoyo.unwrap_or(false) || r.yoyo_ease.is_some(),
            overwrite: r.overwrite.unwrap_or_default(),
            immediate_render: r.immediate_render,
            paused: r.paused.unwrap_or(false),
            reversed: r.reversed.unwrap_or(false),
            time_scale: r.time_scale.unwrap_or(1.0),
            space: r.color_space.unwrap_or_default(),
            reduce: r.reduce.unwrap_or_default(),
            keep: r.keep.unwrap_or(false),
            tag: o.tag,
            events: o.events,
        }
    }

    // ------------------------------------------------------------------
    // Building (GSAP gsap.to / from / fromTo / set / delayedCall / timeline)
    // ------------------------------------------------------------------

    /// GSAP `gsap.to()` (and `from` / `fromTo`, which differ only in their
    /// props): a tween of `props` on `targets`, placed on the root at its
    /// current time plus `delay`. A `stagger` builds a staggered group,
    /// [`End::Keys`] / [`End::Values`] props build a keyframes group.
    pub fn tween(&mut self, t: Targets, props: &[PropTo], o: TweenOpts) -> TweenId {
        let root = self.root;
        let n = self.build_tween(root, t, props, &o, Place::Now);
        self.finish_build();
        self.id_of(n)
    }

    /// GSAP `gsap.to()`: from the current values to the given ends.
    pub fn to(&mut self, t: Targets, props: &[PropTo], o: TweenOpts) -> TweenId {
        debug_assert!(
            props.iter().all(|p| p.from.is_none()),
            "to(): props must not carry an explicit from (use from_to)"
        );
        self.tween(t, props, o)
    }

    /// GSAP `gsap.from()`: from the given values to the current ones. Renders
    /// the start values at once (immediate render) unless told otherwise.
    pub fn from(&mut self, t: Targets, props: &[PropTo], o: TweenOpts) -> TweenId {
        debug_assert!(
            props
                .iter()
                .all(|p| p.from.is_some() && p.to == End::Current),
            "from(): every prop needs PropTo::from"
        );
        self.tween(t, props, o)
    }

    /// GSAP `gsap.fromTo()`: explicit start and end values. Renders the start
    /// values at once unless told otherwise.
    pub fn from_to(&mut self, t: Targets, props: &[PropTo], o: TweenOpts) -> TweenId {
        debug_assert!(
            props.iter().all(|p| p.from.is_some()),
            "from_to(): every prop needs an explicit from"
        );
        self.tween(t, props, o)
    }

    /// GSAP `gsap.set()`: a zero-duration tween that applies its values at
    /// once (repeat is ignored).
    pub fn set(&mut self, t: Targets, props: &[PropTo], o: TweenOpts) -> TweenId {
        let o = o.duration(0.0).repeat(0);
        self.tween(t, props, o)
    }

    /// GSAP array keyframes (`keyframes: [{..}, {..}]`): the steps run back
    /// to back on `targets`, each with its own props and options (its ease
    /// defaults to linear, GSAP `"none"`); `o.ease` eases the whole run.
    pub fn keyframes(&mut self, t: Targets, steps: &[KeyStep], o: TweenOpts) -> TweenId {
        let root = self.root;
        let n = self.build_keyframes(root, t, steps, &o, Place::Now);
        self.finish_build();
        self.id_of(n)
    }

    /// GSAP `gsap.delayedCall()`: reports [`EventKind::Call`] with `tag`
    /// after `delay` seconds.
    pub fn delayed_call(&mut self, delay: f64, tag: Tag) -> TweenId {
        let root = self.root;
        let n = self.build_marker(root, F_CALL, tag, delay, Place::Now);
        self.finish_build();
        self.id_of(n)
    }

    /// GSAP `gsap.timeline()`: an empty timeline on the root at its current
    /// time plus `delay`. Fill it through [`TweenEngine::tl`].
    pub fn timeline(&mut self, o: TimelineOpts) -> TweenId {
        let root = self.root;
        let n = self.build_timeline(root, &o, Place::Now);
        self.finish_build();
        self.id_of(n)
    }

    /// Housekeeping after every building call: reclaim what an immediate
    /// render completed, then top up the event reserve.
    pub(crate) fn finish_build(&mut self) {
        self.flush_reap();
        self.reserve_events();
    }

    pub(crate) fn reserve_events(&mut self) {
        let want = (self.event_weight.saturating_mul(2).saturating_add(16)).min(1 << 26) as usize;
        if self.events.capacity() < want {
            self.events.reserve(want - self.events.len());
        }
    }

    pub(crate) fn build_timeline(&mut self, parent: u32, o: &TimelineOpts, place: Place) -> u32 {
        let n = self.alloc_node(K_TIMELINE);
        {
            let c = &mut self.cold[n as usize];
            c.delay = o.delay;
            c.repeat = o.repeat;
            c.rdelay = o.repeat_delay.max(0.0);
            c.reduce = o.reduce;
            c.tag = o.tag;
            c.events = o.events;
            c.rts = o.time_scale;
        }
        let h = &mut self.hot[n as usize];
        h.ts = o.time_scale;
        if o.yoyo {
            h.flags |= F_YOYO;
        }
        if o.repeat_refresh {
            h.flags |= F_REFRESH;
        }
        if o.smooth_child_timing {
            h.flags |= F_SMOOTH;
        }
        if o.auto_remove_children {
            h.flags |= F_AUTO_REMOVE;
        }
        if o.keep {
            h.flags |= F_KEEP;
        }
        if o.defaults != TweenOpts::new() {
            self.tl_defaults.push((n, o.defaults));
        }
        self.set_duration_raw(n, 0.0);
        let at = self.place_time(parent, place, None);
        self.add_child(parent, n, at, false);
        if o.reversed {
            self.set_reversed(n, true);
            self.set_paused(n, false);
        }
        if o.paused {
            self.set_paused(n, true);
        }
        n
    }

    /// A call or pause marker: a zero-duration node that only reports.
    pub(crate) fn build_marker(
        &mut self,
        parent: u32,
        what: u32,
        tag: Tag,
        delay: f64,
        place: Place,
    ) -> u32 {
        let n = self.alloc_node(K_TWEEN);
        self.hot[n as usize].flags |= what;
        {
            let c = &mut self.cold[n as usize];
            c.tag = tag;
            c.delay = delay;
        }
        self.set_duration_raw(n, 0.0);
        let at = self.place_time(parent, place, None);
        self.add_child(parent, n, at, false);
        n
    }

    /// The parent-local time a [`Place`] resolves to (before the child's delay).
    pub(crate) fn place_time(&mut self, parent: u32, place: Place, child: Option<u32>) -> f64 {
        match place {
            Place::Now => {
                self.total_duration(parent);
                self.hot[parent as usize].time
            }
            Place::Pos(p) => self.resolve_position(parent, p, child),
        }
    }

    /// Builds one tween call under `parent`: a leaf, a stagger group or a
    /// keyframes group.
    pub(crate) fn build_tween(
        &mut self,
        parent: u32,
        t: Targets,
        props: &[PropTo],
        o: &TweenOpts,
        place: Place,
    ) -> u32 {
        let r = self.resolve_opts(parent, o);
        if let Some(st) = o.stagger.or(r_stagger(self, parent, o)) {
            let n = t.len();
            if n > 0 {
                if let Some(g) = self.build_stagger(parent, t, props, o, &r, st, place) {
                    return g;
                }
            }
        }
        if props
            .iter()
            .any(|p| matches!(p.to, End::Keys(_) | End::Values(_)))
        {
            return self.build_percent_keyframes(parent, t, props, o, &r, place);
        }
        self.build_leaf(parent, t, 0, props, &r, place)
    }

    /// A plain tween node on `t` (target `j` resolves `End::Each` with index
    /// `index_base + j`).
    pub(crate) fn build_leaf(
        &mut self,
        parent: u32,
        t: Targets,
        index_base: u32,
        props: &[PropTo],
        r: &Resolved,
        place: Place,
    ) -> u32 {
        let n = self.alloc_node(K_TWEEN);
        self.setup_tween_node(n, r);
        let dur = r.duration;
        self.push_tracks(n, t, index_base, props, r.space);
        self.set_duration_raw(n, dur);
        self.link_new_tween(parent, n, t, props, r, place, dur == 0.0);
        n
    }

    fn setup_tween_node(&mut self, n: u32, r: &Resolved) {
        let c = &mut self.cold[n as usize];
        c.delay = r.delay;
        c.repeat = r.repeat;
        c.rdelay = r.repeat_delay;
        c.ease = r.ease;
        c.yoyo = r.yoyo_ease;
        c.overwrite = r.overwrite;
        c.reduce = r.reduce;
        c.tag = r.tag;
        c.events = r.events;
        c.rts = r.time_scale;
        let h = &mut self.hot[n as usize];
        h.ts = r.time_scale;
        if r.yoyo {
            h.flags |= F_YOYO;
        }
        if r.repeat_refresh {
            h.flags |= F_REFRESH;
        }
        if r.keep {
            h.flags |= F_KEEP;
        }
    }

    /// Links a freshly built tween or group, applies overwrite `All`,
    /// `reversed` / `paused`, and performs the immediate render.
    #[allow(clippy::too_many_arguments)]
    fn link_new_tween(
        &mut self,
        parent: u32,
        n: u32,
        t: Targets,
        props: &[PropTo],
        r: &Resolved,
        place: Place,
        zero: bool,
    ) {
        if r.overwrite == Overwrite::All {
            self.overwrite_all(n, t);
        }
        let at = self.place_time(parent, place, Some(n));
        self.add_child(parent, n, at, false);
        if r.reversed {
            self.set_reversed(n, true);
            self.set_paused(n, false);
        }
        if r.paused {
            self.set_paused(n, true);
        }
        let has_from = props.iter().any(|p| p.from.is_some());
        let immediate = r.immediate_render.unwrap_or(has_from);
        let parent_is_group = self.kind(parent) == K_GROUP;
        if zero {
            // GSAP: a zero-duration tween at the parent's playhead renders
            // immediately unless immediateRender is false or anything is paused.
            let at_playhead = self.hot[n as usize].start == round7(self.hot[parent as usize].time);
            let fire = r.immediate_render == Some(true)
                || (r.immediate_render != Some(false)
                    && at_playhead
                    && !parent_is_group
                    && self.no_paused_ancestors(n));
            if fire {
                let t0 = (-r.delay).max(0.0);
                self.render(n, t0, false, false, NIL);
            }
        } else if immediate {
            self.pre_render(n, props);
        }
    }

    /// The creation render of an `immediate_render` tween: capture and write
    /// the start values with events suppressed (design D22). A `from()` /
    /// `fromTo()` stays un-initted (GSAP renders its start-at tween), so its
    /// overwrite waits for the first real render; `to()` inits at once.
    fn pre_render(&mut self, n: u32, props: &[PropTo]) {
        if self.kind(n) == K_GROUP {
            return; // children pre-rendered themselves
        }
        let from_like = props.iter().any(|p| p.from.is_some());
        if from_like {
            let (s, e) = self.track_range(n);
            for k in s..e {
                self.resolve_track(k);
            }
            self.write_tracks(n, 0.0, 0.0);
            let h = &mut self.hot[n as usize];
            h.flags |= F_PRE;
            h.ttime = 0.0;
            h.time = 0.0;
        } else {
            // GSAP forces the render past the "same time" test with _tTime = -tiny.
            self.hot[n as usize].ttime = -TINY;
            self.render(n, 0.0, true, false, NIL);
        }
    }

    pub(crate) fn no_paused_ancestors(&self, n: u32) -> bool {
        let mut x = n;
        while x != NIL {
            if self.hot[x as usize].ts == 0.0 {
                return false;
            }
            x = self.cold[x as usize].parent;
        }
        true
    }

    /// Adds the tracks of `props` on every target of `t` to node `n`.
    fn push_tracks(
        &mut self,
        n: u32,
        t: Targets,
        index_base: u32,
        props: &[PropTo],
        space: ColorSpace,
    ) {
        let first = self.tr_slot.len() as u32;
        let mut count = 0u32;
        for j in 0..t.len() {
            let target = t.get(j);
            for p in props {
                let (end, val) = match p.to {
                    End::To(v) => (EndKind::To, v),
                    End::By(v) => (EndKind::By, v),
                    End::Current => (EndKind::Current, p.from.unwrap_or(TweenValue::F64(0.0))),
                    End::Each(vs) => {
                        if vs.is_empty() {
                            continue;
                        }
                        let i = ((index_base + j) as usize).min(vs.len() - 1);
                        (EndKind::To, vs[i])
                    }
                    End::Keys(_) | End::Values(_) => continue,
                };
                let kind = val.kind();
                let slot = self.slot_for(target, p.key, kind);
                let k = self.tr_slot.len() as u32;
                let lanes = match kind {
                    ValueKind::F64 | ValueKind::Int => 1,
                    ValueKind::Vec2 => 2,
                    ValueKind::Vec3 => 3,
                    ValueKind::Vec4 | ValueKind::Color => 4,
                };
                let mut flags = T_ALIVE;
                if p.snap > 0.0 {
                    flags |= T_SNAP;
                }
                self.tr_from.push([0.0; 4]);
                self.tr_to.push([0.0; 4]);
                self.tr_slot.push(slot);
                self.tr_meta.push(TrackMeta {
                    kind,
                    flags,
                    space,
                    lanes,
                });
                self.tr_spec.push(TrackSpec {
                    from: p.from.map(|f| f.to_lanes()),
                    end,
                    val: val.to_lanes(),
                    snap: p.snap,
                });
                self.tr_node.push(n);
                self.tr_next_in_slot
                    .push(self.sl_first_track[slot as usize]);
                self.sl_first_track[slot as usize] = k;
                count += 1;
            }
        }
        let c = &mut self.cold[n as usize];
        c.tracks = first;
        c.n_tracks = count;
        c.live_tracks = count;
    }

    /// A staggered group (GSAP `stagger`): one child tween per target at its
    /// `distribute` delay. `None` when every delay and the duration are 0
    /// (GSAP then drops the inner timeline and builds a plain tween).
    #[allow(clippy::too_many_arguments)]
    fn build_stagger(
        &mut self,
        parent: u32,
        t: Targets,
        props: &[PropTo],
        o: &TweenOpts,
        r: &Resolved,
        st: crate::stagger::Stagger,
        place: Place,
    ) -> Option<u32> {
        let n = t.len();
        let mut delays = std::mem::take(&mut self.build_scratch);
        delays.clear();
        delays.resize(n as usize, 0.0);
        st.delays(n, &mut delays);
        let has_keys = props
            .iter()
            .any(|p| matches!(p.to, End::Keys(_) | End::Values(_)));
        if r.duration == 0.0 && !has_keys && delays.iter().all(|d| *d == 0.0) {
            self.build_scratch = delays;
            return None;
        }
        let g = self.alloc_node(K_GROUP);
        let mut gr = *r;
        gr.ease = Easing::Linear;
        self.setup_tween_node(g, &gr);
        self.cold[g as usize].yoyo = r.yoyo_ease;
        self.cold[g as usize].remap = Easing::Linear;
        // Child options: everything but the group-level ones (GSAP
        // _staggerPropsToSkip), plus the stagger's own repeat settings.
        let mut co = *o;
        co.stagger = None;
        co.delay = Some(0.0);
        co.repeat = st.repeat;
        co.yoyo = st.yoyo;
        co.repeat_delay = st.repeat_delay;
        co.yoyo_ease = None;
        co.repeat_refresh = None;
        co.paused = None;
        co.reversed = None;
        co.time_scale = None;
        co.tag = Tag::NONE;
        co.events = EventMask::NONE;
        co.keep = Some(true);
        co.inherit = Some(false);
        let mut cr = *r;
        cr.delay = 0.0;
        cr.repeat = st.repeat.unwrap_or(0);
        cr.yoyo = st.yoyo.unwrap_or(false);
        cr.repeat_delay = st.repeat_delay.unwrap_or(0.0).max(0.0);
        cr.yoyo_ease = None;
        cr.repeat_refresh = false;
        cr.paused = false;
        cr.reversed = false;
        cr.time_scale = 1.0;
        cr.tag = Tag::NONE;
        cr.events = EventMask::NONE;
        cr.keep = true;
        for i in 0..n {
            let one = Targets::One(t.get(i));
            let pos = Place::Pos(Position::at(delays[i as usize]));
            if has_keys {
                self.build_percent_keyframes_indexed(g, one, i, props, &co, &cr, pos);
            } else {
                self.build_leaf(g, one, i, props, &cr, pos);
            }
        }
        self.build_scratch = delays;
        let inner = self.children_end(g);
        self.cold[g as usize].inner_dur = inner;
        self.set_duration_raw(g, inner);
        self.link_new_tween(parent, g, t, props, &gr, place, false);
        Some(g)
    }

    /// The largest end among `g`'s linked, unpaused children.
    pub(crate) fn children_end(&self, g: u32) -> f64 {
        let mut max = 0.0f64;
        let mut c = self.cold[g as usize].first;
        while c != NIL {
            let h = &self.hot[c as usize];
            if h.ts != 0.0 && h.flags & F_KILLED == 0 {
                max = max.max(self.cold[c as usize].end);
            }
            c = h.next;
        }
        max
    }

    fn build_percent_keyframes(
        &mut self,
        parent: u32,
        t: Targets,
        props: &[PropTo],
        o: &TweenOpts,
        r: &Resolved,
        place: Place,
    ) -> u32 {
        self.build_percent_keyframes_indexed(parent, t, 0, props, o, r, place)
    }

    /// GSAP object keyframes (`{"0%": .., "50%": ..}`, `x: [..]`): per
    /// property, one child tween per key at `prev_at / 100 * duration`;
    /// ordinary props are the group's own tracks, eased by the outer ease.
    #[allow(clippy::too_many_arguments)]
    fn build_percent_keyframes_indexed(
        &mut self,
        parent: u32,
        t: Targets,
        index_base: u32,
        props: &[PropTo],
        o: &TweenOpts,
        r: &Resolved,
        place: Place,
    ) -> u32 {
        let g = self.alloc_node(K_GROUP);
        let outer = o.ease.unwrap_or(Easing::Linear);
        let mut gr = *r;
        gr.ease = outer;
        self.setup_tween_node(g, &gr);
        self.cold[g as usize].remap = outer;
        let dur = r.duration;
        let each = r.ease_each.unwrap_or(Easing::InOutQuad);
        let mut kr = *r;
        kr.delay = 0.0;
        kr.repeat = 0;
        kr.repeat_delay = 0.0;
        kr.yoyo = false;
        kr.yoyo_ease = None;
        kr.repeat_refresh = false;
        kr.paused = false;
        kr.reversed = false;
        kr.time_scale = 1.0;
        kr.tag = Tag::NONE;
        kr.events = EventMask::NONE;
        kr.keep = true;
        kr.immediate_render = Some(false);
        let mut order: Vec<(f64, usize)> = Vec::new();
        for p in props {
            order.clear();
            match p.to {
                End::Keys(ks) => order.extend(ks.iter().enumerate().map(|(i, k)| (k.at, i))),
                End::Values(vs) => {
                    let last = vs.len().saturating_sub(1).max(1) as f64;
                    order.extend(
                        (0..vs.len()).map(|i| (if vs.len() == 1 { 100.0 } else { i as f64 / last * 100.0 }, i)),
                    );
                }
                _ => continue,
            }
            order.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut time = 0.0;
            let mut prev_at = 0.0;
            for &(at, i) in order.iter() {
                let (value, ease) = match p.to {
                    End::Keys(ks) => (ks[i].value, ks[i].ease.unwrap_or(each)),
                    End::Values(vs) => (vs[i], each),
                    _ => unreachable!(),
                };
                let d = (at - prev_at) / 100.0 * dur;
                let mut rr = kr;
                rr.duration = d.max(0.0);
                rr.ease = ease;
                let prop = PropTo {
                    key: p.key,
                    from: None,
                    to: End::To(value),
                    snap: p.snap,
                };
                self.build_leaf(
                    g,
                    t,
                    index_base,
                    std::slice::from_ref(&prop),
                    &rr,
                    Place::Pos(Position::at(time)),
                );
                time += rr.duration;
                prev_at = at;
            }
        }
        // Ordinary props: the group's own tracks.
        let first = self.tr_slot.len();
        self.push_tracks(g, t, index_base, props, r.space);
        debug_assert!(self.cold[g as usize].tracks as usize == first);
        let inner = self.children_end(g).max(dur);
        self.cold[g as usize].inner_dur = inner;
        self.set_duration_raw(g, dur);
        self.link_new_tween(parent, g, t, props, &gr, place, false);
        g
    }

    /// GSAP array keyframes: each step is a child tween at `">"`.
    pub(crate) fn build_keyframes(
        &mut self,
        parent: u32,
        t: Targets,
        steps: &[KeyStep],
        o: &TweenOpts,
        place: Place,
    ) -> u32 {
        let r = self.resolve_opts(parent, o);
        let g = self.alloc_node(K_GROUP);
        let outer = o.ease.unwrap_or(Easing::Linear);
        let mut gr = r;
        gr.ease = outer;
        self.setup_tween_node(g, &gr);
        self.cold[g as usize].remap = outer;
        // Steps inherit {ease: none}, then the enclosing chain (GSAP puts the
        // keyframe defaults on the inner timeline).
        let inner_defaults = TweenOpts::new().ease(Easing::Linear);
        for s in steps {
            let so = s.opts.or(&inner_defaults);
            let mut sr = self.resolve_opts(parent, &so);
            sr.keep = true;
            sr.paused = false;
            sr.reversed = false;
            sr.overwrite = Overwrite::None;
            let pos = Place::Pos(Position::prev_end(0.0));
            let has_keys = s
                .props
                .iter()
                .any(|p| matches!(p.to, End::Keys(_) | End::Values(_)));
            if has_keys {
                self.build_percent_keyframes_indexed(g, t, 0, s.props, &so, &sr, pos);
            } else {
                self.build_leaf(g, t, 0, s.props, &sr, pos);
            }
        }
        let inner = self.children_end(g);
        self.cold[g as usize].inner_dur = inner;
        let dur = o.duration.unwrap_or(inner).max(0.0);
        self.set_duration_raw(g, dur);
        self.link_new_tween(parent, g, t, &[], &gr, place, false);
        g
    }

    // ------------------------------------------------------------------
    // Insertion (GSAP _addToTimeline / _postAddChecks / remove)
    // ------------------------------------------------------------------

    /// GSAP `_addToTimeline`: (re)parents `c` into `tl` at parent time `at`
    /// (its delay is added), sorted by start, and makes it `recent`.
    pub(crate) fn add_child(&mut self, tl: u32, c: u32, at: f64, skip_checks: bool) {
        if self.has(c, F_LINKED) {
            self.unlink(c);
        }
        let delay = self.cold[c as usize].delay;
        let start = round7(at + delay);
        self.hot[c as usize].start = start;
        let tdur = self.total_duration(c);
        let rts = self.time_scale_of(c).abs();
        let end = if rts > 0.0 {
            round7(start + tdur / rts)
        } else {
            round7(start + tdur / TINY)
        };
        self.cold[c as usize].end = end;
        self.insert_sorted(tl, c);
        self.cold[tl as usize].recent = c;
        self.update_weights(c);
        if !skip_checks {
            self.post_add_checks(tl, c);
        }
        if self.hot[tl as usize].ts < 0.0 {
            let tt = self.hot[tl as usize].ttime;
            self.align(tl, tt);
        }
    }

    /// Links `c` into `tl`'s child list after the last child with
    /// `start <= c.start` (equal starts keep insertion order).
    pub(crate) fn insert_sorted(&mut self, tl: u32, c: u32) {
        let start = self.hot[c as usize].start;
        let mut prev = self.cold[tl as usize].last;
        while prev != NIL && self.hot[prev as usize].start > start {
            prev = self.hot[prev as usize].prev;
        }
        let next = if prev != NIL {
            self.hot[prev as usize].next
        } else {
            self.cold[tl as usize].first
        };
        if prev != NIL {
            self.hot[prev as usize].next = c;
        } else {
            self.cold[tl as usize].first = c;
        }
        if next != NIL {
            self.hot[next as usize].prev = c;
        } else {
            self.cold[tl as usize].last = c;
        }
        let h = &mut self.hot[c as usize];
        h.prev = prev;
        h.next = next;
        h.flags |= F_LINKED;
        let cc = &mut self.cold[c as usize];
        cc.parent = tl;
        cc.dp = tl;
    }

    /// Removes `c` from its parent's child list (the parent keeps `dp`).
    pub(crate) fn unlink_raw(&mut self, c: u32) {
        let p = self.cold[c as usize].parent;
        if p == NIL {
            return;
        }
        let (prev, next) = {
            let h = &self.hot[c as usize];
            (h.prev, h.next)
        };
        if prev != NIL {
            self.hot[prev as usize].next = next;
        } else if self.cold[p as usize].first == c {
            self.cold[p as usize].first = next;
        }
        if next != NIL {
            self.hot[next as usize].prev = prev;
        } else if self.cold[p as usize].last == c {
            self.cold[p as usize].last = prev;
        }
        let h = &mut self.hot[c as usize];
        h.next = NIL;
        h.prev = NIL;
        h.flags &= !F_LINKED;
        self.cold[c as usize].parent = NIL;
    }

    /// GSAP `timeline.remove(child)`: unlinks, fixes `recent` and marks the
    /// parent's duration dirty.
    pub(crate) fn unlink(&mut self, c: u32) {
        let p = self.cold[c as usize].parent;
        if p == NIL {
            return;
        }
        self.unlink_raw(c);
        if self.cold[p as usize].recent == c {
            self.cold[p as usize].recent = self.cold[p as usize].last;
        }
        self.uncache(p);
    }

    /// GSAP `_removeFromParent(child, onlyIfParentHasAutoRemove)`: always
    /// clears ACT; unlinks when the parent auto-removes (or unconditionally).
    /// A removed node that is not kept is reclaimed after the render.
    pub(crate) fn remove_from_parent(&mut self, c: u32, only_if_auto_remove: bool) {
        let p = self.cold[c as usize].parent;
        if p != NIL && (!only_if_auto_remove || self.has(p, F_AUTO_REMOVE)) {
            self.unlink(c);
            if !self.has(c, F_KEEP) {
                self.push_reap(c);
            }
        }
        self.set_flag(c, F_ACT, false);
    }

    pub(crate) fn push_reap(&mut self, n: u32) {
        if !self.has(n, F_IN_REAP) {
            self.set_flag(n, F_IN_REAP, true);
            self.reap.push(n);
        }
    }

    /// GSAP `_postAddChecks`: renders a child inserted behind the playhead
    /// (events suppressed) and re-enables a completed ancestor that grew.
    pub(crate) fn post_add_checks(&mut self, tl: u32, c: u32) {
        let ch = self.hot[c as usize];
        let tlh = self.hot[tl as usize];
        let is_tl = ch.flags & K_MASK == K_TIMELINE;
        if ch.time != 0.0
            || (ch.dur == 0.0 && ch.flags & F_INITTED != 0)
            || (ch.start < tlh.time && (ch.dur != 0.0 || !is_tl))
        {
            let raw = self.raw_time(tl);
            let t = self.parent_to_child(raw, c);
            let tdur = self.total_duration(c);
            if ch.dur == 0.0 || t.clamp(0.0, tdur) - self.hot[c as usize].ttime > TINY {
                self.render(c, t, true, false, NIL);
            }
        }
        // _uncache(timeline, child): only when the child extends it.
        let (cend, cstart) = (self.cold[c as usize].end, self.hot[c as usize].start);
        if cend > self.hot[tl as usize].dur || cstart < 0.0 {
            self.uncache(tl);
        }
        let tlh = self.hot[tl as usize];
        if self.cold[tl as usize].dp != NIL
            && tlh.flags & F_INITTED != 0
            && tlh.time >= tlh.dur
            && tlh.ts != 0.0
        {
            let old = tlh.dur;
            self.total_duration(tl);
            if old < self.hot[tl as usize].dur {
                let mut x = tl;
                while self.cold[x as usize].dp != NIL {
                    if self.raw_time(x) >= 0.0 {
                        let tt = self.hot[x as usize].ttime;
                        self.set_total_time(x, tt, false);
                    }
                    x = self.cold[x as usize].dp;
                }
            }
            self.cold[tl as usize].ztime = -TINY;
        }
    }

    // ------------------------------------------------------------------
    // Positions (GSAP _parsePosition)
    // ------------------------------------------------------------------

    /// GSAP `_parsePosition`: the parent-local time a [`Position`] names, for
    /// inserting `child` (whose total duration percent offsets use).
    pub(crate) fn resolve_position(&mut self, tl: u32, p: Position, child: Option<u32>) -> f64 {
        self.total_duration(tl);
        let dur = self.hot[tl as usize].dur;
        let recent = self.cold[tl as usize].recent;
        let clipped = if dur >= BIG {
            if recent == NIL {
                0.0
            } else {
                self.end_time(recent, false)
            }
        } else {
            dur
        };
        let recent_tdur = if recent == NIL {
            0.0
        } else {
            self.total_duration(recent)
        };
        let child_tdur = child.map(|c| self.total_duration(c));
        let base = match p.anchor {
            Anchor::Zero => 0.0,
            Anchor::End => clipped,
            Anchor::PrevStart => {
                if recent == NIL {
                    0.0
                } else {
                    self.hot[recent as usize].start
                }
            }
            Anchor::PrevEnd => {
                if recent == NIL {
                    0.0
                } else {
                    let inc = self.cold[recent as usize].repeat >= 0;
                    self.end_time(recent, inc)
                }
            }
            Anchor::Label(l) => match self.label_time(tl, l) {
                Some(t) => t,
                None => {
                    self.labels.push(Label {
                        tl,
                        tag: l,
                        time: clipped,
                    });
                    clipped
                }
            },
        };
        let off = match p.offset {
            Offset::Secs(x) => x,
            Offset::PercentOfPrev(pc) => pc / 100.0 * recent_tdur,
            // With no child (add_label) a percent of the child is 0. GSAP reads
            // "+=50%" as 50 seconds there and throws on "<+=50%" (deviation).
            Offset::PercentOfChild(pc) => child_tdur.map_or(0.0, |d| pc / 100.0 * d),
        };
        base + off
    }

    /// GSAP `endTime(includeRepeats)`.
    pub(crate) fn end_time(&self, n: u32, include_repeats: bool) -> f64 {
        let h = &self.hot[n as usize];
        let d = if include_repeats {
            self.pure_total_duration(n)
        } else {
            self.pure_duration(n)
        };
        let ts = if h.ts == 0.0 { 1.0 } else { h.ts.abs() };
        h.start + d / ts
    }

    // ------------------------------------------------------------------
    // Durations (GSAP _setDuration / _setEnd / totalDuration)
    // ------------------------------------------------------------------

    /// The total duration of `n` for a node count as: repeats included.
    pub(crate) fn tdur_for(dur: f64, repeat: i32, rdelay: f64, leaf: bool) -> f64 {
        if repeat == 0 || (dur == 0.0 && (leaf || rdelay == 0.0)) {
            dur
        } else if repeat < 0 {
            INFINITE
        } else {
            round7(dur * (repeat as f64 + 1.0) + rdelay * repeat as f64)
        }
    }

    /// Sets one iteration's duration and the derived total duration and end,
    /// without touching the playhead (GSAP `_setDuration(.., 1, 1)`).
    pub(crate) fn set_duration_raw(&mut self, n: u32, dur: f64) {
        let dur = round7(dur).max(0.0);
        let leaf = self.kind(n) != K_TIMELINE;
        let c = self.cold[n as usize];
        let tdur = Self::tdur_for(dur, c.repeat, c.rdelay, leaf);
        let h = &mut self.hot[n as usize];
        h.dur = dur;
        h.tdur = tdur;
        if c.parent != NIL {
            self.set_end(n);
        }
    }

    /// GSAP `_setEnd`: end = start + tdur / |ts or rts or tiny|.
    pub(crate) fn set_end(&mut self, n: u32) {
        let h = self.hot[n as usize];
        let rts = self.cold[n as usize].rts;
        let s = if h.ts != 0.0 {
            h.ts.abs()
        } else if rts != 0.0 {
            rts.abs()
        } else {
            TINY
        };
        let d = h.tdur / s;
        self.cold[n as usize].end = round7(h.start + if d.is_finite() { d } else { 0.0 });
    }

    /// GSAP `_uncache`: marks `n` and every linked ancestor dirty.
    pub(crate) fn uncache(&mut self, n: u32) {
        let mut a = n;
        while a != NIL {
            self.hot[a as usize].flags |= F_DIRTY;
            a = self.cold[a as usize].parent;
        }
    }

    /// GSAP `totalDuration()`: recomputes a dirty timeline first.
    pub(crate) fn total_duration(&mut self, n: u32) -> f64 {
        if self.hot[n as usize].flags & F_DIRTY != 0 {
            if self.kind(n) == K_TIMELINE {
                self.recompute_duration(n);
            } else {
                self.hot[n as usize].flags &= !F_DIRTY;
            }
        }
        self.hot[n as usize].tdur
    }

    /// GSAP `Timeline.totalDuration()` on a dirty timeline: the latest child
    /// end; a negative child start shifts every child (not the labels) and,
    /// under a smooth parent, the timeline's own start and playhead.
    pub(crate) fn recompute_duration(&mut self, tl: u32) {
        let mut max = 0.0f64;
        let mut c = self.cold[tl as usize].last;
        while c != NIL {
            let prev = self.hot[c as usize].prev;
            if self.hot[c as usize].flags & F_DIRTY != 0 {
                self.total_duration(c);
            }
            let h = self.hot[c as usize];
            let counts = h.ts != 0.0 && h.flags & F_KILLED == 0;
            if h.start < 0.0 && counts {
                let start = h.start;
                max -= start;
                let parent = self.cold[tl as usize].parent;
                let dp = self.cold[tl as usize].dp;
                if (parent == NIL && dp == NIL)
                    || (parent != NIL && self.has(parent, F_SMOOTH))
                {
                    // Deviation: GSAP divides by _ts (0 while paused, giving
                    // -Infinity); the requested scale keeps the start finite.
                    let ts = self.hot[tl as usize].ts;
                    let s = if ts != 0.0 {
                        ts
                    } else {
                        self.cold[tl as usize].rts
                    };
                    if s != 0.0 {
                        self.hot[tl as usize].start += round7(start / s);
                    }
                    let th = &mut self.hot[tl as usize];
                    th.time -= start;
                    th.ttime -= start;
                    if parent != NIL {
                        self.set_end(tl);
                    }
                }
                self.shift_children_raw(tl, -start, f64::NEG_INFINITY);
            }
            if counts && self.cold[c as usize].end > max {
                max = self.cold[c as usize].end;
            }
            c = prev;
        }
        let root = self.has(tl, F_ROOT);
        let t = self.hot[tl as usize].time;
        let d = if root && t > max { t } else { max };
        self.set_duration_raw(tl, d);
        self.hot[tl as usize].flags &= !F_DIRTY;
    }

    /// Shifts children starting at or after `ignore_before` (GSAP
    /// `shiftChildren` without the uncache).
    pub(crate) fn shift_children_raw(&mut self, tl: u32, amount: f64, ignore_before: f64) {
        let amount = round7(amount);
        let mut c = self.cold[tl as usize].first;
        while c != NIL {
            let h = &mut self.hot[c as usize];
            if h.start >= ignore_before {
                h.start += amount;
                self.cold[c as usize].end += amount;
            }
            c = self.hot[c as usize].next;
        }
    }

    /// A duration read without mutating (getters): a dirty timeline's
    /// content end is recomputed on the fly.
    pub(crate) fn pure_duration(&self, n: u32) -> f64 {
        let h = &self.hot[n as usize];
        if h.flags & F_DIRTY == 0 || h.flags & K_MASK != K_TIMELINE {
            return h.dur;
        }
        let mut max = 0.0f64;
        let mut min_start = 0.0f64;
        let mut c = self.cold[n as usize].first;
        while c != NIL {
            let ch = &self.hot[c as usize];
            if ch.ts != 0.0 && ch.flags & F_KILLED == 0 {
                let tdur = self.pure_total_duration(c);
                let s = if ch.ts != 0.0 { ch.ts.abs() } else { 1.0 };
                max = max.max(ch.start + tdur / s);
                min_start = min_start.min(ch.start);
            }
            c = ch.next;
        }
        let d = round7(max - min_start);
        if h.flags & F_ROOT != 0 {
            d.max(h.time)
        } else {
            d
        }
    }

    /// The total duration without mutating (see [`Self::pure_duration`]).
    pub(crate) fn pure_total_duration(&self, n: u32) -> f64 {
        let h = &self.hot[n as usize];
        if h.flags & F_DIRTY == 0 || h.flags & K_MASK != K_TIMELINE {
            return h.tdur;
        }
        let c = &self.cold[n as usize];
        Self::tdur_for(self.pure_duration(n), c.repeat, c.rdelay, false)
    }

    // ------------------------------------------------------------------
    // Labels
    // ------------------------------------------------------------------

    pub(crate) fn label_time(&self, tl: u32, l: Tag) -> Option<f64> {
        self.labels
            .iter()
            .find(|x| x.tl == tl && x.tag == l)
            .map(|x| x.time)
    }

    pub(crate) fn set_label(&mut self, tl: u32, l: Tag, time: f64) {
        if let Some(x) = self.labels.iter_mut().find(|x| x.tl == tl && x.tag == l) {
            x.time = time;
        } else {
            self.labels.push(Label { tl, tag: l, time });
        }
    }

    /// GSAP `_getLabelInDirection`: the nearest label strictly before
    /// (`backward`) or after `from`; ties go to the first added.
    pub(crate) fn label_in_direction(&self, tl: u32, from: f64, backward: bool) -> Option<Tag> {
        let mut min = BIG;
        let mut best = None;
        for l in self.labels.iter().filter(|l| l.tl == tl) {
            let d = l.time - from;
            if (d < 0.0) == backward && d != 0.0 && min > d.abs() {
                min = d.abs();
                best = Some(l.tag);
            }
        }
        best
    }

    // ------------------------------------------------------------------
    // Event reserve bookkeeping (section 6.3)
    // ------------------------------------------------------------------

    fn base_weight(&self, n: u32) -> u64 {
        let h = &self.hot[n as usize];
        let c = &self.cold[n as usize];
        if h.flags & (F_CALL | F_PAUSE_NODE) != 0 {
            return 1;
        }
        let m = c.events;
        let mut r = 0u64;
        for bit in [EventMask::START, EventMask::UPDATE, EventMask::REPEAT] {
            if m.has(bit) {
                r += 1;
            }
        }
        if m.has(EventMask::COMPLETE) || m.has(EventMask::REVERSE_COMPLETE) {
            r += 1;
        }
        if m.has(EventMask::INTERRUPT) {
            r += 1;
        }
        if m.has(EventMask::LABELS) {
            r += self.labels.iter().filter(|l| l.tl == n).count() as u64;
        }
        r
    }

    /// Recomputes the event weight of `n`'s subtree after it was (re)linked.
    pub(crate) fn update_weights(&mut self, n: u32) {
        let mut k = 0u32;
        let mut a = self.cold[n as usize].dp;
        while a != NIL {
            if self.kind(a) == K_TIMELINE && self.cold[a as usize].repeat != 0 {
                k += 1;
            }
            a = self.cold[a as usize].dp;
        }
        self.weigh_subtree(n, k);
    }

    fn weigh_subtree(&mut self, n: u32, k: u32) {
        let w = self.base_weight(n) << k.min(20);
        let old = self.cold[n as usize].weight;
        self.event_weight = self.event_weight - old + w;
        self.cold[n as usize].weight = w;
        let kk = if self.kind(n) == K_TIMELINE && self.cold[n as usize].repeat != 0 {
            k + 1
        } else {
            k
        };
        let mut c = self.cold[n as usize].first;
        while c != NIL {
            self.weigh_subtree(c, kk);
            c = self.hot[c as usize].next;
        }
    }

    // ------------------------------------------------------------------
    // Killing and reclamation
    // ------------------------------------------------------------------

    /// Kills `n` (GSAP `kill()`): reports Interrupt when asked and progress
    /// is below 1, marks the subtree killed with its tracks dead, and queues
    /// it for reclamation (unlinked and freed by [`Self::flush_reap`]).
    pub(crate) fn kill_node(&mut self, n: u32, interrupt: bool) {
        if self.has(n, F_KILLED | F_FREE) {
            return;
        }
        if interrupt
            && self.cold[n as usize].events.has(EventMask::INTERRUPT)
            && self.progress_of(n) < 1.0
        {
            self.emit(n, EventKind::Interrupt);
        }
        self.mark_killed(n);
        self.push_reap(n);
    }

    fn mark_killed(&mut self, n: u32) {
        self.hot[n as usize].flags |= F_KILLED;
        self.hot[n as usize].flags &= !F_ACT;
        self.kill_tracks_of(n);
        let mut c = self.cold[n as usize].first;
        while c != NIL {
            self.mark_killed(c);
            c = self.hot[c as usize].next;
        }
    }

    fn kill_tracks_of(&mut self, n: u32) {
        let (s, e) = self.track_range(n);
        for k in s..e {
            self.kill_track(k);
        }
    }

    /// Marks one track dead.
    pub(crate) fn kill_track(&mut self, k: u32) {
        let m = &mut self.tr_meta[k as usize];
        if m.flags & T_ALIVE != 0 {
            m.flags &= !T_ALIVE;
            self.dead_tracks += 1;
            let n = self.tr_node[k as usize];
            let c = &mut self.cold[n as usize];
            c.live_tracks = c.live_tracks.saturating_sub(1);
        }
    }

    #[inline]
    pub(crate) fn track_range(&self, n: u32) -> (u32, u32) {
        let c = &self.cold[n as usize];
        (c.tracks, c.tracks + c.n_tracks)
    }

    /// Unlinks and frees everything queued in `reap`. Runs at the end of
    /// `advance` and of every building or control call.
    pub(crate) fn flush_reap(&mut self) {
        while let Some(n) = self.reap.pop() {
            self.set_flag(n, F_IN_REAP, false);
            if self.has(n, F_FREE) {
                continue;
            }
            let killed = self.has(n, F_KILLED);
            if killed {
                self.unlink(n);
            }
            if killed || (!self.has(n, F_LINKED) && !self.has(n, F_KEEP)) {
                self.free_subtree(n);
            }
        }
    }

    /// Frees `n` and its descendants: handles go stale, tracks die, labels
    /// and defaults of freed timelines are dropped.
    pub(crate) fn free_subtree(&mut self, n: u32) {
        let mut c = self.cold[n as usize].first;
        while c != NIL {
            let next = self.hot[c as usize].next;
            self.free_subtree(c);
            c = next;
        }
        if self.has(n, F_LINKED) {
            self.unlink_raw(n);
        }
        self.kill_tracks_of(n);
        if self.kind(n) == K_TIMELINE {
            self.labels.retain(|l| l.tl != n);
            self.tl_defaults.retain(|d| d.0 != n);
        }
        let w = self.cold[n as usize].weight;
        self.event_weight -= w;
        let gen = self.cold[n as usize].gen.wrapping_add(1).max(1);
        self.cold[n as usize] = Cold::new(gen);
        self.hot[n as usize] = Hot {
            next: NIL,
            prev: NIL,
            flags: F_FREE,
            ..Hot::default()
        };
        self.free_nodes.push(n);
        self.stats.live_nodes = self.stats.live_nodes.saturating_sub(1);
    }

    /// In-place, stable track compaction: drops dead tracks, fixes every
    /// node's track range and rebuilds the per-slot rival chains. No
    /// allocation; O(nodes + tracks + slots).
    pub(crate) fn compact_tracks(&mut self) {
        for n in 0..self.hot.len() {
            if self.hot[n].flags & F_FREE == 0 {
                self.cold[n].n_tracks = 0;
                self.cold[n].tracks = 0;
            }
        }
        let mut w = 0usize;
        let mut last_node = NIL;
        for k in 0..self.tr_slot.len() {
            if self.tr_meta[k].flags & T_ALIVE == 0 {
                continue;
            }
            let n = self.tr_node[k];
            if n != last_node {
                self.cold[n as usize].tracks = w as u32;
                last_node = n;
            }
            self.cold[n as usize].n_tracks += 1;
            if w != k {
                self.tr_from[w] = self.tr_from[k];
                self.tr_to[w] = self.tr_to[k];
                self.tr_slot[w] = self.tr_slot[k];
                self.tr_meta[w] = self.tr_meta[k];
                self.tr_spec[w] = self.tr_spec[k];
                self.tr_node[w] = n;
            }
            w += 1;
        }
        self.tr_from.truncate(w);
        self.tr_to.truncate(w);
        self.tr_slot.truncate(w);
        self.tr_meta.truncate(w);
        self.tr_spec.truncate(w);
        self.tr_node.truncate(w);
        self.tr_next_in_slot.truncate(w);
        for f in self.sl_first_track.iter_mut() {
            *f = NIL;
        }
        for k in 0..w {
            let s = self.tr_slot[k] as usize;
            self.tr_next_in_slot[k] = self.sl_first_track[s];
            self.sl_first_track[s] = k as u32;
        }
        self.dead_tracks = 0;
        self.stats.compactions += 1;
    }

    // ------------------------------------------------------------------
    // Events
    // ------------------------------------------------------------------

    /// Appends an event of node `n` if its mask asks for `kind` (calls and
    /// pauses always report).
    pub(crate) fn emit(&mut self, n: u32, kind: EventKind) {
        let c = &self.cold[n as usize];
        let wanted = match kind {
            EventKind::Start => c.events.has(EventMask::START),
            EventKind::Update => c.events.has(EventMask::UPDATE),
            EventKind::Repeat => c.events.has(EventMask::REPEAT),
            EventKind::Complete => c.events.has(EventMask::COMPLETE),
            EventKind::ReverseComplete => c.events.has(EventMask::REVERSE_COMPLETE),
            EventKind::Interrupt => c.events.has(EventMask::INTERRUPT),
            EventKind::Label(_) => c.events.has(EventMask::LABELS),
            EventKind::Call { .. } | EventKind::Pause => true,
        };
        if !wanted {
            return;
        }
        let tag = match kind {
            EventKind::Label(l) => l,
            _ => c.tag,
        };
        let ev = TweenEvent {
            id: self.id_of(n),
            tag,
            kind,
            total_time: self.hot[n as usize].ttime,
            iteration: self.iteration_of(n),
        };
        if self.events.len() == self.events.capacity() {
            self.stats.event_overflows += 1;
        }
        self.events.push(ev);
    }

    /// The fired callbacks since the last drain, in GSAP firing order.
    pub fn events(&self) -> &[TweenEvent] {
        &self.events
    }

    /// Drains the event queue into `buf` without allocating: the engine
    /// takes `buf`'s storage for its next events. Call once per frame.
    pub fn swap_events(&mut self, buf: &mut Vec<TweenEvent>) {
        buf.clear();
        std::mem::swap(&mut self.events, buf);
        self.reserve_events();
    }

    /// Drops every queued event.
    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    /// Engine counters.
    pub fn stats(&self) -> Stats {
        let mut s = self.stats;
        s.nodes = self.hot.len() as u32;
        s.tracks = self.tr_slot.len() as u32;
        s.dead_tracks = self.dead_tracks;
        s.slots = self.sl_val.len() as u32;
        s
    }

    // ------------------------------------------------------------------
    // Value store
    // ------------------------------------------------------------------

    #[inline]
    fn slot_hash(t: TargetId, p: PropKey) -> u64 {
        splitmix64(p.0 ^ (t.0 as u64).rotate_left(32))
    }

    fn find_slot(&self, t: TargetId, p: PropKey) -> Option<u32> {
        if self.sl_index.is_empty() {
            return None;
        }
        let mask = self.sl_index.len() - 1;
        let mut i = Self::slot_hash(t, p) as usize & mask;
        loop {
            let s = self.sl_index[i];
            if s == NIL {
                return None;
            }
            if self.sl_target[s as usize] == t && self.sl_key[s as usize] == p {
                return Some(s);
            }
            i = (i + 1) & mask;
        }
    }

    fn index_insert(&mut self, s: u32) {
        let mask = self.sl_index.len() - 1;
        let mut i = Self::slot_hash(self.sl_target[s as usize], self.sl_key[s as usize]) as usize & mask;
        while self.sl_index[i] != NIL {
            i = (i + 1) & mask;
        }
        self.sl_index[i] = s;
    }

    /// The slot of (t, p), created (unseeded, of `kind`) when missing.
    pub(crate) fn slot_for(&mut self, t: TargetId, p: PropKey, kind: ValueKind) -> u32 {
        if let Some(s) = self.find_slot(t, p) {
            return s;
        }
        let s = self.sl_val.len() as u32;
        self.sl_val.push([0.0; 4]);
        self.sl_kind.push(kind);
        self.sl_target.push(t);
        self.sl_key.push(p);
        self.sl_first_track.push(NIL);
        self.sl_stamp.push(0);
        self.sl_seeded.push(false);
        self.slot_gen = self.slot_gen.wrapping_add(1);
        if self.changed.capacity() < self.sl_val.len() {
            self.changed.reserve(self.sl_val.len() - self.changed.len());
        }
        if (self.sl_val.len() * 2) > self.sl_index.len() {
            let size = (self.sl_val.len() * 2).next_power_of_two().max(16);
            self.sl_index.clear();
            self.sl_index.resize(size, NIL);
            for i in 0..self.sl_val.len() as u32 {
                self.index_insert(i);
            }
        } else {
            self.index_insert(s);
        }
        s
    }

    /// Sets the current value of (t, p): GSAP reads a target's current value
    /// when a tween starts; here the host seeds it. Marks the slot changed.
    pub fn seed(&mut self, t: TargetId, p: PropKey, v: TweenValue) -> SlotId {
        let s = self.slot_for(t, p, v.kind());
        self.sl_kind[s as usize] = v.kind();
        self.sl_seeded[s as usize] = true;
        self.write_slot(s, v.to_lanes());
        SlotId(s)
    }

    /// The slot of (t, p), if one exists.
    pub fn slot(&self, t: TargetId, p: PropKey) -> Option<SlotId> {
        self.find_slot(t, p).map(SlotId)
    }

    /// The value in slot `s`.
    pub fn value(&self, s: SlotId) -> TweenValue {
        let i = s.0 as usize;
        if i >= self.sl_val.len() {
            return TweenValue::F64(0.0);
        }
        TweenValue::from_lanes(self.sl_kind[i], self.sl_val[i])
    }

    /// The current value of (t, p).
    pub fn get(&self, t: TargetId, p: PropKey) -> Option<TweenValue> {
        self.find_slot(t, p).map(|s| self.value(SlotId(s)))
    }

    /// The current value of (t, p) as a number (lane 0).
    pub fn get_f64(&self, t: TargetId, p: PropKey) -> Option<f64> {
        self.find_slot(t, p).map(|s| self.sl_val[s as usize][0])
    }

    /// The (target, property) pair of slot `s`.
    pub fn slot_key(&self, s: SlotId) -> (TargetId, PropKey) {
        (self.sl_target[s.0 as usize], self.sl_key[s.0 as usize])
    }

    /// How many slots exist.
    pub fn slot_count(&self) -> u32 {
        self.sl_val.len() as u32
    }

    /// Bumps whenever a slot is created, so cached slot lookups can rebuild.
    pub fn slot_generation(&self) -> u32 {
        self.slot_gen
    }

    /// Slots written since the last [`TweenEngine::clear_changes`], in
    /// first-write order, each listed once.
    pub fn changes(&self) -> &[SlotId] {
        &self.changed
    }

    /// Whether slot `s` is in [`TweenEngine::changes`] (O(1)).
    pub fn is_changed(&self, s: SlotId) -> bool {
        self.sl_stamp
            .get(s.0 as usize)
            .is_some_and(|st| *st == self.change_gen)
    }

    /// Empties the change list (O(1)).
    pub fn clear_changes(&mut self) {
        self.change_gen += 1;
        self.changed.clear();
    }

    /// Lists every slot as changed (to re-push everything after a reload).
    pub fn mark_all_changed(&mut self) {
        for s in 0..self.sl_val.len() {
            if self.sl_stamp[s] != self.change_gen {
                self.sl_stamp[s] = self.change_gen;
                self.changed.push(SlotId(s as u32));
            }
        }
    }

    /// Writes lanes into slot `s` if they differ (bitwise) and records the change.
    #[inline]
    pub(crate) fn write_slot(&mut self, s: u32, v: [f64; 4]) {
        let i = s as usize;
        let old = self.sl_val[i];
        if old[0].to_bits() != v[0].to_bits()
            || old[1].to_bits() != v[1].to_bits()
            || old[2].to_bits() != v[2].to_bits()
            || old[3].to_bits() != v[3].to_bits()
        {
            self.sl_val[i] = v;
            if self.sl_stamp[i] != self.change_gen {
                self.sl_stamp[i] = self.change_gen;
                self.changed.push(SlotId(s));
            }
        }
    }

    // ------------------------------------------------------------------
    // Engine-wide utilities
    // ------------------------------------------------------------------

    /// GSAP `gsap.getById()`: the first live animation tagged `tag`.
    pub fn get_by_tag(&self, tag: Tag) -> Option<TweenId> {
        if tag == Tag::NONE {
            return None;
        }
        (0..self.hot.len() as u32)
            .find(|&n| {
                n != self.root
                    && self.hot[n as usize].flags & (F_FREE | F_KILLED) == 0
                    && self.cold[n as usize].tag == tag
            })
            .map(|n| self.id_of(n))
    }

    /// Kills every animation without reporting (GSAP
    /// `gsap.globalTimeline.clear()`); slot values are kept.
    pub fn kill_all(&mut self) {
        for n in 0..self.hot.len() as u32 {
            if n != self.root && self.hot[n as usize].flags & (F_FREE | F_KILLED) == 0 {
                let p = self.cold[n as usize].dp;
                // Kill only subtree roots: nodes whose last parent is the root
                // or who are orphans.
                if p == self.root || p == NIL || self.hot[p as usize].flags & F_FREE != 0 {
                    self.kill_node(n, false);
                }
            }
        }
        self.flush_reap();
    }

    /// GSAP `gsap.globalTimeline.timeScale()`.
    pub fn root_time_scale(&self) -> f64 {
        self.hot[self.root as usize].ts
    }

    /// Sets the root time scale: every root-level animation speeds up or
    /// slows down (0 freezes the engine).
    pub fn set_root_time_scale(&mut self, s: f64) {
        let r = self.root as usize;
        self.hot[r].ts = s;
        self.cold[r].rts = s;
    }
}

/// A stagger inherited from the defaults chain (GSAP allows `stagger` in
/// `defaults`).
fn r_stagger(e: &TweenEngine, parent: u32, o: &TweenOpts) -> Option<crate::stagger::Stagger> {
    if o.inherit == Some(false) {
        return e.defaults.stagger;
    }
    let mut p = parent;
    while p != NIL {
        if let Some(d) = e.tl_defaults.iter().find(|d| d.0 == p) {
            if d.1.stagger.is_some() {
                return d.1.stagger;
            }
        }
        let c = &e.cold[p as usize];
        p = if c.parent != NIL { c.parent } else { c.dp };
    }
    e.defaults.stagger
}
