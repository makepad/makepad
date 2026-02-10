use {
    crate::{
        animator::AnimatorImpl,
        makepad_derive_widget::*,
        makepad_draw::*,
        scroll_bar::{ScrollAxis, ScrollBar, ScrollBarAction},
        widget::*,
    },
    std::collections::HashMap,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PortalListBase = #(PortalList::register_widget(vm))

    mod.widgets.PortalList = set_type_default() do mod.widgets.PortalListBase {
        width: Fill
        height: Fill
        capture_overload: true
        scroll_bar: mod.widgets.ScrollBar {}
        flow: Down
    }
}

/// The maximum number of items that will be shown as part of a smooth scroll animation.
const SMOOTH_SCROLL_MAXIMUM_WINDOW: usize = 20;

#[derive(Clone, Copy)]
struct ScrollSample {
    abs: f64,
    time: f64,
}

enum ScrollState {
    Stopped,
    Drag {
        samples: Vec<ScrollSample>,
    },
    Flick {
        delta: f64,
        next_frame: NextFrame,
    },
    Pulldown {
        next_frame: NextFrame,
    },
    ScrollingTo {
        target_id: usize,
        delta: f64,
        next_frame: NextFrame,
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
    Scroll,
    SmoothScrollReached,
    #[default]
    None,
}

impl ListDrawState {
    fn is_down_again(&self) -> bool {
        matches!(self, Self::DownAgain { .. })
    }
}

#[derive(Default)]
struct WidgetItem {
    widget: WidgetRef,
    template: LiveId,
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

