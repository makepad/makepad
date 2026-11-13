//! The data behind a timeline editor (the widgets' `Sequencer`, an ImSequencer
//! / GSDevTools-like view): a plain model of tracks of bars on a time axis,
//! built from a timeline of this engine ([`SequencerModel::from_engine`]),
//! the mapping of the editor's edits back onto that timeline
//! ([`apply_sequencer_edit`]) and the snapping maths of its drags
//! ([`SequencerModel::item_snap_targets`], [`SequencerModel::drag_move`],
//! [`snap_time`], ...). Plain data and pure functions: nothing here draws,
//! and any timeline source can fill a [`SequencerModel`] by hand.
//!
//! Model time is the root timeline's LOCAL time (its own time scale is not
//! applied). Every nested animation's first iteration is placed through each
//! parent's start, time scale and direction: a child of a reversed timeline
//! sits where it plays (mirrored in its parent) and is flagged
//! [`SequencerItem::reversed`]. Later iterations of a repeating parent are
//! drawn on the parent's own bar. Nested timelines' labels are not shown.
//! The children of stagger and keyframes groups are locked (moving one would
//! desync the group's cached content duration); keyframes steps under a
//! non-linear group ease are placed linearly.

use crate::control::{AnimKind, AnimRef};
use crate::engine::TweenEngine;
use crate::ids::{Tag, TweenId};
use crate::spec::Position;
use crate::BIG;

/// What a track shows (the editor colours its bars by kind unless the track
/// has a colour of its own).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SequencerKind {
    #[default]
    Tween,
    Timeline,
    /// A stagger or keyframes group.
    Group,
    Call,
    Pause,
}

impl SequencerKind {
    /// "tween", "timeline", "group", "call" or "pause".
    pub fn name(self) -> &'static str {
        match self {
            SequencerKind::Tween => "tween",
            SequencerKind::Timeline => "timeline",
            SequencerKind::Group => "group",
            SequencerKind::Call => "call",
            SequencerKind::Pause => "pause",
        }
    }
}

/// One bar (or marker) on a track, in model time (seconds).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SequencerItem {
    /// The host's id (engine models: [`TweenId::to_bits`]).
    pub id: u64,
    /// Where the first iteration starts (the delay has already passed).
    pub start: f64,
    /// Drawn hatched before `start` (0 inside a reversed parent, where the
    /// delay passes after the bar).
    pub delay: f64,
    /// One iteration; 0 = a marker (a diamond).
    pub duration: f64,
    /// Extra iterations; -1 forever.
    pub repeat: i32,
    pub repeat_delay: f64,
    /// Every other iteration runs backwards.
    pub yoyo: bool,
    /// The item runs backwards in model time (it is reversed, or it sits in
    /// an odd number of reversed parents).
    pub reversed: bool,
    /// The caption: a marker's name, a tween's ease.
    pub label: String,
    /// No move or resize (selection and the tooltip still work).
    pub locked: bool,
}

impl SequencerItem {
    /// From `start` to the end of the last iteration: infinite for
    /// `repeat < 0`.
    pub fn active_len(&self) -> f64 {
        if self.repeat < 0 {
            f64::INFINITY
        } else {
            let k = self.repeat as f64;
            self.duration * (k + 1.0) + self.repeat_delay * k
        }
    }

    /// `start + active_len()`.
    pub fn end(&self) -> f64 {
        self.start + self.active_len()
    }
}

/// One row. Tracks are a pre-order flattened tree: `depth` nests a track
/// under the nearest earlier track of a smaller depth, and a track is shown
/// while every ancestor is expanded.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SequencerTrack {
    pub id: u64,
    pub name: String,
    /// RGBA 0..1 (the editor's colour type as plain numbers); `None`: the
    /// kind's theme colour.
    pub color: Option<[f32; 4]>,
    pub kind: SequencerKind,
    pub depth: u32,
    /// Shown unfolded until the user folds it (the editor keeps its own fold
    /// state per track id across model updates).
    pub expanded: bool,
    pub has_children: bool,
    pub items: Vec<SequencerItem>,
}

