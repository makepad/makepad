use {
    crate::{
        animator::AnimatorImpl,
        event::{ScrollPhase, TouchState, TAP_COUNT_DISTANCE},
        flat_list::WidgetItem,
        makepad_derive_widget::*,
        makepad_draw::*,
        scroll_bar::{ScrollAxis, ScrollBar, ScrollBarAction},
        scroll_motion::{
            estimate_release_velocity, press_settles_finger_scroll, push_sample,
            rubber_band_bounce, soften_bounce_velocity, stretch_displayed, stretch_raw, Fling,
            FrameClock, MomentumStream, ScrollSample, CATCH_PRESS_WINDOW, FLING_BOOST_MAX_DWELL,
            FLING_DECEL_RATE_PER_MS, FLING_MIN_TOTAL_DELTA, PER_FRAME_TO_PER_SECOND,
            RUBBER_BAND_TOUCH_RANGE,
        },
        widget::*,
        widget_async::CxSplashVmExt,
        widget_tree::CxWidgetExt,
    },
    std::collections::HashMap,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The quad one continuation row is drawn with. It carries no colours of
    // its own, so nothing anybody can see is drawn until a list says what its
    // ruling is.
    set_type_default() do #(DrawFillerQuad::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_row: uniform(#0000)
        color_row_alt: uniform(#0000)
        pixel: fn() {
            return self.color_row.mix(self.color_row_alt, self.alternate)
        }
    }

    mod.widgets.PortalListBase = #(PortalList::register_widget(vm))

    mod.widgets.PortalList = set_type_default() do mod.widgets.PortalListBase {
        width: Fill
        height: Fill
        capture_overload: true
        scroll_bar: mod.widgets.ScrollBar {}
        flow: Down

        // The ruling the continuation rows carry on with: the same theme pair
        // every other ruled list in the library stripes by, so a list whose
        // own rows use the house colours agrees with its own filler for free.
        // Drawn only when `filler_rows` is on, which it is not by default.
        filler +: {
            color_row: theme.color_bg_even
            color_row_alt: theme.color_bg_odd
        }

        // The kinetic knobs, restated here so the design overlay can reach
        // them. A `#[live]` default alone gives the panel a value with no
        // bounds, so it renders as a bare number with no scrubber; the
        // bounds live in the annotation, and the annotation lives in the
        // DSL. These are the only such knobs the overlay can reach at all:
        // it expands an object one level, so a View's `scroll_bars.` rows
        // stop at the bar itself, while this list's are its own fields.
        /** the slowest release that still throws the list 0.05..2 step 0.05 */
        flick_scroll_minimum: 0.2
        /** the fastest a hard flick may throw it 20..600 step 10 */
        flick_scroll_maximum: 240.0
        // Interpolated rather than copied: the default is one number in
        // scroll_motion.rs and restating it here by hand is how the two
        // quietly stop agreeing.
        /** how fast a flick runs down, per millisecond 0.98..0.9999 step 0.0005 */
        fling_decel: #(crate::scroll_motion::FLING_DECEL_RATE_PER_MS)
        /** rubber-band past the top edge 0..1 step 1 */
        bounce_at_start: true
        /** rubber-band past the bottom edge 0..1 step 1 */
        bounce_at_end: true
        /** how fast the tail follows new content 0.05..1 step 0.05 */
        smooth_tail_speed: 0.25
        // Named here only so the panel can say what they are. Both are read
        // by nothing: a knob that silently does nothing is worse than one
        // that says it does nothing.
        /** DEAD: the fling speed comes from the measured release now */
        flick_scroll_scaling: 0.005
        /** DEAD: the run-down rate is fling_decel now */
        flick_scroll_decay: 0.97
    }
}

/// The maximum number of items that will be shown as part of a smooth scroll animation.
const SMOOTH_SCROLL_MAXIMUM_WINDOW: usize = 20;

/// How many frames a `smooth_scroll_to_end` animation takes, whatever the
/// distance, so a long list doesn't crawl at a fixed pixels-per-frame rate.
const SMOOTH_SCROLL_TO_END_FRAMES: f64 = 24.0;

/// The quad one continuation row is drawn with.
///
/// `alternate` is `0.0` on an even row index and `1.0` on an odd one, which is
/// the parity a host's own row background stripes by, so the ruling carries on
/// in the colour the next real row would have had.
#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawFillerQuad {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    alternate: f32,
}

/// The most continuation rows one draw will put in the gap. A pitch smaller
/// than [`FILLER_MIN_PITCH`] is refused outright, so this only ever bites on a
/// viewport tall enough to want hundreds of rows, where the ones past it are
/// off screen anyway.
const MAX_FILLER_ROWS: usize = 256;

/// The smallest gap worth ruling, and the smallest row pitch worth ruling it
/// with, in pixels. Below either, the filler draws nothing: sub-pixel rows are
/// a hairline of the wrong colour, not a table.
const FILLER_MIN_PITCH: f64 = 0.5;

/// Where a short list's continuation rows go, relative to the viewport's own
/// start along the scroll axis.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FillerBand {
    /// The edge of the content the rows continue from.
    origin: f64,
    /// How much room there is to rule.
    gap: f64,
    /// `1` when the rows run on past the content, `-1` when they run back
    /// before it.
    step: isize,
}

/// How many continuation rows fit in a gap, and how tall the last one is.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FillerRun {
    /// The number of rows to draw, the last of them clamped.
    rows: usize,
    /// The size of the final row, which is what was left over.
    last: f64,
}

/// Everything the continuation rows need, worked out while the drawn rows are
/// still on hand and used once the borrow on them is released.
#[derive(Clone, Copy)]
struct FillerPlan {
    /// The viewport the rows run inside.
    viewport: Rect,
    /// Which way they run, from where, and how far.
    band: FillerBand,
    /// One row's size along the scroll axis, taken from the row the filler
    /// continues from: this list has no uniform row height, so the pitch can
    /// only come from a row that was actually measured.
    pitch: f64,
    /// The index of that row, which is where the striping's parity continues
    /// from.
    from_index: usize,
}

enum ScrollState {
    Stopped,
    Drag {
        samples: Vec<ScrollSample>,
        /// The initial touch position along the scroll axis at FingerDown.
        initial_abs: f64,
        /// Whether this drag has exceeded the minimum threshold to be treated
        /// as scroll rather than a tap. Until committed, no scroll deltas are applied.
        committed: bool,
    },
    Flick {
        /// The momentum-fling animation state (native iOS-style exponential decay,
        /// frame-rate-independent). Shared with ScrollBar via [`crate::scroll_motion`]
        /// so every scrollable widget decelerates identically.
        fling: Fling,
        next_frame: NextFrame,
        /// True while the fling is pinned at a non-bouncing edge: it keeps
        /// decaying silently (no movement, no redraws, presses are ordinary
        /// clicks), and resumes if content grows past the edge — the same
        /// contract as `MomentumStream::Pinned`, so a touch flick also
        /// continues naturally through pagination.
        parked: bool,
    },
    Pulldown {
        next_frame: NextFrame,
        /// Overscroll offset when the bounce began.
        x0: f64,
        /// Overscroll speed (px/s, positive into the overscroll) when the bounce began,
        /// carried in from the fling. Softened against the overscroll headroom on the
        /// bounce's first frame (see `scroll_motion::soften_bounce_velocity`).
        v0: f64,
        /// Jitter-smoothed animation time, started on the first bounce frame.
        clock: FrameClock,
        /// Whether this bounce is at the start (top) edge or the end (bottom) edge.
        at_start: bool,
        /// Whether a finger on the screen caused this bounce (touch/mouse drag or
        /// the flick it released), which springs back with the stronger iOS curve,
        /// rather than trackpad momentum (see `scroll_motion`).
        touch: bool,
    },
    ScrollingTo {
        target_id: usize,
        delta: f64,
        next_frame: NextFrame,
        /// Pixel offset from the top of the viewport where the target item's
        /// top edge should end up once scrolling completes.
        top_offset: f64,
    },
    Tailing {
        next_frame: NextFrame,
        /// Current scroll velocity for smooth animation
        velocity: f64,
    },
}

/// Auto-scroll state while selecting text beyond viewport bounds
struct SelectScrollState {
    next_frame: NextFrame,
    last_abs: DVec2,
}

#[derive(Clone)]
enum ListDrawState {
    Begin,
    Down {
        index: usize,
        pos: f64,
        viewport: Rect,
    },
    Up {
        index: usize,
        pos: f64,
        hit_bottom: bool,
        viewport: Rect,
    },
    DownAgain {
        index: usize,
        pos: f64,
        viewport: Rect,
    },
    End {
        viewport: Rect,
    },
}

#[derive(Clone, Debug, Default)]
pub enum PortalListAction {
    /// The viewport position changed since the last completed draw. Emitted at
    /// most once per frame, from the draw, so every kind of scrolling (events,
    /// animations, draw-side renormalization) reports identically.
    Scroll,
    /// The start of the list is now on screen (within `reached_start_margin`
    /// items). Sent once; not repeated while the start stays on screen. Sent
    /// again after the start goes off screen and comes back, or after
    /// [`PortalListRef::forget_reached_edges`]. Infinite lists load more
    /// history when they get this.
    ReachedStart,
    /// The end of the list is now on screen (within `reached_end_margin`
    /// items); same behavior as [`Self::ReachedStart`].
    ReachedEnd,
    SmoothScrollReached,
    #[default]
    None,
}

impl ListDrawState {
    fn is_down_again(&self) -> bool {
        matches!(self, Self::DownAgain { .. })
    }
}

struct AlignItem {
    align_range: TurtleAlignRange,
    size: Vec2d,
    shift: f64,
    index: usize,
}

/// Cache for computing average item height
#[derive(Default)]
struct HeightCache {
    /// Running sum of measured heights
    measured_sum: f64,
    /// Count of measured items
    measured_count: usize,
}

impl HeightCache {
    fn record_height(&mut self, height: f64) {
        self.measured_sum += height;
        self.measured_count += 1;
    }

    fn average(&self) -> f64 {
        if self.measured_count > 0 {
            self.measured_sum / self.measured_count as f64
        } else {
            30.0 // reasonable default
        }
    }

    fn _reset(&mut self) {
        self.measured_sum = 0.0;
        self.measured_count = 0;
    }
}

/// Fenwick tree (Binary Indexed Tree) for O(log n) prefix sum queries on item heights.
/// Enables fast mapping between virtual scroll position and item index.
struct HeightTree {
    /// 1-indexed tree array storing partial sums
    tree: Vec<f64>,
    /// Number of items
    size: usize,
    /// Default height for unmeasured items
    default_height: f64,
    /// Tracks which items have been measured
    measured: Vec<bool>,
}

impl HeightTree {
    /// Create a new tree for `size` items, all initialized to `default_height`
    fn new(size: usize, default_height: f64) -> Self {
        if size == 0 {
            return HeightTree {
                tree: Vec::new(),
                size: 0,
                default_height,
                measured: Vec::new(),
            };
        }

        // Build the tree with all items having default_height
        let mut tree = vec![0.0; size + 1]; // 1-indexed

        // Initialize: each position contributes default_height
        for i in 1..=size {
            tree[i] += default_height;
            let parent = i + (i & i.wrapping_neg());
            if parent <= size {
                tree[parent] += tree[i];
            }
        }

        HeightTree {
            tree,
            size,
            default_height,
            measured: vec![false; size],
        }
    }

    /// Get the prefix sum of heights from index 0 to i (inclusive)
    fn prefix_sum(&self, i: usize) -> f64 {
        if self.size == 0 {
            return 0.0;
        }
        let i = i.min(self.size - 1);
        let mut sum = 0.0;
        let mut j = i + 1; // convert to 1-indexed
        while j > 0 {
            sum += self.tree[j];
            j -= j & j.wrapping_neg(); // clear lowest set bit
        }
        sum
    }

    /// Get the height at a specific index
    fn point_query(&self, i: usize) -> f64 {
        if i >= self.size {
            return self.default_height;
        }
        if i == 0 {
            self.prefix_sum(0)
        } else {
            self.prefix_sum(i) - self.prefix_sum(i - 1)
        }
    }

    /// Update the height at index i to new_height.
    /// Returns true when this is the item's first measurement or its stored
    /// height actually changed, so callers can react only to real changes.
    fn update(&mut self, i: usize, new_height: f64) -> bool {
        if i >= self.size {
            return false;
        }

        let first_measurement = !self.measured[i];
        let old_height = self.point_query(i);
        let delta = new_height - old_height;

        self.measured[i] = true;

        if delta.abs() < 0.001 {
            // No significant change
            return first_measurement;
        }

        let mut j = i + 1; // convert to 1-indexed
        while j <= self.size {
            self.tree[j] += delta;
            j += j & j.wrapping_neg(); // add lowest set bit
        }
        true
    }

    /// Get the total sum of all heights
    fn total(&self) -> f64 {
        if self.size == 0 {
            return 0.0;
        }
        self.prefix_sum(self.size - 1)
    }

    /// Binary search to find the item index where cumulative height reaches target.
    /// Returns (item_index, offset_within_item)
    fn find_position(&self, target: f64) -> (usize, f64) {
        if self.size == 0 || target <= 0.0 {
            return (0, 0.0);
        }

        let total = self.total();
        if target >= total {
            // Beyond the end
            return (
                self.size.saturating_sub(1),
                self.point_query(self.size.saturating_sub(1)),
            );
        }

        // Binary search using the Fenwick tree structure
        let mut pos = 0usize;
        let mut sum = 0.0;
        let mut bit = (self.size + 1).next_power_of_two() >> 1;

        while bit > 0 {
            let next_pos = pos + bit;
            if next_pos <= self.size && sum + self.tree[next_pos] < target {
                pos = next_pos;
                sum += self.tree[pos];
            }
            bit >>= 1;
        }

        // pos is now the index (1-indexed) where prefix_sum < target
        // The target falls within item at index pos (0-indexed)
        let item_idx = pos; // convert back to 0-indexed
        let offset = target - sum;

        (item_idx.min(self.size.saturating_sub(1)), offset.max(0.0))
    }

    /// Resize the tree when range changes - extends efficiently, only recreates if shrinking
    fn resize(&mut self, new_size: usize, default_height: f64) {
        if new_size == self.size {
            return;
        }

        if new_size > self.size {
            // Extend the tree - add new items with default_height
            let old_size = self.size;
            self.size = new_size;
            self.tree.resize(new_size + 1, 0.0);
            self.measured.resize(new_size, false);

            // Add each new item to the tree
            for i in old_size..new_size {
                let mut j = i + 1; // 1-indexed
                while j <= new_size {
                    self.tree[j] += default_height;
                    j += j & j.wrapping_neg();
                }
            }
        } else {
            // Shrinking - rebuild (rare case, e.g., clearing chat)
            *self = HeightTree::new(new_size, default_height);
        }
    }

    /// Update the default height for unmeasured items
    fn update_default_height(&mut self, new_default: f64) {
        // Re-applying the default walks every unmeasured item, which is costly
        // on large lists, so only do it once the average has drifted noticeably.
        // Unmeasured heights are estimates anyway, so sub-pixel drift is fine.
        if (new_default - self.default_height).abs() < 0.5 {
            return;
        }

        let delta = new_default - self.default_height;
        self.default_height = new_default;

        // Update all unmeasured items
        for i in 0..self.size {
            if !self.measured[i] {
                let mut j = i + 1;
                while j <= self.size {
                    self.tree[j] += delta;
                    j += j & j.wrapping_neg();
                }
            }
        }
    }
}

#[derive(Script, WidgetRegister, WidgetRef, WidgetSet)]
pub struct PortalList {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,

    #[rust]
    area: Area,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[rust]
    range_start: usize,
    #[rust(usize::MAX)]
    range_end: usize,
    #[rust(0usize)]
    view_window: usize,
    #[rust(0usize)]
    visible_items: usize,

    /// The minimum release speed for a fling, in per-frame pixels at a nominal 60fps
    /// (×60 → px/s). Below this a finger lift is a stop, not a flick; an active fling
    /// also stops once it decays below this speed.
    #[live(0.2)]
    flick_scroll_minimum: f64,
    /// The maximum fling speed, in per-frame pixels at a nominal 60fps (×60 → px/s).
    /// 240 → 14,400 px/s. This is the ceiling on how fast a hard flick can throw the
    /// list; raise it for even faster flicks, lower it to tame them.
    #[live(240.0)]
    flick_scroll_maximum: f64,
    /// Deprecated: unused. The fling speed now comes directly from the tracked release
    /// velocity (see [`crate::scroll_motion`]); kept only so existing DSL doesn't break.
    #[live(0.005)]
    flick_scroll_scaling: f64,
    /// Deprecated: unused. The deceleration rate is now `fling_decel`; kept so existing DSL
    /// doesn't break.
    #[live(0.97)]
    flick_scroll_decay: f64,
    /// Per-ms velocity decay of a touch-drag flick; lower stops sooner (see `scroll_motion`).
    #[live(FLING_DECEL_RATE_PER_MS)]
    fling_decel: f64,
    /// Where the OS trackpad momentum stream stands for this list (see `scroll_motion`).
    /// While it is `Live`, deltas move the list even though `scroll_state` stays `Stopped`.
    #[rust] momentum: MomentumStream,
    /// Current rubber-band overshoot past the end (bottom) edge, in pixels. Driven by
    /// the bounce animation and applied by the draw as an extra upward shift.
    #[rust] bounce_overshoot: f64,
    /// True while a finger-driven gesture is actively stretching the rubber band at
    /// either edge, so the draw displays the stretch instead of pinning it flush.
    #[rust] stretching: bool,
    /// When a trackpad touch stopped live scroll motion. The press belonging to that
    /// same tap (arriving separately, at finger lift) is consumed as the stop rather
    /// than delivered as a click.
    #[rust] touch_caught_motion_at: Option<f64>,
    /// Wall-clock time of the last finger-driven scroll delta (trackpad Began/Changed),
    /// so presses during active scrolling count as stops rather than clicks. `None` until
    /// a finger has actually scrolled this list — see [`press_settles_finger_scroll`].
    #[rust] last_finger_scroll_time: Option<f64>,
    /// The `(velocity, time)` of a fling the user just caught with a press. A quick
    /// same-direction re-flick adds this speed back (fling boost), so repeated
    /// flicks build up speed; consumed by the press's release either way.
    #[rust] caught_fling: Option<(f64, f64)>,
    /// Viewport position (`first_id`, `first_scroll`, `bounce_overshoot`) of the most
    /// recent `Scroll` action, compared at the end of each draw. Scrolling of every kind
    /// funnels through a draw, so this coalesces notification to at most one action per
    /// frame in which the viewport actually moved, wherever the motion came from
    /// (events, animations, or draw-side renormalization).
    #[rust] last_notified_pos: (usize, f64, f64),
    /// Whether we already told the app the start of the list is on screen
    /// (so `ReachedStart` is sent once, not on every draw).
    #[rust] told_reached_start: bool,
    /// Whether we already told the app the end of the list is on screen.
    #[rust] told_reached_end: bool,
    /// Whether the last mouse move was inside the list, so the move that leaves
    /// is still forwarded to items (their hover-out) before fan-out stops.
    #[rust] pointer_was_inside: bool,
    /// Whether content rubber-bands past the start (top) edge. The stretch and the
    /// bounce follow the incoming gesture's momentum (see `scroll_motion`); there is
    /// no fixed cap.
    #[live(true)]
    bounce_at_start: bool,
    /// Whether content rubber-bands past the end (bottom) edge.
    #[live(true)]
    bounce_at_end: bool,
    /// Whether to emit [`PortalListAction::Scroll`] whenever the viewport moves, at
    /// most once per frame. On by default; lists with no scroll-position consumers
    /// can disable it to skip the per-frame action. The one-shot edge sentinels
    /// are controlled separately by [`Self::reached_start_margin`] and
    /// [`Self::reached_end_margin`].
    #[live(true)]
    emit_scroll_actions: bool,
    /// Whether (and how early) to emit [`PortalListAction::ReachedStart`]:
    /// `Some(0)` (the default) fires exactly when the first item is drawn; a
    /// small margin lets an infinite list prefetch (e.g. paginate history)
    /// shortly before the user actually hits the top; `nil`/`None` never emits.
    #[live(Some(0u32))]
    reached_start_margin: Option<u32>,
    /// Whether (and how early) to emit [`PortalListAction::ReachedEnd`]
    /// (see [`Self::reached_start_margin`]). `Some(0)` is the default.
    #[live(Some(0u32))]
    reached_end_margin: Option<u32>,
    #[live(true)]
    align_top_when_empty: bool,
    #[live(false)]
    grab_key_focus: bool,
    #[live(true)]
    drag_scrolling: bool,
    /// The minimum distance (in pixels) a finger/mouse must move before a drag is
    /// "committed" and treated as a scroll gesture rather than a tap/click.
    /// This prevents accidental micro-scrolling when tapping interactive items.
    /// Defaults to `TAP_COUNT_DISTANCE`, keeping it consistent with the
    /// platform's tap-vs-drag distinction.
    #[rust(TAP_COUNT_DISTANCE)]
    drag_scroll_threshold: f64,