    /// Update the height at index i to new_height
    fn update(&mut self, i: usize, new_height: f64) {
        if i >= self.size {
            return;
        }

        let old_height = self.point_query(i);
        let delta = new_height - old_height;

        if delta.abs() < 0.001 {
            // No significant change
            self.measured[i] = true;
            return;
        }

        self.measured[i] = true;

        let mut j = i + 1; // convert to 1-indexed
        while j <= self.size {
            self.tree[j] += delta;
            j += j & j.wrapping_neg(); // add lowest set bit
        }
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
        if (new_default - self.default_height).abs() < 0.001 {
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

    #[live(0.2)]
    flick_scroll_minimum: f64,
    #[live(80.0)]
    flick_scroll_maximum: f64,
    #[live(0.005)]
    flick_scroll_scaling: f64,
    #[live(0.97)]
    flick_scroll_decay: f64,
    #[live(80.0)]
    max_pull_down: f64,
    #[live(true)]
    align_top_when_empty: bool,
    #[live(false)]
    grab_key_focus: bool,
    #[live(true)]
    drag_scrolling: bool,

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

    // Templates stored as rooted ScriptObjectRef - populated in on_after_apply
    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    items: ComponentMap<usize, WidgetItem>,
    #[rust]
    reusable_items: Vec<WidgetItem>,

    #[rust(ScrollState::Stopped)]
    scroll_state: ScrollState,
    /// Whether the PortalList was actively scrolling during the most recent finger down hit.
    #[rust]
    was_scrolling: bool,

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
        cx.begin_turtle(walk, self.layout);
        self.draw_align_list.clear();
    }

    fn end(&mut self, cx: &mut Cx2d) {
        self.at_end = false;
        self.not_filling_viewport = false;

        let vi = self.vec_index;
        let mut visible_items = 0;

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
                for i in first_index..list.len() {
                    let item = &list[i];
                    last_pos += item.size.index(vi);
                    if item.index < self.range_end {
                        last_item_pos = Some(last_pos);
                    } else {
                        break;
                    }
                }

                if list[0].index == self.range_start {
                    let mut total = 0.0;
                    for item in list.iter() {
                        if item.index >= self.range_end {
                            break;
                        }
                        total += item.size.index(vi);
                    }
                    self.not_filling_viewport = total < viewport.size.index(vi);
                }

                if list.first().unwrap().index == self.range_start && first_pos > 0.0 {
                    let min = if let ScrollState::Stopped = self.scroll_state {
                        0.0
                    } else {
                        self.max_pull_down
                    };

                    let mut pos = first_pos.min(min);
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
                            -first_pos
                        } else {
                            let ret = viewport.size.index(vi) - last_item_pos;
                            if ret >= 0.0 {
                                self.at_end = true;
                            }
                            ret.max(0.0)
                        }
                    } else {
                        0.0
                    };

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
                // Capture measured heights into height_tree and height_cache
                for item in list.iter() {
                    if item.index >= self.range_start && item.index < self.range_end {
                        let height = item.size.index(vi);
                        let idx = item.index - self.range_start;

                        // Record in cache for average calculation
                        self.height_cache.record_height(height);

                        // Update in tree
                        if let Some(ref mut tree) = self.height_tree {
                            tree.update(idx, height);
                        }
                    }
                }

                // Update unmeasured items with new average if it changed significantly
                if let Some(ref mut tree) = self.height_tree {
                    let new_avg = self.height_cache.average();
                    tree.update_default_height(new_avg);
                }

                // When tail_range is true and we're not already at the end, we need to scroll
                // down to keep the bottom of the content visible
                if self.tail_range && !self.at_end {
                    // Calculate how much we need to scroll to get to the end
                    // The shift calculation above tells us: viewport_height - last_item_pos
                    // When this is negative, content extends beyond viewport
                    if let Some(last_pos) = last_item_pos {
                        let viewport_height = viewport.size.index(vi);
                        let overflow = last_pos - viewport_height;
                        if overflow > 0.5 {
                            // Content extends beyond viewport, store the adjustment needed
                            self.tail_adjustment_needed = overflow;
                        }
                    }
                }

                // Apply tail scroll adjustment - scroll down to keep bottom visible
                if self.tail_adjustment_needed > 0.5 {
                    if self.smooth_tail {
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

        // Keep items when selecting so we can copy text from scrolled-out items
        if !self.keep_invisible && !self.is_selecting {
            if self.reuse_items {
                let reusable_items = &mut self.reusable_items;
                self.items.retain_visible_with(|v| {
                    reusable_items.push(v);
                });
            } else {
                self.items.retain_visible();
            }
        }

        cx.end_turtle_with_area(&mut self.area);
        self.visible_items = visible_items;
    }

    /// Returns the index of the next visible item that will be drawn by this PortalList.
    pub fn next_visible_item(&mut self, cx: &mut Cx2d) -> Option<usize> {
        let vi = self.vec_index;
        let layout = if vi == Vec2Index::Y {
            Layout::flow_down()
        } else {
            Layout::flow_right()
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
                                    metrics: Metrics::default(),
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
                                    metrics: Metrics::default(),
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
                                            metrics: Metrics::default(),
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
                                            metrics: Metrics::default(),
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
                                    metrics: Metrics::default(),
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
                                    metrics: Metrics::default(),
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
                                cx.begin_turtle(
                                    Walk {
                                        abs_pos: Some(dvec2(
                                            viewport.pos.x,
                                            viewport.pos.y + total_height,
                                        )),
                                        margin: Default::default(),
                                        width: Size::fill(),
                                        height: Size::fit(),
                                        metrics: Metrics::default(),
                                    },
                                    Layout::flow_down(),
                                );
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

                    cx.begin_turtle(
                        Walk {
                            abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y)),
                            margin: Default::default(),
                            width: Size::fill(),
                            height: Size::fit(),
                            metrics: Metrics::default(),
                        },
                        Layout::flow_down(),
                    );

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
            match self.items.entry(entry_id) {
                Entry::Occupied(mut occ) => {
                    if occ.get().template == template {
                        (occ.get().widget.clone(), true)
                    } else {
                        let widget_ref = if let Some(pos) = self
                            .reusable_items
                            .iter()
                            .position(|v| v.template == template)
                        {
                            self.reusable_items.remove(pos).widget
                        } else {
                            cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value))
                        };
                        occ.insert(WidgetItem {
                            template,
                            widget: widget_ref.clone(),
                        });
                        (widget_ref, false)
                    }
                }
                Entry::Vacant(vac) => {
                    let widget_ref = if let Some(pos) = self
                        .reusable_items
                        .iter()
                        .position(|v| v.template == template)
                    {
                        self.reusable_items.remove(pos).widget
                    } else {
                        cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value))
                    };
                    vac.insert(WidgetItem {
                        template,
                        widget: widget_ref.clone(),
                    });
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

