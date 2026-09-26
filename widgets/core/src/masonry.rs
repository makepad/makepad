//! Masonry — columns of items that each keep their own height.
//!
//! A grid of cards whose contents are different lengths leaves a ragged
//! band of nothing along the bottom of every row, because a row can only be
//! as short as its tallest member. A masonry has no rows. Each item is as
//! tall as it asked to be, and the next item is put into whichever column is
//! shortest at that moment, so the columns stay level with one another and
//! the holes close up.
//!
//! # Strict order and tight packing cannot both be had
//!
//! This layout will move items past one another, and it has to. Filling the
//! shortest column is what closes the gaps, and the shortest column is
//! rarely the next one along: put a tall item first and the two items after
//! it both go beside it, so item three is drawn higher up the page than item
//! one finishes. Reading left to right, top to bottom, no longer recovers
//! the order they were written in.
//!
//! The rule here is one pass, in order: each item takes the shortest column
//! as things stand when it is reached, and nothing is moved afterwards to
//! even the columns up. That keeps the drift local — an item is never far
//! from where it was written — and it keeps the layout stable, since adding
//! an item at the end cannot rearrange the ones before it. A layout that
//! packed perfectly would have to sort by height, and then the order would
//! be gone altogether. If the order carries meaning, this is the wrong
//! container.
//!
//! # How many columns
//!
//! Either a number, or a floor. `columns: 3` is three columns whatever the
//! width. `columns: 0` derives the count from `min_column_width`: as many
//! columns as will hold that width, which is what makes the block reflow as
//! its container is resized. Whichever way the count is arrived at, the
//! columns then share the full width equally, so `min_column_width` is a
//! minimum for the count and not the width the columns end up with.
//!
//! # What it deliberately does not do
//!
//! It does not scroll — put it in a view that does. It does not virtualize:
//! every child is drawn, so a feed of thousands wants a list, not this. It
//! does not animate an item from one column to another when the count
//! changes; the block simply repacks. It does not measure a `Fill` height,
//! since there is no row to fill against — an item needs a definite or a
//! `Fit` height, and a `Fill` one collapses to nothing. And it does not
//! stretch the shortest column to meet the tallest: the columns end level
//! only to the extent the items allow.
use crate::{
    column_fit::{column_count, column_width, finite, item_walk, MAX_COLUMNS},
    makepad_derive_widget::*, makepad_draw::*, view::EventOrder, widget::*,
    widget_tree::CxWidgetExt,
};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.MasonryBase = set_type_default() do #(Masonry::register_widget(vm))

    /** Columns of items that each keep their own height. */
    mod.widgets.Masonry = set_type_default() do mod.widgets.MasonryBase{
        width: Fill
        height: Fit

        /** a fixed column count; 0 derives it from min_column_width 0..12 step 1 */
        columns: 0
        /** the narrowest a derived column may be, in pixels 40..600 step 10 */
        min_column_width: 180.
        /** pixels between one column and the next 0..64 step 1 */
        column_gap: theme.space_2
        /** pixels between one item and the next down a column 0..64 step 1 */
        row_gap: theme.space_2
    }
}

/// The column heights, and the rule that decides which column takes the
/// next item.
///
/// It is a plain struct rather than a method on the widget for the usual two
/// reasons: the packing can be tested without a draw pass, and the draw pass
/// and anything else that wants to know where an item went are reading the
/// same numbers from the same place.
#[derive(Clone, Debug, Default, PartialEq)]
struct Columns {
    /// How far down each column has been filled, including the gap that
    /// follows its last item.
    heights: Vec<f64>,
    row_gap: f64,
    placed: usize,
}

impl Columns {
    fn reset(&mut self, count: usize, row_gap: f64) {
        self.heights.clear();
        // Zero columns would be a block that can hold nothing, which is
        // never what was meant by it.
        self.heights.resize(count.clamp(1, MAX_COLUMNS), 0.0);
        self.row_gap = row_gap;
        self.placed = 0;
    }

    fn count(&self) -> usize {
        self.heights.len()
    }

    fn height(&self, column: usize) -> f64 {
        self.heights.get(column).copied().unwrap_or(0.0)
    }

