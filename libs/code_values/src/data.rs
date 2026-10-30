//! Reading a literal's parts from the source text, and writing a changed
//! number back as it was written: pure, no drawing.
//!
//! The host passes only where a value starts and what control it is; the
//! items (each number's, colour's or word's bytes) are read here from the
//! text itself ([`lit_at`]), so a write replaces exactly the bytes of the
//! numbers that changed and keeps everything between them.

use crate::Control;
use std::ops::Range;

/// A Splash literal as written: each part's bytes in the source.
#[derive(Clone, Debug, PartialEq)]
pub enum Lit {
    Num { range: Range<usize>, value: f64 },
    Color { range: Range<usize>, rgba: [f32; 4] },
    Str { range: Range<usize>, text: String },
    List { range: Range<usize>, items: Vec<Lit> },
    Obj { range: Range<usize>, fields: Vec<(String, Lit)> },
    /// `vec2(..)`, `vec3(..)`, `vec4(..)` of plain numbers.
    Vector { range: Range<usize>, items: Vec<(Range<usize>, f64)> },
    /// Anything else (a name, a call, an expression): shown, not edited.
    Other { range: Range<usize> },
}

impl Lit {
    pub fn range(&self) -> Range<usize> {
        match self {
            Lit::Num { range, .. } | Lit::Color { range, .. } | Lit::Str { range, .. } | Lit::List { range, .. } | Lit::Obj { range, .. } | Lit::Vector { range, .. } | Lit::Other { range } => range.clone(),
        }
    }

    /// The numbers of a vector: `vec3(x, y, z)` or a short list `[x, y, z]`.
    pub fn numbers(&self) -> Option<Vec<(Range<usize>, f64)>> {
        match self {
            Lit::Vector { items, .. } => Some(items.clone()),
            Lit::List { items, .. } if !items.is_empty() => items.iter().map(|i| match i {
                Lit::Num { range, value } => Some((range.clone(), *value)),
                _ => None,
            }).collect(),
            _ => None,
        }
    }
}