/// A named time on the ruler (a timeline label).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SequencerLabel {
    /// The host's id (engine models: the label's [`Tag`] value), so a
    /// [`SequencerAction::LabelMoved`] needs no string.
    pub id: u64,
    pub name: String,
    pub time: f64,
}

/// A musical grid over the time axis: steps (beats) `step` long from
/// `origin`, every `per_major`th a major one (a bar). Step 0 is the first
/// step of major 1; steps before `origin` count back from it (major 0, then
/// -1, ...), so a host can put a count-in before it. With a grid the
/// ruler counts majors and steps, drags snap to majors (steps with Shift)
/// and Left / Right nudge by a step (a major with Shift).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SequencerGrid {
    pub origin: f64,
    pub step: f64,
    pub per_major: u32,
}

impl SequencerGrid {
    /// How many steps from `origin` time `t` is.
    pub fn steps_at(&self, t: f64) -> f64 {
        (t - self.origin) / self.step
    }

    /// The time `k` steps from `origin`.
    pub fn time_of(&self, k: f64) -> f64 {
        self.origin + k * self.step
    }

    /// The length of `steps` steps.
    pub fn span(&self, steps: f64) -> f64 {
        steps * self.step
    }

    /// `t` moved to the nearest multiple of `steps` steps from `origin`.
    pub fn snap(&self, t: f64, steps: f64) -> f64 {
        if !(self.step > 0.0) || !(steps > 0.0) || !t.is_finite() {
            return t;
        }
        self.time_of((self.steps_at(t) / steps).round() * steps)
    }

    /// Step `k` as (major, step within it), both counted from 1: step 0 is
    /// (1, 1), step -1 is (0, per_major).
    pub fn place(&self, k: i64) -> (i64, i64) {
        let n = self.per_major.max(1) as i64;
        (k.div_euclid(n) + 1, k.rem_euclid(n) + 1)
    }
}

/// Which edge of an item a drag sets ([`SequencerModel::snap_drag_to_grid`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequencerEdge {
    /// The whole item: its start (or, failing that, its end) lands on the grid.
    Move,
    Start,
    End,
}

/// What a timeline editor shows: plain data any timeline source can fill.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SequencerModel {
    /// The extent of the time axis (seconds).
    pub duration: f64,
    pub playhead: f64,
    pub tracks: Vec<SequencerTrack>,
    pub labels: Vec<SequencerLabel>,
    /// Where the time axis starts: 0, or before it for a count-in. Items at
    /// depth 0, the playhead and labels may go back to here.
    pub start: f64,
    /// A musical grid over the axis, if the host has one.
    pub grid: Option<SequencerGrid>,
}

/// What a timeline editor reports (the widgets' `Sequencer` emits these as
/// widget actions). The edits (`ItemMoved`, `ItemResized`, `LabelMoved`) go
/// to [`apply_sequencer_edit`]; transport, selection and view stay with the
/// host.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum SequencerAction {
    /// A scrub began (a press on the ruler, Home / End).
    ScrubStart,
    /// The playhead was dragged to this model time.
    Scrub(f64),
    ScrubEnd,
    /// An item's first iteration now starts here (model time).
    ItemMoved {
        id: u64,
        start: f64,
    },
    /// An item's iteration now lasts this long (model seconds).
    ItemResized {
        id: u64,
        duration: f64,
    },
    LabelMoved {
        id: u64,
        time: f64,
    },
    /// A drag or nudge that emitted edits ended.
    EditEnd,
    /// The selection changed (on press).
    Selected(Option<u64>),
    /// The user zoomed or panned: pixels per second, the time at the left
    /// edge of the track area.
    Zoom(f64, f64),
    #[default]
    None,
}

/// Where a drag puts an item ([`SequencerModel::drag_move`] and the resize
/// functions): its new start and duration, and the snap target that won
/// (to draw as a guide), if any.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SequencerDrag {
    pub start: f64,
    pub duration: f64,
    pub guide: Option<f64>,
}

// ---------------------------------------------------------------------------
// Snapping
// ---------------------------------------------------------------------------

