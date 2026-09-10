//! TileList — a virtualised set of items laid out N across.
//!
//! A picker of a thousand things wants to be a grid and wants to cost a
//! screenful, and in this library those two wants pull apart. The containers
//! that lay items out across a width draw every child they hold, so a
//! thousand of them is a thousand draws on every frame; the containers that
//! draw only what is on screen are one item wide. This is the join between
//! them: the rows are a portal list, so only the rows on screen exist, and
//! each row is N tiles across.
//!
//! # How many across
//!
//! `columns: 0` fits as many tiles of at least `min_tile_width` as the width
//! allows, and is the setting that makes the block reflow when its container
//! is resized. `columns: 1` is a plain row list — the same widget, drawn one
//! across. Any other number is that many at any width. The count comes from
//! the same arithmetic the masonry uses, so a page that puts the two side by
//! side gets the same answer from both about what a width holds; and as
//! there, the least width sets the COUNT and not the width the tiles get:
//! once the count is settled the tiles share the whole row equally.
//!
//! # What it reports
//!
//! [`TileList::next_tile`] hands back the flat item index together with the
//! row and column it landed in. That is deliberate: the widget is the only
//! party that knows the column count it settled on this frame, so a host
//! that had to work out `row * columns + slot` for itself would be deriving
//! it from a number it does not have. Ask the tile where it is.
//!
//! # The host fills, the row draws
//!
//! The loop is `next_tile` until it answers `None`, and each answer is a
//! widget to write content into. Do not draw it: the tiles of a row are
//! drawn together, in the row's own turtle, once the last slot of that row
//! has been handed out. A host that stops the loop early still gets every
//! row drawn, but the tiles it did not fill are not blank: rows are
//! recycled, so an unfilled tile is still showing whatever the last item to
//! stand in it wrote. Run the loop to `None`.
//!
//! # What it deliberately does not do
//!
//! It does not keep per-item state. Rows are recycled as they scroll and the
//! tiles inside them are recycled with the row, so a tile that was told it
//! was selected stays selected under the next index that lands in it. Set
//! every visual property of a tile from its index, every draw; the fact that
//! it looked right last time is not evidence.
//!
//! It does not size tiles to each other's height. A row is as tall as its
//! tallest tile, and rows are free to differ — which is the difference
//! between this and a table. Tiles need a definite or a `Fit` height: a
//! `Fill` one has no row to fill against and collapses to nothing.
//!
//! It does not scroll sideways, and it is `Fill` by nature: put it in
//! something with a real height, because `Fill` inside a `Fit` page resolves
//! to nothing at all and a list laid out with no height is drawn with none.
use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    masonry::{column_count, column_width, finite, item_walk},
    portal_list::PortalList,
    widget::*,
    widget_async::CxSplashVmExt,
    widget_tree::CxWidgetExt,
};
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.TileRowBase = #(TileRow::register_widget(vm))

    /** One row of a tile list's tiles. The list makes these; nothing else should. */
    mod.widgets.TileRow = set_type_default() do mod.widgets.TileRowBase{
        width: Fill
        height: Fit
    }

    mod.widgets.TileListBase = #(TileList::register_widget(vm))

    /** A virtualised set of items laid out N across. */
    mod.widgets.TileList = set_type_default() do mod.widgets.TileListBase{
        width: Fill
        height: Fill

        /** how many tiles across; 0 fits as many as the width allows 0..12 step 1 */
        columns: 0
        /** the narrowest a derived tile may be, in pixels 40..600 step 10 */
        min_tile_width: 160.
        /** pixels between one tile and the next across 0..64 step 1 */
        column_gap: theme.space_2
        /** pixels between one row and the next down 0..64 step 1 */
        row_gap: theme.space_2

        // The rows scroll in a portal list, and that is the whole of the
        // virtualisation. It is a slot (`list:`) rather than a child
        // (`list :=`) so that a `Tile := …` written on a TileList is the
        // one named thing on the instance, and is therefore unambiguously
        // the tile template rather than a widget somebody would see.
        list: mod.widgets.PortalList{
            width: Fill
            height: Fill
            flow: Down
            Row := mod.widgets.TileRow{}
        }
    }
}

/// Where one item of the set ended up, and the widget to put it in.
///
/// `index` is the item the host asked for; `row` and `column` are where this
/// draw decided to put it. Keeping all three together is the point of the
/// type: the column count is the widget's answer to the width, and a host
/// that recomputed the index from a count it guessed would be right until
/// the first resize.
pub struct TilePlacement {
    /// The flat item index, counting across the rows.
    pub index: usize,
    pub row: usize,
    pub column: usize,
    /// How many across this draw settled on.
    pub columns: usize,
    /// The tile to fill. Filling it is the host's job; drawing it is not.
    ///
    /// A real widget: a slot is handed out only once the row has a tile
    /// standing in it, so a list with no `Tile := …` template on it hands
    /// out nothing at all and says so in the log, rather than handing out
    /// placements with nothing in them for a host to write into and not see.
    pub widget: WidgetRef,
}

