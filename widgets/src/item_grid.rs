//! ItemGrid — a picker of many items: a grid that reflows to its width,
//! keeps which items are chosen, and answers the arrow keys.
//!
//! # Why this is not a tile list
//!
//! [`TileList`] is the LAYOUT, and this widget owns one rather than being a
//! second of them: the rows are that widget's portal list, the column count
//! is its arithmetic, and none of it is repeated here. What the layout says
//! of itself is that it keeps nothing about an item — rows are recycled and
//! the tiles ride along inside them, so a tile told it was selected stays
//! selected under the next index that lands in it — and that it has no
//! keyboard. That is the right division for a layout and the wrong one for
//! a picker, because the two things a picker needs are the two a host is
//! worst placed to supply: a selection that outlives the widget a recycled
//! row hands back, and arrow keys that move by the column count, which is
//! this draw's answer to a width nobody outside the draw has measured. So
//! what is added here is the model that names ITEMS rather than slots — an
//! [`ItemSelection`] over item indices, a cursor the keys walk and the
//! viewport follows, a hit test against the items this draw actually put on
//! screen — and every answer it gives is an item index. Reach for the tile
//! list when the host owns which item is current; reach for this when it
//! should not have to.
//!
//! # What it reports
//!
//! [`ItemGridAction::Picked`] when the chosen set changed,
//! [`ItemGridAction::Activated`] for a second press or Return,
//! [`ItemGridAction::CursorMoved`] for a walk that chose nothing. All three
//! carry an item index. Nothing reports a row and a slot, and nothing leaves
//! a host to work out `row * columns + slot`: the column count is the
//! draw's, so a host deriving the index from a count of its own would be
//! right until the first resize.
//!
//! # The keyboard
//!
//! Left and right step one item. Up and down step a whole row — the column
//! count as it stands, so the same key moves by a different number of items
//! after a resize, which is the point. Home and End go to the ends, the page
//! keys move by the rows on screen, Return activates, the space bar flips
//! the item under the cursor, and the primary key with A takes the lot.
//! Shift sweeps and the primary key toggles; which key is primary is
//! [`KeyModifiers::is_primary`]'s business and not this widget's.
//!
//! A sweep runs in ITEM order and never as a rectangle. A rectangle would be
//! the wrong answer the moment the window changed width: the same two ends
//! would enclose a different set of items, and a reader who swept twelve
//! things would find they had nine.
//!
//! # The host fills the face
//!
//! The loop is [`ItemGrid::next_cell`] until it answers `None`, and each
//! answer carries the item index together with what is TRUE of that item
//! now — whether it is chosen, whether the keys are standing on it. Set the
//! face from those, every draw. They are handed over rather than left to be
//! remembered because the face is recycled and the truth is not.
//!
//! Fill the face; do not draw it. The faces of a row are drawn together, in
//! the row's own turtle, once that row's last slot has been handed out.
//!
//! The face does not answer the press. The grid hit-tests the pointer
//! against the items it put on screen and claims a press that lands on one
//! before the layout sees it, so a face's own press handling never runs:
//! `interactive: false` on the face the grid ships is what keeps it from
//! looking pressable, and a face that needs a press of its own belongs
//! outside the grid. The pick is made on the release, and only when the
//! release was a tap: a finger that went on to scroll the rows, or a
//! button that dragged, chose nothing; a finger held on an item flips it,
//! the one touch gesture that means what a modifier key means to a mouse.
//! A press anywhere else in the grid is left alone and
//! reaches the layout, which is what keeps the rows flingable — and so is a
//! press in the band the layout draws its scroll bar in, because that bar
//! is painted OVER the last column rather than beside it. See
//! [`ItemRect::scroll_bar_band`].
//!
//! The face has to draw something of its own as well. Where an item landed
//! is read back from the face's area once the rows are drawn, so a face
//! that painted nothing — a bare view with no background — has no rect, is
//! left out of the hit test, and that one item quietly stops answering the
//! pointer. The row the grid ships has a background, which is the other
//! thing that makes it a working face.
//!
//! # What it deliberately does not do
//!
//! It holds no items. The host owns the data and writes it into the face;
//! this owns which of them are chosen. It does not drag-select a rubber
//! band, does not reorder and does not delete — a picker answers what was
//! picked and the host does the rest. It does not keep a hover of its own:
//! the ring says where the keys are and the face says what is chosen.
//!
//! It is `Fill` by nature, like the layout inside it: `Fill` in a `Fit` page
//! resolves to nothing at all, so put it in something with a real height.
use crate::{
    event::TouchState,
    item_selection::{
        ItemSelection, SelectionChange, SelectionGesture, SelectionMode, SelectionMove,
        SelectionStep,
    },
    makepad_derive_widget::*,
    makepad_draw::*,
    tile_list::TileList,
    widget::*,
    widget_tree::CxWidgetExt,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawItemRingBase = #(DrawItemRing::script_component(vm))
    set_type_default() do #(DrawItemRing::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.ItemGridBase = #(ItemGrid::register_widget(vm))

    /** A picker of many items: a grid that reflows to its width, draws only
     * the rows on screen, and keeps which items are chosen. */
    mod.widgets.ItemGrid = set_type_default() do mod.widgets.ItemGridBase{
        width: Fill
        height: Fill

        /** how many items across; 0 fits as many as the width allows 0..12 step 1 */
        columns: 0
        /** the narrowest a derived item may be, in pixels 40..600 step 10 */
        min_item_width: 160.
        /** pixels between one item and the next across 0..64 step 1 */
        column_gap: theme.space_2
        /** pixels between one row and the next down 0..64 step 1 */
        row_gap: theme.space_2
        /** many items at once, or one at a time */
        multi: true

        /** how wide the layout's scroll bar is: the band at the right edge a press there belongs to 0..24 step 1 */
        bar_size: 10.

        /** The ring around the item the keys are standing on. It belongs to
         * the grid rather than to the face, because a face is recycled and
         * the reader's place in the set is not. */
        draw_ring +: {
            /** the ring */
            color: uniform(theme.color_primary)
            /** the ring's thickness in points 0.5..4 step 0.5 */
            thickness: uniform(2.0)
            /** corner rounding 0..24 step 0.5 */
            radius: uniform(theme.radius_s)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                // Both properties above are in points and the sdf is in
                // neither: a stroke paints its width on BOTH sides of the
                // path it runs along, and sdf.box draws twice the radius it
                // is given. So the line is stroked at half the thickness on
                // a path pulled in by that same half — which is what puts
                // the whole ring inside the item it rings rather than half
                // over its neighbour — and the radius is halved the way
                // every other rounded corner in the library halves it.
                let half = self.thickness * 0.5
                let r = min(self.radius, min(self.rect_size.x, self.rect_size.y) * 0.5)
                sdf.box(
                    half,
                    half,
                    self.rect_size.x - self.thickness,
                    self.rect_size.y - self.thickness,
                    r * 0.5
                )
                sdf.stroke(self.color, half)
                return sdf.result
            }
        }

        // The layout, whole, as a child. It is a slot (`grid:`) rather than
        // a named child. The tiles are the list's, so a face of one's own
        // is written into it: `grid +: { Tile := MyFace{} }`.
        grid: mod.widgets.TileList{
            width: Fill
            height: Fill

            /** The face of one item. `interactive: false` is not decoration:
             * the grid answers the press itself, and a face that looked
             * pressable would promise what it cannot do. */
            Tile := mod.widgets.ListItem{
                height: 72.
                interactive: false
                padding: theme.space_2
                draw_bg +: {
                    color: theme.color_surface_container
                    radius: theme.radius_s
                }
            }
        }
    }
}

/// The ring around the item the keys are standing on.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawItemRing {
    #[deref]
    draw_super: DrawQuad,
}

/// What the grid says happened, in items.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ItemGridAction {
    /// The chosen set changed; the item the gesture landed on.
    Picked(usize),
    /// A second press, or Return on the cursor: open this one.
    Activated(usize),
    /// The cursor moved and the chosen set did not — a walk with the
    /// primary key held, which is how a reader steps past things they have
    /// already picked.
    CursorMoved(usize),
    #[default]
    None,
}

/// One item's place this draw: which item, and the face to put it in.
///
/// The two flags are the reason the type exists. The face is recycled, so
/// what it was told last time is true of the WIDGET and no longer true of
/// the item standing in it; these are true of the item, now, and a host that
/// sets the face from them cannot leave a stale tick behind.
pub struct ItemGridCell {
    /// The item index, counting across the rows.
    pub index: usize,
    /// How many items across this draw settled on.
    pub columns: usize,
    /// Whether this item is one of the chosen ones.
    pub selected: bool,
    /// Whether the keys are standing on it.
    pub cursor: bool,
    /// The face to fill. Filling it is the host's job; drawing it is not.
    pub widget: WidgetRef,
}