/// The nearest pair of an edge in `edges` and a target in `targets` within
/// `radius` (seconds): `(target - edge, target)`. Ties go to the earlier
/// target; non-finite values are skipped.
pub fn snap_nearest(targets: &[f64], edges: &[f64], radius: f64) -> Option<(f64, f64)> {
    let mut best: Option<(f64, f64)> = None;
    for &c in targets {
        if !c.is_finite() {
            continue;
        }
        for &edge in edges {
            if !edge.is_finite() {
                continue;
            }
            let d = c - edge;
            if d.abs() <= radius && best.map_or(true, |(bd, _)| d.abs() < bd.abs()) {
                best = Some((d, c));
            }
        }
    }
    best
}

/// `t` moved onto the nearest target within `radius`, else `t`.
pub fn snap_time(targets: &[f64], t: f64, radius: f64) -> f64 {
    match snap_nearest(targets, &[t], radius) {
        Some((d, _)) => t + d,
        None => t,
    }
}

impl SequencerModel {
    /// The (track, item) indices of item `id`.
    pub fn find(&self, item: u64) -> Option<(usize, usize)> {
        self.tracks
            .iter()
            .enumerate()
            .find_map(|(ti, t)| t.items.iter().position(|i| i.id == item).map(|ii| (ti, ii)))
    }

    /// Item `id`.
    pub fn item(&self, item: u64) -> Option<&SequencerItem> {
        self.find(item).map(|(t, i)| &self.tracks[t].items[i])
    }

    /// Track `track` and its descendants (the following tracks deeper than
    /// it), as a range of track indices.
    pub fn subtree(&self, track: usize) -> std::ops::Range<usize> {
        let Some(t) = self.tracks.get(track) else {
            return track..track;
        };
        let end = self.tracks[track + 1..]
            .iter()
            .position(|n| n.depth <= t.depth)
            .map_or(self.tracks.len(), |p| track + 1 + p);
        track..end
    }

    /// The earliest start a move may give item `id`: its parent row's first
    /// item's start plus the item's own delay (0 at depth 0), so an item is
    /// never placed before its parent.
    pub fn min_start(&self, item: u64) -> Option<f64> {
        let (ti, ii) = self.find(item)?;
        let d = self.tracks[ti].depth;
        let parent = if d == 0 {
            self.start
        } else {
            self.tracks[..ti]
                .iter()
                .rev()
                .find(|t| t.depth < d)
                .and_then(|t| t.items.first())
                .map_or(0.0, |i| i.start)
        };
        Some(parent + self.tracks[ti].items[ii].delay)
    }

    /// The snap targets for dragging (moving or resizing) item `id`, taken
    /// once when the drag starts: 0, the duration, `playhead`, the labels and
    /// the start and end of every item OUTSIDE the dragged item's subtree.
    /// Whatever the dragged subtree defines is left out: its own children
    /// ride with it, and an end it defines (the model's duration, an
    /// ancestor's end) moves with it once the host rebuilds the model; as
    /// targets they would pin the bar where it already is and make it trail
    /// the pointer by up to the snap radius. Allocation-free once `out` has
    /// grown.
    pub fn item_snap_targets(&self, item: u64, playhead: f64, out: &mut Vec<f64>) {
        out.clear();
        let Some((ti, _)) = self.find(item) else {
            return;
        };
        let sub = self.subtree(ti);
        let own_end = self.tracks[sub.clone()]
            .iter()
            .flat_map(|t| t.items.iter())
            .map(|i| i.end())
            .filter(|e| e.is_finite())
            .fold(f64::NEG_INFINITY, f64::max);
        let eps = 1e-9 * own_end.abs().max(1.0);
        let defined = |x: f64| own_end.is_finite() && (x - own_end).abs() <= eps;
        let mut push = |x: f64| {
            if x.is_finite() {
                out.push(x);
            }
        };
        push(self.start);
        if !defined(self.duration) {
            push(self.duration);
        }
        push(playhead);
        for l in &self.labels {
            push(l.time);
        }
        // Before the subtree: the ancestors (each a shallower track than
        // every track between it and the dragged one) keep their start but
        // lose an end the subtree defines.
        let mut min_depth = self.tracks[ti].depth;
        for i in (0..ti).rev() {
            let t = &self.tracks[i];
            let ancestor = t.depth < min_depth;
            min_depth = min_depth.min(t.depth);
            for it in &t.items {
                push(it.start);
                if !(ancestor && defined(it.end())) {
                    push(it.end());
                }
            }
        }
        for t in &self.tracks[sub.end..] {
            for it in &t.items {
                push(it.start);
                push(it.end());
            }
        }
    }