/// The index arithmetic: how many across, how many there are, and therefore
/// which item sits in which slot of which row.
///
/// A plain struct rather than a handful of methods on the widget, for the
/// reason the masonry gives for the same choice: it can be tested without a
/// draw pass, and the draw pass and everything that asks where an item went
/// read the same numbers from the same place.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TileGrid {
    /// Never zero. A set laid out none across is not a set laid out.
    columns: usize,
    count: usize,
}

impl Default for TileGrid {
    /// One across and nothing in it, which is the state before the first
    /// draw: no width to divide and no count yet.
    ///
    /// Written out rather than derived so that the invariant above holds of
    /// every grid there is. A derived default is none across, and the
    /// divisions below would then be by zero the moment a count arrived
    /// before a width did — which is exactly the order a host restoring a
    /// saved position uses.
    fn default() -> Self {
        Self::new(1, 0)
    }
}

impl TileGrid {
    fn new(columns: usize, count: usize) -> Self {
        Self {
            columns: columns.max(1),
            count,
        }
    }

    /// How many rows the count needs.
    ///
    /// Written as `(count - 1) / columns + 1` rather than the usual
    /// `(count + columns - 1) / columns`, because the usual form wraps for a
    /// count near the top of the range and this one cannot.
    fn rows(&self) -> usize {
        if self.count == 0 {
            0
        } else {
            (self.count - 1) / self.columns + 1
        }
    }

    /// How many tiles a given row actually holds: the full count, except in
    /// the last row, which holds the remainder.
    fn slots_in_row(&self, row: usize) -> usize {
        let rows = self.rows();
        // Asked first, and not as `row + 1 > rows`: a row index at the top
        // of the range is exactly what a list whose count has just shrunk
        // hands over, and adding one to it panics.
        if row >= rows {
            return 0;
        }
        if row + 1 < rows {
            self.columns
        } else {
            // The last row: whatever is left after the full ones.
            self.count - (rows - 1) * self.columns
        }
    }

    /// The item in a slot, or `None` when the slot is past the end of the
    /// set — which the last row of nearly every set has.
    fn index(&self, row: usize, column: usize) -> Option<usize> {
        if column >= self.columns {
            return None;
        }
        let index = row.checked_mul(self.columns)?.checked_add(column)?;
        (index < self.count).then_some(index)
    }

    /// The row an item is in.
    fn row_of(&self, index: usize) -> Option<usize> {
        (index < self.count).then(|| index / self.columns)
    }

    /// The slot an item is in, across its row.
    fn column_of(&self, index: usize) -> Option<usize> {
        (index < self.count).then(|| index % self.columns)
    }
}

/// The row to scroll to so that the item at the top of the viewport is still
/// the item at the top of the viewport after the column count changes.
///
/// Without this a resize teleports the reader: the list scrolls by rows, and
/// when three across becomes four, row 40 stops meaning item 120 and starts
/// meaning item 160. The saturation is not paranoia — the multiplication is
/// a row index times a column count, and a list told it holds `usize::MAX`
/// items has a row index that will not multiply.
fn retarget_row(first_row: usize, was: usize, now: usize) -> usize {
    if was == 0 || now == 0 || was == now {
        return first_row;
    }
    first_row.saturating_mul(was) / now
}

/// Which row belongs at the top of the viewport this draw, or `None` when
/// there is no row to put there.
///
/// `asked` is a [`TileList::scroll_to_item`] made since the last draw, and it
/// outranks the correction above: a reader who asked to be somewhere is not
/// being kept where they were. It arrives as an item rather than a row
/// because the row it is in depends on how many go across, and the widget
/// does not know that until it has a width — this is the first moment it
/// does.
///
/// Everything is then pinned inside the set. That clamp is not tidiness: a
/// first row past the end draws NOTHING, and nothing in the portal list's
/// pass takes it back. It asks for that row, the row is out of range and
/// draws nothing, it walks up to the one before, that is out of range too,
/// and the pass ends having drawn nothing at all — on that frame and on
/// every frame after it, because the bad row is still the first row.
fn first_row(
    was_first: usize,
    was_columns: usize,
    grid: TileGrid,
    asked: Option<usize>,
) -> Option<usize> {
    let rows = grid.rows();
    if rows == 0 {
        return None;
    }
    let row = match asked {
        Some(index) => index / grid.columns,
        None => retarget_row(was_first, was_columns, grid.columns),
    };
    Some(row.min(rows - 1))
}

/// The grid a question about an item is answered with: how many across the
/// last draw settled on, at the count the HOST has given.
///
/// The two halves come from different places on purpose. The column count is
/// the draw's answer to a width and nobody outside the draw can know it; the
/// item count is the host's and is true the moment it is set. Answering from
/// the draw's grid alone is what makes `item_count()` and `place_of()`
/// disagree — the host says a thousand, the grid still says none, and every
/// place is `None` until a frame has gone by.
fn answer_grid(draw: TileGrid, count: usize) -> TileGrid {
    TileGrid::new(draw.columns, count)
}