struct Parser<'a> {
    src: &'a str,
    b: &'a [u8],
    i: usize,
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    /// Whitespace and comments.
    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.i += 1;
            } else if c == b'/' && self.b.get(self.i + 1) == Some(&b'/') {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.i += 1;
                }
            } else if c == b'/' && self.b.get(self.i + 1) == Some(&b'*') {
                self.i += 2;
                while self.i + 1 < self.b.len() && !(self.b[self.i] == b'*' && self.b[self.i + 1] == b'/') {
                    self.i += 1;
                }
                self.i = (self.i + 2).min(self.b.len());
            } else {
                break;
            }
        }
    }

    fn skip_seps(&mut self) {
        loop {
            self.skip_ws();
            if self.peek() == Some(b',') || self.peek() == Some(b';') {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    /// Past a balanced run up to a separator or a closing bracket at depth 0.
    fn skip_expr(&mut self) {
        let mut depth = 0i32;
        while let Some(c) = self.peek() {
            match c {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                b',' | b'\n' if depth == 0 => return,
                b'"' => {
                    self.i += 1;
                    while let Some(c) = self.peek() {
                        self.i += if c == b'\\' { 2 } else { 1 };
                        if c == b'"' {
                            break;
                        }
                    }
                    continue;
                }
                _ => {}
            }
            self.i += 1;
        }
    }

    /// Past one balanced bracket group starting at the opening.
    fn skip_group(&mut self) {
        let mut depth = 0i32;
        while let Some(c) = self.peek() {
            match c {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth <= 0 {
                        self.i += 1;
                        return;
                    }
                }
                b'"' => {
                    self.i += 1;
                    while let Some(c) = self.peek() {
                        self.i += if c == b'\\' { 2 } else { 1 };
                        if c == b'"' {
                            break;
                        }
                    }
                    continue;
                }
                _ => {}
            }
            self.i += 1;
        }
    }

    fn number(&mut self) -> Option<(Range<usize>, f64)> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        let digits = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == b'.' || ((c == b'e' || c == b'E') && self.i > digits) || ((c == b'-' || c == b'+') && matches!(self.b[self.i - 1], b'e' | b'E')) {
                self.i += 1;
            } else {
                break;
            }
        }
        // A name that starts with digits is not a number.
        if self.peek().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_') {
            self.i = start;
            return None;
        }
        let v = self.src[start..self.i].parse::<f64>().ok()?;
        Some((start..self.i, v))
    }

    /// After a value: an operator or a call means it was an expression.
    fn continues(&self) -> bool {
        let mut j = self.i;
        while j < self.b.len() && (self.b[j] == b' ' || self.b[j] == b'\t') {
            j += 1;
        }
        match self.b.get(j) {
            Some(b'+' | b'*' | b'/' | b'%' | b'(' | b'.' | b'?' | b'<' | b'>' | b'=' | b'&' | b'|') => !(self.b[j] == b'/' && matches!(self.b.get(j + 1), Some(b'/' | b'*'))),
            Some(b'-') => self.b.get(j + 1).is_some_and(|c| c.is_ascii_whitespace()),
            _ => false,
        }
    }

    fn value(&mut self) -> Option<Lit> {
        self.skip_ws();
        let start = self.i;
        let c = self.peek()?;
        let lit = match c {
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_seps();
                    match self.peek() {
                        Some(b']') => {
                            self.i += 1;
                            break;
                        }
                        None => return None,
                        _ => items.push(self.value()?),
                    }
                }
                Lit::List { range: start..self.i, items }
            }
            b'{' => {
                self.i += 1;
                let mut fields = Vec::new();
                loop {
                    self.skip_seps();
                    match self.peek() {
                        Some(b'}') => {
                            self.i += 1;
                            break;
                        }
                        None => return None,
                        Some(c) if is_ident(c) || c == b'"' => {
                            let key = if c == b'"' {
                                match self.value()? {
                                    Lit::Str { text, .. } => text,
                                    _ => return None,
                                }
                            } else {
                                let k = self.i;
                                while self.peek().is_some_and(is_ident) {
                                    self.i += 1;
                                }
                                self.src[k..self.i].to_string()
                            };
                            self.skip_ws();
                            if self.peek() != Some(b':') {
                                // Not a plain object (a body, a spread): opaque.
                                self.i = start;
                                self.skip_group();
                                return Some(Lit::Other { range: start..self.i });
                            }
                            self.i += 1;
                            fields.push((key, self.value()?));
                        }
                        Some(_) => {
                            self.i = start;
                            self.skip_group();
                            return Some(Lit::Other { range: start..self.i });
                        }
                    }
                }
                Lit::Obj { range: start..self.i, fields }
            }
            b'"' => {
                self.i += 1;
                let mut text = String::new();
                let mut run = self.i;
                loop {
                    match self.peek() {
                        None => return None,
                        Some(b'"') => {
                            text.push_str(&self.src[run..self.i]);
                            self.i += 1;
                            break;
                        }
                        Some(b'\\') => {
                            text.push_str(&self.src[run..self.i]);
                            let e = self.b.get(self.i + 1).copied().unwrap_or(b'\\');
                            text.push(match e {
                                b'n' => '\n',
                                b't' => '\t',
                                b'r' => '\r',
                                other => other as char,
                            });
                            self.i += 2;
                            run = self.i;
                        }
                        Some(_) => self.i += 1,
                    }
                }
                Lit::Str { range: start..self.i, text }
            }
            b'#' => {
                self.i += 1;
                while self.peek().is_some_and(is_ident) {
                    self.i += 1;
                }
                match parse_hex(&self.src[start..self.i]) {
                    Some(rgba) => Lit::Color { range: start..self.i, rgba },
                    None => Lit::Other { range: start..self.i },
                }
            }
            b'-' | b'.' | b'0'..=b'9' => match self.number() {
                Some((range, value)) => Lit::Num { range, value },
                None => {
                    self.skip_expr();
                    Lit::Other { range: start..self.i }
                }
            },
            c if c.is_ascii_alphabetic() || c == b'_' || c == b'@' => {
                self.i += 1;
                while self.peek().is_some_and(is_ident) {
                    self.i += 1;
                }
                let word = &self.src[start..self.i];
                if self.peek() == Some(b'(') {
                    if matches!(word, "vec2" | "vec3" | "vec4") {
                        let save = self.i;
                        self.i += 1;
                        let mut items = Vec::new();
                        let mut plain = true;
                        loop {
                            self.skip_seps();
                            match self.peek() {
                                Some(b')') => {
                                    self.i += 1;
                                    break;
                                }
                                Some(b'-' | b'.' | b'0'..=b'9') => match self.number() {
                                    Some(n) if !self.continues() => items.push(n),
                                    _ => {
                                        plain = false;
                                        break;
                                    }
                                },
                                _ => {
                                    plain = false;
                                    break;
                                }
                            }
                        }
                        if plain && items.len() >= 2 {
                            return Some(self.after(Lit::Vector { range: start..self.i, items }, start));
                        }
                        self.i = save;
                    }
                    self.skip_group();
                }
                Lit::Other { range: start..self.i }
            }
            _ => return None,
        };
        Some(self.after(lit, start))
    }

    /// A value followed by an operator is an expression: opaque.
    fn after(&mut self, lit: Lit, start: usize) -> Lit {
        if self.continues() {
            self.skip_expr();
            while self.i > start && self.b[self.i - 1].is_ascii_whitespace() {
                self.i -= 1;
            }
            return Lit::Other { range: start..self.i };
        }
        lit
    }
}