    /// The snap targets for dragging label `id`: 0, the duration,
    /// `playhead`, the other labels and every item's start and end.
    pub fn label_snap_targets(&self, label: u64, playhead: f64, out: &mut Vec<f64>) {
        out.clear();
        let mut push = |x: f64| {
            if x.is_finite() {
                out.push(x);
            }
        };
        push(self.start);
        push(self.duration);
        push(playhead);
        for l in &self.labels {
            if l.id != label {
                push(l.time);
            }
        }
        for t in &self.tracks {
            for it in &t.items {
                push(it.start);
                push(it.end());
            }
        }
    }

    /// A move of item `id` whose start the pointer puts at `raw`: its start
    /// or its end snaps to the nearest target within `radius` (pass no
    /// targets to place freely), then the start is kept at or after
    /// [`SequencerModel::min_start`]. `None` for an unknown item.
    pub fn drag_move(
        &self,
        item: u64,
        raw: f64,
        targets: &[f64],
        radius: f64,
    ) -> Option<SequencerDrag> {
        let it = self.item(item)?;
        let len = it.active_len();
        let snap = if len.is_finite() {
            snap_nearest(targets, &[raw, raw + len], radius)
        } else {
            snap_nearest(targets, &[raw], radius)
        };
        let start = (raw + snap.map_or(0.0, |s| s.0)).max(self.min_start(item)?);
        Some(SequencerDrag {
            start,
            duration: it.duration,
            guide: snap.map(|s| s.1),
        })
    }

    /// A right-edge resize of item `id` whose first iteration's end the
    /// pointer puts at `raw_end`: the end snaps, the duration is at least
    /// `min_duration`.
    pub fn drag_resize_end(
        &self,
        item: u64,
        raw_end: f64,
        targets: &[f64],
        radius: f64,
        min_duration: f64,
    ) -> Option<SequencerDrag> {
        let it = self.item(item)?;
        let snap = snap_nearest(targets, &[raw_end], radius);
        let end = raw_end + snap.map_or(0.0, |s| s.0);
        Some(SequencerDrag {
            start: it.start,
            duration: (end - it.start).max(min_duration),
            guide: snap.map(|s| s.1),
        })
    }

    /// A left-edge resize of item `id` that keeps its first iteration's end
    /// at `end` (the end at the press): the start the pointer puts at
    /// `raw_start` snaps, stays at or after the item's minimum start and at
    /// least `min_duration` before `end`.
    pub fn drag_resize_start(
        &self,
        item: u64,
        raw_start: f64,
        end: f64,
        targets: &[f64],
        radius: f64,
        min_duration: f64,
    ) -> Option<SequencerDrag> {
        let min = self.min_start(item)?;
        let snap = snap_nearest(targets, &[raw_start], radius);
        let start = (raw_start + snap.map_or(0.0, |s| s.0))
            .max(min)
            .min(end - min_duration);
        Some(SequencerDrag {
            start,
            duration: end - start,
            guide: snap.map(|s| s.1),
        })
    }

