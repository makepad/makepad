//! Read-only inspection: what a debugger or a timeline view needs to draw
//! an engine's animations without touching their state.
//!
//! [`TweenEngine::inspect`] walks every linked animation depth-first from
//! the root (GSAP's `globalTimeline.getChildren(true, true, true)`) into a
//! caller-owned list; [`TweenEngine::inspect_labels`] and
//! [`TweenEngine::inspect_tracks`] read one node's labels and animated
//! properties. Nothing here changes the engine, and the only allocation is
//! the caller's list growing.

use crate::engine::{
    TweenEngine, F_CALL, F_FREE, F_KEEP, F_KILLED, F_LINKED, F_PAUSED, F_PAUSE_NODE, F_ROOT,
    K_GROUP, K_MASK, K_TIMELINE, NIL,
};
use crate::ids::{PropKey, Tag, TargetId, TweenId};

/// What kind of animation a node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectKind {
    /// A tween (GSAP `gsap.to` and friends).
    Tween,
    /// A timeline.
    Timeline,
    /// A stagger or keyframes group: a tween with an inner timeline.
    Group,
    /// A delayed call (`tl.call`).
    Call,
    /// A pause (`tl.addPause`).
    Pause,
}

/// One animation as the inspector sees it.
#[derive(Clone, Copy, Debug)]
pub struct InspectNode {
    /// Its handle (controls work on it like on any handle).
    pub id: TweenId,
    /// Its parent's handle; [`TweenId::NONE`] for a top-level animation
    /// (a child of the root).
    pub parent: TweenId,
    /// 0 for a top-level animation, 1 for its children, and so on.
    pub depth: u32,
    pub kind: InspectKind,
    /// Its GSAP `id` (or a call's tag); [`Tag::NONE`] when unnamed.
    pub tag: Tag,
    /// Start in the parent's local time, delay included.
    pub start: f64,
    /// Start in the time of its top-level ancestor (starts summed up the
    /// chain; nested time scales are not applied). A top-level node's own
    /// start is 0 here, so its children line up under it.
    pub local_start: f64,
    /// One iteration.
    pub duration: f64,
    /// All iterations with repeat delays ([`crate::INFINITE`] for `repeat: -1`).
    pub total_duration: f64,
    /// Iteration time at the last render.
    pub time: f64,
    /// Total time at the last render.
    pub total_time: f64,
    pub repeat: i32,
    pub yoyo: bool,
    pub paused: bool,
    pub reversed: bool,
    /// Whether it is on its parent's timeline. A kept animation that
    /// completed (or was removed) is detached: it is listed as top-level,
    /// after the linked ones, and plays again on a restart.
    pub linked: bool,
    /// Requested time scale (sign dropped; see `reversed`).
    pub time_scale: f64,
    /// How many property tracks it animates (a tween's own, not its children's).
    pub tracks: u32,
    /// Its first animated property, when it has one.
    pub first_track: Option<(TargetId, PropKey)>,
}

impl TweenEngine {
    /// Every live animation, depth-first in start order (a parent before its
    /// children), into `out` (cleared first): the root's children first,
    /// then each kept (or paused) animation that is detached from any
    /// parent (a completed `keep` timeline), as a top-level entry.
    /// Read-only; allocates only when `out` grows. O(live nodes) plus one
    /// pass over the node slots for the detached ones.
    pub fn inspect(&self, out: &mut Vec<InspectNode>) {
        out.clear();
        if self.root != NIL && (self.root as usize) < self.cold.len() {
            let mut c = self.cold[self.root as usize].first;
            while c != NIL {
                self.inspect_node(c, TweenId::NONE, 0, 0.0, true, out);
                c = self.hot[c as usize].next;
            }
        }
        for n in 0..self.hot.len() as u32 {
            let f = self.hot[n as usize].flags;
            let held = f & (F_KEEP | F_PAUSED) != 0;
            if held && f & (F_LINKED | F_FREE | F_KILLED | F_ROOT) == 0 && n != self.root {
                self.inspect_node(n, TweenId::NONE, 0, 0.0, true, out);
            }
        }
    }

    fn inspect_node(
        &self,
        n: u32,
        parent: TweenId,
        depth: u32,
        base: f64,
        top: bool,
        out: &mut Vec<InspectNode>,
    ) {
        let h = &self.hot[n as usize];
        if h.flags & (F_FREE | F_KILLED) != 0 {
            return;
        }
        let c = &self.cold[n as usize];
        let kind = match h.flags & K_MASK {
            K_TIMELINE => InspectKind::Timeline,
            K_GROUP => InspectKind::Group,
            _ if h.flags & F_PAUSE_NODE != 0 => InspectKind::Pause,
            _ if h.flags & F_CALL != 0 => InspectKind::Call,
            _ => InspectKind::Tween,
        };
        let local_start = if top { 0.0 } else { base + h.start };
        let first_track = (c.n_tracks > 0).then(|| {
            let slot = self.tr_slot[c.tracks as usize] as usize;
            (self.sl_target[slot], self.sl_key[slot])
        });
        let id = self.id_of(n);
        out.push(InspectNode {
            id,
            parent,
            depth,
            kind,
            tag: c.tag,
            start: h.start,
            local_start,
            // The cached durations can be stale until the next render;
            // the pure reads walk a dirty timeline's children instead.
            duration: self.pure_duration(n),
            total_duration: self.pure_total_duration(n),
            time: h.time,
            total_time: h.ttime,
            repeat: c.repeat,
            yoyo: c.yoyo.is_some() || h.flags & crate::engine::F_YOYO != 0,
            paused: h.flags & crate::engine::F_PAUSED != 0,
            reversed: c.rts < 0.0,
            linked: h.flags & F_LINKED != 0,
            time_scale: c.rts.abs(),
            tracks: c.n_tracks,
            first_track,
        });
        if matches!(kind, InspectKind::Timeline | InspectKind::Group) {
            let mut k = c.first;
            while k != NIL {
                self.inspect_node(k, id, depth + 1, local_start, false, out);
                k = self.hot[k as usize].next;
            }
        }
    }

    /// The labels of timeline `tl` in time order, into `out` (cleared
    /// first). Empty for a stale handle or a node with no labels.
    pub fn inspect_labels(&self, tl: TweenId, out: &mut Vec<(Tag, f64)>) {
        out.clear();
        if let Some(n) = self.node(tl) {
            out.extend(self.labels_of(n).iter().map(|l| (l.tag, l.time)));
        }
    }

    /// Every (target, property) animation `a` animates itself, in track
    /// order, into `out` (cleared first).
    pub fn inspect_tracks(&self, a: TweenId, out: &mut Vec<(TargetId, PropKey)>) {
        out.clear();
        let Some(n) = self.node(a) else {
            return;
        };
        let c = &self.cold[n as usize];
        for t in c.tracks..c.tracks + c.n_tracks {
            let slot = self.tr_slot[t as usize] as usize;
            out.push((self.sl_target[slot], self.sl_key[slot]));
        }
    }
}
