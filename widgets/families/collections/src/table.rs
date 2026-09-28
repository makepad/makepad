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
//!
//! # Headings on the diagonal
//!
//! `header_angle` is permission, not an instruction. With it set, a heading
//! turns only when its name does not fit its column flat — measured, name
//! plus the cell padding against the column's own width — and a heading that
//! does fit stays exactly where it was, flat on the bottom line of the band.
//! A table of wide columns with an angle set therefore draws the same
//! rectangles as the same table without one, and the band is only as tall as
//! the names that did turn need.
//!
//! That is the whole point of it: a column of three-digit numbers is three
//! digits wide and its name is ten times that, so the name goes on the
//! diagonal and the column keeps the width of its VALUES. Turning a name
//! that already fits buys nothing and costs the reader a tilted head.
//! `header_lean` says which way the ones that do turn lean; the arithmetic
//! is in `diagonal_text.rs`, which the panel kit's own diagonal header
//! shares.
use crate::{
    diagonal_text::{
        diagonal_overhang, diagonal_row_height, draw_diagonal_name, heading_fits_flat, DiagonalLean,
        DiagonalRun,
    },
    family_api::measure,
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
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

/// Which columns' headings turn, given what each column came out as.
fn turned_headings(name_widths: &[f64], widths: &[f64], pad: f64) -> Vec<bool> {
    name_widths
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let width = widths.get(i).copied().unwrap_or(0.0);
            !heading_fits_flat(*name, width, pad)
        })
        .collect()
}

/// The longest name among the headings that turn, or `None` when none do.
fn longest_turned(name_widths: &[f64], turned: &[bool]) -> Option<(usize, f64)> {
    name_widths
        .iter()
        .copied()
        .enumerate()
        .filter(|(i, _)| turned.get(*i).copied().unwrap_or(false))
        .fold(None, |best: Option<(usize, f64)>, (i, w)| match best {
            Some((_, b)) if b >= w => best,
            _ => Some((i, w)),
        })
}

/// How far the turned names' ink reaches past the columns' own span, on the
/// side they lean over: the strip the table has to keep back for it.
///
/// Measured column by column from where each name actually stands, rather
/// than as the longest name's whole reach, because a name standing over a
/// column in the middle mostly hangs over its neighbours — which is the
/// point — and only what leaves the columns altogether needs room of its
/// own. Nothing when nothing turned.
fn ink_overreach(
    name_widths: &[f64],
    widths: &[f64],
    turned: &[bool],
    angle: f64,
    lean: DiagonalLean,
) -> f64 {
    let span: f64 = widths.iter().sum();
    let mut x = 0.0;
    let mut over = 0.0f64;
    for (i, width) in widths.iter().enumerate() {
        if turned.get(i).copied().unwrap_or(false) {
            let middle = x + width * 0.5;
            let reach = diagonal_overhang(name_widths.get(i).copied().unwrap_or(0.0), angle);
            over = over.max(match lean {
                DiagonalLean::Rise => middle + reach - span,
                DiagonalLean::Fall => reach - middle,
            });
        }
        x += width;
    }
    over.max(0.0)
}

/// The columns' widths, which headings turn, and the strip kept back for
/// the ink, settled together for a table with `room` inside it.
///
/// They depend on each other — a strip kept back narrows the sharing
/// columns, and a narrower column may be one more that has to turn — so this
/// starts from no strip at all and widens it until the ink fits. Each round
/// can only turn more headings, never fewer, so it ends within a round per
/// column; and a table where nothing turns stops on the first round with the
/// widths it always had, which is what "exactly as before" rests on.
fn settle_columns(
    cols: &[TableColumn],
    natural: &[f64],
    names: &[f64],
    room: f64,
    min_width: f64,
    pad: f64,
    angle: f64,
    lean: DiagonalLean,
) -> (Vec<f64>, Vec<bool>, f64) {
    let mut reserve = 0.0f64;
    let mut rounds = 0;
    loop {
        let widths = column_widths(cols, natural, (room - reserve).max(0.0), min_width);
        let turned = if names.is_empty() {
            vec![false; widths.len()]
        } else {
            turned_headings(names, &widths, pad)
        };
        let need = ink_overreach(names, &widths, &turned, angle, lean);
        rounds += 1;
        if need <= reserve + 0.01 || rounds > cols.len() + 1 {
            return (widths, turned, reserve);
        }
        reserve = need;
    }
}