    /// `drag` (from [`SequencerModel::drag_move`] or a resize of item `id`)
    /// with the edge it sets put on the grid, every `steps` steps, when it did
    /// not snap to a target already (`drag.guide`) and there is a grid: a
    /// move puts its start on the grid (keeping its length), a resize the
    /// edge it drags, leaving at least one unit; the start stays at or after
    /// [`SequencerModel::min_start`]. The guide is the grid time it took.
    pub fn snap_drag_to_grid(
        &self,
        item: u64,
        drag: SequencerDrag,
        edge: SequencerEdge,
        steps: f64,
    ) -> SequencerDrag {
        let (Some(g), None) = (self.grid, drag.guide) else {
            return drag;
        };
        let Some(min) = self.min_start(item) else {
            return drag;
        };
        let unit = g.span(steps);
        let end = drag.start + drag.duration;
        match edge {
            SequencerEdge::Move => {
                let s = g.snap(drag.start, steps);
                let s = if s < min { s + unit } else { s };
                SequencerDrag { start: s.max(min), duration: drag.duration, guide: Some(s) }
            }
            SequencerEdge::Start => {
                let s = g.snap(drag.start, steps);
                let s = if s < min { s + unit } else { s };
                let s = s.max(min).min(end - unit.min(drag.duration).max(1e-9));
                SequencerDrag { start: s, duration: end - s, guide: Some(s) }
            }
            SequencerEdge::End => {
                let e = g.snap(end, steps).max(drag.start + unit);
                SequencerDrag { start: drag.start, duration: e - drag.start, guide: Some(e) }
            }
        }
    }

    /// The tree under timeline `root` of a tween engine: row 0 is `root`
    /// (locked, at 0), then every child in start order, depth first; stagger
    /// and keyframes groups and nested timelines are foldable rows whose
    /// children follow them (group children locked, groups folded at first).
    /// See the module docs for how time is mapped. `name_of(id, tag)` names
    /// each row (and, with `TweenId::NONE`, each label); a call or pause
    /// row's caption is its name, a tween's its ease. A stale `root` gives
    /// the empty model. Allocates (build it when the timeline changes, not
    /// per frame).
    pub fn from_engine(
        e: &TweenEngine,
        root: TweenId,
        mut name_of: impl FnMut(TweenId, Tag) -> String,
    ) -> SequencerModel {
        let r = e.anim_ref(root);
        if !r.is_alive() {
            return SequencerModel::default();
        }
        let tdur = r.total_duration();
        let finite = tdur < BIG;
        let labels = r
            .labels()
            .map(|(tag, time)| SequencerLabel {
                id: tag.0,
                name: name_of(TweenId::NONE, tag),
                time,
            })
            .collect();
        let mut m = SequencerModel {
            duration: if finite { tdur } else { r.duration() },
            playhead: if finite { r.total_time() } else { r.time() },
            tracks: Vec::new(),
            labels,
            ..Default::default()
        };
        let at = RowAt {
            depth: 0,
            frame: Frame::ROOT,
            locked: true,
            root: true,
        };
        engine_rows(e, root, at, &mut m.tracks, &mut name_of);
        m
    }
}

// ---------------------------------------------------------------------------
// Engine models
// ---------------------------------------------------------------------------

/// How a parent's local time maps to model time: forwards
/// (`origin + t * k`) or mirrored (`origin + (span - t) * k`, a parent that
/// plays backwards in model time). `span` is the parent's local extent.
#[derive(Clone, Copy, Debug)]
struct Frame {
    origin: f64,
    k: f64,
    mirror: bool,
    span: f64,
}

impl Frame {
    const ROOT: Frame = Frame {
        origin: 0.0,
        k: 1.0,
        mirror: false,
        span: 0.0,
    };

    /// The model start of the local range `[t, t + len]`.
    fn place(&self, t: f64, len: f64) -> f64 {
        if self.mirror {
            self.origin + (self.span - t - len) * self.k
        } else {
            self.origin + t * self.k
        }
    }

    /// The local start of a range of local length `len` whose model start is
    /// `model` (the inverse of [`Frame::place`]).
    fn local(&self, model: f64, len: f64) -> f64 {
        if self.mirror {
            self.span - len - (model - self.origin) / self.k
        } else {
            (model - self.origin) / self.k
        }
    }
}

/// Where [`engine_rows`] places a node.
#[derive(Clone, Copy)]
struct RowAt {
    depth: u32,
    /// The parent's frame.
    frame: Frame,
    locked: bool,
    root: bool,
}

