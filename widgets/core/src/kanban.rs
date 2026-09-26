//! KanbanBoard — columns of cards, dragged within a column to put them in
//! order and across columns to move them on.
//!
//! # Who owns the cards
//!
//! The board does, and this is the one place it parts company with
//! [`crate::reorder_list::ReorderList`]. The reorder list moves nothing: it
//! reports where a row was dropped and the host applies it, because the host
//! is the only thing that knows what a row IS. A board is different in one
//! respect that decides the whole design — a column may cap how many cards
//! it will hold, so the board is the thing that knows whether a drop can
//! happen at all. Something that has to answer "no" has to be the thing that
//! moves the card when the answer is yes; a widget that drew a refusal and
//! then reported the move anyway would be lying to the host. So the board
//! moves its own cards and says what it did.
//!
//! # The gesture
//!
//! A press on a card and a drag lifts it: the card stays where it lies,
//! washed over so it reads as picked up, and a marker is drawn in the gap it
//! would land in. Let go and it goes there. The gap is decided by the
//! midpoints of the cards drawn in the column under the pointer, which is
//! exactly the rule the reorder list already uses — [`ReorderDrag`] does that
//! part here too, so a card and a row land in the same place for the same
//! reason.
//!
//! Two things the row machine cannot do on its own, and this file adds:
//! the column under the pointer, and waking on SIDEWAYS travel. A card
//! carried straight across to the next column never moves up or down at all,
//! and a gesture that only watches y would never start.
//!
//! Escape drops the gesture. A wheel scrolls the column under the pointer,
//! and is swallowed during a drag so the cards cannot slide out from under
//! the finger; instead, a drag held near a column's top or bottom edge
//! crawls that column, so a long column can be reached without letting go.
//!
//! # What a cap means
//!
//! `cap` is the most cards a column will hold; 0 is no limit. A full column
//! says so beside its count and turns the marker to the refusal colour, and
//! the drop leaves the card where it was. It still reorders its OWN cards: a
//! cap is about how much work is in flight, not about whether the column may
//! be tidied.
//!
//! # What it deliberately does not do
//!
//! It does not virtualise: every card is a live widget, which is right for
//! the tens of cards a board is read at a glance and wrong for thousands.
//! It does not scroll sideways — columns are squeezed towards
//! `min_column_width` to fit the width they are given, and a board with more
//! columns than that will hold clips them; a board that needs to page
//! through columns wants a scrolling container around it. It does not add,
//! remove or rename columns, edit a card, or move a card by keyboard, and a
//! card is one line of text: anything richer belongs in the `card` template,
//! which the board fills by writing into the `title` inside it.
//!
//! A board takes the height it is given. In a parent that sizes to fit,
//! `height: Fill` resolves to nothing and the board is laid out and never
//! painted, so a board on a page needs a stated height.
use crate::{
    badge::measure,
    makepad_derive_widget::*,
    makepad_draw::*,
    reorder_list::{ReorderDrag, RowBand},
    widget::*,
    widget_tree::CxWidgetExt,
};

script_mod! {
    use mod.prelude.widgets_internal.*

    /** One column of a board: `heading` is what it is called, `cap` the most
     * cards it will hold (0 is no limit), and `cards` the ones it starts
     * with, top first. */
    // The bare type, with no default written over it: a board tells its
    // columns from its card template by the TYPE of each entry, and only an
    // object carrying this type's own tag is a column.
    //
    // Declared before the `use` below, because a block's `use` is a snapshot
    // of the namespace as it stood when the block began: a name declared in
    // the same block is only reachable by its full path afterwards.
    mod.widgets.KanbanColumn = #(KanbanColumnSpec::script_api(vm))

    use mod.widgets.*

    mod.widgets.KanbanBoardBase = #(KanbanBoard::register_widget(vm))

    set_type_default() do #(DrawKanbanColumn::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    /** The card a board draws when its host offers no template of its own:
     * a filled card with one line of text in it.
     *
     * The board writes each card's text into the `title` inside the body, so
     * a template of your own needs one under that name or the cards all read
     * whatever the template says. */
    mod.widgets.KanbanCard = mod.widgets.FilledCard{
        width: Fill
        height: Fit
        radius: theme.radius_s
        padding: theme.mspace_1{left: theme.space_2, right: theme.space_2}
        body: mod.widgets.CardBody{
            title := mod.widgets.Label{
                width: Fill
                text: ""
            }
        }
    }

    /** Columns of cards: drag one within its column to put it in order, or
     * across to move it on. A column can cap how many it will hold. */
    mod.widgets.KanbanBoard = set_type_default() do mod.widgets.KanbanBoardBase{
        width: Fill
        // A board takes the height it is given; inside a parent that sizes
        // to fit, this resolves to nothing and the board is laid out and
        // never painted. Such a page has to state a height.
        height: Fill
        padding: theme.mspace_2

        /** how wide a column is; they are squeezed when they do not all fit 120..400 step 10 */
        column_width: 200.
        /** the least a column may be squeezed to before they are clipped 80..300 step 10 */
        min_column_width: 120.
        /** the room between two columns 0..40 step 1 */
        column_gap: theme.space_2
        /** the height of a column's heading band 0..60 step 1 */
        header_height: 26.
        /** the room between two cards 0..24 step 1 */
        card_spacing: theme.space_2
        /** the room between a column's edge and its cards 0..24 step 1 */
        card_inset: theme.space_2
        /** travel before a press on a card becomes a drag 0..20 step 1 */
        drag_threshold: 4.
        /** how thick the drop marker is drawn 1..8 step 0.5 */
        marker_size: 2.
        /** what a column that will take no more cards says about itself */
        full_text: "Full"

        /** The board's own ground, under the columns. */
        draw_bg +: {
            color: theme.color_surface
        }

        /** A column: a rounded panel, a line under its heading band, and the
         * two states the board writes into it every draw.
         *
         * `target`, `full` and `header_px` are PLAIN, because each has a
         * field on the draw struct behind it and a value the runtime is
         * asked to bind to a field cannot be declared as an instance. The
         * colours below them have no such field: they are the shader's own
         * and are uniforms. */
        draw_column +: {
            /** the column a live drag would drop into 0..1 step 1 */
            target: 0.0
            /** the column will take no more cards 0..1 step 1 */
            full: 0.0
            /** where the heading band ends, in pixels 0..80 step 1 */
            header_px: 26.0

            color: uniform(theme.color_surface_container_low)
            color_target: uniform(theme.color_surface_container_high)
            border_color: uniform(theme.color_outline_variant)
            border_color_full: uniform(theme.color_warning)
            rule_color: uniform(theme.color_outline_variant)
            /** the line around a column 0..4 step 0.5 */
            border_size: uniform(1.0)
            /** corner rounding 0..24 step 0.5 */
            border_radius: uniform(theme.radius_l)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_size * 0.5
                    self.border_size * 0.5
                    self.rect_size.x - self.border_size
                    self.rect_size.y - self.border_size
                    self.border_radius
                )
                sdf.fill_keep(self.color.mix(self.color_target, self.target))
                sdf.stroke(self.border_color.mix(self.border_color_full, self.full), self.border_size)
                // The line under the heading, so the count reads as the
                // column's own and not as something about the first card.
                sdf.rect(0.0, self.header_px, self.rect_size.x, self.border_size)
                sdf.fill(self.rule_color)
                return sdf.result
            }
        }

        draw_heading +: {
            color: theme.color_label_outer
            text_style: theme.font_bold{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_count +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_full +: {
            color: theme.color_warning
            text_style: theme.font_bold{font_size: theme.font_size_p, line_spacing: 1.0}
        }

        /** The gap the card would land in. */
        draw_marker +: {
            draw_depth: 10.0
            color: theme.color_primary
        }
        /** The same marker where the column will not take the card. */
        draw_refusal +: {
            draw_depth: 10.0
            color: theme.color_warning
        }
        /** Laid over the card being carried, which is still lying where it
         * was: the board moves nothing until the drop. */
        draw_lift +: {
            draw_depth: 10.0
            color: theme.color_drag_target_preview
        }

        // A named entry, the way a list declares its item template: it lands
        // in the vec and is collected rather than drawn as a child. A caller
        // writing its own `card :=` replaces this one.
        card := mod.widgets.KanbanCard{}
    }
}

