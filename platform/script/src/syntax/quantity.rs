//! Read arithmetic in a display unit and write its meaning in standard Splash units.

use super::{parse_value_with, Grouped, Kind, Node, SyntaxError};
use crate::suggest::levenshtein;
use crate::tokenizer::{lex_with, ScriptTokenTag};
use crate::units::{UnitEntry, UnitTable};
use std::borrow::Cow;
use std::ops::Range;

/// What a value box holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Measure {
    /// A number with no physical dimension.
    Plain,
    /// A distance, in metres in a script.
    Length,
    /// An angle, in radians in a script.
    Angle,
    /// A duration, in seconds in a script.
    Time,
    /// A dimensionless factor, displayed as a percentage.
    Fraction,
}

impl Measure {
    fn of(unit: &str) -> Self {
        match unit {
            "mm" | "cm" | "m" | "in" | "ft" => Self::Length,
            "deg" | "°" | "rad" => Self::Angle,
            "s" | "ms" => Self::Time,
            "%" => Self::Fraction,
            _ => Self::Plain,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Plain => "a plain number",
            Self::Length => "a length",
            Self::Angle => "an angle",
            Self::Time => "a time",
            Self::Fraction => "a percentage",
        }
    }

    fn physical(self) -> bool {
        matches!(self, Self::Length | Self::Angle | Self::Time)
    }
}

fn base_units() -> UnitTable {
    UnitTable::standard().with("in", 254.0, 10000.0).with("ft", 3048.0, 10000.0).with("%", 1.0, 100.0)
}

/// How a box reads text and displays the result.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    /// The quantity the box accepts.
    pub measure: Measure,
    /// The suffix as it appears beside a number.
    pub unit: String,
    /// Base units per display unit (metres, radians, seconds or a fraction).
    pub scale: f64,
}

impl Reading {
    /// Read the display suffix, ignoring surrounding whitespace. An unrecognised suffix labels a plain number.
    pub fn from_suffix(suffix: &str) -> Self {
        let unit = suffix.trim();
        let measure = Measure::of(unit);
        let scale = if measure == Measure::Plain {
            1.0
        } else {
            let units = base_units();
            units.entry(units.lookup(unit).unwrap()).to_base(1.0)
        };
        Self { measure, unit: unit.to_owned(), scale }
    }

    fn units(&self) -> UnitTable {
        let base = base_units();
        if !self.measure.physical() {
            return if self.measure == Measure::Plain && !self.unit.is_empty() {
                base.with(&self.unit, 1.0, 1.0)
            } else {
                base
            };
        }
        let mut units = base.clone();
        let (mul, div) = base.lookup(&self.unit).map(|ix| base.entry(ix))
            .filter(|unit| unit.to_base(1.0) == self.scale)
            .map_or((self.scale, 1.0), |unit| (unit.mul, unit.div));
        for entry in base.entries().iter().filter(|entry| Measure::of(&entry.suffix) == self.measure) {
            let (a, b) = if entry.mul == mul && entry.div == div {
                (1.0, 1.0)
            } else {
                (entry.mul * div, entry.div * mul)
            };
            units = units.with(&entry.suffix, a, b);
        }
        units
    }

    // How one display unit is written in a script using Splash's standard table.
    fn script_unit(&self) -> (f64, &str) {
        match self.measure {
            Measure::Fraction => (self.scale, ""),
            Measure::Length if matches!(self.unit.as_str(), "in" | "ft") => (self.scale, "m"),
            _ => (1.0, &self.unit),
        }
    }
}

/// A name the host makes available while reading the expression.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedValue {
    /// The identifier as written in a script.
    pub name: String,
    /// Its label as shown to the person editing the value.
    pub label: String,
    /// Its value in base units, as held by Splash.
    pub value: f64,
    /// The meaning of that value.
    pub measure: Measure,
}

/// The preview and the script that produces it.
#[derive(Clone, Debug, PartialEq)]
pub struct Reckoned {
    /// The result in the box's display unit.
    pub value: f64,
    /// A single number, optionally negated, bare or in the box's own unit.
    pub plain: bool,
    /// The names read, once each in first-use order.
    pub names: Vec<String>,
    /// The expression with its units made explicit for a standard Splash script.
    pub script: String,
}