/// The gap a row carries above it.
///
/// Above rather than below, and none at all above the first, so that the gap
/// falls BETWEEN rows: carried below, the last row ends the scroll extent
/// with a band of dead space that the reader can scroll to and nothing is
/// in. The sibling this widget borrows its arithmetic from drops its
/// trailing gap for the same reason.
fn gap_above(row: usize, row_gap: f64) -> f64 {
    if row == 0 {
        0.0
    } else {
        finite(row_gap)
    }
}

/// The row being handed out, tile by tile.
struct RowCursor {
    row: usize,
    /// The item in this row's first slot.
    base: usize,
    /// How many slots this row has; the last row has fewer.
    slots: usize,
    /// The next slot to hand to the host.
    slot: usize,
    widget: WidgetRef,
}

#[derive(Script, Widget)]
pub struct TileList {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,

    /// The portal list the rows scroll in. Redrawing and hit testing go
    /// through it because everything this widget draws is inside it.
    #[redraw]
    #[find]
    #[live]
    list: WidgetRef,

    /// How many tiles across. Zero derives the count from `min_tile_width`.
    #[live]
    pub columns: usize,
    /// The narrowest a derived tile may be. It sets the count, not the
    /// width: the tiles then share the row equally.
    #[live(160.0)]
    pub min_tile_width: f64,
    #[live]
    pub column_gap: f64,
    #[live]
    pub row_gap: f64,
    #[live(true)]
    #[visible]
    visible: bool,

    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    draw_state: DrawStateWrap<()>,
    /// How many items the host says there are.
    #[rust]
    count: usize,
    #[rust]
    grid: TileGrid,
    #[rust]
    tile_width: f64,
    /// The column count the previous draw used, so a change in it can be
    /// turned into a scroll correction before the rows are asked for.
    #[rust]
    was_columns: usize,
    /// An item the host has asked to be shown, waiting for a draw to turn it
    /// into a row. Held rather than acted on because the row an item is in
    /// depends on the column count, and the column count comes from a width
    /// that a host restoring a saved position has not given the widget yet.
    #[rust]
    pending_scroll: Option<usize>,
    #[rust]
    cursor: Option<RowCursor>,
    /// Whether this draw has given the list its row range yet. The host may
    /// set the item count after the draw has begun, so the range cannot be
    /// worked out until the first tile is asked for.
    #[rust]
    ranged: bool,
    /// Whether a draw is open. `next_tile` outside one would drive the
    /// portal list's draw state with no turtle under it.
    #[rust]
    drawing: bool,
    /// A missing tile template is worth saying once, not sixty times a
    /// second.
    #[rust]
    warned: bool,
}

impl ScriptHook for TileList {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.templates.clear();
            // Said once per version of the instance, not once per widget:
            // a reload is where a missing `Tile` template gets added, and a
            // reload that still has none is worth hearing about again.
            self.warned = false;
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        // The named entries of the instance are templates, the way a list's
        // item templates are. `Tile` is the only one read.
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        if let Some(id) = kv.key.as_id() {
                            if let Some(template_obj) = kv.value.as_object() {
                                self.templates
                                    .insert(id, vm.bx.heap.new_object_ref(template_obj));
                            }
                        }
                    }
                });
            }
        }
        // The tiles were built from a template that may have just changed.
        // They are rebuilt rather than patched, the way a list's items are:
        // the rows belong to the portal list, so they are reached through
        // it and emptied, and the next draw fills them again.
        if apply.is_reload() {
            self.cursor = None;
            if let Some(list) = self.list.borrow::<PortalList>() {
                for (_, item) in list.items().iter() {
                    if let Some(mut row) = item.widget.borrow_mut::<TileRow>() {
                        row.discard_tiles();
                    }
                }
            }
        }
    }
}

impl Widget for TileList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible && event.requires_visibility() {
            return;
        }
        self.list.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ()) {
            if !self.visible {
                self.draw_state.end();
                return DrawStep::done();
            }
            // Without this the design overlay cannot reach the list, and
            // through it the rows: nothing else puts the slot in the tree.
            cx.widget_tree_insert_child(self.uid, live_id!(list), self.list.clone());
            if self.list.draw_walk(cx, scope, walk).is_step() {
                // The list has opened its turtle, so there is finally a
                // width to divide.
                self.measure(cx);
                self.cursor = None;
                self.ranged = false;
                self.drawing = true;
                // Hand ourselves to the host, which fills tiles until
                // `next_tile` runs out.
                return DrawStep::make_step();
            }
            self.draw_state.end();
            return DrawStep::done();
        }
        // The host is done with us. Whatever it filled or left, every row
        // the list asked for still has to be drawn, or the turtle the list
        // opened for the last one is never closed.
        self.drain(cx);
        self.drawing = false;
        let _ = self.list.draw_walk(cx, scope, walk);
        self.draw_state.end();
        DrawStep::done()
    }
}