    /// The column the next item belongs in.
    ///
    /// Ties go to the leftmost. That is what keeps a run of same-height
    /// items in the order they were written — with any other tie-break a
    /// block of identical cards fills in some order nobody asked for.
    fn shortest(&self) -> usize {
        let mut best = 0;
        for (index, height) in self.heights.iter().enumerate().skip(1) {
            // Strictly less, so an equal column further right never wins.
            if *height < self.heights[best] {
                best = index;
            }
        }
        best
    }

    /// Put an item of this height in a named column, and answer the y its
    /// top sits at.
    ///
    /// The column is named rather than chosen here because the draw pass has
    /// to know where an item is going before it draws it, and may be
    /// interrupted between the two.
    fn place_in(&mut self, column: usize, height: f64) -> f64 {
        let Some(filled) = self.heights.get_mut(column) else {
            return 0.0;
        };
        let top = *filled;
        *filled = top + height.max(0.0) + self.row_gap;
        self.placed += 1;
        top
    }

    /// How tall the whole block is: the tallest column, without the gap that
    /// trails its last item.
    fn extent(&self) -> f64 {
        if self.placed == 0 {
            return 0.0;
        }
        let tallest = self.heights.iter().fold(0.0f64, |tallest, height| tallest.max(*height));
        (tallest - self.row_gap).max(0.0)
    }
}

#[derive(Clone, Copy)]
enum DrawState {
    Drawing {
        child_index: usize,
        /// The column the open item is going into. It is carried in the
        /// state because a child may yield mid-draw, and the column has to
        /// survive the interruption or the item is credited to the wrong
        /// one when it resumes.
        column: usize,
        item_open: bool,
    },
}

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct Masonry {
    #[uid]
    uid: WidgetUid,
    #[source]
    pub source: ScriptObjectRef,
    #[live]
    pub draw_bg: DrawQuad,
    #[live(false)]
    pub show_bg: bool,
    #[layout]
    pub layout: Layout,
    #[walk]
    pub walk: Walk,

    /// A fixed column count. Zero derives it from `min_column_width`.
    #[live]
    pub columns: usize,
    /// The narrowest a derived column may be. It sets the count, not the
    /// width: the columns then share the whole container equally.
    #[live(180.0)]
    pub min_column_width: f64,
    #[live]
    pub column_gap: f64,
    #[live]
    pub row_gap: f64,

    #[live]
    event_order: EventOrder,
    #[live(true)]
    pub visible: bool,

    #[rust]
    area: Area,
    #[rust]
    draw_state: DrawStateWrap<DrawState>,
    #[rust]
    pub children: SmallVec<[(LiveId, WidgetRef); 2]>,
    #[rust]
    live_update_order: SmallVec<[LiveId; 1]>,

    #[rust]
    packing: Columns,
    #[rust]
    column_width: f64,
    #[rust]
    block_width: f64,
    #[rust]
    origin: Vec2d,
}

impl ScriptHook for Masonry {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if !apply.is_eval() {
            self.live_update_order.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if !apply.is_eval() {
            if let Some(object) = value.as_object() {
                let mut anonymous = 0usize;
                vm.vec_with(object, |vm, values| {
                    for value in values {
                        let id = if let Some(id) = value.key.as_id() {
                            Some(id)
                        } else if value.key.is_nil() {
                            let id = LiveId(anonymous as u64);
                            anonymous += 1;
                            Some(id)
                        } else {
                            None
                        };
                        let Some(id) = id else { continue };
                        if !WidgetRef::value_is_newable_widget(vm, value.value) {
                            continue;
                        }
                        if apply.is_reload() {
                            self.live_update_order.push(id);
                        }
                        if let Some((_, child)) =
                            self.children.iter_mut().find(|(child_id, _)| *child_id == id)
                        {
                            child.script_apply(vm, apply, scope, value.value);
                        } else {
                            self.children.push((
                                id,
                                WidgetRef::script_from_value_scoped(vm, scope, value.value),
                            ));
                        }
                    }
                });
            }
        }
        // A reload rewrites the order the items were written in, and the
        // written order is the only thing the packing has to go on.
        if apply.is_reload() && (!self.live_update_order.is_empty() || self.children.is_empty()) {
            for (index, id) in self.live_update_order.iter().enumerate() {
                if let Some(position) = self.children.iter().position(|(old, _)| old == id) {
                    self.children.swap(index, position);
                }
            }
            self.children.truncate(self.live_update_order.len());
        }
        vm.cx_mut().widget_tree_mark_dirty(self.uid);
    }
}