/// What stopped the reading and where to select it in the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReckonError {
    /// Byte range in the original text.
    pub span: Range<usize>,
    /// A short explanation to show beside the box.
    pub message: String,
}

impl ReckonError {
    fn new(span: Range<usize>, message: impl Into<String>) -> Self {
        Self { span, message: message.into() }
    }
}

fn suffix<'a>(literal: &str, units: &'a UnitTable) -> Option<&'a UnitEntry> {
    units.entries().iter()
        .filter(|u| u.suffix.len() < literal.trim_start_matches('-').len() && literal.ends_with(&u.suffix))
        .max_by_key(|u| u.suffix.len())
}

fn identifier_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '$')
}

// Ordinary arithmetic goes straight to the parser for a single lex. If there could be a
// separated unit, a prefix '+', or characters the tokenizer discards, inspect the tokens.
fn needs_preparation(text: &str) -> bool {
    let mut previous = ' ';
    let mut separated = true;
    for c in text.chars() {
        if c.is_whitespace() {
            separated = true;
            continue;
        }
        let ordinary = match c {
            '+' => identifier_char(previous) || matches!(previous, ')' | '.' | '%'),
            '*' | '/' => separated || previous != '/', // comment openers
            'a'..='z' | 'A'..='Z' | '%' => !separated || !(identifier_char(previous) || previous == '.'),
            '0'..='9' | '_' | '$' | '.' | '-' | '(' | ')' => true,
            _ => false,
        };
        if !ordinary { return true; }
        previous = c;
        separated = false;
    }
    false
}

// Spaces inside a literal can be numeric separators in the parse copy. Even a multibyte
// display suffix can be blanked with underscores. Every token boundary keeps its byte offset.
fn prepare<'a>(text: &'a str, reading: &Reading, units: &UnitTable) -> Result<Cow<'a, str>, ReckonError> {
    if (reading.measure != Measure::Plain || reading.unit.is_empty()) && !needs_preparation(text) {
        return Ok(Cow::Borrowed(text));
    }
    let mut bytes = text.as_bytes().to_vec();
    let mut end = 0;
    let mut expecting_operand = true;
    let check_gap = |span: Range<usize>| {
        if let Some((offset, c)) = text[span.clone()].char_indices().find(|(_, c)| !c.is_whitespace()) {
            let start = span.start + offset;
            return Err(ReckonError::new(start..start + c.len_utf8(), format!("`{c}` isn't part of a number")));
        }
        Ok(())
    };
    for token in lex_with(text, units) {
        let span = token.span();
        if span.end <= end {
            continue; // A suffix already consumed with its number.
        }
        if span.start < end {
            return Err(ReckonError::new(span.clone(), format!("unexpected `{}` after the number", &text[span])));
        }
        check_gap(end..span.start)?;
        end = span.end;
        let word = &text[span.clone()];
        if token.token().number_f64().is_some() {
            let digits_end = suffix(word, units).map_or(end, |u| end - u.suffix.len());
            let rest = &text[digits_end..];
            let joined = rest.trim_start_matches(' ');
            let start = text.len() - joined.len();
            if let Some(unit) = units.entries().iter()
                .filter(|u| joined.strip_prefix(&u.suffix).is_some_and(|tail| !tail.starts_with(identifier_char)))
                .max_by_key(|u| u.suffix.len())
            {
                let unit_end = start + unit.suffix.len();
                // With a right operand, a separate '%' is the modulo operator.
                let tail = text[unit_end..].trim_start();
                let modulo = unit.suffix == "%" && start != digits_end
                    && tail.starts_with(|c: char| identifier_char(c) || matches!(c, '(' | '.' | '='));
                if !modulo {
                    bytes[digits_end..start].fill(b'_');
                    if reading.measure == Measure::Plain && unit.suffix == reading.unit {
                        bytes[start..unit_end].fill(b'_');
                    }
                    end = unit_end;
                }
            }
            expecting_operand = false;
        } else {
            if word == "+" && expecting_operand {
                // The VM accepts unary '+'. The syntax tree accepts '~' in the same position.
                // The fold reads the original sign before it evaluates the operand.
                bytes[span.start] = b'~';
            }
            if token.token().kind() == ScriptTokenTag::Separator {
                return Err(ReckonError::new(span, format!("use one expression here, without `{word}`")));
            }
            expecting_operand = matches!(token.token().kind(), ScriptTokenTag::OpenRound | ScriptTokenTag::Operator);
        }
    }
    check_gap(end..text.len())?;
    Ok(Cow::Owned(String::from_utf8(bytes).expect("only whole characters were replaced with ASCII")))
}

