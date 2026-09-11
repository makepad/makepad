//! Table — the plain table: columns with headings, rules, stripes, and rows
//! that are all present.
//!
//! There are two table-shaped widgets in this library and they answer
//! different questions.
//!
//! `DataGrid` is a spreadsheet. It virtualises both axes over a sparse size
//! table, holds none of your data, asks for cells one at a time inside a
//! draw loop, and carries column and row resizing, column reordering,
//! rectangle selection and cell editing. Reach for it when the row count is
//! large enough that drawing them all is out of the question, or when the
//! thing being built really is a sheet.
//!
//! This one is for the other case, which is far more common: a fixed list of
//! records — a price list, a set of results, the rows of a form — where every
//! row is already in memory and the whole job is to line them up under
//! headings and be readable. It takes the rows, all of them, and draws them.
//! There is no draw loop to write and no host callback to forget: a `Table`
//! with markup in it shows that markup, which a grid with no host behind it
//! never does.
//!
//! # Columns
//!
//! A column is a heading, a width and an alignment, written as one string:
//! `"Amount|90|end"`. The width may be left out (`"Name"`) or left empty
//! (`"Status||center"`), and a column without one shares what the fixed
//! columns leave over. Three parallel lists were the alternative and they go
//! out of step the first time somebody adds a column to one of them.
//!
//! # Cells
//!
//! A cell is text, or a widget. A cell written `@name` is built from the
//! template of that name on the table's own instance, so a column of buttons
//! or chips costs one named entry in the markup and nothing in the Rust.
//! Widget cells are not measured, so give their column a width.
//!
//! # Sorting says what was asked for
//!
//! Pressing a heading cycles that column: unsorted, up, down, unsorted
//! again. The table does **not** reorder anything. It moves the indicator
//! and raises `SortChanged`; the host, which owns the rows, puts them in the
//! new order and hands them back. That is the same contract `DataGrid`
//! keeps, and it is not laziness — the table holds text and widgets, not
//! values, so sorting here would sort the way a number happens to be
//! rendered ("10" before "9") and would have nothing at all to say about
//! dates, money or names. The third press matters as much as the first two:
//! it has to be able to get back to the order the data arrived in, which a
//! two-state toggle can never do.
//!
//! # What it deliberately does NOT do
//!
//! * **No virtualisation.** Every row is held; only the rows in view are
//!   drawn. That is honest for tens of rows and dishonest for millions —
//!   past a few hundred, the grid next door is the answer.
//! * **No selection and no editing.** There is nothing to press in a body
//!   row, so there is no hover wash and no focus ring on one. A cell that
//!   has to be pressable is a widget cell.
//! * **No column resizing or reordering.** Those need a grip on every
//!   boundary and a drag model, and they are what makes the grid a grid.
//! * **No keyboard.** The body scrolls under the wheel and the thumb; there
//!   is nothing to move a cursor between.
use crate::{
    badge::measure, makepad_derive_widget::*, makepad_draw::*, widget::*,
    widget_tree::CxWidgetExt,
};
use std::collections::HashMap;

/// Where a column's text sits in its cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TableAlign {
    #[default]
    Start,
    Center,
    End,
}

/// One column: what it is called, how wide it is, and where its text sits.
///
/// `width` of `None` means the column shares whatever the fixed columns
/// leave over.
#[derive(Clone, Debug, PartialEq)]
pub struct TableColumn {
    pub heading: String,
    pub width: Option<f64>,
    pub align: TableAlign,
}

impl TableColumn {
    /// A sharing column, aligned to the start.
    pub fn new(heading: &str) -> Self {
        Self { heading: heading.to_string(), width: None, align: TableAlign::Start }
    }

    pub fn fixed(heading: &str, width: f64) -> Self {
        Self { heading: heading.to_string(), width: Some(width), align: TableAlign::Start }
    }

    pub fn with_align(mut self, align: TableAlign) -> Self {
        self.align = align;
        self
    }
}

/// What one cell holds.
#[derive(Clone, Debug, PartialEq)]
pub enum TableCell {
    Text(String),
    /// A widget built from the template of this name on the table's
    /// instance. The table keeps one per cell, so a button in row four
    /// stays the same button across redraws and reports for row four.
    Widget(LiveId),
}

impl TableCell {
    pub fn text(text: &str) -> Self {
        Self::Text(text.to_string())
    }
}

/// The separator between the fields of a column spec and between the cells
/// of a row. A heading or a value that needs one has to come in through
/// `set_columns` / `set_rows` instead; markup is for tables written by hand.
const FIELD: char = '|';

/// One column spec: `"heading"`, `"heading|width"` or `"heading|width|align"`.
///
/// The fields are positional, so an alignment without a width leaves the
/// width field empty. A width that is not a positive number — including the
/// word `fill` — means the column shares, which is also what a missing field
/// means.
pub fn parse_column_spec(spec: &str) -> TableColumn {
    let mut fields = spec.split(FIELD);
    let heading = fields.next().unwrap_or("").trim().to_string();
    let width = fields
        .next()
        .map(str::trim)
        .and_then(|field| field.parse::<f64>().ok())
        .filter(|width| *width > 0.0 && width.is_finite());
    let align = match fields.next().map(|field| field.trim().to_lowercase()).as_deref() {
        Some("end") | Some("right") => TableAlign::End,
        Some("center") | Some("centre") => TableAlign::Center,
        _ => TableAlign::Start,
    };
    TableColumn { heading, width, align }
}