/// A column: a rounded panel, the line under its heading band, and the two
/// states the board writes into it on every draw.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawKanbanColumn {
    #[deref]
    draw_super: DrawQuad,
    /// The column a live drag would drop into.
    #[live]
    target: f32,
    /// The column will take no more cards.
    #[live]
    full: f32,
    /// Where the heading band ends, so the shader and the text agree on one
    /// number rather than each keeping its own.
    #[live]
    header_px: f32,
}

/// A column as it is written in the DSL.
///
/// It is a declaration, not the column itself: the board copies it once and
/// then owns what it holds, because the cards move and the declaration does
/// not.
#[derive(Script, ScriptHook, Default)]
pub struct KanbanColumnSpec {
    #[source]
    source: ScriptObjectRef,
    /// What the column is called.
    #[live]
    pub heading: String,
    /// The most cards it will hold; 0 is no limit.
    #[live]
    pub cap: usize,
    /// The cards it starts with, top first.
    #[live]
    pub cards: Vec<String>,
}

impl KanbanColumnSpec {
    fn to_column(&self) -> KanbanColumn {
        KanbanColumn {
            heading: self.heading.clone(),
            cap: self.cap,
            cards: self.cards.clone(),
        }
    }
}

/// One column of the running board: what it is called, how many cards it
/// will hold, and the cards it holds now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KanbanColumn {
    pub heading: String,
    /// The most cards this column will hold; 0 is no limit.
    pub cap: usize,
    pub cards: Vec<String>,
}

impl KanbanColumn {
    pub fn is_full(&self) -> bool {
        self.cap > 0 && self.cards.len() >= self.cap
    }

    /// What the number beside the heading says. A capped column shows what
    /// it is allowed as well as what it holds, since the cap is the only
    /// reason a drop is ever refused and a person is owed the reason before
    /// the refusal.
    pub fn count_text(&self) -> String {
        if self.cap > 0 {
            format!("{} / {}", self.cards.len(), self.cap)
        } else {
            format!("{}", self.cards.len())
        }
    }
}

/// What a drop would do to the board.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum KanbanLanding {
    /// The card lands at this index of the target column.
    Moved { to: usize },
    /// The target column is full, and a full column takes nothing from
    /// anywhere else.
    Refused,
    /// The card is already where it was dropped, or the drop names nothing
    /// that exists.
    Nothing,
}

/// The columns and their cards, and the arithmetic of moving one card.
///
/// It is a separate type from the widget for two reasons: it can be
/// exercised without a script heap behind it, and the rule that decides
/// where a card lands is then written once, in one place, rather than in the
/// gesture and again in the drawing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KanbanBoardModel {
    pub columns: Vec<KanbanColumn>,
}

impl KanbanBoardModel {
    pub fn card_count(&self) -> usize {
        self.columns.iter().map(|column| column.cards.len()).sum()
    }

    /// Where a column's cards begin in the flat pool of card widgets.
    pub fn offset_of(&self, column: usize) -> usize {
        self.columns
            .iter()
            .take(column)
            .map(|column| column.cards.len())
            .sum()
    }

    /// What dropping the card at `from` in `from_column` into the gap before
    /// `slot` of `to_column` would do. `slot` counts the gaps of the target
    /// column as they are DRAWN: the gap before card 0, before card 1, and
    /// so on, with `len` meaning after the last one.
    pub fn plan(
        &self,
        from_column: usize,
        from: usize,
        to_column: usize,
        slot: usize,
    ) -> KanbanLanding {
        let Some(source) = self.columns.get(from_column) else {
            return KanbanLanding::Nothing;
        };
        let Some(target) = self.columns.get(to_column) else {
            return KanbanLanding::Nothing;
        };
        if from >= source.cards.len() {
            return KanbanLanding::Nothing;
        }
        if from_column == to_column {
            // The card is still lying in the column while it is carried, so
            // every gap below it counts the card itself and has to give that
            // one back.
            let to = if slot > from { slot - 1 } else { slot };
            let to = to.min(source.cards.len() - 1);
            if to == from {
                KanbanLanding::Nothing
            } else {
                KanbanLanding::Moved { to }
            }
        } else {
            if target.is_full() {
                return KanbanLanding::Refused;
            }
            KanbanLanding::Moved {
                to: slot.min(target.cards.len()),
            }
        }
    }

    /// [`plan`](Self::plan), and then do it.
    pub fn apply(
        &mut self,
        from_column: usize,
        from: usize,
        to_column: usize,
        slot: usize,
    ) -> KanbanLanding {
        let landing = self.plan(from_column, from, to_column, slot);
        if let KanbanLanding::Moved { to } = landing {
            let card = self.columns[from_column].cards.remove(from);
            self.columns[to_column].cards.insert(to, card);
        }
        landing
    }

