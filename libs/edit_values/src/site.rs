//! Value sites: what a tunable value is and how a document lists them.

use crate::anchor::Anchor;
use crate::edit::Splice;

/// A site's stable id: a path its document resolves (`acid.cutoff`,
/// `Motion.layers[2].post[0].strength`, `vase_height`, `body:size[1]`).
pub type SiteId = String;

/// What a value measures; it picks the control and how a typed entry parses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ValueKind {
    #[default]
    Number,
    Length,
    Radius,
    Diameter,
    Angle,
    Time,
    Frequency,
    Gain,
    /// A factor, shown in percent.
    Ratio,
    Count,
    Bool,
    Choice,
    Colour,
    Ease,
    /// A position of `dims` coordinates.
    Point,
    /// A list of 2D points (a section, a revolve profile).
    Profile,
    Text,
    Font,
    Media,
    /// A name of a function (a slot: another of the same parameters fits).
    Function,
    /// A record of fields.
    Record,
    /// A list of values.
    List,
    /// Code: shown, edited as text only.
    Code,
}

impl ValueKind {
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            ValueKind::Number | ValueKind::Length | ValueKind::Radius | ValueKind::Diameter | ValueKind::Angle | ValueKind::Time | ValueKind::Frequency | ValueKind::Gain | ValueKind::Ratio | ValueKind::Count
        )
    }
    pub fn is_length(self) -> bool {
        matches!(self, ValueKind::Length | ValueKind::Radius | ValueKind::Diameter)
    }
    /// The name a program or an AI uses for the kind.
    pub fn name(self) -> &'static str {
        match self {
            ValueKind::Number => "number",
            ValueKind::Length => "length",
            ValueKind::Radius => "radius",
            ValueKind::Diameter => "diameter",
            ValueKind::Angle => "angle",
            ValueKind::Time => "time",
            ValueKind::Frequency => "frequency",
            ValueKind::Gain => "gain",
            ValueKind::Ratio => "ratio",
            ValueKind::Count => "count",
            ValueKind::Bool => "bool",
            ValueKind::Choice => "choice",
            ValueKind::Colour => "colour",
            ValueKind::Ease => "ease",
            ValueKind::Point => "point",
            ValueKind::Profile => "profile",
            ValueKind::Text => "text",
            ValueKind::Font => "font",
            ValueKind::Media => "media",
            ValueKind::Function => "function",
            ValueKind::Record => "record",
            ValueKind::List => "list",
            ValueKind::Code => "code",
        }
    }
    pub fn parse(name: &str) -> Option<ValueKind> {
        ALL_KINDS.iter().copied().find(|k| k.name() == name || (name == "color" && *k == ValueKind::Colour))
    }
}

const ALL_KINDS: [ValueKind; 23] = [
    ValueKind::Number,
    ValueKind::Length,
    ValueKind::Radius,
    ValueKind::Diameter,
    ValueKind::Angle,
    ValueKind::Time,
    ValueKind::Frequency,
    ValueKind::Gain,
    ValueKind::Ratio,
    ValueKind::Count,
    ValueKind::Bool,
    ValueKind::Choice,
    ValueKind::Colour,
    ValueKind::Ease,
    ValueKind::Point,
    ValueKind::Profile,
    ValueKind::Text,
    ValueKind::Font,
    ValueKind::Media,
    ValueKind::Function,
    ValueKind::Record,
    ValueKind::List,
    ValueKind::Code,
];

/// A value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Num(f64),
    Bool(bool),
    /// Index into the site's choices.
    Choice(usize),
    Text(String),
    /// Linear RGBA.
    Colour([f32; 4]),
    Point(Vec<f64>),
    Profile(Vec<[f64; 2]>),
    /// Source text written as it is (an ease curve, an expression, a font name).
    Code(String),
}

impl Default for Value {
    fn default() -> Self {
        Value::Num(0.0)
    }
}

impl Value {
    pub fn num(&self) -> Option<f64> {
        match self {
            Value::Num(x) => Some(*x),
            _ => None,
        }
    }
}