/// The literal starting at byte `start` of `source` (whitespace before it
/// skipped), with each part's bytes.
pub fn lit_at(source: &str, start: usize) -> Option<Lit> {
    if start > source.len() {
        return None;
    }
    Parser { src: source, b: source.as_bytes(), i: start }.value()
}

/// A Splash colour: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` (or with an
/// `x` after the `#`), as 0..1 RGBA.
pub fn parse_hex(text: &str) -> Option<[f32; 4]> {
    let h = text.strip_prefix('#')?;
    let h = h.strip_prefix('x').unwrap_or(h);
    if !h.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let n = |s: &str| u8::from_str_radix(s, 16).ok().map(|v| v as f32 / 255.0);
    let d = |c: char| u8::from_str_radix(&c.to_string(), 16).ok().map(|v| (v * 17) as f32 / 255.0);
    let cs: Vec<char> = h.chars().collect();
    Some(match cs.len() {
        3 => [d(cs[0])?, d(cs[1])?, d(cs[2])?, 1.0],
        4 => [d(cs[0])?, d(cs[1])?, d(cs[2])?, d(cs[3])?],
        6 => [n(&h[0..2])?, n(&h[2..4])?, n(&h[4..6])?, 1.0],
        8 => [n(&h[0..2])?, n(&h[2..4])?, n(&h[4..6])?, n(&h[6..8])?],
        _ => return None,
    })
}

/// Names a position goes by (a vec2 or vec3 under one is a point).
fn position_name(key: &str) -> bool {
    const NAMES: &[&str] = &["pos", "position", "at", "target", "eye", "from", "to", "look_at", "center", "centre", "origin", "offset", "anchor", "pivot", "p", "point", "loc", "location", "focus", "start", "end", "via"];
    let key = key.to_ascii_lowercase();
    let key = key.split('[').next().unwrap_or(&key);
    NAMES.contains(&key) || ["_pos", "_position", "_target", "_at", "_center", "_origin", "_offset"].iter().any(|s| key.ends_with(s))
}

/// Names a direction goes by (a vec3 under one is a direction).
fn direction_name(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    let key = key.split('[').next().unwrap_or(&key);
    matches!(key, "dir" | "direction" | "normal" | "axis" | "forward" | "up" | "toward" | "towards" | "sun" | "light") || ["_dir", "_direction", "_normal", "_axis"].iter().any(|s| key.ends_with(s))
}

/// Names a list of 3D keys goes by.
fn path_name(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    matches!(key.as_str(), "path" | "points" | "keys" | "route" | "track" | "rail" | "curve" | "spline" | "waypoints" | "positions") || ["_path", "_points", "_keys", "_route"].iter().any(|s| key.ends_with(s))
}