/// One row: cells separated by `|`, each trimmed. A cell written `@name` is
/// the template of that name rather than the text `@name`.
pub fn parse_row_line(line: &str) -> Vec<TableCell> {
    line.split(FIELD)
        .map(|cell| {
            let cell = cell.trim();
            match cell.strip_prefix('@') {
                // Interned on the way in, so a cell naming a template that
                // is not there can print the name it asked for instead of a
                // hash. The id itself is the same either way.
                Some(name) if !name.is_empty() => TableCell::Widget(
                    LiveId::from_str_with_lut(name).unwrap_or_else(|_| LiveId::from_str(name)),
                ),
                _ => TableCell::Text(cell.to_string()),
            }
        })
        .collect()
}

/// The sort a press on `col` moves to: unsorted, up, down, unsorted again.
///
/// Pressing a different column starts that column at up rather than
/// carrying the old direction over — the direction belongs to the question,
/// and the question just changed.
pub fn next_sort(current: Option<(usize, bool)>, col: usize) -> Option<(usize, bool)> {
    match current {
        Some((at, true)) if at == col => Some((col, false)),
        Some((at, false)) if at == col => None,
        _ => Some((col, true)),
    }
}

/// The width of every column, given the room inside the table.
///
/// A column with a width takes it. The rest share what is left: at least
/// what their own content needs, which is what `natural` carries, plus an
/// equal cut of anything over. When the fixed columns and the content
/// together do not fit, the sharing columns split what is left equally
/// instead, down to `min_width`, and the far edge of the table is clipped —
/// a table narrower than the widths it was handed is a mistake in the
/// widths, and squeezing the columns nobody sized would hide that mistake
/// in the wrong place.
fn column_widths(
    cols: &[TableColumn],
    natural: &[f64],
    room: f64,
    min_width: f64,
) -> Vec<f64> {
    let mut out: Vec<f64> = cols.iter().map(|col| col.width.unwrap_or(min_width)).collect();
    let sharing: Vec<usize> =
        cols.iter().enumerate().filter(|(_, col)| col.width.is_none()).map(|(i, _)| i).collect();
    if sharing.is_empty() {
        return out;
    }
    let wants = |i: usize| natural.get(i).copied().unwrap_or(min_width).max(min_width);
    let fixed: f64 = cols.iter().filter_map(|col| col.width).sum();
    let want: f64 = sharing.iter().map(|i| wants(*i)).sum();
    let left = room - fixed;
    if left >= want {
        let extra = (left - want) / sharing.len() as f64;
        for i in &sharing {
            out[*i] = wants(*i) + extra;
        }
    } else {
        let share = (left / sharing.len() as f64).max(min_width);
        for i in &sharing {
            out[*i] = share;
        }
    }
    out
}

/// The half-open range of rows showing through a body of `height` scrolled
/// to `scroll`. Always inside `rows`, so a stale scroll cannot ask for a row
/// that is not there.
fn rows_in_view(scroll: f64, height: f64, row_height: f64, rows: usize) -> (usize, usize) {
    if rows == 0 || row_height <= 0.0 || height <= 0.0 {
        return (0, 0);
    }
    let first = ((scroll / row_height).floor().max(0.0) as usize).min(rows);
    let last = ((((scroll + height) / row_height).ceil().max(0.0)) as usize).clamp(first, rows);
    (first, last)
}

/// The shortest a thumb may become, however long the table is. A thumb that
/// tracked the ratio all the way down would be a few pixels tall on a long
/// table and impossible to catch.
const MIN_THUMB: f64 = 24.0;

/// Where the scroll thumb sits in a track of `track` points, and how long it
/// is. A zero length means the content fits and there is no thumb to draw.
fn thumb_span(
    scroll: f64,
    scroll_max: f64,
    track: f64,
    content: f64,
    min_len: f64,
) -> (f64, f64) {
    if track <= 0.0 || content <= track {
        return (0.0, 0.0);
    }
    let len = (track * track / content).max(min_len).min(track);
    let travel = track - len;
    let at = if scroll_max > 0.0 { (scroll / scroll_max).clamp(0.0, 1.0) } else { 0.0 };
    (travel * at, len)
}

/// The scroll a thumb dragged to `top` means. The inverse of `thumb_span`,
/// so a thumb picked up and put down again lands the content where it was.
fn scroll_for_thumb(top: f64, track: f64, len: f64, scroll_max: f64) -> f64 {
    let travel = track - len;
    if travel <= 0.0 {
        return 0.0;
    }
    (top / travel).clamp(0.0, 1.0) * scroll_max
}