impl TileList {
    /// How wide a tile is and how many go across, from the width the list's
    /// turtle actually got.
    fn measure(&mut self, cx: &mut Cx2d) {
        let inner = cx.turtle().inner_rect().size.x;
        // A tile list inside a `Fit` parent has no width to answer with;
        // there is nothing to divide then, so it is one across.
        let width = inner.is_finite().then(|| inner.max(0.0));
        let gap = finite(self.column_gap);
        let least = finite(self.min_tile_width);
        let columns = column_count(width, self.columns, least, gap);
        self.tile_width = column_width(width, columns, gap, least);
        self.grid = TileGrid::new(columns, self.count);
    }

    /// Give the list its row range, and settle which row is at the top of it.
    fn begin_rows(&mut self, cx: &mut Cx, list: &mut PortalList) {
        if self.ranged {
            return;
        }
        self.ranged = true;
        // The host may have changed the count since the measurement, so the
        // grid is rebuilt on the count as it stands now.
        self.grid = TileGrid::new(self.grid.columns, self.count);
        let rows = self.grid.rows();
        list.set_item_range(cx, 0, rows);
        let was_first = list.first_id();
        let asked = self.pending_scroll.is_some();
        if let Some(row) = first_row(was_first, self.was_columns, self.grid, self.pending_scroll) {
            // The ask is spent only now that it has been answered: a set
            // with nothing in it yet has no row to show, and dropping the
            // ask there would lose a position restored before the count
            // arrived.
            self.pending_scroll = None;
            // Only when it moves, or when it was asked for. The call pins
            // the scroll to the top of the row, so making it every draw
            // would stop the list scrolling at all — it would snap back to
            // a row edge on every frame. An ask is different: a reader
            // wheeled part-way down the top row still expects
            // `scroll_to_item` to put that row's edge back at the top.
            if row != was_first || asked {
                list.set_first_id_and_scroll(row, 0.0);
            }
        }
        self.was_columns = self.grid.columns;
    }

    /// The next item to fill, or `None` when the visible rows are done.
    ///
    /// Every answer carries where the item landed; see [`TilePlacement`].
    /// Fill the widget and ask again — the row draws itself once its last
    /// slot has been handed out, so nothing here should be drawn by the
    /// caller.
    pub fn next_tile(&mut self, cx: &mut Cx2d) -> Option<TilePlacement> {
        if !self.drawing {
            return None;
        }
        // A clone of the handle, not a borrow of the field: the row draws
        // below happen while the list is borrowed, and a borrow rooted in
        // `self` would keep the widget itself locked as well.
        let list_ref = self.list.clone();
        let mut list = list_ref.borrow_mut::<PortalList>()?;
        self.begin_rows(cx, &mut list);
        let columns = self.grid.columns;
        loop {
            if let Some(cursor) = &mut self.cursor {
                if cursor.slot < cursor.slots {
                    let column = cursor.slot;
                    cursor.slot += 1;
                    let widget = cursor
                        .widget
                        .borrow::<TileRow>()
                        .and_then(|row| row.tiles.get(column).cloned())
                        .unwrap_or_else(WidgetRef::empty);
                    return Some(TilePlacement {
                        index: cursor.base + column,
                        row: cursor.row,
                        column,
                        columns,
                        widget,
                    });
                }
            }
            // That row is full: draw it, then go looking for the next one.
            if let Some(cursor) = self.cursor.take() {
                cursor.widget.draw_all(cx, &mut Scope::empty());
            }
            let row = list.next_visible_item(cx)?;
            // The first slot of the row holding an item is the same
            // question as the row being in range at all, so one answer
            // does for both. A row past the end draws nothing, which is
            // how the list is told it has reached the bottom.
            let Some(base) = self.grid.index(row, 0) else {
                continue;
            };
            let widget = list.item(cx, row, live_id!(Row));
            // What the row can actually back, not what the arithmetic asked
            // for: a slot with no tile behind it would be handed to the host
            // as a placement it can fill and never see.
            let slots = self.fill_row(cx, &widget, row, base, self.grid.slots_in_row(row));
            self.cursor = Some(RowCursor {
                row,
                base,
                slots,
                slot: 0,
                widget,
            });
        }
    }

    /// Make sure a row holds `slots` tiles, and tell it the measurements the
    /// list has settled on.
    ///
    /// Answers how many tiles the row is showing, which is `slots` unless
    /// there was no tile to build — no `Tile := …` on the instance — and
    /// then it is fewer, or none. The caller hands out that number and not
    /// the number it asked for.
    fn fill_row(
        &mut self,
        cx: &mut Cx2d,
        widget: &WidgetRef,
        row: usize,
        base: usize,
        slots: usize,
    ) -> usize {
        let have = widget
            .borrow::<TileRow>()
            .map(|inner| inner.tiles.len())
            .unwrap_or(0);
        let mut fresh = Vec::new();
        for _ in have..slots {
            let Some(tile) = self.new_tile(cx) else {
                break;
            };
            fresh.push(tile);
        }
        let Some(mut inner) = widget.borrow_mut::<TileRow>() else {
            return 0;
        };
        inner.adopt(
            cx,
            base,
            slots,
            fresh,
            self.tile_width,
            finite(self.column_gap),
            gap_above(row, self.row_gap),
        );
        inner.used
    }