/// The control for a literal, read from its text: `text` is the literal
/// as written (from its first byte), `key` the field or `let` it is
/// written under (`pos`, `dir`, `COLORS`; empty when none).
///
/// - `vec2(..)` (or `[x, y]`) under a position's name: [`Control::Point`];
/// - `vec3(..)` (or `[x, y, z]`) under a position's name: [`Control::Point3`],
///   under a direction's: [`Control::Direction`];
/// - a list of colours: [`Control::Swatches`];
/// - a list of two or more 3D points (`vec3`s, or `[x, y, z]`s under a
///   path's name): [`Control::Path3`];
/// - a list of records (lists or objects of numbers and words):
///   [`Control::Table`];
/// - a list of numbers: [`Control::Data`].
///
/// Anything else: `None` (the host's own control, if any).
pub fn control_of_literal(text: &str, key: &str) -> Option<Control> {
    let lit = lit_at(text, 0)?;
    // The literal must be the whole text.
    if text[lit.range().end..].trim().len() > 0 {
        return None;
    }
    let vector = |lit: &Lit| -> Option<Vec<f64>> {
        let explicit = matches!(lit, Lit::Vector { .. });
        let n = lit.numbers()?;
        (explicit || n.len() <= 4).then(|| n.into_iter().map(|(_, v)| v).collect())
    };
    match &lit {
        Lit::Vector { .. } | Lit::List { .. } if vector(&lit).is_some_and(|v| v.len() == 2 || v.len() == 3) && (matches!(lit, Lit::Vector { .. }) || position_name(key) || direction_name(key)) => {
            let v = vector(&lit)?;
            if v.len() == 2 {
                return position_name(key).then(|| Control::Point([v[0] as f32, v[1] as f32]));
            }
            let v3 = [v[0] as f32, v[1] as f32, v[2] as f32];
            if direction_name(key) {
                Some(Control::Direction(v3))
            } else if position_name(key) {
                Some(Control::Point3(v3))
            } else {
                None
            }
        }
        Lit::List { items, .. } if !items.is_empty() => {
            if items.iter().all(|i| matches!(i, Lit::Color { .. })) {
                return Some(Control::Swatches { colors: items.iter().filter_map(|i| if let Lit::Color { rgba, .. } = i { Some(*rgba) } else { None }).collect() });
            }
            if items.iter().all(|i| matches!(i, Lit::Num { .. })) {
                return Some(Control::Data { values: items.iter().filter_map(|i| if let Lit::Num { value, .. } = i { Some(*value as f32) } else { None }).collect(), pairs: false, name: key.to_string(), axis: crate::DataAxis::Index });
            }
            let keys: Option<Vec<[f32; 3]>> = items.iter().map(|i| {
                let v = i.numbers()?;
                (v.len() == 3 && (matches!(i, Lit::Vector { .. }) || path_name(key))).then(|| [v[0].1 as f32, v[1].1 as f32, v[2].1 as f32])
            }).collect();
            if let Some(keys) = keys.filter(|k| k.len() >= 2) {
                return Some(Control::Path3 { keys });
            }
            let table = table_of(&lit)?;
            Some(Control::Table { rows: table.rows, cols: table.heads.len(), heads: table.heads.clone() })
        }
        _ => None,
    }
}

/// A cell's value.
#[derive(Clone, Debug, PartialEq)]
pub enum CellValue {
    Num(f64),
    Str(String),
    Color([f32; 4]),
    /// Shown, not edited.
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub range: Range<usize>,
    pub row: usize,
    pub col: usize,
    pub value: CellValue,
}

/// A list's items as a grid: records as rows (a list of lists or objects),
/// with a column per field.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub rows: usize,
    /// The columns' heads: the objects' keys, else their index.
    pub heads: Vec<String>,
    /// Whether the heads are names (records are objects).
    pub named: bool,
    pub cells: Vec<Cell>,
}

fn cell_of(lit: &Lit, row: usize, col: usize) -> Option<Cell> {
    let value = match lit {
        Lit::Num { value, .. } => CellValue::Num(*value),
        Lit::Str { text, .. } => CellValue::Str(text.clone()),
        Lit::Color { rgba, .. } => CellValue::Color(*rgba),
        Lit::Other { .. } => CellValue::Other,
        _ => return None,
    };
    Some(Cell { range: lit.range(), row, col, value })
}