fn parse_error(text: &str, err: SyntaxError) -> ReckonError {
    let before = text[..err.at].trim_end();
    if err.at == text.len() || text[err.at..].starts_with(')') {
        if let Some(op @ ('+' | '-' | '*' | '/')) = before.chars().next_back() {
            return ReckonError::new(before.len() - 1..before.len(), format!("nothing after `{op}`"));
        }
    }
    let tail = &text[err.at..];
    let len = if tail.starts_with(identifier_char) {
        tail.find(|c| !identifier_char(c)).unwrap_or(tail.len())
    } else {
        tail.chars().next().map_or(0, char::len_utf8)
    };
    let word = &tail[..len];
    let message = if err.message == "more text follows the value" {
        format!("an operator is missing before `{word}`")
    } else {
        err.message.trim_start_matches("this ").to_owned()
    };
    ReckonError::new(err.at..err.at + len, message)
}

// Round in decimal, including at exponents where scaling the double first would overflow.
fn digits(value: f64) -> String {
    let rounded = format!("{value:.11e}").parse::<f64>().unwrap();
    let rounded = if rounded.is_finite() { rounded } else { value };
    let decimal = rounded.to_string();
    let exponent = format!("{rounded:e}");
    if exponent.len() < decimal.len() { exponent } else { decimal }
}

#[derive(Clone, Copy, PartialEq)]
enum Form { Literal, Name, Paren, Expression }

struct Quantity {
    value: f64,
    dim: i32,
    flexible: bool,
    named: bool,
    plain: bool,
    form: Form,
    span: Range<usize>,
}

struct Fold<'a> {
    text: &'a str,
    reading: &'a Reading,
    units: &'a UnitTable,
    names: &'a [NamedValue],
    used: Vec<String>,
    edits: Vec<(Range<usize>, String)>,
}