    /// Whether the current gesture should be suppressed from child widgets.
    /// Set when either:
    /// - A drag scroll commits (finger moves past threshold), or
    /// - A finger-down/click occurs while a scroll animation was in progress
    ///   (the tap-to-stop-scroll gesture).
    /// Cleared on the next finger-down that arrives when not scrolling.
    #[rust]
    suppress_child_events: bool,

    #[rust]
    first_id: usize,
    #[rust]
    first_scroll: f64,
    #[rust(Vec2Index::X)]
    vec_index: Vec2Index,

    #[live]
    scroll_bar: ScrollBar,
    #[live]
    capture_overload: bool,
    #[live(false)]
    keep_invisible: bool,
    #[live(true)]
    skip_widget_tree_search: bool,

    #[rust]
    draw_state: DrawStateWrap<ListDrawState>,
    #[rust]
    draw_align_list: Vec<AlignItem>,
    #[rust]
    detect_tail_in_draw: bool,

    #[live(false)]
    auto_tail: bool,

    #[live(false)]
    smooth_tail: bool,
    /// Speed factor for smooth tail animation (0.0-1.0, lower = slower). Default 0.25.
    #[live(0.25)]
    smooth_tail_speed: f64,

    #[rust(false)]
    tail_range: bool,
    #[rust(0.0)]
    tail_adjustment_needed: f64,
    #[rust(false)]
    at_end: bool,
    #[rust(true)]
    not_filling_viewport: bool,
    #[live(false)]
    reuse_items: bool,

    /// Whether a list whose rows don't fill the viewport carries its ruling on
    /// past the last row, so the leftover space reads as blank paper rather
    /// than as the list stopping mid-air.
    ///
    /// Off by default, and worth turning on only for a list that is already
    /// ruled: a table, a ledger, a directory listing. On a list drawn over a
    /// plain ground it would sprout stripes out of nowhere.
    ///
    /// The rows are drawn with [`Self::filler`], at the pitch of the row the
    /// filler continues from — this list has no uniform row height, so the
    /// pitch can only come from a row that was measured — and the last one is
    /// clamped to whatever room is left. A list resting at its end instead of
    /// its start (`align_top_when_empty: false`) keeps its gap at the leading
    /// edge, so the ruling runs the other way, back before the first row. An
    /// empty list draws none of this: an empty state is what answers that.
    #[live(false)]
    filler_rows: bool,
    /// The quad one continuation row is drawn with; see [`Self::filler_rows`].
    #[live]
    filler: DrawFillerQuad,

    /// A row the host asked to keep on screen, honoured at the end of the next
    /// draw and then forgotten. `None` — the default — leaves the list
    /// positioned by scrolling alone.
    #[rust]
    keep_visible_in_draw: Option<usize>,
    /// How many rows a recalled row lands short of the edge it came back over
    /// (see [`Self::keep_index_visible`]). One row by default: a row pinned
    /// flush against an edge reads as the end of the list, with nothing beyond
    /// it to say the list goes on.
    #[live(1)]
    keep_visible_lead: usize,

    // Templates stored as rooted ScriptObjectRef - populated in on_after_apply
    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    items: ComponentMap<usize, WidgetItem>,
    #[rust]
    reusable_items: HashMap<LiveId, Vec<WidgetItem>>,

    #[rust(ScrollState::Stopped)]
    scroll_state: ScrollState,
    /// Whether the PortalList was actively scrolling during the most recent finger down hit.
    #[rust]
    was_scrolling: bool,
    /// The running total of all user-driven scrolling of this list.
    /// See [`PortalListRef::user_scroll_travel()`].
    #[rust]
    user_scroll_travel: f64,

    // Cross-boundary text selection support
    /// Enable text selection across items (for TextFlow content)
    #[live(false)]
    pub selectable: bool,
    /// Selection anchor point (item_id, char_index)
    #[rust]
    selection_anchor: Option<(usize, usize)>,
    /// Selection cursor point (item_id, char_index)
    #[rust]
    selection_cursor: Option<(usize, usize)>,
    /// Whether currently in a selection drag
    #[rust]
    is_selecting: bool,
    /// Auto-scroll state during selection
    #[rust]
    select_scroll_state: Option<SelectScrollState>,

    // Pixel-based scrollbar support
    /// Height tree for O(log n) scroll position lookups
    #[rust]
    height_tree: Option<HeightTree>,
    /// Cache for computing average item height
    #[rust]
    height_cache: HeightCache,
}

impl ScriptHook for PortalList {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.templates.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Collect templates from the object's vec - only vec key IDs (name) end up in the vec
        // Only collect during template applies (not eval) to avoid storing temporary objects
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        // Templates use vec key ids (name) - they end up in the vec
                        if let Some(id) = kv.key.as_id() {
                            if let Some(template_obj) = kv.value.as_object() {
                                // Root the template object so it survives GC
                                self.templates
                                    .insert(id, vm.bx.heap.new_object_ref(template_obj));
                            }
                        }
                    }
                });
            }
        }

        // Update existing items if templates changed
        if apply.is_reload() {
            for (_, item) in self.items.iter_mut() {
                if let Some(template_ref) = self.templates.get(&item.template) {
                    let template_value: ScriptValue = template_ref.as_object().into();
                    item.widget.script_apply(vm, apply, scope, template_value);
                }
            }
        }

        // Set vec_index based on flow
        if let Flow::Down = self.layout.flow {
            self.vec_index = Vec2Index::Y;
        } else {
            self.vec_index = Vec2Index::X;
        }

        if self.auto_tail {
            self.tail_range = true;
        }
    }
}