/// The list `lit` as a table of records: each item a list, a vector or an
/// object of plain values (numbers, words, colours). `None` when it is not
/// one.
pub fn table_of(lit: &Lit) -> Option<Table> {
    let Lit::List { items, .. } = lit else { return None };
    if items.is_empty() {
        return None;
    }
    let mut heads: Vec<String> = Vec::new();
    let mut cells = Vec::new();
    let named = items.iter().all(|i| matches!(i, Lit::Obj { .. }));
    for (row, item) in items.iter().enumerate() {
        match item {
            Lit::List { items: fields, .. } if !fields.is_empty() => {
                for (col, f) in fields.iter().enumerate() {
                    cells.push(cell_of(f, row, col)?);
                }
            }
            Lit::Vector { items: fields, .. } => {
                for (col, (range, v)) in fields.iter().enumerate() {
                    cells.push(Cell { range: range.clone(), row, col, value: CellValue::Num(*v) });
                }
            }
            Lit::Obj { fields, .. } if named && !fields.is_empty() => {
                for (key, f) in fields {
                    let col = match heads.iter().position(|h| h == key) {
                        Some(c) => c,
                        None => {
                            heads.push(key.clone());
                            heads.len() - 1
                        }
                    };
                    cells.push(cell_of(f, row, col)?);
                }
            }
            _ => return None,
        }
    }
    // A table has something to edit.
    if !cells.iter().any(|c| c.value != CellValue::Other) {
        return None;
    }
    if !named {
        let cols = cells.iter().map(|c| c.col + 1).max().unwrap_or(0);
        heads = (0..cols).map(|c| c.to_string()).collect();
        if items.iter().all(|i| matches!(i, Lit::Vector { .. })) {
            for (c, h) in heads.iter_mut().enumerate() {
                *h = ["x", "y", "z", "w"].get(c).map_or(h.clone(), |s| s.to_string());
            }
        }
    }
    Some(Table { rows: items.len(), heads, named, cells })
}

/// The colours of the list `[…]` at `start`: each one's bytes and value.
pub fn color_items(source: &str, start: usize) -> Vec<(Range<usize>, [f32; 4])> {
    match lit_at(source, start) {
        Some(Lit::List { items, .. }) => items.iter().filter_map(|i| if let Lit::Color { range, rgba } = i { Some((range.clone(), *rgba)) } else { None }).collect(),
        _ => Vec::new(),
    }
}

/// The numbers of the vector at `start` (`vec3(..)` or `[x, y, z]`).
pub fn vector_items(source: &str, start: usize) -> Vec<(Range<usize>, f64)> {
    lit_at(source, start).and_then(|l| l.numbers()).unwrap_or_default()
}

/// The keys of the 3D path at `start`: each key's three numbers.
pub fn path_items(source: &str, start: usize) -> Vec<Vec<(Range<usize>, f64)>> {
    match lit_at(source, start) {
        Some(Lit::List { items, .. }) => items.iter().filter_map(|i| i.numbers().filter(|n| n.len() == 3)).collect(),
        _ => Vec::new(),
    }
}

/// The key a value at `start` is written under: `key: …` or `let KEY = …`.
pub(crate) fn key_before(source: &str, start: usize) -> String {
    let before = source[..start].trim_end();
    let before = if let Some(b) = before.strip_suffix(':') {
        b
    } else if let Some(b) = before.strip_suffix('=') {
        b.trim_end_matches(['+', '-', '*', '/'])
    } else {
        return String::new();
    };
    let before = before.trim_end();
    let from = before.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
    before[from..].to_string()
}

/// A round step for a span: 1, 2 or 5 times a power of ten, about a tenth
/// of it.
pub fn nice_step(span: f64) -> f64 {
    if !(span.is_finite() && span > 0.0) {
        return 1.0;
    }
    let raw = span / 10.0;
    let p = 10f64.powf(raw.log10().floor());
    let m = raw / p;
    p * if m < 1.5 { 1.0 } else if m < 3.5 { 2.0 } else if m < 7.5 { 5.0 } else { 10.0 }
}

/// The smallest round number (1, 2, 5 × 10ⁿ) at or above `v`.
pub fn nice_ceil(v: f64) -> f64 {
    if !(v.is_finite() && v > 0.0) {
        return 1.0;
    }
    let p = 10f64.powf(v.log10().floor());
    let m = v / p;
    p * if m <= 1.0 { 1.0 } else if m <= 2.0 { 2.0 } else if m <= 5.0 { 5.0 } else { 10.0 }
}

/// Decimal places a step needs.
pub fn places_of(step: f64) -> usize {
    (0..8).find(|p| {
        let scaled = step * 10f64.powi(*p as i32);
        (scaled - scaled.round()).abs() < 1e-6 * scaled.abs().max(1.0)
    }).unwrap_or(8)
}

/// `value` written at `places` decimals, without a negative zero.
pub fn fmt_places(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    if text.starts_with("-0") && text.trim_start_matches(['-', '0', '.']).is_empty() { text[1..].to_string() } else { text }
}

/// `value` snapped to `step`, written at the step's places.
pub fn fmt_snapped(value: f64, step: f64) -> String {
    fmt_places((value / step).round() * step, places_of(step))
}