impl Fold<'_> {
    fn fault(&self, span: Range<usize>, why: &str) -> ReckonError {
        let text = &self.text[span.clone()];
        ReckonError::new(span, format!("`{text}` {why}"))
    }

    fn check_measure(&self, measure: Measure, span: &Range<usize>) -> Result<(), ReckonError> {
        if measure.physical() && measure != self.reading.measure {
            let why = format!("is {}; this box takes {}", measure.description(), self.reading.measure.description());
            return Err(self.fault(span.clone(), &why));
        }
        Ok(())
    }

    fn number(&mut self, node: &Node, value: f64) -> Result<Quantity, ReckonError> {
        let literal = node.text(self.text);
        let unit = suffix(literal, self.units).map_or("", |u| u.suffix.as_str());
        let measure = Measure::of(unit);
        self.check_measure(measure, &node.span)?;
        let raw = literal[..literal.len() - unit.len()].trim_end();
        // The tokenizer emits zero for unfinished decimals such as '1e'. A preview must
        // distinguish those from an actual zero. Numeric type suffixes also denote numbers.
        if value == 0.0 && raw.trim_end_matches(['f', 'u', 'i', 'h']).replace('_', "").parse::<f64>().is_err() {
            return Err(self.fault(node.span.clone(), "isn't a complete number"));
        }
        let edit = match unit {
            "" => None,
            "%" => Some(digits(value)),
            "in" | "ft" => {
                let (factor, suffix) = self.reading.script_unit();
                Some(format!("{}{suffix}", digits(value * factor)))
            }
            _ if measure == Measure::Plain && unit == self.reading.unit => Some(raw.to_owned()),
            _ if measure == Measure::Plain => return Err(self.fault(node.span.clone(), "isn't in a unit this box takes")),
            _ if raw.len() + unit.len() != literal.len() => Some(format!("{raw}{unit}")),
            _ => None,
        };
        if let Some(edit) = edit {
            self.edits.push((node.span.clone(), edit));
        }
        Ok(Quantity {
            value, dim: i32::from(measure.physical()), flexible: measure == Measure::Plain,
            named: false, plain: unit.is_empty() || unit == self.reading.unit,
            form: Form::Literal, span: node.span.clone(),
        })
    }

    fn unknown(&self, name: &str, span: Range<usize>) -> ReckonError {
        if self.names.is_empty() {
            return self.fault(span, "is not a number");
        }
        let mut message = format!("no parameter `{name}`");
        if let Some(item) = self.names.iter().find(|n| n.label.to_lowercase().replace(' ', "_") == name) {
            message.push_str(&format!("; did you mean `{}` ({})?", item.name, item.label));
        } else if let Some((_, item)) = self.names.iter().map(|n| (levenshtein(name, &n.name), n))
            .filter(|(distance, _)| *distance <= 2).min_by_key(|(distance, _)| *distance)
        {
            message.push_str(&format!("; did you mean `{}`?", item.name));
        }
        ReckonError::new(span, message)
    }

    fn node(&mut self, node: &Node) -> Result<Quantity, ReckonError> {
        match &node.kind {
            Kind::Number(value) => self.number(node, *value),
            Kind::Ident(name) if !name.contains('.') && name.starts_with(|c: char| c.is_alphabetic() || matches!(c, '_' | '$')) => {
                let item = self.names.iter().find(|n| n.name == *name)
                    .ok_or_else(|| self.unknown(name, node.span.clone()))?;
                self.check_measure(item.measure, &node.span)?;
                if !self.used.contains(name) { self.used.push(name.clone()); }
                let physical = item.measure.physical();
                let value = if physical { item.value / self.reading.scale } else { item.value };
                Ok(Quantity {
                    value, dim: i32::from(physical), flexible: item.measure == Measure::Plain,
                    named: true, plain: false, form: Form::Name, span: node.span.clone(),
                })
            }
            Kind::Unary { op, operand } if op == "-" || node.text(self.text).starts_with('+') => {
                let mut q = self.node(operand)?;
                if op == "-" { q.value = -q.value; }
                q.plain &= op == "-" && q.form == Form::Literal && !operand.text(self.text).starts_with('-');
                if q.form != Form::Literal { q.form = Form::Expression; }
                q.span = node.span.clone();
                Ok(q)
            }
            Kind::Paren { items, .. } if items.len() == 1 => {
                let mut q = self.node(&items[0])?;
                q.plain = false;
                q.form = Form::Paren;
                q.span = node.span.clone();
                Ok(q)
            }
            Kind::Chain { .. } => match node.grouped() {
                Grouped::Operand(_) => Err(self.fault(node.span.clone(), "has an unsupported operator")),
                group => self.group(&group),
            },
            Kind::Call { .. } => Err(self.fault(node.span.clone(), "is a call; use arithmetic here")),
            Kind::Paren { .. } => Err(self.fault(node.span.clone(), "must hold one value")),
            _ => Err(self.fault(node.span.clone(), "isn't a number or arithmetic")),
        }
    }

    // Give a flexible subtree the unit required by its sum or by the whole reading.
    // A flexible physical/percentage subtree contains only bare numbers and plain names,
    // so replacing its span cannot cover a unit edit made in a child.
    fn promote(&mut self, q: &mut Quantity, dim: i32) {
        q.dim = dim;
        q.flexible = false;
        if dim == 0 && self.reading.measure != Measure::Fraction { return; }
        let (factor, unit) = self.reading.script_unit();
        let text = &self.text[q.span.clone()];
        let replacement = if q.named {
            let multiplier = digits(factor);
            if matches!(q.form, Form::Name | Form::Paren) {
                format!("{text} * {multiplier}{unit}")
            } else {
                format!("({text}) * {multiplier}{unit}")
            }
        } else if q.form == Form::Literal && factor == 1.0 && !text.ends_with(['f', 'u', 'i', 'h']) {
            format!("{text}{unit}")
        } else {
            format!("{}{unit}", digits(q.value * factor))
        };
        self.edits.push((q.span.clone(), replacement));
        if dim == 0 { q.value *= factor; }
    }

    fn dimension(&self, dim: i32) -> String {
        match dim {
            0 => "a plain number".into(),
            1 => self.reading.measure.description().into(),
            2 if self.reading.measure == Measure::Length => "an area".into(),
            _ => format!("{} to the power {dim}", self.reading.measure.description()),
        }
    }

    fn group(&mut self, group: &Grouped<'_>) -> Result<Quantity, ReckonError> {
        let (op, op_span, lhs, rhs) = match group {
            Grouped::Operand(node) => return self.node(node),
            Grouped::Binary { op, op_span, lhs, rhs } => (*op, op_span, lhs, rhs),
        };
        if !matches!(op, "+" | "-" | "*" | "/") {
            return Err(self.fault(op_span.clone(), "isn't supported here; use +, -, * or /"));
        }
        let mut left = self.group(lhs)?;
        let mut right = self.group(rhs)?;
        let span = left.span.start..right.span.end;
        let (value, dim) = match op {
            "+" | "-" => {
                if left.flexible && !right.flexible && matches!(right.dim, 0 | 1) {
                    self.promote(&mut left, right.dim);
                } else if right.flexible && !left.flexible && matches!(left.dim, 0 | 1) {
                    self.promote(&mut right, left.dim);
                }
                if left.dim != right.dim {
                    let operation = if op == "+" { "add" } else { "subtract" };
                    let why = format!("can't {operation} {} and {}", self.dimension(left.dim), self.dimension(right.dim));
                    return Err(self.fault(span, &why));
                }
                (if op == "+" { left.value + right.value } else { left.value - right.value }, left.dim)
            }
            "*" => (left.value * right.value, left.dim + right.dim),
            "/" if right.value == 0.0 => return Err(self.fault(span, "divides by zero")),
            "/" => (left.value / right.value, left.dim - right.dim),
            _ => unreachable!(),
        };
        Ok(Quantity {
            value, dim, flexible: left.flexible && right.flexible, named: left.named || right.named,
            plain: false, form: Form::Expression, span,
        })
    }
}