/// How tall the heading band is, given the longest turned name and the line
/// it is written in: the stated height when nothing turned — so a table with
/// nothing to turn is not a point taller for having the option — and
/// otherwise what the longest turned name needs, but never less than the
/// stated height, which the flat headings beside it still stand in.
///
/// Up to the whole point: a Fit table sizes itself to the band plus its
/// rows, and a band of 106.07 points leaves the rows a fraction short of what
/// they asked for — which is a scroll bar down the side of a table that has
/// nothing to scroll.
fn heading_band_height(stated: f64, longest_turned: Option<f64>, line: f64, angle: f64) -> f64 {
    match longest_turned {
        None => stated,
        Some(width) => diagonal_row_height(width, line, angle).ceil().max(stated),
    }
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
        /** how far a heading too long for its column is turned, in degrees; one that fits stays flat, and 0 turns none 0..80 step 5 */
        header_angle: 0.
        /** which way a turned heading leans: Rise hangs it over the right, Fall over the left */
        header_lean: DiagonalLean.Rise
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
    /// The same headings, turned. It carries no style of its own on purpose:
    /// the face and the ink are copied from `draw_heading` before each draw,
    /// so a table styled once is styled at every angle and there is no second
    /// place to forget.
    #[live]
    draw_heading_turned: DrawRotatedText,

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
    /// How far a heading is turned from the horizontal when it has to be,
    /// in degrees. Nought never turns one. Past nought, only a heading whose
    /// name does not fit its column flat turns; the band is then as tall as
    /// the longest turned name needs and never shorter than `header_height`,
    /// and with nothing to turn it is `header_height` exactly.
    #[live]
    pub header_angle: f64,
    /// Which way a turned heading leans. `Rise` by default — the
    /// spreadsheet's way, which hangs the ink to the RIGHT, where the room
    /// beyond the last column is the table's own.
    #[live(DiagonalLean::Rise)]
    pub header_lean: DiagonalLean,
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

    /// Scratch for one turned name's glyphs, kept so a row of them does not
    /// allocate once a frame each.
    #[rust]
    head_glyphs: Vec<PathGlyphInstance>,
    /// Which headings the last draw turned, one per column. Decided every
    /// pass from the widths the columns came out at, so a column that grows
    /// wide enough lays its name flat again without being told.
    #[rust]
    head_turned: Vec<bool>,
    /// Where each turned name's ink went, by column, from the last draw:
    /// what says a name stood over its own column and stayed inside the
    /// table, rather than what the arithmetic promised it would.
    #[rust]
    head_runs: Vec<(usize, DiagonalRun)>,

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

    /// Whether a heading MAY turn this pass. Which ones do is the columns'
    /// business: see `heading_fits_flat`.
    fn may_turn(&self) -> bool {
        self.show_header && self.header_angle > 0.0
    }

    /// The face the turned headings are drawn in: the flat ones' face,
    /// copied over every pass so the two can never drift apart.
    fn turn_heading_face(&mut self) {
        self.draw_heading_turned.text_style = self.draw_heading.text_style.clone();
        self.draw_heading_turned.color = self.draw_heading.color;
    }

    /// Every heading's width, measured exactly as `natural_widths` measures
    /// it, so the fit test and the width a sharing column asked for are the
    /// same number. The bare heading and not the sort mark: pressing a
    /// heading must not be what tips it onto the diagonal.
    fn heading_widths(&self, cx: &mut Cx2d) -> Vec<f64> {
        self.cols.iter().map(|col| measure(&self.draw_heading, cx, &col.heading)).collect()
    }

    /// How tall the heading band is for this set of turned headings: the
    /// stated height when none turned, else what the longest turned name
    /// needs. Only that one name is shaped for its line.
    fn band_height(
        &self,
        cx: &mut Cx2d,
        cols: &[TableColumn],
        names: &[f64],
        turned: &[bool],
        stated: f64,
    ) -> f64 {
        match longest_turned(names, turned) {
            None => stated,
            Some((index, width)) => {
                let name = cols.get(index).map_or("", |col| col.heading.as_str());
                let line = self.heading_line(cx, name);
                heading_band_height(stated, Some(width), line, self.header_angle)
            }
        }
    }

    /// The height of one line of `name` in the heading face: what a turned
    /// name stands across.
    fn heading_line(&self, cx: &mut Cx2d, name: &str) -> f64 {
        let size = self.draw_heading.text_style.font_size as f64;
        match self.draw_heading.prepare_single_line_run(cx, name) {
            Some(run) => ((run.ascender_in_lpxs - run.descender_in_lpxs) as f64).max(size),
            None => size,
        }
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
        // Every heading's width when a heading may turn, and nothing at all
        // when none may: without an angle not one more thing is measured, so
        // the flat table costs what it always did.
        let may_turn = self.may_turn();
        let names = if may_turn {
            self.turn_heading_face();
            self.heading_widths(cx)
        } else {
            Vec::new()
        };
        let stated_h = if self.show_header { self.header_height.max(0.0) } else { 0.0 };
        let content_h = body.len() as f64 * self.row_height;

        // A Fill inside a Fit resolves to nothing, so a Fit table has to
        // work out its own size before a single row is drawn — including
        // the band, which is what the turned names make it. It is worked
        // out here at the widths the table asks for; a table squeezed below
        // those turns more headings once it knows its room, and then the
        // body gives up the difference rather than the band.
        let asked_turned = if may_turn {
            turned_headings(&names, &natural, self.cell_pad)
        } else {
            vec![false; cols.len()]
        };
        let asked_reserve =
            ink_overreach(&names, &natural, &asked_turned, self.header_angle, self.header_lean);
        let asked_h = self.band_height(cx, &cols, &names, &asked_turned, stated_h);
        let natural_w = natural.iter().sum::<f64>() + frame * 2.0 + asked_reserve;
        let natural_h = asked_h + content_h + frame * 2.0;
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
        // The columns' real widths, which headings those leave unable to lie
        // flat, and the strip their ink needs beside the columns. With
        // nothing turned that is the whole inside, divided as it always was
        // and starting at its left edge; with something turned, the strip is
        // kept back on the side the names lean towards.
        let (widths, turned, reserve) = settle_columns(
            &cols,
            &natural,
            &names,
            inner.size.x,
            self.min_col_width,
            self.cell_pad,
            self.header_angle,
            self.header_lean,
        );
        let header_h = self.band_height(cx, &cols, &names, &turned, stated_h);
        self.head_turned.clone_from(&turned);
        self.head_runs.clear();
        let lean_left = self.header_lean == DiagonalLean::Fall;
        let band_x = inner.pos.x + if lean_left { reserve } else { 0.0 };

        // The heading band, which does not move: the body is clipped to its
        // own rect below it, so there is nothing to keep it above.
        self.heads.clear();
        if header_h > 0.0 {
            let band = Rect { pos: inner.pos, size: dvec2(inner.size.x, header_h) };
            self.draw_fill.color = self.color_header;
            self.draw_fill.draw_abs(cx, band);

            let mut x = band_x;
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
            let mut glyphs = std::mem::take(&mut self.head_glyphs);
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
                if turned.get(index).copied().unwrap_or(false) {
                    // Deliberately unclipped: a turned name is MEANT to
                    // cross its neighbours, and the only thing it may not
                    // cross is the table, which the strip kept back above
                    // makes sure of.
                    if let Some(run) = draw_diagonal_name(
                        &mut self.draw_heading_turned,
                        cx,
                        &mut glyphs,
                        rect,
                        &label,
                        self.header_angle,
                        self.header_lean,
                    ) {
                        self.head_runs.push((index, run));
                    }
                } else {
                    // A name that fits lies where it always did: in a slot
                    // of the stated height on the band's bottom line, so it
                    // reads along the same line the turned names stand on.
                    // With nothing turned the slot IS the heading.
                    let slot = Rect {
                        // Bracketed so that with nothing turned the offset
                        // is an exact nought and not a rounding of one.
                        pos: dvec2(rect.pos.x, rect.pos.y + (rect.size.y - stated_h)),
                        size: dvec2(rect.size.x, stated_h),
                    };
                    draw_run(&mut self.draw_heading, cx, slot, self.cell_pad, col.align, &label);
                }
            }
            self.head_glyphs = glyphs;
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

            let mut x = band_x;
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
            let mut x = band_x;
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
            // A wheel a cell's own scroll view already used is left alone. One
            // the rows move by is the table's, so the page around it stays put;
            // one pointing past the edge the rows rest on goes on to the page.
            Hit::FingerScroll(fe)
                if self.scroll_max > 0.0 && !event.scroll_handled(Vec2Index::Y) =>
            {
                let next = (self.scroll + fe.scroll.y).clamp(0.0, self.scroll_max);
                if next != self.scroll {
                    self.scroll = next;
                    event.set_scroll_handled(Vec2Index::Y);
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

/// The turned heading row, drawn. What the arithmetic says about where a
/// name goes is only worth something if that is where the table actually put
/// it — and the rule it rests on, that a heading turns only when its name
/// does not fit its column, has to hold in a real layout, where the widths
/// come out of the sharing sum and not out of a test's head.
#[cfg(test)]
mod diagonal_headers {
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;

    const SIZE: DVec2 = DVec2 { x: 900.0, y: 1600.0 };

    /// The tables in one pass. The wide ones are one table three times —
    /// flat, and with an angle in each lean — whose columns are all wide
    /// enough for their names. The narrow ones are what the option is for:
    /// numbers in 32-point columns under long names, beside a wide first
    /// column with a short one.
    fn scene(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    wide_off := Table{
                        width: 500. height: Fit
                        sortable: true
                        columns: ["First" "Born|90|end" "Last" "Town||center"]
                        rows: ["Anna|1961|Berg|Ely" "Tom|1974|Hale|Wells next the Sea"]
                    }
                    wide_on := Table{
                        width: 500. height: Fit
                        sortable: true
                        header_angle: 45.
                        columns: ["First" "Born|90|end" "Last" "Town||center"]
                        rows: ["Anna|1961|Berg|Ely" "Tom|1974|Hale|Wells next the Sea"]
                    }
                    wide_fall := Table{
                        width: 500. height: Fit
                        sortable: true
                        header_angle: 45.
                        header_lean: DiagonalLean.Fall
                        columns: ["First" "Born|90|end" "Last" "Town||center"]
                        rows: ["Anna|1961|Berg|Ely" "Tom|1974|Hale|Wells next the Sea"]
                    }
                    narrow_off := Table{
                        width: Fit height: Fit
                        columns: ["Site" "Readings taken|32|end" "Average reading|32|end" "Days without a reading|32|end"]
                        rows: ["North gate|12|11|2" "South gate|9|8|4"]
                    }
                    narrow := Table{
                        width: Fit height: Fit
                        sortable: true
                        header_angle: 45.
                        columns: ["Site" "Readings taken|32|end" "Average reading|32|end" "Days without a reading|32|end"]
                        rows: ["North gate|12|11|2" "South gate|9|8|4"]
                    }
                    narrow_fall := Table{
                        width: Fit height: Fit
                        sortable: true
                        header_angle: 45.
                        header_lean: DiagonalLean.Fall
                        columns: ["Site" "Readings taken|32|end" "Average reading|32|end" "Days without a reading|32|end"]
                        rows: ["North gate|12|11|2" "South gate|9|8|4"]
                    }
                    short_turned := Table{
                        width: Fit height: Fit
                        header_angle: 45.
                        columns: ["Site" "Readings taken|32|end"]
                        rows: ["North gate|12"]
                    }
                    long_flat := Table{
                        width: Fit height: Fit
                        header_angle: 45.
                        columns: ["The site the readings were taken at" "Readings taken|32|end"]
                        rows: ["North gate|12"]
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// One whole pass over `root`, the way a frame draws it.
    fn draw(cx: &mut Cx, root: &WidgetRef) {
        let pass = DrawPass::new(cx);
        let mut draw_list = DrawList2d::new(cx);
        pass.set_size(cx, SIZE);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        draw_list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(SIZE, Layout::flow_down());
        root.draw_all(&mut cx2d, &mut Scope::empty());
        cx2d.end_pass_sized_turtle();
        draw_list.end(&mut cx2d);
        cx2d.end_pass(&pass);
    }

    fn start(cx: &mut Cx) -> WidgetRef {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = scene(cx);
        draw(cx, &root);
        root
    }

    /// What one table in the scene drew: its heading rects, the box it drew
    /// in, the body under the band, which headings turned and where their
    /// ink went.
    struct Drawn {
        heads: Vec<Rect>,
        area: Rect,
        body: Rect,
        turned: Vec<bool>,
        runs: Vec<(usize, DiagonalRun)>,
    }

    fn drawn(cx: &Cx, root: &WidgetRef, id: &[LiveId]) -> Drawn {
        let widget = root.widget(cx, id);
        let table = widget.borrow::<Table>().expect("it is a Table");
        Drawn {
            heads: table.heads.clone(),
            area: table.area.rect(cx),
            body: table.body_rect,
            turned: table.head_turned.clone(),
            runs: table.head_runs.clone(),
        }
    }

    /// A rect moved so its table's box starts at the origin: the tables
    /// stand one under another, and only where things are INSIDE a table
    /// can be compared between two of them.
    fn local(rect: Rect, area: Rect) -> Rect {
        Rect { pos: rect.pos - area.pos, size: rect.size }
    }

    /// Two rects the same to a billionth of a point: the tables stand at
    /// different heights, and taking one origin off leaves a rounding that
    /// is not a difference in the layout.
    fn same(a: Rect, b: Rect) -> bool {
        (a.pos - b.pos).length() < 1e-9 && (a.size - b.size).length() < 1e-9
    }

    fn band_height(heads: &[Rect]) -> f64 {
        heads.first().expect("a heading row").size.y
    }

    /// The operator's rule, whole: a table whose columns all hold their
    /// names flat looks exactly the same with an angle set as without one —
    /// the same box, the same band, the same headings, the same body — in
    /// either lean. Nothing turns, so nothing moves.
    #[test]
    fn wide_columns_with_an_angle_draw_exactly_what_they_drew_without_one() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let root = start(&mut cx);
        let off = drawn(&cx, &root, ids!(wide_off));
        for id in [ids!(wide_on), ids!(wide_fall)] {
            let on = drawn(&cx, &root, id);
            assert_eq!(on.turned, vec![false; 4], "no heading here needs turning");
            assert!(on.runs.is_empty(), "and none was drawn turned");
            assert_eq!(on.area.size, off.area.size, "the same box");
            assert!(
                same(local(on.body, on.area), local(off.body, off.area)),
                "the same body: {:?} against {:?}",
                local(on.body, on.area),
                local(off.body, off.area)
            );
            assert_eq!(on.heads.len(), off.heads.len());
            for (a, b) in on.heads.iter().zip(&off.heads) {
                let (a, b) = (local(*a, on.area), local(*b, off.area));
                assert!(same(a, b), "the same heading: {a:?} against {b:?}");
            }
        }
    }

    /// Only the names too long for their columns turn. The wide first column
    /// under a short name keeps it flat beside them, and the narrow columns
    /// keep the width they were given — a turned name does not widen its
    /// column, which is the reason for turning it.
    #[test]
    fn only_a_name_too_long_for_its_column_turns() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let root = start(&mut cx);
        for id in [ids!(narrow), ids!(narrow_fall)] {
            let table = drawn(&cx, &root, id);
            assert_eq!(table.turned, vec![false, true, true, true]);
            assert_eq!(
                table.runs.iter().map(|(col, _)| *col).collect::<Vec<_>>(),
                vec![1, 2, 3],
                "the turned ones, and only those, were drawn turned"
            );
            for head in &table.heads[1..] {
                assert!((head.size.x - 32.0).abs() < 1e-9, "a narrow column stays narrow");
            }
        }
    }

    /// The band is as tall as the longest TURNED name needs. A long name
    /// that fits its wide column flat does not make it any taller, and
    /// with something turned it is taller than the flat band it replaces.
    #[test]
    fn the_band_is_as_tall_as_the_longest_turned_name() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let root = start(&mut cx);
        let flat = drawn(&cx, &root, ids!(narrow_off));
        let turned = drawn(&cx, &root, ids!(narrow));
        assert!((band_height(&flat.heads) - 28.0).abs() < 1e-9, "the stated height");
        assert!(band_height(&turned.heads) > 28.0, "turned names need more room");
        let short = drawn(&cx, &root, ids!(short_turned));
        let long = drawn(&cx, &root, ids!(long_flat));
        assert_eq!(long.turned, vec![false, true], "the long name fits its own column");
        assert_eq!(
            band_height(&long.heads),
            band_height(&short.heads),
            "a flat name, however long, does not raise the band"
        );
        assert!(
            band_height(&turned.heads) > band_height(&short.heads),
            "and a longer turned name raises it more"
        );
        for head in &turned.heads {
            assert_eq!(head.pos.y, turned.heads[0].pos.y, "one band");
            assert_eq!(head.size.y, band_height(&turned.heads), "one bottom line");
        }
    }

    /// Each turned name stands on the middle of its own column's bottom
    /// edge, and all of its ink is inside the table: the strip kept back for
    /// it is on the side it leans over.
    #[test]
    fn each_turned_name_stands_over_its_own_column_and_inside_the_table() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let root = start(&mut cx);
        for (id, lean) in [(ids!(narrow), DiagonalLean::Rise), (ids!(narrow_fall), DiagonalLean::Fall)] {
            let table = drawn(&cx, &root, id);
            for (col, run) in &table.runs {
                let head = table.heads[*col];
                let foot = match lean {
                    DiagonalLean::Rise => run.start,
                    DiagonalLean::Fall => run.end,
                };
                assert!((foot.x - (head.pos.x + head.size.x * 0.5)).abs() < 1e-6, "column {col}");
                assert!((foot.y - (head.pos.y + head.size.y)).abs() < 1e-6, "column {col}");
                let ink = run.bounds;
                assert!(ink.pos.x >= table.area.pos.x - 1e-6, "{lean:?} {col} leaves on the left");
                assert!(
                    ink.pos.x + ink.size.x <= table.area.pos.x + table.area.size.x + 1e-6,
                    "{lean:?} {col} leaves on the right"
                );
                assert!(ink.pos.y >= head.pos.y - 1e-6, "{lean:?} {col} leaves the band");
            }
        }
    }

    /// A press on a turned heading lands on the column its name stands on:
    /// the heading rects tile the band edge to edge whatever is turned, and
    /// the hit walks those, not the ink.
    #[test]
    fn a_press_on_a_turned_heading_finds_its_own_column() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let root = start(&mut cx);
        for id in [ids!(narrow), ids!(narrow_fall)] {
            let widget = root.widget(&cx, id);
            let table = widget.borrow::<Table>().expect("it is a Table");
            for (index, head) in table.heads.iter().enumerate() {
                for at in [
                    dvec2(head.pos.x + head.size.x * 0.5, head.pos.y + 2.0),
                    dvec2(head.pos.x + head.size.x * 0.5, head.pos.y + head.size.y - 2.0),
                ] {
                    assert_eq!(table.head_at(at), Some(index), "column {index}");
                }
                assert!(table.can_sort(index), "column {index} sorts");
            }
            for pair in table.heads.windows(2) {
                assert!(
                    (pair[0].pos.x + pair[0].size.x - pair[1].pos.x).abs() < 1e-9,
                    "the headings tile the band"
                );
            }
        }
    }

    /// Sorting does not tip a heading onto the diagonal. The mark is drawn
    /// after the name, but the fit is decided on the name alone — a heading
    /// that jumped to 45 degrees because it was pressed would move the row
    /// it names.
    #[test]
    fn sorting_leaves_every_heading_where_it_was() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let root = start(&mut cx);
        let before = drawn(&cx, &root, ids!(narrow));
        let wide_before = drawn(&cx, &root, ids!(wide_on));
        for (id, col) in [(ids!(narrow), 0), (ids!(wide_on), 1)] {
            let widget = root.widget(&cx, id);
            widget
                .borrow_mut::<Table>()
                .expect("it is a Table")
                .set_sort_indicator(&mut cx, Some((col, true)));
        }
        draw(&mut cx, &root);
        let after = drawn(&cx, &root, ids!(narrow));
        let wide_after = drawn(&cx, &root, ids!(wide_on));
        assert_eq!(after.turned, before.turned);
        assert_eq!(after.heads, before.heads);
        assert_eq!(wide_after.turned, wide_before.turned);
        assert_eq!(wide_after.heads, wide_before.heads);
    }

    /// A column made wide enough for its name lays it flat again, and the
    /// others keep theirs turned; made narrow again, it turns again. The
    /// table has no grip to drag a column by, so this is the host handing
    /// it a new width — the same thing a resize is.
    #[test]
    fn a_column_made_wide_enough_lays_its_name_flat_again() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let root = start(&mut cx);
        let columns = |second: f64| {
            vec![
                TableColumn::new("Site"),
                TableColumn::fixed("Readings taken", second).with_align(TableAlign::End),
                TableColumn::fixed("Average reading", 32.0).with_align(TableAlign::End),
                TableColumn::fixed("Days without a reading", 32.0).with_align(TableAlign::End),
            ]
        };
        let widget = root.widget(&cx, ids!(narrow));
        widget.borrow_mut::<Table>().unwrap().set_columns(&mut cx, columns(200.0));
        draw(&mut cx, &root);
        let wide = drawn(&cx, &root, ids!(narrow));
        assert_eq!(wide.turned, vec![false, false, true, true], "wide enough: flat");
        assert!((wide.heads[1].size.x - 200.0).abs() < 1e-9);

        widget.borrow_mut::<Table>().unwrap().set_columns(&mut cx, columns(32.0));
        draw(&mut cx, &root);
        let narrow = drawn(&cx, &root, ids!(narrow));
        assert_eq!(narrow.turned, vec![false, true, true, true], "narrow again: turned");
    }
}