/// What a table reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TableAction {
    /// A heading was pressed and the sort moved on. `ascending` is `None`
    /// when the column has cycled back to unsorted.
    ///
    /// The table has NOT reordered anything: the rows belong to the host,
    /// which is the only thing that can put them in a different order. This
    /// says what the person asked for.
    SortChanged {
        col: usize,
        ascending: Option<bool>,
    },
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // Every value on the ground is a PLAIN literal with a field on the draw
    // struct behind it, because Rust writes all of them per draw. instance()
    // never binds to the Rust field, and uniform() on a type that also
    // carries instance slots moves those slots out from under a caller who
    // overrides one.
    mod.widgets.DrawTableGroundBase = #(DrawTableGround::script_component(vm))
    set_type_default() do #(DrawTableGround::script_shader(vm)){
        ..mod.draw.DrawQuad
        /** the field the rows stand on */
        color: theme.color_surface_container_low
        /** the frame around the whole table */
        border_color: theme.color_outline_variant
        /** how thick the frame is 0..4 step 0.5 */
        border_size: 1.0
        /** corner rounding 0..16 step 0.5 */
        radius: 4.0
        framed: 0.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(
                self.border_size * 0.5
                self.border_size * 0.5
                self.rect_size.x - self.border_size
                self.rect_size.y - self.border_size
                self.radius
            )
            sdf.fill_keep(self.color)
            // The frame is a flag rather than a second draw type: a table
            // without one still wants its corners rounded, so the box is
            // drawn either way and only the stroke is switched off.
            sdf.stroke(
                vec4(0.0, 0.0, 0.0, 0.0).mix(self.border_color, self.framed),
                self.border_size
            )
            return sdf.result
        }
    }

    mod.widgets.TableBase = #(Table::register_widget(vm))

    /** Columns with headings and rows that are all present: the plain table,
     * for tens of records rather than millions. Cells carry text, or the
     * widget named by a `@name` cell. */
    mod.widgets.Table = set_type_default() do mod.widgets.TableBase{
        width: Fill
        height: Fit

        /** the columns, each written heading|width|align with the last two optional */
        columns: []
        /** the rows, cells separated by | ; a cell written @name is that template */
        rows: []

        /** how tall one body row is 16..64 step 1 */
        row_height: 26.
        /** how tall the heading band is 16..64 step 1 */
        header_height: 28.
        /** the room on each side of a cell's text 0..32 step 1 */
        cell_pad: 10.
        /** the narrowest a sharing column may become 16..240 step 2 */
        min_col_width: 40.
        /** how thick a hairline is 0..3 step 0.5 */
        line_size: 1.
        /** how wide the scroll thumb is 0..14 step 1 */
        thumb_size: 6.

        /** show the heading band */
        show_header: true
        /** wash every other row */
        striped: false
        /** a hairline under every row */
        row_lines: true
        /** a hairline between every column */
        column_lines: false
        /** a frame around the whole table */
        framed: false
        /** pressing a heading cycles that column's sort */
        sortable: false

        /** the heading band */
        color_header: theme.color_surface_container_highest
        /** a heading under the pointer, when that column sorts */
        color_header_hover: theme.color_primary_container
        /** every other row, when the table is striped */
        color_stripe: theme.color_surface_container_high
        /** the hairlines between rows and columns */
        color_rule: theme.color_outline_variant
        /** the scroll thumb */
        color_thumb: theme.color_outline

        draw_text +: {
            color: theme.color_text
            // line_spacing 1.0 because the cell text is placed by hand: a
            // taller line box moves the ink down inside it and the row comes
            // out bottom-heavy.
            text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
        }
        draw_heading +: {
            color: theme.color_text
            text_style: theme.font_bold{font_size: theme.font_size_p line_spacing: 1.0}
        }
    }

    /** Every other row washed, so a wide row can be followed across without
     * a finger on the screen. */
    mod.widgets.TableStriped = mod.widgets.Table{
        striped: true
    }

    /** A hairline between every column and a frame around the whole table,
     * for a table of short values where the columns are the point. */
    mod.widgets.TableBordered = mod.widgets.Table{
        column_lines: true
        framed: true
    }

    /** Tighter rows for a table that has to fit. It tightens the rows and
     * does not shrink the type: a table you cannot read is not denser. */
    mod.widgets.TableCompact = mod.widgets.Table{
        row_height: 22.
        header_height: 24.
        cell_pad: 6.
    }
}

/// The table's field, and the frame around it. Everything else the table
/// paints is a flat rectangle, which `DrawColor` already is.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTableGround {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_size: f32,
    #[live]
    radius: f32,
    #[live]
    framed: f32,
}

#[derive(Script, Widget)]
pub struct Table {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The table's own rect, so a wheel anywhere in it scrolls the body and
    /// a press anywhere in it lands somewhere.
    #[redraw]
    #[live]
    draw_bg: DrawTableGround,
    /// The band, the stripes, the rules and the thumb: one flat rectangle
    /// with the colour set before each draw.
    #[live]
    draw_fill: DrawColor,
    #[live]
    pub draw_text: DrawText,
    #[live]
    pub draw_heading: DrawText,

    /// The columns as markup, one spec per column.
    #[live]
    pub columns: Vec<String>,
    /// The rows as markup, one line per row.
    #[live]
    pub rows: Vec<String>,

    #[live(26.0)]
    pub row_height: f64,
    #[live(28.0)]
    pub header_height: f64,
    #[live(10.0)]
    pub cell_pad: f64,
    #[live(40.0)]
    pub min_col_width: f64,
    #[live(1.0)]
    pub line_size: f64,
    #[live(6.0)]
    pub thumb_size: f64,

    #[live(true)]
    pub show_header: bool,
    #[live]
    pub striped: bool,
    #[live(true)]
    pub row_lines: bool,
    #[live]
    pub column_lines: bool,
    #[live]
    pub framed: bool,
    /// Pressing a column heading cycles that column's sort. Off by default:
    /// a heading that does something when pressed has to mean it, and most
    /// tables are read-only.
    #[live]
    pub sortable: bool,

    #[live]
    pub color_header: Vec4f,
    #[live]
    pub color_header_hover: Vec4f,
    #[live]
    pub color_stripe: Vec4f,
    #[live]
    pub color_rule: Vec4f,
    #[live]
    pub color_thumb: Vec4f,