    fn delta_top_scroll(
        &mut self,
        cx: &mut Cx,
        delta: f64,
        clip_top: bool,
        transition_to_pulldown: bool,
    ) {
        if self.range_start == self.range_end {
            self.first_scroll = 0.0;
        } else {
            self.first_scroll += delta;
        }

        if self.first_id == self.range_start {
            self.first_scroll = self.first_scroll.min(self.max_pull_down);
            if transition_to_pulldown && self.first_scroll > 0.0 {
                self.scroll_state = ScrollState::Pulldown {
                    next_frame: cx.new_next_frame(),
                };
            }
        }
        if clip_top && self.first_id == self.range_start && self.first_scroll > 0.0 {
            self.first_scroll = 0.0;
        }
        if self.at_end && delta < 0.0 {
            self.was_scrolling = false;
            self.scroll_state = ScrollState::Stopped;
        }
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

    /// Sets the first visible item and scroll offset.
    pub fn set_first_id_and_scroll(&mut self, first_id: usize, first_scroll: f64) {
        self.first_id = first_id;
        self.first_scroll = first_scroll;
    }

    /// Returns the number of items that are currently visible in the viewport.
    pub fn visible_items(&self) -> usize {
        self.visible_items
    }

    /// Initiates a smooth scrolling animation to the specified target item in the list.
    pub fn smooth_scroll_to(
        &mut self,
        cx: &mut Cx,
        target_id: usize,
        speed: f64,
        max_items_to_show: Option<usize>,
    ) {
        if self.items.is_empty() {
            return;
        }
        if target_id < self.range_start || target_id > self.range_end {
            return;
        }

        let max_items_to_show = max_items_to_show.unwrap_or(SMOOTH_SCROLL_MAXIMUM_WINDOW);
        let scroll_direction: f64;
        let starting_id: Option<usize>;
        if target_id > self.first_id {
            scroll_direction = -1.0;
            starting_id = ((target_id.saturating_sub(self.first_id)) > max_items_to_show)
                .then_some(target_id.saturating_sub(max_items_to_show));
        } else {
            scroll_direction = 1.0;
            starting_id = ((self.first_id.saturating_sub(target_id)) > max_items_to_show)
                .then_some(target_id + max_items_to_show);
        }

        if let Some(start) = starting_id {
            self.first_id = start;
        }
        self.scroll_state = ScrollState::ScrollingTo {
            target_id,
            delta: speed.abs() * scroll_direction,
            next_frame: cx.new_next_frame(),
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
        let speed = speed * self.range_end as f64;
        self.smooth_scroll_to(cx, self.range_end, speed, max_items_to_show);
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
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }

    fn uid_to_widget(&self, uid: WidgetUid) -> WidgetRef {
        for item in self.items.values() {
            let r = item.widget.uid_to_widget(uid);
            if !r.is_empty() {
                return r;
            }
        }
        WidgetRef::empty()
    }

    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        for item in self.items.values() {
            item.widget.find_widgets(path, cached, results);
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        for item in self.items.values() {
            item.widget.find_widgets_from_point(cx, point, found);
        }
    }

    fn widget_tree_walk(&self, nodes: &mut Vec<WidgetTreeNode>) {
        for item in self.items.values() {
            item.widget.widget_tree_walk(nodes);
        }
    }
}

impl Widget for PortalList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();

        let mut scroll_to = None;
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

            cx.widget_action(uid, &scope.path, PortalListAction::Scroll);
            self.was_scrolling = false;
            self.area.redraw(cx);
        }