impl WidgetNode for Masonry {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn set_scroll_pos(&mut self, cx: &mut Cx, position: Vec2d) {
        self.layout.scroll = position;
        self.redraw(cx);
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        for (_, child) in &mut self.children {
            child.redraw(cx);
        }
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, child) in &self.children {
            visit(*id, child.clone());
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        for (_, child) in &self.children {
            child.find_widgets_from_point(cx, point, found);
        }
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            if visible && matches!(self.area, Area::Empty) {
                cx.redraw_all();
            } else {
                self.redraw(cx);
            }
        }
    }

    fn visible(&self) -> bool {
        self.visible
    }

    fn layer_areas(&self) -> Vec<(&'static str, Area)> {
        self.show_bg
            .then(|| vec![("draw_bg", self.draw_bg.area())])
            .unwrap_or_default()
    }
}

impl Widget for Masonry {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible && event.requires_visibility() {
            return;
        }
        match &self.event_order {
            EventOrder::Up => {
                for (_, child) in self.children.iter_mut().rev() {
                    child.handle_event(cx, event, scope);
                }
            }
            EventOrder::Down => {
                for (_, child) in &mut self.children {
                    child.handle_event(cx, event, scope);
                }
            }
            EventOrder::List(order) => {
                for id in order {
                    if let Some((_, child)) =
                        self.children.iter_mut().find(|(child_id, _)| child_id == id)
                    {
                        child.handle_event(cx, event, scope);
                    }
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(
            cx,
            DrawState::Drawing {
                child_index: 0,
                column: 0,
                item_open: false,
            },
        ) {
            if !self.visible {
                self.draw_state.end();
                return DrawStep::done();
            }
            // Every item is placed absolutely, so the container's own flow
            // must not also be moving a cursor along.
            let layout = Layout {
                flow: Flow::Overlay,
                align: Align::default(),
                distribute: Distribute::Start,
                spacing: 0.0,
                wrap_spacing: 0.0,
                ..self.layout
            };
            if self.show_bg {
                self.draw_bg.begin(cx, walk, layout);
            } else {
                cx.begin_turtle(walk, layout);
            }
            self.prepare_layout(cx);
        }

        while let Some(DrawState::Drawing {
            child_index,
            column,
            item_open,
        }) = self.draw_state.get()
        {
            if child_index >= self.children.len() {
                // The block's height is not known until the last item has
                // been measured, so the turtle is walked here rather than at
                // the start the way a grid can afford to.
                cx.walk_turtle(
                    Walk::fixed(self.block_width, self.packing.extent())
                        .with_abs_pos(self.origin),
                );
                if self.show_bg {
                    self.draw_bg.end(cx);
                    self.area = self.draw_bg.area();
                } else {
                    cx.end_turtle_with_area(&mut self.area);
                }
                self.draw_state.end();
                break;
            }
            if !self.children[child_index].1.visible() {
                // A hidden item leaves no hole: the columns never hear
                // about it.
                self.draw_state.set(DrawState::Drawing {
                    child_index: child_index + 1,
                    column: 0,
                    item_open: false,
                });
                continue;
            }

            let column = if item_open {
                column
            } else {
                let column = self.packing.shortest();
                let position = self.origin
                    + dvec2(
                        column as f64 * (self.column_width + finite(self.column_gap)),
                        self.packing.height(column),
                    );
                cx.begin_turtle(
                    Walk::new(Size::Fixed(self.column_width), Size::fit())
                        .with_abs_pos(position),
                    Layout {
                        flow: Flow::Down,
                        clip_x: false,
                        clip_y: false,
                        ..Default::default()
                    },
                );
                self.draw_state.set(DrawState::Drawing {
                    child_index,
                    column,
                    item_open: true,
                });
                column
            };

            let authored = self.children[child_index].1.walk(cx);
            let child_walk = item_walk(authored, self.column_width);
            self.children[child_index].1.draw_walk(cx, scope, child_walk)?;
            // How tall the item turned out is only knowable now, which is
            // why the column it went into had to be chosen before it.
            let rect = cx.end_turtle();
            self.packing.place_in(column, rect.size.y);
            self.draw_state.set(DrawState::Drawing {
                child_index: child_index + 1,
                column: 0,
                item_open: false,
            });
        }
        DrawStep::done()
    }
}

impl Masonry {
    fn prepare_layout(&mut self, cx: &mut Cx2d) {
        self.origin = cx.turtle().inner_origin();
        let inner = cx.turtle().inner_size();
        let width = inner.x.is_finite().then(|| inner.x.max(0.0));
        let column_gap = finite(self.column_gap);
        let count = column_count(width, self.columns, finite(self.min_column_width), column_gap);
        self.column_width =
            column_width(width, count, column_gap, finite(self.min_column_width));
        self.block_width =
            self.column_width * count as f64 + column_gap * count.saturating_sub(1) as f64;
        self.packing.reset(count, finite(self.row_gap));
    }

    /// How many columns the last draw settled on.
    pub fn column_count(&self) -> usize {
        self.packing.count()
    }
}

impl MasonryRef {
    /// How many columns the last draw settled on, for a host that wants to
    /// say so out loud. It is zero before the first draw, since until then
    /// there is no width to have derived it from.
    pub fn column_count(&self) -> usize {
        self.borrow().map(|inner| inner.column_count()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(count: usize, row_gap: f64) -> Columns {
        let mut columns = Columns::default();
        columns.reset(count, row_gap);
        columns
    }

    /// One item placed the way the draw pass places one: the column is
    /// chosen first, and the height is credited to it afterwards.
    fn place(columns: &mut Columns, height: f64) -> usize {
        let column = columns.shortest();
        columns.place_in(column, height);
        column
    }

    #[test]
    fn each_item_goes_into_the_column_that_is_shortest_when_it_is_reached() {
        let mut c = columns(3, 0.0);
        // Empty columns are taken left to right, so a short block still
        // reads in the order it was written.
        assert_eq!(place(&mut c, 100.0), 0);
        assert_eq!(place(&mut c, 40.0), 1);
        assert_eq!(place(&mut c, 60.0), 2);
        // 100 / 40 / 60 now: the next item goes on top of the 40.
        assert_eq!(place(&mut c, 30.0), 1);
        // 100 / 70 / 60: and the next on top of the 60.
        assert_eq!(place(&mut c, 10.0), 2);
        // 100 / 70 / 70: a tie, and the leftmost of the two takes it.
        assert_eq!(place(&mut c, 10.0), 1);
        assert_eq!(c.heights, vec![100.0, 80.0, 70.0]);
    }

    #[test]
    fn a_tall_item_is_passed_by_the_ones_written_after_it() {
        // This is the cost the module doc names, written down as a test so
        // nobody later "fixes" it into a layout that leaves holes.
        let mut c = columns(2, 0.0);
        assert_eq!(place(&mut c, 200.0), 0);
        assert_eq!(place(&mut c, 20.0), 1);
        assert_eq!(place(&mut c, 20.0), 1);
        assert_eq!(place(&mut c, 20.0), 1);
        // Items two, three and four are all stacked beside item one, and
        // the last of them finishes far above where item one ends.
        assert_eq!(c.heights, vec![200.0, 60.0]);
    }

    #[test]
    fn same_height_items_keep_the_order_they_were_written_in() {
        let mut c = columns(4, 8.0);
        let first: Vec<usize> = (0..4).map(|_| place(&mut c, 50.0)).collect();
        let second: Vec<usize> = (0..4).map(|_| place(&mut c, 50.0)).collect();
        assert_eq!(first, vec![0, 1, 2, 3]);
        assert_eq!(second, vec![0, 1, 2, 3]);
    }

    #[test]
    fn the_row_gap_falls_between_items_and_never_after_the_last_one() {
        let mut c = columns(2, 10.0);
        place(&mut c, 30.0);
        place(&mut c, 30.0);
        assert_eq!(c.extent(), 30.0, "one item in each column and no trailing gap");
        place(&mut c, 20.0);
        assert_eq!(c.extent(), 60.0, "30, the gap, then 20");
    }

    #[test]
    fn an_empty_set_is_a_block_of_no_height() {
        let empty = columns(3, 12.0);
        assert_eq!(empty.count(), 3);
        assert_eq!(empty.extent(), 0.0);
        assert_eq!(empty.shortest(), 0);
        assert_eq!(empty.height(0), 0.0);
        // Asking for a column past the end answers zero rather than
        // panicking: the count can change under a draw that is resuming.
        assert_eq!(empty.height(99), 0.0);
        // And a block told it has no columns still has one to put things in.
        let none = columns(0, 12.0);
        assert_eq!(none.count(), 1);
        assert_eq!(none.extent(), 0.0);
    }
}
