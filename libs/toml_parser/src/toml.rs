//! A dependency-free TOML 1.0 subset parser producing a hierarchical document.
//!
//! Supported: bare, quoted and dotted keys; `[table]` and `[[array-of-tables]]`
//! headers with the standard nesting rules; inline tables; arrays (mixed
//! element types, trailing comma, comments and newlines inside); basic and
//! literal strings, single- and multi-line, with escapes; integers with
//! underscores and `0x`/`0o`/`0b` radix prefixes; floats with fraction and
//! exponent, `inf` and `nan`; booleans; date-times, dates and times (kept as
//! their source text); comments.
//!
//! Every scalar carries a [`TomlSpan`]: zero-based UTF-8 byte offsets covering
//! the complete lexeme (quotes included). Duplicate keys, redefined tables and
//! table/value conflicts are errors that point at the offending key.
use std::collections::BTreeMap;

/// Zero-based UTF-8 byte offsets of a lexeme in the source text.
#[derive(PartialEq, Debug, Clone, Default)]
pub struct TomlSpan {
    pub start: usize,
    pub len: usize,
}

impl TomlSpan {
    pub fn end(&self) -> usize {
        self.start + self.len
    }
}

/// A table: keys in sorted order.
pub type TomlTable = BTreeMap<String, Toml>;

#[derive(PartialEq, Debug, Clone)]
pub enum Toml {
    Str(String, TomlSpan),
    Bool(bool, TomlSpan),
    Num(f64, TomlSpan),
    /// A date, time or date-time kept as written.
    Date(String, TomlSpan),
    Array(Vec<Toml>),
    /// A `[table]`, a dotted-key table or an inline table `{ k = v }`.
    Table(TomlTable),
    /// The elements of an `[[array-of-tables]]`, in source order.
    ArrayOfTables(Vec<TomlTable>),
}

impl Toml {
    pub fn into_str(self) -> Option<String> {
        match self {
            Self::Str(v, _) => Some(v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(v, _) => Some(v.as_str()),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v, _) => Some(*v),
            _ => None,
        }
    }
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Self::Num(v, _) => Some(*v),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Toml]> {
        match self {
            Self::Array(v) => Some(v.as_slice()),
            _ => None,
        }
    }
    pub fn as_table(&self) -> Option<&TomlTable> {
        match self {
            Self::Table(t) => Some(t),
            _ => None,
        }
    }
    pub fn as_array_of_tables(&self) -> Option<&[TomlTable]> {
        match self {
            Self::ArrayOfTables(v) => Some(v.as_slice()),
            _ => None,
        }
    }
    /// The span of a scalar value; arrays and tables have none.
    pub fn span(&self) -> Option<&TomlSpan> {
        match self {
            Self::Str(_, s) | Self::Bool(_, s) | Self::Num(_, s) | Self::Date(_, s) => Some(s),
            _ => None,
        }
    }
}

/// A parsed TOML file.
#[derive(PartialEq, Debug, Clone, Default)]
pub struct TomlDocument {
    pub root: TomlTable,
}

impl TomlDocument {
    /// Look up a value by key path. Each segment is one key; quoted keys are
    /// passed without quotes (`["patch", "https://example.org/x"]`). A segment
    /// that lands on an array of tables continues into its last element, the
    /// same table a following `[a.b]` header would have extended.
    pub fn get_path(&self, segments: &[&str]) -> Option<&Toml> {
        let (first, rest) = segments.split_first()?;
        let mut current = self.root.get(*first)?;
        for segment in rest {
            let table = match current {
                Toml::Table(table) => table,
                Toml::ArrayOfTables(tables) => tables.last()?,
                _ => return None,
            };
            current = table.get(*segment)?;
        }
        Some(current)
    }
}

pub struct TomlErr {
    pub msg: String,
    pub span: TomlSpan,
}

impl std::fmt::Debug for TomlErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Toml error: {}, start:{} len:{}",
            self.msg, self.span.start, self.span.len
        )
    }
}

impl std::fmt::Display for TomlErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.msg, self.span.start)
    }
}

/// Parse a complete TOML text.
pub fn parse_toml(text: &str) -> Result<TomlDocument, TomlErr> {
    let mut parser = Parser {
        src: text,
        bytes: text.as_bytes(),
        pos: 0,
    };
    let mut root: NodeTable = BTreeMap::new();
    let mut scope: Vec<Key> = Vec::new();
    loop {
        parser.skip_blank_lines();
        if parser.at_end() {
            break;
        }
        if parser.peek() == b'[' {
            let header = parser.parse_header()?;
            define_header(&mut root, &header.path, header.array, &header.span)?;
            scope = header.path;
            parser.expect_line_end()?;
            continue;
        }
        let key = parser.parse_key()?;
        parser.skip_ws();
        parser.expect_byte(b'=', "expected `=` after key")?;
        parser.skip_ws();
        let value = parser.parse_value()?;
        let table = navigate(&mut root, &scope, Navigation::Header)?;
        insert_value(table, &key, value)?;
        parser.expect_line_end()?;
    }
    Ok(TomlDocument {
        root: table_to_toml(root),
    })
}

// --- internal tree with table provenance -----------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Origin {
    /// Created on the way to a deeper header, `[a.b]` creating `a`.
    Implicit,
    /// Defined by its own `[a]` header.
    Header,
    /// Created by a dotted key, `a.b = 1` creating `a`.
    Dotted,
    /// An inline table `{ ... }`; closed to later extension.
    Inline,
}

type NodeTable = BTreeMap<String, Node>;

enum Node {
    Value(Toml),
    Table { map: NodeTable, origin: Origin },
    ArrayOfTables(Vec<NodeTable>),
}

fn table_to_toml(table: NodeTable) -> TomlTable {
    table
        .into_iter()
        .map(|(key, node)| (key, node_to_toml(node)))
        .collect()
}