impl PortalList {
    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        // The outer turtle wraps the inner item turtle (Fill cross-axis, Fit
        // main-axis). If we let `self.layout.align` apply here, a non-zero
        // main-axis align would shift the inner turtle as a whole when items
        // don't fill the viewport — showing up as leading empty space at the
        // start of the list. Drop align on the outer turtle: the user's
        // `align` config is reserved for the inner item turtle (cross-axis
        // only — see `next_visible_item`).
        let outer_layout = Layout {
            align: Align::default(),
            ..self.layout
        };
        cx.begin_turtle(walk, outer_layout);
        self.draw_align_list.clear();
    }

    fn end(&mut self, cx: &mut Cx2d) {
        // Note: we intentionally do NOT reset `at_end` here. It is explicitly
        // set to true or false in every code path below. If the draw cycle
        // doesn't reach the calculation (e.g., draw_state isn't End, or list
        // is empty), we preserve the previous value rather than introducing
        // a spurious `false` that would cause downstream consumers to flicker.
        self.not_filling_viewport = false;

        let vi = self.vec_index;
        let mut visible_items = 0;
        let mut filler_plan = None;

        if let Some(ListDrawState::End { viewport }) = self.draw_state.get() {
            let list = &mut self.draw_align_list;
            if !list.is_empty() {
                list.sort_by(|a, b| a.index.cmp(&b.index));
                let first_index = list.iter().position(|v| v.index == self.first_id).unwrap();

                let mut first_pos = self.first_scroll;
                for i in (0..first_index).rev() {
                    let item = &list[i];
                    first_pos -= item.size.index(vi);
                }

                let mut last_pos = self.first_scroll;
                let mut last_item_pos = None;
                let mut last_drawn_index = None;
                // The size of that same last row, which is the pitch the
                // continuation rows take when there are any.
                let mut last_drawn_size = 0.0;
                for i in first_index..list.len() {
                    let item = &list[i];
                    last_pos += item.size.index(vi);
                    if item.index < self.range_end {
                        last_item_pos = Some(last_pos);
                        last_drawn_index = Some(item.index);
                        last_drawn_size = item.size.index(vi);
                    } else {
                        break;
                    }
                }
                // Whether the very last item in the range was actually drawn.
                // The draw loop can stop early when it encounters a zero-size
                // item (e.g., an empty placeholder widget), which would leave
                // `last_item_pos` far short of the viewport bottom. Without
                // this guard, `at_end` could become a false positive whenever
                // a zero-size item appears in the middle of the visible range.
                let drew_last_item = last_drawn_index == Some(self.range_end.saturating_sub(1));

                // How far the two branches below move the whole run of rows
                // from where `first_pos`/`last_item_pos` measured it. Adding it
                // back is what puts the content's two edges in the viewport's
                // own coordinates, which is all the filler needs. Both branches
                // set it before anything reads it.
                let content_shift;
                let mut total_at_start = None;
                if list[0].index == self.range_start {
                    let mut total = 0.0;
                    for item in list.iter() {
                        if item.index >= self.range_end {
                            break;
                        }
                        total += item.size.index(vi);
                    }
                    self.not_filling_viewport = total < viewport.size.index(vi);
                    total_at_start = Some(total);
                }

                if list.first().unwrap().index == self.range_start && first_pos > 0.0 {
                    // We're at the top of the list with a gap above the first item.
                    // We're also at the end if all content fits in the viewport.
                    self.at_end = self.not_filling_viewport && drew_last_item;

                    // Content shorter than the viewport with bottom alignment rests
                    // with a deliberate gap above the first item (the else-branch
                    // below creates it); that gap is layout, not overscroll, so the
                    // pin must leave it alone instead of snapping the content back
                    // to the top on alternating draws.
                    let resting_gap = match total_at_start {
                        Some(total) if !self.align_top_when_empty && self.not_filling_viewport => {
                            (viewport.size.index(vi) - total).max(0.0)
                        }
                        _ => 0.0,
                    };

                    let min = match &self.scroll_state {
                        // The bounce spring and a live drag own the gap; its size comes
                        // from the rubber-band math, not a fixed cap. Only when this
                        // edge bounces, though: every legitimate start gap is created
                        // under bounce_at_start, so on a non-bouncing start edge any
                        // gap is renormalization backlog and pins immediately.
                        ScrollState::Pulldown { .. } | ScrollState::Drag { .. }
                            if self.bounce_at_start =>
                        {
                            f64::INFINITY
                        }
                        // A finger-driven stretch (trackpad fingers still down) also
                        // displays its gap; anything else pins to the edge, because a
                        // renormalization backlog from a fast coast is a layout artifact,
                        // not physical travel, and must not show as a giant bounce.
                        _ if self.stretching && self.bounce_at_start => f64::INFINITY,
                        _ => resting_gap,
                    };

                    let mut pos = first_pos.min(min);
                    content_shift = pos - first_pos;
                    for item in list.iter() {
                        let shift = Vec2d::from_index_pair(vi, pos, 0.0);
                        cx.shift_align_range(
                            &item.align_range,
                            shift - Vec2d::from_index_pair(vi, item.shift, 0.0),
                        );
                        pos += item.size.index(vi);
                        visible_items += 1;
                    }
                    self.first_scroll = first_pos.min(min);
                    self.first_id = self.range_start;
                } else {
                    let shift = if let Some(last_item_pos) = last_item_pos {
                        if self.align_top_when_empty && self.not_filling_viewport {
                            // All items fit in the viewport without filling it.
                            self.at_end = drew_last_item;
                            -first_pos
                        } else {
                            let ret = viewport.size.index(vi) - last_item_pos;
                            // Use a 1px tolerance for floating-point accumulation
                            // errors across item sizes, and require that the last
                            // item in the range was actually drawn.
                            self.at_end = ret >= -1.0 && drew_last_item;
                            if self.bounce_overshoot > 0.0
                                && self.at_end
                                && matches!(
                                    self.scroll_state,
                                    ScrollState::Pulldown { at_start: false, .. }
                                        | ScrollState::Stopped
                                        | ScrollState::Drag { .. }
                                )
                            {
                                // The bottom rubber band shifts the content up past
                                // flush, opening a gap below the last item.
                                ret.max(0.0) - self.bounce_overshoot
                            } else {
                                ret.max(0.0)
                            }
                        }
                    } else {
                        self.at_end = false;
                        0.0
                    };

                    content_shift = shift;
                    let mut first_id_changed = false;
                    let start_pos = self.first_scroll + shift;
                    let mut pos = start_pos;
                    for i in (0..first_index).rev() {
                        let item = &list[i];
                        let visible = pos > 0.0;
                        pos -= item.size.index(vi);
                        let shift = Vec2d::from_index_pair(vi, pos, 0.0);
                        cx.shift_align_range(
                            &item.align_range,
                            shift - Vec2d::from_index_pair(vi, item.shift, 0.0),
                        );
                        if visible {
                            self.first_scroll = pos;
                            self.first_id = item.index;
                            first_id_changed = true;
                            if item.index < self.range_end {
                                visible_items += 1;
                            }
                        }
                    }

                    let mut pos = start_pos;
                    for i in first_index..list.len() {
                        let item = &list[i];
                        let shift = Vec2d::from_index_pair(vi, pos, 0.0);
                        cx.shift_align_range(
                            &item.align_range,
                            shift - Vec2d::from_index_pair(vi, item.shift, 0.0),
                        );
                        pos += item.size.index(vi);
                        let invisible = pos < 0.0;
                        if invisible {
                            self.first_scroll = pos - item.size.index(vi);
                            self.first_id = item.index;
                            first_id_changed = true;
                        } else if item.index < self.range_end {
                            visible_items += 1;
                        }
                    }

                    if !first_id_changed {
                        self.first_scroll = start_pos;
                    }
                }

                // Where the continuation rows go, worked out here where the
                // drawn rows and their settled positions are both on hand. The
                // drawing itself waits for the borrow on the rows to be
                // released, at the end of this block.
                if self.filler_rows && self.not_filling_viewport {
                    if let (Some(last_index), Some(last_pos)) =
                        (last_drawn_index, last_item_pos)
                    {
                        let first = &list[0];
                        let (from_index, pitch) = if self.align_top_when_empty {
                            (last_index, last_drawn_size)
                        } else {
                            (first.index, first.size.index(vi))
                        };
                        filler_plan = filler_band(
                            self.align_top_when_empty,
                            first_pos + content_shift,
                            last_pos + content_shift,
                            viewport.size.index(vi),
                        )
                        .map(|band| FillerPlan {
                            viewport,
                            band,
                            pitch,
                            from_index,
                        });
                    }
                }

                // Capture measured heights into height_tree and height_cache
                for item in list.iter() {
                    if item.index >= self.range_start && item.index < self.range_end {
                        let height = item.size.index(vi);
                        let idx = item.index - self.range_start;

                        if let Some(ref mut tree) = self.height_tree {
                            // Fold the height into the running average only on first
                            // measurement or a real change; re-recording every visible
                            // item per frame would weight the average by on-screen time.
                            if tree.update(idx, height) {
                                self.height_cache.record_height(height);
                            }
                        } else {
                            self.height_cache.record_height(height);
                        }
                    }
                }

                // Update unmeasured items with new average if it changed significantly
                if let Some(ref mut tree) = self.height_tree {
                    let new_avg = self.height_cache.average();
                    tree.update_default_height(new_avg);
                }

                // An active scroll gesture/animation — a momentum fling, a pulldown bounce, or a
                // finger drag — owns `first_scroll` directly. The tail auto-scroll below re-asserts
                // position by subtracting `overflow` from `first_scroll` on every draw; during such
                // an animation that races and cancels its smooth sub-pixel motion. At the slow end of
                // a flick (or while dragging near the bottom), where the real per-frame motion is
                // sub-pixel, this snap-back dominates and the content shakes in place instead of
                // moving cleanly. So skip computing/applying the tail adjustment while a gesture owns
                // the scroll position (symmetrical with the scroll-bar guard above); auto-tail
                // re-evaluates cleanly once the gesture settles.
                let animation_owns_scroll = matches!(
                    self.scroll_state,
                    ScrollState::Flick { .. } | ScrollState::Pulldown { .. } | ScrollState::Drag { .. }
                );

                // When tail_range is true and we're not already at the end, we need to scroll
                // down to keep the bottom of the content visible
                if self.tail_range && !self.at_end {
                    // Calculate how much we need to scroll to get to the end
                    // The shift calculation above tells us: viewport_height - last_item_pos
                    // When this is negative, content extends beyond viewport
                    if let Some(last_pos) = last_item_pos {
                        let viewport_height = viewport.size.index(vi);
                        let overflow = last_pos - viewport_height;
                        // Don't store an adjustment while an animation owns the scroll position —
                        // applying it would fight the active fling/drag (see note above).
                        if overflow > 0.5 && !animation_owns_scroll {
                            // Content extends beyond viewport, store the adjustment needed
                            self.tail_adjustment_needed = overflow;
                        }
                    }
                }

                // Apply tail scroll adjustment - scroll down to keep bottom visible
                if self.tail_adjustment_needed > 0.5 {
                    if animation_owns_scroll {
                        // Discard any pending adjustment so it can't apply mid-fling (or on the
                        // first post-fling frame), which would fight the momentum animation.
                        self.tail_adjustment_needed = 0.0;
                    } else if self.smooth_tail {
                        // Start or continue smooth tailing animation via scroll state
                        if !matches!(self.scroll_state, ScrollState::Tailing { .. }) {
                            // Start new tailing animation with initial velocity
                            self.scroll_state = ScrollState::Tailing {
                                next_frame: cx.new_next_frame(),
                                velocity: 0.0,
                            };
                        }
                        // Note: if already Tailing, the event handler will schedule next frames
                        // and the velocity will naturally adapt to the accumulated tail_adjustment_needed
                    } else {
                        // Instant jump: adjust first_scroll to scroll down
                        self.first_scroll -= self.tail_adjustment_needed;
                        self.tail_adjustment_needed = 0.0;
                        self.area.redraw(cx);
                    }
                }
            }
        }

        // After the rows and before the scroll bar: the ruling is the ground
        // the rows sit on, and the bar sits over both.
        if let Some(plan) = filler_plan {
            self.draw_filler_rows(cx, plan);
        }

        let rect = cx.turtle().rect();
        if self.at_end || self.view_window == 0 || self.view_window > visible_items {
            self.view_window = visible_items.max(4) - 3;
        }
        if self.detect_tail_in_draw {
            self.detect_tail_in_draw = false;
            if self.auto_tail && self.at_end {
                self.tail_range = true;
            }
        }

        // Use pixel-based total from height_tree, fallback to old calculation
        let virtual_total = if let Some(ref tree) = self.height_tree {
            tree.total()
        } else {
            let total_views =
                (self.range_end - self.range_start) as f64 / self.view_window.max(1) as f64;
            rect.size.index(vi) * total_views
        };

        // Position first so the handle drawn THIS frame already reflects a
        // programmatic reposition (set_first_id_and_scroll) instead of
        // lagging one frame; re-asserted after the draw once view_total is
        // known so the clamp is exact.
        if !self.scroll_bar.animator_in_state(cx, ids!(hover.drag)) {
            self.update_scroll_bar(cx);
        }
        match self.vec_index {
            Vec2Index::Y => {
                self.scroll_bar.draw_scroll_bar(
                    cx,
                    ScrollAxis::Vertical,
                    rect,
                    dvec2(100.0, virtual_total),
                );
            }
            Vec2Index::X => {
                self.scroll_bar.draw_scroll_bar(
                    cx,
                    ScrollAxis::Horizontal,
                    rect,
                    dvec2(virtual_total, 100.0),
                );
            }
        }

        // Update scroll bar position AFTER draw_scroll_bar sets view_total
        // This ensures the position is clamped correctly
        if !self.scroll_bar.animator_in_state(cx, ids!(hover.drag)) {
            self.update_scroll_bar(cx);
        }

        // Keep items when selecting so we can copy text from scrolled-out items.
        // When a selection is active (but drag finished), keep selected items alive
        // so their selection state persists when scrolled back into view.
        if !self.keep_invisible && !self.is_selecting {
            let selection_range = self.get_selection_range();
            if self.reuse_items {
                let reusable_items = &mut self.reusable_items;
                self.items.retain_visible_with(|v: WidgetItem| {
                    reusable_items.entry(v.template).or_default().push(v);
                });
            } else if let Some((start, end)) = selection_range {
                self.items
                    .retain_visible_and(|item_id, _| *item_id >= start.0 && *item_id <= end.0);
            } else {
                self.items.retain_visible();
            }
            cx.widget_tree_mark_dirty(self.uid);
        }

        cx.end_turtle_with_area(&mut self.area);
        self.visible_items = visible_items;
    
        // The coast is over the instant the list is drawn at the edge it was headed
        // toward; the rest of the OS stream (which can outlive the edge by a second
        // or more) must not keep moving it. A bouncing edge takes the remaining
        // momentum into the rubber-band spring. A non-bouncing edge pins, with the
        // stream left running silently: if content grows past the edge (e.g.
        // pagination prepending items), the same stream resumes moving the list, so
        // the original flick continues naturally through the load.
        if let MomentumStream::Live { last_delta_time, velocity, direction } = self.momentum {
            let reached_end = direction < 0.0 && self.at_end;
            let reached_start = direction > 0.0
                && self.first_id == self.range_start
                && self.first_scroll >= 0.0;
            if reached_end || reached_start {
                let edge_bounces = if reached_start {
                    self.bounce_at_start
                } else {
                    self.bounce_at_end
                };
                if edge_bounces && velocity.abs() > 0.0 {
                    self.momentum = MomentumStream::Idle;
                    if reached_start {
                        self.first_scroll = 0.0;
                    }
                    self.scroll_state = ScrollState::Pulldown {
                        next_frame: cx.new_next_frame(),
                        x0: 0.0,
                        v0: velocity.abs(),
                        clock: FrameClock::default(),
                        at_start: reached_start,
                        touch: false,
                    };
                } else {
                    self.momentum = MomentumStream::Pinned { last_delta_time };
                }
            }
        }

        // Tell the app when an end of the list comes on screen — once, not on
        // every draw. While it stays on screen we stay quiet; once it goes off
        // screen, coming back gets announced again.
        if let Some(margin) = self.reached_start_margin {
            let start_on_screen = self.first_id <= self.range_start + margin as usize;
            if start_on_screen && !self.told_reached_start {
                cx.widget_action(self.widget_uid(), PortalListAction::ReachedStart);
            }
            self.told_reached_start = start_on_screen;
        }
        if let Some(margin) = self.reached_end_margin {
            let end_on_screen = self.at_end
                || self.first_id + self.visible_items + margin as usize >= self.range_end;
            if end_on_screen && !self.told_reached_end {
                cx.widget_action(self.widget_uid(), PortalListAction::ReachedEnd);
            }
            self.told_reached_end = end_on_screen;
        }

        // Scrolling of every kind funnels through a draw, so one comparison here
        // notifies every viewport move — event-driven, animated, or draw-side
        // renormalization — at most once per frame.
        if self.emit_scroll_actions {
            let pos = (self.first_id, self.first_scroll, self.bounce_overshoot);
            if pos != self.last_notified_pos {
                self.last_notified_pos = pos;
                cx.widget_action(self.widget_uid(), PortalListAction::Scroll);
            }
        }

        // A row the host asked to hold, judged last of all: against the window
        // this draw actually landed on, so a list whose rows moved under it is
        // measured where it now is rather than where it was, and after the
        // notifications above, which belong to the position just drawn.
        self.apply_keep_visible(cx);
}

    /// Draws the continuation rows of a short list: one quad per row, at the
    /// pitch of the row the ruling continues from, with the row nearest the
    /// viewport edge clamped to what is left. Nothing is allocated here — the
    /// run is two numbers and the quad is reused for every row.
    fn draw_filler_rows(&mut self, cx: &mut Cx2d, plan: FillerPlan) {
        let vi = self.vec_index;
        let run = filler_run(plan.band.gap, plan.pitch, MAX_FILLER_ROWS);
        let mut pos = plan.band.origin;
        for n in 0..run.rows {
            let size = if n + 1 == run.rows { run.last } else { plan.pitch };
            // Running backwards, a row is laid out from its own far edge.
            let start = if plan.band.step > 0 { pos } else { pos - size };
            self.filler.alternate =
                filler_alternate(plan.from_index, plan.band.step * (n as isize + 1));
            let rect = match vi {
                Vec2Index::Y => Rect {
                    pos: dvec2(plan.viewport.pos.x, plan.viewport.pos.y + start),
                    size: dvec2(plan.viewport.size.x, size),
                },
                Vec2Index::X => Rect {
                    pos: dvec2(plan.viewport.pos.x + start, plan.viewport.pos.y),
                    size: dvec2(size, plan.viewport.size.y),
                },
            };
            self.filler.draw_abs(cx, rect);
            pos += if plan.band.step > 0 { size } else { -size };
        }
    }

    /// Returns the index of the next visible item that will be drawn by this PortalList.
    pub fn next_visible_item(&mut self, cx: &mut Cx2d) -> Option<usize> {
        let vi = self.vec_index;
        // Propagate only the cross-axis component of the PortalList's own
        // `align` to the inner item turtle, so items shorter than the bar's
        // cross-axis size get centered. Main-axis align is intentionally
        // dropped: items are stacked along the flow direction and may
        // overflow (PortalList scrolls), so main-axis centering would either
        // create leading empty space when items don't fill the viewport or
        // misalign with the scroll origin. The default Align{x:0,y:0}
        // preserves prior top-left item behavior.
        let layout = if vi == Vec2Index::Y {
            Layout::flow_down().with_align_x(self.layout.align.x)
        } else {
            Layout::flow_right().with_align_y(self.layout.align.y)
        };

        if let Some(draw_state) = self.draw_state.get() {
            match draw_state {
                ListDrawState::Begin => {
                    let viewport = cx.turtle().inner_rect();
                    self.draw_state.set(ListDrawState::Down {
                        index: self.first_id,
                        pos: self.first_scroll,
                        viewport,
                    });
                    match vi {
                        Vec2Index::Y => {
                            cx.begin_turtle(
                                Walk {
                                    abs_pos: Some(dvec2(
                                        viewport.pos.x,
                                        viewport.pos.y + self.first_scroll,
                                    )),
                                    margin: Default::default(),
                                    width: Size::fill(),
                                    height: Size::fit(),
                                    ..Default::default()
                                },
                                layout,
                            );
                        }
                        Vec2Index::X => {
                            cx.begin_turtle(
                                Walk {
                                    abs_pos: Some(dvec2(
                                        viewport.pos.x + self.first_scroll,
                                        viewport.pos.y,
                                    )),
                                    margin: Default::default(),
                                    width: Size::fit(),
                                    height: Size::fill(),
                                    ..Default::default()
                                },
                                layout,
                            );
                        }
                    }
                    return Some(self.first_id);
                }
                ListDrawState::Down {
                    index,
                    pos,
                    viewport,
                }
                | ListDrawState::DownAgain {
                    index,
                    pos,
                    viewport,
                } => {
                    let is_down_again = draw_state.is_down_again();
                    let did_draw = cx.turtle_has_align_items();
                    let align_range = cx.get_turtle_align_range();
                    let rect = cx.end_turtle();
                    self.draw_align_list.push(AlignItem {
                        align_range,
                        shift: pos,
                        size: rect.size,
                        index,
                    });

                    if !did_draw || pos + rect.size.index(vi) > viewport.size.index(vi) {
                        if self.first_id > 0 && !is_down_again {
                            self.draw_state.set(ListDrawState::Up {
                                index: self.first_id - 1,
                                pos: self.first_scroll,
                                hit_bottom: index >= self.range_end,
                                viewport,
                            });
                            match vi {
                                Vec2Index::Y => {
                                    cx.begin_turtle(
                                        Walk {
                                            abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y)),
                                            margin: Default::default(),
                                            width: Size::fill(),
                                            height: Size::fit(),
                                            ..Default::default()
                                        },
                                        layout,
                                    );
                                }
                                Vec2Index::X => {
                                    cx.begin_turtle(
                                        Walk {
                                            abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y)),
                                            margin: Default::default(),
                                            width: Size::fit(),
                                            height: Size::fill(),
                                            ..Default::default()
                                        },
                                        layout,
                                    );
                                }
                            }
                            return Some(self.first_id - 1);
                        } else {
                            self.draw_state.set(ListDrawState::End { viewport });
                            return None;
                        }
                    }
                    if is_down_again {
                        self.draw_state.set(ListDrawState::DownAgain {
                            index: index + 1,
                            pos: pos + rect.size.index(vi),
                            viewport,
                        });
                    } else {
                        self.draw_state.set(ListDrawState::Down {
                            index: index + 1,
                            pos: pos + rect.size.index(vi),
                            viewport,
                        });
                    }
                    match vi {
                        Vec2Index::Y => {
                            cx.begin_turtle(
                                Walk {
                                    abs_pos: Some(dvec2(
                                        viewport.pos.x,
                                        viewport.pos.y + pos + rect.size.index(vi),
                                    )),
                                    margin: Default::default(),
                                    width: Size::fill(),
                                    height: Size::fit(),
                                    ..Default::default()
                                },
                                layout,
                            );
                        }
                        Vec2Index::X => {
                            cx.begin_turtle(
                                Walk {
                                    abs_pos: Some(dvec2(
                                        viewport.pos.x + pos + rect.size.index(vi),
                                        viewport.pos.y,
                                    )),
                                    margin: Default::default(),
                                    width: Size::fit(),
                                    height: Size::fill(),
                                    ..Default::default()
                                },
                                layout,
                            );
                        }
                    }
                    return Some(index + 1);
                }
                ListDrawState::Up {
                    index,
                    pos,
                    hit_bottom,
                    viewport,
                } => {
                    let did_draw = cx.turtle_has_align_items();
                    let align_range = cx.get_turtle_align_range();
                    let rect = cx.end_turtle();
                    self.draw_align_list.push(AlignItem {
                        align_range,
                        size: rect.size,
                        shift: 0.0,
                        index,
                    });
                    if index == self.range_start {
                        if pos - rect.size.index(vi) > 0.0 {
                            if let Some(last_index) =
                                self.draw_align_list.iter().map(|v| v.index).max()
                            {
                                let total_height: f64 =
                                    self.draw_align_list.iter().map(|v| v.size.index(vi)).sum();
                                self.draw_state.set(ListDrawState::DownAgain {
                                    index: last_index + 1,
                                    pos: total_height,
                                    viewport,
                                });
                                // Axis-aware like the Down path: a horizontal
                                // list's re-entry item must NOT be a Fill-wide
                                // row — it was measured at the whole viewport
                                // width, which poisoned the size tree (the
                                // scrollbar handle changed length while
                                // scrolling a horizontal list).
                                match vi {
                                    Vec2Index::Y => cx.begin_turtle(
                                        Walk {
                                            abs_pos: Some(dvec2(
                                                viewport.pos.x,
                                                viewport.pos.y + total_height,
                                            )),
                                            margin: Default::default(),
                                            width: Size::fill(),
                                            height: Size::fit(),
                                            ..Default::default()
                                        },
                                        layout,
                                    ),
                                    Vec2Index::X => cx.begin_turtle(
                                        Walk {
                                            abs_pos: Some(dvec2(
                                                viewport.pos.x + total_height,
                                                viewport.pos.y,
                                            )),
                                            margin: Default::default(),
                                            width: Size::fit(),
                                            height: Size::fill(),
                                            ..Default::default()
                                        },
                                        layout,
                                    ),
                                }
                                return Some(last_index + 1);
                            }
                        }
                        self.draw_state.set(ListDrawState::End { viewport });
                        return None;
                    }

                    if !did_draw
                        || pos
                            < if hit_bottom {
                                -viewport.size.index(vi)
                            } else {
                                0.0
                            }
                    {
                        self.draw_state.set(ListDrawState::End { viewport });
                        return None;
                    }

                    self.draw_state.set(ListDrawState::Up {
                        index: index - 1,
                        hit_bottom,
                        pos: pos - rect.size.index(vi),
                        viewport,
                    });

                    // Same axis rule for every further "before first_id" item.
                    match vi {
                        Vec2Index::Y => cx.begin_turtle(
                            Walk {
                                abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y)),
                                margin: Default::default(),
                                width: Size::fill(),
                                height: Size::fit(),
                                ..Default::default()
                            },
                            layout,
                        ),
                        Vec2Index::X => cx.begin_turtle(
                            Walk {
                                abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y)),
                                margin: Default::default(),
                                width: Size::fit(),
                                height: Size::fill(),
                                ..Default::default()
                            },
                            layout,
                        ),
                    }

                    return Some(index - 1);
                }
                _ => (),
            }
        }
        None
    }

    /// Creates a new widget from the given `template` or returns an existing widget,
    /// if one already exists with the same `entry_id`.
    pub fn item(&mut self, cx: &mut Cx, entry_id: usize, template: LiveId) -> WidgetRef {
        self.item_with_existed(cx, entry_id, template).0
    }

    /// Creates a new widget from the given `template` or returns an existing widget,
    /// if one already exists with the same `entry_id` and `template`.
    pub fn item_with_existed(
        &mut self,
        cx: &mut Cx,
        entry_id: usize,
        template: LiveId,
    ) -> (WidgetRef, bool) {
        use std::collections::hash_map::Entry;

        if let Some(template_ref) = self.templates.get(&template) {
            let template_value: ScriptValue = template_ref.as_object().into();
            // Instantiate items in the VM whose heap actually minted the template.
            // Using `cx.with_vm` (the main VM) here would dereference an isolate-heap
            // object id against the main heap and panic out-of-bounds. Resolving from
            // the template ref itself is exact; for a non-isolated (main-app) list it
            // yields MAIN_SPLASH_VM_ID, i.e. exactly `cx.with_vm`, so normal usage is
            // unchanged.
            let Some(vm_id) = cx.script_ref_vm_id(template_ref) else {
                // The isolate that minted this template has been reclaimed, so
                // the list is on its way out too. An empty item is the only
                // honest answer: instantiating from a dead heap is what the
                // comment above warns about.
                return (WidgetRef::empty(), false);
            };
            match self.items.entry(entry_id) {
                Entry::Occupied(mut occ) => {
                    if occ.get().template == template {
                        (occ.get().widget.clone(), true)
                    } else {
                        let widget_ref = if let Some(reused) = self
                            .reusable_items
                            .get_mut(&template)
                            .and_then(|pool| pool.pop())
                        {
                            let widget_ref = reused.widget;
                            // Reused items must be reset to template defaults, otherwise
                            // stale instance/animator state (e.g. selected) can leak to a new entry.
                            cx.with_script_vm_id(vm_id, |vm| {
                                let mut widget_ref = widget_ref.clone();
                                widget_ref.script_apply(
                                    vm,
                                    &Apply::Reload,
                                    &mut Scope::empty(),
                                    template_value,
                                );
                            });
                            widget_ref
                        } else {
                            cx.with_script_vm_id(vm_id, |vm| {
                                WidgetRef::script_from_value(vm, template_value)
                            })
                        };
                        occ.insert(WidgetItem {
                            template,
                            widget: widget_ref.clone(),
                        });
                        cx.widget_tree_insert_child(
                            self.uid,
                            LiveId(entry_id as u64),
                            widget_ref.clone(),
                        );
                        (widget_ref, false)
                    }
                }
                Entry::Vacant(vac) => {
                    let widget_ref = if let Some(reused) = self
                        .reusable_items
                        .get_mut(&template)
                        .and_then(|pool| pool.pop())
                    {
                        let widget_ref = reused.widget;
                        // Reused items must be reset to template defaults, otherwise
                        // stale instance/animator state (e.g. selected) can leak to a new entry.
                        cx.with_script_vm_id(vm_id, |vm| {
                            let mut widget_ref = widget_ref.clone();
                            widget_ref.script_apply(
                                vm,
                                &Apply::Reload,
                                &mut Scope::empty(),
                                template_value,
                            );
                        });
                        widget_ref
                    } else {
                        cx.with_script_vm_id(vm_id, |vm| {
                            WidgetRef::script_from_value(vm, template_value)
                        })
                    };
                    vac.insert(WidgetItem {
                        template,
                        widget: widget_ref.clone(),
                    });
                    cx.widget_tree_insert_child(
                        self.uid,
                        LiveId(entry_id as u64),
                        widget_ref.clone(),
                    );
                    (widget_ref, false)
                }
            }
        } else {
            error!("Template not found: {template}. Did you add it to the <PortalList> instance?");
            (WidgetRef::empty(), false)
        }
    }

    /// Returns a reference to the template and widget for the given `entry_id`.
    pub fn get_item(&self, entry_id: usize) -> Option<(LiveId, WidgetRef)> {
        self.items
            .get(&entry_id)
            .map(|item| (item.template, item.widget.clone()))
    }

    /// Returns the current in-use items in this PortalList, keyed by entry id.
    ///
    /// This excludes widgets in the reusable pool.
    pub fn items(&self) -> &ComponentMap<usize, WidgetItem> {
        &self.items
    }

    pub fn set_item_range(&mut self, cx: &mut Cx, range_start: usize, range_end: usize) {
        let range_changed = self.range_start != range_start || self.range_end != range_end;
        self.range_start = range_start;

        if range_changed {
            self.range_end = range_end;

            // Initialize or resize the height tree
            let size = range_end.saturating_sub(range_start);
            let default_height = self.height_cache.average();

            if let Some(ref mut tree) = self.height_tree {
                tree.resize(size, default_height);
            } else {
                self.height_tree = Some(HeightTree::new(size, default_height));
            }

            if self.tail_range {
                self.first_id = self.range_end.max(1) - 1;
                self.first_scroll = 0.0;
            }
            self.update_scroll_bar(cx);
        }
    }

    pub fn update_scroll_bar(&mut self, cx: &mut Cx) {
        // Use pixel-based position from height_tree
        if let Some(ref tree) = self.height_tree {
            let first_idx = self.first_id.saturating_sub(self.range_start);

            // Get cumulative height up to (but not including) first_id
            let height_before = if first_idx > 0 {
                tree.prefix_sum(first_idx - 1)
            } else {
                0.0
            };

            // first_scroll is typically 0 or negative (item partially scrolled off top)
            // Negate it because negative first_scroll means we've scrolled down into the item
            let scroll_pos = (height_before - self.first_scroll).max(0.0);
            self.scroll_bar.set_scroll_pos_no_action(cx, scroll_pos);
        } else {
            // Fallback to old integer-based calculation
            let scroll_pos = ((self.first_id - self.range_start) as f64
                / ((self.range_end - self.range_start).max(self.view_window + 1) - self.view_window)
                    as f64)
                * self.scroll_bar.get_scroll_view_total();
            self.scroll_bar.set_scroll_pos_no_action(cx, scroll_pos);
        }
    }

    /// Instantly stops every kind of scroll motion — drag/fling/bounce animation,
    /// rubber-band overshoot, and any OS momentum stream — returning the list to its
    /// resting state. Explicit navigation (keyboard, programmatic scrolls) calls this
    /// first, so no leftover motion resumes from wherever the viewport lands.
    fn stop_all_scroll_motion(&mut self) {
        self.scroll_state = ScrollState::Stopped;
        self.momentum = MomentumStream::Idle;
        self.bounce_overshoot = 0.0;
        self.stretching = false;
        self.was_scrolling = false;
        self.caught_fling = None;
    }

    /// Whether this list keeps a scroll delta it is about to apply (positive
    /// toward the start), judged from where it rests now; see
    /// [`list_keeps_scroll_delta`]. `finger_driven` is a trackpad gesture with
    /// the fingers down, the only input that stretches a rubber band.
    fn keeps_scroll_delta(&self, delta: f64, finger_driven: bool) -> bool {
        let empty = self.range_start == self.range_end;
        let at_start = empty || (self.first_id == self.range_start && self.first_scroll >= 0.0);
        let at_end = empty || self.at_end;
        let band = finger_driven
            && !empty
            && ((delta > 0.0 && self.bounce_at_start)
                || (delta < 0.0 && self.bounce_at_end)
                || (self.first_id == self.range_start && self.first_scroll > 0.0)
                || self.bounce_overshoot > 0.0);
        list_keeps_scroll_delta(delta, at_start, at_end, band)
    }

    /// Whether a press at `time` belongs to a touch that just stopped live motion.
    fn press_is_catch(&self, time: f64) -> bool {
        self.touch_caught_motion_at
            .is_some_and(|at| time - at < CATCH_PRESS_WINDOW)
    }

    /// Whether anything is moving the list at `time`: an animation state, a live
    /// coast, or active finger-driven scrolling. Presses during any of these are
    /// stops, not clicks.
    fn motion_live(&self, time: f64) -> bool {
        matches!(
            self.scroll_state,
            ScrollState::Flick { parked: false, .. }
                | ScrollState::Pulldown { .. }
                | ScrollState::ScrollingTo { .. }
                | ScrollState::Tailing { .. }
        ) || self.momentum.is_live(time)
            || press_settles_finger_scroll(self.last_finger_scroll_time, time)
    }

    fn delta_top_scroll(
        &mut self,
        cx: &mut Cx,
        delta: f64,
        clip_top: bool,
        transition_to_pulldown: bool,
        bounce_velocity: f64,
        touch: bool,
        user_driven: bool,
    ) {
        let mut delta = delta;
        if user_driven {
            self.user_scroll_travel += delta;
        }
        let fingers_down = !clip_top && !transition_to_pulldown;
        let extent = self.area.rect(cx).size.index(self.vec_index);

        // A finger-driven gesture past the end edge stretches the rubber band
        // instead of scrolling. A finger on the screen follows the loose iOS
        // curve, a trackpad finger the stiffer linear one (see `scroll_motion`);
        // movement back toward the content unwinds along the same curve first.
        if fingers_down
            && self.bounce_at_end
            && self.at_end
            && (delta < 0.0 || self.bounce_overshoot > 0.0)
        {
            let raw = stretch_raw(self.bounce_overshoot, extent, touch) - delta;
            if raw >= 0.0 {
                self.bounce_overshoot = stretch_displayed(raw, extent, touch);
                delta = 0.0;
            } else {
                self.bounce_overshoot = 0.0;
                delta = -raw;
            }
        }

        if self.range_start == self.range_end {
            self.first_scroll = 0.0;
        } else if self.first_id == self.range_start
            && self.bounce_at_start
            && fingers_down
            && delta > 0.0
            && self.first_scroll + delta > 0.0
        {
            // Same rubber band past the start edge: split the delta at the edge
            // and damp only the part beyond it.
            let into = (-self.first_scroll).max(0.0).min(delta);
            let over = delta - into;
            let raw = stretch_raw(self.first_scroll.max(0.0), extent, touch) + over;
            self.first_scroll =
                self.first_scroll.min(0.0) + into + stretch_displayed(raw, extent, touch);
        } else if self.first_id == self.range_start
            && self.bounce_at_start
            && fingers_down
            && touch
            && delta < 0.0
            && self.first_scroll > 0.0
        {
            // A finger moving back toward the content unwinds the stretch along
            // the same curve it stretched by, so the content tracks the finger;
            // only travel beyond the unwind scrolls the content again.
            let raw = stretch_raw(self.first_scroll, extent, true) + delta;
            if raw >= 0.0 {
                self.first_scroll = stretch_displayed(raw, extent, true);
            } else {
                self.first_scroll = raw;
            }
        } else {
            self.first_scroll += delta;
        }

        // One huge downward wheel delta (a fast coast) used to leave the whole
        // travel as negative first_scroll backlog: the draw loop then
        // instantiates and draws every row between the old viewport and the
        // new one (dy=8000 drew ~200 live rows down to y=-7391), only for
        // end() to renormalise afterwards. Convert any beyond-a-viewport
        // backlog into a first_id jump through the height tree first — the
        // same mapping a scroll-bar drag lands by — so the draw only ever
        // walks the rows around the viewport.
        if extent > 0.0 && self.first_scroll < -extent {
            if let Some(tree) = self.height_tree.as_ref() {
                let first_idx = self.first_id.saturating_sub(self.range_start);
                let height_before =
                    if first_idx > 0 { tree.prefix_sum(first_idx - 1) } else { 0.0 };
                let target = (height_before - self.first_scroll).max(0.0);
                let (item_idx, offset) = tree.find_position(target);
                self.first_id = self.range_start + item_idx;
                self.first_scroll = -offset;
            }
        }

        if self.first_id == self.range_start {
            if !self.bounce_at_start {
                self.first_scroll = self.first_scroll.min(0.0);
            }
            if transition_to_pulldown && self.bounce_at_start && self.first_scroll > 0.0 {
                self.scroll_state = ScrollState::Pulldown {
                    next_frame: cx.new_next_frame(),
                    x0: self.first_scroll,
                    v0: bounce_velocity,
                    clock: FrameClock::default(),
                    at_start: true,
                    touch,
                };
            }
        }
        if clip_top && self.first_id == self.range_start && self.first_scroll > 0.0 {
            self.first_scroll = 0.0;
        }
        if self.at_end && delta < 0.0 {
            self.was_scrolling = false;
            if transition_to_pulldown
                && self.bounce_at_end
                && bounce_velocity < 0.0
                && !matches!(self.scroll_state, ScrollState::Pulldown { .. })
            {
                // A fling reaching the end bounces there too, seeded with the
                // remaining momentum.
                self.scroll_state = ScrollState::Pulldown {
                    next_frame: cx.new_next_frame(),
                    x0: 0.0,
                    v0: -bounce_velocity,
                    clock: FrameClock::default(),
                    at_start: false,
                    touch,
                };
            } else {
                self.scroll_state = ScrollState::Stopped;
            }
        }
        self.stretching = fingers_down
            && ((self.first_id == self.range_start && self.first_scroll > 0.0)
                || self.bounce_overshoot > 0.0);
        self.update_scroll_bar(cx);
    }

    /// Returns `true` if currently at the end of the list.
    pub fn is_at_end(&self) -> bool {
        self.at_end
    }

    /// Enables or disables auto-tracking the last item in the list.
    pub fn set_tail_range(&mut self, tail_range: bool) {
        self.tail_range = tail_range;
    }

    /// Keeps the row at `index` on screen.
    ///
    /// A row that is already showing holds the list exactly where it is: this
    /// is the whole point of asking for it rather than scrolling to the row,
    /// which would move a list that had no need to move. A row that has gone
    /// off either edge is brought back the shortest way, landing
    /// `keep_visible_lead` rows short of the edge it came back over so it is
    /// not pinned flush against it.
    ///
    /// Ask for this whenever the row the host cares about may have moved under
    /// the list: after rows were inserted or removed above it (its index has
    /// changed, so the list is showing a different stretch of the data than
    /// the host thinks), or when the current item is chosen somewhere else
    /// entirely and this list is one of the places that shows it. It is a
    /// one-shot request, honoured at the end of the next draw and then
    /// forgotten, so the user is free to scroll the row off afterwards — the
    /// list never drags it back on its own.
    ///
    /// Two positions the list already holds win over the request, and it is
    /// dropped rather than fought: a list that is tailing (`auto_tail` or
    /// [`Self::set_tail_range`]) is already following its last row, and a
    /// finger or a fling that owns the scroll is the user's, not the host's.
    pub fn keep_index_visible(&mut self, cx: &mut Cx, index: usize) {
        self.keep_visible_in_draw = Some(index);
        self.area.redraw(cx);
    }

    /// Consumes a [`Self::keep_index_visible`] request at the end of a draw,
    /// where the window that was actually drawn is known.
    fn apply_keep_visible(&mut self, cx: &mut Cx2d) {
        let Some(index) = self.keep_visible_in_draw.take() else {
            return;
        };
        if index >= self.range_end {
            return;
        }
        // The tail owns a tailing list, and a gesture owns a list being
        // scrolled; either way the request has nothing to correct.
        if self.tail_range
            || matches!(
                self.scroll_state,
                ScrollState::Flick { .. }
                    | ScrollState::Pulldown { .. }
                    | ScrollState::Drag { .. }
            )
        {
            return;
        }
        if let Some(first_id) = first_id_keeping_index_visible(
            index,
            self.first_id,
            self.visible_items,
            self.range_start,
            self.keep_visible_lead,
        ) {
            self.first_id = first_id;
            self.first_scroll = 0.0;
            // The list was repositioned by code, so showing an end of it now
            // is news again — the same rule `set_first_id_and_scroll` follows.
            self.forget_reached_edges();
            self.area.redraw(cx);
        }
    }

    /// Sets the flow direction, e.g. to switch a list between a vertical
    /// (`Flow::Down`) and horizontal (`Flow::right()`) layout at runtime.
    pub fn set_flow(&mut self, cx: &mut Cx, flow: Flow) {
        if self.layout.flow != flow {
            self.layout.flow = flow;
            // `vec_index` is the axis the list actually lays out along; it's normally
            // derived from `flow` in `on_after_apply`, so keep it in sync here too.
            self.vec_index = if let Flow::Down = flow { Vec2Index::Y } else { Vec2Index::X };
            self.redraw(cx);
        }
    }

    /// Sets the first visible item and scroll offset.
    pub fn set_first_id_and_scroll(&mut self, first_id: usize, first_scroll: f64) {
        self.first_id = first_id;
        // A positive offset on the first item means a gap above the start edge;
        // when that edge doesn't bounce, pin to it rather than letting a
        // programmatic reposition conjure overscroll no gesture could create.
        self.first_scroll = if first_id == self.range_start && !self.bounce_at_start {
            first_scroll.min(0.0)
        } else {
            first_scroll
        };
        // The list was repositioned by code, so showing the start/end now is
        // news again (e.g. a list re-purposed for other content).
        self.forget_reached_edges();
    }

    /// Forget that we already announced the start/end being on screen, so the
    /// next draw that shows one announces it again.
    fn forget_reached_edges(&mut self) {
        self.told_reached_start = false;
        self.told_reached_end = false;
    }

    /// Returns the number of items that are currently visible in the viewport.
    pub fn visible_items(&self) -> usize {
        self.visible_items
    }

    /// The item currently shown first. A list whose content shrank under it
    /// needs this to notice that its viewport is now past the end.
    /// One past the last item id set by `set_item_range`.
    pub fn range_end(&self) -> usize {
        self.range_end
    }

    /// The current scroll offset of `first_id`'s top relative to the
    /// viewport start (paired with [`Self::set_first_id_and_scroll`] for
    /// programmatic nudges; the draw pass renormalizes and clamps).
    pub fn first_scroll(&self) -> f64 {
        self.first_scroll
    }

    pub fn first_id(&self) -> usize {
        self.first_id
    }

    /// Computes the top position of `target_id` relative to the viewport top
    /// using `first_id`, `first_scroll`, and the height tree.
    ///
    /// Returns `None` if the height tree is not yet available (before the first draw).
    /// A return value of `0.0` means the item's top is exactly at the viewport top;
    /// negative means it is above the viewport, positive means below.
    fn item_top_from_height_tree(&self, target_id: usize) -> Option<f64> {
        let tree = self.height_tree.as_ref()?;
        let first_idx = self.first_id.saturating_sub(self.range_start);
        let target_idx = target_id.saturating_sub(self.range_start);

        let prefix = |i: usize| if i > 0 { tree.prefix_sum(i - 1) } else { 0.0 };

        if target_id >= self.first_id {
            Some(self.first_scroll + prefix(target_idx) - prefix(first_idx))
        } else {
            Some(self.first_scroll - (prefix(first_idx) - prefix(target_idx)))
        }
    }

    /// Initiates a smooth scrolling animation to the specified target item.
    ///
    /// If the target item's top is already visible within the viewport, no scrolling
    /// occurs and [`PortalListAction::SmoothScrollReached`] is emitted immediately.
    ///
    /// Otherwise, the list animates until the target item's top edge is positioned at
    /// `top_offset` pixels below the viewport's top edge. A value of `0.0` places the
    /// item flush with the viewport top; `20.0` leaves a 20 px margin. Negative values
    /// are clamped to `0.0`.
    pub fn smooth_scroll_to(
        &mut self,
        cx: &mut Cx,
        target_id: usize,
        speed: f64,
        max_items_to_show: Option<usize>,
        top_offset: f64,
    ) {
        if self.items.is_empty() {
            return;
        }
        if target_id < self.range_start || target_id > self.range_end {
            return;
        }

        // Check if the target item's top is already visible in the viewport.
        // Compute the target's top position relative to viewport using first_id,
        // first_scroll, and the height_tree — this avoids relying on widget rects
        // which may be clipped for partially-visible items.
        let vi = self.vec_index;
        let viewport_size = self.area.rect(cx).size.index(vi);
        let item_top = self.item_top_from_height_tree(target_id);
        if viewport_size > 0.0 {
            if let Some(item_top) = item_top {
                // Targeting the last item means "go to the bottom", and a tall
                // last item can have its top on screen with most of it below.
                let settled = if target_id + 1 >= self.range_end {
                    self.at_end
                } else {
                    item_top >= 0.0 && item_top < viewport_size
                };
                if settled {
                    // Same as arriving under animation: we're at the end, so follow it.
                    if target_id + 1 >= self.range_end {
                        self.detect_tail_in_draw = true;
                    }
                    cx.widget_action(self.widget_uid(), PortalListAction::SmoothScrollReached);
                    return;
                }
            }
        }

        let max_items_to_show = max_items_to_show.unwrap_or(SMOOTH_SCROLL_MAXIMUM_WINDOW);

        // Determine scroll direction from the item's actual pixel position
        // relative to the viewport, not from index comparison alone.
        // When first_scroll is very negative, items with target_id > first_id
        // can still be above the viewport.
        let scroll_direction: f64 = if let Some(item_top) = item_top {
            if item_top < 0.0 {
                1.0
            } else {
                -1.0
            }
        } else {
            // Height tree unavailable; fall back to index comparison.
            if target_id > self.first_id {
                -1.0
            } else {
                1.0
            }
        };

        let starting_id: Option<usize>;
        if target_id > self.first_id {
            starting_id = ((target_id.saturating_sub(self.first_id)) > max_items_to_show)
                .then_some(target_id.saturating_sub(max_items_to_show));
        } else {
            starting_id = ((self.first_id.saturating_sub(target_id)) > max_items_to_show)
                .then_some(target_id + max_items_to_show);
        }

        if let Some(start) = starting_id {
            self.first_id = start;
        }
        // A programmatic scroll stops every other kind of motion first, so a
        // leftover fling, bounce, or OS momentum stream can't fight or resume
        // after it finishes.
        self.stop_all_scroll_motion();
        self.scroll_state = ScrollState::ScrollingTo {
            target_id,
            delta: speed.abs() * scroll_direction,
            next_frame: cx.new_next_frame(),
            top_offset,
        };
    }

    /// Trigger a scrolling animation to the end of the list.
    pub fn smooth_scroll_to_end(
        &mut self,
        cx: &mut Cx,
        speed: f64,
        max_items_to_show: Option<usize>,
    ) {
        if self.items.is_empty() {
            return;
        }
        // `range_end` is exclusive, so the last item is one before it.
        let target_id = self.range_end.saturating_sub(1);
        // `speed` is a per-frame pixel delta. Scale it to the distance so the
        // animation takes the same time from anywhere in the list.
        let speed = match self.item_top_from_height_tree(target_id) {
            Some(item_top) => (item_top.abs() / SMOOTH_SCROLL_TO_END_FRAMES).max(speed),
            None => speed,
        };
        // Pass an unbounded window so `smooth_scroll_to` doesn't teleport the anchor
        // to the last few items first; that jump is most of the travel, and it's why
        // this only looked smooth when you were already near the bottom.
        self.smooth_scroll_to(
            cx,
            target_id,
            speed,
            max_items_to_show.or(Some(usize::MAX)),
            0.0,
        );
    }

    /// Returns whether this PortalList is currently filling the viewport.
    pub fn is_filling_viewport(&self) -> bool {
        !self.not_filling_viewport
    }

    /// Returns the "start" position of the item with the given `entry_id`.
    pub fn position_of_item(&self, cx: &Cx, entry_id: usize) -> Option<f64> {
        const ZEROED: Rect = Rect {
            pos: Vec2d { x: 0.0, y: 0.0 },
            size: Vec2d { x: 0.0, y: 0.0 },
        };

        if let Some(item) = self.items.get(&entry_id) {
            let item_rect = item.widget.area().rect(cx);
            if item_rect == ZEROED {
                return None;
            }
            let self_rect = self.area.rect(cx);
            if self_rect == ZEROED {
                return None;
            }
            let vi = self.vec_index;
            Some(item_rect.pos.index(vi) - self_rect.pos.index(vi))
        } else {
            None
        }
    }

    // ---- Cross-boundary text selection methods ----

    /// Check if we have an active selection
    pub fn has_selection(&self) -> bool {
        self.selection_anchor.is_some() && self.selection_cursor.is_some()
    }

    /// Clear the selection state
    pub fn clear_selection(&mut self, cx: &mut Cx) {
        self.selection_anchor = None;
        self.selection_cursor = None;
        self.is_selecting = false;
        self.select_scroll_state = None;

        // Clear selection on all items
        for item in self.items.values() {
            item.widget.selection_clear();
        }
        cx.hide_clipboard_actions();
        self.area.redraw(cx);
    }

    /// Get the selection range (start_item, start_char) to (end_item, end_char) in sorted order
    /// Find which item and character index is at the given absolute position
    fn hit_test_selection(&self, cx: &Cx, abs: DVec2) -> Option<(usize, usize)> {
        let vi = self.vec_index;
        let mouse_pos = abs.index(vi);
        if self.items.is_empty() {
            return None;
        }

        // Get the PortalList's own rect to check viewport bounds
        let list_rect = self.area.rect(cx);
        let list_top = list_rect.pos.index(vi);
        let list_bottom = list_top + list_rect.size.index(vi);

        // Find visible items (those with non-zero rects) and their bounds
        // Items in the map may include non-visible items with zero rects
        let mut first_visible_id: Option<usize> = None;
        let mut last_visible_id: Option<usize> = None;
        let mut bottom_edge = list_top;

        for (&item_id, item) in self.items.iter() {
            let rect = item.widget.area().rect(cx);
            // Only consider items with valid (non-zero) rects
            if rect.size.index(vi) > 0.0 {
                if first_visible_id.is_none() || item_id < first_visible_id.unwrap() {
                    first_visible_id = Some(item_id);
                }
                if last_visible_id.is_none() || item_id > last_visible_id.unwrap() {
                    last_visible_id = Some(item_id);
                }
                let item_bottom = rect.pos.index(vi) + rect.size.index(vi);
                if item_bottom > bottom_edge {
                    bottom_edge = item_bottom;
                }
            }
        }

        let first_id = first_visible_id.unwrap_or(self.first_id);
        let last_id = last_visible_id.unwrap_or(self.first_id);

        // Check if mouse is above or below the viewport
        if mouse_pos < list_top {
            return Some((first_id, 0));
        } else if mouse_pos > list_bottom {
            let text_len = self
                .items
                .get(&last_id)
                .map(|item| item.widget.selection_text_len())
                .unwrap_or(0);
            return Some((last_id, text_len));
        }

        // Mouse is within the viewport - find which item contains this position
        for (item_id, item) in self.items.iter() {
            let item_rect = item.widget.area().rect(cx);
            if item_rect.contains(abs) {
                // Found the item, now get char index
                let char_idx = item.widget.selection_point_to_char_index(cx, abs);
                if let Some(char_idx) = char_idx {
                    return Some((*item_id, char_idx));
                }
            }
        }
        // Mouse is inside viewport but not in any item (gap between items or empty space)
        // Find the closest item boundary and snap to it

        // Snap to end of last item if below all items
        if mouse_pos > bottom_edge {
            let text_len = self
                .items
                .get(&last_id)
                .map(|item| item.widget.selection_text_len())
                .unwrap_or(0);
            return Some((last_id, text_len));
        }

        // Mouse is in a gap between items - find the item directly above
        // For selection purposes, gaps belong to the item above (snap to end of that item)
        let mut item_above: Option<(usize, f64)> = None; // (item_id, bottom_edge)

        for (item_id, item) in self.items.iter() {
            let item_rect = item.widget.area().rect(cx);
            let item_top = item_rect.pos.index(vi);
            let item_bottom = item_top + item_rect.size.index(vi);

            // Skip items with zero-size rects (not currently visible)
            if item_rect.size.index(vi) <= 0.0 {
                continue;
            }

            // Item is above the mouse position
            if item_bottom <= mouse_pos {
                if item_above.is_none() || item_bottom > item_above.unwrap().1 {
                    item_above = Some((*item_id, item_bottom));
                }
            }
        }

        // Snap to end of item above
        if let Some((above_id, _)) = item_above {
            let text_len = self
                .items
                .get(&above_id)
                .map(|item| item.widget.selection_text_len())
                .unwrap_or(0);
            return Some((above_id, text_len));
        }

        // No item above - snap to start of first item
        Some((first_id, 0))
    }

    fn get_selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let anchor = self.selection_anchor?;
        let cursor = self.selection_cursor?;

        // Sort by item_id first, then by char_index
        if anchor.0 < cursor.0 || (anchor.0 == cursor.0 && anchor.1 <= cursor.1) {
            Some((anchor, cursor))
        } else {
            Some((cursor, anchor))
        }
    }

    /// Collect selected text from all items in the selection range
    pub fn get_selected_text(&self) -> String {
        let Some((start, end)) = self.get_selection_range() else {
            return String::new();
        };

        let mut result = String::new();

        // Iterate through items in order
        for item_id in start.0..=end.0 {
            if let Some(item) = self.items.get(&item_id) {
                let text = if item_id == start.0 && item_id == end.0 {
                    // Single item selection
                    item.widget.selection_get_text_for_range(start.1, end.1)
                } else if item_id == start.0 {
                    // First item - from start char to end
                    item.widget
                        .selection_get_text_for_range(start.1, item.widget.selection_text_len())
                } else if item_id == end.0 {
                    // Last item - from beginning to end char
                    item.widget.selection_get_text_for_range(0, end.1)
                } else {
                    // Middle item - full text
                    item.widget.selection_get_full_text()
                };

                if !result.is_empty() && !text.is_empty() {
                    result.push('\n');
                }
                result.push_str(&text);
            }
        }

        result
    }

    fn selection_clipboard_rect(&self, cx: &Cx) -> Rect {
        let Some((start, end)) = self.get_selection_range() else {
            return self.area.rect(cx);
        };

        let mut out: Option<Rect> = None;
        for item_id in start.0..=end.0 {
            if let Some(item) = self.items.get(&item_id) {
                let rect = item.widget.area().rect(cx);
                if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
                    continue;
                }
                out = Some(if let Some(acc) = out {
                    let x0 = acc.pos.x.min(rect.pos.x);
                    let y0 = acc.pos.y.min(rect.pos.y);
                    let x1 = (acc.pos.x + acc.size.x).max(rect.pos.x + rect.size.x);
                    let y1 = (acc.pos.y + acc.size.y).max(rect.pos.y + rect.size.y);
                    Rect {
                        pos: dvec2(x0, y0),
                        size: dvec2((x1 - x0).max(1.0), (y1 - y0).max(1.0)),
                    }
                } else {
                    rect
                });
            }
        }

        out.unwrap_or_else(|| self.area.rect(cx))
    }

    fn select_all_visible(&mut self, cx: &mut Cx) {
        let Some((&first_id, _)) = self.items.iter().min_by_key(|(id, _)| *id) else {
            return;
        };
        let Some((&last_id, last_item)) = self.items.iter().max_by_key(|(id, _)| *id) else {
            return;
        };

        self.selection_anchor = Some((first_id, 0));
        self.selection_cursor = Some((last_id, last_item.widget.selection_text_len()));
        self.update_item_selections(cx);
        self.area.redraw(cx);
    }

    /// Update selection visuals on TextFlow items based on current selection state
    fn update_item_selections(&mut self, cx: &mut Cx) {
        let Some((start, end)) = self.get_selection_range() else {
            return;
        };
        for (item_id, item) in self.items.iter() {
            let item_id = *item_id;
            if item_id < start.0 || item_id > end.0 {
                // Not in selection range
                item.widget.selection_clear();
            } else if item_id == start.0 && item_id == end.0 {
                // Single item selection
                item.widget.selection_set(start.1, end.1);
            } else if item_id == start.0 {
                // First item - from start char to end of text
                let len = item.widget.selection_text_len();
                item.widget.selection_set(start.1, len);
            } else if item_id == end.0 {
                // Last item - from beginning to end char
                item.widget.selection_set(0, end.1);
            } else {
                // Middle item - select all
                item.widget.selection_select_all();
            }
            // Required for cached item templates (e.g. View with new_batch),
            // where selection mutations do not automatically invalidate draw caches.
            item.widget.redraw(cx);
        }

        self.area.redraw(cx);
    }

    /// Check if a point hits any interactive widget (link, button, etc.) in any of the visible items.
    fn point_hits_interactive_item(&self, cx: &Cx, abs: DVec2) -> bool {
        for item in self.items.values() {
            if item
                .widget
                .find_interactive_widget_from_point(cx, abs)
                .is_some()
            {
                return true;
            }
        }
        false
    }
}

