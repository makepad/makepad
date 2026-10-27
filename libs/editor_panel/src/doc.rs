//! The document a panel edits, as the panel sees it: files of Splash code,
//! named paths into them, and the few things the panel asks of the code it
//! edits (where a `let` is, which colours it writes, how a value is written
//! back in the form of the literal there, a keyed value's key at the
//! playhead). A host installs one [`EditorDoc`] ([`crate::register`]); the
//! panel keeps the gestures, the held values and the splices, the document
//! kind keeps what its code means.
//!
//! An outline of a file (what the host's document kind makes of its text:
//! regions, functions) and the table of several documents ([`crate::set_docs`])
//! are the document kind's own: the panel holds them as `dyn Any` and hands
//! them back to the [`EditorDoc`].

use std::any::Any;
use std::ops::Range;

/// A file a path can reach: its name as the document imports it (`""` for
/// the document itself), its text and its outline (the document kind's own).
#[derive(Clone, Copy)]
pub struct File<'a> {
    pub name: &'a str,
    pub text: &'a str,
    pub code: &'a dyn Any,
}

/// What a value is, as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Number,
    /// A number with a time unit (`0.12s`, `120ms`).
    Time,
    /// A number with an angle unit (`90deg`, `1.2rad`).
    Angle,
    /// A number in virtual pixels (`12px`).
    Length,
    Colour,
    Font,
    Text,
    Ease,
    /// `@name`.
    Choice,
    Bool,
    /// A name of a function (a slot: another of the same parameters fits).
    Function,
    Record,
    List,
    /// Anything else: code (a call, arithmetic), replaced as code.
    Expression,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Number => "number",
            Kind::Time => "time",
            Kind::Angle => "angle",
            Kind::Length => "length",
            Kind::Colour => "colour",
            Kind::Font => "font",
            Kind::Text => "text",
            Kind::Ease => "ease",
            Kind::Choice => "choice",
            Kind::Bool => "bool",
            Kind::Function => "function",
            Kind::Record => "record",
            Kind::List => "list",
            Kind::Expression => "expression",
        }
    }
}

/// Where a path's value is written.
#[derive(Clone, Debug, PartialEq)]
pub struct Site {
    /// Index into the files given.
    pub file: usize,
    pub range: Range<usize>,
    pub kind: Kind,
    pub text: String,
}

/// One place a path reaches, and how a set moves it: a number set by `d`
/// from where the gesture began moves this one by `scale * d`.
#[derive(Clone, Debug, PartialEq)]
pub struct Bound {
    pub site: Site,
    pub scale: f64,
    /// The member of a set this place belongs to (several places of one
    /// member move together).
    pub member: String,
}

/// A bindable path of a document.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    pub kind: Kind,
    pub value: String,
    pub label: Option<String>,
}

/// A path of a set of documents worth binding.
#[derive(Clone, Debug, PartialEq)]
pub struct DocEntry {
    pub path: String,
    pub kind: Kind,
    pub value: String,
    pub label: Option<String>,
    /// How many places it reaches.
    pub reach: usize,
}

/// A value to write.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Number(f64),
    Text(String),
    Bool(bool),
    /// Code as written (a name for a function slot, `@choice`, an
    /// expression).
    Code(String),
}

/// A colour a file writes, and every place it is written.
#[derive(Clone, Debug, PartialEq)]
pub struct Swatch {
    pub rgba: [f64; 4],
    /// The name a tag gives it, when it has one.
    pub name: Option<String>,
    pub sites: Vec<Range<usize>>,
}

/// What the panel asks of the document it edits.
pub trait EditorDoc {
    // ------------------------------------------------ the code's outline

    /// The outline of a text (what [`File::code`] holds for it).
    fn outline(&self, text: &str) -> Box<dyn Any>;
    /// The bytes of the top-level `let <name>`.
    fn let_region(&self, text: &str, code: &dyn Any, name: &str) -> Option<Range<usize>>;
    /// The top-level `let` whose whole value is the literal at `site` (a
    /// `/**tag*/` before it allowed): its name.
    fn let_of_literal(&self, text: &str, code: &dyn Any, site: &Range<usize>) -> Option<String>;
    /// The functions that could stand where the function a path names is:
    /// the same parameters, in the document or a module it imports.
    fn fns_like(&self, files: &[File], path: &str) -> Vec<String>;