/// Where one item landed on screen.
///
/// Read back from the face's own area once the rows have been drawn, rather
/// than worked out from the column arithmetic a second time: what the hit
/// test wants to know is where the item IS, and a face that came out
/// somewhere else — a margin on the template, a row that ran short — is
/// still the thing the reader is aiming at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemRect {
    pub index: usize,
    pub rect: Rect,
}

impl ItemRect {
    /// The item under a point, if the point landed on one.
    ///
    /// The gaps between items belong to nothing: a press there is not a
    /// press on the nearest item, because a reader aiming at an item hits
    /// the item.
    pub fn index_under(cells: &[ItemRect], point: DVec2) -> Option<usize> {
        cells
            .iter()
            .find(|cell| cell.rect.contains(point))
            .map(|cell| cell.index)
    }

    /// The band the layout draws its scroll bar in, if it is drawing one.
    ///
    /// The bar is an overlay and not a column of its own: the rows are laid
    /// out across the WHOLE viewport and the bar is painted on top of the
    /// right edge of it, so the last column reaches under the bar and the
    /// pixels there show the bar. A press on them is the bar's. If the grid
    /// claimed it — and it would, because a press on an item is claimed
    /// before the layout sees the event — the bar could no longer be
    /// dragged or even hovered: whoever claims a press first gets it, and
    /// the bar tests the ordinary way, which answers nothing once anything
    /// else has claimed.
    ///
    /// There is no band while the rows all fit, because the bar draws
    /// itself only when they do not. That shows here as items this draw
    /// never handed out, or as a row hanging over an edge of the viewport.
    pub fn scroll_bar_band(
        cells: &[ItemRect],
        view: Rect,
        count: usize,
        bar_size: f64,
    ) -> Option<Rect> {
        // A width somebody is in the middle of typing is not a bar.
        if !bar_size.is_finite() || bar_size <= 0.0 {
            return None;
        }
        let all_on_screen = cells.len() >= count
            && cells.iter().all(|cell| Self::is_whole(cell.rect, view));
        if all_on_screen {
            return None;
        }
        let band = bar_size.min(view.size.x);
        Some(Rect {
            pos: dvec2(view.pos.x + view.size.x - band, view.pos.y),
            size: dvec2(band, view.size.y),
        })
    }

    /// Where an item landed, if this draw put it on screen at all.
    pub fn find(cells: &[ItemRect], index: usize) -> Option<Rect> {
        cells
            .iter()
            .find(|cell| cell.index == index)
            .map(|cell| cell.rect)
    }

    /// Whether a rect sits inside the viewport WHOLE.
    ///
    /// Both corners, not the middle: an item whose bottom half is under the
    /// edge is an item the reader cannot read, and the ring the grid draws
    /// on it would be drawn outside the list's own pass, across whatever
    /// stands below the grid.
    pub fn is_whole(rect: Rect, view: Rect) -> bool {
        view.contains(rect.pos) && view.contains(rect.pos + rect.size)
    }

    /// Whether the viewport has to move for the reader to see an item.
    ///
    /// True when this draw did not put the item on screen, and true when it
    /// put only part of it there. A cursor that moved somewhere invisible
    /// has, as far as the reader is concerned, not moved.
    pub fn needs_reveal(cells: &[ItemRect], view: Rect, index: usize) -> bool {
        match Self::find(cells, index) {
            Some(rect) => !Self::is_whole(rect, view),
            None => true,
        }
    }
}

/// The picker's model: which items are chosen, where the cursor stands, and
/// what a press or a key does to both.
///
/// A plain struct with no pixels in it, for the reason the layout's own
/// arithmetic gives for the same choice: the rules can be read and tested
/// without a draw pass, and the draw, the pointer and the keyboard all read
/// the same numbers from the same place. The selection itself is
/// [`ItemSelection`], which already knows what shift and the primary key
/// mean; what is added here is the count — an item index means nothing
/// without one — and the grid's reading of the keyboard, where down is a
/// whole row and a row is however many across the draw settled on.
pub struct ItemGridPicker {
    selection: ItemSelection<usize>,
    count: usize,
}

impl Default for ItemGridPicker {
    fn default() -> Self {
        Self::new(SelectionMode::Many)
    }
}

impl ItemGridPicker {
    pub fn new(mode: SelectionMode) -> Self {
        Self {
            selection: ItemSelection::new(mode),
            count: 0,
        }
    }

    pub fn mode(&self) -> SelectionMode {
        self.selection.mode()
    }