/// A node's own time scale as a divisor: |time scale|, 1 when 0.
fn time_scale_div(a: &AnimRef) -> f64 {
    let ts = a.time_scale().abs();
    if ts > 0.0 && ts.is_finite() {
        ts
    } else {
        1.0
    }
}

/// How long a node is active in its parent's local time: its total duration
/// over its time scale (one iteration when it repeats forever).
fn local_len(a: &AnimRef) -> f64 {
    let td = a.total_duration();
    let d = if td < BIG { td } else { a.duration() };
    d / time_scale_div(a)
}

fn kind_of(k: AnimKind) -> SequencerKind {
    match k {
        AnimKind::Tween => SequencerKind::Tween,
        AnimKind::Timeline => SequencerKind::Timeline,
        AnimKind::Stagger | AnimKind::Keyframes => SequencerKind::Group,
        AnimKind::Call => SequencerKind::Call,
        AnimKind::Pause => SequencerKind::Pause,
    }
}

/// The scale a group applies to its children: its duration over its
/// content (1 unless the group was stretched), 1 for timelines.
fn group_scale(a: &AnimRef, kind: AnimKind) -> f64 {
    let inner = a.inner_duration();
    if matches!(kind, AnimKind::Stagger | AnimKind::Keyframes) && inner > 0.0 {
        a.duration() / inner
    } else {
        1.0
    }
}

/// Whether node `a` plays its children backwards in its parent's time: it
/// is reversed, unless the iteration its parent meets first (its last one)
/// is itself a mirrored yoyo pass.
fn plays_backwards(a: &AnimRef) -> bool {
    let r = a.repeat();
    a.reversed() && !(a.yoyo() && r > 0 && r % 2 == 1)
}

/// The frame node `a` (of `kind`, placed in `parent` at model `start`)
/// gives its children.
fn child_frame(a: &AnimRef, kind: AnimKind, parent: Frame, start: f64, k: f64) -> Frame {
    let group = kind != AnimKind::Timeline;
    Frame {
        origin: start,
        k: k * group_scale(a, kind),
        mirror: parent.mirror != plays_backwards(a),
        span: if group {
            a.inner_duration()
        } else {
            a.duration()
        },
    }
}

fn engine_rows(
    e: &TweenEngine,
    id: TweenId,
    at: RowAt,
    out: &mut Vec<SequencerTrack>,
    name_of: &mut impl FnMut(TweenId, Tag) -> String,
) {
    let a = e.anim_ref(id);
    let Some(kind) = a.kind() else {
        return;
    };
    let f = at.frame;
    // The root's own time scale and direction are not applied: model time is
    // its local time.
    let kk = if at.root {
        1.0
    } else {
        f.k / time_scale_div(&a)
    };
    let start = if at.root {
        0.0
    } else {
        f.place(a.start_time(), local_len(&a))
    };
    let delay = if at.root || f.mirror {
        0.0
    } else {
        a.delay() * f.k
    };
    let label = match kind {
        AnimKind::Tween => {
            let mut s = format!("{:?}", a.ease());
            if let Some((cut, _)) = s.char_indices().nth(24) {
                s.truncate(cut);
            }
            s
        }
        AnimKind::Stagger => format!("stagger x{}", a.child_count()),
        AnimKind::Keyframes => "keyframes".to_string(),
        AnimKind::Timeline => String::new(),
        AnimKind::Call | AnimKind::Pause => name_of(id, a.tag()),
    };
    let skind = kind_of(kind);
    out.push(SequencerTrack {
        id: id.to_bits(),
        name: name_of(id, a.tag()),
        color: None,
        kind: skind,
        depth: at.depth,
        expanded: skind != SequencerKind::Group,
        has_children: a.child_count() > 0,
        items: vec![SequencerItem {
            id: id.to_bits(),
            start,
            delay,
            duration: a.duration() * kk,
            repeat: a.repeat(),
            repeat_delay: a.repeat_delay() * kk,
            yoyo: a.yoyo(),
            reversed: !at.root && (f.mirror != a.reversed()),
            label,
            locked: at.locked,
        }],
    });
    if matches!(
        kind,
        AnimKind::Timeline | AnimKind::Stagger | AnimKind::Keyframes
    ) {
        let group = kind != AnimKind::Timeline;
        let frame = if at.root {
            Frame {
                k: group_scale(&a, kind),
                ..Frame::ROOT
            }
        } else {
            child_frame(&a, kind, f, start, kk)
        };
        let child = RowAt {
            depth: at.depth + 1,
            frame,
            locked: (at.locked && !at.root) || group,
            root: false,
        };
        for c in a.children() {
            engine_rows(e, c, child, out, name_of);
        }
    }
}

