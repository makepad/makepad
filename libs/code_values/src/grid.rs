//! A list's items as a grid of typed cells: a number list's "grid" tab
//! ([`Control::Data`]) and a table of records ([`Control::Table`]).
//!
//! - a click on a cell types into it: Enter commits, Tab moves on (shift
//!   back), the arrows move up and down, Escape leaves it as it was;
//! - a drag on a number scrubs it (shift for finer);
//! - each cell's write replaces only that cell's bytes.
//!
//! Only the rows in view are laid out (a list of 140,000 numbers is 17,500
//! rows; twelve are shown); the list itself is read once per change.

use crate::data::*;
use crate::*;

pub(crate) const ROW_H: f64 = 20.0;
const TOP_H: f64 = 28.0;
const HEAD_H: f64 = 18.0;
const FOOT_H: f64 = 18.0;
const LABEL_W: f64 = 44.0;
pub(crate) const MAX_ROWS: usize = 12;

/// A cell: its row and column.
pub(crate) type At = (usize, usize);

#[derive(Clone, Debug)]
pub(crate) struct CellEdit {
    pub at: At,
    pub text: String,
    /// The first key replaces what was written.
    pub fresh: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct CellPress {
    pub at: At,
    pub from: DVec2,
    pub base: f64,
    pub decimals: usize,
    pub moved: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct GridState {
    pub top: usize,
    pub hover: Option<At>,
    pub edit: Option<CellEdit>,
    pub press: Option<CellPress>,
    /// A number list's grid: whether its items pair up (its tabs show).
    pub plot: Option<bool>,
    pub col_w: f64,
    /// The typed text is not a value of its cell's kind.
    pub refused: bool,
}

impl GridState {
    pub fn new(plot: Option<bool>) -> Self {
        Self { top: 0, hover: None, edit: None, press: None, plot, col_w: 60.0, refused: false }
    }
}

/// The grid's cells in view, read from the source.
#[derive(Clone, Debug)]
pub(crate) struct GridModel {
    pub rows: usize,
    pub cols: usize,
    pub heads: Vec<String>,
    /// The cells of the rows in view (all of a table's).
    pub cells: Vec<Cell>,
    /// A number list: item `row * cols + col`; row labels count items.
    pub list: Option<usize>,
    /// Per column, its numbers' range (for the bars under them).
    pub ranges: Vec<(f64, f64)>,
    /// What a row's label counts up by.
    pub label_step: usize,
}

impl GridModel {
    pub fn cell(&self, at: At) -> Option<&Cell> {
        // Cells are in reading order.
        let i = self.cells.partition_point(|c| (c.row, c.col) < at);
        self.cells.get(i).filter(|c| (c.row, c.col) == at)
    }

    /// The cell after (or before) `at` that takes typing, in reading order.
    fn step(&self, table: Option<&Table>, at: At, back: bool) -> Option<At> {
        if let Some(n) = self.list {
            let i = at.0 * self.cols + at.1;
            let j = if back { (i + n - 1) % n } else { (i + 1) % n };
            return Some((j / self.cols, j % self.cols));
        }
        let cells = &table?.cells;
        let order: Vec<At> = cells.iter().filter(|c| c.value != CellValue::Other).map(|c| (c.row, c.col)).collect();
        let k = order.iter().position(|a| *a == at)?;
        let n = order.len();
        Some(order[if back { (k + n - 1) % n } else { (k + 1) % n }])
    }

    /// The cell above or below `at`.
    fn vertical(&self, table: Option<&Table>, at: At, up: bool) -> Option<At> {
        let row = if up { at.0.checked_sub(1)? } else { at.0 + 1 };
        if let Some(n) = self.list {
            return (row * self.cols + at.1 < n).then_some((row, at.1));
        }
        table?.cells.iter().any(|c| (c.row, c.col) == (row, at.1) && c.value != CellValue::Other).then_some((row, at.1))
    }
}

/// A cell's text as shown.
fn shown(source: &str, cell: &Cell) -> String {
    match &cell.value {
        CellValue::Str(s) => s.replace('\n', " ⏎ "),
        _ => source[cell.range.clone()].to_string(),
    }
}

/// The popup's size for a grid of `model`, its columns `col_w` wide.
pub(crate) fn grid_size(model: &GridModel, col_w: f64) -> DVec2 {
    let w = (LABEL_W + model.cols as f64 * col_w + 22.0).max(300.0);
    let h = TOP_H + HEAD_H + model.rows.clamp(1, MAX_ROWS) as f64 * ROW_H + 8.0 + FOOT_H;
    dvec2(w, h)
}

/// The rect of cell `at` (rows from `top`).
fn cell_rect(rect: Rect, st: &GridState, at: At) -> Rect {
    Rect {
        pos: dvec2(rect.pos.x + 8.0 + LABEL_W + at.1 as f64 * st.col_w, rect.pos.y + TOP_H + HEAD_H + (at.0 as f64 - st.top as f64) * ROW_H),
        size: dvec2(st.col_w - 2.0, ROW_H - 2.0),
    }
}

/// The "plot" and "grid" tabs of a number list's popup.
pub(crate) fn tabs(rect: Rect) -> [Rect; 2] {
    let r = |k: f64| Rect { pos: dvec2(rect.pos.x + 8.0 + k * 38.0, rect.pos.y + 5.0), size: dvec2(36.0, 16.0) };
    [r(0.0), r(1.0)]
}

/// The grid of the value `s` in `source`, the rows from `top` in view.
pub(crate) fn grid_model(caches: &mut Caches, source: &str, s: &Site, plot: Option<bool>, top: usize) -> Option<GridModel> {
    match &s.control {
        Control::Data { .. } => {
            let items = caches.list_items(source, s.range.start);
            let n = items.len();
            let pairs = plot.unwrap_or(false) && n % 2 == 0;
            let cols = if pairs { 2 } else { n.clamp(1, 8) };
            let rows = n.div_ceil(cols);
            let from = (top * cols).min(n);
            let to = ((top + MAX_ROWS) * cols).min(n);
            let cells: Vec<Cell> = (from..to).map(|k| Cell { range: items[k].0.clone(), row: k / cols, col: k % cols, value: CellValue::Num(items[k].1) }).collect();
            let heads = if pairs { vec!["x".to_string(), "y".to_string()] } else { (0..cols).map(|c| format!("+{c}")).collect() };
            // A list's columns are not fields: one range for them all (a
            // pair's x and y each their own).
            let mut ranges = vec![(f64::MAX, f64::MIN); cols];
            for (k, (_, v)) in items.iter().enumerate() {
                let r = &mut ranges[if pairs { k % 2 } else { 0 }];
                *r = (r.0.min(*v), r.1.max(*v));
            }
            if !pairs {
                let all = ranges[0];
                ranges.iter_mut().for_each(|r| *r = all);
            }
            Some(GridModel { rows, cols, heads, cells, list: Some(n), ranges, label_step: if pairs { 1 } else { cols } })
        }
        Control::Table { .. } => {
            let table = caches.table_at(source, s.range.start)?;
            let mut ranges = vec![(f64::MAX, f64::MIN); table.heads.len()];
            for c in &table.cells {
                if let CellValue::Num(v) = c.value {
                    let r = &mut ranges[c.col];
                    *r = (r.0.min(v), r.1.max(v));
                }
            }
            let a = table.cells.partition_point(|c| c.row < top);
            let b = table.cells.partition_point(|c| c.row < top + MAX_ROWS);
            Some(GridModel { rows: table.rows, cols: table.heads.len(), heads: table.heads.clone(), cells: table.cells[a..b].to_vec(), list: None, ranges, label_step: 1 })
        }
        _ => None,
    }
}

impl CodeValues {
    /// The grid of the value at `site`, the rows from `top` in view.
    pub(crate) fn grid_model(&mut self, site: usize, plot: Option<bool>, top: usize) -> Option<GridModel> {
        let s = self.sites.get(site)?;
        grid_model(&mut self.caches, &self.source, s, plot, top)
    }

    /// The columns' width for a grid: the widest text in view, within
    /// reason.
    pub(crate) fn grid_col_w(&self, cx: &mut Cx, model: &GridModel) -> f64 {
        let mut w: f64 = 30.0;
        for c in &model.cells {
            w = w.max(self.text_w(cx, &shown(&self.source, c), false).min(140.0) + if matches!(c.value, CellValue::Color(_)) { 16.0 } else { 0.0 });
        }
        for h in &model.heads {
            w = w.max(self.text_w(cx, h, true));
        }
        let fit = (620.0 - LABEL_W - 22.0) / model.cols.max(1) as f64;
        (w + 16.0).clamp(44.0, 150.0).min(fit.max(36.0))
    }

    /// The tabs over a number list's plot or grid.
    pub(crate) fn draw_tabs(&mut self, cx: &mut Cx2d, rect: Rect, grid: bool) {
        for (k, t) in tabs(rect).into_iter().enumerate() {
            let on = (k == 1) == grid;
            self.boxed(cx, t, if on { vec4(1.0, 0.48, 0.18, 0.18) } else { vec4(1.0, 1.0, 1.0, 0.04) }, if on { vec4(1.0, 0.48, 0.18, 0.8) } else { vec4(1.0, 1.0, 1.0, 0.14) }, 3.0);
            self.draw_glyph.color = if on { vec4(1.0, 0.7, 0.45, 1.0) } else { vec4(0.7, 0.72, 0.76, 1.0) };
            let label = if k == 0 { "plot" } else { "grid" };
            let w = self.text_w(cx, label, true);
            self.draw_glyph.draw_abs(cx, t.pos + dvec2((t.size.x - w) * 0.5, 3.0), label);
        }
    }

    pub(crate) fn draw_grid_popup(&mut self, cx: &mut Cx2d, site: usize, rect: Rect, st: &GridState) {
        self.boxed(cx, rect, vec4(0.08, 0.09, 0.11, 1.0), vec4(1.0, 1.0, 1.0, 0.16), 6.0);
        let Some(model) = self.grid_model(site, st.plot, st.top) else { return };
        let name = match self.sites.get(site).map(|s| &s.control) {
            Some(Control::Data { name, .. }) => name.clone(),
            _ => String::new(),
        };
        // The title: the tabs, the name, the size.
        let mut x = rect.pos.x + 10.0;
        if st.plot.is_some() {
            self.draw_tabs(cx, rect, true);
            x = tabs(rect)[1].pos.x + 46.0;
        }
        let size = match model.list {
            Some(n) => format!("{n} items"),
            None => format!("{} × {}", model.rows, model.cols),
        };
        if !name.is_empty() {
            self.draw_label.color = vec4(0.92, 0.93, 0.95, 1.0);
            self.draw_label.draw_abs(cx, dvec2(x, rect.pos.y + 7.0), &name);
            x += self.text_w(cx, &name, false) + 10.0;
        }
        self.draw_glyph.color = vec4(0.55, 0.58, 0.63, 1.0);
        self.draw_glyph.draw_abs(cx, dvec2(x, rect.pos.y + 8.0), &size);
        // The heads.
        for (c, h) in model.heads.iter().enumerate() {
            let r = cell_rect(rect, st, (st.top, c));
            self.draw_glyph.color = vec4(0.62, 0.66, 0.72, 1.0);
            let mut h = h.clone();
            if self.text_w(cx, &h, true) > r.size.x - 8.0 {
                h = h.chars().take(((r.size.x - 12.0) / 4.6).max(1.0) as usize).collect::<String>() + "…";
            }
            self.draw_glyph.draw_abs(cx, dvec2(r.pos.x + 6.0, rect.pos.y + TOP_H + 2.0), &h);
        }
        self.solid(cx, Rect { pos: dvec2(rect.pos.x + 8.0, rect.pos.y + TOP_H + HEAD_H - 3.0), size: dvec2(rect.size.x - 16.0, 1.0) }, vec4(1.0, 1.0, 1.0, 0.10));
        let bottom = st.top + MAX_ROWS;
        for row in st.top..model.rows.min(bottom) {
            let y = rect.pos.y + TOP_H + HEAD_H + (row - st.top) as f64 * ROW_H;
            if row % 2 == 1 {
                self.solid(cx, Rect { pos: dvec2(rect.pos.x + 8.0, y - 1.0), size: dvec2(rect.size.x - 16.0, ROW_H) }, vec4(1.0, 1.0, 1.0, 0.025));
            }
            self.draw_glyph.color = vec4(0.45, 0.48, 0.53, 1.0);
            let label = (row * model.label_step).to_string();
            self.draw_glyph.draw_abs(cx, dvec2(rect.pos.x + 10.0, y + 4.0), &label);
        }
        for cell in model.cells.iter() {
            let at = (cell.row, cell.col);
            let r = cell_rect(rect, st, at);
            let editing = st.edit.as_ref().filter(|e| e.at == at);
            let hot = st.hover == Some(at) || st.press.as_ref().is_some_and(|p| p.at == at);
            if let Some(edit) = editing {
                let rim = if st.refused { vec4(0.95, 0.3, 0.3, 1.0) } else { vec4(1.0, 0.48, 0.18, 1.0) };
                self.boxed(cx, r, vec4(0.03, 0.035, 0.045, 1.0), rim, 3.0);
                let w = self.text_w(cx, &edit.text, false);
                if edit.fresh && !edit.text.is_empty() {
                    self.solid(cx, Rect { pos: r.pos + dvec2(4.0, 3.0), size: dvec2(w + 4.0, r.size.y - 6.0) }, vec4(1.0, 0.48, 0.18, 0.28));
                }
                self.draw_label.color = vec4(0.95, 0.96, 0.98, 1.0);
                self.draw_label.draw_abs(cx, r.pos + dvec2(6.0, 4.0), &edit.text);
                self.solid(cx, Rect { pos: r.pos + dvec2(6.0 + w + 1.0, 3.0), size: dvec2(1.2, r.size.y - 6.0) }, vec4(1.0, 0.62, 0.32, 1.0));
                continue;
            }
            if hot {
                self.boxed(cx, r, vec4(1.0, 1.0, 1.0, 0.06), vec4(1.0, 0.48, 0.18, 0.55), 3.0);
            }
            let text = shown(&self.source, cell);
            let (ink, dx) = match &cell.value {
                CellValue::Num(v) => {
                    // A thin bar under it: where it sits in its column.
                    let (lo, hi) = model.ranges[cell.col];
                    if hi > lo {
                        let f = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
                        self.solid(cx, Rect { pos: dvec2(r.pos.x + 4.0, r.pos.y + r.size.y - 2.0), size: dvec2(((r.size.x - 8.0) * f).max(1.0), 1.5) }, vec4(0.31, 0.79, 0.69, 0.45));
                    }
                    (if hot { vec4(1.0, 0.65, 0.38, 1.0) } else { vec4(0.88, 0.9, 0.93, 1.0) }, 0.0)
                }
                CellValue::Str(_) => (vec4(0.86, 0.86, 0.67, 1.0), 0.0),
                CellValue::Color(rgba) => {
                    self.draw_ctl.shape = 2.0;
                    self.draw_ctl.fill = vec4(rgba[0], rgba[1], rgba[2], rgba[3]);
                    self.draw_ctl.edge = vec4(1.0, 1.0, 1.0, 0.25);
                    self.draw_ctl.stroke = 1.0;
                    self.draw_ctl.radius = 2.0;
                    self.draw_ctl.draw_abs(cx, Rect { pos: r.pos + dvec2(5.0, 4.0), size: dvec2(10.0, 10.0) });
                    (vec4(0.88, 0.9, 0.93, 1.0), 14.0)
                }
                CellValue::Other => (vec4(0.5, 0.53, 0.58, 1.0), 0.0),
            };
            let mut t = text;
            let mut tw = self.text_w(cx, &t, false);
            if tw > r.size.x - 10.0 - dx {
                let keep = ((r.size.x - 16.0 - dx) / (tw / t.chars().count().max(1) as f64)).max(1.0) as usize;
                t = t.chars().take(keep).collect::<String>() + "…";
                tw = self.text_w(cx, &t, false);
            }
            let x = if matches!(cell.value, CellValue::Num(_)) { r.pos.x + r.size.x - 6.0 - tw } else { r.pos.x + 6.0 + dx };
            self.draw_label.color = ink;
            self.draw_label.draw_abs(cx, dvec2(x.max(r.pos.x + 4.0 + dx), r.pos.y + 4.0), &t);
        }
        if model.rows > MAX_ROWS {
            // Where the shown rows are among all of them.
            let track = Rect { pos: dvec2(rect.pos.x + rect.size.x - 7.0, rect.pos.y + TOP_H + HEAD_H), size: dvec2(2.0, MAX_ROWS as f64 * ROW_H) };
            self.solid(cx, track, vec4(1.0, 1.0, 1.0, 0.06));
            let f0 = st.top as f64 / model.rows as f64;
            let f1 = ((st.top + MAX_ROWS) as f64 / model.rows as f64).min(1.0);
            self.solid(cx, Rect { pos: dvec2(track.pos.x, track.pos.y + f0 * track.size.y), size: dvec2(2.0, ((f1 - f0) * track.size.y).max(3.0)) }, vec4(1.0, 1.0, 1.0, 0.3));
        }
        let hint = if st.edit.is_some() { "Enter commits · Tab next · ↑↓ rows · Esc leaves it" } else { "click a cell to type · drag a number to scrub · scroll" };
        self.draw_glyph.color = vec4(0.5, 0.53, 0.58, 1.0);
        self.draw_glyph.draw_abs(cx, dvec2(rect.pos.x + 10.0, rect.pos.y + rect.size.y - FOOT_H + 3.0), hint);
    }

    /// The cell under `abs`.
    fn grid_cell_at(&self, rect: Rect, st: &GridState, model: &GridModel, abs: DVec2) -> Option<At> {
        let x = abs.x - (rect.pos.x + 8.0 + LABEL_W);
        let y = abs.y - (rect.pos.y + TOP_H + HEAD_H);
        if x < 0.0 || y < 0.0 || !rect.contains(abs) {
            return None;
        }
        let at = ((y / ROW_H) as usize + st.top, (x / st.col_w) as usize);
        if at.1 >= model.cols || at.0 >= st.top + MAX_ROWS {
            return None;
        }
        model.cell(at).map(|_| at)
    }

    /// Commit what is typed: `false` when it is not a value of the cell's
    /// kind (the cell stays open).
    fn grid_commit(&mut self, out: &mut CodeValuesResponse, model: &GridModel, edit: &CellEdit) -> bool {
        let Some(cell) = model.cell(edit.at).cloned() else { return true };
        let written = self.source[cell.range.clone()].to_string();
        let text = match &cell.value {
            CellValue::Num(_) => match typed_number(&edit.text) {
                Some(t) => t,
                None => return false,
            },
            CellValue::Str(_) => quote(&edit.text),
            CellValue::Color(_) => {
                let t = edit.text.trim();
                let t = if t.starts_with('#') { t.to_string() } else { format!("#{t}") };
                if parse_hex(&t).is_none() {
                    return false;
                }
                t
            }
            CellValue::Other => return true,
        };
        if text != written {
            let id = self.next_gesture();
            self.gesture = Some(Gesture { id, span: cell.range.clone(), last: written.clone(), original: written, scrub: None });
            out.focus = Some(cell.range.start);
            self.write(out, text, true);
        }
        true
    }

    /// Start typing into cell `at`: its text as written (words unquoted).
    fn grid_begin(&self, model: &GridModel, at: At) -> Option<CellEdit> {
        let cell = model.cell(at)?;
        let text = match &cell.value {
            CellValue::Str(s) => s.clone(),
            CellValue::Other => return None,
            _ => self.source[cell.range.clone()].to_string(),
        };
        Some(CellEdit { at, text, fresh: true })
    }

    pub(crate) fn grid_event(&mut self, cx: &mut Cx, event: &Event, site: usize, rect: Rect, mut st: GridState) -> CodeValuesResponse {
        let mut r = CodeValuesResponse::default();
        let Some(model) = self.grid_model(site, st.plot, st.top) else {
            self.popup = None;
            return r;
        };
        let set = |this: &mut Self, st: GridState| this.popup = Some(Popup::Grid { site, rect, grid: st });
        match event {
            Event::MouseDown(e) if !rect.contains(e.abs) => {
                if let Some(edit) = st.edit.take() {
                    self.grid_commit(&mut r, &model, &edit);
                }
                self.popup = None;
                self.gesture = None;
                r.consumed = true;
                r.redraw = true;
            }
            Event::MouseDown(e) => {
                r.consumed = true;
                r.redraw = true;
                if let Some(pairs) = st.plot {
                    if tabs(rect)[0].contains(e.abs) {
                        if let Some(edit) = st.edit.take() {
                            self.grid_commit(&mut r, &model, &edit);
                        }
                        let h = if pairs { PLOT_H * 1.6 } else { PLOT_H };
                        self.popup = Some(Popup::Plot { site, rect: Rect { pos: rect.pos, size: dvec2(PLOT_W, h) }, hover: None, drag: None, pairs });
                        self.pop_list.redraw(cx);
                        return r;
                    }
                }
                let at = self.grid_cell_at(rect, &st, &model, e.abs);
                if let Some(edit) = st.edit.clone() {
                    if at == Some(edit.at) {
                        return r;
                    }
                    if !self.grid_commit(&mut r, &model, &edit) {
                        st.refused = true;
                        set(self, st);
                        self.pop_list.redraw(cx);
                        return r;
                    }
                    st.edit = None;
                    st.refused = false;
                }
                // The commit may have moved the cells: read them again.
                let Some(model) = self.grid_model(site, st.plot, st.top) else { return r };
                if let Some(cell) = at.and_then(|at| model.cell(at)) {
                    let at = (cell.row, cell.col);
                    match &cell.value {
                        CellValue::Num(v) => {
                            let written = &self.source[cell.range.clone()];
                            let decimals = written.split('.').nth(1).map_or(0, |d| d.bytes().take_while(u8::is_ascii_digit).count());
                            st.press = Some(CellPress { at, from: e.abs, base: *v, decimals, moved: false });
                        }
                        CellValue::Other => {}
                        _ => st.edit = self.grid_begin(&model, at),
                    }
                }
                set(self, st);
            }
            Event::MouseMove(e) => {
                r.consumed = true;
                if let Some(mut p) = st.press.clone() {
                    let travel = (e.abs.x - p.from.x) + (p.from.y - e.abs.y);
                    if !p.moved && travel.abs() > 3.0 {
                        if let Some(cell) = model.cell(p.at) {
                            p.moved = true;
                            let written = self.source[cell.range.clone()].to_string();
                            let id = self.next_gesture();
                            r.focus = Some(cell.range.start);
                            self.gesture = Some(Gesture { id, span: cell.range.clone(), last: written.clone(), original: written, scrub: None });
                        }
                    }
                    if p.moved {
                        let steps = travel / 2.0 * if e.modifiers.shift { 0.1 } else { 1.0 };
                        self.write(&mut r, scrubbed(p.base, p.decimals, steps), false);
                        cx.set_cursor(MouseCursor::EwResize);
                    }
                    st.press = Some(p);
                    set(self, st);
                    r.redraw = true;
                } else {
                    let over = self.grid_cell_at(rect, &st, &model, e.abs).filter(|at| model.cell(*at).is_some_and(|c| c.value != CellValue::Other));
                    if over != st.hover {
                        st.hover = over;
                        set(self, st);
                        r.redraw = true;
                    }
                    if let Some(c) = over.and_then(|at| model.cell(at)) {
                        cx.set_cursor(if matches!(c.value, CellValue::Num(_)) { MouseCursor::EwResize } else { MouseCursor::Text });
                    }
                }
            }
            Event::MouseUp(e) => {
                r.consumed = true;
                if let Some(p) = st.press.take() {
                    if p.moved {
                        let travel = (e.abs.x - p.from.x) + (p.from.y - e.abs.y);
                        let steps = travel / 2.0 * if e.modifiers.shift { 0.1 } else { 1.0 };
                        self.write(&mut r, scrubbed(p.base, p.decimals, steps), true);
                        self.gesture = None;
                    } else {
                        // A click: type into it.
                        st.edit = self.grid_begin(&model, p.at);
                    }
                    set(self, st);
                    r.redraw = true;
                }
            }
            Event::Scroll(e) if rect.contains(e.abs) => {
                let max_top = model.rows.saturating_sub(MAX_ROWS);
                let rows = ((e.scroll.y.abs() / ROW_H).round() as usize).max(1);
                st.top = if e.scroll.y > 0.0 { (st.top + rows).min(max_top) } else if e.scroll.y < 0.0 { st.top.saturating_sub(rows) } else { st.top };
                st.hover = None;
                set(self, st);
                r.consumed = true;
                r.redraw = true;
            }
            Event::TextInput(t) => {
                r.consumed = true;
                if let Some(edit) = st.edit.as_mut() {
                    let typed: String = t.input.chars().filter(|c| *c != '\n' && *c != '\r' && *c != '\t').collect();
                    if edit.fresh {
                        edit.text = typed;
                    } else {
                        edit.text.push_str(&typed);
                    }
                    edit.fresh = false;
                    st.refused = false;
                    set(self, st);
                    r.redraw = true;
                }
            }
            Event::KeyDown(k) => {
                r.consumed = true;
                r.redraw = true;
                let Some(mut edit) = st.edit.clone() else {
                    if k.key_code == KeyCode::Escape && st.press.as_ref().is_some_and(|p| p.moved) {
                        // Mid-scrub: back to the number as it was, and closed.
                        self.revert(&mut r);
                    } else if matches!(k.key_code, KeyCode::Escape | KeyCode::ReturnKey) {
                        self.popup = None;
                        self.gesture = None;
                    }
                    self.pop_list.redraw(cx);
                    return r;
                };
                match k.key_code {
                    KeyCode::Backspace => {
                        if edit.fresh {
                            edit.text.clear();
                        } else {
                            edit.text.pop();
                        }
                        edit.fresh = false;
                        st.refused = false;
                        st.edit = Some(edit);
                    }
                    KeyCode::Escape => {
                        st.edit = None;
                        st.refused = false;
                    }
                    KeyCode::ReturnKey | KeyCode::Tab | KeyCode::ArrowUp | KeyCode::ArrowDown => {
                        if !self.grid_commit(&mut r, &model, &edit) {
                            st.refused = true;
                            set(self, st);
                            self.pop_list.redraw(cx);
                            return r;
                        }
                        st.refused = false;
                        st.edit = None;
                        let table = match (model.list, self.sites.get(site)) {
                            (None, Some(s)) => self.caches.table_at(&self.source, s.range.start),
                            _ => None,
                        };
                        let next = match k.key_code {
                            KeyCode::Tab => model.step(table.as_deref(), edit.at, k.modifiers.shift),
                            KeyCode::ArrowUp => model.vertical(table.as_deref(), edit.at, true),
                            KeyCode::ArrowDown => model.vertical(table.as_deref(), edit.at, false),
                            _ => None,
                        };
                        if let Some(at) = next {
                            // Bring it into view, then type into it.
                            if at.0 < st.top {
                                st.top = at.0;
                            } else if at.0 >= st.top + MAX_ROWS {
                                st.top = at.0 + 1 - MAX_ROWS;
                            }
                            if let Some(m) = self.grid_model(site, st.plot, st.top) {
                                st.edit = self.grid_begin(&m, at);
                            }
                        }
                    }
                    _ => {}
                }
                set(self, st);
            }
            Event::KeyUp(_) => r.consumed = true,
            _ => {}
        }
        if r.redraw {
            self.pop_list.redraw(cx);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(src: &str, control: Control, plot: Option<bool>, top: usize) -> GridModel {
        // The model as a popup reads it, without a window.
        let site = Site { range: 0..src.len(), control, limits: Default::default() };
        grid_model(&mut Caches::default(), src, &site, plot, top).unwrap()
    }

    #[test]
    fn number_lists_lay_out_rows_in_view() {
        let src = format!("[{}]", (0..100).map(|k| k.to_string()).collect::<Vec<_>>().join(", "));
        let data = Control::Data { values: vec![], pairs: false, name: String::new(), axis: DataAxis::Index };
        let m = model(&src, data.clone(), Some(false), 0);
        assert_eq!((m.rows, m.cols, m.list), (13, 8, Some(100)));
        // Only the rows in view are laid out.
        assert_eq!(m.cells.len(), MAX_ROWS * 8);
        let m = model(&src, data.clone(), Some(false), 10);
        assert_eq!(m.cells.len(), 20);
        assert_eq!(&src[m.cell((12, 3)).unwrap().range.clone()], "99");
        // Tab runs on through the list and wraps.
        assert_eq!(m.step(None, (12, 3), false), Some((0, 0)));
        assert_eq!(m.step(None, (0, 0), true), Some((12, 3)));
        assert_eq!(m.vertical(None, (11, 5), false), None);
        let m = model(&src, data, Some(true), 0);
        assert_eq!((m.rows, m.cols, m.heads.clone()), (50, 2, vec!["x".to_string(), "y".to_string()]));
    }

    #[test]
    fn tables_lay_out_with_their_heads_and_tab_order() {
        let src = "[{n: \"a\", v: 1}, {n: \"b\", v: 2, w: f(1)}]";
        let control = Control::Table { rows: 2, cols: 3, heads: vec![] };
        let m = model(src, control, None, 0);
        assert_eq!(m.heads, vec!["n", "v", "w"]);
        let table = table_of(&lit_at(src, 0).unwrap()).unwrap();
        // `f(1)` is shown, not typed into.
        assert_eq!(m.step(Some(&table), (1, 1), false), Some((0, 0)));
        assert_eq!(m.step(Some(&table), (0, 1), false), Some((1, 0)));
        assert_eq!(m.vertical(Some(&table), (0, 1), false), Some((1, 1)));
        assert_eq!(m.vertical(Some(&table), (0, 1), true), None);
    }
}