    /// One tile, built from the instance's `Tile` template.
    fn new_tile(&mut self, cx: &mut Cx) -> Option<WidgetRef> {
        if !self.templates.contains_key(&live_id!(Tile)) {
            if !self.warned {
                self.warned = true;
                warning!("TileList has no Tile template: add `Tile := ...` to the instance");
            }
            return None;
        }
        let template = self.templates.get(&live_id!(Tile))?;
        let value: ScriptValue = template.as_object().into();
        // Built in the VM whose heap minted the template, not in the main
        // one: a list living in an isolate would otherwise dereference an
        // isolate object against the main heap.
        let vm_id = cx.script_ref_vm_id(template)?;
        Some(cx.with_script_vm_id(vm_id, |vm| WidgetRef::script_from_value(vm, value)))
    }

    /// Draw out whatever rows are left, filled or not.
    fn drain(&mut self, cx: &mut Cx2d) {
        while self.next_tile(cx).is_some() {}
    }

    /// Tell the list how many items there are. Safe to call during a draw:
    /// the row range is not taken until the first tile is asked for.
    pub fn set_item_count(&mut self, cx: &mut Cx, count: usize) {
        if self.count != count {
            self.count = count;
            self.redraw(cx);
        }
    }

    pub fn item_count(&self) -> usize {
        self.count
    }

    /// How many tiles across the last draw settled on. Zero before the first
    /// draw, since until then there is no width to have derived it from.
    pub fn column_count(&self) -> usize {
        self.was_columns
    }

    /// How many rows the set needs at that count, across the column count
    /// the last draw settled on. Before the first draw there is no width
    /// and so no column count, and the answer is on the one-across
    /// fallback — the same caveat as [`Self::place_of`], and
    /// [`Self::column_count`] answering zero is the tell.
    pub fn row_count(&self) -> usize {
        answer_grid(self.grid, self.count).rows()
    }

    /// Where an item sits, as (row, column) — the inverse of what
    /// [`TilePlacement`] reports.
    ///
    /// Answered at the count the host has given and the column count the
    /// last draw settled on. Before the first draw there is no width and so
    /// no column count, and the answer is the one-across fallback the rest
    /// of the widget uses for a width it does not have; [`Self::column_count`]
    /// answers zero until then, which is how a host can tell.
    pub fn place_of(&self, index: usize) -> Option<(usize, usize)> {
        let grid = answer_grid(self.grid, self.count);
        Some((grid.row_of(index)?, grid.column_of(index)?))
    }

    /// The item in a slot, if the set reaches that far. Answered like
    /// [`Self::place_of`].
    pub fn index_at(&self, row: usize, column: usize) -> Option<usize> {
        answer_grid(self.grid, self.count).index(row, column)
    }

    /// The first item of the row currently at the top of the viewport.
    ///
    /// Answered at the count the host has given, like [`Self::place_of`]:
    /// the list's own top row is corrected against the count only on the
    /// next draw, so between a `set_item_count` that shrank the set and
    /// that draw it can stand past the end. This answers the first item of
    /// the last row instead, which is where that draw will put it.
    pub fn first_visible_item(&self) -> usize {
        let first_row = self
            .list
            .borrow::<PortalList>()
            .map(|list| list.first_id())
            .unwrap_or(0);
        let grid = answer_grid(self.grid, self.count);
        let rows = grid.rows();
        if rows == 0 {
            return 0;
        }
        grid.index(first_row.min(rows - 1), 0).unwrap_or(0)
    }

    /// Put the row an item is in at the top of the viewport.
    ///
    /// Which row that is, is not decided here. It depends on how many go
    /// across, the column count comes from a width, and a host restoring a
    /// saved position — `set_item_count` then `scroll_to_item`, before
    /// anything has been drawn — has given the widget no width to have
    /// derived one from. So the item is remembered and turned into a row on
    /// the next draw, which is the first moment both numbers exist.
    pub fn scroll_to_item(&mut self, cx: &mut Cx, index: usize) {
        self.pending_scroll = Some(index);
        self.redraw(cx);
    }
}