fn node_to_toml(node: Node) -> Toml {
    match node {
        Node::Value(value) => value,
        Node::Table { map, .. } => Toml::Table(table_to_toml(map)),
        Node::ArrayOfTables(tables) => {
            Toml::ArrayOfTables(tables.into_iter().map(table_to_toml).collect())
        }
    }
}

#[derive(Clone, Debug)]
struct Key {
    /// The key path, quotes removed; one element per dotted segment.
    segments: Vec<String>,
    span: TomlSpan,
}

struct Header {
    path: Vec<Key>,
    array: bool,
    span: TomlSpan,
}

#[derive(Clone, Copy, PartialEq)]
enum Navigation {
    /// Header semantics: implicit tables, entering the last element of an
    /// array of tables.
    Header,
    /// Dotted-key semantics: dotted tables, arrays of tables are closed.
    Dotted,
}

fn conflict(msg: String, span: &TomlSpan) -> TomlErr {
    TomlErr {
        msg,
        span: span.clone(),
    }
}

fn dotted(keys: &[Key]) -> String {
    keys.iter()
        .flat_map(|key| key.segments.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join(".")
}

/// Walk `path` from `root`, creating tables on the way, and return the table
/// the last segment names.
fn navigate<'a>(
    root: &'a mut NodeTable,
    path: &[Key],
    navigation: Navigation,
) -> Result<&'a mut NodeTable, TomlErr> {
    let mut current = root;
    for key in path {
        for segment in &key.segments {
            let origin = if navigation == Navigation::Header {
                Origin::Implicit
            } else {
                Origin::Dotted
            };
            let node = current.entry(segment.clone()).or_insert_with(|| Node::Table {
                map: BTreeMap::new(),
                origin,
            });
            let next = match node {
                Node::Table {
                    origin: Origin::Inline,
                    ..
                } => {
                    return Err(conflict(
                        format!("cannot extend inline table `{}`", segment),
                        &key.span,
                    ))
                }
                Node::Table { map, origin } => {
                    if navigation == Navigation::Dotted {
                        match origin {
                            // A table defined by its own header cannot be
                            // extended by dotted keys from another scope.
                            Origin::Header => {
                                return Err(conflict(
                                    format!(
                                        "cannot extend table `{}` defined by a header with dotted keys",
                                        segment
                                    ),
                                    &key.span,
                                ))
                            }
                            // Dotted traversal defines the table.
                            Origin::Implicit => *origin = Origin::Dotted,
                            Origin::Dotted | Origin::Inline => {}
                        }
                    }
                    map
                }
                Node::ArrayOfTables(tables) if navigation == Navigation::Header => tables
                    .last_mut()
                    .expect("an array of tables always has an element"),
                Node::ArrayOfTables(_) => {
                    return Err(conflict(
                        format!(
                            "cannot extend array of tables `{}` with a dotted key",
                            segment
                        ),
                        &key.span,
                    ))
                }
                Node::Value(_) => {
                    return Err(conflict(
                        format!("key `{}` is a value, not a table", segment),
                        &key.span,
                    ))
                }
            };
            current = next;
        }
    }
    Ok(current)
}

fn define_header(
    root: &mut NodeTable,
    path: &[Key],
    array: bool,
    span: &TomlSpan,
) -> Result<(), TomlErr> {
    let (last, parents) = path
        .split_last()
        .ok_or_else(|| conflict("empty table header".into(), span))?;
    let (last_name, last_parents) = last
        .segments
        .split_last()
        .ok_or_else(|| conflict("empty table header".into(), span))?;
    let parent = navigate(root, parents, Navigation::Header)?;
    let parent = navigate(
        parent,
        &[Key {
            segments: last_parents.to_vec(),
            span: last.span.clone(),
        }],
        Navigation::Header,
    )?;
    let name = dotted(path);
    match parent.get_mut(last_name) {
        None => {
            let node = if array {
                Node::ArrayOfTables(vec![BTreeMap::new()])
            } else {
                Node::Table {
                    map: BTreeMap::new(),
                    origin: Origin::Header,
                }
            };
            parent.insert(last_name.clone(), node);
            Ok(())
        }
        Some(Node::ArrayOfTables(tables)) if array => {
            tables.push(BTreeMap::new());
            Ok(())
        }
        Some(Node::Table { origin, .. }) if !array => match origin {
            Origin::Implicit => {
                *origin = Origin::Header;
                Ok(())
            }
            Origin::Header => Err(conflict(format!("table `{}` defined twice", name), span)),
            Origin::Dotted => Err(conflict(
                format!("table `{}` was already defined by dotted keys", name),
                span,
            )),
            Origin::Inline => Err(conflict(
                format!("table `{}` was already defined as an inline table", name),
                span,
            )),
        },
        Some(Node::ArrayOfTables(_)) => Err(conflict(
            format!("`{}` is an array of tables, not a table", name),
            span,
        )),
        Some(Node::Table { .. }) => Err(conflict(
            format!("`{}` is a table, not an array of tables", name),
            span,
        )),
        Some(Node::Value(_)) => Err(conflict(
            format!("`{}` is a value, not a table", name),
            span,
        )),
    }
}

fn insert_value(table: &mut NodeTable, key: &Key, value: Node) -> Result<(), TomlErr> {
    let (last, parents) = key
        .segments
        .split_last()
        .ok_or_else(|| conflict("empty key".into(), &key.span))?;
    let parent = navigate(
        table,
        &[Key {
            segments: parents.to_vec(),
            span: key.span.clone(),
        }],
        Navigation::Dotted,
    )?;
    if parent.contains_key(last) {
        return Err(conflict(
            format!("duplicate key `{}`", key.segments.join(".")),
            &key.span,
        ));
    }
    parent.insert(last.clone(), value);
    Ok(())
}

// --- scanner ----------------------------------------------------------------

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