/// The rule without a window: which headings turn, the strip kept back for
/// their ink, and the band — all settled from numbers.
#[cfg(test)]
mod heading_fit {
    use super::*;

    fn cols(specs: &[&str]) -> Vec<TableColumn> {
        specs.iter().map(|spec| parse_column_spec(spec)).collect()
    }

    /// A column sized to its own heading holds it: the fit is measured the
    /// same way the column's width is, so the two cannot disagree by a hair.
    #[test]
    fn a_column_sized_to_its_heading_holds_it_flat() {
        assert!(heading_fits_flat(40.0, 60.0, 10.0));
        assert!(heading_fits_flat(40.0, 40.0 + 2.0 * 10.0, 10.0), "exactly at the width");
        assert!(!heading_fits_flat(40.0, 59.0, 10.0));
    }

    /// Nothing turned means nothing kept back and nothing moved: the widths
    /// are the ones the table always had, to the bit.
    #[test]
    fn with_nothing_to_turn_the_widths_are_the_old_ones() {
        let cols = cols(&["First", "Born|90|end", "Last"]);
        let natural = vec![60.0, 90.0, 70.0];
        let names = vec![30.0, 28.0, 26.0];
        let (widths, turned, reserve) =
            settle_columns(&cols, &natural, &names, 500.0, 40.0, 10.0, 45.0, DiagonalLean::Rise);
        assert_eq!(widths, column_widths(&cols, &natural, 500.0, 40.0));
        assert_eq!(turned, vec![false; 3]);
        assert_eq!(reserve, 0.0);
        assert_eq!(heading_band_height(28.0, longest_turned(&names, &turned).map(|(_, w)| w), 14.0, 45.0), 28.0);
    }