impl TileListRef {
    pub fn set_item_count(&self, cx: &mut Cx, count: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_item_count(cx, count);
        }
    }

    pub fn column_count(&self) -> usize {
        self.borrow().map(|inner| inner.column_count()).unwrap_or(0)
    }

    /// See [`TileList::row_count`]: on the one-across fallback until the
    /// first draw has settled a column count.
    pub fn row_count(&self) -> usize {
        self.borrow().map(|inner| inner.row_count()).unwrap_or(0)
    }

    pub fn first_visible_item(&self) -> usize {
        self.borrow()
            .map(|inner| inner.first_visible_item())
            .unwrap_or(0)
    }

    /// Where an item sits, as (row, column), at the column count the last
    /// draw settled on and the count the host has given. `None` once the set
    /// no longer reaches that far.
    pub fn place_of(&self, index: usize) -> Option<(usize, usize)> {
        self.borrow().and_then(|inner| inner.place_of(index))
    }

    /// The item in a slot, if the set reaches that far.
    pub fn index_at(&self, row: usize, column: usize) -> Option<usize> {
        self.borrow().and_then(|inner| inner.index_at(row, column))
    }

    pub fn scroll_to_item(&self, cx: &mut Cx, index: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.scroll_to_item(cx, index);
        }
    }

    /// The tiles that raised an action this pass, each with the item index
    /// it was showing.
    ///
    /// This is the hit test. A tile's own action carries no item number —
    /// it is a button, and buttons do not know what they are for — so the
    /// row it belongs to is asked which slot it was, and the slot plus the
    /// row is the item.
    pub fn tiles_with_actions(&self, actions: &Actions) -> Vec<(usize, WidgetRef)> {
        let mut found = Vec::new();
        let Some(inner) = self.borrow() else {
            return found;
        };
        let Some(list) = inner.list.borrow::<PortalList>() else {
            return found;
        };
        for action in actions {
            let Some(action) = action.downcast_ref::<WidgetAction>() else {
                continue;
            };
            // The innermost group wins, and the row groups its tiles before
            // the portal list groups its rows, so this is the tile's own.
            let Some(group) = &action.group else {
                continue;
            };
            for (_, item) in list.items().iter() {
                let Some(row) = item.widget.borrow::<TileRow>() else {
                    continue;
                };
                if row.uid != group.group_uid {
                    continue;
                }
                if let Some(hit) = row.tile_of(group.item_uid) {
                    found.push(hit);
                }
            }
        }
        found
    }
}

/// One row of a tile list.
///
/// It exists so that the portal list has something to virtualise: a row is
/// an item, the item is recycled, and the tiles ride along inside it. It is
/// not a general container — the list sets its measurements and its
/// contents every draw, and anything written on it in the DSL is written
/// over.
#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister)]
pub struct TileRow {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,

    #[rust]
    area: Area,
    #[rust]
    draw_state: DrawStateWrap<usize>,
    /// The tiles, kept across draws and refilled by the host. There may be
    /// more of them than the row is using: a row that lost a column when the
    /// list reflowed keeps the spare rather than throwing away a widget it
    /// is about to want back.
    #[rust]
    tiles: Vec<WidgetRef>,
    /// How many of them this row is showing.
    #[rust]
    used: usize,
    /// The item in the first slot.
    #[rust]
    base: usize,
    #[rust]
    tile_width: f64,
    #[rust]
    column_gap: f64,
    /// The gap between this row and the one above it, which the row carries
    /// as top padding. See [`gap_above`].
    #[rust]
    gap_above: f64,
}

impl WidgetNode for TileRow {
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
        for tile in &mut self.tiles {
            tile.redraw(cx);
        }
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (slot, tile) in self.tiles.iter().enumerate().take(self.used) {
            visit(LiveId(slot as u64), tile.clone());
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        for tile in self.tiles.iter().take(self.used) {
            tile.find_widgets_from_point(cx, point, found);
        }
    }
}