    /// One at a time, or many.
    ///
    /// A change of mode starts a new selection rather than trimming the old
    /// one: a set of twelve things cannot be carried into a list that holds
    /// one, and picking which of the twelve survives is not a decision a
    /// widget should make on a reader's behalf.
    pub fn set_mode(&mut self, mode: SelectionMode) {
        if self.selection.mode() != mode {
            self.selection = ItemSelection::new(mode);
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// Tell the model how many items there are.
    ///
    /// A set that GREW cannot have let anything go, so nothing is scanned
    /// for the common case of a list filling up. A set that shrank is a
    /// delete: the items past the end are gone, and holding on to them
    /// would leave a selection that nothing can show or clear again.
    pub fn set_count(&mut self, count: usize) -> SelectionChange {
        if self.count == count {
            return SelectionChange::default();
        }
        let shrank = count < self.count;
        self.count = count;
        if !shrank {
            return SelectionChange::default();
        }
        let order = self.order();
        self.selection.retain(&order)
    }

    /// Press an item, with whatever was held down at the time.
    ///
    /// A press past the end of the set is not a press: an index the order
    /// does not hold has no position for a range to be measured from.
    pub fn press(&mut self, index: usize, modifiers: KeyModifiers) -> SelectionChange {
        self.press_with(index, SelectionGesture::from_modifiers(modifiers))
    }

    /// Press an item with the gesture named outright: a finger held on an
    /// item toggles it without a key to say so.
    pub fn press_with(&mut self, index: usize, gesture: SelectionGesture) -> SelectionChange {
        if index >= self.count {
            return SelectionChange::default();
        }
        let order = self.order();
        self.selection.click(&order, index, gesture)
    }

    /// Walk the cursor with a key, and take what it lands on unless the
    /// modifiers say otherwise.
    ///
    /// `columns` is how many items go across and `rows` how many rows the
    /// viewport is showing, both of them the draw's own numbers: down is a
    /// row, and a page is a screenful.
    pub fn key(
        &mut self,
        key: KeyCode,
        modifiers: KeyModifiers,
        columns: usize,
        rows: usize,
    ) -> SelectionChange {
        let Some(step) = Self::step_for(key, columns, rows) else {
            return SelectionChange::default();
        };
        let order = self.order();
        self.selection
            .key_move(&order, step, SelectionMove::from_modifiers(modifiers))
    }

    /// Flip the item under the cursor — the space bar, and the only way a
    /// walk with the primary key held ends in a decision.
    pub fn toggle_cursor(&mut self) -> SelectionChange {
        let order = self.order();
        self.selection.toggle_cursor(&order)
    }

    pub fn select_all(&mut self) -> SelectionChange {
        let order = self.order();
        self.selection.select_all(&order)
    }

    pub fn clear(&mut self) -> SelectionChange {
        self.selection.clear()
    }

    /// Put a selection in from outside — a restore, or a host that owns the
    /// truth. Items past the end of the set are dropped.
    pub fn set_chosen(&mut self, items: &[usize]) -> SelectionChange {
        let order = self.order();
        self.selection.set_chosen(&order, items)
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.selection.is_selected(index)
    }

    /// The chosen items, in item order.
    pub fn chosen(&self) -> Vec<usize> {
        let order = self.order();
        self.selection.chosen(&order)
    }

    pub fn cursor(&self) -> Option<usize> {
        self.selection.cursor()
    }

    /// Where a key sends the cursor in a grid this many across, showing
    /// this many rows.
    ///
    /// This is the whole of what makes the keyboard a GRID's keyboard: up
    /// and down are a column count, and the column count is the draw's
    /// answer to a width. Nothing outside a draw can know it, which is why
    /// no host can write this rule for itself.
    ///
    /// None across is one across and no rows on screen is one row — the
    /// state before the first draw, where a key pressed should still move
    /// by something rather than by nothing. The multiplication saturates
    /// because `rows` and `columns` are numbers that arrive from outside;
    /// the step is clamped to the ends of the set by the model below, so an
    /// enormous one is a Home or an End and not a wrap.
    pub fn step_for(key: KeyCode, columns: usize, rows: usize) -> Option<SelectionStep> {
        let across = isize::try_from(columns.max(1)).unwrap_or(isize::MAX);
        let down = isize::try_from(rows.max(1)).unwrap_or(isize::MAX);
        let page = across.saturating_mul(down);
        match key {
            KeyCode::ArrowLeft => Some(SelectionStep::Prev),
            KeyCode::ArrowRight => Some(SelectionStep::Next),
            KeyCode::ArrowUp => Some(SelectionStep::By(-across)),
            KeyCode::ArrowDown => Some(SelectionStep::By(across)),
            KeyCode::PageUp => Some(SelectionStep::By(-page)),
            KeyCode::PageDown => Some(SelectionStep::By(page)),
            KeyCode::Home => Some(SelectionStep::First),
            KeyCode::End => Some(SelectionStep::Last),
            _ => None,
        }
    }

    /// The display order, which for a grid of indices is simply 0..count.
    ///
    /// Built per gesture because [`ItemSelection`] takes the order on every
    /// call and stores none of it — the price of a model a sorted or
    /// filtered list can share, and a scan that is nothing beside laying
    /// the rows out. It is also the reason a set of MILLIONS wants a model
    /// that speaks in ranges: this one materialises a position per item on
    /// every press.
    fn order(&self) -> Vec<usize> {
        (0..self.count).collect()
    }
}

#[derive(Script, Widget)]
pub struct ItemGrid {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    /// The ring on the item the keys are standing on.
    #[live]
    draw_ring: DrawItemRing,

    /// The layout: a tile list, whole. Redrawing and finding go through it
    /// because everything but the ring is inside it.
    #[redraw]
    #[find]
    #[live]
    grid: WidgetRef,

    /// How many items across. Zero derives the count from `min_item_width`.
    #[live]
    pub columns: usize,
    /// The narrowest a derived item may be. It sets the count, not the
    /// width: the items then share the row equally.
    #[live(160.0)]
    pub min_item_width: f64,
    #[live]
    pub column_gap: f64,
    #[live]
    pub row_gap: f64,
    /// Many items at once, or one at a time.
    #[live(true)]
    pub multi: bool,
    /// How wide the layout's scroll bar is. It is not a measurement this
    /// widget lays anything out by: it is how far in from the right edge
    /// the bar's own band reaches, and so which presses are the bar's
    /// rather than the last column's. See [`ItemRect::scroll_bar_band`].
    #[live(10.0)]
    pub bar_size: f64,
    #[live(true)]
    #[visible]
    visible: bool,

    /// The grid's own area: what the pointer is tested against, what holds
    /// the key focus, and what the ring is measured against. The tile
    /// list's area would do for none of those, since it is the thing inside
    /// this one.
    #[rust]
    area: Area,
    /// The viewport the items were laid out in, taken from the turtle while
    /// it is open. Kept rather than read back from the area because the two
    /// questions that need it — is the cursor visible, does it have to be
    /// revealed — are asked once during the draw and once between draws,
    /// and they must be asked about the same rectangle.
    #[rust]
    view: Rect,
    #[rust]
    draw_state: DrawStateWrap<()>,
    #[rust]
    picker: ItemGridPicker,
    /// The items handed to the host this draw, in the order they went out.
    #[rust]
    handed: Vec<(usize, WidgetRef)>,
    /// Where those items landed, read back once the rows are drawn.
    #[rust]
    cells: Vec<ItemRect>,
    /// Whether a draw is open. `next_cell` outside one would drive the
    /// layout's draw state with no turtle under it.
    #[rust]
    drawing: bool,
}

impl ScriptHook for ItemGrid {
    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        // The mode is a property, and the model is built around one, so the
        // two are reconciled wherever the property can have changed.
        self.picker.set_mode(if self.multi {
            SelectionMode::Many
        } else {
            SelectionMode::One
        });
    }
}

/// What a release over an item comes to. See [`ItemGrid::pick_outcome`].
#[derive(Clone, Copy, Debug, PartialEq)]
enum PickOutcome {
    Press,
    Activate,
    Nothing,
}

impl Widget for ItemGrid {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible && event.requires_visibility() {
            return;
        }
        // Before the layout, and only over an item: whoever claims a press
        // first gets it, and a press on an item is the grid's to answer —
        // which is the other half of why the face it ships answers nothing.
        // The layout still gets everything else, the scroll bar and a drag
        // that flings the rows included.
        self.handle_pick(cx, event);
        self.grid.handle_event(cx, event, scope);
        self.claim_keys(cx, event);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ()) {
            if !self.visible {
                // Nothing on screen holds an item any more, and the rects
                // of the draw before this one would answer presses for
                // items that are no longer there.
                self.cells.clear();
                self.draw_state.end();
                return DrawStep::done();
            }
            // Without this the design overlay cannot reach the layout, and
            // through it the rows: nothing else puts the slot in the tree.
            cx.widget_tree_insert_child(self.uid, live_id!(grid), self.grid.clone());
            cx.begin_turtle(walk, self.layout);
            self.push_layout(cx);
            self.handed.clear();
            let inner = self.grid.walk(cx);
            if self.grid.draw_walk(cx, scope, inner).is_step() {
                self.drawing = true;
                // Hand ourselves to the host, which fills faces until
                // `next_cell` runs out.
                return DrawStep::make_step();
            }
            // The layout drew nothing and asked for no filling, so this
            // draw put no item anywhere.
            self.cells.clear();
            cx.end_turtle_with_area(&mut self.area);
            self.draw_state.end();
            return DrawStep::done();
        }
        // The host is done with us. Whatever it filled or left, every row
        // the layout asked for still has to be drawn, or the turtle it
        // opened for the last one is never closed.
        self.drain(cx);
        self.drawing = false;
        let inner = self.grid.walk(cx);
        let _ = self.grid.draw_walk(cx, scope, inner);
        self.view = cx.turtle().inner_rect();
        self.gather_cells(cx);
        self.draw_cursor_ring(cx);
        cx.end_turtle_with_area(&mut self.area);
        self.draw_state.end();
        DrawStep::done()
    }
}

impl ItemGrid {
    /// Give the layout the measurements and the count that this widget's
    /// own properties ask for.
    ///
    /// Forwarded rather than read from there, so that a host writes one
    /// widget's properties and the tweaker shows one widget's properties.
    /// It happens before the layout's turtle opens, because the column
    /// count is derived the moment it does.
    fn push_layout(&mut self, cx: &mut Cx) {
        let count = self.picker.count();
        let Some(mut list) = self.grid.borrow_mut::<TileList>() else {
            return;
        };
        list.columns = self.columns;
        list.min_tile_width = self.min_item_width;
        list.column_gap = self.column_gap;
        list.row_gap = self.row_gap;
        list.set_item_count(cx, count);
    }

    /// The next item to fill, or `None` when the visible rows are done.
    ///
    /// Every answer carries what is true of that item now; see
    /// [`ItemGridCell`]. Fill the face and ask again — the faces of a row
    /// are drawn together once the row's last slot has gone out, so nothing
    /// here should be drawn by the caller.
    pub fn next_cell(&mut self, cx: &mut Cx2d) -> Option<ItemGridCell> {
        if !self.drawing {
            return None;
        }
        // A clone of the handle, not a borrow of the field: the row draws
        // that happen inside `next_tile` would otherwise be running under a
        // borrow rooted in `self`.
        let grid = self.grid.clone();
        let place = {
            let mut list = grid.borrow_mut::<TileList>()?;
            list.next_tile(cx)?
        };
        self.handed.push((place.index, place.widget.clone()));
        Some(ItemGridCell {
            index: place.index,
            columns: place.columns,
            selected: self.picker.is_selected(place.index),
            cursor: self.picker.cursor() == Some(place.index),
            widget: place.widget,
        })
    }

    /// Hand out whatever is left, filled or not, so every row gets drawn.
    fn drain(&mut self, cx: &mut Cx2d) {
        while self.next_cell(cx).is_some() {}
    }

    /// Read back where the items landed, now that the rows have been drawn.
    ///
    /// A face that drew nothing at all has no rect, and is left out rather
    /// than entered as a point: an empty rect at the origin would catch
    /// every press that missed everything.
    fn gather_cells(&mut self, cx: &Cx) {
        self.cells.clear();
        for (index, widget) in &self.handed {
            let rect = widget.area().rect(cx);
            if rect.size.x > 0.0 && rect.size.y > 0.0 {
                self.cells.push(ItemRect {
                    index: *index,
                    rect,
                });
            }
        }
    }