fn is_bare_key_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

impl<'a> Parser<'a> {
    fn at_end(&self) -> bool {
        self.pos >= self.bytes.len()
    }
    fn peek(&self) -> u8 {
        self.bytes.get(self.pos).copied().unwrap_or(0)
    }
    fn peek_at(&self, offset: usize) -> u8 {
        self.bytes.get(self.pos + offset).copied().unwrap_or(0)
    }
    fn starts_with(&self, s: &str) -> bool {
        self.bytes[self.pos..].starts_with(s.as_bytes())
    }
    fn err(&self, msg: impl Into<String>, start: usize) -> TomlErr {
        TomlErr {
            msg: msg.into(),
            span: TomlSpan {
                start,
                len: self.pos.saturating_sub(start).max(1).min(self.bytes.len().saturating_sub(start)),
            },
        }
    }
    fn span_from(&self, start: usize) -> TomlSpan {
        TomlSpan {
            start,
            len: self.pos - start,
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), b' ' | b'\t') && !self.at_end() {
            self.pos += 1;
        }
    }
    fn skip_comment(&mut self) {
        if self.peek() == b'#' {
            while !self.at_end() && self.peek() != b'\n' {
                self.pos += 1;
            }
        }
    }
    /// Skip whitespace, comments and newlines.
    fn skip_blank_lines(&mut self) {
        loop {
            self.skip_ws();
            self.skip_comment();
            match self.peek() {
                b'\n' => self.pos += 1,
                b'\r' if self.peek_at(1) == b'\n' => self.pos += 2,
                _ => return,
            }
        }
    }
    /// After a value or header: only whitespace, a comment, a newline or the
    /// end of the text may follow.
    fn expect_line_end(&mut self) -> Result<(), TomlErr> {
        self.skip_ws();
        self.skip_comment();
        let start = self.pos;
        match self.peek() {
            _ if self.at_end() => Ok(()),
            b'\n' => {
                self.pos += 1;
                Ok(())
            }
            b'\r' if self.peek_at(1) == b'\n' => {
                self.pos += 2;
                Ok(())
            }
            _ => {
                self.pos += 1;
                Err(self.err("expected a newline after the value", start))
            }
        }
    }
    fn expect_byte(&mut self, byte: u8, msg: &str) -> Result<(), TomlErr> {
        let start = self.pos;
        if self.peek() == byte && !self.at_end() {
            self.pos += 1;
            Ok(())
        } else {
            self.pos = (self.pos + 1).min(self.bytes.len());
            Err(self.err(msg, start))
        }
    }

    // --- keys ---

    fn parse_simple_key(&mut self) -> Result<String, TomlErr> {
        let start = self.pos;
        match self.peek() {
            b'"' => {
                self.pos += 1;
                self.read_basic_string(false)
            }
            b'\'' => {
                self.pos += 1;
                self.read_literal_string(false)
            }
            b if is_bare_key_byte(b) => {
                while is_bare_key_byte(self.peek()) && !self.at_end() {
                    self.pos += 1;
                }
                Ok(self.src[start..self.pos].to_string())
            }
            _ => {
                self.pos = (self.pos + 1).min(self.bytes.len());
                Err(self.err("expected a key", start))
            }
        }
    }

    fn parse_key(&mut self) -> Result<Key, TomlErr> {
        let start = self.pos;
        let mut segments = vec![self.parse_simple_key()?];
        loop {
            let save = self.pos;
            self.skip_ws();
            if self.peek() == b'.' && !self.at_end() {
                self.pos += 1;
                self.skip_ws();
                segments.push(self.parse_simple_key()?);
            } else {
                self.pos = save;
                break;
            }
        }
        Ok(Key {
            segments,
            span: self.span_from(start),
        })
    }

    fn parse_header(&mut self) -> Result<Header, TomlErr> {
        let start = self.pos;
        self.expect_byte(b'[', "expected `[`")?;
        let array = self.peek() == b'[';
        if array {
            self.pos += 1;
        }
        self.skip_ws();
        let key = self.parse_key()?;
        self.skip_ws();
        self.expect_byte(b']', "expected `]` to close the table header")?;
        if array {
            self.expect_byte(b']', "expected `]]` to close the array-of-tables header")?;
        }
        Ok(Header {
            path: vec![key],
            array,
            span: self.span_from(start),
        })
    }

    // --- values ---

    fn parse_value(&mut self) -> Result<Node, TomlErr> {
        let start = self.pos;
        match self.peek() {
            b'"' => {
                self.pos += 1;
                let multiline = self.starts_with("\"\"");
                if multiline {
                    self.pos += 2;
                    self.skip_first_newline();
                }
                let value = self.read_basic_string(multiline)?;
                Ok(Node::Value(Toml::Str(value, self.span_from(start))))
            }
            b'\'' => {
                self.pos += 1;
                let multiline = self.starts_with("''");
                if multiline {
                    self.pos += 2;
                    self.skip_first_newline();
                }
                let value = self.read_literal_string(multiline)?;
                Ok(Node::Value(Toml::Str(value, self.span_from(start))))
            }
            b'[' => self.parse_array().map(Node::Value),
            b'{' => self.parse_inline_table(),
            b't' if self.starts_with("true") && !is_bare_key_byte(self.peek_at(4)) => {
                self.pos += 4;
                Ok(Node::Value(Toml::Bool(true, self.span_from(start))))
            }
            b'f' if self.starts_with("false") && !is_bare_key_byte(self.peek_at(5)) => {
                self.pos += 5;
                Ok(Node::Value(Toml::Bool(false, self.span_from(start))))
            }
            b'+' | b'-' | b'0'..=b'9' | b'i' | b'n' => {
                if self.looks_like_date_time() {
                    return self.parse_date_time().map(Node::Value);
                }
                self.parse_number().map(Node::Value)
            }
            _ => {
                self.pos = (self.pos + 1).min(self.bytes.len());
                Err(self.err("expected a value", start))
            }
        }
    }

    fn skip_first_newline(&mut self) {
        if self.peek() == b'\r' && self.peek_at(1) == b'\n' {
            self.pos += 2;
        } else if self.peek() == b'\n' {
            self.pos += 1;
        }
    }

    fn parse_array(&mut self) -> Result<Toml, TomlErr> {
        let start = self.pos;
        self.expect_byte(b'[', "expected `[`")?;
        let mut values = Vec::new();
        loop {
            self.skip_blank_lines();
            if self.at_end() {
                return Err(self.err("unterminated array", start));
            }
            if self.peek() == b']' {
                self.pos += 1;
                return Ok(Toml::Array(values));
            }
            let node = self.parse_value()?;
            values.push(node_to_toml(node));
            self.skip_blank_lines();
            match self.peek() {
                b',' => self.pos += 1,
                b']' => {}
                _ => {
                    let here = self.pos;
                    self.pos = (self.pos + 1).min(self.bytes.len());
                    return Err(self.err("expected `,` or `]` in array", here));
                }
            }
        }
    }

    /// `{ key = value, key = value }` on one line: only horizontal whitespace
    /// between the parts, no trailing comma, no comments; a value may itself
    /// span lines (a multi-line string or an array).
    fn parse_inline_table(&mut self) -> Result<Node, TomlErr> {
        let start = self.pos;
        self.expect_byte(b'{', "expected `{`")?;
        let mut map: NodeTable = BTreeMap::new();
        self.skip_ws();
        if self.peek() == b'}' && !self.at_end() {
            self.pos += 1;
            return Ok(Node::Table {
                map,
                origin: Origin::Inline,
            });
        }
        loop {
            if self.at_end() {
                return Err(self.err("unterminated inline table", start));
            }
            let key = self.parse_key()?;
            self.skip_ws();
            self.expect_byte(b'=', "expected `=` after key")?;
            self.skip_ws();
            let value = self.parse_value()?;
            insert_value(&mut map, &key, value)?;
            self.skip_ws();
            match self.peek() {
                b',' => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.peek() == b'}' {
                        let here = self.pos;
                        self.pos += 1;
                        return Err(self.err("trailing comma in inline table", here));
                    }
                }
                b'}' => {
                    self.pos += 1;
                    return Ok(Node::Table {
                        map,
                        origin: Origin::Inline,
                    });
                }
                _ => {
                    let here = self.pos;
                    self.pos = (self.pos + 1).min(self.bytes.len());
                    return Err(self.err(
                        "expected `,` or `}` in inline table (entries stay on one line)",
                        here,
                    ));
                }
            }
        }
    }

    // --- scalars ---

    fn looks_like_date_time(&self) -> bool {
        let digits = |n: usize| (0..n).all(|i| self.peek_at(i).is_ascii_digit());
        (digits(4) && self.peek_at(4) == b'-') || (digits(2) && self.peek_at(2) == b':')
    }

    fn parse_date_time(&mut self) -> Result<Toml, TomlErr> {
        let start = self.pos;
        let allowed = |b: u8| b.is_ascii_digit() || matches!(b, b'-' | b':' | b'.' | b'+' | b'T' | b't' | b'Z' | b'z');
        while allowed(self.peek()) && !self.at_end() {
            self.pos += 1;
        }
        // A space may separate the date from the time (`1979-05-27 07:32:00`).
        if self.peek() == b' ' && self.peek_at(1).is_ascii_digit() && self.peek_at(2).is_ascii_digit() && self.peek_at(3) == b':' {
            self.pos += 1;
            while allowed(self.peek()) && !self.at_end() {
                self.pos += 1;
            }
        }
        let text = &self.src[start..self.pos];
        if !valid_date_time(text.as_bytes()) {
            return Err(self.err("invalid date or time", start));
        }
        Ok(Toml::Date(text.to_string(), self.span_from(start)))
    }

    fn parse_number(&mut self) -> Result<Toml, TomlErr> {
        let start = self.pos;
        let mut negative = false;
        match self.peek() {
            b'+' => self.pos += 1,
            b'-' => {
                negative = true;
                self.pos += 1;
            }
            _ => {}
        }
        let sign = if negative { -1.0 } else { 1.0 };
        if self.starts_with("inf") {
            self.pos += 3;
            return Ok(Toml::Num(sign * f64::INFINITY, self.span_from(start)));
        }
        if self.starts_with("nan") {
            self.pos += 3;
            return Ok(Toml::Num(sign * f64::NAN, self.span_from(start)));
        }
        if self.peek() == b'0' && matches!(self.peek_at(1), b'x' | b'o' | b'b') {
            if self.pos != start {
                self.pos += 2;
                return Err(self.err("a radix integer cannot carry a sign", start));
            }
            let radix = match self.peek_at(1) {
                b'x' => 16,
                b'o' => 8,
                _ => 2,
            };
            self.pos += 2;
            let digits = self.read_digits(|b| (b as char).is_digit(radix));
            if digits.is_empty() {
                return Err(self.err("expected digits after radix prefix", start));
            }
            let value = u64::from_str_radix(&digits, radix)
                .map_err(|_| self.err("integer out of range", start))?;
            return Ok(Toml::Num(sign * value as f64, self.span_from(start)));
        }
        let mut text = self.read_digits(|b| b.is_ascii_digit());
        if text.is_empty() {
            self.pos = (self.pos + 1).min(self.bytes.len());
            return Err(self.err("expected a number", start));
        }
        if text.len() > 1 && text.starts_with('0') {
            return Err(self.err("leading zeros are not allowed", start));
        }
        let mut is_float = false;
        if self.peek() == b'.' && self.peek_at(1).is_ascii_digit() {
            self.pos += 1;
            is_float = true;
            text.push('.');
            text.push_str(&self.read_digits(|b| b.is_ascii_digit()));
        }
        if matches!(self.peek(), b'e' | b'E') {
            let save = self.pos;
            self.pos += 1;
            let mut exponent = String::from("e");
            if matches!(self.peek(), b'+' | b'-') {
                exponent.push(self.peek() as char);
                self.pos += 1;
            }
            let digits = self.read_digits(|b| b.is_ascii_digit());
            if digits.is_empty() {
                self.pos = save;
            } else {
                is_float = true;
                exponent.push_str(&digits);
                text.push_str(&exponent);
            }
        }
        let value = if is_float {
            text.parse::<f64>()
                .map_err(|_| self.err("invalid float", start))?
        } else {
            text.parse::<u64>()
                .map(|v| v as f64)
                .map_err(|_| self.err("integer out of range", start))?
        };
        Ok(Toml::Num(sign * value, self.span_from(start)))
    }

    /// Read digits accepted by `accept`, dropping `_` separators.
    fn read_digits(&mut self, accept: impl Fn(u8) -> bool) -> String {
        let mut out = String::new();
        while !self.at_end() {
            let b = self.peek();
            if accept(b) {
                out.push(b as char);
            } else if b == b'_' && !out.is_empty() && accept(self.peek_at(1)) {
                // separator between digits
            } else {
                break;
            }
            self.pos += 1;
        }
        out
    }

    // --- strings (the opening delimiter has been consumed) ---

    fn read_basic_string(&mut self, multiline: bool) -> Result<String, TomlErr> {
        let start = self.pos;
        let mut out = String::new();
        loop {
            if self.at_end() {
                return Err(self.err("unterminated string", start));
            }
            let b = self.peek();
            match b {
                b'"' => {
                    if !multiline {
                        self.pos += 1;
                        return Ok(out);
                    }
                    let mut quotes = 0;
                    while self.peek() == b'"' && !self.at_end() {
                        quotes += 1;
                        self.pos += 1;
                    }
                    if quotes > 5 {
                        return Err(self.err("too many quotes closing the string", start));
                    }
                    if quotes >= 3 {
                        for _ in 0..quotes - 3 {
                            out.push('"');
                        }
                        return Ok(out);
                    }
                    for _ in 0..quotes {
                        out.push('"');
                    }
                }
                b'\\' => {
                    self.pos += 1;
                    let escape_start = self.pos;
                    match self.peek() {
                        b'b' => {
                            out.push('\u{0008}');
                            self.pos += 1;
                        }
                        b't' => {
                            out.push('\t');
                            self.pos += 1;
                        }
                        b'n' => {
                            out.push('\n');
                            self.pos += 1;
                        }
                        b'f' => {
                            out.push('\u{000C}');
                            self.pos += 1;
                        }
                        b'r' => {
                            out.push('\r');
                            self.pos += 1;
                        }
                        b'"' => {
                            out.push('"');
                            self.pos += 1;
                        }
                        b'\\' => {
                            out.push('\\');
                            self.pos += 1;
                        }
                        b'u' | b'U' => {
                            let digits = if self.peek() == b'u' { 4 } else { 8 };
                            self.pos += 1;
                            let mut code: u32 = 0;
                            for _ in 0..digits {
                                let d = (self.peek() as char)
                                    .to_digit(16)
                                    .ok_or_else(|| self.err("invalid unicode escape", escape_start))?;
                                code = code * 16 + d;
                                self.pos += 1;
                            }
                            out.push(
                                char::from_u32(code)
                                    .ok_or_else(|| self.err("invalid unicode escape", escape_start))?,
                            );
                        }
                        // Line-ending backslash: only whitespace may follow
                        // on the line, then all whitespace and newlines up to
                        // the next content are trimmed.
                        b' ' | b'\t' | b'\n' | b'\r' if multiline => {
                            while matches!(self.peek(), b' ' | b'\t') && !self.at_end() {
                                self.pos += 1;
                            }
                            let newline = match self.peek() {
                                b'\n' => 1,
                                b'\r' if self.peek_at(1) == b'\n' => 2,
                                _ => {
                                    return Err(self.err(
                                        "a line-ending backslash must be followed by a newline",
                                        escape_start,
                                    ))
                                }
                            };
                            self.pos += newline;
                            while matches!(self.peek(), b' ' | b'\t' | b'\n' | b'\r') && !self.at_end() {
                                self.pos += 1;
                            }
                        }
                        _ => return Err(self.err("invalid escape sequence", escape_start)),
                    }
                }
                b'\n' | b'\r' if !multiline => {
                    return Err(self.err("newline in single-line string", start))
                }
                b if is_forbidden_control(b, multiline) => {
                    return Err(self.err("control character in string", self.pos))
                }
                _ => {
                    let ch = self.src[self.pos..].chars().next().unwrap_or('\0');
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    fn read_literal_string(&mut self, multiline: bool) -> Result<String, TomlErr> {
        let start = self.pos;
        let mut out = String::new();
        loop {
            if self.at_end() {
                return Err(self.err("unterminated string", start));
            }
            match self.peek() {
                b'\'' => {
                    if !multiline {
                        self.pos += 1;
                        return Ok(out);
                    }
                    let mut quotes = 0;
                    while self.peek() == b'\'' && !self.at_end() {
                        quotes += 1;
                        self.pos += 1;
                    }
                    if quotes > 5 {
                        return Err(self.err("too many quotes closing the string", start));
                    }
                    if quotes >= 3 {
                        for _ in 0..quotes - 3 {
                            out.push('\'');
                        }
                        return Ok(out);
                    }
                    for _ in 0..quotes {
                        out.push('\'');
                    }
                }
                b'\n' | b'\r' if !multiline => {
                    return Err(self.err("newline in single-line string", start))
                }
                b if is_forbidden_control(b, multiline) => {
                    return Err(self.err("control character in string", self.pos))
                }
                _ => {
                    let ch = self.src[self.pos..].chars().next().unwrap_or('\0');
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }
}

/// Control characters TOML forbids raw inside strings: everything below
/// 0x20 except tab (and the newline bytes inside multi-line strings), plus
/// DEL.
fn is_forbidden_control(b: u8, multiline: bool) -> bool {
    match b {
        b'\t' => false,
        b'\n' | b'\r' => !multiline,
        0x7f => true,
        _ => b < 0x20,
    }
}

/// Shape and range check for an offset date-time, local date-time, local
/// date or local time, as written.
fn valid_date_time(text: &[u8]) -> bool {
    fn digits(text: &[u8], at: usize, n: usize) -> Option<u32> {
        let slice = text.get(at..at + n)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(slice).ok()?.parse().ok()
    }
    fn time(text: &[u8], at: usize) -> Option<usize> {
        let hour = digits(text, at, 2)?;
        if text.get(at + 2) != Some(&b':') {
            return None;
        }
        let minute = digits(text, at + 3, 2)?;
        if text.get(at + 5) != Some(&b':') {
            return None;
        }
        let second = digits(text, at + 6, 2)?;
        if hour > 23 || minute > 59 || second > 60 {
            return None;
        }
        let mut end = at + 8;
        if text.get(end) == Some(&b'.') {
            let start = end + 1;
            let mut frac = start;
            while text.get(frac).is_some_and(u8::is_ascii_digit) {
                frac += 1;
            }
            if frac == start {
                return None;
            }
            end = frac;
        }
        Some(end)
    }
    fn date(text: &[u8]) -> Option<usize> {
        let year = digits(text, 0, 4)?;
        if text.get(4) != Some(&b'-') {
            return None;
        }
        let month = digits(text, 5, 2)?;
        if text.get(7) != Some(&b'-') {
            return None;
        }
        let day = digits(text, 8, 2)?;
        let _ = year;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        Some(10)
    }
    fn offset(text: &[u8], at: usize) -> Option<usize> {
        match text.get(at) {
            None => Some(at),
            Some(b'Z') | Some(b'z') => Some(at + 1),
            Some(b'+') | Some(b'-') => {
                let hour = digits(text, at + 1, 2)?;
                if text.get(at + 3) != Some(&b':') {
                    return None;
                }
                let minute = digits(text, at + 4, 2)?;
                if hour > 23 || minute > 59 {
                    return None;
                }
                Some(at + 6)
            }
            _ => None,
        }
    }
    if let Some(end) = date(text) {
        if end == text.len() {
            return true;
        }
        if !matches!(text.get(end), Some(b'T') | Some(b't') | Some(b' ')) {
            return false;
        }
        let Some(end) = time(text, end + 1) else {
            return false;
        };
        return offset(text, end) == Some(text.len());
    }
    time(text, 0) == Some(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_str<'a>(doc: &'a TomlDocument, path: &[&str]) -> Option<&'a str> {
        doc.get_path(path).and_then(Toml::as_str)
    }

    #[test]
    fn parses_single_line_strings() {
        let doc = parse_toml("[package]\nname = \"robrix\"\nx = 'literal'\n").unwrap();
        assert_eq!(get_str(&doc, &["package", "name"]), Some("robrix"));
        assert_eq!(get_str(&doc, &["package", "x"]), Some("literal"));
    }

    #[test]
    fn parses_multiline_basic_string_and_following_keys() {
        let toml = "[package]\n\
                    version = \"1.0.0-alpha.1\"\n\
                    long_description = \"\"\"\n\
                    line one\n\
                    line two\n\
                    \"\"\"\n\
                    identifier = \"rs.robius.robrix\"\n";
        let doc = parse_toml(toml).expect("multi-line basic string should parse");
        assert_eq!(get_str(&doc, &["package", "version"]), Some("1.0.0-alpha.1"));
        assert_eq!(get_str(&doc, &["package", "identifier"]), Some("rs.robius.robrix"));
        assert_eq!(
            get_str(&doc, &["package", "long_description"]),
            Some("line one\nline two\n")
        );
    }

    #[test]
    fn parses_multiline_literal_string_and_line_ending_backslash() {
        let toml = "[package]\n\
                    cmd = '''\n\
                    raw \\n not an escape\n\
                    '''\n\
                    joined = \"\"\"one \\\n    two\"\"\"\n\
                    after = \"ok\"\n";
        let doc = parse_toml(toml).expect("multi-line literal string should parse");
        assert_eq!(get_str(&doc, &["package", "cmd"]), Some("raw \\n not an escape\n"));
        assert_eq!(get_str(&doc, &["package", "joined"]), Some("one two"));
        assert_eq!(get_str(&doc, &["package", "after"]), Some("ok"));
    }

    #[test]
    fn quoted_header_segments_stay_indivisible() {
        let toml = "[patch.\"https://github.com/kevinaboos/makepad\"]\n\
                    makepad-widgets = \"1.0\"\n\
                    [target.'cfg(target_os = \"ios\")'.dependencies]\n\
                    foo = \"2.0\"\n\
                    [package.metadata.packager]\n\
                    identifier = \"rs.robius.robrix\"\n";
        let doc = parse_toml(toml).expect("quoted section headers should parse");
        assert_eq!(
            get_str(&doc, &["package", "metadata", "packager", "identifier"]),
            Some("rs.robius.robrix")
        );
        assert_eq!(
            get_str(
                &doc,
                &["patch", "https://github.com/kevinaboos/makepad", "makepad-widgets"]
            ),
            Some("1.0")
        );
        assert_eq!(
            get_str(&doc, &["target", "cfg(target_os = \"ios\")", "dependencies", "foo"]),
            Some("2.0")
        );
    }

    #[test]
    fn basic_string_escapes_are_processed() {
        let doc = parse_toml("[s]\na = \"tab\\there\"\nb = \"q\\\"q\"\nc = \"\\u00e9\"\n").unwrap();
        assert_eq!(get_str(&doc, &["s", "a"]), Some("tab\there"));
        assert_eq!(get_str(&doc, &["s", "b"]), Some("q\"q"));
        assert_eq!(get_str(&doc, &["s", "c"]), Some("é"));
    }

    #[test]
    fn repeated_bins_are_an_array_of_tables() {
        let toml = "[package]\nname = \"makepad-studio\"\n\
                    [[bin]]\nname = \"studio\"\npath = \"src/main.rs\"\n\
                    [[bin]]\nname = \"studio-git-guard\"\ntest = false\n\
                    [[bin]]\nname = \"studio-rustc-guard\"\n\
                    [[bin]]\nname = \"studio-flow\"\n";
        let doc = parse_toml(toml).unwrap();
        let bins = doc.get_path(&["bin"]).and_then(Toml::as_array_of_tables).unwrap();
        let names: Vec<&str> = bins.iter().map(|b| b["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            ["studio", "studio-git-guard", "studio-rustc-guard", "studio-flow"]
        );
        assert_eq!(bins[1]["test"].as_bool(), Some(false));
        assert_eq!(get_str(&doc, &["bin", "name"]), Some("studio-flow"));
    }

    #[test]
    fn nested_arrays_of_tables_attach_to_the_latest_element() {
        let toml = "[[fruits]]\nname = \"apple\"\n\
                    [fruits.physical]\ncolor = \"red\"\n\
                    [[fruits.varieties]]\nname = \"red delicious\"\n\
                    [[fruits.varieties]]\nname = \"granny smith\"\n\
                    [[fruits]]\nname = \"banana\"\n\
                    [[fruits.varieties]]\nname = \"plantain\"\n";
        let doc = parse_toml(toml).unwrap();
        let fruits = doc.get_path(&["fruits"]).and_then(Toml::as_array_of_tables).unwrap();
        assert_eq!(fruits.len(), 2);
        assert_eq!(fruits[0]["name"].as_str(), Some("apple"));
        assert_eq!(
            fruits[0]["physical"].as_table().unwrap()["color"].as_str(),
            Some("red")
        );
        let apple_varieties = fruits[0]["varieties"].as_array_of_tables().unwrap();
        assert_eq!(apple_varieties.len(), 2);
        assert_eq!(apple_varieties[1]["name"].as_str(), Some("granny smith"));
        let banana_varieties = fruits[1]["varieties"].as_array_of_tables().unwrap();
        assert_eq!(banana_varieties.len(), 1);
        assert_eq!(banana_varieties[0]["name"].as_str(), Some("plantain"));
    }

    #[test]
    fn dotted_keys_nest_and_quoted_keys_do_not_split() {
        let toml = "a.b.c = 1\na.b.d = \"x\"\n\"e.f\" = true\n[package]\nmetadata.auto = \"h\"\n";
        let doc = parse_toml(toml).unwrap();
        assert_eq!(doc.get_path(&["a", "b", "c"]).and_then(Toml::as_num), Some(1.0));
        assert_eq!(get_str(&doc, &["a", "b", "d"]), Some("x"));
        assert_eq!(doc.get_path(&["e.f"]).and_then(Toml::as_bool), Some(true));
        assert_eq!(get_str(&doc, &["package", "metadata", "auto"]), Some("h"));
    }

    #[test]
    fn inline_tables_and_arrays() {
        let toml = "deps = { a = \"1\", b = { path = \"../b\", version = \"2\" } }\n\
                    resources = [{ src = \"x\" }, { src = \"y\" },]\n\
                    mixed = [1, 2.5, \"s\", [true, false], # comment\n  0x10, 1_000, -3, +4, 6.02e23, inf, -nan]\n";
        let doc = parse_toml(toml).unwrap();
        assert_eq!(get_str(&doc, &["deps", "a"]), Some("1"));
        assert_eq!(get_str(&doc, &["deps", "b", "version"]), Some("2"));
        let resources = doc.get_path(&["resources"]).and_then(Toml::as_array).unwrap();
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[1].as_table().unwrap()["src"].as_str(), Some("y"));
        let mixed = doc.get_path(&["mixed"]).and_then(Toml::as_array).unwrap();
        assert_eq!(mixed.len(), 11);
        assert_eq!(mixed[0].as_num(), Some(1.0));
        assert_eq!(mixed[1].as_num(), Some(2.5));
        assert_eq!(mixed[2].as_str(), Some("s"));
        assert_eq!(mixed[3].as_array().unwrap().len(), 2);
        assert_eq!(mixed[4].as_num(), Some(16.0));
        assert_eq!(mixed[5].as_num(), Some(1000.0));
        assert_eq!(mixed[6].as_num(), Some(-3.0));
        assert_eq!(mixed[7].as_num(), Some(4.0));
        assert_eq!(mixed[8].as_num(), Some(6.02e23));
        assert_eq!(mixed[9].as_num(), Some(f64::INFINITY));
        assert!(mixed[10].as_num().unwrap().is_nan());
    }

    #[test]
    fn dates_and_times_are_kept_as_text() {
        let toml = "a = 1979-05-27T07:32:00Z\nb = 1979-05-27 07:32:00.999-07:00\nc = 1979-05-27\nd = 07:32:00\n";
        let doc = parse_toml(toml).unwrap();
        let date = |k| match doc.get_path(&[k]) {
            Some(Toml::Date(v, _)) => v.clone(),
            other => panic!("{k}: {other:?}"),
        };
        assert_eq!(date("a"), "1979-05-27T07:32:00Z");
        assert_eq!(date("b"), "1979-05-27 07:32:00.999-07:00");
        assert_eq!(date("c"), "1979-05-27");
        assert_eq!(date("d"), "07:32:00");
    }

    #[test]
    fn spans_are_zero_based_byte_offsets_over_the_whole_lexeme() {
        let toml = "name = \"é\"\nversion = \"1.2.3\"\nn = 42\n";
        let doc = parse_toml(toml).unwrap();
        let version = doc.get_path(&["version"]).unwrap().span().unwrap().clone();
        assert_eq!(&toml[version.start..version.end()], "\"1.2.3\"");
        let n = doc.get_path(&["n"]).unwrap().span().unwrap().clone();
        assert_eq!(&toml[n.start..n.end()], "42");
        let name = doc.get_path(&["name"]).unwrap().span().unwrap().clone();
        assert_eq!(&toml[name.start..name.end()], "\"é\"");
    }

    #[test]
    fn duplicate_keys_and_conflicts_are_errors() {
        assert!(parse_toml("a = 1\na = 2\n").is_err());
        assert!(parse_toml("a = 1\n[a]\n").is_err());
        assert!(parse_toml("[a]\n[a]\n").is_err());
        assert!(parse_toml("[[a]]\n[a]\n").is_err());
        assert!(parse_toml("[a]\n[[a]]\n").is_err());
        assert!(parse_toml("a = { b = 1 }\n[a.c]\n").is_err());
        assert!(parse_toml("a = { b = 1 }\na.c = 2\n").is_err());
        assert!(parse_toml("a.b = 1\n[a]\n").is_err());
        assert!(parse_toml("x = 1 y = 2\n").is_err());
        assert!(parse_toml("x = \"unterminated\n").is_err());
        let err = parse_toml("first = 1\nsecond = 2\nfirst = 3\n").unwrap_err();
        assert_eq!(err.span.start, "first = 1\nsecond = 2\n".len());
    }

    #[test]
    fn table_provenance_is_enforced_in_both_orders() {
        // A header-defined table cannot be extended by dotted keys later.
        assert!(parse_toml("[a.b]\nx = 1\n[a]\nb.y = 2\n").is_err());
        // A dotted-key-defined table cannot be redefined by a header.
        assert!(parse_toml("a.b = 1\n[a.b]\n").is_err());
        // Dotted traversal defines the implicit table for good.
        assert!(parse_toml("a.x = 1\n[a.b]\n[a]\n").is_err());
        // Sub-tables under a dotted table are still allowed by header.
        assert!(parse_toml("[fruit]\napple.taste.sweet = true\n[fruit.apple.texture]\nsmooth = true\n").is_ok());
        // Implicit tables may be defined by a header afterwards.
        assert!(parse_toml("[a.b.c]\n[a]\n[a.b]\n").is_ok());
        // ... but only once.
        assert!(parse_toml("[a.b.c]\n[a]\n[a]\n").is_err());
    }

    #[test]
    fn inline_tables_are_single_line_without_trailing_comma() {
        assert!(parse_toml("t = { a = 1, }\n").is_err());
        assert!(parse_toml("t = { a = 1,\n b = 2 }\n").is_err());
        assert!(parse_toml("t = { a = 1 # c\n }\n").is_err());
        assert!(parse_toml("t = {\n}\n").is_err());
        let doc = parse_toml("t = { a = 1, b = \"\"\"\nmulti\n\"\"\", c = [\n1,\n2\n] }\n").unwrap();
        assert_eq!(doc.get_path(&["t", "b"]).and_then(Toml::as_str), Some("multi\n"));
        assert_eq!(doc.get_path(&["t", "c"]).and_then(Toml::as_array).map(<[Toml]>::len), Some(2));
        assert!(parse_toml("t = {}\n").unwrap().get_path(&["t"]).and_then(Toml::as_table).is_some());
    }

    #[test]
    fn malformed_scalars_are_errors() {
        // Line-ending backslash without a newline.
        assert!(parse_toml("s = \"\"\"a\\ b\"\"\"\n").is_err());
        assert!(parse_toml("s = \"\"\"a \\\n   b\"\"\"\n").is_ok());
        // Raw control characters.
        assert!(parse_toml("s = \"a\u{1}b\"\n").is_err());
        assert!(parse_toml("s = 'a\u{7f}b'\n").is_err());
        assert!(parse_toml("s = \"a\tb\"\n").is_ok());
        // Excess closing quotes.
        assert!(parse_toml("s = \"\"\"a\"\"\"\"\"\"\n").is_err());
        assert!(parse_toml("s = '''a''''''\n").is_err());
        assert_eq!(parse_toml("s = \"\"\"a\"\"\"\"\"\n").unwrap().get_path(&["s"]).and_then(Toml::as_str), Some("a\"\""));
        // Numbers.
        assert!(parse_toml("n = 01\n").is_err());
        assert!(parse_toml("n = 00.5\n").is_err());
        assert!(parse_toml("n = +0x1\n").is_err());
        assert!(parse_toml("n = -0b1\n").is_err());
        assert!(parse_toml("n = 1__0\n").is_err());
        assert!(parse_toml("n = 0\nm = 0.5\nk = 0e3\n").is_ok());
        // Dates.
        assert!(parse_toml("d = 1234-\n").is_err());
        assert!(parse_toml("d = 1979-13-01\n").is_err());
        assert!(parse_toml("d = 1979-05-27T07:32\n").is_err());
        assert!(parse_toml("d = 07:32\n").is_err());
        assert!(parse_toml("d = 1979-05-27T07:32:00+07\n").is_err());
        assert!(parse_toml("d = 1979-05-27T07:32:00.5+07:00\n").is_ok());
    }

    #[test]
    fn comments_blank_lines_and_crlf() {
        let toml = "# leading\r\n\r\n[a] # trailing\r\nb = 1 # after value\r\n\r\n";
        let doc = parse_toml(toml).unwrap();
        assert_eq!(doc.get_path(&["a", "b"]).and_then(Toml::as_num), Some(1.0));
    }
}