impl Widget for TileRow {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.uid;
        for tile in self.tiles.iter().take(self.used) {
            let tile_uid = tile.widget_uid();
            // Grouped so the list can answer "which item was that": the
            // group survives the portal list's own grouping of rows, which
            // only fills in a group where there is none.
            cx.group_widget_actions(uid, tile_uid, |cx| tile.handle_event(cx, event, scope));
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, 0usize) {
            cx.begin_turtle(
                walk,
                Layout {
                    // Across, and never wrapping: the row count is the
                    // list's arithmetic, and a turtle that wrapped a tile
                    // onto a second line would be answering it a second
                    // time, differently.
                    flow: Flow::right(),
                    spacing: self.column_gap,
                    // The gap between rows is carried by the row rather than
                    // by the list, because the list stacks its items edge to
                    // edge and has no spacing of its own to give. Above the
                    // row and not below it, so that the last row does not
                    // end the scroll extent with a band of nothing.
                    padding: Inset {
                        top: self.gap_above,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
        }
        while let Some(slot) = self.draw_state.get() {
            if slot >= self.used || slot >= self.tiles.len() {
                cx.end_turtle_with_area(&mut self.area);
                self.draw_state.end();
                break;
            }
            let tile = self.tiles[slot].clone();
            let authored = tile.walk(cx);
            // The slot is advanced only once the tile is through. A tile
            // that yields mid-draw comes back to this same slot and picks up
            // its own draw where it left off; advancing first would skip it.
            tile.draw_walk(cx, scope, item_walk(authored, self.tile_width))?;
            self.draw_state.set(slot + 1);
        }
        DrawStep::done()
    }
}

impl TileRow {
    /// Take the measurements and any newly built tiles from the list.
    fn adopt(
        &mut self,
        cx: &mut Cx,
        base: usize,
        slots: usize,
        fresh: Vec<WidgetRef>,
        tile_width: f64,
        column_gap: f64,
        gap_above: f64,
    ) {
        self.base = base;
        self.tile_width = tile_width;
        self.column_gap = column_gap;
        self.gap_above = gap_above;
        self.tiles.extend(fresh);
        self.used = slots.min(self.tiles.len());
        for (slot, tile) in self.tiles.iter().enumerate().take(self.used) {
            cx.widget_tree_insert_child(self.uid, LiveId(slot as u64), tile.clone());
        }
    }

    /// Throw the tiles away, so the list builds them again from whatever
    /// the template says now.
    fn discard_tiles(&mut self) {
        self.tiles.clear();
        self.used = 0;
    }

    /// Which item a tile of this row was showing.
    fn tile_of(&self, uid: WidgetUid) -> Option<(usize, WidgetRef)> {
        self.tiles
            .iter()
            .take(self.used)
            .position(|tile| tile.widget_uid() == uid)
            .map(|slot| (self.base + slot, self.tiles[slot].clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_row_holds_the_column_count_and_the_last_row_holds_the_remainder() {
        let grid = TileGrid::new(4, 10);
        assert_eq!(grid.rows(), 3);
        assert_eq!(grid.slots_in_row(0), 4);
        assert_eq!(grid.slots_in_row(1), 4);
        assert_eq!(grid.slots_in_row(2), 2, "ten items leave two in the third row");
        // A row past the end has no slots at all, which is what stops the
        // draw rather than drawing an empty band.
        assert_eq!(grid.slots_in_row(3), 0);
        assert_eq!(grid.slots_in_row(usize::MAX), 0);
        // A set that divides exactly has no short row.
        let exact = TileGrid::new(4, 8);
        assert_eq!(exact.rows(), 2);
        assert_eq!(exact.slots_in_row(1), 4);
    }

    #[test]
    fn the_item_in_a_slot_is_its_row_times_the_column_count_plus_the_slot() {
        let grid = TileGrid::new(3, 10);
        assert_eq!(grid.index(0, 0), Some(0));
        assert_eq!(grid.index(0, 2), Some(2));
        assert_eq!(grid.index(1, 0), Some(3));
        assert_eq!(grid.index(3, 0), Some(9));
        // And back again, for a host holding an index and asking where it
        // is on screen.
        assert_eq!(grid.row_of(9), Some(3));
        assert_eq!(grid.column_of(9), Some(0));
        assert_eq!(grid.row_of(5), Some(1));
        assert_eq!(grid.column_of(5), Some(2));
    }

    #[test]
    fn a_slot_past_the_end_of_the_set_holds_no_item() {
        let grid = TileGrid::new(3, 10);
        // The last row is short: two of its three slots are nothing.
        assert_eq!(grid.index(3, 0), Some(9));
        assert_eq!(grid.index(3, 1), None);
        assert_eq!(grid.index(3, 2), None);
        // A slot outside the row entirely, and a row outside the set.
        assert_eq!(grid.index(0, 3), None);
        assert_eq!(grid.index(9, 0), None);
        assert_eq!(grid.row_of(10), None);
        assert_eq!(grid.column_of(10), None);
    }

    #[test]
    fn one_across_is_a_list_and_every_row_holds_exactly_one_item() {
        let grid = TileGrid::new(1, 5);
        assert_eq!(grid.rows(), 5);
        for row in 0..5 {
            assert_eq!(grid.slots_in_row(row), 1);
            assert_eq!(grid.index(row, 0), Some(row));
            assert_eq!(grid.row_of(row), Some(row));
            assert_eq!(grid.column_of(row), Some(0));
        }
        // Zero across is one across: a set laid out none across is not a set
        // laid out.
        assert_eq!(TileGrid::new(0, 5).columns, 1);
    }

    #[test]
    fn a_set_of_nothing_has_no_rows_and_no_places() {
        let grid = TileGrid::new(4, 0);
        assert_eq!(grid.rows(), 0);
        assert_eq!(grid.slots_in_row(0), 0);
        assert_eq!(grid.index(0, 0), None);
        assert_eq!(grid.row_of(0), None);
    }

    #[test]
    fn a_set_too_large_to_multiply_out_does_not_wrap() {
        // The row count is the division, so it survives a count at the top
        // of the range; the index into that row is what cannot be computed,
        // and it answers None rather than a wrapped number.
        let grid = TileGrid::new(4, usize::MAX);
        assert_eq!(grid.rows(), usize::MAX / 4 + 1);
        assert_eq!(grid.index(usize::MAX / 2, 0), None);
        assert_eq!(grid.index(usize::MAX, 3), None);
        assert_eq!(grid.slots_in_row(usize::MAX), 0);
    }

    #[test]
    fn the_column_count_comes_from_the_width_when_no_number_is_given() {
        // The masonry's arithmetic, called the way this widget calls it:
        // four tiles of 160 with 8 between them need 664.
        assert_eq!(column_count(Some(664.0), 0, 160.0, 8.0), 4);
        assert_eq!(column_count(Some(663.0), 0, 160.0, 8.0), 3);
        // A number that was asked for wins at any width, and one across is
        // an ordinary row list.
        assert_eq!(column_count(Some(664.0), 1, 160.0, 8.0), 1);
        assert_eq!(column_count(Some(200.0), 4, 160.0, 8.0), 4);
        // Nothing to measure against — a tile list inside a Fit parent —
        // is one across, not none.
        assert_eq!(column_count(None, 0, 160.0, 8.0), 1);
        // And the tiles then share the row equally, so the least width is a
        // floor on the COUNT and not on the width they end up with.
        assert_eq!(column_width(Some(664.0), 4, 8.0, 160.0), 160.0);
        assert_eq!(column_width(Some(800.0), 4, 8.0, 160.0), 194.0);
    }

    #[test]
    fn the_item_at_the_top_stays_at_the_top_when_the_column_count_changes() {
        // Row 40 of three across is item 120, which is row 30 of four.
        assert_eq!(retarget_row(40, 3, 4), 30);
        // Widening past the top of the set lands on the row the item is in,
        // not on the row with the same number.
        assert_eq!(retarget_row(10, 4, 3), 13);
        // An item that no longer starts a row is shown in the row it is now
        // part of, which is the row the division answers.
        assert_eq!(retarget_row(5, 3, 2), 7);
        // Nothing changed, nothing moves — including on the first draw,
        // when there is no previous count to correct from. A row is only
        // ever corrected against a count it was actually laid out at; where
        // the first draw goes is `first_row`'s business, below.
        assert_eq!(retarget_row(40, 3, 3), 40);
        assert_eq!(retarget_row(40, 0, 4), 40);
        assert_eq!(retarget_row(40, 4, 0), 40);
        // A row index that will not multiply saturates instead of wrapping.
        assert_eq!(retarget_row(usize::MAX, 4, 2), usize::MAX / 2);
    }

    #[test]
    fn an_item_asked_for_before_the_first_draw_is_shown_when_the_column_count_is_known() {
        // A host restoring a saved position: the count is set and the item
        // asked for with nothing drawn yet, so there is no column count to
        // divide the item by at the time of asking. Item 500 of a thousand,
        // four across, is row 125 — and the previous column count is 0,
        // because there was no previous draw.
        let grid = TileGrid::new(4, 1000);
        assert_eq!(first_row(0, 0, grid, Some(500)), Some(125));
        // The ask outranks keeping the reader where they were. Both things
        // happened between draws; only one of them was asked for.
        assert_eq!(first_row(40, 3, grid, Some(500)), Some(125));
        // With nothing asked for, the correction is what is left: row 40 of
        // three across is item 120, which is row 30 of four.
        assert_eq!(first_row(40, 3, grid, None), Some(30));
        assert_eq!(first_row(40, 4, grid, None), Some(40));
    }

    #[test]
    fn a_first_row_past_the_end_of_the_set_is_pulled_back_to_the_last_one() {
        // Four across, a thousand items: 250 rows, numbered 0..=249.
        let grid = TileGrid::new(4, 1000);
        // An item past the end of the set is the last row and not row 1250,
        // which would draw nothing at all and go on drawing nothing.
        assert_eq!(first_row(0, 4, grid, Some(5000)), Some(249));
        // The same for a set that shrank under a reader who was standing
        // near the bottom of the old one.
        assert_eq!(first_row(900, 4, grid, None), Some(249));
        // A set with nothing in it has no row to put anywhere, and the ask
        // is not answered — so the caller can keep holding it.
        assert_eq!(first_row(40, 4, TileGrid::new(4, 0), Some(3)), None);
    }

    #[test]
    fn a_place_asked_for_before_a_draw_uses_the_count_the_host_has_given() {
        // Nothing drawn yet: no width, so no column count, and the draw's
        // grid holds nothing. The host has said a thousand, so that is the
        // number the question is answered against.
        let asked = answer_grid(TileGrid::default(), 1000);
        assert_eq!(asked.count, 1000);
        assert_eq!(asked.row_of(500), Some(500), "one across until a width says otherwise");
        // Once a draw has settled on four across, the column count is the
        // draw's and the count is still the host's — including a count set
        // after that draw, which is the case that used to answer None.
        let asked = answer_grid(TileGrid::new(4, 12), 1000);
        assert_eq!(asked.row_of(500), Some(125));
        assert_eq!(asked.column_of(500), Some(0));
        assert_eq!(asked.rows(), 250);
        // And a set the host has just cut is answered at the new count, not
        // at the one the last draw laid out.
        let asked = answer_grid(TileGrid::new(4, 1000), 8);
        assert_eq!(asked.rows(), 2);
        assert_eq!(asked.row_of(500), None);
    }

    #[test]
    fn the_row_gap_falls_between_rows_and_never_after_the_last_one() {
        // Carried above each row: the first has none, so the top of the
        // list is flush and the bottom of it is not a band of nothing the
        // reader can scroll into.
        assert_eq!(gap_above(0, 8.0), 0.0);
        assert_eq!(gap_above(1, 8.0), 8.0);
        assert_eq!(gap_above(249, 8.0), 8.0);
        // A gap somebody is in the middle of typing is not a request.
        assert_eq!(gap_above(1, -4.0), 0.0);
        assert_eq!(gap_above(1, f64::NAN), 0.0);
    }
}