    /// Ring the item the keys are standing on.
    ///
    /// Drawn here, after the layout's pass and before this widget's own
    /// turtle closes: the ring is the grid's pixels, not a property of a
    /// face that gets recycled. Only a whole item is ringed — see
    /// [`ItemRect::is_whole`].
    fn draw_cursor_ring(&mut self, cx: &mut Cx2d) {
        let Some(index) = self.picker.cursor() else {
            return;
        };
        let Some(rect) = ItemRect::find(&self.cells, index) else {
            return;
        };
        if !ItemRect::is_whole(rect, self.view) {
            return;
        }
        self.draw_ring.draw_abs(cx, rect);
    }

    /// The item a press at this point belongs to.
    ///
    /// Not simply the item under it: the band the layout draws its scroll
    /// bar in belongs to the bar, whatever is drawn under it. One rule for
    /// the hit test and for the index, so the two cannot disagree about
    /// what a press landed on.
    fn item_under(&self, point: DVec2) -> Option<usize> {
        let band = ItemRect::scroll_bar_band(
            &self.cells,
            self.view,
            self.picker.count(),
            self.bar_size,
        );
        if band.is_some_and(|band| band.contains(point)) {
            return None;
        }
        ItemRect::index_under(&self.cells, point)
    }

    /// The pointer and the keyboard, both answered in items.
    fn handle_pick(&mut self, cx: &mut Cx, event: &Event) {
        // The ordinary test AND the item list: a press in the gaps between
        // items, on the bar, or past the last row is not claimed here, so
        // it reaches the layout that has something to do with it.
        let hit = {
            let grid = &*self;
            event.hits_with_test(cx, grid.area, |abs, rect, margin| {
                Inset::rect_contains_with_inset(abs, rect, margin)
                    && grid.item_under(abs).is_some()
            })
        };
        match hit {
            // The press claims the keyboard and the digit; the pick waits
            // for the release, so a finger that goes on to scroll the rows
            // has chosen nothing.
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(self.area);
            }
            // A finger held on an item flips it: the one touch gesture that
            // means Toggle, since a finger has no keys to hold.
            Hit::FingerLongPress(lp) => {
                if let Some(index) = self.item_under(lp.abs) {
                    let change = self.picker.press_with(index, SelectionGesture::Toggle);
                    self.announce(cx, change, Some(index));
                }
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                let Some(index) = self.item_under(fe.abs_start) else {
                    return;
                };
                match Self::pick_outcome(fe.was_tap(), fe.tap_count, fe.modifiers) {
                    PickOutcome::Activate => {
                        cx.widget_action(self.uid, ItemGridAction::Activated(index));
                    }
                    PickOutcome::Press => {
                        let change = self.picker.press(index, fe.modifiers);
                        self.announce(cx, change, Some(index));
                    }
                    PickOutcome::Nothing => {}
                }
            }
            Hit::KeyDown(ke) => self.handle_key(cx, ke),
            // The ring is drawn whether or not the grid has the focus, but
            // a focus that arrived by Tab has a cursor to show and the
            // reader has to see it appear.
            Hit::KeyFocus(_) | Hit::KeyFocusLost(_) => self.area.redraw(cx),
            _ => {}
        }
    }

    /// What a release over an item means. A release that was a tap picks;
    /// the second tap of a pair with no key held is an ask to open, since
    /// the first of the pair already chose it, while a second quick press
    /// with a key held is a second pick -- a toggle undone, a sweep
    /// adjusted; a release after a drag or a hold picks nothing, the drag
    /// having been the rows scrolling and the hold already answered.
    fn pick_outcome(was_tap: bool, tap_count: u32, modifiers: KeyModifiers) -> PickOutcome {
        if !was_tap {
            return PickOutcome::Nothing;
        }
        let held = modifiers.shift || modifiers.control || modifiers.logo || modifiers.alt;
        if tap_count > 1 && !held {
            PickOutcome::Activate
        } else {
            PickOutcome::Press
        }
    }

    /// Whether a press leaves the keyboard with the grid.
    ///
    /// `visible` is the part of the grid that is actually on screen and
    /// `claimed` is where whatever took the press already is, if anything
    /// took it. Three things stop a press from being the grid's. A point
    /// outside the visible part is not in the grid at all, however far the
    /// authored rect reaches — inside a page that scrolls, that rect runs
    /// on up behind whatever stands above the page, and a press on a field
    /// up there must not have its focus taken away by a grid nobody can
    /// see. And a press already taken by something that is not inside the
    /// grid was for that thing: a sheet, a menu, a panel over the top of
    /// it. Everything the grid contains — the bar, the rows, the faces —
    /// lies within it, so a claim from one of those is the grid's own and
    /// the keyboard stays.
    ///
    /// `held_outside` is the third, and it is the app-wide pointer-capture
    /// rule: a control that is dragged continuously locks the pointer, and
    /// nothing else may take a press, a hover or a FOCUS from that pointer
    /// until the release. A press arriving while a slider, a scroll bar or a
    /// resizer holds the mouse is handed straight off the capture list and
    /// marks nothing on the event, so it reaches here looking exactly like
    /// bare background — which is why the two rects above cannot see it and
    /// the capture list has to be asked. It is consulted ONLY when nothing
    /// claimed the press: a claim is the better answer when there is one,
    /// and the grid's own layout and bar capture on the way past, so reading
    /// the capture list first would refuse the keyboard on every ordinary
    /// press.
    pub fn keeps_keys(
        visible: Rect,
        claimed: Option<Rect>,
        point: DVec2,
        held_outside: bool,
    ) -> bool {
        if !visible.contains(point) {
            return false;
        }
        match claimed {
            Some(rect) => rect.is_inside_of(visible),
            None => !held_outside,
        }
    }

    /// Keep the keyboard with the grid after a press anywhere inside it.
    ///
    /// Whatever the press was for — an item, the bar, the space past the
    /// last row — the arrows should still walk items afterwards. A press on
    /// an item has already taken the keyboard by the time this runs; what
    /// this adds is the rest of the grid, which the layout answers and
    /// which would otherwise leave the arrows with whoever had them before.
    /// The raw event and not a hit, because a hit would claim the press
    /// this is deliberately letting through; after the layout rather than
    /// before, because the last writer of a key focus is the one that gets
    /// it; and the CLIPPED rect, because the other one is where the grid
    /// was authored rather than where it is. See [`Self::keeps_keys`].
    fn claim_keys(&mut self, cx: &mut Cx, event: &Event) {
        let Some((abs, taken)) = Self::press_start(event) else {
            return;
        };
        let claimed = (!taken.is_empty()).then(|| taken.clipped_rect(cx));
        // The grid and the layout inside it are both `mine`: a press either
        // of them holds is the grid's own press and not an outside one.
        let held_outside = cx
            .fingers
            .is_mouse_held_outside(&[self.area, self.grid.area()]);
        if Self::keeps_keys(self.area.clipped_rect(cx), claimed, abs, held_outside) {
            cx.set_key_focus(self.area);
        }
    }

    /// Where a press began and what had already claimed it, from whichever
    /// event begins one here.
    ///
    /// A finger is never a mouse button: the touch platforms deliver
    /// `TouchUpdate` and the finger events are synthesised from it, so a
    /// widget that reads the raw event - as [`Self::claim_keys`] must,
    /// since a hit would claim the press it is deliberately letting
    /// through - has to read both or work on neither. Only the start of a
    /// touch is a press; a move or a lift is the same one continuing.
    pub fn press_start(event: &Event) -> Option<(DVec2, Area)> {
        match event {
            Event::MouseDown(press) => Some((press.abs, press.handled.get())),
            Event::TouchUpdate(touch) => touch
                .touches
                .iter()
                .find(|point| point.state == TouchState::Start)
                .map(|point| (point.abs, point.handled.get())),
            _ => None,
        }
    }

    fn handle_key(&mut self, cx: &mut Cx, ke: KeyEvent) {
        match ke.key_code {
            KeyCode::ReturnKey => {
                if let Some(index) = self.picker.cursor() {
                    cx.widget_action(self.uid, ItemGridAction::Activated(index));
                }
            }
            KeyCode::Space => {
                let change = self.picker.toggle_cursor();
                let at = self.picker.cursor();
                self.announce(cx, change, at);
            }
            KeyCode::KeyA if ke.modifiers.is_primary() => {
                let change = self.picker.select_all();
                let at = self.picker.cursor();
                self.announce(cx, change, at);
            }
            _ => {
                let columns = self.column_count();
                let rows = self.rows_on_screen(columns);
                let change = self.picker.key(ke.key_code, ke.modifiers, columns, rows);
                if !change.any() {
                    return;
                }
                self.reveal_cursor(cx);
                let at = self.picker.cursor();
                self.announce(cx, change, at);
            }
        }
    }

    /// Say what changed, in items.
    ///
    /// The two flags of a [`SelectionChange`] are reported as two different
    /// things on purpose: a walk with the primary key held moves the cursor
    /// and chooses nothing, and calling that a new answer would make every
    /// arrow press look like a decision.
    fn announce(&mut self, cx: &mut Cx, change: SelectionChange, at: Option<usize>) {
        if !change.any() {
            return;
        }
        self.redraw_all(cx);
        let Some(index) = at else {
            return;
        };
        if change.chosen {
            cx.widget_action(self.uid, ItemGridAction::Picked(index));
        } else {
            cx.widget_action(self.uid, ItemGridAction::CursorMoved(index));
        }
    }

    /// Scroll the cursor into the viewport, but only when it is not already
    /// there.
    ///
    /// The layout scrolls by rows and puts the row it is asked for at the
    /// TOP, so asking on every key press would jerk the whole grid up a row
    /// at a time under a reader who was only stepping sideways.
    fn reveal_cursor(&mut self, cx: &mut Cx) {
        let Some(index) = self.picker.cursor() else {
            return;
        };
        if !ItemRect::needs_reveal(&self.cells, self.view, index) {
            return;
        }
        self.scroll_to_item(cx, index);
    }

    /// The ring lives in this widget's own turtle and the items live in the
    /// layout's, so both are asked to redraw. The derived `redraw` reaches
    /// only the second.
    fn redraw_all(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        self.grid.redraw(cx);
    }

    /// Tell the grid how many items there are. Safe to call during a draw:
    /// the layout takes its row range when the first face is asked for.
    pub fn set_item_count(&mut self, cx: &mut Cx, count: usize) {
        let change = self.picker.set_count(count);
        if let Some(mut list) = self.grid.borrow_mut::<TileList>() {
            list.set_item_count(cx, count);
        }
        // A set that shrank has let go of whatever was chosen past its new
        // end. No action for it: nothing was picked, the set moved.
        if change.any() {
            self.area.redraw(cx);
        }
    }

    pub fn item_count(&self) -> usize {
        self.picker.count()
    }

    /// How many items across the last draw settled on. Zero before the
    /// first draw, since until then there is no width to have derived it
    /// from.
    pub fn column_count(&self) -> usize {
        self.grid
            .borrow::<TileList>()
            .map(|list| list.column_count())
            .unwrap_or(0)
    }

    /// How many rows of items the viewport is showing, counted from the
    /// items this draw actually put in it. At least one, so that a page key
    /// pressed before anything has been drawn still moves.
    fn rows_on_screen(&self, columns: usize) -> usize {
        (self.cells.len() / columns.max(1)).max(1)
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.picker.is_selected(index)
    }

    /// The chosen items, in item order.
    pub fn chosen(&self) -> Vec<usize> {
        self.picker.chosen()
    }

    /// The item the keys are standing on, if any. Not the same as the
    /// chosen set: a swept range has one cursor and many chosen items.
    pub fn cursor(&self) -> Option<usize> {
        self.picker.cursor()
    }

    /// Put a selection in from outside, and show the reader where it went.
    ///
    /// The cursor lands on the last of the items, and a restore that left
    /// it somewhere off screen has not given the reader their selection
    /// back — they would have to press an arrow to find out where it is.
    /// Asked for before the first draw it waits for one, which is the
    /// ordinary case: a host restoring a saved selection has given the
    /// layout no width yet for it to have worked out a row from.
    pub fn set_chosen(&mut self, cx: &mut Cx, items: &[usize]) {
        let change = self.picker.set_chosen(items);
        if change.any() {
            self.reveal_cursor(cx);
            self.redraw_all(cx);
        }
    }

    pub fn clear_selection(&mut self, cx: &mut Cx) {
        let change = self.picker.clear();
        if change.any() {
            self.redraw_all(cx);
        }
    }

    /// Put the row an item is in at the top of the viewport.
    pub fn scroll_to_item(&mut self, cx: &mut Cx, index: usize) {
        if let Some(mut list) = self.grid.borrow_mut::<TileList>() {
            list.scroll_to_item(cx, index);
        }
        self.area.redraw(cx);
    }
}