    /// Put a card at the end of a column. False when the column is full or
    /// there is no such column, which is the same answer a drop gets.
    pub fn push(&mut self, column: usize, card: &str) -> bool {
        let Some(column) = self.columns.get_mut(column) else {
            return false;
        };
        if column.is_full() {
            return false;
        }
        column.cards.push(card.to_string());
        true
    }
}

/// One column as the drag sees it: its index, its left edge and its width.
pub type KanbanColumnBand = (usize, f64, f64);

/// The column a pointer at `x` is aiming at: the one it is inside, else the
/// nearest one.
///
/// Nearest rather than none, because a finger in the gap between two columns
/// or off the end of the board is still aiming at a column; answering
/// "nowhere" would drop the card back where it came from every time the hand
/// strayed a few points.
pub fn kanban_column_at(columns: &[KanbanColumnBand], x: f64) -> Option<usize> {
    let mut nearest: Option<(usize, f64)> = None;
    for (index, left, width) in columns {
        if x >= *left && x < left + width {
            return Some(*index);
        }
        let distance = if x < *left { left - x } else { x - (left + width) };
        if nearest.map_or(true, |(_, best)| distance < best) {
            nearest = Some((*index, distance));
        }
    }
    nearest.map(|(index, _)| index)
}

/// How wide each of `count` columns is drawn in `width` points.
///
/// They keep the width they asked for while they all fit, and are squeezed
/// evenly when they do not — down to `least`, and no further. Past that the
/// board would rather clip a column than draw one too narrow to read, and
/// the doc says so.
pub fn kanban_column_width(want: f64, least: f64, width: f64, gap: f64, count: usize) -> f64 {
    if count == 0 {
        return want;
    }
    let gaps = gap * count.saturating_sub(1) as f64;
    let room = (width - gaps) / count as f64;
    if room >= want {
        want
    } else {
        room.max(least.min(want))
    }
}

/// One live drag of a card, from the press to the release.
///
/// The row machine the reorder list already has does the slot — which gap of
/// the column under the pointer the card would land in, decided by the
/// midpoints of the cards drawn there. Two things it cannot do on its own
/// are added here: which column the pointer is over, and waking on sideways
/// travel. A card carried straight across to the next column never moves up
/// or down, and the row machine only watches y, so a board that left the
/// waking to it would have a gesture that never started.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KanbanDrag {
    /// The column the card was picked up from.
    pub from_column: usize,
    /// The column under the pointer now.
    pub to_column: usize,
    start_x: f64,
    start_y: f64,
    row: ReorderDrag,
}

impl KanbanDrag {
    pub fn press(column: usize, card: usize, x: f64, y: f64) -> Self {
        Self {
            from_column: column,
            to_column: column,
            start_x: x,
            start_y: y,
            row: ReorderDrag::press(card, y),
        }
    }

    /// The card being carried, as an index into its own column.
    pub fn from(&self) -> usize {
        self.row.from
    }

    /// The gap of `to_column` the card would land in.
    pub fn slot(&self) -> usize {
        self.row.slot
    }

    /// True once the press has travelled far enough to be a drag. A press
    /// that never does is a press, and moves nothing.
    pub fn active(&self) -> bool {
        self.row.active
    }

    /// Advance with a pointer at `x`, `y` over `column`, whose cards are
    /// `rows`. True when something a person can see has changed.
    pub fn move_to(
        &mut self,
        x: f64,
        y: f64,
        threshold: f64,
        column: usize,
        rows: &[RowBand],
    ) -> bool {
        let mut changed = false;
        if !self.row.active {
            if (x - self.start_x).abs() < threshold && (y - self.start_y).abs() < threshold {
                return false;
            }
            self.row.active = true;
            changed = true;
        }
        if self.to_column != column {
            self.to_column = column;
            changed = true;
        }
        if rows.is_empty() {
            // An empty column has no midpoints to compare against, and one
            // gap to land in.
            if self.row.slot != 0 {
                self.row.slot = 0;
                changed = true;
            }
        } else if self.row.move_to(y, 0.0, rows) {
            // The threshold is 0 here on purpose: waking is settled above,
            // and this call is only being asked for the slot.
            changed = true;
        }
        changed
    }

    /// What the release asks the board to do: source column and card, target
    /// column and gap. Nothing for a press that never became a drag.
    pub fn commit(self) -> Option<(usize, usize, usize, usize)> {
        if !self.row.active {
            return None;
        }
        Some((
            self.from_column,
            self.row.from,
            self.to_column,
            self.row.slot,
        ))
    }
}

/// Where a card ended up.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct KanbanMove {
    pub from_column: usize,
    pub from: usize,
    pub to_column: usize,
    pub to: usize,
}

/// What a board reports. It has already moved the card by the time either of
/// these arrives: the cap makes the board the only thing that can decide,
/// and something that decides has to act.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum KanbanBoardAction {
    Moved(KanbanMove),
    /// A card was carried to a column that is full and stayed where it was.
    Refused {
        column: usize,
    },
    #[default]
    None,
}

/// Where a glyph's ink begins below the y handed to `draw_abs`, as a share
/// of the font size: the call takes the top of the LINE box.
const INK_TOP: f64 = 0.30;
/// How near a column's top or bottom edge a held drag starts crawling it,
/// and how far it crawls each frame.
const EDGE_BAND: f64 = 28.0;
const EDGE_CRAWL: f64 = 9.0;

/// The y at which a line of text of `font_size` sits centred in a band.
fn centred_baseline(top: f64, height: f64, font_size: f64) -> f64 {
    top + (height - font_size) * 0.5 - font_size * INK_TOP
}

/// Where a column was drawn, kept from one draw to the next so a press knows
/// what it landed on. Rects rather than areas: an `Area` a widget keeps for
/// itself goes stale on the first redraw, and these are rebuilt every draw
/// anyway.
#[derive(Copy, Clone, Debug, Default)]
struct ColumnGeom {
    panel: Rect,
    body: Rect,
    /// How tall the cards came out, for the scroll to clamp against.
    content: f64,
}

#[derive(Script, Widget)]
pub struct KanbanBoard {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_column: DrawKanbanColumn,
    #[live]
    draw_heading: DrawText,
    #[live]
    draw_count: DrawText,
    #[live]
    draw_full: DrawText,
    #[live]
    draw_marker: DrawColor,
    #[live]
    draw_refusal: DrawColor,
    #[live]
    draw_lift: DrawColor,