    #[rust]
    cols: Vec<TableColumn>,
    #[rust]
    body: Vec<Vec<TableCell>>,
    /// A host has handed over columns or rows, so that markup stops seeding.
    #[rust]
    cols_set: bool,
    #[rust]
    rows_set: bool,
    #[rust]
    seeded_cols: Vec<String>,
    #[rust]
    seeded_rows: Vec<String>,

    #[rust]
    sort: Option<(usize, bool)>,
    /// Columns that refuse to sort even when the table does — a column of
    /// controls, a column of pictures, anything with no order to be in. Set
    /// from the host rather than the markup: which column has no order is
    /// something the side that owns the rows knows anyway.
    #[rust]
    unsortable: Vec<usize>,

    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    cells: HashMap<(usize, usize), WidgetRef>,
    /// The cells actually drawn this pass. A widget scrolled out of view
    /// keeps the area it last drew at, so events go only to these — without
    /// it a button off the top of the table stays pressable where it used
    /// to be.
    #[rust]
    drawn_cells: Vec<(usize, usize)>,

    #[rust]
    heads: Vec<Rect>,
    #[rust]
    hot_head: Option<usize>,
    #[rust]
    head_press: Option<usize>,

    #[rust]
    scroll: f64,
    #[rust]
    scroll_max: f64,
    #[rust]
    content_h: f64,
    #[rust]
    body_rect: Rect,
    #[rust]
    thumb: Option<Rect>,
    /// Where the thumb was caught, measured from its own top, so it moves
    /// under the finger rather than jumping to it.
    #[rust]
    thumb_grab: Option<f64>,
    #[rust]
    area: Area,
}

impl ScriptHook for Table {
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
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Cell templates arrive as named entries on the instance, the way a
        // list's item templates do.
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
            // The cell widgets came from templates that may have just
            // changed, so they are rebuilt rather than patched.
            self.cells.clear();
            self.drawn_cells.clear();
        }
    }
}

impl Table {
    /// The columns, from the host. Markup columns stop being read.
    pub fn set_columns(&mut self, cx: &mut Cx, columns: Vec<TableColumn>) {
        self.cols_set = true;
        self.cols = columns;
        self.redraw(cx);
    }

    /// The rows, from the host. Markup rows stop being read.
    ///
    /// Handing over a different set of rows drops the cell widgets: a
    /// widget belongs to the cell it was built for, and keeping it across a
    /// change of rows would put row four's button in row two.
    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<Vec<TableCell>>) {
        self.rows_set = true;
        self.body = rows;
        self.cells.clear();
        self.drawn_cells.clear();
        self.scroll = self.scroll.min(self.scroll_max);
        self.redraw(cx);
    }

    /// The rows in the markup form: one line per row, cells separated by
    /// `|`. This is what a host re-sorting its own rows hands back.
    pub fn set_row_lines(&mut self, cx: &mut Cx, lines: &[String]) {
        let rows = lines.iter().map(|line| parse_row_line(line)).collect();
        self.set_rows(cx, rows);
    }

    pub fn row_count(&self) -> usize {
        self.body.len()
    }

    /// The column being sorted and which way, or nothing.
    pub fn sort(&self) -> Option<(usize, bool)> {
        self.sort
    }

    pub fn set_sort_indicator(&mut self, cx: &mut Cx, sort: Option<(usize, bool)>) {
        self.sort = sort;
        self.redraw(cx);
    }

    /// Columns that will not sort, whatever `sortable` says.
    pub fn set_unsortable_cols(&mut self, cols: Vec<usize>) {
        self.unsortable = cols;
    }

    /// The widget in a cell, or an empty ref for a cell that has none. The
    /// host reads that widget's own actions to find out it was pressed.
    pub fn cell_widget_ref(&self, row: usize, col: usize) -> WidgetRef {
        self.cells.get(&(row, col)).cloned().unwrap_or_default()
    }

    /// A table written in markup shows itself without the host saying
    /// anything, and shows itself again after a live reload changes it.
    fn seed(&mut self) {
        if !self.cols_set && self.seeded_cols != self.columns {
            self.seeded_cols = self.columns.clone();
            self.cols = self.columns.iter().map(|spec| parse_column_spec(spec)).collect();
        }
        if !self.rows_set && self.seeded_rows != self.rows {
            self.seeded_rows = self.rows.clone();
            self.body = self.rows.iter().map(|line| parse_row_line(line)).collect();
            self.cells.clear();
            self.drawn_cells.clear();
        }
    }

    fn can_sort(&self, col: usize) -> bool {
        self.sortable
            && !self.unsortable.contains(&col)
            && self.cols.get(col).is_some_and(|c| !c.heading.is_empty())
    }

    fn head_at(&self, pos: Vec2d) -> Option<usize> {
        self.heads.iter().position(|rect| rect.contains(pos))
    }

    /// What each sharing column would need to show its own content: the
    /// widest of its heading and its text cells, plus the padding either
    /// side. A widget cell measures as nothing, which is why a column of
    /// widgets should be given a width.
    fn natural_widths(&self, cx: &mut Cx2d) -> Vec<f64> {
        let mut out = Vec::with_capacity(self.cols.len());
        for (index, col) in self.cols.iter().enumerate() {
            if let Some(width) = col.width {
                out.push(width);
                continue;
            }
            let mut widest = if self.show_header {
                measure(&self.draw_heading, cx, &col.heading)
            } else {
                0.0
            };
            for row in &self.body {
                if let Some(TableCell::Text(text)) = row.get(index) {
                    widest = widest.max(measure(&self.draw_text, cx, text));
                }
            }
            out.push((widest + self.cell_pad * 2.0).max(self.min_col_width));
        }
        out
    }

    /// The widget for a cell, built once and kept. A template that is not
    /// there gives nothing back, and the caller draws the name instead.
    fn cell_widget(&mut self, cx: &mut Cx, row: usize, col: usize, name: LiveId) -> Option<WidgetRef> {
        if let Some(widget) = self.cells.get(&(row, col)) {
            return Some(widget.clone());
        }
        let template = self.templates.get(&name)?;
        let value: ScriptValue = template.as_object().into();
        let widget = cx.with_vm(|vm| WidgetRef::script_from_value(vm, value));
        // A tree node under the table, so the design overlay can pick a cell
        // and style the template it came from.
        cx.widget_tree_insert_child(
            self.uid,
            LiveId::from_lo_hi(col as u32 + 1, row as u32 + 1),
            widget.clone(),
        );
        self.cells.insert((row, col), widget.clone());
        Some(widget)
    }
}