/// How a value moves: `soft` sets a control's travel and grows when a value lands past it;
/// only `hard` limits clamp.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Range {
    pub soft_min: f64,
    pub soft_max: f64,
    pub hard_min: Option<f64>,
    pub hard_max: Option<f64>,
}

impl Range {
    pub fn soft(min: f64, max: f64) -> Range {
        Range { soft_min: min, soft_max: max, hard_min: None, hard_max: None }
    }
    pub fn hard(min: f64, max: f64) -> Range {
        Range { soft_min: min, soft_max: max, hard_min: Some(min), hard_max: Some(max) }
    }
    /// `x` within the hard limits.
    pub fn clamp(&self, x: f64) -> f64 {
        let x = self.hard_min.map_or(x, |m| x.max(m));
        self.hard_max.map_or(x, |m| x.min(m))
    }
}

/// How a control maps its travel onto the range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Taper {
    #[default]
    Linear,
    Log,
    /// Whole numbers.
    Int,
}

/// How a held (dragged) value can show before it is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Live {
    /// Patched into the running program, no write and no reload.
    Patch,
    /// Evaluated as a coalesced preview (only the newest held value is ever evaluated).
    #[default]
    Preview,
    /// Only a written value shows (a full reload).
    Reload,
}

/// One tunable value of a document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ValueSite {
    pub id: SiteId,
    pub label: String,
    /// The panel group (a module, an object, "Parameters").
    pub group: String,
    pub kind: ValueKind,
    /// `mm`, `deg`, `s`, `ms`, `hz`, `db`, `st`, `ct`, `%`, `px` or empty.
    pub unit: String,
    pub range: Range,
    /// One arrow click; 0 = derived from the range.
    pub step: f64,
    pub taper: Taper,
    /// Choice labels (a `Choice` value indexes them).
    pub choices: Vec<String>,
    pub default: Option<Value>,
    pub value: Value,
    /// The value as written in the source (empty when not written).
    pub text: String,
    /// Tuned rarely (mesh detail, segments): listed last.
    pub detail: bool,
    /// Worked out from other values: shown, not edited.
    pub derived: bool,
    /// One line on what it does.
    pub about: String,
    /// Where it is written now: file index and byte range (`None`: not written yet).
    pub span: Option<(usize, std::ops::Range<usize>)>,
    pub live: Live,
    /// Its handle in 3D, where the document has geometry.
    pub anchor: Option<Anchor>,
    /// Other ids this value shapes or belongs to (objects, nodes), for highlighting.
    pub shapes: Vec<String>,
}

impl ValueSite {
    /// The file it is written in (0 when not written).
    pub fn file(&self) -> usize {
        self.span.as_ref().map_or(0, |s| s.0)
    }
    /// Its bytes there (empty when not written).
    pub fn range(&self) -> std::ops::Range<usize> {
        self.span.as_ref().map_or(0..0, |s| s.1.clone())
    }
    /// Whether its kind is a number with a unit or none.
    pub fn is_numeric(&self) -> bool {
        self.kind.is_numeric()
    }
}

/// A document's editable values.
pub trait ValueSource {
    /// Every site, panel order.
    fn sites(&self) -> Vec<ValueSite>;
    /// One site by id, resolved again against the document as it is now.
    fn resolve(&self, id: &str) -> Result<ValueSite, String>;
    /// The splices that write `value` at `id` (exact bytes, formatted like the source; an
    /// unwritten field inserted in its node's layout). Documents that are not text return an
    /// error and take [`crate::Change::Set`] in their [`crate::EditSink`] instead.
    fn write(&self, id: &str, value: &Value) -> Result<Vec<Splice>, String>;
    /// Ids close to one that no longer resolves (a panel offers them).
    fn nearest(&self, id: &str) -> Vec<String> {
        let want = id.rsplit(['.', ':']).next().unwrap_or(id);
        self.sites().into_iter().map(|s| s.id).filter(|s| s.ends_with(want) && s != id).take(5).collect()
    }
}