impl ItemGridRef {
    pub fn set_item_count(&self, cx: &mut Cx, count: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_item_count(cx, count);
        }
    }

    pub fn item_count(&self) -> usize {
        self.borrow().map(|inner| inner.item_count()).unwrap_or(0)
    }

    /// See [`ItemGrid::column_count`]: zero until a draw has settled one.
    pub fn column_count(&self) -> usize {
        self.borrow().map(|inner| inner.column_count()).unwrap_or(0)
    }

    pub fn chosen(&self) -> Vec<usize> {
        self.borrow().map(|inner| inner.chosen()).unwrap_or_default()
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.borrow()
            .map(|inner| inner.is_selected(index))
            .unwrap_or(false)
    }

    pub fn cursor(&self) -> Option<usize> {
        self.borrow().and_then(|inner| inner.cursor())
    }

    pub fn set_chosen(&self, cx: &mut Cx, items: &[usize]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_chosen(cx, items);
        }
    }

    pub fn clear_selection(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_selection(cx);
        }
    }

    pub fn scroll_to_item(&self, cx: &mut Cx, index: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.scroll_to_item(cx, index);
        }
    }

    /// The item the chosen set changed to this pass, if it did.
    ///
    /// This is the hit test, answered. A face raises no action and knows no
    /// item number — it is a row of text, and rows of text do not know what
    /// they are for — so the grid tests the pointer against the items it
    /// put on screen and says which one it was.
    pub fn picked(&self, actions: &Actions) -> Option<usize> {
        match self.action(actions) {
            ItemGridAction::Picked(index) => Some(index),
            _ => None,
        }
    }

    /// The item a second press or a Return asked to open.
    pub fn activated(&self, actions: &Actions) -> Option<usize> {
        match self.action(actions) {
            ItemGridAction::Activated(index) => Some(index),
            _ => None,
        }
    }

    /// The item the cursor walked to without choosing it.
    pub fn cursor_moved(&self, actions: &Actions) -> Option<usize> {
        match self.action(actions) {
            ItemGridAction::CursorMoved(index) => Some(index),
            _ => None,
        }
    }

    fn action(&self, actions: &Actions) -> ItemGridAction {
        actions
            .find_widget_action(self.widget_uid())
            .map(|action| action.cast::<ItemGridAction>())
            .unwrap_or(ItemGridAction::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    /// The additive key, whichever this machine calls it. Both are set so
    /// that the test means the same thing everywhere: the model asks
    /// `is_primary`, and which key that is, is the machine's business.
    fn primary() -> KeyModifiers {
        KeyModifiers {
            control: true,
            logo: true,
            ..Default::default()
        }
    }

    fn shift() -> KeyModifiers {
        KeyModifiers {
            shift: true,
            ..Default::default()
        }
    }

    fn grid_of(count: usize) -> ItemGridPicker {
        let mut picker = ItemGridPicker::new(SelectionMode::Many);
        picker.set_count(count);
        picker
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            pos: dvec2(x, y),
            size: dvec2(w, h),
        }
    }

    /// A context with the library registered, and whatever registering the
    /// rest of it had to say about shaders thrown away, so that what a test
    /// reads afterwards is its own.
    fn cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }

    /// One draw of a grid, in a pass of its own.
    ///
    /// The filling loop is the one the docs give a host — `next_cell` until
    /// it answers `None` — because a grid nobody fills never gets past its
    /// own first step, and then nothing has landed anywhere to be pressed.
    fn draw_once(cx: &mut Cx, grid: &mut ItemGrid, size: DVec2) {
        let pass = DrawPass::new(cx);
        pass.set_size(cx, size);
        let mut draw_list = DrawList2d::new(cx);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        draw_list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(size, Layout::flow_down());
        let walk = grid.walk;
        while grid
            .draw_walk(&mut cx2d, &mut Scope::empty(), walk)
            .is_step()
        {
            while let Some(cell) = grid.next_cell(&mut cx2d) {
                cell.widget
                    .set_text(&mut cx2d, &format!("item {}", cell.index));
            }
        }
        cx2d.end_pass_sized_turtle();
        draw_list.end(&mut cx2d);
        cx2d.end_pass(&pass);
    }

    /// A grid built from its type default the way a page builds one, told
    /// how many items it holds, and drawn once into a viewport of this
    /// size.
    fn drawn_grid(cx: &mut Cx, count: usize, size: DVec2) -> ItemGrid {
        let mut grid = cx.with_vm(ItemGrid::script_new_with_default);
        grid.set_item_count(cx, count);
        draw_once(cx, &mut grid, size);
        grid
    }

    /// Press the grid once, and answer what the press was claimed by.
    ///
    /// One press per grid: a claim is kept until the platform releases the
    /// digit, and nothing here is the platform, so a second press on the
    /// same grid would be answered by the capture rather than by the hit
    /// test under examination.
    fn press_at(cx: &mut Cx, grid: &mut ItemGrid, abs: DVec2) -> (Area, ActionsBuf) {
        let event = Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        });
        let actions = cx.capture_actions(|cx| {
            grid.handle_event(cx, &event, &mut Scope::empty());
            // The pick is made on the release: a tap is a down and an up
            // at the same point.
            let release = Event::MouseUp(crate::event::MouseUpEvent {
                abs,
                button: MouseButton::PRIMARY,
                window_id: WindowId(1, 1),
                modifiers: KeyModifiers::default(),
                time: 0.0,
            });
            grid.handle_event(cx, &release, &mut Scope::empty());
        });
        let Event::MouseDown(press) = &event else {
            unreachable!()
        };
        (press.handled.get(), actions)
    }

    #[test]
    fn a_release_picks_only_when_it_was_a_tap_and_opens_only_with_no_key_held() {
        crate::on_test_cx(|| {
        let bare = KeyModifiers::default();
        let ctrl = KeyModifiers {
            control: true,
            ..Default::default()
        };
        assert_eq!(ItemGrid::pick_outcome(true, 1, bare), PickOutcome::Press);
        assert_eq!(ItemGrid::pick_outcome(true, 2, bare), PickOutcome::Activate);
        // A second quick press with a key held is a second pick, not an open.
        assert_eq!(ItemGrid::pick_outcome(true, 2, ctrl), PickOutcome::Press);
        // After a drag or a hold there is nothing left to pick.
        assert_eq!(ItemGrid::pick_outcome(false, 1, bare), PickOutcome::Nothing);
        });
    }

    /// A finger held on an item flips it, and flips it back: the touch
    /// route to holding several.
    #[test]
    fn a_finger_held_on_an_item_flips_it() {
        crate::on_test_cx(|| {
        let mut picker = ItemGridPicker::new(SelectionMode::Many);
        picker.set_count(6);
        picker.press_with(2, SelectionGesture::Toggle);
        picker.press_with(4, SelectionGesture::Toggle);
        assert_eq!(picker.chosen(), vec![2, 4]);
        picker.press_with(2, SelectionGesture::Toggle);
        assert_eq!(picker.chosen(), vec![4]);
        });
    }

    #[test]
    fn down_moves_by_the_column_count_and_up_moves_back() {
        crate::on_test_cx(|| {
        assert_eq!(
            ItemGridPicker::step_for(KeyCode::ArrowDown, 4, 3),
            Some(SelectionStep::By(4))
        );
        assert_eq!(
            ItemGridPicker::step_for(KeyCode::ArrowUp, 4, 3),
            Some(SelectionStep::By(-4))
        );
        // The same key after a resize is a different number of items, which
        // is the whole reason this rule cannot live in a host.
        assert_eq!(
            ItemGridPicker::step_for(KeyCode::ArrowDown, 7, 3),
            Some(SelectionStep::By(7))
        );
        // Sideways is one item whatever the width.
        assert_eq!(
            ItemGridPicker::step_for(KeyCode::ArrowRight, 7, 3),
            Some(SelectionStep::Next)
        );
        assert_eq!(
            ItemGridPicker::step_for(KeyCode::ArrowLeft, 7, 3),
            Some(SelectionStep::Prev)
        );
        assert_eq!(
            ItemGridPicker::step_for(KeyCode::Home, 4, 3),
            Some(SelectionStep::First)
        );
        assert_eq!(
            ItemGridPicker::step_for(KeyCode::End, 4, 3),
            Some(SelectionStep::Last)
        );
        // A key the grid has no rule for is left to whatever else wants it.
        assert_eq!(ItemGridPicker::step_for(KeyCode::KeyF, 4, 3), None);
        // None across is one across: the state before the first draw, where
        // a key press should still move by something.
        assert_eq!(
            ItemGridPicker::step_for(KeyCode::ArrowDown, 0, 0),
            Some(SelectionStep::By(1))
        );
        });
    }

    #[test]
    fn a_page_key_moves_by_the_rows_on_screen() {
        crate::on_test_cx(|| {
        assert_eq!(
            ItemGridPicker::step_for(KeyCode::PageDown, 4, 5),
            Some(SelectionStep::By(20))
        );
        assert_eq!(
            ItemGridPicker::step_for(KeyCode::PageUp, 4, 5),
            Some(SelectionStep::By(-20))
        );
        // Numbers that arrive from outside saturate rather than wrap, and a
        // saturated step is a Home or an End once the model clamps it.
        assert_eq!(
            ItemGridPicker::step_for(KeyCode::PageDown, usize::MAX, usize::MAX),
            Some(SelectionStep::By(isize::MAX))
        );
        assert_eq!(
            ItemGridPicker::step_for(KeyCode::PageUp, usize::MAX, usize::MAX),
            Some(SelectionStep::By(-isize::MAX))
        );
        });
    }

    #[test]
    fn the_arrows_walk_the_grid_and_take_what_they_land_on() {
        crate::on_test_cx(|| {
        let mut picker = grid_of(20);
        picker.press(5, KeyModifiers::default());
        assert_eq!(picker.chosen(), vec![5]);
        // Four across: down is item 9, and it is chosen, because a grid
        // whose arrows leave the selection behind makes the reader press a
        // second key to mean what they already meant.
        let change = picker.key(KeyCode::ArrowDown, KeyModifiers::default(), 4, 3);
        assert!(change.chosen && change.cursor);
        assert_eq!(picker.cursor(), Some(9));
        assert_eq!(picker.chosen(), vec![9]);
        picker.key(KeyCode::ArrowRight, KeyModifiers::default(), 4, 3);
        assert_eq!(picker.cursor(), Some(10));
        // The ends are the ends of the SET, not of a row.
        picker.key(KeyCode::End, KeyModifiers::default(), 4, 3);
        assert_eq!(picker.cursor(), Some(19));
        // And a step off the bottom stops at the last item rather than
        // wrapping to the top.
        picker.key(KeyCode::ArrowDown, KeyModifiers::default(), 4, 3);
        assert_eq!(picker.cursor(), Some(19));
        picker.key(KeyCode::Home, KeyModifiers::default(), 4, 3);
        assert_eq!(picker.cursor(), Some(0));
        });
    }

    #[test]
    fn a_sweep_runs_in_item_order_and_not_as_a_rectangle() {
        crate::on_test_cx(|| {
        let mut picker = grid_of(20);
        picker.press(2, KeyModifiers::default());
        picker.press(9, shift());
        // Four across, so items 2 and 9 are corners of a rectangle holding
        // 2, 3, 6, 7. That is not what was asked for: the set is the run
        // between the two, and it stays the same run when the grid reflows
        // to five across and the rectangle would have moved.
        assert_eq!(picker.chosen(), vec![2, 3, 4, 5, 6, 7, 8, 9]);
        });
    }

    #[test]
    fn the_primary_key_walks_past_things_and_the_space_bar_decides() {
        crate::on_test_cx(|| {
        let mut picker = grid_of(20);
        picker.press(4, KeyModifiers::default());
        let change = picker.key(KeyCode::ArrowDown, primary(), 4, 3);
        // Moved, chose nothing: the reader is stepping over their own picks
        // on the way somewhere.
        assert!(change.cursor && !change.chosen);
        assert_eq!(picker.cursor(), Some(8));
        assert_eq!(picker.chosen(), vec![4]);
        // And the space bar is what ends that walk in a decision.
        let change = picker.toggle_cursor();
        assert!(change.chosen);
        assert_eq!(picker.chosen(), vec![4, 8]);
        // Twice is off again.
        picker.toggle_cursor();
        assert_eq!(picker.chosen(), vec![4]);
        });
    }

    #[test]
    fn one_at_a_time_keeps_the_last_press_only() {
        crate::on_test_cx(|| {
        let mut picker = ItemGridPicker::new(SelectionMode::One);
        picker.set_count(20);
        picker.press(3, KeyModifiers::default());
        picker.press(7, KeyModifiers::default());
        assert_eq!(picker.chosen(), vec![7]);
        // A sweep has nothing to span in a one-of grid.
        picker.press(12, shift());
        assert_eq!(picker.chosen(), vec![12]);
        // And the mode is a property somebody can change: the selection
        // starts again rather than being trimmed to one of twelve.
        picker.set_mode(SelectionMode::Many);
        assert_eq!(picker.mode(), SelectionMode::Many);
        assert!(picker.chosen().is_empty());
        });
    }

    #[test]
    fn a_press_past_the_end_of_the_set_is_not_a_press() {
        crate::on_test_cx(|| {
        let mut picker = grid_of(20);
        let change = picker.press(20, KeyModifiers::default());
        assert!(!change.any());
        assert!(picker.chosen().is_empty());
        assert_eq!(picker.cursor(), None);
        // A grid with nothing in it answers nothing to anything.
        let mut empty = grid_of(0);
        assert!(!empty.press(0, KeyModifiers::default()).any());
        assert!(!empty
            .key(KeyCode::ArrowDown, KeyModifiers::default(), 4, 3)
            .any());
        assert_eq!(empty.cursor(), None);
        });
    }

    #[test]
    fn a_set_that_shrinks_lets_go_of_the_items_that_left() {
        crate::on_test_cx(|| {
        let mut picker = grid_of(20);
        picker.press(4, KeyModifiers::default());
        picker.press(15, primary());
        assert_eq!(picker.chosen(), vec![4, 15]);
        // Growing cannot have lost anything, so nothing is dropped and
        // nothing is scanned.
        assert!(!picker.set_count(40).any());
        assert_eq!(picker.chosen(), vec![4, 15]);
        // Shrinking is a delete: item 15 is gone, and holding on to it
        // would leave a pick nothing can show or clear again.
        let change = picker.set_count(10);
        assert!(change.any());
        assert_eq!(picker.chosen(), vec![4]);
        // The cursor was standing on 15, so it left with it. The next arrow
        // press starts from the near end of the set rather than from an
        // item that is no longer there.
        assert_eq!(picker.cursor(), None);
        // The count is what the questions are answered against.
        assert_eq!(picker.count(), 10);
        });
    }

    #[test]
    fn everything_can_be_taken_and_let_go_of() {
        crate::on_test_cx(|| {
        let mut picker = grid_of(6);
        assert!(picker.select_all().any());
        assert_eq!(picker.chosen(), vec![0, 1, 2, 3, 4, 5]);
        assert!(picker.clear().any());
        assert!(picker.chosen().is_empty());
        // A restore from outside, and items past the end are dropped rather
        // than kept for a set that might grow into them.
        picker.set_chosen(&[1, 3, 99]);
        assert_eq!(picker.chosen(), vec![1, 3]);
        assert!(picker.is_selected(3));
        assert!(!picker.is_selected(99));
        });
    }

    #[test]
    fn the_item_under_a_point_is_the_one_whose_face_holds_it() {
        crate::on_test_cx(|| {
        let cells = vec![
            ItemRect {
                index: 12,
                rect: rect(0.0, 0.0, 100.0, 50.0),
            },
            ItemRect {
                index: 13,
                rect: rect(110.0, 0.0, 100.0, 50.0),
            },
        ];
        assert_eq!(ItemRect::index_under(&cells, dvec2(50.0, 25.0)), Some(12));
        assert_eq!(ItemRect::index_under(&cells, dvec2(150.0, 25.0)), Some(13));
        // The gap between two items belongs to neither of them: a press
        // there is not a press on the nearer one.
        assert_eq!(ItemRect::index_under(&cells, dvec2(105.0, 25.0)), None);
        assert_eq!(ItemRect::index_under(&cells, dvec2(50.0, 400.0)), None);
        // The index is the ITEM and not the position in the list of rects:
        // only the rows on screen are in there, and the first of them is
        // rarely item zero.
        assert_eq!(ItemRect::find(&cells, 13), Some(rect(110.0, 0.0, 100.0, 50.0)));
        assert_eq!(ItemRect::find(&cells, 0), None);
        });
    }

    #[test]
    fn an_item_that_is_not_wholly_on_screen_has_to_be_revealed() {
        crate::on_test_cx(|| {
        let view = rect(0.0, 0.0, 300.0, 200.0);
        let cells = vec![
            ItemRect {
                index: 4,
                rect: rect(0.0, 10.0, 100.0, 50.0),
            },
            // Hanging over the bottom edge: half an item is not an item the
            // reader can read.
            ItemRect {
                index: 9,
                rect: rect(0.0, 170.0, 100.0, 50.0),
            },
        ];
        assert!(!ItemRect::needs_reveal(&cells, view, 4));
        assert!(ItemRect::needs_reveal(&cells, view, 9));
        // An item this draw did not put on screen at all is the ordinary
        // case: the cursor walked off the viewport.
        assert!(ItemRect::needs_reveal(&cells, view, 400));
        assert!(ItemRect::is_whole(rect(0.0, 10.0, 100.0, 50.0), view));
        assert!(!ItemRect::is_whole(rect(-10.0, 10.0, 100.0, 50.0), view));
        });
    }

    #[test]
    fn the_band_the_scroll_bar_is_drawn_in_is_the_bars_and_not_the_last_columns() {
        crate::on_test_cx(|| {
        let view = rect(0.0, 0.0, 300.0, 200.0);
        // Two items across a viewport they share whole, the way the column
        // arithmetic divides it: the last of them ends where the viewport
        // does, which is where the bar would be drawn.
        let across = vec![
            ItemRect {
                index: 0,
                rect: rect(0.0, 0.0, 146.0, 72.0),
            },
            ItemRect {
                index: 1,
                rect: rect(154.0, 0.0, 146.0, 72.0),
            },
        ];
        // Two items in the set and both of them on screen: nothing is
        // hidden, so the layout draws no bar and the last column keeps its
        // own right edge.
        assert_eq!(ItemRect::scroll_bar_band(&across, view, 2, 10.0), None);
        // The same draw out of a set of a thousand. The rows the reader
        // cannot see are what the bar is for, and it is drawn over the
        // right ten points of the viewport rather than beside them.
        let band = ItemRect::scroll_bar_band(&across, view, 1000, 10.0);
        assert_eq!(band, Some(rect(290.0, 0.0, 10.0, 200.0)));
        let band = band.unwrap();
        assert!(band.contains(dvec2(295.0, 40.0)), "the bar's own pixels");
        assert!(!band.contains(dvec2(285.0, 40.0)), "still the item's");
        // A row hanging over an edge says what a row never handed out says:
        // the set does not fit, so there is a bar.
        let hanging = vec![ItemRect {
            index: 0,
            rect: rect(0.0, 160.0, 146.0, 72.0),
        }];
        assert!(ItemRect::scroll_bar_band(&hanging, view, 1, 10.0).is_some());
        // A bar of no width has no band; nor has a width somebody is in the
        // middle of typing.
        assert_eq!(ItemRect::scroll_bar_band(&across, view, 1000, 0.0), None);
        assert_eq!(
            ItemRect::scroll_bar_band(&across, view, 1000, f64::NAN),
            None
        );
        // And a set with nothing in it has nothing off screen.
        assert_eq!(ItemRect::scroll_bar_band(&[], view, 0, 10.0), None);
        });
    }

    #[test]
    fn a_press_the_grid_cannot_be_seen_at_does_not_take_the_keyboard() {
        crate::on_test_cx(|| {
        let visible = rect(0.0, 100.0, 300.0, 200.0);
        // The ordinary press: inside the part on screen, nothing else
        // holding it.
        assert!(ItemGrid::keeps_keys(
            visible,
            None,
            dvec2(150.0, 150.0),
            false
        ));
        // The rest of the authored rect — in a page that scrolls, the part
        // that has gone up behind whatever stands above the page. A press
        // there belongs to whatever is drawn there.
        assert!(!ItemGrid::keeps_keys(
            visible,
            None,
            dvec2(150.0, 40.0),
            false
        ));
        // A claim from inside the grid is the grid's own: the bar, a row,
        // the grid itself having taken a press on an item.
        assert!(ItemGrid::keeps_keys(
            visible,
            Some(rect(290.0, 100.0, 10.0, 200.0)),
            dvec2(295.0, 150.0),
            false
        ));
        assert!(ItemGrid::keeps_keys(
            visible,
            Some(visible),
            dvec2(150.0, 150.0),
            false
        ));
        // A sheet or a menu over the top of the grid is not inside it, and
        // the press was for that: taking the keyboard here would leave what
        // the reader just pressed unable to hear a key.
        assert!(!ItemGrid::keeps_keys(
            visible,
            Some(rect(100.0, 50.0, 200.0, 120.0)),
            dvec2(150.0, 150.0),
            false
        ));
        });
    }

    /// The app-wide pointer-capture rule, on the one press-driven thing the
    /// grid does outside its own hit test.
    ///
    /// A press that arrives while a slider, a scroll bar or a resizer holds
    /// the mouse is handed off the capture list and marks NOTHING on the
    /// event, so it reaches `claim_keys` looking like bare background. The
    /// grid must not take the keyboard off the control being dragged.
    #[test]
    fn a_press_another_control_holds_does_not_take_the_keyboard() {
        crate::on_test_cx(|| {
        let visible = rect(0.0, 100.0, 300.0, 200.0);
        let point = dvec2(150.0, 150.0);
        assert!(
            ItemGrid::keeps_keys(visible, None, point, false),
            "the same press on an unheld pointer is the grid's"
        );
        assert!(
            !ItemGrid::keeps_keys(visible, None, point, true),
            "the keyboard was taken off a control mid-drag"
        );
        // A press something inside the grid DID claim is still the grid's,
        // whatever the capture list says: the grid's own layout and bar
        // capture on the way past, so reading the capture list first would
        // refuse the keyboard on every ordinary press.
        assert!(ItemGrid::keeps_keys(visible, Some(visible), point, true));
        });
    }

    /// The keyboard rule is asked the same question by a finger as by a
    /// mouse. Read only `MouseDown` and the arrows stay with whoever had
    /// them on every touch platform, which is where a picker of thousands
    /// is most likely to be driven from.
    #[test]
    fn a_finger_begins_a_press_the_same_way_a_mouse_button_does() {
        crate::on_test_cx(|| {
        let at = dvec2(150.0, 150.0);
        let mouse = Event::MouseDown(MouseDownEvent {
            abs: at,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        });
        assert_eq!(ItemGrid::press_start(&mouse).map(|(p, _)| p), Some(at));

        let touch = |state| {
            Event::TouchUpdate(crate::event::TouchUpdateEvent {
                time: 0.0,
                window_id: WindowId(1, 1),
                modifiers: KeyModifiers::default(),
                touches: vec![crate::event::TouchPoint {
                    state,
                    abs: at,
                    time: 0.0,
                    uid: 1,
                    rotation_angle: 0.0,
                    force: 1.0,
                    radius: dvec2(1.0, 1.0),
                    handled: Cell::new(Area::Empty),
                    sweep_lock: Cell::new(Area::Empty),
                }],
            })
        };
        assert_eq!(
            ItemGrid::press_start(&touch(TouchState::Start)).map(|(p, _)| p),
            Some(at)
        );
        // The rest of the touch is that press continuing, not another one:
        // claiming the keyboard again on every move would take it back
        // from anything the press had handed it to.
        for state in [TouchState::Move, TouchState::Stable, TouchState::Stop] {
            assert!(ItemGrid::press_start(&touch(state)).is_none());
        }
        // And what the press had already been claimed by is carried out of
        // either, since that is what decides whose press it was.
        let claimed = Area::Empty;
        assert_eq!(ItemGrid::press_start(&mouse).map(|(_, a)| a), Some(claimed));
        });
    }

    /// The pointer path end to end: a real draw, a real press on a face,
    /// and an answer in items.
    #[test]
    fn a_press_on_an_item_picks_that_item_and_says_which() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = drawn_grid(&mut cx, 400, dvec2(400.0, 300.0));
        assert!(
            grid.column_count() > 1,
            "a width holding one item is not a grid"
        );
        let cell = *grid
            .cells
            .get(1)
            .expect("the draw put no second item on screen");
        let (_, actions) = press_at(&mut cx, &mut grid, cell.rect.center());
        assert_eq!(grid.chosen(), vec![cell.index]);
        assert_eq!(grid.cursor(), Some(cell.index));
        let said: Vec<ItemGridAction> = actions
            .filter_widget_actions_cast::<ItemGridAction>(grid.widget_uid())
            .collect();
        assert_eq!(said, vec![ItemGridAction::Picked(cell.index)]);
        });
    }

    /// The band along the right edge is the bar's, and the grid claiming it
    /// is what would stop the bar being dragged or even hovered.
    #[test]
    fn a_press_in_the_scroll_bar_band_is_left_to_the_layout() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = drawn_grid(&mut cx, 400, dvec2(400.0, 300.0));
        let view = grid.view;
        let point = dvec2(view.pos.x + view.size.x - 3.0, view.pos.y + 20.0);
        // The item really is under the point: the rows are laid out across
        // the whole width and the bar is painted on top of them.
        assert!(
            ItemRect::index_under(&grid.cells, point).is_some(),
            "the band is not over an item, so this proves nothing"
        );
        let (claimed, _) = press_at(&mut cx, &mut grid, point);
        assert!(grid.chosen().is_empty(), "the bar's press picked an item");
        assert_eq!(grid.cursor(), None);
        // And it reached the bar itself, which is the point of leaving it
        // alone: what answered is a tall sliver the width of the band, not
        // the grid and not the list behind it.
        let band = ItemRect::scroll_bar_band(&grid.cells, view, grid.item_count(), grid.bar_size)
            .expect("a set this long does not fit, so there is a bar");
        let answered = claimed.rect(&cx);
        assert!(
            answered.size.x <= band.size.x && band.contains(answered.center()),
            "the press never reached the bar: {answered:?}"
        );
        });
    }

    /// And the same point in a grid that all fits is an ordinary part of
    /// the item, because a layout with nothing hidden draws no bar.
    #[test]
    fn a_grid_that_all_fits_answers_presses_right_across_its_width() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = drawn_grid(&mut cx, 3, dvec2(400.0, 300.0));
        let view = grid.view;
        assert_eq!(
            ItemRect::scroll_bar_band(&grid.cells, view, grid.item_count(), grid.bar_size),
            None
        );
        let cell = *grid
            .cells
            .get(1)
            .expect("the draw put no second item on screen");
        let point = dvec2(view.pos.x + view.size.x - 3.0, cell.rect.center().y);
        assert_eq!(ItemRect::index_under(&grid.cells, point), Some(cell.index));
        press_at(&mut cx, &mut grid, point);
        assert_eq!(grid.chosen(), vec![cell.index]);
        });
    }

    /// The space past the last row is the layout's, not an item's: it is
    /// the press [`ItemGrid::claim_keys`] exists for, and the grid must let
    /// it through to get there.
    #[test]
    fn a_press_past_the_last_row_is_left_to_the_layout() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = drawn_grid(&mut cx, 3, dvec2(400.0, 300.0));
        let view = grid.view;
        let point = dvec2(view.center().x, view.pos.y + view.size.y - 4.0);
        assert_eq!(
            ItemRect::index_under(&grid.cells, point),
            None,
            "that point is on an item, so this proves nothing"
        );
        let (claimed, actions) = press_at(&mut cx, &mut grid, point);
        assert!(grid.chosen().is_empty());
        assert_eq!(grid.cursor(), None);
        assert!(
            actions
                .filter_widget_actions_cast::<ItemGridAction>(grid.widget_uid())
                .next()
                .is_none(),
            "the grid answered a press that was not on an item"
        );
        // The layout answered instead, over the whole of itself: it is
        // what scrolls, and a press it never sees is a list that cannot be
        // flung.
        assert_eq!(claimed.rect(&cx), grid.area.rect(&cx));
        });
    }

    /// The ring is the grid's own pixels, drawn on the item the keys are
    /// standing on. This is also where its shader is built and run, so a
    /// mistake in the pixel function fails here.
    #[test]
    fn the_ring_is_drawn_on_the_item_the_cursor_stands_on() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = drawn_grid(&mut cx, 400, dvec2(400.0, 300.0));
        let cell = *grid
            .cells
            .get(1)
            .expect("the draw put no second item on screen");
        press_at(&mut cx, &mut grid, cell.rect.center());
        draw_once(&mut cx, &mut grid, dvec2(400.0, 300.0));
        assert_eq!(
            crate::makepad_draw::makepad_platform::shader_error::take(),
            None,
            "the ring's shader failed to compile"
        );
        assert_eq!(
            grid.draw_ring.area().rect(&cx),
            cell.rect,
            "the ring is not on the item the keys are standing on"
        );
        });
    }

    /// A selection put in from outside is shown and not merely held: the
    /// cursor lands on the last of the items, and a cursor the reader
    /// cannot see is a restore they have to go looking for.
    #[test]
    fn a_restored_selection_is_scrolled_to() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let size = dvec2(400.0, 300.0);
        let mut grid = drawn_grid(&mut cx, 1000, size);
        assert!(
            ItemRect::find(&grid.cells, 900).is_none(),
            "item 900 was on screen already, so this proves nothing"
        );
        grid.set_chosen(&mut cx, &[900]);
        assert_eq!(grid.cursor(), Some(900));
        draw_once(&mut cx, &mut grid, size);
        assert!(
            ItemRect::find(&grid.cells, 900).is_some(),
            "the restore left the reader at the other end of the set"
        );
        assert!(grid.is_selected(900));
        // The layout was asked in rows, which is the only thing it scrolls
        // by: the top of the viewport is the start of the row item 900 is
        // in.
        let columns = grid.column_count();
        let first = grid
            .grid
            .borrow::<TileList>()
            .expect("the layout is a tile list")
            .first_visible_item();
        assert_eq!(first, 900 / columns * columns);
        });
    }
}
