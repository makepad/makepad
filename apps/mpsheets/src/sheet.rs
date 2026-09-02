//! The document model: cells, a dependency graph with topological
//! recalculation, per-cell formatting, undo/redo, and CSV.
//!
//! Nothing here knows about Makepad either — the whole workbook can be driven
//! and asserted from tests.

use crate::formula::{self, CellRef, ErrKind, Expr, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

pub type Pos = (usize, usize);

/// How many cells a single formula may register as precedents. A range larger
/// than this still evaluates, it just does not wire up auto-recalculation.
const DEP_CAP: usize = 20_000;

// ---------------------------------------------------------------------------
// formatting
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NumFormat {
    #[default]
    General,
    /// `0.00`
    Fixed2,
    /// `#,##0.00`
    Thousands,
    /// `0.0%`
    Percent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HAlign {
    /// Numbers right, text left — the spreadsheet default.
    #[default]
    Auto,
    Left,
    Center,
    Right,
}

impl HAlign {
    /// The 0.0/0.5/1.0 the grid's `CellStyle` wants.
    pub fn factor(self, is_num: bool) -> f64 {
        match self {
            HAlign::Auto => {
                if is_num {
                    1.0
                } else {
                    0.0
                }
            }
            HAlign::Left => 0.0,
            HAlign::Center => 0.5,
            HAlign::Right => 1.0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellFormat {
    pub bold: bool,
    pub italic: bool,
    pub align: HAlign,
    pub num: NumFormat,
}

impl CellFormat {
    pub fn is_default(&self) -> bool {
        *self == CellFormat::default()
    }
}

/// Group the integer part with thousands separators.
fn group_thousands(int_part: &str) -> String {
    let (sign, digits) = match int_part.strip_prefix('-') {
        Some(d) => ("-", d),
        None => ("", int_part),
    };
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{sign}{out}")
}

/// Render a value for the grid, applying the cell's number format. Non-numeric
/// values ignore the format entirely, exactly as in Excel.
pub fn display_value(v: &Value, fmt: NumFormat) -> String {
    let Value::Num(n) = v else {
        return v.to_text();
    };
    match fmt {
        NumFormat::General => formula::format_general(*n),
        NumFormat::Fixed2 => format!("{:.2}", n),
        NumFormat::Thousands => {
            let s = format!("{:.2}", n);
            let (i, f) = s.split_once('.').unwrap_or((s.as_str(), "00"));
            format!("{}.{}", group_thousands(i), f)
        }
        NumFormat::Percent => format!("{:.1}%", n * 100.0),
    }
}

// ---------------------------------------------------------------------------
// cells
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct Cell {
    /// Exactly what the user typed, `=` and all.
    pub input: String,
    /// Parsed formula, if the input started with `=`.
    pub ast: Option<Rc<Expr>>,
    /// For non-formula cells: the typed literal, already coerced.
    pub literal: Value,
    pub format: CellFormat,
}

impl Cell {
    fn set_input(&mut self, input: &str) {
        self.input = input.to_string();
        self.ast = None;
        self.literal = Value::Empty;
        if let Some(src) = input.strip_prefix('=') {
            match formula::parse(src) {
                Ok(e) => self.ast = Some(Rc::new(e)),
                // A formula that will not parse is stored as a parse error, so
                // the text stays editable but the cell shows #ERROR!.
                Err(_) => self.ast = Some(Rc::new(Expr::ErrLit(ErrKind::Parse))),
            }
            return;
        }
        self.literal = coerce_literal(input);
    }
}

/// Typed text becomes a number when it reads as one, a bool for TRUE/FALSE,
/// text otherwise. A trailing `%` makes a percentage.
fn coerce_literal(input: &str) -> Value {
    let t = input.trim();
    if t.is_empty() {
        return Value::Empty;
    }
    if let Ok(n) = t.parse::<f64>() {
        if n.is_finite() {
            return Value::Num(n);
        }
    }
    if let Some(p) = t.strip_suffix('%') {
        if let Ok(n) = p.trim().parse::<f64>() {
            if n.is_finite() {
                return Value::Num(n / 100.0);
            }
        }
    }
    match t.to_ascii_uppercase().as_str() {
        "TRUE" => Value::Bool(true),
        "FALSE" => Value::Bool(false),
        _ => Value::Text(input.to_string()),
    }
}

// ---------------------------------------------------------------------------
// sheet
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Sheet {
    pub name: String,
    cells: HashMap<Pos, Cell>,
    values: HashMap<Pos, Value>,
    /// cell -> the cells it reads
    precedents: HashMap<Pos, HashSet<Pos>>,
    /// cell -> the cells that read it
    dependents: HashMap<Pos, HashSet<Pos>>,
    /// display column -> width in points, when the user resized it
    pub col_widths: HashMap<usize, f64>,
}

impl Sheet {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// The raw text of a cell — what the formula bar shows.
    pub fn input(&self, pos: Pos) -> &str {
        self.cells.get(&pos).map(|c| c.input.as_str()).unwrap_or("")
    }

    pub fn value(&self, pos: Pos) -> Value {
        self.values.get(&pos).cloned().unwrap_or(Value::Empty)
    }

    pub fn format(&self, pos: Pos) -> CellFormat {
        self.cells
            .get(&pos)
            .map(|c| c.format.clone())
            .unwrap_or_default()
    }

    /// What the grid prints in this cell.
    pub fn display(&self, pos: Pos) -> String {
        let v = self.value(pos);
        if matches!(v, Value::Empty) {
            return String::new();
        }
        display_value(&v, self.format(pos).num)
    }

    /// Bounding box of every non-empty cell, or None when the sheet is blank.
    pub fn used_range(&self) -> Option<(Pos, Pos)> {
        let mut it = self
            .cells
            .iter()
            .filter(|(_, c)| !c.input.is_empty())
            .map(|(p, _)| *p);
        let first = it.next()?;
        let (mut r0, mut c0, mut r1, mut c1) = (first.0, first.1, first.0, first.1);
        for (r, c) in it {
            r0 = r0.min(r);
            c0 = c0.min(c);
            r1 = r1.max(r);
            c1 = c1.max(c);
        }
        Some(((r0, c0), (r1, c1)))
    }

    // -- mutation ---------------------------------------------------------

    /// Set a cell's text and rewire its dependencies. Does **not** recalculate;
    /// callers batch a set of writes and then call [`Sheet::recalc`].
    fn write(&mut self, pos: Pos, input: &str) {
        // drop the old edges
        if let Some(old) = self.precedents.remove(&pos) {
            for p in old {
                if let Some(set) = self.dependents.get_mut(&p) {
                    set.remove(&pos);
                    if set.is_empty() {
                        self.dependents.remove(&p);
                    }
                }
            }
        }
        if input.is_empty() {
            // Keep a cell that still carries formatting.
            match self.cells.get_mut(&pos) {
                Some(c) if !c.format.is_default() => {
                    c.set_input("");
                }
                _ => {
                    self.cells.remove(&pos);
                }
            }
            self.values.remove(&pos);
            return;
        }
        let cell = self.cells.entry(pos).or_default();
        cell.set_input(input);
        if let Some(ast) = cell.ast.clone() {
            let mut set = HashSet::new();
            ast.each_ref(DEP_CAP, &mut |r, c| {
                set.insert((r, c));
            });
            for p in &set {
                self.dependents.entry(*p).or_default().insert(pos);
            }
            self.precedents.insert(pos, set);
        }
    }

    fn write_format(&mut self, pos: Pos, f: impl FnOnce(&mut CellFormat)) {
        let cell = self.cells.entry(pos).or_default();
        f(&mut cell.format);
        if cell.input.is_empty() && cell.format.is_default() {
            self.cells.remove(&pos);
        }
    }

    /// Recompute `seeds` and everything downstream of them, in dependency
    /// order. Cells that take part in a cycle (or read one) get `#CIRC!`.
    pub fn recalc(&mut self, seeds: &[Pos]) {
        // 1. dirty set = seeds + transitive dependents
        let mut dirty: HashSet<Pos> = HashSet::new();
        let mut queue: VecDeque<Pos> = seeds.iter().copied().collect();
        while let Some(p) = queue.pop_front() {
            if !dirty.insert(p) {
                continue;
            }
            if let Some(deps) = self.dependents.get(&p) {
                for d in deps {
                    if !dirty.contains(d) {
                        queue.push_back(*d);
                    }
                }
            }
        }

        // 2. Kahn over the dirty subgraph: a cell is ready once every
        //    precedent that is itself dirty has been computed.
        let mut indegree: HashMap<Pos, usize> = HashMap::new();
        for p in &dirty {
            let n = self
                .precedents
                .get(p)
                .map(|s| s.iter().filter(|q| dirty.contains(q)).count())
                .unwrap_or(0);
            indegree.insert(*p, n);
        }
        let mut ready: VecDeque<Pos> = indegree
            .iter()
            .filter(|(_, n)| **n == 0)
            .map(|(p, _)| *p)
            .collect();
        let mut order: Vec<Pos> = Vec::with_capacity(dirty.len());
        while let Some(p) = ready.pop_front() {
            order.push(p);
            if let Some(deps) = self.dependents.get(&p) {
                for d in deps.clone() {
                    if let Some(n) = indegree.get_mut(&d) {
                        *n -= 1;
                        if *n == 0 {
                            ready.push_back(d);
                        }
                    }
                }
            }
        }

        // 3. evaluate in order
        for pos in &order {
            let v = self.eval_one(*pos);
            match v {
                Value::Empty => {
                    self.values.remove(pos);
                }
                v => {
                    self.values.insert(*pos, v);
                }
            }
        }

        // 4. anything left never became ready: it is in, or downstream of, a cycle
        for pos in dirty {
            if !order.contains(&pos) {
                self.values.insert(pos, Value::Err(ErrKind::Circ));
            }
        }
    }

    fn eval_one(&self, pos: Pos) -> Value {
        let Some(cell) = self.cells.get(&pos) else {
            return Value::Empty;
        };
        match &cell.ast {
            None => cell.literal.clone(),
            Some(ast) => {
                let values = &self.values;
                let mut src =
                    |r: usize, c: usize| values.get(&(r, c)).cloned().unwrap_or(Value::Empty);
                formula::eval(ast, &mut src)
            }
        }
    }

    /// Recompute the whole sheet from scratch (after a load).
    pub fn recalc_all(&mut self) {
        let all: Vec<Pos> = self.cells.keys().copied().collect();
        self.values.clear();
        self.recalc(&all);
    }

    // -- snapshots for undo ------------------------------------------------

    fn snapshot(&self, positions: &[Pos]) -> Vec<(Pos, Option<Cell>)> {
        positions
            .iter()
            .map(|p| (*p, self.cells.get(p).cloned()))
            .collect()
    }

    fn restore(&mut self, snap: &[(Pos, Option<Cell>)]) {
        let mut touched = Vec::with_capacity(snap.len());
        for (pos, cell) in snap {
            match cell {
                Some(c) => {
                    self.write(*pos, &c.input);
                    self.write_format(*pos, |f| *f = c.format.clone());
                }
                None => {
                    self.write(*pos, "");
                    self.cells.remove(pos);
                }
            }
            touched.push(*pos);
        }
        self.recalc(&touched);
    }

    // -- statistics --------------------------------------------------------

    /// SUM / COUNT / numeric-count over a set of cells, for the status bar.
    pub fn stats(&self, positions: impl Iterator<Item = Pos>) -> SelectionStats {
        let mut s = SelectionStats::default();
        for p in positions {
            let v = self.value(p);
            match v {
                Value::Empty => (),
                Value::Num(n) => {
                    s.count += 1;
                    s.numeric += 1;
                    s.sum += n;
                    s.min = Some(s.min.map_or(n, |m: f64| m.min(n)));
                    s.max = Some(s.max.map_or(n, |m: f64| m.max(n)));
                }
                _ => s.count += 1,
            }
        }
        s
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct SelectionStats {
    /// Non-empty cells.
    pub count: usize,
    /// Cells holding a number.
    pub numeric: usize,
    pub sum: f64,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl SelectionStats {
    pub fn average(&self) -> Option<f64> {
        (self.numeric > 0).then(|| self.sum / self.numeric as f64)
    }
}

// ---------------------------------------------------------------------------
// undo
// ---------------------------------------------------------------------------

struct UndoEntry {
    sheet: usize,
    before: Vec<(Pos, Option<Cell>)>,
    after: Vec<(Pos, Option<Cell>)>,
}

// ---------------------------------------------------------------------------
// workbook
// ---------------------------------------------------------------------------

pub struct Workbook {
    pub sheets: Vec<Sheet>,
    pub active: usize,
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
}

impl Default for Workbook {
    fn default() -> Self {
        Self {
            sheets: vec![Sheet::new("Sheet1")],
            active: 0,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

impl Workbook {
    /// A workbook holding the worked example, so a fresh window shows a real
    /// sheet rather than an empty void.
    pub fn with_demo() -> Self {
        Self {
            sheets: vec![demo_sheet()],
            ..Default::default()
        }
    }

    pub fn sheet(&self) -> &Sheet {
        &self.sheets[self.active]
    }

    pub fn sheet_mut(&mut self) -> &mut Sheet {
        let i = self.active;
        &mut self.sheets[i]
    }

    pub fn add_sheet(&mut self) -> usize {
        let mut n = self.sheets.len() + 1;
        while self.sheets.iter().any(|s| s.name == format!("Sheet{n}")) {
            n += 1;
        }
        self.sheets.push(Sheet::new(format!("Sheet{n}")));
        self.active = self.sheets.len() - 1;
        self.active
    }

    pub fn rename_sheet(&mut self, index: usize, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if let Some(s) = self.sheets.get_mut(index) {
            s.name = name.to_string();
        }
    }

    pub fn remove_sheet(&mut self, index: usize) {
        if self.sheets.len() <= 1 || index >= self.sheets.len() {
            return;
        }
        self.sheets.remove(index);
        self.active = self.active.min(self.sheets.len() - 1);
        // Undo history refers to sheet indices, which just shifted.
        self.undo.clear();
        self.redo.clear();
    }

    pub fn open_loaded_sheet(&mut self, sheet: Sheet) {
        self.sheets.push(sheet);
        self.active = self.sheets.len() - 1;
        self.undo.clear();
        self.redo.clear();
    }

    /// The one funnel every cell mutation goes through: snapshot, apply,
    /// recalculate, record undo.
    fn transact(&mut self, positions: &[Pos], mut f: impl FnMut(&mut Sheet, Pos)) {
        if positions.is_empty() {
            return;
        }
        let sheet_idx = self.active;
        let before = self.sheet().snapshot(positions);
        {
            let sheet = self.sheet_mut();
            for p in positions {
                f(sheet, *p);
            }
            sheet.recalc(positions);
        }
        let after = self.sheet().snapshot(positions);
        if before
            .iter()
            .zip(after.iter())
            .all(|(b, a)| cells_equal(&b.1, &a.1))
        {
            return; // nothing actually changed; do not pollute the undo stack
        }
        self.undo.push(UndoEntry {
            sheet: sheet_idx,
            before,
            after,
        });
        self.redo.clear();
        if self.undo.len() > 500 {
            self.undo.remove(0);
        }
    }

    pub fn set_input(&mut self, pos: Pos, input: &str) {
        let input = input.to_string();
        self.transact(&[pos], move |s, p| s.write(p, &input));
    }

    pub fn clear_cells(&mut self, positions: &[Pos]) {
        self.transact(positions, |s, p| s.write(p, ""));
    }

    pub fn set_format(&mut self, positions: &[Pos], f: impl Fn(&mut CellFormat) + Copy) {
        self.transact(positions, move |s, p| s.write_format(p, f));
    }

    /// Write a rectangular block of raw text with its top-left at `origin`.
    pub fn paste_block(&mut self, origin: Pos, rows: &[Vec<String>]) {
        let mut positions = Vec::new();
        let mut texts = HashMap::new();
        for (dr, row) in rows.iter().enumerate() {
            for (dc, text) in row.iter().enumerate() {
                let p = (origin.0 + dr, origin.1 + dc);
                positions.push(p);
                texts.insert(p, text.clone());
            }
        }
        self.transact(&positions, move |s, p| {
            s.write(p, texts.get(&p).map(|t| t.as_str()).unwrap_or(""))
        });
    }

    /// Fill `dest` from the block `src`, translating relative references —
    /// the fill-handle rule. `src` and `dest` are inclusive rectangles.
    pub fn fill(&mut self, src: (Pos, Pos), dest: (Pos, Pos)) {
        let (s0, s1) = src;
        let (d0, d1) = dest;
        let src_rows = s1.0 - s0.0 + 1;
        let src_cols = s1.1 - s0.1 + 1;

        // Read the source block once, before anything is written.
        let mut template: Vec<(String, CellFormat)> = Vec::new();
        for r in s0.0..=s1.0 {
            for c in s0.1..=s1.1 {
                let sheet = self.sheet();
                template.push((sheet.input((r, c)).to_string(), sheet.format((r, c))));
            }
        }

        let mut positions = Vec::new();
        let mut plan: HashMap<Pos, (String, CellFormat)> = HashMap::new();
        for r in d0.0..=d1.0 {
            for c in d0.1..=d1.1 {
                // Cells inside the source block keep what they have.
                if r >= s0.0 && r <= s1.0 && c >= s0.1 && c <= s1.1 {
                    continue;
                }
                // Tile the source block over the destination. Signed maths so
                // that dragging the handle up or left works as well as down
                // or right.
                let sr = (r as isize - s0.0 as isize).rem_euclid(src_rows as isize) as usize;
                let sc = (c as isize - s0.1 as isize).rem_euclid(src_cols as isize) as usize;
                let (input, fmt) = &template[sr * src_cols + sc];
                let drow = r as isize - (s0.0 + sr) as isize;
                let dcol = c as isize - (s0.1 + sc) as isize;
                let new_input = translate_input(input, drow, dcol);
                positions.push((r, c));
                plan.insert((r, c), (new_input, fmt.clone()));
            }
        }
        self.transact(&positions, move |s, p| {
            if let Some((input, fmt)) = plan.get(&p) {
                s.write(p, input);
                s.write_format(p, |f| *f = fmt.clone());
            }
        });
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.undo.pop() else {
            return false;
        };
        self.active = entry.sheet.min(self.sheets.len() - 1);
        self.sheets[self.active].restore(&entry.before);
        self.redo.push(entry);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.redo.pop() else {
            return false;
        };
        self.active = entry.sheet.min(self.sheets.len() - 1);
        self.sheets[self.active].restore(&entry.after);
        self.undo.push(entry);
        true
    }
}

fn cells_equal(a: &Option<Cell>, b: &Option<Cell>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.input == y.input && x.format == y.format,
        _ => false,
    }
}

/// Shift the relative references in a cell's raw text. Non-formula text is
/// returned unchanged.
pub fn translate_input(input: &str, drow: isize, dcol: isize) -> String {
    let Some(src) = input.strip_prefix('=') else {
        return input.to_string();
    };
    match formula::parse(src) {
        Ok(e) => format!("={}", e.translate(drow, dcol).to_formula()),
        Err(_) => input.to_string(),
    }
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

/// Export the used range. Cells export as what they *show*, which is what
/// every other program means by CSV.
pub fn to_csv(sheet: &Sheet) -> String {
    let Some(((r0, c0), (r1, c1))) = sheet.used_range() else {
        return String::new();
    };
    let mut out = String::new();
    for r in r0..=r1 {
        for c in c0..=c1 {
            if c > c0 {
                out.push(',');
            }
            let text = sheet.display((r, c));
            if text.contains([',', '"', '\n', '\r']) {
                out.push('"');
                out.push_str(&text.replace('"', "\"\""));
                out.push('"');
            } else {
                out.push_str(&text);
            }
        }
        out.push('\n');
    }
    out
}

/// Parse CSV into rows of fields (RFC 4180 quoting).
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => in_quotes = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => (),
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    if !any {
        return Vec::new();
    }
    rows
}

/// Build a sheet from CSV text. Fields starting with `=` become live formulas.
pub fn sheet_from_csv(name: &str, text: &str) -> Sheet {
    let mut sheet = Sheet::new(name);
    for (r, row) in parse_csv(text).iter().enumerate() {
        for (c, field) in row.iter().enumerate() {
            if !field.is_empty() {
                sheet.write((r, c), field);
            }
        }
    }
    sheet.recalc_all();
    sheet
}

/// Tab-separated text for the clipboard.
pub fn to_tsv(sheet: &Sheet, from: Pos, to: Pos) -> String {
    let mut out = String::new();
    for r in from.0..=to.0 {
        for c in from.1..=to.1 {
            if c > from.1 {
                out.push('\t');
            }
            out.push_str(&sheet.display((r, c)));
        }
        out.push('\n');
    }
    out
}

/// Split clipboard text into a block. Tabs separate columns; a single cell
/// with no tabs or newlines stays one cell.
pub fn parse_tsv(text: &str) -> Vec<Vec<String>> {
    text.replace("\r\n", "\n")
        .trim_end_matches('\n')
        .split('\n')
        .map(|line| line.split('\t').map(|s| s.to_string()).collect())
        .collect()
}

/// A small worked example so a fresh window is not an empty void.
pub fn demo_sheet() -> Sheet {
    let mut s = Sheet::new("Budget");
    let mut set = |cell: &str, v: &str| {
        let r = formula::parse_a1(cell).unwrap();
        s.write((r.row, r.col), v);
    };
    set("A1", "Quarterly Budget");
    set("A3", "Month");
    set("B3", "Revenue");
    set("C3", "Costs");
    set("D3", "Profit");
    set("E3", "Margin");
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun"];
    let revenue = [120500, 132800, 128400, 145200, 158900, 171300];
    let costs = [88400, 91200, 91850, 91400, 94800, 98100];
    for i in 0..6 {
        let row = 4 + i;
        set(&format!("A{row}"), months[i]);
        set(&format!("B{row}"), &revenue[i].to_string());
        set(&format!("C{row}"), &costs[i].to_string());
        set(&format!("D{row}"), &format!("=B{row}-C{row}"));
        set(&format!("E{row}"), &format!("=D{row}/B{row}"));
    }
    set("A10", "TOTAL");
    set("B10", "=SUM(B4:B9)");
    set("C10", "=SUM(C4:C9)");
    set("D10", "=B10-C10");
    set("E10", "=D10/B10");
    set("A12", "Best month");
    set("B12", "=MAX(D4:D9)");
    set("A13", "Average profit");
    set("B13", "=ROUND(AVERAGE(D4:D9))");
    set("A14", "Months above 40k");
    set("B14", "=IF(B12>40000,\"yes\",\"no\")");

    for cell in [
        "A1", "A3", "B3", "C3", "D3", "E3", "A10", "B10", "C10", "D10", "E10",
    ] {
        let r = formula::parse_a1(cell).unwrap();
        s.write_format((r.row, r.col), |f| f.bold = true);
    }
    for cell in ["B4", "B5", "B6", "B7", "B8", "B9", "B10"] {
        let r = formula::parse_a1(cell).unwrap();
        s.write_format((r.row, r.col), |f| f.num = NumFormat::Thousands);
    }
    for cell in ["C4", "C5", "C6", "C7", "C8", "C9", "C10"] {
        let r = formula::parse_a1(cell).unwrap();
        s.write_format((r.row, r.col), |f| f.num = NumFormat::Thousands);
    }
    for cell in ["D4", "D5", "D6", "D7", "D8", "D9", "D10", "B12", "B13"] {
        let r = formula::parse_a1(cell).unwrap();
        s.write_format((r.row, r.col), |f| f.num = NumFormat::Thousands);
    }
    for cell in ["E4", "E5", "E6", "E7", "E8", "E9", "E10"] {
        let r = formula::parse_a1(cell).unwrap();
        s.write_format((r.row, r.col), |f| f.num = NumFormat::Percent);
    }
    for cell in ["A3", "B3", "C3", "D3", "E3"] {
        let r = formula::parse_a1(cell).unwrap();
        s.write_format((r.row, r.col), |f| f.align = HAlign::Center);
    }
    s.col_widths.insert(0, 130.0);
    s.recalc_all();
    s
}

/// `A1` for a position — re-exported for the UI's name box.
pub fn pos_name(pos: Pos) -> String {
    formula::ref_name(pos.0, pos.1)
}

/// Parse a name-box entry like `b7` into a position.
pub fn name_pos(text: &str) -> Option<Pos> {
    formula::parse_a1(text.trim()).map(|r: CellRef| (r.row, r.col))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(name: &str) -> Pos {
        let r = formula::parse_a1(name).unwrap();
        (r.row, r.col)
    }

    fn wb_with(cells: &[(&str, &str)]) -> Workbook {
        let mut wb = Workbook::default();
        for (name, text) in cells {
            wb.set_input(at(name), text);
        }
        wb
    }

    fn shown(wb: &Workbook, name: &str) -> String {
        wb.sheet().display(at(name))
    }

    // -- recalculation -----------------------------------------------------

    #[test]
    fn formula_sees_its_inputs() {
        let wb = wb_with(&[("A1", "1"), ("A2", "2"), ("A3", "3"), ("B1", "=SUM(A1:A3)")]);
        assert_eq!(shown(&wb, "B1"), "6");
    }

    #[test]
    fn editing_an_input_updates_dependents() {
        let mut wb = wb_with(&[("A1", "1"), ("A2", "2"), ("A3", "3"), ("B1", "=SUM(A1:A3)")]);
        assert_eq!(shown(&wb, "B1"), "6");
        wb.set_input(at("A2"), "10");
        assert_eq!(shown(&wb, "B1"), "14", "the sum must follow its range");
        wb.set_input(at("A2"), "");
        assert_eq!(shown(&wb, "B1"), "4", "clearing a cell recalculates too");
    }

    #[test]
    fn chains_recalculate_in_order() {
        let mut wb = wb_with(&[
            ("A1", "1"),
            ("B1", "=A1*2"),
            ("C1", "=B1*2"),
            ("D1", "=C1*2"),
        ]);
        assert_eq!(shown(&wb, "D1"), "8");
        wb.set_input(at("A1"), "3");
        assert_eq!(shown(&wb, "B1"), "6");
        assert_eq!(shown(&wb, "C1"), "12");
        assert_eq!(shown(&wb, "D1"), "24");
    }

    #[test]
    fn diamond_dependency_computes_once_and_correctly() {
        let mut wb = wb_with(&[
            ("A1", "2"),
            ("B1", "=A1+1"),
            ("C1", "=A1*3"),
            ("D1", "=B1+C1"),
        ]);
        assert_eq!(shown(&wb, "D1"), "9");
        wb.set_input(at("A1"), "5");
        assert_eq!(shown(&wb, "D1"), "21");
    }

    #[test]
    fn a_formula_becoming_a_literal_drops_its_edges() {
        let mut wb = wb_with(&[("A1", "1"), ("B1", "=A1+1")]);
        assert_eq!(shown(&wb, "B1"), "2");
        wb.set_input(at("B1"), "99");
        wb.set_input(at("A1"), "50");
        assert_eq!(shown(&wb, "B1"), "99", "B1 no longer follows A1");
    }

    #[test]
    fn cycles_report_circ_and_recover() {
        let mut wb = wb_with(&[("A1", "=B1"), ("B1", "=A1")]);
        assert_eq!(shown(&wb, "A1"), "#CIRC!");
        assert_eq!(shown(&wb, "B1"), "#CIRC!");
        // breaking the cycle heals both cells
        wb.set_input(at("B1"), "7");
        assert_eq!(shown(&wb, "A1"), "7");
        assert_eq!(shown(&wb, "B1"), "7");
    }

    #[test]
    fn self_reference_is_a_cycle() {
        let wb = wb_with(&[("A1", "=A1+1")]);
        assert_eq!(shown(&wb, "A1"), "#CIRC!");
    }

    #[test]
    fn a_cell_downstream_of_a_cycle_is_also_circ() {
        let wb = wb_with(&[("A1", "=B1"), ("B1", "=A1"), ("C1", "=A1+1")]);
        assert_eq!(shown(&wb, "C1"), "#CIRC!");
    }

    #[test]
    fn long_cycle_is_detected() {
        let wb = wb_with(&[("A1", "=A2"), ("A2", "=A3"), ("A3", "=A1")]);
        assert_eq!(shown(&wb, "A1"), "#CIRC!");
        assert_eq!(shown(&wb, "A3"), "#CIRC!");
    }

    // -- literals and display ---------------------------------------------

    #[test]
    fn typed_text_is_coerced() {
        let wb = wb_with(&[
            ("A1", "42"),
            ("A2", "-3.5"),
            ("A3", "hello"),
            ("A4", "TRUE"),
            ("A5", "50%"),
            ("A6", "1e3"),
        ]);
        assert_eq!(wb.sheet().value(at("A1")), Value::Num(42.0));
        assert_eq!(wb.sheet().value(at("A2")), Value::Num(-3.5));
        assert_eq!(wb.sheet().value(at("A3")), Value::Text("hello".into()));
        assert_eq!(wb.sheet().value(at("A4")), Value::Bool(true));
        assert_eq!(wb.sheet().value(at("A5")), Value::Num(0.5));
        assert_eq!(wb.sheet().value(at("A6")), Value::Num(1000.0));
    }

    #[test]
    fn the_formula_bar_shows_raw_text() {
        let wb = wb_with(&[("A1", "2"), ("B1", "=A1*3")]);
        assert_eq!(wb.sheet().input(at("B1")), "=A1*3");
        assert_eq!(shown(&wb, "B1"), "6", "but the grid shows the value");
    }

    #[test]
    fn broken_formulas_stay_editable() {
        let wb = wb_with(&[("A1", "=1+")]);
        assert_eq!(shown(&wb, "A1"), "#ERROR!");
        assert_eq!(wb.sheet().input(at("A1")), "=1+");
    }

    #[test]
    fn number_formats() {
        let mut wb = wb_with(&[("A1", "1234.5"), ("A2", "0.256"), ("A3", "text")]);
        assert_eq!(shown(&wb, "A1"), "1234.5");
        wb.set_format(&[at("A1")], |f| f.num = NumFormat::Fixed2);
        assert_eq!(shown(&wb, "A1"), "1234.50");
        wb.set_format(&[at("A1")], |f| f.num = NumFormat::Thousands);
        assert_eq!(shown(&wb, "A1"), "1,234.50");
        wb.set_format(&[at("A2")], |f| f.num = NumFormat::Percent);
        assert_eq!(shown(&wb, "A2"), "25.6%");
        // a format never mangles text
        wb.set_format(&[at("A3")], |f| f.num = NumFormat::Fixed2);
        assert_eq!(shown(&wb, "A3"), "text");
    }

    #[test]
    fn thousands_grouping_edges() {
        assert_eq!(group_thousands("1"), "1");
        assert_eq!(group_thousands("100"), "100");
        assert_eq!(group_thousands("1000"), "1,000");
        assert_eq!(group_thousands("1234567"), "1,234,567");
        assert_eq!(group_thousands("-1234567"), "-1,234,567");
    }

    #[test]
    fn formats_survive_clearing_content() {
        let mut wb = wb_with(&[("A1", "5")]);
        wb.set_format(&[at("A1")], |f| f.bold = true);
        wb.clear_cells(&[at("A1")]);
        assert_eq!(shown(&wb, "A1"), "");
        assert!(wb.sheet().format(at("A1")).bold, "formatting is not content");
    }

    // -- undo / redo -------------------------------------------------------

    #[test]
    fn undo_and_redo_a_cell_edit() {
        let mut wb = wb_with(&[("A1", "1")]);
        wb.set_input(at("A1"), "2");
        assert_eq!(shown(&wb, "A1"), "2");
        assert!(wb.undo());
        assert_eq!(shown(&wb, "A1"), "1");
        assert!(wb.redo());
        assert_eq!(shown(&wb, "A1"), "2");
    }

    #[test]
    fn undo_recalculates_dependents() {
        let mut wb = wb_with(&[("A1", "1"), ("B1", "=A1*10")]);
        wb.set_input(at("A1"), "5");
        assert_eq!(shown(&wb, "B1"), "50");
        wb.undo();
        assert_eq!(shown(&wb, "B1"), "10", "undo must recalculate, not just restore");
    }

    #[test]
    fn undo_restores_a_deleted_cell() {
        let mut wb = wb_with(&[("A1", "hello")]);
        wb.clear_cells(&[at("A1")]);
        assert_eq!(shown(&wb, "A1"), "");
        wb.undo();
        assert_eq!(shown(&wb, "A1"), "hello");
    }

    #[test]
    fn undo_stack_unwinds_in_order() {
        let mut wb = Workbook::default();
        for v in ["1", "2", "3"] {
            wb.set_input(at("A1"), v);
        }
        assert_eq!(shown(&wb, "A1"), "3");
        wb.undo();
        assert_eq!(shown(&wb, "A1"), "2");
        wb.undo();
        assert_eq!(shown(&wb, "A1"), "1");
        wb.undo();
        assert_eq!(shown(&wb, "A1"), "", "the first write is undoable too");
        assert!(!wb.undo(), "nothing left to undo");
    }

    #[test]
    fn a_new_edit_clears_the_redo_stack() {
        let mut wb = wb_with(&[("A1", "1")]);
        wb.set_input(at("A1"), "2");
        wb.undo();
        assert!(wb.can_redo());
        wb.set_input(at("A1"), "9");
        assert!(!wb.can_redo());
    }

    #[test]
    fn opening_a_loaded_sheet_activates_it_and_clears_history() {
        let mut wb = Workbook::default();
        wb.set_input(at("A1"), "1");
        wb.set_input(at("A1"), "2");
        assert!(wb.undo());
        assert!(wb.can_undo());
        assert!(wb.can_redo());

        wb.open_loaded_sheet(Sheet::new("Loaded"));

        assert_eq!(wb.active, 1);
        assert_eq!(wb.sheet().name, "Loaded");
        assert!(!wb.can_undo());
        assert!(!wb.can_redo());
    }

    #[test]
    fn a_no_op_edit_is_not_recorded() {
        let mut wb = wb_with(&[("A1", "1")]);
        assert!(wb.can_undo());
        while wb.undo() {}
        wb.set_input(at("A1"), "");
        assert!(!wb.can_undo(), "writing empty over empty changes nothing");
    }

    #[test]
    fn undo_covers_formatting() {
        let mut wb = wb_with(&[("A1", "1")]);
        wb.set_format(&[at("A1")], |f| f.bold = true);
        assert!(wb.sheet().format(at("A1")).bold);
        wb.undo();
        assert!(!wb.sheet().format(at("A1")).bold);
    }

    // -- fill --------------------------------------------------------------

    #[test]
    fn fill_down_translates_relative_refs() {
        let mut wb = wb_with(&[
            ("A1", "1"),
            ("A2", "2"),
            ("A3", "3"),
            ("B1", "=A1*10"),
        ]);
        wb.fill((at("B1"), at("B1")), (at("B1"), at("B3")));
        assert_eq!(wb.sheet().input(at("B2")), "=A2*10");
        assert_eq!(wb.sheet().input(at("B3")), "=A3*10");
        assert_eq!(shown(&wb, "B2"), "20");
        assert_eq!(shown(&wb, "B3"), "30");
    }

    #[test]
    fn fill_right_translates_columns() {
        let mut wb = wb_with(&[("A1", "2"), ("B1", "3"), ("A2", "=A1*2")]);
        wb.fill((at("A2"), at("A2")), (at("A2"), at("B2")));
        assert_eq!(wb.sheet().input(at("B2")), "=B1*2");
        assert_eq!(shown(&wb, "B2"), "6");
    }

    #[test]
    fn fill_keeps_absolute_refs_pinned() {
        let mut wb = wb_with(&[("A1", "10"), ("C1", "=$A$1*2")]);
        wb.fill((at("C1"), at("C1")), (at("C1"), at("C3")));
        assert_eq!(wb.sheet().input(at("C3")), "=$A$1*2");
        assert_eq!(shown(&wb, "C3"), "20");
    }

    #[test]
    fn fill_copies_literals_and_formats() {
        let mut wb = wb_with(&[("A1", "hi")]);
        wb.set_format(&[at("A1")], |f| f.bold = true);
        wb.fill((at("A1"), at("A1")), (at("A1"), at("A3")));
        assert_eq!(shown(&wb, "A3"), "hi");
        assert!(wb.sheet().format(at("A3")).bold);
    }

    #[test]
    fn fill_repeats_a_multi_cell_source() {
        let mut wb = wb_with(&[("A1", "x"), ("A2", "y")]);
        wb.fill((at("A1"), at("A2")), (at("A1"), at("A6")));
        assert_eq!(shown(&wb, "A3"), "x");
        assert_eq!(shown(&wb, "A4"), "y");
        assert_eq!(shown(&wb, "A5"), "x");
    }

    #[test]
    fn fill_off_the_sheet_yields_ref_errors() {
        let mut wb = wb_with(&[("B2", "=A2")]);
        // filling left past column A must produce #REF!
        wb.fill((at("B2"), at("B2")), (at("A2"), at("B2")));
        assert_eq!(shown(&wb, "A2"), "#REF!");
    }

    #[test]
    fn fill_is_undoable_in_one_step() {
        let mut wb = wb_with(&[("A1", "1"), ("B1", "=A1*10")]);
        wb.fill((at("B1"), at("B1")), (at("B1"), at("B5")));
        assert_eq!(shown(&wb, "B5"), "0");
        wb.undo();
        assert_eq!(wb.sheet().input(at("B5")), "", "one fill, one undo");
    }

    // -- paste -------------------------------------------------------------

    #[test]
    fn paste_block_writes_a_rectangle() {
        let mut wb = Workbook::default();
        let rows = parse_tsv("1\t2\n3\t4\n");
        wb.paste_block(at("B2"), &rows);
        assert_eq!(shown(&wb, "B2"), "1");
        assert_eq!(shown(&wb, "C2"), "2");
        assert_eq!(shown(&wb, "B3"), "3");
        assert_eq!(shown(&wb, "C3"), "4");
    }

    #[test]
    fn paste_is_one_undo_step() {
        let mut wb = Workbook::default();
        wb.paste_block(at("A1"), &parse_tsv("1\t2\n3\t4"));
        wb.undo();
        assert_eq!(shown(&wb, "A1"), "");
        assert_eq!(shown(&wb, "B2"), "");
    }

    #[test]
    fn pasted_formulas_are_live() {
        let mut wb = wb_with(&[("A1", "5")]);
        wb.paste_block(at("B1"), &parse_tsv("=A1*2"));
        assert_eq!(shown(&wb, "B1"), "10");
    }

    #[test]
    fn tsv_round_trip() {
        let wb = wb_with(&[("A1", "1"), ("B1", "2"), ("A2", "3"), ("B2", "4")]);
        let tsv = to_tsv(wb.sheet(), at("A1"), at("B2"));
        assert_eq!(tsv, "1\t2\n3\t4\n");
        assert_eq!(parse_tsv(&tsv), vec![vec!["1", "2"], vec!["3", "4"]]);
    }

    // -- CSV ---------------------------------------------------------------

    #[test]
    fn csv_export_writes_values_not_formulas() {
        let wb = wb_with(&[("A1", "1"), ("B1", "2"), ("C1", "=A1+B1")]);
        assert_eq!(to_csv(wb.sheet()), "1,2,3\n");
    }

    #[test]
    fn csv_quotes_when_it_has_to() {
        let wb = wb_with(&[("A1", "a,b"), ("B1", "say \"hi\""), ("C1", "plain")]);
        assert_eq!(to_csv(wb.sheet()), "\"a,b\",\"say \"\"hi\"\"\",plain\n");
    }

    #[test]
    fn csv_import_round_trips() {
        let text = "name,qty\napple,3\npear,4\n";
        let sheet = sheet_from_csv("S", text);
        assert_eq!(sheet.display(at("A1")), "name");
        assert_eq!(sheet.display(at("B3")), "4");
        assert_eq!(to_csv(&sheet), text);
    }

    #[test]
    fn csv_import_keeps_formulas_live() {
        let sheet = sheet_from_csv("S", "1,2,=A1+B1\n");
        assert_eq!(sheet.display(at("C1")), "3");
    }

    #[test]
    fn csv_parsing_handles_quotes_and_blanks() {
        assert_eq!(
            parse_csv("a,\"b,c\",\n"),
            vec![vec!["a".to_string(), "b,c".to_string(), String::new()]]
        );
        assert_eq!(parse_csv(""), Vec::<Vec<String>>::new());
        assert_eq!(
            parse_csv("x\r\ny\r\n"),
            vec![vec!["x".to_string()], vec!["y".to_string()]]
        );
    }

    #[test]
    fn used_range_is_the_bounding_box() {
        let wb = wb_with(&[("B2", "x"), ("D5", "y")]);
        assert_eq!(wb.sheet().used_range(), Some(((1, 1), (4, 3))));
        assert_eq!(Sheet::new("empty").used_range(), None);
    }

    // -- statistics --------------------------------------------------------

    #[test]
    fn selection_stats_for_the_status_bar() {
        let wb = wb_with(&[("A1", "1"), ("A2", "2"), ("A3", "text"), ("A4", "3")]);
        let s = wb
            .sheet()
            .stats([at("A1"), at("A2"), at("A3"), at("A4"), at("A5")].into_iter());
        assert_eq!(s.sum, 6.0);
        assert_eq!(s.numeric, 3);
        assert_eq!(s.count, 4, "text counts as filled, blanks do not");
        assert_eq!(s.average(), Some(2.0));
        assert_eq!(s.min, Some(1.0));
        assert_eq!(s.max, Some(3.0));
    }

    // -- sheets ------------------------------------------------------------

    #[test]
    fn sheets_are_independent() {
        let mut wb = Workbook::default();
        wb.set_input(at("A1"), "first");
        wb.add_sheet();
        assert_eq!(wb.sheets.len(), 2);
        assert_eq!(shown(&wb, "A1"), "", "the new sheet starts blank");
        wb.set_input(at("A1"), "second");
        wb.active = 0;
        assert_eq!(shown(&wb, "A1"), "first");
    }

    #[test]
    fn sheets_can_be_renamed_and_removed() {
        let mut wb = Workbook::default();
        wb.add_sheet();
        wb.rename_sheet(1, "Data");
        assert_eq!(wb.sheets[1].name, "Data");
        wb.rename_sheet(1, "   ");
        assert_eq!(wb.sheets[1].name, "Data", "a blank name is refused");
        wb.remove_sheet(1);
        assert_eq!(wb.sheets.len(), 1);
        wb.remove_sheet(0);
        assert_eq!(wb.sheets.len(), 1, "the last sheet cannot be removed");
    }

    #[test]
    fn name_box_round_trip() {
        assert_eq!(pos_name((3, 1)), "B4");
        assert_eq!(name_pos("b4"), Some((3, 1)));
        assert_eq!(name_pos(" C10 "), Some((9, 2)));
        assert_eq!(name_pos("nonsense"), None);
    }

    // -- the shipped demo --------------------------------------------------

    #[test]
    fn demo_sheet_computes() {
        let s = demo_sheet();
        // revenue total of the six months
        assert_eq!(s.value(at("B10")), Value::Num(857100.0));
        assert_eq!(s.display(at("B10")), "857,100.00");
        // profit = revenue - costs
        assert_eq!(s.value(at("D4")), Value::Num(32100.0));
        assert_eq!(s.display(at("E4")), "26.6%");
        assert_eq!(s.display(at("B14")), "yes");
        assert!(s.format(at("A1")).bold);
    }

    #[test]
    fn the_headline_case_from_the_brief() {
        // "typing =SUM(A1:A3) after entering 1,2,3 shows 6 and updates when A2 changes"
        let mut wb = Workbook::default();
        wb.set_input(at("A1"), "1");
        wb.set_input(at("A2"), "2");
        wb.set_input(at("A3"), "3");
        wb.set_input(at("A4"), "=SUM(A1:A3)");
        assert_eq!(shown(&wb, "A4"), "6");
        wb.set_input(at("A2"), "20");
        assert_eq!(shown(&wb, "A4"), "24");
    }
}