impl WidgetNode for PortalList {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (item_id, item) in self.items.iter() {
            visit(LiveId(*item_id as u64), item.widget.clone());
        }
    }

    fn skip_widget_tree_search(&self) -> bool {
        self.skip_widget_tree_search
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        for item in self.items.values() {
            item.widget.find_widgets_from_point(cx, point, found);
        }
    }
}

impl Widget for PortalList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();

        // Selection autoscroll is driven by next-frame ticks. If pointer-up happens outside
        // hit testing, selection can get "stuck" and keep scheduling frames. Clear it on
        // global release/wheel-without-button as a safety net.
        if self.is_selecting {
            let clear_selection_autoscroll = match event {
                Event::MouseUp(_) => true,
                Event::TouchUpdate(e) => e
                    .touches
                    .iter()
                    .any(|touch| matches!(touch.state, TouchState::Stop | TouchState::Stable)),
                Event::Scroll(_) => cx.fingers.first_mouse_button.is_none(),
                _ => false,
            };
            if clear_selection_autoscroll {
                self.is_selecting = false;
                self.select_scroll_state = None;
            }
        }

        let mut scroll_to = None;
        let bar_pos_before = self.scroll_bar.get_scroll_pos();
        self.scroll_bar
            .handle_event_with(cx, event, &mut |_cx, action| {
                if let ScrollBarAction::Scroll {
                    scroll_pos,
                    view_total,
                    view_visible,
                } = action
                {
                    scroll_to = Some((scroll_pos, scroll_pos + 0.5 >= view_total - view_visible));
                }
            });

        if let Some((scroll_to, at_end)) = scroll_to {
            // A momentum fling / programmatic scroll owns `first_scroll` directly and updates the
            // scroll bar only to reflect itself (via `set_scroll_pos_no_action`). The bar can still
            // emit a Scroll action whose height-tree position is quantized to the nearest item/offset;
            // honoring it here snaps `first_scroll` back by up to ~1px every frame, fighting the
            // animation. At the slow end of a flick — where the real per-frame motion is sub-pixel —
            // that snap-back dominates and the content shakes in place instead of gliding to a stop.
            // So ignore bar-driven scroll resets while an animation owns the position; honor them only
            // for genuine user scroll-bar drags (when no such animation is active).
            let animation_owns_scroll = matches!(
                self.scroll_state,
                ScrollState::Flick { parked: false, .. }
                    | ScrollState::Pulldown { .. }
                    | ScrollState::ScrollingTo { .. }
                    | ScrollState::Tailing { .. }
            );
            if !animation_owns_scroll {
                // Set tail_range based on whether we're at the end
                self.tail_range = at_end && self.auto_tail;

                // Use height_tree to map scroll position to item + offset
                if let Some(ref tree) = self.height_tree {
                    let (item_idx, offset) = tree.find_position(scroll_to);
                    self.first_id = self.range_start + item_idx;
                    // first_scroll is negative when scrolled into the item
                    self.first_scroll = -offset;
                } else {
                    // Fallback to old integer-based calculation
                    self.first_id = ((scroll_to / self.scroll_bar.get_scroll_view_visible())
                        * self.view_window as f64) as usize;
                    self.first_scroll = 0.0;
                }

                // Bar-driven movement doesn't pass through `delta_top_scroll`,
                // so account for it here.
                self.user_scroll_travel -= scroll_to - bar_pos_before;

                // A scroll-bar drag stops every other kind of motion, so a leftover
                // fling, bounce, or OS momentum stream can't resume from wherever
                // the drag lands.
                self.stop_all_scroll_motion();
                self.area.redraw(cx);
            }
        }

        // When selectable, we handle mouse/touch events at PortalList level for cross-item selection.
        // However, we need to pass through events to interactive items (links, buttons, etc.)
        // so they can be clicked. We check if the event point hits any interactive item.
        //
        // Hover events (FingerHoverIn/Out/Over) are ALWAYS passed through so interactive items
        // can properly show/hide their hover states.
        let mut pass_through_to_children = true;

        // Suppress all interaction events to children when:
        // - A committed drag scroll is active (finger/mouse moved past threshold), or
        // - The current gesture started as a tap-to-stop-scroll (user tapped while
        //   a flick animation was in progress).
        // Hover events are not suppressed so widgets can still update their hover state.
        //
        // For down events (MouseDown/TouchStart), we also check whether the scroll
        // is currently animating. This is needed because `suppress_child_events` is
        // set in hit processing which runs *after* child event forwarding.
        // Catch-suppression lives for exactly one gesture: it is armed by a press that
        // stops a moving list and must always end with that press, so reset it on the
        // raw mouse-up rather than relying on a FingerUp hit (which depends on capture
        // state and can be missed, leaving the flag stuck and eating the next click).
        if let Event::MouseUp(e) = event {
            if e.button.is_primary() {
                self.suppress_child_events = false;
            }
        }

        // A live trackpad coast moves the list while `scroll_state` is `Stopped`, so it
        // counts as animating too. A press during any motion — including the pulldown
        // bounce, via `motion_live` — only stops the motion, never reaching a child;
        // the bounce spring itself keeps settling through it.
        let coasting_now = match event {
            Event::MouseDown(e) => self.motion_live(e.time) || self.press_is_catch(e.time),
            Event::MouseMove(e) => self.momentum.is_live(e.time),
            Event::TouchUpdate(e) => self.momentum.is_live(e.time),
            _ => false,
        };
        let is_scroll_animating = matches!(
            self.scroll_state,
            ScrollState::Flick { parked: false, .. }
                | ScrollState::ScrollingTo { .. }
                | ScrollState::Tailing { .. }
        ) || coasting_now;
        if self.suppress_child_events || is_scroll_animating {
            match event {
                // Suppress in-progress interactions so children don't react to
                // a gesture that the list is handling as part of a "scroll" action.
                Event::MouseDown(_) | Event::MouseMove(_) => {
                    pass_through_to_children = false;
                }
                // Don't suppress touch events if a touch-stop occurred (finger was released).
                // Without this, a child widget in this list that captured `FingerDown`
                // (e.g. a button that has been pressed/hovered) will never see the FingerUp,
                // meaning it'll get stuck in that old pressed/hovered state.
                Event::TouchUpdate(e) => {
                    let has_release = e
                        .touches
                        .iter()
                        .any(|t| matches!(t.state, TouchState::Stop));
                    if !has_release {
                        pass_through_to_children = false;
                    }
                }
                // Note: MouseUp should pass through just like "touch stop" (finger releases) above.
                _ => {}
            }
        }

        // The selectable logic can only further restrict pass-through, never override
        // the drag-scroll suppression above.
        if self.selectable && pass_through_to_children {
            match event {
                // Always pass hover events through for proper hover state management
                Event::MouseMove(_) => {
                    // MouseMove generates hover events - always pass through
                    pass_through_to_children = true;
                }
                // For click/drag events, only pass through if over an interactive item
                Event::MouseDown(e) => {
                    pass_through_to_children =
                        !self.is_selecting && self.point_hits_interactive_item(cx, e.abs);
                }
                Event::MouseUp(e) => {
                    pass_through_to_children =
                        !self.is_selecting && self.point_hits_interactive_item(cx, e.abs);
                }
                Event::TouchUpdate(e) => {
                    if self.is_selecting {
                        pass_through_to_children = false;
                    } else if let Some(t) = e.touches.first() {
                        pass_through_to_children = self.point_hits_interactive_item(cx, t.abs);
                    }
                }
                _ => {}
            }
        }

        // A pointer move only matters to items when the pointer is over the
        // list, or just left it (so the last hovered item still sees its
        // hover-out). One rect test here guards the entire item subtree
        // from high-rate mouse-move fan-out — but ONLY while no button is
        // held: with a button down a captured child may be mid-drag, and
        // its FingerMoves must keep flowing wherever the pointer (or a
        // scrub pin's virtual abs) goes. Gating those dropped every move
        // once a drag left the list rect — the "scrubbing goes lossy when
        // the pointer leaves the panel" bug.
        if let Event::MouseMove(e) = event {
            let inside = self.area.is_valid(cx) && self.area.clipped_rect(cx).contains(e.abs);
            if !inside
                && !self.pointer_was_inside
                && cx.fingers.first_mouse_button.is_none()
            {
                pass_through_to_children = false;
            }
            self.pointer_was_inside = inside;
        }

        if pass_through_to_children {
            // Iterate in visual order (by item_id) for deterministic event handling
            // Use keys().min/max to get actual item range without allocation
            if let (Some(&min_id), Some(&max_id)) =
                (self.items.keys().min(), self.items.keys().max())
            {
                for item_id in min_id..=max_id {
                    if let Some(item) = self.items.get_mut(&item_id) {
                        let item_uid = item.widget.widget_uid();
                        cx.group_widget_actions(uid, item_uid, |cx| {
                            item.widget.handle_event(cx, event, scope);
                        });
                    }
                }
            }
        }

        // Handle auto-scroll during selection
        if let Some(mut scroll_state) = self.select_scroll_state.take() {
            if scroll_state.next_frame.is_event(event).is_some() {
                let rect = self.area.rect(cx);
                let vi = self.vec_index;
                let scroll_margin = 20.0;

                let top_edge = rect.pos.index(vi);
                let bottom_edge = rect.pos.index(vi) + rect.size.index(vi);
                let mouse_pos = scroll_state.last_abs.index(vi);

                if mouse_pos < top_edge + scroll_margin {
                    // Mouse above viewport - scroll up (only if not already at top).
                    // Selection auto-scroll clips at the edges instead of stretching
                    // the rubber band: nothing would spring a selection's stretch back.
                    if self.first_id > self.range_start || self.first_scroll < 0.0 {
                        let distance = (top_edge + scroll_margin - mouse_pos).max(1.0);
                        let scroll_speed = (5.0 + distance * 0.5).clamp(5.0, 50.0);
                        self.delta_top_scroll(cx, scroll_speed, true, false, 0.0, false, true);
                        self.area.redraw(cx);
                    }
                } else if mouse_pos > bottom_edge - scroll_margin {
                    // Mouse below viewport - scroll down (only if not already at end)
                    if !self.at_end {
                        let distance = (mouse_pos - (bottom_edge - scroll_margin)).max(1.0);
                        let scroll_speed = -(5.0 + distance * 0.5).clamp(5.0, 50.0);
                        self.delta_top_scroll(cx, scroll_speed, true, false, 0.0, false, true);
                        self.area.redraw(cx);
                    }
                }
                // Always request next frame while selecting
                scroll_state.next_frame = cx.new_next_frame();
            }
            // Keep scroll state alive while is_selecting is true
            self.select_scroll_state = Some(scroll_state);
        }

        match &mut self.scroll_state {
            ScrollState::ScrollingTo {
                target_id,
                delta,
                next_frame,
                top_offset,
            } => {
                if next_frame.is_event(event).is_some() {
                    // Copy values out of the borrow so we can call &self methods.
                    let target_id = *target_id;
                    let delta_val = *delta;
                    let top_offset = *top_offset;
                    let scrolling_down = delta_val < 0.0;

                    // Check if the target item has reached (or passed) the
                    // desired position using the height tree for accuracy.
                    let vi = self.vec_index;
                    let viewport_size = self.area.rect(cx).size.index(vi);
                    let item_top = self.item_top_from_height_tree(target_id);
                    let mut target_reached = false;

                    if let Some(item_top) = item_top {
                        // Clamp: the item must at least reach the viewport (top >= 0).
                        let effective_target = top_offset.max(0.0);
                        if scrolling_down {
                            target_reached = item_top <= effective_target;
                        } else {
                            target_reached = item_top >= effective_target;
                        }

                        // For boundary conditions (e.g., near list start/end), the
                        // effective_target may not be reachable. Consider it reached
                        // if the item's top is visible and we're at a list boundary.
                        if !target_reached
                            && item_top >= 0.0
                            && item_top < viewport_size
                            && (self.first_id == self.range_start || self.at_end)
                        {
                            target_reached = true;
                        }
                    }

                    // Fallback: if we're scrolling down and the list has hit
                    // the end, the target is as visible as it can get.
                    if scrolling_down && self.at_end && target_id > self.first_id {
                        target_reached = true;
                    }

                    if !target_reached {
                        // Overshoot protection: if first_id has scrolled past
                        // target_id, snap it back so the item gets drawn.
                        let distance_to_target = target_id as isize - self.first_id as isize;
                        let overshot = distance_to_target.signum() == delta_val.signum() as isize;
                        if overshot {
                            self.first_id = target_id;
                        }

                        if let ScrollState::ScrollingTo { next_frame, .. } = &mut self.scroll_state
                        {
                            *next_frame = cx.new_next_frame();
                        }
                        self.delta_top_scroll(cx, delta_val, true, false, 0.0, false, false);
                        self.area.redraw(cx);
                    } else {
                        self.was_scrolling = false;
                        self.scroll_state = ScrollState::Stopped;
                        // Landing on the last item means we're following the end again,
                        // so let the next draw pick tailing back up.
                        if target_id + 1 >= self.range_end {
                            self.detect_tail_in_draw = true;
                        }
                        cx.widget_action(uid, PortalListAction::SmoothScrollReached);
                    }
                }
            }
            ScrollState::Flick { fling, next_frame, .. } => {
                if let Some(ne) = next_frame.is_event(event) {
                    // The scroll animation lives in `scroll_motion::Fling`, shared with
                    // ScrollBar. Both the touch-drag flick and the trackpad deceleration tail
                    // self-decay; they differ only in decay rate and edge behavior.
                    match fling.step(ne.time) {
                        None => {
                            // First fling frame: time baseline established, no movement yet.
                            *next_frame = cx.new_next_frame();
                        }
                        Some(displacement) => {
                            let min_velocity =
                                self.flick_scroll_minimum * PER_FRAME_TO_PER_SECOND;
                            if fling.is_active(min_velocity) {
                                *next_frame = cx.new_next_frame();
                                // A touch flick may overscroll into the pulldown bounce; a trackpad
                                // tail clips at the edges like a wheel.
                                let ov = fling.allows_overscroll();
                                let fling_velocity = fling.velocity;
                                let before = (self.first_id, self.first_scroll);
                                self.delta_top_scroll(cx, displacement, !ov, ov, fling_velocity, true, false);
                                let moved = (self.first_id, self.first_scroll) != before;
                                if let ScrollState::Flick { parked, .. } =
                                    &mut self.scroll_state
                                {
                                    if displacement != 0.0 && !moved {
                                        // Pinned at a non-bouncing edge: park the fling.
                                        // It keeps decaying silently and presses are
                                        // ordinary clicks, but if content grows past the
                                        // edge (e.g. pagination prepending items), the
                                        // next step moves the list again and the flick
                                        // continues naturally.
                                        if !*parked {
                                            *parked = true;
                                            self.was_scrolling = false;
                                        }
                                    } else {
                                        if *parked && moved {
                                            *parked = false;
                                        }
                                        self.area.redraw(cx);
                                    }
                                } else {
                                    // The delta handed the motion to the pulldown bounce.
                                    self.area.redraw(cx);
                                }
                            } else {
                                self.was_scrolling = false;
                                self.momentum = MomentumStream::Idle;
                                self.scroll_state = ScrollState::Stopped;
                            }
                        }
                    }
                }
            }
            ScrollState::Pulldown { next_frame, x0, v0, clock, at_start, touch } => {
                if let Some(ne) = next_frame.is_event(event) {
                    // The rubber-band bounce, seeded with the velocity that remained
                    // when the edge was reached: iOS's spring for a finger on the
                    // screen, Chrome's macOS curve for trackpad (see `scroll_motion`).
                    // The bounce may never travel farther than the drag stretch can
                    // reach, however fast the fling that hit the edge was: the seed
                    // velocity is softened against the headroom (huge flicks compress,
                    // gentle ones keep their feel), and the clamp below is a backstop.
                    let max_overscroll =
                        self.area.rect(cx).size.index(self.vec_index) * RUBBER_BAND_TOUCH_RANGE;
                    if clock.not_started() {
                        *v0 = soften_bounce_velocity(*v0, *x0, max_overscroll, *touch);
                    }
                    let t = clock.advance(ne.time);
                    let (x, past_peak) = rubber_band_bounce(*x0, *v0, t, *touch);
                    let x = x.min(max_overscroll);
                    // The overshoot ramps up from zero first, so only settle once the
                    // spring is past its peak and back near the edge.
                    let settled = x <= 0.5 && (past_peak || (*v0 <= 0.0 && *x0 <= 0.5));
                    let at_start_edge = *at_start;
                    if settled {
                        if at_start_edge {
                            self.first_scroll = 0.0;
                        }
                        self.bounce_overshoot = 0.0;
                        self.stretching = false;
                        self.was_scrolling = false;
                        self.scroll_state = ScrollState::Stopped;
                    } else {
                        if at_start_edge {
                            self.first_scroll = x.max(0.0);
                        } else {
                            self.bounce_overshoot = x.max(0.0);
                        }
                        *next_frame = cx.new_next_frame();
                    }
                    self.area.redraw(cx);
                }
            }
            ScrollState::Tailing {
                next_frame,
                velocity,
            } => {
                if next_frame.is_event(event).is_some() {
                    if self.tail_adjustment_needed > 0.5 || velocity.abs() > 0.5 {
                        // Spring-damper animation for smooth, natural-feeling scroll
                        // This creates momentum that absorbs rapid content additions gracefully

                        // Spring constant - how strongly we're pulled toward target
                        // Higher = faster response, lower = more gradual
                        let spring_k = self.smooth_tail_speed * 0.15;

                        // Damping ratio - prevents oscillation
                        // 1.0 = critically damped (no overshoot), <1 = underdamped (bouncy)
                        let damping = 0.85;

                        // Calculate spring force toward target
                        let spring_force = self.tail_adjustment_needed * spring_k;

                        // Apply spring force to velocity, then apply damping
                        *velocity = (*velocity + spring_force) * damping;

                        // Clamp velocity to reasonable bounds
                        let max_velocity = 60.0; // pixels per frame
                        *velocity = velocity.clamp(0.0, max_velocity);

                        // Apply velocity to scroll position
                        let step = *velocity;
                        if step > 0.1 {
                            self.first_scroll -= step;
                            self.tail_adjustment_needed =
                                (self.tail_adjustment_needed - step).max(0.0);
                        }

                        // Continue animation if still moving or not at target
                        if self.tail_adjustment_needed > 0.5 || velocity.abs() > 0.5 {
                            *next_frame = cx.new_next_frame();
                        } else {
                            self.tail_adjustment_needed = 0.0;
                            *velocity = 0.0;
                            self.scroll_state = ScrollState::Stopped;
                        }
                        self.area.redraw(cx);
                    } else {
                        self.tail_adjustment_needed = 0.0;
                        self.scroll_state = ScrollState::Stopped;
                    }
                }
            }
            _ => (),
        }

        let vi = self.vec_index;
        let is_scroll = matches!(event, Event::Scroll(_));
        if self.scroll_bar.is_area_captured(cx) {
            self.scroll_state = ScrollState::Stopped;
        }

        if !self.scroll_bar.is_area_captured(cx) || is_scroll {
            let hit = event.hits_with_capture_overload(cx, self.area, self.capture_overload);
            match hit {
                // A scroll view in one of the rows already moved by this delta.
                Hit::FingerScroll(_) if event.scroll_handled(vi) => {}
                Hit::FingerScroll(e) => {
                    // Whatever this list moves by is its own: the event is marked
                    // handled, so the scroll views around the list, which see it
                    // after, don't move by it as well. A delta pointing past an edge
                    // the list rests on is left for them (`keeps_scroll_delta`).
                    //
                    // Trackpad scrolling on macOS is a gesture with phases: user-driven deltas
                    // while the fingers move (`Began`/`Changed`), then the OS's own decaying
                    // `Momentum` deltas after they lift. Both are applied exactly as delivered,
                    // so the feel and the deceleration are the native ones, and the OS ending
                    // its stream on a touch gives the native instant stop. Phase-less events
                    // (`None`: wheels, X11, Windows) apply their delta directly.
                    let delta = -e.scroll.index(vi);
                    match e.phase {
                        ScrollPhase::Momentum => {
                            // Only a stream this list is expecting, applying, or riding
                            // pinned at an edge is processed; a stray stream (e.g. one whose
                            // coast a press already caught) can't restart motion.
                            if self.momentum.accepts_deltas(e.time)
                                && matches!(self.scroll_state, ScrollState::Stopped)
                                && delta != 0.0
                            {
                                self.tail_range = false;
                                self.detect_tail_in_draw = true;
                                self.was_scrolling = false;
                                let before = (self.first_id, self.first_scroll);
                                // Overscroll at the top enters the pulldown bounce, like a
                                // touch flick does, seeded with the stream's velocity.
                                let dt = self
                                    .momentum
                                    .prev_delta_time()
                                    .map_or(1.0 / 240.0, |prev| (e.time - prev).max(1.0 / 240.0));
                                let max_v = self.flick_scroll_maximum * PER_FRAME_TO_PER_SECOND;
                                let velocity = (delta / dt).clamp(-max_v, max_v);
                                self.delta_top_scroll(cx, delta, false, true, velocity, false, false);
                                if matches!(self.scroll_state, ScrollState::Pulldown { .. }) {
                                    // The bounce owns the motion now; drop the rest of the
                                    // stream so it can't fight the spring.
                                    self.momentum = MomentumStream::Idle;
                                } else if (self.first_id, self.first_scroll) == before
                                    || (self.at_end && delta < 0.0)
                                {
                                    // Pinned at a non-bouncing edge: the delta had no effect
                                    // and presses here are ordinary clicks, but the stream
                                    // stays armed. If content grows past the edge (e.g.
                                    // pagination prepending items), the next delta moves the
                                    // list again and the flick continues naturally.
                                    self.momentum =
                                        MomentumStream::Pinned { last_delta_time: e.time };
                                } else {
                                    self.momentum = MomentumStream::Live {
                                        last_delta_time: e.time,
                                        velocity,
                                        direction: delta.signum(),
                                    };
                                }
                                // A pinned stream's deltas change nothing; skip the redraw
                                // so a stream outliving the edge doesn't spin no-op frames.
                                if (self.first_id, self.first_scroll) != before
                                    || !matches!(self.momentum, MomentumStream::Pinned { .. })
                                {
                                    self.area.redraw(cx);
                                }
                                // Pinned, the coast chains on to the scroll view around.
                                if !matches!(self.momentum, MomentumStream::Pinned { .. }) {
                                    event.set_scroll_handled(vi);
                                }
                            }
                        }
                        ScrollPhase::MomentumEnded => {
                            // A stream that ends while still live was cut by a touch (or
                            // faded on its final delta); one that ends pinned or merely
                            // expected just goes idle. The cut is kept because the touch
                            // that caused it can be delivered after this event, and a
                            // duplicate end event must not erase a fresh cut first.
                            self.momentum = if self.momentum.is_live(e.time) {
                                MomentumStream::Cut { at: e.time }
                            } else if matches!(self.momentum, MomentumStream::Cut { .. }) {
                                self.momentum
                            } else {
                                MomentumStream::Idle
                            };
                        }
                        ScrollPhase::Touched => {
                            // A finger contacted the trackpad: instantly stop any kinetic
                            // scrolling, like the native catch. The zero delta is not applied
                            // and a drag in progress is left alone. If the touch stopped real
                            // motion — including a stream whose end event already marked it
                            // cut, in whichever order the two were delivered — its own press
                            // (arriving separately at finger lift) is the second half of the
                            // catch, not a click. During the rubber-band bounce the press is
                            // likewise a catch, and the spring settles on its own.
                            let was_moving =
                                self.motion_live(e.time) || self.momentum.cut_near(e.time);
                            self.momentum = MomentumStream::Idle;
                            if matches!(self.scroll_state, ScrollState::Flick { .. }) {
                                self.scroll_state = ScrollState::Stopped;
                                self.was_scrolling = false;
                                self.area.redraw(cx);
                            }
                            if was_moving {
                                self.touch_caught_motion_at = Some(e.time);
                            }
                        }
                        ScrollPhase::Ended => {
                            // Fingers lifted. Apply the final delta; the OS momentum stream
                            // that may follow is now expected. A stretched rubber band
                            // springs back from here instead (and takes no momentum).
                            // The final delta clips at the edges like a wheel's; a stretch
                            // left showing is the list's, and its spring starts below.
                            let keeps = self.keeps_scroll_delta(delta, false);
                            self.was_scrolling = false;
                            self.scroll_state = ScrollState::Stopped;
                            self.momentum = MomentumStream::Expected { since: e.time };
                            self.stretching = false;
                            if self.bounce_at_start
                                && self.first_id == self.range_start
                                && self.first_scroll > 0.0
                            {
                                self.momentum = MomentumStream::Idle;
                                self.scroll_state = ScrollState::Pulldown {
                                    next_frame: cx.new_next_frame(),
                                    x0: self.first_scroll,
                                    v0: 0.0,
                                    clock: FrameClock::default(),
                                    at_start: true,
                                    touch: false,
                                };
                            } else if self.bounce_at_end && self.bounce_overshoot > 0.0 {
                                self.momentum = MomentumStream::Idle;
                                self.scroll_state = ScrollState::Pulldown {
                                    next_frame: cx.new_next_frame(),
                                    x0: self.bounce_overshoot,
                                    v0: 0.0,
                                    clock: FrameClock::default(),
                                    at_start: false,
                                    touch: false,
                                };
                            }
                            if delta != 0.0 {
                                self.tail_range = false;
                                self.detect_tail_in_draw = true;
                                self.delta_top_scroll(cx, delta, true, false, 0.0, false, true);
                                self.area.redraw(cx);
                            }
                            if keeps || matches!(self.scroll_state, ScrollState::Pulldown { .. }) {
                                event.set_scroll_handled(vi);
                            }
                        }
                        // Finger-driven deltas apply directly, stretching the rubber band
                        // past an edge. A user-driven delta also stops any in-progress
                        // momentum fling, so putting fingers back on the pad catches the
                        // scroll.
                        ScrollPhase::Began | ScrollPhase::Changed => {
                            self.tail_range = false;
                            self.detect_tail_in_draw = true;
                            self.was_scrolling = false;
                            self.momentum = MomentumStream::Idle;
                            if delta != 0.0 {
                                self.last_finger_scroll_time = Some(e.time);
                            }
                            self.scroll_state = ScrollState::Stopped;
                            if self.keeps_scroll_delta(delta, true) {
                                event.set_scroll_handled(vi);
                            }
                            self.delta_top_scroll(cx, delta, false, false, 0.0, false, true);
                            self.area.redraw(cx);
                        }
                        // `None` (wheels) applies the delta directly and clips at the edges.
                        _ => {
                            if self.keeps_scroll_delta(delta, false) {
                                event.set_scroll_handled(vi);
                            }
                            self.tail_range = false;
                            self.detect_tail_in_draw = true;
                            self.was_scrolling = false;
                            self.momentum = MomentumStream::Idle;
                            self.bounce_overshoot = 0.0;
                            self.scroll_state = ScrollState::Stopped;
                            // Clip to the top and don't transition to pulldown; overscroll bounce
                            // is only for touch drag/flick.
                            self.delta_top_scroll(cx, delta, true, false, 0.0, false, true);
                            // Note: we intentionally do NOT reset `at_end` here.
                            // `at_end` is authoritatively recalculated each draw cycle
                            // in `end()`, and the redraw is already triggered below.
                            // Eagerly resetting it here would create a stale `false` value
                            // visible to any code that checks `is_at_end()` before the
                            // next draw completes.
                            self.area.redraw(cx);
                        }
                    }
                }
                Hit::KeyDown(ke) => {
                    // Keyboard navigation instantly stops every kind of scroll
                    // motion and owns the viewport from a resting state.
                    if matches!(
                        ke.key_code,
                        KeyCode::Home
                            | KeyCode::End
                            | KeyCode::PageUp
                            | KeyCode::PageDown
                            | KeyCode::ArrowUp
                            | KeyCode::ArrowDown
                    ) {
                        self.stop_all_scroll_motion();
                    }
                    match ke.key_code {
                    KeyCode::Home => {
                        self.first_id = 0;
                        self.first_scroll = 0.0;
                        self.tail_range = false;
                        self.update_scroll_bar(cx);
                        self.area.redraw(cx);
                    }
                    KeyCode::End => {
                        self.first_id = self.range_end.max(1) - 1;
                        self.first_scroll = 0.0;
                        if self.auto_tail {
                            self.tail_range = true;
                        }
                        self.update_scroll_bar(cx);
                        self.area.redraw(cx);
                    }
                    KeyCode::PageUp => {
                        self.first_id = self.first_id.max(self.view_window) - self.view_window;
                        self.first_scroll = 0.0;
                        self.tail_range = false;
                        self.update_scroll_bar(cx);
                        self.area.redraw(cx);
                    }
                    KeyCode::PageDown => {
                        self.first_id += self.view_window;
                        self.first_scroll = 0.0;
                        if self.first_id >= self.range_end.max(1) {
                            self.first_id = self.range_end.max(1) - 1;
                        }
                        self.detect_tail_in_draw = true;
                        self.update_scroll_bar(cx);
                        self.area.redraw(cx);
                    }
                    KeyCode::ArrowDown => {
                        self.first_id += 1;
                        if self.first_id >= self.range_end.max(1) {
                            self.first_id = self.range_end.max(1) - 1;
                        }
                        self.detect_tail_in_draw = true;
                        self.first_scroll = 0.0;
                        self.update_scroll_bar(cx);
                        self.area.redraw(cx);
                    }
                    KeyCode::ArrowUp => {
                        if self.first_id > 0 {
                            self.first_id -= 1;
                            if self.first_id < self.range_start {
                                self.first_id = self.range_start;
                            }
                            self.first_scroll = 0.0;
                            self.area.redraw(cx);
                            self.tail_range = false;
                            self.update_scroll_bar(cx);
                        }
                    }
                    KeyCode::KeyA => {
                        if self.selectable && ke.modifiers.is_primary() {
                            self.select_all_visible(cx);
                            let selection_rect = self.selection_clipboard_rect(cx);
                            cx.show_clipboard_actions(true, selection_rect, cx.keyboard_shift);
                        }
                    }
                    _ => (),
                    }
                }
                Hit::FingerDown(fe) => {
                    if self.grab_key_focus {
                        cx.set_key_focus(self.area);
                    }
                    // A press that doesn't end up moving us off the end shouldn't stop
                    // tailing, so let the next draw re-arm it like the scroll paths do.
                    self.tail_range = false;
                    self.detect_tail_in_draw = true;
                    self.was_scrolling = match &self.scroll_state {
                        ScrollState::Drag { samples, .. } => samples.len() > 1,
                        // A press while anything moves the list (a coast, active finger
                        // scrolling, the bounce) or whose own touch just stopped motion
                        // is a stop, not a click. The bounce spring itself keeps settling.
                        _ => self.motion_live(fe.time) || self.press_is_catch(fe.time),
                    };
                    // One press consumes the catch; later presses are ordinary clicks.
                    self.touch_caught_motion_at = None;

                    // If the list was animating (flick, pulldown, etc.) when the user
                    // tapped/clicked, suppress forwarding this entire gesture to children.
                    // The tap should only stop the scroll, not activate a child widget.
                    // If the list was NOT scrolling, clear any previous suppression.
                    self.suppress_child_events = self.was_scrolling;

                    // A press stops an active momentum fling or pulldown bounce, the "press to
                    // catch the scroll" behavior that iOS, Android, and macOS all have. It runs
                    // before the selection and drag branches below so it works even when the
                    // list is `selectable`: on a selectable list a tap on text takes the
                    // selection branch, which would otherwise leave the fling running. `Drag`
                    // below still supersedes `Stopped` for drag-scroll, and `suppress_child_events`
                    // (set above from `was_scrolling`) keeps this press from also activating a child.
                    // A press kills the momentum stream outright, so a still-live trackpad
                    // stream (e.g. a mouse click during a trackpad coast) can't restart
                    // motion. A pulldown bounce is left running: stopping it would freeze
                    // the list with an open gap at the top, and it settles on its own.
                    self.momentum = MomentumStream::Idle;
                    if let ScrollState::Flick { fling, parked, .. } = &self.scroll_state {
                        // Remember the caught speed briefly: a quick same-direction
                        // re-flick adds it back, so repeated flicks build up speed
                        // the way Chrome and native scrollers allow. A hold, a tap,
                        // or an opposite-direction flick discards it.
                        self.caught_fling = (!parked).then_some((fling.velocity, fe.time));
                        self.scroll_state = ScrollState::Stopped;
                    }

                    // Handle selection when selectable, but not if clicking on interactive items
                    let on_interactive = self.point_hits_interactive_item(cx, fe.abs);
                    if self.selectable && fe.is_primary_hit() && !on_interactive {
                        let hit = self.hit_test_selection(cx, fe.abs);
                        if let Some((item_id, char_idx)) = hit {
                            cx.set_key_focus(self.area);
                            if fe.device.is_touch() {
                                cx.hide_clipboard_actions();
                            }
                            self.selection_anchor = Some((item_id, char_idx));
                            self.selection_cursor = Some((item_id, char_idx));
                            self.is_selecting = true;
                            self.select_scroll_state = Some(SelectScrollState {
                                next_frame: cx.new_next_frame(),
                                last_abs: fe.abs,
                            });
                            self.update_item_selections(cx);
                        }
                    } else if self.drag_scrolling && fe.is_primary_hit()
                        && cx.is_scrolling_allowed_within(&self.area)
                    {
                        // Always enter drag state to enable drag-to-scroll even over
                        // interactive widgets (buttons, links, etc.). The drag threshold
                        // prevents micro-scrolling during taps/clicks, and child widgets
                        // use `was_tap()` to distinguish taps from drags on FingerUp.
                        let initial = fe.abs.index(vi);
                        self.scroll_state = ScrollState::Drag {
                            samples: vec![ScrollSample {
                                abs: initial,
                                time: fe.time,
                            }],
                            initial_abs: initial,
                            // When on interactive items, require a drag threshold before
                            // committing to scroll (for both touch and mouse).
                            // For non-interactive areas, commit immediately since
                            // there's no ambiguity.
                            committed: !on_interactive,
                        };
                    }
                }
                Hit::FingerMove(e) => {
                    // Handle selection when selecting
                    if self.is_selecting {
                        cx.set_cursor(MouseCursor::Text);

                        // Update last_abs for auto-scroll
                        if let Some(state) = &mut self.select_scroll_state {
                            state.last_abs = e.abs;
                        }

                        // Update cursor position
                        let hit = self.hit_test_selection(cx, e.abs);
                        if let Some((item_id, char_idx)) = hit {
                            self.selection_cursor = Some((item_id, char_idx));
                            self.update_item_selections(cx);
                        }
                    } else {
                        // Only update cursor for mouse — skip the expensive
                        // interactive-widget hit test for touch events entirely.
                        if !e.device.is_touch() && !self.point_hits_interactive_item(cx, e.abs) {
                            cx.set_cursor(MouseCursor::Default);
                        }
                        if let ScrollState::Drag {
                            samples,
                            initial_abs,
                            committed,
                        } = &mut self.scroll_state
                        {
                            let new_abs = e.abs.index(vi);

                            // Check if the drag threshold has been exceeded.
                            if !*committed {
                                if (new_abs - *initial_abs).abs() >= self.drag_scroll_threshold {
                                    *committed = true;
                                    self.suppress_child_events = true;
                                } else {
                                    // Still under threshold — track samples but don't scroll.
                                    push_sample(samples, new_abs, e.time);
                                    // Don't apply scroll delta yet.
                                    return;
                                }
                            }

                            let old_sample = *samples.last().unwrap();
                            push_sample(samples, new_abs, e.time);
                            self.delta_top_scroll(cx, new_abs - old_sample.abs, false, false, 0.0, true, true);
                            self.area.redraw(cx);
                        }
                    }
                }
                Hit::FingerUp(fe) if fe.is_primary_hit() => {
                    // The press's release settles the fate of any fling it caught:
                    // only a qualifying same-direction flick below adds it back.
                    // A tap or a slow lift discards it — a catch stays a stop.
                    let caught_fling = self.caught_fling.take();

                    // End selection if we were selecting
                    if self.is_selecting {
                        self.is_selecting = false;
                        self.select_scroll_state = None;
                    }

                    if self.selectable && fe.device.is_touch() {
                        let has_selection = self.has_selection();
                        if has_selection {
                            let selection_rect = self.selection_clipboard_rect(cx);
                            cx.show_clipboard_actions(true, selection_rect, cx.keyboard_shift);
                        } else {
                            cx.hide_clipboard_actions();
                        }
                    }

                    // Always clear touch drag scrolling active flag on finger up.
                    self.suppress_child_events = false;
                    // The fingers are off the screen, so no stretch is being held
                    // (mirrors the trackpad ScrollPhase::Ended clear); the Pulldown
                    // seeds below own any remaining overscroll from here.
                    self.stretching = false;

                    if let ScrollState::Drag {
                        samples, committed, ..
                    } = &mut self.scroll_state
                    {
                        // If the drag was never committed (finger didn't move past
                        // threshold), just stop — this was a tap, not a scroll.
                        if !*committed {
                            self.was_scrolling = false;
                            self.scroll_state = ScrollState::Stopped;
                        } else {
                            // Estimate the release velocity (pixels/second) like a native
                            // VelocityTracker (see `scroll_motion`): oldest→newest of the last
                            // ~4 finger positions. `abs` is the finger position along the scroll
                            // axis in the same pixel units as `first_scroll`, so this is
                            // directly a scroll velocity.
                            let (release_velocity, total_delta) =
                                estimate_release_velocity(samples);
                            // Cap to a sane maximum flick speed (px/s). `flick_scroll_maximum`
                            // is a per-frame value; ×60 converts it to per-second.
                            let max_velocity = self.flick_scroll_maximum * PER_FRAME_TO_PER_SECOND;
                            let release_velocity =
                                release_velocity.clamp(-max_velocity, max_velocity);
                            // Minimum release speed (px/s) below which a lift is treated as a stop,
                            // not a fling. `flick_scroll_minimum` is per-frame; ×60 → per-second.
                            let min_velocity = self.flick_scroll_minimum * PER_FRAME_TO_PER_SECOND;
                            if self.bounce_at_start
                                && self.first_id == self.range_start
                                && self.first_scroll > 0.0
                            {
                                self.scroll_state = ScrollState::Pulldown {
                                    next_frame: cx.new_next_frame(),
                                    x0: self.first_scroll,
                                    v0: release_velocity,
                                    clock: FrameClock::default(),
                                    at_start: true,
                                    touch: true,
                                };
                            } else if self.bounce_at_end && self.bounce_overshoot > 0.0 {
                                // The lift velocity carries into the bounce here too:
                                // positive into the overscroll, so a flick released
                                // mid-stretch springs farther before returning.
                                self.scroll_state = ScrollState::Pulldown {
                                    next_frame: cx.new_next_frame(),
                                    x0: self.bounce_overshoot,
                                    v0: -release_velocity,
                                    clock: FrameClock::default(),
                                    at_start: false,
                                    touch: true,
                                };
                            } else if total_delta.abs() > FLING_MIN_TOTAL_DELTA
                                && release_velocity.abs() > min_velocity
                            {
                                // Fling boost: a quick same-direction re-flick adds the
                                // speed of the fling this press caught, so repeated
                                // flicks build up speed like native scrollers allow.
                                let release_velocity = match caught_fling {
                                    Some((caught_velocity, caught_at))
                                        if caught_velocity * release_velocity > 0.0
                                            && fe.time - caught_at
                                                < FLING_BOOST_MAX_DWELL =>
                                    {
                                        (release_velocity + caught_velocity)
                                            .clamp(-max_velocity, max_velocity)
                                    }
                                    _ => release_velocity,
                                };
                                self.scroll_state = ScrollState::Flick {
                                    fling: Fling::new(release_velocity, self.fling_decel),
                                    next_frame: cx.new_next_frame(),
                                    parked: false,
                                };
                            } else {
                                self.was_scrolling = false;
                                self.scroll_state = ScrollState::Stopped;
                            }
                        }
                    }
                }
                Hit::FingerHoverIn(fhe) | Hit::FingerHoverOver(fhe) if self.selectable => {
                    // Only set Text cursor if not over an interactive item
                    // (interactive items like links will set their own cursor, e.g., Hand)
                    if !self.point_hits_interactive_item(cx, fhe.abs) {
                        cx.set_cursor(MouseCursor::Text);
                    }
                }
                Hit::KeyFocus(_) => {}
                Hit::KeyFocusLost(_) => {
                    // Clear selection when losing focus (if selectable)
                    if self.selectable && self.has_selection() {
                        self.clear_selection(cx);
                    }
                }
                Hit::TextCopy(tc) => {
                    // Handle copy when selectable
                    if self.selectable && self.has_selection() {
                        let text = self.get_selected_text();
                        if !text.is_empty() {
                            *tc.response.borrow_mut() = Some(text);
                        }
                    }
                }
                Hit::TextCut(tc) => {
                    // Non-editable text: treat cut as copy.
                    if self.selectable && self.has_selection() {
                        let text = self.get_selected_text();
                        if !text.is_empty() {
                            *tc.response.borrow_mut() = Some(text);
                        }
                    }
                }
                _ => (),
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ListDrawState::Begin) {
            self.begin(cx, walk);
            return DrawStep::make_step();
        }
        if self.draw_state.get().is_some() {
            self.end(cx);
            self.draw_state.end();
        }
        DrawStep::done()
    }
}