/// The frame of the children of `p` under `root` (as [`engine_rows`] builds
/// it).
fn edit_frame(e: &TweenEngine, root: TweenId, p: TweenId, depth: u32) -> Option<Frame> {
    let a = e.anim_ref(p);
    let kind = a.kind()?;
    if p == root {
        return Some(Frame {
            k: group_scale(&a, kind),
            ..Frame::ROOT
        });
    }
    if depth > 256 {
        return None;
    }
    let f = edit_frame(e, root, a.parent()?, depth + 1)?;
    let start = f.place(a.start_time(), local_len(&a));
    Some(child_frame(&a, kind, f, start, f.k / time_scale_div(&a)))
}

/// The parent frame of an editable node `n` of `root`: `None` for `root`
/// itself, a stale id, a node outside `root`, or a group's child (read-only).
fn editable_frame(e: &TweenEngine, root: TweenId, n: TweenId) -> Option<Frame> {
    if n == root || !e.anim_ref(n).is_alive() {
        return None;
    }
    let p = e.anim_ref(n).parent()?;
    if e.anim_ref(p).kind()? != AnimKind::Timeline {
        return None;
    }
    let f = edit_frame(e, root, p, 0)?;
    (f.k > 0.0 && f.k.is_finite()).then_some(f)
}

/// Applies a timeline editor's edit to the timeline `root` of an engine
/// model made by [`SequencerModel::from_engine`]:
/// - `ItemMoved` sets the start (`set_start_time`, GSAP `startTime()`), so
///   the item's first iteration starts at the requested model time (inside
///   a reversed parent: ends there, mirrored);
/// - `ItemResized` makes one iteration last the requested model time: a
///   tween through `set_duration` (it keeps its progress), a timeline
///   through its time scale (so its repeats and repeat delay scale with it
///   and the bar reads back the dragged length); inside a reversed parent
///   the start is re-placed so the bar's left edge stays;
/// - `LabelMoved` re-adds an existing label of `root` at its new time.
///
/// Then `root` is refreshed so the values at the playhead follow. Every
/// other action, a stale or foreign id, a group's child, `root` itself and a
/// non-finite or non-positive value answer `false` and change nothing. The
/// host calls it (the editor never sees the engine) and then rebuilds the
/// model.
pub fn apply_sequencer_edit(e: &mut TweenEngine, root: TweenId, a: SequencerAction) -> bool {
    let done = match a {
        SequencerAction::ItemMoved { id, start } => {
            let n = TweenId::from_bits(id);
            match editable_frame(e, root, n) {
                Some(f) if start.is_finite() => {
                    let len = local_len(&e.anim_ref(n));
                    e.anim(n).set_start_time(f.local(start, len));
                    true
                }
                _ => false,
            }
        }
        SequencerAction::ItemResized { id, duration } => {
            let n = TweenId::from_bits(id);
            match editable_frame(e, root, n) {
                Some(f) if duration.is_finite() && duration > 0.0 => {
                    resize(e, root, n, f, duration)
                }
                _ => false,
            }
        }
        SequencerAction::LabelMoved { id, time } => {
            let r = e.anim_ref(root);
            if time.is_finite() && r.label_time(Tag(id)).is_some() {
                e.tl(root).add_label(Tag(id), Position::at(time));
                true
            } else {
                false
            }
        }
        _ => false,
    };
    if done {
        e.anim(root).refresh();
    }
    done
}