/// Read one value without running script code. Names come only from the supplied list.
pub fn evaluate(text: &str, reading: &Reading, names: &[NamedValue]) -> Result<Reckoned, ReckonError> {
    if text.trim().is_empty() {
        return Err(ReckonError::new(0..text.len(), "type a number"));
    }
    let units = reading.units();
    let prepared = prepare(text, reading, &units)?;
    let node = parse_value_with(&prepared, &units).map_err(|err| parse_error(text, err))?;
    let mut fold = Fold { text, reading, units: &units, names, used: Vec::new(), edits: Vec::new() };
    let mut q = fold.node(&node)?;
    let value = match q.dim {
        1 => q.value,
        0 if q.flexible => {
            let raw = q.value;
            if reading.measure != Measure::Plain {
                fold.promote(&mut q, i32::from(reading.measure.physical()));
            }
            raw
        }
        0 if reading.measure == Measure::Plain => q.value,
        0 if reading.measure == Measure::Fraction => q.value * reading.scale.recip(),
        dim => {
            let got = if dim == 0 { "a plain ratio".into() } else { fold.dimension(dim) };
            return Err(fold.fault(q.span, &format!("is {got}, not {}", reading.measure.description())));
        }
    };
    if !value.is_finite() {
        return Err(fold.fault(q.span, "is too large to hold"));
    }
    // A promotion on the left of a sum can be decided after edits on its right.
    fold.edits.sort_by_key(|(span, _)| span.start);
    let mut script = String::with_capacity(text.len());
    let mut end = 0;
    for (span, replacement) in fold.edits {
        script.push_str(&text[end..span.start]);
        script.push_str(&replacement);
        end = span.end;
    }
    script.push_str(&text[end..]);
    Ok(Reckoned { value, plain: q.plain, names: fold.used, script })
}
