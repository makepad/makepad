//! The document a panel edits, as the panel sees it: files of Splash code,
//! its values ([`ValueSource`]: named paths into the files, resolved again
//! on every read and written back as exact splices), and the few things a
//! panel asks of the code beyond its values ([`PanelDoc`]: where a `let`
//! is, which colours it writes, a keyed value's key at the playhead). A
//! host installs one [`PanelDoc`] ([`crate::register`]); the panel keeps
//! the gestures, the held values and the splices, the document kind keeps
//! what its code means.
//!
//! An outline of a file (what the host's document kind makes of its text:
//! regions, functions) and the table of several documents ([`crate::set_docs`])
//! are the document kind's own: the panel holds them as `dyn Any` and hands
//! them back to the [`PanelDoc`].

pub use makepad_edit_values::{Value, ValueKind, ValueSite, ValueSource};
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

/// One place a path reaches, and how a set moves it: a number set by `d`
/// from where the gesture began moves this one by `scale * d`.
#[derive(Clone, Debug, PartialEq)]
pub struct Bound {
    pub site: ValueSite,
    pub scale: f64,
    /// The member of a set this place belongs to (several places of one
    /// member move together).
    pub member: String,
}

/// A colour a file writes, and every place it is written.
#[derive(Clone, Debug, PartialEq)]
pub struct Swatch {
    pub rgba: [f64; 4],
    /// The name a tag gives it, when it has one.
    pub name: Option<String>,
    pub sites: Vec<Range<usize>>,
}

/// A document kind as the panels edit it: its values over the files given
/// ([`PanelDoc::values`]), and what a panel asks of its code beyond them.
/// Everything but the values and the outline has a default for a document
/// that has no such thing (one file, no colours, no keys, no sets).
pub trait PanelDoc {
    // ------------------------------------------------ the code's outline

    /// The outline of a text (what [`File::code`] holds for it).
    fn outline(&self, text: &str) -> Box<dyn Any>;
    /// The bytes of the top-level `let <name>`.
    fn let_region(&self, text: &str, code: &dyn Any, name: &str) -> Option<Range<usize>>;
    /// The top-level `let` whose whole value is the literal at `site` (a
    /// `/**tag*/` before it allowed): its name.
    fn let_of_literal(&self, _text: &str, _code: &dyn Any, _site: &Range<usize>) -> Option<String> {
        None
    }
    /// The functions that could stand where the function a path names is:
    /// the same parameters, in the document or a module it imports.
    fn fns_like(&self, _files: &[File], _path: &str) -> Vec<String> {
        Vec::new()
    }

    // ------------------------------------------------ values

    /// The values of `files` (ids are the paths a panel binds; a site's
    /// span names its file by index into `files`).
    fn values<'a>(&'a self, files: &'a [File<'a>]) -> Box<dyn ValueSource + 'a>;
    /// Every place a path reaches over several documents (`table`: the
    /// document kind's own, given to [`crate::set_docs`]).
    fn resolve_in(&self, _files: &[File], _table: &dyn Any, path: &str) -> Result<Vec<Bound>, String> {
        Err(format!("`{path}`: this document kind has no sets of documents"))
    }
    /// Every value of several documents worth binding, with how many places
    /// each reaches.
    fn list_in(&self, _files: &[File], _table: &dyn Any) -> Vec<(ValueSite, usize)> {
        Vec::new()
    }
    /// Where a value set at `path`, moved from `from` to `to`, is written
    /// (a clip's time on the nearest beat), anything else as given.
    fn snap(&self, _table: &dyn Any, _path: &str, _from: f64, to: f64) -> f64 {
        to
    }
    /// Whether a value set at `path` lands on a grid ([`snap`](Self::snap)).
    fn snaps(&self, _path: &str) -> bool {
        false
    }
    /// Where a write of `value` at `path` would go and what it writes (the
    /// site's bytes and the literal), without the write's read-back check.
    fn place(&self, files: &[File], path: &str, value: &Value) -> Result<(Range<usize>, String), String> {
        let splices = self.values(files).write(path, value)?;
        splices.into_iter().next().map(|s| (s.range, s.text)).ok_or_else(|| format!("`{path}`: nothing to write"))
    }
    /// The one literal token a range holds (a `-` before a number with it).
    fn literal_token(&self, _text: &str, range: &Range<usize>) -> Option<Range<usize>> {
        Some(range.clone())
    }
    /// The value as code in the form of the literal at `site`.
    fn form(&self, site: &ValueSite, value: &Value) -> Result<String, String> {
        match value {
            Value::Num(n) => Ok(fmt_num(*n)),
            Value::Bool(b) => Ok(b.to_string()),
            Value::Text(t) => Ok(fmt_text(t)),
            Value::Code(c) => Ok(c.clone()),
            _ => Err(format!("`{}` cannot take that value", site.id)),
        }
    }

    // ------------------------------------------------ colours

    /// The colours a text writes.
    fn palette(&self, _text: &str) -> Vec<Swatch> {
        Vec::new()
    }
    /// Each colour literal of a text, where and what.
    fn palette_sites(&self, _text: &str) -> Vec<(Range<usize>, [f64; 4])> {
        Vec::new()
    }
    /// The text with colours moved at every place each is written (old, new).
    fn recolour(&self, text: &str, _map: &[([f64; 4], [f64; 4])]) -> String {
        text.to_string()
    }
    /// [`recolour`](Self::recolour) at sites read earlier (the text as a
    /// gesture began).
    fn recolour_at(&self, text: &str, _sites: &[(Range<usize>, [f64; 4])], _map: &[([f64; 4], [f64; 4])]) -> String {
        text.to_string()
    }

    // ------------------------------------------------ keyed values

    /// A keyed value (`keys(…)`) at `range`, sampled at time `at`; `None`
    /// when its keys do not read.
    fn keys_sample(&self, _text: &str, _range: Range<usize>, _at: f64) -> Option<f64> {
        None
    }
    /// A number set on a keyed value at `range`: that key's value (a key
    /// within `half` of `at`), or a new key at `at`; as the bytes to replace
    /// and what they become. `None` when its keys do not read.
    fn keys_set(&self, _text: &str, _range: Range<usize>, _at: f64, _half: f64, _value: f64) -> Option<(Range<usize>, String)> {
        None
    }
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