    #[live(200.0)]
    pub column_width: f64,
    #[live(120.0)]
    pub min_column_width: f64,
    #[live(6.0)]
    pub column_gap: f64,
    #[live(26.0)]
    pub header_height: f64,
    #[live(6.0)]
    pub card_spacing: f64,
    #[live(6.0)]
    pub card_inset: f64,
    #[live(4.0)]
    pub drag_threshold: f64,
    #[live(2.0)]
    pub marker_size: f64,
    /// What a column that will take no more cards says about itself.
    #[live("Full".to_string())]
    pub full_text: String,

    /// The card template, collected from the instance by name.
    #[rust]
    card_template: Option<ScriptObjectRef>,
    /// The columns as they were written down. Copied into `board` once; the
    /// declaration is not the board, because the cards move and it does not.
    #[rust]
    declared: Vec<KanbanColumn>,
    #[rust]
    board: KanbanBoardModel,
    #[rust]
    built: bool,
    /// One widget per card, appended to and never reordered: card 2 is
    /// whatever is second in the board NOW, and its text is written on every
    /// draw.
    #[rust]
    pool: Vec<WidgetRef>,
    #[rust]
    geom: Vec<ColumnGeom>,
    /// Where every card was drawn, in column order. The drag reads these
    /// rather than the cards' own areas, so a card scrolled half out of its
    /// column still has the rect the midpoint rule needs.
    #[rust]
    card_rects: Vec<Vec<Rect>>,
    #[rust]
    scroll: Vec<f64>,
    #[rust]
    drag: Option<KanbanDrag>,
    /// Where the pointer was last seen, so a drag HELD at a column's edge
    /// keeps crawling between moves.
    #[rust]
    pointer: Option<DVec2>,
    #[rust]
    scroll_pump: NextFrame,
    #[rust]
    warned: bool,
}

impl ScriptHook for KanbanBoard {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        // The columns and the card template arrive as named entries on the
        // instance, the way a list's item templates do. They are told apart
        // by TYPE rather than by name, so a caller may call its columns
        // whatever it likes.
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                let mut declared: Vec<KanbanColumn> = Vec::new();
                let mut template: Option<ScriptObjectRef> = None;
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        let Some(id) = kv.key.as_id() else {
                            continue;
                        };
                        let Some(val_obj) = kv.value.as_object() else {
                            continue;
                        };
                        if vm
                            .bx
                            .heap
                            .type_matches_id(val_obj, KanbanColumnSpec::script_type_id_static())
                        {
                            let spec = KanbanColumnSpec::script_from_value(vm, kv.value);
                            declared.push(spec.to_column());
                        } else if id == live_id!(card) {
                            template = Some(vm.bx.heap.new_object_ref(val_obj));
                        }
                    }
                });
                // Collected into locals and then swapped in whole, rather
                // than appended to what is already held: the same object can
                // be applied more than once, and a board that appended would
                // end up with each of its columns twice.
                if !declared.is_empty() || template.is_some() {
                    self.declared = declared;
                    if template.is_some() {
                        self.card_template = template;
                    }
                    self.built = false;
                }
            }
        }
        if apply.is_reload() {
            // The cards come from a template that may have just changed, so
            // they are rebuilt rather than patched — and with them goes
            // whatever the board had been dragged into, because on a reload
            // what is written down IS the board.
            self.pool.clear();
            self.warned = false;
            self.built = false;
        }
    }
}

impl KanbanBoard {
    /// Copy the declaration into the running board, once. Called at the top
    /// of both the draw and the event pass rather than from `on_after_new`,
    /// so it cannot depend on which hook the runtime calls first.
    fn ensure_built(&mut self) {
        if self.built {
            return;
        }
        self.built = true;
        self.board = KanbanBoardModel {
            columns: self.declared.clone(),
        };
        self.scroll = vec![0.0; self.board.columns.len()];
        self.drag = None;
        self.pointer = None;
    }

    /// The widget standing for the `index`th card of the board, made from
    /// the template on first use. The pool is only ever appended to, so a
    /// card widget keeps its place and its text is written every draw.
    fn card_widget(&mut self, cx: &mut Cx, index: usize) -> Option<WidgetRef> {
        if let Some(card) = self.pool.get(index) {
            return Some(card.clone());
        }
        // Taken as a value in its own statement: the template's borrow of
        // `self` has to be over before the miss below can write to `self`.
        let template: Option<ScriptValue> = self
            .card_template
            .as_ref()
            .map(|template| template.as_object().into());
        let Some(value) = template else {
            if !self.warned {
                self.warned = true;
                warning!("KanbanBoard has no `card` template, so its cards cannot be drawn");
            }
            return None;
        };
        let card = cx.with_vm(|vm| WidgetRef::script_from_value(vm, value));
        self.pool.push(card.clone());
        Some(card)
    }

    fn column_bands(&self) -> Vec<KanbanColumnBand> {
        self.geom
            .iter()
            .enumerate()
            .map(|(index, geom)| (index, geom.panel.pos.x, geom.panel.size.x))
            .collect()
    }