/// Draw one run inside a cell, placed by the column's alignment and clipped
/// to the cell so a long value cannot bleed into its neighbour.
///
/// A free function, and not a method, because the draw loop is already
/// holding the columns and the rows: taking `&mut self` here would take the
/// lot.
fn draw_run(
    draw_text: &mut DrawText,
    cx: &mut Cx2d,
    rect: Rect,
    pad: f64,
    align: TableAlign,
    text: &str,
) {
    if text.is_empty() || rect.size.x <= 0.0 {
        return;
    }
    let width = measure(draw_text, cx, text);
    let x = match align {
        TableAlign::Start => rect.pos.x + pad,
        TableAlign::Center => rect.pos.x + (rect.size.x - width) * 0.5,
        TableAlign::End => rect.pos.x + rect.size.x - pad - width,
    };
    // draw_abs takes the top of the LINE box, and the ink starts about
    // three tenths of the font size below it.
    let size = draw_text.text_style.font_size as f64;
    let y = rect.pos.y + (rect.size.y - size) * 0.5 - size * 0.30;
    cx.push_clip_rect(rect);
    draw_text.draw_abs(cx, dvec2(x, y), text);
    cx.pop_clip_rect();
}

fn align_x(align: TableAlign) -> f64 {
    match align {
        TableAlign::Start => 0.0,
        TableAlign::Center => 0.5,
        TableAlign::End => 1.0,
    }
}

impl Widget for Table {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.seed();
        // The model is taken by value for the pass: the draw loop calls back
        // into the table to build cell widgets, and a borrow of the columns
        // held across that would be a borrow of the whole table.
        let cols = self.cols.clone();
        let body = self.body.clone();

        let natural = self.natural_widths(cx);
        let frame = if self.framed { self.draw_bg.border_size as f64 } else { 0.0 };
        let header_h = if self.show_header { self.header_height.max(0.0) } else { 0.0 };
        let content_h = body.len() as f64 * self.row_height;