/// A changed number as written: the places it was written with, at least
/// enough to show a move across `span` (the view's width).
pub fn fmt_like(value: f64, written: &str, span: f64) -> String {
    let written_places = written.split('.').nth(1).map_or(0, |d| d.bytes().take_while(u8::is_ascii_digit).count());
    let needed = if span <= 0.0 { 0 } else { (2.0 - span.log10().floor()).clamp(0.0, 6.0) as usize };
    let places = written_places.max(needed.min(written_places.max(3)));
    fmt_places(value, places)
}

pub fn len3(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// A direction as a unit vector (straight up when it has no length).
pub fn normalize3(v: [f64; 3]) -> [f64; 3] {
    let l = len3(v);
    if !(l > 1e-12) || !l.is_finite() {
        return [0.0, 1.0, 0.0];
    }
    [v[0] / l, v[1] / l, v[2] / l]
}

/// A direction dragged to `unit`, written at the length it was written
/// with (a light's `dir` may not be unit length; its length is kept).
pub fn redirect(unit: [f64; 3], written: [f64; 3]) -> [f64; 3] {
    let u = normalize3(unit);
    let l = len3(written);
    let l = if l > 1e-12 && l.is_finite() { l } else { 1.0 };
    [u[0] * l, u[1] * l, u[2] * l]
}

/// A number typed into a cell: a plain number (with an optional sign).
pub fn typed_number(text: &str) -> Option<String> {
    let t = text.trim();
    let v = t.parse::<f64>().ok()?;
    v.is_finite().then(|| {
        // Keep what was typed, without a leading `+`.
        t.trim_start_matches('+').to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_read_with_their_bytes() {
        let src = "pos: vec3(1.5, -2, .25) // x";
        let lit = lit_at(src, 5).unwrap();
        let n = lit.numbers().unwrap();
        assert_eq!(n.iter().map(|(r, _)| &src[r.clone()]).collect::<Vec<_>>(), vec!["1.5", "-2", ".25"]);
        assert_eq!(lit.range(), 5..src.find(')').unwrap() + 1);
        // An expression is opaque.
        assert!(matches!(lit_at("vec3(a, 1, 2)", 0), Some(Lit::Other { .. })));
        assert!(matches!(lit_at("[1, 2 * k, 3]", 0), Some(Lit::List { items, .. }) if matches!(items[1], Lit::Other { .. }) && items.len() == 3));
        assert!(matches!(lit_at("[1, 3 - 1]", 0), Some(Lit::List { items, .. }) if items.len() == 2 && matches!(items[1], Lit::Other { .. })));
        assert!(matches!(lit_at("[1, -1]", 0), Some(Lit::List { items, .. }) if items.len() == 2 && matches!(items[1], Lit::Num { value, .. } if value == -1.0)));
    }

    #[test]
    fn colours_parse_in_every_form() {
        assert_eq!(parse_hex("#ff0080"), Some([1.0, 0.0, 128.0 / 255.0, 1.0]));
        assert_eq!(parse_hex("#fff"), Some([1.0, 1.0, 1.0, 1.0]));
        assert_eq!(parse_hex("#x3a4656").map(|c| (c[0] * 255.0).round()), Some(58.0));
        assert_eq!(parse_hex("#00000080").map(|c| c[3]), Some(128.0 / 255.0));
        assert_eq!(parse_hex("#zz"), None);
        let src = "let C = [#ff5a36, #4ec9b0,\n  #ffffff80]";
        let items = color_items(src, src.find('[').unwrap());
        assert_eq!(items.iter().map(|(r, _)| &src[r.clone()]).collect::<Vec<_>>(), vec!["#ff5a36", "#4ec9b0", "#ffffff80"]);
    }

    #[test]
    fn literals_classify_by_shape_and_name() {
        assert_eq!(control_of_literal("vec3(0, 1.6, 4)", "pos"), Some(Control::Point3([0.0, 1.6, 4.0])));
        assert_eq!(control_of_literal("vec3(0.62, 0.34, -0.3)", "dir"), Some(Control::Direction([0.62, 0.34, -0.3])));
        assert_eq!(control_of_literal("vec3(1.0, 0.95, 0.88)", "color"), None);
        assert_eq!(control_of_literal("vec2(960, 540)", "at"), Some(Control::Point([960.0, 540.0])));
        assert_eq!(control_of_literal("[40, -2]", "pos"), Some(Control::Point([40.0, -2.0])));
        assert_eq!(control_of_literal("vec2(2, 1)", "size"), None);
        assert!(matches!(control_of_literal("[#ff0000, #00ff00]", "PALETTE"), Some(Control::Swatches { colors }) if colors.len() == 2));
        assert!(matches!(control_of_literal("[vec3(-128, 0, 0), vec3(128, 0, 0), vec3(0, 4, 9)]", "points"), Some(Control::Path3 { keys }) if keys.len() == 3 && keys[2] == [0.0, 4.0, 9.0]));
        assert!(matches!(control_of_literal("[[0, 1, 2], [3, 4, 5]]", "path"), Some(Control::Path3 { keys }) if keys.len() == 2));
        assert_eq!(control_of_literal("[[0, 1, 2], [3, 4, 5]]", "R"), Some(Control::Table { rows: 2, cols: 3, heads: vec!["0".into(), "1".into(), "2".into()] }));
        assert_eq!(control_of_literal("[{name: \"a\", v: 1}, {name: \"b\", v: 2.5, w: 3}]", "ROWS"), Some(Control::Table { rows: 2, cols: 3, heads: vec!["name".into(), "v".into(), "w".into()] }));
        assert!(matches!(control_of_literal("[1, 2, 3, 4, 5]", "D"), Some(Control::Data { values, .. }) if values.len() == 5));
        assert_eq!(control_of_literal("[a, b]", "x"), None);
    }

    #[test]
    fn tables_read_typed_cells() {
        let src = "[{label: \"Q1\", value: 12}, {label: \"Q2\", value: -3.5}]";
        let t = table_of(&lit_at(src, 0).unwrap()).unwrap();
        assert_eq!(t.heads, vec!["label", "value"]);
        assert!(t.named);
        assert_eq!(t.cells.len(), 4);
        assert_eq!(t.cells[3].value, CellValue::Num(-3.5));
        assert_eq!(&src[t.cells[3].range.clone()], "-3.5");
        assert_eq!(t.cells[2].value, CellValue::Str("Q2".into()));
        assert_eq!(&src[t.cells[2].range.clone()], "\"Q2\"");
        assert_eq!((t.cells[3].row, t.cells[3].col), (1, 1));
    }

    #[test]
    fn numbers_keep_their_written_precision() {
        assert_eq!(fmt_like(1.23456, "1.50", 4.0), "1.23");
        assert_eq!(fmt_like(2.0, "-2", 4.0), "2.00");
        assert_eq!(fmt_like(2.0, "-2", 10.0), "2.0");
        assert_eq!(fmt_like(2.26, "0", 40.0), "2.3");
    }

    #[test]
    fn snapping_lands_on_round_values() {
        assert_eq!(nice_step(10.0), 1.0);
        assert_eq!(nice_step(3.0), 0.2);
        assert_eq!(nice_step(0.7), 0.05);
        assert_eq!(nice_ceil(3.2), 5.0);
        assert_eq!(nice_ceil(0.9), 1.0);
        assert_eq!(fmt_snapped(1.234, 0.2), "1.2");
        assert_eq!(fmt_snapped(1.234, 0.05), "1.25");
        assert_eq!(fmt_snapped(-0.01, 0.5), "0.0");
        assert_eq!(fmt_snapped(17.0, 5.0), "15");
        assert_eq!(places_of(0.25), 2);
        assert_eq!(places_of(0.5), 1);
        assert_eq!(places_of(0.01), 2);
    }

    #[test]
    fn directions_normalise_and_keep_their_length() {
        let u = normalize3([3.0, 0.0, 4.0]);
        assert!((u[0] - 0.6).abs() < 1e-12 && (u[2] - 0.8).abs() < 1e-12);
        assert_eq!(normalize3([0.0, 0.0, 0.0]), [0.0, 1.0, 0.0]);
        let w = [0.62, 0.34, -0.3];
        let d = redirect([0.0, 0.0, -2.0], w);
        assert!((len3(d) - len3(w)).abs() < 1e-12);
        assert!((d[2] + len3(w)).abs() < 1e-12);
        assert_eq!(redirect([1.0, 0.0, 0.0], [0.0; 3]), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn typed_numbers_are_checked() {
        assert_eq!(typed_number(" 2.50 "), Some("2.50".into()));
        assert_eq!(typed_number("+3"), Some("3".into()));
        assert_eq!(typed_number("-.5"), Some("-.5".into()));
        assert_eq!(typed_number("abc"), None);
        assert_eq!(typed_number("inf"), None);
    }
}