impl PortalListRef {
    /// Sets the first item to be shown and its scroll offset.
    pub fn set_first_id_and_scroll(&self, id: usize, s: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_first_id_and_scroll(id, s);
        }
    }

    /// Sets the first item to be shown by this PortalList to the item with the given `id`.
    pub fn set_first_id(&self, id: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.first_id = id;
            // The list was repositioned by code; announce the edges again.
            inner.forget_reached_edges();
        }
    }

    /// Returns the ID of the item currently shown as the first item in this PortalList.
    pub fn first_id(&self) -> usize {
        if let Some(inner) = self.borrow() {
            inner.first_id
        } else {
            0
        }
    }

    /// Enables whether the PortalList auto-tracks the last item in the list.
    pub fn set_tail_range(&self, tail_range: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.tail_range = tail_range;
        }
    }

    /// See [`PortalList::keep_index_visible()`].
    pub fn keep_index_visible(&self, cx: &mut Cx, index: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.keep_index_visible(cx, index);
        }
    }

    /// See [`PortalList::set_flow`].
    pub fn set_flow(&self, cx: &mut Cx, flow: Flow) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_flow(cx, flow);
        }
    }

    /// See [`PortalList::is_at_end()`].
    pub fn is_at_end(&self) -> bool {
        let Some(inner) = self.borrow() else {
            return false;
        };
        inner.is_at_end()
    }

    /// See [`PortalList::visible_items()`].
    pub fn visible_items(&self) -> usize {
        let Some(inner) = self.borrow() else { return 0 };
        inner.visible_items()
    }

    /// Returns whether this PortalList was scrolling when the most recent finger hit occurred.
    pub fn was_scrolling(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.was_scrolling)
    }

    /// Returns whether the given `actions` contain an action indicating that this PortalList
    /// was scrolled. Emitted only when the list opts in via `emit_scroll_actions`.
    pub fn scrolled(&self, actions: &Actions) -> bool {
        actions
            .filter_widget_actions(self.widget_uid())
            .any(|item| matches!(item.cast(), PortalListAction::Scroll))
    }

    /// Returns whether the given `actions` say the start of this PortalList's range just
    /// came into view (see [`PortalListAction::ReachedStart`]).
    pub fn reached_start(&self, actions: &Actions) -> bool {
        actions
            .filter_widget_actions(self.widget_uid())
            .any(|item| matches!(item.cast(), PortalListAction::ReachedStart))
    }

    /// Returns whether the given `actions` say the end of this PortalList's range just
    /// came into view (see [`PortalListAction::ReachedEnd`]).
    pub fn reached_end(&self, actions: &Actions) -> bool {
        actions
            .filter_widget_actions(self.widget_uid())
            .any(|item| matches!(item.cast(), PortalListAction::ReachedEnd))
    }

    /// Returns the current scroll offset of this PortalList.
    pub fn scroll_position(&self) -> f64 {
        let Some(inner) = self.borrow() else {
            return 0.0;
        };
        inner.first_scroll
    }

    /// Returns the running total of all user-driven scrolling of this list, in pixels.
    ///
    /// This sums every scroll delta the *user* requested: wheel and trackpad
    /// deltas (including OS momentum), touch drags, scroll-bar drags, and
    /// selection auto-scroll. A positive delta moves the view towards the start
    /// of the range (revealing lower-index items), a negative one towards the
    /// end. A delta absorbed at a clamped edge still counts. Movement the list
    /// performs on its own (smooth scrolling, fling coasting, bounce-back,
    /// tail-following) is excluded.
    ///
    /// Sample it at two points in time and subtract to learn whether the user
    /// scrolled the list in between, and in which direction, even while the
    /// list is also moving programmatically. Unlike comparing [`Self::first_id()`],
    /// this is unaffected by items being inserted or removed, since it measures
    /// movement rather than position.
    pub fn user_scroll_travel(&self) -> f64 {
        let Some(inner) = self.borrow() else {
            return 0.0;
        };
        inner.user_scroll_travel
    }

    /// Returns a compact debug line with the current animated scroll state.
    pub fn debug_scroll_state_line(&self) -> String {
        let Some(inner) = self.borrow() else {
            return "state=detached".to_string();
        };

        let mut state = "Stopped";
        let mut delta = 0.0;
        let mut velocity = 0.0;
        let mut target_id: Option<usize> = None;
        let mut drag_samples = 0usize;

        match &inner.scroll_state {
            ScrollState::Stopped => {}
            ScrollState::Drag { samples, .. } => {
                state = "Drag";
                drag_samples = samples.len();
            }
            ScrollState::Flick { fling, .. } => {
                state = "Flick";
                delta = fling.velocity;
            }
            ScrollState::Pulldown { .. } => {
                state = "Pulldown";
            }
            ScrollState::ScrollingTo {
                target_id: tid,
                delta: d,
                ..
            } => {
                state = "ScrollingTo";
                target_id = Some(*tid);
                delta = *d;
            }
            ScrollState::Tailing { velocity: v, .. } => {
                state = "Tailing";
                velocity = *v;
            }
        }

        let bar_pos = inner.scroll_bar.get_scroll_pos();
        let bar_total = inner.scroll_bar.get_scroll_view_total();
        let bar_visible = inner.scroll_bar.get_scroll_view_visible();

        format!(
            "state={} first_id={} first_scroll={:.2} delta={:.2} velocity={:.2} target={:?} drag_samples={} tail_range={} tail_adjust={:.2} at_end={} not_fill={} auto_tail={} smooth_tail={} select_auto={} was_scrolling={} detect_tail={} bar={:.2}/{:.2} vis={:.2}",
            state,
            inner.first_id,
            inner.first_scroll,
            delta,
            velocity,
            target_id,
            drag_samples,
            inner.tail_range,
            inner.tail_adjustment_needed,
            inner.at_end,
            inner.not_filling_viewport,
            inner.auto_tail,
            inner.smooth_tail,
            inner.select_scroll_state.is_some(),
            inner.was_scrolling,
            inner.detect_tail_in_draw,
            bar_pos,
            bar_total,
            bar_visible
        )
    }

    /// See [`PortalList::item()`].
    pub fn item(&self, cx: &mut Cx, entry_id: usize, template: LiveId) -> WidgetRef {
        if let Some(mut inner) = self.borrow_mut() {
            inner.item(cx, entry_id, template)
        } else {
            WidgetRef::empty()
        }
    }

    /// See [`PortalList::item_with_existed()`].
    pub fn item_with_existed(
        &self,
        cx: &mut Cx,
        entry_id: usize,
        template: LiveId,
    ) -> (WidgetRef, bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.item_with_existed(cx, entry_id, template)
        } else {
            (WidgetRef::empty(), false)
        }
    }

    /// See [`PortalList::get_item()`].
    pub fn get_item(&self, entry_id: usize) -> Option<(LiveId, WidgetRef)> {
        let Some(inner) = self.borrow() else {
            return None;
        };
        inner.get_item(entry_id)
    }

    pub fn position_of_item(&self, cx: &Cx, entry_id: usize) -> Option<f64> {
        let Some(inner) = self.borrow() else {
            return None;
        };
        inner.position_of_item(cx, entry_id)
    }

    pub fn items_with_actions(&self, actions: &Actions) -> ItemsWithActions {
        let mut set = Vec::new();
        self.items_with_actions_vec(actions, &mut set);
        set
    }

    fn items_with_actions_vec(&self, actions: &Actions, set: &mut ItemsWithActions) {
        let uid = self.widget_uid();
        if let Some(inner) = self.borrow() {
            for action in actions {
                if let Some(action) = action.as_widget_action() {
                    if let Some(group) = &action.group {
                        if group.group_uid == uid {
                            for (item_id, item) in inner.items.iter() {
                                if group.item_uid == item.widget.widget_uid() {
                                    set.push((*item_id, item.widget.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn any_items_with_actions(&self, actions: &Actions) -> bool {
        let uid = self.widget_uid();
        for action in actions {
            if let Some(action) = action.as_widget_action() {
                if let Some(group) = &action.group {
                    if group.group_uid == uid {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Initiates a smooth scrolling animation to the specified target item.
    ///
    /// See [`PortalList::smooth_scroll_to()`] for full documentation.
    pub fn smooth_scroll_to(
        &self,
        cx: &mut Cx,
        target_id: usize,
        speed: f64,
        max_items_to_show: Option<usize>,
        top_offset: f64,
    ) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        inner.smooth_scroll_to(cx, target_id, speed, max_items_to_show, top_offset);
    }

    /// Returns the ID of the item that is currently being smoothly scrolled to, if any.
    pub fn is_smooth_scrolling(&self) -> Option<usize> {
        let Some(inner) = self.borrow() else {
            return None;
        };
        if let ScrollState::ScrollingTo { target_id, .. } = inner.scroll_state {
            Some(target_id)
        } else {
            None
        }
    }

    /// Returns whether the given `actions` contain an action indicating that this PortalList completed
    /// a smooth scroll, reaching the target.
    pub fn smooth_scroll_reached(&self, actions: &Actions) -> bool {
        actions
            .filter_widget_actions(self.widget_uid())
            .any(|item| matches!(item.cast(), PortalListAction::SmoothScrollReached))
    }

    /// Trigger a scrolling animation to the end of the list.
    pub fn smooth_scroll_to_end(&self, cx: &mut Cx, speed: f64, max_items_to_show: Option<usize>) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        inner.smooth_scroll_to_end(cx, speed, max_items_to_show);
    }

    /// Immediately jumps to the end of the list without animation.
    pub fn scroll_to_end(&self, cx: &mut Cx) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        if inner.range_end > 0 {
            inner.first_id = inner.range_end - 1;
            inner.first_scroll = 0.0;
            inner.area.redraw(cx);
        }
    }

    /// Returns whether this PortalList is currently filling the viewport.
    pub fn is_filling_viewport(&self) -> bool {
        let Some(inner) = self.borrow() else {
            return false;
        };
        inner.is_filling_viewport()
    }

    /// It indicates if we have items not displayed towards the end of the list (below).
    pub fn further_items_bellow_exist(&self) -> bool {
        let Some(inner) = self.borrow() else {
            return false;
        };
        !(inner.at_end || inner.not_filling_viewport)
    }
}

type ItemsWithActions = Vec<(usize, WidgetRef)>;

/// Whether a list keeps a scroll delta it is handed, which makes the delta
/// spent: the event is marked handled, and no scroll view around the list
/// moves by it as well. `delta` is positive toward the list's start.
///
/// A delta the list has room for is kept, even when the room is smaller than
/// the delta. So is one a rubber band takes (`band`: the fingers are on the
/// trackpad and the edge stretches, or a stretch is showing that the delta may
/// unwind). A delta pointing past an edge the list already rests on is not, and
/// the scroll view around the list moves by it instead — the rule a
/// `ScrollBar` pinned at its limit follows, so nested scroll views and lists
/// hand a wheel on the same way.
///
/// The list is judged before the delta is applied, and `at_end` comes from its
/// last draw: a delta toward the end is added to the scroll even when the last
/// row already sits on the bottom, and only the next draw pins it back, so the
/// scroll changing is no proof the list moved.
fn list_keeps_scroll_delta(delta: f64, at_start: bool, at_end: bool, band: bool) -> bool {
    if delta == 0.0 {
        false
    } else if band {
        true
    } else if delta > 0.0 {
        !at_start
    } else {
        !at_end
    }
}

/// Where a list must sit so the row at `index` is on screen, or `None` when it
/// already is and the list must not move at all.
///
/// The window is the `visible_items` rows from `first_id`, the stretch the last
/// draw put on screen. A row that went off the start comes back `lead` rows
/// down from the start of the window, and one that went off the end comes back
/// `lead` rows up from the end of it, so the row the host cares about always
/// has a neighbour beyond it saying the list goes on. Both are clamped so the
/// row itself stays in the window, which is what a list too short for the lead,
/// or a row too near the start of the range for it, comes down to.
///
/// A window of no rows means nothing was drawn, so the row is off screen by
/// definition and comes back at the start of the window.
fn first_id_keeping_index_visible(
    index: usize,
    first_id: usize,
    visible_items: usize,
    range_start: usize,
    lead: usize,
) -> Option<usize> {
    if visible_items > 0 && index >= first_id && index < first_id + visible_items {
        return None;
    }
    if index < first_id || visible_items == 0 {
        return Some(index.saturating_sub(lead).max(range_start));
    }
    // Off the end: put the row `lead` rows up from the bottom of the window,
    // never so far that the row itself goes off the top of it.
    let first = index
        .saturating_add(lead)
        .saturating_add(1)
        .saturating_sub(visible_items)
        .max(range_start)
        .min(index);
    Some(first)
}

/// Where a short list's continuation rows go, and which way they run, from the
/// content's two edges in the viewport's own coordinates.
///
/// A list resting at its start leaves its gap after the last row and the ruling
/// runs on past it. A list resting at its end (`align_top_when_empty: false`)
/// leaves the gap at the leading edge instead, so the ruling runs the other
/// way, back before the first row. `None` when there is no gap worth ruling.
fn filler_band(
    align_top: bool,
    content_start: f64,
    content_end: f64,
    viewport: f64,
) -> Option<FillerBand> {
    let band = if align_top {
        FillerBand {
            origin: content_end,
            gap: viewport - content_end,
            step: 1,
        }
    } else {
        FillerBand {
            origin: content_start,
            gap: content_start,
            step: -1,
        }
    };
    (band.gap > FILLER_MIN_PITCH).then_some(band)
}

/// How many continuation rows of `pitch` fit in `gap`, and how tall the last of
/// them is once it is clamped to what is left.
///
/// A gap shorter than one row is one clamped row, not none: that sliver is the
/// top of the row the list would have drawn next, and leaving it blank is the
/// ragged edge the ruling exists to remove. A gap or a pitch too small to rule,
/// or one that isn't a number, draws nothing at all.
fn filler_run(gap: f64, pitch: f64, max_rows: usize) -> FillerRun {
    let none = FillerRun { rows: 0, last: 0.0 };
    if !(gap > FILLER_MIN_PITCH) || !(pitch > FILLER_MIN_PITCH) || max_rows == 0 {
        return none;
    }
    let whole = (gap / pitch).floor();
    // A float too big for a usize saturates rather than wrapping, and the cap
    // below catches it either way.
    let mut rows = whole as usize;
    let mut last = pitch;
    let rest = gap - whole * pitch;
    if rest > FILLER_MIN_PITCH {
        rows += 1;
        last = rest;
    }
    if rows > max_rows {
        rows = max_rows;
        last = pitch;
    }
    FillerRun { rows, last }
}

/// The striping of the continuation row `offset` rows past `from_index`
/// (negative for the rows before it): `0.0` on an even row index and `1.0` on
/// an odd one, the parity a ruled list's own rows stripe by, so the ruling
/// carries on in the colour the next real row would have had.
fn filler_alternate(from_index: usize, offset: isize) -> f32 {
    let index = from_index as i64 + offset as i64;
    if index.rem_euclid(2) == 0 {
        0.0
    } else {
        1.0
    }
}

impl PortalListSet {
    pub fn set_first_id(&self, id: usize) {
        for list in self.iter() {
            list.set_first_id(id);
        }
    }

    pub fn items_with_actions(&self, actions: &Actions) -> ItemsWithActions {
        let mut set = Vec::new();
        for list in self.iter() {
            list.items_with_actions_vec(actions, &mut set);
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ScrollEvent;
    use crate::log_list::LogList;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    // Deltas are positive toward the start: a wheel rolled up.
    const UP: f64 = 60.0;
    const DOWN: f64 = -60.0;

    /// Between its edges a list keeps every wheel, whichever way it rolls, so
    /// the page around it stays put.
    #[test]
    fn a_list_between_its_edges_keeps_the_wheel_both_ways() {
        assert!(list_keeps_scroll_delta(UP, false, false, false));
        assert!(list_keeps_scroll_delta(DOWN, false, false, false));
    }

    /// At the top, a wheel rolled up has nowhere to take the list and goes on
    /// to the page; rolled down it still moves the list and stays.
    #[test]
    fn at_the_top_only_the_wheel_toward_the_top_passes_on() {
        assert!(!list_keeps_scroll_delta(UP, true, false, false));
        assert!(list_keeps_scroll_delta(DOWN, true, false, false));
    }

    /// The same at the bottom, the other way round.
    #[test]
    fn at_the_bottom_only_the_wheel_toward_the_bottom_passes_on() {
        assert!(!list_keeps_scroll_delta(DOWN, false, true, false));
        assert!(list_keeps_scroll_delta(UP, false, true, false));
    }

    /// A list whose rows all fit rests on both edges and never scrolls, so
    /// it takes no wheel from the page it sits on.
    #[test]
    fn a_list_that_fits_passes_every_wheel_on() {
        assert!(!list_keeps_scroll_delta(UP, true, true, false));
        assert!(!list_keeps_scroll_delta(DOWN, true, true, false));
    }

    /// A rubber band past an edge is the list moving: fingers stretching it
    /// keep the delta even at that edge.
    #[test]
    fn a_rubber_band_keeps_what_it_stretches_by() {
        assert!(list_keeps_scroll_delta(UP, true, true, true));
        assert!(list_keeps_scroll_delta(DOWN, true, true, true));
    }

    /// No delta along the list's axis is nothing to keep: the other axis of
    /// the same event stays free for a scroll view that runs across it.
    #[test]
    fn no_delta_is_never_kept() {
        assert!(!list_keeps_scroll_delta(0.0, false, false, false));
        assert!(!list_keeps_scroll_delta(0.0, false, false, true));
    }

    // A list showing five rows from row 10, i.e. rows 10 to 14.
    const FIRST: usize = 10;
    const WINDOW: usize = 5;
    /// One row of daylight between a recalled row and the edge it came over.
    const LEAD: usize = 1;

    /// Asking for a row that is already showing is not a scroll request: the
    /// list holds exactly where it is, wherever in the window the row sits.
    #[test]
    fn a_row_already_on_screen_holds_the_list_still() {
        for index in FIRST..FIRST + WINDOW {
            assert_eq!(
                first_id_keeping_index_visible(index, FIRST, WINDOW, 0, LEAD),
                None,
                "the list moved for row {index}, which was already showing"
            );
        }
    }

    /// A row that went off the top comes back with the lead above it, so it
    /// isn't pinned to the very edge.
    #[test]
    fn a_row_off_the_top_comes_back_short_of_the_top() {
        assert_eq!(
            first_id_keeping_index_visible(3, FIRST, WINDOW, 0, LEAD),
            Some(2)
        );
        assert_eq!(
            first_id_keeping_index_visible(9, FIRST, WINDOW, 0, 3),
            Some(6)
        );
    }

    /// The same off the bottom: the row lands one row up from the end of the
    /// window rather than half off it.
    #[test]
    fn a_row_off_the_bottom_comes_back_short_of_the_bottom() {
        let first = first_id_keeping_index_visible(20, FIRST, WINDOW, 0, LEAD).unwrap();
        assert_eq!(first, 17);
        // 17..22 shows the row with one row after it.
        assert!(first <= 20 && 20 < first + WINDOW);
        assert_eq!(20 - first + LEAD, WINDOW - 1);
    }

    /// The lead is daylight, not a promise: near the start of the range there
    /// is none to be had, and the list stops at the first row it has.
    #[test]
    fn the_lead_never_runs_past_the_start_of_the_range() {
        assert_eq!(
            first_id_keeping_index_visible(0, FIRST, WINDOW, 0, LEAD),
            Some(0)
        );
        assert_eq!(
            first_id_keeping_index_visible(5, FIRST, WINDOW, 5, LEAD),
            Some(5)
        );
    }

    /// A window with no room for the lead still shows the row itself: the row
    /// is the request, the lead is the manners.
    #[test]
    fn a_window_too_short_for_the_lead_still_shows_the_row() {
        assert_eq!(
            first_id_keeping_index_visible(20, FIRST, 1, 0, LEAD),
            Some(20)
        );
        assert_eq!(
            first_id_keeping_index_visible(20, FIRST, 2, 0, 4),
            Some(20)
        );
    }

    /// A list that drew nothing has no window to judge against, so the row is
    /// off screen by definition and comes back at the start of one.
    #[test]
    fn a_list_that_drew_nothing_brings_the_row_to_its_start() {
        assert_eq!(
            first_id_keeping_index_visible(FIRST, FIRST, 0, 0, LEAD),
            Some(FIRST - LEAD)
        );
    }

    /// A short list resting at its start leaves its gap after the last row,
    /// and the ruling runs on past it.
    #[test]
    fn a_list_resting_at_its_start_rules_the_gap_after_its_last_row() {
        assert_eq!(
            filler_band(true, 0.0, 80.0, 120.0),
            Some(FillerBand {
                origin: 80.0,
                gap: 40.0,
                step: 1
            })
        );
    }

    /// A short list resting at its end leaves the gap at the other edge, so
    /// the ruling runs back before the first row instead.
    #[test]
    fn a_list_resting_at_its_end_rules_the_gap_before_its_first_row() {
        assert_eq!(
            filler_band(false, 40.0, 120.0, 120.0),
            Some(FillerBand {
                origin: 40.0,
                gap: 40.0,
                step: -1
            })
        );
    }

    /// Rows that reach the edge leave nothing to rule, whichever edge the list
    /// rests on — and neither does a sliver too thin to be a row.
    #[test]
    fn rows_that_fill_the_viewport_leave_nothing_to_rule() {
        assert_eq!(filler_band(true, 0.0, 120.0, 120.0), None);
        assert_eq!(filler_band(false, 0.0, 120.0, 120.0), None);
        assert_eq!(filler_band(true, 0.0, 119.7, 120.0), None);
    }

    /// The gap takes whole rows at the pitch of the row it continues, and the
    /// last one takes what is left over.
    #[test]
    fn whole_rows_fill_the_gap_and_the_last_one_takes_what_is_left() {
        assert_eq!(
            filler_run(45.0, 20.0, MAX_FILLER_ROWS),
            FillerRun {
                rows: 3,
                last: 5.0
            }
        );
        assert_eq!(
            filler_run(40.0, 20.0, MAX_FILLER_ROWS),
            FillerRun {
                rows: 2,
                last: 20.0
            }
        );
    }

    /// A gap shorter than one row is one clamped row: that sliver is the top
    /// of the row the list would have drawn next, and leaving it blank is the
    /// ragged edge the ruling is there to remove.
    #[test]
    fn a_gap_shorter_than_a_row_is_one_clamped_row() {
        assert_eq!(
            filler_run(5.0, 20.0, MAX_FILLER_ROWS),
            FillerRun {
                rows: 1,
                last: 5.0
            }
        );
    }

    /// Nothing is ruled without a gap and a row height to rule it by, and a
    /// number that isn't one rules nothing either.
    #[test]
    fn a_row_with_no_height_rules_nothing() {
        let none = FillerRun {
            rows: 0,
            last: 0.0,
        };
        assert_eq!(filler_run(40.0, 0.0, MAX_FILLER_ROWS), none);
        assert_eq!(filler_run(0.0, 20.0, MAX_FILLER_ROWS), none);
        assert_eq!(filler_run(0.2, 20.0, MAX_FILLER_ROWS), none);
        assert_eq!(filler_run(f64::NAN, 20.0, MAX_FILLER_ROWS), none);
        assert_eq!(filler_run(40.0, f64::NAN, MAX_FILLER_ROWS), none);
        assert_eq!(filler_run(40.0, 20.0, 0), none);
    }

    /// However thin the rows, one draw only ever puts so many of them down;
    /// the rest would be off the far edge anyway.
    #[test]
    fn the_run_of_rows_is_capped() {
        assert_eq!(
            filler_run(1000.0, 1.0, 8),
            FillerRun {
                rows: 8,
                last: 1.0
            }
        );
        assert_eq!(filler_run(f64::INFINITY, 1.0, 8).rows, 8);
    }

    /// The striping carries on from the row it continues, forwards past the
    /// last row and backwards before the first.
    #[test]
    fn the_striping_carries_on_from_the_row_it_continues() {
        assert_eq!(filler_alternate(4, 1), 1.0);
        assert_eq!(filler_alternate(4, 2), 0.0);
        assert_eq!(filler_alternate(5, 1), 0.0);
        assert_eq!(filler_alternate(0, -1), 1.0);
        assert_eq!(filler_alternate(0, -2), 0.0);
    }

    const PANE: DVec2 = dvec2(300.0, 120.0);

    /// One draw of the pane, in the pass and draw list the test keeps.
    fn frame(cx: &mut Cx, log: &mut LogList, pass: &DrawPass, draw_list: &mut DrawList2d) {
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(pass, None);
        draw_list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(PANE, Layout::flow_down());
        let _ = log.draw_walk(&mut cx2d, &mut Scope::empty(), Walk::fixed(PANE.x, PANE.y));
        cx2d.end_pass_sized_turtle();
        draw_list.end(&mut cx2d);
        cx2d.end_pass(pass);
    }

    /// A wheel rolled over the middle of the pane, `dy` toward the end, and
    /// whether the list kept it — which is all a scroll view around the list
    /// reads before moving by the same wheel.
    fn wheel(cx: &mut Cx, log: &mut LogList, dy: f64, handled: bool) -> bool {
        let event = Event::Scroll(ScrollEvent {
            window_id: WindowId(1, 1),
            scroll: dvec2(0.0, dy),
            abs: PANE * 0.5,
            modifiers: KeyModifiers::default(),
            handled_x: Cell::new(false),
            handled_y: Cell::new(handled),
            is_mouse: true,
            time: 0.0,
            phase: ScrollPhase::None,
        });
        log.handle_event(cx, &event, &mut Scope::empty());
        event.scroll_handled(Vec2Index::Y)
    }

    /// The first row showing and how far it is scrolled, to a hundredth of a
    /// point: a draw re-measures the rows, and that alone moves the scroll by
    /// millionths.
    fn place(cx: &Cx, log: &LogList) -> (usize, f64) {
        let list = log.portal_list(cx, ids!(list));
        let list = list.borrow().expect("the log holds no portal list");
        (list.first_id(), (list.first_scroll() * 100.0).round() / 100.0)
    }

    /// How many rows the list last drew.
    fn window(cx: &Cx, log: &LogList) -> usize {
        log.portal_list(cx, ids!(list)).visible_items()
    }

    /// Ask the list to keep `index` on screen.
    fn keep(cx: &mut Cx, log: &LogList, index: usize) {
        let list = log.portal_list(cx, ids!(list));
        list.keep_index_visible(cx, index);
    }

    /// Keeping a row, through a real list: a row that is showing holds the
    /// list exactly where it is, and one that has gone off either edge comes
    /// back with a row to spare beyond it.
    #[test]
    fn the_list_holds_the_row_it_was_asked_to_keep() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let mut log = cx.with_vm(LogList::script_new_with_default);
        log.lines = (0..200).map(|n| format!("log | line {n}")).collect();
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, PANE);
        let mut draw_list = DrawList2d::new(&mut cx);
        for _ in 0..3 {
            frame(&mut cx, &mut log, &pass, &mut draw_list);
        }
        // Off the tail: a tailing list follows its last row and a request to
        // keep another one is dropped rather than fought.
        for _ in 0..2 {
            wheel(&mut cx, &mut log, -600.0, false);
            frame(&mut cx, &mut log, &pass, &mut draw_list);
        }
        let (first, _) = place(&cx, &log);
        let rows = window(&cx, &log);
        assert!(
            first > 40 && first + rows + 20 < 200,
            "the log did not settle in its middle: {first} + {rows}"
        );

        // A row that is showing is not a scroll request.
        let before = place(&cx, &log);
        keep(&mut cx, &log, first + 1);
        frame(&mut cx, &mut log, &pass, &mut draw_list);
        assert_eq!(place(&cx, &log), before, "a row already showing moved the list");

        // A row off the top comes back one row short of it.
        keep(&mut cx, &log, first - 20);
        frame(&mut cx, &mut log, &pass, &mut draw_list);
        frame(&mut cx, &mut log, &pass, &mut draw_list);
        assert_eq!(
            place(&cx, &log).0,
            first - 21,
            "the recalled row landed pinned to the top edge"
        );

        // And a row off the bottom comes back with a row to spare below it.
        let far = first + 20;
        keep(&mut cx, &log, far);
        frame(&mut cx, &mut log, &pass, &mut draw_list);
        frame(&mut cx, &mut log, &pass, &mut draw_list);
        let (now, _) = place(&cx, &log);
        let rows = window(&cx, &log);
        assert!(
            now <= far && far < now + rows,
            "the row the list was asked to keep is off screen: {now} + {rows} for {far}"
        );
        assert!(
            far + LEAD < now + rows,
            "the recalled row landed pinned to the bottom edge: {now} + {rows} for {far}"
        );
    }

    /// The continuation rows are ground, not layout: turning them on under a
    /// list too short to fill its viewport leaves every real row exactly where
    /// it was.
    #[test]
    fn the_continuation_rows_leave_the_real_rows_alone() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let mut log = cx.with_vm(LogList::script_new_with_default);
        log.lines = (0..3).map(|n| format!("log | line {n}")).collect();
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, PANE);
        let mut draw_list = DrawList2d::new(&mut cx);
        for _ in 0..3 {
            frame(&mut cx, &mut log, &pass, &mut draw_list);
        }
        let before = place(&cx, &log);
        {
            let list = log.portal_list(&cx, ids!(list));
            let mut list = list.borrow_mut().expect("the log holds no portal list");
            assert!(
                !list.is_filling_viewport(),
                "three lines filled the pane, so there is no gap to rule"
            );
            list.filler_rows = true;
        }
        for _ in 0..2 {
            frame(&mut cx, &mut log, &pass, &mut draw_list);
        }
        assert_eq!(place(&cx, &log), before, "the ruling moved the rows it fills after");
    }

    /// The wheel over a list inside a scrolling page: the list keeps every
    /// wheel it moves by, so the page stays put, and hands on the one that
    /// points past the edge it rests on, so the page moves instead. A log
    /// follows its newest line, which puts this one at its bottom to start.
    #[test]
    fn a_list_keeps_the_wheel_it_moves_by_and_hands_on_the_wheel_past_its_edge() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let mut log = cx.with_vm(LogList::script_new_with_default);
        log.lines = (0..200).map(|n| format!("log | line {n}")).collect();
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, PANE);
        let mut draw_list = DrawList2d::new(&mut cx);
        for _ in 0..3 {
            frame(&mut cx, &mut log, &pass, &mut draw_list);
        }
        let bottom = place(&cx, &log);
        assert!(bottom.0 > 150, "the log did not open on its newest lines: {bottom:?}");

        assert!(!wheel(&mut cx, &mut log, 60.0, false), "a wheel past the bottom was kept");
        // The draw is what pins the list back on its bottom.
        frame(&mut cx, &mut log, &pass, &mut draw_list);
        assert_eq!(place(&cx, &log), bottom, "the list moved past its bottom");
        assert!(wheel(&mut cx, &mut log, -60.0, false), "a wheel up the list was handed on");
        frame(&mut cx, &mut log, &pass, &mut draw_list);
        assert_ne!(place(&cx, &log), bottom, "the kept wheel did not move the list");
        assert!(wheel(&mut cx, &mut log, 30.0, false), "off the bottom, a wheel down is the list's");
        frame(&mut cx, &mut log, &pass, &mut draw_list);

        // A wheel a row's own scroll view already used moves nothing here.
        let before = place(&cx, &log);
        assert!(wheel(&mut cx, &mut log, -60.0, true));
        frame(&mut cx, &mut log, &pass, &mut draw_list);
        assert_eq!(place(&cx, &log), before, "the list moved by a wheel already used");

        for _ in 0..200 {
            if place(&cx, &log) == (0, 0.0) {
                break;
            }
            wheel(&mut cx, &mut log, -600.0, false);
            frame(&mut cx, &mut log, &pass, &mut draw_list);
        }
        assert_eq!(place(&cx, &log), (0, 0.0), "the wheel never reached the top");
        assert!(!wheel(&mut cx, &mut log, -60.0, false), "a wheel past the top was kept");
        assert!(wheel(&mut cx, &mut log, 60.0, false), "a wheel down from the top was handed on");
    }
}