    // ------------------------------------------------ paths

    /// The place a path names in one document, or why none (naming the
    /// nearest paths that exist).
    fn resolve(&self, files: &[File], path: &str) -> Result<Site, String>;
    /// [`resolve`](Self::resolve) without the nearest paths looked for.
    fn resolve_exact(&self, files: &[File], path: &str) -> Result<Site, String>;
    /// Every place a path reaches over several documents (`table`: the
    /// document kind's own, given to [`crate::set_docs`]).
    fn resolve_in(&self, files: &[File], table: &dyn Any, path: &str) -> Result<Vec<Bound>, String>;
    /// Every path of a document worth binding.
    fn list(&self, files: &[File]) -> Vec<Entry>;
    /// Every path of several documents worth binding.
    fn list_in(&self, files: &[File], table: &dyn Any) -> Vec<DocEntry>;
    /// Where a value set at `path`, moved from `from` to `to`, is written
    /// (a clip's time on the nearest beat), anything else as given.
    fn snap(&self, table: &dyn Any, path: &str, from: f64, to: f64) -> f64;
    /// Whether a value set at `path` lands on a grid ([`snap`](Self::snap)).
    fn snaps(&self, path: &str) -> bool;

    // ------------------------------------------------ colours

    /// The colours a text writes.
    fn palette(&self, text: &str) -> Vec<Swatch>;
    /// Each colour literal of a text, where and what.
    fn palette_sites(&self, text: &str) -> Vec<(Range<usize>, [f64; 4])>;
    /// The text with colours moved at every place each is written (old, new).
    fn recolour(&self, text: &str, map: &[([f64; 4], [f64; 4])]) -> String;
    /// [`recolour`](Self::recolour) at sites read earlier (the text as a
    /// gesture began).
    fn recolour_at(&self, text: &str, sites: &[(Range<usize>, [f64; 4])], map: &[([f64; 4], [f64; 4])]) -> String;

    // ------------------------------------------------ keyed values

    /// A keyed value (`keys(…)`) at `range`, sampled at time `at`; `None`
    /// when its keys do not read.
    fn keys_sample(&self, text: &str, range: Range<usize>, at: f64) -> Option<f64>;
    /// A number set on a keyed value at `range`: that key's value (a key
    /// within `half` of `at`), or a new key at `at`; as the bytes to replace
    /// and what they become. `None` when its keys do not read.
    fn keys_set(&self, text: &str, range: Range<usize>, at: f64, half: f64, value: f64) -> Option<(Range<usize>, String)>;

    // ------------------------------------------------ writing a value

    /// Where a write of `value` at `path` would go and what it writes (the
    /// site's file bytes and the literal), without the read-back check.
    fn place(&self, files: &[File], path: &str, value: &Value) -> Result<(Range<usize>, String), String>;
    /// The write of `value` at `path`, checked to read back as written.
    fn write(&self, files: &[File], path: &str, value: &Value) -> Result<(Range<usize>, String), String>;
    /// The one literal token a range holds (a `-` before a number with it).
    fn literal_token(&self, text: &str, range: &Range<usize>) -> Option<Range<usize>>;
    /// The value as code in the form of the literal at `site`.
    fn form(&self, site: &Site, value: &Value) -> Result<String, String>;
}

/// A number as the editor writes it: at most four decimals, none trailing.
pub fn fmt_num(value: f64) -> String {
    if !value.is_finite() {
        return "0".into();
    }
    makepad_value_pickers::panel::host::fmt_number((value * 10_000.0).round() / 10_000.0, 4)
}

/// Text as a string literal of the code.
pub fn fmt_text(text: &str) -> String {
    makepad_widgets::makepad_script::syntax::write::quote(text)
}
