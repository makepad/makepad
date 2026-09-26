use {
    crate::{
        diagonal_text::{diagonal_row_height, draw_diagonal_name, DiagonalLean},
        table::heading_fits_flat,
        flat_list::WidgetItem,
        makepad_derive_widget::*,
        makepad_draw::text::selection::Cursor,
        makepad_draw::*,
        scroll_bar::{ScrollAxis, ScrollBar, ScrollBarAction},
        text_input::TextInputWidgetRefExt,
        tip::TipAction,
        widget::*,
        widget_async::CxSplashVmExt,
        widget_tree::CxWidgetExt,
    },
    std::collections::HashMap,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawDataGridCell::script_shader(vm)){
        ..mod.draw.DrawQuad
        border_color: uniform(#d4d4d4)
        border_size: uniform(1.0)
        pixel: fn(){
            let px = self.border_size / self.rect_size.x
            let py = self.border_size / self.rect_size.y
            if self.pos.x >= 1.0 - px || self.pos.y >= 1.0 - py {
                return vec4(self.border_color.rgb * self.border_color.a, self.border_color.a)
            }
            return vec4(self.color.rgb * self.color.a, self.color.a)
        }
    }

    mod.widgets.GridSelectMode = #(GridSelectMode::script_api(vm))

    mod.widgets.DataGridBase = #(DataGrid::register_widget(vm))

    mod.widgets.DataGrid = set_type_default() do mod.widgets.DataGridBase {
        width: Fill
        height: Fill

        color_bg: #fafafa
        color_cell: #ffffff
        color_cell_alt: #f5f6f8
        color_text: #202020
        color_header: #f1f3f4
        color_header_active: #xd7e3fc
        color_header_text: #444444
        // Transparent: the sorted heading is drawn like the others.
        color_header_sorted: #x00000000
        color_selection: #x4285f41f
        color_selection_border: #x1a73e8
        color_drag_marker: #x1a73e8
        color_resize_guide: #x1a73e866
        color_header_edge: #x9aa0a6

        /** how far a heading too long for its column is turned, in degrees; one that fits stays flat, and 0 turns none 0..80 step 5 */
        header_angle: 0.
        /** which way a turned heading leans: Rise hangs it over the right, Fall over the left */
        header_lean: DiagonalLean.Rise

        draw_text +: {
            text_style: theme.font_regular{font_size: 9.0}
            color: #202020
        }
        draw_text_bold +: {
            text_style: theme.font_bold{font_size: 9.0}
            color: #202020
        }
        // Size 0: the headings are drawn in draw_text's style. A size of
        // its own gives them a face and size of their own.
        draw_text_header +: {
            text_style: theme.font_regular{font_size: 0.0}
        }
        scroll_bar_h: mod.widgets.ScrollBar{
            draw_bg +: {
                color: uniform(#x00000038)
                color_hover: uniform(#x00000060)
                color_drag: uniform(#x00000085)
            }
        }
        scroll_bar_v: mod.widgets.ScrollBar{
            draw_bg +: {
                color: uniform(#x00000038)
                color_hover: uniform(#x00000060)
                color_drag: uniform(#x00000085)
            }
        }

        // The editor edit_cell seats: a text field that fills the cell it
        // is drawn into, in the grid's own type size. A grid that wants
        // another look declares its own Editor := TextInput{...} and that
        // one takes this one's place; whatever it looks like it has to be
        // a TextInput, since Return, Escape and the loss of the keyboard
        // are read from it.
        Editor := mod.widgets.TextInput{
            width: Fill
            height: Fill
            margin: 0.
            padding: Inset{left: 4, right: 4, top: 5, bottom: 3}
            empty_text: ""
            draw_bg +: {
                border_radius: 0.
                border_size: 2.0
            }
            draw_text +: {
                text_style: theme.font_regular{font_size: 9.0}
            }
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawDataGridCell {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
}

/// Per-axis size table: a uniform default size plus a sparse, sorted set of
/// overrides. Offset/index queries are O(log overrides) on top of simple
/// multiplication, so axes with millions of entries cost nothing until
/// individual rows/columns actually get resized.
#[derive(Default)]
pub struct AxisSizes {
    default: f64,
    /// Sorted by index: (index, size)
    overrides: Vec<(usize, f64)>,
    /// cum[i] = sum of (size - default) over overrides[0..=i]
    cum: Vec<f64>,
}

impl AxisSizes {
    fn new(default: f64) -> Self {
        Self {
            default,
            overrides: Vec::new(),
            cum: Vec::new(),
        }
    }

    fn set_default(&mut self, default: f64) {
        if self.default != default {
            self.default = default;
            self.rebuild_cum();
        }
    }

    fn rebuild_cum(&mut self) {
        self.cum.clear();
        let mut acc = 0.0;
        for (_, size) in &self.overrides {
            acc += size - self.default;
            self.cum.push(acc);
        }
    }

    /// Number of overrides with index < i, via binary search.
    fn overrides_before(&self, i: usize) -> usize {
        self.overrides.partition_point(|(idx, _)| *idx < i)
    }

    pub fn size_of(&self, i: usize) -> f64 {
        match self.overrides.binary_search_by_key(&i, |(idx, _)| *idx) {
            Ok(pos) => self.overrides[pos].1,
            Err(_) => self.default,
        }
    }

    pub fn offset_of(&self, i: usize) -> f64 {
        let n = self.overrides_before(i);
        let extra = if n == 0 { 0.0 } else { self.cum[n - 1] };
        i as f64 * self.default + extra
    }

    pub fn total(&self, count: usize) -> f64 {
        self.offset_of(count)
    }

    /// Find the entry containing `pos`, returning (index, offset_within).
    pub fn index_at(&self, pos: f64, count: usize) -> (usize, f64) {
        if count == 0 || pos <= 0.0 {
            return (0, 0.0);
        }
        let mut lo = 0usize;
        let mut hi = count - 1;
        while lo < hi {
            let mid = lo + (hi - lo + 1) / 2;
            if self.offset_of(mid) <= pos {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        (lo, pos - self.offset_of(lo))
    }

    pub fn set(&mut self, i: usize, size: f64) {
        match self.overrides.binary_search_by_key(&i, |(idx, _)| *idx) {
            Ok(pos) => self.overrides[pos].1 = size,
            Err(pos) => self.overrides.insert(pos, (i, size)),
        }
        self.rebuild_cum();
    }

    pub fn clear_overrides(&mut self) {
        self.overrides.clear();
        self.cum.clear();
    }

    /// Replace every override with `sizes`, one per index from zero.
    /// Returns whether anything changed.
    fn set_all(&mut self, sizes: impl Iterator<Item = f64>) -> bool {
        let overrides: Vec<(usize, f64)> = sizes.enumerate().collect();
        if overrides == self.overrides {
            return false;
        }
        self.overrides = overrides;
        self.rebuild_cum();
        true
    }

    /// Move every override to the index `to` gives it, for a host that
    /// puts the columns in another order at once.
    fn remap(&mut self, to: impl Fn(usize) -> usize) {
        for (idx, _) in self.overrides.iter_mut() {
            *idx = to(*idx);
        }
        self.overrides.sort_by_key(|(idx, _)| *idx);
        self.rebuild_cum();
    }

    /// Remap override indices after a column move: the entry at `from` lands
    /// at `to`, entries between shift by one.
    fn apply_move(&mut self, from: usize, to: usize) {
        if from == to {
            return;
        }
        for (idx, _) in self.overrides.iter_mut() {
            let i = *idx;
            *idx = if i == from {
                to
            } else if from < to && i > from && i <= to {
                i - 1
            } else if to < from && i >= to && i < from {
                i + 1
            } else {
                i
            };
        }
        self.overrides.sort_by_key(|(idx, _)| *idx);
        self.rebuild_cum();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GridSelectKind {
    Cells,
    Rows,
    Cols,
    All,
}

/// A rectangular selection between `anchor` and `head`, both (row, display_col).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridSelection {
    pub kind: GridSelectKind,
    pub anchor: (usize, usize),
    pub head: (usize, usize),
}

impl GridSelection {
    pub fn single(row: usize, display_col: usize) -> Self {
        Self {
            kind: GridSelectKind::Cells,
            anchor: (row, display_col),
            head: (row, display_col),
        }
    }

    pub fn row_range(&self) -> (usize, usize) {
        (
            self.anchor.0.min(self.head.0),
            self.anchor.0.max(self.head.0),
        )
    }

    pub fn col_range(&self) -> (usize, usize) {
        (
            self.anchor.1.min(self.head.1),
            self.anchor.1.max(self.head.1),
        )
    }

    pub fn contains(&self, row: usize, display_col: usize) -> bool {
        let (r0, r1) = self.row_range();
        let (c0, c1) = self.col_range();
        match self.kind {
            GridSelectKind::All => true,
            GridSelectKind::Rows => row >= r0 && row <= r1,
            GridSelectKind::Cols => display_col >= c0 && display_col <= c1,
            GridSelectKind::Cells => {
                row >= r0 && row <= r1 && display_col >= c0 && display_col <= c1
            }
        }
    }

    pub fn contains_row(&self, row: usize) -> bool {
        let (r0, r1) = self.row_range();
        match self.kind {
            GridSelectKind::All | GridSelectKind::Cols => true,
            _ => row >= r0 && row <= r1,
        }
    }

    pub fn contains_col(&self, display_col: usize) -> bool {
        let (c0, c1) = self.col_range();
        match self.kind {
            GridSelectKind::All | GridSelectKind::Rows => true,
            _ => display_col >= c0 && display_col <= c1,
        }
    }
}

/// What the grid selects when it is pressed or steered with the keys,
/// declared as `selection:`. A list is not a spreadsheet: its rows are
/// picked whole, and a list whose host keeps the picks itself wants the
/// grid to pick nothing at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Script, ScriptHook)]
pub enum GridSelectMode {
    /// The spreadsheet: a press selects a cell and a drag a rectangle,
    /// a row number selects its row and a heading its column.
    #[pick]
    #[default]
    Cells,
    /// A press or an arrow key selects the whole row, and shift extends
    /// by rows. A heading press sorts but never selects a column, and no
    /// cell is drawn as the active one.
    Rows,
    /// The grid never selects: no overlay, no rubber band, no heading
    /// lit. A press still raises `CellClicked` with its modifiers, for a
    /// host that keeps its own picks and outlines its own cursor. Nor does
    /// it hold a selection: one left from another mode, or set from code,
    /// is dropped, so Delete, typing and copying find nothing to act on.
    Off,
}

#[derive(Clone, Debug, Default)]
pub enum DataGridAction {
    #[default]
    None,
    /// The viewport scrolled; at most once per drawn frame.
    Scrolled,
    CellClicked {
        row: usize,
        col: usize,
        modifiers: KeyModifiers,
    },
    CellDoubleClicked {
        row: usize,
        col: usize,
    },
    /// A primary press on a cell came back up without the pointer ever
    /// going further than `drag_threshold` from where it went down: a
    /// click, finished. Raised after the press's `CellClicked`, whatever
    /// `row_drag` says, with the keys held as it came up. A host that acts
    /// on the release — opening what was clicked, say — reads this, so a
    /// press that turns into a drag never acts.
    ///
    /// `row` is the line pressed, as the rows stood at the press. The grid
    /// holds no data, so a host whose rows can move under a held press,
    /// sorted again or filled in, acts on the item it found on that line
    /// at `CellClicked`, not on whatever the line draws now. A line the
    /// rows no longer reach raises no release.
    CellReleased {
        row: usize,
        col: usize,
        modifiers: KeyModifiers,
    },
    /// With `row_drag` on, a press on a cell travelled past
    /// `drag_threshold`: the row is being carried. Raised once per press,
    /// with `abs` where the pointer is as the carry begins and the keys
    /// held at the press. From then on the grid leaves the press alone: it
    /// selects nothing, does not scroll unless `row_drag_scroll` is on, and
    /// raises no `CellReleased` when the press comes up. Where the row goes
    /// is the host's to follow, from `RowDragMoved` and `RowDragEnded`.
    ///
    /// `row` is the line pressed, as the rows stood at the press, like
    /// `CellReleased`'s: a host whose rows moved since `CellClicked` carries
    /// the item it found then. A line the rows no longer reach carries
    /// nothing.
    RowDragStarted {
        row: usize,
        col: usize,
        abs: DVec2,
        modifiers: KeyModifiers,
    },
    /// A carried row's pointer is at `abs`, in the pointer's coordinates,
    /// wherever it has gone: raised for every move after the one that
    /// started the carry, and for every step `row_drag_scroll` scrolls the
    /// rows under a pointer that holds still, with the pointer where it
    /// is. A host that reorders its rows under the pointer asks
    /// [`DataGrid::row_gap_at`] where the row belongs now.
    RowDragMoved {
        abs: DVec2,
    },
    /// The carried row was let go at `abs`, with the keys held as the press
    /// came up. Raised once, and only for a press that carried its row.
    RowDragEnded {
        abs: DVec2,
        modifiers: KeyModifiers,
    },
    /// A secondary press, the right button, on a cell, with `abs` where it
    /// went down: where a host opens a menu for the row. Only the secondary
    /// button asks. Ctrl with the primary button is still a primary press,
    /// since that is how a list toggles its picks. The press selects
    /// nothing and takes no keyboard focus, so a menu opened on a row
    /// finds the selection, and the picks a host keeps, as they were.
    CellContextMenu {
        row: usize,
        col: usize,
        abs: DVec2,
    },
    /// The same, on a column heading: `col` is the data column. Nothing is
    /// sorted.
    HeaderContextMenu {
        col: usize,
        abs: DVec2,
    },
    /// A heading was pressed and the sort moved on. `ascending` is
    /// `None` when the column has cycled back to unsorted.
    ///
    /// The grid does NOT reorder anything: the rows belong to the host,
    /// which fills cells on demand and is the only thing that can put
    /// them in a different order. This says what the person asked for.
    SortChanged {
        col: usize,
        ascending: Option<bool>,
    },
    HeaderClicked {
        col: usize,
        display_col: usize,
        modifiers: KeyModifiers,
    },
    ColumnResized {
        col: usize,
        display_col: usize,
        width: f64,
    },
    RowResized {
        row: usize,
        height: f64,
    },
    ColumnMoved {
        from_display: usize,
        to_display: usize,
    },
    /// A heading was dragged somewhere else and dropped: `order` is the
    /// data column now drawn at each display position, left to right.
    /// Raised after `ColumnMoved`, and only for a drag, never for
    /// [`DataGrid::set_col_order`]. It is what a host keeps to put the
    /// columns back later with `set_col_order`.
    ColumnOrderChanged {
        order: Vec<usize>,
    },
    SelectionChanged {
        selection: Option<GridSelection>,
    },
    /// The person asked to edit a cell: F2, Return or a double-click with
    /// `replace: None`, or typed straight over it with the typed text in
    /// `replace`. The grid seats nothing yet. It holds no data, so only
    /// the host knows what the cell says, and the host answers with
    /// [`DataGrid::edit_cell`] and the text to start from — the cell's own
    /// to amend it, or `replace` to replace it. A host that does not
    /// answer has a read-only grid, which is what an unanswered request
    /// ought to be.
    EditCell {
        row: usize,
        col: usize,
        replace: Option<String>,
    },
    /// The seated editor closed with a value: Return, Tab, a click
    /// somewhere else, or the host asking. The grid has already put the
    /// editor away and, for a key, moved the selection on; the text is the
    /// host's to keep or to refuse. Refusing is nothing more than not
    /// writing it down — the cell redraws with what the host still has —
    /// and a host that wants the person to try again reopens the editor
    /// with [`DataGrid::edit_cell`] and the refused text.
    CellEdited {
        row: usize,
        col: usize,
        text: String,
    },
    /// The seated editor was abandoned with Escape; the cell keeps what it
    /// had.
    EditCancelled {
        row: usize,
        col: usize,
    },
    /// Delete/Backspace pressed with a selection active.
    ClearCells,
}

/// One visible cell handed out by [`DataGrid::next_cell`] during drawing.
/// `col` is the data column (survives reordering), `display_col` the visual one.
#[derive(Clone, Copy, Debug)]
pub struct GridCell {
    pub row: usize,
    pub col: usize,
    pub display_col: usize,
    pub rect: Rect,
}

/// Styling for the fast text-cell path.
#[derive(Clone, Copy)]
pub struct CellStyle {
    pub bg: Option<Vec4f>,
    pub color: Option<Vec4f>,
    /// 0.0 = left, 0.5 = center, 1.0 = right
    pub align: f64,
    pub bold: bool,
    pub font_scale: f64,
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            bg: None,
            color: None,
            align: 0.0,
            bold: false,
            font_scale: 1.0,
        }
    }
}

/// How one column heading is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HeaderLook {
    /// The label's colour, and the marks'.
    color: Vec4f,
    /// Where the label sits across the heading, 0.0 left to 1.0 right.
    align: f64,
    /// The sort marks after the label, and whether they are worn faded.
    mark: Option<(&'static str, bool)>,
}

/// Where a heading label `tw` wide starts in `rect`: `align` of the way
/// across the room inside the padding. Past the middle the room loses
/// `reserve` at the right, the space the sort marks take, a little more
/// the further right the label goes and all of it at 1.0, so a label
/// aligned right ends before the marks and the centred default is where
/// it always was. A label with no room starts at the left padding, as a
/// cell's does.
fn header_label_x(rect: Rect, pad: f64, tw: f64, align: f64, reserve: f64) -> f64 {
    let room = rect.size.x - 2.0 * pad - reserve * ((align - 0.5) * 2.0).clamp(0.0, 1.0);
    if tw >= room {
        rect.pos.x + pad
    } else {
        rect.pos.x + pad + (room - tw) * align
    }
}

/// The clip for a cell's text `tw` wide that is wider than its room, the
/// cell less the padding either side. A text that still ends a point short
/// of the next column is not cut: it shows whole, as it always has, and
/// an amount in a tight column keeps its last digit. A text that has to be
/// cut is cut the padding short of the next column, as far short as a text
/// that fits stops. A grid without padding keeps one point off the next
/// column either way, and a cell narrower than its padding shows nothing.
fn cell_text_clip(rect: Rect, pad: f64, tw: f64) -> Rect {
    let whole = pad + tw <= rect.size.x - 1.0;
    let off = if whole { 1.0 } else { pad.max(1.0) };
    Rect {
        pos: rect.pos,
        size: dvec2((rect.size.x - off).max(0.0), rect.size.y),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum HitZone {
    Corner,
    ColHeader {
        display_col: usize,
        resize_edge: Option<usize>,
    },
    RowHeader {
        row: usize,
        resize_edge: Option<usize>,
    },
    Cell {
        row: usize,
        display_col: usize,
    },
    Outside,
}

enum Interact {
    None,
    ColResize {
        display_col: usize,
        start_size: f64,
        start_abs: f64,
    },
    RowResize {
        row: usize,
        start_size: f64,
        start_abs: f64,
    },
    ColDragPending {
        display_col: usize,
        down_abs: DVec2,
        modifiers: KeyModifiers,
    },
    ColDrag {
        display_col: usize,
        cur_abs: DVec2,
        insert_at: usize,
    },
    /// A primary press on a cell, until it comes up.
    CellPress {
        row: usize,
        display_col: usize,
        down_abs: DVec2,
        modifiers: KeyModifiers,
        /// The press drags out a selection as the pointer moves.
        rubber_band: bool,
        /// The pointer has been further than `drag_threshold` from where
        /// it went down, so the press is no longer a click.
        travelled: bool,
        /// The press is carrying its row out (`row_drag`).
        carrying: bool,
        /// The scroll as the press went down, which a `drag_scrolling`
        /// press moves away from by the pointer's travel.
        scroll_at_press: DVec2,
        /// Where the pointer is now: where a carry near an edge that
        /// scrolls under a still pointer says the pointer is.
        at: DVec2,
    },
}

impl Default for Interact {
    fn default() -> Self {
        Self::None
    }
}

/// Geometry of the current frame, computed at draw begin and reused for hit
/// testing until the next draw.
#[derive(Clone, Default)]
struct GridViewport {
    widget_rect: Rect,
    data_rect: Rect,
    /// Column-header strip (excluding the corner box). Zero-size when hidden.
    col_header_rect: Rect,
    /// Row-header strip (excluding the corner box). Zero-size when hidden.
    row_header_rect: Rect,
    corner_rect: Rect,
    /// Visible display columns: (display_col, abs x, width)
    vis_cols: Vec<(usize, f64, f64)>,
    /// First visible row and its abs y.
    row0: usize,
    row0_y: f64,
    /// One-past-last visible row.
    row1: usize,
    total_w: f64,
    total_h: f64,
}

impl GridViewport {
    /// Move every cached rect by `delta`. The viewport is computed from the
    /// turtle at draw time, but a parent that sizes itself by `Fill` can
    /// still shift the whole widget afterwards (deferred alignment): the
    /// instances move with it, the cached geometry does not. Re-anchoring
    /// to where the widget actually landed keeps hit testing honest.
    fn translate(&mut self, delta: DVec2) {
        self.widget_rect.pos += delta;
        self.data_rect.pos += delta;
        self.col_header_rect.pos += delta;
        self.row_header_rect.pos += delta;
        self.corner_rect.pos += delta;
        for (_, x, _) in &mut self.vis_cols {
            *x += delta.x;
        }
        self.row0_y += delta.y;
    }
}

struct CellIter {
    row: usize,
    y: f64,
    row_h: f64,
    col_i: usize,
}

#[derive(Script, WidgetRegister, WidgetRef, WidgetSet)]
pub struct DataGrid {
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

    #[live(96.0)]
    default_col_width: f64,
    #[live(26.0)]
    default_row_height: f64,
    #[live(28.0)]
    col_header_height: f64,
    /// How far a column heading is turned from the horizontal when it has
    /// to be, in degrees. Nought never turns one. Past nought, only a heading
    /// whose name does not fit its column flat turns, standing on the middle
    /// of its own column — which is how a sheet of narrow numeric columns
    /// keeps long names over them — and one that fits stays exactly as it
    /// was. The strip is as tall as the longest turned name needs, and with
    /// nothing to turn it is `col_header_height` exactly: an angle on a sheet
    /// of wide columns changes nothing. The arithmetic is in
    /// `diagonal_text.rs`.
    #[live]
    header_angle: f64,
    /// Which way a turned heading leans, and so which side its ink hangs
    /// over. `Rise` by default: the spreadsheet's way.
    #[live(DiagonalLean::Rise)]
    header_lean: DiagonalLean,
    #[live(52.0)]
    row_header_width: f64,
    #[live(true)]
    show_col_headers: bool,
    #[live(true)]
    show_row_headers: bool,
    #[live(false)]
    zebra_stripes: bool,
    #[live(true)]
    allow_col_resize: bool,
    /// A thin line in `color_header_edge` at the edge between two column
    /// headings, the middle half of the heading's height, so the edge a
    /// pointer drags to resize a column can be seen before it is found. For
    /// a grid whose headings are the colour of the lines under its cells,
    /// where the cells' own edges show nothing up there. Off by default.
    #[live(false)]
    header_edges: bool,
    #[live(true)]
    allow_row_resize: bool,
    /// A column heading can be dragged along the headings and dropped
    /// between two others, with a bar showing where it will land; the grid
    /// then raises `ColumnMoved` and `ColumnOrderChanged`. Off by default.
    #[live(false)]
    allow_col_reorder: bool,
    /// Pressing a column heading cycles that column's sort. Off by
    /// default: a grid whose headings do something when pressed has to
    /// mean it, and most of them are read-only tables.
    #[live(false)]
    pub sortable: bool,
    /// Columns that refuse to sort even when the grid does — a column of
    /// thumbnails, a row of controls, anything with no order to be in.
    ///
    /// Set from the host, not the markup: the DSL can only carry lists of
    /// strings, and which column has no order to be in is something the
    /// side that owns the rows knows anyway.
    #[rust]
    unsortable_cols: Vec<usize>,
    /// Columns, by data column, that stay where they are drawn when
    /// columns are dragged: a column of row badges on the left, a column of
    /// controls on the right. Set from the host, like `unsortable_cols`.
    #[rust]
    unmovable_cols: Vec<usize>,
    #[live(24.0)]
    min_col_width: f64,
    #[live(14.0)]
    min_row_height: f64,
    #[live(6.0)]
    cell_pad_x: f64,
    /// Where a column heading's label sits across the heading, from 0.0 at
    /// the left to 1.0 at the right; centred unless declared. Past the
    /// middle the label keeps clear of the sort marks, all the way clear at
    /// 1.0. Row numbers stay centred.
    #[live(0.5)]
    header_align: f64,
    /// The pointer over a cell: `Default` unless declared, as it has always
    /// been. A list whose rows open when clicked says so with `Hand`.
    /// Edges and a heading that can be dragged keep their own shapes.
    #[live]
    cell_cursor: MouseCursor,
    #[live(true)]
    grab_key_focus: bool,
    /// Cells, whole rows, or nothing. See [`GridSelectMode`].
    #[live]
    selection: GridSelectMode,
    /// How far, in points, a press on a cell may travel and still be a
    /// click that raises `CellReleased`. Past it, a `row_drag` grid starts
    /// carrying the row.
    #[live(5.0)]
    drag_threshold: f64,
    /// A press on a cell that travels past `drag_threshold` carries its
    /// row out (`RowDragStarted`) instead of dragging out a selection:
    /// for a list whose rows are dragged somewhere else. Off by default.
    #[live(false)]
    row_drag: bool,
    /// While a row is carried, a pointer in the top or bottom row's height
    /// of the rows scrolls the grid that way, faster the nearer the edge,
    /// and keeps scrolling while the pointer holds still, raising
    /// `RowDragMoved` for each step: for a list whose rows are reordered
    /// inside it. A pointer taken off the rows, over the headings or out
    /// of the grid, scrolls nothing. Off by default, since a row carried
    /// out of a list to somewhere else must not scroll it.
    #[live(false)]
    row_drag_scroll: bool,
    /// A press on a cell that travels past `drag_threshold` scrolls the
    /// grid with the pointer, as a finger scrolls a list on a phone,
    /// instead of dragging out a selection or carrying its row: the point
    /// pressed stays under the pointer. It is no click, so it raises no
    /// `CellReleased`. Off by default.
    #[live(false)]
    drag_scrolling: bool,
    #[live(100usize)]
    rows: usize,
    #[live(26usize)]
    cols: usize,

    #[live]
    color_bg: Vec4f,
    #[live]
    color_cell: Vec4f,
    #[live]
    color_cell_alt: Vec4f,
    #[live]
    color_text: Vec4f,
    #[live]
    color_header: Vec4f,
    #[live]
    color_header_active: Vec4f,
    #[live]
    color_header_text: Vec4f,
    /// The sorted column's heading label and marks. Transparent, the
    /// default, draws them in `color_header_text` like the rest.
    #[live]
    color_header_sorted: Vec4f,
    #[live]
    color_selection: Vec4f,
    #[live]
    color_selection_border: Vec4f,
    #[live]
    color_drag_marker: Vec4f,
    #[live]
    color_resize_guide: Vec4f,
    /// The line `header_edges` draws between two headings.
    #[live]
    color_header_edge: Vec4f,

    #[live]
    draw_cell: DrawDataGridCell,
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_overlay: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_text_bold: DrawText,
    /// The headings' text: labels, sort marks and row numbers. With a font
    /// size of 0, the default, they are drawn with `draw_text` instead, as
    /// they always have been, so a grid that restyles its cells restyles
    /// its headings with them. Any size above 0 gives the headings their
    /// own face and size. Their colour comes from `color_header_text` or
    /// `color_header_sorted` either way.
    #[live]
    draw_text_header: DrawText,
    /// The same headings, turned. It carries no style of its own on purpose:
    /// the face and the ink are copied from whichever of the two above the
    /// headings are drawn in, before each draw, so there is no second place
    /// to restyle and no way for the two to drift apart.
    #[live]
    draw_header_turned: DrawRotatedText,
    #[live]
    scroll_bar_h: ScrollBar,
    #[live]
    scroll_bar_v: ScrollBar,

    #[rust]
    col_sizes: AxisSizes,
    #[rust]
    row_sizes: AxisSizes,
    /// display index -> data column; None = identity
    #[rust]
    col_order: Option<Vec<u32>>,
    #[rust]
    col_labels: Vec<String>,
    /// Every data column's name, measured flat in the heading face, and the
    /// height of a line in it: what decides which headings turn and how
    /// tall the strip is. Held rather than shaped each draw because a sheet
    /// may have thousands of columns and the names move far less often than
    /// the widths — which are NOT part of this, so a resize re-decides from
    /// the held names at the cost of a comparison per column. `None` means
    /// it has to be measured again — which the setters that can change a
    /// name, a column count or the style all say.
    #[rust]
    header_names: Option<(Vec<f64>, f64)>,
    #[rust]
    head_glyphs: Vec<PathGlyphInstance>,
    /// (data col, ascending)
    #[rust]
    sort_indicator: Option<(usize, bool)>,

    #[rust]
    scroll: DVec2,
    #[rust]
    selected: Option<GridSelection>,
    #[rust]
    interact: Interact,
    #[rust]
    sizes_initialized: bool,

    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    items: ComponentMap<u64, WidgetItem>,
    #[rust]
    reusable_items: HashMap<LiveId, Vec<WidgetItem>>,

    #[rust]
    draw_state: DrawStateWrap<()>,
    #[rust]
    vp: GridViewport,
    #[rust]
    iter: Option<CellIter>,
    #[rust]
    last_notified_scroll: DVec2,
    /// Provider used to answer clipboard-copy requests for the current
    /// selection, set by the app via [`DataGridRef::set_copy_provider`].
    #[rust]
    copy_provider: Option<CopyProvider>,
    /// The cell whose editor is seated, as (row, data col). The grid owns
    /// the editor from [`Self::edit_cell`] to the commit or the cancel: it
    /// is the only party that can keep the item out of the reuse pool when
    /// the cell scrolls away, and the only one that can turn the editor
    /// losing the keyboard into a commit.
    #[rust]
    editing: Option<(usize, usize)>,
    /// Set by [`Self::edit_cell`], cleared by the first draw after it. The
    /// editor has no area until it has been drawn once, and key focus set
    /// on an area that does not exist yet lands nowhere.
    #[rust]
    edit_focus_pending: bool,
    /// Whether this frame's cell loop drew the editor, so that [`Self::end`]
    /// knows to draw it where the loop did not reach.
    #[rust]
    editor_drawn: bool,
    /// A cell asked for before the first draw, when the viewport has no
    /// size to scroll it into view against; done on the draw that gives
    /// it one.
    #[rust]
    scroll_pending: Option<(usize, usize)>,
    /// An edit the grid shrank out from under, reported on the next event,
    /// when there is a context to say it with.
    #[rust]
    edit_dropped: Option<(usize, usize)>,
    /// Rows the host outlined during this frame's cell loop, drawn over
    /// every cell when the frame ends.
    #[rust]
    outlines: Vec<(usize, Vec4f)>,
    /// A tip for each column heading, by data column; empty for none.
    #[rust]
    header_tips: Vec<String>,
    /// The heading whose tip was last raised, as (display col, data col),
    /// until the pointer leaves it.
    #[rust]
    tip_head: Option<(usize, usize)>,
    /// The frame a carry near an edge scrolls on next, while it scrolls,
    /// and when the step before it was taken.
    #[rust]
    row_drag_frame: Option<NextFrame>,
    #[rust]
    row_drag_frame_time: Option<f64>,
}

pub type CopyProvider = Box<dyn FnMut(&GridSelection) -> String>;

impl ScriptHook for DataGrid {
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
        if apply.is_reload() {
            for (_, item) in self.items.iter_mut() {
                if let Some(template_ref) = self.templates.get(&item.template) {
                    let template_value: ScriptValue = template_ref.as_object().into();
                    item.widget.script_apply(vm, apply, scope, template_value);
                }
            }
        }
        if !self.sizes_initialized {
            self.sizes_initialized = true;
            self.col_sizes = AxisSizes::new(self.default_col_width);
            self.row_sizes = AxisSizes::new(self.default_row_height);
        } else {
            self.col_sizes.set_default(self.default_col_width);
            self.row_sizes.set_default(self.default_row_height);
        }
        // A grid switched to selecting nothing lets go of what it held.
        if self.selection == GridSelectMode::Off {
            self.selected = None;
        }
        // The angle or the heading face may just have moved under the strip,
        // and both change how tall it has to be.
        self.remeasure_headers();
    }
}

fn cell_key(row: usize, col: usize) -> u64 {
    ((row as u64) << 32) | (col as u64 & 0xffff_ffff)
}

/// Spreadsheet-style column name: A..Z, AA..AZ, ...
pub fn column_letters(col: usize) -> String {
    let mut n = col;
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

impl DataGrid {
    // ---------------------------------------------------------------
    // model access
    // ---------------------------------------------------------------

    pub fn set_grid_size(&mut self, rows: usize, cols: usize) {
        if self.rows == rows && self.cols == cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        self.remeasure_headers();
        if let Some(order) = &self.col_order {
            if order.len() != cols {
                self.col_order = None;
            }
        }
        if let Some(sel) = &mut self.selected {
            if sel.anchor.0 >= rows || sel.head.0 >= rows {
                self.selected = None;
            } else if sel.anchor.1 >= cols || sel.head.1 >= cols {
                self.selected = None;
            }
        }
        // A cell that is no longer there cannot be edited. The editor
        // item itself is retired by the next draw's sweep.
        if let Some((row, col)) = self.editing {
            if row >= rows || col >= cols {
                self.editing = None;
                self.edit_focus_pending = false;
                self.edit_dropped = Some((row, col));
            }
        }
        // If we're mid-draw (size set from the draw loop before iteration),
        // recompute the frame's geometry against the already-known rect.
        if self.iter.is_some() {
            self.compute_viewport();
            self.reset_iter();
        }
    }

    pub fn display_to_data(&self, display_col: usize) -> usize {
        match &self.col_order {
            Some(order) => order.get(display_col).map(|c| *c as usize).unwrap_or(display_col),
            None => display_col,
        }
    }

    pub fn data_to_display(&self, col: usize) -> usize {
        match &self.col_order {
            Some(order) => order
                .iter()
                .position(|c| *c as usize == col)
                .unwrap_or(col),
            None => col,
        }
    }

    /// Change the uniform default cell size (existing per-index overrides stay).
    pub fn set_default_sizes(&mut self, cx: &mut Cx, col_width: f64, row_height: f64) {
        self.default_col_width = col_width;
        self.default_row_height = row_height;
        self.col_sizes.set_default(col_width);
        self.row_sizes.set_default(row_height);
        self.area.redraw(cx);
    }

    pub fn set_col_width(&mut self, display_col: usize, width: f64) {
        self.col_sizes.set(display_col, width.max(self.min_col_width));
    }

    pub fn col_width(&self, display_col: usize) -> f64 {
        self.col_sizes.size_of(display_col)
    }

    /// Every column's width at once, in display order: the first width is
    /// the column drawn leftmost now. Once headings can be dragged that is
    /// not the order of the host's own columns, so a host that keeps its
    /// widths by column hands them over through [`Self::col_order`], the
    /// width of `order[0]` first. A column past the end of `widths`
    /// goes back to `default_col_width`, so a shorter list clears what a
    /// longer one set, and a width below `min_col_width` is raised to it,
    /// as a dragged edge is. A width for a column the grid does not have
    /// yet waits for it.
    ///
    /// Unlike [`Self::set_col_width`] the frame is measured again at once.
    /// Called from the draw loop before the first [`Self::next_cell`], the
    /// widths land in the frame being drawn, not the next one; called from
    /// event code, a pointer finds the new columns at once and the grid
    /// redraws. Widths the grid already has change
    /// nothing, so pushing them again on every draw costs nothing. A host
    /// that fits the columns to [`Self::data_width`] and also lets people
    /// drag an edge pushes only when the width it fits to changes: a push
    /// during the drag would put the edge back under the pointer.
    pub fn set_col_widths(&mut self, cx: &mut Cx, widths: &[f64]) {
        let min = self.min_col_width;
        if !self.col_sizes.set_all(widths.iter().map(|w| w.max(min))) {
            return;
        }
        // Hit testing reads the frame's geometry too, so it is measured
        // again whether or not a draw is under way.
        self.compute_viewport();
        if self.iter.is_some() {
            self.reset_iter();
        } else {
            self.area.redraw(cx);
        }
    }

    /// Every column's width, in display order: what a host keeps to put
    /// the widths back later with [`Self::set_col_widths`]. The width of
    /// data column `col_order()[i]` is the `i`th.
    pub fn col_widths(&self) -> Vec<f64> {
        (0..self.cols).map(|c| self.col_sizes.size_of(c)).collect()
    }

    /// The width the columns share: the grid's width less the row-number
    /// strip, in the frame being drawn when read from the draw loop and in
    /// the last one drawn otherwise, and nothing before the first draw.
    /// The vertical scroll bar is drawn over the columns, not beside them.
    pub fn data_width(&self) -> f64 {
        self.vp.data_rect.size.x
    }

    pub fn set_row_height(&mut self, row: usize, height: f64) {
        self.row_sizes.set(row, height.max(self.min_row_height));
    }

    /// Every row back to the default height: the overrides from
    /// [`Self::set_row_height`] and from dragging row edges, all at once.
    pub fn clear_row_heights(&mut self) {
        self.row_sizes.clear_overrides();
    }

    pub fn set_col_labels(&mut self, labels: Vec<String>) {
        // Only when they really changed: a host's draw loop hands the same
        // list over on every frame, and a turned strip re-measured every
        // frame would shape every name in the sheet to find a height that
        // has not moved.
        if self.col_labels != labels {
            self.remeasure_headers();
        }
        self.col_labels = labels;
    }

    /// A tip for each column heading, in data columns like
    /// [`Self::set_col_labels`], so a tip stays with its column when the
    /// column is dragged somewhere else. An empty tip, or a column past the
    /// end of the list, has none. The pointer resting on a heading with a
    /// tip raises [`TipAction::HoverIn`] with the text and the heading's
    /// rectangle, and leaving it raises [`TipAction::HoverOut`], for the
    /// window's `TipLayer` to show and hide; with no tips set the grid
    /// raises neither.
    pub fn set_header_tips(&mut self, tips: Vec<String>) {
        self.header_tips = tips;
    }

    /// Columns that will not sort, whatever `sortable` says.
    pub fn set_unsortable_cols(&mut self, cols: Vec<usize>) {
        self.unsortable_cols = cols;
    }

    /// Columns, by data column, that keep their place when headings are
    /// dragged, whatever `allow_col_reorder` says: their own headings do
    /// not drag, and no other column can be dropped on the far side of
    /// one. A column of row badges stays first and a column of controls
    /// stays last this way.
    pub fn set_unmovable_cols(&mut self, cols: Vec<usize>) {
        self.unmovable_cols = cols;
    }

    /// The data column drawn at each display position, left to right: the
    /// order `ColumnOrderChanged` reports, and what
    /// [`Self::set_col_order`] takes back.
    pub fn col_order(&self) -> Vec<usize> {
        (0..self.cols).map(|d| self.display_to_data(d)).collect()
    }

    /// Draw the columns in `order`, the data column for each display
    /// position from the left: how a host puts back an order it kept from
    /// `ColumnOrderChanged`. It has to name every column once, and an order
    /// that does not is refused and changes nothing; the answer says which.
    /// The grid raises nothing for it, since the host already knows.
    ///
    /// It does what dropping a dragged heading does: widths already set go
    /// with their columns, and a selection, which is held in display
    /// positions, is cleared when the order changes. The order the grid
    /// already has changes nothing, so it can be pushed on every draw. Like
    /// [`Self::set_col_widths`] the frame is measured again at once, so from
    /// the draw loop it lands in the frame being drawn. A change in the
    /// number of columns puts them back in their own order, so a host that
    /// changes the count sets its order after it.
    pub fn set_col_order(&mut self, cx: &mut Cx, order: &[usize]) -> bool {
        let cols = self.cols;
        let mut seen = vec![false; cols];
        let every_column_once = order.len() == cols
            && order
                .iter()
                .all(|&c| c < cols && !std::mem::replace(&mut seen[c], true));
        if !every_column_once {
            return false;
        }
        let old = self.col_order();
        if old == order {
            return true;
        }
        let mut lands_at = vec![0; cols];
        for (display_col, &col) in order.iter().enumerate() {
            lands_at[col] = display_col;
        }
        // A width waiting for a column the grid does not have, from a
        // longer list of widths or from before the columns were cut, stays
        // where it waits: only the columns there are change places.
        self.col_sizes.remap(|display_col| match old.get(display_col) {
            Some(&col) => lands_at[col],
            None => display_col,
        });
        let identity = order.iter().enumerate().all(|(d, &c)| d == c);
        self.col_order = (!identity).then(|| order.iter().map(|&c| c as u32).collect());
        self.selected = None;
        self.compute_viewport();
        if self.iter.is_some() {
            self.reset_iter();
        } else {
            self.area.redraw(cx);
        }
        true
    }

    /// The column being sorted and which way, or nothing.
    pub fn sort(&self) -> Option<(usize, bool)> {
        self.sort_indicator
    }

    pub fn set_sort_indicator(&mut self, sort: Option<(usize, bool)>) {
        self.sort_indicator = sort;
    }

    /// What is selected, or nothing. With `selection: Off` it is always
    /// nothing.
    pub fn selection(&self) -> Option<GridSelection> {
        match self.selection {
            GridSelectMode::Off => None,
            _ => self.selected,
        }
    }

    /// Select `selection`, or nothing. With `selection: Off` the grid holds
    /// no selection, and one set here is dropped.
    pub fn set_selection(&mut self, cx: &mut Cx, selection: Option<GridSelection>) {
        self.selected = selection.filter(|_| self.selection != GridSelectMode::Off);
        self.area.redraw(cx);
    }

    /// The active (head) cell as (row, data col).
    pub fn active_cell(&self) -> Option<(usize, usize)> {
        self.selection()
            .map(|s| (s.head.0, self.display_to_data(s.head.1)))
    }

    pub fn col_label(&self, col: usize) -> String {
        if let Some(label) = self.col_labels.get(col) {
            label.clone()
        } else {
            column_letters(col)
        }
    }

    /// Whether a column heading MAY turn. Which ones do is decided per
    /// column by [`DataGrid::header_turns`].
    fn headers_may_turn(&self) -> bool {
        self.show_col_headers && self.header_angle > 0.0
    }

    /// The names have to be measured again: a name, a column count, an
    /// angle or a face has changed under them.
    fn remeasure_headers(&mut self) {
        self.header_names = None;
    }

    /// Whether the heading over `display_col` is turned: only when turning
    /// is allowed at all and its name does not fit the column flat, by the
    /// same test the table's headings use. So a column dragged wide enough
    /// lays its name flat again on the next draw, and one dragged narrow
    /// turns it, without anything being told.
    fn header_turns(&self, display_col: usize) -> bool {
        if !self.headers_may_turn() {
            return false;
        }
        let Some((names, _)) = &self.header_names else {
            return false;
        };
        let data_col = self.display_to_data(display_col);
        let name = names.get(data_col).copied().unwrap_or(0.0);
        !heading_fits_flat(name, self.col_sizes.size_of(display_col), self.cell_pad_x)
    }

    /// The longest name among the turned headings, over EVERY column and
    /// not only the ones in view: a strip that grew and shrank as the sheet
    /// scrolled sideways would move every row under it, and the point of a
    /// heading row is that it stays.
    fn longest_turned_header(&self) -> Option<f64> {
        let (names, _) = self.header_names.as_ref()?;
        let mut longest = None;
        for display_col in 0..self.cols {
            if self.header_turns(display_col) {
                let name = names.get(self.display_to_data(display_col)).copied().unwrap_or(0.0);
                longest = Some(longest.map_or(name, |l: f64| l.max(name)));
            }
        }
        longest
    }

    /// The strip: the stated height when no heading turns — so a grid with
    /// an angle set and nothing to turn is not a point taller for it — and
    /// otherwise what the longest turned name needs, never less than the
    /// stated height the flat headings beside it still stand in.
    fn header_area_height(&self) -> f64 {
        if !self.show_col_headers {
            return 0.0;
        }
        // Before the first draw nothing is measured yet, so nothing turns
        // and the strip is the stated height: one draw behind, not missing.
        match (self.longest_turned_header(), &self.header_names) {
            (Some(longest), Some((_, line))) => {
                // Up to the whole point: the strip is the top of every row's
                // arithmetic below it, and a fractional one puts every line
                // in the sheet half a pixel off its neighbour's.
                diagonal_row_height(longest, *line, self.header_angle)
                    .ceil()
                    .max(self.col_header_height)
            }
            _ => self.col_header_height,
        }
    }

    /// The part of a heading a flat name is placed in: the bottom slice of
    /// the stated height, so a flat name beside turned ones reads along the
    /// same bottom line they stand on. With nothing turned the slice IS the
    /// heading.
    fn flat_header_slot(&self, rect: Rect) -> Rect {
        let h = self.col_header_height.min(rect.size.y);
        Rect {
            // Bracketed so that with nothing turned the offset is an exact
            // nought and not a rounding of one.
            pos: dvec2(rect.pos.x, rect.pos.y + (rect.size.y - h)),
            size: dvec2(rect.size.x, h),
        }
    }

    /// The face the turned headings are drawn in: the flat ones' face,
    /// copied over so the two can never drift apart.
    fn turn_header_face(&mut self) {
        let own = self.headers_have_own_text();
        let (style, color) = if own {
            (self.draw_text_header.text_style.clone(), self.color_header_text)
        } else {
            (self.draw_text.text_style.clone(), self.color_header_text)
        };
        self.draw_header_turned.text_style = style;
        self.draw_header_turned.color = color;
    }

    /// Every data column's name, measured the way a flat heading measures
    /// itself to decide whether it overflows — so "fits flat" here and "is
    /// clipped" there are the same number — and the height of a line in the
    /// turned face, which a turned name stands across.
    fn measure_header_names(&mut self, cx: &mut Cx2d) -> (Vec<f64>, f64) {
        let mut names = Vec::with_capacity(self.cols);
        for col in 0..self.cols {
            let label = self.col_label(col);
            names.push(self.header_text_width(cx, &label));
        }
        // The line is the face's, not any one name's: ascender to
        // descender is the same whatever is written, so one probe settles
        // it even for a sheet whose first name is empty.
        let mut line = self.draw_header_turned.text_style.font_size as f64;
        if let Some(run) = self.draw_header_turned.prepare_single_line_run(cx, "Ag") {
            line = line.max((run.ascender_in_lpxs - run.descender_in_lpxs) as f64);
        }
        (names, line)
    }

    fn header_area_width(&self) -> f64 {
        if self.show_row_headers {
            self.row_header_width
        } else {
            0.0
        }
    }

    // ---------------------------------------------------------------
    // geometry
    // ---------------------------------------------------------------

    fn compute_viewport(&mut self) {
        let rect = self.vp.widget_rect;
        let hw = self.header_area_width();
        let hh = self.header_area_height();
        let data_rect = Rect {
            pos: rect.pos + dvec2(hw, hh),
            size: dvec2((rect.size.x - hw).max(0.0), (rect.size.y - hh).max(0.0)),
        };
        let total_w = self.col_sizes.total(self.cols);
        let total_h = self.row_sizes.total(self.rows);
        self.scroll.x = self.scroll.x.min((total_w - data_rect.size.x).max(0.0)).max(0.0);
        self.scroll.y = self.scroll.y.min((total_h - data_rect.size.y).max(0.0)).max(0.0);

        let (col0, col0_off) = self.col_sizes.index_at(self.scroll.x, self.cols);
        let mut vis_cols = std::mem::take(&mut self.vp.vis_cols);
        vis_cols.clear();
        let mut x = data_rect.pos.x - col0_off;
        let mut c = col0;
        while c < self.cols && x < data_rect.pos.x + data_rect.size.x {
            let w = self.col_sizes.size_of(c);
            vis_cols.push((c, x, w));
            x += w;
            c += 1;
        }

        let (row0, row0_off) = self.row_sizes.index_at(self.scroll.y, self.rows);
        let mut y = data_rect.pos.y - row0_off;
        let row0_y = y;
        let mut r = row0;
        while r < self.rows && y < data_rect.pos.y + data_rect.size.y {
            y += self.row_sizes.size_of(r);
            r += 1;
        }

        self.vp = GridViewport {
            widget_rect: rect,
            data_rect,
            col_header_rect: Rect {
                pos: rect.pos + dvec2(hw, 0.0),
                size: dvec2(data_rect.size.x, hh),
            },
            row_header_rect: Rect {
                pos: rect.pos + dvec2(0.0, hh),
                size: dvec2(hw, data_rect.size.y),
            },
            corner_rect: Rect {
                pos: rect.pos,
                size: dvec2(hw, hh),
            },
            vis_cols,
            row0,
            row0_y,
            row1: r,
            total_w,
            total_h,
        };
    }

    fn reset_iter(&mut self) {
        let row_h = if self.vp.row0 < self.rows {
            self.row_sizes.size_of(self.vp.row0)
        } else {
            0.0
        };
        self.iter = Some(CellIter {
            row: self.vp.row0,
            y: self.vp.row0_y,
            row_h,
            col_i: 0,
        });
    }

    fn cell_rect(&self, row: usize, display_col: usize) -> Rect {
        let x = self.vp.data_rect.pos.x + self.col_sizes.offset_of(display_col) - self.scroll.x;
        let y = self.vp.data_rect.pos.y + self.row_sizes.offset_of(row) - self.scroll.y;
        Rect {
            pos: dvec2(x, y),
            size: dvec2(self.col_sizes.size_of(display_col), self.row_sizes.size_of(row)),
        }
    }

    /// The row drawn level with `abs`, in the pointer's own coordinates:
    /// what a host reordering rows under a carried pointer asks. Only the
    /// height counts, so a pointer carried sideways past the columns still
    /// finds the row it is level with. `None` above the rows (in the
    /// heading strip or above the grid), below the last row, and below
    /// the grid.
    ///
    /// Measured against the frame last drawn and the scroll as it is now,
    /// with the row heights as they are now.
    pub fn row_at(&self, abs: DVec2) -> Option<usize> {
        let data = self.vp.data_rect;
        if self.rows == 0 || abs.y < data.pos.y || abs.y >= data.pos.y + data.size.y {
            return None;
        }
        let dy = abs.y - data.pos.y + self.scroll.y;
        if dy >= self.row_sizes.total(self.rows) {
            return None;
        }
        Some(self.row_sizes.index_at(dy, self.rows).0)
    }

    /// Where a row is drawn, in the pointer's coordinates: from the left of
    /// the data area across the columns in view, at the row's own height.
    /// A row scrolled out of view still has a rectangle, above the grid or
    /// below it, so a host can tell how far away it is; a row the grid
    /// does not have has none. Measured as [`Self::row_at`] measures.
    pub fn row_rect(&self, row: usize) -> Option<Rect> {
        if row >= self.rows {
            return None;
        }
        let data = self.vp.data_rect;
        let width = (self.col_sizes.total(self.cols) - self.scroll.x).clamp(0.0, data.size.x);
        Some(Rect {
            pos: dvec2(data.pos.x, data.pos.y + self.row_sizes.offset_of(row) - self.scroll.y),
            size: dvec2(width, self.row_sizes.size_of(row)),
        })
    }

    /// The gap a carried row let go at `abs` would drop into, from 0 in
    /// front of the first row to `rows` behind the last: in front of the
    /// row level with the pointer while the pointer is above that row's
    /// middle, behind it once it is below. A host that reorders its rows
    /// while one is carried moves the row from line `from` to this gap, or
    /// to one line less when the gap is past `from`, since the gap counts
    /// the carried row itself. Halfway points keep rows of different
    /// heights from trading places under a pointer that holds still.
    ///
    /// Only the height counts while the pointer is over the rows, the row
    /// numbers included, and under the last row, where the rows do not
    /// fill the grid, is the gap behind it. Anywhere else, over the
    /// headings or above, below or beside the grid, there is no gap, so a
    /// host leaves the row where it is: a carry that strays over something
    /// else moves nothing, and `row_drag_scroll` scrolls from just inside
    /// the top and bottom edges of the rows. A grid with no rows has no
    /// gap either. Measured as [`Self::row_at`] measures.
    pub fn row_gap_at(&self, abs: DVec2) -> Option<usize> {
        if self.rows == 0 || !self.over_rows(abs) {
            return None;
        }
        let dy = abs.y - self.vp.data_rect.pos.y + self.scroll.y;
        if dy >= self.row_sizes.total(self.rows) {
            return Some(self.rows);
        }
        let (row, within) = self.row_sizes.index_at(dy, self.rows);
        Some(if within < self.row_sizes.size_of(row) * 0.5 { row } else { row + 1 })
    }

    /// Whether `abs` is over the rows: level with the data area, inside
    /// the grid's width, the row numbers included.
    fn over_rows(&self, abs: DVec2) -> bool {
        let widget = self.vp.widget_rect;
        let data = self.vp.data_rect;
        abs.x >= widget.pos.x
            && abs.x < widget.pos.x + widget.size.x
            && abs.y >= data.pos.y
            && abs.y < data.pos.y + data.size.y
    }

    // ---------------------------------------------------------------
    // draw cycle
    // ---------------------------------------------------------------

    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_turtle(walk, self.layout);
        self.vp.widget_rect = cx.turtle().rect();
        // Before the viewport, which is laid out under the heading strip:
        // with an angle set, how tall that strip is depends on which names
        // fit their columns, and that is a measurement. Without one nothing
        // is measured at all, so the flat grid costs what it always did.
        if self.headers_may_turn() {
            self.turn_header_face();
            if self.header_names.is_none() {
                self.header_names = Some(self.measure_header_names(cx));
            }
        }
        self.compute_viewport();
        if let Some((row, display_col)) = self.scroll_pending.take() {
            self.scroll_to_cell(row, display_col);
            self.compute_viewport();
        }
        self.draw_bg.color = self.color_bg;
        self.draw_bg.draw_abs(cx, self.vp.widget_rect);
        cx.push_clip_rect(self.vp.data_rect);
        self.editor_drawn = false;
        self.outlines.clear();
        self.reset_iter();
    }

    /// Next visible cell in row-major order. The app draws each cell with
    /// [`Self::cell_text`], [`Self::cell_bg`] or a widget item; skipped cells
    /// simply show the grid background.
    ///
    /// The cell being edited is never handed out: the grid draws its own
    /// editor there and moves on, so a host's loop needs no case for it.
    pub fn next_cell(&mut self, cx: &mut Cx2d) -> Option<GridCell> {
        loop {
            let cell = self.next_cell_in_order()?;
            if self.editing == Some((cell.row, cell.col)) {
                self.draw_editor(cx, &cell);
                continue;
            }
            return Some(cell);
        }
    }

    fn next_cell_in_order(&mut self) -> Option<GridCell> {
        let iter = self.iter.as_mut()?;
        loop {
            if iter.row >= self.vp.row1 || self.vp.vis_cols.is_empty() {
                return None;
            }
            if iter.col_i >= self.vp.vis_cols.len() {
                iter.col_i = 0;
                iter.y += iter.row_h;
                iter.row += 1;
                if iter.row >= self.vp.row1 {
                    return None;
                }
                iter.row_h = self.row_sizes.size_of(iter.row);
                continue;
            }
            let (display_col, x, w) = self.vp.vis_cols[iter.col_i];
            iter.col_i += 1;
            let rect = Rect {
                pos: dvec2(x, iter.y),
                size: dvec2(w, iter.row_h),
            };
            let row = iter.row;
            let col = match &self.col_order {
                Some(order) => order[display_col] as usize,
                None => display_col,
            };
            return Some(GridCell {
                row,
                col,
                display_col,
                rect,
            });
        }
    }

    fn default_cell_bg(&self, row: usize) -> Vec4f {
        if self.zebra_stripes && row % 2 == 1 {
            self.color_cell_alt
        } else {
            self.color_cell
        }
    }

    /// Draw only the cell background/gridline quad.
    pub fn cell_bg(&mut self, cx: &mut Cx2d, cell: &GridCell, color: Vec4f) {
        self.draw_cell.color = color;
        self.draw_cell.draw_abs(cx, cell.rect);
    }

    pub fn cell_text(&mut self, cx: &mut Cx2d, cell: &GridCell, text: &str) {
        self.cell_text_styled(cx, cell, text, CellStyle::default());
    }

    /// Fast path: one background quad + directly drawn text. Batches into two
    /// draw calls across all cells; no widget instantiation.
    pub fn cell_text_styled(&mut self, cx: &mut Cx2d, cell: &GridCell, text: &str, style: CellStyle) {
        let bg = style.bg.unwrap_or_else(|| self.default_cell_bg(cell.row));
        self.draw_cell.color = bg;
        self.draw_cell.draw_abs(cx, cell.rect);
        if text.is_empty() {
            return;
        }
        let color = style.color.unwrap_or(self.color_text);
        let pad = self.cell_pad_x;
        let dt = if style.bold {
            &mut self.draw_text_bold
        } else {
            &mut self.draw_text
        };
        dt.color = color;
        dt.font_scale = style.font_scale as f32;
        let laidout = dt.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        let tw = laidout.size_in_lpxs.width as f64 * style.font_scale;
        let th = laidout.size_in_lpxs.height as f64 * style.font_scale;
        let avail = cell.rect.size.x - 2.0 * pad;
        let x = if style.align <= 0.0 || tw >= avail {
            cell.rect.pos.x + pad
        } else {
            cell.rect.pos.x + pad + (avail - tw) * style.align
        };
        let y = cell.rect.pos.y + (cell.rect.size.y - th) * 0.5;
        let overflow = tw > avail;
        if overflow {
            cx.push_clip_rect(cell_text_clip(cell.rect, pad, tw));
        }
        dt.draw_abs(cx, dvec2(x, y), text);
        if overflow {
            cx.pop_clip_rect();
        }
        dt.font_scale = 1.0;
    }

    /// Get (or create/reuse) the widget item hosted in a cell. Configure it,
    /// then draw with [`Self::draw_item`].
    pub fn item(&mut self, cx: &mut Cx, row: usize, col: usize, template: LiveId) -> Option<WidgetRef> {
        use std::collections::hash_map::Entry;
        let entry_id = cell_key(row, col);
        let Some(template_ref) = self.templates.get(&template) else {
            error!("DataGrid template not found: {template}");
            return None;
        };
        let template_value: ScriptValue = template_ref.as_object().into();
        // For a non-isolated (main-app) grid this is exactly `cx.with_vm`. When
        // the isolate that minted the template has been reclaimed there is no
        // heap left to instantiate from, and no cell is the only honest answer.
        let Some(vm_id) = cx.script_ref_vm_id(template_ref) else {
            return None;
        };
        let make_or_reuse = |cx: &mut Cx, reusable: &mut HashMap<LiveId, Vec<WidgetItem>>| {
            if let Some(reused) = reusable.get_mut(&template).and_then(|pool| pool.pop()) {
                let widget_ref = reused.widget;
                cx.with_script_vm_id(vm_id, |vm| {
                    let mut widget_ref = widget_ref.clone();
                    widget_ref.script_apply(vm, &Apply::Reload, &mut Scope::empty(), template_value);
                });
                widget_ref
            } else {
                cx.with_script_vm_id(vm_id, |vm| WidgetRef::script_from_value(vm, template_value))
            }
        };
        match self.items.entry(entry_id) {
            Entry::Occupied(mut occ) => {
                if occ.get().template == template {
                    Some(occ.get().widget.clone())
                } else {
                    let widget_ref = make_or_reuse(cx, &mut self.reusable_items);
                    occ.insert(WidgetItem {
                        template,
                        widget: widget_ref.clone(),
                    });
                    cx.widget_tree_insert_child(self.uid, LiveId(entry_id), widget_ref.clone());
                    Some(widget_ref)
                }
            }
            Entry::Vacant(vac) => {
                let widget_ref = make_or_reuse(cx, &mut self.reusable_items);
                vac.insert(WidgetItem {
                    template,
                    widget: widget_ref.clone(),
                });
                cx.widget_tree_insert_child(self.uid, LiveId(entry_id), widget_ref.clone());
                Some(widget_ref)
            }
        }
    }

    /// The live widget item hosted at a cell, if one exists this frame.
    pub fn get_item(&self, row: usize, col: usize) -> Option<(LiveId, WidgetRef)> {
        self.items
            .get(&cell_key(row, col))
            .map(|item| (item.template, item.widget.clone()))
    }

    /// Draw a configured widget item inside a cell: background quad, cell
    /// clip, then the widget in a fixed-size turtle at the cell position.
    pub fn draw_item(
        &mut self,
        cx: &mut Cx2d,
        cell: &GridCell,
        item: &WidgetRef,
        bg: Option<Vec4f>,
    ) {
        let bg = bg.unwrap_or_else(|| self.default_cell_bg(cell.row));
        self.draw_cell.color = bg;
        self.draw_cell.draw_abs(cx, cell.rect);
        cx.push_clip_rect(Rect {
            pos: cell.rect.pos,
            size: cell.rect.size - dvec2(1.0, 1.0),
        });
        cx.begin_turtle(
            Walk {
                abs_pos: Some(cell.rect.pos),
                margin: Default::default(),
                width: Size::Fixed(cell.rect.size.x),
                height: Size::Fixed(cell.rect.size.y),
                ..Default::default()
            },
            Layout::flow_down(),
        );
        item.draw_all(cx, &mut Scope::empty());
        cx.end_turtle();
        cx.pop_clip_rect();
    }

    /// The seated editor, drawn in the cell it edits. Seeded once, in
    /// [`Self::edit_cell`], and never here: a draw that wrote the seed
    /// again would write over whatever has been typed since.
    fn draw_editor(&mut self, cx: &mut Cx2d, cell: &GridCell) {
        let Some(item) = self.item(cx, cell.row, cell.col, live_id!(Editor)) else {
            return;
        };
        self.editor_drawn = true;
        self.draw_item(cx, cell, &item, None);
        // Only now does the editor have an area to hand the keyboard to.
        if self.edit_focus_pending {
            self.edit_focus_pending = false;
            item.as_text_input().take_key_focus(cx);
        }
    }

    fn end(&mut self, cx: &mut Cx2d) {
        self.iter = None;
        // The editor is drawn whether or not its cell was on screen this
        // frame, under the same clip as the cells, so off screen it is cut
        // away as any cell is. That keeps two things current. Its item: the
        // sweep below retires every item nobody asked for, and a scroll
        // that carried the edited cell out of view would otherwise pool the
        // live editor with the text still in it, where the next reuse
        // re-applies the template over it. And its area: the keyboard
        // leaving the editor - the click somewhere else that ends an edit -
        // is delivered to the area the editor drew last, and one from an
        // earlier frame is refused before the editor sees it, so an edit
        // scrolled out of view would never commit.
        if let Some((row, col)) = self.editing {
            if !self.editor_drawn {
                let display_col = self.data_to_display(col);
                let cell = GridCell {
                    row,
                    col,
                    display_col,
                    rect: self.cell_rect(row, display_col),
                };
                self.draw_editor(cx, &cell);
            }
        }
        cx.pop_clip_rect();
        self.draw_selection_overlay(cx);
        self.draw_row_outlines(cx);
        self.draw_headers(cx);
        self.draw_header_edges(cx);
        self.draw_interact_overlay(cx);
        self.draw_scroll_bars(cx);

        let reusable_items = &mut self.reusable_items;
        self.items.retain_visible_with(|v: WidgetItem| {
            reusable_items.entry(v.template).or_default().push(v);
        });
        cx.widget_tree_mark_dirty(self.uid);
        cx.end_turtle_with_area(&mut self.area);

        if self.scroll != self.last_notified_scroll {
            self.last_notified_scroll = self.scroll;
            cx.widget_action(self.uid, DataGridAction::Scrolled);
        }
    }

    fn selection_rect(&self, sel: &GridSelection) -> Rect {
        let (r0, r1) = sel.row_range();
        let (c0, c1) = sel.col_range();
        let (r0, r1, c0, c1) = match sel.kind {
            GridSelectKind::All => (0, self.rows.saturating_sub(1), 0, self.cols.saturating_sub(1)),
            GridSelectKind::Rows => (r0, r1, 0, self.cols.saturating_sub(1)),
            GridSelectKind::Cols => (0, self.rows.saturating_sub(1), c0, c1),
            GridSelectKind::Cells => (r0, r1, c0, c1),
        };
        let x0 = self.vp.data_rect.pos.x + self.col_sizes.offset_of(c0) - self.scroll.x;
        let y0 = self.vp.data_rect.pos.y + self.row_sizes.offset_of(r0) - self.scroll.y;
        let x1 = self.vp.data_rect.pos.x + self.col_sizes.offset_of(c1) + self.col_sizes.size_of(c1)
            - self.scroll.x;
        let y1 = self.vp.data_rect.pos.y + self.row_sizes.offset_of(r1) + self.row_sizes.size_of(r1)
            - self.scroll.y;
        Rect {
            pos: dvec2(x0, y0),
            size: dvec2(x1 - x0, y1 - y0),
        }
    }

    fn draw_selection_overlay(&mut self, cx: &mut Cx2d) {
        let Some(sel) = self.selected else {
            return;
        };
        if self.rows == 0 || self.cols == 0 || self.selection == GridSelectMode::Off {
            return;
        }
        cx.push_clip_rect(self.vp.data_rect);
        let rect = self.selection_rect(&sel);
        self.draw_overlay.color = self.color_selection;
        self.draw_overlay.draw_abs(cx, rect);
        // range border
        let bc = self.color_selection_border;
        let b = 1.0;
        self.draw_overlay.color = bc;
        self.draw_overlay.draw_abs(cx, Rect { pos: rect.pos, size: dvec2(rect.size.x, b) });
        self.draw_overlay.draw_abs(cx, Rect {
            pos: dvec2(rect.pos.x, rect.pos.y + rect.size.y - b),
            size: dvec2(rect.size.x, b),
        });
        self.draw_overlay.draw_abs(cx, Rect { pos: rect.pos, size: dvec2(b, rect.size.y) });
        self.draw_overlay.draw_abs(cx, Rect {
            pos: dvec2(rect.pos.x + rect.size.x - b, rect.pos.y),
            size: dvec2(b, rect.size.y),
        });
        // active cell border, slightly thicker. A list that selects rows
        // has no active cell: the border would sit on whichever column
        // happened to be pressed.
        if self.selection != GridSelectMode::Rows {
            let head = self.cell_rect(sel.head.0, sel.head.1);
            self.draw_frame(cx, head, 2.0);
        }
        cx.pop_clip_rect();
    }

    /// Four edges `b` thick, inside `rect`, in the overlay's colour.
    fn draw_frame(&mut self, cx: &mut Cx2d, rect: Rect, b: f64) {
        self.draw_overlay.draw_abs(cx, Rect { pos: rect.pos, size: dvec2(rect.size.x, b) });
        self.draw_overlay.draw_abs(cx, Rect {
            pos: dvec2(rect.pos.x, rect.pos.y + rect.size.y - b),
            size: dvec2(rect.size.x, b),
        });
        self.draw_overlay.draw_abs(cx, Rect { pos: rect.pos, size: dvec2(b, rect.size.y) });
        self.draw_overlay.draw_abs(cx, Rect {
            pos: dvec2(rect.pos.x + rect.size.x - b, rect.pos.y),
            size: dvec2(b, rect.size.y),
        });
    }

    /// Outline a row across its visible cells, [`Self::ROW_OUTLINE`]
    /// points thick, in `color`: a keyboard cursor, a row being carried,
    /// anything a host marks on a row without the grid selecting it.
    ///
    /// Called from the draw loop, as the cells are. The outline itself is
    /// drawn when the frame ends, over every cell, so it does not matter
    /// whether the row's own cells have been drawn yet; a row that is not
    /// on screen draws nothing. Outlines last one frame, so a host asks
    /// for them again on every draw, the way it draws its cells.
    pub fn outline_row(&mut self, row: usize, color: Vec4f) {
        if row < self.rows {
            self.outlines.push((row, color));
        }
    }

    pub const ROW_OUTLINE: f64 = 1.5;

    fn draw_row_outlines(&mut self, cx: &mut Cx2d) {
        let outlines = std::mem::take(&mut self.outlines);
        let (Some(first), Some(last)) = (self.vp.vis_cols.first(), self.vp.vis_cols.last()) else {
            return;
        };
        let x0 = first.1;
        let x1 = last.1 + last.2;
        cx.push_clip_rect(self.vp.data_rect);
        for (row, color) in outlines {
            if row < self.vp.row0 || row >= self.vp.row1 {
                continue;
            }
            let y = self.vp.data_rect.pos.y + self.row_sizes.offset_of(row) - self.scroll.y;
            let rect = Rect {
                pos: dvec2(x0, y),
                size: dvec2(x1 - x0, self.row_sizes.size_of(row)),
            };
            self.draw_overlay.color = color;
            self.draw_frame(cx, rect, Self::ROW_OUTLINE);
        }
        cx.pop_clip_rect();
    }

    fn draw_headers(&mut self, cx: &mut Cx2d) {
        let vp = self.vp.clone();
        if self.show_col_headers && vp.col_header_rect.size.y > 0.0 {
            cx.push_clip_rect(vp.col_header_rect);
            for (display_col, x, w) in vp.vis_cols.iter().copied() {
                // Only a spreadsheet lights its headings. A selected row
                // contains every column, so a list would light the whole
                // strip for every row it picked.
                let selected = self.selection == GridSelectMode::Cells
                    && self
                        .selected
                        .map(|s| s.contains_col(display_col))
                        .unwrap_or(false);
                let rect = Rect {
                    pos: dvec2(x, vp.col_header_rect.pos.y),
                    size: dvec2(w, vp.col_header_rect.size.y),
                };
                self.draw_cell.color = if selected {
                    self.color_header_active
                } else {
                    self.color_header
                };
                self.draw_cell.draw_abs(cx, rect);
                let data_col = self.display_to_data(display_col);
                let label = self.col_label(data_col);
                let look = self.header_look(data_col);
                if self.header_turns(display_col) {
                    // A turned name is not inside its column, so the width
                    // that would hide a flat label means nothing here: a
                    // column two points across still gets its name, which is
                    // the whole reason for turning them. The sort marks ride
                    // the end of the name rather than sitting against an
                    // edge the name no longer runs along.
                    let mut label = label;
                    // Only the mark that says this column IS sorted. The
                    // faded pair that offers a sort has nowhere to sit here:
                    // it belongs against the right edge of a heading, and a
                    // turned name does not run along one — stuck on the end
                    // of every name instead, it doubles the ink in a row
                    // whose whole point is to be readable.
                    if let Some((mark, false)) = look.mark {
                        label.push(' ');
                        label.push_str(mark);
                    }
                    self.draw_header_turned.color = look.color;
                    let mut glyphs = std::mem::take(&mut self.head_glyphs);
                    draw_diagonal_name(
                        &mut self.draw_header_turned,
                        cx,
                        &mut glyphs,
                        rect,
                        &label,
                        self.header_angle,
                        self.header_lean,
                    );
                    self.head_glyphs = glyphs;
                } else if w >= 15.0 {
                    let cell = GridCell {
                        row: 0,
                        col: data_col,
                        display_col,
                        rect: self.flat_header_slot(rect),
                    };
                    // Only a label past the middle comes near the marks, so
                    // only then are they measured for it to keep clear of.
                    let reserve = match look.mark {
                        Some((mark, _)) if look.align > 0.5 => {
                            Self::RESIZE_MARGIN + self.cell_pad_x + self.header_text_width(cx, mark)
                        }
                        _ => 0.0,
                    };
                    self.header_text(cx, &cell, &label, look.color, look.align, reserve);
                    if let Some((mark, faded)) = look.mark {
                        self.header_mark(cx, &cell, mark, faded, look.color);
                    }
                }
            }
            cx.pop_clip_rect();
        }
        if self.show_row_headers && vp.row_header_rect.size.x > 0.0 {
            cx.push_clip_rect(vp.row_header_rect);
            let mut y = vp.row0_y;
            for row in vp.row0..vp.row1 {
                let h = self.row_sizes.size_of(row);
                let selected = self.selection != GridSelectMode::Off
                    && self.selected.map(|s| s.contains_row(row)).unwrap_or(false);
                let rect = Rect {
                    pos: dvec2(vp.row_header_rect.pos.x, y),
                    size: dvec2(vp.row_header_rect.size.x, h),
                };
                self.draw_cell.color = if selected {
                    self.color_header_active
                } else {
                    self.color_header
                };
                self.draw_cell.draw_abs(cx, rect);
                if h >= 12.0 {
                    let cell = GridCell {
                        row,
                        col: 0,
                        display_col: 0,
                        rect,
                    };
                    let label = (row + 1).to_string();
                    self.header_text(cx, &cell, &label, self.color_header_text, 0.5, 0.0);
                }
                y += h;
            }
            cx.pop_clip_rect();
        }
        if self.show_col_headers && self.show_row_headers {
            self.draw_cell.color = self.color_header;
            self.draw_cell.draw_abs(cx, vp.corner_rect);
        }
    }

    /// Where `header_edges` draws its lines: one point wide on the right
    /// edge of every heading but the last column's, over the middle half of
    /// the heading's height, where the cells' own edge lines fall, and only
    /// those in the heading strip. Nothing with `header_edges` off.
    fn header_edge_rects(&self) -> Vec<Rect> {
        // Within the stated height at the bottom of the strip: a strip made
        // tall for turned names would otherwise run its edges up through
        // the names crossing them. With nothing turned that is the strip.
        let strip = self.flat_header_slot(self.vp.col_header_rect);
        if !self.header_edges || !self.show_col_headers || strip.size.y <= 0.0 {
            return Vec::new();
        }
        let inset = (strip.size.y * 0.25).round();
        self.vp
            .vis_cols
            .iter()
            .copied()
            .filter(|&(display_col, _, _)| display_col + 1 < self.cols)
            .map(|(_, x, w)| x + w)
            .filter(|&edge| edge > strip.pos.x && edge <= strip.pos.x + strip.size.x)
            .map(|edge| Rect {
                pos: dvec2(edge - 1.0, strip.pos.y + inset),
                size: dvec2(1.0, strip.size.y - 2.0 * inset),
            })
            .collect()
    }

    fn draw_header_edges(&mut self, cx: &mut Cx2d) {
        let rects = self.header_edge_rects();
        if rects.is_empty() {
            return;
        }
        cx.push_clip_rect(self.vp.col_header_rect);
        self.draw_overlay.color = self.color_header_edge;
        for rect in rects {
            self.draw_overlay.draw_abs(cx, rect);
        }
        cx.pop_clip_rect();
    }

    /// How the heading of data column `data_col` is drawn: its colour, where
    /// its label sits, and the sort marks after it. With nothing declared
    /// this is the stock heading, the sorted one included.
    fn header_look(&self, data_col: usize) -> HeaderLook {
        let sorted = matches!(self.sort_indicator, Some((c, _)) if c == data_col);
        let color = if sorted && self.color_header_sorted.w > 0.0 {
            self.color_header_sorted
        } else {
            self.color_header_text
        };
        // A column that CAN be sorted says so before it is. Without it
        // there is nothing on screen to tell a sortable heading from a
        // plain one, and the only way to find out is to press every
        // heading in the row.
        //
        // Drawn at the right edge in its own pass rather than stuck on the
        // end of the label: appended, it drags the heading off centre and
        // the marks land in a different place in every column.
        let can_sort = self.sortable && !self.unsortable_cols.contains(&data_col);
        let mark = match self.sort_indicator {
            Some((c, asc)) if c == data_col => Some((if asc { "▲" } else { "▼" }, false)),
            // The filled pair, not the hollow one: the hollow triangles are
            // only in faces this chain does not carry and rendered as tofu,
            // so an unsorted column is the same marks worn lighter. Two
            // means either way from here; one means this way.
            _ if can_sort => Some(("▲▼", true)),
            _ => None,
        };
        HeaderLook {
            color,
            align: self.header_align.clamp(0.0, 1.0),
            mark,
        }
    }

    /// Whether the headings have a text style of their own, or borrow the
    /// cells'.
    fn headers_have_own_text(&self) -> bool {
        self.draw_text_header.text_style.font_size > 0.0
    }

    fn header_draw_text(&mut self) -> &mut DrawText {
        if self.headers_have_own_text() {
            &mut self.draw_text_header
        } else {
            &mut self.draw_text
        }
    }

    fn header_text_width(&mut self, cx: &mut Cx2d, text: &str) -> f64 {
        let dt = self.header_draw_text();
        let laidout = dt.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        laidout.size_in_lpxs.width as f64
    }

    /// A heading's label in `color`, placed by [`header_label_x`].
    fn header_text(
        &mut self,
        cx: &mut Cx2d,
        cell: &GridCell,
        text: &str,
        color: Vec4f,
        align: f64,
        reserve: f64,
    ) {
        let pad = self.cell_pad_x;
        let dt = self.header_draw_text();
        dt.color = color;
        let laidout = dt.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        let tw = laidout.size_in_lpxs.width as f64;
        let th = laidout.size_in_lpxs.height as f64;
        let x = header_label_x(cell.rect, pad, tw, align, reserve);
        let y = cell.rect.pos.y + (cell.rect.size.y - th) * 0.5;
        let overflow = tw > cell.rect.size.x - 2.0 * pad;
        if overflow {
            cx.push_clip_rect(Rect {
                pos: cell.rect.pos,
                size: cell.rect.size - dvec2(1.0, 0.0),
            });
        }
        dt.draw_abs(cx, dvec2(x, y), text);
        if overflow {
            cx.pop_clip_rect();
        }
    }

    /// The sort marks, against the right edge of a heading, in the
    /// heading's `color`. Faded while the column is only sortable, full
    /// once it is sorted, so the row reads as one lit column among several
    /// offers.
    fn header_mark(&mut self, cx: &mut Cx2d, cell: &GridCell, mark: &str, faded: bool, color: Vec4f) {
        let pad = self.cell_pad_x;
        let dt = self.header_draw_text();
        dt.color = if faded {
            Vec4f { w: color.w * 0.45, ..color }
        } else {
            color
        };
        let laidout = dt.layout(cx, 0.0, 0.0, None, false, Align::default(), mark);
        let mw = laidout.size_in_lpxs.width as f64;
        let mh = laidout.size_in_lpxs.height as f64;
        // Clear of the resize grab zone as well as of the padding: the
        // marks are the part of a heading people aim at, and the last few
        // points of a column belong to the edge drag.
        let x = cell.rect.pos.x + cell.rect.size.x - Self::RESIZE_MARGIN - pad - mw;
        let y = cell.rect.pos.y + (cell.rect.size.y - mh) * 0.5;
        dt.draw_abs(cx, dvec2(x, y), mark);
    }

    fn draw_interact_overlay(&mut self, cx: &mut Cx2d) {
        match &self.interact {
            Interact::ColDrag {
                display_col,
                cur_abs,
                ..
            } => {
                let display_col = *display_col;
                let cur_abs = *cur_abs;
                // ghost header following the pointer
                let w = self.col_sizes.size_of(display_col).min(220.0);
                let rect = Rect {
                    pos: dvec2(cur_abs.x - w * 0.5, self.vp.widget_rect.pos.y),
                    size: dvec2(w, self.col_header_height.max(22.0)),
                };
                let mut ghost = self.color_header_active;
                ghost.w = 0.85;
                self.draw_cell.color = ghost;
                self.draw_cell.draw_abs(cx, rect);
                let data_col = self.display_to_data(display_col);
                let label = self.col_label(data_col);
                let cell = GridCell {
                    row: 0,
                    col: data_col,
                    display_col,
                    rect,
                };
                let color = self.header_look(data_col).color;
                self.header_text(cx, &cell, &label, color, self.header_align.clamp(0.0, 1.0), 0.0);
                // The bar where it will land, over the ghost: the gap is
                // most often right under the pointer.
                if let Some(marker) = self.col_drop_marker() {
                    self.draw_overlay.color = self.color_drag_marker;
                    self.draw_overlay.draw_abs(cx, marker);
                }
            }
            Interact::ColResize { display_col, .. } => {
                let x = self.vp.data_rect.pos.x
                    + self.col_sizes.offset_of(*display_col)
                    + self.col_sizes.size_of(*display_col)
                    - self.scroll.x;
                self.draw_overlay.color = self.color_resize_guide;
                self.draw_overlay.draw_abs(cx, Rect {
                    pos: dvec2(x - 1.0, self.vp.widget_rect.pos.y),
                    size: dvec2(2.0, self.vp.widget_rect.size.y),
                });
            }
            Interact::RowResize { row, .. } => {
                let y = self.vp.data_rect.pos.y
                    + self.row_sizes.offset_of(*row)
                    + self.row_sizes.size_of(*row)
                    - self.scroll.y;
                self.draw_overlay.color = self.color_resize_guide;
                self.draw_overlay.draw_abs(cx, Rect {
                    pos: dvec2(self.vp.widget_rect.pos.x, y - 1.0),
                    size: dvec2(self.vp.widget_rect.size.x, 2.0),
                });
            }
            _ => (),
        }
    }

    fn draw_scroll_bars(&mut self, cx: &mut Cx2d) {
        let rect = self.vp.data_rect;
        // ScrollBar::draw_scroll_bar positions its quad with draw_rel against
        // the current turtle origin (it expects to live inside the viewport
        // turtle, as in PortalList). Our widget turtle is offset from the data
        // region by the header strips, so wrap the bars in an abs turtle that
        // exactly covers the data rect.
        cx.begin_turtle(
            Walk {
                abs_pos: Some(rect.pos),
                margin: Default::default(),
                width: Size::Fixed(rect.size.x),
                height: Size::Fixed(rect.size.y),
                ..Default::default()
            },
            Layout::flow_down(),
        );
        let totals = dvec2(self.vp.total_w, self.vp.total_h);
        self.scroll_bar_h.set_scroll_pos_no_action(cx, self.scroll.x);
        self.scroll_bar_h
            .draw_scroll_bar(cx, ScrollAxis::Horizontal, rect, totals);
        self.scroll_bar_v.set_scroll_pos_no_action(cx, self.scroll.y);
        self.scroll_bar_v
            .draw_scroll_bar(cx, ScrollAxis::Vertical, rect, totals);
        cx.end_turtle();
    }

    // ---------------------------------------------------------------
    // scrolling
    // ---------------------------------------------------------------

    pub fn set_scroll(&mut self, cx: &mut Cx, scroll: DVec2) {
        self.scroll = dvec2(scroll.x.max(0.0), scroll.y.max(0.0));
        self.area.redraw(cx);
    }

    pub fn scroll_pos(&self) -> DVec2 {
        self.scroll
    }

    /// `scroll` held to what there is to scroll: from nothing to the rows
    /// and columns past the data area, as the frame last drawn measured it.
    fn scroll_clamped(&self, scroll: DVec2) -> DVec2 {
        let data = self.vp.data_rect.size;
        let max_x = (self.col_sizes.total(self.cols) - data.x).max(0.0);
        let max_y = (self.row_sizes.total(self.rows) - data.y).max(0.0);
        dvec2(scroll.x.clamp(0.0, max_x), scroll.y.clamp(0.0, max_y))
    }

    /// Turn `drag_scrolling` on or off from code, for a host whose list
    /// scrolls under a finger in one layout and not in another.
    pub fn set_drag_scrolling(&mut self, on: bool) {
        self.drag_scrolling = on;
    }

    /// Rows a second a carry on the very edge of the rows scrolls.
    const ROW_DRAG_SCROLL_ROWS: f64 = 10.0;

    /// How fast, in points a second, a carried row's pointer at `abs`
    /// scrolls the grid, up below zero and down above it: nothing until
    /// the pointer is within a row's height inside the top or bottom edge
    /// of the rows, then faster the nearer the edge. Nothing with
    /// `row_drag_scroll` off, and nothing for a pointer that is not over
    /// the rows, where [`Self::row_gap_at`] has no gap either: a carry
    /// taken off the list, to somewhere else or over the headings, stops
    /// the scroll.
    fn row_drag_scroll_speed(&self, abs: DVec2) -> f64 {
        if !self.row_drag_scroll || !self.over_rows(abs) {
            return 0.0;
        }
        let data = self.vp.data_rect;
        let band = self.default_row_height.max(8.0).min(data.size.y * 0.5);
        let top = band - (abs.y - data.pos.y);
        let bottom = band - (data.pos.y + data.size.y - abs.y);
        let rows_per_second = Self::ROW_DRAG_SCROLL_ROWS * self.default_row_height;
        if top > 0.0 {
            -rows_per_second * top / band
        } else if bottom > 0.0 {
            rows_per_second * bottom / band
        } else {
            0.0
        }
    }

    /// The carried row's pointer moved, or the carry began: if it is now
    /// where the grid scrolls, scroll on the next frame.
    fn start_row_drag_scroll(&mut self, cx: &mut Cx) {
        let Interact::CellPress { at, carrying: true, .. } = self.interact else {
            return;
        };
        if self.row_drag_frame.is_none() && self.row_drag_scroll_speed(at) != 0.0 {
            self.row_drag_frame_time = None;
            self.row_drag_frame = Some(cx.new_next_frame());
        }
    }

    /// One step of a carry scrolling at an edge, `dt` seconds long, with
    /// the pointer where it last was. Scrolls no further than there is to
    /// scroll, and when the rows moved says so with `RowDragMoved`, since
    /// the row under the pointer is another one now. Returns whether the
    /// rows moved.
    fn step_row_drag_scroll(&mut self, cx: &mut Cx, dt: f64) -> bool {
        let Interact::CellPress { at, carrying: true, .. } = self.interact else {
            return false;
        };
        let speed = self.row_drag_scroll_speed(at);
        let scroll = self.scroll_clamped(self.scroll + dvec2(0.0, speed * dt));
        if speed == 0.0 || scroll == self.scroll {
            return false;
        }
        self.scroll = scroll;
        self.area.redraw(cx);
        cx.widget_action(self.uid, DataGridAction::RowDragMoved { abs: at });
        true
    }

    /// The frame a carry at an edge asked for came, at `time`. Steps as
    /// far as the time since the last step, or a sixtieth of a second for
    /// the first, and asks for another frame while there is still room to
    /// scroll the way the pointer asks. Once the rows reach the end, or
    /// the pointer leaves the edge, the frames stop until it moves again.
    fn row_drag_scroll_frame(&mut self, cx: &mut Cx, time: f64) {
        let dt = match self.row_drag_frame_time {
            Some(last) => (time - last).clamp(0.0, 0.05),
            None => 1.0 / 60.0,
        };
        self.row_drag_frame = None;
        self.row_drag_frame_time = Some(time);
        self.step_row_drag_scroll(cx, dt);
        let Interact::CellPress { at, carrying: true, .. } = self.interact else {
            return;
        };
        let speed = self.row_drag_scroll_speed(at);
        let room = self.scroll_clamped(self.scroll + dvec2(0.0, speed.signum())) != self.scroll;
        if speed != 0.0 && room {
            self.row_drag_frame = Some(cx.new_next_frame());
        }
    }

    /// (visible rows, visible cols) from the last drawn frame.
    pub fn visible_counts(&self) -> (usize, usize) {
        (self.vp.row1 - self.vp.row0.min(self.vp.row1), self.vp.vis_cols.len())
    }

    /// Scroll the minimum amount needed to bring a cell fully into view.
    pub fn scroll_cell_into_view(&mut self, cx: &mut Cx, row: usize, display_col: usize) {
        // Before the first draw the viewport has no size, and a scroll
        // measured against nothing would leave the row just past the top.
        if self.vp.data_rect.size.x <= 0.0 || self.vp.data_rect.size.y <= 0.0 {
            self.scroll_pending = Some((row, display_col));
        } else {
            self.scroll_to_cell(row, display_col);
        }
        self.area.redraw(cx);
    }

    fn scroll_to_cell(&mut self, row: usize, display_col: usize) {
        let x0 = self.col_sizes.offset_of(display_col);
        let x1 = x0 + self.col_sizes.size_of(display_col);
        let y0 = self.row_sizes.offset_of(row);
        let y1 = y0 + self.row_sizes.size_of(row);
        let vw = self.vp.data_rect.size.x;
        let vh = self.vp.data_rect.size.y;
        if x0 < self.scroll.x {
            self.scroll.x = x0;
        } else if x1 > self.scroll.x + vw {
            self.scroll.x = x1 - vw;
        }
        if y0 < self.scroll.y {
            self.scroll.y = y0;
        } else if y1 > self.scroll.y + vh {
            self.scroll.y = y1 - vh;
        }
    }

    // ---------------------------------------------------------------
    // editing
    // ---------------------------------------------------------------

    /// Seat the editor at a cell with `text` in it and the caret after the
    /// last character. This is the host's answer to
    /// [`DataGridAction::EditCell`]: the grid holds no data, so the text —
    /// the cell's own to amend it, what was typed to replace it — has to
    /// come from the side that does. An edit already under way is
    /// committed first, so a host that reseats freely never loses one.
    ///
    /// The selection moves to the cell and the cell is scrolled into view.
    /// The keyboard follows on the next draw, once the editor has an area
    /// to give it to.
    pub fn edit_cell(&mut self, cx: &mut Cx, row: usize, col: usize, text: &str) {
        if row >= self.rows || col >= self.cols {
            return;
        }
        if self.editing.is_some() {
            self.finish_edit(cx, None);
        }
        let Some(item) = self.item(cx, row, col, live_id!(Editor)) else {
            return;
        };
        item.set_text(cx, text);
        // set_text keeps the caret where it was, which in a fresh field is
        // the start: the next character typed would land before the seed
        // rather than after it.
        item.as_text_input().set_cursor(
            cx,
            Cursor {
                index: text.len(),
                prefer_next_row: false,
            },
            false,
        );
        self.editing = Some((row, col));
        self.edit_focus_pending = true;
        let display_col = self.data_to_display(col);
        if self.selection != GridSelectMode::Off && self.active_cell() != Some((row, col)) {
            self.selected = Some(self.selection_at(row, display_col));
            self.emit_selection_changed(cx);
        }
        self.scroll_cell_into_view(cx, row, display_col);
    }

    /// Close the editor and report what it holds as
    /// [`DataGridAction::CellEdited`], leaving the selection where it is:
    /// how a click elsewhere, or a host's own control, ends an edit. Does
    /// nothing when no editor is seated.
    pub fn commit_edit(&mut self, cx: &mut Cx) {
        self.finish_edit(cx, None);
    }

    /// Put the editor away and keep the cell as it was, reporting
    /// [`DataGridAction::EditCancelled`].
    pub fn cancel_edit(&mut self, cx: &mut Cx) {
        let Some((row, col)) = self.editing.take() else {
            return;
        };
        self.edit_focus_pending = false;
        self.take_keys_back(cx, row, col);
        cx.widget_action(self.uid, DataGridAction::EditCancelled { row, col });
        self.area.redraw(cx);
    }

    /// The keyboard back from a retired editor, if it still holds it. A
    /// commit or a cancel the host asks for outright would otherwise leave
    /// the field focused, and a field the sweep has pooled is never asked
    /// to give the focus up: the arrows would be dead until a click.
    fn take_keys_back(&mut self, cx: &mut Cx, row: usize, col: usize) {
        if let Some((_, editor)) = self.get_item(row, col) {
            if cx.has_key_focus(editor.area()) {
                cx.set_key_focus(self.area);
            }
        }
    }

    /// The cell whose editor is seated, as (row, data col).
    pub fn editing(&self) -> Option<(usize, usize)> {
        self.editing
    }

    /// The commit itself. `step` is how far the selection moves on
    /// afterwards, for the keys that commit and step; `None` leaves it
    /// where the person put it, since a click has already done that.
    fn finish_edit(&mut self, cx: &mut Cx, step: Option<(isize, isize)>) {
        let Some((row, col)) = self.editing.take() else {
            return;
        };
        self.edit_focus_pending = false;
        // From the item, not from the key that closed it: a click away
        // and a host's commit have no key, and the field's text is the
        // one thing every way of closing has in common.
        let text = self
            .get_item(row, col)
            .map(|(_, editor)| editor.text())
            .unwrap_or_default();
        self.take_keys_back(cx, row, col);
        cx.widget_action(self.uid, DataGridAction::CellEdited { row, col, text });
        if let Some((dr, dc)) = step {
            if self.selection != GridSelectMode::Off {
                let display_col = self.data_to_display(col);
                self.selected = Some(self.selection_at(row, display_col));
                self.move_head(cx, dr, dc, false);
            }
            // The keyboard came from the grid and goes back to it, so the
            // arrows work from the cell the selection just landed on.
            cx.set_key_focus(self.area);
        }
        self.area.redraw(cx);
    }

    /// What the seated editor reported this pass, in the grid's own
    /// verbs. Return commits and steps down, up with shift; Tab commits
    /// and steps right, left with shift; Escape cancels; and the editor
    /// losing the keyboard for any other reason — a click on another cell,
    /// or anywhere else at all — commits where it stands. Return is looked
    /// for before the focus loss it causes: the field drops the keyboard
    /// before it reports the return, and a commit that took the loss for
    /// the reason would step nowhere.
    fn handle_editor_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let Some((row, col)) = self.editing else {
            return;
        };
        let Some((_, editor)) = self.get_item(row, col) else {
            return;
        };
        let editor = editor.as_text_input();
        if let Some((_, mods)) = editor.returned(actions) {
            self.finish_edit(cx, Some(if mods.shift { (-1, 0) } else { (1, 0) }));
        } else if let Some(ke) = editor
            .key_down_unhandled(actions)
            .filter(|ke| ke.key_code == KeyCode::Tab)
        {
            self.finish_edit(cx, Some(if ke.modifiers.shift { (0, -1) } else { (0, 1) }));
        } else if editor.escaped(actions) {
            self.cancel_edit(cx);
            cx.set_key_focus(self.area);
        } else if editor.key_focus_lost(actions) {
            self.finish_edit(cx, None);
        }
    }
    /// A key that reaches the grid while an editor is seated: the frame
    /// between [`Self::edit_cell`] and the draw that hands the editor the
    /// keyboard, which two key messages queued behind one stalled frame
    /// can straddle. The key is the editor's. Return, Tab and Escape do
    /// what they would have done in the field, and anything else waits
    /// for it. Read as a fresh request, a Return here would ask the host
    /// to seat a second editor, and seating it would commit the first with
    /// only what was typed before the frame.
    fn key_while_seating(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        let shift = ke.modifiers.shift;
        match ke.key_code {
            KeyCode::ReturnKey | KeyCode::NumpadEnter => {
                self.finish_edit(cx, Some(if shift { (-1, 0) } else { (1, 0) }));
            }
            KeyCode::Tab => {
                self.finish_edit(cx, Some(if shift { (0, -1) } else { (0, 1) }));
            }
            KeyCode::Escape => self.cancel_edit(cx),
            _ => (),
        }
    }
    /// Text typed in that frame goes after the seed, where the next
    /// character would have landed in the field, instead of raising a
    /// second [`DataGridAction::EditCell`] whose answer would commit the
    /// first edit with the seed alone.
    fn typed_while_seating(&mut self, cx: &mut Cx, input: &str) {
        let Some((row, col)) = self.editing else {
            return;
        };
        let Some((_, editor)) = self.get_item(row, col) else {
            return;
        };
        let mut text = editor.text();
        text.push_str(input);
        editor.set_text(cx, &text);
        editor.as_text_input().set_cursor(
            cx,
            Cursor {
                index: text.len(),
                prefer_next_row: false,
            },
            false,
        );
    }

    // ---------------------------------------------------------------
    // hit testing
    // ---------------------------------------------------------------

    const RESIZE_MARGIN: f64 = 4.0;
    /// How far either side of a column's edge a pointer takes hold of it,
    /// in points, and never more than a third of the column on that side,
    /// so a narrow column keeps a middle to press and carry.
    const RESIZE_GRAB: f64 = 6.0;

    fn hit_zone(&self, pos: DVec2) -> HitZone {
        let vp = &self.vp;
        if !vp.widget_rect.contains(pos) {
            return HitZone::Outside;
        }
        if self.show_col_headers && self.show_row_headers && vp.corner_rect.contains(pos) {
            return HitZone::Corner;
        }
        if self.show_col_headers && vp.col_header_rect.contains(pos) {
            let mut resize_edge = None;
            let mut nearest = f64::INFINITY;
            let mut display_col = None;
            let grab = |w: f64| Self::RESIZE_GRAB.min(w / 3.0);
            for (at, (dc, x, w)) in vp.vis_cols.iter().copied().enumerate() {
                let off = pos.x - (x + w);
                let past = vp.vis_cols.get(at + 1).map_or(Self::RESIZE_GRAB, |&(_, _, next)| grab(next));
                if off >= -grab(w) && off <= past && off.abs() < nearest {
                    nearest = off.abs();
                    resize_edge = Some(dc);
                }
                if pos.x >= x && pos.x < x + w {
                    display_col = Some(dc);
                }
            }
            if let Some(dc) = display_col {
                return HitZone::ColHeader {
                    display_col: dc,
                    resize_edge,
                };
            }
            return HitZone::Outside;
        }
        if self.show_row_headers && vp.row_header_rect.contains(pos) {
            let mut y = vp.row0_y;
            let mut resize_edge = None;
            let mut hit_row = None;
            for row in vp.row0..vp.row1 {
                let h = self.row_sizes.size_of(row);
                if (pos.y - (y + h)).abs() <= Self::RESIZE_MARGIN {
                    resize_edge = Some(row);
                }
                if pos.y >= y && pos.y < y + h {
                    hit_row = Some(row);
                }
                y += h;
            }
            if let Some(row) = hit_row {
                return HitZone::RowHeader { row, resize_edge };
            }
            return HitZone::Outside;
        }
        if vp.data_rect.contains(pos) {
            if let Some((row, display_col)) = self.cell_at(pos) {
                return HitZone::Cell { row, display_col };
            }
        }
        HitZone::Outside
    }

    /// The pointer's shape at `abs`: an edge that can be dragged and a
    /// heading that can be carried say so, a cell wears `cell_cursor`, and
    /// anywhere else is the default.
    fn hover_cursor(&self, abs: DVec2) -> MouseCursor {
        match self.hit_zone(abs) {
            HitZone::ColHeader { resize_edge: Some(_), .. } if self.allow_col_resize => MouseCursor::ColResize,
            HitZone::RowHeader { resize_edge: Some(_), .. } if self.allow_row_resize => MouseCursor::RowResize,
            HitZone::ColHeader { display_col, .. } if self.can_move_col(display_col) => MouseCursor::Grab,
            HitZone::Cell { .. } => self.cell_cursor,
            _ => MouseCursor::Default,
        }
    }

    /// The pointer is at `abs`, or has left the grid with `None`: raise the
    /// tip of the heading it rests on, or take the last one down. Once per
    /// heading entered and once per heading left, so a pointer moving
    /// about inside one heading says nothing more.
    ///
    /// A pointer on an edge it could drag raises none, and while a press is
    /// under way, a resize, a carry or a press not yet either, the grid
    /// raises nothing at all.
    fn hover_tip(&mut self, cx: &mut Cx, abs: Option<DVec2>) {
        if !matches!(self.interact, Interact::None) {
            return;
        }
        let over = abs.and_then(|abs| self.tip_at(abs));
        if over == self.tip_head {
            return;
        }
        self.tip_head = over;
        let action = match over {
            Some((display_col, col)) => {
                TipAction::HoverIn(self.header_tips[col].clone(), self.heading_rect(display_col))
            }
            None => TipAction::HoverOut,
        };
        cx.widget_action(self.uid, action);
    }

    /// The heading whose tip a pointer at `abs` would raise, as (display
    /// col, data col): none on a cell, on a heading without a tip, or on an
    /// edge that can be dragged, where the hand is about to resize and a
    /// bubble would only get in its way.
    fn tip_at(&self, abs: DVec2) -> Option<(usize, usize)> {
        match self.hit_zone(abs) {
            HitZone::ColHeader { resize_edge: Some(_), .. } if self.allow_col_resize => None,
            HitZone::ColHeader { display_col, .. } => {
                let col = self.display_to_data(display_col);
                let has_tip = self.header_tips.get(col).is_some_and(|tip| !tip.is_empty());
                has_tip.then_some((display_col, col))
            }
            _ => None,
        }
    }

    /// A press on a heading at `abs` takes any heading tip down at once,
    /// one on screen or one still waiting to show (a tip raised by the move
    /// that brought the pointer there can reach the tip layer after the
    /// press has cleared it), and the heading pressed stays quiet until the
    /// pointer leaves it, so a tip does not come back over a hand that has
    /// just sorted or resized.
    fn press_takes_tip_down(&mut self, cx: &mut Cx, abs: DVec2) {
        if self.header_tips.iter().any(|tip| !tip.is_empty()) {
            cx.widget_action(self.uid, TipAction::HoverOut);
        }
        self.tip_head = self.tip_at(abs);
    }

    /// Where a column heading is drawn, cut to the heading strip: of a
    /// heading scrolled half out of view, only the half that shows.
    fn heading_rect(&self, display_col: usize) -> Rect {
        let strip = self.vp.col_header_rect;
        let x0 = self.vp.data_rect.pos.x + self.col_sizes.offset_of(display_col) - self.scroll.x;
        let x1 = x0 + self.col_sizes.size_of(display_col);
        let (x0, x1) = (x0.max(strip.pos.x), x1.min(strip.pos.x + strip.size.x));
        Rect {
            pos: dvec2(x0, strip.pos.y),
            size: dvec2((x1 - x0).max(0.0), strip.size.y),
        }
    }

    fn cell_at(&self, pos: DVec2) -> Option<(usize, usize)> {
        if self.rows == 0 || self.cols == 0 {
            return None;
        }
        let dx = pos.x - self.vp.data_rect.pos.x + self.scroll.x;
        let dy = pos.y - self.vp.data_rect.pos.y + self.scroll.y;
        if dx < 0.0 || dy < 0.0 {
            return None;
        }
        let (col, _) = self.col_sizes.index_at(dx, self.cols);
        let (row, _) = self.row_sizes.index_at(dy, self.rows);
        if dx > self.vp.total_w || dy > self.vp.total_h {
            return None;
        }
        Some((row, col))
    }

    /// Like `cell_at` but clamps to the nearest cell, for drag-selection.
    fn cell_at_clamped(&self, pos: DVec2) -> (usize, usize) {
        let dx = (pos.x - self.vp.data_rect.pos.x + self.scroll.x).max(0.0);
        let dy = (pos.y - self.vp.data_rect.pos.y + self.scroll.y).max(0.0);
        let (col, _) = self.col_sizes.index_at(dx.min(self.vp.total_w - 0.5), self.cols);
        let (row, _) = self.row_sizes.index_at(dy.min(self.vp.total_h - 0.5), self.rows);
        (row, col)
    }

    /// Whether the heading at `display_col` can be dragged somewhere else.
    fn can_move_col(&self, display_col: usize) -> bool {
        self.allow_col_reorder && !self.unmovable_cols.contains(&self.display_to_data(display_col))
    }

    /// The gap between headings, from 0 before the first to `cols` after
    /// the last, that a heading dragged from `from` drops into with the
    /// pointer at `abs_x`: in front of the heading under the pointer left
    /// of its middle, behind it right of it. Kept between the unmovable
    /// columns on either side of `from`, so those stay where they are.
    fn col_drag_insert_at(&self, from: usize, abs_x: f64) -> usize {
        let dx = abs_x - self.vp.data_rect.pos.x + self.scroll.x;
        let gap = if dx <= 0.0 {
            0
        } else {
            let (col, within) = self.col_sizes.index_at(dx, self.cols);
            let w = self.col_sizes.size_of(col);
            if within > w * 0.5 {
                (col + 1).min(self.cols)
            } else {
                col
            }
        };
        let fixed = |d: usize| self.unmovable_cols.contains(&self.display_to_data(d));
        let lo = (0..from).rev().find(|&d| fixed(d)).map_or(0, |d| d + 1);
        let hi = (from + 1..self.cols).find(|&d| fixed(d)).unwrap_or(self.cols);
        gap.clamp(lo, hi)
    }

    /// The bar showing where a dragged heading will land: across the
    /// headings and the rows at the gap it would drop into, kept inside the
    /// columns' strip so a gap at either end still shows.
    fn col_drop_marker(&self) -> Option<Rect> {
        let Interact::ColDrag { insert_at, .. } = self.interact else {
            return None;
        };
        let data = self.vp.data_rect;
        let widget = self.vp.widget_rect;
        let x = data.pos.x + self.col_sizes.offset_of(insert_at) - self.scroll.x;
        let right = (data.pos.x + data.size.x - 2.0).max(data.pos.x);
        Some(Rect {
            pos: dvec2((x - 1.0).clamp(data.pos.x, right), widget.pos.y),
            size: dvec2(2.0, widget.size.y),
        })
    }

    // ---------------------------------------------------------------
    // interaction
    // ---------------------------------------------------------------

    fn emit_selection_changed(&mut self, cx: &mut Cx) {
        cx.widget_action(
            self.uid,
            DataGridAction::SelectionChanged {
                selection: self.selected,
            },
        );
        self.area.redraw(cx);
    }

    /// What a press or a key selects in this mode: cells, or rows. `Off`
    /// never gets this far.
    fn press_kind(&self) -> GridSelectKind {
        match self.selection {
            GridSelectMode::Rows => GridSelectKind::Rows,
            _ => GridSelectKind::Cells,
        }
    }

    /// A fresh selection at one cell, of the kind this mode selects.
    fn selection_at(&self, row: usize, display_col: usize) -> GridSelection {
        GridSelection {
            kind: self.press_kind(),
            anchor: (row, display_col),
            head: (row, display_col),
        }
    }

    fn select_cell(&mut self, cx: &mut Cx, row: usize, display_col: usize, extend: bool) {
        let kind = self.press_kind();
        match (&mut self.selected, extend) {
            (Some(sel), true) => {
                sel.head = (row, display_col);
                sel.kind = kind;
            }
            _ => {
                self.selected = Some(self.selection_at(row, display_col));
            }
        }
        self.emit_selection_changed(cx);
    }

    /// A primary press on a cell: what it selects in this mode, the drag
    /// it starts, and what it reports. The pointer handler calls this and
    /// so do the tests, which have no window to press in.
    fn press_cell(
        &mut self,
        cx: &mut Cx,
        row: usize,
        display_col: usize,
        abs: DVec2,
        modifiers: KeyModifiers,
        tap_count: u32,
    ) {
        let uid = self.uid;
        // With the selection off there is nothing to select and nothing to
        // rubber-band, and a row that can be carried, or a grid dragged to
        // scroll, never rubber-bands: the press is held only to tell a
        // click from a drag.
        let selects = self.selection != GridSelectMode::Off;
        if selects {
            self.select_cell(cx, row, display_col, modifiers.shift);
        }
        self.interact = Interact::CellPress {
            row,
            display_col,
            down_abs: abs,
            modifiers,
            rubber_band: selects && !self.row_drag && !self.drag_scrolling,
            travelled: false,
            carrying: false,
            scroll_at_press: self.scroll,
            at: abs,
        };
        let col = self.display_to_data(display_col);
        cx.widget_action(
            uid,
            DataGridAction::CellClicked {
                row,
                col,
                modifiers,
            },
        );
        if tap_count > 1 {
            cx.widget_action(uid, DataGridAction::CellDoubleClicked { row, col });
            // And the same request F2 makes, so a host that
            // seats the grid's editor answers one action for
            // the keys and the mouse alike. A double-click on
            // the cell being edited lands on the editor, not
            // here.
            cx.widget_action(
                uid,
                DataGridAction::EditCell {
                    row,
                    col,
                    replace: None,
                },
            );
        }
    }

    /// The pointer moved while a cell was pressed. Past `drag_threshold`
    /// the press stops being a click. With `drag_scrolling` on it scrolls
    /// the grid by the pointer's travel; with `row_drag` on it starts
    /// carrying its row, once; and otherwise it drags out the selection as
    /// a press always has.
    fn move_cell_press(&mut self, cx: &mut Cx, abs: DVec2) {
        let threshold = self.drag_threshold;
        let row_drag = self.row_drag;
        let drag_scrolling = self.drag_scrolling;
        let Interact::CellPress {
            row,
            display_col,
            down_abs,
            modifiers,
            rubber_band,
            travelled,
            carrying,
            scroll_at_press,
            at,
        } = &mut self.interact
        else {
            return;
        };
        *at = abs;
        if *carrying {
            cx.widget_action(self.uid, DataGridAction::RowDragMoved { abs });
            self.start_row_drag_scroll(cx);
            return;
        }
        if (abs - *down_abs).length() > threshold {
            *travelled = true;
        }
        if drag_scrolling {
            // Measured from where the press went down, not from where it
            // passed the threshold, so the point pressed stays under the
            // pointer.
            let target = travelled.then(|| *scroll_at_press - (abs - *down_abs));
            if let Some(target) = target {
                let scroll = self.scroll_clamped(target);
                if scroll != self.scroll {
                    self.scroll = scroll;
                    self.area.redraw(cx);
                }
            }
            return;
        }
        if row_drag {
            if *travelled && *row < self.rows {
                *carrying = true;
                let (row, display_col, modifiers) = (*row, *display_col, *modifiers);
                let col = self.display_to_data(display_col);
                cx.widget_action(
                    self.uid,
                    DataGridAction::RowDragStarted {
                        row,
                        col,
                        abs,
                        modifiers,
                    },
                );
                self.start_row_drag_scroll(cx);
            }
            return;
        }
        if !*rubber_band {
            return;
        }
        let (row, col) = self.cell_at_clamped(abs);
        let changed = match &self.selected {
            Some(sel) => sel.head != (row, col),
            None => true,
        };
        if changed {
            let kind = self.press_kind();
            if let Some(sel) = &mut self.selected {
                sel.head = (row, col);
                sel.kind = kind;
            } else {
                self.selected = Some(self.selection_at(row, col));
            }
            // drag auto-scroll
            let dr = self.vp.data_rect;
            if abs.x > dr.pos.x + dr.size.x {
                self.scroll.x += (abs.x - dr.pos.x - dr.size.x).min(40.0);
            } else if abs.x < dr.pos.x {
                self.scroll.x = (self.scroll.x - (dr.pos.x - abs.x).min(40.0)).max(0.0);
            }
            if abs.y > dr.pos.y + dr.size.y {
                self.scroll.y += (abs.y - dr.pos.y - dr.size.y).min(40.0);
            } else if abs.y < dr.pos.y {
                self.scroll.y = (self.scroll.y - (dr.pos.y - abs.y).min(40.0)).max(0.0);
            }
            self.emit_selection_changed(cx);
        }
    }

    /// The press on a cell came up at `abs` with `modifiers` held. One
    /// that never went further than `drag_threshold` was a click and says
    /// so; one that carried its row or dragged out a selection is over.
    fn release_cell_press(&mut self, cx: &mut Cx, abs: DVec2, modifiers: KeyModifiers) {
        let Interact::CellPress {
            row,
            display_col,
            down_abs,
            travelled,
            carrying,
            ..
        } = std::mem::take(&mut self.interact)
        else {
            return;
        };
        if carrying {
            self.row_drag_frame = None;
            cx.widget_action(self.uid, DataGridAction::RowDragEnded { abs, modifiers });
            return;
        }
        if travelled || row >= self.rows || (abs - down_abs).length() > self.drag_threshold {
            return;
        }
        let col = self.display_to_data(display_col);
        cx.widget_action(
            self.uid,
            DataGridAction::CellReleased {
                row,
                col,
                modifiers,
            },
        );
    }

    /// A secondary press at `abs`: a menu asked for on a cell or on a
    /// heading, and anywhere else nothing. It selects nothing, sorts
    /// nothing and leaves the keyboard where it is. A press that lands
    /// while another gesture holds the pointer, a row being carried or a
    /// column being dragged, belongs to that gesture and asks for no menu.
    fn context_press(&mut self, cx: &mut Cx, abs: DVec2) {
        if !matches!(self.interact, Interact::None) {
            return;
        }
        let action = match self.hit_zone(abs) {
            HitZone::Cell { row, display_col } => DataGridAction::CellContextMenu {
                row,
                col: self.display_to_data(display_col),
                abs,
            },
            HitZone::ColHeader { display_col, .. } => DataGridAction::HeaderContextMenu {
                col: self.display_to_data(display_col),
                abs,
            },
            _ => return,
        };
        cx.widget_action(self.uid, action);
    }

    /// A press and release on a heading that did not become a column
    /// drag. The spreadsheet selects the column; a list sorts and leaves
    /// its selection alone, since a heading is not something a list picks.
    fn click_header(&mut self, cx: &mut Cx, display_col: usize, modifiers: KeyModifiers) {
        let uid = self.uid;
        if self.selection == GridSelectMode::Cells {
            let extend = modifiers.shift;
            match (&mut self.selected, extend) {
                (Some(sel), true) if sel.kind == GridSelectKind::Cols => {
                    sel.head = (sel.head.0, display_col);
                }
                _ => {
                    self.selected = Some(GridSelection {
                        kind: GridSelectKind::Cols,
                        anchor: (0, display_col),
                        head: (self.rows.saturating_sub(1), display_col),
                    });
                }
            }
            self.emit_selection_changed(cx);
        }
        let col = self.display_to_data(display_col);
        // Unsorted, then up, then down, then unsorted
        // again. The third press has to be able to get
        // back to the order the data arrived in, which a
        // two-state toggle can never do.
        if self.sortable && !self.unsortable_cols.contains(&col) {
            let next = match self.sort_indicator {
                Some((c, true)) if c == col => Some((col, false)),
                Some((c, false)) if c == col => None,
                _ => Some((col, true)),
            };
            self.sort_indicator = next;
            self.redraw(cx);
            cx.widget_action(
                uid,
                DataGridAction::SortChanged {
                    col,
                    ascending: next.map(|(_, asc)| asc),
                },
            );
        }
        cx.widget_action(
            uid,
            DataGridAction::HeaderClicked {
                col,
                display_col,
                modifiers,
            },
        );
    }

    fn move_head(&mut self, cx: &mut Cx, dr: isize, dc: isize, extend: bool) {
        if self.rows == 0 || self.cols == 0 || self.selection == GridSelectMode::Off {
            return;
        }
        let (row, col) = match self.selected {
            Some(sel) => sel.head,
            None => (0, 0),
        };
        let row = (row as isize + dr).clamp(0, self.rows as isize - 1) as usize;
        let col = (col as isize + dc).clamp(0, self.cols as isize - 1) as usize;
        let kind = self.press_kind();
        if extend {
            if let Some(sel) = &mut self.selected {
                sel.head = (row, col);
                sel.kind = kind;
            } else {
                self.selected = Some(self.selection_at(row, col));
            }
        } else {
            self.selected = Some(self.selection_at(row, col));
        }
        self.scroll_cell_into_view(cx, row, col);
        self.emit_selection_changed(cx);
    }

    fn move_column(&mut self, cx: &mut Cx, from: usize, to: usize) {
        let to = if to > from { to - 1 } else { to };
        if from == to || from >= self.cols || to >= self.cols {
            return;
        }
        let order = self.col_order.get_or_insert_with(|| {
            (0..self.cols as u32).collect()
        });
        let moved = order.remove(from);
        order.insert(to, moved);
        self.col_sizes.apply_move(from, to);
        self.selected = None;
        // A pointer that comes back before the redraw finds the columns
        // where they are now.
        self.compute_viewport();
        cx.widget_action(
            self.uid,
            DataGridAction::ColumnMoved {
                from_display: from,
                to_display: to,
            },
        );
        cx.widget_action(
            self.uid,
            DataGridAction::ColumnOrderChanged {
                order: self.col_order(),
            },
        );
        self.area.redraw(cx);
    }

    fn handle_key_down(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        let uid = self.uid;
        let shift = ke.modifiers.shift;
        let cmd = ke.modifiers.logo || ke.modifiers.control;
        let page = (self.vp.data_rect.size.y / self.row_sizes.size_of(0).max(1.0)).max(1.0) as isize;
        match ke.key_code {
            KeyCode::ArrowUp => {
                if cmd {
                    self.move_head_to(cx, Some(0), None, shift);
                } else {
                    self.move_head(cx, -1, 0, shift);
                }
            }
            KeyCode::ArrowDown => {
                if cmd {
                    self.move_head_to(cx, Some(self.rows.saturating_sub(1)), None, shift);
                } else {
                    self.move_head(cx, 1, 0, shift);
                }
            }
            KeyCode::ArrowLeft => {
                if cmd {
                    self.move_head_to(cx, None, Some(0), shift);
                } else {
                    self.move_head(cx, 0, -1, shift);
                }
            }
            KeyCode::ArrowRight => {
                if cmd {
                    self.move_head_to(cx, None, Some(self.cols.saturating_sub(1)), shift);
                } else {
                    self.move_head(cx, 0, 1, shift);
                }
            }
            KeyCode::PageUp => self.move_head(cx, -page, 0, shift),
            KeyCode::PageDown => self.move_head(cx, page, 0, shift),
            KeyCode::Home => {
                if cmd {
                    self.move_head_to(cx, Some(0), Some(0), shift);
                } else {
                    self.move_head_to(cx, None, Some(0), shift);
                }
            }
            KeyCode::End => {
                if cmd {
                    self.move_head_to(
                        cx,
                        Some(self.rows.saturating_sub(1)),
                        Some(self.cols.saturating_sub(1)),
                        shift,
                    );
                } else {
                    self.move_head_to(cx, None, Some(self.cols.saturating_sub(1)), shift);
                }
            }
            KeyCode::Tab => {
                self.move_head(cx, 0, if shift { -1 } else { 1 }, false);
            }
            KeyCode::ReturnKey | KeyCode::NumpadEnter => {
                if let Some((row, col)) = self.active_cell() {
                    cx.widget_action(
                        uid,
                        DataGridAction::EditCell {
                            row,
                            col,
                            replace: None,
                        },
                    );
                }
            }
            KeyCode::F2 => {
                if let Some((row, col)) = self.active_cell() {
                    cx.widget_action(
                        uid,
                        DataGridAction::EditCell {
                            row,
                            col,
                            replace: None,
                        },
                    );
                }
            }
            KeyCode::Delete | KeyCode::Backspace => {
                if self.selection().is_some() {
                    cx.widget_action(uid, DataGridAction::ClearCells);
                }
            }
            KeyCode::KeyA if cmd && self.selection != GridSelectMode::Off => {
                self.selected = Some(GridSelection {
                    kind: GridSelectKind::All,
                    anchor: (0, 0),
                    head: (0, 0),
                });
                self.emit_selection_changed(cx);
            }
            _ => (),
        }
    }

    fn move_head_to(&mut self, cx: &mut Cx, row: Option<usize>, col: Option<usize>, extend: bool) {
        if self.selection == GridSelectMode::Off {
            return;
        }
        let (cur_row, cur_col) = match self.selected {
            Some(sel) => sel.head,
            None => (0, 0),
        };
        let row = row.unwrap_or(cur_row).min(self.rows.saturating_sub(1));
        let col = col.unwrap_or(cur_col).min(self.cols.saturating_sub(1));
        if extend {
            if let Some(sel) = &mut self.selected {
                sel.head = (row, col);
            } else {
                self.selected = Some(self.selection_at(row, col));
            }
        } else {
            self.selected = Some(self.selection_at(row, col));
        }
        self.scroll_cell_into_view(cx, row, col);
        self.emit_selection_changed(cx);
    }
}

impl WidgetNode for DataGrid {
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
            visit(LiveId(*item_id), item.widget.clone());
        }
    }
    fn skip_widget_tree_search(&self) -> bool {
        true
    }
    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        for item in self.items.values() {
            item.widget.find_widgets_from_point(cx, point, found);
        }
    }
}

impl Widget for DataGrid {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.uid;

        // Where the widget actually is this frame, after any deferred
        // alignment by its parents; the cached viewport follows it.
        let drawn = self.area.rect(cx);
        if drawn.size.x > 0.0 && drawn.size.y > 0.0 {
            let delta = drawn.pos - self.vp.widget_rect.pos;
            if delta.x != 0.0 || delta.y != 0.0 {
                self.vp.translate(delta);
            }
        }

        // Scroll bar drag / animation
        let mut sx = None;
        let mut sy = None;
        self.scroll_bar_h.handle_event_with(cx, event, &mut |_cx, action| {
            if let ScrollBarAction::Scroll { scroll_pos, .. } = action {
                sx = Some(scroll_pos);
            }
        });
        self.scroll_bar_v.handle_event_with(cx, event, &mut |_cx, action| {
            if let ScrollBarAction::Scroll { scroll_pos, .. } = action {
                sy = Some(scroll_pos);
            }
        });
        // Wheel / trackpad over the whole grid
        self.scroll_bar_h
            .handle_scroll_event(cx, event, self.area, &mut |_cx, action| {
                if let ScrollBarAction::Scroll { scroll_pos, .. } = action {
                    sx = Some(scroll_pos);
                }
            });
        self.scroll_bar_v
            .handle_scroll_event(cx, event, self.area, &mut |_cx, action| {
                if let ScrollBarAction::Scroll { scroll_pos, .. } = action {
                    sy = Some(scroll_pos);
                }
            });
        if sx.is_some() || sy.is_some() {
            if let Some(sx) = sx {
                self.scroll.x = sx;
            }
            if let Some(sy) = sy {
                self.scroll.y = sy;
            }
            self.area.redraw(cx);
        }

        // Forward to hosted cell widgets, except while a grid gesture owns the pointer
        let suppress_children = !matches!(self.interact, Interact::None)
            && matches!(
                event,
                Event::MouseDown(_) | Event::MouseMove(_) | Event::TouchUpdate(_)
            );
        if !suppress_children {
            for (_item_id, item) in self.items.iter_mut() {
                let item_uid = item.widget.widget_uid();
                cx.group_widget_actions(uid, item_uid, |cx| {
                    item.widget.handle_event(cx, event, scope)
                });
            }
        }

        // An edit the grid shrank out from under: the host hears the
        // cancel it was promised, and the keyboard comes back from an
        // editor that is about to be retired.
        if let Some((row, col)) = self.edit_dropped.take() {
            self.take_keys_back(cx, row, col);
            cx.widget_action(self.uid, DataGridAction::EditCancelled { row, col });
        }
        if let Event::Actions(actions) = event {
            self.handle_editor_actions(cx, actions);
        }
        if let Some(ne) = self.row_drag_frame.and_then(|frame| frame.is_event(event)) {
            self.row_drag_scroll_frame(cx, ne.time);
        }
        // The pointer gone from the window, or an overlay clearing every
        // hover as it opens: a heading's tip goes with it.
        if matches!(event, Event::MouseLeave(_) | Event::ClearHover) {
            self.hover_tip(cx, None);
        }

        match event.hits(cx, self.area) {
            Hit::KeyFocus(_) | Hit::KeyFocusLost(_) => {
                self.area.redraw(cx);
            }
            Hit::KeyDown(ke) => {
                if self.editing.is_some() {
                    self.key_while_seating(cx, &ke);
                } else {
                    self.handle_key_down(cx, &ke);
                }
            }
            Hit::TextInput(te) => {
                if !te.input.is_empty() && !te.was_paste {
                    if self.editing.is_some() {
                        self.typed_while_seating(cx, &te.input);
                    } else if let Some((row, col)) = self.active_cell() {
                        cx.widget_action(
                            uid,
                            DataGridAction::EditCell {
                                row,
                                col,
                                replace: Some(te.input.clone()),
                            },
                        );
                    }
                }
            }
            Hit::TextCopy(te) => {
                if let (Some(sel), Some(provider)) = (self.selection(), &mut self.copy_provider) {
                    *te.response.borrow_mut() = Some(provider(&sel));
                }
            }
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                cx.set_cursor(self.hover_cursor(fe.abs));
                self.hover_tip(cx, Some(fe.abs));
            }
            Hit::FingerHoverOut(_) => self.hover_tip(cx, None),
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if self.grab_key_focus {
                    cx.set_key_focus(self.area);
                }
                match self.hit_zone(fe.abs) {
                    HitZone::Corner if self.selection != GridSelectMode::Off => {
                        self.selected = Some(GridSelection {
                            kind: GridSelectKind::All,
                            anchor: (0, 0),
                            head: (0, 0),
                        });
                        self.emit_selection_changed(cx);
                    }
                    HitZone::ColHeader {
                        display_col,
                        resize_edge,
                    } => {
                        self.press_takes_tip_down(cx, fe.abs);
                        if let (Some(edge), true) = (resize_edge, self.allow_col_resize) {
                            self.interact = Interact::ColResize {
                                display_col: edge,
                                start_size: self.col_sizes.size_of(edge),
                                start_abs: fe.abs.x,
                            };
                            cx.set_cursor(MouseCursor::ColResize);
                        } else {
                            self.interact = Interact::ColDragPending {
                                display_col,
                                down_abs: fe.abs,
                                modifiers: fe.modifiers,
                            };
                        }
                    }
                    HitZone::RowHeader { row, resize_edge } => {
                        if let (Some(edge), true) = (resize_edge, self.allow_row_resize) {
                            self.interact = Interact::RowResize {
                                row: edge,
                                start_size: self.row_sizes.size_of(edge),
                                start_abs: fe.abs.y,
                            };
                            cx.set_cursor(MouseCursor::RowResize);
                        } else if self.selection != GridSelectMode::Off {
                            let extend = fe.modifiers.shift;
                            match (&mut self.selected, extend) {
                                (Some(sel), true) if sel.kind == GridSelectKind::Rows => {
                                    sel.head = (row, sel.head.1);
                                }
                                _ => {
                                    self.selected = Some(GridSelection {
                                        kind: GridSelectKind::Rows,
                                        anchor: (row, 0),
                                        head: (row, self.cols.saturating_sub(1)),
                                    });
                                }
                            }
                            self.emit_selection_changed(cx);
                        }
                    }
                    HitZone::Cell { row, display_col } => {
                        self.press_cell(cx, row, display_col, fe.abs, fe.modifiers, fe.tap_count);
                    }
                    HitZone::Corner | HitZone::Outside => (),
                }
            }
            // The right button asks for a menu, and does nothing else: no
            // key focus, no selection, no sort.
            Hit::FingerDown(fe) if fe.mouse_button().is_some_and(|b| b.is_secondary()) => {
                self.context_press(cx, fe.abs);
            }
            Hit::FingerMove(fe) => match &mut self.interact {
                Interact::ColResize {
                    display_col,
                    start_size,
                    start_abs,
                } => {
                    let display_col = *display_col;
                    let size = (*start_size + fe.abs.x - *start_abs).max(self.min_col_width);
                    self.col_sizes.set(display_col, size);
                    self.area.redraw(cx);
                }
                Interact::RowResize {
                    row,
                    start_size,
                    start_abs,
                } => {
                    let row = *row;
                    let size = (*start_size + fe.abs.y - *start_abs).max(self.min_row_height);
                    self.row_sizes.set(row, size);
                    self.area.redraw(cx);
                }
                Interact::ColDragPending {
                    display_col,
                    down_abs,
                    ..
                } => {
                    let (display_col, down_abs) = (*display_col, *down_abs);
                    if (fe.abs - down_abs).length() > self.drag_threshold
                        && self.can_move_col(display_col)
                    {
                        let insert_at = self.col_drag_insert_at(display_col, fe.abs.x);
                        self.interact = Interact::ColDrag {
                            display_col,
                            cur_abs: fe.abs,
                            insert_at,
                        };
                        cx.set_cursor(MouseCursor::Grabbing);
                        self.area.redraw(cx);
                    }
                }
                Interact::ColDrag { display_col, .. } => {
                    let from = *display_col;
                    let insert = self.col_drag_insert_at(from, fe.abs.x);
                    if let Interact::ColDrag {
                        cur_abs, insert_at, ..
                    } = &mut self.interact
                    {
                        *cur_abs = fe.abs;
                        *insert_at = insert;
                    }
                    // edge auto-scroll while dragging a header, held to
                    // what there is to scroll so the next gap is measured
                    // against columns that are really there
                    let dr = self.vp.data_rect;
                    if fe.abs.x > dr.pos.x + dr.size.x - 30.0 {
                        self.scroll = self.scroll_clamped(self.scroll + dvec2(14.0, 0.0));
                    } else if fe.abs.x < dr.pos.x + 30.0 {
                        self.scroll = self.scroll_clamped(self.scroll - dvec2(14.0, 0.0));
                    }
                    self.area.redraw(cx);
                }
                Interact::CellPress { .. } => self.move_cell_press(cx, fe.abs),
                Interact::None => (),
            },
            // A press taken away (a list or the host took the finger): a
            // live resize keeps the size it reached and says so; nothing else
            // happens — no header click, no column move, no cell release or
            // row drop.
            Hit::FingerUp(fe) if fe.cancelled => {
                match std::mem::take(&mut self.interact) {
                    Interact::ColResize { display_col, .. } => {
                        let col = self.display_to_data(display_col);
                        cx.widget_action(
                            uid,
                            DataGridAction::ColumnResized {
                                col,
                                display_col,
                                width: self.col_sizes.size_of(display_col),
                            },
                        );
                    }
                    Interact::RowResize { row, .. } => {
                        cx.widget_action(
                            uid,
                            DataGridAction::RowResized {
                                row,
                                height: self.row_sizes.size_of(row),
                            },
                        );
                    }
                    _ => {}
                }
                cx.set_cursor(MouseCursor::Default);
                self.area.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                match std::mem::take(&mut self.interact) {
                    Interact::ColResize { display_col, .. } => {
                        let col = self.display_to_data(display_col);
                        cx.widget_action(
                            uid,
                            DataGridAction::ColumnResized {
                                col,
                                display_col,
                                width: self.col_sizes.size_of(display_col),
                            },
                        );
                    }
                    Interact::RowResize { row, .. } => {
                        cx.widget_action(
                            uid,
                            DataGridAction::RowResized {
                                row,
                                height: self.row_sizes.size_of(row),
                            },
                        );
                    }
                    Interact::ColDragPending {
                        display_col,
                        modifiers,
                        ..
                    } => {
                        // A press-and-release on a header: the sort and the
                        // click, and the column too in a spreadsheet.
                        self.click_header(cx, display_col, modifiers);
                    }
                    Interact::ColDrag {
                        display_col,
                        insert_at,
                        ..
                    } => {
                        self.move_column(cx, display_col, insert_at);
                        cx.set_cursor(MouseCursor::Default);
                    }
                    // Only the button that pressed ends the press.
                    press @ Interact::CellPress { .. } => {
                        self.interact = press;
                        if fe.is_primary_hit() {
                            self.release_cell_press(cx, fe.abs, fe.modifiers);
                        }
                    }
                    Interact::None => (),
                }
                self.area.redraw(cx);
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ()) {
            self.begin(cx, walk);
            return DrawStep::make_step();
        }
        self.end(cx);
        self.draw_state.end();
        DrawStep::done()
    }
}

impl DataGridRef {
    pub fn set_grid_size(&self, rows: usize, cols: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_grid_size(rows, cols);
        }
    }

    pub fn set_col_labels(&self, labels: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_col_labels(labels);
        }
    }

    pub fn set_col_width(&self, display_col: usize, width: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_col_width(display_col, width);
        }
    }

    /// See [`DataGrid::set_col_widths`].
    pub fn set_col_widths(&self, cx: &mut Cx, widths: &[f64]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_col_widths(cx, widths);
        }
    }

    /// See [`DataGrid::col_widths`].
    pub fn col_widths(&self) -> Vec<f64> {
        self.borrow().map(|inner| inner.col_widths()).unwrap_or_default()
    }

    /// See [`DataGrid::data_width`].
    pub fn data_width(&self) -> f64 {
        self.borrow().map(|inner| inner.data_width()).unwrap_or(0.0)
    }

    /// See [`DataGrid::set_row_height`]; redraws.
    pub fn set_row_height(&self, cx: &mut Cx, row: usize, height: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_row_height(row, height);
            inner.area.redraw(cx);
        }
    }

    /// See [`DataGrid::clear_row_heights`]; redraws.
    pub fn clear_row_heights(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_row_heights();
            inner.area.redraw(cx);
        }
    }

    /// See [`DataGrid::row_at`].
    pub fn row_at(&self, abs: DVec2) -> Option<usize> {
        self.borrow().and_then(|inner| inner.row_at(abs))
    }

    /// See [`DataGrid::row_rect`].
    pub fn row_rect(&self, row: usize) -> Option<Rect> {
        self.borrow().and_then(|inner| inner.row_rect(row))
    }

    /// See [`DataGrid::row_gap_at`].
    pub fn row_gap_at(&self, abs: DVec2) -> Option<usize> {
        self.borrow().and_then(|inner| inner.row_gap_at(abs))
    }

    /// See [`DataGrid::set_scroll`]. The grid clamps it to what there is
    /// to scroll on its next draw.
    pub fn set_scroll(&self, cx: &mut Cx, scroll: DVec2) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_scroll(cx, scroll);
        }
    }

    /// See [`DataGrid::scroll_pos`].
    pub fn scroll_pos(&self) -> DVec2 {
        self.borrow().map(|inner| inner.scroll_pos()).unwrap_or_default()
    }

    /// See [`DataGrid::set_drag_scrolling`].
    pub fn set_drag_scrolling(&self, on: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_drag_scrolling(on);
        }
    }

    /// The sort a heading press just asked for, if it changed this pass.
    /// `Some((col, None))` means that column went back to unsorted.
    ///
    /// Every action from this grid is looked at, not just the first: one
    /// press on a heading raises the selection change, the sort and the
    /// header click, in that order, and asking only for the first one
    /// hands back the selection and reports no sort at all.
    pub fn sort_changed(&self, actions: &Actions) -> Option<(usize, Option<bool>)> {
        actions
            .filter_widget_actions_cast::<DataGridAction>(self.widget_uid())
            .find_map(|a| match a {
                DataGridAction::SortChanged { col, ascending } => Some((col, ascending)),
                _ => None,
            })
    }

    pub fn sort(&self) -> Option<(usize, bool)> {
        self.borrow().and_then(|inner| inner.sort())
    }

    /// See [`DataGrid::set_header_tips`].
    pub fn set_header_tips(&self, tips: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_header_tips(tips);
        }
    }

    pub fn set_unsortable_cols(&self, cols: Vec<usize>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_unsortable_cols(cols);
        }
    }

    /// See [`DataGrid::set_unmovable_cols`].
    pub fn set_unmovable_cols(&self, cols: Vec<usize>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_unmovable_cols(cols);
        }
    }

    /// See [`DataGrid::col_order`].
    pub fn col_order(&self) -> Vec<usize> {
        self.borrow().map(|inner| inner.col_order()).unwrap_or_default()
    }

    /// See [`DataGrid::set_col_order`].
    pub fn set_col_order(&self, cx: &mut Cx, order: &[usize]) -> bool {
        self.borrow_mut()
            .map(|mut inner| inner.set_col_order(cx, order))
            .unwrap_or(false)
    }

    pub fn set_sort_indicator(&self, sort: Option<(usize, bool)>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_sort_indicator(sort);
        }
    }

    pub fn selection(&self) -> Option<GridSelection> {
        self.borrow().and_then(|inner| inner.selection())
    }

    pub fn set_selection(&self, cx: &mut Cx, selection: Option<GridSelection>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selection(cx, selection);
        }
    }

    pub fn active_cell(&self) -> Option<(usize, usize)> {
        self.borrow().and_then(|inner| inner.active_cell())
    }

    pub fn display_to_data(&self, display_col: usize) -> usize {
        self.borrow()
            .map(|inner| inner.display_to_data(display_col))
            .unwrap_or(display_col)
    }

    pub fn scroll_cell_into_view(&self, cx: &mut Cx, row: usize, display_col: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.scroll_cell_into_view(cx, row, display_col);
        }
    }

    pub fn visible_counts(&self) -> (usize, usize) {
        self.borrow()
            .map(|inner| inner.visible_counts())
            .unwrap_or((0, 0))
    }

    pub fn get_item(&self, row: usize, col: usize) -> Option<(LiveId, WidgetRef)> {
        self.borrow().and_then(|inner| inner.get_item(row, col))
    }

    /// See [`DataGrid::edit_cell`].
    pub fn edit_cell(&self, cx: &mut Cx, row: usize, col: usize, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.edit_cell(cx, row, col, text);
        }
    }

    /// See [`DataGrid::commit_edit`].
    pub fn commit_edit(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.commit_edit(cx);
        }
    }

    /// See [`DataGrid::cancel_edit`].
    pub fn cancel_edit(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.cancel_edit(cx);
        }
    }

    /// The cell whose editor is seated, as (row, data col).
    pub fn editing(&self) -> Option<(usize, usize)> {
        self.borrow().and_then(|inner| inner.editing())
    }

    pub fn set_copy_provider(&self, provider: Box<dyn FnMut(&GridSelection) -> String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.copy_provider = Some(provider);
        }
    }

    pub fn redraw(&self, cx: &mut Cx) {
        if let Some(inner) = self.borrow_mut() {
            inner.area.redraw(cx);
        }
    }

    pub fn set_default_sizes(&self, cx: &mut Cx, col_width: f64, row_height: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_default_sizes(cx, col_width, row_height);
        }
    }

    /// Fired actions, filtered to this grid.
    pub fn actions(&self, actions: &Actions) -> Vec<DataGridAction> {
        let uid = self.widget_uid();
        actions
            .iter()
            .filter_map(|action| {
                action
                    .as_widget_action()
                    .filter(|wa| wa.widget_uid == uid)
                    .map(|wa| wa.cast::<DataGridAction>())
            })
            .filter(|a| !matches!(a, DataGridAction::None))
            .collect()
    }

    /// The cell widgets that produced any of the given actions, as
    /// (row, col, widget).
    pub fn cell_widgets_with_actions(&self, actions: &Actions) -> Vec<(usize, usize, WidgetRef)> {
        let uid = self.widget_uid();
        let mut out = Vec::new();
        for action in actions {
            if let Some(action) = action.downcast_ref::<WidgetAction>() {
                if let Some(group) = &action.group {
                    if group.group_uid == uid {
                        if let Some(inner) = self.borrow() {
                            for (key, item) in inner.items.iter() {
                                if group.item_uid == item.widget.widget_uid() {
                                    let row = (*key >> 32) as usize;
                                    let col = (*key & 0xffff_ffff) as usize;
                                    out.push((row, col, item.widget.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_input::{TextInputAction, TextInputRef};

    fn cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }

    /// A grid built from its own type default, the way an app's DSL builds
    /// one, with rows to edit and no window: everything below runs before
    /// any draw, which is also when a host's first `edit_cell` runs.
    fn grid(cx: &mut Cx) -> DataGrid {
        let mut grid = cx.with_vm(DataGrid::script_new_with_default);
        grid.set_grid_size(20, 3);
        grid
    }

    fn editor(grid: &DataGrid) -> TextInputRef {
        let (row, col) = grid.editing().expect("no editor seated");
        let (_, editor) = grid.get_item(row, col).expect("no editor item");
        editor.as_text_input()
    }

    /// What the seated editor would report, as the grid receives it.
    fn from_editor(grid: &DataGrid, action: TextInputAction) -> Action {
        Box::new(WidgetAction {
            data: None,
            action: Box::new(action),
            widget_uid: editor(grid).widget_uid(),
            group: None,
        })
    }

    fn deliver(cx: &mut Cx, grid: &mut DataGrid, actions: ActionsBuf) -> Vec<DataGridAction> {
        let uid = grid.widget_uid();
        let emitted = cx.capture_actions(|cx| {
            grid.handle_event(cx, &Event::Actions(actions), &mut Scope::empty());
        });
        emitted.filter_widget_actions_cast::<DataGridAction>(uid).collect()
    }

    fn edited(actions: &[DataGridAction]) -> Vec<(usize, usize, String)> {
        actions
            .iter()
            .filter_map(|a| match a {
                DataGridAction::CellEdited { row, col, text } => Some((*row, *col, text.clone())),
                _ => None,
            })
            .collect()
    }

    fn tab(shift: bool) -> KeyEvent {
        KeyEvent {
            key_code: KeyCode::Tab,
            is_repeat: false,
            modifiers: KeyModifiers {
                shift,
                ..Default::default()
            },
            time: 0.0,
        }
    }

    /// The host's answer to EditCell: the editor is there, holds the text
    /// it was given, and the caret is after it rather than before it, so
    /// typing carries on from the seed instead of landing in front of it.
    #[test]
    fn edit_cell_seats_an_editor_holding_the_text_with_the_caret_after_it() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.edit_cell(&mut cx, 4, 1, "abc");
        assert_eq!(grid.editing(), Some((4, 1)));
        assert_eq!(editor(&grid).text(), "abc");
        assert_eq!(editor(&grid).cursor().index, 3);
        assert_eq!(grid.active_cell(), Some((4, 1)), "the selection follows the edit");
        });
    }

    #[test]
    fn a_cell_the_grid_does_not_have_seats_nothing() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.edit_cell(&mut cx, 25, 1, "abc");
        assert_eq!(grid.editing(), None);
        grid.edit_cell(&mut cx, 1, 3, "abc");
        assert_eq!(grid.editing(), None);
        });
    }

    /// Return hands the host the text the field holds, puts the editor
    /// away, and steps the selection down so the next Return edits the
    /// next row.
    #[test]
    fn return_commits_what_the_field_holds_and_steps_down() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.edit_cell(&mut cx, 4, 1, "abc");
        editor(&grid).set_text(&mut cx, "abcd");
        let action = from_editor(
            &grid,
            TextInputAction::Returned("abcd".into(), KeyModifiers::default()),
        );
        let out = deliver(&mut cx, &mut grid, vec![action]);
        assert_eq!(edited(&out), vec![(4, 1, "abcd".to_string())]);
        assert_eq!(grid.editing(), None);
        assert_eq!(grid.active_cell(), Some((5, 1)));
        });
    }

    #[test]
    fn shift_return_steps_up_and_tab_steps_sideways() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        let shift = KeyModifiers {
            shift: true,
            ..Default::default()
        };

        grid.edit_cell(&mut cx, 4, 1, "a");
        let action = from_editor(&grid, TextInputAction::Returned("a".into(), shift));
        deliver(&mut cx, &mut grid, vec![action]);
        assert_eq!(grid.active_cell(), Some((3, 1)));

        grid.edit_cell(&mut cx, 4, 1, "a");
        let action = from_editor(&grid, TextInputAction::KeyDownUnhandled(tab(false)));
        let out = deliver(&mut cx, &mut grid, vec![action]);
        assert_eq!(edited(&out).len(), 1, "tab commits");
        assert_eq!(grid.active_cell(), Some((4, 2)));

        grid.edit_cell(&mut cx, 4, 1, "a");
        let action = from_editor(&grid, TextInputAction::KeyDownUnhandled(tab(true)));
        deliver(&mut cx, &mut grid, vec![action]);
        assert_eq!(grid.active_cell(), Some((4, 0)));
        });
    }

    /// Stepping stops at the edge: a Return on the last row commits and
    /// stays, rather than walking off the grid.
    #[test]
    fn stepping_off_the_last_row_stays_on_it() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.edit_cell(&mut cx, 19, 1, "a");
        let action = from_editor(
            &grid,
            TextInputAction::Returned("a".into(), KeyModifiers::default()),
        );
        deliver(&mut cx, &mut grid, vec![action]);
        assert_eq!(grid.active_cell(), Some((19, 1)));
        });
    }

    /// Escape puts the editor away and says so, and says nothing about a
    /// value: the host has nothing to write and the cell keeps what it
    /// had. The selection stays on the cell that was being edited.
    #[test]
    fn escape_puts_the_editor_away_and_reports_no_value() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.edit_cell(&mut cx, 4, 1, "abc");
        editor(&grid).set_text(&mut cx, "abcd");
        let action = from_editor(&grid, TextInputAction::Escaped);
        let out = deliver(&mut cx, &mut grid, vec![action]);
        assert!(edited(&out).is_empty(), "nothing was edited: {out:?}");
        assert!(
            out.iter()
                .any(|a| matches!(a, DataGridAction::EditCancelled { row: 4, col: 1 })),
            "the cancel is reported: {out:?}"
        );
        assert_eq!(grid.editing(), None);
        assert_eq!(grid.active_cell(), Some((4, 1)));
        });
    }

    /// The editor losing the keyboard — a click on another cell, or on
    /// anything else — is a commit where it stands: the value is reported
    /// and the selection is left wherever the click put it.
    #[test]
    fn losing_the_keyboard_commits_without_stepping() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.edit_cell(&mut cx, 4, 1, "abc");
        editor(&grid).set_text(&mut cx, "abcd");
        let action = from_editor(&grid, TextInputAction::KeyFocusLost);
        let out = deliver(&mut cx, &mut grid, vec![action]);
        assert_eq!(edited(&out), vec![(4, 1, "abcd".to_string())]);
        assert_eq!(grid.editing(), None);
        assert_eq!(grid.active_cell(), Some((4, 1)));
        });
    }

    /// The field drops the keyboard before it reports the Return, so the
    /// two can arrive in one pass. That is one commit, and it steps: a
    /// grid that took the focus loss for the reason would leave the
    /// person on the row they just finished.
    #[test]
    fn a_return_and_the_focus_loss_it_causes_are_one_stepping_commit() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.edit_cell(&mut cx, 4, 1, "abc");
        let actions = vec![
            from_editor(
                &grid,
                TextInputAction::Returned("abc".into(), KeyModifiers::default()),
            ),
            from_editor(&grid, TextInputAction::KeyFocusLost),
        ];
        let out = deliver(&mut cx, &mut grid, actions);
        assert_eq!(edited(&out).len(), 1);
        assert_eq!(grid.active_cell(), Some((5, 1)));
        });
    }

    /// Commit before restart: seating a second editor while one is live
    /// hands the host the first one's text first, so a host that reseats
    /// on every request never loses an edit.
    #[test]
    fn a_second_edit_commits_the_first_before_it_starts() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.edit_cell(&mut cx, 4, 1, "first");
        let uid = grid.widget_uid();
        let emitted = cx.capture_actions(|cx| grid.edit_cell(cx, 2, 0, "second"));
        let out: Vec<DataGridAction> = emitted
            .filter_widget_actions_cast::<DataGridAction>(uid)
            .collect();
        assert_eq!(edited(&out), vec![(4, 1, "first".to_string())]);
        assert_eq!(grid.editing(), Some((2, 0)));
        assert_eq!(editor(&grid).text(), "second");
        assert_eq!(grid.active_cell(), Some((2, 0)));
        });
    }

    /// The host's own commit and cancel, for a control outside the grid.
    #[test]
    fn the_host_can_commit_or_cancel_from_outside() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        let uid = grid.widget_uid();

        grid.edit_cell(&mut cx, 4, 1, "abc");
        let emitted = cx.capture_actions(|cx| grid.commit_edit(cx));
        let out: Vec<DataGridAction> = emitted
            .filter_widget_actions_cast::<DataGridAction>(uid)
            .collect();
        assert_eq!(edited(&out), vec![(4, 1, "abc".to_string())]);
        assert_eq!(grid.editing(), None);

        grid.edit_cell(&mut cx, 4, 1, "abc");
        let emitted = cx.capture_actions(|cx| grid.cancel_edit(cx));
        let out: Vec<DataGridAction> = emitted
            .filter_widget_actions_cast::<DataGridAction>(uid)
            .collect();
        assert!(edited(&out).is_empty());
        assert!(out
            .iter()
            .any(|a| matches!(a, DataGridAction::EditCancelled { row: 4, col: 1 })));
        assert_eq!(grid.editing(), None);

        // And with nothing seated, neither says anything.
        let emitted = cx.capture_actions(|cx| {
            grid.commit_edit(cx);
            grid.cancel_edit(cx);
        });
        assert_eq!(emitted.len(), 0);
        });
    }

    /// An editor's reports are only read while it is seated: after a
    /// cancel, the focus loss the cancel itself causes must not commit.
    #[test]
    fn a_focus_loss_after_the_editor_is_gone_commits_nothing() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.edit_cell(&mut cx, 4, 1, "abc");
        let lost = from_editor(&grid, TextInputAction::KeyFocusLost);
        grid.cancel_edit(&mut cx);
        let out = deliver(&mut cx, &mut grid, vec![lost]);
        assert!(edited(&out).is_empty(), "{out:?}");
        });
    }

    /// A grid that shrinks under the editor puts it away rather than
    /// reporting a commit for a row that no longer exists -- and says so,
    /// once, on the next event, since the host was promised a cancel or a
    /// commit for every editor that closes.
    #[test]
    fn a_grid_that_shrinks_under_the_editor_puts_it_away_and_says_so() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.edit_cell(&mut cx, 15, 1, "abc");
        grid.set_grid_size(10, 3);
        assert_eq!(grid.editing(), None);
        let out = deliver(&mut cx, &mut grid, Vec::new());
        assert!(edited(&out).is_empty());
        assert!(
            out.iter()
                .any(|a| matches!(a, DataGridAction::EditCancelled { row: 15, col: 1 })),
            "the dropped edit was not reported: {out:?}"
        );
        assert!(deliver(&mut cx, &mut grid, Vec::new()).is_empty(), "reported twice");
        });
    }

    /// A cell asked for before the first draw is scrolled to on that
    /// draw, not measured against a viewport that has no size yet.
    #[test]
    fn an_edit_before_the_first_draw_waits_for_a_viewport_to_scroll_in() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.edit_cell(&mut cx, 4, 1, "abc");
        assert_eq!((grid.scroll.x, grid.scroll.y), (0.0, 0.0), "scrolled against nothing");
        assert_eq!(grid.scroll_pending, Some((4, 1)));
        });
    }
    /// The editor is drawn every frame, on screen or off, so its area is
    /// always the current frame's and the keyboard leaving it - what a
    /// click anywhere else does - reaches it after the cell has scrolled
    /// away. Two frames at a size that shows a few rows: the edit is made
    /// in one, the wheel carries the row off the bottom before the other,
    /// and the focus loss still commits.
    #[test]
    fn an_edit_scrolled_out_of_view_still_commits_when_the_keyboard_leaves() {
        crate::on_test_cx(|| {
        use crate::makepad_draw::cx_draw::CxDraw;
        fn frame(cx: &mut Cx, grid: &mut DataGrid, pass: &DrawPass, draw_list: &mut DrawList2d) {
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(pass, None);
            draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(dvec2(300.0, 100.0), Layout::flow_overlay());
            // The host's loop, as a page writes it: every cell it is
            // handed, and never the one being edited.
            while !grid
                .draw_walk(&mut cx2d, &mut Scope::empty(), Walk::fixed(300.0, 100.0))
                .is_done()
            {
                while let Some(cell) = grid.next_cell(&mut cx2d) {
                    grid.cell_text(&mut cx2d, &cell, "-");
                }
            }
            cx2d.end_pass_sized_turtle();
            draw_list.end(&mut cx2d);
            cx2d.end_pass(pass);
        }
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, dvec2(300.0, 100.0));
        let mut draw_list = DrawList2d::new(&mut cx);
        grid.edit_cell(&mut cx, 4, 1, "abc");
        frame(&mut cx, &mut grid, &pass, &mut draw_list);
        let on_screen = editor(&grid).area();
        assert!(on_screen.is_valid(&cx), "the editor was not drawn in its cell");
        grid.scroll_cell_into_view(&mut cx, 19, 0);
        frame(&mut cx, &mut grid, &pass, &mut draw_list);
        assert_eq!(grid.editing(), Some((4, 1)), "the edit did not survive the scroll");
        let off_screen = editor(&grid).area();
        assert!(!on_screen.is_valid(&cx), "the second frame drew nothing new");
        assert!(
            off_screen.is_valid(&cx),
            "the editor was not drawn once its cell left the viewport"
        );
        // The keyboard leaves the editor, as it does when anything else
        // is clicked; the platform names the area it left.
        let lost = cx.capture_actions(|cx| {
            let leaving = KeyFocusEvent {
                prev: off_screen,
                focus: Area::Empty,
            };
            grid.handle_event(cx, &Event::KeyFocus(leaving), &mut Scope::empty());
        });
        let out = deliver(&mut cx, &mut grid, lost);
        assert_eq!(edited(&out), vec![(4, 1, "abc".to_string())]);
        assert_eq!(grid.editing(), None);
        });
    }

    fn mods(shift: bool, control: bool) -> KeyModifiers {
        KeyModifiers {
            shift,
            control,
            ..Default::default()
        }
    }

    fn key(key_code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            key_code,
            modifiers,
            ..Default::default()
        }
    }

    /// A grid laid out the way a draw lays it out, 300 by 200 points at
    /// the origin, so the pointer steps can find its cells with no window.
    fn laid_out(cx: &mut Cx) -> DataGrid {
        let mut grid = grid(cx);
        grid.vp.widget_rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(300.0, 200.0),
        };
        grid.compute_viewport();
        grid
    }

    /// The middle of a cell, where a pointer presses it.
    fn middle(grid: &DataGrid, row: usize, display_col: usize) -> DVec2 {
        let rect = grid.cell_rect(row, display_col);
        rect.pos + rect.size * 0.5
    }

    /// What one step of the grid raised, for the steps a test takes by
    /// hand: a press, a heading click, a key.
    fn raised(
        cx: &mut Cx,
        grid: &mut DataGrid,
        step: impl FnOnce(&mut Cx, &mut DataGrid),
    ) -> Vec<DataGridAction> {
        let uid = grid.widget_uid();
        let emitted = cx.capture_actions(|cx| step(cx, grid));
        emitted.filter_widget_actions_cast::<DataGridAction>(uid).collect()
    }

    fn clicked(actions: &[DataGridAction]) -> Vec<(usize, usize, KeyModifiers)> {
        actions
            .iter()
            .filter_map(|a| match a {
                DataGridAction::CellClicked { row, col, modifiers } => Some((*row, *col, *modifiers)),
                _ => None,
            })
            .collect()
    }

    fn selection_changed(actions: &[DataGridAction]) -> bool {
        actions
            .iter()
            .any(|a| matches!(a, DataGridAction::SelectionChanged { .. }))
    }

    /// A host that keeps its own picks turns the grid's off. A press then
    /// selects nothing and starts no rubber band, and still says which
    /// cell it was and what was held, since Ctrl and Shift are how the
    /// host's picks are made. The keys select nothing either.
    #[test]
    fn a_press_with_the_selection_off_selects_nothing_and_reports_the_modifiers() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.selection = GridSelectMode::Off;
        let (at, away) = (middle(&grid, 3, 1), middle(&grid, 6, 2));
        for held in [mods(false, true), mods(true, false)] {
            let out = raised(&mut cx, &mut grid, |cx, grid| {
                grid.press_cell(cx, 3, 1, at, held, 1);
                grid.move_cell_press(cx, away);
                grid.release_cell_press(cx, away, held);
            });
            assert_eq!(clicked(&out), vec![(3, 1, held)]);
            assert!(!selection_changed(&out), "a rubber band started: {out:?}");
            assert_eq!(grid.selection(), None);
        }
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            grid.handle_key_down(cx, &key(KeyCode::ArrowDown, mods(true, false)));
            grid.handle_key_down(cx, &key(KeyCode::KeyA, mods(false, true)));
        });
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(grid.selection(), None);
        });
    }

    /// A grid switched off holds no selection, whether one was left from
    /// the mode before or the host sets one: none is reported, and Delete,
    /// Return and typing, which act on a selection, raise nothing.
    #[test]
    fn a_grid_with_the_selection_off_holds_none_and_its_keys_find_none() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.selection = GridSelectMode::Rows;
        grid.press_cell(&mut cx, 3, 1, DVec2::default(), KeyModifiers::default(), 1);
        assert!(grid.selection().is_some());
        grid.selection = GridSelectMode::Off;
        assert_eq!(grid.selection(), None, "left over from rows");
        assert_eq!(grid.active_cell(), None);
        grid.set_selection(&mut cx, Some(GridSelection::single(5, 0)));
        assert_eq!(grid.selection(), None, "set by the host");
        grid.selection = GridSelectMode::Cells;
        assert_eq!(grid.selection(), None, "a selection set while off was kept");

        grid.selection = GridSelectMode::Rows;
        grid.press_cell(&mut cx, 3, 1, DVec2::default(), KeyModifiers::default(), 1);
        grid.selection = GridSelectMode::Off;
        let none = KeyModifiers::default();
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            for code in [KeyCode::Delete, KeyCode::Backspace, KeyCode::ReturnKey, KeyCode::F2] {
                grid.handle_key_down(cx, &key(code, none));
            }
        });
        assert!(out.is_empty(), "{out:?}");
        });
    }

    /// A list picks rows. A press anywhere in one selects all of it, shift
    /// carries the pick down to another row, and the arrows move a row
    /// selection rather than a cell.
    #[test]
    fn a_press_with_rows_selects_the_whole_row() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.selection = GridSelectMode::Rows;
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            grid.press_cell(cx, 4, 2, DVec2::default(), KeyModifiers::default(), 1)
        });
        assert_eq!(clicked(&out), vec![(4, 2, KeyModifiers::default())]);
        let sel = grid.selection().expect("the press selected nothing");
        assert_eq!(sel.kind, GridSelectKind::Rows);
        assert_eq!(sel.row_range(), (4, 4));
        assert!(sel.contains(4, 0) && sel.contains(4, 2) && !sel.contains(5, 2));

        grid.press_cell(&mut cx, 7, 0, DVec2::default(), mods(true, false), 1);
        let sel = grid.selection().unwrap();
        assert_eq!((sel.kind, sel.row_range()), (GridSelectKind::Rows, (4, 7)));

        grid.handle_key_down(&mut cx, &key(KeyCode::ArrowDown, KeyModifiers::default()));
        let sel = grid.selection().unwrap();
        assert_eq!((sel.kind, sel.row_range()), (GridSelectKind::Rows, (8, 8)));
        });
    }

    /// A heading press on a list sorts and says so, and that is all: the
    /// rows picked before it stay picked and no column is selected. The
    /// spreadsheet still selects the column, as it always has.
    #[test]
    fn a_heading_press_with_rows_or_off_changes_only_the_sort() {
        crate::on_test_cx(|| {
        for mode in [GridSelectMode::Rows, GridSelectMode::Off] {
            let mut cx = cx();
            let mut grid = grid(&mut cx);
            grid.sortable = true;
            grid.selection = mode;
            grid.press_cell(&mut cx, 2, 0, DVec2::default(), KeyModifiers::default(), 1);
            let before = grid.selection();
            let out = raised(&mut cx, &mut grid, |cx, grid| {
                grid.click_header(cx, 1, KeyModifiers::default())
            });
            assert_eq!(grid.selection(), before, "{mode:?}");
            assert!(!selection_changed(&out), "{mode:?}: {out:?}");
            assert_eq!(grid.sort(), Some((1, true)), "{mode:?}");
            assert!(out.iter().any(|a| matches!(
                a,
                DataGridAction::SortChanged { col: 1, ascending: Some(true) }
            )));
            assert!(out
                .iter()
                .any(|a| matches!(a, DataGridAction::HeaderClicked { col: 1, .. })));
        }
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.click_header(&mut cx, 1, KeyModifiers::default());
        let sel = grid.selection().expect("the spreadsheet lost its column pick");
        assert_eq!((sel.kind, sel.col_range()), (GridSelectKind::Cols, (1, 1)));
        });
    }

    fn released(actions: &[DataGridAction]) -> Vec<(usize, usize, KeyModifiers)> {
        actions
            .iter()
            .filter_map(|a| match a {
                DataGridAction::CellReleased { row, col, modifiers } => Some((*row, *col, *modifiers)),
                _ => None,
            })
            .collect()
    }

    fn carried(actions: &[DataGridAction]) -> Vec<(usize, usize, DVec2, KeyModifiers)> {
        actions
            .iter()
            .filter_map(|a| match a {
                DataGridAction::RowDragStarted {
                    row,
                    col,
                    abs,
                    modifiers,
                } => Some((*row, *col, *abs, *modifiers)),
                _ => None,
            })
            .collect()
    }

    /// A press that comes up where it went down is a click, reported as
    /// it goes down and again as it comes up, with the keys held then.
    #[test]
    fn a_press_and_release_in_place_is_clicked_then_released() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        let at = middle(&grid, 2, 1);
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            grid.press_cell(cx, 2, 1, at, KeyModifiers::default(), 1);
            grid.release_cell_press(cx, at, mods(true, false));
        });
        let down = out.iter().position(|a| matches!(a, DataGridAction::CellClicked { .. }));
        let up = out.iter().position(|a| matches!(a, DataGridAction::CellReleased { .. }));
        assert!(matches!((down, up), (Some(down), Some(up)) if down < up), "{out:?}");
        assert_eq!(released(&out), vec![(2, 1, mods(true, false))]);
        assert!(matches!(grid.interact, Interact::None), "the press outlived its release");
        });
    }

    /// Four points of wobble between down and up is still a click, in a
    /// grid that carries rows and in one that does not.
    #[test]
    fn four_points_of_travel_is_still_a_click() {
        crate::on_test_cx(|| {
        for row_drag in [false, true] {
            let mut cx = cx();
            let mut grid = laid_out(&mut cx);
            grid.row_drag = row_drag;
            let at = middle(&grid, 2, 1);
            let wobble = at + dvec2(0.0, 4.0);
            let out = raised(&mut cx, &mut grid, |cx, grid| {
                grid.press_cell(cx, 2, 1, at, KeyModifiers::default(), 1);
                grid.move_cell_press(cx, wobble);
                grid.release_cell_press(cx, wobble, KeyModifiers::default());
            });
            assert_eq!(released(&out).len(), 1, "row_drag {row_drag}: {out:?}");
            assert!(carried(&out).is_empty(), "row_drag {row_drag}: {out:?}");
        }
        });
    }

    /// Six points with `row_drag` on carries the row, and the grid says so
    /// once and then leaves the press alone: coming back to where it went
    /// down is no click, the selection does not follow the pointer, and
    /// the view does not scroll however far past the edge it goes.
    #[test]
    fn six_points_with_row_drag_carries_the_row_and_does_nothing_else() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.row_drag = true;
        let at = middle(&grid, 2, 1);
        let (six, other_cell) = (at + dvec2(0.0, 6.0), middle(&grid, 5, 2));
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            grid.press_cell(cx, 2, 1, at, mods(false, true), 1);
            grid.move_cell_press(cx, six);
            grid.move_cell_press(cx, other_cell);
            grid.move_cell_press(cx, at + dvec2(400.0, 400.0));
            grid.release_cell_press(cx, at, KeyModifiers::default());
        });
        assert_eq!(carried(&out), vec![(2, 1, six, mods(false, true))], "{out:?}");
        assert!(released(&out).is_empty(), "{out:?}");
        assert_eq!(grid.selection(), Some(GridSelection::single(2, 1)));
        assert_eq!(grid.scroll, DVec2::default(), "the carry scrolled the grid");
        });
    }

    /// The rows cut short under a held press leave the line pressed
    /// behind: coming up is no click on it, and travelling carries nothing.
    #[test]
    fn a_press_on_a_line_the_rows_no_longer_reach_neither_releases_nor_carries() {
        crate::on_test_cx(|| {
        for (row_drag, travel) in [(false, 0.0), (true, 0.0), (true, 6.0)] {
            let mut cx = cx();
            let mut grid = laid_out(&mut cx);
            grid.row_drag = row_drag;
            let at = middle(&grid, 5, 1);
            let to = at + dvec2(0.0, travel);
            let out = raised(&mut cx, &mut grid, |cx, grid| {
                grid.press_cell(cx, 5, 1, at, KeyModifiers::default(), 1);
                grid.set_grid_size(3, 3);
                grid.move_cell_press(cx, to);
                grid.release_cell_press(cx, to, KeyModifiers::default());
            });
            assert!(released(&out).is_empty(), "row_drag {row_drag}, {travel}: {out:?}");
            assert!(carried(&out).is_empty(), "row_drag {row_drag}, {travel}: {out:?}");
        }
        });
    }

    /// With `row_drag` off a press still drags out a rectangle, as it
    /// always has, and a drag is not a click.
    #[test]
    fn without_row_drag_a_drag_still_selects_a_rectangle() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        let (from, to) = (middle(&grid, 1, 0), middle(&grid, 3, 2));
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            grid.press_cell(cx, 1, 0, from, KeyModifiers::default(), 1);
            grid.move_cell_press(cx, to);
            grid.release_cell_press(cx, to, KeyModifiers::default());
        });
        let sel = grid.selection().expect("the drag selected nothing");
        assert_eq!((sel.kind, sel.anchor, sel.head), (GridSelectKind::Cells, (1, 0), (3, 2)));
        assert!(carried(&out).is_empty(), "{out:?}");
        assert!(released(&out).is_empty(), "{out:?}");
        });
    }

    /// Rows of three heights, scrolled part way: every row on screen is
    /// found at its own rectangle, top edge, middle and last point alike,
    /// so a host that asks which row is under the pointer and where that
    /// row is gets one answer from both.
    #[test]
    fn row_rect_and_row_at_give_back_each_others_answer() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.set_row_height(1, 40.0);
        grid.set_row_height(3, 15.0);
        grid.set_row_height(6, 60.0);
        grid.set_scroll(&mut cx, dvec2(0.0, 30.0));
        let data = grid.vp.data_rect;
        let mut seen = 0;
        for row in 0..20 {
            let rect = grid.row_rect(row).expect("a row the grid has has a rectangle");
            assert_eq!(rect.size.y, grid.row_sizes.size_of(row));
            for y in [rect.pos.y, rect.pos.y + rect.size.y * 0.5, rect.pos.y + rect.size.y - 0.01] {
                if y < data.pos.y || y >= data.pos.y + data.size.y {
                    continue;
                }
                seen += 1;
                assert_eq!(grid.row_at(dvec2(rect.pos.x + 10.0, y)), Some(row), "row {row} at {y}");
                // Level with the row is enough; the columns do not matter.
                assert_eq!(grid.row_at(dvec2(-500.0, y)), Some(row), "row {row} off to the side");
            }
        }
        assert!(seen > 6, "the test looked at almost nothing ({seen})");
        assert_eq!(grid.row_rect(20), None);
        });
    }

    /// Clearing the overrides puts every row back on the default pitch.
    #[test]
    fn clearing_the_row_heights_restores_uniform_rows() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.set_row_height(0, 50.0);
        grid.set_row_height(4, 14.0);
        grid.clear_row_heights();
        let pitch = grid.default_row_height;
        let top = grid.vp.data_rect.pos.y;
        for row in 0..20 {
            let rect = grid.row_rect(row).unwrap();
            assert_eq!((rect.pos.y, rect.size.y), (top + pitch * row as f64, pitch), "row {row}");
        }
        assert_eq!(grid.row_at(dvec2(60.0, top + pitch * 2.5)), Some(2));
        });
    }

    /// Above the rows — the heading strip, or above the grid — and below
    /// the last row or below the grid, there is no row.
    #[test]
    fn a_point_above_or_below_the_rows_has_no_row() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        let data = grid.vp.data_rect;
        assert_eq!(grid.row_at(dvec2(60.0, data.pos.y - 1.0)), None, "in the headings");
        assert_eq!(grid.row_at(dvec2(60.0, -10.0)), None, "above the grid");
        assert_eq!(grid.row_at(dvec2(60.0, data.pos.y + data.size.y + 5.0)), None, "below the grid");
        assert_eq!(grid.row_at(dvec2(60.0, data.pos.y)), Some(0));
        // Three rows in a grid with room for more: the space under them
        // is inside the grid and still not a row.
        grid.set_grid_size(3, 3);
        let below_last = grid.row_rect(2).map(|r| r.pos.y + r.size.y + 1.0).unwrap();
        assert!(below_last < data.pos.y + data.size.y);
        assert_eq!(grid.row_at(dvec2(60.0, below_last)), None, "under the last row");
        });
    }

    /// The middle of a column heading, where a pointer presses it.
    fn heading(grid: &DataGrid, display_col: usize) -> DVec2 {
        let strip = grid.vp.col_header_rect;
        let (_, x, w) = grid.vp.vis_cols[display_col];
        dvec2(x + w * 0.5, strip.pos.y + strip.size.y * 0.5)
    }

    fn menus(actions: &[DataGridAction]) -> Vec<DataGridAction> {
        actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    DataGridAction::CellContextMenu { .. } | DataGridAction::HeaderContextMenu { .. }
                )
            })
            .cloned()
            .collect()
    }

    /// A secondary press asks for a menu on the cell or the heading under
    /// it, where it went down, by data column, and changes nothing: the
    /// selection stays, no sort moves, and a row number, the corner or a
    /// press while a row is held asks for nothing.
    #[test]
    fn a_secondary_press_on_a_cell_or_a_heading_asks_for_a_menu_and_changes_nothing() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.sortable = true;
        // The first column moved to the end: data columns 1, 2, 0.
        grid.move_column(&mut cx, 0, 3);
        grid.press_cell(&mut cx, 3, 1, middle(&grid, 3, 1), KeyModifiers::default(), 1);
        grid.release_cell_press(&mut cx, middle(&grid, 3, 1), KeyModifiers::default());
        let before = grid.selection();
        assert!(before.is_some());

        let (cell, head) = (middle(&grid, 6, 2), heading(&grid, 0));
        let out = raised(&mut cx, &mut grid, |cx, grid| grid.context_press(cx, cell));
        assert!(
            matches!(menus(&out)[..], [DataGridAction::CellContextMenu { row: 6, col: 0, abs }] if abs == cell),
            "{out:?}"
        );
        assert_eq!(out.len(), 1, "{out:?}");
        let out = raised(&mut cx, &mut grid, |cx, grid| grid.context_press(cx, head));
        assert!(
            matches!(menus(&out)[..], [DataGridAction::HeaderContextMenu { col: 1, abs }] if abs == head),
            "{out:?}"
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(grid.selection(), before);
        assert_eq!(grid.sort(), None);

        let row_number = dvec2(10.0, middle(&grid, 4, 0).y);
        let corner = dvec2(10.0, 10.0);
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            grid.context_press(cx, row_number);
            grid.context_press(cx, corner);
            grid.press_cell(cx, 2, 0, middle(grid, 2, 0), KeyModifiers::default(), 1);
            grid.context_press(cx, cell);
        });
        assert!(menus(&out).is_empty(), "{out:?}");
        });
    }

    /// A grid drawn into a window-less pass, so real pointer events can
    /// find it: `host` runs where a page's draw loop does, after the grid
    /// has measured the frame and before the cells are handed out, and the
    /// cells it hands out are returned.
    struct Frame {
        pass: DrawPass,
        draw_list: DrawList2d,
        size: DVec2,
    }

    impl Frame {
        fn new(cx: &mut Cx, size: DVec2) -> Self {
            let pass = DrawPass::new(cx);
            pass.set_size(cx, size);
            Frame {
                pass,
                draw_list: DrawList2d::new(cx),
                size,
            }
        }

        fn draw(
            &mut self,
            cx: &mut Cx,
            grid: &mut DataGrid,
            mut host: impl FnMut(&mut Cx2d, &mut DataGrid),
        ) -> Vec<GridCell> {
            use crate::makepad_draw::cx_draw::CxDraw;
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(self.size, Layout::flow_overlay());
            let mut cells = Vec::new();
            let walk = Walk::fixed(self.size.x, self.size.y);
            while !grid.draw_walk(&mut cx2d, &mut Scope::empty(), walk).is_done() {
                host(&mut cx2d, grid);
                while let Some(cell) = grid.next_cell(&mut cx2d) {
                    grid.cell_text(&mut cx2d, &cell, "-");
                    cells.push(cell);
                }
            }
            cx2d.end_pass_sized_turtle();
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
            cells
        }
    }

    fn mouse_down(abs: DVec2, button: MouseButton, modifiers: KeyModifiers) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button,
            window_id: WindowId(1, 1),
            modifiers,
            handled: std::cell::Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn mouse_up(abs: DVec2, button: MouseButton, modifiers: KeyModifiers) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button,
            window_id: WindowId(1, 1),
            modifiers,
            time: 0.0,
        })
    }

    /// A primary drag along `path` through the grid's own pointer
    /// handling: down at the first point, a move to each of the rest, and
    /// up at the last, with the button recorded as held in between the way
    /// the platform records it. What each event raised, in order.
    fn primary_drag(cx: &mut Cx, grid: &mut DataGrid, path: &[DVec2]) -> Vec<Vec<DataGridAction>> {
        let none = KeyModifiers::default();
        let (first, last) = (path[0], path[path.len() - 1]);
        let mut out = Vec::new();
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WindowId(1, 1)));
        out.push(sent(cx, grid, mouse_down(first, MouseButton::PRIMARY, none)));
        for at in &path[1..] {
            out.push(sent(cx, grid, mouse_move(*at)));
        }
        out.push(sent(cx, grid, mouse_up(last, MouseButton::PRIMARY, none)));
        cx.fingers.first_mouse_button = None;
        out
    }

    fn mouse_move(abs: DVec2) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: DVec2::default(),
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
            handled: std::cell::Cell::new(Area::Empty),
        })
    }

    /// What one pointer event raised, with the key focus it asked for
    /// settled the way the event loop settles it between events.
    fn sent(cx: &mut Cx, grid: &mut DataGrid, event: Event) -> Vec<DataGridAction> {
        let out = raised(cx, grid, |cx, grid| grid.handle_event(cx, &event, &mut Scope::empty()));
        cx.action(());
        cx.handle_actions();
        out
    }

    /// Through the grid's own pointer handling: the right button asks for
    /// a menu and leaves the keyboard where it was, and Ctrl with the left
    /// button, which is how a list toggles a pick, is a press like any
    /// other, never a menu.
    #[test]
    fn only_the_secondary_button_asks_for_a_menu_and_it_takes_no_focus() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        let mut frame = Frame::new(&mut cx, dvec2(300.0, 200.0));
        frame.draw(&mut cx, &mut grid, |_, _| {});
        let at = middle(&grid, 2, 1);
        let ctrl = mods(false, true);

        let out = sent(&mut cx, &mut grid, mouse_down(at, MouseButton::SECONDARY, ctrl));
        assert!(
            matches!(menus(&out)[..], [DataGridAction::CellContextMenu { row: 2, col: 1, .. }]),
            "{out:?}"
        );
        assert!(clicked(&out).is_empty(), "the right button pressed the cell: {out:?}");
        assert_eq!(grid.selection(), None);
        assert!(!cx.has_key_focus(grid.area), "the right button took the keyboard");
        let out = sent(&mut cx, &mut grid, mouse_up(at, MouseButton::SECONDARY, ctrl));
        assert!(released(&out).is_empty() && menus(&out).is_empty(), "{out:?}");

        let out = sent(&mut cx, &mut grid, mouse_down(at, MouseButton::PRIMARY, ctrl));
        assert_eq!(clicked(&out), vec![(2, 1, ctrl)]);
        assert!(menus(&out).is_empty(), "Ctrl-click asked for a menu: {out:?}");
        assert!(cx.has_key_focus(grid.area), "a primary press still takes the keyboard");
        let out = sent(&mut cx, &mut grid, mouse_up(at, MouseButton::PRIMARY, ctrl));
        assert_eq!(released(&out), vec![(2, 1, ctrl)]);
        });
    }

    /// Widths a host sets from its draw loop, fitted to the width the grid
    /// measured for this frame, are the widths this frame's cells are
    /// handed out at, and the columns they bring into view are handed out
    /// too, rather than a frame later.
    #[test]
    fn widths_set_in_the_draw_loop_land_in_the_frame_being_drawn() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.set_grid_size(20, 6);
        let mut frame = Frame::new(&mut cx, dvec2(300.0, 200.0));
        let cells = frame.draw(&mut cx, &mut grid, |_, _| {});
        let first_row = |cells: &[GridCell]| -> Vec<f64> {
            cells.iter().filter(|c| c.row == 0).map(|c| c.rect.size.x).collect()
        };
        assert_eq!(first_row(&cells), vec![96.0; 3], "the stock widths show three columns");

        let mut measured = 0.0;
        let cells = frame.draw(&mut cx, &mut grid, |cx, grid| {
            measured = grid.data_width();
            let each = (measured / 6.0).floor();
            grid.set_col_widths(cx, &[each; 6]);
        });
        assert_eq!(measured, 300.0 - grid.row_header_width);
        assert_eq!(first_row(&cells), vec![(measured / 6.0).floor(); 6]);
        assert_eq!(grid.visible_counts().1, 6);
        assert_eq!(cells.len(), 6 * grid.visible_counts().0, "a cell was handed out twice or not at all");
        });
    }

    /// Every push replaces all the widths: a shorter list puts the columns
    /// it leaves out back on the default, a width below the minimum is
    /// raised to it, and from event code the columns under the pointer
    /// move at once.
    #[test]
    fn a_shorter_list_of_widths_puts_the_rest_back_on_the_default() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.set_col_widths(&mut cx, &[200.0, 100.0, 100.0]);
        assert_eq!(grid.col_widths(), vec![200.0, 100.0, 100.0]);
        let at = dvec2(grid.vp.data_rect.pos.x + 100.0, 100.0);
        let row = grid.row_at(at).unwrap();
        assert_eq!(grid.hit_zone(at), HitZone::Cell { row, display_col: 0 });
        grid.set_col_widths(&mut cx, &[5.0]);
        assert_eq!(grid.col_widths(), vec![grid.min_col_width, 96.0, 96.0]);
        assert_eq!(grid.hit_zone(at), HitZone::Cell { row, display_col: 1 });
        });
    }

    /// With nothing declared every heading is the stock one: centred, in
    /// the heading colour, the sorted one too, in the cells' text style;
    /// the marks are as they were; and the pointer over a cell is the
    /// default one.
    #[test]
    fn a_heading_and_the_pointer_look_as_they_always_have_until_declared() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.sortable = true;
        grid.set_unsortable_cols(vec![2]);
        grid.set_sort_indicator(Some((1, true)));
        let stock = |mark| HeaderLook {
            color: grid.color_header_text,
            align: 0.5,
            mark,
        };
        assert_eq!(grid.header_look(0), stock(Some(("▲▼", true))));
        assert_eq!(grid.header_look(1), stock(Some(("▲", false))));
        assert_eq!(grid.header_look(2), stock(None));
        assert!(!grid.headers_have_own_text());
        assert_eq!(grid.hover_cursor(middle(&grid, 2, 1)), MouseCursor::Default);
        });
    }

    /// Declared in the markup, the sorted heading takes its own colour and
    /// the rest keep theirs, every label sits where `header_align` puts it,
    /// the headings get their own text style, and a cell wears the declared
    /// pointer while an edge still offers the drag.
    #[test]
    fn a_declared_heading_look_and_pointer_are_what_the_grid_uses() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let declared = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                DataGrid{
                    rows: 0
                    cols: 3
                    sortable: true
                    header_align: 0.0
                    color_header_text: #x808080ff
                    color_header_sorted: #xffffffff
                    cell_cursor: MouseCursor.Hand
                    draw_text_header +: {text_style +: {font_size: 8.0}}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let declared = declared.as_data_grid();
        let mut grid = declared.borrow_mut().expect("the markup built no grid");
        grid.set_grid_size(20, 3);
        grid.vp.widget_rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(300.0, 200.0),
        };
        grid.compute_viewport();
        grid.set_sort_indicator(Some((1, false)));
        let white = vec4(1.0, 1.0, 1.0, 1.0);
        assert_eq!(grid.header_look(1).color, white);
        assert_ne!(grid.header_look(0).color, white);
        assert_eq!(grid.header_look(0).color, grid.color_header_text);
        assert_eq!(grid.header_look(2).align, 0.0);
        assert!(grid.headers_have_own_text());
        assert_eq!(grid.hover_cursor(middle(&grid, 2, 1)), MouseCursor::Hand);
        let (_, x, w) = grid.vp.vis_cols[0];
        let edge = dvec2(x + w, heading(&grid, 0).y);
        assert_eq!(grid.hover_cursor(edge), MouseCursor::ColResize);
        assert_eq!(grid.hover_cursor(heading(&grid, 0)), MouseCursor::Default);
        });
    }

    /// The label's place: centred with the marks ignored, as ever; at the
    /// left padding at 0.0; ending clear of the marks at 1.0; and a label
    /// with no room starts at the padding whatever the alignment.
    #[test]
    fn a_label_sits_across_its_heading_and_clear_of_the_marks_on_the_right() {
        crate::on_test_cx(|| {
        let rect = Rect {
            pos: dvec2(100.0, 0.0),
            size: dvec2(120.0, 28.0),
        };
        let (pad, tw, marks) = (6.0, 40.0, 20.0);
        assert_eq!(header_label_x(rect, pad, tw, 0.5, marks), 100.0 + 6.0 + (108.0 - 40.0) * 0.5);
        assert_eq!(header_label_x(rect, pad, tw, 0.0, marks), 106.0);
        let right = header_label_x(rect, pad, tw, 1.0, marks);
        assert_eq!(right + tw, 220.0 - pad - marks, "the label runs into the marks");
        let between = header_label_x(rect, pad, tw, 0.75, marks);
        assert!(between > header_label_x(rect, pad, tw, 0.5, marks) && between < right);
        assert_eq!(header_label_x(rect, pad, 200.0, 1.0, marks), 106.0);
        });
    }

    /// A cell text too wide for its cell is cut the padding short of the
    /// next column, the same gap a text that fits leaves, over the whole
    /// height of the cell; a text over its room that still ends a point
    /// short of the next column shows whole; no padding still keeps a
    /// point, and a cell narrower than its padding clips to nothing rather
    /// than to a negative width.
    #[test]
    fn a_cut_cell_text_stops_the_padding_short_of_the_next_column() {
        crate::on_test_cx(|| {
        let rect = Rect {
            pos: dvec2(100.0, 40.0),
            size: dvec2(80.0, 22.0),
        };
        let pad = 6.0;
        let next_column = rect.pos.x + rect.size.x;
        // 73 points of text end at 179, a point short of the next column:
        // past its 68 of room, and still shown whole.
        let clip = cell_text_clip(rect, pad, 73.0);
        assert_eq!(clip.pos, rect.pos);
        assert_eq!(clip.size.y, rect.size.y);
        assert_eq!(next_column - (clip.pos.x + clip.size.x), 1.0);
        assert!(rect.pos.x + pad + 73.0 <= clip.pos.x + clip.size.x, "the text ends inside the clip");
        // Half a point more, and it is cut the padding short.
        let clip = cell_text_clip(rect, pad, 73.5);
        assert_eq!(clip.pos, rect.pos);
        assert_eq!(clip.size.y, rect.size.y);
        assert_eq!(next_column - (clip.pos.x + clip.size.x), pad);
        let fitting_text_end = rect.pos.x + pad + (rect.size.x - 2.0 * pad);
        assert_eq!(clip.pos.x + clip.size.x, fitting_text_end);
        assert_eq!(cell_text_clip(rect, pad, 200.0).size.x, 74.0);
        assert_eq!(cell_text_clip(rect, 0.0, 200.0).size.x, 79.0);
        assert_eq!(cell_text_clip(Rect { pos: rect.pos, size: dvec2(4.0, 22.0) }, pad, 30.0).size.x, 0.0);
        });
    }

    fn tips_raised(
        cx: &mut Cx,
        grid: &mut DataGrid,
        step: impl FnOnce(&mut Cx, &mut DataGrid),
    ) -> Vec<TipAction> {
        let uid = grid.widget_uid();
        let emitted = cx.capture_actions(|cx| step(cx, grid));
        emitted.filter_widget_actions_cast::<TipAction>(uid).collect()
    }

    /// The pointer resting on a heading with a tip raises it once, with the
    /// part of the heading that shows; moving inside that heading says
    /// nothing more; a heading without a tip, a cell, or leaving the grid
    /// takes it down; and a tip belongs to its data column wherever that
    /// column is dragged.
    #[test]
    fn a_heading_with_a_tip_raises_it_and_leaving_takes_it_down() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.set_header_tips(vec!["the first".into(), String::new(), "the third".into()]);
        let strip = grid.vp.col_header_rect;
        let (first, second, third) = (heading(&grid, 0), heading(&grid, 1), heading(&grid, 2));

        let out = tips_raised(&mut cx, &mut grid, |cx, grid| grid.hover_tip(cx, Some(middle(grid, 3, 0))));
        assert!(out.is_empty(), "a cell raised a tip: {out:?}");
        let out = tips_raised(&mut cx, &mut grid, |cx, grid| {
            grid.hover_tip(cx, Some(first));
            grid.hover_tip(cx, Some(first + dvec2(20.0, 3.0)));
        });
        let rect = Rect {
            pos: dvec2(strip.pos.x, strip.pos.y),
            size: dvec2(96.0, strip.size.y),
        };
        assert_eq!(out, vec![TipAction::HoverIn("the first".into(), rect)]);
        let out = tips_raised(&mut cx, &mut grid, |cx, grid| {
            grid.hover_tip(cx, Some(second));
            grid.hover_tip(cx, Some(second + dvec2(10.0, 0.0)));
        });
        assert_eq!(out, vec![TipAction::HoverOut], "a heading without a tip");

        // The third heading runs past the right edge; its tip hangs off
        // the part that shows.
        let out = tips_raised(&mut cx, &mut grid, |cx, grid| {
            grid.hover_tip(cx, Some(third));
            grid.hover_tip(cx, None);
        });
        let shown = Rect {
            pos: dvec2(strip.pos.x + 192.0, strip.pos.y),
            size: dvec2(strip.size.x - 192.0, strip.size.y),
        };
        assert_eq!(
            out,
            vec![TipAction::HoverIn("the third".into(), shown), TipAction::HoverOut]
        );

        // The first column moved to the end: its tip went with it.
        grid.move_column(&mut cx, 0, 3);
        let out = tips_raised(&mut cx, &mut grid, |cx, grid| {
            grid.hover_tip(cx, Some(first));
            grid.hover_tip(cx, Some(third));
        });
        assert_eq!(out, vec![TipAction::HoverIn("the first".into(), shown)], "{out:?}");
        });
    }

    /// A grid given no tips raises nothing from its headings at all.
    #[test]
    fn a_grid_without_tips_raises_none() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        let (first, second) = (heading(&grid, 0), heading(&grid, 1));
        let out = tips_raised(&mut cx, &mut grid, |cx, grid| {
            grid.hover_tip(cx, Some(first));
            grid.hover_tip(cx, Some(second));
            grid.hover_tip(cx, None);
        });
        assert!(out.is_empty(), "{out:?}");
        });
    }

    fn selections(actions: &[DataGridAction]) -> usize {
        actions
            .iter()
            .filter(|a| matches!(a, DataGridAction::SelectionChanged { .. }))
            .count()
    }

    /// With `drag_scrolling` a press that travels scrolls the grid by the
    /// travel, both ways, so the point pressed stays under the pointer. It
    /// stops at what there is to scroll, drags out no selection, carries
    /// no row even in a grid whose rows can be carried, and coming up is
    /// no click.
    #[test]
    fn a_drag_to_scroll_moves_the_grid_by_the_travel_and_nothing_else() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.drag_scrolling = true;
        grid.row_drag = true;
        let at = middle(&grid, 5, 1);
        grid.press_cell(&mut cx, 5, 1, at, KeyModifiers::default(), 1);
        let pressed = grid.selection();
        assert!(pressed.is_some(), "the press itself still selects");
        // 300 by 200 with row numbers and headings: 248 points across 288
        // of columns and 172 down 520 of rows.
        let (max_x, max_y) = (288.0 - 248.0, 520.0 - 172.0);
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            grid.move_cell_press(cx, at + dvec2(0.0, -4.0));
            assert_eq!(grid.scroll, DVec2::default(), "four points scrolled");
            let travelled = at + dvec2(-30.0, -60.0);
            grid.move_cell_press(cx, travelled);
            assert_eq!(grid.scroll, dvec2(30.0, 60.0));
            assert_eq!(grid.row_at(travelled), Some(5), "the row pressed left the pointer");
            grid.move_cell_press(cx, at + dvec2(-500.0, -1000.0));
            assert_eq!(grid.scroll, dvec2(max_x, max_y), "scrolled past the end");
            grid.move_cell_press(cx, at + dvec2(0.0, 10.0));
            assert_eq!(grid.scroll, DVec2::default(), "scrolled before the start");
            grid.release_cell_press(cx, at, KeyModifiers::default());
        });
        assert_eq!(selections(&out), 0, "{out:?}");
        assert!(carried(&out).is_empty(), "{out:?}");
        assert!(released(&out).is_empty(), "{out:?}");
        assert_eq!(grid.selection(), pressed);

        // A press that stays inside the threshold is still a click.
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            grid.press_cell(cx, 2, 0, at, KeyModifiers::default(), 1);
            grid.move_cell_press(cx, at + dvec2(4.0, 0.0));
            grid.release_cell_press(cx, at + dvec2(4.0, 0.0), KeyModifiers::default());
        });
        assert_eq!(released(&out).len(), 1, "{out:?}");
        assert_eq!(grid.scroll, DVec2::default());
        });
    }

    /// The same through the grid's own pointer handling, and off by
    /// default: without the flag the same drag drags out a selection and
    /// leaves the view where it was.
    #[test]
    fn a_drag_on_a_drawn_grid_scrolls_only_when_asked() {
        crate::on_test_cx(|| {
        for drag_scrolling in [false, true] {
            let mut cx = cx();
            let mut grid = grid(&mut cx);
            let mut frame = Frame::new(&mut cx, dvec2(300.0, 200.0));
            frame.draw(&mut cx, &mut grid, |_, _| {});
            grid.set_drag_scrolling(drag_scrolling);
            let at = middle(&grid, 5, 1);
            let up = at + dvec2(0.0, -52.0);
            let out = primary_drag(&mut cx, &mut grid, &[at, up]);
            let out = out[1..].concat();
            if drag_scrolling {
                assert_eq!(grid.scroll, dvec2(0.0, 52.0));
                assert_eq!(selections(&out), 0, "{out:?}");
                assert_eq!(grid.selection(), Some(GridSelection::single(5, 1)));
            } else {
                assert_eq!(grid.scroll, DVec2::default());
                let sel = grid.selection().expect("the drag selected nothing");
                assert_eq!((sel.anchor, sel.head), ((5, 1), (3, 1)));
            }
        }
        });
    }

    /// A press a widget in a cell took is that widget's alone. The grid
    /// neither presses the cell nor drags itself to scroll, so a slider, a
    /// scroll bar or a field seated in a cell keeps the pointer for itself
    /// and the rows under it stand still while its thumb moves.
    ///
    /// Every gesture the grid has starts from `Event::hits` on its own
    /// area, and a press a child captured is already marked handled by the
    /// time that runs — which is why this drives `handle_event` rather
    /// than `press_cell`: the ordering that makes it true (cells first,
    /// the grid's own hits after) lives nowhere else.
    #[test]
    fn a_press_a_cell_widget_took_never_presses_or_scrolls_the_grid() {
        crate::on_test_cx(|| {
        // A drag on bare cells is the grid's own, so what follows is
        // about the editor holding the press and not about a drag that
        // does nothing anywhere.
        let mut bare_cx = cx();
        let mut bare = grid(&mut bare_cx);
        let mut bare_frame = Frame::new(&mut bare_cx, dvec2(300.0, 200.0));
        bare_frame.draw(&mut bare_cx, &mut bare, |_, _| {});
        bare.set_drag_scrolling(true);
        let at = middle(&bare, 5, 1);
        let out = primary_drag(&mut bare_cx, &mut bare, &[at, at + dvec2(0.0, -52.0)]).concat();
        assert_eq!(clicked(&out).len(), 1, "a bare cell took no press: {out:?}");
        assert_eq!(bare.scroll, dvec2(0.0, 52.0), "a bare cell did not drag-scroll");

        let mut cx = cx();
        let mut grid = grid(&mut cx);
        let mut frame = Frame::new(&mut cx, dvec2(300.0, 200.0));
        frame.draw(&mut cx, &mut grid, |_, _| {});
        grid.set_drag_scrolling(true);
        grid.row_drag = true;
        // A seated editor is a real widget in a cell that takes a press and
        // then tracks the pointer itself, exactly as a slider in a cell
        // would; it is the one such widget the grid can seat on its own.
        grid.edit_cell(&mut cx, 5, 1, "abc");
        frame.draw(&mut cx, &mut grid, |_, _| {});

        let at = middle(&grid, 5, 1);
        let out = primary_drag(&mut cx, &mut grid, &[at, at + dvec2(0.0, -52.0)]).concat();
        assert!(
            clicked(&out).is_empty(),
            "the grid pressed a cell the editor holds: {out:?}"
        );
        assert!(
            carried(&out).is_empty(),
            "the grid carried a row out from under the editor: {out:?}"
        );
        assert_eq!(
            grid.scroll,
            DVec2::default(),
            "the grid drag-scrolled under a press the editor holds"
        );
        assert!(
            matches!(grid.interact, Interact::None),
            "the grid started a gesture from a press a cell widget holds"
        );

        });
    }

    /// A column edge dragged is a control that holds the pointer: it goes
    /// on resizing while the pointer is far outside the grid, and the
    /// release out there is still its own. Capture, not the rectangle,
    /// decides who a moving pointer belongs to.
    #[test]
    fn a_column_resize_keeps_the_pointer_outside_the_grid() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        let mut frame = Frame::new(&mut cx, dvec2(300.0, 200.0));
        frame.draw(&mut cx, &mut grid, |_, _| {});
        let (_, x, w) = grid.vp.vis_cols[0];
        let edge = dvec2(x + w, heading(&grid, 0).y);
        let start = grid.col_widths()[0];
        let out = primary_drag(
            &mut cx,
            &mut grid,
            &[edge, edge + dvec2(20.0, 0.0), edge + dvec2(40.0, 900.0)],
        )
        .concat();
        assert_eq!(
            grid.col_widths()[0],
            start + 40.0,
            "the resize stopped at the grid's own edge"
        );
        let resized = out
            .iter()
            .filter(|a| matches!(a, DataGridAction::ColumnResized { .. }))
            .count();
        assert_eq!(resized, 1, "the release outside the grid ended no resize: {out:?}");
        });
    }

    /// The columns as a frame hands them out, left to right: each one's
    /// data column and width.
    fn drawn_columns(cells: &[GridCell]) -> Vec<(usize, f64)> {
        cells.iter().filter(|c| c.row == 0).map(|c| (c.col, c.rect.size.x)).collect()
    }

    fn order_changes(actions: &[DataGridAction]) -> Vec<Vec<usize>> {
        actions
            .iter()
            .filter_map(|a| match a {
                DataGridAction::ColumnOrderChanged { order } => Some(order.clone()),
                _ => None,
            })
            .collect()
    }

    fn header_clicks(actions: &[DataGridAction]) -> usize {
        actions
            .iter()
            .filter(|a| matches!(a, DataGridAction::HeaderClicked { .. }))
            .count()
    }

    /// A heading carried right of the last heading's middle shows a bar at
    /// the gap after it while it is held, and dropped there it moves its
    /// column to the end: the grid says which column went where and then
    /// the whole order, the column takes its width with it, and nothing is
    /// sorted or clicked on the way.
    #[test]
    fn a_heading_dropped_past_another_moves_its_column_and_reports_the_order() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.allow_col_reorder = true;
        grid.sortable = true;
        grid.set_col_widths(&mut cx, &[100.0, 60.0, 80.0]);
        let mut frame = Frame::new(&mut cx, dvec2(300.0, 200.0));
        frame.draw(&mut cx, &mut grid, |_, _| {});
        let (first, third) = (heading(&grid, 0), heading(&grid, 2));
        let past = third + dvec2(10.0, 0.0);
        let none = KeyModifiers::default();

        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WindowId(1, 1)));
        let mut out = sent(&mut cx, &mut grid, mouse_down(first, MouseButton::PRIMARY, none));
        out.extend(sent(&mut cx, &mut grid, mouse_move(past)));
        let marker = grid.col_drop_marker().expect("no bar while the heading is held");
        let after_last = grid.vp.data_rect.pos.x + 240.0;
        assert_eq!((marker.pos.x, marker.size.x), (after_last - 1.0, 2.0));
        assert_eq!((marker.pos.y, marker.size.y), (0.0, 200.0), "the bar runs the height of the grid");
        frame.draw(&mut cx, &mut grid, |_, _| {});
        let up = sent(&mut cx, &mut grid, mouse_up(past, MouseButton::PRIMARY, none));
        cx.fingers.first_mouse_button = None;
        out.extend(up.iter().cloned());

        let moved = up.iter().position(|a| {
            matches!(a, DataGridAction::ColumnMoved { from_display: 0, to_display: 2 })
        });
        let ordered = up.iter().position(|a| matches!(a, DataGridAction::ColumnOrderChanged { .. }));
        assert!(matches!((moved, ordered), (Some(m), Some(o)) if m < o), "{up:?}");
        assert_eq!(order_changes(&out), vec![vec![1, 2, 0]]);
        assert_eq!(header_clicks(&out), 0, "{out:?}");
        assert_eq!(grid.sort(), None);
        assert_eq!(grid.col_drop_marker(), None, "the bar outlived the drop");
        assert_eq!(grid.col_order(), vec![1, 2, 0]);
        assert_eq!(grid.col_widths(), vec![60.0, 80.0, 100.0]);
        let cells = frame.draw(&mut cx, &mut grid, |_, _| {});
        assert_eq!(drawn_columns(&cells), vec![(1, 60.0), (2, 80.0), (0, 100.0)]);
        });
    }

    /// Four points of travel on a heading is a press, which sorts, and
    /// moves nothing; and with `allow_col_reorder` off, the default, a
    /// heading dragged all the way across is still only a press.
    #[test]
    fn a_heading_that_does_not_travel_or_may_not_move_is_only_pressed() {
        crate::on_test_cx(|| {
        for (allow, travel) in [(true, 4.0), (false, 200.0)] {
            let mut cx = cx();
            let mut grid = grid(&mut cx);
            grid.allow_col_reorder = allow;
            grid.sortable = true;
            grid.set_col_widths(&mut cx, &[80.0, 80.0, 80.0]);
            let mut frame = Frame::new(&mut cx, dvec2(300.0, 200.0));
            frame.draw(&mut cx, &mut grid, |_, _| {});
            let at = heading(&grid, 0);
            let out = primary_drag(&mut cx, &mut grid, &[at, at + dvec2(travel, 0.0)]).concat();
            assert_eq!(header_clicks(&out), 1, "allow {allow}: {out:?}");
            assert!(order_changes(&out).is_empty(), "allow {allow}: {out:?}");
            assert_eq!(grid.col_order(), vec![0, 1, 2]);
            assert_eq!(grid.sort(), Some((0, true)));
        }
        });
    }

    /// The host puts the columns in an order of its own: the grid draws
    /// them that way at once, the widths go with their columns, the
    /// selection is cleared and nothing is raised. An order that does not
    /// name every column once is refused, the order the grid already has
    /// changes nothing, and their own order puts the columns back.
    #[test]
    fn the_host_sets_the_order_and_the_widths_go_with_their_columns() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.set_col_widths(&mut cx, &[100.0, 60.0, 80.0]);
        grid.set_selection(&mut cx, Some(GridSelection::single(2, 1)));
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            assert!(grid.set_col_order(cx, &[2, 0, 1]));
        });
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(grid.col_order(), vec![2, 0, 1]);
        assert_eq!(grid.display_to_data(0), 2);
        assert_eq!(grid.col_widths(), vec![80.0, 100.0, 60.0]);
        let measured: Vec<f64> = grid.vp.vis_cols.iter().map(|(_, _, w)| *w).collect();
        assert_eq!(measured, vec![80.0, 100.0, 60.0], "the frame was not measured again");
        assert_eq!(grid.selection(), None);

        for refused in [&[0, 1][..], &[0, 0, 1], &[0, 1, 3], &[0, 1, 2, 3]] {
            assert!(!grid.set_col_order(&mut cx, refused), "{refused:?}");
            assert_eq!(grid.col_order(), vec![2, 0, 1], "{refused:?}");
        }
        grid.set_selection(&mut cx, Some(GridSelection::single(1, 1)));
        assert!(grid.set_col_order(&mut cx, &[2, 0, 1]));
        assert_eq!(grid.selection(), Some(GridSelection::single(1, 1)), "the same order cleared it");

        assert!(grid.set_col_order(&mut cx, &[0, 1, 2]));
        assert!(grid.col_order.is_none());
        assert_eq!(grid.col_widths(), vec![100.0, 60.0, 80.0]);
        });
    }

    /// Unmovable columns keep their place: a badge column first and a
    /// column of controls last. Their headings do not drag, and a heading
    /// between them dropped beyond either lands next to it instead.
    #[test]
    fn unmovable_columns_keep_their_place() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.set_grid_size(20, 4);
        grid.allow_col_reorder = true;
        grid.set_unmovable_cols(vec![0, 3]);
        grid.set_col_widths(&mut cx, &[40.0, 60.0, 60.0, 60.0]);
        let mut frame = Frame::new(&mut cx, dvec2(300.0, 200.0));
        frame.draw(&mut cx, &mut grid, |_, _| {});
        let left_edge = grid.vp.data_rect.pos.x + 2.0;
        let right_edge = grid.vp.data_rect.pos.x + 218.0;
        assert_eq!(grid.hover_cursor(heading(&grid, 0)), MouseCursor::Default);
        assert_eq!(grid.hover_cursor(heading(&grid, 1)), MouseCursor::Grab);

        let badges = heading(&grid, 0);
        let out = primary_drag(&mut cx, &mut grid, &[badges, dvec2(right_edge, 14.0)]).concat();
        assert!(order_changes(&out).is_empty(), "the badge column moved: {out:?}");
        assert_eq!(header_clicks(&out), 1);

        let third = heading(&grid, 2);
        let out = primary_drag(&mut cx, &mut grid, &[third, dvec2(left_edge, 14.0)]).concat();
        assert_eq!(order_changes(&out), vec![vec![0, 2, 1, 3]], "dropped past the badges, it lands behind them");
        frame.draw(&mut cx, &mut grid, |_, _| {});

        let none = KeyModifiers::default();
        let from = heading(&grid, 1);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WindowId(1, 1)));
        sent(&mut cx, &mut grid, mouse_down(from, MouseButton::PRIMARY, none));
        sent(&mut cx, &mut grid, mouse_move(dvec2(right_edge, 14.0)));
        let marker = grid.col_drop_marker().unwrap();
        assert_eq!(marker.pos.x, grid.vp.data_rect.pos.x + 160.0 - 1.0, "the bar stops in front of the controls");
        let out = sent(&mut cx, &mut grid, mouse_up(dvec2(right_edge, 14.0), MouseButton::PRIMARY, none));
        cx.fingers.first_mouse_button = None;
        assert_eq!(order_changes(&out), vec![vec![0, 1, 2, 3]], "dropped past the controls, it lands in front of them");
        });
    }

    /// Widths waiting for columns the grid does not have, from a longer
    /// list of widths or left when the columns were cut, stay where they
    /// wait when the host orders the columns there are.
    #[test]
    fn an_order_leaves_the_widths_past_the_last_column_where_they_are() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.set_col_widths(&mut cx, &[100.0, 60.0, 80.0, 70.0]);
        assert!(grid.set_col_order(&mut cx, &[0, 2, 1]));
        assert_eq!(grid.col_widths(), vec![100.0, 80.0, 60.0]);
        assert_eq!(grid.col_width(3), 70.0, "the width waiting for a fourth column moved");

        let mut grid = laid_out(&mut cx);
        grid.set_grid_size(20, 4);
        grid.set_col_widths(&mut cx, &[140.0, 120.0, 100.0, 70.0]);
        grid.set_grid_size(20, 3);
        assert!(grid.set_col_order(&mut cx, &[0, 2, 1]));
        assert_eq!(grid.col_widths(), vec![140.0, 100.0, 120.0]);
        assert_eq!(grid.col_width(3), 70.0, "the width the fourth column left moved");
        });
    }

    fn carry_moves(actions: &[DataGridAction]) -> Vec<DVec2> {
        actions
            .iter()
            .filter_map(|a| match a {
                DataGridAction::RowDragMoved { abs } => Some(*abs),
                _ => None,
            })
            .collect()
    }

    fn carry_ends(actions: &[DataGridAction]) -> Vec<(DVec2, KeyModifiers)> {
        actions
            .iter()
            .filter_map(|a| match a {
                DataGridAction::RowDragEnded { abs, modifiers } => Some((*abs, *modifiers)),
                _ => None,
            })
            .collect()
    }

    /// A carried row reports every move after the one that started the
    /// carry, wherever the pointer goes, and where it was let go with the
    /// keys held then, once. A press that carried nothing reports neither.
    #[test]
    fn a_carried_row_reports_every_move_and_where_it_was_let_go() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.row_drag = true;
        let at = middle(&grid, 2, 1);
        let (start, on, away) = (at + dvec2(0.0, 6.0), middle(&grid, 5, 1), dvec2(-300.0, 900.0));
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            grid.press_cell(cx, 2, 1, at, KeyModifiers::default(), 1);
            grid.move_cell_press(cx, start);
            grid.move_cell_press(cx, on);
            grid.move_cell_press(cx, away);
            grid.release_cell_press(cx, away, mods(true, false));
        });
        assert_eq!(carried(&out).len(), 1, "{out:?}");
        assert_eq!(carry_moves(&out), vec![on, away]);
        assert_eq!(carry_ends(&out), vec![(away, mods(true, false))]);
        assert!(released(&out).is_empty(), "{out:?}");
        let started = out.iter().position(|a| matches!(a, DataGridAction::RowDragStarted { .. }));
        let moved = out.iter().position(|a| matches!(a, DataGridAction::RowDragMoved { .. }));
        let ended = out.iter().position(|a| matches!(a, DataGridAction::RowDragEnded { .. }));
        assert!(matches!((started, moved, ended), (Some(s), Some(m), Some(e)) if s < m && m < e), "{out:?}");
        assert_eq!(grid.scroll, DVec2::default(), "a carry scrolled with row_drag_scroll off");

        for row_drag in [false, true] {
            grid.row_drag = row_drag;
            let out = raised(&mut cx, &mut grid, |cx, grid| {
                grid.press_cell(cx, 2, 1, at, KeyModifiers::default(), 1);
                grid.move_cell_press(cx, at + dvec2(0.0, 3.0));
                if !row_drag {
                    grid.move_cell_press(cx, on);
                }
                grid.release_cell_press(cx, at, KeyModifiers::default());
            });
            assert!(carry_moves(&out).is_empty() && carry_ends(&out).is_empty(), "{out:?}");
        }
        });
    }

    /// The gap is in front of the row level with the pointer above that
    /// row's middle and behind it below, with rows of any height, so rows
    /// of different heights do not trade places under a still pointer;
    /// behind the last row under the rows; at the rows in view on their
    /// top and bottom edges when scrolled; and nowhere for a pointer over
    /// the headings, above, below or beside the grid, or a grid with no
    /// rows.
    #[test]
    fn the_gap_for_a_carried_row_is_either_side_of_the_middle_of_the_row_level_with_it() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        // Rows 26 high, but row 1 is 60: row 0 from 0 to 26, row 1 from 26
        // to 86, row 2 from 86 to 112, and on at 26.
        grid.set_row_height(1, 60.0);
        let data = grid.vp.data_rect;
        let at = |y: f64| dvec2(100.0, data.pos.y + y);
        assert_eq!(grid.row_gap_at(at(5.0)), Some(0));
        assert_eq!(grid.row_gap_at(at(12.9)), Some(0));
        assert_eq!(grid.row_gap_at(at(13.0)), Some(1));
        assert_eq!(grid.row_gap_at(at(55.9)), Some(1), "above the tall row's middle");
        assert_eq!(grid.row_gap_at(at(56.0)), Some(2));
        assert_eq!(grid.row_gap_at(dvec2(10.0, data.pos.y + 5.0)), Some(0), "over the row numbers");

        grid.set_scroll(&mut cx, dvec2(0.0, 40.0));
        // The top of the view is 14 into the tall row, above its middle.
        assert_eq!(grid.row_gap_at(dvec2(100.0, data.pos.y)), Some(1), "on the top edge, scrolled");
        // The bottom of the view is at 172 + 40 = 212, 22 into row 6.
        let bottom = data.pos.y + data.size.y;
        assert_eq!(grid.row_gap_at(dvec2(100.0, bottom - 0.5)), Some(7), "on the bottom edge, scrolled");
        for (away, said) in [
            (dvec2(100.0, 10.0), "over the headings"),
            (dvec2(100.0, -500.0), "above the grid"),
            (dvec2(100.0, bottom), "just below the rows"),
            (dvec2(100.0, 900.0), "below the grid"),
        ] {
            assert_eq!(grid.row_gap_at(away), None, "{said}");
        }

        assert_eq!(grid.row_gap_at(dvec2(-1.0, data.pos.y + 5.0)), None, "left of the grid");
        assert_eq!(grid.row_gap_at(dvec2(300.0, data.pos.y + 5.0)), None, "right of the grid");

        grid.set_scroll(&mut cx, DVec2::default());
        // A carried row 26 high on line 0 over the tall row on line 1: past
        // the tall row's middle the carried row belongs behind it, and once
        // the host has swapped them the same still pointer leaves it there.
        let line_for = |from: usize, gap: usize| if gap > from { gap - 1 } else { gap };
        let still = at(57.0);
        assert_eq!(grid.row_gap_at(still).map(|gap| line_for(0, gap)), Some(1));
        grid.set_row_height(0, 60.0);
        grid.set_row_height(1, 26.0);
        assert_eq!(grid.row_gap_at(still).map(|gap| line_for(1, gap)), Some(1), "the rows traded back");

        grid.set_grid_size(3, 3);
        assert_eq!(grid.row_gap_at(at(150.0)), Some(3), "under the last row");
        grid.set_grid_size(0, 3);
        assert_eq!(grid.row_gap_at(at(5.0)), None, "no rows");
        });
    }

    fn next_frame(frame: NextFrame, time: f64) -> Event {
        Event::NextFrame(NextFrameEvent {
            frame: 1,
            time,
            set: [frame].into_iter().collect(),
        })
    }

    /// With `row_drag_scroll` a carry in the top or bottom row's height of
    /// the rows scrolls that way, faster the nearer the edge, and goes on
    /// scrolling frame by frame under a still pointer, saying so each
    /// step. It stops at the end of the rows, when the pointer is taken
    /// off the rows and when the row is let go, and scrolls nothing with
    /// the flag off, away from the edges, or anywhere off the rows: over
    /// the headings, above, below or beside the grid.
    #[test]
    fn a_carry_at_an_edge_scrolls_only_when_asked_and_faster_nearer_the_edge() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.row_drag = true;
        let data = grid.vp.data_rect;
        let bottom = data.pos.y + data.size.y;
        let (x, row) = (150.0, grid.default_row_height);
        let near_bottom = dvec2(x, bottom - row * 0.5);
        let near_top = dvec2(x, data.pos.y + row * 0.5);
        assert_eq!(grid.row_drag_scroll_speed(near_bottom), 0.0, "scrolls with the flag off");

        grid.row_drag_scroll = true;
        let full = DataGrid::ROW_DRAG_SCROLL_ROWS * row;
        assert_eq!(grid.row_drag_scroll_speed(dvec2(x, data.pos.y + 86.0)), 0.0);
        assert_eq!(grid.row_drag_scroll_speed(near_bottom), full * 0.5);
        assert_eq!(grid.row_drag_scroll_speed(near_top), -full * 0.5);
        assert_eq!(grid.row_drag_scroll_speed(dvec2(x, data.pos.y)), -full, "on the top edge");
        for (away, said) in [
            (dvec2(x, bottom), "just below the rows"),
            (dvec2(x, bottom + row), "a row below the grid"),
            (dvec2(x, bottom + 500.0), "far below the grid"),
            (dvec2(x, data.pos.y - 1.0), "over the headings"),
            (dvec2(x, -500.0), "far above the grid"),
            (dvec2(400.0, bottom - 1.0), "beside the grid"),
        ] {
            assert_eq!(grid.row_drag_scroll_speed(away), 0.0, "{said}");
        }

        let at = middle(&grid, 2, 1);
        grid.press_cell(&mut cx, 2, 1, at, KeyModifiers::default(), 1);
        grid.move_cell_press(&mut cx, near_bottom);
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            let frame = grid.row_drag_frame.expect("no frame asked for at the edge");
            grid.handle_event(cx, &next_frame(frame, 1.0), &mut Scope::empty());
            let frame = grid.row_drag_frame.expect("the scroll stopped under a still pointer");
            grid.handle_event(cx, &next_frame(frame, 1.02), &mut Scope::empty());
        });
        let expected = full * 0.5 * (1.0 / 60.0 + 0.02);
        assert!((grid.scroll.y - expected).abs() < 1e-9, "{} against {expected}", grid.scroll.y);
        assert_eq!(carry_moves(&out), vec![near_bottom, near_bottom]);

        // At the end of the rows there is nothing to scroll: no step, no
        // report, and no more frames until the pointer moves again.
        let max_y = 20.0 * row - data.size.y;
        grid.scroll.y = max_y;
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            let frame = grid.row_drag_frame.expect("stopped before the end");
            grid.handle_event(cx, &next_frame(frame, 1.04), &mut Scope::empty());
        });
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(grid.scroll.y, max_y);
        assert_eq!(grid.row_drag_frame, None);

        let below = dvec2(x, bottom + 40.0);
        let out = raised(&mut cx, &mut grid, |cx, grid| {
            grid.move_cell_press(cx, near_top);
            let frame = grid.row_drag_frame.expect("no frame asked for at the top");
            grid.handle_event(cx, &next_frame(frame, 2.0), &mut Scope::empty());
            assert!(grid.scroll.y < max_y, "the top edge did not scroll up");
            // Taken off the list, the carry stops scrolling: the frame
            // already asked for steps nothing and asks for no other.
            let scrolled = grid.scroll.y;
            grid.move_cell_press(cx, below);
            let frame = grid.row_drag_frame.expect("the scroll stopped under a still pointer");
            grid.handle_event(cx, &next_frame(frame, 2.02), &mut Scope::empty());
            assert_eq!(grid.scroll.y, scrolled, "the scroll went on off the list");
            assert_eq!(grid.row_drag_frame, None, "a frame was asked for off the list");
            grid.release_cell_press(cx, below, KeyModifiers::default());
        });
        assert_eq!(carry_moves(&out), vec![near_top, near_top, below]);
        assert_eq!(carry_ends(&out).len(), 1);
        assert_eq!(grid.row_drag_frame, None, "a frame outlived the carry");
        });
    }

    /// What one pointer event raised, the heading tips and the grid's own
    /// actions apart.
    fn sent_with_tips(cx: &mut Cx, grid: &mut DataGrid, event: Event) -> (Vec<TipAction>, Vec<DataGridAction>) {
        let uid = grid.widget_uid();
        let emitted = cx.capture_actions(|cx| grid.handle_event(cx, &event, &mut Scope::empty()));
        let tips = emitted
            .filter_widget_actions_cast::<TipAction>(uid)
            .filter(|tip| *tip != TipAction::None)
            .collect();
        let grid_actions = emitted
            .filter_widget_actions_cast::<DataGridAction>(uid)
            .filter(|action| !matches!(action, DataGridAction::None))
            .collect();
        cx.action(());
        cx.handle_actions();
        (tips, grid_actions)
    }

    /// Through the grid's own pointer handling: a pointer on an edge raises
    /// no tip and one moving onto an edge takes the tip down; a press on an
    /// edge with a tip up takes it down and still resizes, raising nothing
    /// while the edge is dragged; a press in a heading with a tip up takes
    /// it down and still presses the heading, which stays quiet after the
    /// release until the pointer leaves it.
    #[test]
    fn a_tip_keeps_off_an_edge_and_a_press_takes_it_down_and_still_resizes() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = grid(&mut cx);
        grid.set_grid_size(20, 4);
        grid.set_header_tips(vec!["tip".into(); 4]);
        let mut frame = Frame::new(&mut cx, dvec2(500.0, 200.0));
        frame.draw(&mut cx, &mut grid, |_, _| {});
        let none = KeyModifiers::default();
        let y = heading(&grid, 0).y;
        let (_, x, w) = grid.vp.vis_cols[0];
        let edge = dvec2(x + w, y);

        for at in [edge, edge - dvec2(5.0, 0.0), edge + dvec2(5.0, 0.0)] {
            let (tips, _) = sent_with_tips(&mut cx, &mut grid, mouse_move(at));
            assert!(tips.is_empty(), "a tip on the edge at {at:?}: {tips:?}");
        }
        let first = heading(&grid, 0);
        let (tips, _) = sent_with_tips(&mut cx, &mut grid, mouse_move(first));
        assert!(matches!(tips[..], [TipAction::HoverIn(..)]), "{tips:?}");
        let (tips, _) = sent_with_tips(&mut cx, &mut grid, mouse_move(edge));
        assert_eq!(tips, vec![TipAction::HoverOut], "onto the edge");

        // The tip up again, then a press on the edge.
        sent_with_tips(&mut cx, &mut grid, mouse_move(first));
        let near = edge - dvec2(2.0, 0.0);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WindowId(1, 1)));
        let (tips, _) = sent_with_tips(&mut cx, &mut grid, mouse_down(near, MouseButton::PRIMARY, none));
        assert_eq!(tips, vec![TipAction::HoverOut], "the press left the tip up");
        assert!(matches!(grid.interact, Interact::ColResize { display_col: 0, .. }));
        let (tips, _) = sent_with_tips(&mut cx, &mut grid, mouse_move(near + dvec2(40.0, 0.0)));
        assert!(tips.is_empty(), "{tips:?}");
        assert_eq!(grid.col_width(0), w + 40.0);
        grid.hover_tip(&mut cx, Some(heading(&grid, 1)));
        assert_eq!(grid.tip_head, None, "a tip was raised during the resize");
        let up = mouse_up(near + dvec2(40.0, 0.0), MouseButton::PRIMARY, none);
        let (tips, out) = sent_with_tips(&mut cx, &mut grid, up);
        cx.fingers.first_mouse_button = None;
        assert!(tips.is_empty(), "{tips:?}");
        assert!(matches!(out[..], [DataGridAction::ColumnResized { col: 0, .. }]), "{out:?}");

        // A press in the middle of a heading with its tip up.
        let second = heading(&grid, 1);
        let (tips, _) = sent_with_tips(&mut cx, &mut grid, mouse_move(second));
        assert!(matches!(tips[..], [TipAction::HoverIn(..)]), "{tips:?}");
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WindowId(1, 1)));
        let (tips, _) = sent_with_tips(&mut cx, &mut grid, mouse_down(second, MouseButton::PRIMARY, none));
        assert_eq!(tips, vec![TipAction::HoverOut]);
        assert!(matches!(grid.interact, Interact::ColDragPending { display_col: 1, .. }));
        let (_, out) = sent_with_tips(&mut cx, &mut grid, mouse_up(second, MouseButton::PRIMARY, none));
        cx.fingers.first_mouse_button = None;
        assert!(out.iter().any(|a| matches!(a, DataGridAction::HeaderClicked { col: 1, .. })), "{out:?}");
        let (tips, _) = sent_with_tips(&mut cx, &mut grid, mouse_move(second + dvec2(3.0, 0.0)));
        assert!(tips.is_empty(), "the pressed heading raised its tip again: {tips:?}");
        let third = heading(&grid, 2);
        let (tips, _) = sent_with_tips(&mut cx, &mut grid, mouse_move(third));
        assert!(matches!(tips[..], [TipAction::HoverIn(..)]), "{tips:?}");
        });
    }

    /// An edge takes hold six points either side of it, the nearest edge
    /// winning, and never more than a third of the column on either side,
    /// so a narrow column keeps a middle.
    #[test]
    fn an_edge_takes_hold_six_points_either_side_and_leaves_a_narrow_column_a_middle() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        grid.min_col_width = 8.0;
        grid.set_col_widths(&mut cx, &[96.0, 96.0, 12.0]);
        let y = heading(&grid, 0).y;
        let edge_of = |grid: &DataGrid, dc: usize| {
            let (_, x, w) = grid.vp.vis_cols[dc];
            x + w
        };
        let zone = |grid: &DataGrid, x: f64| match grid.hit_zone(dvec2(x, y)) {
            HitZone::ColHeader { resize_edge, .. } => resize_edge,
            other => panic!("not a heading: {other:?}"),
        };
        let first = edge_of(&grid, 0);
        assert_eq!(zone(&grid, first - 5.5), Some(0));
        assert_eq!(zone(&grid, first + 5.5), Some(0));
        assert_eq!(zone(&grid, first - 6.5), None);
        assert_eq!(zone(&grid, first + 6.5), None);
        let second = edge_of(&grid, 1);
        assert_eq!(zone(&grid, second + 3.5), Some(1));
        assert_eq!(zone(&grid, second + 6.0), None, "the narrow column's middle");
        assert_eq!(zone(&grid, second + 12.0 - 3.5), Some(2));
        });
    }

    /// Off by default, `header_edges` draws nothing; on, a line one point
    /// wide ends on the right edge of each heading in the strip, over the
    /// middle half of its height, and the last column has none.
    #[test]
    fn heading_edges_fall_on_the_column_edges_and_the_last_column_has_none() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut grid = laid_out(&mut cx);
        assert!(grid.header_edge_rects().is_empty());
        grid.header_edges = true;
        let strip = grid.vp.col_header_rect;
        let rects = grid.header_edge_rects();
        let edges: Vec<f64> = grid
            .vp
            .vis_cols
            .iter()
            .map(|&(_, x, w)| x + w)
            .filter(|&e| e <= strip.pos.x + strip.size.x)
            .collect();
        assert!(!edges.is_empty());
        assert_eq!(rects.iter().map(|r| r.pos.x + r.size.x).collect::<Vec<_>>(), edges);
        let inset = (strip.size.y * 0.25).round();
        for rect in &rects {
            assert_eq!(rect.size.x, 1.0);
            assert_eq!(rect.pos.y, strip.pos.y + inset);
            assert_eq!(rect.size.y, strip.size.y - 2.0 * inset);
        }
        grid.set_grid_size(20, 2);
        grid.compute_viewport();
        assert_eq!(grid.header_edge_rects().len(), 1, "the last column drew an edge");
        });
    }

    /// A grid drawn once into a window-less pass, so the headings are
    /// measured the way a real frame measures them. The cells it handed out.
    fn drawn(cx: &mut Cx, grid: &mut DataGrid) -> Vec<GridCell> {
        let mut frame = Frame::new(cx, dvec2(420.0, 200.0));
        frame.draw(cx, grid, |_, _| {})
    }

    /// The sheet the option is for: a wide first column under a short name,
    /// then numbers in 32-point columns under names far longer than that.
    fn narrow_grid(cx: &mut Cx, angle: f64) -> DataGrid {
        let mut grid = grid(cx);
        grid.set_grid_size(20, 4);
        grid.set_col_labels(vec![
            "Site".into(),
            "Readings taken".into(),
            "Average reading".into(),
            "Days without a reading".into(),
        ]);
        grid.set_col_widths(cx, &[120.0, 32.0, 32.0, 32.0]);
        grid.header_angle = angle;
        grid.remeasure_headers();
        grid
    }

    fn turned(grid: &DataGrid) -> Vec<bool> {
        (0..grid.cols).map(|c| grid.header_turns(c)).collect()
    }

    /// The operator's rule, whole: a sheet whose columns all hold their
    /// names flat is exactly the same sheet with an angle set — the same
    /// strip, the same cells, the same columns, the same edges between the
    /// headings — in either lean. Nothing turns, so nothing moves.
    #[test]
    fn wide_columns_with_an_angle_draw_exactly_what_they_drew_without_one() {
        let mut cx = cx();
        let labels: Vec<String> = vec!["First".into(), "Last".into(), "Born".into()];
        let mut off = grid(&mut cx);
        off.set_col_labels(labels.clone());
        let off_cells = drawn(&mut cx, &mut off);
        for lean in [DiagonalLean::Rise, DiagonalLean::Fall] {
            let mut on = grid(&mut cx);
            on.set_col_labels(labels.clone());
            on.header_angle = 45.0;
            on.header_lean = lean;
            on.remeasure_headers();
            let on_cells = drawn(&mut cx, &mut on);
            assert_eq!(turned(&on), vec![false; 3], "no heading here needs turning");
            assert_eq!(on.vp.col_header_rect, off.vp.col_header_rect, "the same strip");
            assert_eq!(on.vp.data_rect, off.vp.data_rect, "the same body");
            assert_eq!(on.vp.vis_cols, off.vp.vis_cols, "the same columns");
            assert_eq!(on.header_edge_rects(), off.header_edge_rects(), "the same edges");
            let rects = |cells: &[GridCell]| cells.iter().map(|c| c.rect).collect::<Vec<_>>();
            assert_eq!(rects(&on_cells), rects(&off_cells), "the same cells");
        }
    }

    /// Only the names too long for their columns turn: the wide first
    /// column keeps its short name flat beside them, and the strip grows to
    /// hold the turned ones — with the cells starting under it.
    #[test]
    fn only_a_name_too_long_for_its_column_turns() {
        let mut cx = cx();
        let mut grid = narrow_grid(&mut cx, 45.0);
        drawn(&mut cx, &mut grid);
        assert_eq!(turned(&grid), vec![false, true, true, true]);
        let strip = grid.vp.col_header_rect.size.y;
        assert!(strip > 28.0, "turned names need more room than the stated {strip}");
        assert_eq!(grid.vp.data_rect.pos.y, grid.vp.widget_rect.pos.y + strip);
        assert_eq!(grid.col_widths(), vec![120.0, 32.0, 32.0, 32.0], "no column widened");

        let mut flat = narrow_grid(&mut cx, 0.0);
        drawn(&mut cx, &mut flat);
        assert_eq!(turned(&flat), vec![false; 4], "nought turns nothing");
        assert_eq!(flat.vp.col_header_rect.size.y, 28.0);
    }

    /// The strip is as tall as the longest TURNED name needs: a long name
    /// lying flat in a column wide enough for it does not raise it.
    #[test]
    fn a_long_name_that_fits_flat_does_not_raise_the_strip() {
        let mut cx = cx();
        let mut short = narrow_grid(&mut cx, 45.0);
        short.set_grid_size(20, 2);
        short.set_col_labels(vec!["Site".into(), "Readings taken".into()]);
        short.set_col_widths(&mut cx, &[120.0, 32.0]);
        drawn(&mut cx, &mut short);
        let mut long = narrow_grid(&mut cx, 45.0);
        long.set_grid_size(20, 2);
        long.set_col_labels(vec!["The site the readings were taken at".into(), "Readings taken".into()]);
        long.set_col_widths(&mut cx, &[300.0, 32.0]);
        drawn(&mut cx, &mut long);
        assert_eq!(turned(&long), vec![false, true]);
        assert_eq!(long.vp.col_header_rect.size.y, short.vp.col_header_rect.size.y);
    }

    /// A press anywhere in a turned heading, top to bottom of the taller
    /// strip, lands on the column the name stands on.
    #[test]
    fn a_press_on_a_turned_heading_finds_its_own_column() {
        let mut cx = cx();
        let mut grid = narrow_grid(&mut cx, 45.0);
        drawn(&mut cx, &mut grid);
        let strip = grid.vp.col_header_rect;
        for (display_col, x, w) in grid.vp.vis_cols.clone() {
            for at in [strip.pos.y + 2.0, strip.pos.y + strip.size.y - 2.0] {
                let hit = grid.hit_zone(dvec2(x + w * 0.5, at));
                assert!(
                    matches!(hit, HitZone::ColHeader { display_col: hit, .. } if hit == display_col),
                    "column {display_col} at {at}: {hit:?}"
                );
            }
        }
    }

    /// Sorting does not tip a heading onto the diagonal or off it: the fit
    /// is decided on the name, not on the mark after it.
    #[test]
    fn sorting_leaves_every_heading_where_it_was() {
        let mut cx = cx();
        let mut grid = narrow_grid(&mut cx, 45.0);
        drawn(&mut cx, &mut grid);
        let before = (turned(&grid), grid.vp.col_header_rect);
        for col in 0..4 {
            grid.set_sort_indicator(Some((col, true)));
            drawn(&mut cx, &mut grid);
            assert_eq!((turned(&grid), grid.vp.col_header_rect), before, "sorted on {col}");
        }
    }

    /// Drag a narrow column's edge until it holds its name and the name
    /// lies down flat on the next draw; the others stay turned. Widen them
    /// all and the strip goes back to the stated height, so the option
    /// leaves no trace on a sheet that no longer needs it.
    #[test]
    fn dragging_a_narrow_column_wide_lays_its_name_flat_again() {
        let mut cx = cx();
        let mut grid = narrow_grid(&mut cx, 45.0);
        let mut frame = Frame::new(&mut cx, dvec2(420.0, 200.0));
        frame.draw(&mut cx, &mut grid, |_, _| {});
        assert_eq!(turned(&grid), vec![false, true, true, true]);

        let (_, x, w) = grid.vp.vis_cols[1];
        let edge = dvec2(x + w, heading(&grid, 1).y);
        primary_drag(&mut cx, &mut grid, &[edge, edge + dvec2(60.0, 0.0), edge + dvec2(120.0, 0.0)]);
        assert_eq!(grid.col_widths()[1], 32.0 + 120.0, "the drag resized the column");
        frame.draw(&mut cx, &mut grid, |_, _| {});
        assert_eq!(turned(&grid), vec![false, false, true, true], "dragged wide: flat");

        grid.set_col_widths(&mut cx, &[120.0, 152.0, 160.0, 200.0]);
        frame.draw(&mut cx, &mut grid, |_, _| {});
        assert_eq!(turned(&grid), vec![false; 4], "all wide: nothing turned");
        assert_eq!(grid.vp.col_header_rect.size.y, 28.0, "and the strip is the stated one");
    }
}