    /// A column's cards in the shape the row machine wants them.
    fn row_bands(&self, column: usize) -> Vec<RowBand> {
        self.card_rects
            .get(column)
            .map(|rects| {
                rects
                    .iter()
                    .enumerate()
                    .map(|(index, rect)| (index, rect.pos.y, rect.size.y))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The card under a point, if the point is on one. A press has to be on
    /// a card, so this is a containment test and not the nearest-column rule
    /// the drag uses.
    fn card_at(&self, point: DVec2) -> Option<(usize, usize)> {
        for (column, geom) in self.geom.iter().enumerate() {
            if !geom.body.contains(point) {
                continue;
            }
            let Some(rects) = self.card_rects.get(column) else {
                continue;
            };
            for (index, rect) in rects.iter().enumerate() {
                if point.y >= rect.pos.y && point.y < rect.pos.y + rect.size.y {
                    return Some((column, index));
                }
            }
        }
        None
    }

    /// The marker's rect, and whether the column would refuse the card.
    fn marker_rect(&self, drag: KanbanDrag) -> Option<(Rect, bool)> {
        let geom = self.geom.get(drag.to_column)?;
        let rects = self.card_rects.get(drag.to_column)?;
        let inset = self.card_inset.max(0.0);
        let gap = self.card_spacing * 0.5;
        let y = if let Some(rect) = rects.get(drag.slot()) {
            rect.pos.y - gap
        } else if let Some(rect) = rects.last() {
            rect.pos.y + rect.size.y + gap
        } else {
            geom.body.pos.y + inset
        };
        let thickness = self.marker_size.max(1.0);
        // The first gap sits on the column's own top edge and the last on
        // its bottom one; unclamped, the marker lands outside the column and
        // is clipped away exactly when it matters most.
        let low = geom.body.pos.y + 1.0;
        let high = (geom.body.pos.y + geom.body.size.y - thickness - 1.0).max(low);
        let y = y.clamp(low, high);
        let refused = drag.to_column != drag.from_column
            && self
                .board
                .columns
                .get(drag.to_column)
                .is_some_and(|column| column.is_full());
        Some((
            Rect {
                pos: dvec2(geom.body.pos.x + inset, y - thickness * 0.5),
                size: dvec2((geom.body.size.x - inset * 2.0).max(0.0), thickness),
            },
            refused,
        ))
    }

    /// Scroll one column by `delta`, clamped to its cards; whether it moved.
    fn scroll_column(&mut self, cx: &mut Cx, column: usize, delta: f64) -> bool {
        let room = {
            let Some(geom) = self.geom.get(column) else {
                return false;
            };
            (geom.content - geom.body.size.y).max(0.0)
        };
        let Some(scroll) = self.scroll.get_mut(column) else {
            return false;
        };
        let next = (*scroll + delta).clamp(0.0, room);
        if next != *scroll {
            *scroll = next;
            self.draw_bg.redraw(cx);
            return true;
        }
        false
    }

    /// A drag held near a column's top or bottom edge crawls it, so a column
    /// longer than the board can be reached without letting go.
    fn drag_edge_autoscroll(&mut self, cx: &mut Cx) {
        let (Some(point), Some(drag)) = (self.pointer, self.drag) else {
            return;
        };
        if !drag.active() {
            return;
        }
        let column = drag.to_column;
        let Some(geom) = self.geom.get(column).copied() else {
            return;
        };
        let top = geom.body.pos.y;
        let bottom = geom.body.pos.y + geom.body.size.y;
        let delta = if point.y < top + EDGE_BAND {
            -EDGE_CRAWL
        } else if point.y > bottom - EDGE_BAND {
            EDGE_CRAWL
        } else {
            return;
        };
        if self.scroll_column(cx, column, delta) {
            // Still moving, so ask for another frame: the pointer is being
            // held still and nothing else will come.
            self.scroll_pump = cx.new_next_frame();
        }
    }

    fn cancel_drag(&mut self, cx: &mut Cx) {
        self.drag = None;
        self.pointer = None;
        self.draw_bg.redraw(cx);
    }

    /// The board as it stands.
    pub fn columns(&self) -> &[KanbanColumn] {
        &self.board.columns
    }

    /// Put a card at the end of a column. False when the column is full,
    /// which is the same answer a drop gets.
    pub fn add_card(&mut self, cx: &mut Cx, column: usize, card: &str) -> bool {
        self.ensure_built();
        let added = self.board.push(column, card);
        if added {
            self.draw_bg.redraw(cx);
        }
        added
    }
}

impl Widget for KanbanBoard {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_built();
        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().inner_rect();
        let count = self.board.columns.len();
        // A board with no room is a board that was given none: an unknown
        // size arrives as NaN, which fails this test as surely as zero does.
        if count == 0 || !(rect.size.x > 0.0) || !(rect.size.y > 0.0) {
            self.draw_bg.end(cx);
            return DrawStep::done();
        }

        let width = kanban_column_width(
            self.column_width,
            self.min_column_width,
            rect.size.x,
            self.column_gap,
            count,
        );
        let header = self.header_height.max(0.0).min(rect.size.y);
        let inset = self.card_inset.max(0.0);
        let drag = self.drag.filter(|drag| drag.active());

        let mut geom: Vec<ColumnGeom> = Vec::with_capacity(count);
        let mut card_rects: Vec<Vec<Rect>> = Vec::with_capacity(count);
        let mut pool_at = 0;

        for index in 0..count {
            let x = rect.pos.x + (width + self.column_gap) * index as f64;
            let panel = Rect {
                pos: dvec2(x, rect.pos.y),
                size: dvec2(width, rect.size.y),
            };
            let body = Rect {
                pos: dvec2(x, rect.pos.y + header),
                size: dvec2(width, (rect.size.y - header).max(0.0)),
            };

            let heading = self.board.columns[index].heading.clone();
            let cards = self.board.columns[index].cards.clone();
            let count_text = self.board.columns[index].count_text();
            let full = self.board.columns[index].is_full();

            self.draw_column.target = if drag.is_some_and(|drag| drag.to_column == index) {
                1.0
            } else {
                0.0
            };
            self.draw_column.full = if full { 1.0 } else { 0.0 };
            self.draw_column.header_px = header as f32;
            self.draw_column.draw_abs(cx, panel);

            // The heading band, clipped to itself: a long heading running
            // into the next column would be read as that column's.
            cx.push_clip_rect(Rect {
                pos: panel.pos,
                size: dvec2(width, header),
            });
            let size = self.draw_heading.text_style.font_size as f64;
            let y = centred_baseline(panel.pos.y, header, size);
            self.draw_heading
                .draw_abs(cx, dvec2(panel.pos.x + inset, y), &heading);

            let size = self.draw_count.text_style.font_size as f64;
            let y = centred_baseline(panel.pos.y, header, size);
            let text_width = measure(&self.draw_count, cx, &count_text);
            let mut right = panel.pos.x + width - inset - text_width;
            self.draw_count.draw_abs(cx, dvec2(right, y), &count_text);

            if full && !self.full_text.is_empty() {
                let word = self.full_text.clone();
                let size = self.draw_full.text_style.font_size as f64;
                let y = centred_baseline(panel.pos.y, header, size);
                let word_width = measure(&self.draw_full, cx, &word);
                right -= word_width + self.card_spacing;
                self.draw_full.draw_abs(cx, dvec2(right, y), &word);
            }
            cx.pop_clip_rect();

            // The column's own scroll, clamped against the height the cards
            // came out at LAST draw: this one has not happened yet.
            let drawn_height = self.geom.get(index).map_or(0.0, |geom| geom.content);
            let room = (drawn_height - body.size.y).max(0.0);
            let offset = self
                .scroll
                .get(index)
                .copied()
                .unwrap_or(0.0)
                .clamp(0.0, room);
            if let Some(scroll) = self.scroll.get_mut(index) {
                *scroll = offset;
            }

            let body_layout = Layout {
                flow: Flow::Down,
                spacing: self.card_spacing,
                padding: Inset {
                    left: inset,
                    top: inset,
                    right: inset,
                    bottom: inset,
                },
                scroll: dvec2(0.0, offset),
                clip_x: true,
                clip_y: true,
                ..Layout::default()
            };
            cx.begin_turtle(Walk::abs_rect(body), body_layout);
            let mut rects = Vec::with_capacity(cards.len());
            for text in &cards {
                let Some(card) = self.card_widget(cx.cx.cx, pool_at) else {
                    break;
                };
                // A tree node under the board every draw, so a test and the
                // design overlay can reach a card by name rather than by
                // hunting for a rect — and so the title below can be found
                // inside it.
                cx.widget_tree_insert_child(
                    self.uid,
                    LiveId::from_str_num("card", pool_at as u64),
                    card.clone(),
                );
                pool_at += 1;
                let title = card.widget(cx.cx.cx, ids!(title));
                if title.is_empty() {
                    if !self.warned {
                        self.warned = true;
                        warning!("KanbanBoard's `card` template holds no `title`, so a card cannot be given its text");
                    }
                } else {
                    title.set_text(cx.cx.cx, text);
                }
                // Where the card went, taken from the turtle rather than
                // from the card's area: an area is only resolved once the
                // pass is over, and the drag needs these now.
                let top = cx.turtle().pos().y;
                let card_walk = card.walk(cx.cx.cx);
                card.draw_walk_all(cx, scope, card_walk);
                let bottom = cx.turtle().pos().y;
                rects.push(Rect {
                    pos: dvec2(body.pos.x + inset, top),
                    size: dvec2(
                        (body.size.x - inset * 2.0).max(0.0),
                        (bottom - top).max(0.0),
                    ),
                });
            }
            let content = cx.turtle().used_height() + inset;
            cx.end_turtle();

            geom.push(ColumnGeom {
                panel,
                body,
                content,
            });
            card_rects.push(rects);
        }
        self.geom = geom;
        self.card_rects = card_rects;

        if let Some(drag) = drag {
            // The card being carried is still lying where it was, so it is
            // washed over: without that, the only sign of what is in the
            // hand is a marker somewhere else.
            let lift = self
                .card_rects
                .get(drag.from_column)
                .and_then(|rects| rects.get(drag.from()))
                .copied();
            let body = self.geom.get(drag.from_column).map(|geom| geom.body);
            if let (Some(rect), Some(body)) = (lift, body) {
                cx.push_clip_rect(body);
                self.draw_lift.draw_abs(cx, rect);
                cx.pop_clip_rect();
            }
            let marker = self.marker_rect(drag);
            let body = self.geom.get(drag.to_column).map(|geom| geom.body);
            if let (Some((rect, refused)), Some(body)) = (marker, body) {
                cx.push_clip_rect(body);
                if refused {
                    self.draw_refusal.draw_abs(cx, rect);
                } else {
                    self.draw_marker.draw_abs(cx, rect);
                }
                cx.pop_clip_rect();
            }
        }

        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ensure_built();
        let uid = self.uid;

        // The cards first: a control inside a card is still a control, and a
        // press it takes marks the event handled, so the board never sees
        // it. During a drag they are handed nothing at all — the gesture is
        // modal.
        if self.drag.is_none() {
            let drawn = self.board.card_count();
            for card in self.pool.iter().take(drawn) {
                card.handle_event(cx, event, scope);
            }
        } else {
            if let Event::KeyDown(key) = event {
                if key.key_code == KeyCode::Escape {
                    self.cancel_drag(cx);
                    return;
                }
            }
            // Cards slide under a HELD pointer, so each frame re-derives the
            // gap from the fresh geometry and keeps the crawl going.
            if self.scroll_pump.is_event(event).is_some() {
                if let (Some(mut drag), Some(point)) = (self.drag, self.pointer) {
                    let rows = self.row_bands(drag.to_column);
                    if drag.move_to(
                        point.x,
                        point.y,
                        self.drag_threshold,
                        drag.to_column,
                        &rows,
                    ) {
                        self.draw_bg.redraw(cx);
                    }
                    self.drag = Some(drag);
                }
                self.drag_edge_autoscroll(cx);
            }
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerScroll(_) if event.scroll_handled(Vec2Index::Y) => {}
            Hit::FingerScroll(e) => {
                // A live drag is modal for the columns: a wheel would slide
                // the cards away under the pointer mid-gesture, and so would a
                // page around the board scrolling by it.
                if self.drag.is_some() {
                    event.set_scroll_handled(Vec2Index::Y);
                    return;
                }
                // A column the wheel moves keeps it, so the page stays put; a
                // column resting on the edge the wheel points past hands it on.
                let bands = self.column_bands();
                if let Some(column) = kanban_column_at(&bands, e.abs.x) {
                    if self.scroll_column(cx, column, e.scroll.y) {
                        event.set_scroll_handled(Vec2Index::Y);
                    }
                }
            }
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                if self.card_at(e.abs).is_some() {
                    cx.set_cursor(MouseCursor::Grab);
                }
            }
            Hit::FingerDown(e) if e.device.is_primary_hit() => {
                if let Some((column, card)) = self.card_at(e.abs) {
                    self.drag = Some(KanbanDrag::press(column, card, e.abs.x, e.abs.y));
                    self.pointer = Some(e.abs);
                }
            }
            Hit::FingerMove(e) => {
                let Some(mut drag) = self.drag else {
                    return;
                };
                self.pointer = Some(e.abs);
                let bands = self.column_bands();
                let column = kanban_column_at(&bands, e.abs.x).unwrap_or(drag.to_column);
                let rows = self.row_bands(column);
                if drag.move_to(e.abs.x, e.abs.y, self.drag_threshold, column, &rows) {
                    self.draw_bg.redraw(cx);
                }
                if drag.active() {
                    cx.set_cursor(MouseCursor::Grabbing);
                }
                self.drag = Some(drag);
                self.drag_edge_autoscroll(cx);
            }
            Hit::FingerUp(fe) => {
                let Some(drag) = self.drag.take() else {
                    return;
                };
                self.pointer = None;
                // Taken away: the card goes back where it was, nothing moves.
                if fe.cancelled {
                    self.draw_bg.redraw(cx);
                    return;
                }
                if let Some((from_column, from, to_column, slot)) = drag.commit() {
                    match self.board.apply(from_column, from, to_column, slot) {
                        KanbanLanding::Moved { to } => {
                            cx.widget_action(
                                uid,
                                KanbanBoardAction::Moved(KanbanMove {
                                    from_column,
                                    from,
                                    to_column,
                                    to,
                                }),
                            );
                        }
                        KanbanLanding::Refused => {
                            cx.widget_action(
                                uid,
                                KanbanBoardAction::Refused { column: to_column },
                            );
                        }
                        KanbanLanding::Nothing => {}
                    }
                }
                self.draw_bg.redraw(cx);
            }
            _ => (),
        }
    }
}

impl KanbanBoardRef {
    /// The move this actions pass reported. The board has already made it.
    pub fn moved(&self, actions: &Actions) -> Option<KanbanMove> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            KanbanBoardAction::Moved(moved) => Some(moved),
            _ => None,
        }
    }

    /// The column that turned a card away, because it is full.
    pub fn refused(&self, actions: &Actions) -> Option<usize> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            KanbanBoardAction::Refused { column } => Some(column),
            _ => None,
        }
    }

    /// A column's cards, top first.
    pub fn cards(&self, column: usize) -> Vec<String> {
        // The borrow is bound to a name before anything is read through it:
        // one taken off a temporary in the middle of an expression dies
        // before the expression ends.
        if let Some(inner) = self.borrow() {
            return inner
                .columns()
                .get(column)
                .map(|column| column.cards.clone())
                .unwrap_or_default();
        }
        Vec::new()
    }

    /// A column's heading, for a host saying where a card went.
    pub fn heading(&self, column: usize) -> String {
        if let Some(inner) = self.borrow() {
            return inner
                .columns()
                .get(column)
                .map(|column| column.heading.clone())
                .unwrap_or_default();
        }
        String::new()
    }

    /// Put a card at the end of a column. False when the column is full.
    pub fn add_card(&self, cx: &mut Cx, column: usize, card: &str) -> bool {
        if let Some(mut inner) = self.borrow_mut() {
            return inner.add_card(cx, column, card);
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(heading: &str, cap: usize, cards: &[&str]) -> KanbanColumn {
        KanbanColumn {
            heading: heading.to_string(),
            cap,
            cards: cards.iter().map(|card| card.to_string()).collect(),
        }
    }

    /// Three columns: a loose one, one that will take three, and one that is
    /// already at its cap of two.
    fn sample_board() -> KanbanBoardModel {
        KanbanBoardModel {
            columns: vec![
                column("To do", 0, &["a", "b", "c"]),
                column("Doing", 3, &["d"]),
                column("Done", 2, &["e", "f"]),
            ],
        }
    }

    fn cards(board: &KanbanBoardModel, column: usize) -> Vec<&str> {
        board.columns[column]
            .cards
            .iter()
            .map(|card| card.as_str())
            .collect()
    }

    #[test]
    fn a_card_moved_down_its_own_column_gives_back_the_gap_it_left() {
        let mut board = sample_board();
        // "a" is carried to the gap under "c", which is gap 3 while "a" is
        // still lying in the column.
        assert_eq!(board.apply(0, 0, 0, 3), KanbanLanding::Moved { to: 2 });
        assert_eq!(cards(&board, 0), vec!["b", "c", "a"]);
    }

    #[test]
    fn a_card_moved_up_its_own_column_lands_in_the_gap_itself() {
        let mut board = sample_board();
        assert_eq!(board.apply(0, 2, 0, 0), KanbanLanding::Moved { to: 0 });
        assert_eq!(cards(&board, 0), vec!["c", "a", "b"]);
    }

    #[test]
    fn a_card_dropped_back_where_it_lies_moves_nothing() {
        let mut board = sample_board();
        // Its own gap, and the gap directly under it: both are where it
        // already is.
        assert_eq!(board.apply(0, 1, 0, 1), KanbanLanding::Nothing);
        assert_eq!(board.apply(0, 1, 0, 2), KanbanLanding::Nothing);
        assert_eq!(cards(&board, 0), vec!["a", "b", "c"]);
    }

    #[test]
    fn a_card_crosses_to_another_column_at_the_gap_it_was_dropped_in() {
        let mut board = sample_board();
        assert_eq!(board.apply(0, 1, 1, 0), KanbanLanding::Moved { to: 0 });
        assert_eq!(cards(&board, 0), vec!["a", "c"]);
        assert_eq!(cards(&board, 1), vec!["b", "d"]);
    }

    #[test]
    fn a_card_dropped_past_the_last_one_lands_at_the_end() {
        let mut board = sample_board();
        // Past the end of another column: the gap is that column's length.
        assert_eq!(board.apply(0, 0, 1, 1), KanbanLanding::Moved { to: 1 });
        assert_eq!(cards(&board, 1), vec!["d", "a"]);
        // And past the end of its own, which counts itself and gives it
        // back.
        let mut board = sample_board();
        assert_eq!(board.apply(0, 0, 0, 3), KanbanLanding::Moved { to: 2 });
        assert_eq!(cards(&board, 0), vec!["b", "c", "a"]);
    }

    #[test]
    fn a_gap_past_the_end_of_a_short_column_is_still_the_end() {
        let mut board = sample_board();
        // A pointer below every card in a nearly empty column can name a gap
        // beyond it; the card lands at the end rather than nowhere.
        assert_eq!(board.apply(0, 0, 1, 9), KanbanLanding::Moved { to: 1 });
        assert_eq!(cards(&board, 1), vec!["d", "a"]);
    }

    #[test]
    fn a_full_column_refuses_a_card_from_another_one() {
        let mut board = sample_board();
        assert_eq!(board.apply(0, 0, 2, 1), KanbanLanding::Refused);
        assert_eq!(cards(&board, 0), vec!["a", "b", "c"], "the card stayed put");
        assert_eq!(cards(&board, 2), vec!["e", "f"]);
    }

    #[test]
    fn a_full_column_still_puts_its_own_cards_in_order() {
        // A cap is about how much work is in flight, not about whether the
        // column may be tidied.
        let mut board = sample_board();
        assert_eq!(board.apply(2, 0, 2, 2), KanbanLanding::Moved { to: 1 });
        assert_eq!(cards(&board, 2), vec!["f", "e"]);
    }

    #[test]
    fn a_column_under_its_cap_takes_one_more_and_then_is_full() {
        let mut board = sample_board();
        assert!(board.push(1, "g"), "the second of three");
        assert!(board.push(1, "h"), "the third");
        assert!(!board.push(1, "i"), "and no more");
        assert_eq!(cards(&board, 1), vec!["d", "g", "h"]);
        assert!(board.columns[1].is_full());
        assert!(board.push(0, "j"), "an uncapped column takes whatever it is given");
    }

    #[test]
    fn a_drop_that_names_nothing_that_exists_does_nothing() {
        let mut board = sample_board();
        assert_eq!(board.apply(9, 0, 0, 0), KanbanLanding::Nothing);
        assert_eq!(board.apply(0, 9, 0, 0), KanbanLanding::Nothing);
        assert_eq!(board.apply(0, 0, 9, 0), KanbanLanding::Nothing);
    }

    #[test]
    fn a_count_says_the_cap_only_when_there_is_one() {
        let board = sample_board();
        assert_eq!(board.columns[0].count_text(), "3");
        assert_eq!(board.columns[1].count_text(), "1 / 3");
        assert!(!board.columns[1].is_full());
        assert!(board.columns[2].is_full());
    }

    /// Three columns 200 wide with a 10 gap, at x = 0, 210, 420.
    fn bands() -> Vec<KanbanColumnBand> {
        (0..3).map(|i| (i, i as f64 * 210.0, 200.0)).collect()
    }

    #[test]
    fn the_pointer_belongs_to_the_column_it_is_in() {
        let bands = bands();
        assert_eq!(kanban_column_at(&bands, 0.0), Some(0));
        assert_eq!(kanban_column_at(&bands, 199.0), Some(0));
        assert_eq!(kanban_column_at(&bands, 250.0), Some(1));
        assert_eq!(kanban_column_at(&bands, 500.0), Some(2));
    }

    #[test]
    fn a_pointer_between_or_beyond_the_columns_takes_the_nearest() {
        let bands = bands();
        assert_eq!(kanban_column_at(&bands, 202.0), Some(0), "in the gap, nearer the first");
        assert_eq!(kanban_column_at(&bands, 208.0), Some(1), "nearer the second");
        assert_eq!(kanban_column_at(&bands, -50.0), Some(0), "off the left");
        assert_eq!(kanban_column_at(&bands, 5000.0), Some(2), "off the right");
        assert_eq!(kanban_column_at(&[], 10.0), None, "no columns, no answer");
    }

    #[test]
    fn columns_keep_their_width_until_they_do_not_fit_and_are_then_squeezed() {
        assert_eq!(kanban_column_width(200.0, 120.0, 800.0, 10.0, 3), 200.0);
        // (500 - 20) / 3 = 160, which is above the least.
        assert_eq!(kanban_column_width(200.0, 120.0, 500.0, 10.0, 3), 160.0);
        // Past the least they stop shrinking and the board clips instead.
        assert_eq!(kanban_column_width(200.0, 120.0, 300.0, 10.0, 5), 120.0);
        assert_eq!(kanban_column_width(200.0, 120.0, 0.0, 10.0, 0), 200.0);
    }

    /// Four cards of height 60 starting at y = 100.
    fn rows() -> Vec<RowBand> {
        (0..4).map(|i| (i, 100.0 + i as f64 * 60.0, 60.0)).collect()
    }

    #[test]
    fn a_press_that_travels_sideways_still_becomes_a_drag() {
        // The row machine alone watches y only, and a card carried straight
        // across to the next column never moves up or down at all.
        let rows = rows();
        let mut drag = KanbanDrag::press(0, 1, 300.0, 170.0);
        assert!(!drag.active());
        assert!(!drag.move_to(302.0, 170.0, 4.0, 0, &rows), "2 points is still a press");
        assert!(drag.move_to(340.0, 170.0, 4.0, 1, &rows), "sideways travel wakes it");
        assert!(drag.active());
        assert_eq!(drag.to_column, 1);
    }

    #[test]
    fn the_gap_follows_the_pointer_and_the_release_names_it() {
        let rows = rows();
        let mut drag = KanbanDrag::press(0, 0, 300.0, 110.0);
        drag.move_to(300.0, 255.0, 4.0, 0, &rows);
        assert_eq!(drag.slot(), 3, "past card 2's midpoint: the gap before card 3");
        assert_eq!(drag.commit(), Some((0, 0, 0, 3)));
        // And what the board makes of that: gap 3 of its own column, with
        // the card itself counted in it, is index 2.
        let mut board = sample_board();
        assert_eq!(board.apply(0, 0, 0, 3), KanbanLanding::Moved { to: 2 });
    }

    #[test]
    fn a_drag_into_an_empty_column_lands_at_its_top() {
        let mut drag = KanbanDrag::press(0, 2, 300.0, 220.0);
        assert!(drag.move_to(600.0, 220.0, 4.0, 2, &[]));
        assert_eq!(drag.slot(), 0, "an empty column has one gap");
        assert_eq!(drag.commit(), Some((0, 2, 2, 0)));
    }

    #[test]
    fn a_press_that_never_travelled_commits_nothing() {
        let rows = rows();
        let mut drag = KanbanDrag::press(1, 0, 300.0, 110.0);
        drag.move_to(301.0, 112.0, 4.0, 1, &rows);
        assert_eq!(drag.commit(), None);
    }

    #[test]
    fn only_real_changes_are_reported() {
        let rows = rows();
        let mut drag = KanbanDrag::press(0, 1, 300.0, 170.0);
        assert!(drag.move_to(300.0, 200.0, 4.0, 0, &rows), "woken, and the gap moved");
        assert!(!drag.move_to(300.0, 201.0, 4.0, 0, &rows), "same gap, same column");
        assert!(drag.move_to(300.0, 260.0, 4.0, 0, &rows), "a new gap");
        assert!(drag.move_to(560.0, 260.0, 4.0, 1, &rows), "a new column");
    }

    #[test]
    fn a_line_of_text_sits_centred_in_its_band() {
        // The call takes the top of the line box and the ink starts below
        // it, so the drop comes off the middle rather than being left in it.
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(near(centred_baseline(0.0, 26.0, 10.0), 8.0 - 10.0 * INK_TOP));
        assert!(near(centred_baseline(100.0, 10.0, 10.0), 100.0 - 10.0 * INK_TOP));
    }
}