fn resize(e: &mut TweenEngine, root: TweenId, n: TweenId, f: Frame, duration: f64) -> bool {
    let a = e.anim_ref(n);
    // Where the bar starts now: a mirrored parent re-places it afterwards.
    let model_start = f.place(a.start_time(), local_len(&a));
    if a.kind() == Some(AnimKind::Timeline) {
        // The drawn iteration is `duration() * k / |ts|`: set the scale that
        // makes it the dragged length (GSAP's `duration(v)` would spread a
        // new TOTAL over repeats whose delay is in local time instead).
        let content = a.duration();
        if !(content > 0.0) {
            return false;
        }
        let sign = if a.reversed() { -1.0 } else { 1.0 };
        e.anim(n).set_time_scale(sign * content * f.k / duration);
    } else {
        let local = duration / f.k * time_scale_div(&a);
        e.anim(n).set_duration(local);
    }
    if f.mirror {
        if let Some(f2) = editable_frame(e, root, n) {
            let len = local_len(&e.anim_ref(n));
            e.anim(n).set_start_time(f2.local(model_start, len));
        }
    }
    true
}

#[cfg(test)]
mod grid_tests {
    use super::*;

    fn model() -> SequencerModel {
        let item = SequencerItem { id: 1, start: 2.0, duration: 4.0, repeat: 0, ..Default::default() };
        SequencerModel {
            duration: 60.0,
            start: -2.0,
            grid: Some(SequencerGrid { origin: 1.0, step: 0.5, per_major: 4 }),
            tracks: vec![SequencerTrack { id: 10, items: vec![item], ..Default::default() }],
            ..Default::default()
        }
    }

    #[test]
    fn a_grid_counts_majors_and_steps_from_its_origin_and_before_it() {
        let g = model().grid.unwrap();
        assert_eq!(g.place(0), (1, 1));
        assert_eq!(g.place(5), (2, 2));
        assert_eq!(g.place(-1), (0, 4));
        assert_eq!(g.place(-4), (0, 1));
        assert_eq!(g.place(-5), (-1, 4));
        assert_eq!(g.snap(1.9, 4.0), 1.0);
        assert_eq!(g.snap(1.9, 1.0), 2.0);
        assert_eq!(g.snap(-0.4, 4.0), -1.0);
    }

    #[test]
    fn a_drag_lands_on_the_grid_unless_it_snapped_to_a_target() {
        let m = model();
        let d = m.drag_move(1, 2.3, &[], 0.1).unwrap();
        let g = m.snap_drag_to_grid(1, d, SequencerEdge::Move, 4.0);
        assert_eq!((g.start, g.duration), (3.0, 4.0));
        let g = m.snap_drag_to_grid(1, d, SequencerEdge::Move, 1.0);
        assert_eq!(g.start, 2.5);
        // Back into the count-in, to the axis start at most.
        let d = m.drag_move(1, -1.2, &[], 0.1).unwrap();
        let g = m.snap_drag_to_grid(1, d, SequencerEdge::Move, 4.0);
        assert_eq!(g.start, -1.0);
        // A target won: left as it is.
        let d = m.drag_move(1, 2.3, &[2.31], 0.1).unwrap();
        assert_eq!(m.snap_drag_to_grid(1, d, SequencerEdge::Move, 4.0), d);
        // Resizes: the dragged edge on the grid, at least a unit long.
        let d = m.drag_resize_end(1, 6.6, &[], 0.1, 0.01).unwrap();
        let g = m.snap_drag_to_grid(1, d, SequencerEdge::End, 4.0);
        assert_eq!((g.start, g.duration), (2.0, 5.0));
        let d = m.drag_resize_end(1, 2.2, &[], 0.1, 0.01).unwrap();
        let g = m.snap_drag_to_grid(1, d, SequencerEdge::End, 4.0);
        assert_eq!(g.duration, 2.0);
        let d = m.drag_resize_start(1, 2.8, 6.0, &[], 0.1, 0.01).unwrap();
        let g = m.snap_drag_to_grid(1, d, SequencerEdge::Start, 1.0);
        assert_eq!((g.start, g.duration), (3.0, 3.0));
    }
}