        // When selectable, we handle mouse/touch events at PortalList level for cross-item selection.
        // However, we need to pass through events to interactive items (links, buttons, etc.)
        // so they can be clicked. We check if the event point hits any interactive item.
        //
        // Hover events (FingerHoverIn/Out/Over) are ALWAYS passed through so interactive items
        // can properly show/hide their hover states.
        let mut pass_through_to_children = true;
        if self.selectable {
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
                    // Mouse above viewport - scroll up (only if not already at top)
                    if self.first_id > self.range_start || self.first_scroll < 0.0 {
                        let distance = (top_edge + scroll_margin - mouse_pos).max(1.0);
                        let scroll_speed = (5.0 + distance * 0.5).clamp(5.0, 50.0);
                        self.delta_top_scroll(cx, scroll_speed, false, false);
                        self.area.redraw(cx);
                    }
                } else if mouse_pos > bottom_edge - scroll_margin {
                    // Mouse below viewport - scroll down (only if not already at end)
                    if !self.at_end {
                        let distance = (mouse_pos - (bottom_edge - scroll_margin)).max(1.0);
                        let scroll_speed = -(5.0 + distance * 0.5).clamp(5.0, 50.0);
                        self.delta_top_scroll(cx, scroll_speed, false, false);
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
            } => {
                if next_frame.is_event(event).is_some() {
                    let target_id = *target_id;

                    let distance_to_target = target_id as isize - self.first_id as isize;
                    let target_passed = distance_to_target.signum() == delta.signum() as isize;
                    if target_passed {
                        self.first_id = target_id;
                        self.area.redraw(cx);
                    }

                    let distance_to_target = target_id as isize - self.first_id as isize;
                    let target_visible_at_end = self.at_end && target_id > self.first_id;
                    let target_reached = distance_to_target == 0 || target_visible_at_end;

                    if !target_reached {
                        *next_frame = cx.new_next_frame();
                        let delta = *delta;
                        self.delta_top_scroll(cx, delta, true, false);
                        cx.widget_action(uid, &scope.path, PortalListAction::Scroll);
                        self.area.redraw(cx);
                    } else {
                        self.was_scrolling = false;
                        self.scroll_state = ScrollState::Stopped;
                        cx.widget_action(uid, &scope.path, PortalListAction::SmoothScrollReached);
                    }
                }
            }
            ScrollState::Flick { delta, next_frame } => {
                if next_frame.is_event(event).is_some() {
                    *delta = *delta * self.flick_scroll_decay;
                    if delta.abs() > self.flick_scroll_minimum {
                        *next_frame = cx.new_next_frame();
                        let delta = *delta;
                        self.delta_top_scroll(cx, delta, false, true);
                        cx.widget_action(uid, &scope.path, PortalListAction::Scroll);
                        self.area.redraw(cx);
                    } else {
                        self.was_scrolling = false;
                        self.scroll_state = ScrollState::Stopped;
                    }
                }
            }
            ScrollState::Pulldown { next_frame } => {
                if next_frame.is_event(event).is_some() {
                    if self.first_id == self.range_start && self.first_scroll > 0.0 {
                        self.first_scroll *= 0.85;
                        if self.first_scroll < 1.0 {
                            self.first_scroll = 0.0;
                            self.was_scrolling = false;
                            self.scroll_state = ScrollState::Stopped;
                        } else {
                            *next_frame = cx.new_next_frame();
                            cx.widget_action(uid, &scope.path, PortalListAction::Scroll);
                        }
                        self.area.redraw(cx);
                    } else {
                        self.was_scrolling = false;
                        self.scroll_state = ScrollState::Stopped;
                    }
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
                        cx.widget_action(uid, &scope.path, PortalListAction::Scroll);
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
                Hit::FingerScroll(e) => {
                    self.tail_range = false;
                    self.detect_tail_in_draw = true;
                    self.was_scrolling = false;
                    self.scroll_state = ScrollState::Stopped;
                    // For mouse wheel: clip to top and don't transition to pulldown
                    // (pulldown/overscroll is only for touch drag/flick)
                    self.delta_top_scroll(cx, -e.scroll.index(vi), true, false);
                    cx.widget_action(uid, &scope.path, PortalListAction::Scroll);
                    self.area.redraw(cx);
                }
                Hit::KeyDown(ke) => match ke.key_code {
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
                    _ => (),
                },
                Hit::FingerDown(fe) => {
                    if self.grab_key_focus {
                        cx.set_key_focus(self.area);
                    }
                    self.tail_range = false;
                    self.was_scrolling = match &self.scroll_state {
                        ScrollState::Drag { samples } => samples.len() > 1,
                        ScrollState::Stopped => false,
                        _ => true,
                    };

                    // Handle selection when selectable, but not if clicking on interactive items
                    let on_interactive = self.point_hits_interactive_item(cx, fe.abs);
                    if self.selectable && fe.is_primary_hit() && !on_interactive {
                        let hit = self.hit_test_selection(cx, fe.abs);
                        if let Some((item_id, char_idx)) = hit {
                            cx.set_key_focus(self.area);
                            self.selection_anchor = Some((item_id, char_idx));
                            self.selection_cursor = Some((item_id, char_idx));
                            self.is_selecting = true;
                            self.select_scroll_state = Some(SelectScrollState {
                                next_frame: cx.new_next_frame(),
                                last_abs: fe.abs,
                            });
                            self.update_item_selections(cx);
                        }
                    } else if self.drag_scrolling && fe.is_primary_hit() && !on_interactive {
                        self.scroll_state = ScrollState::Drag {
                            samples: vec![ScrollSample {
                                abs: fe.abs.index(vi),
                                time: fe.time,
                            }],
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
                        // Don't override cursor when over interactive items (they set their own)
                        if !self.point_hits_interactive_item(cx, e.abs) {
                            cx.set_cursor(MouseCursor::Default);
                        }
                        if let ScrollState::Drag { samples } = &mut self.scroll_state {
                            let new_abs = e.abs.index(vi);
                            let old_sample = *samples.last().unwrap();
                            samples.push(ScrollSample {
                                abs: new_abs,
                                time: e.time,
                            });
                            if samples.len() > 4 {
                                samples.remove(0);
                            }
                            self.delta_top_scroll(cx, new_abs - old_sample.abs, false, false);
                            self.area.redraw(cx);
                        }
                    }
                }
                Hit::FingerUp(fe) if fe.is_primary_hit() => {
                    // End selection if we were selecting
                    if self.is_selecting {
                        self.is_selecting = false;
                        self.select_scroll_state = None;
                    }

                    if let ScrollState::Drag { samples } = &mut self.scroll_state {
                        let mut last = None;
                        let mut scaled_delta = 0.0;
                        let mut total_delta = 0.0;
                        for sample in samples.iter().rev() {
                            if last.is_none() {
                                last = Some(sample);
                            } else {
                                total_delta += last.unwrap().abs - sample.abs;
                                scaled_delta += (last.unwrap().abs - sample.abs)
                                    / (last.unwrap().time - sample.time);
                            }
                        }
                        scaled_delta *= self.flick_scroll_scaling;
                        if self.first_id == self.range_start && self.first_scroll > 0.0 {
                            self.scroll_state = ScrollState::Pulldown {
                                next_frame: cx.new_next_frame(),
                            };
                        } else if total_delta.abs() > 10.0
                            && scaled_delta.abs() > self.flick_scroll_minimum
                        {
                            self.scroll_state = ScrollState::Flick {
                                delta: scaled_delta
                                    .min(self.flick_scroll_maximum)
                                    .max(-self.flick_scroll_maximum),
                                next_frame: cx.new_next_frame(),
                            };
                        } else {
                            self.was_scrolling = false;
                            self.scroll_state = ScrollState::Stopped;
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
            inner.first_id = id;
            inner.first_scroll = s;
        }
    }

    /// Sets the first item to be shown by this PortalList to the item with the given `id`.
    pub fn set_first_id(&self, id: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.first_id = id;
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

    /// Returns whether the given `actions` contain an action indicating that this PortalList was scrolled.
    pub fn scrolled(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let PortalListAction::Scroll = item.cast() {
                return true;
            }
        }
        false
    }

    /// Returns the current scroll offset of this PortalList.
    pub fn scroll_position(&self) -> f64 {
        let Some(inner) = self.borrow() else {
            return 0.0;
        };
        inner.first_scroll
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

    /// Initiates a smooth scrolling animation to the specified target item in the list.
    pub fn smooth_scroll_to(
        &self,
        cx: &mut Cx,
        target_id: usize,
        speed: f64,
        max_items_to_show: Option<usize>,
    ) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        inner.smooth_scroll_to(cx, target_id, speed, max_items_to_show);
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
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let PortalListAction::SmoothScrollReached = item.cast() {
                return true;
            }
        }
        false
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