        // A Fill inside a Fit resolves to nothing, so a Fit table has to
        // work out its own size before a single row is drawn.
        let natural_w = natural.iter().sum::<f64>() + frame * 2.0;
        let natural_h = header_h + content_h + frame * 2.0;
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(natural_w),
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(natural_h),
                other => other,
            },
            ..walk
        };

        self.draw_bg.framed = if self.framed { 1.0 } else { 0.0 };
        self.draw_bg.begin(cx, walk, self.layout);
        // The INNER rect: rows laid out against the turtle's own rect would
        // ignore any padding the caller asked for.
        let outer = cx.turtle().inner_rect();
        let inner = Rect {
            pos: outer.pos + dvec2(frame, frame),
            size: dvec2(
                (outer.size.x - frame * 2.0).max(0.0),
                (outer.size.y - frame * 2.0).max(0.0),
            ),
        };
        let widths = column_widths(&cols, &natural, inner.size.x, self.min_col_width);

        // The heading band, which does not move: the body is clipped to its
        // own rect below it, so there is nothing to keep it above.
        self.heads.clear();
        if header_h > 0.0 {
            let band = Rect { pos: inner.pos, size: dvec2(inner.size.x, header_h) };
            self.draw_fill.color = self.color_header;
            self.draw_fill.draw_abs(cx, band);

            let mut x = inner.pos.x;
            for (index, width) in widths.iter().enumerate() {
                let rect = Rect { pos: dvec2(x, band.pos.y), size: dvec2(*width, header_h) };
                if self.hot_head == Some(index) {
                    self.draw_fill.color = self.color_header_hover;
                    self.draw_fill.draw_abs(cx, rect);
                }
                self.heads.push(rect);
                x += width;
            }

            if self.row_lines && self.line_size > 0.0 {
                self.draw_fill.color = self.color_rule;
                self.draw_fill.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(inner.pos.x, band.pos.y + header_h - self.line_size),
                        size: dvec2(inner.size.x, self.line_size),
                    },
                );
            }

            // The labels last, so neither the hover wash nor the rule paints
            // over them.
            for (index, col) in cols.iter().enumerate() {
                let Some(rect) = self.heads.get(index).copied() else {
                    continue;
                };
                let mut label = col.heading.clone();
                if let Some((sorted, ascending)) = self.sort {
                    if sorted == index {
                        // U+25B2 and U+25BC, which the default fonts carry.
                        label.push_str(if ascending { " \u{25B2}" } else { " \u{25BC}" });
                    }
                }
                draw_run(&mut self.draw_heading, cx, rect, self.cell_pad, col.align, &label);
            }
        }

        let body_rect = Rect {
            pos: dvec2(inner.pos.x, inner.pos.y + header_h),
            size: dvec2(inner.size.x, (inner.size.y - header_h).max(0.0)),
        };
        self.body_rect = body_rect;
        self.content_h = content_h;
        self.scroll_max = (content_h - body_rect.size.y).max(0.0);
        self.scroll = self.scroll.clamp(0.0, self.scroll_max);

        cx.push_clip_rect(body_rect);
        let (first, last) =
            rows_in_view(self.scroll, body_rect.size.y, self.row_height, body.len());
        self.drawn_cells.clear();
        for index in first..last {
            let y = body_rect.pos.y + index as f64 * self.row_height - self.scroll;
            if self.striped && index % 2 == 1 {
                self.draw_fill.color = self.color_stripe;
                self.draw_fill.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(inner.pos.x, y),
                        size: dvec2(inner.size.x, self.row_height),
                    },
                );
            }

            let mut x = inner.pos.x;
            for (col_index, col) in cols.iter().enumerate() {
                let width = widths.get(col_index).copied().unwrap_or(0.0);
                let rect = Rect { pos: dvec2(x, y), size: dvec2(width, self.row_height) };
                x += width;
                match body[index].get(col_index) {
                    Some(TableCell::Text(text)) => {
                        draw_run(&mut self.draw_text, cx, rect, self.cell_pad, col.align, text);
                    }
                    Some(TableCell::Widget(name)) => {
                        let name = *name;
                        match self.cell_widget(cx.cx.cx, index, col_index, name) {
                            Some(widget) => {
                                cx.begin_turtle(
                                    Walk {
                                        abs_pos: Some(rect.pos),
                                        width: Size::Fixed(rect.size.x),
                                        height: Size::Fixed(rect.size.y),
                                        ..Default::default()
                                    },
                                    Layout {
                                        flow: Flow::right(),
                                        align: Align { x: align_x(col.align), y: 0.5 },
                                        padding: Inset {
                                            left: self.cell_pad,
                                            right: self.cell_pad,
                                            top: 0.0,
                                            bottom: 0.0,
                                        },
                                        ..Layout::default()
                                    },
                                );
                                widget.draw_all(cx, scope);
                                cx.end_turtle();
                                self.drawn_cells.push((index, col_index));
                            }
                            None => {
                                // A cell naming a template that is not there
                                // draws the name, so a typo is visible rather
                                // than an empty box.
                                let miss = format!("@{name}");
                                draw_run(
                                    &mut self.draw_text,
                                    cx,
                                    rect,
                                    self.cell_pad,
                                    col.align,
                                    &miss,
                                );
                            }
                        }
                    }
                    None => {}
                }
            }

            // No rule under the last row: the field ends there, and a line
            // hanging under the final row reads as a row that failed to draw.
            if self.row_lines && self.line_size > 0.0 && index + 1 < body.len() {
                self.draw_fill.color = self.color_rule;
                self.draw_fill.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(inner.pos.x, y + self.row_height - self.line_size),
                        size: dvec2(inner.size.x, self.line_size),
                    },
                );
            }
        }

        if self.column_lines && self.line_size > 0.0 && widths.len() > 1 {
            let mut x = inner.pos.x;
            for width in &widths[..widths.len() - 1] {
                x += width;
                self.draw_fill.color = self.color_rule;
                self.draw_fill.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(x - self.line_size * 0.5, body_rect.pos.y),
                        size: dvec2(self.line_size, body_rect.size.y),
                    },
                );
            }
        }
        cx.pop_clip_rect();

        let (top, len) = thumb_span(
            self.scroll,
            self.scroll_max,
            body_rect.size.y,
            content_h,
            MIN_THUMB,
        );
        self.thumb = if len > 0.0 && self.thumb_size > 0.0 {
            let rect = Rect {
                pos: dvec2(
                    inner.pos.x + inner.size.x - self.thumb_size - 2.0,
                    body_rect.pos.y + top,
                ),
                size: dvec2(self.thumb_size, len),
            };
            self.draw_fill.color = self.color_thumb;
            self.draw_fill.draw_abs(cx, rect);
            Some(rect)
        } else {
            None
        };

        self.draw_bg.end(cx);
        self.area = self.draw_bg.area();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Only the cells that drew this pass: the rest are holding the area
        // they last drew at, somewhere off the top or bottom of the body.
        let live: Vec<WidgetRef> = self
            .drawn_cells
            .iter()
            .filter_map(|key| self.cells.get(key).cloned())
            .collect();
        for widget in &live {
            widget.handle_event(cx, event, scope);
        }

        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let at = self.head_at(fe.abs).filter(|col| self.can_sort(*col));
                if at != self.hot_head {
                    self.hot_head = at;
                    cx.set_cursor(if at.is_some() {
                        MouseCursor::Hand
                    } else {
                        MouseCursor::Default
                    });
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hot_head.take().is_some() {
                    self.redraw(cx);
                }
            }
            Hit::FingerScroll(fe) if self.scroll_max > 0.0 => {
                let next = (self.scroll + fe.scroll.y).clamp(0.0, self.scroll_max);
                if next != self.scroll {
                    self.scroll = next;
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                // The thumb is drawn slim and caught wide: a six-point strip
                // is a fair target for the eye and a poor one for a finger.
                if let Some(thumb) = self.thumb {
                    let catch = Rect {
                        pos: dvec2(thumb.pos.x - 6.0, thumb.pos.y),
                        size: dvec2(thumb.size.x + 8.0, thumb.size.y),
                    };
                    if catch.contains(fe.abs) {
                        self.thumb_grab = Some(fe.abs.y - thumb.pos.y);
                        return;
                    }
                }
                self.head_press = self.head_at(fe.abs).filter(|col| self.can_sort(*col));
            }
            Hit::FingerMove(fe) => {
                let Some(grab) = self.thumb_grab else {
                    return;
                };
                let (_, len) = thumb_span(
                    self.scroll,
                    self.scroll_max,
                    self.body_rect.size.y,
                    self.content_h,
                    MIN_THUMB,
                );
                let top = fe.abs.y - grab - self.body_rect.pos.y;
                let next =
                    scroll_for_thumb(top, self.body_rect.size.y, len, self.scroll_max);
                if next != self.scroll {
                    self.scroll = next;
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(fe) => {
                self.thumb_grab = None;
                // A press that slid off its heading is not a press on it.
                let pressed = self.head_press.take();
                if let Some(col) = pressed {
                    if fe.is_over && self.head_at(fe.abs) == Some(col) {
                        let next = next_sort(self.sort, col);
                        self.sort = next;
                        let uid = self.uid;
                        self.redraw(cx);
                        cx.widget_action(
                            uid,
                            TableAction::SortChanged {
                                col,
                                ascending: next.map(|(_, ascending)| ascending),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }

    /// The sort, so a test can read the table in one line.
    fn text(&self) -> String {
        match self.sort {
            Some((col, ascending)) => format!(
                "{} {}",
                self.cols.get(col).map(|c| c.heading.as_str()).unwrap_or_default(),
                if ascending { "up" } else { "down" }
            ),
            None => String::new(),
        }
    }
}

impl TableRef {
    pub fn set_columns(&self, cx: &mut Cx, columns: Vec<TableColumn>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_columns(cx, columns);
        }
    }

    pub fn set_rows(&self, cx: &mut Cx, rows: Vec<Vec<TableCell>>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_rows(cx, rows);
        }
    }

    /// The rows in the markup form: one line per row, cells separated by
    /// `|`. What a host hands back after re-sorting its own rows.
    pub fn set_row_lines(&self, cx: &mut Cx, lines: &[String]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_row_lines(cx, lines);
        }
    }

    pub fn row_count(&self) -> usize {
        self.borrow().map(|inner| inner.row_count()).unwrap_or(0)
    }

    /// The sort a heading press just asked for, if it changed this pass.
    /// `Some((col, None))` means that column went back to unsorted, and the
    /// rows belong in the order they arrived in.
    pub fn sort_changed(&self, actions: &Actions) -> Option<(usize, Option<bool>)> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            TableAction::SortChanged { col, ascending } => Some((col, ascending)),
            _ => None,
        }
    }

    pub fn sort(&self) -> Option<(usize, bool)> {
        self.borrow().and_then(|inner| inner.sort())
    }

    pub fn set_sort_indicator(&self, cx: &mut Cx, sort: Option<(usize, bool)>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_sort_indicator(cx, sort);
        }
    }

    pub fn set_unsortable_cols(&self, cols: Vec<usize>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_unsortable_cols(cols);
        }
    }

    /// The widget in a cell, for reading its own actions. An empty ref when
    /// that cell holds text or has not been drawn yet.
    pub fn cell_widget(&self, row: usize, col: usize) -> WidgetRef {
        self.borrow().map(|inner| inner.cell_widget_ref(row, col)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(count: usize) -> Vec<TableColumn> {
        (0..count).map(|i| TableColumn::new(&format!("c{i}"))).collect()
    }

    /// A spec is heading, width, alignment, in that order, and every field
    /// after the first may be left out or left empty.
    #[test]
    fn a_column_spec_reads_heading_width_and_alignment() {
        assert_eq!(
            parse_column_spec("Name"),
            TableColumn { heading: "Name".into(), width: None, align: TableAlign::Start }
        );
        assert_eq!(
            parse_column_spec("Amount|90|end"),
            TableColumn { heading: "Amount".into(), width: Some(90.0), align: TableAlign::End }
        );
        assert_eq!(
            parse_column_spec("Status||center"),
            parse_column_spec("Status | | centre"),
            "the fields are trimmed, and an empty width means the column shares"
        );
        assert_eq!(parse_column_spec("Status||center").align, TableAlign::Center);
        assert_eq!(parse_column_spec("Status||center").width, None);
    }

    /// A width that is not a positive finite number means the column shares,
    /// which is the same as leaving the field out. A table whose column
    /// vanished because somebody wrote `fill` would be a puzzle.
    #[test]
    fn a_width_that_is_not_a_number_shares() {
        assert_eq!(parse_column_spec("Name|fill").width, None);
        assert_eq!(parse_column_spec("Name|0").width, None);
        assert_eq!(parse_column_spec("Name|-40").width, None);
    }

    /// A cell beginning with @ names a template; a bare @ is just text.
    #[test]
    fn a_cell_is_text_unless_it_names_a_template() {
        let cells = parse_row_line("Kettle | 12.50 | @open | @");
        assert_eq!(cells[0], TableCell::text("Kettle"));
        assert_eq!(cells[1], TableCell::text("12.50"));
        assert_eq!(cells[2], TableCell::Widget(live_id!(open)));
        assert_eq!(cells[3], TableCell::text("@"), "a bare @ names nothing");
    }

    /// Three presses on one heading get back to the order the data arrived
    /// in. A two-state toggle never can, which is the whole reason for the
    /// third state.
    #[test]
    fn a_third_press_returns_to_unsorted() {
        let mut sort = None;
        sort = next_sort(sort, 2);
        assert_eq!(sort, Some((2, true)));
        sort = next_sort(sort, 2);
        assert_eq!(sort, Some((2, false)));
        sort = next_sort(sort, 2);
        assert_eq!(sort, None, "back to the order it arrived in");
    }

    /// Moving to another column starts that column at ascending rather than
    /// carrying the old direction over.
    #[test]
    fn another_column_starts_ascending() {
        assert_eq!(next_sort(Some((2, false)), 5), Some((5, true)));
        assert_eq!(next_sort(Some((2, true)), 5), Some((5, true)));
    }

    /// With nothing fixed and equal content, the columns come out equal.
    #[test]
    fn sharing_columns_split_the_room() {
        let widths = column_widths(&share(3), &[50.0, 50.0, 50.0], 300.0, 40.0);
        assert_eq!(widths, vec![100.0, 100.0, 100.0]);
    }

    /// A sharing column keeps what its own content needs and only the
    /// surplus is split, so a column of long names does not come out the
    /// same width as a column of ticks.
    #[test]
    fn a_wider_column_stays_wider() {
        let widths = column_widths(&share(2), &[200.0, 60.0], 400.0, 40.0);
        assert_eq!(widths, vec![270.0, 130.0], "the 140 over is split evenly");
    }

    /// Fixed columns take their width first; what is left is shared.
    #[test]
    fn fixed_columns_take_their_width_first() {
        let cols = vec![
            TableColumn::new("name"),
            TableColumn::fixed("amount", 80.0),
            TableColumn::new("note"),
        ];
        let widths = column_widths(&cols, &[60.0, 80.0, 60.0], 400.0, 40.0);
        assert_eq!(widths, vec![160.0, 80.0, 160.0]);
    }

    /// When the fixed widths already fill the table there is nothing left to
    /// share, and the sharing columns fall back to the floor rather than to
    /// nothing — a column of zero width is invisible, and an invisible
    /// column is a bug that looks like missing data.
    #[test]
    fn a_table_too_narrow_for_its_widths_floors_the_rest() {
        let cols = vec![TableColumn::fixed("a", 200.0), TableColumn::new("b")];
        let widths = column_widths(&cols, &[200.0, 90.0], 210.0, 40.0);
        assert_eq!(widths, vec![200.0, 40.0]);
    }

    /// With no sharing column the leftover room is simply left over. The
    /// last column does not silently stretch into it: the widths were asked
    /// for, and a table that quietly ignores one of them is worse than a
    /// gap.
    #[test]
    fn nothing_stretches_when_every_column_is_fixed() {
        let cols = vec![TableColumn::fixed("a", 100.0), TableColumn::fixed("b", 50.0)];
        assert_eq!(column_widths(&cols, &[100.0, 50.0], 400.0, 40.0), vec![100.0, 50.0]);
    }

    /// The view holds the row under the top edge and the one crossing the
    /// bottom, so a half row is drawn rather than left blank.
    #[test]
    fn the_view_takes_the_partial_rows_at_both_edges() {
        assert_eq!(rows_in_view(0.0, 100.0, 25.0, 20), (0, 4));
        assert_eq!(rows_in_view(10.0, 100.0, 25.0, 20), (0, 5));
        assert_eq!(rows_in_view(50.0, 100.0, 25.0, 20), (2, 6));
    }

    /// A scroll past the end, or a row count that just shrank, can never ask
    /// for a row that is not there.
    #[test]
    fn the_view_stays_inside_the_rows() {
        assert_eq!(rows_in_view(1000.0, 100.0, 25.0, 6), (6, 6));
        assert_eq!(rows_in_view(0.0, 100.0, 25.0, 2), (0, 2));
        assert_eq!(rows_in_view(0.0, 100.0, 25.0, 0), (0, 0));
    }

    /// Content that fits has no thumb at all, rather than a thumb the whole
    /// length of the track that does nothing when dragged.
    #[test]
    fn content_that_fits_has_no_thumb() {
        assert_eq!(thumb_span(0.0, 0.0, 200.0, 150.0, 24.0), (0.0, 0.0));
    }

    /// The thumb reaches the top at zero and the bottom at the end, and the
    /// drag maps back to exactly the scroll it came from.
    #[test]
    fn the_thumb_and_the_drag_agree() {
        let (track, content) = (200.0, 400.0);
        let max = content - track;
        let (top, len) = thumb_span(0.0, max, track, content, 24.0);
        assert_eq!((top, len), (0.0, 100.0));

        let (top, len) = thumb_span(max, max, track, content, 24.0);
        assert_eq!(top, track - len, "the thumb ends flush with the track");
        assert_eq!(scroll_for_thumb(top, track, len, max), max);

        let (top, len) = thumb_span(50.0, max, track, content, 24.0);
        assert_eq!(scroll_for_thumb(top, track, len, max), 50.0);
    }

    /// A very long table still leaves something to catch.
    #[test]
    fn the_thumb_never_shrinks_past_the_floor() {
        let (_, len) = thumb_span(0.0, 9000.0, 200.0, 9200.0, 24.0);
        assert_eq!(len, 24.0);
    }
}