    /// The strip is on the side the names lean over and only as wide as the
    /// ink that leaves the columns: a name over the middle hangs over its
    /// neighbours, which needs nothing kept back.
    #[test]
    fn the_strip_is_what_leaves_the_columns() {
        let cols = cols(&["Site", "Readings taken|32|end", "Days without a reading|32|end"]);
        let natural = vec![90.0, 32.0, 32.0];
        let names = vec![30.0, 90.0, 140.0];
        let (widths, turned, reserve) =
            settle_columns(&cols, &natural, &names, 500.0, 40.0, 10.0, 45.0, DiagonalLean::Rise);
        assert_eq!(turned, vec![false, true, true]);
        assert_eq!(widths[1], 32.0);
        // The last name stands 16 in from the columns' end and reaches
        // 140 cos 45 past its foot.
        let expect = 140.0 * 45f64.to_radians().cos() - 16.0;
        assert!((reserve - expect).abs() < 1e-6, "{reserve} against {expect}");

        // Falling, the same names hang back over the wide first column, which
        // has the room for all of them: nothing is kept back at all.
        let (_, turned, reserve) =
            settle_columns(&cols, &natural, &names, 500.0, 40.0, 10.0, 45.0, DiagonalLean::Fall);
        assert_eq!(turned, vec![false, true, true]);
        assert_eq!(reserve, 0.0);
    }

    /// Squeezed below what its heading needs, a sharing column turns too:
    /// the rule is about the width a column came out at, not how it was
    /// asked for.
    #[test]
    fn a_squeezed_sharing_column_turns() {
        let cols = cols(&["Readings taken", "Average reading"]);
        let natural = vec![120.0, 130.0];
        let names = vec![100.0, 110.0];
        let (_, roomy, _) =
            settle_columns(&cols, &natural, &names, 400.0, 20.0, 10.0, 45.0, DiagonalLean::Rise);
        assert_eq!(roomy, vec![false, false]);
        let (_, squeezed, _) =
            settle_columns(&cols, &natural, &names, 120.0, 20.0, 10.0, 45.0, DiagonalLean::Rise);
        assert_eq!(squeezed, vec![true, true]);
    }
}
